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
use meerkat_core::lifecycle::core_executor::{
    CoreApplyOutput, CoreExecutor, CoreExecutorControl, CoreExecutorError,
};
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

struct SessionRuntimeControlHandle {
    runtime: Arc<SessionRuntime>,
    session_id: SessionId,
}

#[async_trait::async_trait]
impl CoreExecutorControl for SessionRuntimeControlHandle {
    async fn control(&self, command: RunControlCommand) -> Result<(), CoreExecutorError> {
        if command.should_interrupt_current_run() {
            return self
                .runtime
                .interrupt(&self.session_id)
                .await
                .map_err(|e| CoreExecutorError::control_failed_runtime(e.message));
        }
        if matches!(command, RunControlCommand::InterruptYielding) {
            return self
                .runtime
                .interrupt_yielding(&self.session_id)
                .await
                .map_err(|e| CoreExecutorError::control_failed_runtime(e.message));
        }
        Ok(())
    }
}

#[cfg(feature = "mob")]
struct MobRpcRuntimeControlHandle {
    session_service: Arc<dyn MobSessionService>,
    session_id: SessionId,
}

#[cfg(feature = "mob")]
#[async_trait::async_trait]
impl CoreExecutorControl for MobRpcRuntimeControlHandle {
    async fn control(&self, command: RunControlCommand) -> Result<(), CoreExecutorError> {
        if command.should_interrupt_current_run() {
            return self
                .session_service
                .interrupt(&self.session_id)
                .await
                .map_err(|err| CoreExecutorError::control_failed_runtime(err.to_string()));
        }
        if matches!(command, RunControlCommand::InterruptYielding) {
            return self
                .session_service
                .cancel_after_boundary(&self.session_id)
                .await
                .or_else(|err| match err {
                    SessionError::NotRunning { .. } | SessionError::Unsupported(_) => Ok(()),
                    err => Err(err),
                })
                .map_err(|err| CoreExecutorError::control_failed_runtime(err.to_string()));
        }
        Ok(())
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
    fn control_handle(&self) -> Option<Arc<dyn CoreExecutorControl>> {
        Some(Arc::new(SessionRuntimeControlHandle {
            runtime: Arc::clone(&self.runtime),
            session_id: self.session_id.clone(),
        }))
    }

    async fn apply(
        &mut self,
        run_id: meerkat_core::lifecycle::RunId,
        primitive: RunPrimitive,
    ) -> Result<CoreApplyOutput, CoreExecutorError> {
        if let Some(reason) = primitive.peer_response_terminal_apply_intent_violation() {
            return Err(CoreExecutorError::apply_failed_primitive_rejected(
                reason.to_string(),
            ));
        }

        // Context-only staged primitives may land directly as runtime
        // system-context appends, but terminal peer responses carry a typed
        // apply intent that requires a requester reaction turn.
        if primitive.is_context_only_apply_without_turn() {
            let RunPrimitive::StagedInput(staged) = &primitive else {
                unreachable!("context-only apply without turn only matches staged primitives");
            };
            return self
                .runtime
                .apply_runtime_context_appends_via_service(
                    &self.session_id,
                    run_id,
                    pending_system_context_appends_from_primitive(&staged.context_appends),
                    primitive.apply_boundary(),
                    staged.contributing_input_ids.clone(),
                )
                .await
                .map_err(|err| CoreExecutorError::apply_failed_runtime_context(err.to_string()));
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
            Err(rpc_err) => Err(CoreExecutorError::apply_failed_runtime_turn(
                rpc_err.message,
            )),
        }
    }

    async fn control(&mut self, command: RunControlCommand) -> Result<(), CoreExecutorError> {
        match command {
            command if command.should_interrupt_current_run() => self
                .runtime
                .interrupt(&self.session_id)
                .await
                .map_err(|e| CoreExecutorError::control_failed_runtime(e.message)),
            RunControlCommand::InterruptYielding => self
                .runtime
                .interrupt_yielding(&self.session_id)
                .await
                .map_err(|e| CoreExecutorError::control_failed_runtime(e.message)),
            RunControlCommand::StopRuntimeExecutor { .. } => {
                let discard_result = self.runtime.discard_live_session(&self.session_id).await;
                self.runtime
                    .runtime_adapter()
                    .unregister_session(&self.session_id)
                    .await;
                match discard_result {
                    Ok(()) | Err(SessionError::NotFound { .. }) => Ok(()),
                    Err(err) => Err(CoreExecutorError::control_failed_runtime(err.to_string())),
                }
            }
            _ => Ok(()),
        }
    }
}

#[cfg(feature = "mob")]
#[async_trait::async_trait]
impl CoreExecutor for MobRpcRuntimeExecutor {
    fn control_handle(&self) -> Option<Arc<dyn CoreExecutorControl>> {
        Some(Arc::new(MobRpcRuntimeControlHandle {
            session_service: Arc::clone(&self.session_service),
            session_id: self.session_id.clone(),
        }))
    }

    async fn apply(
        &mut self,
        run_id: meerkat_core::lifecycle::RunId,
        primitive: RunPrimitive,
    ) -> Result<CoreApplyOutput, CoreExecutorError> {
        if let Some(reason) = primitive.peer_response_terminal_apply_intent_violation() {
            return Err(CoreExecutorError::apply_failed_primitive_rejected(
                reason.to_string(),
            ));
        }

        if primitive.is_context_only_apply_without_turn() {
            let RunPrimitive::StagedInput(staged) = &primitive else {
                unreachable!("context-only apply without turn only matches staged primitives");
            };
            return self
                .session_service
                .apply_runtime_context_appends_with_boundary(
                    &self.session_id,
                    run_id,
                    pending_system_context_appends_from_primitive(&staged.context_appends),
                    primitive.apply_boundary(),
                    staged.contributing_input_ids.clone(),
                )
                .await
                .map_err(|err| CoreExecutorError::apply_failed_runtime_context(err.to_string()));
        }

        let prompt = primitive.extract_content_input();
        let pre_turn_context_appends = match &primitive {
            RunPrimitive::StagedInput(staged)
                if primitive.is_peer_response_terminal_context_and_run() =>
            {
                pending_system_context_appends_from_primitive(&staged.context_appends)
            }
            _ => Vec::new(),
        };
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
            pre_turn_context_appends,
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
        result.map_err(|err| CoreExecutorError::apply_failed_runtime_turn(err.to_string()))
    }

    async fn control(&mut self, command: RunControlCommand) -> Result<(), CoreExecutorError> {
        match command {
            command if command.should_interrupt_current_run() => self
                .session_service
                .interrupt(&self.session_id)
                .await
                .map_err(|e| CoreExecutorError::control_failed_runtime(e.to_string())),
            RunControlCommand::InterruptYielding => self
                .session_service
                .cancel_after_boundary(&self.session_id)
                .await
                .or_else(|err| match err {
                    SessionError::NotRunning { .. } | SessionError::Unsupported(_) => Ok(()),
                    err => Err(err),
                })
                .map_err(|e| CoreExecutorError::control_failed_runtime(e.to_string())),
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
                    Err(err) => Err(CoreExecutorError::control_failed_runtime(err.to_string())),
                }
            }
            _ => Ok(()),
        }
    }
}
