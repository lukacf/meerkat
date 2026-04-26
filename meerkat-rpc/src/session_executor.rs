//! SessionRuntimeExecutor — implements `CoreExecutor` by delegating to `SessionRuntime`.
//!
//! This is the bridge between the v9 runtime layer and the existing RPC session
//! management. When the RuntimeLoop processes a queued input, it calls
//! `CoreExecutor::apply()` on this executor, which translates the `RunPrimitive`
//! into a `SessionRuntime::start_turn()` call.

use std::sync::Arc;

use meerkat_core::EventEnvelope;
use meerkat_core::PendingSystemContextAppend;
use meerkat_core::event::AgentEvent;
use meerkat_core::lifecycle::core_executor::{CoreApplyOutput, CoreExecutor, CoreExecutorError};
use meerkat_core::lifecycle::run_control::RunControlCommand;
use meerkat_core::lifecycle::run_primitive::{CoreRenderable, RunPrimitive};
use meerkat_core::service::SessionError;
use meerkat_core::types::SessionId;
use tokio::sync::mpsc;

use crate::router::NotificationSink;
use crate::session_runtime::SessionRuntime;
#[cfg(feature = "mob")]
use meerkat_mob::MobSessionService;

/// Implements `CoreExecutor` by delegating to `SessionRuntime::start_turn()`.
///
/// Each session gets its own executor instance, which is owned by the
/// RuntimeLoop task. The executor extracts the prompt from the `RunPrimitive`
/// and calls `start_turn`, forwarding events to the RPC notification sink.
pub struct SessionRuntimeExecutor {
    runtime: Arc<SessionRuntime>,
    session_id: SessionId,
}

#[cfg(feature = "mob")]
/// Implements `CoreExecutor` directly against a mob-capable session service.
pub struct MobRpcRuntimeExecutor {
    session_service: Arc<dyn MobSessionService>,
    session_id: SessionId,
    notification_sink: NotificationSink,
}

#[cfg(feature = "mob")]
impl MobRpcRuntimeExecutor {
    pub fn new(
        session_service: Arc<dyn MobSessionService>,
        session_id: SessionId,
        notification_sink: NotificationSink,
    ) -> Self {
        Self {
            session_service,
            session_id,
            notification_sink,
        }
    }
}

impl SessionRuntimeExecutor {
    /// Create a new executor for a specific session.
    ///
    /// The notification sink is NOT captured here — the executor reads the
    /// current sink from the runtime at apply time so reconnected TCP clients
    /// always get events routed to the live transport.
    pub fn new(runtime: Arc<SessionRuntime>, session_id: SessionId) -> Self {
        Self {
            runtime,
            session_id,
        }
    }
}

fn render_context_append_text(content: &CoreRenderable) -> String {
    match content {
        CoreRenderable::Text { text } => text.clone(),
        CoreRenderable::Blocks { blocks } => meerkat_core::types::text_content(blocks),
        CoreRenderable::Json { value } => {
            serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
        }
        CoreRenderable::Reference { uri, label } => match label {
            Some(label) if !label.trim().is_empty() => format!("[Reference] {label} ({uri})"),
            _ => format!("[Reference] {uri}"),
        },
        _ => String::new(),
    }
}

fn pending_system_context_appends_from_primitive(
    appends: &[meerkat_core::lifecycle::run_primitive::ConversationContextAppend],
) -> Vec<PendingSystemContextAppend> {
    let accepted_at = meerkat_core::time_compat::SystemTime::now();
    appends
        .iter()
        .map(|append| PendingSystemContextAppend {
            text: render_context_append_text(&append.content),
            source: Some(append.key.clone()),
            idempotency_key: Some(append.key.clone()),
            accepted_at,
        })
        .collect()
}

#[async_trait::async_trait]
impl CoreExecutor for SessionRuntimeExecutor {
    async fn apply(
        &mut self,
        run_id: meerkat_core::lifecycle::RunId,
        primitive: RunPrimitive,
    ) -> Result<CoreApplyOutput, CoreExecutorError> {
        // Context-only-immediate primitives (e.g. peer_response_terminal
        // with handling_mode=None) must land as runtime system-context
        // appends on the session transcript, not trigger a turn. Without
        // this gate, the authoritative peer terminal result is dropped
        // and subsequent turns (including realtime recalls on the
        // reconstructed session state) cannot see the peer-returned
        // token/status.
        // A context-only staged primitive — no conversation appends, only
        // context appends (e.g. peer_response_terminal) — must not trigger a
        // turn. Route it through `apply_runtime_context_appends` regardless
        // of the apply boundary: the runtime boundary for peer terminal
        // responses is Steer-derived (RunCheckpoint), so the core
        // `is_context_only_immediate` gate (boundary == Immediate) does not
        // match. Keep the stricter gate for explicit immediate short-circuits
        // used elsewhere; here we only need to detect that no turn work is
        // implied by this primitive.
        if let RunPrimitive::StagedInput(staged) = &primitive
            && staged.appends.is_empty()
            && !staged.context_appends.is_empty()
        {
            return self
                .runtime
                .apply_runtime_context_appends_via_service(
                    &self.session_id,
                    run_id,
                    pending_system_context_appends_from_primitive(&staged.context_appends),
                    staged.contributing_input_ids.clone(),
                )
                .await
                .map_err(|err| CoreExecutorError::ApplyFailed {
                    reason: err.to_string(),
                });
        }
        let prompt = primitive.extract_content_input();

        // Create an event channel and forward events to the notification sink.
        // Read the CURRENT sink from the runtime (not the one captured at
        // executor creation) so reconnected TCP clients get events.
        let (event_tx, mut event_rx) = mpsc::channel::<EventEnvelope<AgentEvent>>(128);
        let sink = self.runtime.current_notification_sink();
        let sid = self.session_id.clone();
        let forwarder = tokio::spawn(async move {
            while let Some(event) = event_rx.recv().await {
                sink.emit_event(&sid, &event).await;
            }
        });

        let turn_overrides = crate::session_runtime::SessionRuntime::turn_overrides_from_metadata(
            primitive.turn_metadata(),
        );
        let result = Box::pin(
            self.runtime.apply_runtime_turn(
                &self.session_id,
                run_id,
                &primitive,
                prompt,
                event_tx,
                primitive
                    .turn_metadata()
                    .and_then(|meta| meta.skill_references.clone()),
                primitive
                    .turn_metadata()
                    .and_then(|meta| meta.flow_tool_overlay.clone()),
                None,
                turn_overrides,
            ),
        )
        .await;

        // Wait for the event forwarder to finish
        let _ = forwarder.await;

        match result {
            Ok(output) => Ok(output),
            Err(rpc_err) => Err(CoreExecutorError::ApplyFailed {
                reason: rpc_err.message,
            }),
        }
    }

