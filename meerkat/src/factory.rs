//! AgentFactory - shared wiring for Meerkat interfaces.

#[cfg(not(feature = "memory-store"))]
use async_trait::async_trait;
#[cfg(not(feature = "memory-store"))]
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use meerkat_client::{
    DefaultClientFactory, DefaultFactoryConfig, FactoryError, LlmClient, LlmClientAdapter,
    LlmClientFactory, LlmProvider, ProviderResolver,
};
use meerkat_core::service::{CreateSessionRequest, SessionBuildOptions};
use meerkat_core::{
    Agent, AgentBuilder, AgentEvent, AgentLlmClient, AgentSessionStore, AgentToolDispatcher,
    BudgetLimits, Config, HookRunOverrides, OutputSchema, Provider, ScopedAgentEvent, Session,
    SessionMetadata, SessionTooling, StreamScopeFrame,
};
#[cfg(feature = "sub-agents")]
use meerkat_core::{ConcurrencyLimits, SubAgentManager};
#[cfg(not(feature = "memory-store"))]
use meerkat_core::{SessionId, SessionMeta};
#[cfg(feature = "jsonl-store")]
use meerkat_store::JsonlStore;
#[cfg(all(
    feature = "memory-store",
    any(not(feature = "jsonl-store"), feature = "sub-agents")
))]
use meerkat_store::MemoryStore;
#[cfg(not(feature = "memory-store"))]
use meerkat_store::SessionFilter;
use meerkat_store::{SessionStore, StoreAdapter};
#[cfg(not(target_arch = "wasm32"))]
use meerkat_tools::EmptyToolDispatcher;
#[cfg(not(target_arch = "wasm32"))]
use meerkat_tools::builtin::shell::ShellConfig;
#[cfg(all(feature = "sub-agents", not(target_arch = "wasm32")))]
use meerkat_tools::builtin::sub_agent::{SubAgentConfig, SubAgentToolSet, SubAgentToolState};
#[cfg(not(target_arch = "wasm32"))]
use meerkat_tools::builtin::{
    BuiltinToolConfig, CompositeDispatcher, MemoryTaskStore, TaskStore,
    ToolPolicyLayer,
};
#[cfg(not(target_arch = "wasm32"))]
use meerkat_tools::builtin::FileTaskStore;
#[cfg(not(target_arch = "wasm32"))]
use meerkat_tools::BuiltinDispatcherConfig;
use meerkat_tools::CompositeDispatcherError;
#[cfg(all(any(not(feature = "memory-store"), feature = "sub-agents"), not(target_arch = "wasm32")))]
use tokio::sync::RwLock;
#[cfg(all(any(not(feature = "memory-store"), feature = "sub-agents"), target_arch = "wasm32"))]
use tokio_with_wasm::alias::sync::RwLock;
#[cfg(not(target_arch = "wasm32"))]
use tokio::sync::mpsc;
#[cfg(target_arch = "wasm32")]
use tokio_with_wasm::alias::sync::mpsc;

#[cfg(feature = "comms")]
use crate::compose_tools_with_comms;
#[cfg(not(target_arch = "wasm32"))]
use crate::{create_default_hook_engine, resolve_layered_hooks_config};

/// Ephemeral in-process store used when no storage backend feature is enabled.
#[cfg(not(feature = "memory-store"))]
#[derive(Default)]
#[allow(dead_code)]
struct EphemeralSessionStore {
    sessions: RwLock<HashMap<SessionId, Session>>,
}

#[cfg(not(feature = "memory-store"))]
impl EphemeralSessionStore {
    fn new() -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
        }
    }
}

#[cfg(not(feature = "memory-store"))]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl SessionStore for EphemeralSessionStore {
    async fn save(&self, session: &Session) -> Result<(), meerkat_store::StoreError> {
        self.sessions
            .write()
            .await
            .insert(session.id().clone(), session.clone());
        Ok(())
    }

    async fn load(&self, id: &SessionId) -> Result<Option<Session>, meerkat_store::StoreError> {
        Ok(self.sessions.read().await.get(id).cloned())
    }

    async fn list(
        &self,
        filter: SessionFilter,
    ) -> Result<Vec<SessionMeta>, meerkat_store::StoreError> {
        let mut metas: Vec<SessionMeta> = self
            .sessions
            .read()
            .await
            .values()
            .map(SessionMeta::from)
            .collect();

        metas.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));

        if let Some(created_after) = filter.created_after {
            metas.retain(|m| m.created_at >= created_after);
        }
        if let Some(updated_after) = filter.updated_after {
            metas.retain(|m| m.updated_at >= updated_after);
        }
        if let Some(offset) = filter.offset {
            metas = metas.into_iter().skip(offset).collect();
        }
        if let Some(limit) = filter.limit {
            metas.truncate(limit);
        }

        Ok(metas)
    }

    async fn delete(&self, id: &SessionId) -> Result<(), meerkat_store::StoreError> {
        self.sessions.write().await.remove(id);
        Ok(())
    }
}

/// Type-erased agent using trait objects.
pub type DynAgent = Agent<dyn AgentLlmClient, dyn AgentToolDispatcher, dyn AgentSessionStore>;

#[derive(Clone)]
struct ErasedLlmClientOverride(Arc<dyn LlmClient>);

#[cfg(all(feature = "sub-agents", feature = "comms"))]
#[derive(Clone)]
struct SubAgentCommsWiring {
    parent_context: meerkat_comms::runtime::ParentCommsContext,
    parent_trusted_peers: Arc<RwLock<meerkat_comms::TrustedPeers>>,
}

/// Encode an LLM client override for transport in `SessionBuildOptions`.
///
/// `SessionBuildOptions` lives in `meerkat-core` and cannot depend directly on
/// `meerkat-client`, so the override is carried as `Arc<dyn Any + Send + Sync>`.
pub fn encode_llm_client_override_for_service(
    client: Arc<dyn LlmClient>,
) -> Arc<dyn std::any::Any + Send + Sync> {
    Arc::new(ErasedLlmClientOverride(client))
}

/// Decode an LLM client override from `SessionBuildOptions`.
///
/// Accepts the current typed wrapper and the legacy `Arc<dyn LlmClient>` payload
/// to preserve compatibility with older callers.
pub fn decode_llm_client_override_from_service(
    value: &Arc<dyn std::any::Any + Send + Sync>,
) -> Option<Arc<dyn LlmClient>> {
    if let Some(typed) = value.as_ref().downcast_ref::<ErasedLlmClientOverride>() {
        return Some(typed.0.clone());
    }
    value.as_ref().downcast_ref::<Arc<dyn LlmClient>>().cloned()
}

