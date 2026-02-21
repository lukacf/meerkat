//! SessionRuntime - keeps agents alive between turns.
//!
//! Delegates to [`PersistentSessionService`] for session lifecycle management.
//! `FactoryAgentBuilder` bridges `AgentFactory::build_agent()` into the
//! `SessionAgentBuilder` / `SessionAgent` traits used by the service.
//!
//! The runtime preserves the two-step create-then-run API required by the
//! JSON-RPC handlers: `create_session()` stages a build config and returns a
//! `SessionId`; the first `start_turn()` call for that ID materializes the
//! session inside the service (which runs the first turn).

use std::sync::{Arc, RwLock as StdRwLock};

use indexmap::IndexMap;
use meerkat::{
    AgentBuildConfig, AgentFactory, FactoryAgentBuilder, PersistentSessionService, SessionStore,
};
use meerkat_client::LlmClient;
use meerkat_core::event::AgentEvent;
use meerkat_core::service::{CreateSessionRequest, SessionError, SessionService, StartTurnRequest};
use meerkat_core::skills::{SkillError, SourceIdentityRegistry};
use meerkat_core::types::{RunResult, SessionId};
use meerkat_core::{Config, ConfigStore, Session};
use tokio::sync::{RwLock, mpsc};

use crate::error;
use crate::protocol::RpcError;

#[derive(Clone)]
struct SkillIdentityRegistryState {
    generation: u64,
    registry: SourceIdentityRegistry,
}

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Observable state of a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    /// The session is idle and ready to accept a new turn.
    Idle,
    /// A turn is currently running.
    Running,
    /// The session is shutting down.
    ShuttingDown,
}

impl SessionState {
    /// Return a stable string representation matching the serde `rename_all` convention.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Running => "running",
            Self::ShuttingDown => "shutting_down",
        }
    }
}

/// Summary information about a session.
#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub session_id: SessionId,
    pub state: SessionState,
}

// FactoryAgent and FactoryAgentBuilder are imported from meerkat::service_factory.

// ---------------------------------------------------------------------------
// SessionRuntime
// ---------------------------------------------------------------------------

/// Core runtime that manages agent sessions.
///
/// Wraps [`PersistentSessionService`] for session lifecycle management while
/// preserving the two-step create-then-run API required by JSON-RPC handlers.
pub struct SessionRuntime {
    service: PersistentSessionService<FactoryAgentBuilder>,
    /// Sessions that have been "created" (ID returned to caller) but not yet
    /// materialized in the service. The first `start_turn` call promotes them.
    pending: RwLock<IndexMap<SessionId, AgentBuildConfig>>,
    max_sessions: usize,
    /// Override LLM client for all sessions (primarily for testing).
    pub default_llm_client: Option<Arc<dyn LlmClient>>,
    realm_id: Option<String>,
    instance_id: Option<String>,
    backend: Option<String>,
    config_runtime: Option<Arc<meerkat_core::ConfigRuntime>>,
    skill_identity_registry: Arc<StdRwLock<SkillIdentityRegistryState>>,
}

impl SessionRuntime {
    /// Create a new session runtime.
    pub fn new(
        factory: AgentFactory,
        config: Config,
        max_sessions: usize,
        store: Arc<dyn SessionStore>,
    ) -> Self {
        let builder = FactoryAgentBuilder::new(factory, config);
        let service = PersistentSessionService::new(builder, max_sessions, store);

        Self {
            service,
            pending: RwLock::new(IndexMap::new()),
            max_sessions,
            default_llm_client: None,
            realm_id: None,
            instance_id: None,
            backend: None,
            config_runtime: None,
            skill_identity_registry: Arc::new(StdRwLock::new(SkillIdentityRegistryState {
                generation: 0,
                registry: SourceIdentityRegistry::default(),
            })),
        }
    }

