//! SessionRuntimeExecutor — implements `CoreExecutor` by delegating to `SessionRuntime`.
//!
//! This is the bridge between the v9 runtime layer and the existing RPC session
//! management. When the RuntimeLoop processes a queued input, it calls
//! `CoreExecutor::apply()` on this executor, which translates the `RunPrimitive`
//! into a machine-owned `SessionRuntime::apply_runtime_turn_with_pre_admission()` call.

use std::sync::Arc;

use meerkat_core::EventEnvelope;
use meerkat_core::PendingSystemContextAppend;
use meerkat_core::event::AgentEvent;
use meerkat_core::lifecycle::core_executor::{
    CoreApplyFailureCause, CoreApplyFailureCauseKind, CoreApplyOutput, CoreExecutor,
    CoreExecutorBoundaryHandle, CoreExecutorError, CoreExecutorInterruptHandle,
};
use meerkat_core::lifecycle::run_primitive::{CoreRenderable, RunPrimitive};
use meerkat_core::service::{SessionError, SessionService};
use meerkat_core::types::SessionId;
use tokio::sync::mpsc;

use crate::router::NotificationSink;
use crate::session_runtime::SessionRuntime;
use crate::{error, protocol::RpcError};
#[cfg(feature = "mob")]
use meerkat_mob::MobSessionService;

/// Implements `CoreExecutor` by delegating to the runtime-owned apply path.
///
/// Each session gets its own executor instance, which is owned by the
/// RuntimeLoop task. The executor extracts the prompt from the `RunPrimitive`
/// and calls the runtime-owned apply path, forwarding events to the RPC
/// notification sink.
pub struct SessionRuntimeExecutor {
    runtime: Arc<SessionRuntime>,
    session_id: SessionId,
}

#[cfg(feature = "mob")]
/// Implements `CoreExecutor` directly against a mob-capable session service.
pub struct MobRpcRuntimeExecutor {
    session_service: Arc<dyn MobSessionService>,
    runtime: Option<Arc<SessionRuntime>>,
    session_id: SessionId,
    notification_sink: NotificationSink,
}

