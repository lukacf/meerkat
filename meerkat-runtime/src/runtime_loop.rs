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
            format!("[External Event: {}] {}", e.event_type, e.payload)
        }
        Input::SystemGenerated(s) => s.content.clone(),
        Input::Projected(p) => p.content.clone(),
    }
}

/// Convert an `Input` + its ID to a `RunPrimitive` for `CoreExecutor::apply()`.
pub(crate) fn input_to_primitive(input: &Input, input_id: InputId) -> RunPrimitive {
    match input {
        Input::SystemGenerated(system) => RunPrimitive::StagedInput(StagedRunInput {
            boundary: RunApplyBoundary::Immediate,
            appends: vec![],
            context_appends: vec![
                meerkat_core::lifecycle::run_primitive::ConversationContextAppend {
                    key: format!("system-generated:{input_id}"),
                    content: CoreRenderable::Text {
                        text: system.content.clone(),
                    },
                },
            ],
            contributing_input_ids: vec![input_id],
            turn_metadata: None,
        }),
        _ => {
            // Use multimodal blocks when available (PromptInput/PeerInput with images),
            // otherwise fall back to text.
            let content = match input {
                Input::Prompt(p) if p.blocks.is_some() => CoreRenderable::Blocks {
                    blocks: p.blocks.clone().unwrap_or_default(),
                },
                Input::Peer(p) if p.blocks.is_some() => CoreRenderable::Blocks {
                    blocks: p.blocks.clone().unwrap_or_default(),
                },
                _ => CoreRenderable::Text {
                    text: input_to_prompt(input),
                },
            };
            let turn_metadata = match input {
                Input::Prompt(prompt) => prompt.turn_metadata.clone(),
                Input::FlowStep(flow_step) => flow_step.turn_metadata.clone(),
                _ => None,
            };
            RunPrimitive::StagedInput(StagedRunInput {
                boundary: RunApplyBoundary::RunStart,
                appends: vec![ConversationAppend {
                    role: ConversationAppendRole::User,
                    content,
                }],
                context_appends: vec![],
                contributing_input_ids: vec![input_id],
                turn_metadata,
            })
        }
    }
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
                maybe_wake = wake_rx.recv() => {
                    match maybe_wake {
                        Some(()) => process_queue(&driver, &mut *executor, completions.as_ref()).await,
                        None => break,
                    }
                }
                maybe_command = control_rx.recv() => {
                    match maybe_command {
                        Some(command) => {
                            let _ = executor.control(command).await;
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
    completions: Option<&crate::session_adapter::SharedCompletionRegistry>,
) {
    loop {
        // Dequeue and prepare under the driver lock
        let dequeued = {
            let mut d = driver.lock().await;

            // Only process if the runtime can process queue (Idle or Retired)
            if !d.can_process_queue() {
                break;
            }

            match d.dequeue_next() {
                Some((input_id, input)) => {
                    let run_id = RunId::new();

                    // Start run in the state machine
                    if d.start_run(run_id.clone()).is_err() {
                        break;
                    }

                    // Stage the input (Queued → Staged).
                    // Do NOT apply here — apply only after successful execution.
                    // If we pre-applied and the executor failed, the input would
                    // be stranded in AppliedPendingConsumption because RunFailed
                    // only rolls back Staged inputs.
                    if d.stage_input(&input_id, &run_id).is_err() {
                        let _ = d.complete_run();
                        break;
                    }

                    let primitive = input_to_primitive(&input, input_id.clone());
                    Some((input_id, run_id, primitive))
                }
                None => None,
            }
        };

        match dequeued {
            Some((input_id, run_id, primitive)) => {
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
                            break;
                        }

                        // RunCompleted transitions APC → Consumed and returns to Idle/Retired
                        if let Err(err) = d
                            .as_driver_mut()
                            .on_run_event(RunEvent::RunCompleted {
                                run_id,
                                consumed_input_ids: vec![input_id.clone()],
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
                            break;
                        }

                        // Resolve completion waiters unconditionally
                        if let Some(completions) = completions.as_ref() {
                            let mut reg = completions.lock().await;
                            match run_result {
                                Some(result) => {
                                    reg.resolve_completed(&input_id, result);
                                }
                                None => {
                                    reg.resolve_without_result(&input_id);
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
                                completions.lock().await.resolve_abandoned(
                                    &input_id,
                                    format!("runtime failure snapshot failed: {err}"),
                                );
                            }
                            break;
                        }
                        // Resolve completion waiter so callers don't hang.
                        if let Some(completions) = completions.as_ref() {
                            completions
                                .lock()
                                .await
                                .resolve_abandoned(&input_id, format!("apply failed: {error_msg}"));
                        }
                        let should_continue = d.take_wake_requested();
                        drop(d);
                        if should_continue {
                            continue;
                        }
                        // Leave the failing input queued for a future wake instead of
                        // hot-looping on the same payload indefinitely.
                        break;
                    }
                }
            }
            None => break, // Queue empty
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
