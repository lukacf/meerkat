//! Configuration for Meerkat
//!
//! Supports layered configuration: defaults → file → env (secrets only) → CLI

use crate::{
    budget::BudgetLimits,
    mcp_config::McpServerConfig,
    retry::RetryPolicy,
    types::{OutputSchema, SecurityMode},
};
use serde::de::Deserializer;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;

/// Complete configuration for Meerkat
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub agent: AgentConfig,
    pub provider: ProviderConfig,
    pub storage: StorageConfig,
    pub budget: BudgetConfig,
    pub retry: RetryConfig,
    pub tools: ToolsConfig,
    // New schema fields for interface consolidation.
    pub models: ModelDefaults,
    pub max_tokens: u32,
    pub shell: ShellDefaults,
    pub store: StoreConfig,
    pub providers: ProviderSettings,
    pub comms: CommsRuntimeConfig,
    pub limits: LimitsConfig,
    pub rest: RestServerConfig,
}

impl Default for Config {
    fn default() -> Self {
        let defaults = template_defaults();
        let agent = AgentConfig::default();
        let max_tokens = defaults
            .max_tokens
            .filter(|value| *value > 0)
            .unwrap_or(agent.max_tokens_per_turn);
        Self {
            agent,
            provider: ProviderConfig::default(),
            storage: StorageConfig::default(),
            budget: BudgetConfig::default(),
            retry: RetryConfig::default(),
            tools: ToolsConfig::default(),
            models: ModelDefaults::default(),
            max_tokens,
            shell: ShellDefaults::default(),
            store: StoreConfig::default(),
            providers: ProviderSettings::default(),
            comms: CommsRuntimeConfig::default(),
            limits: LimitsConfig::default(),
            rest: RestServerConfig::default(),
        }
    }
}

