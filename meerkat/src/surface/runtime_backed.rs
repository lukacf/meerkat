use std::sync::Arc;

use meerkat_core::lifecycle::core_executor::{
    CoreApplyOutput, CoreExecutor, CoreExecutorBoundaryHandle, CoreExecutorError,
    CoreExecutorInterruptHandle,
};
use meerkat_core::lifecycle::run_primitive::{
    ConversationContextAppend, CoreRenderable, RunPrimitive, RuntimeTurnMetadata,
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
use meerkat_runtime::{MeerkatMachine, RuntimeDriverError};
use tokio::sync::mpsc;

#[cfg(all(test, feature = "jsonl-store", not(target_arch = "wasm32")))]
use crate::JsonlStore;
#[cfg(test)]
use crate::MachineSessionArchiveProtocol;
use crate::{
    CreateSessionRequest, FactoryAgentBuilder, MachineServiceTurnCommitProtocol,
    PersistentSessionService, RunResult, Session, SessionError, SessionId,
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
    event_tx: Option<mpsc::Sender<EventEnvelope<AgentEvent>>>,
    turn_metadata: RuntimeTurnMetadata,
}

pub fn split_runtime_backed_eager_create_request(
    mut request: CreateSessionRequest,
) -> (CreateSessionRequest, Option<RuntimeBackedInitialTurn>) {
    if request.initial_turn != InitialTurnPolicy::RunImmediately {
        return (request, None);
    }

    let mut turn_metadata = request
        .build
        .as_mut()
        .and_then(|build| build.initial_turn_metadata.take())
        .unwrap_or_default();
    if turn_metadata.render_metadata.is_none() {
        turn_metadata.render_metadata = request.render_metadata.take();
    } else {
        request.render_metadata = None;
    }
    if turn_metadata.skill_references.is_none() {
        turn_metadata.skill_references = request.skill_references.take();
    } else {
        request.skill_references = None;
    }

    let prompt = std::mem::replace(
        &mut request.prompt,
        meerkat_core::types::ContentInput::Text(String::new()),
    );
    let event_tx = request.event_tx.take();
    request.initial_turn = InitialTurnPolicy::Defer;
    request.deferred_prompt_policy = DeferredPromptPolicy::Discard;

    (
        request,
        Some(RuntimeBackedInitialTurn {
            prompt,
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
        system_prompt: None,
        event_tx: initial_turn.event_tx,
        runtime: StartTurnRuntimeSemantics::new(
            None,
            initial_turn
                .turn_metadata
                .handling_mode
                .unwrap_or(HandlingMode::Queue),
            None,
            None,
            Vec::new(),
            Some(initial_turn.turn_metadata),
        ),
    }
}

async fn materialize_error_preserves_runtime_session(
    service: &Arc<PersistentSessionService<FactoryAgentBuilder>>,
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

pub async fn run_runtime_backed_initial_turn_with_machine(
    service: &Arc<PersistentSessionService<FactoryAgentBuilder>>,
    adapter: &Arc<MeerkatMachine>,
    session_id: &SessionId,
    initial_turn: RuntimeBackedInitialTurn,
) -> Result<RunResult, SurfaceRuntimeMaterializeError> {
    let admission = service.reserve_runtime_turn_admission(session_id).await?;
    let request = start_turn_request_from_initial_turn(initial_turn);
    let result = service
        .run_machine_committed_live_turn(
            MachineServiceTurnCommitProtocol::from_machine(adapter),
            session_id,
            request,
            admission,
        )
        .await;
    result.map_err(|(error, _admission)| SurfaceRuntimeMaterializeError::Session(error))
}

pub async fn materialize_session<F>(
    service: &Arc<PersistentSessionService<FactoryAgentBuilder>>,
    adapter: &Arc<MeerkatMachine>,
    session: Session,
    mut request: CreateSessionRequest,
    executor_factory: F,
) -> Result<RunResult, SurfaceRuntimeMaterializeError>
where
    F: FnOnce(SessionId) -> Box<dyn CoreExecutor>,
{
    let prepared_session_id = session.id().clone();
    let reserved_admission = service.reserve_create_session_admission().await?;
    let runtime_was_registered = adapter.contains_session(&prepared_session_id).await;
    let bindings = match adapter.prepare_bindings(prepared_session_id.clone()).await {
        Ok(bindings) => bindings,
        Err(error) => {
            if !runtime_was_registered {
                adapter.unregister_session(&prepared_session_id).await;
            }
            return Err(SurfaceRuntimeMaterializeError::RuntimeBindings(error));
        }
    };
    if let Err(error) =
        install_prepared_runtime_interrupt_handle(service, adapter, &prepared_session_id).await
    {
        if !runtime_was_registered {
            adapter.unregister_session(&prepared_session_id).await;
        }
        return Err(SurfaceRuntimeMaterializeError::RuntimeDriver(error));
    }

    let mut build = request.build.unwrap_or_default();
    build.resume_session = Some(session);
    build.runtime_build_mode = meerkat_core::RuntimeBuildMode::SessionOwned(bindings);
    request.build = Some(build);
    let (request, initial_turn) = split_runtime_backed_eager_create_request(request);

    let result = match service
        .create_session_with_reserved_admission(request, reserved_admission)
        .await
    {
        Ok(result) => result,
        Err(error) => {
            if !runtime_was_registered {
                adapter.unregister_session(&prepared_session_id).await;
            }
            return Err(SurfaceRuntimeMaterializeError::Session(error));
        }
    };

    if let Err(error) =
        ensure_materialized_session_id_matches(&prepared_session_id, &result.session_id)
    {
        if !runtime_was_registered {
            adapter.unregister_session(&prepared_session_id).await;
        }
        return Err(error);
    }

    let result = match initial_turn {
        Some(initial_turn) => {
            match run_runtime_backed_initial_turn_with_machine(
                service,
                adapter,
                &prepared_session_id,
                initial_turn,
            )
            .await
            {
                Ok(result) => result,
                Err(error) => {
                    if !runtime_was_registered
                        && !materialize_error_preserves_runtime_session(
                            service,
                            &prepared_session_id,
                            &error,
                        )
                        .await
                    {
                        adapter.unregister_session(&prepared_session_id).await;
                    }
                    return Err(error);
                }
            }
        }
        None => result,
    };

    if let Err(error) =
        ensure_materialized_session_id_matches(&prepared_session_id, &result.session_id)
    {
        if !runtime_was_registered {
            adapter.unregister_session(&prepared_session_id).await;
        }
        return Err(error);
    }

    adapter
        .ensure_session_with_executor(
            result.session_id.clone(),
            executor_factory(result.session_id.clone()),
        )
        .await;

    Ok(result)
}

pub async fn materialize_session_with_reserved_admission<F>(
    service: &Arc<PersistentSessionService<FactoryAgentBuilder>>,
    adapter: &Arc<MeerkatMachine>,
    session: Session,
    mut request: CreateSessionRequest,
    reserved_admission: crate::RuntimeContextAdmissionGuard,
    executor_factory: F,
) -> Result<RunResult, SurfaceRuntimeMaterializeError>
where
    F: FnOnce(SessionId) -> Box<dyn CoreExecutor>,
{
    let prepared_session_id = session.id().clone();
    let runtime_was_registered = adapter.contains_session(&prepared_session_id).await;
    let bindings = match adapter.prepare_bindings(prepared_session_id.clone()).await {
        Ok(bindings) => bindings,
        Err(error) => {
            if !runtime_was_registered {
                adapter.unregister_session(&prepared_session_id).await;
            }
            return Err(SurfaceRuntimeMaterializeError::RuntimeBindings(error));
        }
    };
    if let Err(error) =
        install_prepared_runtime_interrupt_handle(service, adapter, &prepared_session_id).await
    {
        if !runtime_was_registered {
            adapter.unregister_session(&prepared_session_id).await;
        }
        return Err(SurfaceRuntimeMaterializeError::RuntimeDriver(error));
    }

    let mut build = request.build.unwrap_or_default();
    build.resume_session = Some(session);
    build.runtime_build_mode = meerkat_core::RuntimeBuildMode::SessionOwned(bindings);
    request.build = Some(build);
    let (request, initial_turn) = split_runtime_backed_eager_create_request(request);

    let result = match service
        .create_session_with_reserved_admission(request, reserved_admission)
        .await
    {
        Ok(result) => result,
        Err(error) => {
            if !runtime_was_registered {
                adapter.unregister_session(&prepared_session_id).await;
            }
            return Err(SurfaceRuntimeMaterializeError::Session(error));
        }
    };

    if let Err(error) =
        ensure_materialized_session_id_matches(&prepared_session_id, &result.session_id)
    {
        if !runtime_was_registered {
            adapter.unregister_session(&prepared_session_id).await;
        }
        return Err(error);
    }

    let result = match initial_turn {
        Some(initial_turn) => {
            match run_runtime_backed_initial_turn_with_machine(
                service,
                adapter,
                &prepared_session_id,
                initial_turn,
            )
            .await
            {
                Ok(result) => result,
                Err(error) => {
                    if !runtime_was_registered
                        && !materialize_error_preserves_runtime_session(
                            service,
                            &prepared_session_id,
                            &error,
                        )
                        .await
                    {
                        adapter.unregister_session(&prepared_session_id).await;
                    }
                    return Err(error);
                }
            }
        }
        None => result,
    };

    if let Err(error) =
        ensure_materialized_session_id_matches(&prepared_session_id, &result.session_id)
    {
        if !runtime_was_registered {
            adapter.unregister_session(&prepared_session_id).await;
        }
        return Err(error);
    }

    adapter
        .ensure_session_with_executor(
            result.session_id.clone(),
            executor_factory(result.session_id.clone()),
        )
        .await;

    Ok(result)
}

pub async fn install_prepared_runtime_interrupt_handle(
    service: &Arc<PersistentSessionService<FactoryAgentBuilder>>,
    adapter: &Arc<MeerkatMachine>,
    session_id: &SessionId,
) -> Result<(), RuntimeDriverError> {
    adapter
        .install_prepared_session_interrupt_handle(
            session_id,
            Arc::new(PersistentRuntimeInterruptHandle {
                service: Arc::clone(service),
                adapter: Arc::clone(adapter),
                session_id: session_id.clone(),
            }),
        )
        .await
}

#[cfg(feature = "comms")]
pub async fn configure_peer_ingress(
    adapter: &Arc<MeerkatMachine>,
    service: &Arc<PersistentSessionService<FactoryAgentBuilder>>,
    session_id: &SessionId,
    keep_alive: bool,
) {
    let comms_rt = service.comms_runtime(session_id).await;
    adapter
        .update_peer_ingress_context(session_id, keep_alive, comms_rt)
        .await;
}

pub fn default_persistent_executor(
    service: Arc<PersistentSessionService<FactoryAgentBuilder>>,
    adapter: Arc<MeerkatMachine>,
    session_id: SessionId,
) -> Box<dyn CoreExecutor> {
    Box::new(PersistentRuntimeExecutor::new(service, adapter, session_id))
}

pub struct PersistentRuntimeExecutor {
    service: Arc<PersistentSessionService<FactoryAgentBuilder>>,
    adapter: Arc<MeerkatMachine>,
    session_id: SessionId,
}

struct PersistentRuntimeBoundaryHandle {
    service: Arc<PersistentSessionService<FactoryAgentBuilder>>,
    adapter: Arc<MeerkatMachine>,
    session_id: SessionId,
}

#[async_trait::async_trait]
impl CoreExecutorBoundaryHandle for PersistentRuntimeBoundaryHandle {
    async fn cancel_after_boundary(&self, _reason: String) -> Result<(), CoreExecutorError> {
        self.service
            .cancel_after_boundary_with_machine_authority(
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

struct PersistentRuntimeInterruptHandle {
    service: Arc<PersistentSessionService<FactoryAgentBuilder>>,
    adapter: Arc<MeerkatMachine>,
    session_id: SessionId,
}

#[async_trait::async_trait]
impl CoreExecutorInterruptHandle for PersistentRuntimeInterruptHandle {
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

impl PersistentRuntimeExecutor {
    pub fn new(
        service: Arc<PersistentSessionService<FactoryAgentBuilder>>,
        adapter: Arc<MeerkatMachine>,
        session_id: SessionId,
    ) -> Self {
        Self {
            service,
            adapter,
            session_id,
        }
    }
}

fn pending_system_context_appends(
    appends: &[ConversationContextAppend],
) -> Vec<meerkat_core::PendingSystemContextAppend> {
    appends
        .iter()
        .map(|append| meerkat_core::PendingSystemContextAppend {
            text: match &append.content {
                CoreRenderable::Text { text } => text.clone(),
                CoreRenderable::Blocks { blocks } => meerkat_core::types::text_content(blocks),
                CoreRenderable::Json { value } => {
                    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
                }
                CoreRenderable::Reference { uri, label } => match label {
                    Some(label) => format!("{label}: {uri}"),
                    None => uri.clone(),
                },
                CoreRenderable::SystemNotice { kind, body, blocks } => {
                    meerkat_core::types::SystemNoticeMessage::with_blocks(
                        *kind,
                        body.clone(),
                        blocks.clone(),
                    )
                    .model_projection_text()
                }
                _ => String::new(),
            },
            source: Some(append.key.clone()),
            idempotency_key: Some(append.key.clone()),
            accepted_at: meerkat_core::time_compat::SystemTime::now(),
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
        prompt: primitive.extract_content_input(),
        system_prompt: None,
        event_tx: None,
        runtime: StartTurnRuntimeSemantics::new(
            None,
            HandlingMode::Queue,
            None,
            None,
            pre_turn_context_appends,
            metadata.cloned(),
        )
        .with_typed_turn_appends(primitive.typed_turn_appends()),
    })
}

#[async_trait::async_trait]
impl CoreExecutor for PersistentRuntimeExecutor {
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
                Err(SessionError::NotFound { .. }) => {
                    let session = self
                        .service
                        .load_authoritative_session(&self.session_id)
                        .await
                        .map_err(|error| {
                            CoreExecutorError::apply_failed_runtime_context(error.to_string())
                        })?
                        .ok_or_else(|| {
                            CoreExecutorError::apply_failed_runtime_context(format!(
                                "session not found: {}",
                                self.session_id
                            ))
                        })?;
                    if self
                        .service
                        .session_archived_by_authority(&self.session_id, &session)
                        .await
                        .map_err(|error| {
                            CoreExecutorError::apply_failed_runtime_context(error.to_string())
                        })?
                    {
                        self.adapter.unregister_session(&self.session_id).await;
                        return Err(CoreExecutorError::apply_failed_runtime_context(format!(
                            "session not found: {}",
                            self.session_id
                        )));
                    }
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
                    Box::pin(materialize_session(
                        &self.service,
                        &self.adapter,
                        session,
                        recovered.into_deferred_create_request(),
                        move |session_id| {
                            default_persistent_executor(
                                Arc::clone(&service),
                                Arc::clone(&adapter),
                                session_id,
                            )
                        },
                    ))
                    .await
                    .map_err(|error| {
                        CoreExecutorError::apply_failed_runtime_context(error.to_string())
                    })?;
                    return self
                        .service
                        .apply_runtime_context_appends_with_boundary(
                            &self.session_id,
                            run_id,
                            appends,
                            boundary,
                            contributing_input_ids,
                        )
                        .await
                        .map_err(|error| {
                            CoreExecutorError::apply_failed_runtime_context(error.to_string())
                        });
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
        let req = start_turn_request_from_primitive(&primitive)?;
        self.service
            .apply_runtime_turn(
                &self.session_id,
                run_id,
                req,
                boundary,
                contributing_input_ids,
            )
            .await
            .map_err(CoreExecutorError::apply_failed_from_session_error)
    }

    async fn cancel_after_boundary(&mut self, _reason: String) -> Result<(), CoreExecutorError> {
        self.service
            .cancel_after_boundary_with_machine_authority(
                &self.session_id,
                self.adapter.session_control_authority(),
            )
            .await
            .map_err(|error| CoreExecutorError::control_failed_runtime(error.to_string()))
    }

    async fn stop_runtime_executor(&mut self, _reason: String) -> Result<(), CoreExecutorError> {
        Ok(())
    }

    async fn cleanup_after_runtime_stop_terminalized(&mut self) -> Result<(), CoreExecutorError> {
        let discard_result = self.service.discard_live_session(&self.session_id).await;
        self.adapter.unregister_session(&self.session_id).await;
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
    use meerkat_core::service::SessionService;

    #[cfg(all(
        feature = "openai",
        feature = "openai-realtime",
        not(target_arch = "wasm32")
    ))]
    use std::collections::VecDeque;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, Ordering};

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
            flow_tool_overlay: Some(meerkat_core::service::TurnToolOverlay {
                allowed_tools: Some(vec!["runtime_tool".to_string()]),
                blocked_tools: None,
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

        assert_eq!(req.runtime.render_metadata, None);
        assert_eq!(
            req.runtime.handling_mode,
            meerkat_core::types::HandlingMode::Queue
        );
        assert_eq!(req.runtime.skill_references, None);
        assert_eq!(req.runtime.flow_tool_overlay, None);
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
            model: "gpt-5.4".to_string(),
            prompt: meerkat_core::ContentInput::Text(String::new()),
            render_metadata: None,
            system_prompt: Some("surface runtime regression".to_string()),
            max_tokens: None,
            event_tx: None,
            skill_references: None,
            initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
            deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
            build: Some(build),
            labels: None,
        }
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
            None,
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

        fn provider(&self) -> &'static str {
            "blocking-test"
        }

        async fn health_check(&self) -> Result<(), meerkat_client::LlmError> {
            Ok(())
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

        fn provider(&self) -> &'static str {
            "terminal-failure-test"
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
            .expect("prompt should complete");
        assert!(
            matches!(outcome, CompletionOutcome::Completed(ref run) if run.text == "ok"),
            "unexpected completion outcome: {outcome:?}"
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
        adapter.unregister_session(&session_id).await;
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
        adapter.unregister_session(&existing.session_id).await;
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
        service
            .discard_live_session(&result.session_id)
            .await
            .expect("discard live session");
        adapter.unregister_session(&result.session_id).await;
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
        service
            .discard_live_session(&result.session_id)
            .await
            .expect("discard live session");
        adapter.unregister_session(&result.session_id).await;
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

        service
            .discard_live_session(&session_id)
            .await
            .expect("discard live session");
        adapter.unregister_session(&session_id).await;
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
        service
            .discard_live_session(&result.session_id)
            .await
            .expect("discard live session");
        adapter.unregister_session(&result.session_id).await;
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
        let _ = service.discard_live_session(&session_id).await;
        adapter.unregister_session(&session_id).await;
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
        adapter.unregister_session(&session_id).await;
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

        service
            .discard_live_session(&result.session_id)
            .await
            .expect("discard live session");
        adapter.unregister_session(&result.session_id).await;
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

        service
            .discard_live_session(&result.session_id)
            .await
            .expect("discard live session");
        adapter.unregister_session(&result.session_id).await;
    }

    #[tokio::test]
    async fn persistent_runtime_executor_preserves_typed_terminal_failure_cause() {
        use futures::StreamExt;
        use meerkat_core::event::AgentEvent;
        use meerkat_core::lifecycle::InputId;
        use meerkat_core::lifecycle::RunId;
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
        let error = executor
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
            .expect_err("terminal LLM failure should surface as typed executor failure");

        match error {
            CoreExecutorError::TerminalFailure {
                outcome,
                cause_kind,
                message,
            } => {
                assert_eq!(outcome, meerkat_core::TurnTerminalOutcome::Failed);
                assert_eq!(cause_kind, meerkat_core::TurnTerminalCauseKind::LlmFailure);
                assert_eq!(
                    message,
                    meerkat_core::TurnTerminalCauseKind::LlmFailure
                        .default_message(meerkat_core::TurnTerminalOutcome::Failed)
                );
            }
            other => panic!("expected typed terminal failure, got {other:?}"),
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
                error_report: Some(report),
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

        match service.discard_live_session(&result.session_id).await {
            Ok(()) | Err(SessionError::NotFound { .. }) => {}
            Err(error) => panic!("discard live session failed: {error}"),
        }
        adapter.unregister_session(&result.session_id).await;
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

        configure_peer_ingress(&adapter, &service, &result.session_id, true).await;
        expect_prompt_completion(&adapter, &result.session_id, "peer ingress prompt").await;
        service
            .discard_live_session(&result.session_id)
            .await
            .expect("discard live session");
        adapter.unregister_session(&result.session_id).await;
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

        configure_peer_ingress(&adapter, &service, &result.session_id, false).await;
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

        service
            .discard_live_session(&result.session_id)
            .await
            .expect("discard live session");
        adapter.unregister_session(&result.session_id).await;
    }

    #[tokio::test]
    async fn persistent_runtime_executor_routes_context_only_staged_input_through_runtime_context_appends()
     {
        use futures::StreamExt;
        use meerkat_core::event::AgentEvent;
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

        service
            .discard_live_session(&result.session_id)
            .await
            .expect("discard live session");
        adapter.unregister_session(&result.session_id).await;
    }

    #[tokio::test]
    async fn persistent_runtime_executor_recovers_persisted_session_for_context_only_apply() {
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
            .discard_live_session(&result.session_id)
            .await
            .expect("discard live session");
        adapter.unregister_session(&result.session_id).await;

        let mut executor = PersistentRuntimeExecutor::new(
            Arc::clone(&service),
            Arc::clone(&adapter),
            result.session_id.clone(),
        );
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
            .expect("context-only apply should recover persisted session");

        assert_eq!(
            output.receipt.boundary,
            meerkat_core::lifecycle::run_primitive::RunApplyBoundary::RunCheckpoint
        );
        assert!(adapter.contains_session(&result.session_id).await);

        let exported = service
            .export_live_session(&result.session_id)
            .await
            .expect("export recovered live session");
        let system_context = exported
            .messages()
            .iter()
            .find_map(|message| match message {
                meerkat_core::types::Message::System(system) => Some(system.content.as_str()),
                _ => None,
            })
            .unwrap_or("");
        assert!(
            system_context.contains("runtime-backed-context-recovery")
                && system_context.contains("runtime-backed recovered context"),
            "context-only recovery should persist runtime context append: {system_context}"
        );

        service
            .discard_live_session(&result.session_id)
            .await
            .expect("discard live session");
        adapter.unregister_session(&result.session_id).await;
    }

    #[tokio::test]
    async fn persistent_runtime_executor_archived_context_only_apply_unregisters_runtime() {
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

        assert!(
            rejected.is_err(),
            "archived context-only apply should reject: {rejected:?}"
        );
        assert!(
            !adapter.contains_session(&result.session_id).await,
            "archived context-only rejection should unregister stale runtime state"
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

        match output.terminal {
            Some(CoreApplyTerminal::RunResult(run_result)) => {
                assert_eq!(run_result.text, "ok");
                assert_eq!(run_result.session_id, result.session_id);
            }
            other => panic!("expected terminal peer response run result, got {other:?}"),
        }

        let exported = service
            .export_live_session(&result.session_id)
            .await
            .expect("export live session after terminal peer response");
        let system_context = exported
            .messages()
            .iter()
            .find_map(|message| match message {
                meerkat_core::types::Message::System(system) => Some(system.content.as_str()),
                _ => None,
            })
            .unwrap_or("");
        assert!(
            system_context.contains("peer_response_terminal:analyst-rt:req-456"),
            "terminal peer response source must be applied before reaction turn: {system_context}"
        );
        assert!(
            system_context.contains("Peer terminal response from analyst-rt"),
            "terminal peer response context must be applied before reaction turn: {system_context}"
        );

        service
            .discard_live_session(&result.session_id)
            .await
            .expect("discard live session");
        adapter.unregister_session(&result.session_id).await;
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

        service
            .discard_live_session(&result.session_id)
            .await
            .expect("discard live session");
        adapter.unregister_session(&result.session_id).await;
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
