//! Shared runtime-backed session host.
//!
//! Owns the common assembly used by runtime-backed surfaces:
//! persistent session service, runtime bindings provider, executor attachment,
//! persisted-session access, and transport-neutral peer-input admission.

use std::path::PathBuf;
use std::sync::{Arc, RwLock as StdRwLock};

use meerkat::{
    AgentFactory, Config, CreateSessionRequest, FactoryAgentBuilder, LlmClient, PersistenceBundle,
    PersistentSessionService, RunResult, Session, SessionError, SessionFilter, SessionId,
    SessionService, SessionStore, SessionStoreError,
    surface::{
        SurfaceSessionRecoveryContext, SurfaceSessionRecoveryOverrides, build_recovered_session,
    },
};
use meerkat_core::BlobStore;
#[cfg(feature = "comms")]
use meerkat_core::agent::CommsRuntime as AgentCommsRuntime;
use meerkat_core::comms::{EventStream, StreamError};
use meerkat_core::error::AgentError;
#[cfg(feature = "comms")]
use meerkat_core::interaction::PeerInputCandidate;
use meerkat_core::service::MobToolsFactory;
#[cfg(feature = "comms")]
use meerkat_runtime::SessionServiceRuntimeExt;
#[cfg(feature = "comms")]
use meerkat_runtime::accept::AcceptOutcome;
#[cfg(feature = "comms")]
use meerkat_runtime::comms_bridge::peer_input_candidate_to_runtime_input;
#[cfg(feature = "comms")]
use meerkat_runtime::identifiers::LogicalRuntimeId;
use meerkat_runtime::session_adapter::RuntimeBindingsError;
use meerkat_runtime::{RuntimeDriverError, RuntimeSessionAdapter};
use meerkat_store::{MemoryBlobStore, StoreAdapter};

#[cfg(feature = "comms")]
use meerkat::CommsRuntime;
#[cfg(feature = "mob")]
use meerkat_mob_mcp::{AgentMobToolSurfaceFactory, MobMcpState};
#[cfg(not(target_arch = "wasm32"))]
use meerkat_store::JsonlStore;

use meerkat_core::lifecycle::core_executor::CoreExecutor;

const DEFAULT_SESSION_COMMS_NAME_PREFIX: &str = "runtime";

