//! Configuration and error types for sub-agent tools

use meerkat_core::config::ResolvedSubAgentConfig;
use meerkat_core::ops::ConcurrencyLimits;
use serde::{Deserialize, Serialize};

/// Error types for sub-agent tool operations
#[derive(Debug, thiserror::Error)]
pub enum SubAgentError {
    /// Sub-agent was not found
    #[error("Sub-agent not found: {id}")]
    NotFound { id: String },

    /// Sub-agent is not in the expected state
    #[error("Sub-agent {id} is {actual_state}, expected {expected_state}")]
    InvalidState {
        id: String,
        actual_state: String,
        expected_state: String,
    },

    /// Concurrency limit exceeded
    #[error("Concurrency limit exceeded: {message}")]
    ConcurrencyLimitExceeded { message: String },

    /// Depth limit exceeded
    #[error("Maximum sub-agent depth ({max_depth}) exceeded")]
    DepthLimitExceeded { max_depth: u32 },

    /// Invalid provider specified
    #[error("Invalid provider: {provider}")]
    InvalidProvider { provider: String },

    /// Invalid model specified (not in allowlist)
    #[error("Model '{model}' is not in allowlist for provider '{provider}'. Allowed models: {}", allowed.join(", "))]
    InvalidModel {
        model: String,
        provider: String,
        allowed: Vec<String>,
    },

    /// Missing API key for provider
    #[error("Missing API key for provider: {provider}")]
    MissingApiKey { provider: String },

    /// Invalid arguments provided to tool
    #[error("Invalid arguments: {message}")]
    InvalidArguments { message: String },

    /// Sub-agent execution failed
    #[error("Sub-agent execution failed: {message}")]
    ExecutionFailed { message: String },

    /// Tool access policy violation
    #[error("Tool access denied: {tool_name}")]
    ToolAccessDenied { tool_name: String },

    /// Client factory error
    #[error("Client factory error: {message}")]
    ClientFactoryError { message: String },
}

impl SubAgentError {
    /// Create a NotFound error
    pub fn not_found(id: impl Into<String>) -> Self {
        Self::NotFound { id: id.into() }
    }

    /// Create an InvalidState error
    pub fn invalid_state(
        id: impl Into<String>,
        actual: impl Into<String>,
        expected: impl Into<String>,
    ) -> Self {
        Self::InvalidState {
            id: id.into(),
            actual_state: actual.into(),
            expected_state: expected.into(),
        }
    }

    /// Create a ConcurrencyLimitExceeded error
    pub fn concurrency_limit(message: impl Into<String>) -> Self {
        Self::ConcurrencyLimitExceeded {
            message: message.into(),
        }
    }

    /// Create a DepthLimitExceeded error
    pub fn depth_limit(max_depth: u32) -> Self {
        Self::DepthLimitExceeded { max_depth }
    }

    /// Create an InvalidProvider error
    pub fn invalid_provider(provider: impl Into<String>) -> Self {
        Self::InvalidProvider {
            provider: provider.into(),
        }
    }

    /// Create a MissingApiKey error
    pub fn missing_api_key(provider: impl Into<String>) -> Self {
        Self::MissingApiKey {
            provider: provider.into(),
        }
    }

    /// Create an InvalidArguments error
    pub fn invalid_arguments(message: impl Into<String>) -> Self {
        Self::InvalidArguments {
            message: message.into(),
        }
    }

    /// Create an ExecutionFailed error
    pub fn execution_failed(message: impl Into<String>) -> Self {
        Self::ExecutionFailed {
            message: message.into(),
        }
    }

    /// Create a ToolAccessDenied error
    pub fn tool_access_denied(tool_name: impl Into<String>) -> Self {
        Self::ToolAccessDenied {
            tool_name: tool_name.into(),
        }
    }

    /// Create a ClientFactoryError
    pub fn client_factory_error(message: impl Into<String>) -> Self {
        Self::ClientFactoryError {
            message: message.into(),
        }
    }
}