    /// Create a runtime that resolves config from a shared config store.
    pub fn new_with_config_store(
        factory: AgentFactory,
        initial_config: Config,
        config_store: Arc<dyn ConfigStore>,
        max_sessions: usize,
        store: Arc<dyn SessionStore>,
    ) -> Self {
        let builder =
            FactoryAgentBuilder::new_with_config_store(factory, initial_config, config_store);
        let service = PersistentSessionService::new(builder, max_sessions, store);

        Self {
            service,
            pending: RwLock::new(IndexMap::new()),
            max_sessions,
            default_llm_client: None,
            realm_id: None,
            instance_id: None,
            backend: None,
            config_runtime: None,
            skill_identity_registry: Arc::new(StdRwLock::new(SkillIdentityRegistryState {
                generation: 0,
                registry: SourceIdentityRegistry::default(),
            })),
        }
    }

    /// Attach realm context defaults used for session metadata.
    pub fn set_realm_context(
        &mut self,
        realm_id: Option<String>,
        instance_id: Option<String>,
        backend: Option<String>,
    ) {
        self.realm_id = realm_id;
        self.instance_id = instance_id;
        self.backend = backend;
    }

    /// Active realm id for this runtime, if configured.
    pub fn realm_id(&self) -> Option<&str> {
        self.realm_id.as_deref()
    }

    /// Attach config runtime for generation stamping.
    pub fn set_config_runtime(&mut self, runtime: Arc<meerkat_core::ConfigRuntime>) {
        self.config_runtime = Some(runtime);
    }

    /// Shared config runtime used by config handlers.
    pub fn config_runtime(&self) -> Option<Arc<meerkat_core::ConfigRuntime>> {
        self.config_runtime.as_ref().map(Arc::clone)
    }

    pub fn set_skill_identity_registry(&self, registry: SourceIdentityRegistry) {
        if let Ok(mut slot) = self.skill_identity_registry.write() {
            slot.registry = registry;
        }
    }

    pub fn set_skill_identity_registry_for_generation(
        &self,
        generation: u64,
        registry: SourceIdentityRegistry,
    ) {
        if let Ok(mut slot) = self.skill_identity_registry.write()
            && generation >= slot.generation
        {
            slot.generation = generation;
            slot.registry = registry;
        }
    }

    pub fn skill_identity_registry(&self) -> SourceIdentityRegistry {
        self.skill_identity_registry
            .read()
            .map(|state| state.registry.clone())
            .unwrap_or_default()
    }

    /// Build a source identity registry from runtime config.
    pub fn build_skill_identity_registry(
        config: &Config,
    ) -> Result<SourceIdentityRegistry, SkillError> {
        config.skills.build_source_identity_registry()
    }

    /// Create a new session with the given build configuration.
    ///
    /// Returns the session ID on success. The session is staged as "pending"
    /// and will be materialized inside the service on the first `start_turn`.
    pub async fn create_session(
        &self,
        build_config: AgentBuildConfig,
    ) -> Result<SessionId, RpcError> {
        // Check combined capacity (pending + active).
        {
            let pending = self.pending.read().await;
            let active = self
                .service
                .list(Default::default())
                .await
                .map_err(session_error_to_rpc)?
                .len();
            let total = pending.len() + active;
            if total >= self.max_sessions {
                return Err(RpcError {
                    code: error::INTERNAL_ERROR,
                    message: format!("Max sessions reached ({}/{})", total, self.max_sessions),
                    data: None,
                });
            }
        }

        // Pre-create a session to claim a stable SessionId.
        let session = Session::new();
        let session_id = session.id().clone();

        // Inject the pre-created session so the agent builder will reuse this ID.
        let build_config = AgentBuildConfig {
            resume_session: Some(session),
            ..build_config
        };

        {
            let mut pending = self.pending.write().await;
            pending.insert(session_id.clone(), build_config);
        }

        Ok(session_id)
    }