impl Config {
    /// Return the config template as TOML.
    pub fn template_toml() -> &'static str {
        CONFIG_TEMPLATE_TOML
    }

    /// Parse the config template into a Config value.
    pub fn template() -> Result<Self, ConfigError> {
        toml::from_str(CONFIG_TEMPLATE_TOML).map_err(ConfigError::Parse)
    }

    /// Load configuration from all sources with proper layering
    /// Order: defaults → project config OR global config → env vars (secrets only)
    /// → CLI (CLI applied separately)
    pub async fn load() -> Result<Self, ConfigError> {
        let cwd = std::env::current_dir()?;
        let home = dirs::home_dir();
        Self::load_from_with_env(&cwd, home.as_deref(), |key| std::env::var(key).ok()).await
    }

    /// Load config like [`Config::load`], but with explicit start directory, home directory,
    /// and environment variable provider.
    ///
    /// This exists primarily to make tests deterministic without mutating the process-wide
    /// environment (which is unsafe in multi-threaded programs on Unix).
    #[doc(hidden)]
    pub async fn load_from_with_env<F>(
        start_dir: &std::path::Path,
        home_dir: Option<&std::path::Path>,
        env: F,
    ) -> Result<Self, ConfigError>
    where
        F: FnMut(&str) -> Option<String>,
    {
        let mut config = Self::default();

        // 1. Load project config (if exists). Local config replaces global.
        if let Some(path) = Self::find_project_config_from(start_dir).await {
            config.merge_file(&path).await?;
        } else if let Some(path) = home_dir.map(|home| home.join(".rkat/config.toml")) {
            if tokio::fs::try_exists(&path).await.unwrap_or(false) {
                config.merge_file(&path).await?;
            }
        }

        // 2. Apply environment variable overrides (secrets only)
        config.apply_env_overrides_from(env)?;

        Ok(config)
    }

    /// Load config like [`Config::load`], but with explicit start directory and home directory.
    #[doc(hidden)]
    pub async fn load_from(
        start_dir: &std::path::Path,
        home_dir: Option<&std::path::Path>,
    ) -> Result<Self, ConfigError> {
        Self::load_from_with_env(start_dir, home_dir, |key| std::env::var(key).ok()).await
    }

    /// Convert config limits into runtime budget limits.
    pub fn budget_limits(&self) -> BudgetLimits {
        self.limits.to_budget_limits()
    }

    /// Get global config path (~/.rkat/config.toml)
    pub fn global_config_path() -> Option<PathBuf> {
        dirs::home_dir().map(|h| h.join(".rkat/config.toml"))
    }

    /// Find project config by walking up directories (.rkat/config.toml)
    ///
    /// Only returns a path if both `.rkat/` directory AND `config.toml` exist.
    /// This allows `.rkat/` to be created for session storage without requiring
    /// a config file.
    async fn find_project_config_from(start_dir: &std::path::Path) -> Option<PathBuf> {
        let mut current = start_dir.to_path_buf();
        loop {
            let marker_dir = current.join(".rkat");
            let config_path = marker_dir.join("config.toml");

            // Only treat as project config if config.toml actually exists
            let config_exists = tokio::fs::try_exists(&config_path).await.unwrap_or(false);
            if config_exists {
                return Some(config_path);
            }

            if !current.pop() {
                return None;
            }
        }
    }

    /// Merge configuration from a TOML file
    pub async fn merge_file(&mut self, path: &PathBuf) -> Result<(), ConfigError> {
        let content = tokio::fs::read_to_string(path).await?;

        let file_config: Config = toml::from_str(&content).map_err(ConfigError::Parse)?;

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
        if other.agent.tool_instructions.is_some() {
            self.agent.tool_instructions = other.agent.tool_instructions;
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

        // New schema fields (replace if non-default)
        if other.models != ModelDefaults::default() {
            self.models = other.models;
        }
        if other.max_tokens != Config::default().max_tokens {
            self.max_tokens = other.max_tokens;
        }
        if other.shell != ShellDefaults::default() {
            self.shell = other.shell;
        }
        if other.store != StoreConfig::default() {
            self.store = other.store;
        }
        if other.providers != ProviderSettings::default() {
            self.providers = other.providers;
        }
        if other.comms != CommsRuntimeConfig::default() {
            self.comms = other.comms;
        }
        if other.limits != LimitsConfig::default() {
            self.limits = other.limits;
        }
        if other.rest != RestServerConfig::default() {
            self.rest = other.rest;
        }
    }

    /// Apply environment variable overrides
    pub fn apply_env_overrides(&mut self) -> Result<(), ConfigError> {
        self.apply_env_overrides_from(|key| std::env::var(key).ok())
    }

    /// Apply environment variable overrides (secrets only) using an explicit env provider.
    ///
    /// This exists primarily to make tests deterministic without mutating the process-wide
    /// environment (which is unsafe in multi-threaded programs on Unix).
    #[doc(hidden)]
    pub fn apply_env_overrides_from<F>(&mut self, mut env: F) -> Result<(), ConfigError>
    where
        F: FnMut(&str) -> Option<String>,
    {
        // Provider API keys (fallback if not set in config). Only secret env vars are honored.
        match &mut self.provider {
            ProviderConfig::Anthropic { api_key, .. } => {
                if api_key.is_none() {
                    let key = env("RKAT_ANTHROPIC_API_KEY").or_else(|| env("ANTHROPIC_API_KEY"));
                    if let Some(key) = key {
                        *api_key = Some(key);
                    }
                }
            }
            ProviderConfig::OpenAI { api_key, .. } => {
                if api_key.is_none() {
                    let key = env("RKAT_OPENAI_API_KEY").or_else(|| env("OPENAI_API_KEY"));
                    if let Some(key) = key {
                        *api_key = Some(key);
                    }
                }
            }
            ProviderConfig::Gemini { api_key } => {
                if api_key.is_none() {
                    // Try GEMINI_API_KEY first, then GOOGLE_API_KEY
                    let key = env("RKAT_GEMINI_API_KEY")
                        .or_else(|| env("GEMINI_API_KEY"))
                        .or_else(|| env("GOOGLE_API_KEY"));
                    if let Some(key) = key {
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
        // Apply JSON merge patch override if present
        if let Some(delta) = cli.override_config {
            let mut value = serde_json::to_value(&self).unwrap_or_default();
            crate::config_store::merge_patch(&mut value, delta.0);
            if let Ok(updated) = serde_json::from_value(value) {
                *self = updated;
            }
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
    /// Arbitrary configuration overrides via JSON merge patch
    pub override_config: Option<ConfigDelta>,
}

fn default_structured_output_retries() -> u32 {
    2
}

/// Agent behavior configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentConfig {
    /// System prompt to prepend
    pub system_prompt: Option<String>,
    /// Path to system prompt file (alternative to inline system_prompt)
    pub system_prompt_file: Option<PathBuf>,
    /// Optional tool usage instructions appended to system prompt
    pub tool_instructions: Option<String>,
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
    /// Provider-specific parameters (e.g., thinking config, reasoning effort)
    ///
    /// This is a generic JSON bag that providers can extract provider-specific
    /// options from. Each provider implementation is responsible for reading
    /// and applying relevant parameters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_params: Option<serde_json::Value>,
    /// Output schema for structured output extraction.
    ///
    /// When set, the agent will perform an extraction turn after completing
    /// the agentic work, forcing the LLM to output validated JSON. The final
    /// response text becomes the schema-only JSON string.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<OutputSchema>,
    /// Maximum retries for structured output validation failures.
    #[serde(default = "default_structured_output_retries")]
    pub structured_output_retries: u32,
}

impl Default for AgentConfig {
    fn default() -> Self {
        let defaults = template_defaults();
        let agent = defaults.agent.as_ref();
        Self {
            system_prompt: None,
            system_prompt_file: None,
            tool_instructions: None,
            model: agent.and_then(|cfg| cfg.model.clone()).unwrap_or_default(),
            max_tokens_per_turn: agent
                .and_then(|cfg| cfg.max_tokens_per_turn)
                .unwrap_or_default(),
            temperature: None,
            checkpoint_interval: agent.and_then(|cfg| cfg.checkpoint_interval),
            budget_warning_threshold: agent
                .and_then(|cfg| cfg.budget_warning_threshold)
                .unwrap_or_default(),
            max_turns: None,
            provider_params: None,
            output_schema: None,
            structured_output_retries: default_structured_output_retries(),
        }
    }
}

/// Model defaults by provider.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ModelDefaults {
    pub anthropic: String,
    pub openai: String,
    pub gemini: String,
}

impl Default for ModelDefaults {
    fn default() -> Self {
        let defaults = template_defaults();
        let models = defaults.models.as_ref();
        Self {
            anthropic: models
                .and_then(|cfg| cfg.anthropic.clone())
                .unwrap_or_default(),
            openai: models
                .and_then(|cfg| cfg.openai.clone())
                .unwrap_or_default(),
            gemini: models
                .and_then(|cfg| cfg.gemini.clone())
                .unwrap_or_default(),
        }
    }
}

/// Default shell program
pub const DEFAULT_SHELL_PROGRAM: &str = "nu";
/// Default shell timeout in seconds
pub const DEFAULT_SHELL_TIMEOUT_SECS: u64 = 30;
/// Default shell security mode
pub const DEFAULT_SHELL_SECURITY_MODE: SecurityMode = SecurityMode::Unrestricted;

/// Shell defaults configured at the config layer.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(default)]
pub struct ShellDefaults {
    pub program: String,
    pub timeout_secs: u64,
    /// Security mode: unrestricted, allow_list, deny_list
    pub security_mode: SecurityMode,
    /// Patterns for allow/deny lists (glob format)
    pub security_patterns: Vec<String>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct ShellDefaultsSeed {
    program: Option<String>,
    timeout_secs: Option<u64>,
    security_mode: Option<SecurityMode>,
    security_patterns: Option<Vec<String>>,
    #[serde(alias = "allowlist")]
    allowlist: Option<Vec<String>>,
}

impl<'de> Deserialize<'de> for ShellDefaults {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let seed = ShellDefaultsSeed::deserialize(deserializer)?;
        let mut defaults = ShellDefaults::default();

        if let Some(program) = seed.program {
            defaults.program = program;
        }
        if let Some(timeout_secs) = seed.timeout_secs {
            defaults.timeout_secs = timeout_secs;
        }
        if let Some(security_mode) = seed.security_mode {
            defaults.security_mode = security_mode;
        }
        if let Some(security_patterns) = seed.security_patterns.or(seed.allowlist.clone()) {
            defaults.security_patterns = security_patterns;
        }

        if seed.security_mode.is_none() && seed.allowlist.is_some() {
            defaults.security_mode = SecurityMode::AllowList;
        }

        Ok(defaults)
    }
}

impl Default for ShellDefaults {
    fn default() -> Self {
        let defaults = template_defaults();
        let shell = defaults.shell.as_ref();
        Self {
            program: shell
                .and_then(|cfg| cfg.program.clone())
                .unwrap_or_else(|| DEFAULT_SHELL_PROGRAM.to_string()),
            timeout_secs: shell
                .and_then(|cfg| cfg.timeout_secs)
                .unwrap_or(DEFAULT_SHELL_TIMEOUT_SECS),
            security_mode: shell
                .and_then(|cfg| cfg.security_mode)
                .unwrap_or(DEFAULT_SHELL_SECURITY_MODE),
            security_patterns: shell
                .and_then(|cfg| cfg.security_patterns.clone())
                .unwrap_or_default(),
        }
    }
}

const CONFIG_TEMPLATE_TOML: &str = include_str!("config_template.toml");

#[derive(Debug, Deserialize)]
struct TemplateAgentDefaults {
    model: Option<String>,
    max_tokens_per_turn: Option<u32>,
    checkpoint_interval: Option<u32>,
    budget_warning_threshold: Option<f32>,
}

#[derive(Debug, Deserialize)]
struct TemplateModelDefaults {
    anthropic: Option<String>,
    openai: Option<String>,
    gemini: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TemplateShellDefaults {
    program: Option<String>,
    timeout_secs: Option<u64>,
    security_mode: Option<SecurityMode>,
    security_patterns: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct TemplateDefaults {
    agent: Option<TemplateAgentDefaults>,
    models: Option<TemplateModelDefaults>,
    shell: Option<TemplateShellDefaults>,
    max_tokens: Option<u32>,
}

impl TemplateDefaults {
    fn empty() -> Self {
        Self {
            agent: None,
            models: None,
            shell: None,
            max_tokens: None,
        }
    }
}

fn template_defaults() -> &'static TemplateDefaults {
    static DEFAULTS: OnceLock<TemplateDefaults> = OnceLock::new();
    DEFAULTS.get_or_init(|| {
        toml::from_str(CONFIG_TEMPLATE_TOML).unwrap_or_else(|e| {
            // This is an embedded resource, so it really shouldn't fail in production
            // but we avoid panic! to satisfy lint policies.
            tracing::error!("Invalid config template defaults: {}", e);
            TemplateDefaults::empty()
        })
    })
}

/// Paths for session and task persistence.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct StoreConfig {
    pub sessions_path: Option<PathBuf>,
    pub tasks_path: Option<PathBuf>,
}

/// Provider settings sourced from config.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct ProviderSettings {
    pub base_urls: Option<HashMap<String, String>>,
    pub api_keys: Option<HashMap<String, String>>,
}

/// Runtime limits configured at the config layer.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct LimitsConfig {
    pub budget: Option<u64>,
    #[serde(with = "optional_duration_serde")]
    pub max_duration: Option<Duration>,
}

impl LimitsConfig {
    pub fn to_budget_limits(&self) -> BudgetLimits {
        BudgetLimits {
            max_tokens: self.budget,
            max_duration: self.max_duration,
            max_tool_calls: None,
        }
    }
}

/// REST server configuration sourced from config.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct RestServerConfig {
    pub host: String,
    pub port: u16,
}

impl Default for RestServerConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 8080,
        }
    }
}