/// Full configuration for building an agent via [`AgentFactory::build_agent()`].
pub struct AgentBuildConfig {
    /// Model name (e.g. "claude-sonnet-4-5").
    pub model: String,
    /// Explicit provider. If `None`, inferred from the model name.
    pub provider: Option<Provider>,
    /// Max tokens per turn. If `None`, uses `Config::max_tokens`.
    pub max_tokens: Option<u32>,
    /// Override the system prompt. If `None`, uses the default composed prompt.
    pub system_prompt: Option<String>,
    /// Optional output schema for structured extraction.
    pub output_schema: Option<OutputSchema>,
    /// How many retries for structured output validation.
    pub structured_output_retries: u32,
    /// Run-scoped hook overrides.
    pub hooks_override: HookRunOverrides,
    /// Whether to enable comms host mode.
    pub host_mode: bool,
    /// Name for the comms participant (required when `host_mode` is `true`).
    pub comms_name: Option<String>,
    /// Friendly metadata for peer discovery (flows to `InprocRegistry` and `peers()` output).
    pub peer_meta: Option<meerkat_core::PeerMeta>,
    /// Resume from an existing session instead of starting fresh.
    pub resume_session: Option<Session>,
    /// Budget limits. If `None`, uses `Config::budget_limits()`.
    pub budget_limits: Option<BudgetLimits>,
    /// Optional event channel for streaming agent events.
    pub event_tx: Option<mpsc::Sender<AgentEvent>>,
    /// Optional scoped event channel for attributed multi-agent streaming.
    pub scoped_event_tx: Option<mpsc::Sender<ScopedAgentEvent>>,
    /// Base scope path used for attributed nested stream events.
    pub scoped_event_path: Option<Vec<StreamScopeFrame>>,
    /// Override LLM client (for testing or embedding).
    pub llm_client_override: Option<Arc<dyn LlmClient>>,
    /// Provider-specific parameters (e.g., thinking config, reasoning effort).
    pub provider_params: Option<serde_json::Value>,
    /// External tool dispatcher to compose with builtins (e.g., MCP callback tools).
    pub external_tools: Option<Arc<dyn AgentToolDispatcher>>,
    /// Per-build override for factory-level `enable_builtins`.
    /// When `Some`, takes precedence over `AgentFactory::enable_builtins`.
    pub override_builtins: Option<bool>,
    /// Per-build override for factory-level `enable_shell`.
    /// When `Some`, takes precedence over `AgentFactory::enable_shell`.
    pub override_shell: Option<bool>,
    /// Per-build override for factory-level `enable_subagents`.
    /// When `Some`, takes precedence over `AgentFactory::enable_subagents`.
    pub override_subagents: Option<bool>,
    /// Per-build override for factory-level `enable_memory`.
    /// When `Some`, takes precedence over `AgentFactory::enable_memory`.
    pub override_memory: Option<bool>,
    /// Per-build override for factory-level `enable_mob`.
    pub override_mob: Option<bool>,
    /// Skills to pre-load at build time (full body injected into system prompt).
    /// `None` = metadata-only inventory (agent discovers and loads via tools).
    /// `Some(ids)` = pre-load these skills into the system prompt.
    /// `Some(vec![])` is normalized to `None`.
    pub preload_skills: Option<Vec<meerkat_core::skills::SkillId>>,
    /// Realm identity for cross-surface storage sharing/isolation.
    pub realm_id: Option<String>,
    /// Optional process/agent instance identifier within a realm.
    pub instance_id: Option<String>,
    /// Backend pinned by the realm manifest (e.g. "redb", "jsonl").
    pub backend: Option<String>,
    /// Config generation used when this session was created/resumed.
    pub config_generation: Option<u64>,
    /// Optional session checkpointer for host-mode persistence.
    pub checkpointer: Option<Arc<dyn meerkat_core::checkpoint::SessionCheckpointer>>,
    /// Comms intents that should be silently injected into the session
    /// without triggering an LLM turn.
    pub silent_comms_intents: Vec<String>,
    /// Maximum peer-count threshold for inline peer lifecycle context injection.
    ///
    /// - `None`: use runtime default
    /// - `0`: never inline peer lifecycle notifications
    /// - `-1`: always inline peer lifecycle notifications
    /// - `>0`: inline only when post-drain peer count is <= threshold
    /// - `<-1`: invalid
    pub max_inline_peer_notifications: Option<i32>,

    // ── Resource overrides (platform-agnostic injection) ──
    //
    // When set, build_agent() uses the provided resource directly,
    // skipping filesystem-based resolution. This enables wasm32 and
    // embedded surfaces to inject pre-built resources programmatically.
    //
    // Precedence: override > factory field > config resolution > default

    /// Pre-built tool dispatcher. Skips shell/file/project resolution.
    pub tool_dispatcher_override: Option<Arc<dyn AgentToolDispatcher>>,

    /// Pre-built session store. Skips feature-flag store creation.
    pub session_store_override: Option<Arc<dyn AgentSessionStore>>,

    /// Pre-built hook engine. Skips filesystem hook config resolution.
    pub hook_engine_override: Option<Arc<dyn meerkat_core::HookEngine>>,

    /// Pre-built skill engine. Skips filesystem/git repository resolution.
    pub skill_engine_override: Option<Arc<meerkat_core::skills::SkillRuntime>>,
}

impl std::fmt::Debug for AgentBuildConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentBuildConfig")
            .field("model", &self.model)
            .field("provider", &self.provider)
            .field("max_tokens", &self.max_tokens)
            .field(
                "system_prompt",
                &self
                    .system_prompt
                    .as_deref()
                    .map(|s| if s.len() > 64 { &s[..64] } else { s }),
            )
            .field("output_schema", &self.output_schema.is_some())
            .field("structured_output_retries", &self.structured_output_retries)
            .field("host_mode", &self.host_mode)
            .field("comms_name", &self.comms_name)
            .field("peer_meta", &self.peer_meta)
            .field("resume_session", &self.resume_session.is_some())
            .field("budget_limits", &self.budget_limits)
            .field("event_tx", &self.event_tx.is_some())
            .field("scoped_event_tx", &self.scoped_event_tx.is_some())
            .field("scoped_event_path", &self.scoped_event_path.is_some())
            .field("llm_client_override", &self.llm_client_override.is_some())
            .field("provider_params", &self.provider_params.is_some())
            .field("external_tools", &self.external_tools.is_some())
            .field("override_builtins", &self.override_builtins)
            .field("override_shell", &self.override_shell)
            .field("override_subagents", &self.override_subagents)
            .field("override_memory", &self.override_memory)
            .field("override_mob", &self.override_mob)
            .field("realm_id", &self.realm_id)
            .field("instance_id", &self.instance_id)
            .field("backend", &self.backend)
            .field("config_generation", &self.config_generation)
            .field(
                "max_inline_peer_notifications",
                &self.max_inline_peer_notifications,
            )
            .field("tool_dispatcher_override", &self.tool_dispatcher_override.is_some())
            .field("session_store_override", &self.session_store_override.is_some())
            .field("hook_engine_override", &self.hook_engine_override.is_some())
            .field("skill_engine_override", &self.skill_engine_override.is_some())
            .finish()
    }
}

