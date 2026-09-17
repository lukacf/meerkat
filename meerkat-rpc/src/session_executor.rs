//! SessionRuntimeExecutor — implements `CoreExecutor` by delegating to `SessionRuntime`.
//!
//! This is the bridge between the v9 runtime layer and the existing RPC session
//! management. When the RuntimeLoop processes a queued input, it calls
//! `CoreExecutor::apply()` on this executor, which translates the `RunPrimitive`
//! into a machine-owned `SessionRuntime::apply_runtime_turn_with_pre_admission()` call.

use std::sync::{Arc, RwLock as StdRwLock};

use meerkat_core::EventEnvelope;
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

#[derive(Clone)]
pub(crate) struct RpcRuntimePublicationHandle {
    current: Arc<StdRwLock<Arc<dyn CoreExecutorPublicationHandle>>>,
}

impl RpcRuntimePublicationHandle {
    pub(crate) fn new(current: Arc<dyn CoreExecutorPublicationHandle>) -> Self {
        Self {
            current: Arc::new(StdRwLock::new(current)),
        }
    }

    pub(crate) fn replace(&self, current: Arc<dyn CoreExecutorPublicationHandle>) {
        *self
            .current
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = current;
    }

    pub(crate) fn snapshot(&self) -> Arc<dyn CoreExecutorPublicationHandle> {
        Arc::clone(
            &self
                .current
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
    }
}

#[async_trait::async_trait]
impl CoreExecutorPublicationHandle for RpcRuntimePublicationHandle {
    async fn publish_interaction_terminals(
        &self,
        events: &[AgentEvent],
    ) -> Result<
        Vec<meerkat_core::lifecycle::core_executor::CoreInteractionTerminalPublicationReceipt>,
        CoreExecutorError,
    > {
        let current = Arc::clone(
            &self
                .current
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        current.publish_interaction_terminals(events).await
    }
}

/// Implements `CoreExecutor` by delegating to the runtime-owned apply path.
///
/// Each session gets its own executor instance, which is owned by the
/// RuntimeLoop task. The executor extracts the prompt from the `RunPrimitive`
/// and calls the runtime-owned apply path, forwarding events to the RPC
/// notification sink.
pub struct SessionRuntimeExecutor {
    runtime: Arc<SessionRuntime>,
    session_id: SessionId,
    actor_witness_slot: meerkat::LiveSessionActorWitnessSlot,
    publication_handle: RpcRuntimePublicationHandle,
}

#[cfg(feature = "mob")]
/// Implements `CoreExecutor` directly against a mob-capable session service.
pub struct MobRpcRuntimeExecutor {
    session_service: Arc<dyn MobSessionService>,
    runtime: Option<Arc<SessionRuntime>>,
    session_id: SessionId,
    actor_witness: meerkat::LiveSessionActorWitness,
    publication_handle: RpcRuntimePublicationHandle,
    notification_sink: NotificationSink,
}

#[cfg(feature = "mob")]
impl MobRpcRuntimeExecutor {
    pub fn new(
        session_service: Arc<dyn MobSessionService>,
        runtime: Option<Arc<SessionRuntime>>,
        actor_witness: meerkat::LiveSessionActorWitness,
        notification_sink: NotificationSink,
    ) -> Self {
        let session_id = actor_witness.session_id().clone();
        let publication_handle =
            RpcRuntimePublicationHandle::new(mob_runtime_publication_handle_for_actor(
                Arc::clone(&session_service),
                actor_witness.clone(),
            ));
        Self {
            session_service,
            runtime,
            session_id,
            actor_witness,
            publication_handle,
            notification_sink,
        }
    }