/// Runtime comms configuration (portable across interfaces).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct CommsRuntimeConfig {
    pub mode: CommsRuntimeMode,
    pub address: Option<String>,
    pub auto_enable_for_subagents: bool,
}

impl Default for CommsRuntimeConfig {
    fn default() -> Self {
        Self {
            mode: CommsRuntimeMode::Inproc,
            address: None,
            auto_enable_for_subagents: false,
        }
    }
}

/// Transport mode for comms runtime.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum CommsRuntimeMode {
    Inproc,
    Tcp,
    Uds,
}

impl Default for CommsRuntimeMode {
    fn default() -> Self {
        Self::Inproc
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
            directory: data_dir().map(|d| d.join("sessions")),
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
    /// Builtin tools enabled
    pub builtins_enabled: bool,
    /// Shell tools enabled
    pub shell_enabled: bool,
    /// Comms tools enabled
    pub comms_enabled: bool,
    /// Sub-agent tools enabled
    pub subagents_enabled: bool,
}

impl Default for ToolsConfig {
    fn default() -> Self {
        Self {
            mcp_servers: Vec::new(),
            default_timeout: Duration::from_secs(600),
            tool_timeouts: HashMap::new(),
            max_concurrent: 10,
            builtins_enabled: false,
            shell_enabled: false,
            comms_enabled: false,
            subagents_enabled: false,
        }
    }
}

/// Config scope for persisted settings.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConfigScope {
    Global,
    Project,
}

