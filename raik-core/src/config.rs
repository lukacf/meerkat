//! Configuration for RAIK
//!
//! Supports layered configuration: defaults → env → file → CLI

use crate::{mcp_config::McpServerConfig, retry::RetryPolicy};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

/// Complete configuration for RAIK
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub agent: AgentConfig,
    pub provider: ProviderConfig,
    pub storage: StorageConfig,
    pub budget: BudgetConfig,
    pub retry: RetryConfig,
    pub tools: ToolsConfig,
}


impl Config {
    /// Load configuration from all sources with proper layering
    /// Order: defaults → user config → project config → env vars → CLI (CLI applied separately)
    pub fn load() -> Result<Self, ConfigError> {
        let mut config = Self::default();

        // 1. Load user config (if exists)
        if let Some(path) = Self::user_config_path() {
            if path.exists() {
                config.merge_file(&path)?;
            }
        }

        // 2. Load project config (if exists) - overrides user config
        if let Some(path) = Self::find_project_config() {
            config.merge_file(&path)?;
        }

        // 3. Apply environment variable overrides
        config.apply_env_overrides()?;

        Ok(config)
    }

    /// Get user config path (~/.config/raik/config.toml)
    fn user_config_path() -> Option<PathBuf> {
        dirs::home_dir().map(|h| h.join(".config/raik/config.toml"))
    }

    /// Find project config by walking up directories (.raik/config.toml)
    fn find_project_config() -> Option<PathBuf> {
        let cwd = std::env::current_dir().ok()?;
        let mut dir = cwd.as_path();
        loop {
            let project_config = dir.join(".raik/config.toml");
            if project_config.exists() {
                return Some(project_config);
            }
            // Also check for raik.toml in current directory
            if dir == cwd.as_path() {
                let local = dir.join("raik.toml");
                if local.exists() {
                    return Some(local);
                }
            }
            match dir.parent() {
                Some(parent) => dir = parent,
                None => break,
            }
        }
        None
    }

    /// Merge configuration from a TOML file
    pub fn merge_file(&mut self, path: &PathBuf) -> Result<(), ConfigError> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| ConfigError::IoError(e.to_string()))?;

        let file_config: Config = toml::from_str(&content)
            .map_err(|e| ConfigError::ParseError(e.to_string()))?;

        // Merge (file values override defaults)
        self.merge(file_config);

        Ok(())
    }

    /// Merge another config into this one
    fn merge(&mut self, other: Config) {
        // Agent config
        if other.agent.system_prompt.is_some() {
            self.agent.system_prompt = other.agent.system_prompt;
        }
        if other.agent.model != AgentConfig::default().model {
            self.agent.model = other.agent.model;
        }
        if other.agent.max_tokens_per_turn != AgentConfig::default().max_tokens_per_turn {
            self.agent.max_tokens_per_turn = other.agent.max_tokens_per_turn;
        }

        // Provider config
        self.provider = other.provider;

        // Storage config
        if other.storage.directory.is_some() {
            self.storage.directory = other.storage.directory;
        }

        // Budget config
        if other.budget.max_tokens.is_some() {
            self.budget.max_tokens = other.budget.max_tokens;
        }
        if other.budget.max_duration.is_some() {
            self.budget.max_duration = other.budget.max_duration;
        }
        if other.budget.max_tool_calls.is_some() {
            self.budget.max_tool_calls = other.budget.max_tool_calls;
        }
    }

    /// Apply environment variable overrides
    pub fn apply_env_overrides(&mut self) -> Result<(), ConfigError> {
        // RAIK_MODEL
        if let Ok(model) = std::env::var("RAIK_MODEL") {
            self.agent.model = model;
        }

        // RAIK_MAX_TOKENS
        if let Ok(tokens) = std::env::var("RAIK_MAX_TOKENS") {
            self.budget.max_tokens = Some(
                tokens
                    .parse()
                    .map_err(|_| ConfigError::InvalidValue("RAIK_MAX_TOKENS".to_string()))?,
            );
        }

        // RAIK_MAX_DURATION - supports humantime format (e.g., "5m", "1h30m", "300")
        if let Ok(duration_str) = std::env::var("RAIK_MAX_DURATION") {
            let duration = parse_duration(&duration_str)
                .map_err(|_| ConfigError::InvalidValue("RAIK_MAX_DURATION".to_string()))?;
            self.budget.max_duration = Some(duration);
        }

        // RAIK_STORAGE_DIR
        if let Ok(dir) = std::env::var("RAIK_STORAGE_DIR") {
            self.storage.directory = Some(PathBuf::from(dir));
        }

        // Provider API keys (fallback if not set in config)
        match &mut self.provider {
            ProviderConfig::Anthropic { api_key, .. } => {
                if api_key.is_none() {
                    if let Ok(key) = std::env::var("ANTHROPIC_API_KEY") {
                        *api_key = Some(key);
                    }
                }
            }
            ProviderConfig::OpenAI { api_key, .. } => {
                if api_key.is_none() {
                    if let Ok(key) = std::env::var("OPENAI_API_KEY") {
                        *api_key = Some(key);
                    }
                }
            }
            ProviderConfig::Gemini { api_key } => {
                if api_key.is_none() {
                    // Try GEMINI_API_KEY first, then GOOGLE_API_KEY
                    if let Ok(key) = std::env::var("GEMINI_API_KEY")
                        .or_else(|_| std::env::var("GOOGLE_API_KEY"))
                    {
                        *api_key = Some(key);
                    }
                }
            }
        }

        Ok(())
    }

    /// Apply CLI argument overrides
    pub fn apply_cli_overrides(&mut self, cli: CliOverrides) {
        if let Some(model) = cli.model {
            self.agent.model = model;
        }
        if let Some(tokens) = cli.max_tokens {
            self.budget.max_tokens = Some(tokens);
        }
        if let Some(duration) = cli.max_duration {
            self.budget.max_duration = Some(duration);
        }
        if let Some(calls) = cli.max_tool_calls {
            self.budget.max_tool_calls = Some(calls);
        }
    }
}

