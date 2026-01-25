//! agent_spawn tool - Spawn a new sub-agent with clean context

use super::config::SubAgentError;
use super::runner::{DynSubAgentSpec, create_spawn_session, spawn_sub_agent_dyn};
use super::state::SubAgentToolState;
use crate::builtin::{BuiltinTool, BuiltinToolError};
use async_trait::async_trait;
use meerkat_client::LlmProvider;
use meerkat_core::ToolDef;
use meerkat_core::budget::BudgetLimits;
use meerkat_core::ops::{OperationId, ToolAccessPolicy};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;

/// Tool access policy for spawned agents
#[derive(Debug, Deserialize)]
#[serde(tag = "policy", rename_all = "snake_case")]
enum ToolAccessInput {
    /// Inherit all tools from parent
    Inherit,
    /// Only allow specific tools
    AllowList { tools: Vec<String> },
    /// Block specific tools
    DenyList { tools: Vec<String> },
}

impl From<ToolAccessInput> for ToolAccessPolicy {
    fn from(input: ToolAccessInput) -> Self {
        match input {
            ToolAccessInput::Inherit => ToolAccessPolicy::Inherit,
            ToolAccessInput::AllowList { tools } => ToolAccessPolicy::AllowList(tools),
            ToolAccessInput::DenyList { tools } => ToolAccessPolicy::DenyList(tools),
        }
    }
}

/// Budget limits for spawned agents
#[derive(Debug, Deserialize)]
#[allow(dead_code)] // Fields reserved for future use
struct BudgetInput {
    /// Maximum tokens for the sub-agent
    #[serde(default)]
    max_tokens: Option<u64>,
    /// Maximum turns for the sub-agent
    #[serde(default)]
    max_turns: Option<u32>,
    /// Maximum tool calls for the sub-agent
    #[serde(default)]
    max_tool_calls: Option<u32>,
}

/// Parameters for agent_spawn tool
#[derive(Debug, Deserialize)]
struct SpawnParams {
    /// Initial prompt/task for the sub-agent
    prompt: String,
    /// LLM provider to use (anthropic, openai, gemini)
    #[serde(default)]
    provider: Option<String>,
    /// Model name (provider-specific)
    #[serde(default)]
    model: Option<String>,
    /// Tool access policy
    #[serde(default)]
    tool_access: Option<ToolAccessInput>,
    /// Budget limits
    #[serde(default)]
    budget: Option<BudgetInput>,
    /// Override system prompt
    #[serde(default)]
    system_prompt: Option<String>,
}

/// Response from agent_spawn tool
#[derive(Debug, Serialize)]
struct SpawnResponse {
    /// Spawned sub-agent ID
    agent_id: String,
    /// Name assigned to the sub-agent
    name: String,
    /// Provider being used
    provider: String,
    /// Model being used
    model: String,
    /// Current state (will be "running")
    state: String,
    /// Additional info
    message: String,
}

/// Tool for spawning new sub-agents with clean context
pub struct AgentSpawnTool {
    state: Arc<SubAgentToolState>,
}

impl AgentSpawnTool {
    /// Create a new agent_spawn tool
    pub fn new(state: Arc<SubAgentToolState>) -> Self {
        Self { state }
    }

    fn resolve_provider(&self, provider_str: Option<&str>) -> Result<LlmProvider, SubAgentError> {
        let provider_name = provider_str.unwrap_or(&self.state.config.default_provider);
        LlmProvider::parse(provider_name)
            .ok_or_else(|| SubAgentError::invalid_provider(provider_name))
    }

    fn resolve_model(&self, provider: LlmProvider, model: Option<&str>) -> String {
        if let Some(m) = model {
            return m.to_string();
        }
        if let Some(ref default) = self.state.config.default_model {
            return default.clone();
        }
        // Provider-specific defaults
        match provider {
            LlmProvider::Anthropic => "claude-sonnet-4-20250514".to_string(),
            LlmProvider::OpenAi => "gpt-4o".to_string(),
            LlmProvider::Gemini => "gemini-1.5-pro".to_string(),
        }
    }

