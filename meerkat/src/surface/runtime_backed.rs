use std::sync::Arc;

use meerkat_core::lifecycle::core_executor::{
    CoreApplyOutput, CoreExecutor, CoreExecutorBoundaryHandle, CoreExecutorError,
    CoreExecutorInterruptHandle, CoreExecutorPostStopCleanupHandle, CoreExecutorPublicationHandle,
    CoreExecutorTurnFinalizationBoundaryHandle, CoreExecutorTurnFinalizationGuard,
};
use meerkat_core::lifecycle::run_primitive::{
    ConversationContextAppend, RunPrimitive, RuntimeTurnMetadata,
};
use meerkat_core::service::{
    DeferredPromptPolicy, InitialTurnPolicy, StartTurnRequest, StartTurnRuntimeSemantics,
};
use meerkat_core::types::HandlingMode;
use meerkat_core::{
    AgentEvent, EventEnvelope, SurfaceSessionRecoveryContext, SurfaceSessionRecoveryOverrides,
    build_recovered_session,
};
use meerkat_runtime::meerkat_machine::RuntimeBindingsError;
use meerkat_runtime::{
    EnsureRuntimeExecutorAttachment, MeerkatMachine, PendingRuntimeExecutorAttachment,
    RuntimeDriverError, RuntimeExecutorAttachmentWitness,
};
use tokio::sync::mpsc;

#[cfg(all(test, feature = "jsonl-store", not(target_arch = "wasm32")))]
use crate::JsonlStore;
#[cfg(all(test, feature = "jsonl-store", not(target_arch = "wasm32")))]
use crate::MachineSessionArchiveProtocol;
use crate::{
    CreateSessionRequest, FactoryAgentBuilder, MachineServiceTurnCommitProtocol,
    PersistentSessionService, RunResult, Session, SessionAgentBuilder, SessionError, SessionId,
    WorkGraphService,
};
#[cfg(all(test, feature = "jsonl-store", not(target_arch = "wasm32")))]
use meerkat_store::MemoryBlobStore;

const DEFAULT_RUNTIME_BACKED_ARCHIVED_HISTORY_CAPACITY: usize = 1024;

#[cfg(feature = "session-store")]
pub fn build_runtime_backed_service(
    builder: FactoryAgentBuilder,
    max_sessions: usize,
    persistence: crate::PersistenceBundle,
) -> (
    PersistentSessionService<FactoryAgentBuilder>,
    Arc<MeerkatMachine>,
) {
    build_runtime_backed_service_with_capacities(
        builder,
        max_sessions,
        DEFAULT_RUNTIME_BACKED_ARCHIVED_HISTORY_CAPACITY,
        persistence,
    )
}

#[cfg(feature = "session-store")]
pub fn build_runtime_backed_service_with_capacities(
    mut builder: FactoryAgentBuilder,
    active_session_capacity: usize,
    archived_history_capacity: usize,
    persistence: crate::PersistenceBundle,
) -> (
    PersistentSessionService<FactoryAgentBuilder>,
    Arc<MeerkatMachine>,
) {
    let runtime_adapter = persistence.runtime_adapter();
    #[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
    let event_projection = persistence.event_projection();
    let (store, runtime_store, blob_store) = persistence.into_parts();
    builder = builder.with_image_generation_machine(runtime_adapter.clone());
    builder.default_session_store = Some(Arc::new(meerkat_store::StoreAdapter::new(Arc::clone(
        &store,
    ))));
    builder.default_blob_store = Some(blob_store.clone());
    let mut service = PersistentSessionService::new_with_capacities(
        builder,
        active_session_capacity,
        archived_history_capacity,
        store,
        runtime_store,
        blob_store,
    );
    #[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
    if let Some((event_store, projector)) = event_projection {
        service = service.with_event_projection(event_store, projector);
    }
    (service, runtime_adapter)
}

#[derive(Debug, thiserror::Error)]
pub enum SurfaceRuntimeMaterializeError {
    #[error(transparent)]
    Session(#[from] SessionError),
    #[error(transparent)]
    RuntimeBindings(#[from] RuntimeBindingsError),
    #[error(transparent)]
    RuntimeDriver(#[from] RuntimeDriverError),
    #[error("session service returned mismatched session id: expected {expected}, got {actual}")]
    SessionIdMismatch {
        expected: SessionId,
        actual: SessionId,
    },
}

pub struct RuntimeBackedInitialTurn {
    prompt: meerkat_core::types::ContentInput,
    /// Host-attached injected context moved off the eager create request; it
    /// rides into the first turn's `StartTurnRequest` exactly like `prompt`.
    injected_context: Vec<meerkat_core::types::ContentInput>,
    event_tx: Option<mpsc::Sender<EventEnvelope<AgentEvent>>>,
    turn_metadata: RuntimeTurnMetadata,
}

pub fn split_runtime_backed_eager_create_request(
    mut request: CreateSessionRequest,
) -> (CreateSessionRequest, Option<RuntimeBackedInitialTurn>) {
    if request.initial_turn != InitialTurnPolicy::RunImmediately {
        return (request, None);
    }

    // Dogma K10: `build.initial_turn_metadata` is the sole carrier of
    // initial-turn render/skill facts — there is no request-level duplicate
    // left to fold in.
    let turn_metadata = request
        .build
        .as_mut()
        .and_then(|build| build.initial_turn_metadata.take())
        .unwrap_or_default();

    let prompt = std::mem::replace(
        &mut request.prompt,
        meerkat_core::types::ContentInput::Text(String::new()),
    );
    // Injected context is a first-turn submit-work fact; it moves with the
    // prompt onto the initial turn (the deferred create carries none).
    let injected_context = std::mem::take(&mut request.injected_context);
    let event_tx = request.event_tx.take();
    request.initial_turn = InitialTurnPolicy::Defer;
    request.deferred_prompt_policy = DeferredPromptPolicy::Discard;

    (
        request,
        Some(RuntimeBackedInitialTurn {
            prompt,
            injected_context,
            event_tx,
            turn_metadata: meerkat_runtime::runtime_stamped_prompt_turn_metadata(Some(
                turn_metadata,
            )),
        }),
    )
}

fn start_turn_request_from_initial_turn(
    initial_turn: RuntimeBackedInitialTurn,
) -> StartTurnRequest {
    StartTurnRequest {
        prompt: initial_turn.prompt,
        injected_context: initial_turn.injected_context,
        system_prompt: None,
        event_tx: initial_turn.event_tx,
        runtime: StartTurnRuntimeSemantics::new(
            initial_turn
                .turn_metadata
                .handling_mode
                .unwrap_or(HandlingMode::Queue),
            None,
            Vec::new(),
            Some(initial_turn.turn_metadata),
        ),
    }
}

async fn materialize_error_preserves_runtime_session<B: SessionAgentBuilder + 'static>(
    service: &Arc<PersistentSessionService<B>>,
    session_id: &SessionId,
    error: &SurfaceRuntimeMaterializeError,
) -> bool {
    match error {
        SurfaceRuntimeMaterializeError::Session(error) => {
            service
                .service_turn_error_requires_machine_terminal_receipt(session_id, error)
                .await
        }
        _ => false,
    }
}

pub async fn run_runtime_backed_initial_turn_with_machine<B: SessionAgentBuilder + 'static>(
    service: &Arc<PersistentSessionService<B>>,
    adapter: &Arc<MeerkatMachine>,
    session_id: &SessionId,
    initial_turn: RuntimeBackedInitialTurn,
) -> Result<RunResult, SurfaceRuntimeMaterializeError> {
    run_runtime_backed_initial_turn_with_machine_boundary_mode(
        service,
        adapter,
        session_id,
        initial_turn,
        false,
    )
    .await
}

async fn run_runtime_backed_initial_turn_with_machine_boundary_mode<
    B: SessionAgentBuilder + 'static,
>(
    service: &Arc<PersistentSessionService<B>>,
    adapter: &Arc<MeerkatMachine>,
    session_id: &SessionId,
    initial_turn: RuntimeBackedInitialTurn,
    turn_boundary_already_held: bool,
) -> Result<RunResult, SurfaceRuntimeMaterializeError> {
    let admission = service.reserve_runtime_turn_admission(session_id).await?;
    let request = start_turn_request_from_initial_turn(initial_turn);
    let result = if turn_boundary_already_held {
        service
            .run_machine_committed_live_turn_under_runtime_turn_boundary(
                MachineServiceTurnCommitProtocol::from_machine(adapter),
                session_id,
                request,
                admission,
            )
            .await
    } else {
        service
            .run_machine_committed_live_turn(
                MachineServiceTurnCommitProtocol::from_machine(adapter),
                session_id,
                request,
                admission,
            )
            .await
    };
    result.map_err(|(error, _admission)| SurfaceRuntimeMaterializeError::Session(error))
}

async fn rollback_prepared_runtime_registration(
    prepared: &mut meerkat_runtime::PreparedSessionMaterialization,
    turn_boundary_already_held: bool,
) {
    let session_id = prepared.session_id().clone();
    let result = if turn_boundary_already_held {
        prepared
            .rollback_now_under_turn_finalization_boundary()
            .await
    } else {
        prepared.rollback_now().await
    };
    if let Err(error) = result {
        tracing::warn!(
            %session_id,
            %error,
            "failed to roll back exact prepared runtime registration"
        );
    }
}

pub async fn materialize_session<F, B: SessionAgentBuilder + 'static>(
    service: &Arc<PersistentSessionService<B>>,
    adapter: &Arc<MeerkatMachine>,
    session: Session,
    request: CreateSessionRequest,
    executor_factory: F,
) -> Result<RunResult, SurfaceRuntimeMaterializeError>
where
    F: FnOnce(SessionId) -> Box<dyn CoreExecutor> + Send + 'static,
{
    let reserved_admission = service.reserve_create_session_admission().await?;
    materialize_session_with_reserved_admission(
        service,
        adapter,
        session,
        request,
        reserved_admission,
        executor_factory,
    )
    .await
}

/// Reconstruct only the session-service actor for an executor that already
/// owns its stable, non-reentrant turn-finalization boundary.
///
/// This path requires an exact committed attachment and never invokes
/// `executor_factory`, spawns a runtime loop, or owns attachment rollback.
pub async fn materialize_session_under_runtime_turn_boundary<F, B: SessionAgentBuilder + 'static>(
    service: &Arc<PersistentSessionService<B>>,
    adapter: &Arc<MeerkatMachine>,
    session: Session,
    request: CreateSessionRequest,
    executor_factory: F,
) -> Result<RunResult, SurfaceRuntimeMaterializeError>
where
    F: FnOnce(SessionId) -> Box<dyn CoreExecutor>,
{
    let reserved_admission = service.reserve_create_session_admission().await?;
    materialize_session_with_reserved_admission_under_runtime_turn_boundary(
        service,
        adapter,
        session,
        request,
        reserved_admission,
        executor_factory,
    )
    .await
}

/// Reconstruct a service actor under one captured, exact committed executor
/// attachment. Unlike [`materialize_session`], this API has no unique-attach
/// fallback: if `witness` is stale it fails before actor construction and can
/// never create a replacement executor with stale surface state.
pub async fn materialize_attached_session_actor_only<B: SessionAgentBuilder + 'static>(
    service: &Arc<PersistentSessionService<B>>,
    adapter: &Arc<MeerkatMachine>,
    witness: RuntimeExecutorAttachmentWitness,
    session: Session,
    request: CreateSessionRequest,
) -> Result<RunResult, SurfaceRuntimeMaterializeError> {
    let reserved_admission = service.reserve_create_session_admission().await?;
    materialize_attached_session_actor_only_with_reserved_admission(
        service,
        adapter,
        witness,
        session,
        request,
        reserved_admission,
    )
    .await
}

/// Reserved-admission variant of [`materialize_attached_session_actor_only`].
pub async fn materialize_attached_session_actor_only_with_reserved_admission<
    B: SessionAgentBuilder + 'static,
>(
    service: &Arc<PersistentSessionService<B>>,
    adapter: &Arc<MeerkatMachine>,
    witness: RuntimeExecutorAttachmentWitness,
    session: Session,
    request: CreateSessionRequest,
    reserved_admission: crate::RuntimeContextAdmissionGuard,
) -> Result<RunResult, SurfaceRuntimeMaterializeError> {
    materialize_attached_session_actor_only_with_admission_and_boundary_mode(
        service,
        adapter,
        witness,
        session,
        request,
        reserved_admission,
        false,
    )
    .await
}

pub async fn materialize_session_with_reserved_admission<F, B: SessionAgentBuilder + 'static>(
    service: &Arc<PersistentSessionService<B>>,
    adapter: &Arc<MeerkatMachine>,
    session: Session,
    request: CreateSessionRequest,
    reserved_admission: crate::RuntimeContextAdmissionGuard,
    executor_factory: F,
) -> Result<RunResult, SurfaceRuntimeMaterializeError>
where
    F: FnOnce(SessionId) -> Box<dyn CoreExecutor> + Send + 'static,
{
    if let Some(witness) = adapter
        .current_executor_attachment_witness(session.id())
        .await
    {
        return materialize_attached_session_actor_only_with_admission_and_boundary_mode(
            service,
            adapter,
            witness,
            session,
            request,
            reserved_admission,
            false,
        )
        .await;
    }

    let (result, pending) = materialize_session_with_reserved_admission_transaction(
        service,
        adapter,
        session,
        request,
        reserved_admission,
        move |session_id, _witness| executor_factory(session_id),
    )
    .await?;
    pending.commit().await?;
    Ok(result)
}

/// Reserved-admission actor reconstruction for a runtime executor that already
/// owns the service's stable, non-reentrant turn-finalization boundary.
///
/// This is deliberately actor-only: returning a pending executor attachment
/// while the caller holds that boundary would allow a second loop to escape
/// the boundary or deadlock its commit. `executor_factory` is retained only for
/// source compatibility and is never invoked.
pub async fn materialize_session_with_reserved_admission_under_runtime_turn_boundary<
    F,
    B: SessionAgentBuilder + 'static,
>(
    service: &Arc<PersistentSessionService<B>>,
    adapter: &Arc<MeerkatMachine>,
    session: Session,
    request: CreateSessionRequest,
    reserved_admission: crate::RuntimeContextAdmissionGuard,
    executor_factory: F,
) -> Result<RunResult, SurfaceRuntimeMaterializeError>
where
    F: FnOnce(SessionId) -> Box<dyn CoreExecutor>,
{
    drop(executor_factory);
    let session_id = session.id().clone();
    let witness = adapter
        .current_executor_attachment_witness(&session_id)
        .await
        .ok_or_else(|| {
            SurfaceRuntimeMaterializeError::RuntimeDriver(
                RuntimeDriverError::ValidationFailed {
                    reason: format!(
                        "actor-only materialization under a runtime turn boundary requires an existing committed executor attachment for session {session_id}"
                    ),
                },
            )
        })?;
    materialize_attached_session_actor_only_with_admission_and_boundary_mode(
        service,
        adapter,
        witness,
        session,
        request,
        reserved_admission,
        true,
    )
    .await
}

async fn materialize_session_with_reserved_admission_transaction<
    F,
    B: SessionAgentBuilder + 'static,
