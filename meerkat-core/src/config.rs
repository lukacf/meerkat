//! Configuration for Meerkat
//!
//! Supports layered configuration: defaults → file → env (secrets only) → CLI

use crate::connection::RealmConfigSection;
use crate::mcp_config::McpServerConfig;
use crate::model_profile::catalog::ModelTier;
use crate::{
    budget::BudgetLimits,
    hooks::{HookCapability, HookExecutionMode, HookId, HookPoint},
    retry::RetryPolicy,
    types::{OutputSchema, SecurityMode},
};
use schemars::JsonSchema;
use serde::de::Deserializer;
use serde::ser::SerializeStruct;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;

/// Default active session admission capacity for RPC/CLI runtime surfaces.
pub const DEFAULT_MAX_SESSIONS: usize = 100_000;

/// Complete configuration for Meerkat
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub agent: AgentConfig,
    pub storage: StorageConfig,
    pub budget: BudgetConfig,
    pub retry: RetryConfig,
    pub tools: ToolsConfig,
    // New schema fields for interface consolidation.
    pub models: ModelDefaults,
    /// Top-level per-turn output-token limit.
    ///
    /// `None` means "inherit / use the template default"; an explicit value
    /// (including one equal to the template default) is an override that wins
    /// over an inherited parent realm value. The operative runtime value is
    /// resolved at point-of-use via [`Config::resolved_max_tokens`] — keeping
    /// the field optional is what lets realm inheritance distinguish "unset"
    /// from "explicitly set to the default" (presence, not a `!= default`
    /// heuristic).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    pub shell: ShellDefaults,
    pub store: StoreConfig,
    pub comms: CommsRuntimeConfig,
    pub compaction: CompactionRuntimeConfig,
    pub limits: LimitsConfig,
    pub rest: RestServerConfig,
    pub hooks: HooksConfig,
    pub skills: crate::skills_config::SkillsConfig,
    pub self_hosted: SelfHostedConfig,
    pub provider_tools: ProviderToolsConfig,
    pub model_fallback: ModelFallbackConfig,
    pub presentation: PresentationConfig,
    /// Realm-scoped connection sets (backend profiles, auth profiles,
    /// bindings). TOML keys use the singular `[realm.<id>.*]` namespace
    /// even though the Rust field is plural-adjacent — `#[serde(rename)]`
    /// bridges the two. See
    /// `/Users/luka/.claude/plans/yes-make-a-plan-shimmying-bengio.md`.
    #[serde(rename = "realm", default, skip_serializing_if = "BTreeMap::is_empty")]
    pub realm: BTreeMap<String, RealmConfigSection>,
}

impl Default for Config {
    fn default() -> Self {
        let agent = AgentConfig::default();
        Self {
            agent,
            storage: StorageConfig::default(),
            budget: BudgetConfig::default(),
            retry: RetryConfig::default(),
            tools: ToolsConfig::default(),
            models: ModelDefaults::default(),
            // `None` = use the template/resolved default at point-of-use. Do
            // NOT materialize the template value here: a concrete default would
            // make realm inheritance unable to tell "unset" from "explicitly set
            // to the default". See [`Config::resolved_max_tokens`].
            max_tokens: None,
            shell: ShellDefaults::default(),
            store: StoreConfig::default(),
            comms: CommsRuntimeConfig::default(),
            compaction: CompactionRuntimeConfig::default(),
            limits: LimitsConfig::default(),
            rest: RestServerConfig::default(),
            hooks: HooksConfig::default(),
            skills: crate::skills_config::SkillsConfig::default(),
            self_hosted: SelfHostedConfig::default(),
            provider_tools: ProviderToolsConfig::default(),
            model_fallback: ModelFallbackConfig::default(),
            presentation: PresentationConfig::default(),
            realm: BTreeMap::new(),
        }
    }
}

impl Config {
    /// Active/staged session admission capacity for runtime surfaces.
    pub fn max_sessions(&self) -> usize {
        self.limits.max_sessions.unwrap_or(DEFAULT_MAX_SESSIONS)
    }

    /// Build the effective model registry for this config snapshot and an
    /// explicitly injected model catalog (canonically
    /// `meerkat_models::canonical()`).
    pub fn model_registry(
        &self,
        catalog: crate::model_profile::ModelCatalog,
    ) -> Result<crate::ModelRegistry, ConfigError> {
        crate::ModelRegistry::from_config(self, catalog)
    }

    /// Return the config template as TOML.
    pub fn template_toml() -> &'static str {
        CONFIG_TEMPLATE_TOML
    }

    /// Parse the config template into a Config value.
    pub fn template() -> Result<Self, ConfigError> {
        toml::from_str(CONFIG_TEMPLATE_TOML).map_err(ConfigError::Parse)
    }
}

// File-system dependent methods — not available on wasm32.
#[cfg(not(target_arch = "wasm32"))]
impl Config {
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
        } else if let Some(path) = home_dir.map(|home| home.join(".rkat/config.toml"))
            && tokio::fs::try_exists(&path).await.unwrap_or(false)
        {
            config.merge_file(&path).await?;
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

        if let Some(global_path) = home_dir.map(|home| home.join(".rkat/config.toml"))
            && tokio::fs::try_exists(&global_path).await.unwrap_or(false)
        {
            let content = tokio::fs::read_to_string(&global_path).await?;
            let cfg: Config = toml::from_str(&content).map_err(ConfigError::Parse)?;
            hooks.append_entries_from(&cfg.hooks);
        }

        if let Some(project_path) = Self::find_project_config_from(start_dir).await
            && tokio::fs::try_exists(&project_path).await.unwrap_or(false)
        {
            let content = tokio::fs::read_to_string(&project_path).await?;
            let cfg: Config = toml::from_str(&content).map_err(ConfigError::Parse)?;
            hooks.append_entries_from(&cfg.hooks);
        }

        Ok(hooks)
    }
}

impl Config {
    /// Convert config limits into runtime budget limits.
    pub fn budget_limits(&self) -> BudgetLimits {
        self.limits.to_budget_limits()
    }

    /// Get global config path (~/.rkat/config.toml)
    #[cfg(not(target_arch = "wasm32"))]
    pub fn global_config_path() -> Option<PathBuf> {
        dirs::home_dir().map(|h| h.join(".rkat/config.toml"))
    }

    /// Find project config by walking up directories (.rkat/config.toml)
    ///
    /// Only returns a path if both `.rkat/` directory AND `config.toml` exist.
    /// This allows `.rkat/` to be created for session storage without requiring
    /// a config file.
    #[cfg(not(target_arch = "wasm32"))]
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
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn merge_file(&mut self, path: &PathBuf) -> Result<(), ConfigError> {
        let content = tokio::fs::read_to_string(path).await?;
        self.merge_toml_str(&content)
    }

    /// Merge configuration from a TOML string.
    pub fn merge_toml_str(&mut self, content: &str) -> Result<(), ConfigError> {
        let file_config: Config = toml::from_str(content).map_err(ConfigError::Parse)?;
        let tools_layer = file_config.tools.clone();
        let retry_layer = file_config.retry.clone();
        let self_hosted_layer = file_config.self_hosted.clone();
        let provider_tools_layer = file_config.provider_tools.clone();
        // Merge (file values override defaults)
        self.merge(file_config);
        let parsed: toml::Value = toml::from_str(content).map_err(ConfigError::Parse)?;
        self.merge_tools_from_toml_presence(&parsed, &tools_layer);
        self.merge_retry_from_toml_presence(&parsed, &retry_layer);
        self.merge_self_hosted_from_toml_presence(&parsed, &self_hosted_layer);
        self.merge_provider_tools_from_toml_presence(&parsed, &provider_tools_layer);
        Ok(())
    }

    /// Merge another config into this one.
    ///
    /// Merge semantics are field-specific:
    /// - Scalar/option fields: last non-default wins.
    /// - Structured sections (`provider`, `providers`, `store`, `comms`, `compaction`, etc.):
    ///   whole-section replace when incoming value is non-default.
    /// - Hook entries: append/extend.
    ///
    /// JSON merge-patch semantics are handled separately by `ConfigStore::patch`.
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
        if other.agent.max_tokens_per_turn.is_some() {
            self.agent.max_tokens_per_turn = other.agent.max_tokens_per_turn;
        }
        if other.agent.extraction_prompt.is_some() {
            self.agent.extraction_prompt = other.agent.extraction_prompt;
        }

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
        self.merge_retry(&other.retry);
        self.merge_tools(&other.tools);

        // New schema fields (replace if non-default)
        // Per-provider union, child-wins: a child overriding its anthropic
        // default still inherits the parent's openai default. Custom model
        // registry entries union by id (child overrides the same id).
        let default_models = ModelDefaults::default();
        if other.models.anthropic != default_models.anthropic {
            self.models.anthropic = other.models.anthropic;
        }
        if other.models.openai != default_models.openai {
            self.models.openai = other.models.openai;
        }
        if other.models.gemini != default_models.gemini {
            self.models.gemini = other.models.gemini;
        }
        for (id, entry) in other.models.custom {
            self.models.custom.insert(id, entry);
        }
        if other.max_tokens.is_some() {
            self.max_tokens = other.max_tokens;
        }
        if other.shell != ShellDefaults::default() {
            self.shell = other.shell;
        }
        if other.store != StoreConfig::default() {
            self.store = other.store;
        }
        if other.comms != CommsRuntimeConfig::default() {
            self.comms = other.comms;
        }
        if other.compaction != CompactionRuntimeConfig::default() {
            self.compaction = other.compaction;
        }
        // Per-field child-wins so a child tightens one limit while inheriting
        // the rest of the caps.
        if other.limits.budget.is_some() {
            self.limits.budget = other.limits.budget;
        }
        if other.limits.max_sessions.is_some() {
            self.limits.max_sessions = other.limits.max_sessions;
        }
        if other.limits.max_duration.is_some() {
            self.limits.max_duration = other.limits.max_duration;
        }
        if other.rest != RestServerConfig::default() {
            self.rest = other.rest;
        }
        if other.hooks != HooksConfig::default() {
            let default_hooks = HooksConfig::default();
            if other.hooks.default_timeout_ms != default_hooks.default_timeout_ms {
                self.hooks.default_timeout_ms = other.hooks.default_timeout_ms;
            }
            if other.hooks.payload_max_bytes != default_hooks.payload_max_bytes {
                self.hooks.payload_max_bytes = other.hooks.payload_max_bytes;
            }
            if other.hooks.background_max_concurrency != default_hooks.background_max_concurrency {
                self.hooks.background_max_concurrency = other.hooks.background_max_concurrency;
            }
            self.hooks.entries.extend(other.hooks.entries);
        }
        if other.model_fallback.use_catalog_default_chain {
            self.model_fallback = ModelFallbackConfig::default();
        } else if other.model_fallback != ModelFallbackConfig::default() {
            self.model_fallback = other.model_fallback;
        }

        // Skills: scalar toggles child-wins when non-default; repositories
        // append parent-first, with a child shadowing a same-named parent
        // source rather than dropping the rest. No removal of inherited sources.
        let default_skills = crate::skills_config::SkillsConfig::default();
        if other.skills.enabled != default_skills.enabled {
            self.skills.enabled = other.skills.enabled;
        }
        if other.skills.max_injection_bytes != default_skills.max_injection_bytes {
            self.skills.max_injection_bytes = other.skills.max_injection_bytes;
        }
        if other.skills.inventory_threshold != default_skills.inventory_threshold {
            self.skills.inventory_threshold = other.skills.inventory_threshold;
        }
        for repo in other.skills.repositories {
            if let Some(existing) = self
                .skills
                .repositories
                .iter_mut()
                .find(|existing| existing.name == repo.name)
            {
                *existing = repo;
            } else {
                self.skills.repositories.push(repo);
            }
        }