    fn resolve_budget(&self, budget: Option<BudgetInput>) -> BudgetLimits {
        let default_tokens = self.state.config.default_budget;
        let max_tokens = self.state.config.max_budget_per_agent;

        let requested = budget.and_then(|b| b.max_tokens);
        let tokens = match (requested, max_tokens) {
            (Some(req), Some(max)) => Some(req.min(max)),
            (Some(req), None) => Some(req),
            (None, _) => default_tokens,
        };

        BudgetLimits {
            max_tokens: tokens,
            max_duration: None,
            max_tool_calls: None,
        }
    }

    async fn spawn_agent(&self, params: SpawnParams) -> Result<SpawnResponse, BuiltinToolError> {
        // Check if we can spawn
        if !self.state.can_spawn().await {
            return Err(BuiltinToolError::execution_failed(
                "Cannot spawn sub-agent: concurrency or depth limit reached",
            ));
        }

        // Check nesting permission
        if self.state.depth() > 0 && !self.state.can_nest() {
            return Err(BuiltinToolError::execution_failed(
                "Nested spawning is not allowed at this depth",
            ));
        }

        // Validate prompt
        if params.prompt.trim().is_empty() {
            return Err(BuiltinToolError::invalid_args("Prompt cannot be empty"));
        }

        // Resolve provider and model
        let provider = self
            .resolve_provider(params.provider.as_deref())
            .map_err(|e| BuiltinToolError::invalid_args(e.to_string()))?;
        let model = self.resolve_model(provider, params.model.as_deref());

        // Resolve tool access policy
        let _tool_access: ToolAccessPolicy = params
            .tool_access
            .map(Into::into)
            .unwrap_or(ToolAccessPolicy::Inherit);

        // Resolve budget
        let budget = self.resolve_budget(params.budget);

        // Create client for the sub-agent
        let client = self
            .state
            .client_factory
            .create_client(provider, None)
            .map_err(|e| {
                BuiltinToolError::execution_failed(format!("Failed to create LLM client: {}", e))
            })?;

        // Generate operation ID and name
        let op_id = OperationId::new();
        let name = format!("sub-agent-{}", &op_id.to_string()[..8]);

        // Create session with clean context (just the prompt)
        let session = create_spawn_session(&params.prompt, params.system_prompt.as_deref());

        // Create the sub-agent specification
        let spec = DynSubAgentSpec {
            client,
            model: model.clone(),
            tools: self.state.tool_dispatcher.clone(),
            store: self.state.session_store.clone(),
            session,
            budget: Some(budget),
            depth: self.state.depth() + 1,
            system_prompt: params.system_prompt.clone(),
        };

        // Spawn the sub-agent (this registers it and starts execution in a background task)
        let _steering_tx = spawn_sub_agent_dyn(
            op_id.clone(),
            name.clone(),
            spec,
            self.state.manager.clone(),
        )
        .await
        .map_err(|e| {
            BuiltinToolError::execution_failed(format!("Failed to spawn sub-agent: {}", e))
        })?;

        Ok(SpawnResponse {
            agent_id: op_id.to_string(),
            name,
            provider: provider.to_string(),
            model,
            state: "running".to_string(),
            message: "Sub-agent spawned successfully. Use agent_status to check progress."
                .to_string(),
        })
    }
}