impl AgentBuildConfig {
    /// Create a new build config with sensible defaults for the given model.
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            provider: None,
            max_tokens: None,
            system_prompt: None,
            output_schema: None,
            structured_output_retries: 2,
            hooks_override: HookRunOverrides::default(),
            host_mode: false,
            comms_name: None,
            peer_meta: None,
            resume_session: None,
            budget_limits: None,
            event_tx: None,
            scoped_event_tx: None,
            scoped_event_path: None,
            llm_client_override: None,
            provider_params: None,
            external_tools: None,
            override_builtins: None,
            override_shell: None,
            override_subagents: None,
            override_memory: None,
            override_mob: None,
            preload_skills: None,
            realm_id: None,
            instance_id: None,
            backend: None,
            config_generation: None,
            checkpointer: None,
            silent_comms_intents: Vec::new(),
            max_inline_peer_notifications: None,
            tool_dispatcher_override: None,
            session_store_override: None,
            hook_engine_override: None,
            skill_engine_override: None,
        }
    }

    /// Build config from a service `CreateSessionRequest` + event channel.
    pub fn from_create_session_request(
        req: &CreateSessionRequest,
        event_tx: mpsc::Sender<AgentEvent>,
    ) -> Self {
        let mut build = Self::new(req.model.clone());
        build.system_prompt = req.system_prompt.clone();
        build.max_tokens = req.max_tokens;
        build.host_mode = req.host_mode;
        if let Some(options) = &req.build {
            build.apply_session_build_options(options);
        }
        build.event_tx = Some(event_tx);
        build
    }

    /// Merge `SessionBuildOptions` into this build config.
    pub fn apply_session_build_options(&mut self, build: &SessionBuildOptions) {
        self.provider = build.provider;
        self.output_schema = build.output_schema.clone();
        self.structured_output_retries = build.structured_output_retries;
        self.hooks_override = build.hooks_override.clone();
        self.comms_name = build.comms_name.clone();
        self.peer_meta = build.peer_meta.clone();
        self.resume_session = build.resume_session.clone();
        self.budget_limits = build.budget_limits.clone();
        self.provider_params = build.provider_params.clone();
        self.external_tools = build.external_tools.clone();
        self.llm_client_override = build
            .llm_client_override
            .as_ref()
            .and_then(decode_llm_client_override_from_service);
        self.scoped_event_tx = build.scoped_event_tx.clone();
        self.scoped_event_path = build.scoped_event_path.clone();
        self.override_builtins = build.override_builtins;
        self.override_shell = build.override_shell;
        self.override_subagents = build.override_subagents;
        self.override_memory = build.override_memory;
        self.override_mob = build.override_mob;
        self.preload_skills = build.preload_skills.clone();
        self.realm_id = build.realm_id.clone();
        self.instance_id = build.instance_id.clone();
        self.backend = build.backend.clone();
        self.config_generation = build.config_generation;
        self.checkpointer = build.checkpointer.clone();
        self.silent_comms_intents
            .clone_from(&build.silent_comms_intents);
        self.max_inline_peer_notifications = build.max_inline_peer_notifications;
    }

    /// Convert build options to the service transport representation.
    pub fn to_session_build_options(&self) -> SessionBuildOptions {
        SessionBuildOptions {
            provider: self.provider,
            output_schema: self.output_schema.clone(),
            structured_output_retries: self.structured_output_retries,
            hooks_override: self.hooks_override.clone(),
            comms_name: self.comms_name.clone(),
            peer_meta: self.peer_meta.clone(),
            resume_session: self.resume_session.clone(),
            budget_limits: self.budget_limits.clone(),
            provider_params: self.provider_params.clone(),
            external_tools: self.external_tools.clone(),
            llm_client_override: self
                .llm_client_override
                .clone()
                .map(encode_llm_client_override_for_service),
            scoped_event_tx: self.scoped_event_tx.clone(),
            scoped_event_path: self.scoped_event_path.clone(),
            override_builtins: self.override_builtins,
            override_shell: self.override_shell,
            override_subagents: self.override_subagents,
            override_memory: self.override_memory,
            override_mob: self.override_mob,
            preload_skills: self.preload_skills.clone(),
            realm_id: self.realm_id.clone(),
            instance_id: self.instance_id.clone(),
            backend: self.backend.clone(),
            config_generation: self.config_generation,
            checkpointer: self.checkpointer.clone(),
            silent_comms_intents: self.silent_comms_intents.clone(),
            max_inline_peer_notifications: self.max_inline_peer_notifications,
        }
    }
}

/// Errors that can occur when building an agent via [`AgentFactory::build_agent()`].
#[derive(Debug, thiserror::Error)]
pub enum BuildAgentError {
    /// Cannot infer provider from the given model name.
    #[error("Cannot infer provider from model '{model}'")]
    UnknownProvider { model: String },

    /// API key is not set for the resolved provider.
    #[error("API key not set for provider '{provider}'")]
    MissingApiKey { provider: String },

    /// LLM client creation failed.
    #[error("LLM client creation failed: {0}")]
    LlmClient(#[from] FactoryError),

    /// Tool dispatcher creation failed.
    #[error("Tool dispatcher creation failed: {0}")]
    ToolDispatcher(#[from] CompositeDispatcherError),

    /// Comms runtime failed to initialize.
    #[error("Comms runtime failed: {0}")]
    #[cfg(feature = "comms")]
    Comms(String),

    /// Configuration error.
    #[error("Config error: {0}")]
    Config(String),

    /// `host_mode` was set but `comms_name` is missing.
    #[error("host_mode requires comms_name to be set")]
    #[cfg(feature = "comms")]
    HostModeRequiresCommsName,
}

/// Return the canonical string key for a provider.
pub fn provider_key(provider: Provider) -> &'static str {
    provider.as_str()
}

/// Factory for creating agents with standard configuration.
#[derive(Clone)]
pub struct AgentFactory {
    pub store_path: PathBuf,
    /// Runtime root for realm-scoped artifacts (comms identity/trust, hook layers,
    /// skill caches). When unset, falls back to project_root or store_path.
    pub runtime_root: Option<PathBuf>,
    pub project_root: Option<PathBuf>,
    /// Explicit root for project/workspace conventions (skills, hooks, AGENTS, MCP config).
    /// When unset, convention loading remains disabled unless caller opts in.
    pub context_root: Option<PathBuf>,
    /// Optional user-global convention root (typically HOME).
    pub user_config_root: Option<PathBuf>,
    pub enable_builtins: bool,
    pub enable_shell: bool,
    pub enable_subagents: bool,
    #[cfg(feature = "comms")]
    pub enable_comms: bool,
    pub enable_memory: bool,
    pub enable_mob: bool,
    /// Optional skill source override. When set, bypasses config-driven
    /// repository resolution. For SDK users who wire sources programmatically.
    #[cfg(feature = "skills")]
    pub skill_source: Option<Arc<meerkat_skills::CompositeSkillSource>>,
    /// Optional custom session store. When set, `build_agent()` uses this
    /// instead of the feature-flag-based default (jsonl, memory, or ephemeral).
    custom_store: Option<Arc<dyn SessionStore>>,
}

impl std::fmt::Debug for AgentFactory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut d = f.debug_struct("AgentFactory");
        d.field("store_path", &self.store_path)
            .field("runtime_root", &self.runtime_root)
            .field("project_root", &self.project_root)
            .field("context_root", &self.context_root)
            .field("user_config_root", &self.user_config_root)
            .field("enable_builtins", &self.enable_builtins)
            .field("enable_shell", &self.enable_shell)
            .field("enable_subagents", &self.enable_subagents)
            .field("enable_memory", &self.enable_memory)
            .field("enable_mob", &self.enable_mob);
        #[cfg(feature = "comms")]
        d.field("enable_comms", &self.enable_comms);
        #[cfg(feature = "skills")]
        d.field("skill_source", &self.skill_source.as_ref().map(|_| ".."));
        d.field("custom_store", &self.custom_store.as_ref().map(|_| ".."));
        d.finish()
    }
}

impl AgentFactory {
    /// Create a new factory with the required session store path.
    pub fn new(store_path: impl Into<PathBuf>) -> Self {
        Self {
            store_path: store_path.into(),
            runtime_root: None,
            project_root: None,
            context_root: None,
            user_config_root: None,
            enable_builtins: false,
            enable_shell: false,
            enable_subagents: false,
            #[cfg(feature = "comms")]
            enable_comms: false,
            enable_memory: false,
            enable_mob: false,
            #[cfg(feature = "skills")]
            skill_source: None,
            custom_store: None,
        }
    }

    /// Set a custom skill source (bypasses config-driven repository resolution).
    #[cfg(feature = "skills")]
    pub fn skill_source(mut self, source: Arc<meerkat_skills::CompositeSkillSource>) -> Self {
        self.skill_source = Some(source);
        self
    }

