//! Factory-backed SessionAgent and SessionAgentBuilder implementations.
//!
//! Bridges `AgentFactory::build_agent()` into the `SessionAgent`/`SessionAgentBuilder`
//! traits so any surface can create a `SessionService` backed by the standard factory.

use async_trait::async_trait;
use meerkat_core::Config;
#[cfg(not(target_arch = "wasm32"))]
use meerkat_core::ConfigStore;
use meerkat_core::Session;
use meerkat_core::comms::{CommsCommand, PeerDirectoryEntry, SendError, SendReceipt};
use meerkat_core::event::AgentEvent;
use meerkat_core::service::{CreateSessionRequest, SessionError, TurnToolOverlay};
use meerkat_core::types::{HandlingMode, RenderMetadata, RunResult, SessionId};
use meerkat_session::EphemeralSessionService;
use meerkat_session::ephemeral::{SessionAgent, SessionAgentBuilder, SessionSnapshot};
use std::sync::Arc;

#[cfg(not(target_arch = "wasm32"))]
use tokio::sync::mpsc;
#[cfg(target_arch = "wasm32")]
use tokio_with_wasm::alias::sync::mpsc;

#[cfg(feature = "session-store")]
use crate::PersistenceBundle;
use crate::{AgentBuildConfig, AgentFactory, DynAgent};
use meerkat_client::LlmClient;

/// Wrapper around [`DynAgent`] implementing [`SessionAgent`].
pub struct FactoryAgent {
    agent: DynAgent,
}

impl FactoryAgent {
    /// Access the underlying agent.
    pub fn agent(&self) -> &DynAgent {
        &self.agent
    }

    /// Access the underlying agent mutably.
    pub fn agent_mut(&mut self) -> &mut DynAgent {
        &mut self.agent
    }

    /// Access the current session for inspection.
    pub fn session(&self) -> &Session {
        self.agent.session()
    }

    /// Send a canonical comms command through the wrapped agent runtime.
    pub async fn send(&self, cmd: CommsCommand) -> Result<SendReceipt, SendError> {
        let runtime = self
            .agent
            .comms()
            .ok_or_else(|| SendError::Unsupported("comms runtime is not configured".to_string()))?;
        runtime.send(cmd).await
    }

