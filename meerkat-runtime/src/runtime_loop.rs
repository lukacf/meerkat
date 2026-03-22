//! RuntimeLoop — per-session tokio task that processes queued inputs.
//!
//! When `RuntimeSessionAdapter::accept_input()` queues an input and sets
//! the wake flag, it sends a signal on the wake channel. The RuntimeLoop
//! picks it up, dequeues the input, converts it to a `RunPrimitive`,
//! and applies it via the `CoreExecutor` (which calls `SessionService::start_turn()`
//! under the hood).

use meerkat_core::lifecycle::InputId;
use meerkat_core::lifecycle::run_primitive::{
    ConversationAppend, ConversationAppendRole, CoreRenderable, RunApplyBoundary, RunPrimitive,
    StagedRunInput,
};

use crate::input::Input;
use crate::tokio;

/// Extract a prompt string from an `Input`.
pub(crate) fn input_to_prompt(input: &Input) -> String {
    match input {
        Input::Prompt(p) => p.text.clone(),
        Input::Peer(p) => p.body.clone(),
        Input::FlowStep(f) => f.instructions.clone(),
        Input::ExternalEvent(e) => {
            if let Some(ref blocks) = e.blocks {
                // Multimodal: extract text from block content.
                let text = meerkat_core::types::text_content(blocks);
                format!("[External Event: {}] {text}", e.event_type)
            } else if let Some(text) = e.payload.get("text").and_then(|v| v.as_str()) {
                // Text-only: extract the canonical "text" field from payload.
                format!("[External Event: {}] {text}", e.event_type)
            } else {
                // Arbitrary payload (e.g. REST-originated events) — render raw JSON.
                format!("[External Event: {}] {}", e.event_type, e.payload)
            }
        }
        Input::Continuation(c) => format!("[Continuation] {}", c.reason),
        Input::TerminalPeerResponse(i) => {
            if let Some(ref blocks) = i.blocks {
                let text = meerkat_core::types::text_content(blocks);
                format!("[Terminal Peer Response] {text}")
            } else {
                format!("[Terminal Peer Response] {}", i.body)
            }
        }
        Input::Operation(operation) => {
            format!(
                "[Operation {}] {:?}",
                operation.operation_id, operation.event
            )
        }
    }
}

/// Derive boundary classification from an `Input` variant.
///
/// This is the **shell-level** derivation — useful for tests that construct
/// inputs directly. Production code uses the ingress authority's stored
/// boundary instead (via `prepare_admitted_run`).
#[cfg(test)]
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
        Input::TerminalPeerResponse(_) => RunApplyBoundary::RunStart,
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
        _ => RunApplyBoundary::RunStart,
    }
}

fn input_turn_metadata(
    input: &Input,
) -> Option<meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata> {
    match input {
        Input::Prompt(prompt) => prompt.turn_metadata.clone(),
        Input::FlowStep(flow_step) => flow_step.turn_metadata.clone(),
        Input::Continuation(continuation) => Some(
            meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                handling_mode: Some(continuation.handling_mode),
                ..Default::default()
            },
        ),
        Input::TerminalPeerResponse(_) => Some(
            meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                handling_mode: Some(meerkat_core::types::HandlingMode::Steer),
                ..Default::default()
            },
        ),
        _ => None,
    }
}

fn input_to_append(input: &Input) -> Option<ConversationAppend> {
    let content = match input {
        Input::Prompt(p) if p.blocks.is_some() => CoreRenderable::Blocks {
            blocks: p.blocks.clone().unwrap_or_default(),
        },
        Input::Peer(p) if p.blocks.is_some() => CoreRenderable::Blocks {
            blocks: p.blocks.clone().unwrap_or_default(),
        },
        Input::ExternalEvent(e) if e.blocks.is_some() => CoreRenderable::Blocks {
            blocks: e.blocks.clone().unwrap_or_default(),
        },
        Input::TerminalPeerResponse(i) if i.blocks.is_some() => CoreRenderable::Blocks {
            blocks: i.blocks.clone().unwrap_or_default(),
        },
        Input::Prompt(_)
        | Input::Peer(_)
        | Input::FlowStep(_)
        | Input::ExternalEvent(_)
        | Input::TerminalPeerResponse(_) => CoreRenderable::Text {
            text: input_to_prompt(input),
        },
        Input::Continuation(_) | Input::Operation(_) => return None,
    };

    Some(ConversationAppend {
        role: ConversationAppendRole::User,
        content,
    })
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
    let turn_metadata = inputs
        .iter()
        .find_map(|(_, input)| input_turn_metadata(input));

    RunPrimitive::StagedInput(StagedRunInput {
        boundary,
        appends,
        context_appends: vec![],
        contributing_input_ids,
        turn_metadata,
    })
}