        // Realm sections: OUTER-KEY union, child-wins. A child's `[realm.X]`
        // entry overrides/adds at the realm-id level; the backend/auth/binding
        // sub-maps are NEVER merged across realms, so each section stays intact
        // and the chain walk resolves a binding only within its owning realm
        // (MF-11, preserves the no-inherit-for-backend/auth invariant).
        for (realm_id, section) in other.realm {
            self.realm.insert(realm_id, section);
        }
    }

    fn merge_retry(&mut self, other: &RetryConfig) {
        let defaults = RetryConfig::default();
        if other.max_retries != defaults.max_retries {
            self.retry.max_retries = other.max_retries;
        }
        if other.initial_delay != defaults.initial_delay {
            self.retry.initial_delay = other.initial_delay;
        }
        if other.max_delay != defaults.max_delay {
            self.retry.max_delay = other.max_delay;
        }
        if other.multiplier != defaults.multiplier {
            self.retry.multiplier = other.multiplier;
        }
        if other.call_timeout_override != defaults.call_timeout_override {
            self.retry.call_timeout_override = other.call_timeout_override.clone();
        }
    }

    fn merge_tools(&mut self, other: &ToolsConfig) {
        let defaults = ToolsConfig::default();
        // Union by server name, child-wins; never remove inherited servers
        // (an empty child list keeps the parent's set — emptiness != removal,
        // no tombstones).
        for server in &other.mcp_servers {
            if let Some(existing) = self
                .tools
                .mcp_servers
                .iter_mut()
                .find(|existing| existing.name == server.name)
            {
                existing.clone_from(server);
            } else {
                self.tools.mcp_servers.push(server.clone());
            }
        }
        if other.default_timeout != defaults.default_timeout {
            self.tools.default_timeout = other.default_timeout;
        }
        if other.tool_timeouts != defaults.tool_timeouts {
            self.tools.tool_timeouts.clone_from(&other.tool_timeouts);
        }
        if other.max_concurrent != defaults.max_concurrent {
            self.tools.max_concurrent = other.max_concurrent;
        }
        if other.builtins_enabled != defaults.builtins_enabled {
            self.tools.builtins_enabled = other.builtins_enabled;
        }
        if other.shell_enabled != defaults.shell_enabled {
            self.tools.shell_enabled = other.shell_enabled;
        }
        if other.comms_enabled != defaults.comms_enabled {
            self.tools.comms_enabled = other.comms_enabled;
        }
        if other.mob_enabled != defaults.mob_enabled {
            self.tools.mob_enabled = other.mob_enabled;
        }
        if other.schedule_enabled != defaults.schedule_enabled {
            self.tools.schedule_enabled = other.schedule_enabled;
        }
        if other.workgraph_enabled != defaults.workgraph_enabled {
            self.tools.workgraph_enabled = other.workgraph_enabled;
        }
    }

    fn merge_tools_from_toml_presence(&mut self, parsed: &toml::Value, layer: &ToolsConfig) {
        let Some(tools) = parsed.get("tools").and_then(toml::Value::as_table) else {
            return;
        };
        if tools.contains_key("mcp_servers") {
            self.tools.mcp_servers.clone_from(&layer.mcp_servers);
        }
        if tools.contains_key("default_timeout") {
            self.tools.default_timeout = layer.default_timeout;
        }
        if tools.contains_key("tool_timeouts") {
            self.tools.tool_timeouts.clone_from(&layer.tool_timeouts);
        }
        if tools.contains_key("max_concurrent") {
            self.tools.max_concurrent = layer.max_concurrent;
        }
        if tools.contains_key("builtins_enabled") {
            self.tools.builtins_enabled = layer.builtins_enabled;
        }
        if tools.contains_key("shell_enabled") {
            self.tools.shell_enabled = layer.shell_enabled;
        }
        if tools.contains_key("comms_enabled") {
            self.tools.comms_enabled = layer.comms_enabled;
        }
        if tools.contains_key("mob_enabled") {
            self.tools.mob_enabled = layer.mob_enabled;
        }
        if tools.contains_key("schedule_enabled") {
            self.tools.schedule_enabled = layer.schedule_enabled;
        }
        if tools.contains_key("workgraph_enabled") {
            self.tools.workgraph_enabled = layer.workgraph_enabled;
        }
    }

    fn merge_retry_from_toml_presence(&mut self, parsed: &toml::Value, layer: &RetryConfig) {
        let Some(retry) = parsed.get("retry").and_then(toml::Value::as_table) else {
            return;
        };
        if retry.contains_key("max_retries") {
            self.retry.max_retries = layer.max_retries;
        }
        if retry.contains_key("initial_delay") {
            self.retry.initial_delay = layer.initial_delay;
        }
        if retry.contains_key("max_delay") {
            self.retry.max_delay = layer.max_delay;
        }
        if retry.contains_key("multiplier") {
            self.retry.multiplier = layer.multiplier;
        }
        if retry.contains_key("call_timeout") {
            self.retry.call_timeout_override = layer.call_timeout_override.clone();
        }
    }

    fn merge_provider_tools_from_toml_presence(
        &mut self,
        parsed: &toml::Value,
        layer: &ProviderToolsConfig,
    ) {
        let Some(pt) = parsed.get("provider_tools").and_then(toml::Value::as_table) else {
            return;
        };
        if let Some(anthropic) = pt.get("anthropic").and_then(toml::Value::as_table)
            && anthropic.contains_key("web_search")
        {
            self.provider_tools.anthropic.web_search = layer.anthropic.web_search;
        }
        if let Some(openai) = pt.get("openai").and_then(toml::Value::as_table)
            && openai.contains_key("web_search")
        {
            self.provider_tools.openai.web_search = layer.openai.web_search;
        }
        if let Some(gemini) = pt.get("gemini").and_then(toml::Value::as_table)
            && gemini.contains_key("google_search")
        {
            self.provider_tools.gemini.google_search = layer.gemini.google_search;
        }
    }

    fn merge_self_hosted_from_toml_presence(
        &mut self,
        parsed: &toml::Value,
        layer: &SelfHostedConfig,
    ) {
        let Some(self_hosted) = parsed.get("self_hosted").and_then(toml::Value::as_table) else {
            return;
        };

        // Presence-based: a layer adopts `default_model` only when it declares
        // one, otherwise the inherited explicit default is preserved (it is the
        // typed self-hosted default owner, not a re-derived key-order artifact).
        if self_hosted.contains_key("default_model") {
            self.self_hosted.default_model = layer.default_model.clone();
        }

        if let Some(servers) = self_hosted.get("servers").and_then(toml::Value::as_table) {
            if servers.is_empty() {
                self.self_hosted.servers.clear();
                self.self_hosted.models.clear();
            } else {
                let mut merged_servers = self.self_hosted.servers.clone();
                for (server_id, server_value) in servers {
                    let Some(server_table) = server_value.as_table() else {
                        continue;
                    };
                    let mut merged = self
                        .self_hosted
                        .servers
                        .get(server_id)
                        .cloned()
                        .unwrap_or_default();
                    let Some(server_layer) = layer.servers.get(server_id) else {
                        continue;
                    };
                    if server_table.contains_key("transport") {
                        merged.transport = server_layer.transport;
                    }
                    if server_table.contains_key("base_url") {
                        merged.base_url = server_layer.base_url.clone();
                    }
                    if server_table.contains_key("api_style") {
                        merged.api_style = server_layer.api_style;
                    }
                    merged_servers.insert(server_id.clone(), merged);
                }
                self.self_hosted.servers = merged_servers;
            }
        }

        if let Some(models) = self_hosted.get("models").and_then(toml::Value::as_table) {
            if models.is_empty() {
                self.self_hosted.models.clear();
            } else {
                let mut merged_models = self.self_hosted.models.clone();
                for (model_id, model_value) in models {
                    let Some(model_table) = model_value.as_table() else {
                        continue;
                    };
                    let mut merged = self
                        .self_hosted
                        .models
                        .get(model_id)
                        .cloned()
                        .unwrap_or_default();
                    let Some(model_layer) = layer.models.get(model_id) else {
                        continue;
                    };
                    if model_table.contains_key("server") {
                        merged.server = model_layer.server.clone();
                    }
                    if model_table.contains_key("remote_model") {
                        merged.remote_model = model_layer.remote_model.clone();
                    }
                    if model_table.contains_key("display_name") {
                        merged.display_name = model_layer.display_name.clone();
                    }
                    if model_table.contains_key("family") {
                        merged.family = model_layer.family.clone();
                    }
                    if model_table.contains_key("tier") {
                        merged.tier = model_layer.tier;
                    }
                    if model_table.contains_key("context_window") {
                        merged.context_window = model_layer.context_window;
                    }
                    if model_table.contains_key("max_output_tokens") {
                        merged.max_output_tokens = model_layer.max_output_tokens;
                    }
                    if model_table.contains_key("vision") {
                        merged.vision = model_layer.vision;
                    }
                    if model_table.contains_key("image_tool_results") {
                        merged.image_tool_results = model_layer.image_tool_results;
                    }
                    if model_table.contains_key("inline_video") {
                        merged.inline_video = model_layer.inline_video;
                    }
                    if model_table.contains_key("supports_temperature") {
                        merged.supports_temperature = model_layer.supports_temperature;
                    }
                    if model_table.contains_key("supports_thinking") {
                        merged.supports_thinking = model_layer.supports_thinking;
                    }
                    if model_table.contains_key("supports_reasoning") {
                        merged.supports_reasoning = model_layer.supports_reasoning;
                    }
                    if model_table.contains_key("call_timeout_secs") {
                        merged.call_timeout_secs = model_layer.call_timeout_secs;
                    }
                    merged_models.insert(model_id.clone(), merged);
                }
                self.self_hosted.models = merged_models;
            }
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
    pub fn apply_env_overrides_from<F>(&mut self, _env: F) -> Result<(), ConfigError>
    where
        F: FnMut(&str) -> Option<String>,
    {
        // Plan §6.9 deleted the legacy `config.provider = ProviderConfig::X
        // { api_key }` path. Env-var-based credentials are now read at
        // resolve time through `ResolverEnvironment::env_lookup` applied
        // to `CredentialSourceSpec::Env` inside the provider runtime
        // registry — there is no longer a mutable in-memory field on
        // `Config` that env vars write into. This method is retained as
        // a no-op so callers compile through 0.6.0; it will be deleted
        // once surfaces drop the apply_env_overrides call entirely.
        Ok(())
    }

    /// Apply CLI argument overrides.
    ///
    /// - Explicit CLI flags override scalar runtime knobs.
    /// - `override_config` applies RFC 7396 JSON merge-patch semantics.
    #[cfg(not(target_arch = "wasm32"))]
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

impl Config {
    /// Resolve the operative top-level per-turn output-token limit.
    ///
    /// Precedence: explicit `max_tokens` → embedded template `max_tokens` →
    /// the agent's [`AgentConfig::resolved_max_tokens_per_turn`]. Keeping the
    /// field `Option` (rather than materializing a default) is what lets realm
    /// inheritance distinguish an unset child (inherit the parent) from a child
    /// that explicitly pins the default (override the parent).
    pub fn resolved_max_tokens(&self) -> u32 {
        self.max_tokens
            .or_else(|| template_defaults().max_tokens)
            .filter(|value| *value > 0)
            .unwrap_or_else(|| self.agent.resolved_max_tokens_per_turn())
    }

    /// Validate configuration invariants.
    ///
    /// Called after loading and after persisting. Checks:
    /// - No `"*"` wildcards in allowlists
    /// - Every concrete provider key is present and non-empty
    /// - Resolved defaults are valid
    /// - The effective model registry (custom + self-hosted entries merged
    ///   over the injected catalog) constructs without conflicts
    pub fn validate(&self, catalog: crate::model_profile::ModelCatalog) -> Result<(), ConfigError> {
        if self.max_tokens == Some(0) {
            return Err(ConfigError::Validation(
                "max_tokens must be greater than 0 when set".to_string(),
            ));
        }
        if self.agent.max_tokens_per_turn == Some(0) {
            return Err(ConfigError::Validation(
                "agent.max_tokens_per_turn must be greater than 0 when set".to_string(),
            ));
        }
        if self.budget.max_tokens == Some(0) {
            return Err(ConfigError::Validation(
                "budget.max_tokens must be greater than 0 when set".to_string(),
            ));
        }
        if self.limits.budget == Some(0) {
            return Err(ConfigError::Validation(
                "limits.budget must be greater than 0 when set".to_string(),
            ));
        }
        if self.limits.max_sessions == Some(0) {
            return Err(ConfigError::Validation(
                "limits.max_sessions must be greater than 0 when set".to_string(),
            ));
        }
        if self.compaction.auto_compact_threshold == 0 {
            return Err(ConfigError::Validation(
                "compaction.auto_compact_threshold must be greater than 0".to_string(),
            ));
        }
        if self.compaction.recent_turn_budget == 0 {
            return Err(ConfigError::Validation(
                "compaction.recent_turn_budget must be greater than 0".to_string(),
            ));
        }
        if self.compaction.max_summary_tokens == 0 {
            return Err(ConfigError::Validation(
                "compaction.max_summary_tokens must be greater than 0".to_string(),
            ));
        }
        if self.compaction.min_turns_between_compactions == 0 {
            return Err(ConfigError::Validation(
                "compaction.min_turns_between_compactions must be greater than 0".to_string(),
            ));
        }

        // Plan §6.9 deleted the per-provider config enum block, so
        // there is no longer a nominal conflict between the legacy
        // inline api_key/base_url fields and the shared
        // `config.providers.{base_urls,api_keys}` maps. Those maps stay
        // (scheduled for deletion in §6.10) and are consumed directly.

        crate::model_registry::ModelRegistry::from_config(self, catalog)?;

        Ok(())
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

/// Canonical default for the structured-output validation retry budget.
///
/// This is the single owner of the default value. Surfaces (RPC/REST/MCP) and
/// build options carry `Option<u32>` intent (`None` = inherit) and resolve
/// against this owner at the `AgentFactory` seam — they must not define their
/// own default constant.
pub fn default_structured_output_retries() -> u32 {
    2
}

/// Default maximum number of agent-loop turns before a forced stop.
///
/// Canonical owner of the turn-cap default: the agent loop reads this when
/// [`AgentConfig::max_turns`] is `None` rather than inventing a literal at the
/// run-loop seam.
pub const DEFAULT_MAX_TURNS: u32 = 100;

/// Final fallback for the per-turn output-token limit when neither the config
/// nor the embedded template provides one. Materialized at point-of-use only;
/// the config fields stay `Option` so realm inheritance keeps presence
/// semantics (see [`Config::max_tokens`] / [`AgentConfig::max_tokens_per_turn`]).
pub const DEFAULT_MAX_TOKENS_PER_TURN: u32 = 16384;

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
    /// Maximum tokens to generate per turn.
    ///
    /// `None` means "inherit / use the template default"; an explicit value is
    /// an override that wins over an inherited parent realm value. Resolve the
    /// operative value via [`AgentConfig::resolved_max_tokens_per_turn`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens_per_turn: Option<u32>,
    /// Temperature for sampling
    pub temperature: Option<f32>,
    /// Warning threshold for budget (0.0-1.0)
    pub budget_warning_threshold: f32,
    /// Maximum turns before forced stop
    pub max_turns: Option<u32>,
    /// Typed provider parameter carrier (explicit overrides + build-derived
    /// provider-native tool defaults).
    ///
    /// Parsed FAIL-CLOSED at config ingress: a malformed or unknown-keyed
    /// provider-params shape is rejected when the config is read, never
    /// deferred to the first LLM call. The explicit `params` half is
    /// persisted; `tool_defaults` is `#[serde(skip)]` inside the carrier and
    /// re-derived on every build (including resume) from
    /// `Config.provider_tools` + `ModelProfile.supports_web_search`, so config
    /// changes (e.g. disabling web search) take effect immediately on resumed
    /// sessions. The per-turn effective params come from the carrier's typed
    /// field-wise merge ([`ProviderParamsCarrier::effective_params`]).
    ///
    /// [`ProviderParamsCarrier::effective_params`]: crate::lifecycle::run_primitive::ProviderParamsCarrier::effective_params
    #[serde(
        default,
        skip_serializing_if = "crate::lifecycle::run_primitive::ProviderParamsCarrier::serializes_empty"
    )]
    pub provider_params: crate::lifecycle::run_primitive::ProviderParamsCarrier,
    /// Output schema for structured output extraction.
    ///
    /// When set, the agent will perform an extraction turn after completing
    /// the agentic work, forcing the LLM to output validated JSON. The main
    /// response text remains the committed agentic output; extraction populates
    /// structured output on success or extraction error details on failure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<OutputSchema>,
    /// Maximum retries for structured output validation failures.
    #[serde(default = "default_structured_output_retries")]
    pub structured_output_retries: u32,
    /// Custom prompt for the structured output extraction turn.
    ///
    /// When `output_schema` is set, this prompt is sent as a user message
    /// after the agentic loop to elicit schema-valid JSON. Defaults to a
    /// built-in prompt if `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extraction_prompt: Option<String>,
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
            // `None` = inherit / resolve the template default at point-of-use
            // (see [`AgentConfig::resolved_max_tokens_per_turn`]); never
            // materialize a concrete default into the field (presence semantics).
            max_tokens_per_turn: None,
            temperature: None,
            budget_warning_threshold: agent
                .and_then(|cfg| cfg.budget_warning_threshold)
                .unwrap_or_default(),
            max_turns: None,
            provider_params: crate::lifecycle::run_primitive::ProviderParamsCarrier::default(),
            output_schema: None,
            structured_output_retries: default_structured_output_retries(),
            extraction_prompt: None,
        }
    }
}

