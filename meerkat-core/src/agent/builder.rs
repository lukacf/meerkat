//! Agent builder.

use crate::budget::{Budget, BudgetLimits};
use crate::config::AgentConfig;
use crate::ops::ConcurrencyLimits;
use crate::prompt::SystemPromptConfig;
use crate::retry::RetryPolicy;
use crate::session::Session;
use crate::state::LoopState;
use crate::sub_agent::SubAgentManager;
use crate::types::{Message, OutputSchema};
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

    /// Set output schema for structured output extraction
    pub fn output_schema(mut self, schema: OutputSchema) -> Self {
        self.config.output_schema = Some(schema);
        self
    }

    /// Set maximum retries for structured output validation
    pub fn structured_output_retries(mut self, retries: u32) -> Self {
        self.config.structured_output_retries = retries;
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
        let mut session = self.session.unwrap_or_default();

        // Apply system prompt: use builder's prompt if set, otherwise compose default for new sessions
        let has_system_prompt = matches!(session.messages().first(), Some(Message::System(_)));
        if let Some(prompt) = self.system_prompt {
            session.set_system_prompt(prompt);
        } else if !has_system_prompt {
            // Only set default prompt for new sessions without an existing system prompt
            session.set_system_prompt(SystemPromptConfig::new().compose().await);
        }

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

#[cfg(test)]
#[allow(clippy::panic)]
mod tests {
    use super::*;
    use crate::LlmStreamResult;
    use crate::error::{AgentError, ToolError};
    use crate::types::{StopReason, ToolDef, UserMessage};
    use async_trait::async_trait;

    struct MockClient;

    #[async_trait]
    impl AgentLlmClient for MockClient {
        async fn stream_response(
            &self,
            _messages: &[Message],
            _tools: &[Arc<ToolDef>],
            _max_tokens: u32,
            _temperature: Option<f32>,
            _provider_params: Option<&Value>,
        ) -> Result<LlmStreamResult, AgentError> {
            Ok(LlmStreamResult::new(
                "Done".to_string(),
                vec![],
                StopReason::EndTurn,
                crate::types::Usage::default(),
            ))
        }

        fn provider(&self) -> &'static str {
            "mock"
        }
    }

    struct MockTools;

    #[async_trait]
    impl AgentToolDispatcher for MockTools {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            Arc::new([])
        }

        async fn dispatch(&self, name: &str, _args: &Value) -> Result<Value, ToolError> {
            Err(ToolError::NotFound {
                name: name.to_string(),
            })
        }
    }

    struct MockStore;

    #[async_trait]
    impl AgentSessionStore for MockStore {
        async fn save(&self, _session: &Session) -> Result<(), AgentError> {
            Ok(())
        }
        async fn load(&self, _id: &str) -> Result<Option<Session>, AgentError> {
            Ok(None)
        }
    }

    /// Regression test: AgentBuilder should apply system_prompt to new sessions
    #[tokio::test]
    async fn test_regression_builder_applies_system_prompt_to_new_session() {
        let client = Arc::new(MockClient);
        let tools = Arc::new(MockTools);
        let store = Arc::new(MockStore);

        let agent = AgentBuilder::new()
            .system_prompt("Custom system prompt")
            .build(client, tools, store)
            .await;

        // Check that the system prompt was applied
        let messages = agent.session().messages();
        assert!(!messages.is_empty(), "Session should have messages");

        match &messages[0] {
            Message::System(sys) => {
                assert_eq!(sys.content, "Custom system prompt");
            }
            other => panic!("First message should be System, got: {:?}", other),
        }
    }

    /// Regression test: AgentBuilder should apply system_prompt to resumed sessions
    /// Previously, system_prompt was ignored when resuming a session.
    #[tokio::test]
    async fn test_regression_builder_applies_system_prompt_to_resumed_session() {
        let client = Arc::new(MockClient);
        let tools = Arc::new(MockTools);
        let store = Arc::new(MockStore);

        // Create a session with an existing system prompt
        let mut existing_session = Session::new();
        existing_session.set_system_prompt("Original system prompt".to_string());
        existing_session.push(Message::User(UserMessage {
            content: "Hello".to_string(),
        }));

        // Resume the session with a NEW system prompt
        let agent = AgentBuilder::new()
            .resume_session(existing_session)
            .system_prompt("Updated system prompt")
            .build(client, tools, store)
            .await;

        // Check that the system prompt was UPDATED
        let messages = agent.session().messages();
        assert!(!messages.is_empty(), "Session should have messages");

        match &messages[0] {
            Message::System(sys) => {
                assert_eq!(
                    sys.content, "Updated system prompt",
                    "System prompt should be updated when resuming with a new prompt"
                );
            }
            other => panic!("First message should be System, got: {:?}", other),
        }

        // User message should still be preserved
        assert!(messages.len() >= 2, "Should have system + user messages");
        match &messages[1] {
            Message::User(user) => {
                assert_eq!(user.content, "Hello");
            }
            other => panic!("Second message should be User, got: {:?}", other),
        }
    }

    /// Regression test: Resumed sessions without explicit system_prompt should keep their original
    #[tokio::test]
    async fn test_builder_preserves_existing_system_prompt_on_resume() {
        let client = Arc::new(MockClient);
        let tools = Arc::new(MockTools);
        let store = Arc::new(MockStore);

        // Create a session with an existing system prompt
        let mut existing_session = Session::new();
        existing_session.set_system_prompt("Original system prompt".to_string());

        // Resume WITHOUT specifying a new system prompt
        let agent = AgentBuilder::new()
            .resume_session(existing_session)
            // Note: no .system_prompt() call
            .build(client, tools, store)
            .await;

        // Original system prompt should be preserved
        let messages = agent.session().messages();
        match &messages[0] {
            Message::System(sys) => {
                assert_eq!(
                    sys.content, "Original system prompt",
                    "Original system prompt should be preserved when not overridden"
                );
            }
            other => panic!("First message should be System, got: {:?}", other),
        }
    }
}