/// Build a `RunPrimitive` using the shell-level `input_boundary()` derivation.
///
/// Test-only: production code uses `prepare_admitted_run` which gets the
/// boundary from the ingress authority instead.
#[cfg(test)]
pub(crate) fn inputs_to_primitive(inputs: &[(InputId, Input)]) -> RunPrimitive {
    let boundary = inputs
        .first()
        .map(|(_, input)| input_boundary(input))
        .unwrap_or(RunApplyBoundary::RunStart);
    inputs_to_primitive_with_boundary(inputs, boundary)
}

/// Convert an `Input` + its ID to a `RunPrimitive` for `CoreExecutor::apply()`.
///
/// Test-only: production code uses `prepare_admitted_run`.
#[cfg(test)]
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
        let prepared = {
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
            let dequeued: Vec<_> = batch_ids
                .iter()
                .filter_map(|id| d.dequeue_by_id(id))
                .collect();

            if dequeued.is_empty() {
                return false;
            }

            // Shared helper: start_run → stage → build primitive (with authority boundary).
            match d.prepare_admitted_run(&dequeued) {
                Ok(prepared) => Some(prepared),
                Err(_) => return false,
            }
        };

        match prepared {
            Some((input_ids, run_id, primitive)) => {
                // Execute outside the driver lock (this calls start_turn, which is slow)
                let result = executor.apply(run_id.clone(), primitive).await;

                // Lock again to update driver state
                let mut d = driver.lock().await;
                match result {
                    Ok(output) => {
                        match d
                            .record_run_success(run_id.clone(), output, input_ids.clone())
                            .await
                        {
                            Ok(run_result) => {
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
                            Err(record_err) => {
                                let (_err_run_id, err_msg) = match &record_err {
                                    crate::session_adapter::RecordRunError::BoundaryCommitFailed {
                                        run_id, source, unwind_err,
                                    } => {
                                        tracing::error!(%run_id, error = %source, "failed to record boundary-applied event");
                                        if let Some(ue) = unwind_err {
                                            tracing::error!(%run_id, error = %ue, "failed to unwind runtime after boundary commit failure");
                                        }
                                        (run_id.clone(), format!("runtime boundary commit failed for run {run_id}: {source}"))
                                    }
                                    crate::session_adapter::RecordRunError::CompletionFailed {
                                        run_id, source,
                                    } => {
                                        tracing::error!(error = %source, "failed to record run-completed event");
                                        (run_id.clone(), format!("runtime terminal snapshot failed after completion: {source}"))
                                    }
                                };
                                drop(d);
                                let _ = executor
                                    .control(meerkat_core::lifecycle::run_control::RunControlCommand::StopRuntimeExecutor {
                                        reason: err_msg,
                                    })
                                    .await;
                                return false;
                            }
                        }
                    }
                    Err(e) => {
                        let error_msg = e.to_string();
                        // RunFailed rolls back Staged → Queued and returns to Idle
                        if let Err(err) = d.record_run_failure(run_id, error_msg.clone()).await {
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
    fn input_to_primitive_creates_staged() {
        let input = make_prompt("test prompt");
        let input_id = input.id().clone();
        let primitive = input_to_primitive(&input, input_id.clone());

        match primitive {
            RunPrimitive::StagedInput(staged) => {
                assert_eq!(staged.boundary, RunApplyBoundary::RunStart);
                assert_eq!(staged.contributing_input_ids, vec![input_id]);
                assert_eq!(staged.appends.len(), 1);
                assert_eq!(staged.appends[0].role, ConversationAppendRole::User);
                match &staged.appends[0].content {
                    CoreRenderable::Text { text } => assert_eq!(text, "test prompt"),
                    _ => panic!("Expected Text content"),
                }
            }
            _ => panic!("Expected StagedInput"),
        }
    }

    #[test]
    fn peer_input_with_blocks_produces_blocks_renderable() {
        let blocks = vec![
            meerkat_core::types::ContentBlock::Text {
                text: "see this image".into(),
            },
            meerkat_core::types::ContentBlock::Image {
                media_type: "image/png".into(),
                data: "abc123".into(),
                source_path: None,
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

        match primitive {
            RunPrimitive::StagedInput(staged) => {
                assert_eq!(staged.appends.len(), 1);
                match &staged.appends[0].content {
                    CoreRenderable::Blocks { blocks: got } => {
                        assert_eq!(got.len(), 2);
                    }
                    other => panic!("Expected Blocks content, got {other:?}"),
                }
            }
            _ => panic!("Expected StagedInput"),
        }
    }
}