>(
    service: &Arc<PersistentSessionService<B>>,
    adapter: &Arc<MeerkatMachine>,
    session: Session,
    request: CreateSessionRequest,
    reserved_admission: crate::RuntimeContextAdmissionGuard,
    executor_factory: F,
) -> Result<(RunResult, PendingRuntimeExecutorAttachment), SurfaceRuntimeMaterializeError>
where
    F: FnOnce(SessionId, RuntimeExecutorAttachmentWitness) -> Box<dyn CoreExecutor>
        + Send
        + 'static,
{
    materialize_unique_session_attachment_transaction_with_admission(
        service,
        adapter,
        session,
        request,
        reserved_admission,
        executor_factory,
    )
    .await
}

async fn materialize_unique_session_attachment_transaction_with_admission<
    F,
    B: SessionAgentBuilder + 'static,
>(
    service: &Arc<PersistentSessionService<B>>,
    adapter: &Arc<MeerkatMachine>,
    session: Session,
    mut request: CreateSessionRequest,
    reserved_admission: crate::RuntimeContextAdmissionGuard,
    executor_factory: F,
) -> Result<(RunResult, PendingRuntimeExecutorAttachment), SurfaceRuntimeMaterializeError>
where
    F: FnOnce(SessionId, RuntimeExecutorAttachmentWitness) -> Box<dyn CoreExecutor>
        + Send
        + 'static,
{
    let prepared_session_id = session.id().clone();
    let mut prepared = match adapter
        .prepare_session_materialization(prepared_session_id.clone())
        .await
    {
        Ok(prepared) => prepared,
        Err(error) => {
            return Err(SurfaceRuntimeMaterializeError::RuntimeBindings(error));
        }
    };
    if let Err(error) =
        install_prepared_runtime_interrupt_handle(service, adapter, prepared.bindings()).await
    {
        rollback_prepared_runtime_registration(&mut prepared, false).await;
        return Err(SurfaceRuntimeMaterializeError::RuntimeDriver(error));
    }

    let mut build = request.build.unwrap_or_default();
    build.resume_session = Some(session);
    build.runtime_build_mode =
        meerkat_core::RuntimeBuildMode::SessionOwned(prepared.bindings_clone());
    request.build = Some(build);
    let (request, initial_turn) = split_runtime_backed_eager_create_request(request);

    let create_result = service
        .create_session_with_reserved_admission(request, reserved_admission)
        .await;
    let result = match create_result {
        Ok(result) => result,
        Err(error) => {
            rollback_prepared_runtime_registration(&mut prepared, false).await;
            return Err(SurfaceRuntimeMaterializeError::Session(error));
        }
    };

    if let Err(error) =
        ensure_materialized_session_id_matches(&prepared_session_id, &result.session_id)
    {
        rollback_prepared_runtime_registration(&mut prepared, false).await;
        return Err(error);
    }

    let result = match initial_turn {
        Some(initial_turn) => match run_runtime_backed_initial_turn_with_machine_boundary_mode(
            service,
            adapter,
            &prepared_session_id,
            initial_turn,
            false,
        )
        .await
        {
            Ok(result) => result,
            Err(error) => {
                if materialize_error_preserves_runtime_session(
                    service,
                    &prepared_session_id,
                    &error,
                )
                .await
                {
                    prepared
                        .commit_actor_unattached()
                        .await
                        .map_err(SurfaceRuntimeMaterializeError::RuntimeDriver)?;
                } else {
                    rollback_prepared_runtime_registration(&mut prepared, false).await;
                }
                return Err(error);
            }
        },
        None => result,
    };

    if let Err(error) =
        ensure_materialized_session_id_matches(&prepared_session_id, &result.session_id)
    {
        rollback_prepared_runtime_registration(&mut prepared, false).await;
        return Err(error);
    }

    let executor_session_id = result.session_id.clone();
    let attachment = match prepared
        .ensure_executor_attachment(move |witness| executor_factory(executor_session_id, witness))
        .await
    {
        Ok(EnsureRuntimeExecutorAttachment::Pending(pending)) => pending,
        Ok(EnsureRuntimeExecutorAttachment::Existing(witness)) => {
            rollback_prepared_runtime_registration(&mut prepared, false).await;
            return Err(SurfaceRuntimeMaterializeError::RuntimeDriver(
                RuntimeDriverError::ValidationFailed {
                    reason: format!(
                        "unique session materialization unexpectedly found committed attachment {witness:?}"
                    ),
                },
            ));
        }
        Err(error) => {
            rollback_prepared_runtime_registration(&mut prepared, false).await;
            return Err(SurfaceRuntimeMaterializeError::RuntimeDriver(error));
        }
    };

    Ok((result, attachment))
}

async fn materialize_attached_session_actor_only_with_admission_and_boundary_mode<
    B: SessionAgentBuilder + 'static,
>(
    service: &Arc<PersistentSessionService<B>>,
    adapter: &Arc<MeerkatMachine>,
    witness: RuntimeExecutorAttachmentWitness,
    session: Session,
    mut request: CreateSessionRequest,
    reserved_admission: crate::RuntimeContextAdmissionGuard,
    turn_boundary_already_held: bool,
) -> Result<RunResult, SurfaceRuntimeMaterializeError> {
    let expected_session_id = session.id().clone();
    if witness.session_id() != &expected_session_id {
        return Err(SurfaceRuntimeMaterializeError::RuntimeDriver(
            RuntimeDriverError::StaleAuthority {
                reason: format!(
                    "executor attachment witness for session {} cannot recover actor {}",
                    witness.session_id(),
                    expected_session_id
                ),
            },
        ));
    }

    // Canonical lifecycle lock order is service turn-finalization boundary B
    // before machine mutation fence M. Unregister releases M before acquiring
    // B for surface cleanup, so actor recovery must never hold M while waiting
    // for an ordinary service create to acquire B.
    let locally_owned_turn_boundary = if turn_boundary_already_held {
        None
    } else {
        Some(
            service
                .acquire_runtime_turn_finalization_guard(&expected_session_id)
                .await,
        )
    };
    let mut prepared = adapter
        .prepare_attached_session_actor_recovery(&witness)
        .await?;
    let mut build = request.build.unwrap_or_default();
    build.resume_session = Some(session);
    build.runtime_build_mode =
        meerkat_core::RuntimeBuildMode::SessionOwned(prepared.bindings_clone());
    request.build = Some(build);
    let (request, initial_turn) = split_runtime_backed_eager_create_request(request);

    let result = service
        .create_session_with_reserved_admission_under_runtime_turn_boundary(
            request,
            reserved_admission,
        )
        .await?;
    ensure_materialized_session_id_matches(&expected_session_id, &result.session_id)?;

    // Actor publication is the only lifecycle fact owned by this path. The
    // exact executor already serves and must never be replaced or rolled back.
    prepared.commit_actor()?;
    drop(locally_owned_turn_boundary);

    let result = match initial_turn {
        Some(initial_turn) => {
            run_runtime_backed_initial_turn_with_machine_boundary_mode(
                service,
                adapter,
                &expected_session_id,
                initial_turn,
                turn_boundary_already_held,
            )
            .await?
        }
        None => result,
    };
    ensure_materialized_session_id_matches(&expected_session_id, &result.session_id)?;
    Ok(result)
}

pub async fn install_prepared_runtime_interrupt_handle<B: SessionAgentBuilder + 'static>(
    service: &Arc<PersistentSessionService<B>>,
    adapter: &Arc<MeerkatMachine>,
    bindings: &meerkat_core::SessionRuntimeBindings,
) -> Result<(), RuntimeDriverError> {
    let session_id = bindings.session_id();
    adapter
        .install_prepared_session_executor_handles(
            bindings,
            Arc::new(PersistentRuntimeInterruptHandle {
                service: Arc::clone(service),
                adapter: Arc::clone(adapter),
                session_id: session_id.clone(),
            }),
            persistent_runtime_post_stop_cleanup_handle(Arc::clone(service), session_id.clone()),
        )
        .await
}

/// Install prepared runtime handles whose cleanup is bound to the exact actor
/// witness later published into `actor_witness_slot` by the session service.
pub async fn install_prepared_runtime_interrupt_handle_for_actor_slot<
    B: SessionAgentBuilder + 'static,
>(
    service: &Arc<PersistentSessionService<B>>,
    adapter: &Arc<MeerkatMachine>,
    bindings: &meerkat_core::SessionRuntimeBindings,
    actor_witness_slot: crate::LiveSessionActorWitnessSlot,
) -> Result<(), RuntimeDriverError> {
    let session_id = bindings.session_id();
    adapter
        .install_prepared_session_executor_handles(
            bindings,
            Arc::new(PersistentRuntimeInterruptHandle {
                service: Arc::clone(service),
                adapter: Arc::clone(adapter),
                session_id: session_id.clone(),
            }),
            persistent_runtime_post_stop_cleanup_handle_for_actor_slot(
                Arc::clone(service),
                session_id.clone(),
                actor_witness_slot,
            ),
        )
        .await
}

#[cfg(feature = "comms")]
pub async fn configure_peer_ingress<B: SessionAgentBuilder + 'static>(
    adapter: &Arc<MeerkatMachine>,
    service: &Arc<PersistentSessionService<B>>,
    session_id: &SessionId,
    keep_alive: bool,
) -> Result<(), RuntimeDriverError> {
    let comms_rt = service.comms_runtime(session_id).await;
    adapter
        .update_peer_ingress_context(session_id, keep_alive, comms_rt)
        .await
        .map(|_spawned| ())
}

pub fn default_persistent_executor<B: SessionAgentBuilder + 'static>(
    service: Arc<PersistentSessionService<B>>,
    adapter: Arc<MeerkatMachine>,
    session_id: SessionId,
) -> Box<dyn CoreExecutor> {
    Box::new(PersistentRuntimeExecutor::new(service, adapter, session_id))
}

/// Compile-time guard that the runtime-backed executor + schedule-host layer
/// stay generic over the session-agent builder `B` (not re-pinned to a concrete
/// `FactoryAgentBuilder`). Embedders with a custom builder — e.g. mobkit's
/// callback builder that materializes/resumes sessions through their host SDK —
/// must be able to drive runtime-backed sessions and scheduled firing through
/// their own build path. If these are ever re-pinned to a concrete builder this
/// fails to compile.
#[cfg(test)]
#[allow(dead_code)]
fn assert_runtime_backed_is_builder_generic<B: SessionAgentBuilder + 'static>(
    service: Arc<PersistentSessionService<B>>,
    adapter: Arc<MeerkatMachine>,
    session_id: SessionId,
) -> Box<dyn CoreExecutor> {
    default_persistent_executor::<B>(service, adapter, session_id)
}

pub fn default_persistent_executor_with_workgraph_service<B: SessionAgentBuilder + 'static>(
    service: Arc<PersistentSessionService<B>>,
    adapter: Arc<MeerkatMachine>,
    session_id: SessionId,
    workgraph_service: WorkGraphService,
) -> Box<dyn CoreExecutor> {
    Box::new(PersistentRuntimeExecutor::new_with_workgraph_service(
        service,
        adapter,
        session_id,
        workgraph_service,
    ))
}

pub struct PersistentRuntimeExecutor<B: SessionAgentBuilder> {
    service: Arc<PersistentSessionService<B>>,
    adapter: Arc<MeerkatMachine>,
    session_id: SessionId,
    workgraph_service: Option<WorkGraphService>,
}

struct PersistentRuntimeBoundaryHandle<B: SessionAgentBuilder> {
    service: Arc<PersistentSessionService<B>>,
    adapter: Arc<MeerkatMachine>,
    session_id: SessionId,
}

#[async_trait::async_trait]
impl<B: SessionAgentBuilder + 'static> CoreExecutorBoundaryHandle
    for PersistentRuntimeBoundaryHandle<B>
{
    async fn cancel_after_boundary(
        &self,
        expected_run_id: &meerkat_core::RunId,
        _reason: String,
    ) -> Result<(), CoreExecutorError> {
        self.service
            .cancel_after_boundary_with_machine_authority(
                &self.session_id,
                expected_run_id,
                self.adapter.session_control_authority(),
            )
            .await
            .or_else(|error| match error {
                SessionError::NotRunning { .. } => Ok(()),
                error => Err(error),
            })
            .map_err(|error| CoreExecutorError::control_failed_runtime(error.to_string()))
    }

    async fn prepare_system_context_at_boundary(
        &self,
        expected_run_id: &meerkat_core::RunId,
        appends: Vec<meerkat_core::PendingSystemContextAppend>,
    ) -> Result<
        meerkat_core::lifecycle::CoreBoundaryStageOutput,
        meerkat_core::lifecycle::CoreBoundaryStageError,
    > {
        self.service
            .prepare_live_system_context_boundary(&self.session_id, expected_run_id, appends)
            .await
    }
}

struct PersistentRuntimeInterruptHandle<B: SessionAgentBuilder> {
    service: Arc<PersistentSessionService<B>>,
    adapter: Arc<MeerkatMachine>,
    session_id: SessionId,
}

struct PersistentRuntimePublicationHandle<B: SessionAgentBuilder> {
    service: Arc<PersistentSessionService<B>>,
    session_id: SessionId,
}

struct PersistentRuntimePostStopCleanupHandle<B: SessionAgentBuilder> {
    service: Arc<PersistentSessionService<B>>,
    session_id: SessionId,
    actor_witness_slot: Option<crate::LiveSessionActorWitnessSlot>,
}

struct PersistentRuntimeTurnFinalizationBoundaryHandle<B: SessionAgentBuilder> {
    service: Arc<PersistentSessionService<B>>,
    session_id: SessionId,
}

/// Build the cloneable terminal-publication authority for a persistent
/// runtime-backed session.
///
/// Surface-specific executors use this handle for lifecycle paths that run
/// outside their mutable executor loop (for example, stop terminalization).
pub fn persistent_runtime_publication_handle<B: SessionAgentBuilder + 'static>(
    service: Arc<PersistentSessionService<B>>,
    session_id: SessionId,
) -> Arc<dyn CoreExecutorPublicationHandle> {
    Arc::new(PersistentRuntimePublicationHandle {
        service,
        session_id,
    })
}

/// Build the cloneable service-side cleanup authority retained by the runtime
/// entry until its exact generated unregister transaction completes.
pub fn persistent_runtime_post_stop_cleanup_handle<B: SessionAgentBuilder + 'static>(
    service: Arc<PersistentSessionService<B>>,
    session_id: SessionId,
) -> Arc<dyn CoreExecutorPostStopCleanupHandle> {
    Arc::new(PersistentRuntimePostStopCleanupHandle {
        service,
        session_id,
        actor_witness_slot: None,
    })
}

/// Build exact service-side cleanup authority for an actor-materialization
/// transaction.
///
/// The slot is populated by `PersistentSessionService` at the actor registry
/// insertion boundary.  Cleanup that runs before publication is a no-op;
/// cleanup that runs afterwards compare-and-removes only that incarnation.
pub fn persistent_runtime_post_stop_cleanup_handle_for_actor_slot<
    B: SessionAgentBuilder + 'static,
