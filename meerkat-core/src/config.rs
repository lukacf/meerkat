//! Configuration for Meerkat
//!
//! Supports layered configuration: defaults → file → env (secrets only) → CLI

use crate::{
    budget::BudgetLimits,
    hooks::{HookCapability, HookExecutionMode, HookFailurePolicy, HookId, HookPoint},
    mcp_config::McpServerConfig,
    provider::Provider,
    retry::RetryPolicy,
    types::{OutputSchema, SecurityMode},
};
use schemars::JsonSchema;
use serde::de::Deserializer;
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use serde_json::{Map, Value};
use std::collections::{BTreeMap, HashMap};
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
    pub sub_agents: SubAgentsConfig,
    pub hooks: HooksConfig,
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
            sub_agents: SubAgentsConfig::default(),
            hooks: HooksConfig::default(),
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

    /// Load only hook configuration with explicit global -> project layering.
    ///
    /// This preserves existing config precedence for non-hook fields while allowing
    /// deterministic hook registration ordering across scopes.
    pub async fn load_layered_hooks() -> Result<HooksConfig, ConfigError> {
        let cwd = std::env::current_dir()?;
        let home = dirs::home_dir();
        Self::load_layered_hooks_from(&cwd, home.as_deref()).await
    }

    /// Load only hook configuration with explicit global -> project layering.
    pub async fn load_layered_hooks_from(
        start_dir: &std::path::Path,
        home_dir: Option<&std::path::Path>,
    ) -> Result<HooksConfig, ConfigError> {
        let mut hooks = HooksConfig::default();

        if let Some(global_path) = home_dir.map(|home| home.join(".rkat/config.toml")) {
            if tokio::fs::try_exists(&global_path).await.unwrap_or(false) {
                let content = tokio::fs::read_to_string(&global_path).await?;
                let cfg: Config = toml::from_str(&content).map_err(ConfigError::Parse)?;
                hooks.append_entries_from(&cfg.hooks);
            }
        }

        if let Some(project_path) = Self::find_project_config_from(start_dir).await {
            if tokio::fs::try_exists(&project_path).await.unwrap_or(false) {
                let content = tokio::fs::read_to_string(&project_path).await?;
                let cfg: Config = toml::from_str(&content).map_err(ConfigError::Parse)?;
                hooks.append_entries_from(&cfg.hooks);
            }
        }

        Ok(hooks)
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
        if other.sub_agents != SubAgentsConfig::default() {
            self.sub_agents = other.sub_agents;
        }
        if other.hooks != HooksConfig::default() {
            let default_hooks = HooksConfig::default();
            if other.hooks.default_timeout_ms != default_hooks.default_timeout_ms {
                self.hooks.default_timeout_ms = other.hooks.default_timeout_ms;
            }
            if other.hooks.payload_max_bytes != default_hooks.payload_max_bytes {
                self.hooks.payload_max_bytes = other.hooks.payload_max_bytes;
            }
            self.hooks.entries.extend(other.hooks.entries);
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

/// Sub-agent model policy configuration.
///
/// Controls which providers/models sub-agents may use. The special value
/// `"inherit"` for `default_provider` / `default_model` means "use the
/// parent agent's provider/model".
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct SubAgentsConfig {
    /// Default provider for sub-agents.
    /// `"inherit"` = copy from parent, or an explicit provider name.
    pub default_provider: String,
    /// Default model for sub-agents.
    /// `"inherit"` = copy from parent, or an explicit model name.
    pub default_model: String,
    /// Per-provider allowlists of model names.
    /// Every concrete provider key (`anthropic`, `openai`, `gemini`) must be
    /// present and non-empty. Wildcards (`"*"`) are rejected at validation time.
    pub allowed_models: BTreeMap<String, Vec<String>>,
}

impl Default for SubAgentsConfig {
    fn default() -> Self {
        Self {
            default_provider: "inherit".to_string(),
            default_model: "inherit".to_string(),
            allowed_models: default_allowed_models(),
        }
    }
}

fn default_allowed_models() -> BTreeMap<String, Vec<String>> {
    let mut map = BTreeMap::new();
    map.insert(
        "anthropic".to_string(),
        vec![
            "claude-opus-4-6".to_string(),
            "claude-sonnet-4-5".to_string(),
            "claude-opus-4-5".to_string(),
        ],
    );
    map.insert(
        "openai".to_string(),
        vec!["gpt-5.2".to_string(), "gpt-5.2-pro".to_string()],
    );
    map.insert(
        "gemini".to_string(),
        vec![
            "gemini-3-flash-preview".to_string(),
            "gemini-3-pro-preview".to_string(),
        ],
    );
    map
}

/// Resolved sub-agent configuration with concrete provider/model (no "inherit").
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedSubAgentConfig {
    pub default_provider: Provider,
    pub default_model: String,
    pub allowed_models: BTreeMap<String, Vec<String>>,
}

impl ResolvedSubAgentConfig {
    /// Check whether the given model is in the allowlist for its provider.
    pub fn is_model_allowed(&self, provider: Provider, model: &str) -> bool {
        self.allowed_models
            .get(provider.as_str())
            .map(|list| list.iter().any(|m| m == model))
            .unwrap_or(false)
    }

    /// Format the allowed models as a description string suitable for tool descriptions.
    pub fn allowed_models_description(&self) -> String {
        let parts: Vec<String> = self
            .allowed_models
            .iter()
            .map(|(provider, models)| {
                let title = match provider.as_str() {
                    "anthropic" => "Anthropic",
                    "openai" => "OpenAI",
                    "gemini" => "Gemini",
                    other => other,
                };
                format!("{}: {}", title, models.join(", "))
            })
            .collect();
        format!("Allowed models - {}", parts.join("; "))
    }
}

impl Config {
    /// Validate configuration invariants.
    ///
    /// Called after loading and after persisting. Checks:
    /// - No `"*"` wildcards in allowlists
    /// - Every concrete provider key is present and non-empty
    /// - Resolved defaults are valid
    pub fn validate(&self) -> Result<(), ConfigError> {
        let sa = &self.sub_agents;

        // Validate allowed_models
        for provider in Provider::ALL_CONCRETE {
            let key = provider.as_str();
            let models = sa.allowed_models.get(key).ok_or_else(|| {
                ConfigError::Validation(format!(
                    "sub_agents.allowed_models missing provider key '{}'",
                    key
                ))
            })?;
            if models.is_empty() {
                return Err(ConfigError::Validation(format!(
                    "sub_agents.allowed_models['{}'] must not be empty",
                    key
                )));
            }
            for model in models {
                if model == "*" {
                    return Err(ConfigError::Validation(format!(
                        "sub_agents.allowed_models['{}']: wildcards ('*') are not allowed",
                        key
                    )));
                }
            }
        }

        // If default_provider is not "inherit", it must be a valid provider
        if sa.default_provider != "inherit"
            && Provider::parse_strict(&sa.default_provider).is_none()
        {
            return Err(ConfigError::Validation(format!(
                "sub_agents.default_provider '{}' is not a valid provider name",
                sa.default_provider
            )));
        }

        // If default_model is not "inherit", it must be in the allowlist for some provider
        if sa.default_model != "inherit" && sa.default_provider != "inherit" {
            if let Some(provider) = Provider::parse_strict(&sa.default_provider) {
                let models = sa
                    .allowed_models
                    .get(provider.as_str())
                    .cloned()
                    .unwrap_or_default();
                if !models.iter().any(|m| m == &sa.default_model) {
                    return Err(ConfigError::Validation(format!(
                        "sub_agents.default_model '{}' is not in allowed_models for provider '{}'",
                        sa.default_model,
                        provider.as_str()
                    )));
                }
            }
        }

        Ok(())
    }

    /// Resolve sub-agent configuration, replacing "inherit" with concrete values.
    ///
    /// Priority: explicit config value > inherited parent context > inference > error.
    pub fn resolve_sub_agent_config(
        &self,
        parent_provider: Option<Provider>,
        parent_model: &str,
    ) -> Result<ResolvedSubAgentConfig, ConfigError> {
        let sa = &self.sub_agents;

        // Resolve provider
        let provider = if sa.default_provider == "inherit" {
            // Inherit from parent
            if let Some(p) = parent_provider {
                if p == Provider::Other {
                    return Err(ConfigError::Validation(
                        "Cannot inherit sub-agent provider: parent provider is 'other'".to_string(),
                    ));
                }
                p
            } else {
                // Try to infer from parent model
                Provider::infer_from_model(parent_model).ok_or_else(|| {
                    ConfigError::Validation(format!(
                        "Cannot resolve sub-agent provider: parent provider unknown and model '{}' is ambiguous",
                        parent_model
                    ))
                })?
            }
        } else {
            Provider::parse_strict(&sa.default_provider).ok_or_else(|| {
                ConfigError::Validation(format!(
                    "sub_agents.default_provider '{}' is not a valid provider name",
                    sa.default_provider
                ))
            })?
        };

        // Resolve model
        let model = if sa.default_model == "inherit" {
            parent_model.to_string()
        } else {
            sa.default_model.clone()
        };

        let resolved = ResolvedSubAgentConfig {
            default_provider: provider,
            default_model: model,
            allowed_models: sa.allowed_models.clone(),
        };

        // Validate the resolved default is actually in the allowlist
        if !resolved.is_model_allowed(resolved.default_provider, &resolved.default_model) {
            return Err(ConfigError::Validation(format!(
                "Resolved sub-agent default model '{}' is not in allowed_models for provider '{}'",
                resolved.default_model,
                resolved.default_provider.as_str()
            )));
        }

        Ok(resolved)
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

/// Hook configuration root.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(default)]
pub struct HooksConfig {
    /// Default timeout for one hook invocation.
    pub default_timeout_ms: u64,
    /// Max serialized invocation payload size.
    pub payload_max_bytes: usize,
    /// Ordered hook registrations.
    #[serde(default)]
    pub entries: Vec<HookEntryConfig>,
}

impl HooksConfig {
    pub fn append_entries_from(&mut self, other: &HooksConfig) {
        self.entries.extend(other.entries.clone());
    }
}

impl Default for HooksConfig {
    fn default() -> Self {
        Self {
            default_timeout_ms: 5_000,
            payload_max_bytes: 128 * 1024,
            entries: Vec::new(),
        }
    }
}

/// Run-scoped hook overrides.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Default)]
#[serde(default)]
pub struct HookRunOverrides {
    /// Additional hooks appended after layered config entries.
    #[serde(default)]
    pub entries: Vec<HookEntryConfig>,
    /// Hook ids disabled for this run.
    #[serde(default)]
    pub disable: Vec<HookId>,
}

/// One hook registration entry.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(default)]
pub struct HookEntryConfig {
    pub id: HookId,
    pub enabled: bool,
    pub point: HookPoint,
    pub mode: HookExecutionMode,
    pub capability: HookCapability,
    pub priority: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_policy: Option<HookFailurePolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    pub runtime: HookRuntimeConfig,
}

impl HookEntryConfig {
    pub fn effective_failure_policy(&self) -> HookFailurePolicy {
        self.failure_policy
            .unwrap_or_else(|| crate::hooks::default_failure_policy(self.capability))
    }
}

impl Default for HookEntryConfig {
    fn default() -> Self {
        Self {
            id: HookId::new("hook"),
            enabled: true,
            point: HookPoint::TurnBoundary,
            mode: HookExecutionMode::Foreground,
            capability: HookCapability::Observe,
            priority: 100,
            failure_policy: None,
            timeout_ms: None,
            runtime: HookRuntimeConfig::new("in_process", Some(serde_json::json!({"name":"noop"})))
                .unwrap_or_else(|_| HookRuntimeConfig {
                    kind: "in_process".to_string(),
                    config: None,
                }),
        }
    }
}

/// Runtime configuration used by hook adapters.
///
/// Core treats this as an opaque payload to avoid adapter-specific coupling.
#[derive(Debug, Clone)]
pub struct HookRuntimeConfig {
    pub kind: String,
    #[allow(clippy::box_collection)]
    pub config: Option<Box<RawValue>>,
}

impl PartialEq for HookRuntimeConfig {
    fn eq(&self, other: &Self) -> bool {
        self.kind == other.kind
            && self.config.as_ref().map(|raw| raw.get())
                == other.config.as_ref().map(|raw| raw.get())
    }
}

impl HookRuntimeConfig {
    pub fn new(kind: impl Into<String>, config: Option<Value>) -> Result<Self, serde_json::Error> {
        let config = match config {
            Some(value) => Some(raw_json_from_value(value)?),
            None => None,
        };
        Ok(Self {
            kind: kind.into(),
            config,
        })
    }

    pub fn config_value(&self) -> Result<Value, serde_json::Error> {
        match &self.config {
            Some(raw) => serde_json::from_str(raw.get()),
            None => Ok(Value::Null),
        }
    }
}

impl Default for HookRuntimeConfig {
    fn default() -> Self {
        Self::new("in_process", Some(serde_json::json!({"name":"noop"}))).unwrap_or_else(|_| Self {
            kind: "in_process".to_string(),
            config: None,
        })
    }
}

impl Serialize for HookRuntimeConfig {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut map = Map::new();
        map.insert("type".to_string(), Value::String(self.kind.clone()));

        if let Some(raw) = &self.config {
            let parsed: Value =
                serde_json::from_str(raw.get()).map_err(serde::ser::Error::custom)?;
            match parsed {
                Value::Object(obj) => {
                    for (key, value) in obj {
                        map.insert(key, value);
                    }
                }
                other => {
                    map.insert("config".to_string(), other);
                }
            }
        }

        Value::Object(map).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for HookRuntimeConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        let mut obj = value
            .as_object()
            .cloned()
            .ok_or_else(|| serde::de::Error::custom("hook runtime must be an object"))?;

        let kind = obj
            .remove("type")
            .and_then(|value| value.as_str().map(ToOwned::to_owned))
            .ok_or_else(|| {
                serde::de::Error::custom("hook runtime missing required field 'type'")
            })?;

        let config_value = if let Some(explicit) = obj.remove("config") {
            if obj.is_empty() {
                explicit
            } else {
                obj.insert("config".to_string(), explicit);
                Value::Object(obj)
            }
        } else if obj.is_empty() {
            Value::Null
        } else {
            Value::Object(obj)
        };

        let config = if config_value.is_null() {
            None
        } else {
            Some(raw_json_from_value(config_value).map_err(serde::de::Error::custom)?)
        };

        Ok(Self { kind, config })
    }
}

impl JsonSchema for HookRuntimeConfig {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "HookRuntimeConfig".into()
    }

    fn json_schema(_gen: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "type": "object",
            "required": ["type"],
            "properties": {
                "type": { "type": "string" },
                "config": {}
            },
            "additionalProperties": true
        })
    }
}

