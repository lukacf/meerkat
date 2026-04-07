//! RuntimeLoop — per-session tokio task that processes queued inputs.
//!
//! When `RuntimeSessionAdapter::accept_input()` queues an input and sets
//! the wake flag, it sends a signal on the wake channel. The RuntimeLoop
//! picks it up, dequeues the input, converts it to a `RunPrimitive`,
//! and applies it via the `CoreExecutor` (which calls `SessionService::start_turn()`
//! under the hood).

use meerkat_core::lifecycle::core_executor::CoreApplyTerminal;
use meerkat_core::lifecycle::run_primitive::{
    ConversationAppend, ConversationAppendRole, CoreRenderable, RunApplyBoundary, RunPrimitive,
    StagedRunInput,
};
use meerkat_core::lifecycle::{InputId, RunEvent, RunId};

use crate::input::Input;
use crate::tokio;

/// Extract a prompt string from an `Input`.
pub(crate) fn input_to_prompt(input: &Input) -> String {
    match input {
        Input::Prompt(p) => p.text.clone(),
        Input::Peer(p) => p.body.clone(),
        Input::FlowStep(f) => f.instructions.clone(),
        Input::ExternalEvent(e) => external_event_projection_text(e),
        Input::Continuation(c) => format!("[Continuation] {}", c.reason),
        Input::Operation(operation) => {
            format!(
                "[Operation {}] {:?}",
                operation.operation_id, operation.event
            )
        }
    }
}

fn input_boundary(input: &Input) -> RunApplyBoundary {
    match input {
        Input::Peer(peer)
            if matches!(
                peer.convention,
                Some(crate::input::PeerConvention::ResponseProgress { .. })
            ) =>
        {
            RunApplyBoundary::RunCheckpoint
        }
        Input::Continuation(continuation) => match continuation.handling_mode {
            meerkat_core::types::HandlingMode::Queue => RunApplyBoundary::RunStart,
            meerkat_core::types::HandlingMode::Steer => RunApplyBoundary::RunCheckpoint,
        },
        Input::Prompt(prompt) => {
            match prompt.turn_metadata.as_ref().and_then(|m| m.handling_mode) {
                Some(meerkat_core::types::HandlingMode::Steer) => RunApplyBoundary::RunCheckpoint,
                _ => RunApplyBoundary::RunStart,
            }
        }
        Input::FlowStep(flow_step) => {
            match flow_step
                .turn_metadata
                .as_ref()
                .and_then(|m| m.handling_mode)
            {
                Some(meerkat_core::types::HandlingMode::Steer) => RunApplyBoundary::RunCheckpoint,
                _ => RunApplyBoundary::RunStart,
            }
        }
        Input::ExternalEvent(event) => match event.handling_mode {
            meerkat_core::types::HandlingMode::Steer => RunApplyBoundary::RunCheckpoint,
            meerkat_core::types::HandlingMode::Queue => RunApplyBoundary::RunStart,
        },
        _ => RunApplyBoundary::RunStart,
    }
}

fn input_turn_metadata(
    input: &Input,
) -> Option<meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata> {
    match input {
        Input::Prompt(prompt) => prompt.turn_metadata.clone(),
        Input::FlowStep(flow_step) => flow_step.turn_metadata.clone(),
        Input::ExternalEvent(event) => Some(
            meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                handling_mode: Some(event.handling_mode),
                render_metadata: event.render_metadata.clone(),
                ..Default::default()
            },
        ),
        Input::Continuation(continuation) => Some(
            meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                handling_mode: Some(continuation.handling_mode),
                ..Default::default()
            },
        ),
        _ => None,
    }
}

fn resolve_completion_waiters(
    registry: &mut crate::completion::CompletionRegistry,
    input_ids: &[InputId],
    run_result: Option<meerkat_core::types::RunResult>,
    terminal: Option<CoreApplyTerminal>,
) {
    match terminal {
        Some(CoreApplyTerminal::CallbackPending { tool_name, args }) => {
            for input_id in input_ids {
                registry.resolve_callback_pending(input_id, tool_name.clone(), args.clone());
            }
        }
        Some(CoreApplyTerminal::RunResult(_)) => match run_result {
            Some(result) => {
                for input_id in input_ids {
                    registry.resolve_completed(input_id, result.clone());
                }
            }
            None => {
                for input_id in input_ids {
                    registry.resolve_without_result(input_id);
                }
            }
        },
        None => {
            for input_id in input_ids {
                registry.resolve_without_result(input_id);
            }
        }
    }
}

/// Merge turn metadata from all inputs in a batch.
///
/// Later inputs override scalar fields (last writer wins); collection
/// fields (skill_references, additional_instructions) accumulate.
fn merge_batch_turn_metadata(
    inputs: &[(InputId, Input)],
) -> Option<meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata> {
    use meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata;

    let mut merged: Option<RuntimeTurnMetadata> = None;
    for (_, input) in inputs {
        let Some(meta) = input_turn_metadata(input) else {
            continue;
        };
        let m = merged.get_or_insert_with(RuntimeTurnMetadata::default);
        if meta.handling_mode.is_some() {
            m.handling_mode = meta.handling_mode;
        }
        if meta.keep_alive.is_some() {
            m.keep_alive = meta.keep_alive;
        }
        if meta.model.is_some() {
            m.model = meta.model;
        }
        if meta.provider.is_some() {
            m.provider = meta.provider;
        }
        if meta.provider_params.is_some() {
            m.provider_params = meta.provider_params;
        }
        if meta.render_metadata.is_some() {
            m.render_metadata = meta.render_metadata;
        }
        if meta.flow_tool_overlay.is_some() {
            m.flow_tool_overlay = meta.flow_tool_overlay;
        }
        // Accumulate collection fields.
        if let Some(refs) = meta.skill_references {
            m.skill_references.get_or_insert_with(Vec::new).extend(refs);
        }
        if let Some(instrs) = meta.additional_instructions {
            m.additional_instructions
                .get_or_insert_with(Vec::new)
                .extend(instrs);
        }
    }
    merged
}

