//! RuntimeLoop — per-session tokio task that processes queued inputs.
//!
//! When `MeerkatMachine::accept_input()` queues an input and sets
//! the wake flag, it sends a signal on the wake channel. The RuntimeLoop
//! picks it up, dequeues the input, converts it to a `RunPrimitive`,
//! and applies it via the `CoreExecutor` (which calls `SessionService::start_turn()`
//! under the hood).

use meerkat_core::lifecycle::core_executor::CoreApplyTerminal;
use meerkat_core::lifecycle::run_primitive::{RunApplyBoundary, RunPrimitive, StagedRunInput};
use meerkat_core::lifecycle::{InputId, RunId};

#[cfg(test)]
use crate::input::input_prompt_text;
use crate::input::{Input, runtime_input_projection};
use crate::runtime_state::RuntimeState;
use crate::tokio;

/// Extract a prompt string from an `Input`.
#[cfg(test)]
pub(crate) fn input_to_prompt(input: &Input) -> String {
    input_prompt_text(input)
}

/// Canonical runtime-side constructor for [`RuntimeTurnMetadata`].
///
/// This is the ONLY construction site for `RuntimeTurnMetadata` inside
/// `meerkat-runtime/src/`. Any other literal/default construction is an
/// RMAT-governed seam leak; the `turn_metadata_single_construction_site`
/// integration test greps for that invariant at build time.
pub(crate) fn for_input(
    input: &Input,
) -> Option<meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata> {
    use meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata;
    match input {
        Input::Prompt(prompt) => prompt.turn_metadata.clone(),
        Input::FlowStep(flow_step) => flow_step.turn_metadata.clone(),
        Input::ExternalEvent(event) => Some(RuntimeTurnMetadata {
            handling_mode: Some(event.handling_mode),
            render_metadata: event.render_metadata.clone(),
            ..Default::default()
        }),
        Input::Continuation(continuation) => Some(RuntimeTurnMetadata {
            handling_mode: Some(continuation.handling_mode),
            ..Default::default()
        }),
        _ => None,
    }
}

/// Merge the per-input turn metadata carried by a staged batch into a single
/// typed carrier. Scalar conflicts (two inputs disagreeing on e.g. `model`)
/// are refused with a typed error so caller policy is not silently replaced by
/// shell defaults.
pub(crate) fn merge_batch_turn_metadata(
    inputs: &[(meerkat_core::lifecycle::InputId, Input)],
) -> Result<
    Option<meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata>,
    meerkat_core::lifecycle::run_primitive::TurnMetadataMergeConflict,
> {
    use meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata;
    let mut acc: Option<RuntimeTurnMetadata> = None;
    for (_, input) in inputs {
        let Some(meta) = for_input(input) else {
            continue;
        };
        match acc.as_mut() {
            None => acc = Some(meta),
            Some(existing) => {
                existing.merge(meta)?;
            }
        }
    }
    Ok(acc.filter(|m| !m.is_empty()))
}

fn resolve_completion_waiters(
    registry: &mut crate::completion::CompletionRegistry,
    input_ids: &[InputId],
    run_result: Option<meerkat_core::types::RunResult>,
    terminal: Option<CoreApplyTerminal>,
) {
    if let Some(CoreApplyTerminal::CallbackPending { tool_name, args }) = terminal {
        for input_id in input_ids {
            registry.resolve_callback_pending(input_id, tool_name.clone(), args.clone());
        }
        return;
    }

    if let Some(result) = run_result {
        for input_id in input_ids {
            registry.resolve_completed(input_id, result.clone());
        }
        return;
    }

    match terminal {
        Some(CoreApplyTerminal::RunResult(_) | CoreApplyTerminal::NoPendingBoundary) | None => {
            for input_id in input_ids {
                registry.resolve_without_result(input_id);
            }
        }
        Some(CoreApplyTerminal::CallbackPending { .. }) => unreachable!(),
    }
}

fn primitive_admitted_content_shape(primitive: &RunPrimitive) -> String {
    match primitive {
        RunPrimitive::StagedInput(staged) => {
            match (staged.appends.is_empty(), staged.context_appends.is_empty()) {
                (false, false) => "conversation+context",
                (false, true) => "conversation",
                (true, false) => "context",
                (true, true) => "empty",
            }
        }
        RunPrimitive::ImmediateAppend(_) => "immediate_append",
        RunPrimitive::ImmediateContextAppend(_) => "immediate_context",
        _ => "conversation",
    }
    .to_string()
}

