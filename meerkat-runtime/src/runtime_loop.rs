//! RuntimeLoop — per-session tokio task that processes queued inputs.
//!
//! When `RuntimeSessionAdapter::accept_input()` queues an input and sets
//! the wake flag, it sends a signal on the wake channel. The RuntimeLoop
//! picks it up, dequeues the input, converts it to a `RunPrimitive`,
//! and applies it via the `CoreExecutor` (which calls `SessionService::start_turn()`
//! under the hood).

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
        Input::ExternalEvent(e) => {
            if let Some(blocks) = &e.blocks {
                let text = meerkat_core::types::text_content(blocks);
                if text.is_empty() {
                    format!("[External Event: {}]", e.event_type)
                } else {
                    format!("[External Event: {}] {}", e.event_type, text)
                }
            } else {
                format!("[External Event: {}] {}", e.event_type, e.payload)
            }
        }
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
            // Prepend the text body when it carries content not already in blocks,
            // so multimodal messages keep the text explaining the attached media.
            let raw_blocks = p.blocks.clone().unwrap_or_default();
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
        Input::FlowStep(f) if f.blocks.is_some() => CoreRenderable::Blocks {
            blocks: f.blocks.clone().unwrap_or_default(),
        },
        Input::ExternalEvent(e) if e.blocks.is_some() => {
            let mut blocks = vec![meerkat_core::types::ContentBlock::Text {
                text: external_event_projection_text(e),
            }];
            blocks.extend(e.blocks.clone().unwrap_or_default());
            CoreRenderable::Blocks { blocks }
        }
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

fn external_event_projection_text(event: &crate::input::ExternalEventInput) -> String {
    let label = format!("[External Event: {}]", event.event_type);
    let body = event
        .payload
        .get("body")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|body| !body.is_empty());

    match body {
        Some(body) => format!("{label} {body}"),
        None => label,
    }
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
pub(crate) fn spawn_runtime_loop_with_completions(
    driver: crate::session_adapter::SharedDriver,
    mut executor: Box<dyn meerkat_core::lifecycle::CoreExecutor>,
    mut wake_rx: tokio::sync::mpsc::Receiver<()>,
    mut control_rx: tokio::sync::mpsc::Receiver<
        meerkat_core::lifecycle::run_control::RunControlCommand,
    >,
    completions: Option<crate::session_adapter::SharedCompletionRegistry>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
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
                        }
                        None => break,
                    }
                }
            }
        }

        // Loop exiting — resolve any pending completion waiters as terminated.
        // This ensures callers don't hang if the runtime shuts down mid-work.
        if let Some(ref completions) = completions {
            let mut reg = completions.lock().await;
            reg.resolve_all_terminated("runtime loop exited");
        }
    })
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
                        // Capture run_result before moving output fields
                        let run_result = output.run_result;

                        // BoundaryApplied transitions Staged → Applied → APC
                        // and triggers atomic persistence in PersistentRuntimeDriver
                        if let Err(err) = d
                            .as_driver_mut()
                            .on_run_event(RunEvent::BoundaryApplied {
                                run_id: run_id.clone(),
                                receipt: output.receipt,
                                session_snapshot: output.session_snapshot,
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
                            match run_result {
                                Some(result) => {
                                    for input_id in &input_ids {
                                        reg.resolve_completed(input_id, result.clone());
                                    }
                                }
                                None => {
                                    for input_id in &input_ids {
                                        reg.resolve_without_result(input_id);
                                    }
                                }
                            }
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
#[allow(clippy::unwrap_used, clippy::panic)]
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
            blocks: Some(blocks),
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
                assert_eq!(got.len(), 2);
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
                assert_eq!(got.len(), 3);
                assert_eq!(
                    got[0],
                    meerkat_core::types::ContentBlock::Text {
                        text: "[External Event: webhook] see this event".into(),
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
                assert_eq!(got.len(), 2);
                assert_eq!(
                    got[0],
                    meerkat_core::types::ContentBlock::Text {
                        text: "[External Event: webhook] see attached screenshot".into(),
                    }
                );
                assert_eq!(got[1], blocks[0]);
            }
            other => return Err(format!("expected blocks content, got {other:?}")),
        }
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
}