fn input_to_append(input: &Input) -> Option<ConversationAppend> {
    let content = match input {
        Input::Prompt(p) if p.blocks.is_some() => CoreRenderable::Blocks {
            blocks: p.blocks.clone().unwrap_or_default(),
        },
        Input::Peer(p) if p.blocks.is_some() => {
            let raw_blocks = p.blocks.clone().unwrap_or_default();
            if let Some(prefix) = peer_block_prefix_text(p) {
                let mut blocks = vec![meerkat_core::types::ContentBlock::Text { text: prefix }];
                blocks.extend(raw_blocks);
                CoreRenderable::Blocks { blocks }
            } else {
                // Legacy fallback for non-message or non-peer-identified inputs.
                let body_already_in_blocks = raw_blocks.first().is_some_and(|b| {
                    matches!(b, meerkat_core::types::ContentBlock::Text { text } if text == &p.body)
                });
                if p.body.is_empty() || body_already_in_blocks {
                    CoreRenderable::Blocks { blocks: raw_blocks }
                } else {
                    let mut blocks = vec![meerkat_core::types::ContentBlock::Text {
                        text: p.body.clone(),
                    }];
                    blocks.extend(raw_blocks);
                    CoreRenderable::Blocks { blocks }
                }
            }
        }
        Input::FlowStep(f) if f.blocks.is_some() => CoreRenderable::Blocks {
            blocks: f.blocks.clone().unwrap_or_default(),
        },
        Input::ExternalEvent(e) if e.blocks.is_some() => CoreRenderable::Blocks {
            blocks: e.blocks.clone().unwrap_or_default(),
        },
        Input::Prompt(_) | Input::Peer(_) | Input::FlowStep(_) | Input::ExternalEvent(_) => {
            CoreRenderable::Text {
                text: input_to_prompt(input),
            }
        }
        Input::Continuation(_) | Input::Operation(_) => return None,
    };

    Some(ConversationAppend {
        role: ConversationAppendRole::User,
        content,
    })
}

fn peer_block_prefix_text(peer: &crate::input::PeerInput) -> Option<String> {
    match (&peer.convention, &peer.header.source) {
        (
            Some(crate::input::PeerConvention::Message),
            crate::input::InputOrigin::Peer { peer_id, .. },
        ) => Some(format!("[COMMS MESSAGE from {peer_id}]")),
        _ => None,
    }
}

fn external_event_projection_text(event: &crate::input::ExternalEventInput) -> String {
    let source_name = match &event.header.source {
        crate::input::InputOrigin::External { source_name } if !source_name.trim().is_empty() => {
            source_name.as_str()
        }
        _ => event.event_type.as_str(),
    };
    let body = event
        .payload
        .get("body")
        .and_then(serde_json::Value::as_str)
        .map(str::trim);

    meerkat_core::interaction::format_external_event_projection(source_name, body)
}

pub(crate) fn inputs_to_primitive_with_boundary(
    inputs: &[(InputId, Input)],
    boundary: RunApplyBoundary,
) -> RunPrimitive {
    let appends = inputs
        .iter()
        .filter_map(|(_, input)| input_to_append(input))
        .collect::<Vec<_>>();
    let contributing_input_ids = inputs
        .iter()
        .map(|(input_id, _)| input_id.clone())
        .collect::<Vec<_>>();
    // Merge turn metadata from ALL inputs in the batch, not just the first.
    // Later inputs override scalar fields; collection fields accumulate.
    let turn_metadata = merge_batch_turn_metadata(inputs);

    // Derive typed execution intent from the full batch.
    // If ANY input is ContentTurn, the whole batch is ContentTurn (safe default).
    // Only a fully-homogeneous ResumePending batch becomes ResumePending.
    let execution_kind = if inputs.is_empty() {
        None
    } else if inputs.iter().all(|(_, input)| {
        crate::input::classify_execution_kind(input)
            == meerkat_core::lifecycle::RuntimeExecutionKind::ResumePending
    }) {
        Some(meerkat_core::lifecycle::RuntimeExecutionKind::ResumePending)
    } else {
        Some(meerkat_core::lifecycle::RuntimeExecutionKind::ContentTurn)
    };
    let turn_metadata = {
        let mut meta = turn_metadata.unwrap_or_default();
        meta.execution_kind = execution_kind;
        Some(meta)
    };

    RunPrimitive::StagedInput(StagedRunInput {
        boundary,
        appends,
        context_appends: vec![],
        contributing_input_ids,
        turn_metadata,
    })
}

pub(crate) fn inputs_to_primitive(inputs: &[(InputId, Input)]) -> RunPrimitive {
    let boundary = inputs
        .first()
        .map(|(_, input)| input_boundary(input))
        .unwrap_or(RunApplyBoundary::RunStart);
    inputs_to_primitive_with_boundary(inputs, boundary)
}

/// Convert an `Input` + its ID to a `RunPrimitive` for `CoreExecutor::apply()`.
pub(crate) fn input_to_primitive(input: &Input, input_id: InputId) -> RunPrimitive {
    inputs_to_primitive(&[(input_id, input.clone())])
}