fn primitive_turn_start_input(
    run_id: &RunId,
    primitive: &RunPrimitive,
) -> Option<crate::meerkat_machine::dsl::MeerkatMachineInput> {
    let admitted_content_shape = primitive_admitted_content_shape(primitive);
    match primitive {
        RunPrimitive::ImmediateAppend(_) => Some(
            crate::meerkat_machine::dsl::MeerkatMachineInput::StartImmediateAppend {
                run_id: crate::meerkat_machine::dsl::RunId::from_domain(run_id),
                admitted_content_shape,
            },
        ),
        RunPrimitive::ImmediateContextAppend(_) => Some(
            crate::meerkat_machine::dsl::MeerkatMachineInput::StartImmediateContext {
                run_id: crate::meerkat_machine::dsl::RunId::from_domain(run_id),
                admitted_content_shape,
            },
        ),
        RunPrimitive::StagedInput(staged)
            if staged.appends.is_empty() && !staged.context_appends.is_empty() =>
        {
            Some(
                crate::meerkat_machine::dsl::MeerkatMachineInput::StartImmediateContext {
                    run_id: crate::meerkat_machine::dsl::RunId::from_domain(run_id),
                    admitted_content_shape,
                },
            )
        }
        RunPrimitive::StagedInput(staged) if staged.appends.is_empty() => None,
        RunPrimitive::StagedInput(_) => Some(
            crate::meerkat_machine::dsl::MeerkatMachineInput::StartConversationRun {
                run_id: crate::meerkat_machine::dsl::RunId::from_domain(run_id),
                primitive_kind: crate::meerkat_machine::dsl::TurnPrimitiveKind::ConversationTurn,
                admitted_content_shape,
                vision_enabled: false,
                image_tool_results_enabled: false,
                max_extraction_retries: 0,
            },
        ),
        _ => Some(
            crate::meerkat_machine::dsl::MeerkatMachineInput::StartConversationRun {
                run_id: crate::meerkat_machine::dsl::RunId::from_domain(run_id),
                primitive_kind: crate::meerkat_machine::dsl::TurnPrimitiveKind::ConversationTurn,
                admitted_content_shape,
                vision_enabled: false,
                image_tool_results_enabled: false,
                max_extraction_retries: 0,
            },
        ),
    }
}

async fn prepare_turn_state_for_primitive(
    driver: &crate::meerkat_machine::SharedDriver,
    run_id: &RunId,
    primitive: &RunPrimitive,
) -> Result<(), crate::RuntimeDriverError> {
    let Some(input) = primitive_turn_start_input(run_id, primitive) else {
        return Ok(());
    };
    let authority = {
        let driver = driver.lock().await;
        driver.shared_dsl_authority()
    };
    let mut auth = authority
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    // Retired drain path: `machine_begin_run` treats Retired as
    // `is_retired_drain` and skips the `Prepare` DSL input, leaving
    // phase at Retired. The turn-start transitions
    // (`StartConversationRun{Initializing,Attached}` /
    // `StartImmediate{Append,Context}{Initializing,Attached}`) only
    // guard on Initializing/Attached — no Retired variant. Skip the
    // signal during drain; shell-side `set_control_projection` has
    // already advanced control so `executor.apply` proceeds next.
    if auth.state.lifecycle_phase == crate::meerkat_machine::dsl::MeerkatPhase::Retired {
        return Ok(());
    }
    crate::meerkat_machine::dsl::MeerkatMachineMutator::apply(&mut *auth, input)
        .map(|_| ())
        .map_err(|err| {
            crate::RuntimeDriverError::Internal(format!(
                "failed to start runtime turn state for run {run_id}: {err}"
            ))
        })
}

pub(crate) fn try_inputs_to_primitive_with_boundary(
    inputs: &[(InputId, Input)],
    boundary: RunApplyBoundary,
    execution_kind: Option<meerkat_core::lifecycle::RuntimeExecutionKind>,
) -> Result<RunPrimitive, meerkat_core::lifecycle::run_primitive::TurnMetadataMergeConflict> {
    let projections = inputs
        .iter()
        .map(|(_, input)| runtime_input_projection(input))
        .collect::<Vec<_>>();
    try_projected_inputs_to_primitive_with_boundary(inputs, &projections, boundary, execution_kind)
}

pub(crate) fn try_projected_inputs_to_primitive_with_boundary(
    inputs: &[(InputId, Input)],
    projections: &[crate::ingress_types::RuntimeInputProjection],
    boundary: RunApplyBoundary,
    execution_kind: Option<meerkat_core::lifecycle::RuntimeExecutionKind>,
) -> Result<RunPrimitive, meerkat_core::lifecycle::run_primitive::TurnMetadataMergeConflict> {
    let appends = projections
        .iter()
        .filter_map(|projection| projection.append.clone())
        .collect::<Vec<_>>();
    let context_appends = projections
        .iter()
        .filter_map(|projection| projection.context_append.clone())
        .collect::<Vec<_>>();
    let contributing_input_ids = inputs
        .iter()
        .map(|(input_id, _)| input_id.clone())
        .collect::<Vec<_>>();
    // Merge turn metadata from ALL inputs in the batch, not just the first.
    // Scalar conflicts are typed errors; collection fields accumulate.
    let turn_metadata = merge_batch_turn_metadata(inputs)?;

    let turn_metadata = {
        let mut meta = turn_metadata.unwrap_or_default();
        meta.execution_kind = execution_kind;
        Some(meta)
    };

    Ok(RunPrimitive::StagedInput(StagedRunInput {
        boundary,
        appends,
        context_appends,
        contributing_input_ids,
        turn_metadata,
    }))
}