impl AgentConfig {
    /// Resolve the operative per-turn output-token limit.
    ///
    /// Precedence: explicit field → embedded template → [`DEFAULT_MAX_TOKENS_PER_TURN`].
    /// A `Some(0)` is rejected by [`Config::validate`], so it never reaches here.
    pub fn resolved_max_tokens_per_turn(&self) -> u32 {
        self.max_tokens_per_turn
            .or_else(|| {
                template_defaults()
                    .agent
                    .as_ref()
                    .and_then(|cfg| cfg.max_tokens_per_turn)
            })
            .filter(|value| *value > 0)
            .unwrap_or(DEFAULT_MAX_TOKENS_PER_TURN)
    }
}

/// Presentation defaults for surface-owned renderings.
// Default is all-empty: empty per-provider defaults mean "inherit from the
// injected model catalog". Core owns no provider data, so it cannot (and
// must not) pre-populate these; surfaces resolve empty entries through the
// catalog's per-provider defaults and global default at build time.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct PresentationConfig {
    pub html: HtmlPresentationConfig,
}

/// Defaults for CLI HTML output mode.
///
/// This config controls template selection only. It does not enable HTML output
/// for ordinary runs; surfaces must request the HTML presentation explicitly.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct HtmlPresentationConfig {
    pub default_template: String,
    pub templates: BTreeMap<String, HtmlTemplateConfig>,
}

impl Default for HtmlPresentationConfig {
    fn default() -> Self {
        Self {
            default_template: "polished".to_string(),
            templates: BTreeMap::new(),
        }
    }
}

/// User-defined HTML output template.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct HtmlTemplateConfig {
    pub path: Option<PathBuf>,
    pub body: Option<String>,
}

/// Model defaults by provider, plus user-defined custom model registry
/// entries (`[models.<id>]` tables).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct ModelDefaults {
    pub anthropic: String,
    pub openai: String,
    pub gemini: String,
    /// User-defined model registry entries keyed by model id.
    ///
    /// TOML form: `[models.<id>]` tables alongside the per-provider default
    /// strings. Each entry is merged into the effective [`crate::ModelRegistry`]
    /// by `ModelRegistry::from_config`, the single owner that feeds provider
    /// inference, compaction scaling, capability gates, and call timeouts.
    #[serde(flatten)]
    pub custom: BTreeMap<String, CustomModelConfig>,
}

/// User-defined model registry entry (`[models.<id>]` in `config.toml` or a
/// mob definition).
///
/// Declares an uncatalogued model that an API provider serves so a single
/// definition feeds provider inference, compaction scaling, capability gates,
/// and call timeouts through the effective [`crate::ModelRegistry`].
///
/// Capability flags are conservative when omitted: an undeclared capability is
/// treated as absent rather than guessed from the model name.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CustomModelConfig {
    /// Typed provider that serves this model. Parsed fail-closed at config
    /// ingress: unknown provider names reject the entry.
    pub provider: crate::Provider,
    /// Human-readable display name. Defaults to the model id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// Model context window in tokens (drives compaction scaling).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u32>,
    /// Maximum output tokens per call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
    /// Whether the model accepts image input. Conservative default: false.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vision: Option<bool>,
    /// Whether the model supports provider-native web search tools.
    /// Conservative default: false.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub web_search: Option<bool>,
    /// Default call timeout in seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call_timeout_secs: Option<u64>,
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
    budget_warning_threshold: Option<f32>,
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
    shell: Option<TemplateShellDefaults>,
    max_tokens: Option<u32>,
}

impl TemplateDefaults {
    fn empty() -> Self {
        Self {
            agent: None,
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
    /// Directory for the realm-scoped session database (server surfaces).
    pub database_dir: Option<PathBuf>,
}

// Plan §6.10 deleted `ProviderSettings` entirely. Per-provider api keys
// and base URLs now live exclusively in `[realm.<id>]` config blocks
// (programmatically constructed via `RealmConfigSection::from_inline_api_keys`
// for surfaces that receive credentials at bootstrap).

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum SelfHostedTransport {
    #[default]
    #[serde(alias = "openai_compatible")]
    OpenAiCompatible,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum SelfHostedApiStyle {
    Responses,
    #[default]
    ChatCompletions,
}

/// Self-hosted server connection target.
///
/// Carries connection/transport facts only. Credentials have exactly one
/// owner: the realm auth profile selected via `auth_binding` or a realm
/// default binding. The legacy `bearer_token` / `bearer_token_env` fields are
/// gone and `deny_unknown_fields` rejects them (and any other unknown key) at
/// config parse with a typed [`ConfigError::Parse`] instead of silently
/// tolerating dead credential material in config files.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct SelfHostedServerConfig {
    pub transport: SelfHostedTransport,
    pub base_url: String,
    pub api_style: SelfHostedApiStyle,
}

impl Default for SelfHostedServerConfig {
    fn default() -> Self {
        Self {
            transport: SelfHostedTransport::OpenAiCompatible,
            base_url: String::new(),
            api_style: SelfHostedApiStyle::ChatCompletions,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, JsonSchema)]
#[serde(default)]
pub struct SelfHostedModelConfig {
    pub server: String,
    pub remote_model: String,
    pub display_name: String,
    pub family: String,
    pub tier: ModelTier,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
    pub vision: bool,
    pub image_tool_results: bool,
    pub inline_video: bool,
    pub supports_temperature: bool,
    pub supports_thinking: bool,
    pub supports_reasoning: bool,
    /// Whether the model supports provider-native web search tools.
    #[serde(default)]
    pub supports_web_search: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call_timeout_secs: Option<u64>,
}

impl Default for SelfHostedModelConfig {
    fn default() -> Self {
        Self {
            server: String::new(),
            remote_model: String::new(),
            display_name: String::new(),
            family: String::new(),
            tier: ModelTier::Supported,
            context_window: None,
            max_output_tokens: None,
            vision: false,
            image_tool_results: false,
            inline_video: false,
            supports_temperature: true,
            supports_thinking: false,
            supports_reasoning: false,
            supports_web_search: false,
            call_timeout_secs: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default, JsonSchema)]
#[serde(default)]
pub struct SelfHostedConfig {
    pub servers: BTreeMap<String, SelfHostedServerConfig>,
    pub models: BTreeMap<String, SelfHostedModelConfig>,
    /// The model id (key into [`Self::models`]) that is the canonical
    /// self-hosted default.
    ///
    /// The config owns its own default — it is a declared choice, never a
    /// `BTreeMap` key-order artifact. When more than one model is configured
    /// this MUST be set (and reference a configured model), otherwise the
    /// registry fails closed. With exactly one configured model the default is
    /// unambiguous and may be omitted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_model: Option<String>,
}

// ---------------------------------------------------------------------------
// Provider-native tool defaults
// ---------------------------------------------------------------------------

/// Per-provider defaults for provider-native tools (web search, etc.).
///
/// These defaults are resolved at factory build time and injected as the
/// non-persisted `tool_defaults` half of `AgentConfig.provider_params`
/// (a typed `ProviderTag`). The `enabled` flags
/// here control whether the factory injects tool config for models whose
/// `ModelProfile.supports_web_search` is `true`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct ProviderToolsConfig {
    pub anthropic: AnthropicProviderToolsConfig,
    pub openai: OpenAiProviderToolsConfig,
    pub gemini: GeminiProviderToolsConfig,
}

/// Ordered model failover policy used when a turn reaches a recoverable LLM
/// failure boundary.
///
/// Empty `chain` means "use the catalog-owned default fallback chain" at the
/// factory seam. Core keeps this provider-data-free; the `meerkat` facade
/// resolves catalog defaults and builds concrete clients.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ModelFallbackConfig {
    /// Enables runtime model failover for factory-built agents.
    pub enabled: bool,
    /// When true in a higher-precedence layer, restore the catalog-owned
    /// default chain (`enabled = true`, empty `chain`) over an inherited
    /// disabled/custom fallback policy.
    #[serde(default, skip_serializing_if = "is_false")]
    pub use_catalog_default_chain: bool,
    /// Ordered operator-provided backup targets.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub chain: Vec<ModelFallbackTarget>,
}

impl Default for ModelFallbackConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            use_catalog_default_chain: false,
            chain: Vec::new(),
        }
    }
}

/// One configured model fallback target.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ModelFallbackTarget {
    /// Model id, resolved through the effective [`crate::ModelRegistry`].
    pub model: String,
    /// Optional typed provider override. When omitted, registry ownership of
    /// `model` supplies the provider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<crate::Provider>,
    /// Optional realm-scoped auth binding for this target. When omitted, the
    /// provider's default binding is resolved by the existing provider-runtime
    /// registry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_binding: Option<crate::AuthBindingRef>,
}

/// Anthropic provider-native tool defaults.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct AnthropicProviderToolsConfig {
    /// Enable web search for Anthropic models that support it.
    pub web_search: bool,
}

impl Default for AnthropicProviderToolsConfig {
    fn default() -> Self {
        Self { web_search: true }
    }
}

