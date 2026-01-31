//! Agent builder.

use crate::budget::{Budget, BudgetLimits};
use crate::config::AgentConfig;
use crate::ops::ConcurrencyLimits;
use crate::prompt::SystemPromptConfig;
use crate::retry::RetryPolicy;
use crate::session::Session;
use crate::state::LoopState;
use crate::sub_agent::SubAgentManager;
use serde_json::Value;
use std::sync::Arc;

use super::{Agent, AgentLlmClient, AgentSessionStore, AgentToolDispatcher, CommsRuntime};

/// Builder for creating an Agent
#[derive(Default)]
pub struct AgentBuilder {
    pub(super) config: AgentConfig,
    pub(super) system_prompt: Option<String>,
    pub(super) budget_limits: Option<BudgetLimits>,
    pub(super) retry_policy: RetryPolicy,
    pub(super) session: Option<Session>,
    pub(super) concurrency_limits: ConcurrencyLimits,
    pub(super) depth: u32,
    pub(super) comms_runtime: Option<Arc<dyn CommsRuntime>>,
}

impl AgentBuilder {
    /// Create a new agent builder with default config
    pub fn new() -> Self {
        Self {
            config: AgentConfig::default(),
            system_prompt: None,
            budget_limits: None,
            retry_policy: RetryPolicy::default(),
            session: None,
            concurrency_limits: ConcurrencyLimits::default(),
            depth: 0,
            comms_runtime: None,
        }
    }

    /// Set concurrency limits for sub-agents
    pub fn concurrency_limits(mut self, limits: ConcurrencyLimits) -> Self {
        self.concurrency_limits = limits;
        self
    }

    /// Set the nesting depth for sub-agents
    pub fn depth(mut self, depth: u32) -> Self {
        self.depth = depth;
        self
    }

    /// Set the model to use
    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.config.model = model.into();
        self
    }

    /// Set the system prompt
    pub fn system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = Some(prompt.into());
        self
    }

    /// Set max tokens per turn
    pub fn max_tokens_per_turn(mut self, tokens: u32) -> Self {
        self.config.max_tokens_per_turn = tokens;
        self
    }

    /// Set temperature
    pub fn temperature(mut self, temp: f32) -> Self {
        self.config.temperature = Some(temp);
        self
    }

    /// Set budget limits
    pub fn budget(mut self, limits: BudgetLimits) -> Self {
        self.budget_limits = Some(limits);
        self
    }

    /// Set provider-specific parameters
    pub fn provider_params(mut self, params: Value) -> Self {
        self.config.provider_params = Some(params);
        self
    }

    /// Set retry policy for LLM calls
    pub fn retry_policy(mut self, policy: RetryPolicy) -> Self {
        self.retry_policy = policy;
        self
    }

    /// Resume from an existing session
    pub fn resume_session(mut self, session: Session) -> Self {
        self.session = Some(session);
        self
    }

    /// Set the comms runtime.
    pub fn with_comms_runtime(mut self, runtime: Arc<dyn CommsRuntime>) -> Self {
        self.comms_runtime = Some(runtime);
        self
    }

    /// Build the agent
    pub async fn build<C, T, S>(
        self,
        client: Arc<C>,
        tools: Arc<T>,
        store: Arc<S>,
    ) -> Agent<C, T, S>
    where
        C: AgentLlmClient + ?Sized,
        T: AgentToolDispatcher + ?Sized,
        S: AgentSessionStore + ?Sized,
    {
        let session = match self.session {
            Some(s) => s,
            None => {
                let mut s = Session::new();
                let prompt = match &self.system_prompt {
                    Some(p) => p.clone(),
                    None => SystemPromptConfig::new().compose().await,
                };
                s.set_system_prompt(prompt);
                s
            }
        };

        let budget = Budget::new(self.budget_limits.unwrap_or_default());
        let sub_agent_manager = Arc::new(SubAgentManager::new(self.concurrency_limits, self.depth));

        Agent {
            config: self.config,
            client,
            tools,
            store,
            session,
            budget,
            retry_policy: self.retry_policy,
            state: LoopState::CallingLlm,
            sub_agent_manager,
            depth: self.depth,
            comms_runtime: self.comms_runtime,
        }
    }
}
