//! agent_spawn tool - Spawn a new sub-agent with clean context

use super::config::SubAgentError;
use super::runner::{
    DynSubAgentSpec, SubAgentCommsConfig, create_spawn_session, spawn_sub_agent_dyn,
};
use super::state::SubAgentToolState;
use crate::builtin::{BuiltinTool, BuiltinToolError};
use crate::dispatcher::FilteredDispatcher;
use async_trait::async_trait;
use meerkat_client::LlmProvider;
use meerkat_core::ToolDef;
use meerkat_core::budget::BudgetLimits;
use meerkat_core::ops::{OperationId, ToolAccessPolicy};
use schemars::JsonSchema;
use serde::de;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;

/// Allowed models per provider
/// These are the only models that can be used with agent_spawn
pub mod allowed_models {
    /// Allowed Anthropic models
    pub const ANTHROPIC: &[&str] = &[
        "claude-opus-4-6",
        "claude-sonnet-4-5",
        "claude-opus-4-5",
    ];

    /// Allowed OpenAI models (only latest frontier models)
    pub const OPENAI: &[&str] = &["gpt-5.2", "gpt-5.2-pro"];

    /// Allowed Gemini models
    pub const GEMINI: &[&str] = &["gemini-3-flash-preview", "gemini-3-pro-preview"];

    /// Format allowed models as a description string
    pub fn description() -> String {
        format!(
            "Allowed models - Anthropic: {}; OpenAI: {}; Gemini: {}",
            ANTHROPIC.join(", "),
            OPENAI.join(", "),
            GEMINI.join(", ")
        )
    }
}

/// Tool access policy for spawned agents
#[derive(Debug, JsonSchema)]
#[serde(tag = "policy", rename_all = "snake_case")]
enum ToolAccessInput {
    /// Inherit all tools from parent
    Inherit,
    /// Only allow specific tools
    AllowList { tools: Vec<String> },
    /// Block specific tools
    DenyList { tools: Vec<String> },
}

#[derive(Debug, Deserialize)]
#[serde(tag = "policy", rename_all = "snake_case")]
enum ToolAccessInputRaw {
    Inherit,
    AllowList { tools: Vec<String> },
    DenyList { tools: Vec<String> },
}

impl From<ToolAccessInputRaw> for ToolAccessInput {
    fn from(input: ToolAccessInputRaw) -> Self {
        match input {
            ToolAccessInputRaw::Inherit => ToolAccessInput::Inherit,
            ToolAccessInputRaw::AllowList { tools } => ToolAccessInput::AllowList { tools },
            ToolAccessInputRaw::DenyList { tools } => ToolAccessInput::DenyList { tools },
        }
    }
}

impl<'de> Deserialize<'de> for ToolAccessInput {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        match value {
            Value::String(raw) => {
                let parsed: ToolAccessInputRaw =
                    serde_json::from_str(&raw).map_err(de::Error::custom)?;
                Ok(parsed.into())
            }
            other => {
                let parsed: ToolAccessInputRaw =
                    serde_json::from_value(other).map_err(de::Error::custom)?;
                Ok(parsed.into())
            }
        }
    }
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
#[derive(Debug, Deserialize, JsonSchema)]
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
#[derive(Debug, Deserialize, JsonSchema)]
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
    /// Host mode - agent stays alive processing comms messages after initial prompt
    #[serde(default)]
    host_mode: bool,
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
    /// How to communicate with this child (if comms enabled)
    #[serde(skip_serializing_if = "Option::is_none")]
    comms: Option<String>,
}

/// Generate comms context to inject into child agent's prompt
fn child_comms_context(parent_name: &str) -> String {
    format!(
        r#"
## Parent Communication
Your parent agent is '{parent_name}'. To report findings or request help:
  send_message("{parent_name}", "your message here")

Always report important findings to your parent. Follow instructions from your parent.
"#,
        parent_name = parent_name
    )
}