/// Spawn the per-session runtime loop with optional completion registry.
#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_runtime_loop_with_completions(
    driver: crate::session_adapter::SharedDriver,
    mut executor: Box<dyn meerkat_core::lifecycle::CoreExecutor>,
    mut wake_rx: tokio::sync::mpsc::Receiver<()>,
    mut control_rx: tokio::sync::mpsc::Receiver<
        meerkat_core::lifecycle::run_control::RunControlCommand,
    >,
    completions: Option<crate::session_adapter::SharedCompletionRegistry>,
    detached_wake: Option<std::sync::Arc<crate::detached_wake::DetachedWakeState>>,
    completion_feed: Option<std::sync::Arc<dyn meerkat_core::completion_feed::CompletionFeed>>,
    epoch_cursor_state: Option<std::sync::Arc<meerkat_core::EpochCursorState>>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        // Feed-based idle wake state (local to this loop).
        // Seed from epoch cursor state if available (runtime-backed surfaces),
        // otherwise fall back to the feed watermark to avoid replaying historical
        // completions from a prior runtime loop (e.g., after stop/resume).
        let initial_watermark = epoch_cursor_state
            .as_ref()
            .map(|cs| {
                let obs = cs
                    .runtime_observed_seq
                    .load(std::sync::atomic::Ordering::Acquire);
                let inj = cs
                    .runtime_last_injected_seq
                    .load(std::sync::atomic::Ordering::Acquire);
                // Use the max of observed/injected as the initial watermark only
                // if non-zero (recovered state); otherwise fall back to feed.
                let recovered = obs.max(inj);
                if recovered > 0 {
                    recovered
                } else {
                    completion_feed.as_ref().map(|f| f.watermark()).unwrap_or(0)
                }
            })
            .unwrap_or_else(|| completion_feed.as_ref().map(|f| f.watermark()).unwrap_or(0));
        let mut observed_seq: meerkat_core::completion_feed::CompletionSeq = initial_watermark;
        let mut last_injected_seq: meerkat_core::completion_feed::CompletionSeq =
            epoch_cursor_state
                .as_ref()
                .map(|cs| {
                    cs.runtime_last_injected_seq
                        .load(std::sync::atomic::Ordering::Acquire)
                })
                .filter(|&v| v > 0)
                .unwrap_or(initial_watermark);

        loop {
            // Build a future for the idle wake. Feed-based if available,
            // otherwise falls back to DetachedWakeState Notify.
            let idle_wake = async {
                if let Some(ref feed) = completion_feed {
                    feed.wait_for_advance(observed_seq).await;
                } else if let Some(ref state) = detached_wake {
                    state.notify.notified().await;
                } else {
                    std::future::pending::<()>().await;
                }
            };

            tokio::select! {
                biased;
                maybe_command = control_rx.recv() => {
                    match maybe_command {
                        Some(command) => {
                            if crate::control_plane::apply_executor_control(
                                &driver,
                                completions.as_ref(),
                                &mut *executor,
                                command,
                            )
                            .await
                            {
                                break;
                            }
                        }
                        None => break,
                    }
                }
                maybe_wake = wake_rx.recv() => {
                    match maybe_wake {
                        Some(()) => {
                            if process_queue(
                                &driver,
                                &mut *executor,
                                &mut control_rx,
                                completions.as_ref(),
                            )
                            .await
                            {
                                break;
                            }
                            // Secondary wake path: re-check after queue drain.
                            // If a completion arrived during process_queue, inject
                            // and immediately process the continuation.
                            if maybe_inject_feed_wake(
                                &driver,
                                completion_feed.as_deref(),
                                detached_wake.as_ref(),
                                &mut observed_seq,
                                &mut last_injected_seq,
                                epoch_cursor_state.as_deref(),
                            )
                            .await
                                && process_queue(
                                    &driver,
                                    &mut *executor,
                                    &mut control_rx,
                                    completions.as_ref(),
                                )
                                .await
                            {
                                break;
                            }
                        }
                        None => break,
                    }
                }
                () = idle_wake => {
                    // A completion arrived while idle. Check if it's a
                    // BackgroundToolOp that hasn't been injected yet.
                    // Only BackgroundToolOp triggers idle wake — MobMemberChild
                    // completions already wake through comms terminal response.
                    if let Some(ref feed) = completion_feed {
                        let batch = feed.list_since(observed_seq);

                        let has_new_bg_completion = batch.entries.iter().any(|e| {
                            e.kind
                                == meerkat_core::ops_lifecycle::OperationKind::BackgroundToolOp
                                && e.seq > last_injected_seq
                        });

                        if has_new_bg_completion {
                            // Verify quiescence before injecting.
                            let d = driver.lock().await;
                            let quiescent = d.is_quiescent_for_detached_wake();
                            drop(d);

                            if quiescent {
                                let input = crate::input::Input::Continuation(
                                    crate::input::ContinuationInput::detached_background_op_completed(),
                                );
                                let mut d = driver.lock().await;
                                if d.as_driver_mut().accept_input(input).await.is_ok() {
                                    last_injected_seq = batch.watermark;
                                    if let Some(ref cs) = epoch_cursor_state {
                                        cs.runtime_last_injected_seq.store(batch.watermark, std::sync::atomic::Ordering::Release);
                                    }
                                }
                                // Advance cursor only after successful injection
                                // or when quiescent (no pending BG work to retry).
                                observed_seq = batch.watermark;
                                if let Some(ref cs) = epoch_cursor_state {
                                    cs.runtime_observed_seq.store(batch.watermark, std::sync::atomic::Ordering::Release);
                                }
                                drop(d);
                                if process_queue(
                                    &driver,
                                    &mut *executor,
                                    &mut control_rx,
                                    completions.as_ref(),
                                )
                                .await
                                {
                                    break;
                                }
                            }
                            // Non-quiescent: do NOT advance observed_seq.
                            // The completion stays visible for the next wake
                            // so it's not permanently lost.
                        } else {
                            // No new BG completions — advance to prevent hot-spin
                            // on non-BG entries (MobMemberChild, etc.).
                            observed_seq = batch.watermark;
                            if let Some(ref cs) = epoch_cursor_state {
                                cs.runtime_observed_seq.store(batch.watermark, std::sync::atomic::Ordering::Release);
                            }
                        }
                    } else if let Some(ref state) = detached_wake
                        && state.pending.load(std::sync::atomic::Ordering::Acquire)
                    {
                        // Legacy detached wake path (no feed).
                        let input = crate::input::Input::Continuation(
                            crate::input::ContinuationInput::detached_background_op_completed(),
                        );
                        let mut d = driver.lock().await;
                        if d.as_driver_mut().accept_input(input).await.is_ok() {
                            state.pending.store(false, std::sync::atomic::Ordering::Release);
                        }
                        drop(d);
                        if process_queue(
                            &driver,
                            &mut *executor,
                            &mut control_rx,
                            completions.as_ref(),
                        )
                        .await
                        {
                            break;
                        }
                    }
                }
            }
        }

        // Loop exiting — resolve any pending completion waiters as terminated.
        if let Some(ref completions) = completions {
            let mut reg = completions.lock().await;
            reg.resolve_all_terminated("runtime loop exited");
        }
    })
}