pub(crate) fn inputs_to_primitive_with_boundary(
    inputs: &[(InputId, Input)],
    boundary: RunApplyBoundary,
) -> Result<RunPrimitive, meerkat_core::lifecycle::run_primitive::TurnMetadataMergeConflict> {
    let execution_kind = fallback_batch_execution_kind(inputs);
    try_inputs_to_primitive_with_boundary(inputs, boundary, execution_kind)
}

pub(crate) fn inputs_to_primitive(
    inputs: &[(InputId, Input)],
) -> Result<RunPrimitive, meerkat_core::lifecycle::run_primitive::TurnMetadataMergeConflict> {
    let boundary = inputs
        .first()
        .map(|(_, input)| fallback_unadmitted_semantics(input).boundary)
        .unwrap_or(RunApplyBoundary::RunStart);
    inputs_to_primitive_with_boundary(inputs, boundary)
}

fn fallback_unadmitted_semantics(input: &Input) -> crate::ingress_types::RuntimeInputSemantics {
    let policy = crate::policy_table::DefaultPolicyTable::resolve(input, true);
    crate::ingress_types::RuntimeInputSemantics::from_policy_and_kind(&policy, input.kind())
}

fn fallback_batch_execution_kind(
    inputs: &[(InputId, Input)],
) -> Option<meerkat_core::lifecycle::RuntimeExecutionKind> {
    if inputs.is_empty() {
        return None;
    }
    if inputs.iter().all(|(_, input)| {
        fallback_unadmitted_semantics(input).execution_kind
            == meerkat_core::lifecycle::RuntimeExecutionKind::ResumePending
    }) {
        Some(meerkat_core::lifecycle::RuntimeExecutionKind::ResumePending)
    } else {
        Some(meerkat_core::lifecycle::RuntimeExecutionKind::ContentTurn)
    }
}

/// Convert an `Input` + its ID to a `RunPrimitive` for `CoreExecutor::apply()`.
pub(crate) fn input_to_primitive(
    input: &Input,
    input_id: InputId,
) -> Result<RunPrimitive, meerkat_core::lifecycle::run_primitive::TurnMetadataMergeConflict> {
    inputs_to_primitive(&[(input_id, input.clone())])
}