/// Generate comms instructions for parent about messaging child
fn parent_comms_instructions(child_name: &str) -> String {
    format!(
        "To message this child: send_message(\"{child_name}\", \"your message\")",
        child_name = child_name
    )
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
        // Provider-specific defaults (first model in allowlist)
        match provider {
            LlmProvider::Anthropic => allowed_models::ANTHROPIC[0].to_string(),
            LlmProvider::OpenAi => allowed_models::OPENAI[0].to_string(),
            LlmProvider::Gemini => allowed_models::GEMINI[0].to_string(),
        }
    }

    /// Validate that a model is in the allowlist for the given provider
    pub fn validate_model(&self, provider: LlmProvider, model: &str) -> Result<(), SubAgentError> {
        let allowed = match provider {
            LlmProvider::Anthropic => allowed_models::ANTHROPIC,
            LlmProvider::OpenAi => allowed_models::OPENAI,
            LlmProvider::Gemini => allowed_models::GEMINI,
        };

        if allowed.contains(&model) {
            Ok(())
        } else {
            Err(SubAgentError::InvalidModel {
                model: model.to_string(),
                provider: provider.to_string(),
                allowed: allowed.iter().map(|s| s.to_string()).collect(),
            })
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

        if params.host_mode && self.state.parent_comms.is_none() {
            return Err(BuiltinToolError::invalid_args(
                "host_mode requires comms to be enabled for sub-agents".to_string(),
            ));
        }

        // Resolve provider and model
        let provider = self
            .resolve_provider(params.provider.as_deref())
            .map_err(|e| BuiltinToolError::invalid_args(e.to_string()))?;
        let model = self.resolve_model(provider, params.model.as_deref());

        // Validate model is in allowlist
        self.validate_model(provider, &model)
            .map_err(|e| BuiltinToolError::invalid_args(e.to_string()))?;

        // Resolve tool access policy and apply filtering
        let tool_access: ToolAccessPolicy = params
            .tool_access
            .map(Into::into)
            .unwrap_or(ToolAccessPolicy::Inherit);

        let filtered_tools: Arc<dyn meerkat_core::AgentToolDispatcher> = match &tool_access {
            ToolAccessPolicy::Inherit => self.state.tool_dispatcher.clone(),
            _ => Arc::new(FilteredDispatcher::new(
                self.state.tool_dispatcher.clone(),
                &tool_access,
            )),
        };

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
        // Use first 12 hex chars (8 + 4 after dash) to avoid collision with UUIDv7
        // since agents spawned in same millisecond have same first 8 chars
        let op_id = OperationId::new();
        let op_id_str = op_id.to_string();
        let name = format!("sub-agent-{}{}", &op_id_str[..8], &op_id_str[9..13]);

        // Build comms config if parent has comms enabled
        // Also prepare comms context to inject into child's prompt
        let (comms_config, comms_instructions) =
            if let Some(parent_comms) = &self.state.parent_comms {
                let config = SubAgentCommsConfig {
                    name: name.clone(),
                    base_dir: parent_comms.comms_base_dir.clone(),
                    parent_context: parent_comms.clone(),
                };
                let instructions = Some(parent_comms_instructions(&name));
                (Some(config), instructions)
            } else {
                (None, None)
            };

        // Create prompt with comms context injected if parent has comms enabled
        let enriched_prompt = if let Some(parent_comms) = &self.state.parent_comms {
            format!(
                "{}\n{}",
                child_comms_context(&parent_comms.parent_name),
                params.prompt
            )
        } else {
            params.prompt.clone()
        };

        // Resolve system prompt: explicit override > inherited tool usage instructions > none
        let effective_system_prompt = if params.system_prompt.is_some() {
            // Explicit override takes precedence
            params.system_prompt.clone()
        } else if self.state.config.inherit_system_prompt {
            // Inherit tool usage instructions from parent if configured
            self.state
                .get_tool_usage_instructions()
                .unwrap_or_else(|e| {
                    tracing::error!("{}", e);
                    None
                })
        } else {
            None
        };

        // Create session with clean context (enriched prompt includes comms context)
        let session = create_spawn_session(&enriched_prompt, effective_system_prompt.as_deref());

        // Create the sub-agent specification
        let spec = DynSubAgentSpec {
            client,
            model: model.clone(),
            tools: filtered_tools,
            store: self.state.session_store.clone(),
            session,
            budget: Some(budget),
            depth: self.state.depth() + 1,
            system_prompt: effective_system_prompt,
            comms_config,
            parent_trusted_peers: self.state.parent_trusted_peers.clone(),
            host_mode: params.host_mode,
        };

        // Spawn the sub-agent (this registers it and starts execution in a background task)
        spawn_sub_agent_dyn(
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
            comms: comms_instructions,
        })
    }
}

