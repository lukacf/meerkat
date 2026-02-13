//! Factory-backed SessionAgent and SessionAgentBuilder implementations.
//!
//! Bridges `AgentFactory::build_agent()` into the `SessionAgent`/`SessionAgentBuilder`
//! traits so any surface can create a `SessionService` backed by the standard factory.

use async_trait::async_trait;
use meerkat_core::event::AgentEvent;
use meerkat_core::service::{CreateSessionRequest, SessionError};
use meerkat_core::types::{RunResult, SessionId};
use meerkat_core::{Config, Session};
use meerkat_session::EphemeralSessionService;
use meerkat_session::ephemeral::{SessionAgent, SessionAgentBuilder, SessionSnapshot};
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc};

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

    fn event_injector(&self) -> Option<Arc<dyn meerkat_core::SubscribableInjector>> {
        self.agent.comms()?.event_injector()
    }
}

/// Implements [`SessionAgentBuilder`] by delegating to [`AgentFactory::build_agent()`].
///
/// Surfaces pass a full `AgentBuildConfig` via the `build_config_slot` before
/// calling `SessionService::create_session()`. The builder picks it up and
/// uses it to construct the agent.
///
/// If no config is staged, a minimal config is built from the
/// `CreateSessionRequest` fields.
pub struct FactoryAgentBuilder {
    factory: AgentFactory,
    config: Config,
    /// Optional default LLM client injected into all builds (for testing).
    pub default_llm_client: Option<Arc<dyn LlmClient>>,
    /// Slot for passing a full `AgentBuildConfig` from the surface to the builder.
    pub build_config_slot: Arc<Mutex<Option<AgentBuildConfig>>>,
}

impl FactoryAgentBuilder {
    /// Create a new builder backed by the given factory and config.
    pub fn new(factory: AgentFactory, config: Config) -> Self {
        Self {
            factory,
            config,
            default_llm_client: None,
            build_config_slot: Arc::new(Mutex::new(None)),
        }
    }

    /// Stage a build config for the next `build_agent` call.
    pub async fn stage_config(&self, config: AgentBuildConfig) {
        let mut slot = self.build_config_slot.lock().await;
        *slot = Some(config);
    }

    /// Get a reference to the factory.
    pub fn factory(&self) -> &AgentFactory {
        &self.factory
    }

    /// Get a reference to the config.
    pub fn config(&self) -> &Config {
        &self.config
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
        // If a full build config was staged, use it.
        // Otherwise construct a minimal one from the request.
        let mut build_config = {
            let mut slot = self.build_config_slot.lock().await;
            slot.take().unwrap_or_else(|| {
                let mut bc = AgentBuildConfig::new(req.model.clone());
                bc.system_prompt = req.system_prompt.clone();
                bc.max_tokens = req.max_tokens;
                bc
            })
        };

        // Wire the event channel.
        build_config.event_tx = Some(event_tx);

        // Inject default LLM client if none provided.
        if build_config.llm_client_override.is_none() {
            if let Some(ref client) = self.default_llm_client {
                build_config.llm_client_override = Some(client.clone());
            }
        }

        let agent = self
            .factory
            .build_agent(build_config, &self.config)
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
