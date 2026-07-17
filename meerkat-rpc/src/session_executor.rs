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
    CoreExecutorPostStopCleanupHandle, CoreExecutorPublicationHandle, CoreExecutorTeardownReason,
    CoreExecutorTurnFinalizationBoundaryHandle, CoreExecutorTurnFinalizationGuard,
};
#[cfg(test)]
use meerkat_core::lifecycle::run_primitive::CoreRenderable;
use meerkat_core::lifecycle::run_primitive::RunPrimitive;
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
    async fn cancel_after_boundary(
        &self,
        expected_run_id: &meerkat_core::lifecycle::RunId,
        _reason: String,
    ) -> Result<(), CoreExecutorError> {
        self.runtime
            .cancel_after_boundary_live_with_machine_authority(&self.session_id, expected_run_id)
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

struct SessionRuntimePostStopCleanupHandle {
    runtime: Arc<SessionRuntime>,
    session_id: SessionId,
}

#[async_trait::async_trait]
impl CoreExecutorPostStopCleanupHandle for SessionRuntimePostStopCleanupHandle {
    async fn cleanup_after_runtime_stop_terminalized(&self) -> Result<(), CoreExecutorError> {
        self.runtime
            .discard_live_session_after_runtime_stop_terminalized(&self.session_id)
            .await
            .or_else(|err| match err {
                SessionError::NotFound { .. } => Ok(()),
                err => Err(err),
            })
            .map_err(|err| CoreExecutorError::control_failed_runtime(err.to_string()))
    }

    async fn cleanup_after_runtime_stop_terminalized_under_turn_finalization_boundary(
        &self,
    ) -> Result<(), CoreExecutorError> {
        self.runtime
            .discard_live_session_after_runtime_stop_terminalized_under_runtime_turn_boundary(
                &self.session_id,
            )
            .await
            .or_else(|err| match err {
                SessionError::NotFound { .. } => Ok(()),
                err => Err(err),
            })
            .map_err(|err| CoreExecutorError::control_failed_runtime(err.to_string()))
    }
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
    async fn cancel_after_boundary(
        &self,
        expected_run_id: &meerkat_core::lifecycle::RunId,
        _reason: String,
    ) -> Result<(), CoreExecutorError> {
        if let Some(runtime) = self.runtime.as_ref() {
            return runtime
                .cancel_after_boundary_live_with_machine_authority(
                    &self.session_id,
                    expected_run_id,
                )
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
                    expected_run_id,
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
struct MobRpcRuntimePublicationHandle {
    session_service: Arc<dyn MobSessionService>,
    session_id: SessionId,
}

#[cfg(feature = "mob")]
struct MobRpcRuntimePostStopCleanupHandle {
    session_service: Arc<dyn MobSessionService>,
    session_id: SessionId,
}

#[cfg(feature = "mob")]
struct MobRpcRuntimeTurnFinalizationBoundaryHandle {
    session_service: Arc<dyn MobSessionService>,
    session_id: SessionId,
}

#[cfg(feature = "mob")]
#[async_trait::async_trait]
impl CoreExecutorTurnFinalizationBoundaryHandle for MobRpcRuntimeTurnFinalizationBoundaryHandle {
    async fn acquire(
        &self,
    ) -> Result<Box<dyn CoreExecutorTurnFinalizationGuard>, CoreExecutorError> {
        self.session_service
            .acquire_runtime_turn_finalization_guard(&self.session_id)
            .await
            .map_err(CoreExecutorError::apply_failed_from_session_error)
    }
}

#[cfg(feature = "mob")]
#[async_trait::async_trait]
impl CoreExecutorPostStopCleanupHandle for MobRpcRuntimePostStopCleanupHandle {
    async fn cleanup_after_runtime_stop_terminalized(&self) -> Result<(), CoreExecutorError> {
        self.session_service
            .discard_live_session_after_runtime_stop_terminalized(&self.session_id)
            .await
            .or_else(|err| match err {
                SessionError::NotFound { .. } => Ok(()),
                err => Err(err),
            })
            .map_err(|err| CoreExecutorError::control_failed_runtime(err.to_string()))
    }

    async fn cleanup_after_runtime_stop_terminalized_under_turn_finalization_boundary(
        &self,
    ) -> Result<(), CoreExecutorError> {
        self.session_service
            .discard_live_session_after_runtime_stop_terminalized_under_turn_finalization_boundary(
                &self.session_id,
            )
            .await
            .or_else(|err| match err {
                SessionError::NotFound { .. } => Ok(()),
                err => Err(err),
            })
            .map_err(|err| CoreExecutorError::control_failed_runtime(err.to_string()))
    }
}

#[cfg(feature = "mob")]
#[async_trait::async_trait]
impl CoreExecutorPublicationHandle for MobRpcRuntimePublicationHandle {
    async fn publish_interaction_terminals(
        &self,
        events: &[AgentEvent],
    ) -> Result<
        Vec<meerkat_core::lifecycle::core_executor::CoreInteractionTerminalPublicationReceipt>,
        CoreExecutorError,
    > {
        self.session_service
            .publish_interaction_terminals(&self.session_id, events)
            .await
            .map_err(CoreExecutorError::apply_failed_from_session_error)
    }
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

#[cfg(test)]
mod typed_context_append_tests {
    use super::*;
    use meerkat_core::types::{
        SystemNoticeBlock, SystemNoticeDirection, SystemNoticeKind, SystemNoticeMessage,
    };

    #[test]
    fn context_system_notice_projects_only_via_notice_projection() {
        let blocks = vec![SystemNoticeBlock::Comms {
            sender_taint: None,
            kind: meerkat_core::types::CommsNoticeKind::ResponseTerminal,
            direction: SystemNoticeDirection::Incoming,
            peer: None,
            request_id: Some("req-1".to_string()),
            intent: Some("checksum_token".to_string()),
            status: Some("completed".to_string()),
            summary: Some("Peer terminal response".to_string()),
            payload: None,
            content: Vec::new(),
        }];
        let content = CoreRenderable::SystemNotice {
            kind: SystemNoticeKind::Comms,
            body: Some("Peer terminal response context".to_string()),
            blocks: blocks.clone(),
        };

        assert_eq!(
            content.render_text(),
            SystemNoticeMessage::with_blocks(
                SystemNoticeKind::Comms,
                Some("Peer terminal response context".to_string()),
                blocks,
            )
            .model_projection_text()
        );
    }

    #[test]
    fn rpc_teardown_marker_maps_through_closed_reason_enum() {
        let error = core_executor_error_from_rpc(RpcError {
            code: error::SESSION_NOT_FOUND,
            message: "archived session requires teardown".to_string(),
            data: Some(serde_json::json!({
                "core_executor_teardown_reason": CoreExecutorTeardownReason::ArchivedSession.as_str(),
            })),
        });

        assert!(matches!(
            error,
            CoreExecutorError::TeardownRequired {
                reason: CoreExecutorTeardownReason::ArchivedSession,
                ..
            }
        ));
    }

    #[test]
    fn malformed_rpc_teardown_marker_fails_closed_without_teardown() {
        let error = core_executor_error_from_rpc(RpcError {
            code: error::SESSION_NOT_FOUND,
            message: "malformed teardown marker".to_string(),
            data: Some(serde_json::json!({
                "core_executor_teardown_reason": "future-unrecognized-reason",
            })),
        });

        assert!(matches!(error, CoreExecutorError::ApplyFailed { .. }));
    }

    #[test]
    fn unmarked_owned_session_not_found_requests_unavailable_teardown() {
        let error = core_executor_error_from_rpc(RpcError {
            code: error::SESSION_NOT_FOUND,
            message: "owned session vanished".to_string(),
            data: None,
        });

        assert!(matches!(
            error,
            CoreExecutorError::TeardownRequired {
                reason: CoreExecutorTeardownReason::SessionUnavailable,
                ..
            }
        ));
    }
}

fn pending_system_context_appends_from_primitive(
    appends: &[meerkat_core::lifecycle::run_primitive::ConversationContextAppend],
) -> Vec<PendingSystemContextAppend> {
    let accepted_at = meerkat_core::time_compat::SystemTime::now();
    appends
        .iter()
        .map(|append| PendingSystemContextAppend {
            content: append.content.clone(),
            source: Some(append.key.clone()),
            idempotency_key: Some(append.key.clone()),
            accepted_at,
            source_kind: meerkat_core::session::SystemContextSource::Normal,
            peer_response_terminal: None,
        })
        .collect()
}

pub(crate) fn core_executor_error_from_rpc(err: RpcError) -> CoreExecutorError {
    if err.code == error::REQUEST_CANCELLED {
        return CoreExecutorError::cancelled();
    }

    let teardown_marker = err
        .data
        .as_ref()
        .and_then(|data| data.get("core_executor_teardown_reason"));
    if let Some(marker) = teardown_marker {
        let Some(reason) = marker
            .as_str()
            .and_then(CoreExecutorTeardownReason::from_wire_str)
        else {
            return CoreExecutorError::apply_failed_runtime_turn(format!(
                "invalid core executor teardown reason in RPC error: {marker}"
            ));
        };
        return CoreExecutorError::teardown_required(reason, err.message);
    }
    if err.code == error::SESSION_NOT_FOUND {
        return CoreExecutorError::session_unavailable_requires_teardown(err.message);
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
        Some(CoreApplyFailureCauseKind::ExecutorStopped) => {
            return CoreExecutorError::Stopped;
        }
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

    fn publication_handle(&self) -> Option<Arc<dyn CoreExecutorPublicationHandle>> {
        Some(meerkat::surface::persistent_runtime_publication_handle(
            self.runtime.persistent_service(),
            self.session_id.clone(),
        ))
    }

    fn machine_managed_post_stop_unregister(&self) -> bool {
        true
    }

    fn post_stop_cleanup_handle(&self) -> Option<Arc<dyn CoreExecutorPostStopCleanupHandle>> {
        Some(Arc::new(SessionRuntimePostStopCleanupHandle {
            runtime: Arc::clone(&self.runtime),
            session_id: self.session_id.clone(),
        }))
    }

    fn turn_finalization_boundary_handle(
        &self,
    ) -> Option<Arc<dyn meerkat_core::lifecycle::CoreExecutorTurnFinalizationBoundaryHandle>> {
        Some(
            meerkat::surface::persistent_runtime_turn_finalization_boundary_handle(
                self.runtime.persistent_service(),
                self.session_id.clone(),
            ),
        )
    }

    async fn apply(
        &mut self,
        run_id: meerkat_core::lifecycle::RunId,
        primitive: RunPrimitive,
    ) -> Result<CoreApplyOutput, CoreExecutorError> {
        if self
            .runtime
            .archived_persisted_session_without_live_by_authority(&self.session_id)
            .await
            .map_err(core_executor_error_from_rpc)?
        {
            return Err(CoreExecutorError::archived_session_requires_teardown(
                format!(
                    "archived session {} requires canonical runtime teardown",
                    self.session_id
                ),
            ));
        }
        if let Some(reason) = primitive.peer_response_terminal_apply_intent_violation() {
            return Err(CoreExecutorError::apply_failed_primitive_rejected(
                reason.to_string(),
            ));
        }

        if let Some(output) = self
            .runtime
            .try_forward_runtime_primitive_to_live_adapter(
                &self.session_id,
                run_id.clone(),
                &primitive,
            )
            .await
            .map_err(CoreExecutorError::apply_failed_runtime_context)?
        {
            return Ok(output);
        }

        // Context-only staged primitives may land directly as runtime
        // system-context appends, but terminal peer responses carry a typed
        // apply intent that requires a requester reaction turn.
        if primitive.is_context_only_apply_without_turn() {
            let RunPrimitive::StagedInput(staged) = &primitive else {
                return Err(CoreExecutorError::apply_failed_primitive_rejected(
                    "context-only apply without turn was not carried by a staged primitive",
                ));
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
                .await;
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
                    .and_then(|meta| meta.turn_tool_overlay.clone()),
                turn_overrides,
                pre_admission,
                crate::session_runtime::LlmReconfigureBoundaryOwnership::AlreadyHeld,
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

    async fn reconcile_committed_compaction_projections(
        &mut self,
        intents: &[meerkat_core::CompactionProjectionIntent],
    ) -> Result<(), CoreExecutorError> {
        self.runtime
            .core_session_service()
            .reconcile_runtime_compaction_projections(&self.session_id, intents.to_vec())
            .await
            .map_err(|error| CoreExecutorError::Internal(error.to_string()))
    }

    async fn abort_uncommitted_compaction_projections(&mut self) -> Result<(), CoreExecutorError> {
        self.runtime
            .core_session_service()
            .abort_uncommitted_compaction_projections(&self.session_id)
            .await
            .map_err(|error| CoreExecutorError::Internal(error.to_string()))
    }

    async fn abort_rejected_run_projections(&mut self) -> Result<(), CoreExecutorError> {
        self.runtime
            .core_session_service()
            .abort_rejected_runtime_run_projections(&self.session_id)
            .await
            .map_err(|error| CoreExecutorError::Internal(error.to_string()))
    }

    async fn checkpoint_committed_session_snapshot(
        &mut self,
        session_snapshot: &[u8],
    ) -> Result<(), CoreExecutorError> {
        self.runtime
            .persistent_service()
            .checkpoint_committed_runtime_session_snapshot_under_runtime_turn_boundary(
                &self.session_id,
                session_snapshot,
            )
            .await
            .map_err(CoreExecutorError::apply_failed_from_session_error)
    }

    async fn publish_interaction_terminals(
        &mut self,
        events: &[AgentEvent],
    ) -> Result<
        Vec<meerkat_core::lifecycle::core_executor::CoreInteractionTerminalPublicationReceipt>,
        CoreExecutorError,
    > {
        self.runtime
            .persistent_service()
            .publish_interaction_terminals_exact_batch(&self.session_id, events)
            .await
            .map_err(CoreExecutorError::apply_failed_from_session_error)
    }

    async fn cancel_after_boundary(&mut self, _reason: String) -> Result<(), CoreExecutorError> {
        self.runtime
            .cancel_current_after_boundary_live_with_machine_authority(&self.session_id)
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
        SessionRuntimePostStopCleanupHandle {
            runtime: Arc::clone(&self.runtime),
            session_id: self.session_id.clone(),
        }
        .cleanup_after_runtime_stop_terminalized()
        .await
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

    fn publication_handle(&self) -> Option<Arc<dyn CoreExecutorPublicationHandle>> {
        Some(Arc::new(MobRpcRuntimePublicationHandle {
            session_service: Arc::clone(&self.session_service),
            session_id: self.session_id.clone(),
        }))
    }

    fn machine_managed_post_stop_unregister(&self) -> bool {
        true
    }

    fn post_stop_cleanup_handle(&self) -> Option<Arc<dyn CoreExecutorPostStopCleanupHandle>> {
        Some(Arc::new(MobRpcRuntimePostStopCleanupHandle {
            session_service: Arc::clone(&self.session_service),
            session_id: self.session_id.clone(),
        }))
    }

    fn turn_finalization_boundary_handle(
        &self,
    ) -> Option<Arc<dyn CoreExecutorTurnFinalizationBoundaryHandle>> {
        Some(Arc::new(MobRpcRuntimeTurnFinalizationBoundaryHandle {
            session_service: Arc::clone(&self.session_service),
            session_id: self.session_id.clone(),
        }))
    }

    async fn apply(
        &mut self,
        run_id: meerkat_core::lifecycle::RunId,
        primitive: RunPrimitive,
    ) -> Result<CoreApplyOutput, CoreExecutorError> {
        if let Some(runtime) = self.runtime.as_ref()
            && runtime
                .archived_persisted_session_without_live_by_authority(&self.session_id)
                .await
                .map_err(core_executor_error_from_rpc)?
        {
            return Err(CoreExecutorError::archived_session_requires_teardown(
                format!(
                    "archived session {} requires canonical runtime teardown",
                    self.session_id
                ),
            ));
        }
        if let Some(reason) = primitive.peer_response_terminal_apply_intent_violation() {
            return Err(CoreExecutorError::apply_failed_primitive_rejected(
                reason.to_string(),
            ));
        }

        if primitive.is_context_only_apply_without_turn() {
            let RunPrimitive::StagedInput(staged) = &primitive else {
                return Err(CoreExecutorError::apply_failed_primitive_rejected(
                    "context-only apply without turn was not carried by a staged primitive",
                ));
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
                    .await;
            }
            return Err(CoreExecutorError::apply_failed_runtime_context(format!(
                "mob RPC executor for {} has no SessionRuntime post-handoff authority",
                self.session_id
            )));
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

        let turn_tool_overlay = primitive
            .turn_metadata()
            .and_then(|meta| meta.turn_tool_overlay.clone());
        let pre_admission = self.runtime.as_ref().and_then(|runtime| {
            runtime.take_runtime_pre_admission(&self.session_id, primitive.contributing_input_ids())
        });
        let result = match self.runtime.as_ref() {
            Some(runtime) => {
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
                        turn_tool_overlay,
                        turn_overrides,
                        pre_admission,
                        crate::session_runtime::LlmReconfigureBoundaryOwnership::AlreadyHeld,
                    )
                    .await
                    .map_err(core_executor_error_from_rpc)
            }
            None => {
                drop(pre_admission);
                drop(event_tx);
                Err(CoreExecutorError::apply_failed_runtime_turn(format!(
                    "mob RPC executor for {} has no SessionRuntime post-handoff authority",
                    self.session_id
                )))
            }
        };

        let _ = forwarder.await;
        result
    }

    async fn reconcile_committed_compaction_projections(
        &mut self,
        intents: &[meerkat_core::CompactionProjectionIntent],
    ) -> Result<(), CoreExecutorError> {
        self.session_service
            .reconcile_runtime_compaction_projections(&self.session_id, intents.to_vec())
            .await
            .map_err(|error| CoreExecutorError::Internal(error.to_string()))
    }

    async fn abort_uncommitted_compaction_projections(&mut self) -> Result<(), CoreExecutorError> {
        self.session_service
            .abort_uncommitted_compaction_projections(&self.session_id)
            .await
            .map_err(|error| CoreExecutorError::Internal(error.to_string()))
    }

    async fn abort_rejected_run_projections(&mut self) -> Result<(), CoreExecutorError> {
        self.session_service
            .abort_rejected_runtime_run_projections(&self.session_id)
            .await
            .map_err(|error| CoreExecutorError::Internal(error.to_string()))
    }

    async fn checkpoint_committed_session_snapshot(
        &mut self,
        session_snapshot: &[u8],
    ) -> Result<(), CoreExecutorError> {
        self.session_service
            .checkpoint_committed_runtime_session_snapshot_under_turn_finalization_boundary(
                &self.session_id,
                session_snapshot,
            )
            .await
            .map_err(CoreExecutorError::apply_failed_from_session_error)
    }

    async fn publish_interaction_terminals(
        &mut self,
        events: &[AgentEvent],
    ) -> Result<
        Vec<meerkat_core::lifecycle::core_executor::CoreInteractionTerminalPublicationReceipt>,
        CoreExecutorError,
    > {
        self.session_service
            .publish_interaction_terminals(&self.session_id, events)
            .await
            .map_err(CoreExecutorError::apply_failed_from_session_error)
    }

    async fn cancel_after_boundary(&mut self, _reason: String) -> Result<(), CoreExecutorError> {
        if let Some(runtime) = self.runtime.as_ref() {
            return runtime
                .cancel_current_after_boundary_live_with_machine_authority(&self.session_id)
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
                .cancel_current_after_boundary_with_machine_authority(
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
        MobRpcRuntimePostStopCleanupHandle {
            session_service: Arc::clone(&self.session_service),
            session_id: self.session_id.clone(),
        }
        .cleanup_after_runtime_stop_terminalized()
        .await
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
    use std::sync::atomic::{AtomicUsize, Ordering};

    enum BoundaryCancelOutcome {
        Unsupported,
        NotRunning,
    }

    struct BoundaryCancelSessionService {
        outcome: BoundaryCancelOutcome,
        reconcile_calls: Arc<AtomicUsize>,
        abort_calls: Arc<AtomicUsize>,
    }

    impl BoundaryCancelSessionService {
        fn unsupported() -> Self {
            Self {
                outcome: BoundaryCancelOutcome::Unsupported,
                reconcile_calls: Arc::new(AtomicUsize::new(0)),
                abort_calls: Arc::new(AtomicUsize::new(0)),
            }
        }

        fn not_running() -> Self {
            Self {
                outcome: BoundaryCancelOutcome::NotRunning,
                reconcile_calls: Arc::new(AtomicUsize::new(0)),
                abort_calls: Arc::new(AtomicUsize::new(0)),
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

        async fn reconcile_runtime_compaction_projections(
            &self,
            _id: &SessionId,
            _intents: Vec<meerkat_core::CompactionProjectionIntent>,
        ) -> Result<(), SessionError> {
            self.reconcile_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        async fn abort_uncommitted_compaction_projections(
            &self,
            _id: &SessionId,
        ) -> Result<(), SessionError> {
            self.abort_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
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
    impl MobSessionService for BoundaryCancelSessionService {
        async fn create_session_under_runtime_turn_boundary(
            &self,
            req: CreateSessionRequest,
        ) -> Result<RunResult, SessionError> {
            <Self as SessionService>::create_session(self, req).await
        }

        async fn archive_with_mob_lifecycle_authority_under_runtime_turn_boundary(
            &self,
            session_id: &SessionId,
        ) -> Result<(), SessionError> {
            self.archive_with_mob_lifecycle_authority(session_id).await
        }

        async fn discard_live_session_under_runtime_turn_boundary(
            &self,
            session_id: &SessionId,
        ) -> Result<(), SessionError> {
            self.discard_live_session(session_id).await
        }
    }

    #[tokio::test]
    async fn mob_rpc_executor_forwards_both_compaction_lifecycle_paths() {
        let session_id = SessionId::new();
        let service = BoundaryCancelSessionService::unsupported();
        let reconcile_calls = Arc::clone(&service.reconcile_calls);
        let abort_calls = Arc::clone(&service.abort_calls);
        let session_service: Arc<dyn MobSessionService> = Arc::new(service);
        let mut executor =
            MobRpcRuntimeExecutor::new(session_service, None, session_id, NotificationSink::noop());

        CoreExecutor::reconcile_committed_compaction_projections(&mut executor, &[])
            .await
            .expect("mob RPC committed reconciliation must reach its session service");
        CoreExecutor::abort_uncommitted_compaction_projections(&mut executor)
            .await
            .expect("mob RPC uncommitted abort must reach its session service");

        assert_eq!(reconcile_calls.load(Ordering::SeqCst), 1);
        assert_eq!(abort_calls.load(Ordering::SeqCst), 1);
    }

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

        let expected_run_id = meerkat_core::lifecycle::RunId::new();
        let error = handle
            .cancel_after_boundary(&expected_run_id, "test boundary cancel".to_string())
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

        let expected_run_id = meerkat_core::lifecycle::RunId::new();
        let error = handle
            .cancel_after_boundary(&expected_run_id, "test boundary cancel".to_string())
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