/// Config patch payload (merge-patch semantics applied by ConfigStore).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(transparent)]
pub struct ConfigDelta(pub serde_json::Value);

/// Configuration errors
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Parse error: {0}")]
    Parse(#[from] toml::de::Error),

    #[error("TOML serialization error: {0}")]
    TomlSerialize(#[from] toml::ser::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("UTF-8 error: {0}")]
    Utf8(#[from] std::string::FromUtf8Error),

    #[allow(dead_code)]
    #[error("Invalid value for {0}")]
    InvalidValue(String),

    #[error("Missing required field: {0}")]
    MissingField(String),

    #[error("Missing API key: {0}")]
    MissingApiKey(&'static str),

    #[error("Internal error: {0}")]
    InternalError(String),
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
                let millis = n
                    .as_u64()
                    .ok_or_else(|| D::Error::custom("invalid number"))?;
                Ok(Some(Duration::from_millis(millis)))
            }
            _ => Err(D::Error::custom("expected string or number for duration")),
        }
    }
}

/// Find the project root directory by walking up from `start_dir` looking for `.rkat/`.
pub fn find_project_root(start_dir: &std::path::Path) -> Option<PathBuf> {
    let mut current = start_dir.to_path_buf();
    loop {
        if current.join(".rkat").is_dir() {
            return Some(current);
        }
        if !current.pop() {
            return None;
        }
    }
}

