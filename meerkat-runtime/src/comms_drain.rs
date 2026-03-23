//! Comms inbox drain task.
//!
//! Standalone tokio task that drains `CommsRuntime` inbox and feeds typed
//! `Input` values into `RuntimeSessionAdapter`. Replaces the old
//! `RuntimeCommsBridge` (comms_sink.rs) which implemented the now-removed
//! `RuntimeInputSink` trait on `meerkat-core`.

use std::sync::Arc;
use std::time::Duration;

use meerkat_core::agent::CommsRuntime;
use meerkat_core::event::AgentEvent;
use meerkat_core::interaction::{
    ClassifiedInboxInteraction, InboxInteraction, InteractionContent, PeerInputClass,
};
use meerkat_core::lifecycle::RunControlCommand;
use meerkat_core::types::SessionId;

use meerkat_core::comms_drain_lifecycle_authority::DrainExitReason;

use crate::comms_bridge::classified_interaction_to_runtime_input;
use crate::completion::CompletionOutcome;
use crate::identifiers::LogicalRuntimeId;
use crate::input::{ContinuationInput, Input};
use crate::service_ext::SessionServiceRuntimeExt as _;
use crate::session_adapter::RuntimeSessionAdapter;
use crate::tokio::sync::mpsc;

/// Default idle timeout for the comms drain loop (5 minutes).
pub const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(300);

fn classify_legacy_interaction(interaction: InboxInteraction) -> ClassifiedInboxInteraction {
    let (class, lifecycle_peer) = match &interaction.content {
        InteractionContent::Request { intent, params } if intent == "mob.peer_added" => (
            PeerInputClass::PeerLifecycleAdded,
            Some(
                params
                    .get("peer")
                    .and_then(serde_json::Value::as_str)
                    .filter(|peer| !peer.is_empty())
                    .unwrap_or(interaction.from.as_str())
                    .to_string(),
            ),
        ),
        InteractionContent::Request { intent, params } if intent == "mob.peer_retired" => (
            PeerInputClass::PeerLifecycleRetired,
            Some(
                params
                    .get("peer")
                    .and_then(serde_json::Value::as_str)
                    .filter(|peer| !peer.is_empty())
                    .unwrap_or(interaction.from.as_str())
                    .to_string(),
            ),
        ),
        InteractionContent::Response { .. } => (PeerInputClass::Response, None),
        InteractionContent::Request { .. } => (PeerInputClass::ActionableRequest, None),
        InteractionContent::Message { .. } => (PeerInputClass::ActionableMessage, None),
    };

    ClassifiedInboxInteraction {
        class,
        interaction,
        lifecycle_peer,
    }
}