#[derive(Debug, thiserror::Error)]
pub enum RuntimeSessionHostError {
    #[error("runtime host requires explicit persistence when no session directory is configured")]
    MissingPersistence,
    #[error(transparent)]
    Session(#[from] SessionError),
    #[error(transparent)]
    Store(#[from] SessionStoreError),
    #[error(transparent)]
    RuntimeBindings(#[from] RuntimeBindingsError),
    #[error(transparent)]
    RuntimeDriver(#[from] RuntimeDriverError),
    #[error("persisted session not found: {0}")]
    PersistedSessionNotFound(SessionId),
    #[error("failed to synchronize session comms peers: {0}")]
    PeerSync(String),
    #[error("session service returned mismatched session id: expected {expected}, got {actual}")]
    SessionIdMismatch {
        expected: SessionId,
        actual: SessionId,
    },
}

pub struct RuntimeSessionHostBuilder {
    session_dir: Option<PathBuf>,
    factory: AgentFactory,
    config: Config,
    max_sessions: usize,
    persistence: Option<PersistenceBundle>,
    default_llm_client: Option<Arc<dyn LlmClient>>,
    session_comms_name_prefix: String,
    #[cfg(feature = "comms")]
    shared_comms_runtime: Option<Arc<CommsRuntime>>,
}

impl RuntimeSessionHostBuilder {
    pub fn new(session_dir: impl Into<PathBuf>) -> Self {
        let session_dir = session_dir.into();
        Self {
            factory: AgentFactory::new(session_dir.clone()),
            session_dir: Some(session_dir),
            config: Config::default(),
            max_sessions: 64,
            persistence: None,
            default_llm_client: None,
            session_comms_name_prefix: DEFAULT_SESSION_COMMS_NAME_PREFIX.to_string(),
            #[cfg(feature = "comms")]
            shared_comms_runtime: None,
        }
    }

    pub fn from_factory(factory: AgentFactory) -> Self {
        Self {
            session_dir: None,
            factory,
            config: Config::default(),
            max_sessions: 64,
            persistence: None,
            default_llm_client: None,
            session_comms_name_prefix: DEFAULT_SESSION_COMMS_NAME_PREFIX.to_string(),
            #[cfg(feature = "comms")]
            shared_comms_runtime: None,
        }
    }

    pub fn config(mut self, config: Config) -> Self {
        self.config = config;
        self
    }

    pub fn max_sessions(mut self, max_sessions: usize) -> Self {
        self.max_sessions = max_sessions;
        self
    }

    pub fn persistence(mut self, persistence: PersistenceBundle) -> Self {
        self.persistence = Some(persistence);
        self
    }

    pub fn default_llm_client(mut self, llm_client: Arc<dyn LlmClient>) -> Self {
        self.default_llm_client = Some(llm_client);
        self
    }

    pub fn session_comms_name_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.session_comms_name_prefix = prefix.into();
        self
    }

    pub fn project_root(mut self, path: impl Into<PathBuf>) -> Self {
        self.factory = self.factory.project_root(path);
        self
    }

    pub fn context_root(mut self, path: impl Into<PathBuf>) -> Self {
        self.factory = self.factory.context_root(path);
        self
    }

    pub fn user_config_root(mut self, path: impl Into<PathBuf>) -> Self {
        self.factory = self.factory.user_config_root(path);
        self
    }

    pub fn runtime_root(mut self, path: impl Into<PathBuf>) -> Self {
        self.factory = self.factory.runtime_root(path);
        self
    }

    pub fn builtins(mut self, enabled: bool) -> Self {
        self.factory = self.factory.builtins(enabled);
        self
    }

    pub fn shell(mut self, enabled: bool) -> Self {
        self.factory = self.factory.shell(enabled);
        self
    }

    pub fn memory(mut self, enabled: bool) -> Self {
        self.factory = self.factory.memory(enabled);
        self
    }

    pub fn mob(mut self, enabled: bool) -> Self {
        self.factory = self.factory.mob(enabled);
        self
    }

    #[cfg(feature = "comms")]
    pub fn comms_runtime(mut self, runtime: Arc<CommsRuntime>) -> Self {
        self.factory = self.factory.with_comms_runtime(Arc::clone(&runtime));
        self.shared_comms_runtime = Some(runtime);
        self
    }

    pub async fn build(self) -> Result<RuntimeSessionHost, RuntimeSessionHostError> {
        let persistence = match self.persistence {
            Some(persistence) => persistence,
            None => {
                let session_dir = self
                    .session_dir
                    .clone()
                    .ok_or(RuntimeSessionHostError::MissingPersistence)?;
                build_default_persistence(session_dir).await?
            }
        };

        let mut builder = FactoryAgentBuilder::new(
            self.factory.session_store(persistence.session_store()),
            self.config,
        );
        builder.default_llm_client = self.default_llm_client;

        let mut host = RuntimeSessionHost::from_builder(builder, self.max_sessions, persistence);
        host.set_session_comms_name_prefix(self.session_comms_name_prefix);
        #[cfg(feature = "comms")]
        {
            host.shared_comms_runtime = self.shared_comms_runtime;
        }
        Ok(host)
    }
}

pub struct RuntimeSessionHost {
    service: Arc<PersistentSessionService<FactoryAgentBuilder>>,
    persistence: PersistenceBundle,
    runtime_adapter: Arc<RuntimeSessionAdapter>,
    builder_mob_tools_slot: Arc<StdRwLock<Option<Arc<dyn MobToolsFactory>>>>,
    session_comms_name_prefix: String,
    #[cfg(feature = "comms")]
    shared_comms_runtime: Option<Arc<CommsRuntime>>,
    #[cfg(feature = "mob")]
    mob_state: StdRwLock<Arc<MobMcpState>>,
}

impl RuntimeSessionHost {
    pub fn builder(session_dir: impl Into<PathBuf>) -> RuntimeSessionHostBuilder {
        RuntimeSessionHostBuilder::new(session_dir)
    }

    pub fn from_factory(
        factory: AgentFactory,
        config: Config,
        max_sessions: usize,
        persistence: PersistenceBundle,
    ) -> Self {
        let builder =
            FactoryAgentBuilder::new(factory.session_store(persistence.session_store()), config);
        Self::from_builder(builder, max_sessions, persistence)
    }

    pub fn from_builder(
        mut builder: FactoryAgentBuilder,
        max_sessions: usize,
        persistence: PersistenceBundle,
    ) -> Self {
        let runtime_adapter = persistence.runtime_adapter();
        if builder.default_session_store.is_none() {
            builder.default_session_store =
                Some(Arc::new(StoreAdapter::new(persistence.session_store())));
        }
        let builder_mob_tools_slot = Arc::clone(&builder.default_mob_tools);
        let service = Arc::new(build_persistent_service(
            builder,
            max_sessions,
            persistence.clone(),
            &runtime_adapter,
        ));

        #[cfg(feature = "mob")]
        let mob_state = {
            let mob_state = Arc::new(MobMcpState::new_with_runtime_adapter(
                service.clone(),
                Some(runtime_adapter.clone()),
            ));
            *builder_mob_tools_slot
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::new(
                AgentMobToolSurfaceFactory::new(Arc::clone(&mob_state)),
            ));
            mob_state
        };

        Self {
            service,
            persistence,
            runtime_adapter,
            builder_mob_tools_slot,
            session_comms_name_prefix: DEFAULT_SESSION_COMMS_NAME_PREFIX.to_string(),
            #[cfg(feature = "comms")]
            shared_comms_runtime: None,
            #[cfg(feature = "mob")]
            mob_state: StdRwLock::new(mob_state),
        }
    }

    pub fn service(&self) -> Arc<PersistentSessionService<FactoryAgentBuilder>> {
        self.service.clone()
    }

    pub fn persistence(&self) -> PersistenceBundle {
        self.persistence.clone()
    }

    pub fn session_store(&self) -> Arc<dyn SessionStore> {
        self.persistence.session_store()
    }

    pub fn blob_store(&self) -> Arc<dyn BlobStore> {
        self.persistence.blob_store()
    }

    pub fn runtime_adapter(&self) -> Arc<RuntimeSessionAdapter> {
        self.runtime_adapter.clone()
    }

    pub fn builder_mob_tools_slot(&self) -> Arc<StdRwLock<Option<Arc<dyn MobToolsFactory>>>> {
        Arc::clone(&self.builder_mob_tools_slot)
    }

    pub fn session_comms_name_prefix(&self) -> &str {
        &self.session_comms_name_prefix
    }

    pub fn set_session_comms_name_prefix(&mut self, prefix: impl Into<String>) {
        self.session_comms_name_prefix = prefix.into();
    }

    #[cfg(feature = "mob")]
    pub fn mob_state(&self) -> Arc<MobMcpState> {
        self.mob_state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    #[cfg(feature = "mob")]
    pub fn set_mob_state(&self, mob_state: Arc<MobMcpState>) {
        *self
            .mob_state
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Arc::clone(&mob_state);
        *self
            .builder_mob_tools_slot
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::new(
            AgentMobToolSurfaceFactory::new(Arc::clone(&mob_state)),
        ));
    }

    pub fn set_mob_tools(&self, factory: Arc<dyn MobToolsFactory>) {
        *self
            .builder_mob_tools_slot
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(factory);
    }

    pub async fn list_persisted_sessions(
        &self,
        filter: SessionFilter,
    ) -> Result<Vec<meerkat_core::SessionMeta>, RuntimeSessionHostError> {
        Ok(self.session_store().list(filter).await?)
    }

    pub async fn load_persisted(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<Session>, RuntimeSessionHostError> {
        Ok(self.service.load_persisted(session_id).await?)
    }

    pub async fn materialize_persisted_session(
        &self,
        session_id: &SessionId,
        overrides: &SurfaceSessionRecoveryOverrides,
        mut recovery_context: SurfaceSessionRecoveryContext,
    ) -> Result<SessionId, RuntimeSessionHostError> {
        let session = self
            .load_persisted(session_id)
            .await?
            .ok_or_else(|| RuntimeSessionHostError::PersistedSessionNotFound(session_id.clone()))?;
        recovery_context.runtime_build_mode = None;
        let recovered = build_recovered_session(session.clone(), overrides, recovery_context)
            .map_err(|error| SessionError::Agent(AgentError::InternalError(error.to_string())))?;
        #[cfg(feature = "comms")]
        let keep_alive = recovered.keep_alive;
        let materialized_id = self
            .materialize_session(session, recovered.into_deferred_create_request())
            .await?;
        #[cfg(feature = "comms")]
        {
            self.configure_peer_ingress(&materialized_id, keep_alive)
                .await;
        }
        Ok(materialized_id)
    }

    pub async fn subscribe_events(
        &self,
        session_id: &SessionId,
    ) -> Result<EventStream, StreamError> {
        self.service.subscribe_session_events(session_id).await
    }

    pub async fn discard_live_session(
        &self,
        session_id: &SessionId,
    ) -> Result<(), RuntimeSessionHostError> {
        Ok(self.service.discard_live_session(session_id).await?)
    }

    #[cfg(feature = "mob")]
    pub async fn discard_live_session_with_mob_cleanup(
        &self,
        session_id: &SessionId,
    ) -> Result<(), RuntimeSessionHostError> {
        let discard_result = self.service.discard_live_session(session_id).await;
        if let Err(error) = self
            .mob_state()
            .destroy_session_mobs(&session_id.to_string())
            .await
        {
            tracing::warn!(
                session_id = %session_id,
                error = %error,
                "failed to clean up session mobs after live-session discard"
            );
        }
        Ok(discard_result?)
    }

    pub async fn create_or_resume_session<F>(
        &self,
        resume_id: Option<SessionId>,
        request: CreateSessionRequest,
        executor_factory: F,
    ) -> Result<SessionId, RuntimeSessionHostError>
    where
        F: FnOnce(SessionId) -> Box<dyn CoreExecutor>,
    {
        let result = self
            .create_or_resume_session_with_result(resume_id, request, executor_factory)
            .await?;
        Ok(result.session_id)
    }

    pub async fn create_or_resume_session_with_result<F>(
        &self,
        resume_id: Option<SessionId>,
        request: CreateSessionRequest,
        executor_factory: F,
    ) -> Result<RunResult, RuntimeSessionHostError>
    where
        F: FnOnce(SessionId) -> Box<dyn CoreExecutor>,
    {
        let session = match resume_id {
            Some(session_id) => self.load_persisted(&session_id).await?.ok_or(
                RuntimeSessionHostError::PersistedSessionNotFound(session_id),
            )?,
            None => Session::new(),
        };
        self.materialize_session_with_result_and_executor(session, request, executor_factory)
            .await
    }

    pub async fn materialize_session(
        &self,
        session: Session,
        request: CreateSessionRequest,
    ) -> Result<SessionId, RuntimeSessionHostError> {
        let result = self
            .materialize_session_with_result(session, request)
            .await?;
        Ok(result.session_id)
    }

    pub async fn materialize_session_with_result(
        &self,
        session: Session,
        request: CreateSessionRequest,
    ) -> Result<RunResult, RuntimeSessionHostError> {
        self.materialize_session_with_result_inner::<fn(SessionId) -> Box<dyn CoreExecutor>>(
            session, request, None,
        )
        .await
    }

    pub async fn materialize_session_and_attach_executor<F>(
        &self,
        session: Session,
        request: CreateSessionRequest,
        executor_factory: F,
    ) -> Result<SessionId, RuntimeSessionHostError>
    where
        F: FnOnce(SessionId) -> Box<dyn CoreExecutor>,
    {
        let result = self
            .materialize_session_with_result_and_executor(session, request, executor_factory)
            .await?;
        Ok(result.session_id)
    }

    pub async fn materialize_session_with_result_and_executor<F>(
        &self,
        session: Session,
        request: CreateSessionRequest,
        executor_factory: F,
    ) -> Result<RunResult, RuntimeSessionHostError>
    where
        F: FnOnce(SessionId) -> Box<dyn CoreExecutor>,
    {
        self.materialize_session_with_result_inner(session, request, Some(executor_factory))
            .await
    }

    async fn materialize_session_with_result_inner<F>(
        &self,
        session: Session,
        mut request: CreateSessionRequest,
        executor_factory: Option<F>,
    ) -> Result<RunResult, RuntimeSessionHostError>
    where
        F: FnOnce(SessionId) -> Box<dyn CoreExecutor>,
    {
        let prepared_session_id = session.id().clone();
        let stored_comms_name = session
            .session_metadata()
            .and_then(|metadata| metadata.comms_name.clone());
        let bindings = self
            .runtime_adapter
            .prepare_bindings(prepared_session_id.clone())
            .await?;

        let mut build = request.build.unwrap_or_default();
        build.resume_session = Some(session);
        if build.keep_alive && build.comms_name.is_none() {
            build.comms_name = stored_comms_name.or_else(|| {
                Some(format!(
                    "{}/{}",
                    self.session_comms_name_prefix, prepared_session_id
                ))
            });
        }
        build.runtime_build_mode = meerkat_core::RuntimeBuildMode::SessionOwned(bindings);
        request.build = Some(build);

        let result = match self.service.create_session(request).await {
            Ok(result) => result,
            Err(error) => {
                self.runtime_adapter
                    .unregister_session(&prepared_session_id)
                    .await;
                return Err(RuntimeSessionHostError::Session(error));
            }
        };

        if result.session_id != prepared_session_id {
            self.runtime_adapter
                .unregister_session(&prepared_session_id)
                .await;
            return Err(RuntimeSessionHostError::SessionIdMismatch {
                expected: prepared_session_id,
                actual: result.session_id,
            });
        }

        #[cfg(feature = "comms")]
        if let Err(error) = self.sync_session_trusted_peers(&result.session_id).await {
            let _ = self.service.discard_live_session(&result.session_id).await;
            self.runtime_adapter
                .unregister_session(&result.session_id)
                .await;
            return Err(error);
        }

        if let Some(executor_factory) = executor_factory {
            self.attach_executor(
                &result.session_id,
                executor_factory(result.session_id.clone()),
            )
            .await;
        }
        Ok(result)
    }

    pub async fn attach_executor(&self, session_id: &SessionId, executor: Box<dyn CoreExecutor>) {
        self.runtime_adapter
            .ensure_session_with_executor(session_id.clone(), executor)
            .await;
    }

    pub async fn configure_peer_ingress(&self, session_id: &SessionId, keep_alive: bool) -> bool {
        let comms_rt = self.service.comms_runtime(session_id).await;
        self.runtime_adapter
            .update_peer_ingress_context(session_id, keep_alive, comms_rt)
            .await
    }

    #[cfg(feature = "comms")]
    pub async fn inject_peer_input_candidate(
        &self,
        session_id: &SessionId,
        candidate: &PeerInputCandidate,
    ) -> Result<AcceptOutcome, RuntimeSessionHostError> {
        let runtime_id = LogicalRuntimeId::new(session_id.to_string());
        let input = peer_input_candidate_to_runtime_input(candidate, &runtime_id);
        Ok(self.runtime_adapter.accept_input(session_id, input).await?)
    }

    #[cfg(feature = "comms")]
    async fn sync_session_trusted_peers(
        &self,
        session_id: &SessionId,
    ) -> Result<(), RuntimeSessionHostError> {
        let Some(shared_runtime) = &self.shared_comms_runtime else {
            return Ok(());
        };
        let Some(session_runtime) = self.service.comms_runtime(session_id).await else {
            return Ok(());
        };

        for peer in shared_runtime.peers().await {
            let spec =
                meerkat_core::comms::TrustedPeerSpec::new(peer.name, peer.peer_id, peer.address)
                    .map_err(RuntimeSessionHostError::PeerSync)?;
            session_runtime
                .add_trusted_peer(spec)
                .await
                .map_err(|error| RuntimeSessionHostError::PeerSync(error.to_string()))?;
        }

        Ok(())
    }
}

fn build_persistent_service(
    builder: FactoryAgentBuilder,
    max_sessions: usize,
    persistence: PersistenceBundle,
    runtime_adapter: &Arc<RuntimeSessionAdapter>,
) -> PersistentSessionService<FactoryAgentBuilder> {
    let (store, runtime_store, blob_store) = persistence.into_parts();
    let mut service =
        PersistentSessionService::new(builder, max_sessions, store, runtime_store, blob_store);
    let adapter = runtime_adapter.clone();
    service.set_runtime_bindings_provider(Arc::new(move |session_id| {
        let adapter = adapter.clone();
        Box::pin(async move { adapter.prepare_bindings(session_id).await.ok() })
    }));
    service
}

#[cfg(not(target_arch = "wasm32"))]
async fn build_default_persistence(
    session_dir: PathBuf,
) -> Result<PersistenceBundle, RuntimeSessionHostError> {
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

#[cfg(target_arch = "wasm32")]
async fn build_default_persistence(
    _session_dir: PathBuf,
) -> Result<PersistenceBundle, RuntimeSessionHostError> {
    Err(RuntimeSessionHostError::MissingPersistence)
}