/// Get the data directory for Meerkat.
///
/// Priority:
/// 1. Nearest ancestor containing .rkat/
/// 2. User's home directory ~/.rkat/
pub fn data_dir() -> Option<PathBuf> {
    // 1. Check for project root .rkat
    if let Ok(cwd) = std::env::current_dir() {
        if let Some(root) = find_project_root(&cwd) {
            return Some(root.join(".rkat"));
        }
    }

    // 2. Fallback to ~/.rkat
    dirs::home_dir().map(|h| h.join(".rkat"))
}

// Stub for home directory resolution
pub mod dirs {
    use std::path::PathBuf;

    pub fn home_dir() -> Option<PathBuf> {
        std::env::var_os("HOME").map(PathBuf::from)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn test_config_default() {
        let config = Config::default();
        assert_eq!(config.agent.model, "claude-3-7-sonnet-20250219");
        assert_eq!(config.agent.max_tokens_per_turn, 8192);
        assert_eq!(config.retry.max_retries, 3);
    }

    #[test]
    fn test_config_layering() {
        // 1. Test defaults
        let config = Config::default();
        assert_eq!(config.agent.model, "claude-3-7-sonnet-20250219");
        assert_eq!(config.budget.max_tokens, None);

        // 2. Test env override (secrets only)
        {
            let env = std::collections::HashMap::from([
                ("RKAT_MODEL".to_string(), "env-model".to_string()),
                ("ANTHROPIC_API_KEY".to_string(), "secret-key".to_string()),
            ]);
            let mut config = Config::default();
            config
                .apply_env_overrides_from(|key| env.get(key).cloned())
                .expect("apply env overrides");
            match config.provider {
                ProviderConfig::Anthropic { api_key, .. } => {
                    assert_eq!(api_key.as_deref(), Some("secret-key"));
                }
                _ => unreachable!("expected anthropic provider"),
            }
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
        config.apply_cli_overrides(CliOverrides {
            model: Some("cli-model".to_string()),
            max_tokens: Some(50000),
            ..Default::default()
        });
        // CLI should win over defaults
        assert_eq!(config.agent.model, "cli-model");
        assert_eq!(config.budget.max_tokens, Some(50000));
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

    /// Regression test: Config::load() should succeed when .rkat/ directory exists
    /// but config.toml is missing. This is a common first-run state where .rkat/
    /// is created for session storage.
    #[tokio::test]
    async fn test_regression_load_succeeds_without_config_toml() {
        use tempfile::TempDir;

        // Create temp dir with .rkat/ but NO config.toml
        let temp_dir = TempDir::new().unwrap();
        let rkat_dir = temp_dir.path().join(".rkat");
        std::fs::create_dir(&rkat_dir).unwrap();

        // Verify .rkat exists but config.toml doesn't
        assert!(rkat_dir.exists());
        assert!(!rkat_dir.join("config.toml").exists());

        // Verify load succeeds without consulting process-global HOME/cwd.
        let result =
            Config::load_from_with_env(temp_dir.path(), Some(temp_dir.path()), |_| None).await;

        assert!(
            result.is_ok(),
            "Config::load() should succeed when .rkat/ exists without config.toml: {:?}",
            result.err()
        );
    }
}