/// Spawn the per-session runtime loop with optional completion registry.
#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_runtime_loop_with_completions(
    driver: crate::meerkat_machine::SharedDriver,
    mut executor: Box<dyn meerkat_core::lifecycle::CoreExecutor>,
    mut wake_rx: tokio::sync::mpsc::Receiver<()>,
    mut control_rx: tokio::sync::mpsc::Receiver<
        meerkat_core::lifecycle::run_control::RunControlCommand,
    >,
    completions: Option<crate::meerkat_machine::SharedCompletionRegistry>,
    completion_feed: Option<std::sync::Arc<dyn meerkat_core::completion_feed::CompletionFeed>>,
    epoch_cursor_state: Option<std::sync::Arc<meerkat_core::EpochCursorState>>,
    machine_weak: std::sync::Weak<crate::meerkat_machine::MeerkatMachine>,
    session_id: meerkat_core::types::SessionId,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        // Feed-based idle wake state (local to this loop).
        // Seed from epoch cursor state when available (runtime-backed surfaces).
        // Even an all-zero cursor must win over the feed watermark so a fresh
        // runtime loop cannot silently skip background completions that land
        // before the task reaches its first select iteration. Only callers that
        // do not provide cursor state fall back to the feed watermark to avoid
        // replaying historical completions.
        let initial_watermark = epoch_cursor_state
            .as_ref()
            .map(|cs| {
                let obs = cs
                    .runtime_observed_seq
                    .load(std::sync::atomic::Ordering::Acquire);
                let inj = cs
                    .runtime_last_injected_seq
                    .load(std::sync::atomic::Ordering::Acquire);
                obs.max(inj)
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
            // Build a future for the idle wake. Backed by the completion feed
            // when present; otherwise pends forever (no background ops can
            // complete without a feed-producing ops registry).
            let idle_wake = async {
                if let Some(ref feed) = completion_feed {
                    feed.wait_for_advance(observed_seq).await;
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
                                &machine_weak,
                                &session_id,
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
                            &machine_weak, &session_id,)
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
                                &machine_weak, &session_id,)
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
                                &machine_weak, &session_id,)
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
async fn maybe_inject_feed_wake(
    driver: &crate::meerkat_machine::SharedDriver,
    feed: Option<&dyn meerkat_core::completion_feed::CompletionFeed>,
    observed_seq: &mut meerkat_core::completion_feed::CompletionSeq,
    last_injected_seq: &mut meerkat_core::completion_feed::CompletionSeq,
    epoch_cursor_state: Option<&meerkat_core::EpochCursorState>,
) -> bool {
    let Some(feed) = feed else {
        return false;
    };
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
}

/// Process all queued inputs until the queue is empty.
#[allow(clippy::too_many_arguments)]
async fn process_queue(
    driver: &crate::meerkat_machine::SharedDriver,
    executor: &mut dyn meerkat_core::lifecycle::CoreExecutor,
    control_rx: &mut tokio::sync::mpsc::Receiver<
        meerkat_core::lifecycle::run_control::RunControlCommand,
    >,
    completions: Option<&crate::meerkat_machine::SharedCompletionRegistry>,
    machine_weak: &std::sync::Weak<crate::meerkat_machine::MeerkatMachine>,
    session_id: &meerkat_core::types::SessionId,
) -> bool {
    loop {
        if crate::control_plane::drain_ready_executor_controls(
            driver,
            completions,
            executor,
            control_rx,
            machine_weak,
            session_id,
        )
        .await
        {
            return true;
        }

        // Dequeue and prepare under the driver lock
        let dequeued = {
            let mut d = driver.lock().await;

            // Immediate attached steer pre-binds the DSL run before waking the
            // loop. Honor that Running/current_run_id pair as a queued batch
            // that is already prepared by the checked-in machine.
            let prebound_run_id = if d.runtime_state() == RuntimeState::Running {
                d.current_run_id()
            } else {
                None
            };
            if !d.can_process_queue() && prebound_run_id.is_none() {
                return false;
            }

            // Ask the ingress authority for the next batch of input IDs.
            // The authority implements steer-first priority and same-boundary
            // batching for steer inputs; queue inputs follow the prompt-aware
            // batching policy.
            let batch_ids = crate::meerkat_machine::machine_select_runtime_loop_batch(&d);

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

            let run_id = prebound_run_id.unwrap_or_else(RunId::new);

            // The checked-in Meerkat machine owns the coarse "this dequeued
            // batch has now become a run" transition, including unwind on
            // staging failure.
            let staged_ids: Vec<_> = staged_inputs.iter().map(|(id, _)| id.clone()).collect();

            // The checked-in Meerkat machine now owns runtime-loop batch
            // boundary classification over the stored ingress metadata.
            let boundary = staged_inputs
                .first()
                .map(|(id, _)| crate::meerkat_machine::machine_input_boundary(&d, id))
                .unwrap_or(RunApplyBoundary::RunStart);
            let contributing_input_ids = staged_inputs
                .iter()
                .map(|(staged_input_id, _)| staged_input_id.clone())
                .collect::<Vec<_>>();
            let execution_kind =
                crate::meerkat_machine::machine_batch_execution_kind(&d, &staged_ids);
            let projections =
                crate::meerkat_machine::machine_batch_primitive_projections(&d, &staged_ids);
            let primitive = try_projected_inputs_to_primitive_with_boundary(
                &staged_inputs,
                &projections,
                boundary,
                execution_kind,
            );
            Some((contributing_input_ids, staged_ids, run_id, primitive))
        };

        match dequeued {
            Some((input_ids, staged_ids, run_id, primitive)) => {
                if let Err(err) = crate::meerkat_machine::prepare_runtime_loop_batch_start(
                    driver,
                    run_id.clone(),
                    &staged_ids,
                )
                .await
                {
                    tracing::error!(%run_id, error = %err, "failed to prepare runtime loop batch");
                    return false;
                }
                let primitive = match primitive {
                    Ok(primitive) => primitive,
                    Err(conflict) => {
                        tracing::error!(
                            %run_id,
                            field = conflict.field,
                            reason = conflict.reason,
                            "batch turn-metadata merge conflict"
                        );
                        let _ = crate::meerkat_machine::fail_runtime_loop_run(
                            driver,
                            run_id,
                            conflict.to_string(),
                        )
                        .await;
                        return false;
                    }
                };
                if let Err(error) =
                    prepare_turn_state_for_primitive(driver, &run_id, &primitive).await
                {
                    tracing::error!(%run_id, error = %error, "failed to start runtime turn state");
                    let _ = crate::meerkat_machine::fail_runtime_loop_run(
                        driver,
                        run_id,
                        error.to_string(),
                    )
                    .await;
                    return false;
                }

                // Execute outside the driver lock (this calls start_turn, which is slow)
                let result = executor.apply(run_id.clone(), primitive).await;

                // Lock again to update driver state
                let d = driver.lock().await;
                match result {
                    Ok(output) => {
                        let meerkat_core::lifecycle::core_executor::CoreApplyOutput {
                            receipt,
                            session_snapshot,
                            run_result,
                            terminal,
                        } = output;
                        drop(d);
                        if let Err(err) = crate::meerkat_machine::commit_runtime_loop_run(
                            driver,
                            run_id.clone(),
                            input_ids.clone(),
                            receipt,
                            session_snapshot,
                        )
                        .await
                        {
                            tracing::error!(%run_id, error = %err, "failed to commit runtime loop run");
                            let _ = executor
                                .control(meerkat_core::lifecycle::run_control::RunControlCommand::StopRuntimeExecutor {
                                    reason: format!("runtime loop commit failed for run {run_id}: {err}"),
                                })
                                .await;
                            return true;
                        }

                        // Resolve completion waiters unconditionally
                        if let Some(completions) = completions.as_ref() {
                            let mut reg = completions.lock().await;
                            resolve_completion_waiters(&mut reg, &input_ids, run_result, terminal);
                        }
                    }
                    Err(e) => {
                        let error_msg = e.to_string();
                        drop(d);
                        // RunFailed rolls back Staged → Queued and returns to Idle
                        if let Err(err) = crate::meerkat_machine::fail_runtime_loop_run(
                            driver,
                            run_id,
                            error_msg.clone(),
                        )
                        .await
                        {
                            tracing::error!(error = %err, "failed to record run-failed event");
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
                            return true;
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
                        let mut d = driver.lock().await;
                        let should_continue = d.has_queued_input_outside(&input_ids);
                        if should_continue {
                            d.defer_queued_inputs_behind_backlog(&input_ids);
                            d.take_wake_requested();
                        }
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
    use meerkat_core::lifecycle::run_primitive::{ConversationAppendRole, CoreRenderable};
    use std::sync::Arc;

    use meerkat_core::ops_lifecycle::{
        OperationKind, OperationResult, OperationSpec, OpsLifecycleRegistry,
    };
    use meerkat_core::types::SessionId;

    fn background_spec(name: &str) -> OperationSpec {
        OperationSpec {
            id: meerkat_core::ops_lifecycle::OperationId::new(),
            kind: OperationKind::BackgroundToolOp,
            owner_session_id: SessionId::new(),
            display_name: name.into(),
            source_label: "runtime-loop-test".into(),
            child_session_id: None,
            expect_peer_channel: false,
        }
    }

    fn op_result(id: &meerkat_core::ops_lifecycle::OperationId, content: &str) -> OperationResult {
        OperationResult {
            id: id.clone(),
            content: content.into(),
            is_error: false,
            duration_ms: 42,
            tokens_used: 7,
        }
    }

    fn make_shared_ephemeral_driver(runtime_id: &str) -> crate::meerkat_machine::SharedDriver {
        Arc::new(crate::tokio::sync::Mutex::new(
            crate::meerkat_machine::DriverEntry::Ephemeral(
                crate::driver::ephemeral::EphemeralRuntimeDriver::new(
                    crate::identifiers::LogicalRuntimeId::new(runtime_id),
                ),
            ),
        ))
    }

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
            payload: None,
            blocks: None,
            handling_mode: None,
        });
        assert_eq!(input_to_prompt(&input), "peer message");
    }

    #[test]
    fn input_to_prompt_peer_message_uses_body_when_projection_text_is_empty() {
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
            body: "[COMMS MESSAGE from peer-1]\nplain body payload".into(),
            payload: None,
            blocks: None,
            handling_mode: None,
        });

        assert_eq!(
            input_to_prompt(&input),
            "[COMMS MESSAGE from peer-1]\nplain body payload"
        );
    }

    #[test]
    fn input_to_prompt_peer_request_is_runtime_owned_and_ignores_bogus_body() {
        let input = Input::Peer(PeerInput {
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::Peer {
                    peer_id: "operator-rt".into(),
                    runtime_id: None,
                },
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            convention: Some(crate::input::PeerConvention::Request {
                request_id: "req-123".into(),
                intent: "checksum_token".into(),
            }),
            body: "stale helper-local comms prose".into(),
            payload: Some(serde_json::json!({"subject": "alpha beta gamma"})),
            blocks: None,
            handling_mode: None,
        });

        assert_eq!(
            input_to_prompt(&input),
            "[SYSTEM NOTICE][PEER_REQUEST] Correlated peer request from operator-rt. Intent: checksum_token. Request ID: req-123. Params: {\n  \"subject\": \"alpha beta gamma\"\n}. This is not a normal user request and not a prompt for direct user-facing output. Handle it by calling send_response with to=\"operator-rt\", in_reply_to=\"req-123\", status=\"completed\" or \"failed\", and result=<JSON payload>. Do not use send_message for this reply."
        );
    }

    #[test]
    fn input_to_prompt_peer_response_terminal_is_runtime_owned_from_typed_payload() {
        let input = Input::Peer(PeerInput {
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::Peer {
                    peer_id: "analyst-rt".into(),
                    runtime_id: None,
                },
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            convention: Some(crate::input::PeerConvention::ResponseTerminal {
                request_id: "req-123".into(),
                status: crate::input::ResponseTerminalStatus::Completed,
            }),
            body: "stale helper-local comms prose".into(),
            payload: Some(serde_json::json!({
                "request_intent": "checksum_token",
                "token": "birch seventeen"
            })),
            blocks: None,
            handling_mode: None,
        });

        assert_eq!(
            input_to_prompt(&input),
            "[SYSTEM NOTICE][PEER_RESPONSE_TERMINAL] Correlated peer response from analyst-rt. Request ID: req-123. Status: completed. Result: {\n  \"request_intent\": \"checksum_token\",\n  \"token\": \"birch seventeen\"\n}."
        );
    }

    #[test]
    fn input_to_prompt_peer_response_terminal_omits_payload_key_extraction() {
        // Regression: runtime must not reach into the peer response payload
        // to extract ad-hoc fields (request_intent, token, etc.) and bake
        // them into prompt text. The canonical projection is the typed
        // convention + the Result JSON verbatim; any per-fixture semantics
        // belong to the peer application, not the runtime.
        let input = Input::Peer(PeerInput {
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::Peer {
                    peer_id: "analyst-rt".into(),
                    runtime_id: None,
                },
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            convention: Some(crate::input::PeerConvention::ResponseTerminal {
                request_id: "req-123".into(),
                status: crate::input::ResponseTerminalStatus::Completed,
            }),
            body: "stale helper-local comms prose".into(),
            payload: Some(serde_json::json!({
                "request_intent": "checksum_token",
                "token": "birch seventeen"
            })),
            blocks: None,
            handling_mode: None,
        });

        let rendered = input_to_prompt(&input);
        assert!(
            !rendered.contains("Authoritative result fields:"),
            "runtime must not emit scenario-specific authoritative-field hints: {rendered}"
        );
        assert!(
            !rendered.contains("the exact token answer is"),
            "runtime must not coach the model about specific payload values: {rendered}"
        );
        assert!(
            !rendered.contains("request_intent=checksum_token"),
            "runtime must not re-flatten payload keys into prompt text: {rendered}"
        );
    }

    #[test]
    fn input_to_primitive_creates_staged() -> Result<(), String> {
        let input = make_prompt("test prompt");
        let input_id = input.id().clone();
        let primitive = input_to_primitive(&input, input_id.clone())
            .expect("single input metadata cannot conflict");

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
    fn staged_conversation_input_starts_conversation_turn_state() -> Result<(), String> {
        let input = make_prompt("test prompt");
        let input_id = input.id().clone();
        let primitive =
            input_to_primitive(&input, input_id).expect("single input metadata cannot conflict");
        let run_id = RunId::new();

        let start_input = primitive_turn_start_input(&run_id, &primitive)
            .ok_or_else(|| "expected staged content input to start turn state".to_string())?;

        match start_input {
            crate::meerkat_machine::dsl::MeerkatMachineInput::StartConversationRun {
                run_id: got_run_id,
                primitive_kind,
                admitted_content_shape,
                ..
            } => {
                assert_eq!(
                    got_run_id,
                    crate::meerkat_machine::dsl::RunId::from_domain(&run_id)
                );
                assert_eq!(
                    primitive_kind,
                    crate::meerkat_machine::dsl::TurnPrimitiveKind::ConversationTurn
                );
                assert_eq!(admitted_content_shape, "conversation");
                Ok(())
            }
            other => Err(format!("expected StartConversationRun, got {other:?}")),
        }
    }

    #[test]
    fn peer_response_terminal_creates_immediate_context_staged_input() -> Result<(), String> {
        let input = Input::Peer(PeerInput {
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::Peer {
                    peer_id: "analyst-rt".into(),
                    runtime_id: None,
                },
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            convention: Some(crate::input::PeerConvention::ResponseTerminal {
                request_id: "req-123".into(),
                status: crate::input::ResponseTerminalStatus::Completed,
            }),
            body: "stale helper-local comms prose".into(),
            payload: Some(serde_json::json!({
                "request_intent": "checksum_token",
                "token": "birch seventeen"
            })),
            blocks: None,
            handling_mode: None,
        });
        let input_id = input.id().clone();

        let primitive = inputs_to_primitive_with_boundary(
            &[(input_id.clone(), input)],
            RunApplyBoundary::Immediate,
        )
        .expect("single input metadata cannot conflict");
        let staged = match primitive {
            RunPrimitive::StagedInput(staged) => staged,
            other => return Err(format!("expected staged input, got {other:?}")),
        };
        assert_eq!(staged.boundary, RunApplyBoundary::Immediate);
        assert_eq!(staged.contributing_input_ids, vec![input_id]);
        assert!(staged.appends.is_empty());
        assert_eq!(staged.context_appends.len(), 1);
        assert_eq!(
            staged.context_appends[0].key,
            "peer_response_terminal:analyst-rt:req-123"
        );
        match &staged.context_appends[0].content {
            CoreRenderable::Text { text } => {
                assert!(text.contains("[SYSTEM NOTICE][PEER_RESPONSE_TERMINAL]"));
                assert!(text.contains("birch seventeen"));
            }
            other => return Err(format!("expected text content, got {other:?}")),
        }
        Ok(())
    }

    #[test]
    fn peer_response_terminal_input_boundary_matches_machine_boundary() -> Result<(), String> {
        // Row 12 unification: `input_boundary` (typed Input view) and
        // `machine_input_boundary` (ingress handling_mode view) must agree
        // on ResponseTerminal inputs. Canonical answer is RunStart, since
        // ResponseTerminal is forbidden from carrying a handling_mode
        // override and the runtime-loop batch path already stages it as
        // RunStart.
        let input = Input::Peer(PeerInput {
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::Peer {
                    peer_id: "analyst-rt".into(),
                    runtime_id: None,
                },
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            convention: Some(crate::input::PeerConvention::ResponseTerminal {
                request_id: "req-123".into(),
                status: crate::input::ResponseTerminalStatus::Completed,
            }),
            body: "done".into(),
            payload: Some(serde_json::json!({
                "request_intent": "checksum_token",
                "token": "birch seventeen"
            })),
            blocks: None,
            handling_mode: None,
        });

        let primitive = inputs_to_primitive(&[(input.id().clone(), input)])
            .expect("single input metadata cannot conflict");
        let staged = match primitive {
            RunPrimitive::StagedInput(staged) => staged,
            other => return Err(format!("expected staged input, got {other:?}")),
        };
        assert_eq!(staged.boundary, RunApplyBoundary::RunStart);
        // Terminal peer payload still routes through the context-append
        // path (not user-visible appends) since `input_to_append` returns
        // None for the ResponseTerminal convention.
        assert!(staged.appends.is_empty());
        assert_eq!(staged.context_appends.len(), 1);
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
            payload: None,
            blocks: Some(blocks.clone()),
            handling_mode: None,
        });
        let input_id = input.id().clone();
        let primitive =
            input_to_primitive(&input, input_id).expect("single input metadata cannot conflict");

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
            payload: None,
            blocks: Some(blocks.clone()),
            handling_mode: None,
        });
        let staged = match input_to_primitive(&input, input.id().clone())
            .expect("single input metadata cannot conflict")
        {
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
            payload: None,
            blocks: Some(blocks.clone()),
            handling_mode: None,
        });
        let staged = match input_to_primitive(&input, input.id().clone())
            .expect("single input metadata cannot conflict")
        {
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
        let primitive =
            input_to_primitive(&input, input_id).expect("single input metadata cannot conflict");

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
        let primitive =
            input_to_primitive(&input, input_id).expect("single input metadata cannot conflict");

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
        let primitive = input_to_primitive(&input, input.id().clone())
            .expect("single input metadata cannot conflict");

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
        use crate::comms_bridge::peer_input_candidate_to_runtime_input;
        use crate::identifiers::LogicalRuntimeId;
        use meerkat_core::interaction::{
            InboxInteraction, InteractionContent, PeerInputCandidate, PeerInputClass,
        };

        let from_comms = peer_input_candidate_to_runtime_input(
            &PeerInputCandidate {
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

        let staged = match input_to_primitive(&input, input.id().clone())
            .expect("single input metadata cannot conflict")
        {
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

    #[tokio::test]
    async fn maybe_inject_feed_wake_feed_path_injects_inline_continuation_when_quiescent() {
        let driver = make_shared_ephemeral_driver("feed-inline");
        let registry = crate::ops_lifecycle::RuntimeOpsLifecycleRegistry::new();

        let spec = background_spec("feed-inline");
        let op_id = spec.id.clone();
        registry.register_operation(spec).unwrap();
        registry.provisioning_succeeded(&op_id).unwrap();
        registry
            .complete_operation(&op_id, op_result(&op_id, "done"))
            .unwrap();

        let feed = registry.completion_feed_handle();
        let mut observed_seq = 0;
        let mut last_injected_seq = 0;

        let injected = maybe_inject_feed_wake(
            &driver,
            Some(feed.as_ref()),
            &mut observed_seq,
            &mut last_injected_seq,
            None,
        )
        .await;

        assert!(
            injected,
            "feed-backed path should inject inline when quiescent"
        );
        assert_eq!(observed_seq, feed.watermark());
        assert_eq!(last_injected_seq, feed.watermark());

        let mut guard = driver.lock().await;
        assert_eq!(guard.as_driver().active_input_ids().len(), 1);
        let (_input_id, input) = guard
            .dequeue_next()
            .expect("continuation should be queued inline");
        match input {
            Input::Continuation(continuation) => {
                assert_eq!(continuation.reason, "detached_background_op_completed");
                assert_eq!(
                    continuation.handling_mode,
                    meerkat_core::types::HandlingMode::Steer
                );
            }
            other => panic!("expected inline continuation injection, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn maybe_inject_feed_wake_without_feed_is_noop() {
        let driver = make_shared_ephemeral_driver("no-feed");
        let mut observed_seq = 0;
        let mut last_injected_seq = 0;

        let injected = maybe_inject_feed_wake(
            &driver,
            None,
            &mut observed_seq,
            &mut last_injected_seq,
            None,
        )
        .await;

        assert!(!injected, "no feed means no injection");
        assert_eq!(observed_seq, 0);
        assert_eq!(last_injected_seq, 0);

        let guard = driver.lock().await;
        assert!(
            guard.as_driver().active_input_ids().is_empty(),
            "no feed must not enqueue anything"
        );
    }

    #[tokio::test]
    async fn maybe_inject_feed_wake_no_bg_completions_advances_observed_seq() {
        let driver = make_shared_ephemeral_driver("advance-only");
        let registry = crate::ops_lifecycle::RuntimeOpsLifecycleRegistry::new();
        let feed = registry.completion_feed_handle();
        let mut observed_seq = 0;
        let mut last_injected_seq = 0;

        let injected = maybe_inject_feed_wake(
            &driver,
            Some(feed.as_ref()),
            &mut observed_seq,
            &mut last_injected_seq,
            None,
        )
        .await;

        assert!(!injected, "empty feed should not inject");
        assert_eq!(observed_seq, feed.watermark());
        assert_eq!(last_injected_seq, 0);

        let guard = driver.lock().await;
        assert!(guard.as_driver().active_input_ids().is_empty());
    }

    // --- execution_kind stamping tests ---

    #[test]
    fn primitive_from_prompt_has_content_turn() {
        let input = make_prompt("hello");
        let id = input.id().clone();
        let primitive =
            input_to_primitive(&input, id).expect("single input metadata cannot conflict");
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
        let primitive =
            input_to_primitive(&input, id).expect("single input metadata cannot conflict");
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
            payload: None,
            blocks: None,
            handling_mode: None,
        });
        let id = input.id().clone();
        let primitive =
            input_to_primitive(&input, id).expect("single input metadata cannot conflict");
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
            payload: None,
            blocks: None,
            handling_mode: None,
        });
        let continuation =
            Input::Continuation(ContinuationInput::detached_background_op_completed());
        let inputs = vec![
            (peer.id().clone(), peer),
            (continuation.id().clone(), continuation),
        ];
        let primitive = inputs_to_primitive_with_boundary(&inputs, RunApplyBoundary::RunCheckpoint)
            .expect("mixed test batch metadata should not conflict");
        let meta = primitive
            .turn_metadata()
            .expect("should have turn_metadata");
        assert_eq!(
            meta.execution_kind,
            Some(meerkat_core::lifecycle::RuntimeExecutionKind::ContentTurn),
            "mixed batch must default to ContentTurn"
        );
    }

    #[test]
    fn batch_metadata_conflict_surfaces_typed_error() {
        let mut first = make_prompt("first");
        if let Input::Prompt(prompt) = &mut first {
            prompt.turn_metadata = Some(
                meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                    model: Some(meerkat_core::lifecycle::run_primitive::ModelId::new(
                        "model-a",
                    )),
                    ..Default::default()
                },
            );
        }
        let mut second = make_prompt("second");
        if let Input::Prompt(prompt) = &mut second {
            prompt.turn_metadata = Some(
                meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                    model: Some(meerkat_core::lifecycle::run_primitive::ModelId::new(
                        "model-b",
                    )),
                    ..Default::default()
                },
            );
        }
        let inputs = vec![(first.id().clone(), first), (second.id().clone(), second)];

        let err = try_inputs_to_primitive_with_boundary(
            &inputs,
            RunApplyBoundary::RunStart,
            Some(meerkat_core::lifecycle::RuntimeExecutionKind::ContentTurn),
        )
        .expect_err("conflicting batch metadata should be rejected");

        assert_eq!(err.field, "model");
    }
}