    /// Start a turn on the given session.
    ///
    /// Events are forwarded to `event_tx` during the turn. Returns the
    /// `RunResult` when the turn completes.
    pub async fn start_turn(
        &self,
        session_id: &SessionId,
        prompt: String,
        event_tx: mpsc::Sender<AgentEvent>,
        skill_references: Option<Vec<meerkat_core::skills::SkillKey>>,
    ) -> Result<RunResult, RpcError> {
        // Check if this is a pending (not-yet-materialized) session.
        let pending_config = {
            let mut pending = self.pending.write().await;
            pending.swap_remove(session_id)
        };

        if let Some(mut build_config) = pending_config {
            // Inject default LLM client if the caller didn't provide one.
            if build_config.llm_client_override.is_none()
                && let Some(ref client) = self.default_llm_client
            {
                build_config.llm_client_override = Some(client.clone());
            }
            let runtime_generation = if build_config.config_generation.is_none() {
                if let Some(runtime) = &self.config_runtime {
                    runtime.get().await.ok().map(|snapshot| snapshot.generation)
                } else {
                    None
                }
            } else {
                None
            };

            let mut build = build_config.to_session_build_options();
            build.realm_id = build.realm_id.or_else(|| self.realm_id.clone());
            build.instance_id = build.instance_id.or_else(|| self.instance_id.clone());
            build.backend = build.backend.or_else(|| self.backend.clone());
            build.config_generation = build.config_generation.or(runtime_generation);

            let req = CreateSessionRequest {
                model: build_config.model,
                prompt,
                system_prompt: build_config.system_prompt,
                max_tokens: build_config.max_tokens,
                event_tx: Some(event_tx),
                host_mode: build_config.host_mode,
                skill_references: skill_references.clone(),
                initial_turn: meerkat_core::service::InitialTurnPolicy::RunImmediately,
                build: Some(build),
            };

            let result = self
                .service
                .create_session(req)
                .await
                .map_err(session_error_to_rpc)?;

            return Ok(result);
        }

        // Normal turn on an existing session.
        let req = StartTurnRequest {
            prompt,
            event_tx: Some(event_tx),
            host_mode: false,
            skill_references,
        };

        self.service
            .start_turn(session_id, req)
            .await
            .map_err(session_error_to_rpc)
    }

    /// Interrupt a running turn on the given session.
    ///
    /// If the session is idle, this is a no-op.
    pub async fn interrupt(&self, session_id: &SessionId) -> Result<(), RpcError> {
        // Pending sessions have no running turn.
        {
            let pending = self.pending.read().await;
            if pending.contains_key(session_id) {
                return Ok(());
            }
        }

        match self.service.interrupt(session_id).await {
            Ok(()) => Ok(()),
            // The service returns NotRunning when no turn is active — map to no-op.
            Err(SessionError::NotRunning { .. }) => Ok(()),
            Err(e) => Err(session_error_to_rpc(e)),
        }
    }

    /// Get the current state of a session, or `None` if the session does not exist.
    pub async fn session_state(&self, session_id: &SessionId) -> Option<SessionState> {
        // Check pending sessions first.
        {
            let pending = self.pending.read().await;
            if pending.contains_key(session_id) {
                return Some(SessionState::Idle);
            }
        }

        // Use `list()` instead of `read()` to avoid blocking on a
        // `ReadSnapshot` command while a turn is in progress. `list()`
        // reads state from non-blocking watch receivers.
        if let Ok(summaries) = self.service.list(Default::default()).await {
            for summary in &summaries {
                if summary.session_id == *session_id {
                    return Some(if summary.is_active {
                        SessionState::Running
                    } else {
                        SessionState::Idle
                    });
                }
            }
        }

        None
    }

    /// Archive (remove) a session.
    pub async fn archive_session(&self, session_id: &SessionId) -> Result<(), RpcError> {
        // Check pending sessions first.
        {
            let mut pending = self.pending.write().await;
            if pending.swap_remove(session_id).is_some() {
                return Ok(());
            }
        }

        self.service
            .archive(session_id)
            .await
            .map_err(session_error_to_rpc)
    }

    /// List all active sessions.
    pub async fn list_sessions(&self) -> Vec<SessionInfo> {
        let mut result = Vec::new();

        // Include pending sessions as Idle.
        {
            let pending = self.pending.read().await;
            for session_id in pending.keys() {
                result.push(SessionInfo {
                    session_id: session_id.clone(),
                    state: SessionState::Idle,
                });
            }
        }

        // Include active sessions from the service.
        if let Ok(summaries) = self.service.list(Default::default()).await {
            for summary in summaries {
                let state = if summary.is_active {
                    SessionState::Running
                } else {
                    SessionState::Idle
                };
                result.push(SessionInfo {
                    session_id: summary.session_id,
                    state,
                });
            }
        }

        result
    }