/// Configuration for sub-agent tools
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubAgentConfig {
    /// Default provider for spawned agents (e.g., "anthropic", "openai", "gemini")
    #[serde(default = "default_provider")]
    pub default_provider: String,

    /// Default model for spawned agents (provider-specific)
    #[serde(default)]
    pub default_model: Option<String>,

    /// Concurrency limits for sub-agents
    #[serde(default)]
    pub concurrency_limits: ConcurrencyLimits,

    /// Whether sub-agents can spawn further sub-agents by default
    #[serde(default = "default_allow_nested_spawn")]
    pub allow_nested_spawn: bool,

    /// Maximum budget (in tokens) that can be allocated to a single sub-agent
    #[serde(default)]
    pub max_budget_per_agent: Option<u64>,

    /// Default budget (in tokens) for spawned agents when not specified
    #[serde(default)]
    pub default_budget: Option<u64>,

    /// Whether to inherit system prompt by default
    #[serde(default = "default_inherit_system_prompt")]
    pub inherit_system_prompt: bool,

    /// Whether to enable comms for sub-agents (parent-child communication)
    #[serde(default)]
    pub enable_comms: bool,

    /// Base directory for sub-agent comms identity files
    /// Defaults to a temporary directory
    #[serde(default)]
    pub comms_base_dir: Option<std::path::PathBuf>,

    /// Resolved sub-agent model policy from the global config.
    /// When set, overrides the hardcoded allowlists and default provider/model.
    #[serde(skip)]
    pub resolved_policy: Option<ResolvedSubAgentConfig>,
}

fn default_provider() -> String {
    "anthropic".to_string()
}

fn default_allow_nested_spawn() -> bool {
    true
}

fn default_inherit_system_prompt() -> bool {
    true
}

impl Default for SubAgentConfig {
    fn default() -> Self {
        Self {
            default_provider: default_provider(),
            default_model: None,
            concurrency_limits: ConcurrencyLimits::default(),
            allow_nested_spawn: default_allow_nested_spawn(),
            max_budget_per_agent: None,
            default_budget: Some(50000), // 50k tokens default
            inherit_system_prompt: default_inherit_system_prompt(),
            enable_comms: false,
            comms_base_dir: None,
            resolved_policy: None,
        }
    }
}

impl SubAgentConfig {
    /// Create a new config with defaults
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the default provider
    pub fn with_default_provider(mut self, provider: impl Into<String>) -> Self {
        self.default_provider = provider.into();
        self
    }

    /// Set the default model
    pub fn with_default_model(mut self, model: impl Into<String>) -> Self {
        self.default_model = Some(model.into());
        self
    }

    /// Set concurrency limits
    pub fn with_concurrency_limits(mut self, limits: ConcurrencyLimits) -> Self {
        self.concurrency_limits = limits;
        self
    }

    /// Set whether nested spawn is allowed
    pub fn with_allow_nested_spawn(mut self, allow: bool) -> Self {
        self.allow_nested_spawn = allow;
        self
    }

    /// Set maximum budget per agent
    pub fn with_max_budget_per_agent(mut self, max_budget: u64) -> Self {
        self.max_budget_per_agent = Some(max_budget);
        self
    }

    /// Set default budget
    pub fn with_default_budget(mut self, budget: u64) -> Self {
        self.default_budget = Some(budget);
        self
    }

    /// Set whether to inherit system prompt
    pub fn with_inherit_system_prompt(mut self, inherit: bool) -> Self {
        self.inherit_system_prompt = inherit;
        self
    }

    /// Enable comms for sub-agents
    pub fn with_enable_comms(mut self, enable: bool) -> Self {
        self.enable_comms = enable;
        self
    }

    /// Set the comms base directory
    pub fn with_comms_base_dir(mut self, dir: std::path::PathBuf) -> Self {
        self.comms_base_dir = Some(dir);
        self
    }