    pub(crate) fn replaceable_publication_handle(&self) -> RpcRuntimePublicationHandle {
        self.publication_handle.clone()
    }
}

impl SessionRuntimeExecutor {
    /// Create a new executor for a specific session.
    ///
    /// The notification sink is NOT captured here — the executor reads the
    /// current sink from the runtime at apply time so reconnected TCP clients
    /// always get events routed to the live transport.
    #[allow(clippy::expect_used)]
    pub fn new(
        runtime: Arc<SessionRuntime>,
        actor_witness: meerkat::LiveSessionActorWitness,
    ) -> Self {
        let session_id = actor_witness.session_id().clone();
        let actor_witness_slot = meerkat::LiveSessionActorWitnessSlot::default();
        actor_witness_slot
            .publish(actor_witness.clone())
            .expect("a fresh RPC actor witness slot accepts its first witness");
        let publication_handle = RpcRuntimePublicationHandle::new(
            meerkat::surface::persistent_runtime_publication_handle(
                runtime.persistent_service(),
                actor_witness,
            ),
        );
        Self {
            runtime,
            session_id,
            actor_witness_slot,
            publication_handle,
        }
    }

    /// Create an executor before its exact service actor has been inserted.
    /// The service must publish that actor into this one-shot slot during the
    /// same materialization transaction; publication fails closed until then.
    pub fn new_for_actor_slot(
        runtime: Arc<SessionRuntime>,
        session_id: SessionId,
        actor_witness_slot: meerkat::LiveSessionActorWitnessSlot,
    ) -> Self {
        let publication_handle = RpcRuntimePublicationHandle::new(
            meerkat::surface::persistent_runtime_publication_handle_for_actor_slot(
                runtime.persistent_service(),
                session_id.clone(),
                actor_witness_slot.clone(),
            ),
        );
        Self {
            runtime,
            session_id,
            actor_witness_slot,
            publication_handle,
        }
    }

    pub(crate) fn replaceable_publication_handle(&self) -> RpcRuntimePublicationHandle {
        self.publication_handle.clone()
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

    async fn prepare_transient_turn_context_at_boundary(
        &self,
        expected_run_id: &meerkat_core::lifecycle::RunId,
        contexts: Vec<meerkat_core::lifecycle::run_primitive::TurnRequestContext>,
    ) -> Result<
        meerkat_core::lifecycle::CoreBoundaryStageOutput,
        meerkat_core::lifecycle::CoreBoundaryStageError,
    > {
        self.runtime
            .prepare_live_transient_turn_context_boundary(
                &self.session_id,
                expected_run_id,
                contexts,
            )
            .await
    }
}

struct SessionRuntimeInterruptHandle {
    runtime: Arc<SessionRuntime>,
    session_id: SessionId,
}

struct SessionRuntimePostStopCleanupHandle {
    runtime: Arc<SessionRuntime>,
    session_id: SessionId,
    actor_witness_slot: meerkat::LiveSessionActorWitnessSlot,
}

#[async_trait::async_trait]
impl CoreExecutorPostStopCleanupHandle for SessionRuntimePostStopCleanupHandle {
    fn durability_reload_cleanup_capability(
        &self,
    ) -> meerkat_core::lifecycle::core_executor::CoreDurabilityReloadCleanupCapability {
        meerkat_core::lifecycle::core_executor::CoreDurabilityReloadCleanupCapability::ProcessLocalNonTerminal
    }

    async fn prepare_durability_reload_cleanup(&self) -> Result<(), CoreExecutorError> {
        // The one-shot slot is the bound service-local identity. Executors
        // built through `new_for_actor_slot` are registered BEFORE their
        // exact actor exists; the service publishes the witness into this
        // slot during the same materialization transaction, so witness
        // presence cannot be a registration-time requirement.
        Ok(())
    }