/// Spawn a background task that drains the comms inbox and routes
/// classified interactions through the runtime adapter.
///
/// The task runs until the comms runtime signals DISMISS, the idle timeout
/// expires without inbox activity, or the returned `JoinHandle` is aborted.
///
/// `idle_timeout` controls how long the drain waits for inbox notifications
/// before checking whether the session should exit (e.g. budget exhausted).
/// Pass `None` to use [`DEFAULT_IDLE_TIMEOUT`].
pub fn spawn_comms_drain(
    adapter: Arc<RuntimeSessionAdapter>,
    session_id: SessionId,
    comms_runtime: Arc<dyn CommsRuntime>,
    idle_timeout: Option<Duration>,
) -> crate::tokio::task::JoinHandle<()> {
    let timeout_dur = idle_timeout.unwrap_or(DEFAULT_IDLE_TIMEOUT);
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
                            adapter
                                .notify_comms_drain_exited(&session_id, DrainExitReason::Dismissed)
                                .await;
                            return;
                        }
                        if crate::tokio::time::timeout(timeout_dur, notified)
                            .await
                            .is_err()
                        {
                            tracing::info!("comms_drain: idle timeout expired (legacy), stopping");
                            adapter
                                .notify_comms_drain_exited(
                                    &session_id,
                                    DrainExitReason::IdleTimeout,
                                )
                                .await;
                            return;
                        }
                        continue;
                    }
                    interactions
                        .into_iter()
                        .map(classify_legacy_interaction)
                        .collect()
                }
            };

            // Route each classified interaction through the adapter.
            for ci in classified {
                match ci.class {
                    PeerInputClass::Ack => {
                        // Ack envelopes are filtered at ingress. Skip here.
                    }
                    PeerInputClass::PeerLifecycleAdded | PeerInputClass::PeerLifecycleRetired => {
                        // Lifecycle events must be injected as session context
                        // so the LLM knows when peers connect/disconnect. The
                        // inner agent loop's drain_comms_inbox is suppressed in
                        // host mode, so comms_drain owns this injection.
                        let input = classified_interaction_to_runtime_input(&ci, &runtime_id);
                        if let Err(err) = adapter.accept_input(&session_id, input).await {
                            tracing::warn!(
                                error = %err,
                                "comms_drain: failed to inject peer lifecycle context"
                            );
                        }
                    }
                    PeerInputClass::Response => {
                        // Distinguish progress responses from terminal responses.
                        let (is_terminal, request_id) = match &ci.interaction.content {
                            meerkat_core::interaction::InteractionContent::Response {
                                in_reply_to,
                                status,
                                ..
                            } => {
                                let terminal = matches!(
                                    status,
                                    meerkat_core::interaction::ResponseStatus::Completed
                                        | meerkat_core::interaction::ResponseStatus::Failed
                                );
                                (terminal, Some(in_reply_to.0.to_string()))
                            }
                            _ => (false, None),
                        };

                        if is_terminal {
                            // Terminal response — first inject the response
                            // content so the LLM can reason over the peer's
                            // result, then inject a continuation to advance
                            // the runtime.
                            let content_input =
                                classified_interaction_to_runtime_input(&ci, &runtime_id);
                            if let Err(err) = adapter.accept_input(&session_id, content_input).await
                            {
                                tracing::warn!(
                                    error = %err,
                                    "comms_drain: failed to inject terminal response content"
                                );
                            }
                            let continuation = Input::Continuation(
                                ContinuationInput::terminal_peer_response_for_request(
                                    "terminal peer response injected into session state",
                                    request_id,
                                ),
                            );
                            if let Err(err) = adapter.accept_input(&session_id, continuation).await
                            {
                                tracing::warn!(
                                    error = %err,
                                    "comms_drain: failed to inject terminal response continuation"
                                );
                            }
                        } else {
                            // Progress response — route as peer input for checkpoint-style handling.
                            let input = classified_interaction_to_runtime_input(&ci, &runtime_id);
                            if let Err(err) = adapter.accept_input(&session_id, input).await {
                                tracing::warn!(
                                    error = %err,
                                    "comms_drain: failed to inject progress response"
                                );
                            }
                        }
                    }
                    PeerInputClass::SilentRequest
                    | PeerInputClass::ActionableMessage
                    | PeerInputClass::ActionableRequest
                    | PeerInputClass::PlainEvent => {
                        // Route through the adapter as a peer input.
                        let interaction_id = ci.interaction.id;
                        let subscriber = comms_runtime.interaction_subscriber(&interaction_id);
                        let input = classified_interaction_to_runtime_input(&ci, &runtime_id);
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

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use meerkat_core::interaction::{InteractionId, ResponseStatus};
    use serde_json::json;
    use uuid::Uuid;

    fn legacy_interaction(content: InteractionContent) -> InboxInteraction {
        InboxInteraction {
            id: InteractionId(Uuid::new_v4()),
            from: "peer-a".to_string(),
            content,
            rendered_text: "rendered".to_string(),
        }
    }

    #[test]
    fn legacy_response_preserves_response_class() {
        let classified =
            classify_legacy_interaction(legacy_interaction(InteractionContent::Response {
                in_reply_to: InteractionId(Uuid::new_v4()),
                status: ResponseStatus::Completed,
                result: json!({"ok": true}),
            }));

        assert_eq!(classified.class, PeerInputClass::Response);
        assert!(classified.lifecycle_peer.is_none());
    }

    #[test]
    fn legacy_peer_added_preserves_lifecycle_class_and_peer() {
        let classified =
            classify_legacy_interaction(legacy_interaction(InteractionContent::Request {
                intent: "mob.peer_added".to_string(),
                params: json!({"peer": "peer-b"}),
            }));

        assert_eq!(classified.class, PeerInputClass::PeerLifecycleAdded);
        assert_eq!(classified.lifecycle_peer.as_deref(), Some("peer-b"));
    }

    #[test]
    fn legacy_peer_retired_falls_back_to_sender_for_peer_name() {
        let classified =
            classify_legacy_interaction(legacy_interaction(InteractionContent::Request {
                intent: "mob.peer_retired".to_string(),
                params: json!({}),
            }));

        assert_eq!(classified.class, PeerInputClass::PeerLifecycleRetired);
        assert_eq!(classified.lifecycle_peer.as_deref(), Some("peer-a"));
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