/// CLI argument overrides
#[derive(Debug, Clone, Default)]
pub struct CliOverrides {
    pub model: Option<String>,
    pub max_tokens: Option<u64>,
    pub max_duration: Option<Duration>,
    pub max_tool_calls: Option<usize>,
}

/// Agent behavior configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentConfig {
    /// System prompt to prepend
    pub system_prompt: Option<String>,
    /// Path to system prompt file (alternative to inline system_prompt)
    pub system_prompt_file: Option<PathBuf>,
    /// Model identifier (provider-specific)
    pub model: String,
    /// Maximum tokens to generate per turn
    pub max_tokens_per_turn: u32,
    /// Temperature for sampling
    pub temperature: Option<f32>,
    /// Checkpoint after this many turns
    pub checkpoint_interval: Option<u32>,
    /// Warning threshold for budget (0.0-1.0)
    pub budget_warning_threshold: f32,
    /// Maximum turns before forced stop
    pub max_turns: Option<u32>,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            system_prompt: None,
            system_prompt_file: None,
            model: "claude-sonnet-4-20250514".to_string(),
            max_tokens_per_turn: 8192,
            temperature: None,
            checkpoint_interval: Some(5),
            budget_warning_threshold: 0.8,
            max_turns: None,
        }
    }
}

/// LLM provider configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProviderConfig {
    Anthropic {
        api_key: Option<String>,
        base_url: Option<String>,
    },
    #[serde(rename = "openai")]
    OpenAI {
        api_key: Option<String>,
        base_url: Option<String>,
    },
    Gemini {
        api_key: Option<String>,
    },
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self::Anthropic {
            api_key: None,
            base_url: None,
        }
    }
}

/// Storage configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct StorageConfig {
    /// Storage backend type
    pub backend: StorageBackend,
    /// Directory for file-based storage
    pub directory: Option<PathBuf>,
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            backend: StorageBackend::Jsonl,
            directory: dirs::data_dir().map(|d| d.join("raik/sessions")),
        }
    }
}

/// Available storage backends
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum StorageBackend {
    #[default]
    Jsonl,
    Memory,
}

/// Budget configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct BudgetConfig {
    /// Maximum tokens to consume
    pub max_tokens: Option<u64>,
    /// Maximum duration
    #[serde(with = "optional_duration_serde")]
    pub max_duration: Option<Duration>,
    /// Maximum tool calls
    pub max_tool_calls: Option<usize>,
}

