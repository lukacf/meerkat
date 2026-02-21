//! Factory-backed SessionAgent and SessionAgentBuilder implementations.
//!
//! Bridges `AgentFactory::build_agent()` into the `SessionAgent`/`SessionAgentBuilder`
//! traits so any surface can create a `SessionService` backed by the standard factory.

use async_trait::async_trait;
use meerkat_core::comms::{
    CommsCommand, EventStream, PeerDirectoryEntry, SendAndStreamError, SendError, SendReceipt,
    StreamError, StreamScope,
};
use meerkat_core::event::AgentEvent;
use meerkat_core::service::{CreateSessionRequest, SessionError};
use meerkat_core::types::{RunResult, SessionId};
use meerkat_core::{Config, ConfigStore, Session};
use meerkat_session::EphemeralSessionService;
use meerkat_session::ephemeral::{SessionAgent, SessionAgentBuilder, SessionSnapshot};
use std::sync::Arc;
use tokio::sync::mpsc;

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

    /// Open a command/event stream for a logical session or interaction scope.
    pub fn stream(&self, scope: StreamScope) -> Result<EventStream, StreamError> {
        let runtime = self
            .agent
            .comms()
            .ok_or_else(|| StreamError::NotFound("comms runtime is not configured".to_string()))?;
        runtime.stream(scope)
    }

    /// Send a command and open a command stream in one call.
    pub async fn send_and_stream(
        &self,
        cmd: CommsCommand,
    ) -> Result<(SendReceipt, EventStream), SendAndStreamError> {
        let runtime = self.agent.comms().ok_or_else(|| {
            SendAndStreamError::Send(SendError::Unsupported(
                "comms runtime is not configured".to_string(),
            ))
        })?;
        runtime.send_and_stream(cmd).await
    }

    /// List peers discoverable to this agent runtime.
    pub async fn peers(&self) -> Vec<PeerDirectoryEntry> {
        match self.agent.comms() {
            Some(runtime) => runtime.peers().await,
            None => Vec::new(),
        }
    }
}

#[async_trait]
impl SessionAgent for FactoryAgent {
    async fn run_with_events(
        &mut self,
        prompt: String,
        event_tx: mpsc::Sender<AgentEvent>,
    ) -> Result<RunResult, meerkat_core::error::AgentError> {
        self.agent.run_with_events(prompt, event_tx).await
    }

    async fn run_host_mode(
        &mut self,
        prompt: String,
    ) -> Result<RunResult, meerkat_core::error::AgentError> {
        self.agent.run_host_mode(prompt).await
    }

    fn set_skill_references(&mut self, refs: Option<Vec<meerkat_core::skills::SkillId>>) {
        self.agent.pending_skill_references = refs;
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
        self.agent.session().clone()
    }

    // BRIDGE(M6→M12): Legacy injector accessor, routed through comms_runtime.
    // Remove when event/push is eradicated in M7/M12.
    fn event_injector(&self) -> Option<Arc<dyn meerkat_core::SubscribableInjector>> {
        self.agent.comms_arc()?.event_injector()
    }

    fn comms_runtime(&self) -> Option<Arc<dyn meerkat_core::agent::CommsRuntime>> {
        self.agent.comms_arc()
    }
}

/// Implements [`SessionAgentBuilder`] by delegating to [`AgentFactory::build_agent()`].
pub struct FactoryAgentBuilder {
    factory: AgentFactory,
    config_snapshot: Config,
    config_store: Option<Arc<dyn ConfigStore>>,
    /// Optional default LLM client injected into all builds (for testing).
    pub default_llm_client: Option<Arc<dyn LlmClient>>,
}

impl FactoryAgentBuilder {
    /// Create a new builder backed by the given factory and config.
    pub fn new(factory: AgentFactory, config: Config) -> Self {
        Self {
            factory,
            config_snapshot: config,
            config_store: None,
            default_llm_client: None,
        }
    }