/// Check for new background op completions and inject a continuation if needed.
///
/// Called after queue processing completes (session has returned to idle).
/// Returns `true` if a continuation was injected (caller should process_queue).
/// Supports both feed-based (new) and DetachedWakeState (legacy) paths.
async fn maybe_inject_feed_wake(
    driver: &crate::session_adapter::SharedDriver,
    feed: Option<&dyn meerkat_core::completion_feed::CompletionFeed>,
    wake_state: Option<&std::sync::Arc<crate::detached_wake::DetachedWakeState>>,
    observed_seq: &mut meerkat_core::completion_feed::CompletionSeq,
    last_injected_seq: &mut meerkat_core::completion_feed::CompletionSeq,
    epoch_cursor_state: Option<&meerkat_core::EpochCursorState>,
) -> bool {
    if let Some(feed) = feed {
        let batch = feed.list_since(*observed_seq);

        let has_new_bg_completion = batch.entries.iter().any(|e| {
            e.kind == meerkat_core::ops_lifecycle::OperationKind::BackgroundToolOp
                && e.seq > *last_injected_seq
        });

        if !has_new_bg_completion {
            // No new BG completions — advance to prevent hot-spin
            // on non-BG entries (MobMemberChild, etc.).
            *observed_seq = batch.watermark;
            if let Some(cs) = epoch_cursor_state {
                cs.runtime_observed_seq
                    .store(batch.watermark, std::sync::atomic::Ordering::Release);
            }
            return false;
        }

        // Verify quiescence before injecting.
        let d = driver.lock().await;
        if !d.is_quiescent_for_detached_wake() {
            // Non-quiescent: do NOT advance observed_seq. The completion
            // stays visible for the next wake so it's not permanently lost.
            return false;
        }
        drop(d);

        let input = crate::input::Input::Continuation(
            crate::input::ContinuationInput::detached_background_op_completed(),
        );
        let mut d = driver.lock().await;
        if d.as_driver_mut().accept_input(input).await.is_ok() {
            *last_injected_seq = batch.watermark;
            if let Some(cs) = epoch_cursor_state {
                cs.runtime_last_injected_seq
                    .store(batch.watermark, std::sync::atomic::Ordering::Release);
            }
        }
        // Advance cursor after injection attempt (quiescent path).
        *observed_seq = batch.watermark;
        if let Some(cs) = epoch_cursor_state {
            cs.runtime_observed_seq
                .store(batch.watermark, std::sync::atomic::Ordering::Release);
        }
        true
    } else {
        // Legacy DetachedWakeState path
        let Some(state) = wake_state else {
            return false;
        };

        if !state.pending.load(std::sync::atomic::Ordering::Acquire) {
            return false;
        }
        if state.signaled.load(std::sync::atomic::Ordering::Acquire) {
            return false;
        }

        let d = driver.lock().await;
        if !d.is_quiescent_for_detached_wake() {
            return false;
        }
        drop(d);

        // Legacy path: fire notify to wake the idle arm on next iteration.
        state
            .signaled
            .store(true, std::sync::atomic::Ordering::Release);
        state.notify.notify_one();
        false // Legacy path doesn't inject inline — idle arm handles it
    }
}