    /// Set the resolved sub-agent model policy from global config.
    pub fn with_resolved_policy(mut self, policy: ResolvedSubAgentConfig) -> Self {
        self.resolved_policy = Some(policy);
        self
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn test_sub_agent_error_not_found() {
        let err = SubAgentError::not_found("agent-123");
        assert_eq!(err.to_string(), "Sub-agent not found: agent-123");
    }

    #[test]
    fn test_sub_agent_error_invalid_state() {
        let err = SubAgentError::invalid_state("agent-123", "completed", "running");
        assert_eq!(
            err.to_string(),
            "Sub-agent agent-123 is completed, expected running"
        );
    }

    #[test]
    fn test_sub_agent_error_concurrency_limit() {
        let err = SubAgentError::concurrency_limit("max 8 agents exceeded");
        assert_eq!(
            err.to_string(),
            "Concurrency limit exceeded: max 8 agents exceeded"
        );
    }

    #[test]
    fn test_sub_agent_error_depth_limit() {
        let err = SubAgentError::depth_limit(3);
        assert_eq!(err.to_string(), "Maximum sub-agent depth (3) exceeded");
    }

    #[test]
    fn test_sub_agent_error_invalid_provider() {
        let err = SubAgentError::invalid_provider("unknown");
        assert_eq!(err.to_string(), "Invalid provider: unknown");
    }

    #[test]
    fn test_sub_agent_error_missing_api_key() {
        let err = SubAgentError::missing_api_key("openai");
        assert_eq!(err.to_string(), "Missing API key for provider: openai");
    }

    #[test]
    fn test_sub_agent_error_invalid_arguments() {
        let err = SubAgentError::invalid_arguments("missing prompt field");
        assert_eq!(err.to_string(), "Invalid arguments: missing prompt field");
    }

    #[test]
    fn test_sub_agent_error_execution_failed() {
        let err = SubAgentError::execution_failed("timeout");
        assert_eq!(err.to_string(), "Sub-agent execution failed: timeout");
    }

    #[test]
    fn test_sub_agent_error_tool_access_denied() {
        let err = SubAgentError::tool_access_denied("dangerous_tool");
        assert_eq!(err.to_string(), "Tool access denied: dangerous_tool");
    }

    #[test]
    fn test_sub_agent_config_default() {
        let config = SubAgentConfig::default();
        assert_eq!(config.default_provider, "anthropic");
        assert!(config.default_model.is_none());
        assert!(config.allow_nested_spawn);
        assert_eq!(config.default_budget, Some(50000));
        assert!(config.inherit_system_prompt);
    }

    #[test]
    fn test_sub_agent_config_builder() {
        let config = SubAgentConfig::new()
            .with_default_provider("openai")
            .with_default_model("gpt-4")
            .with_allow_nested_spawn(false)
            .with_max_budget_per_agent(100000)
            .with_default_budget(25000)
            .with_inherit_system_prompt(false);

        assert_eq!(config.default_provider, "openai");
        assert_eq!(config.default_model, Some("gpt-4".to_string()));
        assert!(!config.allow_nested_spawn);
        assert_eq!(config.max_budget_per_agent, Some(100000));
        assert_eq!(config.default_budget, Some(25000));
        assert!(!config.inherit_system_prompt);
    }

    #[test]
    fn test_sub_agent_config_serde_roundtrip() {
        let config = SubAgentConfig::new()
            .with_default_provider("gemini")
            .with_default_model("gemini-pro");

        let json = serde_json::to_string(&config).unwrap();
        let parsed: SubAgentConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.default_provider, "gemini");
        assert_eq!(parsed.default_model, Some("gemini-pro".to_string()));
    }

    #[test]
    fn test_sub_agent_config_serde_defaults() {
        // Empty JSON should use all defaults
        let json = "{}";
        let config: SubAgentConfig = serde_json::from_str(json).unwrap();

        assert_eq!(config.default_provider, "anthropic");
        assert!(config.allow_nested_spawn);
        assert!(config.inherit_system_prompt);
    }

    #[test]
    fn test_sub_agent_error_debug() {
        let err = SubAgentError::not_found("test");
        let debug = format!("{:?}", err);
        assert!(debug.contains("NotFound"));
        assert!(debug.contains("test"));
    }
}