    /// List peers discoverable to this agent runtime.
    pub async fn peers(&self) -> Vec<PeerDirectoryEntry> {
        match self.agent.comms() {
            Some(runtime) => runtime.peers().await,
            None => Vec::new(),
        }
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl SessionAgent for FactoryAgent {
    async fn run_with_events(
        &mut self,
        prompt: meerkat_core::types::ContentInput,
        event_tx: mpsc::Sender<AgentEvent>,
    ) -> Result<RunResult, meerkat_core::error::AgentError> {
        self.agent.run_with_events(prompt, event_tx).await
    }

    async fn run_turn_with_events(
        &mut self,
        prompt: meerkat_core::types::ContentInput,
        handling_mode: HandlingMode,
        render_metadata: Option<RenderMetadata>,
        event_tx: mpsc::Sender<AgentEvent>,
    ) -> Result<RunResult, meerkat_core::error::AgentError> {
        // TODO: Phase 3 will reintroduce interaction-id-scoped admission for
        // non-Queue handling (Steer) and render_metadata turns. For now, all
        // turns go through run_with_events.
        let _ = (handling_mode, render_metadata);
        self.agent.run_with_events(prompt, event_tx).await
    }

    fn set_skill_references(&mut self, refs: Option<Vec<meerkat_core::skills::SkillKey>>) {
        self.agent.pending_skill_references = refs;
    }

    fn set_flow_tool_overlay(
        &mut self,
        overlay: Option<TurnToolOverlay>,
    ) -> Result<(), meerkat_core::error::AgentError> {
        self.agent
            .set_flow_tool_overlay(overlay)
            .map_err(|error| meerkat_core::error::AgentError::ConfigError(error.to_string()))
    }

    fn replace_client(&mut self, client: std::sync::Arc<dyn meerkat_core::AgentLlmClient>) {
        self.agent.replace_client(client);
    }

    fn hot_swap_llm_identity(
        &mut self,
        client: std::sync::Arc<dyn meerkat_core::AgentLlmClient>,
        identity: meerkat_core::SessionLlmIdentity,
    ) -> Result<(), meerkat_core::error::AgentError> {
        let Some(mut metadata) = self.agent.session().session_metadata() else {
            return Err(meerkat_core::error::AgentError::InternalError(
                "session metadata missing during llm identity hot-swap".to_string(),
            ));
        };
        metadata.apply_llm_identity(&identity);
        self.agent
            .session_mut()
            .set_session_metadata(metadata)
            .map_err(|err| {
                meerkat_core::error::AgentError::InternalError(format!(
                    "failed to update session metadata during llm identity hot-swap: {err}"
                ))
            })?;
        self.agent.replace_client(client);
        Ok(())
    }

    fn stage_external_tool_filter(
        &mut self,
        filter: meerkat_core::ToolFilter,
    ) -> Result<(), meerkat_core::error::AgentError> {
        self.agent
            .stage_external_tool_filter(filter)
            .map(|_| ())
            .map_err(|error| meerkat_core::error::AgentError::ConfigError(error.to_string()))
    }

    fn cancel(&mut self) {
        self.agent.cancel();
    }

    fn session_id(&self) -> SessionId {
        self.agent.session().id().clone()
    }

    fn snapshot(&self) -> SessionSnapshot {
        let s = self.agent.session();
        SessionSnapshot {
            created_at: s.created_at(),
            updated_at: s.updated_at(),
            message_count: s.messages().len(),
            total_tokens: s.total_tokens(),
            usage: s.total_usage(),
            last_assistant_text: s.last_assistant_text(),
        }
    }

    fn session_clone(&self) -> Session {
        self.agent.session_with_system_context_state()
    }

    fn apply_runtime_system_context(
        &mut self,
        appends: &[meerkat_core::PendingSystemContextAppend],
    ) {
        self.agent
            .session_mut()
            .append_system_context_blocks(appends);
    }

    fn system_context_state(
        &self,
    ) -> Arc<std::sync::Mutex<meerkat_core::SessionSystemContextState>> {
        self.agent.system_context_state()
    }

    fn event_injector(&self) -> Option<Arc<dyn meerkat_core::EventInjector>> {
        self.agent.comms_arc()?.event_injector()
    }

    #[doc(hidden)]
    fn interaction_event_injector(
        &self,
    ) -> Option<Arc<dyn meerkat_core::event_injector::SubscribableInjector>> {
        self.agent.comms_arc()?.interaction_event_injector()
    }

    fn comms_runtime(&self) -> Option<Arc<dyn meerkat_core::agent::CommsRuntime>> {
        self.agent.comms_arc()
    }
}

/// Implements [`SessionAgentBuilder`] by delegating to [`AgentFactory::build_agent()`].
pub struct FactoryAgentBuilder {
    factory: AgentFactory,
    config_snapshot: Config,
    #[cfg(not(target_arch = "wasm32"))]
    config_store: Option<Arc<dyn ConfigStore>>,
    /// Optional default LLM client injected into all builds (for testing).
    pub default_llm_client: Option<Arc<dyn LlmClient>>,
    /// Optional default tool dispatcher injected when no override is provided.
    /// Used on wasm32 where filesystem-based tool resolution is unavailable.
    pub default_tool_dispatcher: Option<Arc<dyn meerkat_core::AgentToolDispatcher>>,
    /// Optional default session store injected when no override is provided.
    /// Used on wasm32 where filesystem-based stores are unavailable.
    pub default_session_store: Option<Arc<dyn meerkat_core::AgentSessionStore>>,
}

impl FactoryAgentBuilder {
    /// Create a new builder backed by the given factory and config.
    pub fn new(factory: AgentFactory, config: Config) -> Self {
        Self {
            factory,
            config_snapshot: config,
            #[cfg(not(target_arch = "wasm32"))]
            config_store: None,
            default_llm_client: None,
            default_tool_dispatcher: None,
            default_session_store: None,
        }
    }

    /// Create a new builder that resolves config from a store on each build.
    ///
    /// If the store read fails, the builder falls back to `initial_config`.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn new_with_config_store(
        factory: AgentFactory,
        initial_config: Config,
        config_store: Arc<dyn ConfigStore>,
    ) -> Self {
        Self {
            factory,
            config_snapshot: initial_config,
            config_store: Some(config_store),
            default_llm_client: None,
            default_tool_dispatcher: None,
            default_session_store: None,
        }
    }

    async fn resolve_config(&self) -> Config {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(store) = &self.config_store {
            match store.get().await {
                Ok(config) => return config,
                Err(err) => {
                    tracing::warn!("Failed to read latest config from store: {err}");
                }
            }
        }
        self.config_snapshot.clone()
    }

    /// Get a reference to the factory.
    pub fn factory(&self) -> &AgentFactory {
        &self.factory
    }

    /// Get a reference to the config.
    pub fn config(&self) -> &Config {
        &self.config_snapshot
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl SessionAgentBuilder for FactoryAgentBuilder {
    type Agent = FactoryAgent;

    async fn build_agent(
        &self,
        req: &CreateSessionRequest,
        event_tx: mpsc::Sender<AgentEvent>,
    ) -> Result<FactoryAgent, SessionError> {
        let mut build_config = AgentBuildConfig::from_create_session_request(req, event_tx);

        // Inject default LLM client if none provided.
        if build_config.llm_client_override.is_none()
            && let Some(ref client) = self.default_llm_client
        {
            build_config.llm_client_override = Some(client.clone());
        }

        // Inject default tool dispatcher if none provided.
        if build_config.tool_dispatcher_override.is_none()
            && let Some(ref dispatcher) = self.default_tool_dispatcher
        {
            build_config.tool_dispatcher_override = Some(dispatcher.clone());
        }

        // Inject default session store if none provided.
        if build_config.session_store_override.is_none()
            && let Some(ref store) = self.default_session_store
        {
            build_config.session_store_override = Some(store.clone());
        }

        let config = self.resolve_config().await;

        let agent = self
            .factory
            .build_agent(build_config, &config)
            .await
            .map_err(|e| {
                SessionError::Agent(meerkat_core::error::AgentError::InternalError(
                    e.to_string(),
                ))
            })?;

        Ok(FactoryAgent { agent })
    }
}

/// Convenience: build an `EphemeralSessionService` backed by `AgentFactory`.
pub fn build_ephemeral_service(
    factory: AgentFactory,
    config: Config,
    max_sessions: usize,
) -> EphemeralSessionService<FactoryAgentBuilder> {
    let builder = FactoryAgentBuilder::new(factory, config);
    EphemeralSessionService::new(builder, max_sessions)
}

/// Convenience: build a `PersistentSessionService` backed by `AgentFactory`.
#[cfg(feature = "session-store")]
pub fn build_persistent_service(
    factory: AgentFactory,
    config: Config,
    max_sessions: usize,
    persistence: PersistenceBundle,
) -> meerkat_session::PersistentSessionService<FactoryAgentBuilder> {
    let mut builder = FactoryAgentBuilder::new(factory, config);
    let (store, runtime_store) = persistence.into_parts();
    builder.default_session_store = Some(Arc::new(meerkat_store::StoreAdapter::new(Arc::clone(
        &store,
    ))));
    meerkat_session::PersistentSessionService::new(builder, max_sessions, store, runtime_store)
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use futures::stream;
    use meerkat_client::{LlmClient, LlmDoneOutcome, LlmEvent, LlmRequest};
    use meerkat_core::Config;
    use meerkat_core::comms::{InputSource, InputStreamMode};
    use meerkat_core::ops_lifecycle::OpsLifecycleRegistry;
    use meerkat_core::service::SessionBuildOptions;
    use meerkat_core::{ToolCallView, ToolDef, ToolDispatchOutcome, ToolError, ToolResult};
    use meerkat_runtime::RuntimeSessionAdapter;
    use meerkat_session::ephemeral::SessionAgent;
    use std::pin::Pin;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};
    use tempfile::TempDir;

    struct MockLlmClient {
        delta: &'static str,
    }

    impl Default for MockLlmClient {
        fn default() -> Self {
            Self { delta: "ok" }
        }
    }

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl LlmClient for MockLlmClient {
        fn stream<'a>(
            &'a self,
            _request: &'a LlmRequest,
        ) -> Pin<
            Box<dyn futures::Stream<Item = Result<LlmEvent, meerkat_client::LlmError>> + Send + 'a>,
        > {
            Box::pin(stream::iter(vec![
                Ok(LlmEvent::TextDelta {
                    delta: self.delta.to_string(),
                    meta: None,
                }),
                Ok(LlmEvent::Done {
                    outcome: LlmDoneOutcome::Success {
                        stop_reason: meerkat_core::StopReason::EndTurn,
                    },
                }),
            ]))
        }

        fn provider(&self) -> &'static str {
            "mock"
        }

        async fn health_check(&self) -> Result<(), meerkat_client::LlmError> {
            Ok(())
        }
    }