#[async_trait]
impl BuiltinTool for AgentSpawnTool {
    fn name(&self) -> &'static str {
        "agent_spawn"
    }

    fn def(&self) -> ToolDef {
        let mut schema = crate::schema::schema_for::<SpawnParams>();

        // Enrich model description with allowed values
        if let Some(model_prop) = schema
            .get_mut("properties")
            .and_then(|p| p.get_mut("model"))
        {
            if let Some(obj) = model_prop.as_object_mut() {
                obj.insert(
                    "description".to_string(),
                    Value::String(format!(
                        "Model name (provider-specific). {}",
                        allowed_models::description()
                    )),
                );
            }
        }

        ToolDef {
            name: "agent_spawn".to_string(),
            description: "Spawn a new sub-agent with clean context. The sub-agent starts fresh with only the provided prompt - it does not inherit conversation history. Useful for delegating independent tasks.".to_string(),
            input_schema: schema,
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
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::builtin::sub_agent::config::SubAgentConfig;
    use meerkat_client::{FactoryError, LlmClient, LlmClientFactory};
    use meerkat_core::error::{AgentError, ToolError};
    use meerkat_core::ops::ConcurrencyLimits;
    use meerkat_core::session::Session;
    use meerkat_core::sub_agent::SubAgentManager;
    use meerkat_core::{AgentSessionStore, AgentToolDispatcher, ToolCallView, ToolResult};
    use serde_json::json;
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
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            Arc::from([])
        }

        async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolResult, ToolError> {
            Err(ToolError::not_found(call.name))
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
        let manager = Arc::new(SubAgentManager::new(limits, 0));
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
        let manager = Arc::new(SubAgentManager::new(limits, 0));
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

    #[tokio::test]
    async fn test_spawn_host_mode_requires_comms() {
        let state = create_test_state();
        let tool = AgentSpawnTool::new(state);

        let result = tool
            .call(json!({
                "prompt": "Test task",
                "host_mode": true
            }))
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("host_mode requires comms"));
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
            _ => unreachable!("Expected AllowList"),
        }

        let deny: ToolAccessPolicy = ToolAccessInput::DenyList {
            tools: vec!["tool2".to_string()],
        }
        .into();
        match deny {
            ToolAccessPolicy::DenyList(tools) => assert_eq!(tools, vec!["tool2"]),
            _ => unreachable!("Expected DenyList"),
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
        let manager = Arc::new(SubAgentManager::new(limits, 0));
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

    // Model allowlist tests
    mod model_validation {
        use super::*;

        #[test]
        fn test_allowed_model_passes_validation() {
            let state = create_test_state();
            let tool = AgentSpawnTool::new(state);

            // These should all pass validation
            assert!(
                tool.validate_model(LlmProvider::Anthropic, "claude-opus-4-6")
                    .is_ok()
            );
            assert!(
                tool.validate_model(LlmProvider::Anthropic, "claude-sonnet-4-5")
                    .is_ok()
            );
            assert!(
                tool.validate_model(LlmProvider::Anthropic, "claude-opus-4-5")
                    .is_ok()
            );
            assert!(tool.validate_model(LlmProvider::OpenAi, "gpt-5.2").is_ok());
            assert!(
                tool.validate_model(LlmProvider::OpenAi, "gpt-5.2-pro")
                    .is_ok()
            );
            assert!(
                tool.validate_model(LlmProvider::Gemini, "gemini-3-flash-preview")
                    .is_ok()
            );
            assert!(
                tool.validate_model(LlmProvider::Gemini, "gemini-3-pro-preview")
                    .is_ok()
            );
        }

        #[test]
        fn test_disallowed_model_fails_validation() {
            let state = create_test_state();
            let tool = AgentSpawnTool::new(state);

            // Old/wrong model names should fail
            let err = tool
                .validate_model(LlmProvider::OpenAi, "gpt-4o")
                .unwrap_err();
            assert!(err.to_string().contains("not in allowlist"));
            assert!(err.to_string().contains("gpt-5.2")); // Should suggest allowed models

            let err = tool
                .validate_model(LlmProvider::Anthropic, "claude-3-5-sonnet-20241022")
                .unwrap_err();
            assert!(err.to_string().contains("not in allowlist"));

            let err = tool
                .validate_model(LlmProvider::Gemini, "gemini-1.5-flash")
                .unwrap_err();
            assert!(err.to_string().contains("not in allowlist"));
        }

        #[test]
        fn test_tool_def_includes_allowed_models() {
            let state = create_test_state();
            let tool = AgentSpawnTool::new(state);
            let def = tool.def();

            // The description should list allowed models
            let model_desc = def.input_schema["properties"]["model"]["description"]
                .as_str()
                .unwrap();
            assert!(
                model_desc.contains("gpt-5.2"),
                "Should list allowed OpenAI models"
            );
            assert!(
                model_desc.contains("claude-opus-4-6"),
                "Should list allowed Anthropic models"
            );
            assert!(
                model_desc.contains("gemini-3-flash-preview"),
                "Should list allowed Gemini models"
            );
        }
    }

    // Host mode tests
    mod host_mode {
        use super::*;
        use serde_json::json;

        #[test]
        fn test_spawn_params_host_mode_default_false() {
            // When host_mode is not specified, it should default to false
            let params: SpawnParams = serde_json::from_value(json!({"prompt": "test"})).unwrap();
            assert!(!params.host_mode);
        }

        #[test]
        fn test_spawn_params_host_mode_explicit_true() {
            let params: SpawnParams =
                serde_json::from_value(json!({"prompt": "test", "host_mode": true})).unwrap();
            assert!(params.host_mode);
        }

        #[test]
        fn test_spawn_params_host_mode_explicit_false() {
            let params: SpawnParams =
                serde_json::from_value(json!({"prompt": "test", "host_mode": false})).unwrap();
            assert!(!params.host_mode);
        }

        #[test]
        fn test_tool_def_includes_host_mode() {
            let state = create_test_state();
            let tool = AgentSpawnTool::new(state);
            let def = tool.def();

            let schema = &def.input_schema;
            assert!(
                schema["properties"]["host_mode"].is_object(),
                "Schema should include host_mode property"
            );
            assert_eq!(
                schema["properties"]["host_mode"]["type"], "boolean",
                "host_mode should be boolean"
            );
            assert!(
                schema["properties"]["host_mode"]["description"]
                    .as_str()
                    .unwrap()
                    .contains("stays alive"),
                "Description should explain host_mode behavior"
            );
        }
    }

    #[test]
    fn test_spawn_params_tool_access_accepts_json_string() {
        let params: SpawnParams = serde_json::from_value(json!({
            "prompt": "test",
            "tool_access": "{\"policy\":\"allow_list\",\"tools\":[\"shell\"]}"
        }))
        .unwrap();

        let tool_access = params.tool_access.expect("tool_access should parse");
        match tool_access {
            ToolAccessInput::AllowList { tools } => {
                assert_eq!(tools, vec!["shell"]);
            }
            other => panic!("Unexpected tool_access variant: {other:?}"),
        }
    }
}