    async fn cleanup_after_durability_reload_required(&self) -> Result<(), CoreExecutorError> {
        // An empty slot proves the materialization transaction never
        // published an actor for this executor incarnation, so there is no
        // process-local material this handle owns. A witnessed actor is
        // discarded compare-exact; a replacement actor is never this
        // handle's to touch.
        let Some(witness) = self.actor_witness_slot.witness() else {
            return Ok(());
        };
        self.runtime
            .persistent_service()
            .discard_live_session_actor_after_durability_reload_required(&witness)
            .await
            .map(|_| ())
            .map_err(|err| CoreExecutorError::control_failed_runtime(err.to_string()))
    }

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
    async fn hard_cancel_run_if_current(
        &self,
        expected_run_id: &meerkat_core::lifecycle::RunId,
        _reason: String,
    ) -> Result<bool, CoreExecutorError> {
        self.runtime
            .interrupt_run_live_with_machine_authority(&self.session_id, expected_run_id)
            .await
            .or_else(|err| match err {
                SessionError::NotRunning { .. } => Ok(false),
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

    async fn prepare_transient_turn_context_at_boundary(
        &self,
        expected_run_id: &meerkat_core::lifecycle::RunId,
        contexts: Vec<meerkat_core::lifecycle::run_primitive::TurnRequestContext>,
    ) -> Result<
        meerkat_core::lifecycle::CoreBoundaryStageOutput,
        meerkat_core::lifecycle::CoreBoundaryStageError,
    > {
        self.session_service
            .prepare_transient_turn_context_for_active_turn(
                &self.session_id,
                expected_run_id,
                contexts,
            )
            .await
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
    actor_witness: meerkat::LiveSessionActorWitness,
}

#[cfg(feature = "mob")]
pub(crate) fn mob_runtime_publication_handle_for_actor(
    session_service: Arc<dyn MobSessionService>,
    actor_witness: meerkat::LiveSessionActorWitness,
) -> Arc<dyn CoreExecutorPublicationHandle> {
    Arc::new(MobRpcRuntimePublicationHandle {
        session_service,
        actor_witness,
    })
}

#[cfg(feature = "mob")]
struct MobRpcRuntimePostStopCleanupHandle {
    session_service: Arc<dyn MobSessionService>,
    session_id: SessionId,
    actor_witness: meerkat::LiveSessionActorWitness,
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
    fn durability_reload_cleanup_capability(
        &self,
    ) -> meerkat_core::lifecycle::core_executor::CoreDurabilityReloadCleanupCapability {
        meerkat_core::lifecycle::core_executor::CoreDurabilityReloadCleanupCapability::ProcessLocalNonTerminal
    }

    async fn prepare_durability_reload_cleanup(&self) -> Result<(), CoreExecutorError> {
        // The exact actor witness is bound at executor construction, before
        // the attachment is published; there is no later service-local
        // identity to capture.
        Ok(())
    }

    async fn cleanup_after_durability_reload_required(&self) -> Result<(), CoreExecutorError> {
        self.session_service
            .discard_live_session_actor_after_durability_reload_required(&self.actor_witness)
            .await
            .map(|_| ())
            .map_err(|err| CoreExecutorError::control_failed_runtime(err.to_string()))
    }

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
            .publish_interaction_terminals_for_actor(&self.actor_witness, events)
            .await
            .map_err(CoreExecutorError::apply_failed_from_session_error)
    }
}

#[cfg(feature = "mob")]
#[async_trait::async_trait]
impl CoreExecutorInterruptHandle for MobRpcRuntimeInterruptHandle {
    async fn hard_cancel_run_if_current(
        &self,
        expected_run_id: &meerkat_core::lifecycle::RunId,
        _reason: String,
    ) -> Result<bool, CoreExecutorError> {
        if let Some(runtime) = self.runtime.as_ref() {
            return runtime
                .interrupt_run_live_with_machine_authority(&self.session_id, expected_run_id)
                .await
                .or_else(|err| match err {
                    SessionError::NotRunning { .. } => Ok(false),
                    err => Err(err),
                })
                .map_err(|err| CoreExecutorError::control_failed_runtime(err.to_string()));
        }
        if let Some(adapter) = self.session_service.runtime_adapter() {
            return self
                .session_service
                .interrupt_run_with_machine_authority(
                    &self.session_id,
                    expected_run_id,
                    adapter.session_control_authority(),
                )
                .await
                .or_else(|err| match err {
                    SessionError::NotRunning { .. } => Ok(false),
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
        Some(self.publication_handle.snapshot())
    }

    fn machine_managed_post_stop_unregister(&self) -> bool {
        true
    }

    fn post_stop_cleanup_handle(&self) -> Option<Arc<dyn CoreExecutorPostStopCleanupHandle>> {
        Some(Arc::new(SessionRuntimePostStopCleanupHandle {
            runtime: Arc::clone(&self.runtime),
            session_id: self.session_id.clone(),
            actor_witness_slot: self.actor_witness_slot.clone(),
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

    /// The one shared facade-owned realization handle.
    fn pre_dequeue_handle(
        &self,
    ) -> Option<Arc<dyn meerkat_core::lifecycle::CoreExecutorPreDequeueHandle>> {
        Some(meerkat::surface::persistent_runtime_pre_dequeue_handle(
            self.runtime.runtime_adapter(),
            self.session_id.clone(),
        ))
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
        session_snapshot: Arc<Vec<u8>>,
    ) -> Result<(), CoreExecutorError> {
        self.runtime
            .persistent_service()
            .checkpoint_committed_runtime_session_snapshot_under_runtime_turn_boundary(
                &self.session_id,
                session_snapshot,
            )
            .await
            .map_err(CoreExecutorError::apply_failed_from_session_error)?;
        #[cfg(feature = "openai-live")]
        self.runtime
            .runtime_adapter()
            .notify_committed_live_context(&self.session_id);
        Ok(())
    }

    async fn acknowledge_committed_session_boundary(
        &mut self,
        authority: &meerkat_core::CommittedSessionBoundaryAuthority,
    ) -> Result<(), CoreExecutorError> {
        self.runtime
            .persistent_service()
            .acknowledge_committed_runtime_session_boundary_under_runtime_turn_boundary(
                &self.session_id,
                authority,
            )
            .await
            .map_err(CoreExecutorError::apply_failed_from_session_error)?;
        #[cfg(feature = "openai-live")]
        self.runtime
            .runtime_adapter()
            .notify_committed_live_context(&self.session_id);
        Ok(())
    }

    async fn publish_interaction_terminals(
        &mut self,
        events: &[AgentEvent],
    ) -> Result<
        Vec<meerkat_core::lifecycle::core_executor::CoreInteractionTerminalPublicationReceipt>,
        CoreExecutorError,
    > {
        self.publication_handle
            .publish_interaction_terminals(events)
            .await
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
            actor_witness_slot: self.actor_witness_slot.clone(),
        }
        .cleanup_after_runtime_stop_terminalized()
        .await
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod persistent_cleanup_tests {
    use super::*;
    use meerkat::{AgentFactory, Config, PersistenceBundle};
    use meerkat_core::lifecycle::core_executor::CoreDurabilityReloadCleanupCapability;
    use meerkat_core::service::{CreateSessionRequest, InitialTurnPolicy, SessionService};
    use meerkat_core::{ContentInput, SessionBuildOptions, SystemPromptOverride};

    #[tokio::test]
    async fn rpc_degraded_cleanup_is_exact_and_non_terminalizing() {
        let temp = tempfile::tempdir().expect("tempdir");
        let session_store: Arc<dyn meerkat::SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> =
            Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());
        let blob_store: Arc<dyn meerkat_core::BlobStore> =
            Arc::new(meerkat_store::MemoryBlobStore::new());
        let runtime = Arc::new(SessionRuntime::new(
            AgentFactory::new(temp.path().join("sessions")),
            Config::default(),
            2,
            PersistenceBundle::new(session_store, runtime_store, blob_store),
            NotificationSink::noop(),
        ));
        let service = runtime.persistent_service();
        let slot = meerkat::LiveSessionActorWitnessSlot::default();
        let admission = service
            .reserve_create_session_admission()
            .await
            .expect("reserve actor admission");
        let result = service
            .create_session_with_reserved_admission_and_actor_witness(
                CreateSessionRequest {
                    injected_context: Vec::new(),
                    model: "claude-sonnet-4-5".to_string(),
                    prompt: ContentInput::Text(String::new()),
                    system_prompt: SystemPromptOverride::Inherit,
                    max_tokens: None,
                    event_tx: None,
                    initial_turn: InitialTurnPolicy::Defer,
                    deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
                    build: Some(SessionBuildOptions {
                        // CI runs keyless: override the client so the build
                        // never resolves a real provider identity.
                        llm_client_override: Some(meerkat::encode_llm_client_override_for_service(
                            Arc::new(meerkat_llm_core::TestClient::default()),
                        )),
                        ..SessionBuildOptions::default()
                    }),
                    labels: None,
                },
                admission,
                &slot,
            )
            .await
            .expect("materialize RPC actor fixture");
        let durable_before = service
            .load_authoritative_session(&result.session_id)
            .await
            .expect("durable RPC session before cleanup")
            .expect("durable RPC session exists");
        let cleanup = SessionRuntimePostStopCleanupHandle {
            runtime,
            session_id: result.session_id.clone(),
            actor_witness_slot: slot.clone(),
        };
        assert_eq!(
            cleanup.durability_reload_cleanup_capability(),
            CoreDurabilityReloadCleanupCapability::ProcessLocalNonTerminal
        );
        cleanup
            .cleanup_after_durability_reload_required()
            .await
            .expect("RPC degraded cleanup");
        assert!(
            service
                .live_session_actor_witness(&result.session_id)
                .await
                .is_none()
        );
        let durable_after = service
            .load_authoritative_session(&result.session_id)
            .await
            .expect("durable RPC session after cleanup")
            .expect("degraded cleanup retains durable RPC session");
        assert_eq!(
            serde_json::to_value(durable_after).expect("serialize durable RPC session after"),
            serde_json::to_value(durable_before).expect("serialize durable RPC session before"),
            "RPC degraded cleanup must not persist terminal session state"
        );
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
        Some(self.publication_handle.snapshot())
    }

    fn machine_managed_post_stop_unregister(&self) -> bool {
        true
    }

    fn post_stop_cleanup_handle(&self) -> Option<Arc<dyn CoreExecutorPostStopCleanupHandle>> {
        Some(Arc::new(MobRpcRuntimePostStopCleanupHandle {
            session_service: Arc::clone(&self.session_service),
            session_id: self.session_id.clone(),
            actor_witness: self.actor_witness.clone(),
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

    /// The one shared facade-owned realization handle, when this mob member is
    /// bound to a runtime at all.
    ///
    /// A `MobRpcRuntimeExecutor` without a `SessionRuntime` is driving a member
    /// whose turns never pass through a runtime loop, so there is no
    /// pre-dequeue point to hook and no committed handoff it could realize.
    fn pre_dequeue_handle(
        &self,
    ) -> Option<Arc<dyn meerkat_core::lifecycle::CoreExecutorPreDequeueHandle>> {
        self.runtime.as_ref().map(|runtime| {
            meerkat::surface::persistent_runtime_pre_dequeue_handle(
                runtime.runtime_adapter(),
                self.session_id.clone(),
            )
        })
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
        session_snapshot: Arc<Vec<u8>>,
    ) -> Result<(), CoreExecutorError> {
        self.session_service
            .checkpoint_committed_runtime_session_snapshot_under_turn_finalization_boundary(
                &self.session_id,
                session_snapshot,
            )
            .await
            .map_err(CoreExecutorError::apply_failed_from_session_error)?;
        #[cfg(feature = "openai-live")]
        if let Some(runtime) = &self.runtime {
            runtime
                .runtime_adapter()
                .notify_committed_live_context(&self.session_id);
        }
        Ok(())
    }

    async fn acknowledge_committed_session_boundary(
        &mut self,
        authority: &meerkat_core::CommittedSessionBoundaryAuthority,
    ) -> Result<(), CoreExecutorError> {
        self.session_service
            .acknowledge_committed_runtime_session_boundary_under_turn_finalization_boundary(
                &self.session_id,
                authority,
            )
            .await
            .map_err(CoreExecutorError::apply_failed_from_session_error)?;
        #[cfg(feature = "openai-live")]
        if let Some(runtime) = &self.runtime {
            runtime
                .runtime_adapter()
                .notify_committed_live_context(&self.session_id);
        }
        Ok(())
    }

    async fn publish_interaction_terminals(
        &mut self,
        events: &[AgentEvent],
    ) -> Result<
        Vec<meerkat_core::lifecycle::core_executor::CoreInteractionTerminalPublicationReceipt>,
        CoreExecutorError,
    > {
        self.publication_handle
            .publish_interaction_terminals(events)
            .await
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
            actor_witness: self.actor_witness.clone(),
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

    /// Mint one real published actor witness through the persistent service,
    /// exactly as RPC session creation does. Witnesses have no test
    /// constructor by design; authority over an actor exists only when the
    /// registry published it.
    async fn minted_persistent_actor_witness() -> (
        tempfile::TempDir,
        Arc<SessionRuntime>,
        SessionId,
        meerkat::LiveSessionActorWitness,
    ) {
        let temp = tempfile::tempdir().expect("tempdir");
        let session_store: Arc<dyn meerkat::SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> =
            Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());
        let blob_store: Arc<dyn meerkat_core::BlobStore> =
            Arc::new(meerkat_store::MemoryBlobStore::new());
        let runtime = Arc::new(SessionRuntime::new(
            meerkat::AgentFactory::new(temp.path().join("sessions")),
            meerkat::Config::default(),
            2,
            meerkat::PersistenceBundle::new(session_store, runtime_store, blob_store),
            NotificationSink::noop(),
        ));
        let service = runtime.persistent_service();
        let slot = meerkat::LiveSessionActorWitnessSlot::default();
        let admission = service
            .reserve_create_session_admission()
            .await
            .expect("reserve actor admission");
        let result = service
            .create_session_with_reserved_admission_and_actor_witness(
                CreateSessionRequest {
                    injected_context: Vec::new(),
                    model: "claude-sonnet-4-5".to_string(),
                    prompt: meerkat_core::ContentInput::Text(String::new()),
                    system_prompt: meerkat_core::SystemPromptOverride::Inherit,
                    max_tokens: None,
                    event_tx: None,
                    initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
                    deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
                    build: Some(meerkat_core::SessionBuildOptions {
                        llm_client_override: Some(meerkat::encode_llm_client_override_for_service(
                            Arc::new(meerkat_llm_core::TestClient::default()),
                        )),
                        ..meerkat_core::SessionBuildOptions::default()
                    }),
                    labels: None,
                },
                admission,
                &slot,
            )
            .await
            .expect("materialize mob RPC actor fixture");
        let witness = slot.witness().expect("published actor witness");
        (temp, runtime, result.session_id, witness)
    }

    /// The RPC mob cleanup handle must declare the exact capability that
    /// persistent machine-managed registration validates, and its degraded
    /// cleanup must discard only process-local actor material without
    /// terminalizing the durable session.
    #[tokio::test]
    async fn mob_rpc_degraded_cleanup_is_exact_and_non_terminalizing() {
        let (_temp, runtime, session_id, witness) = minted_persistent_actor_witness().await;
        let service = runtime.persistent_service();
        let durable_before = service
            .load_authoritative_session(&session_id)
            .await
            .expect("durable mob RPC session before cleanup")
            .expect("durable mob RPC session exists");

        let executor = MobRpcRuntimeExecutor::new(
            service.clone() as Arc<dyn MobSessionService>,
            None,
            witness.clone(),
            NotificationSink::noop(),
        );
        let cleanup = CoreExecutor::post_stop_cleanup_handle(&executor)
            .expect("mob RPC executor publishes a post-stop cleanup handle");
        assert_eq!(
            cleanup.durability_reload_cleanup_capability(),
            meerkat_core::lifecycle::core_executor::CoreDurabilityReloadCleanupCapability::ProcessLocalNonTerminal,
            "persistent machine-managed registration validates exactly this declaration"
        );
        cleanup
            .prepare_durability_reload_cleanup()
            .await
            .expect("mob RPC degraded cleanup preparation");
        cleanup
            .cleanup_after_durability_reload_required()
            .await
            .expect("mob RPC degraded cleanup");
        assert!(
            service
                .live_session_actor_witness(&session_id)
                .await
                .is_none(),
            "degraded cleanup must discard the exact process-local actor"
        );
        let durable_after = service
            .load_authoritative_session(&session_id)
            .await
            .expect("durable mob RPC session after cleanup")
            .expect("degraded cleanup retains durable mob RPC session");
        assert_eq!(
            serde_json::to_value(durable_after).expect("serialize durable session after"),
            serde_json::to_value(durable_before).expect("serialize durable session before"),
            "mob RPC degraded cleanup must not persist terminal session state"
        );
    }

    enum BoundaryCancelOutcome {
        Unsupported,
        NotRunning,
    }

    struct BoundaryCancelSessionService {
        outcome: BoundaryCancelOutcome,
        reconcile_calls: Arc<AtomicUsize>,
        abort_calls: Arc<AtomicUsize>,
    }

    struct UnusedPublicationHandle;

    #[async_trait::async_trait]
    impl CoreExecutorPublicationHandle for UnusedPublicationHandle {
        async fn publish_interaction_terminals(
            &self,
            _events: &[AgentEvent],
        ) -> Result<
            Vec<meerkat_core::lifecycle::core_executor::CoreInteractionTerminalPublicationReceipt>,
            CoreExecutorError,
        > {
            Err(CoreExecutorError::Internal(
                "compaction forwarding fixture has no actor publication authority".to_string(),
            ))
        }
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
        async fn commit_live_delegation_final_transcript(
            &self,
            _machine: &meerkat_runtime::MeerkatMachine,
            _session_id: &SessionId,
            _provisional: meerkat_core::ProvisionalLiveHandoff,
            _final_event: meerkat_core::RealtimeTranscriptEvent,
        ) -> Result<meerkat_core::FinalLiveUserTranscriptCommitEvidence, SessionError> {
            Err(SessionError::Unsupported(
                "boundary-cancel test service does not support live delegation canonical projection"
                    .into(),
            ))
        }

        async fn materialize_session_resume_verdict(
            &self,
            session_id: &SessionId,
        ) -> Result<meerkat_mob::SessionResumeVerdict, SessionError> {
            meerkat_mob::materialize_nonpersistent_session_resume_verdict(self, session_id).await
        }

        async fn observe_session_resume_authority(
            &self,
            _session_id: &SessionId,
        ) -> Result<meerkat_mob::SessionResumeAuthority, SessionError> {
            // This process-local test double has no RuntimeStore authority rows.
            Ok(meerkat_mob::SessionResumeAuthority::default())
        }

        async fn acknowledge_committed_runtime_session_boundary_under_turn_finalization_boundary(
            &self,
            _session_id: &SessionId,
            _authority: &meerkat_core::CommittedSessionBoundaryAuthority,
        ) -> Result<(), SessionError> {
            Err(SessionError::Unsupported(
                "boundary-cancel test service has no store-owned boundary authority".to_string(),
            ))
        }

        async fn enqueue_committed_parent_session_boundary_after_runtime_turn(
            &self,
            _session_id: &SessionId,
            _runtime_adapter: &meerkat_runtime::MeerkatMachine,
        ) -> Result<usize, SessionError> {
            Ok(0)
        }

        /// Test double: the two-read composition is the exact truth here.
        async fn load_session_for_resume(
            &self,
            session_id: &meerkat_core::SessionId,
        ) -> Result<meerkat_mob::runtime::ResumeSessionLoad, meerkat_core::service::SessionError>
        {
            use meerkat_mob::runtime::ResumeSessionLoad;
            if let Some(session) = self.load_persisted_session(session_id).await? {
                return Ok(ResumeSessionLoad::Active(Box::new(session)));
            }
            if let Some(session) = self.load_revivable_retired_session(session_id).await? {
                return Ok(ResumeSessionLoad::Revivable(Box::new(session)));
            }
            Ok(ResumeSessionLoad::Absent)
        }

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

        async fn archive_with_mob_lifecycle_authority_under_runtime_turn_boundary_before(
            &self,
            session_id: &SessionId,
            _deadline: meerkat_core::time_compat::Instant,
        ) -> Result<(), SessionError> {
            self.archive_with_mob_lifecycle_authority_under_runtime_turn_boundary(session_id)
                .await
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
        let (_temp, _runtime, session_id, actor_witness) = minted_persistent_actor_witness().await;
        let service = BoundaryCancelSessionService::unsupported();
        let reconcile_calls = Arc::clone(&service.reconcile_calls);
        let abort_calls = Arc::clone(&service.abort_calls);
        let session_service: Arc<dyn MobSessionService> = Arc::new(service);
        let mut executor = MobRpcRuntimeExecutor {
            session_service,
            runtime: None,
            session_id,
            actor_witness,
            publication_handle: RpcRuntimePublicationHandle::new(Arc::new(UnusedPublicationHandle)),
            notification_sink: NotificationSink::noop(),
        };

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
