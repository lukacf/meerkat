use std::sync::Arc;

use meerkat_core::lifecycle::core_executor::{CoreApplyOutput, CoreExecutor, CoreExecutorError};
use meerkat_core::lifecycle::run_control::RunControlCommand;
use meerkat_core::lifecycle::run_primitive::RunPrimitive;
use meerkat_core::service::SessionService;
use meerkat_runtime::meerkat_machine::RuntimeBindingsError;
use meerkat_runtime::{MeerkatMachine, RuntimeDriverError};

#[cfg(all(test, feature = "jsonl-store", not(target_arch = "wasm32")))]
use crate::JsonlStore;
use crate::{
    CreateSessionRequest, FactoryAgentBuilder, PersistentSessionService, RunResult, Session,
    SessionError, SessionId, StartTurnRequest,
};
#[cfg(all(test, feature = "jsonl-store", not(target_arch = "wasm32")))]
use meerkat_store::MemoryBlobStore;
#[cfg(all(test, feature = "jsonl-store"))]
use meerkat_store::StoreAdapter;

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

pub fn wire_runtime_bindings(
    service: &mut PersistentSessionService<FactoryAgentBuilder>,
    adapter: &Arc<MeerkatMachine>,
) {
    let adapter = Arc::clone(adapter);
    service.set_runtime_bindings_provider(Arc::new(move |session_id| {
        let adapter = Arc::clone(&adapter);
        Box::pin(async move { adapter.prepare_bindings(session_id).await.ok() })
    }));
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

#[async_trait::async_trait]
impl CoreExecutor for PersistentRuntimeExecutor {
    async fn apply(
        &mut self,
        run_id: meerkat_core::lifecycle::RunId,
        primitive: RunPrimitive,
    ) -> Result<CoreApplyOutput, CoreExecutorError> {
        let prompt = primitive.extract_content_input();
        let req = StartTurnRequest {
            prompt,
            system_prompt: None,
            render_metadata: None,
            handling_mode: meerkat_core::types::HandlingMode::Queue,
            event_tx: None,
            skill_references: primitive
                .turn_metadata()
                .and_then(|meta| meta.skill_references.clone()),
            flow_tool_overlay: primitive
                .turn_metadata()
                .and_then(|meta| meta.flow_tool_overlay.clone()),
            additional_instructions: primitive
                .turn_metadata()
                .and_then(|meta| meta.additional_instructions.clone()),
            execution_kind: primitive
                .turn_metadata()
                .and_then(|meta| meta.execution_kind),
        };

        self.service
            .apply_runtime_turn(
                &self.session_id,
                run_id,
                req,
                primitive.apply_boundary(),
                primitive.contributing_input_ids().to_vec(),
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

    use std::path::PathBuf;

    use meerkat_client::TestClient;
    use meerkat_core::SessionBuildOptions;
    use meerkat_runtime::completion::CompletionOutcome;
    use meerkat_runtime::{Input, PromptInput, RuntimeState};
    use tempfile::TempDir;
    use tokio::time::Duration;

    #[cfg(feature = "comms")]
    use crate::CommsRuntime;
    use crate::{PersistenceBundle, SessionStore, SessionStoreError};

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
        let runtime_adapter = persistence.runtime_adapter();

        let mut factory = crate::AgentFactory::new(temp.path().join("sessions"));
        #[cfg(feature = "comms")]
        if let Some(shared_runtime) = shared_runtime {
            factory = factory.with_comms_runtime(shared_runtime);
        }

        let mut builder = FactoryAgentBuilder::new(factory, crate::Config::default());
        builder.default_llm_client = Some(Arc::new(TestClient::default()));
        builder.default_session_store =
            Some(Arc::new(StoreAdapter::new(persistence.session_store())));
        let (store, runtime_store, blob_store) = persistence.into_parts();
        let mut service =
            PersistentSessionService::new(builder, 4, store, runtime_store, blob_store);
        wire_runtime_bindings(&mut service, &runtime_adapter);
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

        let error = materialize_session(
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
        )
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

        let result = materialize_session(
            &service,
            &adapter,
            Session::new(),
            make_request(SessionBuildOptions::default()),
            {
                let service = Arc::clone(&service);
                let adapter = Arc::clone(&adapter);
                move |session_id| default_persistent_executor(service, adapter, session_id)
            },
        )
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
    async fn wire_runtime_bindings_provider_prepares_runtime_bindings() {
        let temp = tempfile::tempdir().expect("tempdir");
        let persistence = build_default_persistence(temp.path().join("sessions"))
            .await
            .expect("build default persistence");
        let adapter = persistence.runtime_adapter();
        let mut builder = FactoryAgentBuilder::new(
            crate::AgentFactory::new(temp.path().join("sessions")),
            crate::Config::default(),
        );
        builder.default_llm_client = Some(Arc::new(TestClient::default()));
        builder.default_session_store =
            Some(Arc::new(StoreAdapter::new(persistence.session_store())));
        let (store, runtime_store, blob_store) = persistence.into_parts();
        let mut service =
            PersistentSessionService::new(builder, 4, store, runtime_store, blob_store);

        wire_runtime_bindings(&mut service, &adapter);

        let session_id = service
            .create_session(make_request(SessionBuildOptions::default()))
            .await
            .expect("create seed session")
            .session_id;
        service
            .discard_live_session(&session_id)
            .await
            .expect("discard live session");

        let outcome = service
            .apply_runtime_context_appends(
                &session_id,
                meerkat_core::lifecycle::RunId::new(),
                vec![meerkat_core::PendingSystemContextAppend {
                    text: "runtime bindings append".to_string(),
                    source: Some("test".to_string()),
                    idempotency_key: Some("append-1".to_string()),
                    accepted_at: meerkat_core::time_compat::SystemTime::now(),
                }],
                Vec::new(),
            )
            .await
            .expect("runtime context append should rehydrate session");
        assert_eq!(outcome.receipt.run_id.to_string().len(), 36);
    }

    #[tokio::test]
    async fn persistent_runtime_executor_stop_discards_and_unregisters() {
        let temp = tempfile::tempdir().expect("tempdir");
        let (service, adapter) = build_test_service(&temp).await;
        let result = materialize_session(
            &service,
            &adapter,
            Session::new(),
            make_request(SessionBuildOptions::default()),
            {
                let service = Arc::clone(&service);
                let adapter = Arc::clone(&adapter);
                move |session_id| default_persistent_executor(service, adapter, session_id)
            },
        )
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
        let result = materialize_session(
            &service,
            &adapter,
            Session::new(),
            make_request(SessionBuildOptions::default()),
            {
                let service = Arc::clone(&service);
                let adapter = Arc::clone(&adapter);
                move |session_id| default_persistent_executor(service, adapter, session_id)
            },
        )
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
        let result = materialize_session(
            &service,
            &adapter,
            Session::new(),
            make_request(SessionBuildOptions::default()),
            {
                let service = Arc::clone(&service);
                let adapter = Arc::clone(&adapter);
                move |session_id| default_persistent_executor(service, adapter, session_id)
            },
        )
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
        let result = materialize_session(
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
        )
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
        let result = materialize_session(
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
        )
        .await
        .expect("materialize session");

        configure_peer_ingress(&adapter, &service, &result.session_id, false).await;
        let persisted = service
            .load_persisted(&result.session_id)
            .await
            .expect("load persisted session")
            .expect("persisted session");
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
}
