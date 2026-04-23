//! AgentFactory - shared wiring for Meerkat interfaces.

#[cfg(not(feature = "memory-store"))]
use async_trait::async_trait;
use std::collections::BTreeMap;
#[cfg(feature = "skills")]
use std::collections::BTreeSet;
#[cfg(not(feature = "memory-store"))]
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use meerkat_client::{FactoryError, LlmClient, LlmClientAdapter};
#[cfg(feature = "openai")]
use meerkat_client::{OpenAiCompatibleClient, OpenAiCompatibleMode};
use meerkat_core::ops_lifecycle::OpsLifecycleRegistry;
use meerkat_core::service::{CreateSessionRequest, SessionBuildOptions};

/// Default system prompt for wasm32 builds.
/// Mirrors `meerkat_core::prompt::DEFAULT_SYSTEM_PROMPT` which is gated
/// behind `#[cfg(not(target_arch = "wasm32"))]` due to filesystem deps.
#[cfg(target_arch = "wasm32")]
const DEFAULT_WASM_SYSTEM_PROMPT: &str = r"You are an autonomous agent. Your task is to accomplish the user's goal by systematically using the tools available to you.

# Core Behavior
- Break complex tasks into steps and execute them one by one.
- Use tools to gather information, take actions, and verify results.
- When multiple tool calls are independent, execute them in parallel.
- If a tool call fails, analyze the error and try alternative approaches.
- Continue working until the task is complete or you determine it cannot be completed.

# Decision Making
- Act on the information you have. Make reasonable assumptions when necessary.
- If critical information is missing and no tool can provide it, state what you need and why.
- Prioritize correctness over speed. Verify your work when possible.

# Output
- When the task is complete, provide a clear summary of what was accomplished.
- If the task cannot be completed, explain what blocked progress and what was attempted.";
use meerkat_core::{
    Agent, AgentBuilder, AgentEvent, AgentLlmClient, AgentSessionStore, AgentToolDispatcher,
    BlobStore, BudgetLimits, Config, HookRunOverrides, ModelRegistry, OutputSchema, Provider,
    Session, SessionLlmIdentity, SessionMetadata, SessionTooling, ToolCategoryOverride,
};
#[cfg(not(feature = "memory-store"))]
use meerkat_core::{SessionId, SessionMeta};
use meerkat_runtime::RuntimeOpsLifecycleRegistry;
#[cfg(feature = "jsonl-store")]
use meerkat_store::JsonlStore;
#[cfg(all(feature = "memory-store", not(feature = "jsonl-store")))]
use meerkat_store::MemoryStore;
#[cfg(not(feature = "memory-store"))]
use meerkat_store::SessionFilter;
use meerkat_store::{SessionStore, StoreAdapter};
#[cfg(not(target_arch = "wasm32"))]
use meerkat_tools::BuiltinDispatcherConfig;
use meerkat_tools::CompositeDispatcherError;
use meerkat_tools::EmptyToolDispatcher;
#[cfg(all(not(feature = "session-store"), not(target_arch = "wasm32")))]
use meerkat_tools::builtin::FileTaskStore;
#[cfg(all(not(feature = "session-store"), not(target_arch = "wasm32")))]
use meerkat_tools::builtin::MemoryTaskStore;
#[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
use meerkat_tools::builtin::SqliteTaskStore;
#[cfg(not(target_arch = "wasm32"))]
use meerkat_tools::builtin::shell::ShellConfig;
#[cfg(not(target_arch = "wasm32"))]
use meerkat_tools::builtin::{BuiltinToolConfig, CompositeDispatcher, TaskStore, ToolPolicyLayer};
use meerkat_tools::{CatalogControlDispatcher, CatalogControlVisibilityProvider};
#[cfg(all(not(feature = "memory-store"), not(target_arch = "wasm32")))]
use tokio::sync::RwLock;
#[cfg(not(target_arch = "wasm32"))]
use tokio::sync::mpsc;
#[cfg(all(not(feature = "memory-store"), target_arch = "wasm32"))]
use tokio_with_wasm::alias::sync::RwLock;
#[cfg(target_arch = "wasm32")]
use tokio_with_wasm::alias::sync::mpsc;

#[cfg(feature = "comms")]
use crate::compose_tools_with_comms;
#[cfg(not(target_arch = "wasm32"))]
use crate::{create_default_hook_engine, resolve_layered_hooks_config};

/// Ephemeral in-process store used when no storage backend feature is enabled.
#[cfg(not(feature = "memory-store"))]
#[derive(Default)]
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
    async fn save(&self, session: &Session) -> Result<(), meerkat_store::SessionStoreError> {
        self.sessions
            .write()
            .await
            .insert(session.id().clone(), session.clone());
        Ok(())
    }

    async fn load(
        &self,
        id: &SessionId,
    ) -> Result<Option<Session>, meerkat_store::SessionStoreError> {
        Ok(self.sessions.read().await.get(id).cloned())
    }

    async fn list(
        &self,
        filter: SessionFilter,
    ) -> Result<Vec<SessionMeta>, meerkat_store::SessionStoreError> {
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

    async fn delete(&self, id: &SessionId) -> Result<(), meerkat_store::SessionStoreError> {
        self.sessions.write().await.remove(id);
        Ok(())
    }
}

/// Type-erased agent using trait objects.
pub type DynAgent = Agent<dyn AgentLlmClient, dyn AgentToolDispatcher, dyn AgentSessionStore>;

#[derive(Clone)]
struct ErasedLlmClientOverride(Arc<dyn LlmClient>);

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
    /// Durable self-hosted server binding for configured aliases.
    pub self_hosted_server_id: Option<String>,
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
    /// Whether to keep the agent alive after the initial turn (enables comms drain loop).
    pub keep_alive: bool,
    /// Name for the comms participant (required when `keep_alive` is `true`).
    pub comms_name: Option<String>,
    /// Friendly metadata for peer discovery (flows to `InprocRegistry` and `peers()` output).
    pub peer_meta: Option<meerkat_core::PeerMeta>,
    /// Resume from an existing session instead of starting fresh.
    pub resume_session: Option<Session>,
    /// Budget limits. If `None`, uses `Config::budget_limits()`.
    pub budget_limits: Option<BudgetLimits>,
    /// Optional event channel for streaming agent events.
    pub event_tx: Option<mpsc::Sender<AgentEvent>>,
    /// Override LLM client (for testing or embedding).
    pub llm_client_override: Option<Arc<dyn LlmClient>>,
    /// Provider-specific parameters (e.g., thinking config, reasoning effort).
    pub provider_params: Option<serde_json::Value>,
    /// External tool dispatcher to compose with builtins (e.g., MCP callback tools).
    pub external_tools: Option<Arc<dyn AgentToolDispatcher>>,
    /// Serializable tool definitions that can rebuild recoverable
    /// surface-owned dispatchers after persistence or runtime restart.
    pub recoverable_tool_defs: Option<Vec<meerkat_core::ToolDef>>,
    /// Optional blob store override used for image externalization/hydration.
    pub blob_store_override: Option<Arc<dyn BlobStore>>,
    /// Per-build override for factory-level `enable_builtins`.
    /// `Inherit` defers to the factory default.
    pub override_builtins: ToolCategoryOverride,
    /// Per-build override for factory-level `enable_shell`.
    /// `Inherit` defers to the factory default.
    pub override_shell: ToolCategoryOverride,
    /// Per-build override for factory-level `enable_memory`.
    /// `Inherit` defers to the factory default.
    pub override_memory: ToolCategoryOverride,
    /// Per-build override for factory-level `enable_schedule`.
    /// `Inherit` defers to the factory default.
    pub override_schedule: ToolCategoryOverride,
    /// Per-build override for factory-level `enable_mob`.
    pub override_mob: ToolCategoryOverride,
    /// Agent-facing scheduler tools supplied by the embedding surface.
    ///
    /// Scheduler is a surface capability. This dispatcher only controls
    /// visibility/composition; schedule execution remains owned by the
    /// schedule service/host chosen by the embedding surface.
    pub schedule_tools: Option<Arc<dyn AgentToolDispatcher>>,
    /// Runtime-injected mob operator authority context.
    ///
    /// Tool visibility may depend on this context being present, but
    /// dispatch-time authorization must still re-check the typed create/scope
    /// fields on every operator tool call.
    pub mob_tool_authority_context: Option<meerkat_core::service::MobToolAuthorityContext>,
    /// Late-binding mob tool factory, invoked inside `build_agent()` with
    /// session-scoped args (session ID, ops lifecycle, comms runtime) to produce
    /// the mob tool dispatcher. Composed into the tool gateway after comms.
    pub mob_tools: Option<Arc<dyn meerkat_core::service::MobToolsFactory>>,
    /// Skills to pre-load at build time (full body injected into system prompt).
    /// `None` = metadata-only inventory (agent discovers and loads via tools).
    /// `Some(ids)` = pre-load these skills into the system prompt.
    /// `Some(vec![])` is normalized to `None`.
    pub preload_skills: Option<Vec<meerkat_core::skills::SkillId>>,
    /// Realm identity for cross-surface storage sharing/isolation.
    pub realm_id: Option<String>,
    /// Optional process/agent instance identifier within a realm.
    pub instance_id: Option<String>,
    /// Backend pinned by the realm manifest (e.g. "sqlite", "jsonl").
    pub backend: Option<String>,
    /// Config generation used when this session was created/resumed.
    pub config_generation: Option<u64>,
    /// Realm-scoped connection binding (Phase 3 provider-auth redesign).
    ///
    /// When `Some`, `build_agent()` routes provider/auth resolution through
    /// `meerkat-client::runtime::ProviderRuntimeRegistry` against the
    /// realm-scoped `Config.realm[realm_id]` definitions. When `None`, the
    /// legacy flat `(provider, api_key, base_url)` path runs (removed in
    /// Phase 6). `llm_client_override` beats both paths.
    pub connection_ref: Option<meerkat_core::ConnectionRef>,
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

    /// Opaque application context for custom `SessionAgentBuilder` implementations.
    /// Not consumed by the standard build pipeline.
    pub app_context: Option<serde_json::Value>,
    /// Additional instruction sections appended to the system prompt after skill
    /// assembly, before tool instructions. Order preserved.
    pub additional_instructions: Option<Vec<String>>,
    /// When true, the surface should block after MCP tool loading until all
    /// servers finish connecting before starting the first agent turn.
    /// Default: false (servers connect in the background).
    pub wait_for_mcp: bool,
    /// Per-agent environment variables injected into shell tool subprocesses.
    pub shell_env: Option<std::collections::HashMap<String, String>>,
    /// Optional session checkpointer for host-mode persistence.
    pub checkpointer: Option<Arc<dyn meerkat_core::checkpoint::SessionCheckpointer>>,
    /// Explicit call-timeout override at the build seam.
    ///
    /// - `Inherit` (default): defer to config override, then profile default
    /// - `Disabled`: explicitly disable call timeout regardless of profile
    /// - `Value(d)`: explicitly set call timeout to `d`
    pub call_timeout_override: meerkat_core::CallTimeoutOverride,
    /// Typed explicit-override intent for resumed-session metadata merges.
    pub resume_override_mask: meerkat_core::service::ResumeOverrideMask,
    /// Runtime build mode — determines how the factory resolves the ops lifecycle
    /// registry and completion feed. See [`RuntimeBuildMode`] for details.
    pub runtime_build_mode: meerkat_core::RuntimeBuildMode,
    /// Pre-resolved metadata entries to inject into the session before agent build.
    ///
    /// These are set on the session's internal metadata map before `AgentBuilder::build()`
    /// runs, so they are available for early-stage recovery (e.g. inherited tool filter).
    /// Entries here take precedence over any resumed session metadata for the same key.
    pub initial_metadata_entries: std::collections::BTreeMap<String, serde_json::Value>,
}

impl std::fmt::Debug for AgentBuildConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentBuildConfig")
            .field("model", &self.model)
            .field("provider", &self.provider)
            .field("self_hosted_server_id", &self.self_hosted_server_id)
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
            .field("keep_alive", &self.keep_alive)
            .field("resume_override_mask", &self.resume_override_mask)
            .field("comms_name", &self.comms_name)
            .field("peer_meta", &self.peer_meta)
            .field("resume_session", &self.resume_session.is_some())
            .field("budget_limits", &self.budget_limits)
            .field("event_tx", &self.event_tx.is_some())
            .field("llm_client_override", &self.llm_client_override.is_some())
            .field("provider_params", &self.provider_params.is_some())
            .field("external_tools", &self.external_tools.is_some())
            .field("recoverable_tool_defs", &self.recoverable_tool_defs)
            .field("blob_store_override", &self.blob_store_override.is_some())
            .field("override_builtins", &self.override_builtins)
            .field("override_shell", &self.override_shell)
            .field("override_memory", &self.override_memory)
            .field("override_schedule", &self.override_schedule)
            .field("override_mob", &self.override_mob)
            .field("schedule_tools", &self.schedule_tools.is_some())
            .field(
                "mob_tool_authority_context",
                &self.mob_tool_authority_context.is_some(),
            )
            .field("mob_tools", &self.mob_tools.is_some())
            .field("realm_id", &self.realm_id)
            .field("instance_id", &self.instance_id)
            .field("backend", &self.backend)
            .field("config_generation", &self.config_generation)
            .field(
                "max_inline_peer_notifications",
                &self.max_inline_peer_notifications,
            )
            .field(
                "tool_dispatcher_override",
                &self.tool_dispatcher_override.is_some(),
            )
            .field(
                "session_store_override",
                &self.session_store_override.is_some(),
            )
            .field("hook_engine_override", &self.hook_engine_override.is_some())
            .field(
                "skill_engine_override",
                &self.skill_engine_override.is_some(),
            )
            .field("app_context", &self.app_context.is_some())
            .field("additional_instructions", &self.additional_instructions)
            .field("wait_for_mcp", &self.wait_for_mcp)
            .field("runtime_build_mode", &self.runtime_build_mode)
            .finish()
    }
}