#[cfg(feature = "mob")]
impl MobRpcRuntimeExecutor {
    pub fn new(
        session_service: Arc<dyn MobSessionService>,
        runtime: Option<Arc<SessionRuntime>>,
        session_id: SessionId,
        notification_sink: NotificationSink,
    ) -> Self {
        Self {
            session_service,
            runtime,
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

struct SessionRuntimeBoundaryHandle {
    runtime: Arc<SessionRuntime>,
    session_id: SessionId,
}

#[async_trait::async_trait]
impl CoreExecutorBoundaryHandle for SessionRuntimeBoundaryHandle {
    async fn cancel_after_boundary(&self, _reason: String) -> Result<(), CoreExecutorError> {
        self.runtime
            .cancel_after_boundary_live_with_machine_authority(&self.session_id)
            .await
            .or_else(|err| match err {
                SessionError::NotRunning { .. } => Ok(()),
                err => Err(err),
            })
            .map_err(|err| CoreExecutorError::control_failed_runtime(err.to_string()))
    }
}

struct SessionRuntimeInterruptHandle {
    runtime: Arc<SessionRuntime>,
    session_id: SessionId,
}

#[async_trait::async_trait]
impl CoreExecutorInterruptHandle for SessionRuntimeInterruptHandle {
    async fn hard_cancel_current_run(&self, _reason: String) -> Result<(), CoreExecutorError> {
        self.runtime
            .interrupt_live_with_machine_authority(&self.session_id)
            .await
            .or_else(|err| match err {
                SessionError::NotRunning { .. } => Ok(()),
                err => Err(err),
            })
            .map_err(|err| CoreExecutorError::control_failed_runtime(err.to_string()))
    }
}

#[cfg(feature = "mob")]
struct MobRpcRuntimeBoundaryHandle {
    session_service: Arc<dyn MobSessionService>,
    runtime: Option<Arc<SessionRuntime>>,
    session_id: SessionId,
}

#[cfg(feature = "mob")]
#[async_trait::async_trait]
impl CoreExecutorBoundaryHandle for MobRpcRuntimeBoundaryHandle {
    async fn cancel_after_boundary(&self, _reason: String) -> Result<(), CoreExecutorError> {
        if let Some(runtime) = self.runtime.as_ref() {
            return runtime
                .cancel_after_boundary_live_with_machine_authority(&self.session_id)
                .await
                .or_else(|err| match err {
                    SessionError::NotRunning { .. } => Ok(()),
                    err => Err(err),
                })
                .map_err(|err| CoreExecutorError::control_failed_runtime(err.to_string()));
        }
        if let Some(adapter) = self.session_service.runtime_adapter() {
            return self
                .session_service
                .cancel_after_boundary_with_machine_authority(
                    &self.session_id,
                    adapter.session_control_authority(),
                )
                .await
                .or_else(|err| match err {
                    SessionError::NotRunning { .. } => Ok(()),
                    err => Err(err),
                })
                .map_err(|err| CoreExecutorError::control_failed_runtime(err.to_string()));
        }
        Err(CoreExecutorError::control_failed_runtime(format!(
            "mob runtime boundary cancel for session {} has no MeerkatMachine authority",
            self.session_id
        )))
    }
}

#[cfg(feature = "mob")]
struct MobRpcRuntimeInterruptHandle {
    session_service: Arc<dyn MobSessionService>,
    runtime: Option<Arc<SessionRuntime>>,
    session_id: SessionId,
}

#[cfg(feature = "mob")]
#[async_trait::async_trait]
impl CoreExecutorInterruptHandle for MobRpcRuntimeInterruptHandle {
    async fn hard_cancel_current_run(&self, _reason: String) -> Result<(), CoreExecutorError> {
        if let Some(runtime) = self.runtime.as_ref() {
            return runtime
                .interrupt_live_with_machine_authority(&self.session_id)
                .await
                .or_else(|err| match err {
                    SessionError::NotRunning { .. } => Ok(()),
                    err => Err(err),
                })
                .map_err(|err| CoreExecutorError::control_failed_runtime(err.to_string()));
        }
        if let Some(adapter) = self.session_service.runtime_adapter() {
            return self
                .session_service
                .interrupt_with_machine_authority(
                    &self.session_id,
                    adapter.session_control_authority(),
                )
                .await
                .or_else(|err| match err {
                    SessionError::NotRunning { .. } => Ok(()),
                    err => Err(err),
                })
                .map_err(|err| CoreExecutorError::control_failed_runtime(err.to_string()));
        }
        Err(CoreExecutorError::control_failed_runtime(format!(
            "mob runtime interrupt for session {} has no MeerkatMachine authority",
            self.session_id
        )))
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

fn core_executor_error_from_rpc(err: RpcError) -> CoreExecutorError {
    if err.code == error::REQUEST_CANCELLED {
        return CoreExecutorError::cancelled();
    }

    let kind = err
        .data
        .as_ref()
        .and_then(|data| data.get("core_apply_failure_cause"))
        .and_then(serde_json::Value::as_str)
        .and_then(CoreApplyFailureCauseKind::from_wire_str)
        .or((err.code == error::HOOK_DENIED).then_some(CoreApplyFailureCauseKind::HookDenied));
    let message = err.message;
    let cause = match kind {
        Some(CoreApplyFailureCauseKind::HookDenied) => CoreApplyFailureCause::hook_denied(message),
        Some(CoreApplyFailureCauseKind::HookRuntimeFailure) => {
            CoreApplyFailureCause::hook_runtime_failure(message)
        }
        Some(kind) => CoreApplyFailureCause::new(kind, message),
        None => CoreApplyFailureCause::runtime_turn(message),
    };
    CoreExecutorError::apply_failed(cause)
}

#[async_trait::async_trait]
impl CoreExecutor for SessionRuntimeExecutor {
    fn boundary_handle(&self) -> Option<Arc<dyn CoreExecutorBoundaryHandle>> {
        Some(Arc::new(SessionRuntimeBoundaryHandle {
            runtime: Arc::clone(&self.runtime),
            session_id: self.session_id.clone(),
        }))
    }

    fn interrupt_handle(&self) -> Option<Arc<dyn CoreExecutorInterruptHandle>> {
        Some(Arc::new(SessionRuntimeInterruptHandle {
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
            let pre_admission = self
                .runtime
                .take_runtime_pre_admission(&self.session_id, &staged.contributing_input_ids);
            return self
                .runtime
                .apply_runtime_context_appends_via_service(
                    &self.session_id,
                    run_id,
                    pending_system_context_appends_from_primitive(&staged.context_appends),
                    primitive.apply_boundary(),
                    staged.contributing_input_ids.clone(),
                    pre_admission,
                )
                .await
                .map_err(|err| CoreExecutorError::apply_failed_runtime_context(err.to_string()));
        }

        #[cfg(test)]
        self.runtime
            .wait_runtime_routed_pre_promotion_hook(&self.session_id)
            .await;

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
        let pre_admission = self
            .runtime
            .take_runtime_pre_admission(&self.session_id, primitive.contributing_input_ids());
        let result = Box::pin(
            self.runtime.apply_runtime_turn_with_pre_admission(
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
                pre_admission,
            ),
        )
        .await;

        // Wait for the event forwarder to finish
        let _ = forwarder.await;

        match result {
            Ok(output) => Ok(output),
            Err(rpc_err) => Err(core_executor_error_from_rpc(rpc_err)),
        }
    }

    async fn cancel_after_boundary(&mut self, _reason: String) -> Result<(), CoreExecutorError> {
        self.runtime
            .cancel_after_boundary_live_with_machine_authority(&self.session_id)
            .await
            .or_else(|err| match err {
                SessionError::NotRunning { .. } => Ok(()),
                err => Err(err),
            })
            .map_err(|err| CoreExecutorError::control_failed_runtime(err.to_string()))
    }

    async fn stop_runtime_executor(&mut self, _reason: String) -> Result<(), CoreExecutorError> {
        Ok(())
    }

    async fn cleanup_after_runtime_stop_terminalized(&mut self) -> Result<(), CoreExecutorError> {
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
}

#[cfg(feature = "mob")]
#[async_trait::async_trait]
impl CoreExecutor for MobRpcRuntimeExecutor {
    fn boundary_handle(&self) -> Option<Arc<dyn CoreExecutorBoundaryHandle>> {
        Some(Arc::new(MobRpcRuntimeBoundaryHandle {
            session_service: Arc::clone(&self.session_service),
            runtime: self.runtime.clone(),
            session_id: self.session_id.clone(),
        }))
    }

    fn interrupt_handle(&self) -> Option<Arc<dyn CoreExecutorInterruptHandle>> {
        Some(Arc::new(MobRpcRuntimeInterruptHandle {
            session_service: Arc::clone(&self.session_service),
            runtime: self.runtime.clone(),
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
            let pre_admission = self.runtime.as_ref().and_then(|runtime| {
                runtime.take_runtime_pre_admission(&self.session_id, &staged.contributing_input_ids)
            });
            if let Some(runtime) = self.runtime.as_ref() {
                return runtime
                    .apply_runtime_context_appends_via_service(
                        &self.session_id,
                        run_id,
                        pending_system_context_appends_from_primitive(&staged.context_appends),
                        primitive.apply_boundary(),
                        staged.contributing_input_ids.clone(),
                        pre_admission,
                    )
                    .await
                    .map_err(|err| {
                        CoreExecutorError::apply_failed_runtime_context(err.to_string())
                    });
            }
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

        let turn_metadata = primitive.turn_metadata().cloned();
        let skill_references = turn_metadata
            .as_ref()
            .and_then(|meta| meta.skill_references.clone());
        let flow_tool_overlay = turn_metadata
            .as_ref()
            .and_then(|meta| meta.flow_tool_overlay.clone());
        let pre_admission = self.runtime.as_ref().and_then(|runtime| {
            runtime.take_runtime_pre_admission(&self.session_id, primitive.contributing_input_ids())
        });
        let result = match (self.runtime.as_ref(), pre_admission) {
            (Some(runtime), Some(pre_admission)) => {
                let turn_overrides =
                    crate::session_runtime::SessionRuntime::turn_overrides_from_metadata(
                        primitive.turn_metadata(),
                    );
                runtime
                    .apply_runtime_turn_with_pre_admission(
                        &self.session_id,
                        run_id,
                        &primitive,
                        prompt,
                        event_tx,
                        skill_references,
                        flow_tool_overlay,
                        None,
                        turn_overrides,
                        Some(pre_admission),
                    )
                    .await
                    .map_err(core_executor_error_from_rpc)
            }
            (_, pre_admission) => {
                drop(pre_admission);
                let req = meerkat_core::service::StartTurnRequest {
                    prompt,
                    system_prompt: None,
                    event_tx: Some(event_tx),
                    runtime: meerkat_core::service::StartTurnRuntimeSemantics::new(
                        None,
                        meerkat_core::types::HandlingMode::Queue,
                        skill_references,
                        flow_tool_overlay,
                        pre_turn_context_appends,
                        turn_metadata,
                    ),
                };
                self.session_service
                    .apply_runtime_turn(
                        &self.session_id,
                        run_id,
                        req,
                        primitive.apply_boundary(),
                        primitive.contributing_input_ids().to_vec(),
                    )
                    .await
                    .map_err(CoreExecutorError::apply_failed_from_session_error)
            }
        };

        let _ = forwarder.await;
        result
    }

    async fn cancel_after_boundary(&mut self, _reason: String) -> Result<(), CoreExecutorError> {
        if let Some(runtime) = self.runtime.as_ref() {
            return runtime
                .cancel_after_boundary_live_with_machine_authority(&self.session_id)
                .await
                .or_else(|err| match err {
                    SessionError::NotRunning { .. } => Ok(()),
                    err => Err(err),
                })
                .map_err(|e| CoreExecutorError::control_failed_runtime(e.to_string()));
        }
        if let Some(adapter) = self.session_service.runtime_adapter() {
            return self
                .session_service
                .cancel_after_boundary_with_machine_authority(
                    &self.session_id,
                    adapter.session_control_authority(),
                )
                .await
                .or_else(|err| match err {
                    SessionError::NotRunning { .. } => Ok(()),
                    err => Err(err),
                })
                .map_err(|e| CoreExecutorError::control_failed_runtime(e.to_string()));
        }
        Err(CoreExecutorError::control_failed_runtime(format!(
            "mob runtime boundary cancel for session {} has no MeerkatMachine authority",
            self.session_id
        )))
    }

    async fn stop_runtime_executor(&mut self, _reason: String) -> Result<(), CoreExecutorError> {
        Ok(())
    }

    async fn cleanup_after_runtime_stop_terminalized(&mut self) -> Result<(), CoreExecutorError> {
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
}

#[cfg(all(test, feature = "mob"))]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use meerkat_core::RunResult;
    use meerkat_core::service::{
        AppendSystemContextRequest, AppendSystemContextResult, CreateSessionRequest,
        SessionControlError, SessionHistoryPage, SessionHistoryQuery, SessionQuery,
        SessionServiceCommsExt, SessionServiceControlExt, SessionServiceHistoryExt, SessionSummary,
        SessionView, StartTurnRequest,
    };

    enum BoundaryCancelOutcome {
        Unsupported,
        NotRunning,
    }

    struct BoundaryCancelSessionService {
        outcome: BoundaryCancelOutcome,
    }

    impl BoundaryCancelSessionService {
        fn unsupported() -> Self {
            Self {
                outcome: BoundaryCancelOutcome::Unsupported,
            }
        }

        fn not_running() -> Self {
            Self {
                outcome: BoundaryCancelOutcome::NotRunning,
            }
        }
    }

    #[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
    impl SessionService for BoundaryCancelSessionService {
        async fn create_session(
            &self,
            _req: CreateSessionRequest,
        ) -> Result<RunResult, SessionError> {
            unreachable!("boundary-handle tests only call cancel_after_boundary")
        }

        async fn start_turn(
            &self,
            _id: &SessionId,
            _req: StartTurnRequest,
        ) -> Result<RunResult, SessionError> {
            unreachable!("boundary-handle tests only call cancel_after_boundary")
        }

        async fn interrupt(&self, _id: &SessionId) -> Result<(), SessionError> {
            unreachable!("boundary-handle tests only call cancel_after_boundary")
        }

        async fn cancel_after_boundary(&self, id: &SessionId) -> Result<(), SessionError> {
            match self.outcome {
                BoundaryCancelOutcome::Unsupported => Err(SessionError::Unsupported(
                    "cancel_after_boundary".to_string(),
                )),
                BoundaryCancelOutcome::NotRunning => {
                    Err(SessionError::NotRunning { id: id.clone() })
                }
            }
        }

        async fn read(&self, _id: &SessionId) -> Result<SessionView, SessionError> {
            unreachable!("boundary-handle tests only call cancel_after_boundary")
        }

        async fn list(&self, _query: SessionQuery) -> Result<Vec<SessionSummary>, SessionError> {
            unreachable!("boundary-handle tests only call cancel_after_boundary")
        }

        async fn archive(&self, _id: &SessionId) -> Result<(), SessionError> {
            unreachable!("boundary-handle tests only call cancel_after_boundary")
        }
    }

    #[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
    impl SessionServiceCommsExt for BoundaryCancelSessionService {}

    #[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
    impl SessionServiceControlExt for BoundaryCancelSessionService {
        async fn append_system_context(
            &self,
            _id: &SessionId,
            _req: AppendSystemContextRequest,
        ) -> Result<AppendSystemContextResult, SessionControlError> {
            unreachable!("boundary-handle tests only call cancel_after_boundary")
        }
    }

    #[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
    impl SessionServiceHistoryExt for BoundaryCancelSessionService {
        async fn read_history(
            &self,
            _id: &SessionId,
            _query: SessionHistoryQuery,
        ) -> Result<SessionHistoryPage, SessionError> {
            unreachable!("boundary-handle tests only call cancel_after_boundary")
        }
    }

    #[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
    impl MobSessionService for BoundaryCancelSessionService {}

    #[tokio::test]
    async fn mob_rpc_boundary_handle_propagates_unsupported_boundary_cancel() {
        let session_id = SessionId::new();
        let session_service: Arc<dyn MobSessionService> =
            Arc::new(BoundaryCancelSessionService::unsupported());
        let handle = MobRpcRuntimeBoundaryHandle {
            session_service,
            runtime: None,
            session_id,
        };

        let error = handle
            .cancel_after_boundary("test boundary cancel".to_string())
            .await
            .expect_err("unsupported boundary cancel must not be masked as success");

        assert!(
            error
                .to_string()
                .contains("has no MeerkatMachine authority"),
            "unexpected boundary cancel error: {error:?}"
        );
    }

    #[tokio::test]
    async fn mob_rpc_boundary_handle_keeps_not_running_as_noop() {
        let session_id = SessionId::new();
        let session_service: Arc<dyn MobSessionService> =
            Arc::new(BoundaryCancelSessionService::not_running());
        let handle = MobRpcRuntimeBoundaryHandle {
            session_service,
            runtime: None,
            session_id,
        };

        let error = handle
            .cancel_after_boundary("test boundary cancel".to_string())
            .await
            .expect_err("not-running fallback must not bypass MeerkatMachine authority");
        assert!(
            error
                .to_string()
                .contains("has no MeerkatMachine authority"),
            "unexpected boundary cancel error: {error:?}"
        );
    }
}
