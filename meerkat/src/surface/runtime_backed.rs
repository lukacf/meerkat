use std::sync::Arc;

use meerkat_core::lifecycle::core_executor::{
    CoreApplyOutput, CoreExecutor, CoreExecutorBoundaryHandle, CoreExecutorError,
    CoreExecutorInterruptHandle,
};
use meerkat_core::lifecycle::run_primitive::{
    ConversationContextAppend, CoreRenderable, RunPrimitive,
};
use meerkat_core::service::SessionService;
use meerkat_core::types::HandlingMode;
use meerkat_core::{
    SurfaceSessionRecoveryContext, SurfaceSessionRecoveryOverrides, build_recovered_session,
};
use meerkat_runtime::meerkat_machine::RuntimeBindingsError;
use meerkat_runtime::{MeerkatMachine, RuntimeDriverError};

#[cfg(all(test, feature = "jsonl-store", not(target_arch = "wasm32")))]
use crate::JsonlStore;
use crate::{
    CreateSessionRequest, FactoryAgentBuilder, PersistentSessionService, RunResult, Session,
    SessionError, SessionId,
};
#[cfg(all(test, feature = "jsonl-store", not(target_arch = "wasm32")))]
use meerkat_store::MemoryBlobStore;

const DEFAULT_RUNTIME_BACKED_ARCHIVED_HISTORY_CAPACITY: usize = 1024;

fn session_metadata_marks_archived(session: &Session) -> bool {
    session
        .metadata()
        .get("session_archived")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

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
    if request.initial_turn == meerkat_core::service::InitialTurnPolicy::RunImmediately {
        build.initial_turn_metadata = Some(meerkat_runtime::runtime_stamped_prompt_turn_metadata(
            build.initial_turn_metadata.take(),
        ));
    }
    request.build = Some(build);

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
    if request.initial_turn == meerkat_core::service::InitialTurnPolicy::RunImmediately {
        build.initial_turn_metadata = Some(meerkat_runtime::runtime_stamped_prompt_turn_metadata(
            build.initial_turn_metadata.take(),
        ));
    }
    request.build = Some(build);

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
    session_id: SessionId,
}

#[async_trait::async_trait]
impl CoreExecutorBoundaryHandle for PersistentRuntimeBoundaryHandle {
    async fn cancel_after_boundary(&self, _reason: String) -> Result<(), CoreExecutorError> {
        self.service
            .cancel_after_boundary(&self.session_id)
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
    session_id: SessionId,
}

#[async_trait::async_trait]
impl CoreExecutorInterruptHandle for PersistentRuntimeInterruptHandle {
    async fn hard_cancel_current_run(&self, _reason: String) -> Result<(), CoreExecutorError> {
        self.service
            .interrupt(&self.session_id)
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
        render_metadata: None,
        handling_mode: HandlingMode::Queue,
        event_tx: None,
        skill_references: None,
        flow_tool_overlay: None,
        pre_turn_context_appends,
        turn_metadata: metadata.cloned(),
    })
}

#[async_trait::async_trait]
impl CoreExecutor for PersistentRuntimeExecutor {
    fn boundary_handle(&self) -> Option<Arc<dyn CoreExecutorBoundaryHandle>> {
        Some(Arc::new(PersistentRuntimeBoundaryHandle {
            service: Arc::clone(&self.service),
            session_id: self.session_id.clone(),
        }))
    }

