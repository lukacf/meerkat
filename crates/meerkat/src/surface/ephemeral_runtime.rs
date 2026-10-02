//! Runtime-backed composition for an in-memory session service.
//!
//! The store lifetime is ephemeral; session admission and lifecycle are owned
//! by the same MeerkatMachine used by persistent surfaces.

use std::sync::Arc;

use meerkat_core::lifecycle::core_executor::{
    CoreApplyOutput, CoreExecutor, CoreExecutorBoundaryHandle, CoreExecutorError,
    CoreExecutorInterruptHandle, CoreExecutorPostStopCleanupHandle, CoreExecutorPublicationHandle,
    CoreExecutorTurnFinalizationBoundaryHandle, CoreExecutorTurnFinalizationGuard,
    CoreInteractionTerminalPublicationReceipt,
};
use meerkat_core::lifecycle::run_primitive::RunPrimitive;
use meerkat_core::service::{InitialTurnPolicy, StartTurnRequest, StartTurnRuntimeSemantics};
use meerkat_core::{AgentEvent, ContentInput, RuntimeBuildMode, SessionService};
use meerkat_runtime::{
    AcceptOutcome, CompletionOutcome, EnsureRuntimeExecutorAttachment, MeerkatMachine,
    RuntimeCleanupTaskSpawner, RuntimeDriverError, RuntimeExecutorAttachmentWitness,
};

use meerkat_session::LiveSessionActorWitnessSlot;

use crate::{
    CreateSessionRequest, EphemeralSessionService, RunResult, SessionAgentBuilder, SessionError,
    SessionId,
};

