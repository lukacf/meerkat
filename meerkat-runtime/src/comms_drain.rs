//! Comms inbox drain task.
//!
//! Standalone tokio task that drains `CommsRuntime` inbox and feeds typed
//! `Input` values into `MeerkatMachine`. Replaces the old
//! `RuntimeCommsBridge` (comms_sink.rs) which implemented the now-removed
//! `RuntimeInputSink` trait on `meerkat-core`.

use std::sync::Arc;
use std::time::Duration;

use meerkat_core::agent::CommsRuntime;
use meerkat_core::event::AgentEvent;
use meerkat_core::interaction::PeerInputClass;
use meerkat_core::lifecycle::RunControlCommand;
use meerkat_core::types::SessionId;

use crate::comms_bridge::classified_interaction_to_runtime_input;
use crate::completion::CompletionOutcome;
use crate::identifiers::LogicalRuntimeId;
use crate::meerkat_machine::{DrainExitReason, MeerkatMachine};
use crate::service_ext::SessionServiceRuntimeExt as _;
use crate::tokio::sync::mpsc;

/// Default idle timeout for session-backed comms drains.
pub const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(300);

/// Spawn a background task that drains the comms inbox and routes
/// classified interactions through the runtime adapter.
///
/// The task runs until the comms runtime signals DISMISS or the returned
/// `JoinHandle` is aborted by the drain lifecycle authority.
pub fn spawn_comms_drain(
    adapter: Arc<MeerkatMachine>,
    session_id: SessionId,
    comms_runtime: Arc<dyn CommsRuntime>,
    idle_timeout: Option<Duration>,
) -> crate::tokio::task::JoinHandle<()> {
    let timeout_dur = idle_timeout.unwrap_or(DEFAULT_IDLE_TIMEOUT);
    let runtime_id = LogicalRuntimeId::new(session_id.to_string());

    crate::tokio::spawn(async move {
        let inbox_notify = comms_runtime.inbox_notify();

        loop {
            // Register BEFORE drain — notify_waiters() guarantees wakeup
            // from creation, so a message arriving between drain-returns-empty
            // and the await cannot be lost. No enable() needed.
            // (Mirrors the pattern in CommsRuntime::recv_message.)
            let notified = inbox_notify.notified();

            let candidates = comms_runtime.drain_peer_input_candidates().await;
            if candidates.is_empty() {
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
                    adapter
                        .notify_comms_drain_exited(&session_id, DrainExitReason::Dismissed)
                        .await;
                    return;
                }
                if crate::tokio::time::timeout(timeout_dur, notified)
                    .await
                    .is_err()
                {
                    tracing::info!("comms_drain: idle timeout expired, stopping");
                    adapter
                        .notify_comms_drain_exited(&session_id, DrainExitReason::IdleTimeout)
                        .await;
                    return;
                }
                continue;
            }

            // Route each classified interaction through the adapter.
            for candidate in candidates {
                match candidate.class {
                    PeerInputClass::Ack => {
                        // Ack envelopes are filtered at ingress. Skip here.
                    }
                    PeerInputClass::PeerLifecycleAdded
                    | PeerInputClass::PeerLifecycleRetired
                    | PeerInputClass::PeerLifecycleUnwired => {
                        // Lifecycle events must be injected as session context
                        // so the LLM knows when peers connect/disconnect.
                        // comms_drain is the sole keep-alive inbox consumer.
                        let input =
                            classified_interaction_to_runtime_input(&candidate, &runtime_id);
                        if let Err(err) = adapter.accept_input(&session_id, input).await {
                            tracing::warn!(
                                error = %err,
                                "comms_drain: failed to inject peer lifecycle context"
                            );
                        }
                    }
                    PeerInputClass::Response => {
                        // Distinguish progress responses from terminal responses.
                        let is_terminal = matches!(
                            &candidate.interaction.content,
                            meerkat_core::interaction::InteractionContent::Response {
                                status,
                                ..
                            } if matches!(
                                status,
                                meerkat_core::interaction::ResponseStatus::Completed
                                    | meerkat_core::interaction::ResponseStatus::Failed
                            )
                        );

                        if is_terminal {
                            // Terminal response — single admission with
                            // completion tracking. The PeerInput already
                            // carries ResponseTerminal convention which the
                            // policy table maps to WakeIfIdle/Steer as needed.
                            // No synthetic Continuation required.
                            let interaction_id = candidate.interaction.id;
                            let subscriber = comms_runtime.interaction_subscriber(&interaction_id);
                            let content_input =
                                classified_interaction_to_runtime_input(&candidate, &runtime_id);
                            let result = adapter
                                .accept_input_with_completion(&session_id, content_input)
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
                                        "comms_drain: failed to inject terminal response"
                                    );
                                    comms_runtime.mark_interaction_complete(&interaction_id);
                                }
                            }
                        } else {
                            // Progress response — route as peer input for checkpoint-style handling.
                            let input =
                                classified_interaction_to_runtime_input(&candidate, &runtime_id);
                            if let Err(err) = adapter.accept_input(&session_id, input).await {
                                tracing::warn!(
                                    error = %err,
                                    "comms_drain: failed to inject progress response"
                                );
                            }
                        }
                    }
                    PeerInputClass::SilentRequest
                    | PeerInputClass::PeerLifecycleKickoffFailed
                    | PeerInputClass::PeerLifecycleKickoffCancelled
                    | PeerInputClass::ActionableMessage
                    | PeerInputClass::ActionableRequest
                    | PeerInputClass::PlainEvent => {
                        // Route through the adapter as a peer input.
                        let interaction_id = candidate.interaction.id;
                        let subscriber = comms_runtime.interaction_subscriber(&interaction_id);
                        let input =
                            classified_interaction_to_runtime_input(&candidate, &runtime_id);
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

fn interaction_terminal_event(
    interaction_id: meerkat_core::interaction::InteractionId,
    outcome: CompletionOutcome,
) -> AgentEvent {
    match outcome {
        CompletionOutcome::Completed(result) => AgentEvent::InteractionComplete {
            interaction_id,
            result: result.text,
        },
        CompletionOutcome::CompletedWithoutResult => AgentEvent::InteractionComplete {
            interaction_id,
            result: String::new(),
        },
        CompletionOutcome::CallbackPending { tool_name, args } => {
            AgentEvent::InteractionCallbackPending {
                interaction_id,
                tool_name,
                args,
            }
        }
        CompletionOutcome::Abandoned(reason) | CompletionOutcome::RuntimeTerminated(reason) => {
            AgentEvent::InteractionFailed {
                interaction_id,
                error: reason,
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use meerkat_core::InteractionId;
    use serde_json::json;
    use uuid::Uuid;

    #[test]
    fn callback_pending_maps_to_interaction_callback_pending_terminal_event() {
        let interaction_id = InteractionId(Uuid::new_v4());
        let event = interaction_terminal_event(
            interaction_id,
            CompletionOutcome::CallbackPending {
                tool_name: "external_mock".to_string(),
                args: json!({ "value": "browser" }),
            },
        );

        assert!(
            matches!(event, AgentEvent::InteractionCallbackPending { .. }),
            "expected callback-pending interaction event"
        );
        if let AgentEvent::InteractionCallbackPending {
            interaction_id: actual_id,
            tool_name,
            args,
        } = event
        {
            assert_eq!(actual_id, interaction_id);
            assert_eq!(tool_name, "external_mock");
            assert_eq!(args, json!({ "value": "browser" }));
        }
    }
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
            let event = interaction_terminal_event(interaction_id, outcome);

            if crate::tokio::time::timeout(std::time::Duration::from_secs(5), tx.send(event))
                .await
                .is_err()
            {
                tracing::warn!(
                    %interaction_id,
                    "completion bridge dropped terminal event: subscriber send timed out after 5s"
                );
            }
        }

        if let Some(runtime) = comms_runtime {
            runtime.mark_interaction_complete(&interaction_id);
        }
    });
}