    fn interrupt_handle(&self) -> Option<Arc<dyn CoreExecutorInterruptHandle>> {
        Some(Arc::new(PersistentRuntimeInterruptHandle {
            service: Arc::clone(&self.service),
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
                    if session_metadata_marks_archived(&session) {
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
            .cancel_after_boundary(&self.session_id)
            .await
            .map_err(|error| CoreExecutorError::control_failed_runtime(error.to_string()))
    }

    async fn stop_runtime_executor(&mut self, _reason: String) -> Result<(), CoreExecutorError> {
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

#[cfg(all(test, feature = "jsonl-store", not(target_arch = "wasm32")))]
#[allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;

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
    #[cfg(all(
        feature = "openai",
        feature = "openai-realtime",
        not(target_arch = "wasm32")
    ))]
    use meerkat_openai::realtime_attachment::{
        OpenAiRealtimeAttachmentOrchestrator, RealtimeAttachmentToolDispatchHost,
    };
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
    #[cfg(all(
        feature = "openai",
        feature = "openai-realtime",
        not(target_arch = "wasm32")
    ))]
    use meerkat_runtime::{RealtimeAttachmentStatus, SessionServiceRuntimeExt};

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

        assert_eq!(req.render_metadata, None);
        assert_eq!(req.handling_mode, meerkat_core::types::HandlingMode::Queue);
        assert_eq!(req.skill_references, None);
        assert_eq!(req.flow_tool_overlay, None);
        assert_eq!(req.turn_metadata, Some(metadata));
        assert_eq!(
            req.turn_metadata
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

        tokio::time::timeout(Duration::from_secs(2), async {
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

        let error = tokio::time::timeout(Duration::from_secs(2), materialize_task)
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
    async fn direct_runtime_owned_eager_create_rejects_missing_execution_kind_stamp() {
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
            .expect_err("runtime-backed eager create must require stamped execution kind");

        assert!(
            error.to_string().contains("runtime_execution_kind not set"),
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
    async fn persistent_runtime_executor_stop_discards_and_unregisters() {
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

        let result = adapter
            .accept_input_with_completion(
                &result.session_id,
                Input::Prompt(PromptInput::new("must stay unregistered", None)),
            )
            .await;
        assert!(matches!(
            result,
            Err(RuntimeDriverError::NotReady {
                state: RuntimeState::Destroyed,
            })
        ));
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
                            text: "[SYSTEM NOTICE][PEER_RESPONSE_TERMINAL] Correlated peer response from analyst-rt. Request ID: req-123. Status: completed. Result: {\"request_intent\":\"checksum_token\",\"token\":\"birch seventeen\"}.".to_string(),
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
            .archive(&result.session_id)
            .await
            .expect("archive session through service");
        assert!(
            adapter.contains_session(&result.session_id).await,
            "service-only archive leaves runtime registration for this regression"
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
                            text: "[SYSTEM NOTICE][PEER_RESPONSE_TERMINAL] done".to_string(),
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
            system_context.contains("[SYSTEM NOTICE][PEER_RESPONSE_TERMINAL] done"),
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
                            text: "[SYSTEM NOTICE][PEER_RESPONSE_TERMINAL] invalid".to_string(),
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

    #[cfg(all(
        feature = "openai",
        feature = "openai-realtime",
        not(target_arch = "wasm32")
    ))]
    fn realtime_test_capability_surface() -> SessionLlmCapabilitySurface {
        SessionLlmCapabilitySurface {
            supports_temperature: true,
            supports_thinking: false,
            supports_reasoning: false,
            inline_video: false,
            vision: false,
            image_tool_results: false,
            supports_web_search: false,
            realtime: true,
            call_timeout_secs: None,
        }
    }

    #[cfg(all(
        feature = "openai",
        feature = "openai-realtime",
        not(target_arch = "wasm32")
    ))]
    fn install_realtime_test_reconfigure_host(adapter: &Arc<MeerkatMachine>) {
        adapter.set_session_llm_reconfigure_host(Arc::new(
            RuntimeBackedRealtimeTestReconfigureHost {
                identity: meerkat_core::SessionLlmIdentity {
                    model: "gpt-realtime".to_string(),
                    provider: meerkat_core::Provider::OpenAI,
                    self_hosted_server_id: None,
                    provider_params: None,
                    connection_ref: None,
                },
                capability_surface: realtime_test_capability_surface(),
            },
        ));
    }

    #[cfg(all(
        feature = "openai",
        feature = "openai-realtime",
        not(target_arch = "wasm32")
    ))]
    #[async_trait]
    impl RealtimeAttachmentToolDispatchHost for RuntimeBackedRealtimeAttachmentToolDispatchHost {
        async fn dispatch_external_tool_call(
            &self,
            session_id: &SessionId,
            call: meerkat_core::ToolCall,
        ) -> Result<meerkat_core::ToolDispatchOutcome, RuntimeDriverError> {
            self.service
                .dispatch_external_tool_call(session_id, call)
                .await
                .map_err(|err| match err {
                    SessionError::NotFound { .. } => RuntimeDriverError::NotReady {
                        state: RuntimeState::Destroyed,
                    },
                    other => RuntimeDriverError::Internal(other.to_string()),
                })
        }
    }

    #[cfg(all(
        feature = "openai",
        feature = "openai-realtime",
        not(target_arch = "wasm32")
    ))]
    #[tokio::test]
    async fn runtime_backed_openai_live_orchestrator_routes_tool_calls_through_service_host() {
        let temp = tempfile::tempdir().expect("tempdir");
        let (service, adapter) = build_test_service(&temp).await;
        install_realtime_test_reconfigure_host(&adapter);
        let mut request = make_request(SessionBuildOptions::default());
        request.model = "gpt-realtime".to_string();
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
        .expect("materialize session");
        let session_id = result.session_id.clone();

        adapter
            .project_realtime_attachment_intent(&session_id, true)
            .await
            .expect("project live attachment intent");

        let hold_open = Arc::new(Notify::new());
        let sent_events = Arc::new(Mutex::new(Vec::new()));
        let factory = Arc::new(FakeOpenAiLiveFactory {
            sessions: Mutex::new(VecDeque::from([Ok(Box::new(FakeOpenAiLiveSession {
                events: VecDeque::from([
                    OpenAiLiveServerEvent::ResponseFunctionCallArgumentsDone {
                        event_id: "evt_1".to_string(),
                        response_id: "resp_1".to_string(),
                        item_id: "item_1".to_string(),
                        output_index: 0,
                        call_id: "call_1".to_string(),
                        name: "missing_tool".to_string(),
                        arguments: r#"{"x":1}"#.to_string(),
                    },
                ]),
                close_gate: Some(hold_open.clone()),
                sent_events: sent_events.clone(),
            })
                as Box<dyn OpenAiLiveSession>)])),
        });
        let orchestrator = OpenAiRealtimeAttachmentOrchestrator::new(
            Arc::clone(&adapter),
            factory,
            Arc::new(RuntimeBackedRealtimeAttachmentToolDispatchHost {
                service: Arc::clone(&service),
            }),
        );
        let target =
            OpenAiLiveCallTarget::new("call_runtime_backed").expect("call target should succeed");

        orchestrator
            .ensure_attached_for_capable_session(&session_id, &target)
            .await
            .expect("attach should succeed");

        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if !sent_events.lock().await.is_empty() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("function-call error should be routed back through the provider session");

        let sent = sent_events.lock().await.clone();
        assert_eq!(sent.len(), 1);
        let payload =
            serde_json::to_value(&sent[0]).expect("provider event should serialize for checks");
        assert_eq!(payload["item"]["call_id"], "call_1");
        assert!(
            payload["item"]["output"]
                .as_str()
                .expect("function-call output should serialize as text")
                .contains("missing_tool"),
            "missing tool errors should flow through the runtime-backed service host"
        );

        let status = adapter
            .realtime_attachment_status(&session_id)
            .await
            .expect("runtime-backed session should expose live attachment status");
        assert_eq!(status, RealtimeAttachmentStatus::BindingReady);

        hold_open.notify_waiters();
        service
            .discard_live_session(&session_id)
            .await
            .expect("discard live session");
        adapter.unregister_session(&session_id).await;
    }
}