/// OpenAI provider-native tool defaults.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct OpenAiProviderToolsConfig {
    /// Enable web search for OpenAI models that support it.
    pub web_search: bool,
}

impl Default for OpenAiProviderToolsConfig {
    fn default() -> Self {
        Self { web_search: true }
    }
}

/// Gemini provider-native tool defaults.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct GeminiProviderToolsConfig {
    /// Enable Google Search for Gemini models that support it.
    pub google_search: bool,
}

impl Default for GeminiProviderToolsConfig {
    fn default() -> Self {
        Self {
            google_search: true,
        }
    }
}

/// Runtime limits configured at the config layer.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct LimitsConfig {
    pub budget: Option<u64>,
    /// Active session admission capacity. Persisted history does not consume
    /// this limit. RPC observes runtime config updates dynamically; REST/MCP
    /// process-local services apply changes on service restart.
    pub max_sessions: Option<usize>,
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

/// Authentication mode for comms listeners.
///
/// `Open` (default) accepts plain JSON messages over TCP/UDS — no cryptographic
/// verification. Suitable for local agent-to-agent communication where the OS
/// provides process isolation. Non-loopback binding with `Open` auth is a hard
/// error unless explicitly overridden.
///
/// `Ed25519` requires signed CBOR envelopes and a trusted peers list. Use for
/// multi-machine or untrusted network communication.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "snake_case")]
pub enum CommsAuthMode {
    #[default]
    #[serde(rename = "none")]
    Open,
    Ed25519,
}

/// Source identifier for plain (unauthenticated) events.
///
/// Used for diagnostics and prompt formatting — tells the agent where
/// an external event originated from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlainEventSource {
    Tcp,
    Uds,
    Stdin,
    Webhook,
    Rpc,
}

impl std::fmt::Display for PlainEventSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Tcp => write!(f, "tcp"),
            Self::Uds => write!(f, "uds"),
            Self::Stdin => write!(f, "stdin"),
            Self::Webhook => write!(f, "webhook"),
            Self::Rpc => write!(f, "rpc"),
        }
    }
}

/// Runtime comms configuration (portable across interfaces).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct CommsRuntimeConfig {
    pub mode: CommsRuntimeMode,
    /// Address for agent-to-agent (signed) listener.
    pub address: Option<String>,
    /// Peer address advertised to other signed-comms participants.
    pub advertise_address: Option<String>,
    pub auth: CommsAuthMode,
    /// Whether inter-agent peer traffic requires cryptographic validation.
    ///
    /// - `true`: messages require signatures and trusted-sender checks.
    /// - `false`: signatures are not verified and outgoing peer envelopes are
    ///   sent without signing.
    pub require_peer_auth: bool,
    /// Address for the plain-text external event listener.
    /// Only active when `auth = "none"`. Accepts newline-delimited JSON or text.
    pub event_address: Option<String>,
    /// Runtime-only enrollment password for initial comms pairing.
    ///
    /// This is deliberately skipped by serde so CLI/env/file overrides do not
    /// persist a bootstrap secret into realm config or session metadata.
    #[serde(skip)]
    pub pairing_password: Option<String>,
}

impl Default for CommsRuntimeConfig {
    fn default() -> Self {
        Self {
            mode: CommsRuntimeMode::Inproc,
            address: None,
            advertise_address: None,
            auth: CommsAuthMode::default(),
            require_peer_auth: true,
            event_address: None,
            pairing_password: None,
        }
    }
}

/// Runtime compaction configuration (portable across interfaces).
///
/// This config is serialized/deserialized in realm config and mapped to
/// `meerkat_core::CompactionConfig` when wiring the session compactor.
#[derive(Debug, Clone, PartialEq)]
pub struct CompactionRuntimeConfig {
    /// Trigger compaction when input tokens for a turn reach this threshold.
    pub auto_compact_threshold: u64,
    /// Whether `auto_compact_threshold` was explicitly present in config.
    ///
    /// This preserves the difference between inheriting Meerkat's default
    /// threshold and deliberately pinning that same numeric value.
    pub auto_compact_threshold_explicit: bool,
    /// Number of recent complete turns to retain after compaction.
    pub recent_turn_budget: usize,
    /// Maximum tokens for the compaction summary response.
    pub max_summary_tokens: u32,
    /// Minimum session-scoped pre-LLM boundaries between compactions.
    pub min_turns_between_compactions: u32,
}

impl Default for CompactionRuntimeConfig {
    fn default() -> Self {
        Self {
            auto_compact_threshold: 100_000,
            auto_compact_threshold_explicit: false,
            recent_turn_budget: 4,
            max_summary_tokens: 4096,
            min_turns_between_compactions: 3,
        }
    }
}

impl Serialize for CompactionRuntimeConfig {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let defaults = Self::default();
        let include_threshold = self.auto_compact_threshold_explicit
            || self.auto_compact_threshold != defaults.auto_compact_threshold;
        let mut len = 3;
        if include_threshold {
            len += 1;
        }

        let mut state = serializer.serialize_struct("CompactionRuntimeConfig", len)?;
        if include_threshold {
            state.serialize_field("auto_compact_threshold", &self.auto_compact_threshold)?;
        }
        state.serialize_field("recent_turn_budget", &self.recent_turn_budget)?;
        state.serialize_field("max_summary_tokens", &self.max_summary_tokens)?;
        state.serialize_field(
            "min_turns_between_compactions",
            &self.min_turns_between_compactions,
        )?;
        state.end()
    }
}

impl<'de> Deserialize<'de> for CompactionRuntimeConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Seed {
            auto_compact_threshold: Option<u64>,
            recent_turn_budget: Option<usize>,
            max_summary_tokens: Option<u32>,
            min_turns_between_compactions: Option<u32>,
        }

        let seed = Seed::deserialize(deserializer)?;
        let defaults = Self::default();
        Ok(Self {
            auto_compact_threshold: seed
                .auto_compact_threshold
                .unwrap_or(defaults.auto_compact_threshold),
            auto_compact_threshold_explicit: seed.auto_compact_threshold.is_some(),
            recent_turn_budget: seed
                .recent_turn_budget
                .unwrap_or(defaults.recent_turn_budget),
            max_summary_tokens: seed
                .max_summary_tokens
                .unwrap_or(defaults.max_summary_tokens),
            min_turns_between_compactions: seed
                .min_turns_between_compactions
                .unwrap_or(defaults.min_turns_between_compactions),
        })
    }
}

impl From<CompactionRuntimeConfig> for crate::CompactionConfig {
    fn from(value: CompactionRuntimeConfig) -> Self {
        Self {
            auto_compact_threshold: value.auto_compact_threshold,
            recent_turn_budget: value.recent_turn_budget,
            max_summary_tokens: value.max_summary_tokens,
            min_turns_between_compactions: value.min_turns_between_compactions,
        }
    }
}

/// Transport mode for comms runtime.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum CommsRuntimeMode {
    #[default]
    Inproc,
    Tcp,
    Uds,
}

// Plan §6.9 deleted the `ProviderConfig` enum entirely. Per-provider
// credentials now live in:
//   - env vars (ANTHROPIC_API_KEY / OPENAI_API_KEY / GEMINI_API_KEY,
//     RKAT_*-prefixed overrides) — the default path, consumed through
//     `RealmConnectionSet::synthesize_env_default(provider)`.
//   - `[realm.<id>]` blocks in TOML — explicit realm/binding declarations,
//     consumed through `ProviderRuntimeRegistry::resolve`.
//
// The legacy `config.provider = ProviderConfig::{Anthropic,OpenAI,Gemini}`
// block and the legacy shared settings maps are
// removed in the same 0.6.0 cutover (plan §6.10).

/// Storage configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct StorageConfig {
    /// Directory for file-based storage
    pub directory: Option<PathBuf>,
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            directory: data_dir().map(|d| d.join("sessions")),
        }
    }
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

/// Tri-state override for per-call LLM timeout policy.
///
/// This type exists because `Option<Duration>` cannot distinguish between
/// "inherit lower-layer/profile default" and "explicitly disable timeout."
/// Build and config seams use this type; the resolved effective policy on
/// `RetryPolicy` collapses to `Option<Duration>`.
///
/// TOML representation:
/// - omitted key => `Inherit`
/// - `call_timeout = "disabled"` => `Disabled`
/// - `call_timeout = "45s"` => `Value(45s)`
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum CallTimeoutOverride {
    /// Inherit the lower-layer or profile-derived default.
    #[default]
    Inherit,
    /// Explicitly disable call timeout (no timeout applied regardless of profile).
    Disabled,
    /// Explicitly set the call timeout to this duration.
    Value(Duration),
}

impl Serialize for CallTimeoutOverride {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            // Inherit is the default; serialized as absence (skip_serializing_if handles this)
            Self::Inherit => serializer.serialize_none(),
            Self::Disabled => serializer.serialize_str("disabled"),
            Self::Value(d) => {
                let s = humantime_serde::re::humantime::format_duration(*d).to_string();
                serializer.serialize_str(&s)
            }
        }
    }
}

impl<'de> Deserialize<'de> for CallTimeoutOverride {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        if s == "disabled" {
            return Ok(Self::Disabled);
        }
        let d: Duration = s
            .parse::<humantime_serde::re::humantime::Duration>()
            .map(|ht| *ht)
            .map_err(serde::de::Error::custom)?;
        Ok(Self::Value(d))
    }
}

impl CallTimeoutOverride {
    /// Returns `true` when this override is `Inherit` (the default / absent state).
    pub fn is_inherit(&self) -> bool {
        matches!(self, Self::Inherit)
    }
}

/// Typed per-request system-prompt policy.
///
/// **Dogma §10:** inherit, set, and disable are three distinct facts that an
/// overloaded `Option<String>` cannot express — `None` collapses "inherit
/// config/AGENTS/default" together with "no opinion", and there is no way to
/// say "suppress every prompt source". This mirrors the `Inherit`/`Set`/`Clear`
/// shape of `TurnMetadataOverride` and the `Inherit`/`Disabled`/`Value` shape
/// of [`CallTimeoutOverride`].
///
/// This is the canonical type at every boundary that carries the decision:
/// the wire `CreateSessionRequest`/`CoreCreateParams.system_prompt` field, the
/// persisted `SessionBuildState.system_prompt` field, and
/// `AgentBuildConfig.system_prompt`. Its serde implementation below IS the
/// wire/persisted representation — there is no adapter pair:
///
/// - absent / `null` ⇔ `Inherit` (lossless with the retired `Option<String>`
///   `None` shape)
/// - JSON/TOML string ⇔ `Set` (lossless with the retired `Some(prompt)` shape)
/// - `{"action": "disable"}` ⇔ `Disable`
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum SystemPromptOverride {
    /// No per-request opinion: fall through to the config-file override, the
    /// config inline override, then the default prompt + AGENTS.md files.
    #[default]
    Inherit,
    /// Explicit per-request prompt. Wins outright, skipping the config and
    /// AGENTS.md sources (dispatcher/tool/extra sections are still appended).
    Set(String),
    /// Explicitly suppress *every* prompt source: no config override, no
    /// AGENTS.md, no default prompt. Only the appended sections
    /// (extra/config-tool/dispatcher) remain.
    Disable,
}

impl SystemPromptOverride {
    /// Returns `true` when this override is `Inherit` (the default / absent
    /// state). Used by `skip_serializing_if` at field sites.
    #[must_use]
    pub fn is_inherit(&self) -> bool {
        matches!(self, Self::Inherit)
    }

    /// Whether this override carries an explicit per-request decision (either a
    /// `Set` prompt or an explicit `Disable`). Used to decide whether the
    /// prompt must be (re)assembled even for a resumed session.
    #[must_use]
    pub fn is_explicit(&self) -> bool {
        !matches!(self, Self::Inherit)
    }

    /// The explicit per-request prompt text, if any (`Set` only).
    #[must_use]
    pub fn as_set_prompt(&self) -> Option<&str> {
        match self {
            Self::Set(prompt) => Some(prompt.as_str()),
            Self::Inherit | Self::Disable => None,
        }
    }
}

const SYSTEM_PROMPT_OVERRIDE_DISABLE_ACTION: &str = "disable";

impl Serialize for SystemPromptOverride {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            // Inherit is the default; field sites pair this with
            // `skip_serializing_if = "SystemPromptOverride::is_inherit"` so it
            // is normally omitted entirely.
            Self::Inherit => serializer.serialize_none(),
            Self::Set(prompt) => serializer.serialize_str(prompt),
            Self::Disable => {
                use serde::ser::SerializeMap;
                let mut map = serializer.serialize_map(Some(1))?;
                map.serialize_entry("action", SYSTEM_PROMPT_OVERRIDE_DISABLE_ACTION)?;
                map.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for SystemPromptOverride {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct SystemPromptOverrideVisitor;

        impl<'de> serde::de::Visitor<'de> for SystemPromptOverrideVisitor {
            type Value = SystemPromptOverride;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str(
                    "a system prompt string, null (inherit), or {\"action\": \"disable\"}",
                )
            }

            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
                Ok(SystemPromptOverride::Set(value.to_owned()))
            }

            fn visit_string<E: serde::de::Error>(self, value: String) -> Result<Self::Value, E> {
                Ok(SystemPromptOverride::Set(value))
            }

            fn visit_none<E: serde::de::Error>(self) -> Result<Self::Value, E> {
                Ok(SystemPromptOverride::Inherit)
            }

            fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
                Ok(SystemPromptOverride::Inherit)
            }