fn raw_json_from_value(value: Value) -> Result<Box<RawValue>, serde_json::Error> {
    RawValue::from_string(serde_json::to_string(&value)?)
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

    #[error("Validation error: {0}")]
    Validation(String),
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
        assert_eq!(config.agent.model, "claude-opus-4-6");
        assert_eq!(config.agent.max_tokens_per_turn, 16384);
        assert_eq!(config.retry.max_retries, 3);
    }

    #[test]
    fn test_config_layering() {
        // 1. Test defaults
        let config = Config::default();
        assert_eq!(config.agent.model, "claude-opus-4-6");
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

    // === SubAgentsConfig tests ===

    #[test]
    fn test_sub_agents_config_default_validates() {
        let config = Config::default();
        config.validate().expect("Default config should validate");
    }

    #[test]
    fn test_sub_agents_config_default_has_all_providers() {
        let sa = SubAgentsConfig::default();
        assert!(sa.allowed_models.contains_key("anthropic"));
        assert!(sa.allowed_models.contains_key("openai"));
        assert!(sa.allowed_models.contains_key("gemini"));
    }

    #[test]
    fn test_sub_agents_config_toml_roundtrip() {
        let toml_str = r#"
[sub_agents]
default_provider = "openai"
default_model = "gpt-5.2"

[sub_agents.allowed_models]
anthropic = ["claude-opus-4-6"]
openai = ["gpt-5.2", "gpt-5.2-pro"]
gemini = ["gemini-3-flash-preview"]
"#;

        let config: Config = toml::from_str(toml_str).expect("should parse");
        assert_eq!(config.sub_agents.default_provider, "openai");
        assert_eq!(config.sub_agents.default_model, "gpt-5.2");
        assert_eq!(
            config.sub_agents.allowed_models.get("openai").unwrap(),
            &vec!["gpt-5.2".to_string(), "gpt-5.2-pro".to_string()]
        );
    }

    #[test]
    fn test_sub_agents_inherit_resolves_from_parent() {
        let config = Config::default();
        let resolved = config
            .resolve_sub_agent_config(Some(Provider::OpenAI), "gpt-5.2")
            .expect("should resolve");
        assert_eq!(resolved.default_provider, Provider::OpenAI);
        assert_eq!(resolved.default_model, "gpt-5.2");
    }

    #[test]
    fn test_sub_agents_inherit_resolves_provider_from_model() {
        let config = Config::default();
        let resolved = config
            .resolve_sub_agent_config(None, "claude-opus-4-6")
            .expect("should resolve");
        assert_eq!(resolved.default_provider, Provider::Anthropic);
        assert_eq!(resolved.default_model, "claude-opus-4-6");
    }

    #[test]
    fn test_sub_agents_wildcard_rejected() {
        let mut config = Config::default();
        config
            .sub_agents
            .allowed_models
            .get_mut("openai")
            .unwrap()
            .push("*".to_string());
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("wildcards"));
    }

    #[test]
    fn test_sub_agents_missing_provider_rejected() {
        let mut config = Config::default();
        config.sub_agents.allowed_models.remove("gemini");
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("gemini"));
    }

    #[test]
    fn test_sub_agents_empty_list_rejected() {
        let mut config = Config::default();
        config
            .sub_agents
            .allowed_models
            .insert("openai".to_string(), vec![]);
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("must not be empty"));
    }

    #[test]
    fn test_sub_agents_default_not_in_allowlist_rejected() {
        let mut config = Config::default();
        config.sub_agents.default_provider = "openai".to_string();
        config.sub_agents.default_model = "nonexistent-model".to_string();
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("not in allowed_models"));
    }

    #[test]
    fn test_sub_agents_resolve_ambiguous_fails() {
        let config = Config::default();
        // "custom-model" doesn't match any known prefix
        let err = config
            .resolve_sub_agent_config(None, "custom-model")
            .unwrap_err();
        assert!(err.to_string().contains("ambiguous"));
    }

    #[test]
    fn test_sub_agents_resolve_explicit_config() {
        let mut config = Config::default();
        config.sub_agents.default_provider = "gemini".to_string();
        config.sub_agents.default_model = "gemini-3-flash-preview".to_string();
        config.validate().expect("should validate");

        let resolved = config
            .resolve_sub_agent_config(Some(Provider::Anthropic), "claude-opus-4-6")
            .expect("should resolve");
        // Explicit config overrides parent
        assert_eq!(resolved.default_provider, Provider::Gemini);
        assert_eq!(resolved.default_model, "gemini-3-flash-preview");
    }

    #[test]
    fn test_sub_agents_resolved_default_not_in_allowlist() {
        // Even though inherit resolves fine, if the resolved model isn't
        // in the allowlist, it should fail
        let config = Config::default();
        let err = config
            .resolve_sub_agent_config(Some(Provider::Anthropic), "claude-old-model-42")
            .unwrap_err();
        assert!(err.to_string().contains("not in allowed_models"));
    }

    #[test]
    fn test_provider_parse_strict() {
        assert_eq!(
            Provider::parse_strict("anthropic"),
            Some(Provider::Anthropic)
        );
        assert_eq!(Provider::parse_strict("openai"), Some(Provider::OpenAI));
        assert_eq!(Provider::parse_strict("gemini"), Some(Provider::Gemini));
        assert_eq!(Provider::parse_strict("other"), None);
        assert_eq!(Provider::parse_strict("claude"), None);
        assert_eq!(Provider::parse_strict(""), None);
    }

    #[test]
    fn test_provider_infer_from_model() {
        assert_eq!(
            Provider::infer_from_model("claude-opus-4-6"),
            Some(Provider::Anthropic)
        );
        assert_eq!(
            Provider::infer_from_model("gpt-5.2"),
            Some(Provider::OpenAI)
        );
        assert_eq!(
            Provider::infer_from_model("gemini-3-flash-preview"),
            Some(Provider::Gemini)
        );
        assert_eq!(Provider::infer_from_model("llama-3"), None);
        assert_eq!(Provider::infer_from_model(""), None);
    }

    #[test]
    fn test_sub_agents_config_merge() {
        let mut base = Config::default();
        let mut other = Config::default();
        other.sub_agents.default_provider = "openai".to_string();
        other.sub_agents.default_model = "gpt-5.2".to_string();
        base.merge(other);
        assert_eq!(base.sub_agents.default_provider, "openai");
        assert_eq!(base.sub_agents.default_model, "gpt-5.2");
    }

    #[test]
    fn test_resolved_sub_agent_config_is_model_allowed() {
        let config = Config::default();
        let resolved = config
            .resolve_sub_agent_config(Some(Provider::Anthropic), "claude-opus-4-6")
            .unwrap();
        assert!(resolved.is_model_allowed(Provider::Anthropic, "claude-opus-4-6"));
        assert!(resolved.is_model_allowed(Provider::OpenAI, "gpt-5.2"));
        assert!(!resolved.is_model_allowed(Provider::OpenAI, "gpt-4o"));
    }

    #[test]
    fn test_resolved_sub_agent_config_description() {
        let config = Config::default();
        let resolved = config
            .resolve_sub_agent_config(Some(Provider::Anthropic), "claude-opus-4-6")
            .unwrap();
        let desc = resolved.allowed_models_description();
        assert!(desc.contains("Anthropic"));
        assert!(desc.contains("OpenAI"));
        assert!(desc.contains("Gemini"));
        assert!(desc.contains("gpt-5.2"));
        assert!(desc.contains("claude-opus-4-6"));
    }
}