    async fn control(&mut self, command: RunControlCommand) -> Result<(), CoreExecutorError> {
        match command {
            RunControlCommand::CancelCurrentRun { .. } => self
                .runtime
                .interrupt(&self.session_id)
                .await
                .map_err(|e| CoreExecutorError::ControlFailed { reason: e.message }),
            RunControlCommand::StopRuntimeExecutor { .. } => {
                let discard_result = self.runtime.discard_live_session(&self.session_id).await;
                self.runtime
                    .runtime_adapter()
                    .unregister_session(&self.session_id)
                    .await;
                match discard_result {
                    Ok(()) | Err(SessionError::NotFound { .. }) => Ok(()),
                    Err(err) => Err(CoreExecutorError::ControlFailed {
                        reason: err.to_string(),
                    }),
                }
            }
            _ => Ok(()),
        }
    }
}

#[cfg(feature = "mob")]
#[async_trait::async_trait]
impl CoreExecutor for MobRpcRuntimeExecutor {
    async fn apply(
        &mut self,
        run_id: meerkat_core::lifecycle::RunId,
        primitive: RunPrimitive,
    ) -> Result<CoreApplyOutput, CoreExecutorError> {
        // A context-only staged primitive must not trigger a turn. See the
        // corresponding short-circuit in SessionRuntimeExecutor::apply for
        // why we check `appends.is_empty() && !context_appends.is_empty()`
        // instead of the stricter `is_context_only_immediate` gate.
        if let RunPrimitive::StagedInput(staged) = &primitive
            && staged.appends.is_empty()
            && !staged.context_appends.is_empty()
        {
            return self
                .session_service
                .apply_runtime_context_appends(
                    &self.session_id,
                    run_id,
                    pending_system_context_appends_from_primitive(&staged.context_appends),
                    staged.contributing_input_ids.clone(),
                )
                .await
                .map_err(|err| CoreExecutorError::ApplyFailed {
                    reason: err.to_string(),
                });
        }

        let prompt = primitive.extract_content_input();
        let (event_tx, mut event_rx) = mpsc::channel::<EventEnvelope<AgentEvent>>(128);
        let sink = self.notification_sink.clone();
        let sid = self.session_id.clone();
        let forwarder = tokio::spawn(async move {
            while let Some(event) = event_rx.recv().await {
                sink.emit_event(&sid, &event).await;
            }
        });

        let req = meerkat_core::service::StartTurnRequest {
            prompt,
            system_prompt: None,
            render_metadata: None,
            handling_mode: meerkat_core::types::HandlingMode::Queue,
            event_tx: Some(event_tx),
            skill_references: primitive
                .turn_metadata()
                .and_then(|meta| meta.skill_references.clone()),
            flow_tool_overlay: primitive
                .turn_metadata()
                .and_then(|meta| meta.flow_tool_overlay.clone()),
            turn_metadata: primitive.turn_metadata().cloned(),
        };

        let result = self
            .session_service
            .apply_runtime_turn(
                &self.session_id,
                run_id,
                req,
                primitive.apply_boundary(),
                primitive.contributing_input_ids().to_vec(),
            )
            .await;

        let _ = forwarder.await;
        result.map_err(|err| CoreExecutorError::ApplyFailed {
            reason: err.to_string(),
        })
    }

    async fn control(&mut self, command: RunControlCommand) -> Result<(), CoreExecutorError> {
        match command {
            RunControlCommand::CancelCurrentRun { .. } => self
                .session_service
                .interrupt(&self.session_id)
                .await
                .map_err(|e| CoreExecutorError::ControlFailed {
                    reason: e.to_string(),
                }),
            RunControlCommand::StopRuntimeExecutor { .. } => {
                let discard_result = self
                    .session_service
                    .discard_live_session(&self.session_id)
                    .await;
                if let Some(adapter) = self.session_service.runtime_adapter() {
                    adapter.unregister_session(&self.session_id).await;
                }
                match discard_result {
                    Ok(()) | Err(SessionError::NotFound { .. }) => Ok(()),
                    Err(err) => Err(CoreExecutorError::ControlFailed {
                        reason: err.to_string(),
                    }),
                }
            }
            _ => Ok(()),
        }
    }
}