impl AgentBuildConfig {
    /// Create a new build config with sensible defaults for the given model.
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            provider: None,
            self_hosted_server_id: None,
            max_tokens: None,
            system_prompt: None,
            output_schema: None,
            structured_output_retries: 2,
            hooks_override: HookRunOverrides::default(),
            keep_alive: false,
            comms_name: None,
            peer_meta: None,
            resume_session: None,
            budget_limits: None,
            event_tx: None,
            llm_client_override: None,
            provider_params: None,
            external_tools: None,
            recoverable_tool_defs: None,
            blob_store_override: None,
            override_builtins: ToolCategoryOverride::Inherit,
            override_shell: ToolCategoryOverride::Inherit,
            override_memory: ToolCategoryOverride::Inherit,
            override_schedule: ToolCategoryOverride::Inherit,
            override_mob: ToolCategoryOverride::Inherit,
            schedule_tools: None,
            mob_tool_authority_context: None,
            mob_tools: None,
            preload_skills: None,
            realm_id: None,
            instance_id: None,
            backend: None,
            config_generation: None,
            connection_ref: None,
            silent_comms_intents: Vec::new(),
            max_inline_peer_notifications: None,
            tool_dispatcher_override: None,
            session_store_override: None,
            hook_engine_override: None,
            skill_engine_override: None,
            app_context: None,
            additional_instructions: None,
            wait_for_mcp: false,
            shell_env: None,
            checkpointer: None,
            call_timeout_override: meerkat_core::CallTimeoutOverride::default(),
            resume_override_mask: meerkat_core::service::ResumeOverrideMask::default(),
            runtime_build_mode: meerkat_core::RuntimeBuildMode::StandaloneEphemeral,
            initial_metadata_entries: std::collections::BTreeMap::new(),
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
        if let Some(options) = &req.build {
            build.apply_session_build_options(options);
        }
        build.event_tx = Some(event_tx);
        build
    }

    /// Apply the shared host/runtime default for explicit mob operator
    /// enablement.
    ///
    /// This keeps `override_mob` and the generated create-only authority
    /// context aligned at the composition seam. Existing-mob scope must be
    /// injected explicitly elsewhere; this helper never infers it.
    pub fn apply_persisted_mob_operator_access(
        &mut self,
        enable_mob: ToolCategoryOverride,
        persisted_authority_context: Option<meerkat_core::service::MobToolAuthorityContext>,
    ) {
        let (override_mob, authority_context) = meerkat_core::service::resolve_mob_operator_access(
            enable_mob,
            persisted_authority_context,
        );
        self.override_mob = override_mob;
        self.mob_tool_authority_context = authority_context;
    }

    pub fn apply_generated_create_only_mob_operator_access(
        &mut self,
        enable_mob: ToolCategoryOverride,
    ) {
        self.apply_persisted_mob_operator_access(enable_mob, None);
    }

    /// Merge `SessionBuildOptions` into this build config.
    pub fn apply_session_build_options(&mut self, build: &SessionBuildOptions) {
        self.provider = build.provider;
        self.self_hosted_server_id = build.self_hosted_server_id.clone();
        self.output_schema = build.output_schema.clone();
        self.structured_output_retries = build.structured_output_retries;
        self.hooks_override = build.hooks_override.clone();
        self.comms_name = build.comms_name.clone();
        self.peer_meta = build.peer_meta.clone();
        self.resume_session = build.resume_session.clone();
        self.budget_limits = build.budget_limits.clone();
        self.provider_params = build.provider_params.clone();
        self.external_tools = build.external_tools.clone();
        self.recoverable_tool_defs = build.recoverable_tool_defs.clone();
        self.blob_store_override = build.blob_store_override.clone();
        self.llm_client_override = build
            .llm_client_override
            .as_ref()
            .and_then(decode_llm_client_override_from_service);
        self.override_builtins = build.override_builtins;
        self.override_shell = build.override_shell;
        self.override_memory = build.override_memory;
        self.override_schedule = build.override_schedule;
        self.override_mob = build.override_mob;
        self.schedule_tools = build.schedule_tools.clone();
        self.mob_tool_authority_context = build.mob_tool_authority_context.clone();
        self.mob_tools = build.mob_tools.clone();
        self.preload_skills = build.preload_skills.clone();
        self.realm_id = build.realm_id.clone();
        self.instance_id = build.instance_id.clone();
        self.backend = build.backend.clone();
        self.config_generation = build.config_generation;
        // Phase 3: connection_ref flows from SessionBuildOptions into
        // AgentBuildConfig so surfaces can drive binding selection per-request.
        self.connection_ref = build.connection_ref.clone();
        self.keep_alive = build.keep_alive;
        self.silent_comms_intents
            .clone_from(&build.silent_comms_intents);
        self.max_inline_peer_notifications = build.max_inline_peer_notifications;
        self.app_context = build.app_context.clone();
        self.additional_instructions = build.additional_instructions.clone();
        self.shell_env = build.shell_env.clone();
        self.checkpointer = build.checkpointer.clone();
        self.call_timeout_override = build.call_timeout_override.clone();
        self.resume_override_mask = build.resume_override_mask;
        self.runtime_build_mode = build.runtime_build_mode.clone();
    }

    /// Convert build options to the service transport representation.
    pub fn to_session_build_options(&self) -> SessionBuildOptions {
        SessionBuildOptions {
            provider: self.provider,
            self_hosted_server_id: self.self_hosted_server_id.clone(),
            output_schema: self.output_schema.clone(),
            structured_output_retries: self.structured_output_retries,
            hooks_override: self.hooks_override.clone(),
            comms_name: self.comms_name.clone(),
            peer_meta: self.peer_meta.clone(),
            resume_session: self.resume_session.clone(),
            budget_limits: self.budget_limits.clone(),
            provider_params: self.provider_params.clone(),
            external_tools: self.external_tools.clone(),
            recoverable_tool_defs: self.recoverable_tool_defs.clone(),
            blob_store_override: self.blob_store_override.clone(),
            llm_client_override: self
                .llm_client_override
                .clone()
                .map(encode_llm_client_override_for_service),
            override_builtins: self.override_builtins,
            override_shell: self.override_shell,
            override_memory: self.override_memory,
            override_schedule: self.override_schedule,
            override_mob: self.override_mob,
            schedule_tools: self.schedule_tools.clone(),
            mob_tool_authority_context: self.mob_tool_authority_context.clone(),
            mob_tools: self.mob_tools.clone(),
            preload_skills: self.preload_skills.clone(),
            realm_id: self.realm_id.clone(),
            instance_id: self.instance_id.clone(),
            backend: self.backend.clone(),
            config_generation: self.config_generation,
            connection_ref: self.connection_ref.clone(),
            keep_alive: self.keep_alive,
            silent_comms_intents: self.silent_comms_intents.clone(),
            max_inline_peer_notifications: self.max_inline_peer_notifications,
            app_context: self.app_context.clone(),
            additional_instructions: self.additional_instructions.clone(),
            shell_env: self.shell_env.clone(),
            checkpointer: self.checkpointer.clone(),
            call_timeout_override: self.call_timeout_override.clone(),
            resume_override_mask: self.resume_override_mask,
            runtime_build_mode: self.runtime_build_mode.clone(),
        }
    }
}

/// Errors that can occur when building an agent via [`AgentFactory::build_agent()`].
#[derive(Debug, thiserror::Error)]
pub enum BuildAgentError {
    /// Cannot infer provider from the given model name.
    #[error("Cannot infer provider from model '{model}'")]
    UnknownProvider { model: String },

    /// Realm-scoped connection binding failed to resolve into a client.
    ///
    /// Produced by the Phase 3 `ProviderRuntimeRegistry` dispatch path
    /// when `AgentBuildConfig.connection_ref` is set. Wraps the
    /// underlying provider-runtime error as a stringified message.
    #[error("Connection resolution failed: {0}")]
    ConnectionResolution(String),

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

    /// `keep_alive` was set but `comms_name` is missing.
    #[error("keep_alive requires comms_name to be set")]
    #[cfg(feature = "comms")]
    KeepAliveRequiresCommsName,
}

/// Resolver that delegates to `meerkat_models::profile::profile_for()`
/// to look up model-specific operational defaults at call time.
///
/// This struct bridges the dependency gap: `meerkat-core` owns the
/// `ModelOperationalDefaultsResolver` trait, and this facade-layer
/// implementation provides the concrete `meerkat-models` lookup.
struct RegistryBackedDefaultsResolver {
    registry: Arc<ModelRegistry>,
}

impl meerkat_core::ModelOperationalDefaultsResolver for RegistryBackedDefaultsResolver {
    fn call_timeout_for(&self, provider: &str, model: &str) -> Option<std::time::Duration> {
        let _ = provider;
        self.registry
            .profile_for(model)
            .and_then(|p| p.call_timeout_secs)
            .map(std::time::Duration::from_secs)
    }
}

/// Return the canonical string key for a provider.
pub fn provider_key(provider: Provider) -> &'static str {
    provider.as_str()
}

/// Return `true` when the OpenAI model ID advertises
/// `ModelCapabilities.realtime == true` in the curated catalog.
///
/// Drives the AgentFactory branch that routes text turns over the OpenAI
/// Realtime WebSocket instead of the Responses API. OpenAI rejects
/// realtime model IDs on `POST /v1/responses` with
/// `model_not_found`, so any session whose resolved model is
/// realtime-capable must use the WebSocket text adapter.
fn is_openai_realtime_capable(model: &str) -> bool {
    meerkat_core::model_profile::capabilities::capabilities_for("openai", model)
        .map(|caps| caps.realtime)
        .unwrap_or(false)
}

/// Deferred snapshot provider that captures visible tools from a composed tool dispatcher.
///
/// Created before mob tool composition (so it can be passed into `MobToolsBuildArgs`),
/// then updated with the final composed dispatcher after mob tools are added.
/// This ensures the snapshot includes mob/profile tools the parent currently has.
///
/// Uses a `Weak` reference to avoid inflating the `Arc` strong count on the dispatcher,
/// which would cause `bind_ops_lifecycle` to reject rebinding due to shared ownership.
struct DeferredSnapshotProvider {
    dispatcher: std::sync::RwLock<Option<std::sync::Weak<dyn AgentToolDispatcher>>>,
}

impl DeferredSnapshotProvider {
    fn new() -> Self {
        Self {
            dispatcher: std::sync::RwLock::new(None),
        }
    }

    fn set_dispatcher(&self, dispatcher: &Arc<dyn AgentToolDispatcher>) {
        let mut guard = self
            .dispatcher
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *guard = Some(Arc::downgrade(dispatcher));
    }
}

impl meerkat_core::service::VisibleToolSnapshotProvider for DeferredSnapshotProvider {
    fn snapshot_visible_tools(&self) -> Vec<Arc<meerkat_core::types::ToolDef>> {
        let guard = self
            .dispatcher
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match guard.as_ref().and_then(std::sync::Weak::upgrade) {
            Some(d) => d.tools().to_vec(),
            None => Vec::new(),
        }
    }
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
    #[cfg(feature = "comms")]
    pub enable_comms: bool,
    pub enable_memory: bool,
    pub enable_schedule: bool,
    pub enable_mob: bool,
    /// Optional skill source override. When set, bypasses config-driven
    /// repository resolution. For SDK users who wire sources programmatically.
    #[cfg(feature = "skills")]
    pub skill_source: Option<Arc<meerkat_skills::CompositeSkillSource>>,
    /// Optional custom session store. When set, `build_agent()` uses this
    /// instead of the feature-flag-based default (jsonl, memory, or ephemeral).
    custom_store: Option<Arc<dyn SessionStore>>,
    /// Default mob tools factory injected into all builds when mob is enabled.
    /// Surfaces set this when constructing the factory, so every agent built
    /// through this factory gets mob delegation tools without each session
    /// needing to set `SessionBuildOptions.mob_tools`.
    pub mob_tools: Option<Arc<dyn meerkat_core::service::MobToolsFactory>>,
    /// Pre-built comms runtime shared across all sessions built by this factory.
    ///
    /// When set, `build_agent()` uses this runtime for tool composition and
    /// agent wiring instead of creating a per-session runtime from config.
    /// Used by surfaces with stable identity (e.g., a target agent that keeps
    /// the same keypair and TCP listener across session restarts).
    #[cfg(feature = "comms")]
    pub comms_runtime: Option<Arc<meerkat_comms::CommsRuntime>>,
    /// Persistent TokenStore used by the provider-runtime registry when
    /// resolving OAuth-backed bindings (Claude.ai / ChatGPT / Google
    /// Code Assist). When `None`, OAuth bindings surface
    /// `AuthError::InteractiveLoginRequired` — CLI / REST / RPC surfaces
    /// set this to an `AutoTokenStore` during construction so the whole
    /// stack reads the same persisted credentials.
    #[cfg(not(target_arch = "wasm32"))]
    pub token_store: Option<Arc<dyn meerkat_providers::auth_store::TokenStore>>,
    /// Refresh coordinator for OAuth token lifecycle. When `None`, a
    /// fresh `InMemoryCoordinator` is created per build; callers that
    /// need cross-process refresh dedup (e.g. concurrent CLI runs) set
    /// a `FileLockCoordinator` here.
    #[cfg(not(target_arch = "wasm32"))]
    pub refresh_coord: Option<Arc<dyn meerkat_providers::auth_store::RefreshCoordinator>>,
    /// External auth resolvers keyed by handle. Merged into
    /// `ResolverEnvironment.external_resolvers` during `build_agent`.
    /// The WASM runtime registers a `WasmExternalAuthResolver` under
    /// `"wasm_host"` so realm bindings configured with
    /// `CredentialSourceSpec::ExternalResolver { handle: "wasm_host" }`
    /// delegate credential resolution to the JS host's OAuth flow.
    pub external_auth_resolvers:
        BTreeMap<String, Arc<dyn meerkat_providers::ExternalAuthResolverHandle>>,
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
            .field("enable_memory", &self.enable_memory)
            .field("enable_schedule", &self.enable_schedule)
            .field("enable_mob", &self.enable_mob);
        #[cfg(feature = "comms")]
        d.field("enable_comms", &self.enable_comms);
        #[cfg(feature = "skills")]
        d.field("skill_source", &self.skill_source.as_ref().map(|_| ".."));
        d.field("custom_store", &self.custom_store.as_ref().map(|_| ".."));
        d.field("mob_tools", &self.mob_tools.is_some());
        #[cfg(feature = "comms")]
        d.field("comms_runtime", &self.comms_runtime.is_some());
        d.finish()
    }
}

