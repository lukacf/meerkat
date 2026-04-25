use std::sync::Arc;

use meerkat_core::lifecycle::core_executor::{CoreApplyOutput, CoreExecutor, CoreExecutorError};
use meerkat_core::lifecycle::run_control::RunControlCommand;
use meerkat_core::lifecycle::run_primitive::{CoreRenderable, RunPrimitive, RuntimeTurnMetadata};
use meerkat_core::service::SessionService;
use meerkat_core::types::HandlingMode;
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

#[cfg(feature = "session-store")]
pub fn build_runtime_backed_service(
    mut builder: FactoryAgentBuilder,
    max_sessions: usize,
    persistence: crate::PersistenceBundle,
) -> (
    PersistentSessionService<FactoryAgentBuilder>,
    Arc<MeerkatMachine>,
) {
    let runtime_adapter = persistence.runtime_adapter();
    let (store, runtime_store, blob_store) = persistence.into_parts();
    builder.default_session_store = Some(Arc::new(meerkat_store::StoreAdapter::new(Arc::clone(
        &store,
    ))));
    let service =
        PersistentSessionService::new(builder, max_sessions, store, runtime_store, blob_store);
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
    let bindings = adapter
        .prepare_bindings(prepared_session_id.clone())
        .await?;

    let mut build = request.build.unwrap_or_default();
    build.resume_session = Some(session);
    build.runtime_build_mode = meerkat_core::RuntimeBuildMode::SessionOwned(bindings);
    request.build = Some(build);

    let result = match service.create_session(request).await {
        Ok(result) => result,
        Err(error) => {
            adapter.unregister_session(&prepared_session_id).await;
            return Err(SurfaceRuntimeMaterializeError::Session(error));
        }
    };

    if let Err(error) =
        ensure_materialized_session_id_matches(&prepared_session_id, &result.session_id)
    {
        adapter.unregister_session(&prepared_session_id).await;
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
    primitive: &RunPrimitive,
) -> Option<Vec<meerkat_core::PendingSystemContextAppend>> {
    let RunPrimitive::StagedInput(staged) = primitive else {
        return None;
    };
    if !staged.appends.is_empty() || staged.context_appends.is_empty() {
        return None;
    }

    Some(
        staged
            .context_appends
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
            .collect(),
    )
}

fn unsupported_runtime_metadata_fields(metadata: &RuntimeTurnMetadata) -> Vec<&'static str> {
    let mut fields = Vec::new();
    if metadata.model.is_some() {
        fields.push("model");
    }
    if metadata.provider.is_some() {
        fields.push("provider");
    }
    if metadata.provider_params.is_some() {
        fields.push("provider_params");
    }
    if metadata.connection_ref.is_some() {
        fields.push("connection_ref");
    }
    if metadata.keep_alive.is_some() {
        fields.push("keep_alive");
    }
    if metadata.additional_instructions.is_some() {
        fields.push("additional_instructions");
    }
    fields
}

fn start_turn_request_from_primitive(
    primitive: &RunPrimitive,
) -> Result<meerkat_core::service::StartTurnRequest, CoreExecutorError> {
    let metadata = primitive.turn_metadata();
    if let Some(metadata) = metadata {
        let unsupported = unsupported_runtime_metadata_fields(metadata);
        if !unsupported.is_empty() {
            return Err(CoreExecutorError::ApplyFailed {
                reason: format!(
                    "runtime-backed apply cannot yet project {} through StartTurnRequest; refusing to drop canonical RuntimeTurnMetadata fields",
                    unsupported.join(", ")
                ),
            });
        }
    }

    Ok(meerkat_core::service::StartTurnRequest {
        prompt: primitive.extract_content_input(),
        system_prompt: None,
        render_metadata: metadata.and_then(|metadata| metadata.render_metadata.clone()),
        handling_mode: metadata
            .and_then(|metadata| metadata.handling_mode)
            .unwrap_or(HandlingMode::Queue),
        event_tx: None,
        skill_references: metadata.and_then(|metadata| metadata.skill_references.clone()),
        flow_tool_overlay: metadata.and_then(|metadata| metadata.flow_tool_overlay.clone()),
    })
}