>(
    service: Arc<PersistentSessionService<B>>,
    session_id: SessionId,
    actor_witness_slot: crate::LiveSessionActorWitnessSlot,
) -> Arc<dyn CoreExecutorPostStopCleanupHandle> {
    Arc::new(PersistentRuntimePostStopCleanupHandle {
        service,
        session_id,
        actor_witness_slot: Some(actor_witness_slot),
    })
}

/// Build the stable outer mutation boundary shared by runtime-loop, direct,
/// and non-turn session writers for one SessionId.
pub fn persistent_runtime_turn_finalization_boundary_handle<B: SessionAgentBuilder + 'static>(
    service: Arc<PersistentSessionService<B>>,
    session_id: SessionId,
) -> Arc<dyn CoreExecutorTurnFinalizationBoundaryHandle> {
    Arc::new(PersistentRuntimeTurnFinalizationBoundaryHandle {
        service,
        session_id,
    })
}

#[async_trait::async_trait]
impl<B: SessionAgentBuilder + 'static> CoreExecutorTurnFinalizationBoundaryHandle
    for PersistentRuntimeTurnFinalizationBoundaryHandle<B>
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

#[async_trait::async_trait]
impl<B: SessionAgentBuilder + 'static> CoreExecutorPostStopCleanupHandle
    for PersistentRuntimePostStopCleanupHandle<B>
{
    async fn cleanup_after_runtime_stop_terminalized(&self) -> Result<(), CoreExecutorError> {
        if let Some(actor_witness_slot) = self.actor_witness_slot.as_ref() {
            let Some(witness) = actor_witness_slot.witness() else {
                return Ok(());
            };
            let Some(lease) = self
                .service
                .acquire_live_session_actor_turn_boundary_lease_exact(&witness)
                .await
                .map_err(|error| CoreExecutorError::control_failed_runtime(error.to_string()))?
            else {
                return Ok(());
            };
            return self
                .service
                .discard_live_session_actor(&lease)
                .await
                .map(|_| ())
                .map_err(|error| CoreExecutorError::control_failed_runtime(error.to_string()));
        }
        match self
            .service
            .discard_live_session_after_runtime_stop_terminalized(&self.session_id)
            .await
        {
            Ok(()) | Err(SessionError::NotFound { .. }) => Ok(()),
            Err(error) => Err(CoreExecutorError::control_failed_runtime(error.to_string())),
        }
    }

    async fn cleanup_after_runtime_stop_terminalized_under_turn_finalization_boundary(
        &self,
    ) -> Result<(), CoreExecutorError> {
        if let Some(actor_witness_slot) = self.actor_witness_slot.as_ref() {
            let Some(witness) = actor_witness_slot.witness() else {
                return Ok(());
            };
            return self
                .service
                .discard_live_session_actor_under_runtime_turn_boundary(&witness)
                .await
                .map(|_| ())
                .map_err(|error| CoreExecutorError::control_failed_runtime(error.to_string()));
        }
        match self
            .service
            .discard_live_session_after_runtime_stop_terminalized_under_runtime_turn_boundary(
                &self.session_id,
            )
            .await
        {
            Ok(()) | Err(SessionError::NotFound { .. }) => Ok(()),
            Err(error) => Err(CoreExecutorError::control_failed_runtime(error.to_string())),
        }
    }
}

#[async_trait::async_trait]
impl<B: SessionAgentBuilder + 'static> CoreExecutorPublicationHandle
    for PersistentRuntimePublicationHandle<B>
{
    async fn publish_interaction_terminals(
        &self,
        events: &[AgentEvent],
    ) -> Result<
        Vec<meerkat_core::lifecycle::core_executor::CoreInteractionTerminalPublicationReceipt>,
        CoreExecutorError,
    > {
        self.service
            .publish_interaction_terminals_exact_batch(&self.session_id, events)
            .await
            .map_err(CoreExecutorError::apply_failed_from_session_error)
    }
}

#[async_trait::async_trait]
impl<B: SessionAgentBuilder + 'static> CoreExecutorInterruptHandle
    for PersistentRuntimeInterruptHandle<B>
{
    async fn hard_cancel_current_run(&self, _reason: String) -> Result<(), CoreExecutorError> {
        self.service
            .interrupt_with_machine_authority(
                &self.session_id,
                self.adapter.session_control_authority(),
            )
            .await
            .or_else(|error| match error {
                SessionError::NotRunning { .. } => Ok(()),
                error => Err(error),
            })
            .map_err(|error| CoreExecutorError::control_failed_runtime(error.to_string()))
    }
}

impl<B: SessionAgentBuilder + 'static> PersistentRuntimeExecutor<B> {
    pub fn new(
        service: Arc<PersistentSessionService<B>>,
        adapter: Arc<MeerkatMachine>,
        session_id: SessionId,
    ) -> Self {
        Self {
            service,
            adapter,
            session_id,
            workgraph_service: None,
        }
    }

    pub fn new_with_workgraph_service(
        service: Arc<PersistentSessionService<B>>,
        adapter: Arc<MeerkatMachine>,
        session_id: SessionId,
        workgraph_service: WorkGraphService,
    ) -> Self {
        Self {
            service,
            adapter,
            session_id,
            workgraph_service: Some(workgraph_service),
        }
    }

    async fn authoritative_non_archived_session_after_not_found<F>(
        &self,
        not_found: &SessionError,
        authority_failure: F,
    ) -> Result<Session, CoreExecutorError>
    where
        F: Fn(String) -> CoreExecutorError,
    {
        let Some(session) = self
            .service
            .load_authoritative_session(&self.session_id)
            .await
            .map_err(|error| authority_failure(error.to_string()))?
        else {
            return Err(CoreExecutorError::session_unavailable_requires_teardown(
                not_found.to_string(),
            ));
        };
        if self
            .service
            .session_archived_by_authority(&self.session_id, &session)
            .await
            .map_err(|error| authority_failure(error.to_string()))?
        {
            return Err(CoreExecutorError::archived_session_requires_teardown(
                not_found.to_string(),
            ));
        }
        Ok(session)
    }
}

async fn validate_workgraph_attention_primitive(
    workgraph_service: Option<&WorkGraphService>,
    primitive: &RunPrimitive,
) -> Result<(), CoreExecutorError> {
    let Some(workgraph_service) = workgraph_service else {
        return Ok(());
    };
    let projection = match primitive.turn_metadata() {
        Some(metadata) => {
            crate::workgraph_attention_projection_from_overlay(metadata.turn_tool_overlay.as_ref())
                .map_err(|error| {
                    CoreExecutorError::apply_failed_primitive_rejected(error.to_string())
                })?
        }
        None => None,
    };
    let Some(projection) = projection else {
        return Ok(());
    };
    crate::validate_workgraph_attention_projection_current(workgraph_service, &projection)
        .await
        .map_err(|error| CoreExecutorError::apply_failed_primitive_rejected(error.to_string()))
}

fn pending_system_context_appends(
    appends: &[ConversationContextAppend],
) -> Vec<meerkat_core::PendingSystemContextAppend> {
    appends
        .iter()
        .map(|append| meerkat_core::PendingSystemContextAppend {
            content: append.content.clone(),
            source: Some(append.key.clone()),
            idempotency_key: Some(append.key.clone()),
            // Durable keyed conversation context append — not a transient steer.
            source_kind: meerkat_core::session::SystemContextSource::Normal,
            accepted_at: meerkat_core::time_compat::SystemTime::now(),
            peer_response_terminal: None,
        })
        .collect()
}

fn start_turn_request_from_primitive(
    primitive: &RunPrimitive,
) -> Result<meerkat_core::service::StartTurnRequest, CoreExecutorError> {
    let metadata = primitive.turn_metadata();
    let pre_turn_context_appends = match primitive {
        RunPrimitive::StagedInput(staged)
            if primitive.is_peer_response_terminal_context_and_run() =>
        {
            pending_system_context_appends(&staged.context_appends)
        }
        _ => Vec::new(),
    };

    Ok(meerkat_core::service::StartTurnRequest {
        injected_context: Vec::new(),
        prompt: primitive.extract_content_input(),
        system_prompt: None,
        event_tx: None,
        runtime: StartTurnRuntimeSemantics::new(
            HandlingMode::Queue,
            None,
            pre_turn_context_appends,
            metadata.cloned(),
        )
        .with_typed_turn_appends(primitive.typed_turn_appends()),
    })
}

#[async_trait::async_trait]
impl<B: SessionAgentBuilder + 'static> CoreExecutor for PersistentRuntimeExecutor<B> {
    fn boundary_handle(&self) -> Option<Arc<dyn CoreExecutorBoundaryHandle>> {
        Some(Arc::new(PersistentRuntimeBoundaryHandle {
            service: Arc::clone(&self.service),
            adapter: Arc::clone(&self.adapter),
            session_id: self.session_id.clone(),
        }))
    }

    fn interrupt_handle(&self) -> Option<Arc<dyn CoreExecutorInterruptHandle>> {
        Some(Arc::new(PersistentRuntimeInterruptHandle {
            service: Arc::clone(&self.service),
            adapter: Arc::clone(&self.adapter),
            session_id: self.session_id.clone(),
        }))
    }

    fn publication_handle(&self) -> Option<Arc<dyn CoreExecutorPublicationHandle>> {
        Some(persistent_runtime_publication_handle(
            Arc::clone(&self.service),
            self.session_id.clone(),
        ))
    }

    fn machine_managed_post_stop_unregister(&self) -> bool {
        true
    }

    fn post_stop_cleanup_handle(&self) -> Option<Arc<dyn CoreExecutorPostStopCleanupHandle>> {
        Some(persistent_runtime_post_stop_cleanup_handle(
            Arc::clone(&self.service),
            self.session_id.clone(),
        ))
    }

    fn turn_finalization_boundary_handle(
        &self,
    ) -> Option<Arc<dyn CoreExecutorTurnFinalizationBoundaryHandle>> {
        Some(persistent_runtime_turn_finalization_boundary_handle(
            Arc::clone(&self.service),
            self.session_id.clone(),
        ))
    }

    async fn apply(
        &mut self,
        run_id: meerkat_core::lifecycle::RunId,
        primitive: RunPrimitive,
    ) -> Result<CoreApplyOutput, CoreExecutorError> {
        validate_workgraph_attention_primitive(self.workgraph_service.as_ref(), &primitive).await?;
        if let Some(reason) = primitive.peer_response_terminal_apply_intent_violation() {
            return Err(CoreExecutorError::apply_failed_primitive_rejected(
                reason.to_string(),
            ));
        }

        if primitive.is_context_only_apply_without_turn() {
            let RunPrimitive::StagedInput(staged) = &primitive else {
                unreachable!("context-only apply without turn only matches staged primitives");
            };
            let appends = pending_system_context_appends(&staged.context_appends);
            let boundary = staged.boundary;
            let contributing_input_ids = staged.contributing_input_ids.clone();
            match self
                .service
                .apply_runtime_context_appends_with_boundary(
                    &self.session_id,
                    run_id.clone(),
                    appends.clone(),
                    boundary,
                    contributing_input_ids.clone(),
                )
                .await
            {
                Ok(output) => return Ok(output),
                Err(error @ SessionError::NotFound { .. }) => {
                    let session = self
                        .authoritative_non_archived_session_after_not_found(
                            &error,
                            CoreExecutorError::apply_failed_runtime_context,
                        )
                        .await?;
                    let recovered = build_recovered_session(
                        session.clone(),
                        &SurfaceSessionRecoveryOverrides::default(),
                        SurfaceSessionRecoveryContext::default(),
                    )
                    .map_err(|error| {
                        CoreExecutorError::apply_failed_runtime_context(error.to_string())
                    })?;
                    let service = Arc::clone(&self.service);
                    let adapter = Arc::clone(&self.adapter);
                    let workgraph_service = self.workgraph_service.clone();
                    match Box::pin(materialize_session_under_runtime_turn_boundary(
                        &self.service,
                        &self.adapter,
                        session,
                        recovered.into_deferred_create_request(),
                        move |session_id| match workgraph_service {
                            Some(workgraph_service) => {
                                default_persistent_executor_with_workgraph_service(
                                    Arc::clone(&service),
                                    Arc::clone(&adapter),
                                    session_id,
                                    workgraph_service,
                                )
                            }
                            None => default_persistent_executor(
                                Arc::clone(&service),
                                Arc::clone(&adapter),
                                session_id,
                            ),
                        },
                    ))
                    .await
                    {
                        Ok(_) => {}
                        Err(SurfaceRuntimeMaterializeError::Session(
                            error @ SessionError::NotFound { .. },
                        )) => {
                            return Err(
                                match self
                                    .authoritative_non_archived_session_after_not_found(
                                        &error,
                                        CoreExecutorError::apply_failed_runtime_context,
                                    )
                                    .await
                                {
                                    Err(teardown) => teardown,
                                    Ok(_) => {
                                        CoreExecutorError::session_unavailable_requires_teardown(
                                            error.to_string(),
                                        )
                                    }
                                },
                            );
                        }
                        Err(error) => {
                            return Err(CoreExecutorError::apply_failed_runtime_context(
                                error.to_string(),
                            ));
                        }
                    }
                    return match self
                        .service
                        .apply_runtime_context_appends_with_boundary(
                            &self.session_id,
                            run_id,
                            appends,
                            boundary,
                            contributing_input_ids,
                        )
                        .await
                    {
                        Ok(output) => Ok(output),
                        Err(error @ SessionError::NotFound { .. }) => {
                            match self
                                .authoritative_non_archived_session_after_not_found(
                                    &error,
                                    CoreExecutorError::apply_failed_runtime_context,
                                )
                                .await
                            {
                                Err(teardown) => Err(teardown),
                                Ok(_) => {
                                    Err(CoreExecutorError::session_unavailable_requires_teardown(
                                        error.to_string(),
                                    ))
                                }
                            }
                        }
                        Err(error) => Err(CoreExecutorError::apply_failed_runtime_context(
                            error.to_string(),
                        )),
                    };
                }
                Err(error) => {
                    return Err(CoreExecutorError::apply_failed_runtime_context(
                        error.to_string(),
                    ));
                }
            }
        }

        let boundary = primitive.apply_boundary();
        let contributing_input_ids = primitive.contributing_input_ids().to_vec();
        let mut req = start_turn_request_from_primitive(&primitive)?;
        super::inject_workgraph_attention_turn_overlay(
            self.service.as_ref(),
            self.workgraph_service.as_ref(),
            &self.session_id,
            &mut req,
        )
        .await
        .map_err(|error| CoreExecutorError::apply_failed_primitive_rejected(error.to_string()))?;
        match self
            .service
            .apply_runtime_turn(
                &self.session_id,
                run_id,
                req,
                boundary,
                contributing_input_ids,
            )
            .await
        {
            Ok(output) => Ok(output),
            Err(error @ SessionError::NotFound { .. }) => {
                match self
                    .authoritative_non_archived_session_after_not_found(
                        &error,
                        CoreExecutorError::apply_failed_runtime_turn,
                    )
                    .await
                {
                    Err(teardown) => Err(teardown),
                    Ok(_) => Err(CoreExecutorError::session_unavailable_requires_teardown(
                        error.to_string(),
                    )),
                }
            }
            Err(error) => Err(CoreExecutorError::apply_failed_from_session_error(error)),
        }
    }

    async fn checkpoint_committed_session_snapshot(
        &mut self,
        session_snapshot: &[u8],
    ) -> Result<(), CoreExecutorError> {
        self.service
            .checkpoint_committed_runtime_session_snapshot_under_runtime_turn_boundary(
                &self.session_id,
                session_snapshot,
            )
            .await
            .map_err(CoreExecutorError::apply_failed_from_session_error)
    }

    async fn reconcile_committed_compaction_projections(
        &mut self,
        intents: &[meerkat_core::CompactionProjectionIntent],
    ) -> Result<(), CoreExecutorError> {
        self.service
            .reconcile_runtime_compaction_projections(&self.session_id, intents.to_vec())
            .await
            .map_err(|error| CoreExecutorError::Internal(error.to_string()))
    }

    async fn abort_uncommitted_compaction_projections(&mut self) -> Result<(), CoreExecutorError> {
        self.service
            .abort_uncommitted_compaction_projections(&self.session_id)
            .await
            .map_err(|error| CoreExecutorError::Internal(error.to_string()))
    }

    async fn cancel_after_boundary(&mut self, _reason: String) -> Result<(), CoreExecutorError> {
        self.service
            .cancel_current_after_boundary_with_machine_authority(
                &self.session_id,
                self.adapter.session_control_authority(),
            )
            .await
            .map_err(|error| CoreExecutorError::control_failed_runtime(error.to_string()))
    }

    async fn publish_interaction_terminals(
        &mut self,
        events: &[AgentEvent],
    ) -> Result<
        Vec<meerkat_core::lifecycle::core_executor::CoreInteractionTerminalPublicationReceipt>,
        CoreExecutorError,
    > {
        PersistentRuntimePublicationHandle {
            service: Arc::clone(&self.service),
            session_id: self.session_id.clone(),
        }
        .publish_interaction_terminals(events)
        .await
    }

    async fn stop_runtime_executor(&mut self, _reason: String) -> Result<(), CoreExecutorError> {
        Ok(())
    }

    async fn cleanup_after_runtime_stop_terminalized(&mut self) -> Result<(), CoreExecutorError> {
        let discard_result = self
            .service
            .discard_live_session_after_runtime_stop_terminalized(&self.session_id)
            .await;
        match discard_result {
            Ok(()) | Err(SessionError::NotFound { .. }) => Ok(()),
            Err(error) => Err(CoreExecutorError::control_failed_runtime(error.to_string())),
        }
    }
}