impl AgentFactory {
    /// Create a minimal factory for environments without filesystem access (e.g. wasm32).
    ///
    /// All agent resources must be provided via `AgentBuildConfig` overrides
    /// (`tool_dispatcher_override`, `session_store_override`, etc.).
    /// Filesystem-dependent methods are not available.
    pub fn minimal() -> Self {
        Self {
            store_path: PathBuf::new(),
            runtime_root: None,
            project_root: None,
            context_root: None,
            user_config_root: None,
            enable_builtins: false,
            enable_shell: false,
            #[cfg(feature = "comms")]
            enable_comms: false,
            enable_memory: false,
            enable_schedule: false,
            enable_mob: false,
            #[cfg(feature = "skills")]
            skill_source: None,
            custom_store: None,
            mob_tools: None,
            #[cfg(feature = "comms")]
            comms_runtime: None,
            #[cfg(not(target_arch = "wasm32"))]
            token_store: None,
            #[cfg(not(target_arch = "wasm32"))]
            refresh_coord: None,
            external_auth_resolvers: BTreeMap::new(),
        }
    }

    /// Create a new factory with the required session store path.
    ///
    /// The default auto-detecting TokenStore is attached when available —
    /// OAuth-backed bindings will read persisted tokens written by
    /// `rkat auth login` / REST+RPC OAuth completion handlers out of the
    /// box. If the default TokenStore location is unavailable (e.g.
    /// no `$XDG_CONFIG_HOME`), the field stays `None` and OAuth
    /// bindings surface `InteractiveLoginRequired`.
    pub fn new(store_path: impl Into<PathBuf>) -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        let token_store = meerkat_providers::auth_store::TokenStoreBackend::default_auto()
            .and_then(meerkat_providers::auth_store::TokenStoreBackend::open)
            .ok();
        Self {
            store_path: store_path.into(),
            runtime_root: None,
            project_root: None,
            context_root: None,
            user_config_root: None,
            enable_builtins: false,
            enable_shell: false,
            #[cfg(feature = "comms")]
            enable_comms: false,
            enable_memory: false,
            enable_schedule: false,
            enable_mob: false,
            #[cfg(feature = "skills")]
            skill_source: None,
            custom_store: None,
            mob_tools: None,
            #[cfg(feature = "comms")]
            comms_runtime: None,
            #[cfg(not(target_arch = "wasm32"))]
            token_store,
            #[cfg(not(target_arch = "wasm32"))]
            refresh_coord: None,
            external_auth_resolvers: BTreeMap::new(),
        }
    }

    /// Attach a persistent `TokenStore` for OAuth-backed bindings. When
    /// set, the provider-runtime registry reads persisted tokens from
    /// this store during `resolve_binding`.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn with_token_store(
        mut self,
        store: Arc<dyn meerkat_providers::auth_store::TokenStore>,
    ) -> Self {
        self.token_store = Some(store);
        self
    }

    /// Register an external auth resolver that surfaces bindings whose
    /// `CredentialSourceSpec::ExternalResolver { handle }` matches the
    /// supplied `handle`. Used by WASM hosts (browser OAuth flow owned
    /// by the page) and by SDK users embedding meerkat inside an
    /// existing auth story.
    pub fn with_external_auth_resolver(
        mut self,
        handle: impl Into<String>,
        resolver: Arc<dyn meerkat_providers::ExternalAuthResolverHandle>,
    ) -> Self {
        self.external_auth_resolvers.insert(handle.into(), resolver);
        self
    }

    /// Attach a refresh coordinator (cross-process dedup via file lock).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn with_refresh_coordinator(
        mut self,
        coord: Arc<dyn meerkat_providers::auth_store::RefreshCoordinator>,
    ) -> Self {
        self.refresh_coord = Some(coord);
        self
    }

    /// Convenience: attach an auto-detecting `TokenStore` at the user's
    /// default credential directory ($XDG_CONFIG_HOME/meerkat/credentials).
    /// Surfaces (CLI / REST / RPC) call this during factory construction.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn with_default_token_store(
        mut self,
    ) -> Result<Self, meerkat_providers::auth_store::TokenStoreError> {
        let store = meerkat_providers::auth_store::TokenStoreBackend::default_auto()?.open()?;
        self.token_store = Some(store);
        Ok(self)
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

    /// Enable or disable semantic memory (memory_search tool + compaction indexing).
    pub fn memory(mut self, enabled: bool) -> Self {
        self.enable_memory = enabled;
        self
    }

    /// Enable or disable scheduler tools.
    pub fn schedule(mut self, enabled: bool) -> Self {
        self.enable_schedule = enabled;
        self
    }

    /// Enable or disable mob (multi-agent orchestration) tools.
    pub fn mob(mut self, enabled: bool) -> Self {
        self.enable_mob = enabled;
        self
    }

    /// Set the default mob tools factory for all agents built by this factory.
    pub fn mob_tools_factory(
        mut self,
        factory: Arc<dyn meerkat_core::service::MobToolsFactory>,
    ) -> Self {
        self.mob_tools = Some(factory);
        self
    }

    /// Enable or disable comms tools.
    #[cfg(feature = "comms")]
    pub fn comms(mut self, enabled: bool) -> Self {
        self.enable_comms = enabled;
        self
    }

    /// Set a pre-built comms runtime for sessions that don't request their
    /// own identity.
    ///
    /// When set, `build_agent()` uses this runtime for tool composition and
    /// agent wiring — but only when the session's `comms_name` is `None`.
    /// Sessions that set `comms_name` (e.g., mob-spawned members) get their
    /// own per-session runtime so each member has a distinct keypair, inbox,
    /// and trusted-peer set. Sharing the surface runtime with members would
    /// collapse their `PeerCommsMachine` state into one instance, breaking
    /// peer-to-peer addressing.
    #[cfg(feature = "comms")]
    pub fn with_comms_runtime(mut self, runtime: Arc<meerkat_comms::CommsRuntime>) -> Self {
        self.comms_runtime = Some(runtime);
        self
    }

    /// Build a `SkillRuntime` from the factory's skill configuration.
    ///
    /// Returns `None` if skills are disabled or no source is available.
    #[cfg(feature = "skills")]
    fn effective_skill_capabilities(
        &self,
        config: &Config,
        build_config: Option<&AgentBuildConfig>,
    ) -> Vec<String> {
        let mut capabilities: BTreeSet<meerkat_contracts::CapabilityId> =
            meerkat_contracts::available_capabilities(config)
                .into_iter()
                .collect();

        let builtins_enabled = build_config
            .map(|build| build.override_builtins.resolve(self.enable_builtins))
            .unwrap_or(self.enable_builtins);
        if !builtins_enabled {
            capabilities.remove(&meerkat_contracts::CapabilityId::Builtins);
        }

        let shell_enabled = build_config
            .map(|build| build.override_shell.resolve(self.enable_shell))
            .unwrap_or(self.enable_shell);
        if !shell_enabled {
            capabilities.remove(&meerkat_contracts::CapabilityId::Shell);
        }

        let memory_enabled = build_config
            .map(|build| build.override_memory.resolve(self.enable_memory))
            .unwrap_or(self.enable_memory);
        if !memory_enabled {
            capabilities.remove(&meerkat_contracts::CapabilityId::MemoryStore);
        }

        #[cfg(feature = "comms")]
        if !self.enable_comms {
            capabilities.remove(&meerkat_contracts::CapabilityId::Comms);
        }

        capabilities
            .into_iter()
            .map(|cap| cap.to_string())
            .collect()
    }

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
                #[cfg(not(target_arch = "wasm32"))]
                {
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
                }
                #[cfg(target_arch = "wasm32")]
                None
            };

        skill_source.map(|source| {
            let available_caps = self.effective_skill_capabilities(config, None);
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

    #[cfg(any(not(target_arch = "wasm32"), test))]
    fn shell_project_root(&self) -> PathBuf {
        self.project_root.clone().unwrap_or_else(|| {
            // `store_path` may point at an uncreated session leaf (for example
            // `<temp>/sessions`) or a database file. Using it directly as
            // `current_dir` makes process spawn fail with ENOENT, so prefer the
            // nearest existing parent directory when no explicit project root is set.
            self.store_path
                .parent()
                .map(std::path::Path::to_path_buf)
                .filter(|parent| !parent.as_os_str().is_empty())
                .unwrap_or_else(|| {
                    std::env::current_dir().unwrap_or_else(|_| self.store_path.clone())
                })
        })
    }

    fn realm_scope_root(&self, _build_config: &AgentBuildConfig) -> PathBuf {
        self.runtime_root
            .clone()
            .or_else(|| self.project_root.clone())
            .unwrap_or_else(|| self.store_path.clone())
    }

    fn apply_resumed_session_metadata(
        build_config: &mut AgentBuildConfig,
    ) -> Option<SessionMetadata> {
        let metadata = build_config
            .resume_session
            .as_ref()
            .and_then(Session::session_metadata)?;

        let mask = build_config.resume_override_mask;

        if !mask.model {
            build_config.model = metadata.model.clone();
        }
        if !mask.max_tokens {
            build_config.max_tokens = Some(metadata.max_tokens);
        }
        if !mask.structured_output_retries {
            build_config.structured_output_retries = metadata.structured_output_retries;
        }
        if !mask.provider {
            build_config.provider = Some(metadata.provider);
        }
        if !mask.model && !mask.provider {
            build_config.self_hosted_server_id = metadata.self_hosted_server_id.clone();
        }
        if !mask.provider_params {
            build_config.provider_params = metadata.provider_params.clone();
        }
        // Phase 3: propagate persisted connection_ref so resume re-resolves
        // through the same realm binding. The caller keeps precedence —
        // if build_config already carries a connection_ref, leave it.
        if build_config.connection_ref.is_none() {
            build_config.connection_ref = metadata.connection_ref.clone();
        }
        if !mask.override_builtins {
            build_config.override_builtins = metadata.tooling.builtins;
        }
        if !mask.override_shell {
            build_config.override_shell = metadata.tooling.shell;
        }
        if !mask.override_memory {
            build_config.override_memory = metadata.tooling.memory;
        }
        if !mask.override_mob {
            build_config.override_mob = metadata.tooling.mob;
            build_config.mob_tool_authority_context = build_config
                .resume_session
                .as_ref()
                .and_then(Session::mob_tool_authority_context);
        }
        if !mask.preload_skills {
            build_config.preload_skills = metadata.tooling.active_skills.clone();
        }
        if !mask.keep_alive {
            build_config.keep_alive = metadata.keep_alive;
        }
        if !mask.comms_name {
            build_config.comms_name = metadata.comms_name.clone();
        }
        if !mask.peer_meta {
            build_config.peer_meta = metadata.peer_meta.clone();
        }

        Some(metadata)
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

    pub async fn build_llm_client_for_identity(
        &self,
        config: &Config,
        identity: &SessionLlmIdentity,
    ) -> Result<Arc<dyn LlmClient>, FactoryError> {
        let registry = config
            .model_registry()
            .map_err(|err| FactoryError::ClientCreationFailed(err.to_string()))?;
        if matches!(identity.provider, Provider::SelfHosted) {
            return self.build_self_hosted_client_for_identity(&registry, identity);
        }

        // Test-mode shim: hot-swap never needs real credentials under
        // RKAT_TEST_CLIENT=1.
        if std::env::var("RKAT_TEST_CLIENT").ok().as_deref() == Some("1") {
            return Ok(Arc::new(meerkat_client::TestClient::default()));
        }

        let (realm, binding_id): (meerkat_core::RealmConnectionSet, String) =
            match identity.connection_ref.as_ref() {
                Some(conn_ref) => {
                    let section = config.realm.get(&conn_ref.realm_id).ok_or_else(|| {
                        FactoryError::ClientCreationFailed(format!(
                            "realm '{}' not found in config.realm (hot-swap)",
                            conn_ref.realm_id
                        ))
                    })?;
                    let realm =
                        meerkat_core::RealmConnectionSet::from_config(&conn_ref.realm_id, section)
                            .map_err(|e| FactoryError::ClientCreationFailed(e.to_string()))?;
                    (realm, conn_ref.binding_id.clone())
                }
            };

        #[allow(unused_mut)]
        let mut env = meerkat_providers::ResolverEnvironment::with_process_env();
        #[cfg(not(target_arch = "wasm32"))]
        {
            if let Some(store) = self.token_store.clone() {
                env = env.with_token_store(store);
            }
            if let Some(coord) = self.refresh_coord.clone() {
                env = env.with_refresh_coordinator(coord);
            }
        }
        for (handle, resolver) in &self.external_auth_resolvers {
            env = env.with_external_resolver(handle.clone(), resolver.clone());
        }
        let provider_registry = crate::default_provider_registry();
        let connection = provider_registry
            .resolve(&realm, &binding_id, &env)
            .await
            .map_err(|e| FactoryError::ClientCreationFailed(e.to_string()))?;
        provider_registry
            .build_client(connection)
            .map_err(|e| FactoryError::ClientCreationFailed(e.to_string()))
    }

    fn model_registry(&self, config: &Config) -> Result<ModelRegistry, BuildAgentError> {
        config
            .model_registry()
            .map_err(|err| BuildAgentError::Config(err.to_string()))
    }

    fn resolve_provider_from_registry(
        &self,
        registry: &ModelRegistry,
        build_config: &AgentBuildConfig,
    ) -> Result<(Provider, Option<String>), BuildAgentError> {
        if let Some(provider) = build_config.provider {
            return match provider {
                Provider::SelfHosted => {
                    let entry = registry.entry(&build_config.model).ok_or_else(|| {
                        BuildAgentError::Config(format!(
                            "self-hosted model '{}' is not registered in config",
                            build_config.model
                        ))
                    })?;
                    let server_id = entry
                        .self_hosted
                        .as_ref()
                        .map(|server| server.server_id.clone())
                        .ok_or_else(|| {
                            BuildAgentError::Config(format!(
                                "model '{}' is not backed by a self-hosted server",
                                build_config.model
                            ))
                        })?;
                    Ok((
                        Provider::SelfHosted,
                        build_config
                            .self_hosted_server_id
                            .clone()
                            .or(Some(server_id)),
                    ))
                }
                Provider::Other => Ok((Provider::Other, None)),
                other => Ok((other, None)),
            };
        }

        if let Some(entry) = registry.entry(&build_config.model) {
            let server_id = entry
                .self_hosted
                .as_ref()
                .map(|server| server.server_id.clone());
            return Ok((
                entry.provider,
                if matches!(entry.provider, Provider::SelfHosted) {
                    build_config.self_hosted_server_id.clone().or(server_id)
                } else {
                    None
                },
            ));
        }

        let inferred = Provider::infer_from_model(&build_config.model).unwrap_or(Provider::Other);
        if inferred != Provider::Other {
            return Ok((inferred, None));
        }
        if let Some(client) = build_config.llm_client_override.as_ref() {
            return Ok((Provider::from_name(client.provider()), None));
        }

        Err(BuildAgentError::UnknownProvider {
            model: build_config.model.clone(),
        })
    }

    fn build_self_hosted_client_from_registry(
        &self,
        registry: &ModelRegistry,
        identity: &SessionLlmIdentity,
    ) -> Result<Arc<dyn LlmClient>, FactoryError> {
        self.build_self_hosted_client_for_identity(registry, identity)
    }

    fn build_self_hosted_client_for_identity(
        &self,
        registry: &ModelRegistry,
        identity: &SessionLlmIdentity,
    ) -> Result<Arc<dyn LlmClient>, FactoryError> {
        #[cfg(not(feature = "openai"))]
        {
            let _ = registry;
            let _ = identity;
            Err(FactoryError::UnsupportedProvider(
                "self_hosted requires the openai feature".to_string(),
            ))
        }

        #[cfg(feature = "openai")]
        {
            let entry = registry.entry(&identity.model).ok_or_else(|| {
                FactoryError::ClientCreationFailed(format!("unknown model '{}'", identity.model))
            })?;
            let self_hosted = entry.self_hosted.as_ref().ok_or_else(|| {
                FactoryError::ClientCreationFailed(format!(
                    "model '{}' is not self-hosted",
                    identity.model
                ))
            })?;
            if let Some(expected_server_id) = identity.self_hosted_server_id.as_deref()
                && self_hosted.server_id != expected_server_id
            {
                return Err(FactoryError::ClientCreationFailed(format!(
                    "self-hosted model '{}' is bound to server '{}', but registry resolves to '{}'",
                    identity.model, expected_server_id, self_hosted.server_id
                )));
            }
            if self_hosted.transport != meerkat_core::SelfHostedTransport::OpenAiCompatible {
                return Err(FactoryError::UnsupportedProvider(
                    "only openai_compatible transport is supported".to_string(),
                ));
            }

            let mode = match self_hosted.api_style {
                meerkat_core::SelfHostedApiStyle::Responses => OpenAiCompatibleMode::Responses,
                meerkat_core::SelfHostedApiStyle::ChatCompletions => {
                    OpenAiCompatibleMode::ChatCompletions
                }
            };

            Ok(Arc::new(OpenAiCompatibleClient::new(
                mode,
                self_hosted.remote_model.clone(),
                self_hosted.base_url.clone(),
                self_hosted.resolve_bearer_token(),
                entry.profile.supports_temperature,
                entry.profile.supports_thinking,
                entry.profile.supports_reasoning,
            )))
        }
    }

    /// Wrap a session store in the shared adapter.
    pub async fn build_store_adapter<S: SessionStore + 'static>(
        &self,
        store: Arc<S>,
    ) -> StoreAdapter<S> {
        StoreAdapter::new(store)
    }

    /// Build a composite dispatcher so callers can register additional tools.
    #[cfg(not(target_arch = "wasm32"))]
    #[allow(clippy::too_many_arguments)]
    pub async fn build_composite_dispatcher(
        &self,
        store: Arc<dyn TaskStore>,
        config: &BuiltinToolConfig,
        project_root: Option<PathBuf>,
        shell_config: Option<ShellConfig>,
        external: Option<Arc<dyn AgentToolDispatcher>>,
        session_id: Option<String>,
        ops_lifecycle: Option<Arc<dyn OpsLifecycleRegistry>>,
        image_tool_results: bool,
    ) -> Result<CompositeDispatcher, CompositeDispatcherError> {
        CompositeDispatcher::new_with_ops_lifecycle(
            store,
            config,
            project_root,
            shell_config,
            external,
            session_id,
            ops_lifecycle,
            image_tool_results,
        )
    }

    /// Build a shared builtin dispatcher using the provided config.
    #[cfg(not(target_arch = "wasm32"))]
    #[allow(clippy::too_many_arguments)]
    pub async fn build_builtin_dispatcher(
        &self,
        store: Arc<dyn TaskStore>,
        config: BuiltinToolConfig,
        project_root: Option<PathBuf>,
        shell_config: Option<ShellConfig>,
        external: Option<Arc<dyn AgentToolDispatcher>>,
        session_id: Option<String>,
        ops_lifecycle: Option<Arc<dyn OpsLifecycleRegistry>>,
    ) -> Result<Arc<dyn AgentToolDispatcher>, CompositeDispatcherError> {
        self.build_builtin_dispatcher_with_skills(
            store,
            config,
            project_root,
            shell_config,
            external,
            session_id,
            ops_lifecycle,
            None,
        )
        .await
    }

    /// Build a shared builtin dispatcher, optionally including skill tools.
    #[cfg(not(target_arch = "wasm32"))]
    #[allow(clippy::too_many_arguments)]
    pub async fn build_builtin_dispatcher_with_skills(
        &self,
        store: Arc<dyn TaskStore>,
        config: BuiltinToolConfig,
        project_root: Option<PathBuf>,
        shell_config: Option<ShellConfig>,
        external: Option<Arc<dyn AgentToolDispatcher>>,
        session_id: Option<String>,
        ops_lifecycle: Option<Arc<dyn OpsLifecycleRegistry>>,
        #[cfg_attr(not(feature = "skills"), allow(unused_variables))] skill_engine: Option<
            Arc<meerkat_core::skills::SkillRuntime>,
        >,
    ) -> Result<Arc<dyn AgentToolDispatcher>, CompositeDispatcherError> {
        self.build_builtin_dispatcher_with_skills_internal(
            store,
            config,
            project_root,
            shell_config,
            external,
            session_id,
            ops_lifecycle,
            skill_engine,
            // Public API defaults to true (all tools visible).
            true,
        )
        .await
    }

    /// Internal dispatcher builder used by `build_agent`.
    #[cfg(not(target_arch = "wasm32"))]
    #[allow(clippy::too_many_arguments)]
    async fn build_builtin_dispatcher_with_skills_internal(
        &self,
        store: Arc<dyn TaskStore>,
        config: BuiltinToolConfig,
        project_root: Option<PathBuf>,
        shell_config: Option<ShellConfig>,
        external: Option<Arc<dyn AgentToolDispatcher>>,
        session_id: Option<String>,
        ops_lifecycle: Option<Arc<dyn OpsLifecycleRegistry>>,
        #[cfg_attr(not(feature = "skills"), allow(unused_variables))] skill_engine: Option<
            Arc<meerkat_core::skills::SkillRuntime>,
        >,
        image_tool_results: bool,
    ) -> Result<Arc<dyn AgentToolDispatcher>, CompositeDispatcherError> {
        let BuiltinDispatcherConfig {
            store,
            config,
            project_root,
            shell_config,
            external,
            session_id,
            ops_lifecycle,
            image_tool_results,
        } = BuiltinDispatcherConfig {
            store,
            config,
            project_root,
            shell_config,
            external,
            session_id,
            ops_lifecycle,
            image_tool_results,
        };

        #[cfg_attr(not(feature = "skills"), allow(unused_mut))]
        let mut composite = self
            .build_composite_dispatcher(
                store,
                &config,
                project_root,
                shell_config,
                external,
                session_id,
                ops_lifecycle,
                image_tool_results,
            )
            .await?;

        #[cfg(feature = "skills")]
        if let Some(engine) = skill_engine {
            composite
                .register_skill_tools(meerkat_tools::builtin::skills::SkillToolSet::new(engine));
        }

        Ok(Arc::new(composite))
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
        build_config.resume_override_mask.override_builtins |= !matches!(
            build_config.override_builtins,
            ToolCategoryOverride::Inherit
        );
        build_config.resume_override_mask.override_shell |=
            !matches!(build_config.override_shell, ToolCategoryOverride::Inherit);
        build_config.resume_override_mask.override_memory |=
            !matches!(build_config.override_memory, ToolCategoryOverride::Inherit);
        build_config.resume_override_mask.override_mob |=
            !matches!(build_config.override_mob, ToolCategoryOverride::Inherit);

        let explicit_mob_override =
            !matches!(build_config.override_mob, ToolCategoryOverride::Inherit);
        let resumed_session_metadata = Self::apply_resumed_session_metadata(&mut build_config);

        // Explicit build-time mob enablement should surface the generated
        // create-only authority shape when no typed authority was already
        // supplied or recovered. Ambient factory defaults must not do this,
        // and resumed metadata alone must not escalate operator capability.
        if build_config.mob_tool_authority_context.is_none()
            && matches!(build_config.override_mob, ToolCategoryOverride::Enable)
            && (build_config.resume_session.is_none() || explicit_mob_override)
        {
            build_config
                .apply_generated_create_only_mob_operator_access(ToolCategoryOverride::Enable);
        }

        if let Some(value) = build_config.max_inline_peer_notifications
            && value < -1
        {
            return Err(BuildAgentError::Config(format!(
                "max_inline_peer_notifications={value} is invalid (allowed: -1, 0, or >0)"
            )));
        }

        // 1. Validate keep_alive
        #[cfg(feature = "comms")]
        if build_config.keep_alive && build_config.comms_name.is_none() {
            return Err(BuildAgentError::KeepAliveRequiresCommsName);
        }

        let registry = self.model_registry(config)?;

        let explicit_meerkat_tool_policy =
            !matches!(
                build_config.override_builtins,
                ToolCategoryOverride::Inherit
            ) || !matches!(build_config.override_shell, ToolCategoryOverride::Inherit)
                || !matches!(build_config.override_memory, ToolCategoryOverride::Inherit)
                || !matches!(
                    build_config.override_schedule,
                    ToolCategoryOverride::Inherit
                )
                || !matches!(build_config.override_mob, ToolCategoryOverride::Inherit);

        // 2. Resolve provider and any self-hosted server binding.
        let resumed_self_hosted_server_id = resumed_session_metadata
            .as_ref()
            .and_then(|metadata| metadata.self_hosted_server_id.clone());
        let (provider, resolved_self_hosted_server_id) =
            self.resolve_provider_from_registry(&registry, &build_config)?;
        let self_hosted_server_id = if matches!(provider, Provider::SelfHosted) {
            build_config
                .self_hosted_server_id
                .clone()
                .or(resumed_self_hosted_server_id)
                .or(resolved_self_hosted_server_id)
        } else {
            None
        };

        // 3. Create LLM client.
        let llm_client: Arc<dyn LlmClient> = match build_config.llm_client_override.as_ref() {
            Some(client) => Arc::clone(client),
            None if std::env::var("RKAT_TEST_CLIENT").ok().as_deref() == Some("1") => {
                // Test shim: when RKAT_TEST_CLIENT=1 is set by integration
                // tests, short-circuit to an in-process TestClient so tests
                // don't need real provider credentials.
                Arc::new(meerkat_client::TestClient::default())
            }
            None => {
                // Self-hosted routes through the legacy path until it's
                // folded into the backend-kind taxonomy.
                if matches!(provider, Provider::SelfHosted) && build_config.connection_ref.is_none()
                {
                    self.build_self_hosted_client_from_registry(
                        &registry,
                        &SessionLlmIdentity {
                            model: build_config.model.clone(),
                            provider,
                            self_hosted_server_id: self_hosted_server_id.clone(),
                            provider_params: build_config.provider_params.clone(),
                            connection_ref: None,
                        },
                    )
                    .map_err(BuildAgentError::LlmClient)?
                } else {
                    // Resolve the target realm: explicit from connection_ref,
                    // or synthesized from env vars for a default binding.
                    let (realm, binding_id): (meerkat_core::RealmConnectionSet, String) =
                        match &build_config.connection_ref {
                            Some(conn_ref) => {
                                let section =
                                    config.realm.get(&conn_ref.realm_id).ok_or_else(|| {
                                        BuildAgentError::ConnectionResolution(format!(
                                            "realm '{}' not found in config.realm",
                                            conn_ref.realm_id
                                        ))
                                    })?;
                                let realm = meerkat_core::RealmConnectionSet::from_config(
                                    &conn_ref.realm_id,
                                    section,
                                )
                                .map_err(|e| {
                                    BuildAgentError::ConnectionResolution(e.to_string())
                                })?;
                                (realm, conn_ref.binding_id.clone())
                            }
                        };

                    // Provider-runtime registry needs the OAuth-backed
                    // TokenStore attached so persisted tokens (written by
                    // `rkat auth login`, server-side OAuth completion, etc.)
                    // are read during resolve_binding.
                    #[allow(unused_mut)]
                    let mut env = meerkat_providers::ResolverEnvironment::with_process_env();
                    #[cfg(not(target_arch = "wasm32"))]
                    {
                        if let Some(store) = self.token_store.clone() {
                            env = env.with_token_store(store);
                        }
                        if let Some(coord) = self.refresh_coord.clone() {
                            env = env.with_refresh_coordinator(coord);
                        }
                    }
                    for (handle, resolver) in &self.external_auth_resolvers {
                        env = env.with_external_resolver(handle.clone(), resolver.clone());
                    }
                    let provider_registry = crate::default_provider_registry();
                    let connection = provider_registry
                        .resolve(&realm, &binding_id, &env)
                        .await
                        .map_err(|e| BuildAgentError::ConnectionResolution(e.to_string()))?;

                    // Phase 1.5-rev loop closure — refresh-loop middle:
                    // The DSL tracks per-binding auth lifecycle state.
                    // On successful resolve, record the lease's expiry
                    // in the DSL so the runner's CallingLlm arm can
                    // observe `valid` / `expiring` / `reauth_required`
                    // states for this binding. Connects the resolver
                    // side to the state the agent runner consults.
                    if let RuntimeBuildMode::SessionOwned(bindings) =
                        &build_config.runtime_build_mode
                    {
                        let binding_key = format!("{}:{}", realm.realm_id, binding_id);
                        let expires_at = connection
                            .auth_lease
                            .expires_at()
                            .map(|ts| ts.timestamp().max(0) as u64)
                            .unwrap_or(u64::MAX);
                        // Ignore result: the DSL may reject if an earlier
                        // hot-swap already transitioned this binding; the
                        // lease state is orthogonal to error semantics.
                        let _ = bindings.auth_lease.acquire_lease(&binding_key, expires_at);
                    }

                    // Realtime-capable OpenAI models (e.g. gpt-realtime-1.5)
                    // cannot go through the Responses API — POST /v1/responses
                    // returns 404 model_not_found. Route those through the
                    // OpenAI Realtime WebSocket via `OpenAiRealtimeTextAdapter`.
                    // Capability-driven routing owns this decision at the
                    // composition seam (dogma §9).
                    let realtime_route = matches!(provider, Provider::OpenAI)
                        && is_openai_realtime_capable(&build_config.model);
                    #[cfg(not(feature = "openai-realtime"))]
                    if realtime_route {
                        return Err(BuildAgentError::ConnectionResolution(format!(
                            "model '{}' advertises ModelCapabilities.realtime=true; \
                             the meerkat facade must be built with the \
                             `openai-realtime` feature to route text turns over the \
                             Realtime WebSocket",
                            build_config.model
                        )));
                    }
                    #[cfg(feature = "openai-realtime")]
                    if realtime_route {
                        let secret = connection.resolved_secret().ok_or_else(|| {
                            BuildAgentError::ConnectionResolution(
                                "openai realtime text adapter requires an inline API key (\
                                 dynamic authorizers for realtime WS are not yet supported)"
                                    .to_string(),
                            )
                        })?;
                        Arc::new(meerkat_openai::OpenAiRealtimeTextAdapter::new(secret))
                            as Arc<dyn LlmClient>
                    } else {
                        provider_registry
                            .build_client(connection)
                            .map_err(|e| BuildAgentError::ConnectionResolution(e.to_string()))?
                    }
                    #[cfg(not(feature = "openai-realtime"))]
                    {
                        provider_registry
                            .build_client(connection)
                            .map_err(|e| BuildAgentError::ConnectionResolution(e.to_string()))?
                    }
                }
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
        let _conventions_context_root = self
            .context_root
            .as_deref()
            .or(self.project_root.as_deref());
        let _conventions_user_root = self.user_config_root.as_deref();

        // 6a. Build skill engine (override > factory > config > filesystem).
        #[cfg(feature = "skills")]
        let skill_engine: Option<Arc<meerkat_core::skills::SkillRuntime>> = if let Some(engine) =
            build_config.skill_engine_override.take()
        {
            Some(engine)
        } else {
            let skill_source: Option<Arc<meerkat_skills::CompositeSkillSource>> =
                if self.skill_source.is_some() {
                    self.skill_source.clone()
                } else if !config.skills.enabled {
                    None
                } else {
                    #[cfg(not(target_arch = "wasm32"))]
                    {
                        match meerkat_skills::resolve_repositories_with_roots(
                            &config.skills,
                            _conventions_context_root,
                            _conventions_user_root,
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
                    }
                    #[cfg(target_arch = "wasm32")]
                    None
                };

            skill_source.map(|source| {
                let available_caps = self.effective_skill_capabilities(config, Some(&build_config));
                let engine = Arc::new(
                    meerkat_skills::DefaultSkillEngine::new(source, available_caps)
                        .with_inventory_threshold(config.skills.inventory_threshold)
                        .with_max_injection_bytes(config.skills.max_injection_bytes),
                );
                Arc::new(meerkat_core::skills::SkillRuntime::new(engine))
            })
        }; // end else (filesystem resolution fallthrough)
        #[cfg(not(feature = "skills"))]
        let skill_engine: Option<Arc<meerkat_core::skills::SkillRuntime>> = None;

        // 6b. Build tool dispatcher (with optional external tools, per-build overrides, skill tools)
        let persisted_system_prompt = build_config.system_prompt.clone();
        let per_request_prompt = build_config.system_prompt.take();
        let effective_builtins = build_config.override_builtins.resolve(self.enable_builtins);
        #[allow(unused_variables)] // only consumed by non-wasm32 tool dispatcher
        let effective_shell = build_config.override_shell.resolve(self.enable_shell);
        let mut session = build_config.resume_session.clone().unwrap_or_default();
        // Inject pre-resolved metadata entries (e.g. inherited tool filter from
        // spawn tooling) before the builder reads metadata for early-stage recovery.
        for (key, value) in &build_config.initial_metadata_entries {
            session.set_metadata(key, value.clone());
        }
        let _session_id = session.id().to_string();
        // 6b. Create comms runtime before tool wiring.
        // If the factory has a pre-built runtime (surface with stable identity),
        // use it directly. Otherwise create a per-session runtime from config.
        #[cfg(all(feature = "comms", not(target_arch = "wasm32")))]
        let comms_runtime = if let Some(ref shared) = self.comms_runtime
            && build_config.comms_name.is_none()
        {
            // Use the factory's shared runtime only when no per-session comms_name
            // is requested. Mob-spawned members set comms_name and need their own
            // per-session identity — sharing the parent's runtime would make all
            // members route to the same inbox and break peer-to-peer messaging.
            Some(Arc::clone(shared))
        } else if build_config.keep_alive || build_config.comms_name.is_some() {
            let comms_name = build_config
                .comms_name
                .as_ref()
                .ok_or(BuildAgentError::KeepAliveRequiresCommsName)?;
            let silent_intents = Arc::new(
                build_config
                    .silent_comms_intents
                    .iter()
                    .cloned()
                    .collect::<std::collections::HashSet<String>>(),
            );
            // Source the canonical session-claim handle from the build's
            // runtime bindings when present; otherwise fall back to the
            // process-global default registry so bare facade callers still
            // hit a typed canonical owner (dogma #2 — "this session id is
            // in use" never lives in process-global shell statics).
            let session_claim_handle: Arc<dyn meerkat_core::handles::SessionClaimHandle> =
                match &build_config.runtime_build_mode {
                    meerkat_core::RuntimeBuildMode::SessionOwned(bindings) => {
                        Arc::clone(&bindings.session_claim_handle)
                    }
                    meerkat_core::RuntimeBuildMode::StandaloneEphemeral => {
                        meerkat_core::handles::DefaultSessionClaimRegistry::global()
                            as Arc<dyn meerkat_core::handles::SessionClaimHandle>
                    }
                };
            let mut runtime =
                crate::build_session_scoped_comms_runtime_from_config_scoped_with_silent_intents(
                    config,
                    _realm_scope_root.as_path(),
                    self.user_config_root.as_deref(),
                    comms_name,
                    build_config.peer_meta.clone(),
                    // Realm ID is the comms inproc namespace boundary.
                    build_config.realm_id.clone(),
                    session.id(),
                    silent_intents,
                    session_claim_handle,
                )
                .await
                .map_err(BuildAgentError::Comms)?;
            if let Some(blob_store) = build_config.blob_store_override.clone() {
                runtime.set_blob_store(blob_store);
            }
            Some(Arc::new(runtime))
        } else {
            None
        };
        #[cfg(all(feature = "comms", target_arch = "wasm32"))]
        let comms_runtime = if let Some(ref shared) = self.comms_runtime
            && build_config.comms_name.is_none()
        {
            Some(Arc::clone(shared))
        } else if build_config.keep_alive || build_config.comms_name.is_some() {
            let comms_name = build_config
                .comms_name
                .as_ref()
                .ok_or(BuildAgentError::KeepAliveRequiresCommsName)?;
            let silent_intents = Arc::new(
                build_config
                    .silent_comms_intents
                    .iter()
                    .cloned()
                    .collect::<std::collections::HashSet<String>>(),
            );
            let mut runtime = meerkat_comms::CommsRuntime::inproc_only_with_silent_intents(
                comms_name,
                build_config.realm_id.clone(),
                silent_intents,
            )
            .map_err(|e| BuildAgentError::Comms(e.to_string()))?;
            if let Some(ref meta) = build_config.peer_meta {
                runtime.set_peer_meta(meta.clone());
            }
            if let Some(blob_store) = build_config.blob_store_override.clone() {
                runtime.set_blob_store(blob_store);
            }
            Some(Arc::new(runtime))
        } else {
            None
        };
        #[cfg(not(feature = "comms"))]
        #[allow(clippy::no_effect_underscore_binding)]
        let _comms_runtime: Option<()> = None;

        // Resolve model profile for capability gating and runtime defaults.
        let model_profile = registry.profile_for(&model);
        let _image_tool_results = model_profile.as_ref().is_none_or(|p| p.image_tool_results);

        if let Some(profile) = model_profile.as_ref() {
            let has_canonical_visibility_state = session
                .metadata()
                .contains_key(meerkat_core::SESSION_TOOL_VISIBILITY_STATE_KEY);
            let capability_base_filter =
                meerkat_core::capability_base_filter_for_image_tool_results(
                    profile.image_tool_results,
                );
            let mut visibility_state = session.tool_visibility_state().unwrap_or_default();
            if visibility_state.capability_base_filter != capability_base_filter
                || has_canonical_visibility_state
            {
                visibility_state.capability_base_filter = capability_base_filter;
                session
                    .set_tool_visibility_state(visibility_state)
                    .map_err(|err| BuildAgentError::Config(err.to_string()))?;
            }
        }
        // Resolve ops lifecycle registry via RuntimeBuildMode.
        use meerkat_core::runtime_epoch::RuntimeBuildMode;

        let resolved_mode = &build_config.runtime_build_mode;

        #[allow(unused_variables)]
        let (ops_lifecycle, concrete_ops_lifecycle): (
            Arc<dyn OpsLifecycleRegistry>,
            Option<Arc<RuntimeOpsLifecycleRegistry>>,
        ) = match &resolved_mode {
            RuntimeBuildMode::SessionOwned(bindings) => {
                if bindings.session_id != *session.id() {
                    return Err(BuildAgentError::Config(format!(
                        "SessionRuntimeBindings.session_id ({}) does not match session ({}); \
                         bindings may have been prepared for a different session",
                        bindings.session_id,
                        session.id(),
                    )));
                }
                (Arc::clone(&bindings.ops_lifecycle), None)
            }
            RuntimeBuildMode::StandaloneEphemeral => {
                let concrete = Arc::new(RuntimeOpsLifecycleRegistry::new());
                (
                    Arc::clone(&concrete) as Arc<dyn OpsLifecycleRegistry>,
                    Some(concrete),
                )
            }
        };

        // Create the completion feed for cursor-based completion delivery.
        // The feed is obtained from the ops lifecycle registry and consumed
        // directly by the agent boundary for background completion notices.
        let completion_feed = ops_lifecycle.completion_feed();

        // Build the tool dispatcher. Wait-tool-specific binding was removed
        // along with the generic wait tool.
        #[allow(unused_mut)]
        let (mut tools, mut tool_usage_instructions) =
            if let Some(dispatcher) = build_config.tool_dispatcher_override.take() {
                let usage = render_tool_usage_instructions(dispatcher.as_ref());
                (dispatcher, usage)
            } else {
                #[cfg(not(target_arch = "wasm32"))]
                {
                    self.build_tool_dispatcher_for_agent_with_overrides(
                        config,
                        build_config.external_tools,
                        effective_builtins,
                        effective_shell,
                        skill_engine.clone(),
                        build_config.shell_env.clone(),
                        _session_id.clone(),
                        Arc::clone(&ops_lifecycle),
                        _image_tool_results,
                    )
                    .await?
                }
                #[cfg(target_arch = "wasm32")]
                {
                    // Fallback: empty tool dispatcher when no override is set on wasm32.
                    let usage = String::new();
                    (
                        Arc::new(EmptyToolDispatcher) as Arc<dyn AgentToolDispatcher>,
                        usage,
                    )
                }
            };

        tracing::debug!(
            base_tool_count = tools.tools().len(),
            effective_builtins,
            effective_shell,
            "tool composition: base dispatcher built"
        );

        // Bind the session's MCP server lifecycle DSL handle into the tool
        // dispatcher before any composition. Dispatchers that manage per-server
        // MCP handshakes (like `McpRouterAdapter`) use this handle to mirror
        // connection state into MeerkatMachine's `mcp_server_states`. Others
        // are no-ops.
        if let RuntimeBuildMode::SessionOwned(bindings) = resolved_mode {
            tools.bind_mcp_server_lifecycle_handle(Arc::clone(&bindings.mcp_server_lifecycle));
            // W1-A: install the peer-interaction DSL handle on the session's
            // comms runtime so outbound PeerRequest sends record into
            // `pending_peer_requests` and `comms_drain` fires response
            // progress / terminal transitions.
            #[cfg(feature = "comms")]
            if let (Some(runtime), Some(handle)) =
                (&comms_runtime, bindings.peer_interaction.as_ref())
            {
                runtime.install_peer_interaction_handle(Arc::clone(handle));
            }
            // U6 (dogma #5): install the interaction-stream DSL handle so the
            // shell-side `interaction_stream_registry` becomes a pure
            // projection of DSL truth — `Reserved` / `Attached` / `Completed`
            // / `Expired` / `ClosedEarly` lifecycle and the cleanup observer
            // both flow through the DSL.
            #[cfg(feature = "comms")]
            if let (Some(runtime), Some(handle)) =
                (&comms_runtime, bindings.interaction_stream.as_ref())
            {
                runtime.install_interaction_stream_handle(Arc::clone(handle));
            }
        }

        // 7. Create session store adapter (override > factory > feature-flag default)
        let store_adapter: Arc<dyn AgentSessionStore> =
            if let Some(store) = build_config.session_store_override.take() {
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

        // 9a. Compose tools with comms gateway.
        #[cfg(feature = "comms")]
        if let Some(ref runtime) = comms_runtime {
            let composed =
                compose_tools_with_comms(tools, tool_usage_instructions, runtime.tool_material())
                    .map_err(|e| {
                    BuildAgentError::Config(format!("Failed to compose comms tools: {e}"))
                })?;
            tools = composed.0;
            tool_usage_instructions = composed.1;
        }

        tracing::debug!(
            tool_count_after_comms = tools.tools().len(),
            "tool composition: after comms gateway"
        );

        // 9b. Compose tools with scheduler surface (after comms, before mob).
        let effective_schedule = build_config.override_schedule.resolve(self.enable_schedule);
        if effective_schedule && let Some(schedule_dispatcher) = build_config.schedule_tools.take()
        {
            let schedule_usage = render_tool_usage_instructions(schedule_dispatcher.as_ref());
            tools = Arc::new(meerkat_core::DynamicToolComposite::new(vec![
                tools,
                schedule_dispatcher,
            ]));
            if !schedule_usage.is_empty() {
                if !tool_usage_instructions.is_empty() {
                    tool_usage_instructions.push_str("\n\n");
                }
                tool_usage_instructions.push_str(&schedule_usage);
            }
        }

        tracing::debug!(
            tool_count_after_schedule = tools.tools().len(),
            effective_schedule,
            "tool composition: after scheduler gateway"
        );

        // 9c. Compose tools with mob surface (after comms and scheduler, so mob
        // gateway wraps the already-composed base capability stack).
        let effective_mob = build_config.override_mob.resolve(self.enable_mob)
            || build_config.mob_tool_authority_context.is_some();
        let mob_factory = build_config
            .mob_tools
            .take()
            .or_else(|| self.mob_tools.clone());
        // Shared mob authority handle — created inside the mob block, hoisted
        // out so the agent builder can reference it after the block.
        let mut hoisted_mob_authority_handle: Option<
            Arc<std::sync::RwLock<meerkat_core::service::MobToolAuthorityContext>>,
        > = None;
        // Hoisted so we can re-point the Weak reference after bind_ops_lifecycle.
        let mut hoisted_deferred_provider: Option<Arc<DeferredSnapshotProvider>> = None;
        if effective_mob && let Some(mob_factory) = mob_factory {
            // Build comms runtime arg: clone from the comms phase if available.
            #[cfg(feature = "comms")]
            let mob_comms: Option<Arc<dyn meerkat_core::agent::CommsRuntime>> = comms_runtime
                .as_ref()
                .map(|r| Arc::clone(r) as Arc<dyn meerkat_core::agent::CommsRuntime>);
            #[cfg(not(feature = "comms"))]
            let mob_comms: Option<Arc<dyn meerkat_core::agent::CommsRuntime>> = None;

            // Create the shared effective-authority handle. The agent owns the
            // write side (via apply_session_effects); mob tools get a read view.
            let mob_authority_handle = build_config
                .mob_tool_authority_context
                .as_ref()
                .map(|ctx| Arc::new(std::sync::RwLock::new(ctx.clone())));
            hoisted_mob_authority_handle = mob_authority_handle.clone();

            // Use a deferred snapshot provider: created before mob composition
            // so it can be passed into MobToolsBuildArgs, then updated with the
            // final composed dispatcher after mob tools are added. This ensures
            // InheritParent snapshots include mob/profile tools.
            let deferred_provider = Arc::new(DeferredSnapshotProvider::new());
            let snapshot_provider: Arc<dyn meerkat_core::service::VisibleToolSnapshotProvider> =
                Arc::clone(&deferred_provider)
                    as Arc<dyn meerkat_core::service::VisibleToolSnapshotProvider>;
            let mob_args = meerkat_core::service::MobToolsBuildArgs {
                session_id: session.id().clone(),
                model: model.clone(),
                authority_context: build_config.mob_tool_authority_context.clone(),
                effective_authority: mob_authority_handle.clone(),
                comms_name: build_config.comms_name.clone(),
                comms_runtime: mob_comms,
                snapshot_context: meerkat_core::service::MobToolSnapshotContext::ParentOwned(
                    snapshot_provider,
                ),
            };
            let mob_dispatcher = mob_factory
                .build_mob_tools(mob_args)
                .await
                .map_err(|e| BuildAgentError::Config(format!("Mob tool factory: {e}")))?;
            let mob_usage = render_tool_usage_instructions(mob_dispatcher.as_ref());
            // Use DynamicToolComposite (not ToolGateway) so dynamic child
            // dispatchers (e.g. callback tools) can surface additions between turns.
            tools = Arc::new(meerkat_core::DynamicToolComposite::new(vec![
                tools,
                mob_dispatcher,
            ]));
            // Set the final composed dispatcher on the deferred provider so
            // snapshots captured later include mob tools.
            deferred_provider.set_dispatcher(&tools);
            hoisted_deferred_provider = Some(deferred_provider);
            if !mob_usage.is_empty() {
                if !tool_usage_instructions.is_empty() {
                    tool_usage_instructions.push_str("\n\n");
                }
                tool_usage_instructions.push_str(&mob_usage);
            }
        }

        // 9d. Bind capabilities on the FINAL composed dispatcher shape.
        //
        // All composition (comms gateway, mob gateway) is complete.
        // Binding now happens once on the final shape. Gateway wrappers
        // forward bind_* calls to inner entries that support them, with
        // Arc::strong_count guards for shared overrides.
        if tools.capabilities().ops_lifecycle {
            let outcome = tools
                .bind_ops_lifecycle(Arc::clone(&ops_lifecycle), session.id().clone())
                .map_err(|e| {
                    BuildAgentError::Config(format!("Ops lifecycle binding failed: {e}"))
                })?;
            tools = outcome.into_dispatcher();
        }
        // Re-point the deferred snapshot provider after binding may have replaced
        // the dispatcher Arc (bind_ops_lifecycle can return a new Arc).
        if let Some(ref provider) = hoisted_deferred_provider {
            provider.set_dispatcher(&tools);
        }

        tracing::debug!(
            final_tool_count = tools.tools().len(),
            tool_names = %tools.tools().iter().map(|t| t.name.as_str()).collect::<Vec<_>>().join(", "),
            "tool composition: final dispatcher"
        );

        if CatalogControlDispatcher::should_compose_for(tools.as_ref()) {
            if !tool_usage_instructions.is_empty() {
                tool_usage_instructions.push_str("\n\n");
            }
            tool_usage_instructions.push_str(deferred_catalog_guidance());
        }

        // 10. Resolve hooks (override > filesystem layered config)
        #[allow(
            clippy::manual_map,
            clippy::unnecessary_literal_unwrap,
            clippy::needless_match
        )]
        let hook_engine = match build_config.hook_engine_override.take() {
            Some(engine) => Some(engine),
            None => {
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let layered_hooks = resolve_layered_hooks_config(
                        _conventions_context_root,
                        _conventions_user_root,
                        config,
                    )
                    .await;
                    create_default_hook_engine(layered_hooks)
                }
                #[cfg(target_arch = "wasm32")]
                {
                    None
                }
            }
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
                let mut preload = build_config
                    .preload_skills
                    .take()
                    .and_then(|ids| if ids.is_empty() { None } else { Some(ids) });

                // Resumed sessions may carry persisted skill IDs from an older
                // surface or older metadata semantics. Filter to the skills
                // currently available on this surface instead of failing the
                // rebuild outright on an incompatible preload.
                if build_config.resume_session.is_some()
                    && let Some(ids) = preload.as_mut()
                {
                    let available: std::collections::HashSet<_> = engine
                        .list_skills(&meerkat_core::skills::SkillFilter::default())
                        .await
                        .map(|descs| descs.into_iter().map(|desc| desc.id).collect())
                        .unwrap_or_default();
                    let mut dropped = Vec::new();
                    ids.retain(|id| {
                        let keep = available.contains(id);
                        if !keep {
                            dropped.push(id.0.clone());
                        }
                        keep
                    });
                    if !dropped.is_empty() {
                        tracing::warn!(
                            dropped_skills = ?dropped,
                            "dropping persisted active skills that are unavailable on the current surface"
                        );
                    }
                    if ids.is_empty() {
                        preload = None;
                    }
                }

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

                // Persist the skills explicitly activated for this session.
                let skill_ids = preload.clone();

                (inventory, preloaded_sections, skill_ids)
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
        // Append additional instructions after skills, before tool
        // instructions. These are canonical build-state and must survive into
        // persisted recovery / realtime reconstruction, so prompt assembly may
        // read them but must not consume them.
        let additional_instruction_storage: Vec<String> = build_config
            .additional_instructions
            .clone()
            .unwrap_or_default();
        for instruction in &additional_instruction_storage {
            if !instruction.is_empty() {
                extra_sections.push(instruction.as_str());
            }
        }
        let should_apply_system_prompt =
            build_config.resume_session.is_none() || per_request_prompt.is_some();
        #[cfg(not(target_arch = "wasm32"))]
        let system_prompt = if should_apply_system_prompt {
            Some(
                crate::assemble_system_prompt(
                    config,
                    per_request_prompt.as_deref(),
                    _conventions_context_root,
                    &extra_sections,
                    &tool_usage_instructions,
                )
                .await,
            )
        } else {
            None
        };
        #[cfg(target_arch = "wasm32")]
        let system_prompt = if should_apply_system_prompt {
            Some({
                // Precedence: per-request > config inline > default.
                // No AGENTS.md or system_prompt_file on wasm32 (no filesystem).
                let base = per_request_prompt
                    .or_else(|| config.agent.system_prompt.clone())
                    .unwrap_or_else(|| DEFAULT_WASM_SYSTEM_PROMPT.to_string());
                let mut prompt = base;
                for section in &extra_sections {
                    if !section.is_empty() {
                        prompt.push_str("\n\n");
                        prompt.push_str(section);
                    }
                }
                if let Some(ref config_tools) = config.agent.tool_instructions
                    && !config_tools.is_empty()
                {
                    prompt.push_str("\n\n");
                    prompt.push_str(config_tools);
                }
                if !tool_usage_instructions.is_empty() {
                    prompt.push_str("\n\n");
                    prompt.push_str(&tool_usage_instructions);
                }
                prompt
            })
        } else {
            None
        };

        // 11f. Wait for pending MCP connections when requested.
        //
        // poll_external_updates() is forwarded through the full dispatcher
        // chain (CompositeDispatcher → ToolGateway → McpRouterAdapter), so
        // this drains background MCP connection results regardless of how
        // many dispatchers are composed.
        if build_config.wait_for_mcp {
            let timeout = std::time::Duration::from_secs(60);
            let started = meerkat_core::time_compat::Instant::now();
            loop {
                let update = tools.poll_external_updates().await;
                if update.pending.is_empty() {
                    break;
                }
                if started.elapsed() >= timeout {
                    tracing::warn!(
                        "wait_for_mcp timed out after {}s with {} server(s) still pending",
                        timeout.as_secs(),
                        update.pending.len()
                    );
                    break;
                }
                #[cfg(not(target_arch = "wasm32"))]
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                #[cfg(target_arch = "wasm32")]
                tokio_with_wasm::alias::time::sleep(std::time::Duration::from_millis(500)).await;
            }
        }

        let persisted_build_state = meerkat_core::SessionBuildState {
            system_prompt: persisted_system_prompt,
            output_schema: build_config.output_schema.clone(),
            hooks_override: build_config.hooks_override.clone(),
            budget_limits: build_config.budget_limits.clone(),
            recoverable_tool_defs: build_config
                .recoverable_tool_defs
                .clone()
                .unwrap_or_default(),
            silent_comms_intents: build_config.silent_comms_intents.clone(),
            max_inline_peer_notifications: build_config.max_inline_peer_notifications,
            app_context: build_config.app_context.clone(),
            additional_instructions: build_config.additional_instructions.clone(),
            shell_env: build_config.shell_env.clone(),
            mob_tool_authority_context: build_config.mob_tool_authority_context.clone(),
            call_timeout_override: build_config.call_timeout_override.clone(),
        };

        // 12. Build AgentBuilder
        let budget_limits = build_config
            .budget_limits
            .unwrap_or_else(|| config.budget_limits());

        // 12a. Resolve effective call-timeout override: build > config > Inherit
        let effective_call_timeout_override = {
            let build_override = build_config.call_timeout_override;
            if build_override.is_inherit() {
                // Fall through to config-level override
                config.retry.call_timeout_override.clone()
            } else {
                build_override
            }
        };

        let mut builder = AgentBuilder::new()
            .model(model.clone())
            .max_tokens_per_turn(max_tokens)
            .budget(budget_limits)
            .structured_output_retries(build_config.structured_output_retries)
            .with_hook_run_overrides(build_config.hooks_override)
            .with_model_defaults_resolver(Arc::new(RegistryBackedDefaultsResolver {
                registry: Arc::new(registry.clone()),
            }))
            .with_call_timeout_override(effective_call_timeout_override);

        if let Some(params) = build_config.provider_params.clone() {
            builder = builder.provider_params(params);
        }
        if let Some(system_prompt) = system_prompt {
            builder = builder.system_prompt(system_prompt);
        }

        if let Some(schema) = build_config.output_schema {
            builder = builder.output_schema(schema);
        }
        let _is_resumed = build_config.resume_session.is_some();
        builder = builder.resume_session(session);
        #[cfg(feature = "comms")]
        if let Some(runtime) = comms_runtime {
            builder =
                builder.with_comms_runtime(runtime as Arc<dyn meerkat_core::agent::CommsRuntime>);
        }
        if let Some(engine) = hook_engine {
            builder = builder.with_hook_engine(engine);
        }

        // 12b. Wire memory store + memory_search tool (when feature compiled + enabled)
        #[allow(unused_variables)]
        let effective_memory = build_config.override_memory.resolve(self.enable_memory);
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

        // 12f. Wire silent comms intents
        if !build_config.silent_comms_intents.is_empty() {
            builder = builder.with_silent_comms_intents(build_config.silent_comms_intents);
        }
        builder =
            builder.with_max_inline_peer_notifications(build_config.max_inline_peer_notifications);

        // 12g. Wire session checkpointer for host-mode persistence
        if let Some(cp) = build_config.checkpointer {
            builder = builder.with_checkpointer(cp);
        }
        if let Some(blob_store) = build_config.blob_store_override {
            builder = builder.with_blob_store(blob_store);
        }
        builder = builder.with_ops_lifecycle(Arc::clone(&ops_lifecycle));
        if let RuntimeBuildMode::SessionOwned(bindings) = resolved_mode {
            builder = builder.with_epoch_cursor_state(Arc::clone(&bindings.cursor_state));
            builder =
                builder.with_tool_visibility_owner(Arc::clone(&bindings.tool_visibility_owner));
            builder = builder.with_turn_state_handle(Arc::clone(&bindings.turn_state));
            builder = builder
                .with_external_tool_surface_handle(Arc::clone(&bindings.external_tool_surface));
            builder = builder.with_auth_lease_handle(Arc::clone(&bindings.auth_lease));
            builder = builder
                .with_mcp_server_lifecycle_handle(Arc::clone(&bindings.mcp_server_lifecycle));
        }
        // 12h. Wire completion feed + enrichment for cursor-based delivery
        if let Some(feed) = completion_feed {
            builder = builder.with_completion_feed(feed);
        }
        // Extract enrichment provider from the tool dispatcher (if the dispatcher
        // has a shell job manager, it implements CompletionEnrichmentProvider).
        if let Some(enrichment) = tools.completion_enrichment() {
            builder = builder.with_completion_enrichment(enrichment);
        }

        let mut hoisted_control_visibility_provider: Option<Arc<CatalogControlVisibilityProvider>> =
            None;
        if CatalogControlDispatcher::should_compose_for(tools.as_ref()) {
            let visibility_provider = Arc::new(CatalogControlVisibilityProvider::new());
            let control_dispatcher = Arc::new(CatalogControlDispatcher::new(
                Arc::clone(&tools),
                Arc::clone(&visibility_provider),
            )) as Arc<dyn AgentToolDispatcher>;
            tools = Arc::new(meerkat_core::DynamicToolComposite::new(vec![
                tools,
                control_dispatcher,
            ]));
            hoisted_control_visibility_provider = Some(visibility_provider);
        }

        // 13. Build agent
        let mut agent = builder.build(llm_adapter, tools, store_adapter).await;

        if let Some(provider) = hoisted_control_visibility_provider {
            provider.set_scope(agent.tool_scope().clone());
        }

        // Wire mob authority handle into agent for session-effect application.
        if let Some(handle) = hoisted_mob_authority_handle {
            agent.set_mob_authority_handle(handle);
        }

        // 14. Set SessionMetadata
        //
        // Persist the *override intent* (Inherit/Enable/Disable), not the resolved
        // effective bool. This ensures Inherit survives across save/resume cycles so
        // the session continues to follow future runtime defaults.
        let metadata = if let Some(mut metadata) = resumed_session_metadata {
            metadata.model = model;
            metadata.max_tokens = max_tokens;
            metadata.structured_output_retries = build_config.structured_output_retries;
            metadata.provider = provider;
            metadata.self_hosted_server_id = self_hosted_server_id.clone();
            metadata.provider_params = build_config.provider_params;
            metadata.tooling.builtins = build_config.override_builtins;
            metadata.tooling.shell = build_config.override_shell;
            // No override_comms field in AgentBuildConfig — preserve the existing
            // metadata value so explicit Enable/Disable survives across resumes.
            // (metadata.tooling.comms is left unchanged)
            metadata.tooling.mob = build_config.override_mob;
            metadata.tooling.memory = build_config.override_memory;
            if build_config.resume_override_mask.preload_skills {
                metadata.tooling.active_skills = active_skill_ids;
            }
            metadata.keep_alive = build_config.keep_alive;
            metadata.comms_name = build_config.comms_name;
            metadata.peer_meta = build_config.peer_meta;
            metadata.realm_id = build_config.realm_id;
            metadata.instance_id = build_config.instance_id;
            metadata.backend = build_config.backend;
            metadata.config_generation = build_config.config_generation;
            metadata
        } else {
            SessionMetadata {
                schema_version: meerkat_core::SESSION_METADATA_SCHEMA_VERSION,
                model,
                max_tokens,
                structured_output_retries: build_config.structured_output_retries,
                provider,
                self_hosted_server_id,
                provider_params: build_config.provider_params,
                tooling: SessionTooling {
                    builtins: build_config.override_builtins,
                    shell: build_config.override_shell,
                    comms: ToolCategoryOverride::Inherit,
                    mob: build_config.override_mob,
                    memory: build_config.override_memory,
                    active_skills: active_skill_ids,
                },
                keep_alive: build_config.keep_alive,
                comms_name: build_config.comms_name,
                peer_meta: build_config.peer_meta,
                realm_id: build_config.realm_id,
                instance_id: build_config.instance_id,
                backend: build_config.backend,
                config_generation: build_config.config_generation,
                connection_ref: build_config.connection_ref.clone(),
            }
        };
        if let Err(err) = agent.session_mut().set_session_metadata(metadata) {
            tracing::warn!("Failed to store session metadata: {}", err);
        }
        if let Err(err) = agent.session_mut().set_build_state(persisted_build_state) {
            tracing::warn!("Failed to store session build state: {}", err);
        }

        Ok(agent)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use super::*;
    use meerkat_core::{
        SelfHostedApiStyle, SelfHostedModelConfig, SelfHostedServerConfig, SelfHostedTransport,
    };

    #[test]
    fn resumed_self_hosted_binding_overrides_current_registry_server() {
        let temp = tempfile::tempdir().unwrap();
        let factory = AgentFactory::new(temp.path().join("sessions"));
        let mut config = Config::default();
        config.self_hosted.servers.insert(
            "local".to_string(),
            SelfHostedServerConfig {
                transport: SelfHostedTransport::OpenAiCompatible,
                base_url: "http://127.0.0.1:11434".to_string(),
                api_style: SelfHostedApiStyle::ChatCompletions,
                bearer_token: None,
                bearer_token_env: None,
            },
        );
        config.self_hosted.models.insert(
            "gemma-4-e2b".to_string(),
            SelfHostedModelConfig {
                server: "local".to_string(),
                remote_model: "gemma4:e2b".to_string(),
                display_name: "Gemma 4 E2B".to_string(),
                family: "gemma-4".to_string(),
                tier: meerkat_models::ModelTier::Supported,
                context_window: Some(128_000),
                max_output_tokens: Some(8_192),
                vision: true,
                image_tool_results: true,
                inline_video: false,
                supports_temperature: true,
                supports_thinking: true,
                supports_reasoning: true,
                supports_web_search: false,
                call_timeout_secs: Some(600),
            },
        );

        let mut resumed = Session::new();
        resumed
            .set_session_metadata(SessionMetadata {
                schema_version: meerkat_core::SESSION_METADATA_SCHEMA_VERSION,
                model: "gemma-4-e2b".to_string(),
                max_tokens: 8_192,
                structured_output_retries: 2,
                provider: Provider::SelfHosted,
                self_hosted_server_id: Some("other".to_string()),
                provider_params: None,
                tooling: SessionTooling::default(),
                keep_alive: false,
                comms_name: None,
                peer_meta: None,
                realm_id: None,
                instance_id: None,
                backend: None,
                config_generation: None,
                connection_ref: None,
            })
            .expect("resume metadata");

        let mut build = AgentBuildConfig::new("gemma-4-e2b");
        build.resume_session = Some(resumed);
        let _ = AgentFactory::apply_resumed_session_metadata(&mut build);

        let registry = factory.model_registry(&config).expect("registry");
        let (provider, server_id) = factory
            .resolve_provider_from_registry(&registry, &build)
            .expect("resolved provider");

        assert_eq!(provider, Provider::SelfHosted);
        assert_eq!(server_id.as_deref(), Some("other"));
    }

    #[test]
    fn shell_project_root_uses_store_parent_when_project_root_is_unset() {
        let temp = tempfile::tempdir().unwrap();
        let store_path = temp.path().join("sessions");
        let factory = AgentFactory::new(store_path);

        assert_eq!(factory.shell_project_root(), temp.path());
    }

    #[test]
    fn shell_project_root_prefers_explicit_project_root() {
        let temp = tempfile::tempdir().unwrap();
        let explicit_root = temp.path().join("workspace");
        let factory = AgentFactory::new(temp.path().join("sessions")).project_root(&explicit_root);

        assert_eq!(factory.shell_project_root(), explicit_root);
    }
}

impl AgentFactory {
    /// Build the tool dispatcher and usage instructions.
    ///
    /// `effective_builtins` and `effective_shell` override the factory-level
    /// flags for this specific build.
    #[cfg(not(target_arch = "wasm32"))]
    #[allow(clippy::too_many_arguments)]
    async fn build_tool_dispatcher_for_agent_with_overrides(
        &self,
        _config: &Config,
        external: Option<Arc<dyn AgentToolDispatcher>>,
        effective_builtins: bool,
        effective_shell: bool,
        skill_engine: Option<Arc<meerkat_core::skills::SkillRuntime>>,
        shell_env: Option<std::collections::HashMap<String, String>>,
        session_id: String,
        ops_lifecycle: Arc<dyn OpsLifecycleRegistry>,
        image_tool_results: bool,
    ) -> Result<(Arc<dyn AgentToolDispatcher>, String), BuildAgentError> {
        if !effective_builtins {
            // No builtins — return the external tools if provided, otherwise empty.
            return match external {
                Some(ext) => {
                    let usage = render_tool_usage_instructions(ext.as_ref());
                    Ok((ext, usage))
                }
                None => Ok((Arc::new(EmptyToolDispatcher), String::new())),
            };
        }

        // Create a task store.
        // With session-store: SQLite-backed, scoped to the session so /resume
        // restores the correct task set.
        // Without: file-backed (project root) or in-memory fallback.
        #[cfg(feature = "session-store")]
        let task_store: Arc<dyn TaskStore> = Arc::new(SqliteTaskStore::for_session(
            self.store_path.join("tasks.db"),
            &session_id,
        ));
        #[cfg(not(feature = "session-store"))]
        let task_store: Arc<dyn TaskStore> = match self.project_root.as_ref() {
            Some(root) => Arc::new(FileTaskStore::in_project(root)),
            None => Arc::new(MemoryTaskStore::new()),
        };

        // Create shell config if shell is enabled
        let shell_config = if effective_shell {
            let project_root = self.shell_project_root();
            let mut config = ShellConfig::with_project_root(project_root);
            if let Some(env) = shell_env {
                config.env_vars = env;
            }
            Some(config)
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

        let dispatcher = self
            .build_builtin_dispatcher_with_skills_internal(
                task_store,
                builtin_config,
                self.project_root.clone(),
                shell_config,
                external,
                Some(session_id),
                Some(ops_lifecycle),
                skill_engine,
                image_tool_results,
            )
            .await?;

        let usage = render_tool_usage_instructions(dispatcher.as_ref());
        Ok((dispatcher, usage))
    }
}

fn render_tool_usage_instructions(dispatcher: &dyn AgentToolDispatcher) -> String {
    if CatalogControlDispatcher::should_enable_for(dispatcher) {
        return String::new();
    }

    let tools = dispatcher.tools();
    if tools.is_empty() {
        return String::new();
    }
    let mut out = String::from("# Available Tools\n\n");
    for tool in tools.iter() {
        out.push_str(&format!("## {}\n{}\n\n", tool.name, tool.description));
    }
    out
}

fn deferred_catalog_guidance() -> &'static str {
    "Additional tools may be available in a deferred catalog. Use `tool_catalog_search` to discover deferred tools and `tool_catalog_load` to stage the ones you need."
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod prompt_tests {
    use super::{deferred_catalog_guidance, render_tool_usage_instructions};
    use async_trait::async_trait;
    use futures::stream;
    use meerkat_client::{LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest};
    use meerkat_core::error::ToolError;
    use meerkat_core::ops::ToolDispatchOutcome;
    use meerkat_core::types::{StopReason, ToolCallView, ToolDef, ToolResult};
    use meerkat_core::{
        AgentToolDispatcher, Config, Message, ToolCatalogCapabilities,
        ToolCatalogDeferredEligibility, ToolCatalogEntry, ToolCategoryOverride, ToolPlaneClass,
    };
    use std::pin::Pin;
    use std::sync::Arc;

    use crate::{AgentBuildConfig, AgentFactory};

    struct UsageTestDispatcher {
        tools: Arc<[Arc<ToolDef>]>,
        exact_catalog: bool,
        may_require_control_plane: bool,
        pending_sources: Arc<[String]>,
    }

    #[async_trait]
    impl AgentToolDispatcher for UsageTestDispatcher {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            Arc::clone(&self.tools)
        }

        fn tool_catalog_capabilities(&self) -> ToolCatalogCapabilities {
            ToolCatalogCapabilities {
                exact_catalog: self.exact_catalog,
                may_require_catalog_control_plane: self.may_require_control_plane,
            }
        }

        fn tool_catalog(&self) -> Arc<[ToolCatalogEntry]> {
            self.tools
                .iter()
                .map(|tool| {
                    if tool.name == "tool_catalog_search" {
                        ToolCatalogEntry::control_inline(Arc::clone(tool), true)
                    } else if tool.name.starts_with("secret_") {
                        ToolCatalogEntry::session_deferred(
                            Arc::clone(tool),
                            true,
                            "callback:registered".to_string(),
                        )
                    } else {
                        ToolCatalogEntry {
                            tool: Arc::clone(tool),
                            plane: ToolPlaneClass::Session,
                            currently_callable: true,
                            deferred_eligibility: ToolCatalogDeferredEligibility::InlineOnly,
                        }
                    }
                })
                .collect::<Vec<_>>()
                .into()
        }

        fn pending_catalog_sources(&self) -> Arc<[String]> {
            Arc::clone(&self.pending_sources)
        }

        async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
            Ok(ToolResult::new(call.id.to_string(), call.name.to_string(), false).into())
        }
    }

    struct PromptTestClient;

    #[async_trait]
    impl LlmClient for PromptTestClient {
        fn stream<'a>(
            &'a self,
            _request: &'a LlmRequest,
        ) -> Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
            Box::pin(stream::iter(vec![Ok(LlmEvent::Done {
                outcome: LlmDoneOutcome::Success {
                    stop_reason: StopReason::EndTurn,
                },
            })]))
        }

        fn provider(&self) -> &'static str {
            "mock"
        }

        async fn health_check(&self) -> Result<(), LlmError> {
            Ok(())
        }
    }

    fn tools(names: &[&str]) -> Arc<[Arc<ToolDef>]> {
        names
            .iter()
            .map(|name| {
                Arc::new(ToolDef {
                    name: (*name).to_string(),
                    description: format!("{name} tool"),
                    input_schema: serde_json::json!({ "type": "object" }),
                    provenance: None,
                })
            })
            .collect::<Vec<_>>()
            .into()
    }

    #[test]
    fn render_tool_usage_instructions_keeps_inventory_for_non_exact_dispatchers() {
        let dispatcher = UsageTestDispatcher {
            tools: tools(&["visible", "secret"]),
            exact_catalog: false,
            may_require_control_plane: false,
            pending_sources: Arc::from([]),
        };

        let usage = render_tool_usage_instructions(&dispatcher);
        assert!(usage.contains("# Available Tools"));
        assert!(usage.contains("visible tool"));
        assert!(usage.contains("secret tool"));
    }

    #[test]
    fn render_tool_usage_instructions_omits_inventory_for_exact_dispatchers() {
        let dispatcher = UsageTestDispatcher {
            tools: tools(&["visible", "secret_lookup", "secret_audit"]),
            exact_catalog: true,
            may_require_control_plane: false,
            pending_sources: Arc::from([]),
        };

        let usage = render_tool_usage_instructions(&dispatcher);
        assert!(usage.is_empty());
        assert!(deferred_catalog_guidance().contains("tool_catalog_search"));
        assert!(deferred_catalog_guidance().contains("tool_catalog_load"));
    }

    #[test]
    fn render_tool_usage_instructions_keeps_inventory_for_exact_dispatchers_without_deferred_entries()
     {
        let dispatcher = UsageTestDispatcher {
            tools: tools(&["visible"]),
            exact_catalog: true,
            may_require_control_plane: false,
            pending_sources: Arc::from([]),
        };

        let usage = render_tool_usage_instructions(&dispatcher);
        assert!(usage.contains("# Available Tools"));
        assert!(usage.contains("visible tool"));
    }

    #[tokio::test]
    async fn exact_external_sessions_include_deferred_catalog_guidance_in_system_prompt() {
        let temp = tempfile::tempdir().unwrap();
        let factory = AgentFactory::new(temp.path().join("sessions")).builtins(false);
        let secret = Arc::new(ToolDef {
            name: "secret_lookup".to_string(),
            description: "Look up a secret value".to_string(),
            input_schema: serde_json::json!({"type":"object"}),
            provenance: None,
        });
        let secret_audit = Arc::new(ToolDef {
            name: "secret_audit".to_string(),
            description: "Audit a secret value".to_string(),
            input_schema: serde_json::json!({"type":"object"}),
            provenance: None,
        });
        let dispatcher = UsageTestDispatcher {
            tools: vec![Arc::clone(&secret), Arc::clone(&secret_audit)].into(),
            exact_catalog: true,
            may_require_control_plane: false,
            pending_sources: Arc::from([]),
        };
        let mut build_config = AgentBuildConfig::new("claude-sonnet-4-5");
        build_config.llm_client_override = Some(Arc::new(PromptTestClient));
        build_config.override_builtins = ToolCategoryOverride::Disable;
        build_config.external_tools = Some(Arc::new(dispatcher));

        let agent = factory
            .build_agent(build_config, &Config::default())
            .await
            .unwrap();
        let Some(Message::System(message)) = agent.session().messages().first() else {
            unreachable!("expected system prompt");
        };
        let system_prompt = &message.content;

        assert!(
            system_prompt.contains("tool_catalog_search"),
            "exact external sessions should advertise deferred catalog discovery"
        );
        assert!(
            system_prompt.contains("tool_catalog_load"),
            "exact external sessions should advertise deferred catalog loading"
        );
    }

    #[tokio::test]
    async fn exact_inline_only_sessions_do_not_inject_deferred_catalog_surface() {
        let temp = tempfile::tempdir().unwrap();
        let factory = AgentFactory::new(temp.path().join("sessions")).builtins(false);
        let visible = Arc::new(ToolDef {
            name: "visible".to_string(),
            description: "Always-inline tool".to_string(),
            input_schema: serde_json::json!({"type":"object"}),
            provenance: None,
        });
        let dispatcher = UsageTestDispatcher {
            tools: vec![Arc::clone(&visible)].into(),
            exact_catalog: true,
            may_require_control_plane: false,
            pending_sources: Arc::from([]),
        };
        let mut build_config = AgentBuildConfig::new("claude-sonnet-4-5");
        build_config.llm_client_override = Some(Arc::new(PromptTestClient));
        build_config.override_builtins = ToolCategoryOverride::Disable;
        build_config.external_tools = Some(Arc::new(dispatcher));

        let agent = factory
            .build_agent(build_config, &Config::default())
            .await
            .unwrap();
        let Some(Message::System(message)) = agent.session().messages().first() else {
            unreachable!("expected system prompt");
        };
        let system_prompt = &message.content;

        assert!(
            !system_prompt.contains("tool_catalog_search"),
            "inline-only exact sessions should not advertise deferred catalog discovery"
        );
        assert!(
            !system_prompt.contains("tool_catalog_load"),
            "inline-only exact sessions should not advertise deferred catalog loading"
        );
        assert!(
            agent
                .tool_scope()
                .visible_tool_names()
                .unwrap()
                .contains("visible"),
            "inline session tool should remain visible"
        );
        assert!(
            !agent
                .tool_scope()
                .visible_tool_names()
                .unwrap()
                .contains("tool_catalog_search"),
            "control-plane tools should not be injected when there is no deferred catalog"
        );
    }

    #[tokio::test]
    async fn dynamic_exact_sessions_precompose_deferred_catalog_surface_before_threshold() {
        let temp = tempfile::tempdir().unwrap();
        let factory = AgentFactory::new(temp.path().join("sessions")).builtins(false);
        let secret = Arc::new(ToolDef {
            name: "secret_lookup".to_string(),
            description: "Look up a secret value".to_string(),
            input_schema: serde_json::json!({"type":"object"}),
            provenance: None,
        });
        let dispatcher = UsageTestDispatcher {
            tools: vec![Arc::clone(&secret)].into(),
            exact_catalog: true,
            may_require_control_plane: true,
            pending_sources: Arc::from([]),
        };
        let mut build_config = AgentBuildConfig::new("claude-sonnet-4-5");
        build_config.llm_client_override = Some(Arc::new(PromptTestClient));
        build_config.override_builtins = ToolCategoryOverride::Disable;
        build_config.external_tools = Some(Arc::new(dispatcher));

        let agent = factory
            .build_agent(build_config, &Config::default())
            .await
            .unwrap();
        let Some(Message::System(message)) = agent.session().messages().first() else {
            unreachable!("expected system prompt");
        };
        let system_prompt = &message.content;
        let visible_names = agent.tool_scope().visible_tool_names().unwrap();

        assert!(
            system_prompt.contains("tool_catalog_search"),
            "dynamic exact sessions should advertise deferred catalog discovery before the adaptive threshold flips"
        );
        assert!(
            visible_names.contains("tool_catalog_search"),
            "control-plane tools should already be present when the dispatcher may switch into deferred mode later"
        );
        assert!(
            visible_names.contains("secret_lookup"),
            "the session-plane tool should remain inline until adaptive deferred mode activates"
        );
    }
}