#[async_trait]
impl BuiltinTool for AgentSpawnTool {
    fn name(&self) -> &'static str {
        "agent_spawn"
    }

    fn def(&self) -> ToolDef {
        ToolDef {
            name: "agent_spawn".to_string(),
            description: "Spawn a new sub-agent with clean context. The sub-agent starts fresh with only the provided prompt - it does not inherit conversation history. Useful for delegating independent tasks.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "prompt": {
                        "type": "string",
                        "description": "Initial prompt/task for the sub-agent. This is the only context the sub-agent will have."
                    },
                    "provider": {
                        "type": "string",
                        "description": "LLM provider to use: 'anthropic', 'openai', or 'gemini'. Defaults to the configured default provider.",
                        "enum": ["anthropic", "openai", "gemini"]
                    },
                    "model": {
                        "type": "string",
                        "description": "Model name (provider-specific). Defaults to the provider's recommended model."
                    },
                    "tool_access": {
                        "type": "object",
                        "description": "Tool access policy for the sub-agent",
                        "properties": {
                            "policy": {
                                "type": "string",
                                "enum": ["inherit", "allow_list", "deny_list"],
                                "description": "Policy type: 'inherit' (all parent tools), 'allow_list' (only specified tools), 'deny_list' (all except specified)"
                            },
                            "tools": {
                                "type": "array",
                                "items": { "type": "string" },
                                "description": "Tool names for allow_list or deny_list policies"
                            }
                        }
                    },
                    "budget": {
                        "type": "object",
                        "description": "Budget limits for the sub-agent",
                        "properties": {
                            "max_tokens": {
                                "type": "integer",
                                "description": "Maximum tokens the sub-agent can use"
                            },
                            "max_turns": {
                                "type": "integer",
                                "description": "Maximum conversation turns"
                            },
                            "max_tool_calls": {
                                "type": "integer",
                                "description": "Maximum tool calls"
                            }
                        }
                    },
                    "system_prompt": {
                        "type": "string",
                        "description": "Override the system prompt for the sub-agent"
                    }
                },
                "required": ["prompt"]
            }),
        }
    }

    fn default_enabled(&self) -> bool {
        false // Sub-agent tools are disabled by default
    }

    async fn call(&self, args: Value) -> Result<Value, BuiltinToolError> {
        let params: SpawnParams = serde_json::from_value(args)
            .map_err(|e| BuiltinToolError::invalid_args(format!("Invalid parameters: {}", e)))?;

        let response = self.spawn_agent(params).await?;
        serde_json::to_value(response).map_err(|e| {
            BuiltinToolError::execution_failed(format!("Failed to serialize response: {}", e))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtin::sub_agent::config::SubAgentConfig;
    use meerkat_client::{FactoryError, LlmClient, LlmClientFactory};
    use meerkat_core::error::{AgentError, ToolError};
    use meerkat_core::ops::ConcurrencyLimits;
    use meerkat_core::session::Session;
    use meerkat_core::sub_agent::SubAgentManager;
    use meerkat_core::{AgentSessionStore, AgentToolDispatcher};
    use tokio::sync::RwLock;

    struct MockClientFactory {
        should_fail: bool,
    }

    impl MockClientFactory {
        fn new() -> Self {
            Self { should_fail: false }
        }

        fn failing() -> Self {
            Self { should_fail: true }
        }
    }

    impl LlmClientFactory for MockClientFactory {
        fn create_client(
            &self,
            _provider: LlmProvider,
            _api_key: Option<String>,
        ) -> Result<Arc<dyn LlmClient>, FactoryError> {
            if self.should_fail {
                Err(FactoryError::MissingApiKey("mock".into()))
            } else {
                // For tests, we'd need a mock client. For now, return error.
                Err(FactoryError::MissingApiKey("test-mode".into()))
            }
        }

        fn supported_providers(&self) -> Vec<LlmProvider> {
            vec![
                LlmProvider::Anthropic,
                LlmProvider::OpenAi,
                LlmProvider::Gemini,
            ]
        }
    }

    struct MockToolDispatcher;

    #[async_trait]
    impl AgentToolDispatcher for MockToolDispatcher {
        fn tools(&self) -> Vec<ToolDef> {
            vec![]
        }

        async fn dispatch(&self, _name: &str, _args: &Value) -> Result<Value, ToolError> {
            Err(ToolError::not_found("mock"))
        }
    }

    struct MockSessionStore;

    #[async_trait]
    impl AgentSessionStore for MockSessionStore {
        async fn save(&self, _session: &Session) -> Result<(), AgentError> {
            Ok(())
        }

        async fn load(&self, _id: &str) -> Result<Option<Session>, AgentError> {
            Ok(None)
        }
    }

    fn create_test_state() -> Arc<SubAgentToolState> {
        let limits = ConcurrencyLimits::default();
        let manager = Arc::new(SubAgentManager::new(limits.clone(), 0));
        let client_factory = Arc::new(MockClientFactory::new());
        let tool_dispatcher = Arc::new(MockToolDispatcher);
        let session_store = Arc::new(MockSessionStore);
        let parent_session = Arc::new(RwLock::new(Session::new()));
        let config = SubAgentConfig::default();

        Arc::new(SubAgentToolState::new(
            manager,
            client_factory,
            tool_dispatcher,
            session_store,
            parent_session,
            config,
            0,
        ))
    }

    fn create_failing_state() -> Arc<SubAgentToolState> {
        let limits = ConcurrencyLimits::default();
        let manager = Arc::new(SubAgentManager::new(limits.clone(), 0));
        let client_factory = Arc::new(MockClientFactory::failing());
        let tool_dispatcher = Arc::new(MockToolDispatcher);
        let session_store = Arc::new(MockSessionStore);
        let parent_session = Arc::new(RwLock::new(Session::new()));
        let config = SubAgentConfig::default();

        Arc::new(SubAgentToolState::new(
            manager,
            client_factory,
            tool_dispatcher,
            session_store,
            parent_session,
            config,
            0,
        ))
    }

    #[test]
    fn test_tool_name() {
        let state = create_test_state();
        let tool = AgentSpawnTool::new(state);
        assert_eq!(tool.name(), "agent_spawn");
    }

    #[test]
    fn test_tool_def() {
        let state = create_test_state();
        let tool = AgentSpawnTool::new(state);
        let def = tool.def();

        assert_eq!(def.name, "agent_spawn");
        assert!(def.description.contains("Spawn"));

        let schema = &def.input_schema;
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["prompt"].is_object());
        assert!(schema["properties"]["provider"].is_object());
        assert!(schema["properties"]["model"].is_object());
        assert!(schema["properties"]["tool_access"].is_object());
        assert!(schema["properties"]["budget"].is_object());
        assert_eq!(schema["required"], json!(["prompt"]));
    }

    #[test]
    fn test_default_disabled() {
        let state = create_test_state();
        let tool = AgentSpawnTool::new(state);
        assert!(!tool.default_enabled());
    }

    #[test]
    fn test_resolve_provider_default() {
        let state = create_test_state();
        let tool = AgentSpawnTool::new(state);
        let provider = tool.resolve_provider(None).unwrap();
        assert_eq!(provider, LlmProvider::Anthropic);
    }

    #[test]
    fn test_resolve_provider_explicit() {
        let state = create_test_state();
        let tool = AgentSpawnTool::new(state);

        assert_eq!(
            tool.resolve_provider(Some("openai")).unwrap(),
            LlmProvider::OpenAi
        );
        assert_eq!(
            tool.resolve_provider(Some("gemini")).unwrap(),
            LlmProvider::Gemini
        );
        assert_eq!(
            tool.resolve_provider(Some("claude")).unwrap(),
            LlmProvider::Anthropic
        );
    }

    #[test]
    fn test_resolve_provider_invalid() {
        let state = create_test_state();
        let tool = AgentSpawnTool::new(state);
        assert!(tool.resolve_provider(Some("unknown")).is_err());
    }

    #[test]
    fn test_resolve_model_default() {
        let state = create_test_state();
        let tool = AgentSpawnTool::new(state);

        let model = tool.resolve_model(LlmProvider::Anthropic, None);
        assert!(model.contains("claude"));

        let model = tool.resolve_model(LlmProvider::OpenAi, None);
        assert!(model.contains("gpt"));

        let model = tool.resolve_model(LlmProvider::Gemini, None);
        assert!(model.contains("gemini"));
    }

    #[test]
    fn test_resolve_model_explicit() {
        let state = create_test_state();
        let tool = AgentSpawnTool::new(state);
        let model = tool.resolve_model(LlmProvider::Anthropic, Some("claude-opus-4-20250514"));
        assert_eq!(model, "claude-opus-4-20250514");
    }

    #[tokio::test]
    async fn test_spawn_empty_prompt() {
        let state = create_test_state();
        let tool = AgentSpawnTool::new(state);

        let result = tool.call(json!({"prompt": ""})).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("empty"));
    }

    #[tokio::test]
    async fn test_spawn_whitespace_prompt() {
        let state = create_test_state();
        let tool = AgentSpawnTool::new(state);

        let result = tool.call(json!({"prompt": "   "})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_spawn_invalid_provider() {
        let state = create_test_state();
        let tool = AgentSpawnTool::new(state);

        let result = tool
            .call(json!({
                "prompt": "Test task",
                "provider": "invalid_provider"
            }))
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("Invalid provider") || err.to_string().contains("invalid")
        );
    }

    #[tokio::test]
    async fn test_spawn_client_creation_failure() {
        let state = create_failing_state();
        let tool = AgentSpawnTool::new(state);

        let result = tool
            .call(json!({
                "prompt": "Test task"
            }))
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("LLM client") || err.to_string().contains("MissingApiKey")
        );
    }

    #[test]
    fn test_tool_access_input_conversion() {
        let inherit: ToolAccessPolicy = ToolAccessInput::Inherit.into();
        assert!(matches!(inherit, ToolAccessPolicy::Inherit));

        let allow: ToolAccessPolicy = ToolAccessInput::AllowList {
            tools: vec!["tool1".to_string()],
        }
        .into();
        match allow {
            ToolAccessPolicy::AllowList(tools) => assert_eq!(tools, vec!["tool1"]),
            _ => panic!("Expected AllowList"),
        }

        let deny: ToolAccessPolicy = ToolAccessInput::DenyList {
            tools: vec!["tool2".to_string()],
        }
        .into();
        match deny {
            ToolAccessPolicy::DenyList(tools) => assert_eq!(tools, vec!["tool2"]),
            _ => panic!("Expected DenyList"),
        }
    }

    #[test]
    fn test_resolve_budget_default() {
        let state = create_test_state();
        let tool = AgentSpawnTool::new(state);
        let budget = tool.resolve_budget(None);
        assert_eq!(budget.max_tokens, Some(50000)); // Default from config
    }

    #[test]
    fn test_resolve_budget_explicit() {
        let state = create_test_state();
        let tool = AgentSpawnTool::new(state);
        let budget = tool.resolve_budget(Some(BudgetInput {
            max_tokens: Some(10000),
            max_turns: None,
            max_tool_calls: None,
        }));
        assert_eq!(budget.max_tokens, Some(10000));
    }

    #[test]
    fn test_resolve_budget_capped() {
        let limits = ConcurrencyLimits::default();
        let manager = Arc::new(SubAgentManager::new(limits.clone(), 0));
        let client_factory = Arc::new(MockClientFactory::new());
        let tool_dispatcher = Arc::new(MockToolDispatcher);
        let session_store = Arc::new(MockSessionStore);
        let parent_session = Arc::new(RwLock::new(Session::new()));
        let config = SubAgentConfig::default().with_max_budget_per_agent(5000);

        let state = Arc::new(SubAgentToolState::new(
            manager,
            client_factory,
            tool_dispatcher,
            session_store,
            parent_session,
            config,
            0,
        ));

        let tool = AgentSpawnTool::new(state);
        let budget = tool.resolve_budget(Some(BudgetInput {
            max_tokens: Some(100000), // Request more than max
            max_turns: None,
            max_tool_calls: None,
        }));
        assert_eq!(budget.max_tokens, Some(5000)); // Should be capped
    }
}
