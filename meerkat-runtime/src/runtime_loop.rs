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
    let prompt = input_to_prompt(input);
    RunPrimitive::StagedInput(StagedRunInput {
        boundary: RunApplyBoundary::RunStart,
        appends: vec![ConversationAppend {
            role: ConversationAppendRole::User,
            content: CoreRenderable::Text { text: prompt },
        }],
        context_appends: vec![],
        contributing_input_ids: vec![input_id],
    })
}

/// Spawn the per-session runtime loop.
///
/// Returns a `JoinHandle` that runs until the wake channel closes.
/// The loop dequeues inputs from the driver, converts them to `RunPrimitive`,
/// and applies them via the `CoreExecutor`.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn spawn_runtime_loop(
    driver: crate::session_adapter::SharedDriver,
    mut executor: Box<dyn meerkat_core::lifecycle::CoreExecutor>,
    mut wake_rx: tokio::sync::mpsc::Receiver<()>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(()) = wake_rx.recv().await {
            process_queue(&driver, &mut *executor).await;
        }
    })
}

/// Process all queued inputs until the queue is empty.
#[cfg(not(target_arch = "wasm32"))]
async fn process_queue(
    driver: &crate::session_adapter::SharedDriver,
    executor: &mut dyn meerkat_core::lifecycle::CoreExecutor,
) {
    loop {
        // Dequeue and prepare under the driver lock
        let dequeued = {
            let mut d = driver.lock().await;

            // Only process if idle
            if !d.is_idle() {
                break;
            }

            match d.dequeue_next() {
                Some((input_id, input)) => {
                    let run_id = RunId::new();

                    // Start run in the state machine
                    if d.start_run(run_id.clone()).is_err() {
                        break;
                    }

                    // Stage the input (Queued → Staged)
                    if d.stage_input(&input_id, &run_id).is_err() {
                        // Rollback: return to Idle
                        let _ = d.complete_run();
                        break;
                    }

                    // Apply the input (Staged → Applied → AppliedPendingConsumption)
                    if d.apply_input(&input_id, &run_id).is_err() {
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
                let result = executor.apply(primitive).await;

                // Lock again to update driver state
                let mut d = driver.lock().await;
                match result {
                    Ok(_receipt) => {
                        let _ = d
                            .as_driver_mut()
                            .on_run_event(RunEvent::RunCompleted {
                                run_id,
                                consumed_input_ids: vec![input_id],
                            })
                            .await;
                    }
                    Err(e) => {
                        let _ = d
                            .as_driver_mut()
                            .on_run_event(RunEvent::RunFailed {
                                run_id,
                                error: e.to_string(),
                                recoverable: true,
                            })
                            .await;
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
}