    /// Set the project root used for tool persistence.
    pub fn project_root(mut self, path: impl Into<PathBuf>) -> Self {
        self.project_root = Some(path.into());
        self
    }

    /// Set convention context root used for project-level conventions
    /// (skills/hooks/AGENTS/MCP definitions).
    pub fn context_root(mut self, path: impl Into<PathBuf>) -> Self {
        self.context_root = Some(path.into());
        self
    }

    /// Set optional user-global convention root.
    pub fn user_config_root(mut self, path: impl Into<PathBuf>) -> Self {
        self.user_config_root = Some(path.into());
        self
    }

    /// Set runtime root used for realm-scoped runtime artifacts.
    pub fn runtime_root(mut self, path: impl Into<PathBuf>) -> Self {
        self.runtime_root = Some(path.into());
        self
    }

    /// Enable or disable builtin tools.
    pub fn builtins(mut self, enabled: bool) -> Self {
        self.enable_builtins = enabled;
        self
    }

    /// Enable or disable shell tools.
    pub fn shell(mut self, enabled: bool) -> Self {
        self.enable_shell = enabled;
        self
    }

    /// Enable or disable sub-agent tools.
    pub fn subagents(mut self, enabled: bool) -> Self {
        self.enable_subagents = enabled;
        self
    }

    /// Enable or disable semantic memory (memory_search tool + compaction indexing).
    pub fn memory(mut self, enabled: bool) -> Self {
        self.enable_memory = enabled;
        self
    }

    /// Enable or disable mob (multi-agent orchestration) tools.
    pub fn mob(mut self, enabled: bool) -> Self {
        self.enable_mob = enabled;
        self
    }

    /// Enable or disable comms tools.
    #[cfg(feature = "comms")]
    pub fn comms(mut self, enabled: bool) -> Self {
        self.enable_comms = enabled;
        self
    }

    /// Build a `SkillRuntime` from the factory's skill configuration.
    ///
    /// Returns `None` if skills are disabled or no source is available.
    #[cfg(feature = "skills")]
    pub async fn build_skill_runtime(
        &self,
        config: &Config,
    ) -> Option<Arc<meerkat_core::skills::SkillRuntime>> {
        let skill_source: Option<Arc<meerkat_skills::CompositeSkillSource>> =
            if self.skill_source.is_some() {
                self.skill_source.clone()
            } else if !config.skills.enabled {
                None
            } else {
                let conventions_context_root = self
                    .context_root
                    .as_deref()
                    .or(self.project_root.as_deref());
                let conventions_user_root = self.user_config_root.as_deref();
                let runtime_root = self
                    .runtime_root
                    .clone()
                    .or_else(|| self.project_root.clone())
                    .unwrap_or_else(|| self.store_path.clone());
                match meerkat_skills::resolve_repositories_with_roots(
                    &config.skills,
                    conventions_context_root,
                    conventions_user_root,
                    Some(runtime_root.as_path()),
                )
                .await
                {
                    Ok(source) => source.map(Arc::new),
                    Err(e) => {
                        tracing::warn!("Failed to resolve skill repositories: {e}");
                        None
                    }
                }
            };

        skill_source.map(|source| {
            let available_caps: Vec<String> = meerkat_contracts::build_capabilities()
                .into_iter()
                .map(|c| c.id.to_string())
                .collect();
            let engine = Arc::new(
                meerkat_skills::DefaultSkillEngine::new(source, available_caps)
                    .with_inventory_threshold(config.skills.inventory_threshold)
                    .with_max_injection_bytes(config.skills.max_injection_bytes),
            );
            Arc::new(meerkat_core::skills::SkillRuntime::new(engine))
        })
    }

    /// Override the default session store.
    ///
    /// When set, `build_agent()` uses this store instead of the feature-flag-based
    /// default (jsonl, memory, or ephemeral). The store is wrapped in `StoreAdapter`
    /// and passed to `AgentBuilder::build()`.
    pub fn session_store(mut self, store: Arc<dyn SessionStore>) -> Self {
        self.custom_store = Some(store);
        self
    }

    fn realm_scope_root(&self, _build_config: &AgentBuildConfig) -> PathBuf {
        self.runtime_root
            .clone()
            .or_else(|| self.project_root.clone())
            .unwrap_or_else(|| self.store_path.clone())
    }

    /// Build an LLM adapter for the provided client/model.
    pub async fn build_llm_adapter(
        &self,
        client: Arc<dyn LlmClient>,
        model: impl Into<String>,
    ) -> LlmClientAdapter {
        LlmClientAdapter::new(client, model.into())
    }

    /// Build an LLM adapter, optionally wiring an event channel for streaming.
    pub async fn build_llm_adapter_with_events(
        &self,
        client: Arc<dyn LlmClient>,
        model: impl Into<String>,
        event_tx: Option<mpsc::Sender<AgentEvent>>,
    ) -> LlmClientAdapter {
        match event_tx {
            Some(tx) => LlmClientAdapter::with_event_channel(client, model.into(), tx),
            None => LlmClientAdapter::new(client, model.into()),
        }
    }

    /// Build an LLM client for a provider with optional base URL override.
    pub async fn build_llm_client(
        &self,
        provider: Provider,
        api_key: Option<String>,
        base_url: Option<String>,
    ) -> Result<Arc<dyn LlmClient>, FactoryError> {
        let mapped = match provider {
            Provider::Anthropic => LlmProvider::Anthropic,
            Provider::OpenAI => LlmProvider::OpenAi,
            Provider::Gemini => LlmProvider::Gemini,
            Provider::Other => return Err(FactoryError::UnsupportedProvider("other".to_string())),
        };

        let mut config = DefaultFactoryConfig::default();
        if let Some(url) = base_url {
            match mapped {
                LlmProvider::Anthropic => config = config.with_anthropic_base_url(url),
                LlmProvider::OpenAi => config = config.with_openai_base_url(url),
                LlmProvider::Gemini => config = config.with_gemini_base_url(url),
            }
        }

        let factory = DefaultClientFactory::with_config(config);
        factory.create_client(mapped, api_key)
    }

    fn resolve_provider_credentials(
        &self,
        provider: Provider,
        config: &Config,
    ) -> (Option<String>, Option<String>) {
        // Preferred shared settings map (works across all providers).
        let mut base_url = config
            .providers
            .base_urls
            .as_ref()
            .and_then(|map| map.get(provider_key(provider)).cloned());
        let mut api_key = config
            .providers
            .api_keys
            .as_ref()
            .and_then(|map| map.get(provider_key(provider)).cloned());

        // Backward-compatible provider-specific block still supported; if the
        // selected provider matches this variant, explicit values override map values.
        match (&config.provider, provider) {
            (
                meerkat_core::ProviderConfig::Anthropic {
                    api_key: cfg_key,
                    base_url: cfg_url,
                },
                Provider::Anthropic,
            ) => {
                if cfg_key.is_some() {
                    api_key = cfg_key.clone();
                }
                if cfg_url.is_some() {
                    base_url = cfg_url.clone();
                }
            }
            (
                meerkat_core::ProviderConfig::OpenAI {
                    api_key: cfg_key,
                    base_url: cfg_url,
                },
                Provider::OpenAI,
            ) => {
                if cfg_key.is_some() {
                    api_key = cfg_key.clone();
                }
                if cfg_url.is_some() {
                    base_url = cfg_url.clone();
                }
            }
            (meerkat_core::ProviderConfig::Gemini { api_key: cfg_key }, Provider::Gemini) => {
                if cfg_key.is_some() {
                    api_key = cfg_key.clone();
                }
            }
            _ => {}
        }

        // Env fallback remains last for secrets when config omits keys.
        if api_key.is_none() {
            api_key = ProviderResolver::api_key_for(provider);
        }

        (api_key, base_url)
    }