fn ensure_materialized_session_id_matches(
    expected: &SessionId,
    actual: &SessionId,
) -> Result<(), SurfaceRuntimeMaterializeError> {
    if actual == expected {
        return Ok(());
    }

    Err(SurfaceRuntimeMaterializeError::SessionIdMismatch {
        expected: expected.clone(),
        actual: actual.clone(),
    })
}

#[cfg(test)]
mod typed_transcript_contract_tests {
    use super::*;
    use meerkat_core::lifecycle::run_primitive::CoreRenderable;

    #[test]
    fn start_turn_request_carries_typed_turn_appends() {
        let primitive = RunPrimitive::StagedInput(
            meerkat_core::lifecycle::run_primitive::StagedRunInput {
                boundary: meerkat_core::lifecycle::run_primitive::RunApplyBoundary::RunStart,
                appends: vec![meerkat_core::lifecycle::run_primitive::ConversationAppend {
                    role:
                        meerkat_core::lifecycle::run_primitive::ConversationAppendRole::SystemNotice,
                    content: CoreRenderable::SystemNotice {
                        kind: meerkat_core::types::SystemNoticeKind::Comms,
                        body: Some("typed notice".to_string()),
                        blocks: Vec::new(),
                    },
                }],
                context_appends: Vec::new(),
                contributing_input_ids: vec![meerkat_core::lifecycle::InputId::new()],
                turn_metadata: None,
            },
        );

        let req = start_turn_request_from_primitive(&primitive).expect("request");

        assert_eq!(
            req.runtime.typed_turn_appends,
            primitive.typed_turn_appends()
        );
        assert_eq!(
            req.prompt,
            meerkat_core::types::ContentInput::Text(String::new())
        );
    }
}