/// Retry configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RetryConfig {
    /// Maximum number of retry attempts
    pub max_retries: u32,
    /// Initial delay before first retry (supports humantime format: "500ms", "1s")
    #[serde(with = "humantime_serde")]
    pub initial_delay: Duration,
    /// Maximum delay between retries (supports humantime format: "30s", "1m")
    #[serde(with = "humantime_serde")]
    pub max_delay: Duration,
    /// Multiplier for exponential backoff
    pub multiplier: f64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        let policy = RetryPolicy::default();
        Self {
            max_retries: policy.max_retries,
            initial_delay: policy.initial_delay,
            max_delay: policy.max_delay,
            multiplier: policy.multiplier,
        }
    }
}

impl From<RetryConfig> for RetryPolicy {
    fn from(config: RetryConfig) -> Self {
        RetryPolicy {
            max_retries: config.max_retries,
            initial_delay: config.initial_delay,
            max_delay: config.max_delay,
            multiplier: config.multiplier,
        }
    }
}

/// Tools configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ToolsConfig {
    /// MCP server configurations
    #[serde(default)]
    pub mcp_servers: Vec<McpServerConfig>,
    /// Default timeout for tool execution (supports humantime format: "30s", "1m")
    #[serde(with = "humantime_serde")]
    pub default_timeout: Duration,
    /// Per-tool timeout overrides (supports humantime format: "30s", "1m")
    #[serde(default)]
    pub tool_timeouts: HashMap<String, Duration>,
    /// Maximum concurrent tool executions
    pub max_concurrent: usize,
}

impl Default for ToolsConfig {
    fn default() -> Self {
        Self {
            mcp_servers: Vec::new(),
            default_timeout: Duration::from_secs(600),
            tool_timeouts: HashMap::new(),
            max_concurrent: 10,
        }
    }
}

/// Configuration errors
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("IO error: {0}")]
    IoError(String),

    #[error("Parse error: {0}")]
    ParseError(String),

    #[error("Invalid value for {0}")]
    InvalidValue(String),

    #[error("Missing required field: {0}")]
    MissingField(String),

    #[error("Missing API key: {0}")]
    MissingApiKey(&'static str),
}

/// Parse a duration from a human-readable string (e.g., "30s", "5m", "1h30m")
/// Also accepts plain seconds as a number
fn parse_duration(s: &str) -> Result<Duration, String> {
    // First try parsing as humantime
    if let Ok(d) = humantime_serde::re::humantime::parse_duration(s) {
        return Ok(d);
    }
    // Fall back to parsing as plain seconds
    if let Ok(secs) = s.parse::<u64>() {
        return Ok(Duration::from_secs(secs));
    }
    Err(format!("Invalid duration: {}", s))
}

/// Serde helpers for Option<Duration> with humantime format
mod optional_duration_serde {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::time::Duration;