    /// Create a new builder that resolves config from a store on each build.
    ///
    /// If the store read fails, the builder falls back to `initial_config`.
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
        }
    }

    async fn resolve_config(&self) -> Config {
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

#[async_trait]
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
    store: Arc<dyn meerkat_store::SessionStore>,
) -> meerkat_session::PersistentSessionService<FactoryAgentBuilder> {
    let builder = FactoryAgentBuilder::new(factory, config);
    meerkat_session::PersistentSessionService::new(builder, max_sessions, store)
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use futures::stream;
    use meerkat_client::{LlmClient, LlmDoneOutcome, LlmEvent, LlmRequest};
    use meerkat_core::Config;
    use meerkat_core::comms::{InputSource, InputStreamMode};
    use meerkat_core::service::SessionBuildOptions;
    use meerkat_session::ephemeral::SessionAgent;
    use std::pin::Pin;
    use tempfile::TempDir;

    struct MockLlmClient {
        delta: &'static str,
    }

    impl Default for MockLlmClient {
        fn default() -> Self {
            Self { delta: "ok" }
        }
    }

    #[async_trait]
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

    fn mock_input_cmd(session_id: &SessionId, stream: InputStreamMode) -> CommsCommand {
        CommsCommand::Input {
            session_id: session_id.clone(),
            body: "hello".to_string(),
            source: InputSource::Rpc,
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

        let stream = agent.stream(StreamScope::Session(session_id.clone()));
        assert!(matches!(stream, Err(StreamError::NotFound(_))));

        let stream_result = agent
            .send_and_stream(mock_input_cmd(
                &session_id,
                InputStreamMode::ReserveInteraction,
            ))
            .await;
        assert!(matches!(
            stream_result,
            Err(SendAndStreamError::Send(SendError::Unsupported(_)))
        ));

        Ok(())
    }

    #[cfg(feature = "comms")]
    #[tokio::test]
    async fn test_factory_agent_send_and_stream_opens_interaction_stream() -> Result<(), String> {
        let temp = tempfile::tempdir().map_err(|err| format!("tempdir: {err}"))?;
        let mut build_config = AgentBuildConfig::new("claude-sonnet-4-5");
        build_config.host_mode = true;
        build_config.comms_name = Some("factory-agent-comms".to_string());

        let agent = build_factory_agent_with_mock(&temp, build_config).await?;
        let session_id = agent.session().id().clone();
        let (receipt, stream) = agent
            .send_and_stream(mock_input_cmd(
                &session_id,
                InputStreamMode::ReserveInteraction,
            ))
            .await
            .map_err(|err| format!("send_and_stream failed: {err}"))?;

        let interaction_id = match receipt {
            SendReceipt::InputAccepted {
                interaction_id,
                stream_reserved,
            } => {
                assert!(stream_reserved);
                interaction_id
            }
            _ => unreachable!("unexpected receipt variant"),
        };

        assert!(matches!(
            agent.stream(StreamScope::Interaction(interaction_id)),
            Err(StreamError::AlreadyAttached(_))
        ));

        drop(stream);

        let peers = agent.peers().await;
        assert!(
            peers.is_empty(),
            "comms runtime should be configured but no trusted peers are registered"
        );

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
            prompt: "ignored".to_string(),
            system_prompt: None,
            max_tokens: None,
            event_tx: None,
            host_mode: false,
            skill_references: None,
            initial_turn: meerkat_core::service::InitialTurnPolicy::RunImmediately,
            build: Some(build),
        };

        let (build_event_tx, _build_event_rx) = mpsc::channel(8);
        let mut agent = builder
            .build_agent(&req, build_event_tx)
            .await
            .map_err(|err| format!("{err}"))?;

        let (run_event_tx, _run_event_rx) = mpsc::channel(8);
        let result = SessionAgent::run_with_events(&mut agent, "hello".to_string(), run_event_tx)
            .await
            .map_err(|err| format!("{err}"))?;
        assert_eq!(result.text, "override");
        Ok(())
    }
}