            fn visit_some<D2: Deserializer<'de>>(
                self,
                deserializer: D2,
            ) -> Result<Self::Value, D2::Error> {
                deserializer.deserialize_any(SystemPromptOverrideVisitor)
            }

            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<Self::Value, A::Error> {
                let mut action: Option<String> = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "action" => {
                            if action.is_some() {
                                return Err(serde::de::Error::duplicate_field("action"));
                            }
                            action = Some(map.next_value()?);
                        }
                        other => {
                            return Err(serde::de::Error::unknown_field(other, &["action"]));
                        }
                    }
                }
                let action = action.ok_or_else(|| serde::de::Error::missing_field("action"))?;
                if action == SYSTEM_PROMPT_OVERRIDE_DISABLE_ACTION {
                    Ok(SystemPromptOverride::Disable)
                } else {
                    Err(serde::de::Error::custom(format!(
                        "unknown system_prompt override action '{action}' (expected \"disable\")"
                    )))
                }
            }
        }

        deserializer.deserialize_any(SystemPromptOverrideVisitor)
    }
}

impl JsonSchema for SystemPromptOverride {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "SystemPromptOverride".into()
    }

    fn json_schema(_generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "description": "Per-request system-prompt policy: omit/null to inherit, a string to set an explicit prompt, or {\"action\": \"disable\"} to suppress every prompt source.",
            "anyOf": [
                { "type": "null", "description": "Inherit the configured/default prompt sources." },
                { "type": "string", "description": "Explicit per-request system prompt." },
                {
                    "type": "object",
                    "properties": { "action": { "const": "disable" } },
                    "required": ["action"],
                    "additionalProperties": false,
                    "description": "Suppress every prompt source."
                }
            ]
        })
    }
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
    /// Tri-state call-timeout override for per-LLM-call timeout policy.
    ///
    /// - `Inherit` (default / omitted): defer to profile-derived or build-override default
    /// - `Disabled`: explicitly disable call timeout
    /// - `Value(duration)`: explicitly set call timeout
    #[serde(
        default,
        rename = "call_timeout",
        skip_serializing_if = "CallTimeoutOverride::is_inherit"
    )]
    pub call_timeout_override: CallTimeoutOverride,
}

impl Default for RetryConfig {
    fn default() -> Self {
        let policy = RetryPolicy::default();
        Self {
            max_retries: policy.max_retries,
            initial_delay: policy.initial_delay,
            max_delay: policy.max_delay,
            multiplier: policy.multiplier,
            call_timeout_override: CallTimeoutOverride::default(),
        }
    }
}

impl From<RetryConfig> for RetryPolicy {
    fn from(config: RetryConfig) -> Self {
        // Resolve explicit config override into effective call_timeout.
        // `Inherit` means None here — the agent loop resolves profile defaults later.
        let call_timeout = match config.call_timeout_override {
            CallTimeoutOverride::Inherit => None,
            CallTimeoutOverride::Disabled => None,
            CallTimeoutOverride::Value(d) => Some(d),
        };
        RetryPolicy {
            max_retries: config.max_retries,
            initial_delay: config.initial_delay,
            max_delay: config.max_delay,
            multiplier: config.multiplier,
            call_timeout,
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
    /// Mob (multi-agent orchestration) tools enabled
    pub mob_enabled: bool,
    /// Scheduler tools enabled
    pub schedule_enabled: bool,
    /// WorkGraph tools enabled
    pub workgraph_enabled: bool,
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
            mob_enabled: false,
            schedule_enabled: true,
            workgraph_enabled: false,
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
    /// Max number of background hook tasks allowed to run concurrently.
    pub background_max_concurrency: usize,
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
            background_max_concurrency: 32,
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
    pub timeout_ms: Option<u64>,
    pub runtime: HookAdapterConfig,
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
            timeout_ms: None,
            runtime: HookAdapterConfig::in_process("noop"),
        }
    }
}

/// Stable identity for an in-process hook handler registered with the runtime.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Hash)]
#[serde(transparent)]
pub struct HookInProcessHandlerId(String);

impl HookInProcessHandlerId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for HookInProcessHandlerId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl From<&str> for HookInProcessHandlerId {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl From<String> for HookInProcessHandlerId {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl Default for HookInProcessHandlerId {
    fn default() -> Self {
        Self::new("noop")
    }
}

/// Typed payload for [`HookRuntimeKind::InProcess`].
///
/// `name` remains accepted for existing configs, but runtime dispatch uses the
/// typed handler id rather than fishing a string out of opaque adapter config.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(default)]
pub struct HookInProcessRuntimeConfig {
    #[serde(alias = "name")]
    pub handler: HookInProcessHandlerId,
}

impl HookInProcessRuntimeConfig {
    pub fn new(handler: impl Into<HookInProcessHandlerId>) -> Self {
        Self {
            handler: handler.into(),
        }
    }
}

impl Default for HookInProcessRuntimeConfig {
    fn default() -> Self {
        Self::new("noop")
    }
}

/// Discriminant naming the runtime adapter a [`HookAdapterConfig`] selects.
///
/// This is a pure derivation of the [`HookAdapterConfig`] variant
/// ([`HookAdapterConfig::kind`]); it carries no payload and exists only for
/// wire/logging labels and for the `(kind, value)` convenience constructor
/// [`HookAdapterConfig::from_kind_and_value`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HookRuntimeKind {
    InProcess,
    Command,
    Http,
}

impl HookRuntimeKind {
    /// Canonical wire/logging string for this runtime kind.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::InProcess => "in_process",
            Self::Command => "command",
            Self::Http => "http",
        }
    }

    /// Parse the canonical wire form. Returns `None` for unrecognized strings.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "in_process" => Some(Self::InProcess),
            "command" => Some(Self::Command),
            "http" => Some(Self::Http),
            _ => None,
        }
    }
}

impl std::fmt::Display for HookRuntimeKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Typed payload for a [`HookRuntimeKind::Command`] adapter.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct CommandRuntimeConfig {
    pub command: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub env: HashMap<String, String>,
}

fn default_http_method() -> String {
    "POST".to_string()
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// Typed payload for a [`HookRuntimeKind::Http`] adapter.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct HttpRuntimeConfig {
    pub url: String,
    #[serde(default = "default_http_method")]
    pub method: String,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub headers: HashMap<String, String>,
}

/// Closed, typed set of hook runtime adapters the engine can dispatch to.
///
/// This is the single typed owner of "which runtime adapter and its config".
/// It is deserialized once at the config-layering boundary; the hook engine
/// reads the typed variant directly rather than re-parsing an opaque JSON
/// payload on every execution. A malformed command/HTTP payload therefore fails
/// closed at config-deserialization time, not at hook-execution time.
///
/// Wire format is internally tagged on `type` (`in_process`, `command`,
/// `http`), with the variant payload flattened alongside the tag, e.g.
/// `{"type":"command","command":"sh","args":["-c","..."]}`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HookAdapterConfig {
    InProcess(HookInProcessRuntimeConfig),
    Command(CommandRuntimeConfig),
    Http(HttpRuntimeConfig),
}

impl HookAdapterConfig {
    /// Build an in-process adapter referencing a registered handler id.
    pub fn in_process(handler: impl Into<HookInProcessHandlerId>) -> Self {
        Self::InProcess(HookInProcessRuntimeConfig::new(handler))
    }

    /// Build a command adapter.
    pub fn command(
        command: impl Into<String>,
        args: Vec<String>,
        env: HashMap<String, String>,
    ) -> Self {
        Self::Command(CommandRuntimeConfig {
            command: command.into(),
            args,
            env,
        })
    }

    /// Build an HTTP adapter.
    pub fn http(
        url: impl Into<String>,
        method: impl Into<String>,
        headers: HashMap<String, String>,
    ) -> Self {
        Self::Http(HttpRuntimeConfig {
            url: url.into(),
            method: method.into(),
            headers,
        })
    }

    /// Deserialize a `(kind, flattened-payload)` pair into the typed adapter.
    ///
    /// The payload is the variant's fields without the `type` tag (e.g.
    /// `{"command":"sh"}` for [`HookRuntimeKind::Command`]); a `None`/`Null`
    /// payload is treated as an empty object so defaulted fields apply. The
    /// payload is validated here, at the config boundary, and fails closed on
    /// an unknown/malformed shape.
    pub fn from_kind_and_value(
        kind: HookRuntimeKind,
        config: Option<Value>,
    ) -> Result<Self, serde_json::Error> {
        let mut obj = match config {
            Some(Value::Object(obj)) => obj,
            Some(Value::Null) | None => Map::new(),
            Some(other) => {
                let mut obj = Map::new();
                obj.insert("config".to_string(), other);
                obj
            }
        };
        obj.insert("type".to_string(), Value::String(kind.as_str().to_string()));
        serde_json::from_value(Value::Object(obj))
    }

    /// Pure-derivation discriminant of this adapter.
    pub fn kind(&self) -> HookRuntimeKind {
        match self {
            Self::InProcess(_) => HookRuntimeKind::InProcess,
            Self::Command(_) => HookRuntimeKind::Command,
            Self::Http(_) => HookRuntimeKind::Http,
        }
    }
}