    pub fn serialize<S>(duration: &Option<Duration>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match duration {
            Some(d) => {
                let s = humantime_serde::re::humantime::format_duration(*d).to_string();
                s.serialize(serializer)
            }
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<Duration>, D::Error>
    where
        D: Deserializer<'de>,
    {
        use serde::de::Error;

        // Try deserializing as string first (humantime format)
        let value: Option<serde_json::Value> = Option::deserialize(deserializer)?;
        match value {
            None => Ok(None),
            Some(serde_json::Value::String(s)) => {
                humantime_serde::re::humantime::parse_duration(&s)
                    .map(Some)
                    .map_err(|e| D::Error::custom(e.to_string()))
            }
            Some(serde_json::Value::Number(n)) => {
                // Support milliseconds as number for backward compat
                let millis = n.as_u64().ok_or_else(|| D::Error::custom("invalid number"))?;
                Ok(Some(Duration::from_millis(millis)))
            }
            _ => Err(D::Error::custom("expected string or number for duration")),
        }
    }
}

// Stub for dirs crate functionality
mod dirs {
    use std::path::PathBuf;

    pub fn home_dir() -> Option<PathBuf> {
        std::env::var_os("HOME").map(PathBuf::from)
    }

    pub fn data_dir() -> Option<PathBuf> {
        // XDG_DATA_HOME or ~/.local/share
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| home_dir().map(|h| h.join(".local/share")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_default() {
        let config = Config::default();
        assert_eq!(config.agent.model, "claude-sonnet-4-20250514");
        assert_eq!(config.agent.max_tokens_per_turn, 8192);
        assert_eq!(config.retry.max_retries, 3);
    }

    #[test]
    fn test_config_layering() {
        // Test that defaults + env + file + CLI all layer correctly
        // Precedence: defaults < env < file < CLI

        // 1. Test defaults
        let config = Config::default();
        assert_eq!(config.agent.model, "claude-sonnet-4-20250514");
        assert_eq!(config.budget.max_tokens, None);

        // 2. Test env override
        // SAFETY: Test runs single-threaded, env vars are cleaned up
        unsafe {
            std::env::set_var("RAIK_MODEL", "env-model");
            std::env::set_var("RAIK_MAX_TOKENS", "10000");
        }
        let mut config = Config::default();
        config.apply_env_overrides().unwrap();
        assert_eq!(config.agent.model, "env-model");
        assert_eq!(config.budget.max_tokens, Some(10000));
        // SAFETY: Test cleanup
        unsafe {
            std::env::remove_var("RAIK_MODEL");
            std::env::remove_var("RAIK_MAX_TOKENS");
        }

        // 3. Test file merge
        let mut config = Config::default();
        let file_config = Config {
            agent: AgentConfig {
                model: "file-model".to_string(),
                ..Default::default()
            },
            ..Default::default()
        };
        config.merge(file_config);
        assert_eq!(config.agent.model, "file-model");

        // 4. Test CLI override (highest precedence)
        let mut config = Config::default();
        // SAFETY: Test runs single-threaded
        unsafe { std::env::set_var("RAIK_MODEL", "env-model"); }
        config.apply_env_overrides().unwrap();
        config.apply_cli_overrides(CliOverrides {
            model: Some("cli-model".to_string()),
            max_tokens: Some(50000),
            ..Default::default()
        });
        // CLI should win over env
        assert_eq!(config.agent.model, "cli-model");
        assert_eq!(config.budget.max_tokens, Some(50000));
        // SAFETY: Test cleanup
        unsafe { std::env::remove_var("RAIK_MODEL"); }

        // 5. Test full layering: defaults → env → file → CLI
        let mut config = Config::default();
        // Apply env
        // SAFETY: Test runs single-threaded
        unsafe { std::env::set_var("RAIK_MAX_DURATION", "600"); }
        config.apply_env_overrides().unwrap();
        // SAFETY: Test cleanup
        unsafe { std::env::remove_var("RAIK_MAX_DURATION"); }
        // Apply file (merge)
        let file_config = Config {
            budget: BudgetConfig {
                max_tool_calls: Some(100),
                ..Default::default()
            },
            ..Default::default()
        };
        config.merge(file_config);
        // Apply CLI
        config.apply_cli_overrides(CliOverrides {
            model: Some("final-model".to_string()),
            ..Default::default()
        });
        // Verify all layers applied
        assert_eq!(config.agent.model, "final-model"); // CLI
        assert_eq!(config.budget.max_duration, Some(Duration::from_secs(600))); // env
        assert_eq!(config.budget.max_tool_calls, Some(100)); // file
    }

    #[test]
    fn test_provider_config_serialization() {
        let anthropic = ProviderConfig::Anthropic {
            api_key: Some("sk-test".to_string()),
            base_url: None,
        };

        let json = serde_json::to_value(&anthropic).unwrap();
        assert_eq!(json["type"], "anthropic");
        assert_eq!(json["api_key"], "sk-test");

        let openai = ProviderConfig::OpenAI {
            api_key: Some("sk-openai".to_string()),
            base_url: Some("https://custom.openai.com".to_string()),
        };

        let json = serde_json::to_value(&openai).unwrap();
        assert_eq!(json["type"], "openai");

        let gemini = ProviderConfig::Gemini {
            api_key: Some("gemini-key".to_string()),
        };

        let json = serde_json::to_value(&gemini).unwrap();
        assert_eq!(json["type"], "gemini");
    }

    #[test]
    fn test_budget_config_serialization() {
        let budget = BudgetConfig {
            max_tokens: Some(100000),
            max_duration: Some(Duration::from_secs(300)),
            max_tool_calls: Some(50),
        };

        let json = serde_json::to_string(&budget).unwrap();
        let parsed: BudgetConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.max_tokens, Some(100000));
        assert_eq!(parsed.max_duration, Some(Duration::from_secs(300)));
        assert_eq!(parsed.max_tool_calls, Some(50));
    }

    #[test]
    fn test_retry_config_to_policy() {
        let config = RetryConfig::default();
        let policy: RetryPolicy = config.into();

        assert_eq!(policy.max_retries, 3);
        assert_eq!(policy.initial_delay, Duration::from_millis(500));
    }
}