#[async_trait::async_trait]
impl CoreExecutor for PersistentRuntimeExecutor {
    async fn apply(
        &mut self,
        run_id: meerkat_core::lifecycle::RunId,
        primitive: RunPrimitive,
    ) -> Result<CoreApplyOutput, CoreExecutorError> {
        if let Some(appends) = pending_system_context_appends(&primitive) {
            return self
                .service
                .apply_runtime_context_appends_with_boundary(
                    &self.session_id,
                    run_id,
                    appends,
                    primitive.apply_boundary(),
                    primitive.contributing_input_ids().to_vec(),
                )
                .await
                .map_err(|error| CoreExecutorError::ApplyFailed {
                    reason: error.to_string(),
                });
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
            .map_err(|error| CoreExecutorError::ApplyFailed {
                reason: error.to_string(),
            })
    }

    async fn control(&mut self, command: RunControlCommand) -> Result<(), CoreExecutorError> {
        match command {
            RunControlCommand::CancelCurrentRun { .. } => self
                .service
                .interrupt(&self.session_id)
                .await
                .map_err(|error| CoreExecutorError::ControlFailed {
                    reason: error.to_string(),
                }),
            RunControlCommand::CancelAfterBoundary { .. } => self
                .service
                .cancel_after_boundary(&self.session_id)
                .await
                .map_err(|error| CoreExecutorError::ControlFailed {
                    reason: error.to_string(),
                }),
            RunControlCommand::StopRuntimeExecutor { .. } => {
                let discard_result = self.service.discard_live_session(&self.session_id).await;
                self.adapter.unregister_session(&self.session_id).await;
                match discard_result {
                    Ok(()) | Err(SessionError::NotFound { .. }) => Ok(()),
                    Err(error) => Err(CoreExecutorError::ControlFailed {
                        reason: error.to_string(),
                    }),
                }
            }
            _ => Ok(()),
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

    use std::collections::VecDeque;
    use std::path::PathBuf;

    #[cfg(feature = "openai")]
    use async_trait::async_trait;
    use meerkat_client::TestClient;
    use meerkat_core::SessionBuildOptions;
    #[cfg(feature = "openai")]
    use meerkat_llm_core::LlmError;
    #[cfg(feature = "openai")]
    use meerkat_openai::live::{
        OpenAiLiveCallTarget, OpenAiLiveClientEvent, OpenAiLiveServerEvent, OpenAiLiveSession,
        OpenAiLiveSessionFactory,
    };
    #[cfg(feature = "openai")]
    use meerkat_openai::realtime_attachment::{
        OpenAiRealtimeAttachmentOrchestrator, RealtimeAttachmentToolDispatchHost,
    };
    use meerkat_runtime::completion::CompletionOutcome;
    use meerkat_runtime::{Input, PromptInput, RuntimeState};
    use tempfile::TempDir;
    #[cfg(feature = "openai")]
    use tokio::sync::{Mutex, Notify};
    use tokio::time::Duration;

    #[cfg(feature = "comms")]
    use crate::CommsRuntime;
    use crate::{PersistenceBundle, SessionStore, SessionStoreError};
    #[cfg(feature = "openai")]
    use meerkat_runtime::{RealtimeAttachmentStatus, SessionServiceRuntimeExt};

    fn make_request(build: SessionBuildOptions) -> CreateSessionRequest {
        CreateSessionRequest {
            model: "gpt-5.2".to_string(),
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
            .control(RunControlCommand::StopRuntimeExecutor {
                reason: "test stop".to_string(),
            })
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
    async fn persistent_runtime_executor_cancel_surfaces_no_active_run() {
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
            .control(RunControlCommand::CancelCurrentRun {
                reason: "test cancel".to_string(),
            })
            .await
            .expect_err("cancel without an active run must surface the interrupt error");

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
            .control(RunControlCommand::CancelAfterBoundary {
                reason: "test boundary cancel".to_string(),
            })
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

    #[cfg(feature = "openai")]
    struct FakeOpenAiLiveSession {
        events: VecDeque<OpenAiLiveServerEvent>,
        close_gate: Option<Arc<Notify>>,
        sent_events: Arc<Mutex<Vec<OpenAiLiveClientEvent>>>,
    }

    #[cfg(feature = "openai")]
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

    #[cfg(feature = "openai")]
    struct FakeOpenAiLiveFactory {
        sessions: Mutex<VecDeque<Result<Box<dyn OpenAiLiveSession>, LlmError>>>,
    }

    #[cfg(feature = "openai")]
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

    #[cfg(feature = "openai")]
    struct RuntimeBackedRealtimeAttachmentToolDispatchHost {
        service: Arc<PersistentSessionService<FactoryAgentBuilder>>,
    }

    #[cfg(feature = "openai")]
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

    #[cfg(feature = "openai")]
    #[tokio::test]
    async fn runtime_backed_openai_live_orchestrator_routes_tool_calls_through_service_host() {
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
            .attach(&session_id, &target)
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