    /// Wrap a session store in the shared adapter.
    pub async fn build_store_adapter<S: SessionStore + 'static>(
        &self,
        store: Arc<S>,
    ) -> StoreAdapter<S> {
        StoreAdapter::new(store)
    }

    /// Build a composite dispatcher so callers can register sub-agent tools.
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn build_composite_dispatcher(
        &self,
        store: Arc<dyn TaskStore>,
        config: &BuiltinToolConfig,
        shell_config: Option<ShellConfig>,
        external: Option<Arc<dyn AgentToolDispatcher>>,
        session_id: Option<String>,
    ) -> Result<CompositeDispatcher, CompositeDispatcherError> {
        CompositeDispatcher::new(store, config, shell_config, external, session_id)
    }

    /// Build a shared builtin dispatcher using the provided config.
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn build_builtin_dispatcher(
        &self,
        store: Arc<dyn TaskStore>,
        config: BuiltinToolConfig,
        shell_config: Option<ShellConfig>,
        external: Option<Arc<dyn AgentToolDispatcher>>,
        session_id: Option<String>,
    ) -> Result<Arc<dyn AgentToolDispatcher>, CompositeDispatcherError> {
        self.build_builtin_dispatcher_with_skills(
            store,
            config,
            shell_config,
            external,
            session_id,
            None,
        )
        .await
    }

    /// Build a shared builtin dispatcher, optionally including skill tools.
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn build_builtin_dispatcher_with_skills(
        &self,
        store: Arc<dyn TaskStore>,
        config: BuiltinToolConfig,
        shell_config: Option<ShellConfig>,
        external: Option<Arc<dyn AgentToolDispatcher>>,
        session_id: Option<String>,
        #[cfg_attr(not(feature = "skills"), allow(unused_variables))] skill_engine: Option<
            Arc<meerkat_core::skills::SkillRuntime>,
        >,
    ) -> Result<Arc<dyn AgentToolDispatcher>, CompositeDispatcherError> {
        self.build_builtin_dispatcher_with_skills_internal(
            store,
            config,
            shell_config,
            external,
            session_id,
            skill_engine,
            None,
            None,
            #[cfg(all(feature = "sub-agents", feature = "comms"))]
            None,
        )
        .await
    }

    /// Internal dispatcher builder used by `build_agent` when extra sub-agent
    /// comms wiring is available from a live parent runtime.
    #[cfg(not(target_arch = "wasm32"))]
    #[allow(clippy::too_many_arguments)]
    async fn build_builtin_dispatcher_with_skills_internal(
        &self,
        store: Arc<dyn TaskStore>,
        config: BuiltinToolConfig,
        shell_config: Option<ShellConfig>,
        external: Option<Arc<dyn AgentToolDispatcher>>,
        session_id: Option<String>,
        #[cfg_attr(not(feature = "skills"), allow(unused_variables))] skill_engine: Option<
            Arc<meerkat_core::skills::SkillRuntime>,
        >,
        #[cfg_attr(not(feature = "sub-agents"), allow(unused_variables))]
        sub_agent_scoped_event_tx: Option<mpsc::Sender<ScopedAgentEvent>>,
        #[cfg_attr(not(feature = "sub-agents"), allow(unused_variables))]
        sub_agent_scope_path: Option<Vec<StreamScopeFrame>>,
        #[cfg(all(feature = "sub-agents", feature = "comms"))] sub_agent_comms: Option<
            SubAgentCommsWiring,
        >,
    ) -> Result<Arc<dyn AgentToolDispatcher>, CompositeDispatcherError> {
        let builder = BuiltinDispatcherConfig {
            store,
            config,
            shell_config,
            external,
            session_id,
        };
        #[cfg(not(feature = "sub-agents"))]
        {
            #[allow(unused_mut)]
            let mut composite = CompositeDispatcher::new(
                builder.store,
                &builder.config,
                builder.shell_config,
                builder.external,
                builder.session_id,
            )?;
            #[cfg(feature = "skills")]
            if let Some(engine) = skill_engine {
                composite.register_skill_tools(meerkat_tools::builtin::skills::SkillToolSet::new(
                    engine,
                ));
            }
            Ok(Arc::new(composite))
        }

        #[cfg(feature = "sub-agents")]
        if !self.enable_subagents {
            let mut composite = CompositeDispatcher::new(
                builder.store,
                &builder.config,
                builder.shell_config,
                builder.external,
                builder.session_id,
            )?;
            #[cfg(feature = "skills")]
            if let Some(engine) = skill_engine {
                composite.register_skill_tools(meerkat_tools::builtin::skills::SkillToolSet::new(
                    engine,
                ));
            }
            return Ok(Arc::new(composite));
        }

        #[cfg(feature = "sub-agents")]
        {
            let BuiltinDispatcherConfig {
                store,
                config,
                shell_config,
                external,
                session_id,
            } = builder;

            let shell_config_for_subagents = shell_config.clone();
            let mut composite = self
                .build_composite_dispatcher(
                    store,
                    &config,
                    shell_config,
                    external.clone(),
                    session_id,
                )
                .await?;

            let limits = ConcurrencyLimits::default();
            let manager = Arc::new(SubAgentManager::new(limits, 0));
            let client_factory: Arc<dyn LlmClientFactory> = Arc::new(DefaultClientFactory::new());

            let sub_agent_task_store = MemoryTaskStore::new();
            let sub_agent_factory = {
                let factory = self.clone().subagents(false);
                #[cfg(feature = "comms")]
                let factory = factory.comms(false);
                factory
            };
            let sub_agent_dispatcher = sub_agent_factory
                .build_composite_dispatcher(
                    Arc::new(sub_agent_task_store),
                    &config,
                    shell_config_for_subagents,
                    external,
                    None,
                )
                .await?;
            let sub_agent_tools: Arc<dyn AgentToolDispatcher> = Arc::new(sub_agent_dispatcher);

            #[cfg(feature = "memory-store")]
            let sub_agent_store: Arc<dyn meerkat_core::AgentSessionStore> = Arc::new(
                sub_agent_factory
                    .build_store_adapter(Arc::new(MemoryStore::new()))
                    .await,
            );
            #[cfg(not(feature = "memory-store"))]
            let sub_agent_store: Arc<dyn meerkat_core::AgentSessionStore> = Arc::new(
                sub_agent_factory
                    .build_store_adapter(Arc::new(EphemeralSessionStore::new()))
                    .await,
            );

            let parent_session = Arc::new(RwLock::new(Session::new()));
            let mut sub_agent_config = SubAgentConfig::default();
            #[cfg(all(feature = "sub-agents", feature = "comms"))]
            if let Some(ref comms) = sub_agent_comms {
                sub_agent_config = sub_agent_config
                    .with_enable_comms(true)
                    .with_comms_base_dir(comms.parent_context.comms_base_dir.clone());
            }

            #[cfg(all(feature = "sub-agents", feature = "comms"))]
            let state = Arc::new(match sub_agent_comms {
                Some(comms) => SubAgentToolState::with_comms(
                    manager,
                    client_factory,
                    sub_agent_tools,
                    sub_agent_store,
                    parent_session,
                    sub_agent_config,
                    0,
                    comms.parent_context,
                    comms.parent_trusted_peers,
                ),
                None => SubAgentToolState::new(
                    manager,
                    client_factory,
                    sub_agent_tools,
                    sub_agent_store,
                    parent_session,
                    sub_agent_config,
                    0,
                ),
            });

            #[cfg(not(all(feature = "sub-agents", feature = "comms")))]
            let state = Arc::new(SubAgentToolState::new(
                manager,
                client_factory,
                sub_agent_tools,
                sub_agent_store,
                parent_session,
                sub_agent_config,
                0,
            ));
            state
                .set_scoped_stream(
                    sub_agent_scoped_event_tx.clone(),
                    sub_agent_scope_path.clone().unwrap_or_default(),
                )
                .await;

            let tool_set = SubAgentToolSet::new(state);
            composite.register_sub_agent_tools(tool_set, &config)?;

            #[cfg(feature = "skills")]
            if let Some(engine) = skill_engine {
                composite.register_skill_tools(meerkat_tools::builtin::skills::SkillToolSet::new(
                    engine,
                ));
            }

            Ok(Arc::new(composite))
        }
    }

    /// Build a fully-configured, type-erased agent ready to run.
    ///
    /// This method consolidates the agent construction pipeline that was previously
    /// repeated across all surfaces (CLI, REST, MCP server):
    ///   load config, resolve provider/model, check API key, create LLM client +
    ///   adapter, build tool dispatcher, create comms runtime, compose tools with
    ///   comms, resolve hooks, build system prompt, wire AgentBuilder, and set
    ///   SessionMetadata.
    pub async fn build_agent(
        &self,
        mut build_config: AgentBuildConfig,
        config: &Config,
    ) -> Result<DynAgent, BuildAgentError> {
        if let Some(value) = build_config.max_inline_peer_notifications
            && value < -1
        {
            return Err(BuildAgentError::Config(format!(
                "max_inline_peer_notifications={} is invalid (allowed: -1, 0, or >0)",
                value
            )));
        }

        // 1. Validate host_mode
        #[cfg(feature = "comms")]
        if build_config.host_mode && build_config.comms_name.is_none() {
            return Err(BuildAgentError::HostModeRequiresCommsName);
        }

        // 2. Resolve provider
        let provider = match build_config.provider {
            Some(p) => p,
            None => {
                let inferred = ProviderResolver::infer_from_model(&build_config.model);
                if inferred == Provider::Other {
                    return Err(BuildAgentError::UnknownProvider {
                        model: build_config.model.clone(),
                    });
                }
                inferred
            }
        };

        // 3. Create LLM client
        let llm_client: Arc<dyn LlmClient> = match build_config.llm_client_override.as_ref() {
            Some(client) => Arc::clone(client),
            None => {
                let (api_key, base_url) = self.resolve_provider_credentials(provider, config);
                if api_key.is_none() {
                    return Err(BuildAgentError::MissingApiKey {
                        provider: provider_key(provider).to_string(),
                    });
                }
                self.build_llm_client(provider, api_key, base_url)
                    .await
                    .map_err(BuildAgentError::LlmClient)?
            }
        };

        // 4. Create LLM adapter (with optional provider_params, event channel, and shared event tap)
        let model = build_config.model.clone();
        let event_tap = meerkat_core::new_event_tap();
        let mut llm_adapter_inner = match build_config.event_tx.clone() {
            Some(tx) => LlmClientAdapter::with_event_channel(llm_client, model.clone(), tx),
            None => LlmClientAdapter::new(llm_client, model.clone()),
        };
        llm_adapter_inner = llm_adapter_inner.with_event_tap(event_tap.clone());
        if let Some(params) = build_config.provider_params.clone() {
            llm_adapter_inner = llm_adapter_inner.with_provider_params(Some(params));
        }
        let llm_adapter: Arc<dyn AgentLlmClient> = Arc::new(llm_adapter_inner);

        // 5. Resolve max_tokens
        let max_tokens = build_config.max_tokens.unwrap_or(config.max_tokens);
        let _realm_scope_root = self.realm_scope_root(&build_config);
        let conventions_context_root = self
            .context_root
            .as_deref()
            .or(self.project_root.as_deref());
        let conventions_user_root = self.user_config_root.as_deref();

        // 6a. Build skill engine (override > factory > config > filesystem).
        #[cfg(feature = "skills")]
        let skill_engine: Option<Arc<meerkat_core::skills::SkillRuntime>> =
            if let Some(engine) = build_config.skill_engine_override.take() {
                Some(engine)
            } else {
            let skill_source: Option<Arc<meerkat_skills::CompositeSkillSource>> =
                if self.skill_source.is_some() {
                    self.skill_source.clone()
                } else if !config.skills.enabled {
                    None
                } else {
                    match meerkat_skills::resolve_repositories_with_roots(
                        &config.skills,
                        conventions_context_root,
                        conventions_user_root,
                        Some(_realm_scope_root.as_path()),
                    )
                    .await
                    {
                        Ok(source) => source.map(Arc::new),
                        Err(e) => {
                            tracing::warn!("Failed to resolve skill repositories: {e}");
                            None
                        }
                    }
                };

            skill_source.map(|source| {
                let available_caps: Vec<String> = meerkat_contracts::build_capabilities()
                    .into_iter()
                    .map(|c| c.id.to_string())
                    .collect();
                let engine = Arc::new(
                    meerkat_skills::DefaultSkillEngine::new(source, available_caps)
                        .with_inventory_threshold(config.skills.inventory_threshold)
                        .with_max_injection_bytes(config.skills.max_injection_bytes),
                );
                Arc::new(meerkat_core::skills::SkillRuntime::new(engine))
            })
        };  // end else (filesystem resolution fallthrough)
        #[cfg(not(feature = "skills"))]
        let skill_engine: Option<Arc<meerkat_core::skills::SkillRuntime>> = None;

        // 6b. Build tool dispatcher (with optional external tools, per-build overrides, skill tools)
        let per_request_prompt = build_config.system_prompt.take();
        let effective_builtins = build_config
            .override_builtins
            .unwrap_or(self.enable_builtins);
        let effective_shell = build_config.override_shell.unwrap_or(self.enable_shell);
        let effective_subagents = build_config
            .override_subagents
            .unwrap_or(self.enable_subagents);
        // 6b. Create comms runtime (before tool wiring so sub-agent tools can
        // inherit parent comms context when auto-enabled).
        #[cfg(feature = "comms")]
        let comms_runtime = if build_config.host_mode || build_config.comms_name.is_some() {
            let comms_name = build_config
                .comms_name
                .as_ref()
                .ok_or(BuildAgentError::HostModeRequiresCommsName)?;
            let runtime = crate::build_comms_runtime_from_config_scoped(
                config,
                _realm_scope_root.as_path(),
                comms_name,
                build_config.peer_meta.clone(),
                // Realm ID is the comms inproc namespace boundary.
                build_config.realm_id.clone(),
            )
            .await
            .map_err(BuildAgentError::Comms)?;
            Some(runtime)
        } else {
            None
        };
        #[cfg(not(feature = "comms"))]
        let _comms_runtime: Option<()> = None;

        #[cfg(all(feature = "sub-agents", feature = "comms"))]
        let sub_agent_comms = if config.comms.auto_enable_for_subagents && effective_subagents {
            comms_runtime.as_ref().map(|runtime| SubAgentCommsWiring {
                parent_context: meerkat_comms::runtime::ParentCommsContext {
                    parent_name: runtime.participant_name().to_string(),
                    parent_pubkey: *runtime.public_key().as_bytes(),
                    parent_addr: runtime.advertised_address(),
                    comms_base_dir: _realm_scope_root
                        .join(".rkat")
                        .join("subagents")
                        .join("comms"),
                    inproc_namespace: runtime.inproc_namespace().map(ToOwned::to_owned),
                },
                parent_trusted_peers: runtime.trusted_peers_shared(),
            })
        } else {
            None
        };

        #[allow(unused_mut)]
        let (mut tools, mut tool_usage_instructions) =
            if let Some(dispatcher) = build_config.tool_dispatcher_override.take() {
                let usage = render_tool_usage_instructions(dispatcher.tools().as_ref());
                (dispatcher, usage)
            } else {
                #[cfg(not(target_arch = "wasm32"))]
                {
                    self.build_tool_dispatcher_for_agent_with_overrides(
                        config,
                        build_config.external_tools,
                        effective_builtins,
                        effective_shell,
                        effective_subagents,
                        skill_engine.clone(),
                        build_config.scoped_event_tx.clone(),
                        build_config.scoped_event_path.clone(),
                        #[cfg(all(feature = "sub-agents", feature = "comms"))]
                        sub_agent_comms,
                    )
                    .await?
                }
                #[cfg(target_arch = "wasm32")]
                {
                    return Err(BuildAgentError::Config(
                        "wasm32 requires tool_dispatcher_override in AgentBuildConfig".to_string(),
                    ));
                }
            };

        // 7. Create session store adapter (override > factory > feature-flag default)
        let store_adapter: Arc<dyn AgentSessionStore> = if let Some(store) = build_config.session_store_override.take() {
            store
        } else if let Some(store) = &self.custom_store {
            Arc::new(StoreAdapter::new(Arc::clone(store)))
        } else {
            #[cfg(feature = "jsonl-store")]
            {
                let store = JsonlStore::new(self.store_path.clone());
                store
                    .init()
                    .await
                    .map_err(|e| BuildAgentError::Config(format!("Store init failed: {e}")))?;
                Arc::new(StoreAdapter::new(Arc::new(store)))
            }
            #[cfg(all(not(feature = "jsonl-store"), feature = "memory-store"))]
            {
                Arc::new(self.build_store_adapter(Arc::new(MemoryStore::new())).await)
            }
            #[cfg(all(not(feature = "jsonl-store"), not(feature = "memory-store")))]
            {
                Arc::new(
                    self.build_store_adapter(Arc::new(EphemeralSessionStore::new()))
                        .await,
                )
            }
        };

        // 9. Compose tools with comms
        #[cfg(feature = "comms")]
        if let Some(ref runtime) = comms_runtime {
            let composed = compose_tools_with_comms(tools, tool_usage_instructions, runtime)
                .map_err(|e| {
                    BuildAgentError::Config(format!("Failed to compose comms tools: {e}"))
                })?;
            tools = composed.0;
            tool_usage_instructions = composed.1;
        }

        // 10. Resolve hooks (override > filesystem layered config)
        let hook_engine = if let Some(engine) = build_config.hook_engine_override.take() {
            Some(engine)
        } else {
            #[cfg(not(target_arch = "wasm32"))]
            {
                let layered_hooks = resolve_layered_hooks_config(
                    conventions_context_root,
                    conventions_user_root,
                    config,
                )
                .await;
                create_default_hook_engine(layered_hooks)
            }
            #[cfg(target_arch = "wasm32")]
            { None }
        };

        // 11. Generate skill inventory section using the engine created in step 6a
        #[cfg(feature = "skills")]
        let skill_inventory_section = {
            if let Some(ref engine) = skill_engine {
                // Generate inventory section for system prompt
                let inventory = match engine.inventory_section().await {
                    Ok(s) => s,
                    Err(e) => {
                        tracing::warn!("Failed to generate skill inventory section: {e}");
                        String::new()
                    }
                };

                // Normalize preload_skills: Some([]) → None
                let preload = build_config
                    .preload_skills
                    .take()
                    .and_then(|ids| if ids.is_empty() { None } else { Some(ids) });

                // Pre-load requested skills into system prompt (Level 2)
                let mut preloaded_sections = Vec::new();
                if let Some(ref ids) = preload {
                    match engine.resolve_and_render(ids).await {
                        Ok(resolved) => {
                            for skill in &resolved {
                                preloaded_sections.push(skill.rendered_body.clone());
                            }
                        }
                        Err(e) => {
                            return Err(BuildAgentError::Config(format!(
                                "Failed to preload skill: {e}"
                            )));
                        }
                    }
                }

                // Collect active skill IDs
                let skill_ids: Vec<meerkat_core::skills::SkillId> = match engine
                    .list_skills(&meerkat_core::skills::SkillFilter::default())
                    .await
                {
                    Ok(descs) => descs.into_iter().map(|d| d.id).collect(),
                    Err(_) => Vec::new(),
                };

                (inventory, preloaded_sections, Some(skill_ids))
            } else {
                // Skills disabled or no source
                (String::new(), Vec::new(), None)
            }
        };
        #[cfg(not(feature = "skills"))]
        let skill_inventory_section: (
            String,
            Vec<String>,
            Option<Vec<meerkat_core::skills::SkillId>>,
        ) = (String::new(), Vec::new(), None);
        let (inventory_section, preloaded_skill_sections, active_skill_ids) =
            skill_inventory_section;

        // 12. Build system prompt (single canonical path)
        let mut extra_sections: Vec<&str> = Vec::new();
        // Only inject skill inventory (with tool guidance) when builtins are
        // enabled — otherwise browse_skills/load_skill don't exist.
        if !inventory_section.is_empty() && effective_builtins {
            extra_sections.push(inventory_section.as_str());
        }
        for section in &preloaded_skill_sections {
            extra_sections.push(section.as_str());
        }
        #[cfg(not(target_arch = "wasm32"))]
        let system_prompt = crate::assemble_system_prompt(
            config,
            per_request_prompt.as_deref(),
            conventions_context_root,
            &extra_sections,
            &tool_usage_instructions,
        )
        .await;
        #[cfg(target_arch = "wasm32")]
        let system_prompt = {
            let mut prompt = per_request_prompt.unwrap_or_default();
            for section in &extra_sections {
                if !section.is_empty() {
                    prompt.push_str("\n\n");
                    prompt.push_str(section);
                }
            }
            if !tool_usage_instructions.is_empty() {
                prompt.push_str("\n\n");
                prompt.push_str(&tool_usage_instructions);
            }
            prompt
        };

        // 12. Build AgentBuilder
        let budget_limits = build_config
            .budget_limits
            .unwrap_or_else(|| config.budget_limits());

        let mut builder = AgentBuilder::new()
            .model(model.clone())
            .max_tokens_per_turn(max_tokens)
            .budget(budget_limits)
            .system_prompt(system_prompt)
            .structured_output_retries(build_config.structured_output_retries)
            .with_hook_run_overrides(build_config.hooks_override);

        if let Some(schema) = build_config.output_schema {
            builder = builder.output_schema(schema);
        }
        if let Some(session) = build_config.resume_session {
            builder = builder.resume_session(session);
        }
        #[cfg(feature = "comms")]
        let comms_enabled = comms_runtime.is_some();
        #[cfg(not(feature = "comms"))]
        let comms_enabled = false;
        #[cfg(feature = "comms")]
        if let Some(runtime) = comms_runtime {
            builder =
                builder.with_comms_runtime(
                    Arc::new(runtime) as Arc<dyn meerkat_core::agent::CommsRuntime>
                );
        }
        if let Some(engine) = hook_engine {
            builder = builder.with_hook_engine(engine);
        }

        // 12b. Wire memory store + memory_search tool (when feature compiled + enabled)
        #[allow(unused_variables)]
        let effective_memory = build_config.override_memory.unwrap_or(self.enable_memory);
        #[cfg(feature = "memory-store-session")]
        if effective_memory {
            let memory_dir = self.store_path.join("memory");
            match meerkat_memory::HnswMemoryStore::open(&memory_dir) {
                Ok(store) => {
                    let store = Arc::new(store) as Arc<dyn meerkat_core::memory::MemoryStore>;
                    builder = builder.memory_store(Arc::clone(&store));

                    // Compose memory_search tool into the dispatcher
                    let memory_dispatcher =
                        meerkat_memory::MemorySearchDispatcher::new(Arc::clone(&store));
                    let gateway = meerkat_core::ToolGatewayBuilder::new()
                        .add_dispatcher(tools)
                        .add_dispatcher(Arc::new(memory_dispatcher))
                        .build()
                        .map_err(|e| {
                            BuildAgentError::Config(format!("Failed to compose memory tools: {e}"))
                        })?;
                    tools = Arc::new(gateway);
                    // Tool guidance reaches the model via the embedded
                    // `memory-retrieval` skill (loaded in step 11), not
                    // through usage_instructions strings.
                }
                Err(e) => {
                    tracing::warn!(
                        "Failed to open HnswMemoryStore at {}: {e}",
                        memory_dir.display()
                    );
                }
            }
        }

        // 12c. Wire compactor (when session-compaction is enabled)
        #[cfg(feature = "session-compaction")]
        {
            let compactor = Arc::new(meerkat_session::DefaultCompactor::new(
                config.compaction.clone().into(),
            ));
            builder = builder.compactor(compactor);
        }

        // 12d. Wire skill engine for per-turn /skill-ref activation
        if let Some(engine) = skill_engine {
            builder = builder.with_skill_engine(engine);
        }

        // 12e. Wire shared event tap (shared with LLM adapter)
        builder = builder.with_event_tap(event_tap);
        if let Some(tx) = build_config.event_tx {
            builder = builder.with_default_event_tx(tx);
        }
        if let Some(tx) = build_config.scoped_event_tx {
            builder = builder.with_default_scoped_event_tx(tx);
        }
        if let Some(path) = build_config.scoped_event_path {
            builder = builder.with_default_scope_path(path);
        }

        // 12f. Wire session checkpointer for host-mode persistence
        if let Some(cp) = build_config.checkpointer {
            builder = builder.with_checkpointer(cp);
        }

        // 12g. Wire silent comms intents
        if !build_config.silent_comms_intents.is_empty() {
            builder = builder.with_silent_comms_intents(build_config.silent_comms_intents);
        }
        builder =
            builder.with_max_inline_peer_notifications(build_config.max_inline_peer_notifications);

        // 13. Build agent
        let mut agent = builder.build(llm_adapter, tools, store_adapter).await;

        // 14. Set SessionMetadata
        let metadata = SessionMetadata {
            model,
            max_tokens,
            provider,
            tooling: SessionTooling {
                builtins: effective_builtins,
                shell: effective_shell,
                comms: comms_enabled,
                subagents: effective_subagents,
                mob: build_config.override_mob.unwrap_or(self.enable_mob),
                active_skills: active_skill_ids,
            },
            host_mode: build_config.host_mode,
            comms_name: build_config.comms_name,
            peer_meta: build_config.peer_meta,
            realm_id: build_config.realm_id,
            instance_id: build_config.instance_id,
            backend: build_config.backend,
            config_generation: build_config.config_generation,
        };
        if let Err(err) = agent.session_mut().set_session_metadata(metadata) {
            tracing::warn!("Failed to store session metadata: {}", err);
        }

        Ok(agent)
    }
}