    #[derive(Default)]
    struct RegistryBindingProbe {
        bound: AtomicBool,
        seen_registry: Mutex<Option<Arc<dyn OpsLifecycleRegistry>>>,
        seen_session_id: Mutex<Option<SessionId>>,
    }

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl meerkat_core::AgentToolDispatcher for RegistryBindingProbe {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            Arc::from([])
        }

        async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
            Ok(ToolResult::new(call.id.to_string(), "noop".to_string(), false).into())
        }

        fn bind_ops_lifecycle(
            self: Arc<Self>,
            registry: Arc<dyn OpsLifecycleRegistry>,
            owner_session_id: SessionId,
        ) -> Result<
            Arc<dyn meerkat_core::AgentToolDispatcher>,
            meerkat_core::agent::OpsLifecycleBindError,
        > {
            self.bound.store(true, Ordering::SeqCst);
            *self.seen_registry.lock().expect("probe lock") = Some(registry);
            *self.seen_session_id.lock().expect("probe lock") = Some(owner_session_id);
            Ok(self)
        }

        fn supports_ops_lifecycle_binding(&self) -> bool {
            true
        }
    }

    async fn build_factory_agent_with_mock(
        temp: &TempDir,
        mut build_config: AgentBuildConfig,
    ) -> Result<FactoryAgent, String> {
        let factory = AgentFactory::new(temp.path().join("sessions"));
        build_config.llm_client_override = Some(Arc::new(MockLlmClient::default()));
        let agent = factory
            .build_agent(build_config, &Config::default())
            .await
            .map_err(|err| format!("{err}"))?;
        Ok(FactoryAgent { agent })
    }

    #[tokio::test]
    async fn factory_builder_uses_runtime_session_registry_override() -> Result<(), String> {
        let temp = tempfile::tempdir().map_err(|err| format!("tempdir: {err}"))?;
        let factory = AgentFactory::new(temp.path().join("sessions"));
        let mut builder = FactoryAgentBuilder::new(factory, Config::default());
        builder.default_llm_client = Some(Arc::new(MockLlmClient { delta: "ok" }));

        let probe = Arc::new(RegistryBindingProbe::default());
        let probe_dispatcher: Arc<dyn meerkat_core::AgentToolDispatcher> = probe.clone();
        builder.default_tool_dispatcher = Some(probe_dispatcher);

        let runtime_adapter = RuntimeSessionAdapter::ephemeral();
        let session = Session::new();
        let session_id = session.id().clone();
        runtime_adapter.register_session(session_id.clone()).await;
        let expected_registry = runtime_adapter
            .ops_lifecycle_registry(&session_id)
            .await
            .ok_or_else(|| "missing runtime registry".to_string())?
            as Arc<dyn OpsLifecycleRegistry>;

        let req = CreateSessionRequest {
            model: "claude-sonnet-4-5".to_string(),
            prompt: "hello".to_string().into(),
            render_metadata: None,
            system_prompt: None,
            max_tokens: None,
            event_tx: None,
            host_mode: false,
            host_mode_owner: meerkat_core::service::HostModeOwner::ExternalRuntime,
            skill_references: None,
            initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
            build: Some(SessionBuildOptions {
                resume_session: Some(session),
                ops_lifecycle_override: Some(expected_registry.clone()),
                ..SessionBuildOptions::default()
            }),
            labels: None,
        };

        let (event_tx, _event_rx) = mpsc::channel(8);
        let agent = builder
            .build_agent(&req, event_tx)
            .await
            .map_err(|err| err.to_string())?;
        drop(agent);

        assert!(
            probe.bound.load(Ordering::SeqCst),
            "dispatcher should receive ops lifecycle binding"
        );
        let seen_registry = probe
            .seen_registry
            .lock()
            .expect("probe lock")
            .clone()
            .ok_or_else(|| "dispatcher did not record registry".to_string())?;
        let seen_session_id = probe
            .seen_session_id
            .lock()
            .expect("probe lock")
            .clone()
            .ok_or_else(|| "dispatcher did not record session id".to_string())?;

        assert!(
            Arc::ptr_eq(&seen_registry, &expected_registry),
            "factory should use runtime adapter's canonical registry, not a fresh fallback"
        );
        assert_eq!(seen_session_id, session_id);

        Ok(())
    }

    fn mock_input_cmd(session_id: &SessionId, stream: InputStreamMode) -> CommsCommand {
        CommsCommand::Input {
            session_id: session_id.clone(),
            body: "hello".to_string(),
            blocks: None,
            source: InputSource::Rpc,
            handling_mode: meerkat_core::types::HandlingMode::Queue,
            stream,
            allow_self_session: true,
        }
    }

    #[tokio::test]
    async fn test_factory_agent_send_without_comms_runtime_is_unsupported() -> Result<(), String> {
        let temp = tempfile::tempdir().map_err(|err| format!("tempdir: {err}"))?;
        let agent = build_factory_agent_with_mock(
            &temp,
            AgentBuildConfig {
                ..AgentBuildConfig::new("claude-sonnet-4-5")
            },
        )
        .await?;
        let session_id = agent.session().id().clone();
        let result = agent
            .send(mock_input_cmd(&session_id, InputStreamMode::None))
            .await;
        assert!(matches!(result, Err(SendError::Unsupported(_))));

        Ok(())
    }

    #[tokio::test]
    async fn test_session_llm_override_is_applied_end_to_end() -> Result<(), String> {
        let temp = tempfile::tempdir().map_err(|err| format!("tempdir: {err}"))?;
        let factory = AgentFactory::new(temp.path().join("sessions"));
        let mut builder = FactoryAgentBuilder::new(factory, Config::default());
        builder.default_llm_client = Some(Arc::new(MockLlmClient { delta: "default" }));

        let build = SessionBuildOptions {
            llm_client_override: Some(crate::encode_llm_client_override_for_service(Arc::new(
                MockLlmClient { delta: "override" },
            ))),
            ..SessionBuildOptions::default()
        };
        let req = CreateSessionRequest {
            model: "claude-sonnet-4-5".to_string(),
            prompt: "ignored".to_string().into(),
            render_metadata: None,
            system_prompt: None,
            max_tokens: None,
            event_tx: None,
            host_mode: false,
            host_mode_owner: meerkat_core::service::HostModeOwner::ExternalRuntime,
            skill_references: None,
            initial_turn: meerkat_core::service::InitialTurnPolicy::RunImmediately,
            build: Some(build),
            labels: None,
        };

        let (build_event_tx, _build_event_rx) = mpsc::channel(8);
        let mut agent = builder
            .build_agent(&req, build_event_tx)
            .await
            .map_err(|err| format!("{err}"))?;

        let (run_event_tx, _run_event_rx) = mpsc::channel(8);
        let result =
            SessionAgent::run_with_events(&mut agent, "hello".to_string().into(), run_event_tx)
                .await
                .map_err(|err| format!("{err}"))?;
        assert_eq!(result.text, "override");
        Ok(())
    }

    // ── Per-agent provider resolution via Config.providers.api_keys ──
    //
    // These tests verify the architectural invariant that per-agent provider
    // agnosticity works via Config — the same code path used by WASM when
    // default_llm_client is NOT set.

    /// Helper: build a request with the given model (deferred — no initial turn).
    fn make_session_request(model: &str) -> CreateSessionRequest {
        CreateSessionRequest {
            model: model.to_string(),
            prompt: "test".to_string().into(),
            render_metadata: None,
            system_prompt: None,
            max_tokens: None,
            event_tx: None,
            host_mode: false,
            host_mode_owner: meerkat_core::service::HostModeOwner::ExternalRuntime,
            skill_references: None,
            initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
            build: None,
            labels: None,
        }
    }

    /// When Config.providers.api_keys is populated and default_llm_client is None,
    /// build_agent() resolves the correct provider per-model from the Config map.
    /// This is the WASM code path after the default_llm_client fix.
    #[tokio::test]
    async fn test_config_api_keys_resolve_different_providers_per_model() -> Result<(), String> {
        let temp = tempfile::tempdir().map_err(|e| format!("tempdir: {e}"))?;
        let factory = AgentFactory::new(temp.path().join("sessions"));
        let mut config = Config::default();
        config.providers.api_keys = Some(std::collections::HashMap::from([
            ("anthropic".into(), "test-anthropic-key".into()),
            ("openai".into(), "test-openai-key".into()),
            ("gemini".into(), "test-gemini-key".into()),
        ]));
        let builder = FactoryAgentBuilder::new(factory, config);

        let (tx1, _rx1) = mpsc::channel(8);
        builder
            .build_agent(&make_session_request("claude-sonnet-4-5"), tx1)
            .await
            .map_err(|e| format!("anthropic model should build: {e}"))?;

        let (tx2, _rx2) = mpsc::channel(8);
        builder
            .build_agent(&make_session_request("gpt-5.2"), tx2)
            .await
            .map_err(|e| format!("openai model should build: {e}"))?;

        let (tx3, _rx3) = mpsc::channel(8);
        builder
            .build_agent(&make_session_request("gemini-3-flash-preview"), tx3)
            .await
            .map_err(|e| format!("gemini model should build: {e}"))?;

        Ok(())
    }

    #[tokio::test]
    async fn test_default_llm_client_takes_precedence_over_config_keys() -> Result<(), String> {
        let temp = tempfile::tempdir().map_err(|e| format!("tempdir: {e}"))?;
        let factory = AgentFactory::new(temp.path().join("sessions"));
        let config = Config::default();
        let mut builder = FactoryAgentBuilder::new(factory, config);
        builder.default_llm_client = Some(Arc::new(MockLlmClient {
            delta: "from-default",
        }));

        let (tx, _rx) = mpsc::channel(8);
        let mut agent = builder
            .build_agent(&make_session_request("claude-sonnet-4-5"), tx)
            .await
            .map_err(|e| format!("build failed: {e}"))?;

        let (run_tx, _run_rx) = mpsc::channel(8);
        let result = SessionAgent::run_with_events(&mut agent, "hello".into(), run_tx)
            .await
            .map_err(|e| format!("run failed: {e}"))?;
        assert_eq!(result.text, "from-default");
        Ok(())
    }

    #[tokio::test]
    async fn test_unknown_model_prefix_fails_even_with_config_keys() -> Result<(), String> {
        let temp = tempfile::tempdir().map_err(|e| format!("tempdir: {e}"))?;
        let factory = AgentFactory::new(temp.path().join("sessions"));
        let mut config = Config::default();
        config.providers.api_keys = Some(std::collections::HashMap::from([(
            "anthropic".into(),
            "test-key".into(),
        )]));
        let builder = FactoryAgentBuilder::new(factory, config);

        let (tx, _rx) = mpsc::channel(8);
        let result = builder
            .build_agent(&make_session_request("llama-3.1-70b"), tx)
            .await;
        assert!(result.is_err(), "unknown model prefix should fail");
        Ok(())
    }
}
