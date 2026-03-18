//! Comms inbox drain task.
//!
//! Standalone tokio task that drains `CommsRuntime` inbox and feeds typed
//! `Input` values into `RuntimeSessionAdapter`. Replaces the old
//! `RuntimeCommsBridge` (comms_sink.rs) which implemented the now-removed
//! `RuntimeInputSink` trait on `meerkat-core`.

use std::sync::Arc;

use meerkat_core::agent::CommsRuntime;
use meerkat_core::event::AgentEvent;
use meerkat_core::interaction::{ClassifiedInboxInteraction, PeerInputClass};
use meerkat_core::lifecycle::RunControlCommand;
use meerkat_core::types::SessionId;

use crate::comms_bridge::interaction_to_runtime_input;
use crate::completion::CompletionOutcome;
use crate::identifiers::LogicalRuntimeId;
use crate::input::{ContinuationInput, Input};
use crate::service_ext::SessionServiceRuntimeExt as _;
use crate::session_adapter::RuntimeSessionAdapter;
use crate::tokio::sync::mpsc;

/// Spawn a background task that drains the comms inbox and routes
/// classified interactions through the runtime adapter.
///
/// The task runs until the comms runtime signals DISMISS, the budget
/// is exhausted (via adapter), or the returned `JoinHandle` is aborted.
pub fn spawn_comms_drain(
    adapter: Arc<RuntimeSessionAdapter>,
    session_id: SessionId,
    comms_runtime: Arc<dyn CommsRuntime>,
) -> crate::tokio::task::JoinHandle<()> {
    let runtime_id = LogicalRuntimeId::new(session_id.to_string());

    crate::tokio::spawn(async move {
        let inbox_notify = comms_runtime.inbox_notify();

        loop {
            // Wait for inbox activity.
            let notified = inbox_notify.notified();

            // Try classified drain first; fall back to legacy.
            let classified = match comms_runtime.drain_classified_inbox_interactions().await {
                Ok(v) if !v.is_empty() => v,
                Ok(_) => {
                    // Check DISMISS on empty drain.
                    if comms_runtime.dismiss_received() {
                        tracing::info!("comms_drain: DISMISS received, stopping");
                        let _ = adapter
                            .stop_runtime_executor(
                                &session_id,
                                RunControlCommand::StopRuntimeExecutor {
                                    reason: "peer DISMISS".to_string(),
                                },
                            )
                            .await;
                        return;
                    }
                    notified.await;
                    continue;
                }
                Err(_) => {
                    // Legacy runtime — drain unclassified interactions.
                    let interactions = comms_runtime.drain_inbox_interactions().await;
                    if interactions.is_empty() {
                        if comms_runtime.dismiss_received() {
                            tracing::info!("comms_drain: DISMISS received (legacy), stopping");
                            let _ = adapter
                                .stop_runtime_executor(
                                    &session_id,
                                    RunControlCommand::StopRuntimeExecutor {
                                        reason: "peer DISMISS".to_string(),
                                    },
                                )
                                .await;
                            return;
                        }
                        notified.await;
                        continue;
                    }
                    // Wrap as ActionableMessage for routing.
                    interactions
                        .into_iter()
                        .map(|interaction| ClassifiedInboxInteraction {
                            class: PeerInputClass::ActionableMessage,
                            interaction,
                            lifecycle_peer: None,
                        })
                        .collect()
                }
            };

            // Route each classified interaction through the adapter.
            for ci in classified {
                match ci.class {
                    PeerInputClass::PeerLifecycleAdded
                    | PeerInputClass::PeerLifecycleRetired
                    | PeerInputClass::Ack => {
                        // Lifecycle and ack are handled by the inner agent loop's
                        // drain_comms_inbox at turn boundaries. Skip here.
                    }
                    PeerInputClass::Response => {
                        // Terminal response — inject continuation.
                        let request_id = match &ci.interaction.content {
                            meerkat_core::interaction::InteractionContent::Response {
                                in_reply_to,
                                ..
                            } => Some(in_reply_to.0.to_string()),
                            _ => None,
                        };
                        let input = Input::Continuation(
                            ContinuationInput::terminal_peer_response_for_request(
                                "terminal peer response injected into session state",
                                request_id,
                            ),
                        );
                        if let Err(err) = adapter.accept_input(&session_id, input).await {
                            tracing::warn!(
                                error = %err,
                                "comms_drain: failed to inject terminal response continuation"
                            );
                        }
                    }
                    PeerInputClass::SilentRequest
                    | PeerInputClass::ActionableMessage
                    | PeerInputClass::ActionableRequest
                    | PeerInputClass::PlainEvent => {
                        // Route through the adapter as a peer input.
                        let interaction_id = ci.interaction.id;
                        let subscriber = comms_runtime.interaction_subscriber(&interaction_id);
                        let input = interaction_to_runtime_input(&ci.interaction, &runtime_id);
                        let result = adapter
                            .accept_input_with_completion(&session_id, input)
                            .await;

                        match result {
                            Ok((_outcome, handle)) => {
                                if subscriber.is_some() || handle.is_some() {
                                    spawn_completion_bridge(
                                        Some(comms_runtime.clone()),
                                        interaction_id,
                                        subscriber,
                                        handle,
                                    );
                                } else {
                                    comms_runtime.mark_interaction_complete(&interaction_id);
                                }
                            }
                            Err(err) => {
                                tracing::warn!(
                                    error = %err,
                                    "comms_drain: failed to accept peer input"
                                );
                                comms_runtime.mark_interaction_complete(&interaction_id);
                            }
                        }
                    }
                }
            }
        }
    })
}

/// Bridge between a completion handle and the comms interaction lifecycle.
fn spawn_completion_bridge(
    comms_runtime: Option<Arc<dyn CommsRuntime>>,
    interaction_id: meerkat_core::interaction::InteractionId,
    subscriber: Option<mpsc::Sender<AgentEvent>>,
    handle: Option<crate::completion::CompletionHandle>,
) {
    crate::tokio::spawn(async move {
        let outcome = match handle {
            Some(handle) => handle.wait().await,
            None => CompletionOutcome::CompletedWithoutResult,
        };

        if let Some(tx) = subscriber {
            let event = match outcome {
                CompletionOutcome::Completed(result) => AgentEvent::InteractionComplete {
                    interaction_id,
                    result: result.text,
                },
                CompletionOutcome::CompletedWithoutResult => AgentEvent::InteractionComplete {
                    interaction_id,
                    result: String::new(),
                },
                CompletionOutcome::Abandoned(reason)
                | CompletionOutcome::RuntimeTerminated(reason) => AgentEvent::InteractionFailed {
                    interaction_id,
                    error: reason,
                },
            };

            let _ = crate::tokio::time::timeout(std::time::Duration::from_secs(5), tx.send(event))
                .await;
        }

        if let Some(runtime) = comms_runtime {
            runtime.mark_interaction_complete(&interaction_id);
        }
    });
}