impl AgentFactory {
    /// Build the tool dispatcher and usage instructions.
    ///
    /// `effective_builtins`, `effective_shell`, and `effective_subagents` override
    /// the factory-level flags for this specific build. Delegates to
    /// [`Self::build_builtin_dispatcher`] for the full sub-agent wiring path.
    #[cfg(not(target_arch = "wasm32"))]
    #[allow(clippy::too_many_arguments)]
    async fn build_tool_dispatcher_for_agent_with_overrides(
        &self,
        _config: &Config,
        external: Option<Arc<dyn AgentToolDispatcher>>,
        effective_builtins: bool,
        effective_shell: bool,
        effective_subagents: bool,
        skill_engine: Option<Arc<meerkat_core::skills::SkillRuntime>>,
        sub_agent_scoped_event_tx: Option<mpsc::Sender<ScopedAgentEvent>>,
        sub_agent_scope_path: Option<Vec<StreamScopeFrame>>,
        #[cfg(all(feature = "sub-agents", feature = "comms"))] sub_agent_comms: Option<
            SubAgentCommsWiring,
        >,
    ) -> Result<(Arc<dyn AgentToolDispatcher>, String), BuildAgentError> {
        if !effective_builtins {
            // No builtins — return the external tools if provided, otherwise empty.
            return match external {
                Some(ext) => {
                    let usage = render_tool_usage_instructions(ext.tools().as_ref());
                    Ok((ext, usage))
                }
                None => Ok((Arc::new(EmptyToolDispatcher), String::new())),
            };
        }

        // Create a task store - use in-memory for simplicity; callers that need
        // file-backed persistence should use the lower-level APIs.
        let task_store: Arc<dyn TaskStore> = match self.project_root.as_ref() {
            Some(root) => Arc::new(FileTaskStore::in_project(root)),
            None => Arc::new(MemoryTaskStore::new()),
        };

        // Create shell config if shell is enabled
        let shell_config = if effective_shell {
            let project_root = self
                .project_root
                .clone()
                .unwrap_or_else(|| self.store_path.clone());
            Some(ShellConfig::with_project_root(project_root))
        } else {
            None
        };

        // Create builtin tool config - enable shell tools in policy if shell is enabled
        let builtin_config = if effective_shell {
            BuiltinToolConfig {
                policy: ToolPolicyLayer::new()
                    .enable_tool("shell")
                    .enable_tool("shell_job_status")
                    .enable_tool("shell_jobs")
                    .enable_tool("shell_job_cancel"),
                ..Default::default()
            }
        } else {
            BuiltinToolConfig::default()
        };

        // Use a temporary factory with the effective subagents flag to delegate
        // to build_builtin_dispatcher which has the full sub-agent wiring.
        let mut temp_factory = self.clone();
        temp_factory.enable_subagents = effective_subagents;
        let dispatcher = temp_factory
            .build_builtin_dispatcher_with_skills_internal(
                task_store,
                builtin_config,
                shell_config,
                external,
                None,
                skill_engine,
                sub_agent_scoped_event_tx,
                sub_agent_scope_path,
                #[cfg(all(feature = "sub-agents", feature = "comms"))]
                sub_agent_comms,
            )
            .await?;

        let usage = render_tool_usage_instructions(dispatcher.tools().as_ref());
        Ok((dispatcher, usage))
    }
}

fn render_tool_usage_instructions(tools: &[Arc<meerkat_core::ToolDef>]) -> String {
    if tools.is_empty() {
        return String::new();
    }
    let mut out = String::from("# Available Tools\n\n");
    for tool in tools {
        out.push_str(&format!("## {}\n{}\n\n", tool.name, tool.description));
    }
    out
}