impl Default for HookAdapterConfig {
    fn default() -> Self {
        Self::in_process("noop")
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

    #[error("Internal error: {0}")]
    InternalError(String),

    #[error("Validation error: {0}")]
    Validation(String),

    #[error("realm inheritance chain error: {0}")]
    RealmChain(#[from] crate::connection::RealmChainError),
}

/// Compose the effective flat [`Config`] for `head` by folding the per-realm
/// config docs along `head`'s parent chain, root-first / child-wins.
///
/// `docs` is the set of fetched per-realm Configs keyed by realm id. An absent
/// ancestor simply contributes nothing — it is NOT a `Config::default` clobber
/// (the caller passes only the docs it actually found; see
/// [`crate::config_store::RealmConfigSource`]). [`crate::connection::RealmChain`]
/// is the single chain authority (cycle/depth/global/env_default validation),
/// so this composition order can never diverge from the connection resolvers'
/// chain order. The fold reuses the one [`Config::merge`] engine.
pub fn compose_effective_config(
    docs: &std::collections::BTreeMap<crate::connection::RealmId, Config>,
    head: &crate::connection::RealmId,
) -> Result<Config, crate::connection::RealmChainError> {
    use crate::connection::RealmChain;
    // Combined topology: union every fetched doc's realm sections so the chain
    // authority sees each member's parent edge regardless of which doc carried
    // it. Outer-key only — sub-maps are never merged across realms.
    let mut topology = Config::default();
    for doc in docs.values() {
        for (realm_id, section) in &doc.realm {
            topology.realm.insert(realm_id.clone(), section.clone());
        }
    }
    let chain = RealmChain::resolve(&topology, head)?;
    // Fold root-first (chain is head-first, so iterate reversed) — the
    // most-derived head merges last and wins.
    let mut effective = Config::default();
    let default_self_hosted = SelfHostedConfig::default();
    let default_provider_tools = ProviderToolsConfig::default();
    for member in chain.realms().iter().rev() {
        if let Some(doc) = docs.get(member) {
            effective.merge(doc.clone());
            // `Config::merge` carries self_hosted/provider_tools only via the
            // toml-presence layering path (merge_toml_str), not the in-memory
            // fold. Compose folds them whole-section child-wins here so a
            // composed config never silently drops them.
            if doc.self_hosted != default_self_hosted {
                effective.self_hosted = doc.self_hosted.clone();
            }
            if doc.provider_tools != default_provider_tools {
                effective.provider_tools = doc.provider_tools.clone();
            }
        }
    }
    // NOTE (Finding D, non-blocking per adversarial review): `effective.realm`
    // is the outer-key union of every folded doc's sections (via Config::merge),
    // so a doc that hand-places a foreign `[realm.X]` (X != its owning realm)
    // could shadow the authoritative ancestor section. Reachability is narrow —
    // the standard tooling reads `global` from a dedicated home-rooted doc and
    // each realm from its own path, never co-locating a foreign section — and a
    // strict owner-only rebuild is INCOMPATIBLE with the supported
    // single-file-multiple-realms model (one doc carrying `[realm.a]`,
    // `[realm.b]`, ... is how the CLI and surfaces resolve a `--realm`/section
    // that is not itself a chain member). Tracked, not gated.
    Ok(effective)
}

/// Serde helpers for Option<Duration> with humantime format
mod optional_duration_serde {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::time::Duration;

    // Signature constrained by serde's `#[serde(with)]` contract:
    // `serialize_with` passes `&Option<T>`, not `Option<&T>`.
    #[allow(clippy::ref_option)]
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
    if let Ok(cwd) = std::env::current_dir()
        && let Some(root) = find_project_root(&cwd)
    {
        return Some(root.join(".rkat"));
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
    use crate::Provider;

    #[test]
    fn test_config_default() {
        let config = Config::default();
        assert!(
            config.agent.model.is_empty(),
            "core embeds no provider data: the default agent model is empty \
             and resolves through the injected catalog at build time"
        );
        // The field is `None` by default (presence semantics for realm
        // inheritance); the operative default resolves to the template's 16384.
        assert_eq!(config.agent.max_tokens_per_turn, None);
        assert_eq!(config.agent.resolved_max_tokens_per_turn(), 16384);
        assert_eq!(config.retry.max_retries, 3);
        assert_eq!(config.max_sessions(), DEFAULT_MAX_SESSIONS);
    }

    #[test]
    fn config_template_pins_no_model() {
        let config = Config::template().expect("template parses");
        assert!(
            config.agent.model.is_empty(),
            "the core config template must not pin a provider model; defaults \
             come from the injected catalog (meerkat-models)"
        );
        assert!(config.models.anthropic.is_empty());
        assert!(config.models.openai.is_empty());
        assert!(config.models.gemini.is_empty());
    }

    #[test]
    fn test_limits_max_sessions_configures_runtime_capacity() {
        let mut config = Config::default();
        config
            .merge_toml_str(
                r"
[limits]
max_sessions = 7
",
            )
            .expect("merge max_sessions");
        assert_eq!(config.max_sessions(), 7);
    }

    #[test]
    fn test_config_layering() {
        // 1. Test defaults
        let config = Config::default();
        assert!(config.agent.model.is_empty());
        assert_eq!(config.budget.max_tokens, None);

        // 2. Test env override (secrets only)
        {
            // Plan §6.9 deleted the `config.provider = ProviderConfig::X`
            // mutable sink. apply_env_overrides_from is now a no-op;
            // env-var-based credential resolution happens at resolve
            // time in the provider-runtime registry. This branch retains
            // the call-site shape so the merge precedence test passes.
            let env = std::collections::HashMap::from([(
                "RKAT_MODEL".to_string(),
                "env-model".to_string(),
            )]);
            let mut config = Config::default();
            config
                .apply_env_overrides_from(|key| env.get(key).cloned())
                .expect("apply env overrides");
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
    fn test_merge_extraction_prompt_survives_layering() {
        let mut base = Config::default();
        assert!(base.agent.extraction_prompt.is_none());

        // Merge from TOML config file
        let toml = r#"
[agent]
extraction_prompt = "Return JSON only."
"#;
        base.merge_toml_str(toml).expect("merge toml");
        assert_eq!(
            base.agent.extraction_prompt.as_deref(),
            Some("Return JSON only.")
        );

        // A second merge without the field should preserve it
        let toml2 = r#"
[agent]
model = "custom-model"
"#;
        base.merge_toml_str(toml2).expect("merge toml2");
        assert_eq!(
            base.agent.extraction_prompt.as_deref(),
            Some("Return JSON only."),
            "extraction_prompt must survive merge when absent in later layer"
        );
        assert_eq!(base.agent.model, "custom-model");
    }

    #[test]
    fn test_merge_hooks_entries_append() {
        let mut base = Config::default();
        let base_entry = HookEntryConfig {
            id: HookId::new("base"),
            ..HookEntryConfig::default()
        };
        base.hooks.entries.push(base_entry);

        let mut other = Config::default();
        let other_entry = HookEntryConfig {
            id: HookId::new("other"),
            ..HookEntryConfig::default()
        };
        other.hooks.entries.push(other_entry);

        base.merge(other);
        let ids = base
            .hooks
            .entries
            .iter()
            .map(|entry| entry.id.0.as_str())
            .collect::<Vec<_>>();
        assert_eq!(ids, vec!["base", "other"]);
    }

    // ---- P3a: Config::merge inheritance reworks ---------------------------

    fn mcp_server(name: &str, command: &str) -> crate::mcp_config::McpServerConfig {
        crate::mcp_config::McpServerConfig {
            name: name.to_string(),
            transport: crate::mcp_config::McpTransportConfig::Stdio(
                crate::mcp_config::McpStdioConfig {
                    command: command.to_string(),
                    args: Vec::new(),
                    env: std::collections::HashMap::new(),
                },
            ),
            connect_timeout_secs: None,
        }
    }

    fn skill_repo(
        name: &str,
        uuid: &str,
        path: &str,
    ) -> crate::skills_config::SkillRepositoryConfig {
        crate::skills_config::SkillRepositoryConfig {
            name: name.to_string(),
            source_uuid: crate::skills::SourceUuid::parse(uuid).expect("valid uuid"),
            transport: crate::skills_config::SkillRepoTransport::Filesystem {
                path: path.to_string(),
            },
        }
    }

    // RCT-12
    #[test]
    fn effective_config_unions_model_defaults_child_wins() {
        let mut base = Config::default();
        base.models.openai = "parent-openai".to_string();
        base.models.anthropic = "parent-anthropic".to_string();
        let mut child = Config::default();
        child.models.anthropic = "child-anthropic".to_string();
        base.merge(child);
        assert_eq!(
            base.models.openai, "parent-openai",
            "child inherits the parent's openai default"
        );
        assert_eq!(
            base.models.anthropic, "child-anthropic",
            "child overrides only its anthropic default"
        );
    }

    // RCT-14
    #[test]
    fn effective_config_unions_mcp_limits_skills() {
        let mut base = Config::default();
        base.limits.budget = Some(1000);
        base.tools.mcp_servers = vec![mcp_server("shared", "base-cmd")];
        base.skills.repositories = vec![skill_repo(
            "base-repo",
            "00000000-0000-4000-8000-000000000001",
            "/b",
        )];

        let mut child = Config::default();
        child.limits.max_sessions = Some(7);
        child.tools.mcp_servers = vec![mcp_server("shared", "child-cmd"), mcp_server("extra", "x")];
        child.skills.repositories = vec![skill_repo(
            "child-repo",
            "00000000-0000-4000-8000-000000000002",
            "/c",
        )];

        base.merge(child);

        // limits: per-field child-wins (both survive).
        assert_eq!(base.limits.budget, Some(1000));
        assert_eq!(base.limits.max_sessions, Some(7));

        // mcp: union by name, child overrides same name, appends new.
        let names: Vec<&str> = base
            .tools
            .mcp_servers
            .iter()
            .map(|s| s.name.as_str())
            .collect();
        assert_eq!(names, vec!["shared", "extra"]);
        let shared = base
            .tools
            .mcp_servers
            .iter()
            .find(|s| s.name == "shared")
            .unwrap();
        assert!(
            matches!(
                &shared.transport,
                crate::mcp_config::McpTransportConfig::Stdio(c) if c.command == "child-cmd"
            ),
            "child overrides the same-named inherited server"
        );

        // skills: repositories append parent-first.
        let repos: Vec<&str> = base
            .skills
            .repositories
            .iter()
            .map(|r| r.name.as_str())
            .collect();
        assert_eq!(repos, vec!["base-repo", "child-repo"]);
    }

    // RCT-15
    #[test]
    fn config_merge_folds_realm_map_child_wins() {
        let mut base = Config::default();
        base.realm.insert(
            "global".to_string(),
            crate::connection::RealmConfigSection::default(),
        );
        let mut child = Config::default();
        child.realm.insert(
            "team".to_string(),
            crate::connection::RealmConfigSection {
                parent: Some(crate::connection::RealmId::global()),
                ..Default::default()
            },
        );
        base.merge(child);
        assert!(base.realm.contains_key("global"), "inherited realm visible");
        assert!(base.realm.contains_key("team"));
        assert_eq!(
            base.realm.get("team").and_then(|s| s.parent.clone()),
            Some(crate::connection::RealmId::global())
        );
    }

    // RCT-37
    #[test]
    fn child_cannot_remove_inherited_mcp_or_hook_entries() {
        let mut base = Config::default();
        base.tools.mcp_servers = vec![mcp_server("inherited", "cmd")];
        base.hooks.entries.push(HookEntryConfig {
            id: HookId::new("inherited-hook"),
            ..HookEntryConfig::default()
        });

        // Child sets nothing: an empty child must NOT drop inherited entries
        // (emptiness != removal, no tombstones).
        let child = Config::default();
        base.merge(child);

        assert_eq!(
            base.tools.mcp_servers.len(),
            1,
            "empty child must not remove an inherited mcp server"
        );
        assert_eq!(base.tools.mcp_servers[0].name, "inherited");
        assert!(
            base.hooks
                .entries
                .iter()
                .any(|h| h.id.0.as_str() == "inherited-hook"),
            "empty child must not remove an inherited hook"
        );
    }

    #[test]
    fn test_merge_self_hosted_preserves_lower_layer_servers_and_models() {
        let mut base = Config::default();
        base.merge_toml_str(
            r#"
[self_hosted.servers.local]
base_url = "http://127.0.0.1:11434"
"#,
        )
        .expect("base self-hosted server");
        base.merge_toml_str(
            r#"
[self_hosted.models.gemma-4-e2b]
server = "local"
remote_model = "gemma4:e2b"
display_name = "Gemma 4 E2B"
family = "gemma-4"
"#,
        )
        .expect("overlay self-hosted model");

        assert!(base.self_hosted.servers.contains_key("local"));
        assert!(base.self_hosted.models.contains_key("gemma-4-e2b"));
        let registry = base
            .model_registry(*crate::model_profile::test_catalog::TEST_CATALOG)
            .expect("merged self-hosted registry");
        assert_eq!(
            registry
                .entry("gemma-4-e2b")
                .and_then(|entry| entry.self_hosted.as_ref())
                .map(|server| server.server_id.as_str()),
            Some("local")
        );
    }

    #[test]
    fn test_merge_self_hosted_partial_server_override_preserves_existing_fields() {
        let mut config = Config::default();
        config
            .merge_toml_str(
                r#"
[self_hosted.servers.local]
base_url = "http://127.0.0.1:11434"
api_style = "responses"
"#,
            )
            .expect("base server");
        config
            .merge_toml_str(
                r#"
[self_hosted.servers.local]
transport = "openai_compatible"
"#,
            )
            .expect("overlay server");

        let server = config
            .self_hosted
            .servers
            .get("local")
            .expect("merged server");
        assert_eq!(server.base_url, "http://127.0.0.1:11434");
        assert_eq!(server.api_style, SelfHostedApiStyle::Responses);
        assert_eq!(server.transport, SelfHostedTransport::OpenAiCompatible);
    }

    #[test]
    fn test_merge_self_hosted_partial_override_preserves_unrelated_inherited_entries() {
        let mut config = Config::default();
        config
            .merge_toml_str(
                r#"
[self_hosted]
default_model = "gemma-4-e4b"

[self_hosted.servers.local]
base_url = "http://127.0.0.1:11434"

[self_hosted.servers.backup]
base_url = "http://127.0.0.1:11435"

[self_hosted.models.gemma-4-e2b]
server = "local"
remote_model = "gemma4:e2b"
display_name = "Gemma 4 E2B"
family = "gemma-4"

[self_hosted.models.gemma-4-e4b]
server = "backup"
remote_model = "gemma4:e4b"
display_name = "Gemma 4 E4B"
family = "gemma-4"
"#,
            )
            .expect("base self-hosted config");
        config
            .merge_toml_str(
                r#"
[self_hosted.servers.local]
api_style = "responses"
"#,
            )
            .expect("overlay self-hosted config");

        assert!(config.self_hosted.servers.contains_key("backup"));
        assert!(config.self_hosted.models.contains_key("gemma-4-e4b"));
        let registry = config
            .model_registry(*crate::model_profile::test_catalog::TEST_CATALOG)
            .expect("registry should remain valid");
        assert_eq!(
            registry
                .entry("gemma-4-e4b")
                .and_then(|entry| entry.self_hosted.as_ref())
                .map(|server| server.server_id.as_str()),
            Some("backup")
        );
    }

    #[test]
    fn test_merge_self_hosted_empty_table_clears_inherited_entries() {
        let mut config = Config::default();
        config
            .merge_toml_str(
                r#"
[self_hosted.servers.local]
base_url = "http://127.0.0.1:11434"

[self_hosted.models.gemma-4-e2b]
server = "local"
remote_model = "gemma4:e2b"
display_name = "Gemma 4 E2B"
family = "gemma-4"
"#,
            )
            .expect("base self-hosted config");

        config
            .merge_toml_str(
                r"
[self_hosted.servers]

[self_hosted.models]
",
            )
            .expect("clear self-hosted config");

        assert!(config.self_hosted.servers.is_empty());
        assert!(config.self_hosted.models.is_empty());
    }

    #[test]
    fn test_self_hosted_legacy_bearer_token_rejected_at_parse() {
        // Pre-1.0 clean break: server credentials live exclusively in realm
        // auth profiles. The retired `bearer_token` carrier is rejected at
        // config parse with a typed error, never tolerated.
        let err = toml::from_str::<Config>(
            r#"
[self_hosted.servers.local]
base_url = "http://127.0.0.1:11434"
bearer_token = "secret-token"
"#,
        )
        .expect_err("legacy bearer_token must be rejected at config parse");
        assert!(
            err.to_string().contains("bearer_token"),
            "rejection must name the offending field: {err}"
        );
    }

    #[test]
    fn test_self_hosted_legacy_bearer_token_env_rejected_at_parse() {
        let err = toml::from_str::<Config>(
            r#"
[self_hosted.servers.local]
base_url = "http://127.0.0.1:11434"
bearer_token_env = "OLLAMA_TOKEN"
"#,
        )
        .expect_err("legacy bearer_token_env must be rejected at config parse");
        assert!(
            err.to_string().contains("bearer_token_env"),
            "rejection must name the offending field: {err}"
        );
    }

    #[test]
    fn test_self_hosted_legacy_bearer_token_rejected_in_layered_merge() {
        // The layered merge path parses each overlay through the same typed
        // Config deserializer, so a legacy credential field in any layer is
        // rejected with the same typed parse error.
        let mut config = Config::default();
        config
            .merge_toml_str(
                r#"
[self_hosted.servers.local]
base_url = "http://127.0.0.1:11434"
"#,
            )
            .expect("base server");
        let err = config
            .merge_toml_str(
                r#"
[self_hosted.servers.local]
bearer_token_env = "OLLAMA_TOKEN"
"#,
            )
            .expect_err("legacy bearer_token_env overlay must be rejected");
        assert!(matches!(err, ConfigError::Parse(_)));
        assert!(err.to_string().contains("bearer_token_env"));
    }

    // Plan §6.10 deleted the ProviderSettings struct (and its api_keys /
    // base_urls maps) entirely. The corresponding merge test that
    // asserted the "non-default other replaces self" semantics went
    // with it. Realm-scoped base_urls now live in
    // `[realm.<id>.backend.<b>.base_url]` and round-trip through the
    // normal TOML merge path that the
    // `test_merge_extraction_prompt_survives_layering` case exercises.

    #[test]
    fn test_merge_toml_tools_omitted_fields_preserve_lower_layer() {
        let mut config = Config::default();
        config.tools.mob_enabled = true;
        config.tools.shell_enabled = true;

        config
            .merge_toml_str(
                r"
[tools]
shell_enabled = false
",
            )
            .expect("merge should succeed");

        assert!(config.tools.mob_enabled);
        assert!(!config.tools.shell_enabled);
    }

    #[test]
    fn test_merge_toml_tools_explicit_default_overrides_lower_layer() {
        let mut config = Config::default();
        config.tools.mob_enabled = true;

        config
            .merge_toml_str(
                r"
[tools]
mob_enabled = false
",
            )
            .expect("merge should succeed");

        assert!(!config.tools.mob_enabled);
    }

    #[test]
    fn test_merge_toml_retry_omitted_fields_preserve_lower_layer() {
        let mut config = Config::default();
        config.retry.max_retries = 9;

        config
            .merge_toml_str(
                r#"
[retry]
initial_delay = "750ms"
"#,
            )
            .expect("merge should succeed");

        assert_eq!(config.retry.max_retries, 9);
        assert_eq!(config.retry.initial_delay, Duration::from_millis(750));
    }

    #[test]
    fn test_compaction_threshold_presence_is_preserved_at_default_value() {
        let config: Config = toml::from_str(
            r"
[compaction]
auto_compact_threshold = 100000
",
        )
        .expect("config should parse");

        assert_eq!(config.compaction.auto_compact_threshold, 100_000);
        assert!(config.compaction.auto_compact_threshold_explicit);
    }

    #[test]
    fn test_default_compaction_threshold_serializes_as_inherited() {
        let toml = toml::to_string_pretty(&Config::default()).expect("config should serialize");

        assert!(
            !toml.contains("auto_compact_threshold"),
            "default config should not persist an inherited compaction threshold: {toml}"
        );
    }

    #[test]
    fn test_explicit_default_compaction_threshold_serializes() {
        let mut config = Config::default();
        config.compaction.auto_compact_threshold_explicit = true;

        let toml = toml::to_string_pretty(&config).expect("config should serialize");

        assert!(
            toml.contains("auto_compact_threshold = 100000"),
            "explicit default threshold must survive persistence: {toml}"
        );
    }

    #[test]
    fn test_validate_rejects_zero_min_turns_between_compactions() {
        let config = Config {
            compaction: CompactionRuntimeConfig {
                min_turns_between_compactions: 0,
                ..CompactionRuntimeConfig::default()
            },
            ..Config::default()
        };
        let err = config
            .validate(*crate::model_profile::test_catalog::TEST_CATALOG)
            .expect_err("min_turns_between_compactions=0 should be invalid");
        assert!(
            err.to_string()
                .contains("compaction.min_turns_between_compactions")
        );
    }

    // Plan §6.9 deleted the ProviderConfig enum and the
    // `test_provider_config_serialization` test that exercised its
    // serde discriminator. Realm-based credential configs are round-
    // tripped by tests in meerkat-contracts/tests/auth_binding_wire.rs.

    #[test]
    fn test_budget_config_serialization() {
        let budget = BudgetConfig {
            max_tokens: Some(100_000),
            max_duration: Some(Duration::from_secs(300)),
            max_tool_calls: Some(50),
        };

        let json = serde_json::to_string(&budget).unwrap();
        let parsed: BudgetConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.max_tokens, Some(100_000));
        assert_eq!(parsed.max_duration, Some(Duration::from_secs(300)));
        assert_eq!(parsed.max_tool_calls, Some(50));
    }

    #[test]
    fn test_self_hosted_transport_accepts_openai_compatible_alias() {
        let mut config = Config::default();
        config
            .merge_toml_str(
                r#"
[self_hosted.servers.ollama]
transport = "openai_compatible"
base_url = "http://127.0.0.1:11434"
api_style = "chat_completions"
"#,
            )
            .expect("alias should parse");

        assert_eq!(
            config
                .self_hosted
                .servers
                .get("ollama")
                .expect("server should exist")
                .transport,
            SelfHostedTransport::OpenAiCompatible
        );
    }

    #[test]
    fn test_self_hosted_server_config_defaults_to_chat_completions() {
        assert_eq!(
            SelfHostedServerConfig::default().api_style,
            SelfHostedApiStyle::ChatCompletions
        );
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

    #[test]
    fn test_validate_rejects_zero_max_tokens() {
        let config = Config {
            max_tokens: Some(0),
            ..Config::default()
        };
        let err = config
            .validate(*crate::model_profile::test_catalog::TEST_CATALOG)
            .expect_err("max_tokens=0 should be invalid");
        assert!(
            err.to_string()
                .contains("max_tokens must be greater than 0")
        );
    }

    #[test]
    fn test_validate_rejects_zero_limits_max_sessions() {
        let mut config = Config::default();
        config.limits.max_sessions = Some(0);
        let err = config
            .validate(*crate::model_profile::test_catalog::TEST_CATALOG)
            .expect_err("limits.max_sessions=0 should be invalid");
        assert!(err.to_string().contains("limits.max_sessions"));
    }

    #[test]
    fn test_validate_rejects_zero_agent_max_tokens_per_turn() {
        let mut config = Config::default();
        config.agent.max_tokens_per_turn = Some(0);
        let err = config
            .validate(*crate::model_profile::test_catalog::TEST_CATALOG)
            .expect_err("agent.max_tokens_per_turn=0 should be invalid");
        assert!(err.to_string().contains("agent.max_tokens_per_turn"));
    }

    // Plan §6.9 deleted the `config.provider = ProviderConfig::X` block
    // and the matching validate-time conflict checks against
    // `providers.{base_urls,api_keys}`. Those tests went with it.

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

    // === CommsAuthMode tests ===

    #[test]
    fn test_comms_auth_mode_default_is_open() {
        assert_eq!(CommsAuthMode::default(), CommsAuthMode::Open);
    }

    #[test]
    fn test_comms_auth_mode_serde_roundtrip() {
        // Open serializes as "none"
        let json = serde_json::to_string(&CommsAuthMode::Open).unwrap();
        assert_eq!(json, r#""none""#);
        let parsed: CommsAuthMode = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, CommsAuthMode::Open);

        // Ed25519 serializes as "ed25519"
        let json = serde_json::to_string(&CommsAuthMode::Ed25519).unwrap();
        assert_eq!(json, r#""ed25519""#);
        let parsed: CommsAuthMode = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, CommsAuthMode::Ed25519);
    }

    #[test]
    fn test_comms_auth_mode_toml_roundtrip() {
        let config = CommsRuntimeConfig::default();
        let toml_str = toml::to_string(&config).unwrap();
        let parsed: CommsRuntimeConfig = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed.auth, CommsAuthMode::Open);
        assert!(parsed.require_peer_auth);

        // Explicit ed25519
        let toml_str = r#"
mode = "inproc"
auth = "ed25519"
"#;
        let parsed: CommsRuntimeConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(parsed.auth, CommsAuthMode::Ed25519);
        assert!(parsed.require_peer_auth);
    }

    #[test]
    fn test_comms_runtime_config_default_has_open_auth() {
        let config = CommsRuntimeConfig::default();
        assert_eq!(config.auth, CommsAuthMode::Open);
        assert!(config.require_peer_auth);
    }

    // === PlainEventSource tests ===

    #[test]
    fn test_plain_event_source_serde_roundtrip() {
        let cases = [
            (PlainEventSource::Tcp, r#""tcp""#),
            (PlainEventSource::Uds, r#""uds""#),
            (PlainEventSource::Stdin, r#""stdin""#),
            (PlainEventSource::Webhook, r#""webhook""#),
            (PlainEventSource::Rpc, r#""rpc""#),
        ];
        for (variant, expected_json) in cases {
            let json = serde_json::to_string(&variant).unwrap();
            assert_eq!(json, expected_json, "serialize {variant:?}");
            let parsed: PlainEventSource = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, variant, "deserialize {variant:?}");
        }
    }

    #[test]
    fn test_plain_event_source_display() {
        assert_eq!(PlainEventSource::Tcp.to_string(), "tcp");
        assert_eq!(PlainEventSource::Uds.to_string(), "uds");
        assert_eq!(PlainEventSource::Stdin.to_string(), "stdin");
        assert_eq!(PlainEventSource::Webhook.to_string(), "webhook");
        assert_eq!(PlainEventSource::Rpc.to_string(), "rpc");
    }

    // === Regression: event_address in CommsRuntimeConfig ===

    #[test]
    fn test_comms_config_event_address_toml_roundtrip() {
        let toml_str = r#"
mode = "tcp"
address = "127.0.0.1:4200"
advertise_address = "tcp://203.0.113.10:4200"
auth = "none"
require_peer_auth = false
event_address = "127.0.0.1:4201"
"#;
        let parsed: CommsRuntimeConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(parsed.event_address.as_deref(), Some("127.0.0.1:4201"));
        assert_eq!(
            parsed.advertise_address.as_deref(),
            Some("tcp://203.0.113.10:4200")
        );
        assert_eq!(parsed.auth, CommsAuthMode::Open);
        assert!(!parsed.require_peer_auth);
    }

    #[test]
    fn test_comms_config_event_address_defaults_none() {
        let config = CommsRuntimeConfig::default();
        assert!(config.event_address.is_none());
    }

    // ── CallTimeoutOverride tests ──

    #[test]
    fn call_timeout_override_default_is_inherit() {
        assert_eq!(CallTimeoutOverride::default(), CallTimeoutOverride::Inherit);
        assert!(CallTimeoutOverride::default().is_inherit());
    }

    #[test]
    fn call_timeout_override_disabled_is_not_inherit() {
        assert!(!CallTimeoutOverride::Disabled.is_inherit());
    }

    #[test]
    fn call_timeout_override_value_is_not_inherit() {
        assert!(!CallTimeoutOverride::Value(Duration::from_secs(45)).is_inherit());
    }

    #[test]
    fn call_timeout_override_toml_deserialize_disabled() {
        let toml_str = r#"call_timeout = "disabled""#;
        #[derive(Deserialize)]
        struct Wrapper {
            call_timeout: CallTimeoutOverride,
        }
        let w: Wrapper = toml::from_str(toml_str).unwrap();
        assert_eq!(w.call_timeout, CallTimeoutOverride::Disabled);
    }

    #[test]
    fn call_timeout_override_toml_deserialize_duration() {
        let toml_str = r#"call_timeout = "45s""#;
        #[derive(Deserialize)]
        struct Wrapper {
            call_timeout: CallTimeoutOverride,
        }
        let w: Wrapper = toml::from_str(toml_str).unwrap();
        assert_eq!(
            w.call_timeout,
            CallTimeoutOverride::Value(Duration::from_secs(45))
        );
    }

    #[test]
    fn call_timeout_override_toml_deserialize_complex_duration() {
        let toml_str = r#"call_timeout = "2m 30s""#;
        #[derive(Deserialize)]
        struct Wrapper {
            call_timeout: CallTimeoutOverride,
        }
        let w: Wrapper = toml::from_str(toml_str).unwrap();
        assert_eq!(
            w.call_timeout,
            CallTimeoutOverride::Value(Duration::from_secs(150))
        );
    }

    // ── SystemPromptOverride tests ──

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct SystemPromptWrapper {
        #[serde(default, skip_serializing_if = "SystemPromptOverride::is_inherit")]
        system_prompt: SystemPromptOverride,
    }

    #[test]
    fn system_prompt_override_default_is_inherit() {
        assert_eq!(
            SystemPromptOverride::default(),
            SystemPromptOverride::Inherit
        );
        assert!(SystemPromptOverride::default().is_inherit());
        assert!(!SystemPromptOverride::default().is_explicit());
        assert!(SystemPromptOverride::Set("p".to_string()).is_explicit());
        assert!(SystemPromptOverride::Disable.is_explicit());
        assert_eq!(
            SystemPromptOverride::Set("p".to_string()).as_set_prompt(),
            Some("p")
        );
        assert_eq!(SystemPromptOverride::Disable.as_set_prompt(), None);
    }

    #[test]
    fn system_prompt_override_absent_field_parses_as_inherit() {
        // Lossless with the retired `Option<String>` shape: an omitted field
        // (old `None`) is `Inherit`.
        let w: SystemPromptWrapper = serde_json::from_str("{}").unwrap();
        assert_eq!(w.system_prompt, SystemPromptOverride::Inherit);
    }

    #[test]
    fn system_prompt_override_null_parses_as_inherit() {
        let w: SystemPromptWrapper = serde_json::from_str(r#"{"system_prompt": null}"#).unwrap();
        assert_eq!(w.system_prompt, SystemPromptOverride::Inherit);
    }

    #[test]
    fn system_prompt_override_string_round_trips_as_set() {
        // Lossless with the retired `Some(prompt)` shape.
        let w: SystemPromptWrapper =
            serde_json::from_str(r#"{"system_prompt": "You are helpful."}"#).unwrap();
        assert_eq!(
            w.system_prompt,
            SystemPromptOverride::Set("You are helpful.".to_string())
        );
        let json = serde_json::to_string(&w).unwrap();
        assert_eq!(json, r#"{"system_prompt":"You are helpful."}"#);
        let back: SystemPromptWrapper = serde_json::from_str(&json).unwrap();
        assert_eq!(back, w);
    }

    #[test]
    fn system_prompt_override_disable_round_trips() {
        // The suppression fact survives serialize → deserialize: no lossy
        // Disable→Inherit collapse at any persist/wire boundary.
        let w = SystemPromptWrapper {
            system_prompt: SystemPromptOverride::Disable,
        };
        let json = serde_json::to_string(&w).unwrap();
        assert_eq!(json, r#"{"system_prompt":{"action":"disable"}}"#);
        let back: SystemPromptWrapper = serde_json::from_str(&json).unwrap();
        assert_eq!(back.system_prompt, SystemPromptOverride::Disable);
    }

    #[test]
    fn system_prompt_override_inherit_is_omitted_when_serialized() {
        let w = SystemPromptWrapper {
            system_prompt: SystemPromptOverride::Inherit,
        };
        assert_eq!(serde_json::to_string(&w).unwrap(), "{}");
    }

    #[test]
    fn system_prompt_override_unknown_action_rejected() {
        let err = serde_json::from_str::<SystemPromptWrapper>(
            r#"{"system_prompt": {"action": "clear"}}"#,
        )
        .expect_err("unknown action must be rejected");
        assert!(
            err.to_string()
                .contains("unknown system_prompt override action")
        );
    }

    #[test]
    fn system_prompt_override_unknown_key_rejected() {
        serde_json::from_str::<SystemPromptWrapper>(r#"{"system_prompt": {"disable": true}}"#)
            .expect_err("object form must carry exactly the action key");
    }

    #[test]
    fn system_prompt_override_non_string_rejected() {
        serde_json::from_str::<SystemPromptWrapper>(r#"{"system_prompt": 42}"#)
            .expect_err("numeric system_prompt must be rejected");
    }

    #[test]
    fn retry_config_default_has_inherit_call_timeout() {
        let config = RetryConfig::default();
        assert_eq!(config.call_timeout_override, CallTimeoutOverride::Inherit);
    }

    #[test]
    fn retry_config_from_toml_with_call_timeout_value() {
        let toml_str = r#"
[retry]
max_retries = 5
call_timeout = "60s"
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.retry.max_retries, 5);
        assert_eq!(
            config.retry.call_timeout_override,
            CallTimeoutOverride::Value(Duration::from_secs(60))
        );
    }

    #[test]
    fn retry_config_from_toml_with_call_timeout_disabled() {
        let toml_str = r#"
[retry]
call_timeout = "disabled"
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(
            config.retry.call_timeout_override,
            CallTimeoutOverride::Disabled
        );
    }

    #[test]
    fn retry_config_from_toml_omitted_is_inherit() {
        let toml_str = r"
[retry]
max_retries = 2
";
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(
            config.retry.call_timeout_override,
            CallTimeoutOverride::Inherit
        );
    }

    #[test]
    fn retry_policy_from_config_with_value_override() {
        let config = RetryConfig {
            call_timeout_override: CallTimeoutOverride::Value(Duration::from_secs(90)),
            ..RetryConfig::default()
        };
        let policy: crate::retry::RetryPolicy = config.into();
        assert_eq!(policy.call_timeout, Some(Duration::from_secs(90)));
    }

    #[test]
    fn retry_policy_from_config_with_disabled_override() {
        let config = RetryConfig {
            call_timeout_override: CallTimeoutOverride::Disabled,
            ..RetryConfig::default()
        };
        let policy: crate::retry::RetryPolicy = config.into();
        // Disabled collapses to None — the agent loop treats None as "no timeout"
        assert_eq!(policy.call_timeout, None);
    }

    #[test]
    fn retry_policy_from_config_with_inherit_override() {
        let config = RetryConfig {
            call_timeout_override: CallTimeoutOverride::Inherit,
            ..RetryConfig::default()
        };
        let policy: crate::retry::RetryPolicy = config.into();
        assert_eq!(policy.call_timeout, None);
    }

    #[test]
    fn config_merge_preserves_call_timeout_override() {
        let toml_base = r"
[retry]
max_retries = 2
";
        let toml_overlay = r#"
[retry]
call_timeout = "30s"
"#;
        let mut config: Config = toml::from_str(toml_base).unwrap();
        let overlay: Config = toml::from_str(toml_overlay).unwrap();
        let overlay_parsed: toml::Value = toml::from_str(toml_overlay).unwrap();
        config.merge_retry_from_toml_presence(&overlay_parsed, &overlay.retry);
        assert_eq!(config.retry.max_retries, 2); // Not overridden
        assert_eq!(
            config.retry.call_timeout_override,
            CallTimeoutOverride::Value(Duration::from_secs(30))
        );
    }

    // ---- Provider tools config tests ----

    #[test]
    fn test_provider_tools_defaults_all_enabled() {
        let config = Config::default();
        assert!(config.provider_tools.anthropic.web_search);
        assert!(config.provider_tools.openai.web_search);
        assert!(config.provider_tools.gemini.google_search);
    }

    #[test]
    fn test_provider_tools_roundtrip_toml() {
        let config = Config::default();
        let toml_str = toml::to_string(&config.provider_tools).unwrap();
        let parsed: ProviderToolsConfig = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed, config.provider_tools);
    }

    #[test]
    fn test_model_fallback_defaults_enabled_with_catalog_chain() {
        let config = Config::default();
        assert!(config.model_fallback.enabled);
        assert!(config.model_fallback.chain.is_empty());
    }

    #[test]
    fn test_model_fallback_chain_parses_typed_targets() {
        let config: Config = toml::from_str(
            r#"
[model_fallback]
enabled = true

[[model_fallback.chain]]
model = "backup-openai"
provider = "openai"

[[model_fallback.chain]]
model = "backup-anthropic"
provider = "anthropic"
"#,
        )
        .unwrap();

        assert!(config.model_fallback.enabled);
        assert_eq!(config.model_fallback.chain.len(), 2);
        assert_eq!(config.model_fallback.chain[0].model, "backup-openai");
        assert_eq!(
            config.model_fallback.chain[0].provider,
            Some(crate::Provider::OpenAI)
        );
        assert_eq!(
            config.model_fallback.chain[1].provider,
            Some(crate::Provider::Anthropic)
        );
    }

    #[test]
    fn test_model_fallback_catalog_default_reset_overrides_custom_layer() {
        let mut config: Config = toml::from_str(
            r#"
[model_fallback]
enabled = false

[[model_fallback.chain]]
model = "backup-openai"
provider = "openai"
"#,
        )
        .unwrap();
        let reset: Config = toml::from_str(
            r"
[model_fallback]
use_catalog_default_chain = true
",
        )
        .unwrap();

        config.merge(reset);

        assert_eq!(config.model_fallback, ModelFallbackConfig::default());
    }

    #[test]
    fn test_provider_tools_merge_preserves_when_absent() {
        let mut config = Config::default();
        config
            .merge_toml_str(
                r#"[agent]
model = "custom-model"
"#,
            )
            .unwrap();
        // provider_tools should be untouched
        assert!(config.provider_tools.anthropic.web_search);
        assert!(config.provider_tools.openai.web_search);
        assert!(config.provider_tools.gemini.google_search);
    }

    #[test]
    fn test_provider_tools_merge_overrides_single_provider() {
        let mut config = Config::default();
        config
            .merge_toml_str("[provider_tools.anthropic]\nweb_search = false\n")
            .unwrap();
        // Only anthropic should be disabled
        assert!(!config.provider_tools.anthropic.web_search);
        // Others unchanged
        assert!(config.provider_tools.openai.web_search);
        assert!(config.provider_tools.gemini.google_search);
    }

    // ---- AgentConfig.provider_params carrier serialization tests ----

    #[test]
    fn test_provider_tool_defaults_not_serialized() {
        use crate::lifecycle::run_primitive::{
            AnthropicProviderTag, OpaqueProviderBody, ProviderParamsCarrier, ProviderTag,
        };

        let agent_config = AgentConfig {
            provider_params: ProviderParamsCarrier {
                params: Default::default(),
                tool_defaults: Some(ProviderTag::Anthropic(AnthropicProviderTag {
                    web_search: Some(OpaqueProviderBody::from_value(&serde_json::json!({
                        "type": "web_search_20250305"
                    }))),
                    ..Default::default()
                })),
            },
            ..Default::default()
        };
        let json = serde_json::to_value(&agent_config).unwrap();
        // The carrier is transparent over `params`; build-derived tool
        // defaults never persist, and an empty params half serializes nothing.
        assert!(
            json.get("provider_params").is_none(),
            "build-derived tool defaults must not be serialized: {json}"
        );
    }

    /// K2 invariant: malformed provider params are rejected at CONFIG PARSE,
    /// not deferred to the first LLM call.
    #[test]
    fn test_malformed_provider_params_rejected_at_config_parse() {
        // Unknown key inside the typed override shape.
        let unknown_key = serde_json::json!({
            "model": "test-anthropic-default",
            "provider_params": { "not_a_known_knob": true }
        });
        assert!(
            serde_json::from_value::<AgentConfig>(unknown_key).is_err(),
            "unknown provider-params key must fail closed at config ingress"
        );

        // Wrong type for a known knob.
        let wrong_type = serde_json::json!({
            "model": "test-anthropic-default",
            "provider_params": { "temperature": "hot" }
        });
        assert!(
            serde_json::from_value::<AgentConfig>(wrong_type).is_err(),
            "mistyped provider-params knob must fail closed at config ingress"
        );

        // Well-formed typed shape parses.
        let typed = serde_json::json!({
            "model": "test-anthropic-default",
            "provider_params": {
                "temperature": 0.2,
                "provider_tag": { "provider": "anthropic", "effort": "high" }
            }
        });
        let parsed: AgentConfig =
            serde_json::from_value(typed).expect("typed provider params parse");
        assert_eq!(parsed.provider_params.params.temperature, Some(0.2));
    }
}