#[cfg(all(test, feature = "jsonl-store", not(target_arch = "wasm32")))]
#[allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use meerkat_core::lifecycle::run_primitive::CoreRenderable;
    use meerkat_core::service::SessionService;

    #[cfg(all(
        feature = "openai",
        feature = "openai-realtime",
        not(target_arch = "wasm32")
    ))]
    use std::collections::VecDeque;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    #[cfg(all(
        feature = "openai",
        feature = "openai-realtime",
        not(target_arch = "wasm32")
    ))]
    use async_trait::async_trait;
    use meerkat_client::{LlmClient, LlmDoneOutcome, LlmEvent, LlmRequest, TestClient};
    use meerkat_core::SessionBuildOptions;
    use meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata;
    use meerkat_llm_core::{LlmError, LlmStream};
    #[cfg(all(
        feature = "openai",
        feature = "openai-realtime",
        not(target_arch = "wasm32")
    ))]
    use meerkat_openai::live::{
        OpenAiLiveCallTarget, OpenAiLiveClientEvent, OpenAiLiveServerEvent, OpenAiLiveSession,
        OpenAiLiveSessionFactory,
    };
    use meerkat_runtime::SessionServiceRuntimeExt;
    use meerkat_runtime::completion::CompletionOutcome;
    #[cfg(all(
        feature = "openai",
        feature = "openai-realtime",
        not(target_arch = "wasm32")
    ))]
    use meerkat_runtime::{
        HydratedSessionLlmState, ResolvedSessionLlmReconfigure, SessionLlmCapabilitySurface,
        SessionLlmCapabilitySurfaceStatus, SessionLlmReconfigureHost, SessionLlmReconfigureRequest,
    };
    use meerkat_runtime::{Input, PromptInput, RuntimeState};
    use tempfile::TempDir;
    #[cfg(all(
        feature = "openai",
        feature = "openai-realtime",
        not(target_arch = "wasm32")
    ))]
    use tokio::sync::Mutex;
    use tokio::sync::Notify;
    use tokio::time::Duration;

    #[cfg(feature = "comms")]
    use crate::CommsRuntime;
    use crate::{PersistenceBundle, SessionStore, SessionStoreError};
    #[test]
    fn run_primitive_carries_runtime_metadata_into_start_turn_request() {
        let skill = meerkat_core::skills::SkillKey::builtin(
            meerkat_core::skills::SkillName::parse("runtime-metadata").expect("valid skill"),
        );
        let metadata = RuntimeTurnMetadata {
            execution_kind: Some(meerkat_core::lifecycle::RuntimeExecutionKind::ResumePending),
            model: Some(meerkat_core::lifecycle::run_primitive::ModelId::new(
                "model-from-runtime",
            )),
            handling_mode: Some(meerkat_core::types::HandlingMode::Steer),
            render_metadata: Some(meerkat_core::types::RenderMetadata {
                class: meerkat_core::types::RenderClass::FlowStep,
                salience: meerkat_core::types::RenderSalience::Important,
            }),
            skill_references: Some(vec![skill]),
            turn_tool_overlay: Some(meerkat_core::service::TurnToolOverlay {
                allowed_tools: Some(vec!["runtime_tool".into()]),
                blocked_tools: None,
                dispatch_context: Default::default(),
            }),
            ..Default::default()
        };
        let primitive =
            RunPrimitive::StagedInput(meerkat_core::lifecycle::run_primitive::StagedRunInput {
                boundary: meerkat_core::lifecycle::run_primitive::RunApplyBoundary::RunStart,
                appends: vec![meerkat_core::lifecycle::run_primitive::ConversationAppend {
                    role: meerkat_core::lifecycle::run_primitive::ConversationAppendRole::User,
                    content: CoreRenderable::Text {
                        text: "hello".to_string(),
                    },
                }],
                context_appends: Vec::new(),
                contributing_input_ids: vec![meerkat_core::lifecycle::InputId::new()],
                turn_metadata: Some(metadata.clone()),
            });

        let req = start_turn_request_from_primitive(&primitive)
            .expect("metadata should be carried, not rejected");

        assert_eq!(
            req.runtime.handling_mode,
            meerkat_core::types::HandlingMode::Queue
        );
        assert_eq!(req.runtime.turn_tool_overlay, None);
        assert_eq!(
            req.runtime.typed_turn_appends,
            primitive.typed_turn_appends()
        );
        assert_eq!(req.runtime.turn_metadata, Some(metadata));
        assert_eq!(
            req.runtime
                .turn_metadata
                .as_ref()
                .and_then(|metadata| metadata.execution_kind),
            Some(meerkat_core::lifecycle::RuntimeExecutionKind::ResumePending)
        );
    }

    fn make_request(build: SessionBuildOptions) -> CreateSessionRequest {
        CreateSessionRequest {
            injected_context: Vec::new(),
            model: "gpt-5.4".to_string(),
            prompt: meerkat_core::ContentInput::Text(String::new()),
            system_prompt: crate::SystemPromptOverride::Set(
                "surface runtime regression".to_string(),
            ),
            max_tokens: None,
            event_tx: None,
            initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
            deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
            build: Some(build),
            labels: None,
        }
    }

    fn has_applied_runtime_context(session: &Session, source: &str, text: &str) -> bool {
        session
            .system_context_state()
            .unwrap_or_default()
            .applied()
            .iter()
            .any(|append| {
                append.source.as_deref() == Some(source)
                    && append.content.render_text().contains(text)
            })
    }

    fn runtime_output_session_snapshot(output: &CoreApplyOutput) -> Session {
        serde_json::from_slice(
            output
                .session_snapshot
                .as_deref()
                .expect("runtime output should carry a machine-owned session snapshot"),
        )
        .expect("runtime session snapshot should deserialize")
    }

    async fn build_default_persistence(
        session_dir: PathBuf,
    ) -> Result<PersistenceBundle, SessionStoreError> {
        let jsonl_store = Arc::new(JsonlStore::new(session_dir));
        jsonl_store
            .init()
            .await
            .map_err(|error| SessionStoreError::Internal(error.to_string()))?;
        Ok(PersistenceBundle::new(
            jsonl_store as Arc<dyn SessionStore>,
            Arc::new(meerkat_runtime::InMemoryRuntimeStore::new())
                as Arc<dyn meerkat_runtime::RuntimeStore>,
            Arc::new(MemoryBlobStore::new()),
        ))
    }

    async fn build_test_service(
        temp: &TempDir,
    ) -> (
        Arc<PersistentSessionService<FactoryAgentBuilder>>,
        Arc<MeerkatMachine>,
    ) {
        build_test_service_with_runtime(temp, None).await
    }

    async fn build_test_service_with_llm(
        temp: &TempDir,
        llm_client: Arc<dyn LlmClient>,
    ) -> (
        Arc<PersistentSessionService<FactoryAgentBuilder>>,
        Arc<MeerkatMachine>,
    ) {
        let persistence = build_default_persistence(temp.path().join("sessions"))
            .await
            .expect("build default persistence");
        let factory = crate::AgentFactory::new(temp.path().join("sessions"));
        let mut builder = FactoryAgentBuilder::new(factory, crate::Config::default());
        builder.default_llm_client = Some(llm_client);
        let (service, runtime_adapter) = build_runtime_backed_service(builder, 4, persistence);
        (Arc::new(service), runtime_adapter)
    }

    async fn build_test_service_with_capacity(
        temp: &TempDir,
        active_session_capacity: usize,
    ) -> (
        Arc<PersistentSessionService<FactoryAgentBuilder>>,
        Arc<MeerkatMachine>,
    ) {
        let persistence = build_default_persistence(temp.path().join("sessions"))
            .await
            .expect("build default persistence");
        let factory = crate::AgentFactory::new(temp.path().join("sessions"));
        let mut builder = FactoryAgentBuilder::new(factory, crate::Config::default());
        builder.default_llm_client = Some(Arc::new(TestClient::default()));
        let (service, runtime_adapter) =
            build_runtime_backed_service(builder, active_session_capacity, persistence);
        (Arc::new(service), runtime_adapter)
    }

    async fn build_test_service_with_runtime(
        temp: &TempDir,
        #[cfg(feature = "comms")] shared_runtime: Option<Arc<CommsRuntime>>,
        #[cfg(not(feature = "comms"))] _shared_runtime: Option<()>,
    ) -> (
        Arc<PersistentSessionService<FactoryAgentBuilder>>,
        Arc<MeerkatMachine>,
    ) {
        let persistence = build_default_persistence(temp.path().join("sessions"))
            .await
            .expect("build default persistence");

        #[cfg(feature = "comms")]
        let factory = if let Some(shared_runtime) = shared_runtime {
            crate::AgentFactory::new(temp.path().join("sessions"))
                .with_comms_runtime(shared_runtime)
        } else {
            crate::AgentFactory::new(temp.path().join("sessions"))
        };
        #[cfg(not(feature = "comms"))]
        let factory = crate::AgentFactory::new(temp.path().join("sessions"));

        let mut builder = FactoryAgentBuilder::new(factory, crate::Config::default());
        builder.default_llm_client = Some(Arc::new(TestClient::default()));
        let (service, runtime_adapter) = build_runtime_backed_service(builder, 4, persistence);
        (Arc::new(service), runtime_adapter)
    }

    struct BlockingClient {
        started: Arc<AtomicBool>,
        release: Arc<Notify>,
    }

    #[async_trait::async_trait]
    impl LlmClient for BlockingClient {
        fn project_replay_messages(
            &self,
            messages: &[meerkat_core::Message],
        ) -> Result<Vec<meerkat_core::Message>, meerkat_client::LlmError> {
            Ok(messages.to_vec())
        }

        fn stream<'a>(&'a self, _request: &'a LlmRequest) -> LlmStream<'a> {
            let started = Arc::clone(&self.started);
            let release = Arc::clone(&self.release);
            Box::pin(futures::stream::once(async move {
                started.store(true, Ordering::SeqCst);
                release.notified().await;
                Ok(LlmEvent::Done {
                    outcome: LlmDoneOutcome::Success {
                        stop_reason: meerkat_core::StopReason::EndTurn,
                    },
                })
            }))
        }

        fn provider(&self) -> meerkat_core::Provider {
            meerkat_core::Provider::Other
        }

        async fn health_check(&self) -> Result<(), meerkat_client::LlmError> {
            Ok(())
        }
    }

    struct OneToolThenOkClient {
        calls: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl LlmClient for OneToolThenOkClient {
        fn project_replay_messages(
            &self,
            messages: &[meerkat_core::Message],
        ) -> Result<Vec<meerkat_core::Message>, LlmError> {
            Ok(messages.to_vec())
        }

        fn stream<'a>(&'a self, _request: &'a LlmRequest) -> LlmStream<'a> {
            let call_index = self.calls.fetch_add(1, Ordering::SeqCst);
            let events = if call_index == 0 {
                vec![
                    LlmEvent::ToolCallComplete {
                        id: "toolu_blocking".to_string(),
                        name: "blocking_tool".to_string(),
                        args: serde_json::json!({}),
                        meta: None,
                    },
                    LlmEvent::Done {
                        outcome: LlmDoneOutcome::Success {
                            stop_reason: meerkat_core::StopReason::ToolUse,
                        },
                    },
                ]
            } else {
                vec![
                    LlmEvent::TextDelta {
                        delta: "ok".to_string(),
                        meta: None,
                    },
                    LlmEvent::Done {
                        outcome: LlmDoneOutcome::Success {
                            stop_reason: meerkat_core::StopReason::EndTurn,
                        },
                    },
                ]
            };
            Box::pin(futures::stream::iter(events.into_iter().map(Ok)))
        }

        fn provider(&self) -> meerkat_core::Provider {
            meerkat_core::Provider::Other
        }

        async fn health_check(&self) -> Result<(), LlmError> {
            Ok(())
        }
    }

    struct BlockingToolDispatcher {
        started: Arc<Notify>,
        release: Arc<Notify>,
    }

    #[async_trait::async_trait]
    impl meerkat_core::AgentToolDispatcher for BlockingToolDispatcher {
        fn tools(&self) -> Arc<[Arc<meerkat_core::ToolDef>]> {
            Arc::from([Arc::new(meerkat_core::ToolDef::new(
                "blocking_tool",
                "blocks until the test releases it",
                serde_json::json!({ "type": "object", "properties": {} }),
            ))])
        }

        async fn dispatch(
            &self,
            call: meerkat_core::ToolCallView<'_>,
        ) -> Result<meerkat_core::ToolDispatchOutcome, meerkat_core::ToolError> {
            self.started.notify_waiters();
            self.release.notified().await;
            Ok(meerkat_core::ToolDispatchOutcome::sync_result(
                meerkat_core::ToolResult::new(call.id.to_string(), "released".to_string(), false),
            ))
        }
    }

    struct TerminalLlmFailureClient;

    #[async_trait::async_trait]
    impl LlmClient for TerminalLlmFailureClient {
        fn project_replay_messages(
            &self,
            messages: &[meerkat_core::Message],
        ) -> Result<Vec<meerkat_core::Message>, meerkat_client::LlmError> {
            Ok(messages.to_vec())
        }

        fn stream<'a>(&'a self, _request: &'a LlmRequest) -> LlmStream<'a> {
            Box::pin(futures::stream::once(async {
                Err(LlmError::AuthenticationFailed {
                    message: "provider auth denied".to_string(),
                })
            }))
        }

        fn provider(&self) -> meerkat_core::Provider {
            meerkat_core::Provider::Other
        }

        async fn health_check(&self) -> Result<(), LlmError> {
            Ok(())
        }
    }

    struct CallbackPendingDispatcher;

    #[async_trait::async_trait]
    impl meerkat_core::AgentToolDispatcher for CallbackPendingDispatcher {
        fn tools(&self) -> Arc<[Arc<meerkat_core::ToolDef>]> {
            Arc::from([Arc::new(meerkat_core::ToolDef::new(
                "external_callback",
                "external callback test tool",
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "key": { "type": "string" }
                    }
                }),
            ))])
        }

        async fn dispatch(
            &self,
            call: meerkat_core::ToolCallView<'_>,
        ) -> Result<meerkat_core::ToolDispatchOutcome, meerkat_core::ToolError> {
            let args = serde_json::from_str(call.args.get()).unwrap_or_else(|_| {
                serde_json::json!({
                    "raw": call.args.get()
                })
            });
            Err(meerkat_core::ToolError::callback_pending(call.name, args))
        }
    }

    #[tokio::test]
    async fn build_runtime_backed_service_installs_realm_event_projection() {
        let temp = tempfile::tempdir().expect("tempdir");
        let (_manifest, persistence) = crate::open_realm_persistence_in(
            temp.path(),
            "surface-realm",
            Some(meerkat_store::RealmBackend::Sqlite),
            Some(meerkat_store::RealmOrigin::Explicit),
        )
        .await
        .expect("open realm persistence");

        let factory = crate::AgentFactory::new(temp.path().join("sessions"));
        let mut builder = FactoryAgentBuilder::new(factory, crate::Config::default());
        builder.default_llm_client = Some(Arc::new(TestClient::default()));

        let (service, _runtime_adapter) = build_runtime_backed_service(builder, 4, persistence);

        assert!(
            service.has_event_projection(),
            "runtime-backed service must install the realm event projection bridge"
        );
    }

    async fn expect_prompt_completion(
        adapter: &Arc<MeerkatMachine>,
        session_id: &SessionId,
        prompt: &str,
    ) {
        let (_outcome, handle) = adapter
            .accept_input_with_completion(session_id, Input::Prompt(PromptInput::new(prompt, None)))
            .await
            .expect("accept prompt input");
        let handle = handle.expect("completion handle");
        let outcome = tokio::time::timeout(Duration::from_secs(2), handle.wait())
            .await
            .expect("prompt should complete")
            .expect("completion waiter should resolve");
        assert!(
            matches!(outcome, CompletionOutcome::Completed(ref run) if run.text == "ok"),
            "unexpected completion outcome: {outcome:?}"
        );
    }

    async fn expect_unregister_completion(adapter: &MeerkatMachine, session_id: &SessionId) {
        let expected_runtime_id = meerkat_runtime::LogicalRuntimeId::for_session(session_id);
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                match adapter.unregister_session(session_id).await {
                    Ok(()) => break,
                    Err(RuntimeDriverError::UnregisterInProgress { runtime_id }) => {
                        assert_eq!(
                            &runtime_id, &expected_runtime_id,
                            "unregister coordinator reported the wrong runtime identity"
                        );
                    }
                    Err(error) => panic!(
                        "unregister coordinator failed for runtime {expected_runtime_id}: {error}"
                    ),
                }
            }
        })
        .await
        .expect("unregister coordinator should complete within the test deadline");
        assert!(
            !adapter.contains_session(session_id).await,
            "completed unregister must remove runtime {expected_runtime_id}"
        );
    }

    #[tokio::test]
    async fn materialize_session_unregisters_prepared_runtime_on_create_failure() {
        let temp = tempfile::tempdir().expect("tempdir");
        let (service, adapter) = build_test_service(&temp).await;
        let session = Session::new();
        let session_id = session.id().clone();

        let error = Box::pin(materialize_session(
            &service,
            &adapter,
            session,
            make_request(SessionBuildOptions {
                max_inline_peer_notifications: Some(-2),
                ..Default::default()
            }),
            {
                let service = Arc::clone(&service);
                let adapter = Arc::clone(&adapter);
                move |session_id| default_persistent_executor(service, adapter, session_id)
            },
        ))
        .await
        .expect_err("invalid build settings must fail");

        assert!(
            error
                .to_string()
                .contains("max_inline_peer_notifications=-2 is invalid"),
            "unexpected error: {error}"
        );

        let result = adapter
            .accept_input_with_completion(
                &session_id,
                Input::Prompt(PromptInput::new("must stay unregistered", None)),
            )
            .await;
        match result {
            Err(RuntimeDriverError::NotReady {
                state: RuntimeState::Destroyed,
            }) => {}
            Err(error) => panic!("unexpected runtime error after failed materialization: {error}"),
            Ok(_) => panic!("failed materialization must unregister prepared runtime session"),
        }
    }

    #[tokio::test]
    async fn materialize_session_create_failure_preserves_existing_runtime_registration() {
        let temp = tempfile::tempdir().expect("tempdir");
        let (service, adapter) = build_test_service(&temp).await;
        let session = Session::new();
        let session_id = session.id().clone();
        adapter
            .prepare_bindings(session_id.clone())
            .await
            .expect("prepare existing runtime registration");

        let error = Box::pin(materialize_session(
            &service,
            &adapter,
            session,
            make_request(SessionBuildOptions {
                max_inline_peer_notifications: Some(-2),
                ..Default::default()
            }),
            {
                let service = Arc::clone(&service);
                let adapter = Arc::clone(&adapter);
                move |session_id| default_persistent_executor(service, adapter, session_id)
            },
        ))
        .await
        .expect_err("invalid build settings must fail");

        assert!(
            error
                .to_string()
                .contains("max_inline_peer_notifications=-2 is invalid"),
            "unexpected error: {error}"
        );
        assert!(
            adapter.contains_session(&session_id).await,
            "failed materialization must not unregister pre-existing runtime registration"
        );
        expect_unregister_completion(&adapter, &session_id).await;
    }

    #[tokio::test]
    async fn materialize_session_capacity_full_rejects_before_prepare_bindings() {
        let temp = tempfile::tempdir().expect("tempdir");
        let (service, adapter) = build_test_service_with_capacity(&temp, 1).await;

        let existing = service
            .create_session(make_request(SessionBuildOptions::default()))
            .await
            .expect("fill active admission capacity");
        let candidate = Session::new();
        let candidate_id = candidate.id().clone();

        let error = Box::pin(materialize_session(
            &service,
            &adapter,
            candidate,
            make_request(SessionBuildOptions::default()),
            {
                let service = Arc::clone(&service);
                let adapter = Arc::clone(&adapter);
                move |session_id| default_persistent_executor(service, adapter, session_id)
            },
        ))
        .await
        .expect_err("capacity-full materialization should fail before runtime prepare");

        assert!(
            error.to_string().contains("Max sessions"),
            "unexpected materialization error: {error}"
        );
        assert!(
            !adapter.contains_session(&candidate_id).await,
            "capacity-full materialization must not prepare runtime bindings"
        );

        service
            .discard_live_session(&existing.session_id)
            .await
            .expect("cleanup capacity filler");
        expect_unregister_completion(&adapter, &existing.session_id).await;
    }

    #[tokio::test]
    async fn materialize_session_attaches_executor_and_runs_prompt() {
        let temp = tempfile::tempdir().expect("tempdir");
        let (service, adapter) = build_test_service(&temp).await;

        let result = Box::pin(materialize_session(
            &service,
            &adapter,
            Session::new(),
            make_request(SessionBuildOptions::default()),
            {
                let service = Arc::clone(&service);
                let adapter = Arc::clone(&adapter);
                move |session_id| default_persistent_executor(service, adapter, session_id)
            },
        ))
        .await
        .expect("create session with shared default executor");

        expect_prompt_completion(
            &adapter,
            &result.session_id,
            "shared default executor prompt",
        )
        .await;
        expect_unregister_completion(&adapter, &result.session_id).await;
    }

    #[tokio::test]
    async fn materialize_session_reconstructs_actor_without_replacing_existing_attachment() {
        let temp = tempfile::tempdir().expect("tempdir");
        let (service, adapter) = build_test_service(&temp).await;
        let initial = Box::pin(materialize_session(
            &service,
            &adapter,
            Session::new(),
            make_request(SessionBuildOptions::default()),
            {
                let service = Arc::clone(&service);
                let adapter = Arc::clone(&adapter);
                move |session_id| default_persistent_executor(service, adapter, session_id)
            },
        ))
        .await
        .expect("materialize initial runtime-backed session");
        let session_id = initial.session_id;
        let original_witness = adapter
            .current_executor_attachment_witness(&session_id)
            .await
            .expect("initial materialization should commit an exact attachment");
        let persisted = service
            .load_authoritative_session(&session_id)
            .await
            .expect("load authoritative session")
            .expect("authoritative session should exist");
        let recovered = build_recovered_session(
            persisted.clone(),
            &SurfaceSessionRecoveryOverrides::default(),
            SurfaceSessionRecoveryContext::default(),
        )
        .expect("build recovered actor request");

        let turn_boundary = service
            .acquire_runtime_turn_finalization_guard(&session_id)
            .await;
        service
            .discard_live_session_under_runtime_turn_boundary(&session_id)
            .await
            .expect("discard only the live actor");
        assert_eq!(
            adapter
                .current_executor_attachment_witness(&session_id)
                .await
                .as_ref(),
            Some(&original_witness),
            "actor discard must leave the exact executor attachment serving"
        );
        drop(turn_boundary);

        let executor_factory_calls = Arc::new(AtomicUsize::new(0));
        let service_for_factory = Arc::clone(&service);
        let adapter_for_factory = Arc::clone(&adapter);
        let calls_for_factory = Arc::clone(&executor_factory_calls);
        let result = Box::pin(materialize_session(
            &service,
            &adapter,
            persisted,
            recovered.into_deferred_create_request(),
            move |session_id| {
                calls_for_factory.fetch_add(1, Ordering::SeqCst);
                default_persistent_executor(service_for_factory, adapter_for_factory, session_id)
            },
        ))
        .await
        .expect("reconstruct actor against the existing attachment");

        assert_eq!(result.session_id, session_id);
        assert_eq!(
            executor_factory_calls.load(Ordering::SeqCst),
            0,
            "existing-attachment materialization must not construct a replacement executor"
        );
        assert_eq!(
            adapter
                .current_executor_attachment_witness(&session_id)
                .await
                .as_ref(),
            Some(&original_witness),
            "actor reconstruction must preserve exact attachment identity"
        );
        expect_prompt_completion(&adapter, &session_id, "reconstructed actor prompt").await;
        expect_unregister_completion(&adapter, &session_id).await;
    }

    #[tokio::test]
    async fn materialize_under_runtime_turn_boundary_is_actor_only() {
        let temp = tempfile::tempdir().expect("tempdir");
        let (service, adapter) = build_test_service(&temp).await;
        let initial = Box::pin(materialize_session(
            &service,
            &adapter,
            Session::new(),
            make_request(SessionBuildOptions::default()),
            {
                let service = Arc::clone(&service);
                let adapter = Arc::clone(&adapter);
                move |session_id| default_persistent_executor(service, adapter, session_id)
            },
        ))
        .await
        .expect("materialize initial runtime-backed session");
        let session_id = initial.session_id;
        let original_witness = adapter
            .current_executor_attachment_witness(&session_id)
            .await
            .expect("initial materialization should commit an exact attachment");
        let persisted = service
            .load_authoritative_session(&session_id)
            .await
            .expect("load authoritative session")
            .expect("authoritative session should exist");
        let recovered = build_recovered_session(
            persisted.clone(),
            &SurfaceSessionRecoveryOverrides::default(),
            SurfaceSessionRecoveryContext::default(),
        )
        .expect("build recovered actor request");
        let executor_factory_calls = Arc::new(AtomicUsize::new(0));

        let turn_boundary = service
            .acquire_runtime_turn_finalization_guard(&session_id)
            .await;
        service
            .discard_live_session_under_runtime_turn_boundary(&session_id)
            .await
            .expect("discard only the live actor");
        let calls_for_factory = Arc::clone(&executor_factory_calls);
        let result = Box::pin(materialize_session_under_runtime_turn_boundary(
            &service,
            &adapter,
            persisted,
            recovered.into_deferred_create_request(),
            move |_session_id| {
                calls_for_factory.fetch_add(1, Ordering::SeqCst);
                panic!("actor-only materialization must not invoke the executor factory")
            },
        ))
        .await
        .expect("reconstruct actor inside the existing executor boundary");

        assert_eq!(result.session_id, session_id);
        assert_eq!(
            executor_factory_calls.load(Ordering::SeqCst),
            0,
            "under-boundary recovery must remain actor-only"
        );
        assert_eq!(
            adapter
                .current_executor_attachment_witness(&session_id)
                .await
                .as_ref(),
            Some(&original_witness),
            "under-boundary actor recovery must preserve exact attachment identity"
        );
        drop(turn_boundary);

        expect_prompt_completion(&adapter, &session_id, "under-boundary actor recovery").await;
        expect_unregister_completion(&adapter, &session_id).await;
    }

    #[tokio::test]
    async fn materialize_session_stamps_eager_initial_turn_execution_kind() {
        let temp = tempfile::tempdir().expect("tempdir");
        let (service, adapter) = build_test_service(&temp).await;

        let mut request = make_request(SessionBuildOptions::default());
        request.initial_turn = meerkat_core::service::InitialTurnPolicy::RunImmediately;
        let result = Box::pin(materialize_session(
            &service,
            &adapter,
            Session::new(),
            request,
            {
                let service = Arc::clone(&service);
                let adapter = Arc::clone(&adapter);
                move |session_id| default_persistent_executor(service, adapter, session_id)
            },
        ))
        .await
        .expect("runtime-backed eager create should receive stamped metadata");

        assert_eq!(result.text, "ok");
        expect_unregister_completion(&adapter, &result.session_id).await;
    }

    #[tokio::test]
    async fn materialize_session_atomically_commits_failed_initial_turn() {
        let temp = tempfile::tempdir().expect("tempdir");
        let (service, adapter) =
            build_test_service_with_llm(&temp, Arc::new(TerminalLlmFailureClient)).await;
        let session = Session::new();
        let session_id = session.id().clone();
        let mut request = make_request(SessionBuildOptions::default());
        request.initial_turn = meerkat_core::service::InitialTurnPolicy::RunImmediately;
        request.prompt = "persist this failed initial turn".to_string().into();

        let error = Box::pin(materialize_session(&service, &adapter, session, request, {
            let service = Arc::clone(&service);
            let adapter = Arc::clone(&adapter);
            move |session_id| default_persistent_executor(service, adapter, session_id)
        }))
        .await
        .expect_err("fatal initial turn should preserve its typed public failure");

        assert!(
            matches!(
                &error,
                SurfaceRuntimeMaterializeError::Session(SessionError::Agent(
                    meerkat_core::AgentError::TerminalFailure {
                        cause_kind: meerkat_core::TurnTerminalCauseKind::LlmFailure,
                        ..
                    }
                ))
            ),
            "unexpected failed initial-turn surface error: {error}"
        );
        assert_eq!(
            adapter.runtime_state(&session_id).await.unwrap(),
            RuntimeState::Attached,
            "failed service turn must close the machine-owned run"
        );

        let runtime_snapshot = service
            .runtime_store()
            .load_session_snapshot(&meerkat_runtime::LogicalRuntimeId::for_session(&session_id))
            .await
            .expect("runtime snapshot read should succeed")
            .expect("failed turn transcript must commit with lifecycle authority");
        let committed: Session = serde_json::from_slice(&runtime_snapshot)
            .expect("failed runtime snapshot should deserialize");
        assert!(committed.messages().iter().any(|message| {
            format!("{message:?}").contains("persist this failed initial turn")
        }));

        expect_unregister_completion(&adapter, &session_id).await;
        let recovered = service
            .load_authoritative_session(&session_id)
            .await
            .expect("authoritative recovery read should succeed")
            .expect("failed transcript must survive live-session teardown");
        assert_eq!(recovered.messages(), committed.messages());
    }

    #[tokio::test]
    async fn materialize_session_commits_callback_pending_initial_turn_before_returning_error() {
        let temp = tempfile::tempdir().expect("tempdir");
        let llm = TestClient::new(vec![
            LlmEvent::ToolCallComplete {
                id: "toolu_callback".to_string(),
                name: "external_callback".to_string(),
                args: serde_json::json!({ "key": "alpha" }),
                meta: None,
            },
            LlmEvent::Done {
                outcome: LlmDoneOutcome::Success {
                    stop_reason: meerkat_core::StopReason::ToolUse,
                },
            },
        ]);
        let (service, adapter) = build_test_service_with_llm(&temp, Arc::new(llm)).await;
        let session = Session::new();
        let session_id = session.id().clone();
        let mut build = SessionBuildOptions {
            external_tools: Some(Arc::new(CallbackPendingDispatcher)),
            ..Default::default()
        };
        build.override_builtins = meerkat_core::ToolCategoryOverride::Disable;
        let mut request = make_request(build);
        request.initial_turn = meerkat_core::service::InitialTurnPolicy::RunImmediately;
        request.prompt = "call the external callback".to_string().into();

        let error = Box::pin(materialize_session(&service, &adapter, session, request, {
            let service = Arc::clone(&service);
            let adapter = Arc::clone(&adapter);
            move |session_id| default_persistent_executor(service, adapter, session_id)
        }))
        .await
        .expect_err("callback-pending initial turn should surface as a resumable error");

        assert!(
            matches!(
                error,
                SurfaceRuntimeMaterializeError::Session(SessionError::Agent(
                    meerkat_core::AgentError::CallbackPending { .. }
                ))
            ),
            "unexpected materialize error: {error}"
        );
        assert_eq!(
            adapter.runtime_state(&session_id).await.unwrap(),
            RuntimeState::Attached,
            "callback-pending service turn must close the machine-owned run"
        );
        let authoritative = service
            .load_authoritative_session(&session_id)
            .await
            .expect("authoritative load should succeed")
            .expect("callback-pending service turn should leave durable session truth");
        assert!(
            authoritative.messages().len() >= 2,
            "callback-pending service turn should persist the user/tool-call boundary"
        );

        expect_unregister_completion(&adapter, &session_id).await;
    }

    #[tokio::test]
    async fn materialize_session_stamps_existing_eager_initial_turn_metadata() {
        let temp = tempfile::tempdir().expect("tempdir");
        let (service, adapter) = build_test_service(&temp).await;

        let mut request = make_request(SessionBuildOptions {
            initial_turn_metadata: Some(RuntimeTurnMetadata {
                handling_mode: Some(HandlingMode::Queue),
                ..Default::default()
            }),
            ..Default::default()
        });
        request.initial_turn = meerkat_core::service::InitialTurnPolicy::RunImmediately;
        let result = Box::pin(materialize_session(
            &service,
            &adapter,
            Session::new(),
            request,
            {
                let service = Arc::clone(&service);
                let adapter = Arc::clone(&adapter);
                move |session_id| default_persistent_executor(service, adapter, session_id)
            },
        ))
        .await
        .expect("runtime-backed eager create should stamp supplied metadata");

        assert_eq!(result.text, "ok");
        expect_unregister_completion(&adapter, &result.session_id).await;
    }

    #[tokio::test]
    async fn materialize_session_hard_cancel_reaches_eager_first_turn_before_executor_attach() {
        let temp = tempfile::tempdir().expect("tempdir");
        let persistence = build_default_persistence(temp.path().join("sessions"))
            .await
            .expect("build default persistence");
        let started = Arc::new(AtomicBool::new(false));
        let release = Arc::new(Notify::new());
        let factory = crate::AgentFactory::new(temp.path().join("sessions"));
        let mut builder = FactoryAgentBuilder::new(factory, crate::Config::default());
        builder.default_llm_client = Some(Arc::new(BlockingClient {
            started: Arc::clone(&started),
            release: Arc::clone(&release),
        }));
        let (service, adapter) = build_runtime_backed_service(builder, 4, persistence);
        let service = Arc::new(service);
        let session = Session::new();
        let session_id = session.id().clone();
        let mut request = make_request(SessionBuildOptions::default());
        request.initial_turn = meerkat_core::service::InitialTurnPolicy::RunImmediately;
        request.prompt = "blocking initial turn".to_string().into();

        let materialize_task = {
            let service = Arc::clone(&service);
            let adapter = Arc::clone(&adapter);
            let session_id_for_executor = session_id.clone();
            tokio::spawn(async move {
                Box::pin(materialize_session(&service, &adapter, session, request, {
                    let service = Arc::clone(&service);
                    let adapter = Arc::clone(&adapter);
                    move |_session_id| {
                        default_persistent_executor(
                            Arc::clone(&service),
                            Arc::clone(&adapter),
                            session_id_for_executor,
                        )
                    }
                }))
                .await
            })
        };

        tokio::time::timeout(Duration::from_secs(10), async {
            while !started.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("initial service-owned turn should start");

        adapter
            .hard_cancel_current_run(&session_id, "test eager materialization interrupt")
            .await
            .expect("hard cancel must reach the service-owned first turn before executor attach");

        let error = tokio::time::timeout(Duration::from_secs(10), materialize_task)
            .await
            .expect("materialization should finish after interrupt")
            .expect("materialization task should not panic")
            .expect_err("interrupted eager first turn should fail materialization");
        assert!(
            error.to_string().to_lowercase().contains("cancel"),
            "unexpected materialization error: {error}"
        );

        release.notify_waiters();
        expect_unregister_completion(&adapter, &session_id).await;
    }

    #[tokio::test]
    async fn direct_runtime_owned_eager_create_is_removed() {
        let temp = tempfile::tempdir().expect("tempdir");
        let (service, adapter) = build_test_service(&temp).await;
        let session = Session::new();
        let session_id = session.id().clone();
        let bindings = adapter
            .prepare_bindings(session_id.clone())
            .await
            .expect("prepare runtime bindings");

        let mut request = make_request(SessionBuildOptions {
            resume_session: Some(session),
            runtime_build_mode: meerkat_core::RuntimeBuildMode::SessionOwned(bindings),
            ..Default::default()
        });
        request.initial_turn = meerkat_core::service::InitialTurnPolicy::RunImmediately;
        request.prompt = "needs runtime stamp".to_string().into();

        let error = service
            .create_session(request)
            .await
            .expect_err("runtime-backed eager create must be removed");

        assert!(
            error.to_string().contains(
                "runtime-backed eager create_session must route through the MeerkatMachine service-turn commit protocol"
            ),
            "unexpected error: {error}"
        );
        expect_unregister_completion(&adapter, &session_id).await;
    }

    #[test]
    fn ensure_materialized_session_id_matches_reports_mismatch() {
        let expected = SessionId::new();
        let actual = SessionId::new();

        let error = ensure_materialized_session_id_matches(&expected, &actual)
            .expect_err("mismatched session ids must error");

        assert!(matches!(
            error,
            SurfaceRuntimeMaterializeError::SessionIdMismatch {
                expected: ref actual_expected,
                actual: ref actual_actual,
            } if *actual_expected == expected && *actual_actual == actual
        ));
    }

    #[tokio::test]
    async fn persistent_runtime_executor_stop_request_does_not_unregister_before_terminalization() {
        let temp = tempfile::tempdir().expect("tempdir");
        let (service, adapter) = build_test_service(&temp).await;
        let result = Box::pin(materialize_session(
            &service,
            &adapter,
            Session::new(),
            make_request(SessionBuildOptions::default()),
            {
                let service = Arc::clone(&service);
                let adapter = Arc::clone(&adapter);
                move |session_id| default_persistent_executor(service, adapter, session_id)
            },
        ))
        .await
        .expect("materialize session");

        let mut executor = PersistentRuntimeExecutor::new(
            Arc::clone(&service),
            Arc::clone(&adapter),
            result.session_id.clone(),
        );
        executor
            .stop_runtime_executor("test stop".to_string())
            .await
            .expect("stop runtime executor");

        assert!(
            adapter.contains_session(&result.session_id).await,
            "executor stop request must not unregister before machine-owned terminalization commits"
        );
    }

    #[tokio::test]
    async fn persistent_runtime_executor_interrupt_noops_without_active_run() {
        let temp = tempfile::tempdir().expect("tempdir");
        let (service, adapter) = build_test_service(&temp).await;
        let result = Box::pin(materialize_session(
            &service,
            &adapter,
            Session::new(),
            make_request(SessionBuildOptions::default()),
            {
                let service = Arc::clone(&service);
                let adapter = Arc::clone(&adapter);
                move |session_id| default_persistent_executor(service, adapter, session_id)
            },
        ))
        .await
        .expect("materialize session");

        let executor = PersistentRuntimeExecutor::new(
            Arc::clone(&service),
            Arc::clone(&adapter),
            result.session_id.clone(),
        );
        executor
            .interrupt_handle()
            .expect("interrupt handle")
            .hard_cancel_current_run("test cancel".to_string())
            .await
            .expect("interrupt without an active run is a no-op");

        expect_unregister_completion(&adapter, &result.session_id).await;
    }

    #[tokio::test]
    async fn persistent_runtime_executor_cancel_after_boundary_surfaces_no_active_run() {
        let temp = tempfile::tempdir().expect("tempdir");
        let (service, adapter) = build_test_service(&temp).await;
        let result = Box::pin(materialize_session(
            &service,
            &adapter,
            Session::new(),
            make_request(SessionBuildOptions::default()),
            {
                let service = Arc::clone(&service);
                let adapter = Arc::clone(&adapter);
                move |session_id| default_persistent_executor(service, adapter, session_id)
            },
        ))
        .await
        .expect("materialize session");

        let mut executor = PersistentRuntimeExecutor::new(
            Arc::clone(&service),
            Arc::clone(&adapter),
            result.session_id.clone(),
        );
        executor
            .cancel_after_boundary("test boundary cancel".to_string())
            .await
            .expect_err(
                "boundary cancel without an active run must surface the session running-state error",
            );

        expect_unregister_completion(&adapter, &result.session_id).await;
    }

    #[tokio::test]
    async fn persistent_runtime_executor_preserves_typed_terminal_failure_cause() {
        use futures::StreamExt;
        use meerkat_core::event::AgentEvent;
        use meerkat_core::lifecycle::InputId;
        use meerkat_core::lifecycle::RunId;
        use meerkat_core::lifecycle::core_executor::CoreApplyTerminal;
        use meerkat_core::lifecycle::run_primitive::{
            ConversationAppend, ConversationAppendRole, RunApplyBoundary, RuntimeExecutionKind,
            RuntimeTurnMetadata, StagedRunInput,
        };

        let temp = tempfile::tempdir().expect("tempdir");
        let (service, adapter) =
            build_test_service_with_llm(&temp, Arc::new(TerminalLlmFailureClient)).await;
        let result = Box::pin(materialize_session(
            &service,
            &adapter,
            Session::new(),
            make_request(SessionBuildOptions::default()),
            {
                let service = Arc::clone(&service);
                let adapter = Arc::clone(&adapter);
                move |session_id| default_persistent_executor(service, adapter, session_id)
            },
        ))
        .await
        .expect("materialize session");
        let mut events = service
            .subscribe_session_events(&result.session_id)
            .await
            .expect("subscribe_session_events");

        let mut executor = PersistentRuntimeExecutor::new(
            Arc::clone(&service),
            Arc::clone(&adapter),
            result.session_id.clone(),
        );
        let output = executor
            .apply(
                RunId::new(),
                RunPrimitive::StagedInput(StagedRunInput {
                    boundary: RunApplyBoundary::RunStart,
                    appends: vec![ConversationAppend {
                        role: ConversationAppendRole::User,
                        content: CoreRenderable::Text {
                            text: "trigger terminal LLM failure".to_string(),
                        },
                    }],
                    context_appends: Vec::new(),
                    contributing_input_ids: vec![InputId::new()],
                    turn_metadata: Some(RuntimeTurnMetadata {
                        execution_kind: Some(RuntimeExecutionKind::ContentTurn),
                        ..Default::default()
                    }),
                }),
            )
            .await
            .expect("terminal LLM failure should return the failed-but-applied carrier");

        assert_eq!(output.receipt.boundary, RunApplyBoundary::RunStart);
        assert!(
            output.session_snapshot.is_some(),
            "failed-but-applied carrier must preserve the mutated session snapshot"
        );
        match output.terminal {
            Some(CoreApplyTerminal::MachineTerminalFailure { error }) => {
                assert!(error.terminal);
                assert_eq!(
                    error.outcome,
                    Some(meerkat_core::TurnTerminalOutcome::Failed)
                );
                assert_eq!(error.kind, meerkat_core::TurnTerminalCauseKind::LlmFailure);
                assert!(
                    error
                        .detail
                        .as_deref()
                        .is_some_and(|detail| detail.contains("provider auth denied")),
                    "machine-terminal carrier must preserve the concrete LLM failure detail"
                );
            }
            other => panic!("expected typed machine-terminal carrier, got {other:?}"),
        }

        let failed = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                let event = events.next().await.expect("run_failed event should exist");
                if matches!(event.payload, AgentEvent::RunFailed { .. }) {
                    break event.payload;
                }
            }
        })
        .await
        .expect("run_failed timeout");

        match failed {
            AgentEvent::RunFailed {
                error_report: report,
                ..
            } => {
                assert_eq!(report.class, meerkat_core::event::AgentErrorClass::Llm);
                assert_eq!(
                    report.reason,
                    Some(meerkat_core::event::AgentErrorReason::TurnTerminalCause {
                        outcome: meerkat_core::TurnTerminalOutcome::Failed,
                        cause_kind: meerkat_core::TurnTerminalCauseKind::LlmFailure,
                    })
                );
            }
            other => panic!("expected run_failed with typed report, got {other:?}"),
        }

        expect_unregister_completion(&adapter, &result.session_id).await;
    }

    #[cfg(feature = "comms")]
    #[tokio::test]
    async fn configure_peer_ingress_uses_service_comms_runtime() {
        let temp = tempfile::tempdir().expect("tempdir");
        let shared_runtime = Arc::new(
            CommsRuntime::inproc_only("surface-peer-ingress").expect("create inproc comms runtime"),
        );
        let (service, adapter) = build_test_service_with_runtime(&temp, Some(shared_runtime)).await;
        let result = Box::pin(materialize_session(
            &service,
            &adapter,
            Session::new(),
            make_request(SessionBuildOptions {
                comms_name: Some("surface-peer-ingress-session".to_string()),
                keep_alive: true,
                ..Default::default()
            }),
            {
                let service = Arc::clone(&service);
                let adapter = Arc::clone(&adapter);
                move |session_id| default_persistent_executor(service, adapter, session_id)
            },
        ))
        .await
        .expect("materialize session");

        configure_peer_ingress(&adapter, &service, &result.session_id, true)
            .await
            .expect("configure peer ingress");
        expect_prompt_completion(&adapter, &result.session_id, "peer ingress prompt").await;
        expect_unregister_completion(&adapter, &result.session_id).await;
    }

    #[cfg(feature = "comms")]
    #[tokio::test]
    async fn configure_peer_ingress_preserves_keep_alive_flag() {
        let temp = tempfile::tempdir().expect("tempdir");
        let shared_runtime = Arc::new(
            CommsRuntime::inproc_only("surface-keep-alive").expect("create inproc comms runtime"),
        );
        let (service, adapter) = build_test_service_with_runtime(&temp, Some(shared_runtime)).await;
        let result = Box::pin(materialize_session(
            &service,
            &adapter,
            Session::new(),
            make_request(SessionBuildOptions {
                comms_name: Some("surface-keep-alive-session".to_string()),
                keep_alive: true,
                ..Default::default()
            }),
            {
                let service = Arc::clone(&service);
                let adapter = Arc::clone(&adapter);
                move |session_id| default_persistent_executor(service, adapter, session_id)
            },
        ))
        .await
        .expect("materialize session");

        configure_peer_ingress(&adapter, &service, &result.session_id, false)
            .await
            .expect("configure peer ingress");
        let persisted = service
            .export_live_session(&result.session_id)
            .await
            .expect("export live session");
        assert!(
            persisted
                .session_metadata()
                .is_some_and(|metadata| metadata.keep_alive),
            "explicit peer ingress configuration must not mutate persisted keep_alive metadata"
        );

        expect_unregister_completion(&adapter, &result.session_id).await;
    }

    #[tokio::test]
    async fn persistent_runtime_executor_routes_context_only_staged_input_through_runtime_context_appends()
     {
        use futures::StreamExt;
        use meerkat_core::event::AgentEvent;
        use meerkat_core::lifecycle::InputId;
        use meerkat_core::lifecycle::RunId;
        use meerkat_core::lifecycle::run_primitive::{
            ConversationContextAppend, RunApplyBoundary, RuntimeExecutionKind, RuntimeTurnMetadata,
            StagedRunInput,
        };

        let temp = tempfile::tempdir().expect("tempdir");
        let (service, adapter) = build_test_service(&temp).await;
        let result = Box::pin(materialize_session(
            &service,
            &adapter,
            Session::new(),
            make_request(SessionBuildOptions::default()),
            {
                let service = Arc::clone(&service);
                let adapter = Arc::clone(&adapter);
                move |session_id| default_persistent_executor(service, adapter, session_id)
            },
        ))
        .await
        .expect("materialize session");

        let mut events = service
            .subscribe_session_events(&result.session_id)
            .await
            .expect("subscribe_session_events");

        let mut executor = PersistentRuntimeExecutor::new(
            Arc::clone(&service),
            Arc::clone(&adapter),
            result.session_id.clone(),
        );
        let output = executor
            .apply(
                RunId::new(),
                RunPrimitive::StagedInput(StagedRunInput {
                    boundary: meerkat_core::lifecycle::run_primitive::RunApplyBoundary::RunStart,
                    appends: Vec::new(),
                    context_appends: vec![ConversationContextAppend {
                        key: "peer_response_terminal:analyst-rt:req-123".to_string(),
                        content: CoreRenderable::Text {
                            text: "Peer terminal response from analyst-rt\nRequest ID: req-123\nStatus: completed\nPayload: {\"request_intent\":\"checksum_token\",\"request_subject\":\"alpha beta gamma\",\"token\":\"birch seventeen\"}".to_string(),
                        },
                    }],
                    contributing_input_ids: vec![InputId::new()],
                    turn_metadata: Some(RuntimeTurnMetadata {
                        execution_kind: Some(RuntimeExecutionKind::ContentTurn),
                        ..Default::default()
                    }),
                }),
            )
            .await
            .expect("context-only staged input should apply");

        assert_eq!(
            output.receipt.boundary,
            meerkat_core::lifecycle::run_primitive::RunApplyBoundary::RunStart
        );

        assert!(
            tokio::time::timeout(Duration::from_millis(50), events.next())
                .await
                .is_err(),
            "context lifecycle events must remain hidden until the exact runtime snapshot commits"
        );
        let session_snapshot = output
            .session_snapshot
            .as_deref()
            .expect("context-only output should carry a session snapshot");
        service
            .runtime_store()
            .commit_session_snapshot(
                &meerkat_runtime::LogicalRuntimeId::for_session(&result.session_id),
                meerkat_runtime::SessionDelta {
                    session_snapshot: session_snapshot.to_vec(),
                },
            )
            .await
            .expect("commit context-only runtime snapshot");
        executor
            .checkpoint_committed_session_snapshot(session_snapshot)
            .await
            .expect("checkpoint committed context-only snapshot");

        let started = tokio::time::timeout(std::time::Duration::from_secs(2), events.next())
            .await
            .expect("run_started timeout")
            .expect("run_started event should exist");
        assert!(matches!(started.payload, AgentEvent::RunStarted { .. }));

        let completed = tokio::time::timeout(std::time::Duration::from_secs(2), events.next())
            .await
            .expect("run_completed timeout")
            .expect("run_completed event should exist");
        match completed.payload {
            AgentEvent::RunCompleted { result, usage, .. } => {
                assert!(
                    result.is_empty(),
                    "context-only runtime apply should not synthesize assistant output"
                );
                assert_eq!(usage, meerkat_core::types::Usage::default());
            }
            other => panic!("expected run_completed, got {other:?}"),
        }

        let second_output = executor
            .apply(
                RunId::new(),
                RunPrimitive::StagedInput(StagedRunInput {
                    boundary: RunApplyBoundary::RunStart,
                    appends: Vec::new(),
                    context_appends: vec![ConversationContextAppend {
                        key: "peer_response_terminal:analyst-rt:req-124".to_string(),
                        content: CoreRenderable::Text {
                            text: "Peer terminal response from analyst-rt\nRequest ID: req-124\nStatus: completed\ndone"
                                .to_string(),
                        },
                    }],
                    contributing_input_ids: vec![InputId::new()],
                    turn_metadata: Some(RuntimeTurnMetadata {
                        execution_kind: Some(RuntimeExecutionKind::ContentTurn),
                        ..Default::default()
                    }),
                }),
            )
            .await
            .expect("checkpoint must clear the prior pending context-event commit");
        let second_snapshot = second_output
            .session_snapshot
            .as_deref()
            .expect("second context-only output should carry a session snapshot");
        service
            .runtime_store()
            .commit_session_snapshot(
                &meerkat_runtime::LogicalRuntimeId::for_session(&result.session_id),
                meerkat_runtime::SessionDelta {
                    session_snapshot: second_snapshot.to_vec(),
                },
            )
            .await
            .expect("commit second context-only runtime snapshot");
        executor
            .checkpoint_committed_session_snapshot(second_snapshot)
            .await
            .expect("checkpoint second committed context-only snapshot");
        for expected in ["run_started", "run_completed"] {
            tokio::time::timeout(Duration::from_secs(2), events.next())
                .await
                .unwrap_or_else(|_| panic!("second {expected} timeout"))
                .unwrap_or_else(|| panic!("second {expected} event should exist"));
        }

        expect_unregister_completion(&adapter, &result.session_id).await;
    }

    #[tokio::test]
    async fn persistent_runtime_executor_recovers_missing_actor_for_context_only_apply_under_exact_attachment()
     {
        use meerkat_core::lifecycle::InputId;
        use meerkat_core::lifecycle::RunId;
        use meerkat_core::lifecycle::run_primitive::{
            ConversationContextAppend, RuntimeExecutionKind, RuntimeTurnMetadata, StagedRunInput,
        };

        let temp = tempfile::tempdir().expect("tempdir");
        let (service, adapter) = build_test_service(&temp).await;
        let result = Box::pin(materialize_session(
            &service,
            &adapter,
            Session::new(),
            make_request(SessionBuildOptions::default()),
            {
                let service = Arc::clone(&service);
                let adapter = Arc::clone(&adapter);
                move |session_id| default_persistent_executor(service, adapter, session_id)
            },
        ))
        .await
        .expect("materialize session");

        let attachment_witness = adapter
            .current_executor_attachment_witness(&result.session_id)
            .await
            .expect("materialized session should retain an exact executor attachment");
        let actor_lease = service
            .acquire_live_session_actor_turn_boundary_lease(&result.session_id)
            .await
            .expect("capture exact live actor before actor-only recovery");
        assert!(
            service
                .discard_live_session_actor(&actor_lease)
                .await
                .expect("discard exact live actor before actor-only recovery"),
            "exact actor discard should remove the captured actor"
        );
        drop(actor_lease);
        assert_eq!(
            adapter
                .current_executor_attachment_witness(&result.session_id)
                .await
                .as_ref(),
            Some(&attachment_witness),
            "actor loss must not replace or remove the serving executor attachment"
        );

        let mut executor = PersistentRuntimeExecutor::new(
            Arc::clone(&service),
            Arc::clone(&adapter),
            result.session_id.clone(),
        );
        let turn_boundary = executor
            .turn_finalization_boundary_handle()
            .expect("persistent executor should expose its stable turn boundary")
            .acquire()
            .await
            .expect("acquire the runtime-owned turn boundary before apply");
        let output = executor
            .apply(
                RunId::new(),
                RunPrimitive::StagedInput(StagedRunInput {
                    boundary:
                        meerkat_core::lifecycle::run_primitive::RunApplyBoundary::RunCheckpoint,
                    appends: Vec::new(),
                    context_appends: vec![ConversationContextAppend {
                        key: "runtime-backed-context-recovery".to_string(),
                        content: CoreRenderable::Text {
                            text: "runtime-backed recovered context".to_string(),
                        },
                    }],
                    contributing_input_ids: vec![InputId::new()],
                    turn_metadata: Some(RuntimeTurnMetadata {
                        execution_kind: Some(RuntimeExecutionKind::ContentTurn),
                        ..Default::default()
                    }),
                }),
            )
            .await
            .expect("context-only apply should recover the missing actor");
        drop(turn_boundary);

        assert_eq!(
            output.receipt.boundary,
            meerkat_core::lifecycle::run_primitive::RunApplyBoundary::RunCheckpoint
        );
        assert!(adapter.contains_session(&result.session_id).await);

        let staged_snapshot = runtime_output_session_snapshot(&output);
        assert!(
            has_applied_runtime_context(
                &staged_snapshot,
                "runtime-backed-context-recovery",
                "runtime-backed recovered context"
            ),
            "context-only recovery should persist runtime context append: {:?}",
            staged_snapshot.system_context_state()
        );

        expect_unregister_completion(&adapter, &result.session_id).await;
    }

    #[tokio::test]
    async fn persistent_runtime_executor_archived_context_only_apply_requests_post_handoff_teardown()
     {
        use meerkat_core::lifecycle::InputId;
        use meerkat_core::lifecycle::RunId;
        use meerkat_core::lifecycle::run_primitive::{
            ConversationContextAppend, RuntimeExecutionKind, RuntimeTurnMetadata, StagedRunInput,
        };

        let temp = tempfile::tempdir().expect("tempdir");
        let (service, adapter) = build_test_service(&temp).await;
        let result = Box::pin(materialize_session(
            &service,
            &adapter,
            Session::new(),
            make_request(SessionBuildOptions::default()),
            {
                let service = Arc::clone(&service);
                let adapter = Arc::clone(&adapter);
                move |session_id| default_persistent_executor(service, adapter, session_id)
            },
        ))
        .await
        .expect("materialize session");

        service
            .archive_with_machine_protocol(
                &result.session_id,
                MachineSessionArchiveProtocol::from_machine(adapter.as_ref()),
            )
            .await
            .expect("archive session through machine authority");
        assert!(
            adapter.contains_session(&result.session_id).await,
            "machine archive leaves a retired runtime registration for this regression"
        );

        let mut executor = PersistentRuntimeExecutor::new(
            Arc::clone(&service),
            Arc::clone(&adapter),
            result.session_id.clone(),
        );
        let rejected = executor
            .apply(
                RunId::new(),
                RunPrimitive::StagedInput(StagedRunInput {
                    boundary:
                        meerkat_core::lifecycle::run_primitive::RunApplyBoundary::RunCheckpoint,
                    appends: Vec::new(),
                    context_appends: vec![ConversationContextAppend {
                        key: "runtime-backed-archived-context".to_string(),
                        content: CoreRenderable::Text {
                            text: "archived runtime-backed context".to_string(),
                        },
                    }],
                    contributing_input_ids: vec![InputId::new()],
                    turn_metadata: Some(RuntimeTurnMetadata {
                        execution_kind: Some(RuntimeExecutionKind::ContentTurn),
                        ..Default::default()
                    }),
                }),
            )
            .await;

        assert!(matches!(
            rejected,
            Err(CoreExecutorError::TeardownRequired {
                reason: meerkat_core::lifecycle::core_executor::CoreExecutorTeardownReason::ArchivedSession,
                ..
            })
        ));
        assert!(
            adapter.contains_session(&result.session_id).await,
            "CoreExecutor::apply must retain runtime authority until the loop hands the exact executor to the canonical unregister saga"
        );
    }

    #[tokio::test]
    async fn persistent_runtime_executor_archived_normal_turn_requests_post_handoff_teardown() {
        use meerkat_core::lifecycle::run_primitive::{
            ConversationAppend, ConversationAppendRole, RunApplyBoundary, RuntimeExecutionKind,
            RuntimeTurnMetadata, StagedRunInput,
        };
        use meerkat_core::lifecycle::{InputId, RunId};

        let temp = tempfile::tempdir().expect("tempdir");
        let (service, adapter) = build_test_service(&temp).await;
        let result = Box::pin(materialize_session(
            &service,
            &adapter,
            Session::new(),
            make_request(SessionBuildOptions::default()),
            {
                let service = Arc::clone(&service);
                let adapter = Arc::clone(&adapter);
                move |session_id| default_persistent_executor(service, adapter, session_id)
            },
        ))
        .await
        .expect("materialize session");
        service
            .archive_with_machine_protocol(
                &result.session_id,
                MachineSessionArchiveProtocol::from_machine(adapter.as_ref()),
            )
            .await
            .expect("archive session through machine authority");

        let mut executor = PersistentRuntimeExecutor::new(
            Arc::clone(&service),
            Arc::clone(&adapter),
            result.session_id.clone(),
        );
        let rejected = executor
            .apply(
                RunId::new(),
                RunPrimitive::StagedInput(StagedRunInput {
                    boundary: RunApplyBoundary::RunStart,
                    appends: vec![ConversationAppend {
                        role: ConversationAppendRole::User,
                        content: CoreRenderable::Text {
                            text: "normal turn after archive".to_string(),
                        },
                    }],
                    context_appends: Vec::new(),
                    contributing_input_ids: vec![InputId::new()],
                    turn_metadata: Some(RuntimeTurnMetadata {
                        execution_kind: Some(RuntimeExecutionKind::ContentTurn),
                        ..Default::default()
                    }),
                }),
            )
            .await;

        assert!(matches!(
            rejected,
            Err(CoreExecutorError::TeardownRequired {
                reason: meerkat_core::lifecycle::core_executor::CoreExecutorTeardownReason::ArchivedSession,
                ..
            })
        ));
        assert!(
            adapter.contains_session(&result.session_id).await,
            "archived normal-turn apply must retain runtime authority until exact executor handoff"
        );
    }

    #[tokio::test]
    async fn persistent_runtime_executor_missing_normal_turn_requests_unavailable_teardown() {
        use meerkat_core::lifecycle::run_primitive::{
            ConversationAppend, ConversationAppendRole, RunApplyBoundary, RuntimeExecutionKind,
            RuntimeTurnMetadata, StagedRunInput,
        };
        use meerkat_core::lifecycle::{InputId, RunId};

        let temp = tempfile::tempdir().expect("tempdir");
        let (service, adapter) = build_test_service(&temp).await;
        let session_id = SessionId::new();
        adapter
            .register_session(session_id.clone())
            .await
            .expect("register prepared-only runtime authority");
        let mut executor = PersistentRuntimeExecutor::new(
            Arc::clone(&service),
            Arc::clone(&adapter),
            session_id.clone(),
        );
        let rejected = executor
            .apply(
                RunId::new(),
                RunPrimitive::StagedInput(StagedRunInput {
                    boundary: RunApplyBoundary::RunStart,
                    appends: vec![ConversationAppend {
                        role: ConversationAppendRole::User,
                        content: CoreRenderable::Text {
                            text: "normal turn for missing session".to_string(),
                        },
                    }],
                    context_appends: Vec::new(),
                    contributing_input_ids: vec![InputId::new()],
                    turn_metadata: Some(RuntimeTurnMetadata {
                        execution_kind: Some(RuntimeExecutionKind::ContentTurn),
                        ..Default::default()
                    }),
                }),
            )
            .await;

        assert!(matches!(
            rejected,
            Err(CoreExecutorError::TeardownRequired {
                reason: meerkat_core::lifecycle::core_executor::CoreExecutorTeardownReason::SessionUnavailable,
                ..
            })
        ));
        assert!(
            adapter.contains_session(&session_id).await,
            "missing normal-turn apply must retain prepared registration until exact executor handoff"
        );
    }

    #[tokio::test]
    async fn persistent_runtime_executor_runs_terminal_peer_response_context_and_run() {
        use meerkat_core::lifecycle::InputId;
        use meerkat_core::lifecycle::RunId;
        use meerkat_core::lifecycle::core_executor::CoreApplyTerminal;
        use meerkat_core::lifecycle::run_primitive::{
            ConversationContextAppend, PeerResponseTerminalApplyIntent, RuntimeExecutionKind,
            RuntimeTurnMetadata, StagedRunInput,
        };

        let temp = tempfile::tempdir().expect("tempdir");
        let (service, adapter) = build_test_service(&temp).await;
        let result = Box::pin(materialize_session(
            &service,
            &adapter,
            Session::new(),
            make_request(SessionBuildOptions::default()),
            {
                let service = Arc::clone(&service);
                let adapter = Arc::clone(&adapter);
                move |session_id| default_persistent_executor(service, adapter, session_id)
            },
        ))
        .await
        .expect("materialize session");

        let mut executor = PersistentRuntimeExecutor::new(
            Arc::clone(&service),
            Arc::clone(&adapter),
            result.session_id.clone(),
        );
        let output = executor
            .apply(
                RunId::new(),
                RunPrimitive::StagedInput(StagedRunInput {
                    boundary: meerkat_core::lifecycle::run_primitive::RunApplyBoundary::RunStart,
                    appends: Vec::new(),
                    context_appends: vec![ConversationContextAppend {
                        key: "peer_response_terminal:analyst-rt:req-456".to_string(),
                        content: CoreRenderable::Text {
                            text: "Peer terminal response from analyst-rt\nRequest ID: req-456\nStatus: completed\ndone".to_string(),
                        },
                    }],
                    contributing_input_ids: vec![InputId::new()],
                    turn_metadata: Some(RuntimeTurnMetadata {
                        execution_kind: Some(RuntimeExecutionKind::ContentTurn),
                        peer_response_terminal_apply_intent: Some(
                            PeerResponseTerminalApplyIntent::AppendContextAndRun,
                        ),
                        ..Default::default()
                    }),
                }),
            )
            .await
            .expect("terminal peer response should run requester reaction turn");

        match output.terminal.as_ref() {
            Some(CoreApplyTerminal::RunResult(run_result)) => {
                assert_eq!(run_result.text, "ok");
                assert_eq!(run_result.session_id, result.session_id);
            }
            other => panic!("expected terminal peer response run result, got {other:?}"),
        }

        let staged_snapshot = runtime_output_session_snapshot(&output);
        assert!(
            has_applied_runtime_context(
                &staged_snapshot,
                "peer_response_terminal:analyst-rt:req-456",
                "Peer terminal response from analyst-rt"
            ),
            "terminal peer response context must be applied before reaction turn: {:?}",
            staged_snapshot.system_context_state()
        );

        expect_unregister_completion(&adapter, &result.session_id).await;
    }

    #[tokio::test]
    async fn persistent_runtime_executor_rejects_malformed_terminal_peer_response_intent() {
        use meerkat_core::lifecycle::InputId;
        use meerkat_core::lifecycle::RunId;
        use meerkat_core::lifecycle::run_primitive::{
            ConversationContextAppend, PeerResponseTerminalApplyIntent, RunApplyBoundary,
            RuntimeExecutionKind, RuntimeTurnMetadata, StagedRunInput,
        };

        let temp = tempfile::tempdir().expect("tempdir");
        let (service, adapter) = build_test_service(&temp).await;
        let result = Box::pin(materialize_session(
            &service,
            &adapter,
            Session::new(),
            make_request(SessionBuildOptions::default()),
            {
                let service = Arc::clone(&service);
                let adapter = Arc::clone(&adapter);
                move |session_id| default_persistent_executor(service, adapter, session_id)
            },
        ))
        .await
        .expect("materialize session");

        let mut executor = PersistentRuntimeExecutor::new(
            Arc::clone(&service),
            Arc::clone(&adapter),
            result.session_id.clone(),
        );
        let error = executor
            .apply(
                RunId::new(),
                RunPrimitive::StagedInput(StagedRunInput {
                    boundary: RunApplyBoundary::Immediate,
                    appends: Vec::new(),
                    context_appends: vec![ConversationContextAppend {
                        key: "peer_response_terminal:analyst-rt:req-invalid".to_string(),
                        content: CoreRenderable::Text {
                            text: "Peer terminal response from analyst-rt\nRequest ID: req-invalid\nStatus: completed\ninvalid".to_string(),
                        },
                    }],
                    contributing_input_ids: vec![InputId::new()],
                    turn_metadata: Some(RuntimeTurnMetadata {
                        execution_kind: Some(RuntimeExecutionKind::ContentTurn),
                        peer_response_terminal_apply_intent: Some(
                            PeerResponseTerminalApplyIntent::AppendContextAndRun,
                        ),
                        ..Default::default()
                    }),
                }),
            )
            .await
            .expect_err("malformed terminal peer-response intent must be rejected");

        match error {
            CoreExecutorError::ApplyFailed { cause } => assert!(
                cause.message().contains("requires RunStart boundary"),
                "unexpected rejection reason: {cause}"
            ),
            other => panic!("expected ApplyFailed, got {other:?}"),
        }

        expect_unregister_completion(&adapter, &result.session_id).await;
    }

    #[cfg(all(
        feature = "openai",
        feature = "openai-realtime",
        not(target_arch = "wasm32")
    ))]
    struct FakeOpenAiLiveSession {
        events: VecDeque<OpenAiLiveServerEvent>,
        close_gate: Option<Arc<Notify>>,
        sent_events: Arc<Mutex<Vec<OpenAiLiveClientEvent>>>,
    }

    #[cfg(all(
        feature = "openai",
        feature = "openai-realtime",
        not(target_arch = "wasm32")
    ))]
    #[async_trait]
    impl OpenAiLiveSession for FakeOpenAiLiveSession {
        async fn send_raw(&mut self, event: OpenAiLiveClientEvent) -> Result<(), LlmError> {
            self.sent_events.lock().await.push(event);
            Ok(())
        }

        async fn next_event(&mut self) -> Result<Option<OpenAiLiveServerEvent>, LlmError> {
            if let Some(event) = self.events.pop_front() {
                return Ok(Some(event));
            }
            if let Some(gate) = self.close_gate.take() {
                gate.notified().await;
            }
            Ok(None)
        }
    }

    #[cfg(all(
        feature = "openai",
        feature = "openai-realtime",
        not(target_arch = "wasm32")
    ))]
    struct FakeOpenAiLiveFactory {
        sessions: Mutex<VecDeque<Result<Box<dyn OpenAiLiveSession>, LlmError>>>,
    }

    #[cfg(all(
        feature = "openai",
        feature = "openai-realtime",
        not(target_arch = "wasm32")
    ))]
    #[async_trait]
    impl OpenAiLiveSessionFactory for FakeOpenAiLiveFactory {
        async fn open_session(
            &self,
            _open_config: &meerkat_client::realtime_session::RealtimeSessionOpenConfig,
        ) -> Result<Box<dyn OpenAiLiveSession>, LlmError> {
            self.sessions
                .lock()
                .await
                .pop_front()
                .expect("fake openai live factory should have a queued session")
        }

        async fn attach_to_call(
            &self,
            _target: &OpenAiLiveCallTarget,
        ) -> Result<Box<dyn OpenAiLiveSession>, LlmError> {
            self.sessions
                .lock()
                .await
                .pop_front()
                .expect("fake openai live factory should have a queued session")
        }
    }

    #[cfg(all(
        feature = "openai",
        feature = "openai-realtime",
        not(target_arch = "wasm32")
    ))]
    struct RuntimeBackedRealtimeAttachmentToolDispatchHost {
        service: Arc<PersistentSessionService<FactoryAgentBuilder>>,
    }

    #[cfg(all(
        feature = "openai",
        feature = "openai-realtime",
        not(target_arch = "wasm32")
    ))]
    struct RuntimeBackedRealtimeTestReconfigureHost {
        identity: meerkat_core::SessionLlmIdentity,
        capability_surface: SessionLlmCapabilitySurface,
    }

    #[cfg(all(
        feature = "openai",
        feature = "openai-realtime",
        not(target_arch = "wasm32")
    ))]
    #[async_trait]
    impl SessionLlmReconfigureHost for RuntimeBackedRealtimeTestReconfigureHost {
        async fn hydrate_session_llm_state(
            &self,
            _session_id: &SessionId,
        ) -> Result<HydratedSessionLlmState, RuntimeDriverError> {
            Ok(HydratedSessionLlmState {
                current_identity: self.identity.clone(),
                current_visibility_state: Default::default(),
                current_capability_surface: Some(self.capability_surface.clone()),
                capability_surface_status: SessionLlmCapabilitySurfaceStatus::Resolved,
                base_tool_names: Default::default(),
            })
        }

        async fn resolve_target_session_llm_identity(
            &self,
            _request: &SessionLlmReconfigureRequest,
            _current_identity: &meerkat_core::SessionLlmIdentity,
        ) -> Result<ResolvedSessionLlmReconfigure, RuntimeDriverError> {
            Ok(ResolvedSessionLlmReconfigure {
                target_identity: self.identity.clone(),
                target_capability_surface: self.capability_surface.clone(),
            })
        }

        async fn apply_live_session_llm_identity(
            &self,
            _session_id: &SessionId,
            _identity: &meerkat_core::SessionLlmIdentity,
        ) -> Result<(), RuntimeDriverError> {
            Ok(())
        }

        async fn apply_live_session_tool_visibility_state(
            &self,
            _session_id: &SessionId,
            _visibility_state: Option<meerkat_core::SessionToolVisibilityState>,
        ) -> Result<(), RuntimeDriverError> {
            Ok(())
        }

        async fn persist_live_session(
            &self,
            _session_id: &SessionId,
        ) -> Result<(), RuntimeDriverError> {
            Ok(())
        }

        async fn discard_live_session(
            &self,
            _session_id: &SessionId,
        ) -> Result<(), RuntimeDriverError> {
            Ok(())
        }
    }
}