/// Process all queued inputs until the queue is empty.
async fn process_queue(
    driver: &crate::session_adapter::SharedDriver,
    executor: &mut dyn meerkat_core::lifecycle::CoreExecutor,
    control_rx: &mut tokio::sync::mpsc::Receiver<
        meerkat_core::lifecycle::run_control::RunControlCommand,
    >,
    completions: Option<&crate::session_adapter::SharedCompletionRegistry>,
) -> bool {
    loop {
        if crate::control_plane::drain_ready_executor_controls(
            driver,
            completions,
            executor,
            control_rx,
        )
        .await
        {
            return true;
        }

        // Dequeue and prepare under the driver lock
        let dequeued = {
            let mut d = driver.lock().await;

            // Only process if the runtime can process queue (Idle or Retired)
            if !d.can_process_queue() {
                return false;
            }

            // Ask the ingress authority for the next batch of input IDs.
            // The authority implements steer-first priority and same-boundary
            // batching for steer inputs; queue inputs follow the prompt-aware
            // batching policy.
            let batch_ids = d
                .ingress()
                .drain_next_batch(|id| d.ingress().input_boundary(id));

            if batch_ids.is_empty() {
                return false;
            }

            // Dequeue the batch members from the physical queues.
            let staged_inputs: Vec<_> = batch_ids
                .iter()
                .filter_map(|id| d.dequeue_by_id(id))
                .collect();

            if staged_inputs.is_empty() {
                return false;
            }

            let run_id = RunId::new();

            // Start run in the state machine
            if d.start_run(run_id.clone()).is_err() {
                return false;
            }

            // Stage the input batch atomically (Queued → Staged).
            // Do NOT apply here — apply only after successful execution.
            // If we pre-applied and the executor failed, the input would
            // be stranded in AppliedPendingConsumption because RunFailed
            // only rolls back Staged inputs.
            let staged_ids: Vec<_> = staged_inputs.iter().map(|(id, _)| id.clone()).collect();
            if d.stage_batch(&staged_ids, &run_id).is_err() {
                let _ = d.rollback_staged(&staged_ids);
                let _ = d.complete_run();
                return false;
            }

            // Use the authority's boundary classification (derived from stored metadata)
            // instead of the shell-level input_boundary function.
            let boundary = staged_inputs
                .first()
                .map(|(id, _)| d.ingress().input_boundary(id))
                .unwrap_or(RunApplyBoundary::RunStart);
            let primitive = inputs_to_primitive_with_boundary(&staged_inputs, boundary);
            let contributing_input_ids = staged_inputs
                .iter()
                .map(|(staged_input_id, _)| staged_input_id.clone())
                .collect::<Vec<_>>();
            Some((contributing_input_ids, run_id, primitive))
        };

        match dequeued {
            Some((input_ids, run_id, primitive)) => {
                // Execute outside the driver lock (this calls start_turn, which is slow)
                let result = executor.apply(run_id.clone(), primitive).await;

                // Lock again to update driver state
                let mut d = driver.lock().await;
                match result {
                    Ok(output) => {
                        let meerkat_core::lifecycle::core_executor::CoreApplyOutput {
                            receipt,
                            session_snapshot,
                            run_result,
                            terminal,
                        } = output;

                        // BoundaryApplied transitions Staged → Applied → APC
                        // and triggers atomic persistence in PersistentRuntimeDriver
                        if let Err(err) = d
                            .as_driver_mut()
                            .on_run_event(RunEvent::BoundaryApplied {
                                run_id: run_id.clone(),
                                receipt,
                                session_snapshot,
                            })
                            .await
                        {
                            tracing::error!(%run_id, error = %err, "failed to record boundary-applied event");
                            if let Err(unwind_err) = d
                                .as_driver_mut()
                                .on_run_event(RunEvent::RunFailed {
                                    run_id: run_id.clone(),
                                    error: format!("boundary commit failed: {err}"),
                                    recoverable: true,
                                })
                                .await
                            {
                                tracing::error!(
                                    %run_id,
                                    error = %unwind_err,
                                    "failed to unwind runtime after boundary commit failure"
                                );
                            }
                            drop(d);
                            let _ = executor
                                .control(meerkat_core::lifecycle::run_control::RunControlCommand::StopRuntimeExecutor {
                                    reason: format!("runtime boundary commit failed for run {run_id}: {err}"),
                                })
                                .await;
                            return false;
                        }

                        // RunCompleted transitions APC → Consumed and returns to Idle/Retired
                        if let Err(err) = d
                            .as_driver_mut()
                            .on_run_event(RunEvent::RunCompleted {
                                run_id,
                                consumed_input_ids: input_ids.clone(),
                            })
                            .await
                        {
                            tracing::error!(error = %err, "failed to record run-completed event");
                            drop(d);
                            let _ = executor
                                .control(meerkat_core::lifecycle::run_control::RunControlCommand::StopRuntimeExecutor {
                                    reason: format!("runtime terminal snapshot failed after completion: {err}"),
                                })
                                .await;
                            return false;
                        }

                        // Resolve completion waiters unconditionally
                        if let Some(completions) = completions.as_ref() {
                            let mut reg = completions.lock().await;
                            resolve_completion_waiters(&mut reg, &input_ids, run_result, terminal);
                        }
                    }
                    Err(e) => {
                        let error_msg = e.to_string();
                        // RunFailed rolls back Staged → Queued and returns to Idle
                        if let Err(err) = d
                            .as_driver_mut()
                            .on_run_event(RunEvent::RunFailed {
                                run_id,
                                error: error_msg.clone(),
                                recoverable: true,
                            })
                            .await
                        {
                            tracing::error!(error = %err, "failed to record run-failed event");
                            drop(d);
                            let _ = executor
                                .control(meerkat_core::lifecycle::run_control::RunControlCommand::StopRuntimeExecutor {
                                    reason: format!("runtime failure snapshot failed: {err}"),
                                })
                                .await;
                            // Resolve waiter before breaking so callers don't hang.
                            if let Some(completions) = completions.as_ref() {
                                let mut completions = completions.lock().await;
                                for input_id in &input_ids {
                                    completions.resolve_abandoned(
                                        input_id,
                                        format!("runtime failure snapshot failed: {err}"),
                                    );
                                }
                            }
                            return false;
                        }
                        // Resolve completion waiter so callers don't hang.
                        if let Some(completions) = completions.as_ref() {
                            let mut completions = completions.lock().await;
                            for input_id in &input_ids {
                                completions.resolve_abandoned(
                                    input_id,
                                    format!("apply failed: {error_msg}"),
                                );
                            }
                        }
                        let should_continue =
                            d.take_wake_requested() && d.has_queued_input_outside(&input_ids);
                        drop(d);
                        if should_continue {
                            continue;
                        }
                        // Leave the failing input queued for a future wake instead of
                        // hot-looping on the same payload indefinitely.
                        return false;
                    }
                }
            }
            None => return false, // Queue empty
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::input::*;
    use chrono::Utc;

    fn make_prompt(text: &str) -> Input {
        Input::Prompt(PromptInput {
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::Operator,
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            text: text.into(),
            blocks: None,
            turn_metadata: None,
        })
    }

    #[test]
    fn input_to_prompt_extracts_text() {
        let input = make_prompt("hello world");
        assert_eq!(input_to_prompt(&input), "hello world");
    }

    #[test]
    fn input_to_prompt_peer() {
        let input = Input::Peer(PeerInput {
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::Peer {
                    peer_id: "p".into(),
                    runtime_id: None,
                },
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            convention: None,
            body: "peer message".into(),
            blocks: None,
            handling_mode: None,
        });
        assert_eq!(input_to_prompt(&input), "peer message");
    }

    #[test]
    fn input_to_primitive_creates_staged() -> Result<(), String> {
        let input = make_prompt("test prompt");
        let input_id = input.id().clone();
        let primitive = input_to_primitive(&input, input_id.clone());

        let staged = match primitive {
            RunPrimitive::StagedInput(staged) => staged,
            other => return Err(format!("expected staged input, got {other:?}")),
        };
        assert_eq!(staged.boundary, RunApplyBoundary::RunStart);
        assert_eq!(staged.contributing_input_ids, vec![input_id]);
        assert_eq!(staged.appends.len(), 1);
        assert_eq!(staged.appends[0].role, ConversationAppendRole::User);
        match &staged.appends[0].content {
            CoreRenderable::Text { text } => assert_eq!(text, "test prompt"),
            other => return Err(format!("expected text content, got {other:?}")),
        }
        Ok(())
    }

    #[test]
    fn peer_input_with_blocks_produces_blocks_renderable() -> Result<(), String> {
        let blocks = vec![
            meerkat_core::types::ContentBlock::Text {
                text: "see this image".into(),
            },
            meerkat_core::types::ContentBlock::Image {
                media_type: "image/png".into(),
                data: "abc123".into(),
            },
        ];
        let input = Input::Peer(PeerInput {
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::Peer {
                    peer_id: "p".into(),
                    runtime_id: None,
                },
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            convention: Some(crate::input::PeerConvention::Message),
            body: "see this image".into(),
            blocks: Some(blocks.clone()),
            handling_mode: None,
        });
        let input_id = input.id().clone();
        let primitive = input_to_primitive(&input, input_id);

        let staged = match primitive {
            RunPrimitive::StagedInput(staged) => staged,
            other => return Err(format!("expected staged input, got {other:?}")),
        };
        assert_eq!(staged.appends.len(), 1);
        match &staged.appends[0].content {
            CoreRenderable::Blocks { blocks: got } => {
                assert_eq!(got.len(), 3);
                assert_eq!(
                    got[0],
                    meerkat_core::types::ContentBlock::Text {
                        text: "[COMMS MESSAGE from p]".into(),
                    }
                );
                assert_eq!(got[1], blocks[0]);
                assert_eq!(got[2], blocks[1]);
            }
            other => return Err(format!("expected blocks content, got {other:?}")),
        }
        Ok(())
    }

    #[test]
    fn peer_multimodal_append_keeps_source_without_duplicating_block_text() -> Result<(), String> {
        let blocks = vec![
            meerkat_core::types::ContentBlock::Text {
                text: "caption text".into(),
            },
            meerkat_core::types::ContentBlock::Image {
                media_type: "image/png".into(),
                data: "abc123".into(),
            },
        ];
        let input = Input::Peer(PeerInput {
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::Peer {
                    peer_id: "peer-1".into(),
                    runtime_id: None,
                },
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            convention: Some(crate::input::PeerConvention::Message),
            body: "[COMMS MESSAGE from peer-1]\ncaption text\n[image: image/png]".into(),
            blocks: Some(blocks.clone()),
            handling_mode: None,
        });
        let staged = match input_to_primitive(&input, input.id().clone()) {
            RunPrimitive::StagedInput(staged) => staged,
            other => return Err(format!("expected staged input, got {other:?}")),
        };

        match &staged.appends[0].content {
            CoreRenderable::Blocks { blocks: got } => {
                assert_eq!(got.len(), 3);
                assert_eq!(
                    got[0],
                    meerkat_core::types::ContentBlock::Text {
                        text: "[COMMS MESSAGE from peer-1]".into(),
                    }
                );
                assert_eq!(got[1], blocks[0]);
                assert_eq!(got[2], blocks[1]);
            }
            other => return Err(format!("expected blocks content, got {other:?}")),
        }
        Ok(())
    }

    #[test]
    fn peer_image_only_blocks_keep_source_identity_without_text_duplication() -> Result<(), String>
    {
        let blocks = vec![meerkat_core::types::ContentBlock::Image {
            media_type: "image/png".into(),
            data: "abc123".into(),
        }];
        let input = Input::Peer(PeerInput {
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::Peer {
                    peer_id: "peer-1".into(),
                    runtime_id: None,
                },
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            convention: Some(crate::input::PeerConvention::Message),
            body: "[COMMS MESSAGE from peer-1]\n[image: image/png]".into(),
            blocks: Some(blocks.clone()),
            handling_mode: None,
        });
        let staged = match input_to_primitive(&input, input.id().clone()) {
            RunPrimitive::StagedInput(staged) => staged,
            other => return Err(format!("expected staged input, got {other:?}")),
        };

        match &staged.appends[0].content {
            CoreRenderable::Blocks { blocks: got } => {
                assert_eq!(got.len(), 2);
                assert_eq!(
                    got[0],
                    meerkat_core::types::ContentBlock::Text {
                        text: "[COMMS MESSAGE from peer-1]".into(),
                    }
                );
                assert_eq!(got[1], blocks[0]);
            }
            other => return Err(format!("expected blocks content, got {other:?}")),
        }
        Ok(())
    }

    #[test]
    fn flow_step_with_blocks_produces_blocks_renderable() -> Result<(), String> {
        let blocks = vec![
            meerkat_core::types::ContentBlock::Text {
                text: "analyze this screenshot".into(),
            },
            meerkat_core::types::ContentBlock::Image {
                media_type: "image/png".into(),
                data: "abc123".into(),
            },
        ];
        let input = Input::FlowStep(FlowStepInput {
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::Flow {
                    flow_id: "flow-1".into(),
                    step_index: 0,
                },
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            step_id: "step-1".into(),
            instructions: "analyze this screenshot".into(),
            blocks: Some(blocks),
            turn_metadata: None,
        });
        let input_id = input.id().clone();
        let primitive = input_to_primitive(&input, input_id);

        let staged = match primitive {
            RunPrimitive::StagedInput(staged) => staged,
            other => return Err(format!("expected staged input, got {other:?}")),
        };
        assert_eq!(staged.appends.len(), 1);
        match &staged.appends[0].content {
            CoreRenderable::Blocks { blocks: got } => assert_eq!(got.len(), 2),
            other => return Err(format!("expected blocks content, got {other:?}")),
        }
        Ok(())
    }

    #[test]
    fn external_event_with_blocks_produces_blocks_renderable() -> Result<(), String> {
        let blocks = vec![
            meerkat_core::types::ContentBlock::Text {
                text: "see this event".into(),
            },
            meerkat_core::types::ContentBlock::Image {
                media_type: "image/png".into(),
                data: "abc123".into(),
            },
        ];
        let input = Input::ExternalEvent(crate::input::ExternalEventInput {
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::External {
                    source_name: "webhook".into(),
                },
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            event_type: "webhook".into(),
            payload: serde_json::json!({"body": "see this event"}),
            blocks: Some(blocks.clone()),
            handling_mode: meerkat_core::types::HandlingMode::Queue,
            render_metadata: None,
        });
        let input_id = input.id().clone();
        let primitive = input_to_primitive(&input, input_id);

        let staged = match primitive {
            RunPrimitive::StagedInput(staged) => staged,
            other => return Err(format!("expected staged input, got {other:?}")),
        };
        assert_eq!(staged.appends.len(), 1);
        match &staged.appends[0].content {
            CoreRenderable::Blocks { blocks: got } => {
                // Blocks are preserved directly — no synthetic text header prepended.
                assert_eq!(got.len(), 2);
                assert_eq!(got[0], blocks[0]);
                assert_eq!(got[1], blocks[1]);
            }
            other => return Err(format!("expected blocks content, got {other:?}")),
        }
        Ok(())
    }

    #[test]
    fn external_event_with_image_only_blocks_keeps_event_identity() -> Result<(), String> {
        let blocks = vec![meerkat_core::types::ContentBlock::Image {
            media_type: "image/png".into(),
            data: "abc123".into(),
        }];
        let input = Input::ExternalEvent(crate::input::ExternalEventInput {
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::External {
                    source_name: "webhook".into(),
                },
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            event_type: "webhook".into(),
            payload: serde_json::json!({"body": "see attached screenshot"}),
            blocks: Some(blocks.clone()),
            handling_mode: meerkat_core::types::HandlingMode::Queue,
            render_metadata: None,
        });
        let primitive = input_to_primitive(&input, input.id().clone());

        let staged = match primitive {
            RunPrimitive::StagedInput(staged) => staged,
            other => return Err(format!("expected staged input, got {other:?}")),
        };
        match &staged.appends[0].content {
            CoreRenderable::Blocks { blocks: got } => {
                // Image-only blocks preserved directly without text header.
                assert_eq!(got.len(), 1);
                assert_eq!(got[0], blocks[0]);
            }
            other => return Err(format!("expected blocks content, got {other:?}")),
        }
        Ok(())
    }

    #[test]
    fn external_event_prefers_source_name_over_event_type_without_body() -> Result<(), String> {
        let input = Input::ExternalEvent(crate::input::ExternalEventInput {
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::External {
                    source_name: "webhook".into(),
                },
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            event_type: "invoice.created".into(),
            payload: serde_json::json!({"invoice_id": "inv_123"}),
            blocks: None,
            handling_mode: meerkat_core::types::HandlingMode::Queue,
            render_metadata: None,
        });

        assert_eq!(input_to_prompt(&input), "[EVENT via webhook]");
        Ok(())
    }

    #[test]
    fn plain_event_and_direct_runtime_external_event_share_projection() -> Result<(), String> {
        use crate::comms_bridge::classified_interaction_to_runtime_input;
        use crate::identifiers::LogicalRuntimeId;
        use meerkat_core::interaction::{
            ClassifiedInboxInteraction, InboxInteraction, InteractionContent, PeerInputClass,
        };

        let from_comms = classified_interaction_to_runtime_input(
            &ClassifiedInboxInteraction {
                class: PeerInputClass::PlainEvent,
                lifecycle_peer: None,
                interaction: InboxInteraction {
                    id: meerkat_core::interaction::InteractionId(uuid::Uuid::new_v4()),
                    from: "event:webhook".into(),
                    content: InteractionContent::Message {
                        body: "build failed".into(),
                        blocks: None,
                    },
                    rendered_text: "[EVENT via webhook] build failed".into(),
                    handling_mode: meerkat_core::types::HandlingMode::Queue,
                    render_metadata: None,
                },
            },
            &LogicalRuntimeId::new("test"),
        );

        let direct = Input::ExternalEvent(crate::input::ExternalEventInput {
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::External {
                    source_name: "webhook".into(),
                },
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            event_type: "webhook".into(),
            payload: serde_json::json!({"body": "build failed"}),
            blocks: None,
            handling_mode: meerkat_core::types::HandlingMode::Queue,
            render_metadata: None,
        });

        assert_eq!(input_to_prompt(&from_comms), input_to_prompt(&direct));
        assert_eq!(input_to_prompt(&direct), "[EVENT via webhook] build failed");
        Ok(())
    }

    #[test]
    fn external_event_with_steer_preserves_runtime_hints() -> Result<(), String> {
        let render_metadata = meerkat_core::types::RenderMetadata {
            class: meerkat_core::types::RenderClass::ExternalEvent,
            salience: meerkat_core::types::RenderSalience::Urgent,
        };
        let input = Input::ExternalEvent(crate::input::ExternalEventInput {
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::External {
                    source_name: "webhook".into(),
                },
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            event_type: "webhook".into(),
            payload: serde_json::json!({"body": "urgent"}),
            blocks: None,
            handling_mode: meerkat_core::types::HandlingMode::Steer,
            render_metadata: Some(render_metadata.clone()),
        });

        let staged = match input_to_primitive(&input, input.id().clone()) {
            RunPrimitive::StagedInput(staged) => staged,
            other => return Err(format!("expected staged input, got {other:?}")),
        };

        assert_eq!(staged.boundary, RunApplyBoundary::RunCheckpoint);
        assert_eq!(
            staged
                .turn_metadata
                .as_ref()
                .and_then(|meta| meta.handling_mode),
            Some(meerkat_core::types::HandlingMode::Steer)
        );
        assert_eq!(
            staged
                .turn_metadata
                .as_ref()
                .and_then(|meta| meta.render_metadata.clone()),
            Some(render_metadata)
        );
        Ok(())
    }

    #[tokio::test]
    async fn resolve_completion_waiters_surfaces_callback_pending() {
        let mut registry = crate::completion::CompletionRegistry::new();
        let input_id = InputId::new();
        let handle = registry.register(input_id.clone());

        resolve_completion_waiters(
            &mut registry,
            std::slice::from_ref(&input_id),
            None,
            Some(CoreApplyTerminal::CallbackPending {
                tool_name: "external_mock".to_string(),
                args: serde_json::json!({ "value": "browser" }),
            }),
        );

        match handle.wait().await {
            crate::completion::CompletionOutcome::CallbackPending { tool_name, args } => {
                assert_eq!(tool_name, "external_mock");
                assert_eq!(args, serde_json::json!({ "value": "browser" }));
            }
            other => panic!("Expected CallbackPending, got {other:?}"),
        }
    }

    // --- execution_kind stamping tests ---

    #[test]
    fn primitive_from_prompt_has_content_turn() {
        let input = make_prompt("hello");
        let id = input.id().clone();
        let primitive = input_to_primitive(&input, id);
        let meta = primitive
            .turn_metadata()
            .expect("should have turn_metadata");
        assert_eq!(
            meta.execution_kind,
            Some(meerkat_core::lifecycle::RuntimeExecutionKind::ContentTurn)
        );
    }

    #[test]
    fn primitive_from_continuation_has_resume_pending() {
        let input = Input::Continuation(ContinuationInput::detached_background_op_completed());
        let id = input.id().clone();
        let primitive = input_to_primitive(&input, id);
        let meta = primitive
            .turn_metadata()
            .expect("should have turn_metadata");
        assert_eq!(
            meta.execution_kind,
            Some(meerkat_core::lifecycle::RuntimeExecutionKind::ResumePending)
        );
    }

    #[test]
    fn primitive_from_peer_terminal_has_content_turn() {
        let input = Input::Peer(PeerInput {
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::Peer {
                    peer_id: "p".into(),
                    runtime_id: None,
                },
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            convention: Some(PeerConvention::ResponseTerminal {
                request_id: "r".into(),
                status: ResponseTerminalStatus::Completed,
            }),
            body: "done".into(),
            blocks: None,
            handling_mode: None,
        });
        let id = input.id().clone();
        let primitive = input_to_primitive(&input, id);
        let meta = primitive
            .turn_metadata()
            .expect("should have turn_metadata");
        assert_eq!(
            meta.execution_kind,
            Some(meerkat_core::lifecycle::RuntimeExecutionKind::ContentTurn)
        );
    }

    #[test]
    fn mixed_batch_defaults_to_content_turn() {
        // If a batch contains both ContentTurn and ResumePending inputs,
        // the whole batch must be ContentTurn (safe default).
        let peer = Input::Peer(PeerInput {
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::Peer {
                    peer_id: "p".into(),
                    runtime_id: None,
                },
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            convention: Some(PeerConvention::Message),
            body: "msg".into(),
            blocks: None,
            handling_mode: None,
        });
        let continuation =
            Input::Continuation(ContinuationInput::detached_background_op_completed());
        let inputs = vec![
            (peer.id().clone(), peer),
            (continuation.id().clone(), continuation),
        ];
        let primitive = inputs_to_primitive_with_boundary(&inputs, RunApplyBoundary::RunCheckpoint);
        let meta = primitive
            .turn_metadata()
            .expect("should have turn_metadata");
        assert_eq!(
            meta.execution_kind,
            Some(meerkat_core::lifecycle::RuntimeExecutionKind::ContentTurn),
            "mixed batch must default to ContentTurn"
        );
    }
}