#[derive(Debug, thiserror::Error)]
pub enum EphemeralRuntimeError {
    #[error(transparent)]
    Session(#[from] SessionError),
    #[error(transparent)]
    Runtime(#[from] RuntimeDriverError),
    #[error(transparent)]
    Bindings(#[from] meerkat_runtime::meerkat_machine::RuntimeBindingsError),
    #[error(transparent)]
    Materialization(#[from] meerkat_runtime::RuntimeActorMaterializationError),
    #[error(transparent)]
    CompletionWait(#[from] meerkat_runtime::CompletionWaitError),
    #[error("runtime input rejected: {0}")]
    Rejected(meerkat_runtime::RejectReason),
}

impl EphemeralRuntimeError {
    /// Preserve typed admission evidence when translating into a session error.
    pub fn into_session_error(self) -> SessionError {
        let message = self.to_string();
        let (code, data) = match self {
            Self::Session(error) => return error,
            Self::Rejected(reason) => (
                "RUNTIME_INPUT_REJECTED",
                serde_json::json!({ "reason": reason }),
            ),
            Self::Runtime(RuntimeDriverError::NotReady { state }) => {
                ("RUNTIME_NOT_READY", serde_json::json!({ "state": state }))
            }
            Self::Runtime(RuntimeDriverError::NotFound { runtime_id }) => (
                "RUNTIME_NOT_FOUND",
                serde_json::json!({ "runtime_id": runtime_id }),
            ),
            Self::Runtime(RuntimeDriverError::Destroyed) => {
                ("RUNTIME_DESTROYED", serde_json::Value::Null)
            }
            Self::Runtime(RuntimeDriverError::ValidationFailed { reason }) => {
                ("INVALID_PARAMS", serde_json::json!({ "reason": reason }))
            }
            Self::Runtime(RuntimeDriverError::InputIdempotencyConflict { existing_id }) => (
                "INPUT_IDEMPOTENCY_CONFLICT",
                serde_json::json!({ "existing_id": existing_id }),
            ),
            Self::Runtime(RuntimeDriverError::UnregisterInProgress { runtime_id }) => (
                "UNREGISTER_IN_PROGRESS",
                serde_json::json!({ "runtime_id": runtime_id }),
            ),
            Self::Runtime(RuntimeDriverError::RuntimeStopInProgress { runtime_id }) => (
                "RUNTIME_STOP_IN_PROGRESS",
                serde_json::json!({ "runtime_id": runtime_id }),
            ),
            Self::Runtime(_) => ("RUNTIME_ERROR", serde_json::Value::Null),
            Self::Bindings(_) => ("RUNTIME_BINDINGS_ERROR", serde_json::Value::Null),
            Self::Materialization(_) => ("RUNTIME_MATERIALIZATION_ERROR", serde_json::Value::Null),
            Self::CompletionWait(_) => ("RUNTIME_COMPLETION_UNAVAILABLE", serde_json::Value::Null),
        };
        SessionError::FailedWithData {
            message: message.clone(),
            data: serde_json::json!({ "code": code, "message": message, "data": data }),
        }
    }
}

/// Create one canonical runtime-owned actor and attach its executor.
///
/// Exact rollback custody spans actor construction, attachment commit, and
/// peer-ingress installation. Dropping the future before its result is returned
/// retires that same actor incarnation through the machine.
pub async fn materialize_ephemeral_runtime_session<B: SessionAgentBuilder + 'static>(
    service: &Arc<EphemeralSessionService<B>>,
    machine: &Arc<MeerkatMachine>,
    request: CreateSessionRequest,
    keep_alive: bool,
) -> Result<RunResult, EphemeralRuntimeError> {
    materialize_ephemeral_runtime_session_inner(
        service,
        machine,
        request,
        keep_alive,
        #[cfg(all(test, not(target_arch = "wasm32")))]
        None,
    )
    .await
}

async fn materialize_ephemeral_runtime_session_inner<B: SessionAgentBuilder + 'static>(
    service: &Arc<EphemeralSessionService<B>>,
    machine: &Arc<MeerkatMachine>,
    mut request: CreateSessionRequest,
    keep_alive: bool,
    #[cfg(all(test, not(target_arch = "wasm32")))] after_attachment_commit: Option<
        MaterializationCommitTestBarrier,
    >,
) -> Result<RunResult, EphemeralRuntimeError> {
    if machine.has_runtime_persistence() {
        return Err(SessionError::Unsupported(
            "ephemeral session materialization requires a machine without runtime persistence"
                .into(),
        )
        .into());
    }
    if request.initial_turn != InitialTurnPolicy::Defer {
        return Err(SessionError::Unsupported(
            "runtime materialization requires a deferred initial turn; submit the prompt through runtime admission after creation".into(),
        ).into());
    }
    let session_id = SessionId::new();
    let boundary = service
        .acquire_runtime_turn_finalization_guard(&session_id)
        .await;
    let mut prepared = machine
        .prepare_session_materialization(session_id.clone())
        .await?;
    let actor = LiveSessionActorWitnessSlot::default();
    let executor = Arc::new(EphemeralRuntimeHandles {
        service: service.clone(),
        session_id: session_id.clone(),
        actor: actor.clone(),
    });
    let materialized: Result<_, EphemeralRuntimeError> = async {
        machine
            .install_prepared_session_executor_handles(
                prepared.bindings(),
                executor.clone(),
                executor.clone(),
            )
            .await?;
        let build = request.build.get_or_insert_with(Default::default);
        build.runtime_build_mode = RuntimeBuildMode::SessionOwned(prepared.bindings_clone());
        build.mint_session_with_id(session_id.clone());
        build.keep_alive = keep_alive;
        let permit =
            meerkat_runtime::begin_session_runtime_actor_materialization(prepared.bindings())?;
        let (created, _) = service
            .create_session_with_admission_and_witness(request, None, Some(&actor))
            .await?;
        permit.commit()?;
        let attachment = prepared
            .ensure_executor_attachment_under_runtime_turn_finalization_boundary(move |_| {
                Box::new(EphemeralRuntimeExecutor { handles: executor })
            })
            .await?;
        Ok((created, attachment))
    }
    .await;
    let (created, attachment) = match materialized {
        Ok(result) => result,
        Err(error) => {
            prepared
                .rollback_now_under_turn_finalization_boundary()
                .await?;
            return Err(error);
        }
    };
    let witness = match &attachment {
        EnsureRuntimeExecutorAttachment::Pending(pending) => pending.witness().clone(),
        EnsureRuntimeExecutorAttachment::Existing(witness) => witness.clone(),
    };
    // Pending::commit moves its lease into an independent owner task. Capture
    // exact cleanup custody before that handoff so cancellation of its waiter,
    // or of later ingress setup, cannot orphan a successful detached commit.
    let mut unpublished = UnpublishedEphemeralRuntimeAttachment {
        machine: machine.clone(),
        witness,
        cleanup_spawner: prepared.cleanup_task_spawner(),
        armed: true,
    };
    drop(boundary);
    let published: Result<(), EphemeralRuntimeError> = async {
        if let EnsureRuntimeExecutorAttachment::Pending(pending) = attachment {
            pending.commit().await?;
        }
        #[cfg(all(test, not(target_arch = "wasm32")))]
        if let Some(barrier) = after_attachment_commit {
            let _ = barrier.entered.send(unpublished.witness.clone());
            let _ = barrier.release.await;
        }
        let comms = service.comms_runtime(&session_id).await;
        machine
            .update_peer_ingress_context_if_current(&unpublished.witness, keep_alive, comms)
            .await?;
        Ok(())
    }
    .await;
    if let Err(error) = published {
        unpublished.retire().await?;
        return Err(error);
    }
    // No await may follow this custody transfer: returning the result is the
    // caller's synchronous acquisition of the completed materialization.
    unpublished.armed = false;
    Ok(created)
}

/// Mechanical custody only. Every retirement decision and actor cleanup stays
/// behind the machine's exact attachment unregister seam.
struct UnpublishedEphemeralRuntimeAttachment {
    machine: Arc<MeerkatMachine>,
    witness: RuntimeExecutorAttachmentWitness,
    cleanup_spawner: RuntimeCleanupTaskSpawner,
    armed: bool,
}

impl UnpublishedEphemeralRuntimeAttachment {
    async fn retire(&mut self) -> Result<(), RuntimeDriverError> {
        self.machine
            .unregister_executor_attachment_if_current(&self.witness)
            .await?;
        self.armed = false;
        Ok(())
    }
}

impl Drop for UnpublishedEphemeralRuntimeAttachment {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let machine = self.machine.clone();
        let witness = self.witness.clone();
        self.cleanup_spawner.spawn_detached(async move {
            if let Err(error) = machine
                .unregister_executor_attachment_if_current(&witness)
                .await
            {
                tracing::warn!(
                    session_id = %witness.session_id(),
                    %error,
                    "unpublished ephemeral attachment exact retirement failed"
                );
            }
        });
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
struct MaterializationCommitTestBarrier {
    entered: tokio::sync::oneshot::Sender<RuntimeExecutorAttachmentWitness>,
    release: tokio::sync::oneshot::Receiver<()>,
}

/// Submit through generated runtime input admission and await its exact result.
pub async fn run_ephemeral_runtime_turn(
    machine: &Arc<MeerkatMachine>,
    session_id: &SessionId,
    content: ContentInput,
    metadata: Option<meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata>,
) -> Result<CompletionOutcome, EphemeralRuntimeError> {
    let mut prompt = meerkat_runtime::input::PromptInput::new("", metadata);
    prompt.content = content;
    prompt.header.durability = meerkat_runtime::input::InputDurability::Ephemeral;
    let (accepted, completion) = machine
        .accept_input_with_completion(session_id, meerkat_runtime::input::Input::Prompt(prompt))
        .await?;
    if let AcceptOutcome::Rejected { reason } = accepted {
        return Err(EphemeralRuntimeError::Rejected(reason));
    }
    let completion = completion.ok_or_else(|| {
        RuntimeDriverError::Internal("runtime admitted a prompt without a completion handle".into())
    })?;
    Ok(completion.wait().await?)
}

/// Preserve the runtime-owned completion and terminal metadata at a surface boundary.
/// No transcript or executor state is consulted to reconstruct terminality.
pub fn ephemeral_runtime_completion_result(
    outcome: CompletionOutcome,
) -> Result<RunResult, SessionError> {
    use meerkat_core::AgentError;
    let (code, message, data) = match outcome {
        CompletionOutcome::Completed(result) => return Ok(*result),
        CompletionOutcome::CompletedWithoutResult => (
            "RUNTIME_RESULT_UNAVAILABLE",
            "runtime completed the input without a run result".into(),
            serde_json::Value::Null,
        ),
        CompletionOutcome::Cancelled => return Err(SessionError::Agent(AgentError::Cancelled)),
        CompletionOutcome::CallbackPending {
            tool_use_id,
            tool_name,
            args,
        } => {
            return Err(SessionError::Agent(AgentError::CallbackPending {
                tool_use_id,
                tool_name,
                args,
            }));
        }
        CompletionOutcome::CallbackBatchPending { pending_tool_calls } => {
            return Err(SessionError::Agent(AgentError::CallbackBatchPending {
                pending_tool_calls,
            }));
        }
        CompletionOutcome::Abandoned { reason, error } => {
            ("AGENT_ERROR", reason, serde_json::json!({ "error": error }))
        }
        CompletionOutcome::AbandonedWithError { reason, error } => {
            ("AGENT_ERROR", reason, serde_json::json!({ "error": error }))
        }
        CompletionOutcome::CompletedWithFinalizationFailure { error } => (
            "TURN_FINALIZATION_FAILED",
            error
                .detail
                .clone()
                .unwrap_or_else(|| "turn finalization failed".into()),
            serde_json::json!({ "error": error }),
        ),
        CompletionOutcome::RuntimeTerminated { reason, error } => (
            "RUNTIME_TERMINATED",
            reason,
            serde_json::json!({ "error": error }),
        ),
    };
    Err(SessionError::FailedWithData {
        message: message.clone(),
        data: serde_json::json!({ "code": code, "message": message, "data": data }),
    })
}

/// Use generated public interrupt authority for both cancellation and idle no-op.
pub async fn interrupt_ephemeral_runtime_session(
    machine: &Arc<MeerkatMachine>,
    session_id: &SessionId,
) -> Result<(), EphemeralRuntimeError> {
    use meerkat_runtime::{UserInterruptObservation, UserInterruptPublicResult};
    let observation = match machine
        .hard_cancel_current_run(session_id, "user interrupt")
        .await
    {
        Ok(()) => UserInterruptObservation::Accepted,
        Err(RuntimeDriverError::NotReady { state }) => UserInterruptObservation::NotReady(state),
        Err(RuntimeDriverError::Destroyed | RuntimeDriverError::NotFound { .. }) => {
            UserInterruptObservation::Destroyed
        }
        Err(error) => return Err(error.into()),
    };
    match meerkat_runtime::resolve_user_interrupt_public_result(
        observation,
        machine.contains_session(session_id).await,
        false,
    )? {
        UserInterruptPublicResult::Interrupted | UserInterruptPublicResult::StagedNoop => Ok(()),
        UserInterruptPublicResult::NotFound => Err(SessionError::NotFound {
            id: session_id.clone(),
        }
        .into()),
        UserInterruptPublicResult::SessionBusy | UserInterruptPublicResult::Conflict => {
            Err(SessionError::Busy {
                id: session_id.clone(),
            }
            .into())
        }
    }
}

/// Wire one registered local session to another through generated trust authority.
/// Calling this in each direction establishes bidirectional trust.
pub async fn wire_ephemeral_runtime_peer<B: SessionAgentBuilder + 'static>(
    service: &Arc<EphemeralSessionService<B>>,
    machine: &Arc<MeerkatMachine>,
    session_id: &SessionId,
    peer_session_id: &SessionId,
) -> Result<(), EphemeralRuntimeError> {
    let comms = service
        .comms_runtime(session_id)
        .await
        .ok_or_else(|| SessionError::Unsupported("session has no comms runtime".into()))?;
    let peer_comms = service
        .comms_runtime(peer_session_id)
        .await
        .ok_or_else(|| SessionError::Unsupported("peer session has no comms runtime".into()))?;
    let missing = || {
        SessionError::Unsupported("peer comms runtime has no complete advertised endpoint".into())
    };
    let endpoint = meerkat_runtime::meerkat_machine::dsl::PeerEndpoint::new(
        peer_comms.comms_name().ok_or_else(missing)?,
        peer_comms.peer_id().ok_or_else(missing)?.to_string(),
        peer_comms.advertised_address().ok_or_else(missing)?,
        peer_comms.public_key_bytes().ok_or_else(missing)?,
    );
    machine
        .stage_add_direct_peer_endpoint(session_id, endpoint, comms)
        .await
        .map_err(|error| {
            RuntimeDriverError::ValidationFailed {
                reason: error.to_string(),
            }
            .into()
        })
}

struct EphemeralRuntimeHandles<B: SessionAgentBuilder> {
    service: Arc<EphemeralSessionService<B>>,
    session_id: SessionId,
    actor: LiveSessionActorWitnessSlot,
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl<B: SessionAgentBuilder + 'static> CoreExecutorBoundaryHandle for EphemeralRuntimeHandles<B> {
    async fn cancel_after_boundary(
        &self,
        run_id: &meerkat_core::RunId,
        _reason: String,
    ) -> Result<(), CoreExecutorError> {
        self.service
            .cancel_after_boundary_for_run(&self.session_id, run_id)
            .await
            .or_else(|error| match error {
                SessionError::NotRunning { .. } => Ok(()),
                error => Err(error),
            })
            .map_err(CoreExecutorError::apply_failed_from_session_error)
    }

    async fn prepare_turn_boundary_delivery(
        &self,
        run_id: &meerkat_core::RunId,
        delivery: meerkat_core::TurnBoundaryDelivery,
    ) -> Result<
        meerkat_core::lifecycle::CoreBoundaryStageOutput,
        meerkat_core::CoreBoundaryStageError,
    > {
        self.service
            .prepare_transient_turn_context_for_active_turn(&self.session_id, run_id, delivery)
            .await
            .map(|prepared| prepared.into_stage_output(None))
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl<B: SessionAgentBuilder + 'static> CoreExecutorInterruptHandle for EphemeralRuntimeHandles<B> {
    async fn hard_cancel_run_if_current(
        &self,
        run_id: &meerkat_core::RunId,
        _reason: String,
    ) -> Result<bool, CoreExecutorError> {
        self.service
            .interrupt_run_if_current(&self.session_id, run_id)
            .await
            .or_else(|error| match error {
                SessionError::NotRunning { .. } => Ok(false),
                error => Err(error),
            })
            .map_err(CoreExecutorError::apply_failed_from_session_error)
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl<B: SessionAgentBuilder + 'static> CoreExecutorTurnFinalizationBoundaryHandle
    for EphemeralRuntimeHandles<B>
{
    async fn acquire(
        &self,
    ) -> Result<Box<dyn CoreExecutorTurnFinalizationGuard>, CoreExecutorError> {
        Ok(Box::new(
            self.service
                .acquire_runtime_turn_finalization_guard(&self.session_id)
                .await,
        ))
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl<B: SessionAgentBuilder + 'static> CoreExecutorPostStopCleanupHandle
    for EphemeralRuntimeHandles<B>
{
    fn durability_reload_cleanup_capability(
        &self,
    ) -> meerkat_core::lifecycle::core_executor::CoreDurabilityReloadCleanupCapability {
        meerkat_core::lifecycle::core_executor::CoreDurabilityReloadCleanupCapability::ProcessLocalNonTerminal
    }

    async fn prepare_durability_reload_cleanup(&self) -> Result<(), CoreExecutorError> {
        // This slot is published once by the service before attachment. Bind
        // that actor incarnation, never a later lookup by logical SessionId.
        let actor = self.actor.witness().ok_or_else(|| {
            CoreExecutorError::control_failed_runtime(
                "ephemeral runtime cleanup has no published actor witness",
            )
        })?;
        if actor.session_id() != &self.session_id || !actor.is_live() {
            return Err(CoreExecutorError::control_failed_runtime(
                "ephemeral runtime cleanup has no live exact-session actor witness",
            ));
        }
        Ok(())
    }

    async fn cleanup_after_runtime_stop_terminalized(&self) -> Result<(), CoreExecutorError> {
        let _boundary = self
            .service
            .acquire_runtime_turn_finalization_guard(&self.session_id)
            .await;
        self.cleanup_after_runtime_stop_terminalized_under_turn_finalization_boundary()
            .await
    }

    async fn cleanup_after_runtime_stop_terminalized_under_turn_finalization_boundary(
        &self,
    ) -> Result<(), CoreExecutorError> {
        if let Some(actor) = self.actor.witness() {
            self.service
                .discard_live_session_actor(&actor)
                .await
                .map_err(CoreExecutorError::apply_failed_from_session_error)?;
        }
        Ok(())
    }

    async fn cleanup_after_durability_reload_required(&self) -> Result<(), CoreExecutorError> {
        let actor = self.actor.witness().ok_or_else(|| {
            CoreExecutorError::control_failed_runtime(
                "ephemeral runtime cleanup has no published actor witness",
            )
        })?;
        if actor.session_id() != &self.session_id {
            return Err(CoreExecutorError::control_failed_runtime(
                "ephemeral runtime cleanup actor witness belongs to another session",
            ));
        }
        // The degraded-registration caller may already hold the outer turn
        // boundary. Exact discard removes only process-local actor material,
        // signals shutdown without joining it, and cannot remove a successor.
        self.service
            .discard_live_session_actor(&actor)
            .await
            .map(|_| ())
            .map_err(CoreExecutorError::apply_failed_from_session_error)
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl<B: SessionAgentBuilder + 'static> CoreExecutorPublicationHandle
    for EphemeralRuntimeHandles<B>
{
    async fn publish_interaction_terminals(
        &self,
        events: &[AgentEvent],
    ) -> Result<Vec<CoreInteractionTerminalPublicationReceipt>, CoreExecutorError> {
        let actor = self.actor.witness().ok_or_else(|| {
            CoreExecutorError::Internal("runtime executor lost its exact actor witness".into())
        })?;
        self.service
            .publish_runtime_interaction_terminals_for_actor(&actor, events)
            .await
            .map_err(CoreExecutorError::apply_failed_from_session_error)
    }
}

struct EphemeralRuntimeExecutor<B: SessionAgentBuilder> {
    handles: Arc<EphemeralRuntimeHandles<B>>,
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl<B: SessionAgentBuilder + 'static> CoreExecutor for EphemeralRuntimeExecutor<B> {
    fn supports_work_authorization(&self) -> bool {
        true
    }
    fn boundary_handle(&self) -> Option<Arc<dyn CoreExecutorBoundaryHandle>> {
        Some(self.handles.clone())
    }
    fn interrupt_handle(&self) -> Option<Arc<dyn CoreExecutorInterruptHandle>> {
        Some(self.handles.clone())
    }
    fn publication_handle(&self) -> Option<Arc<dyn CoreExecutorPublicationHandle>> {
        Some(self.handles.clone())
    }
    fn machine_managed_post_stop_unregister(&self) -> bool {
        true
    }
    fn post_stop_cleanup_handle(&self) -> Option<Arc<dyn CoreExecutorPostStopCleanupHandle>> {
        Some(self.handles.clone())
    }
    fn turn_finalization_boundary_handle(
        &self,
    ) -> Option<Arc<dyn CoreExecutorTurnFinalizationBoundaryHandle>> {
        Some(self.handles.clone())
    }

    async fn apply(
        &mut self,
        run_id: meerkat_core::RunId,
        primitive: RunPrimitive,
    ) -> Result<CoreApplyOutput, CoreExecutorError> {
        if let Some(reason) = primitive.peer_response_terminal_apply_intent_violation() {
            return Err(CoreExecutorError::apply_failed_primitive_rejected(
                reason.to_string(),
            ));
        }
        let mut metadata = primitive.turn_metadata().cloned();
        if let Some(metadata) = &mut metadata {
            metadata.handling_mode = Some(meerkat_core::HandlingMode::Queue);
            metadata.render_metadata = None;
        }
        let request = StartTurnRequest {
            injected_context: Vec::new(),
            prompt: primitive.extract_content_input(),
            system_prompt: None,
            event_tx: None,
            runtime: StartTurnRuntimeSemantics::new(
                meerkat_core::HandlingMode::Queue,
                primitive
                    .turn_metadata()
                    .and_then(|metadata| metadata.turn_tool_overlay.clone()),
                metadata,
            )
            .with_work_authorization(
                primitive
                    .turn_metadata()
                    .and_then(|meta| meta.work_authorization.clone()),
            )
            .with_typed_turn_appends(primitive.typed_turn_appends()),
        };
        self.handles
            .service
            .apply_runtime_turn(
                &self.handles.session_id,
                run_id,
                request,
                primitive.apply_boundary(),
                primitive.contributing_input_ids().to_vec(),
            )
            .await
            .map_err(CoreExecutorError::apply_failed_from_session_error)
    }

    async fn reconcile_committed_compaction_projections(
        &mut self,
        intents: &[meerkat_core::CompactionProjectionIntent],
    ) -> Result<(), CoreExecutorError> {
        self.handles
            .service
            .reconcile_runtime_compaction_projections(&self.handles.session_id, intents.to_vec())
            .await
            .map_err(CoreExecutorError::apply_failed_from_session_error)
    }
    async fn abort_uncommitted_compaction_projections(&mut self) -> Result<(), CoreExecutorError> {
        self.handles
            .service
            .abort_uncommitted_compaction_projections(&self.handles.session_id)
            .await
            .map_err(CoreExecutorError::apply_failed_from_session_error)
    }
    async fn publish_interaction_terminals(
        &mut self,
        events: &[AgentEvent],
    ) -> Result<Vec<CoreInteractionTerminalPublicationReceipt>, CoreExecutorError> {
        self.handles.publish_interaction_terminals(events).await
    }
    async fn publish_boundary_appends_discarded(
        &mut self,
        discarded: &meerkat_core::event::BoundaryAppendsDiscarded,
    ) -> Result<(), CoreExecutorError> {
        let actor = self.handles.actor.witness().ok_or_else(|| {
            CoreExecutorError::Internal("runtime executor lost its exact actor witness".into())
        })?;
        self.handles
            .service
            .publish_boundary_appends_discarded_for_actor(&actor, discarded)
            .await
            .map_err(CoreExecutorError::apply_failed_from_session_error)
    }
    async fn cancel_after_boundary(&mut self, _reason: String) -> Result<(), CoreExecutorError> {
        self.handles
            .service
            .cancel_after_boundary(&self.handles.session_id)
            .await
            .or_else(|error| match error {
                SessionError::NotRunning { .. } => Ok(()),
                error => Err(error),
            })
            .map_err(CoreExecutorError::apply_failed_from_session_error)
    }
    async fn stop_runtime_executor(&mut self, _reason: String) -> Result<(), CoreExecutorError> {
        Ok(())
    }
    async fn cleanup_after_runtime_stop_terminalized(&mut self) -> Result<(), CoreExecutorError> {
        self.handles.cleanup_after_runtime_stop_terminalized().await
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::{AgentFactory, Config, FactoryAgentBuilder};
    use meerkat_core::service::SessionServiceHistoryExt;
    use meerkat_runtime::SessionServiceRuntimeExt;

    struct CaptureRequestClient {
        inner: meerkat_client::TestClient,
        messages: std::sync::Mutex<Vec<String>>,
    }

    #[async_trait::async_trait]
    impl meerkat_client::LlmClient for CaptureRequestClient {
        fn project_replay_messages(
            &self,
            messages: &[meerkat_core::Message],
        ) -> Result<Vec<meerkat_core::Message>, meerkat_client::LlmError> {
            Ok(messages.to_vec())
        }

        fn stream<'a>(
            &'a self,
            request: &'a meerkat_client::LlmRequest,
        ) -> std::pin::Pin<
            Box<
                dyn futures::Stream<
                        Item = Result<meerkat_client::LlmEvent, meerkat_client::LlmError>,
                    > + Send
                    + 'a,
            >,
        > {
            self.messages
                .lock()
                .expect("request capture")
                .push(serde_json::to_string(&request.messages).expect("serialize request"));
            self.inner.stream(request)
        }

        fn provider(&self) -> meerkat_core::Provider {
            self.inner.provider()
        }

        async fn health_check(&self) -> Result<(), meerkat_client::LlmError> {
            self.inner.health_check().await
        }
    }

    fn service() -> (
        Arc<EphemeralSessionService<FactoryAgentBuilder>>,
        Arc<MeerkatMachine>,
    ) {
        let factory =
            AgentFactory::new(std::env::temp_dir().join("meerkat-runtime-ephemeral-tests"));
        let mut builder = FactoryAgentBuilder::new(factory, Config::default());
        builder.default_llm_client = Some(Arc::new(meerkat_client::TestClient::for_provider(
            meerkat_core::Provider::OpenAI,
        )));
        (
            Arc::new(EphemeralSessionService::new(builder, 4)),
            Arc::new(MeerkatMachine::ephemeral()),
        )
    }

    fn request() -> CreateSessionRequest {
        CreateSessionRequest {
            injected_context: Vec::new(),
            model: "gpt-4o".into(),
            prompt: "".into(),
            system_prompt: meerkat_core::SystemPromptOverride::Inherit,
            max_tokens: None,
            event_tx: None,
            initial_turn: InitialTurnPolicy::Defer,
            deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
            build: None,
            labels: None,
        }
    }

    #[tokio::test]
    async fn ephemeral_materialization_rejects_persistent_machine_before_effects() {
        use meerkat_runtime::RuntimeStore;

        struct BuildProbe {
            inner: FactoryAgentBuilder,
            attempted: Arc<std::sync::Mutex<Vec<SessionId>>>,
        }

        #[async_trait::async_trait]
        impl SessionAgentBuilder for BuildProbe {
            type Agent = <FactoryAgentBuilder as SessionAgentBuilder>::Agent;

            async fn build_agent(
                &self,
                request: &CreateSessionRequest,
                event_tx: tokio::sync::mpsc::Sender<AgentEvent>,
            ) -> Result<Self::Agent, SessionError> {
                let id = request
                    .build
                    .as_ref()
                    .and_then(|build| build.resume_session.as_ref())
                    .expect("materializer must seat the real Agent on its exact session")
                    .id()
                    .clone();
                self.attempted.lock().expect("build attempts").push(id);
                self.inner.build_agent(request, event_tx).await
            }
        }

        let client = Arc::new(CaptureRequestClient {
            inner: meerkat_client::TestClient::for_provider(meerkat_core::Provider::OpenAI),
            messages: std::sync::Mutex::new(Vec::new()),
        });
        let factory =
            AgentFactory::new(std::env::temp_dir().join("meerkat-ephemeral-store-mismatch"));
        let mut builder = FactoryAgentBuilder::new(factory, Config::default());
        builder.default_llm_client = Some(client.clone());
        let attempted = Arc::new(std::sync::Mutex::new(Vec::new()));
        let service = Arc::new(EphemeralSessionService::new(
            BuildProbe {
                inner: builder,
                attempted: attempted.clone(),
            },
            1,
        ));
        let store = Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());
        let machine = Arc::new(MeerkatMachine::persistent(
            store.clone(),
            Arc::new(crate::MemoryBlobStore::new()),
        ));
        assert!(machine.has_runtime_persistence());
        let mut materialization = Box::pin(materialize_ephemeral_runtime_session(
            &service,
            &machine,
            request(),
            false,
        ));
        let (ready_without_suspending, outcome) = match futures::poll!(materialization.as_mut()) {
            std::task::Poll::Ready(outcome) => (true, Ok(outcome)),
            std::task::Poll::Pending => (
                false,
                tokio::time::timeout(std::time::Duration::from_secs(10), materialization.as_mut())
                    .await,
            ),
        };
        drop(materialization);
        let builds = attempted.lock().expect("build attempts").clone();
        let live = service.list(Default::default()).await;
        let catalog = store
            .list_runtime_session_catalog_entries(Default::default())
            .await;
        let provider_calls = client.messages.lock().expect("request capture").len();

        // The old implementation materializes successfully. Retire any actual
        // actor/registration before the intended negative assertion fails.
        let mut cleanup_ids = builds.clone();
        if let Ok(Ok(created)) = &outcome
            && !cleanup_ids.contains(&created.session_id)
        {
            cleanup_ids.push(created.session_id.clone());
        }
        if let Ok(summaries) = &live {
            for summary in summaries {
                if !cleanup_ids.contains(&summary.session_id) {
                    cleanup_ids.push(summary.session_id.clone());
                }
            }
        }
        let mut cleanup_results = Vec::new();
        for id in &cleanup_ids {
            if machine.contains_session(id).await {
                cleanup_results.push(
                    tokio::time::timeout(
                        std::time::Duration::from_secs(10),
                        machine.unregister_session(id),
                    )
                    .await,
                );
            }
            if let Some(actor) = service.live_session_actor_witness(id).await {
                tokio::time::timeout(
                    std::time::Duration::from_secs(10),
                    service.discard_live_session_actor(&actor),
                )
                .await
                .expect("bounded unexpected-actor cleanup")
                .expect("retire exact unexpected actor");
            }
        }
        for result in cleanup_results {
            result
                .expect("bounded unexpected-registration cleanup")
                .expect("exact registration cleanup");
        }

        assert!(
            matches!(&outcome, Ok(Err(EphemeralRuntimeError::Session(SessionError::Unsupported(reason))))
            if reason == "ephemeral session materialization requires a machine without runtime persistence"),
            "wrong owner must fail as configuration, before a turn or store commit; error={:?}, built={builds:?}",
            outcome.as_ref().map(|result| result.as_ref().err())
        );
        assert!(
            ready_without_suspending,
            "reject before any materialization await or task handoff"
        );
        assert!(
            builds.is_empty(),
            "no actor construction or native bindings reach the real builder"
        );
        assert!(live.expect("actual service view").is_empty());
        assert!(catalog.expect("actual store catalog").is_empty());
        assert_eq!(
            provider_calls, 0,
            "no model effect may precede configuration refusal"
        );

        // The same real builder, service capacity and client remain usable on
        // the supported storeless machine; no rejecting fixture substitutes.
        let ephemeral = Arc::new(MeerkatMachine::ephemeral());
        let created = materialize_ephemeral_runtime_session(&service, &ephemeral, request(), false)
            .await
            .expect("ordinary ephemeral materialization");
        let id = created.session_id;
        let outcome =
            run_ephemeral_runtime_turn(&ephemeral, &id, "supported ephemeral turn".into(), None)
                .await;
        let cleanup = ephemeral.unregister_session(&id).await;
        let completed =
            ephemeral_runtime_completion_result(outcome.expect("ordinary native admission"))
                .expect("ordinary ephemeral turn completes");
        cleanup.expect("ordinary ephemeral cleanup");
        assert_eq!(completed.session_id, id);
        assert_eq!(
            attempted.lock().expect("build attempts").as_slice(),
            std::slice::from_ref(&id)
        );
        assert!(!client.messages.lock().expect("request capture").is_empty());
        assert!(!ephemeral.contains_session(&id).await);
        assert!(!service.live_session_actor_registered(&id).await);
    }

    #[tokio::test]
    async fn ephemeral_reload_cleanup_is_exact_nonterminal_and_keeps_held_outer_boundary() {
        let (service, machine) = service();
        let created = materialize_ephemeral_runtime_session(&service, &machine, request(), false)
            .await
            .expect("real actor materialization");
        let id = created.session_id;
        let actor = service
            .live_session_actor_witness(&id)
            .await
            .expect("actual actor witness");
        let slot = LiveSessionActorWitnessSlot::default();
        slot.publish(actor.clone())
            .expect("bind immutable exact actor slot");
        let missing = EphemeralRuntimeHandles {
            service: service.clone(),
            session_id: id.clone(),
            actor: LiveSessionActorWitnessSlot::default(),
        };
        let wrong_session = EphemeralRuntimeHandles {
            service: service.clone(),
            session_id: SessionId::new(),
            actor: slot.clone(),
        };
        assert!(missing.prepare_durability_reload_cleanup().await.is_err());
        assert!(
            wrong_session
                .prepare_durability_reload_cleanup()
                .await
                .is_err()
        );
        let handles = EphemeralRuntimeHandles {
            service: service.clone(),
            session_id: id.clone(),
            actor: slot,
        };
        let prepared = handles.prepare_durability_reload_cleanup().await;
        if prepared.is_err() {
            machine
                .unregister_session(&id)
                .await
                .expect("cleanup failed preparation control");
        }
        prepared.expect("advertised capability must bind the real published actor");
        let before = machine
            .runtime_state(&id)
            .await
            .expect("nonterminal runtime before");
        let outer = service.acquire_runtime_turn_finalization_guard(&id).await;
        let discarded = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            handles.cleanup_after_durability_reload_required().await?;
            handles.cleanup_after_durability_reload_required().await
        })
        .await;
        drop(outer);
        let after = machine.runtime_state(&id).await;
        let still_registered = service.live_session_actor_registered(&id).await;
        let cleanup = machine.unregister_session(&id).await;
        discarded
            .expect("reload cleanup must not reacquire the already-held outer boundary")
            .expect("exact process-local cleanup is idempotent");
        assert!(!still_registered);
        assert!(!actor.is_live());
        assert_eq!(
            after.expect("runtime survives process-local cleanup"),
            before,
            "reload cleanup must not publish a terminal runtime lifecycle"
        );
        cleanup.expect("final native teardown");
    }

    #[tokio::test]
    async fn ephemeral_reload_cleanup_cannot_retire_replacement_actor() {
        let (service, _) = service();
        let slot = LiveSessionActorWitnessSlot::default();
        let (created, _) = service
            .create_session_with_admission_and_witness(request(), None, Some(&slot))
            .await
            .expect("actual first Agent actor");
        let id = created.session_id;
        let original = slot.witness().expect("first actor published by service");
        let handles = EphemeralRuntimeHandles {
            service: service.clone(),
            session_id: id.clone(),
            actor: slot,
        };
        let prepared = handles.prepare_durability_reload_cleanup().await;
        if prepared.is_err() {
            service
                .discard_live_session_actor(&original)
                .await
                .expect("cleanup failed preparation");
        }
        prepared.expect("prepare exact first-actor cleanup authority");
        let initialized_session = service
            .export_session(&id)
            .await
            .expect("export the actual initialized first Agent before retirement");
        assert_eq!(initialized_session.id(), &id);
        assert!(
            initialized_session.session_metadata().is_some(),
            "replacement must resume the real factory-produced metadata"
        );
        assert!(
            service
                .discard_live_session_actor(&original)
                .await
                .expect("retire original actor")
        );
        let mut replacement_request = request();
        replacement_request
            .build
            .get_or_insert_with(Default::default)
            .resume_existing_session(initialized_session);
        let replacement_slot = LiveSessionActorWitnessSlot::default();
        let (replacement, _) = service
            .create_session_with_admission_and_witness(
                replacement_request,
                None,
                Some(&replacement_slot),
            )
            .await
            .expect("actual same-session replacement Agent");
        assert_eq!(replacement.session_id, id);
        let replacement_actor = replacement_slot
            .witness()
            .expect("replacement actor publication");
        assert!(replacement_actor != original);
        let stale_preparation = handles.prepare_durability_reload_cleanup().await;
        let first = handles.cleanup_after_durability_reload_required().await;
        let second = handles.cleanup_after_durability_reload_required().await;
        let current = service.live_session_actor_witness(&id).await;
        let retained_live = replacement_actor.is_live();
        let replacement_view = service.read(&id).await;
        let cleanup = service.discard_live_session_actor(&replacement_actor).await;
        stale_preparation.expect_err(
            "retired immutable actor slot cannot prepare replacement cleanup authority",
        );
        first.expect("stale cleanup is harmless");
        second.expect("stale cleanup remains idempotent");
        assert!(current.as_ref() == Some(&replacement_actor));
        assert!(retained_live);
        assert_eq!(
            replacement_view
                .expect("replacement remains addressable")
                .state
                .session_id,
            id
        );
        assert!(cleanup.expect("cleanup actual replacement"));
    }

    #[tokio::test]
    async fn admitted_transient_context_reaches_only_its_turn_provider_requests() {
        let client = Arc::new(CaptureRequestClient {
            inner: meerkat_client::TestClient::for_provider(meerkat_core::Provider::OpenAI),
            messages: std::sync::Mutex::new(Vec::new()),
        });
        let factory =
            AgentFactory::new(std::env::temp_dir().join("meerkat-transient-runtime-tests"));
        let mut builder = FactoryAgentBuilder::new(factory, Config::default());
        builder.default_llm_client = Some(client.clone());
        let service = Arc::new(EphemeralSessionService::new(builder, 4));
        let machine = Arc::new(MeerkatMachine::ephemeral());
        let created = materialize_ephemeral_runtime_session(&service, &machine, request(), false)
            .await
            .expect("canonical materialization");
        let id = created.session_id;
        let metadata = meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
            transient_turn_context: Some(
                meerkat_core::lifecycle::TurnRequestContext::new("REQUEST_ONLY_PRIVATE_MARKER")
                    .expect("valid context"),
            ),
            transient_turn_context_appends: vec![
                meerkat_core::lifecycle::TurnRequestContext::new("INTERNAL_REQUEST_APPEND_MARKER")
                    .expect("valid internal context"),
            ],
            ..Default::default()
        };
        let result = run_ephemeral_runtime_turn(&machine, &id, "first".into(), Some(metadata))
            .await
            .expect("first admission");
        ephemeral_runtime_completion_result(result).expect("first terminal");
        let first_count = {
            let requests = client.messages.lock().expect("request capture");
            assert!(!requests.is_empty(), "a provider request must execute");
            for request in requests.iter() {
                assert!(request.contains("REQUEST_ONLY_PRIVATE_MARKER"));
                assert!(request.contains("INTERNAL_REQUEST_APPEND_MARKER"));
            }
            requests.len()
        };
        let history = service
            .read_history(&id, meerkat_core::service::SessionHistoryQuery::default())
            .await
            .expect("canonical session history");
        let snapshot = serde_json::to_string(&history).expect("canonical history serialization");
        assert!(!snapshot.contains("REQUEST_ONLY_PRIVATE_MARKER"));
        assert!(!snapshot.contains("INTERNAL_REQUEST_APPEND_MARKER"));
        let result = run_ephemeral_runtime_turn(&machine, &id, "second".into(), None)
            .await
            .expect("successor admission");
        ephemeral_runtime_completion_result(result).expect("successor terminal");
        {
            let requests = client.messages.lock().expect("request capture");
            assert!(requests.len() > first_count);
            for request in &requests[first_count..] {
                assert!(!request.contains("REQUEST_ONLY_PRIVATE_MARKER"));
                assert!(!request.contains("INTERNAL_REQUEST_APPEND_MARKER"));
            }
        }
        machine.unregister_session(&id).await.expect("cleanup");
    }

    #[tokio::test]
    async fn direct_ephemeral_session_runs_and_tears_down_through_machine() {
        let (service, machine) = service();
        let created = materialize_ephemeral_runtime_session(&service, &machine, request(), false)
            .await
            .expect("materialize");
        let id = created.session_id;
        assert!(machine.contains_session(&id).await);
        interrupt_ephemeral_runtime_session(&machine, &id)
            .await
            .expect("idle interrupt is canonical no-op");
        let result = run_ephemeral_runtime_turn(&machine, &id, "hello".into(), None)
            .await
            .expect("runtime turn");
        let result = ephemeral_runtime_completion_result(result).expect("completed result");
        assert_eq!(result.session_id, id);
        let view = service.read(&id).await.expect("session view");
        assert!(!view.state.is_active);
        machine
            .unregister_session(&id)
            .await
            .expect("runtime teardown");
        assert!(!machine.contains_session(&id).await);
        assert!(matches!(
            service.read(&id).await,
            Err(SessionError::NotFound { .. })
        ));
    }

    #[tokio::test]
    async fn failed_materialization_releases_capacity_before_returning() {
        let (service, machine) = service();
        for _ in 0..5 {
            let mut invalid = request();
            invalid.build = Some(meerkat_core::service::SessionBuildOptions {
                max_inline_peer_notifications: Some(-2),
                ..Default::default()
            });
            materialize_ephemeral_runtime_session(&service, &machine, invalid, false)
                .await
                .expect_err("invalid build must fail with no retained actor");
        }
        let created = materialize_ephemeral_runtime_session(&service, &machine, request(), false)
            .await
            .expect("failed attempts released materialization and capacity");
        machine
            .unregister_session(&created.session_id)
            .await
            .expect("cleanup");
    }

    #[tokio::test]
    async fn cancelled_post_commit_materialization_retires_exact_actor_and_releases_capacity() {
        let factory =
            AgentFactory::new(std::env::temp_dir().join("meerkat-cancelled-materialization-tests"));
        let mut builder = FactoryAgentBuilder::new(factory, Config::default());
        builder.default_llm_client = Some(Arc::new(meerkat_client::TestClient::for_provider(
            meerkat_core::Provider::OpenAI,
        )));
        let service = Arc::new(EphemeralSessionService::new(builder, 1));
        let machine = Arc::new(MeerkatMachine::ephemeral());
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let creation = tokio::spawn({
            let service = service.clone();
            let machine = machine.clone();
            async move {
                materialize_ephemeral_runtime_session_inner(
                    &service,
                    &machine,
                    request(),
                    false,
                    Some(MaterializationCommitTestBarrier {
                        entered: entered_tx,
                        release: release_rx,
                    }),
                )
                .await
            }
        });
        let witness = tokio::time::timeout(std::time::Duration::from_secs(10), entered_rx)
            .await
            .expect("materialization must reach the post-commit ingress boundary")
            .expect("committed attachment witness");
        let id = witness.session_id().clone();
        assert_eq!(
            machine.current_executor_attachment_witness(&id).await,
            Some(witness.clone()),
            "the actor is committed, not merely staged"
        );
        assert!(service.live_session_actor_registered(&id).await);

        creation.abort();
        assert!(
            creation
                .await
                .expect_err("outer future was aborted")
                .is_cancelled()
        );
        let _ = release_tx.send(());
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            while machine.contains_session(&id).await
                || service.live_session_actor_registered(&id).await
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("cancellation must complete exact machine and actor retirement");
        assert!(matches!(
            service.read(&id).await,
            Err(SessionError::NotFound { .. })
        ));
        assert!(
            !machine
                .unregister_executor_attachment_if_current(&witness)
                .await
                .expect("retired witness is an idempotent no-op")
        );

        let replacement =
            materialize_ephemeral_runtime_session(&service, &machine, request(), false)
                .await
                .expect("cancelled creation must release max_sessions=1 capacity");
        assert_ne!(replacement.session_id, id);
        let result = run_ephemeral_runtime_turn(
            &machine,
            &replacement.session_id,
            "capacity recovered".into(),
            None,
        )
        .await
        .expect("recovered service admits a real runtime turn");
        ephemeral_runtime_completion_result(result).expect("recovered actor completes");
        machine
            .unregister_session(&replacement.session_id)
            .await
            .expect("recovered actor cleanup");
    }

    /// Holds its first provider request open until the run future is dropped.
    struct BlockingFirstRequestClient {
        inner: meerkat_client::TestClient,
        calls: std::sync::atomic::AtomicUsize,
        started: tokio::sync::Notify,
    }

    #[async_trait::async_trait]
    impl meerkat_client::LlmClient for BlockingFirstRequestClient {
        fn project_replay_messages(
            &self,
            messages: &[meerkat_core::Message],
        ) -> Result<Vec<meerkat_core::Message>, meerkat_client::LlmError> {
            Ok(messages.to_vec())
        }

        fn stream<'a>(
            &'a self,
            request: &'a meerkat_client::LlmRequest,
        ) -> std::pin::Pin<
            Box<
                dyn futures::Stream<
                        Item = Result<meerkat_client::LlmEvent, meerkat_client::LlmError>,
                    > + Send
                    + 'a,
            >,
        > {
            let call = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.started.notify_one();
            if call == 0 {
                Box::pin(futures::stream::pending())
            } else {
                self.inner.stream(request)
            }
        }

        fn provider(&self) -> meerkat_core::Provider {
            self.inner.provider()
        }

        async fn health_check(&self) -> Result<(), meerkat_client::LlmError> {
            self.inner.health_check().await
        }
    }

    #[tokio::test]
    async fn hard_interrupt_of_runtime_backed_run_publishes_cancelled_run_failed() {
        use futures::StreamExt as _;

        let client = Arc::new(BlockingFirstRequestClient {
            inner: meerkat_client::TestClient::for_provider(meerkat_core::Provider::OpenAI),
            calls: std::sync::atomic::AtomicUsize::new(0),
            started: tokio::sync::Notify::new(),
        });
        let factory =
            AgentFactory::new(std::env::temp_dir().join("meerkat-runtime-hard-interrupt-tests"));
        let mut builder = FactoryAgentBuilder::new(factory, Config::default());
        builder.default_llm_client = Some(client.clone());
        let service = Arc::new(EphemeralSessionService::new(builder, 4));
        let machine = Arc::new(MeerkatMachine::ephemeral());
        let created = materialize_ephemeral_runtime_session(&service, &machine, request(), false)
            .await
            .expect("materialize");
        let id = created.session_id;
        let mut events = service
            .subscribe_session_events(&id)
            .await
            .expect("session event stream");

        let turn = tokio::spawn({
            let machine = machine.clone();
            let id = id.clone();
            async move { run_ephemeral_runtime_turn(&machine, &id, "block".into(), None).await }
        });
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            client.started.notified(),
        )
        .await
        .expect("the runtime-backed run must reach its provider request");
        interrupt_ephemeral_runtime_session(&machine, &id)
            .await
            .expect("hard interrupt of the live run");
        let outcome = tokio::time::timeout(std::time::Duration::from_secs(10), turn)
            .await
            .expect("interrupted turn must finish")
            .expect("turn task")
            .expect("admitted prompt");
        assert!(
            matches!(outcome, CompletionOutcome::Cancelled),
            "unexpected outcome: {outcome:?}"
        );

        let (started_run, failed) =
            tokio::time::timeout(std::time::Duration::from_secs(10), async {
                let mut started_run = None;
                while let Some(envelope) = events.next().await {
                    match envelope.payload {
                        AgentEvent::RunStarted { identity, .. } => started_run = identity.run_id,
                        AgentEvent::RunFailed {
                            identity,
                            error_report,
                            ..
                        } => return (started_run, Some((identity, error_report))),
                        AgentEvent::RunCompleted { .. } => {
                            panic!("a hard-interrupted run must not complete")
                        }
                        _ => {}
                    }
                }
                (started_run, None)
            })
            .await
            .expect("the dropped run must publish its terminal on the session stream");
        let started_run = started_run.expect("the interrupted run published RunStarted");
        let (identity, error_report) =
            failed.expect("the dropped run must publish RunFailed before the stream closes");
        assert_eq!(
            error_report.class,
            meerkat_core::event::AgentErrorClass::Cancelled
        );
        assert_eq!(identity.run_id, Some(started_run));

        // The session recovers for an ordinary follow-up turn.
        let result = run_ephemeral_runtime_turn(&machine, &id, "after".into(), None)
            .await
            .expect("follow-up admission");
        ephemeral_runtime_completion_result(result).expect("follow-up completes");
        machine.unregister_session(&id).await.expect("cleanup");
    }

    #[tokio::test]
    async fn concurrent_direct_turns_share_runtime_admission() {
        let (service, machine) = service();
        let created = materialize_ephemeral_runtime_session(&service, &machine, request(), false)
            .await
            .expect("materialize");
        let id = created.session_id;
        let (first, second) = tokio::join!(
            run_ephemeral_runtime_turn(&machine, &id, "first".into(), None),
            run_ephemeral_runtime_turn(&machine, &id, "second".into(), None),
        );
        for result in [first, second] {
            let result = ephemeral_runtime_completion_result(result.expect("admitted prompt"))
                .expect("completed prompt");
            assert_eq!(result.session_id, id);
        }
        machine.unregister_session(&id).await.expect("cleanup");
    }

    #[test]
    fn completion_failure_keeps_exact_typed_evidence() {
        let metadata =
            meerkat_core::event::TurnErrorMetadata::runtime_apply_failure("exact failure");
        let expected = serde_json::to_value(&metadata).unwrap();
        let error = ephemeral_runtime_completion_result(CompletionOutcome::AbandonedWithError {
            reason: "failed attempt".into(),
            error: metadata,
        })
        .expect_err("failure is not success");
        let SessionError::FailedWithData { data, .. } = error else {
            panic!("typed error envelope required")
        };
        assert_eq!(data["code"], "AGENT_ERROR");
        assert_eq!(data["data"]["error"], expected);
    }

    #[test]
    fn admission_rejection_keeps_typed_reason() {
        let reason = meerkat_runtime::RejectReason::NotReady {
            state: meerkat_runtime::RuntimeState::Destroyed,
        };
        let expected = serde_json::to_value(&reason).unwrap();
        let error = EphemeralRuntimeError::Rejected(reason).into_session_error();
        let SessionError::FailedWithData { data, .. } = error else {
            panic!("typed error envelope required")
        };
        assert_eq!(data["code"], "RUNTIME_INPUT_REJECTED");
        assert_eq!(data["data"]["reason"], expected);
    }
}