    /// Get the subscribable event injector for a session, if available.
    ///
    /// Use `inject()` for fire-and-forget or `inject_with_subscription()`
    /// for interaction-scoped streaming.
    pub async fn event_injector(
        &self,
        session_id: &meerkat_core::SessionId,
    ) -> Option<std::sync::Arc<dyn meerkat_core::SubscribableInjector>> {
        self.service.event_injector(session_id).await
    }

    /// Get the comms runtime for a session, if available.
    pub async fn comms_runtime(
        &self,
        session_id: &meerkat_core::SessionId,
    ) -> Option<std::sync::Arc<dyn meerkat_core::agent::CommsRuntime>> {
        self.service.comms_runtime(session_id).await
    }

    /// Shut down the runtime, closing all sessions.
    pub async fn shutdown(&self) {
        // Clear pending sessions.
        {
            let mut pending = self.pending.write().await;
            pending.clear();
        }

        // Shut down the service.
        self.service.shutdown().await;
    }
}

// ---------------------------------------------------------------------------
// Error mapping
// ---------------------------------------------------------------------------

fn session_error_to_rpc(err: SessionError) -> RpcError {
    let code = match &err {
        SessionError::NotFound { .. } => error::SESSION_NOT_FOUND,
        SessionError::Busy { .. } => error::SESSION_BUSY,
        SessionError::NotRunning { .. } => error::INTERNAL_ERROR,
        SessionError::Agent(agent_err) => match agent_err {
            meerkat_core::AgentError::TokenBudgetExceeded { .. }
            | meerkat_core::AgentError::TimeBudgetExceeded { .. }
            | meerkat_core::AgentError::ToolCallBudgetExceeded { .. } => error::BUDGET_EXHAUSTED,
            meerkat_core::AgentError::HookDenied { .. } => error::HOOK_DENIED,
            meerkat_core::AgentError::Llm { .. } => error::PROVIDER_ERROR,
            meerkat_core::AgentError::SessionNotFound(_) => error::SESSION_NOT_FOUND,
            meerkat_core::AgentError::InternalError(msg) => {
                // Build errors (missing API key, unknown provider) are tunneled
                // through InternalError from the builder.
                if msg.contains("API key") || msg.contains("provider") {
                    error::PROVIDER_ERROR
                } else {
                    error::INTERNAL_ERROR
                }
            }
            _ => error::INTERNAL_ERROR,
        },
        _ => error::INTERNAL_ERROR,
    };
    RpcError {
        code,
        message: err.to_string(),
        data: None,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    use async_trait::async_trait;
    use futures::stream;
    use meerkat::AgentBuildConfig;
    use meerkat_client::{LlmClient, LlmError};
    use meerkat_core::StopReason;
    use meerkat_core::skills::{
        SkillKey, SkillKeyRemap, SkillName, SourceIdentityLineage, SourceIdentityLineageEvent,
        SourceUuid,
    };
    use meerkat_core::skills_config::{
        SkillRepoTransport, SkillRepositoryConfig, SkillsConfig, SkillsIdentityConfig,
    };
    use std::pin::Pin;
    use std::sync::Arc;

    // -----------------------------------------------------------------------
    // Mock LLM client
    // -----------------------------------------------------------------------

    struct MockLlmClient;

    #[async_trait]
    impl LlmClient for MockLlmClient {
        fn stream<'a>(
            &'a self,
            _request: &'a meerkat_client::LlmRequest,
        ) -> Pin<
            Box<dyn futures::Stream<Item = Result<meerkat_client::LlmEvent, LlmError>> + Send + 'a>,
        > {
            Box::pin(stream::iter(vec![
                Ok(meerkat_client::LlmEvent::TextDelta {
                    delta: "Hello from mock".to_string(),
                    meta: None,
                }),
                Ok(meerkat_client::LlmEvent::Done {
                    outcome: meerkat_client::LlmDoneOutcome::Success {
                        stop_reason: StopReason::EndTurn,
                    },
                }),
            ]))
        }

        fn provider(&self) -> &'static str {
            "mock"
        }

        async fn health_check(&self) -> Result<(), LlmError> {
            Ok(())
        }
    }

    /// A mock LLM client that introduces a delay before responding.
    /// Used to test concurrent access (SESSION_BUSY).
    struct SlowMockLlmClient {
        delay_ms: u64,
    }

    impl SlowMockLlmClient {
        fn new(delay_ms: u64) -> Self {
            Self { delay_ms }
        }
    }

    #[async_trait]
    impl LlmClient for SlowMockLlmClient {
        fn stream<'a>(
            &'a self,
            _request: &'a meerkat_client::LlmRequest,
        ) -> Pin<
            Box<dyn futures::Stream<Item = Result<meerkat_client::LlmEvent, LlmError>> + Send + 'a>,
        > {
            let delay_ms = self.delay_ms;
            Box::pin(async_stream::stream! {
                tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms)).await;
                yield Ok(meerkat_client::LlmEvent::TextDelta {
                    delta: "Slow response".to_string(),
                    meta: None,
                });
                yield Ok(meerkat_client::LlmEvent::Done {
                    outcome: meerkat_client::LlmDoneOutcome::Success {
                        stop_reason: StopReason::EndTurn,
                    },
                });
            })
        }

        fn provider(&self) -> &'static str {
            "mock"
        }

        async fn health_check(&self) -> Result<(), LlmError> {
            Ok(())
        }
    }

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    fn temp_factory(temp: &tempfile::TempDir) -> AgentFactory {
        AgentFactory::new(temp.path().join("sessions"))
    }

    fn mock_build_config() -> AgentBuildConfig {
        AgentBuildConfig {
            llm_client_override: Some(Arc::new(MockLlmClient)),
            ..AgentBuildConfig::new("claude-sonnet-4-5")
        }
    }

    fn slow_build_config(delay_ms: u64) -> AgentBuildConfig {
        AgentBuildConfig {
            llm_client_override: Some(Arc::new(SlowMockLlmClient::new(delay_ms))),
            ..AgentBuildConfig::new("claude-sonnet-4-5")
        }
    }

    fn make_runtime(factory: AgentFactory, max_sessions: usize) -> SessionRuntime {
        let store: Arc<dyn meerkat::SessionStore> = Arc::new(meerkat::MemoryStore::new());
        SessionRuntime::new(factory, Config::default(), max_sessions, store)
    }

    // -----------------------------------------------------------------------
    // Tests
    // -----------------------------------------------------------------------

    /// 1. Creating a session returns a SessionId and the state is Idle.
    #[tokio::test]
    async fn create_session_returns_id_and_idle_state() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);

        let session_id = runtime.create_session(mock_build_config()).await.unwrap();

        let state = runtime.session_state(&session_id).await;
        assert_eq!(state, Some(SessionState::Idle));
    }

    /// 2. Starting a turn returns a RunResult with expected text.
    #[tokio::test]
    async fn start_turn_returns_run_result() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);

        let session_id = runtime.create_session(mock_build_config()).await.unwrap();
        let (event_tx, _event_rx) = mpsc::channel(100);

        let result = runtime
            .start_turn(&session_id, "Hello".to_string(), event_tx, None)
            .await
            .unwrap();

        assert!(
            result.text.contains("Hello from mock"),
            "Expected mock response text, got: {}",
            result.text
        );
    }

    /// 3. Verify state transitions: Idle -> Running -> Idle during a turn.
    #[tokio::test]
    async fn start_turn_transitions_idle_running_idle() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = Arc::new(make_runtime(temp_factory(&temp), 10));

        // Use a slow mock to give us time to observe Running state
        let session_id = runtime
            .create_session(slow_build_config(100))
            .await
            .unwrap();

        // Verify initial state
        assert_eq!(
            runtime.session_state(&session_id).await,
            Some(SessionState::Idle)
        );

        // Start the turn in a background task so we can observe state mid-run
        let (event_tx, _event_rx) = mpsc::channel(100);
        let runtime_clone = runtime.clone();
        let sid_clone = session_id.clone();

        let turn_handle = tokio::spawn(async move {
            runtime_clone
                .start_turn(&sid_clone, "Hello".to_string(), event_tx, None)
                .await
        });

        // Wait until the session transitions to Running.
        let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(1);
        loop {
            if runtime.session_state(&session_id).await == Some(SessionState::Running) {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "session did not enter running state before deadline"
            );
            tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        }

        // Wait for the turn to complete
        let result = turn_handle.await.unwrap().unwrap();
        assert!(result.text.contains("Slow response"));

        // After completion, state should be Idle again
        let state = runtime.session_state(&session_id).await;
        assert_eq!(
            state,
            Some(SessionState::Idle),
            "Session should be Idle after turn completes"
        );
    }

    /// 4. Starting a second turn while one is running fails with SESSION_BUSY.
    #[tokio::test]
    async fn start_turn_on_busy_session_fails() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = Arc::new(make_runtime(temp_factory(&temp), 10));

        let session_id = runtime
            .create_session(slow_build_config(200))
            .await
            .unwrap();

        // Start first turn in background
        let (event_tx1, _rx1) = mpsc::channel(100);
        let runtime_clone = runtime.clone();
        let sid_clone = session_id.clone();
        let _turn_handle = tokio::spawn(async move {
            runtime_clone
                .start_turn(&sid_clone, "First".to_string(), event_tx1, None)
                .await
        });

        // Wait until the first turn is definitely running.
        let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(1);
        loop {
            if runtime.session_state(&session_id).await == Some(SessionState::Running) {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "session did not enter running state before deadline"
            );
            tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        }

        // Try to start a second turn
        let (event_tx2, _rx2) = mpsc::channel(100);
        let result = runtime
            .start_turn(&session_id, "Second".to_string(), event_tx2, None)
            .await;

        assert!(result.is_err(), "Second turn should fail");
        let err = result.unwrap_err();
        assert_eq!(
            err.code,
            error::SESSION_BUSY,
            "Error code should be SESSION_BUSY, got: {}",
            err.code
        );
    }

    /// 5. Agent events are forwarded through the per-turn event channel.
    #[tokio::test]
    async fn start_turn_emits_events() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);

        let session_id = runtime.create_session(mock_build_config()).await.unwrap();
        let (event_tx, mut event_rx) = mpsc::channel(100);

        let _result = runtime
            .start_turn(&session_id, "Hello".to_string(), event_tx, None)
            .await
            .unwrap();

        // Collect received events
        let mut events = Vec::new();
        while let Ok(event) = event_rx.try_recv() {
            events.push(event);
        }

        // We should have received at least some events (RunStarted, TextDelta, etc.)
        assert!(
            !events.is_empty(),
            "Should have received at least one event"
        );

        // Check that we got a RunStarted event
        let has_run_started = events
            .iter()
            .any(|e| matches!(e, AgentEvent::RunStarted { .. }));
        assert!(has_run_started, "Should have received a RunStarted event");
    }

    /// 6. Interrupting an idle session is a no-op (no error).
    #[tokio::test]
    async fn interrupt_on_idle_is_noop() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);

        let session_id = runtime.create_session(mock_build_config()).await.unwrap();

        // Interrupt while idle should succeed without error
        let result = runtime.interrupt(&session_id).await;
        assert!(result.is_ok(), "Interrupt on idle should not fail");

        // State should still be idle
        assert_eq!(
            runtime.session_state(&session_id).await,
            Some(SessionState::Idle)
        );
    }

    /// 7. Archiving a session removes it from the runtime.
    #[tokio::test]
    async fn archive_session_removes_handle() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);

        let session_id = runtime.create_session(mock_build_config()).await.unwrap();

        // Verify it exists
        assert!(runtime.session_state(&session_id).await.is_some());

        // Archive it
        runtime.archive_session(&session_id).await.unwrap();

        // Verify it's gone
        assert!(
            runtime.session_state(&session_id).await.is_none(),
            "Archived session should no longer exist"
        );

        // Archiving again should fail
        let result = runtime.archive_session(&session_id).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code, error::SESSION_NOT_FOUND);
    }

    /// 8. Max sessions limit is enforced.
    #[tokio::test]
    async fn max_sessions_enforced() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 2);

        // Create two sessions (the max)
        let _s1 = runtime.create_session(mock_build_config()).await.unwrap();
        let _s2 = runtime.create_session(mock_build_config()).await.unwrap();

        // Third should fail
        let result = runtime.create_session(mock_build_config()).await;
        assert!(result.is_err(), "Third session should fail");
        let err = result.unwrap_err();
        assert!(
            err.message.contains("Max sessions"),
            "Error message should mention max sessions, got: {}",
            err.message
        );
    }

    /// 9. session_state returns None for an unknown session ID.
    #[tokio::test]
    async fn session_state_none_for_unknown() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);

        let unknown_id = SessionId::new();
        assert_eq!(runtime.session_state(&unknown_id).await, None);
    }

    /// 10. Shutdown closes all sessions.
    #[tokio::test]
    async fn shutdown_closes_all_sessions() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = make_runtime(temp_factory(&temp), 10);

        let s1 = runtime.create_session(mock_build_config()).await.unwrap();
        let s2 = runtime.create_session(mock_build_config()).await.unwrap();

        // Verify both exist
        assert!(runtime.session_state(&s1).await.is_some());
        assert!(runtime.session_state(&s2).await.is_some());

        // Shutdown
        runtime.shutdown().await;

        // Both should be gone from the sessions map
        let sessions = runtime.list_sessions().await;
        assert!(
            sessions.is_empty(),
            "All sessions should be removed after shutdown"
        );
    }

    /// 11. Startup registry build rejects invalid lineage/remap config.
    #[test]
    fn build_skill_identity_registry_rejects_invalid_config() {
        let cfg = Config {
            skills: SkillsConfig {
                repositories: vec![
                    SkillRepositoryConfig {
                        name: "old".to_string(),
                        source_uuid: SourceUuid::parse("dc256086-0d2f-4f61-a307-320d4148107f")
                            .expect("uuid"),
                        transport: SkillRepoTransport::Filesystem {
                            path: ".rkat/skills-old".to_string(),
                        },
                    },
                    SkillRepositoryConfig {
                        name: "new-a".to_string(),
                        source_uuid: SourceUuid::parse("a93d587d-8f44-438f-8189-6e8cf549f6e7")
                            .expect("uuid"),
                        transport: SkillRepoTransport::Filesystem {
                            path: ".rkat/skills-a".to_string(),
                        },
                    },
                    SkillRepositoryConfig {
                        name: "new-b".to_string(),
                        source_uuid: SourceUuid::parse("e8df561d-d38f-4242-af55-3a6efb34c950")
                            .expect("uuid"),
                        transport: SkillRepoTransport::Filesystem {
                            path: ".rkat/skills-b".to_string(),
                        },
                    },
                ],
                identity: SkillsIdentityConfig {
                    lineage: vec![SourceIdentityLineage {
                        event_id: "split-1".to_string(),
                        recorded_at_unix_secs: 1,
                        required_from_skills: vec![
                            SkillName::parse("email-extractor").expect("skill"),
                            SkillName::parse("pdf-processing").expect("skill"),
                        ],
                        event: SourceIdentityLineageEvent::Split {
                            from: SourceUuid::parse("dc256086-0d2f-4f61-a307-320d4148107f")
                                .expect("uuid"),
                            into: vec![
                                SourceUuid::parse("a93d587d-8f44-438f-8189-6e8cf549f6e7")
                                    .expect("uuid"),
                                SourceUuid::parse("e8df561d-d38f-4242-af55-3a6efb34c950")
                                    .expect("uuid"),
                            ],
                        },
                    }],
                    remaps: vec![SkillKeyRemap {
                        from: SkillKey {
                            source_uuid: SourceUuid::parse("dc256086-0d2f-4f61-a307-320d4148107f")
                                .expect("uuid"),
                            skill_name: SkillName::parse("email-extractor").expect("skill"),
                        },
                        to: SkillKey {
                            source_uuid: SourceUuid::parse("a93d587d-8f44-438f-8189-6e8cf549f6e7")
                                .expect("uuid"),
                            skill_name: SkillName::parse("mail-extractor").expect("skill"),
                        },
                        reason: None,
                    }],
                    aliases: vec![],
                },
                ..SkillsConfig::default()
            },
            ..Config::default()
        };

        let result = SessionRuntime::build_skill_identity_registry(&cfg);
        assert!(matches!(
            result,
            Err(meerkat_core::skills::SkillError::MissingSkillRemaps { .. })
        ));
    }
}
