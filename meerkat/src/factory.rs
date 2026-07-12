//! AgentFactory - shared wiring for Meerkat interfaces.

#[cfg(not(feature = "memory-store"))]
use async_trait::async_trait;
use std::any::Any;
use std::collections::BTreeMap;
#[cfg(feature = "skills")]
use std::collections::BTreeSet;
#[cfg(not(feature = "memory-store"))]
use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use meerkat_client::{FactoryError, LlmClient, LlmClientAdapter};
#[cfg(feature = "openai")]
use meerkat_client::{OpenAiCompatibleClient, OpenAiCompatibleClientOptions, OpenAiCompatibleMode};
use meerkat_core::RuntimeBuildMode;
#[cfg(any(not(feature = "memory-store"), not(target_arch = "wasm32")))]
use meerkat_core::SessionId;
#[cfg(not(feature = "memory-store"))]
use meerkat_core::SessionMeta;
#[cfg(test)]
use meerkat_core::SessionToolVisibilityState;
use meerkat_core::ops_lifecycle::OpsLifecycleRegistry;
use meerkat_core::service::{CreateSessionRequest, SessionBuildOptions};
use meerkat_core::{
    Agent, AgentBuilder, AgentEvent, AgentLlmClient, AgentLlmClientDecorator, AgentSessionStore,
    AgentToolDispatcher, AuthBindingRef, BlobStore, BudgetLimits, Config, HookRunOverrides,
    ModelRegistry, OutputSchema, Provider, RealmConnectionSet, RealmId, Session,
    SessionLlmIdentity, SessionMetadata, SessionTooling, ToolCategoryOverride, ToolFilter,
};
use meerkat_runtime::RuntimeOpsLifecycleRegistry;
#[cfg(test)]
use meerkat_runtime::RuntimeTurnStateHandle;
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
use meerkat_tools::builtin::{
    BuiltinToolConfig, CompositeDispatcher, TaskStore, ToolMode, ToolPolicyLayer,
};
use meerkat_tools::{CatalogControlDispatcher, CatalogControlVisibilityProvider};
#[cfg(all(not(feature = "memory-store"), not(target_arch = "wasm32")))]
use tokio::sync::RwLock;
#[cfg(not(target_arch = "wasm32"))]
use tokio::sync::mpsc;
#[cfg(all(not(feature = "memory-store"), target_arch = "wasm32"))]
use tokio_with_wasm::alias::sync::RwLock;
#[cfg(target_arch = "wasm32")]
use tokio_with_wasm::alias::sync::mpsc;

use crate::model_fallback::{ModelFallbackCandidate, ModelFallbackClient};

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

    async fn save_authoritative_projection_if_current_revision(
        &self,
        session: &Session,
        expected_current_revision: Option<String>,
    ) -> Result<(), meerkat_store::SessionStoreError> {
        let mut sessions = self.sessions.write().await;
        let previous = sessions.get(session.id());
        meerkat_core::session_store::authoritative_projection_current_revision_guard(
            session,
            previous,
            expected_current_revision.as_deref(),
        )?;
        sessions.insert(session.id().clone(), session.clone());
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

    async fn delete_if_current_revision(
        &self,
        id: &SessionId,
        expected_current_revision: &str,
    ) -> Result<bool, meerkat_store::SessionStoreError> {
        let mut sessions = self.sessions.write().await;
        let Some(previous) = sessions.get(id) else {
            return Ok(false);
        };
        let previous_token = meerkat_core::session_store::session_projection_cas_token(previous)?;
        if previous_token != expected_current_revision {
            return Ok(false);
        }
        sessions.remove(id);
        Ok(true)
    }
}

/// Type-erased agent using trait objects.
pub type DynAgent = Agent<dyn AgentLlmClient, dyn AgentToolDispatcher, dyn AgentSessionStore>;

#[cfg(not(target_arch = "wasm32"))]
type CoreAgentFactoryBuildFuture =
    Pin<Box<dyn Future<Output = Result<DynAgent, meerkat_core::AgentBuildPolicyError>> + Send>>;

#[cfg(target_arch = "wasm32")]
type CoreAgentFactoryBuildFuture =
    Pin<Box<dyn Future<Output = Result<DynAgent, meerkat_core::AgentBuildPolicyError>>>>;

struct AgentFactoryPolicyBridgeToken;

static AGENT_FACTORY_POLICY_BRIDGE_TOKEN: AgentFactoryPolicyBridgeToken =
    AgentFactoryPolicyBridgeToken;

fn agent_factory_policy_bridge_token() -> &'static (dyn Any + Send + Sync) {
    &AGENT_FACTORY_POLICY_BRIDGE_TOKEN
}

#[doc(hidden)]
#[allow(improper_ctypes_definitions, unsafe_code)]
#[unsafe(export_name = concat!(
    "__meerkat_agent_factory_policy_bridge_token_is_valid_v1_",
    env!("MEERKAT_AGENT_FACTORY_POLICY_BRIDGE_SYMBOL_SUFFIX")
))]
pub extern "Rust" fn agent_factory_policy_bridge_token_is_valid(
    factory_bridge_token: &(dyn Any + Send + Sync),
) -> bool {
    factory_bridge_token.is::<AgentFactoryPolicyBridgeToken>()
}

#[allow(improper_ctypes_definitions, unsafe_code)]
unsafe extern "Rust" {
    #[link_name = concat!(
        "__meerkat_agent_factory_policy_build_v3_",
        env!("MEERKAT_AGENT_FACTORY_POLICY_BRIDGE_SYMBOL_SUFFIX")
    )]
    fn core_agent_factory_policy_build(
        factory_bridge_token: &'static (dyn Any + Send + Sync),
        builder: AgentBuilder,
        client: Arc<dyn AgentLlmClient>,
        tools: Arc<dyn AgentToolDispatcher>,
        store: Arc<dyn AgentSessionStore>,
    ) -> CoreAgentFactoryBuildFuture;

    #[link_name = concat!(
        "__meerkat_agent_factory_parent_tool_composition_authority_new_v1_",
        env!("MEERKAT_AGENT_FACTORY_POLICY_BRIDGE_SYMBOL_SUFFIX")
    )]
    fn core_agent_factory_parent_tool_composition_authority_new(
        factory_bridge_token: &'static (dyn Any + Send + Sync),
    ) -> Result<meerkat_core::service::ParentToolCompositionAuthority, String>;

    #[link_name = concat!(
        "__meerkat_agent_factory_parent_tool_composition_authority_set_tool_scope_v1_",
        env!("MEERKAT_AGENT_FACTORY_POLICY_BRIDGE_SYMBOL_SUFFIX")
    )]
    fn core_agent_factory_parent_tool_composition_authority_set_tool_scope(
        factory_bridge_token: &'static (dyn Any + Send + Sync),
        authority: &meerkat_core::service::ParentToolCompositionAuthority,
        tool_scope: &meerkat_core::ToolScope,
    ) -> Result<(), String>;
}

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
/// Accepts exactly the typed wrapper produced by
/// [`encode_llm_client_override_for_service`]; any other payload decodes to
/// `None`.
pub fn decode_llm_client_override_from_service(
    value: &Arc<dyn std::any::Any + Send + Sync>,
) -> Option<Arc<dyn LlmClient>> {
    value
        .as_ref()
        .downcast_ref::<ErasedLlmClientOverride>()
        .map(|typed| typed.0.clone())
}

/// Resolve a pre-built session comms runtime override (multi-host mobs
/// DEC-P3H-3). Fail closed on both failure modes: an unrecognized payload
/// never falls back to config-mode construction, and a runtime whose
/// participant name disagrees with the build's `comms_name` never builds a
/// session under the wrong identity.
#[cfg(all(feature = "comms", not(target_arch = "wasm32")))]
fn resolve_session_comms_runtime_override(
    override_payload: Option<Arc<dyn std::any::Any + Send + Sync>>,
    expected_comms_name: Option<&str>,
) -> Result<Option<Arc<meerkat_comms::CommsRuntime>>, BuildAgentError> {
    let Some(payload) = override_payload else {
        return Ok(None);
    };
    let Some(runtime) = crate::sdk::decode_session_comms_runtime_override_from_service(&payload)
    else {
        return Err(BuildAgentError::Config(
            "session_comms_runtime_override carried an unrecognized payload; refusing to fall \
             back to config-mode comms construction"
                .to_string(),
        ));
    };
    let actual = runtime.participant_name().to_string();
    match expected_comms_name {
        Some(expected) if expected == actual => Ok(Some(runtime)),
        Some(expected) => Err(BuildAgentError::Config(format!(
            "session comms runtime override name '{actual}' does not match the build's \
             comms_name '{expected}'"
        ))),
        None => Err(BuildAgentError::Config(
            "session comms runtime override provided without a comms_name on the build".to_string(),
        )),
    }
}

/// wasm32 posture for the session comms runtime override: the encoding
/// helper (`sdk`) and every producer of the payload (the member-host
/// daemon's materializer) are native-only, so a payload arriving here is a
/// wiring error — fail closed, never silently ignore it.
#[cfg(all(feature = "comms", target_arch = "wasm32"))]
fn resolve_session_comms_runtime_override(
    override_payload: Option<Arc<dyn std::any::Any + Send + Sync>>,
    _expected_comms_name: Option<&str>,
) -> Result<Option<Arc<meerkat_comms::CommsRuntime>>, BuildAgentError> {
    match override_payload {
        None => Ok(None),
        Some(_) => Err(BuildAgentError::Config(
            "session_comms_runtime_override is not supported on wasm32 builds".to_string(),
        )),
    }
}

/// A1/ADJ-17 predicate: the factory-appended skill-inventory prompt section
/// applies to `Full` builds only. `SpecPinned` (set ONLY by the member-host
/// spec decompiler, and by revival re-running it) keeps spec-built session
/// prompts byte-pinned across placements; local mobs are unchanged.
fn skill_inventory_prompt_section_enabled(
    sections: meerkat_core::service::HostPromptSections,
) -> bool {
    matches!(sections, meerkat_core::service::HostPromptSections::Full)
}

/// Full configuration for building an agent via [`AgentFactory::build_agent()`].
#[derive(Clone)]
pub struct AgentBuildConfig {
    /// Model name (e.g. "claude-sonnet-4-5").
    pub model: String,
    /// Explicit provider. If `None`, inferred from the model name.
    pub provider: Option<Provider>,
    /// Durable self-hosted server binding for configured aliases.
    pub self_hosted_server_id: Option<String>,
    /// Caller-scoped custom model registry entries (e.g. mob-definition
    /// `[models.<id>]` tables). Merged into the effective `ModelRegistry`
    /// for this build, the single owner feeding provider inference,
    /// compaction scaling, capability gates, and call timeouts.
    pub custom_models: std::collections::BTreeMap<String, meerkat_core::config::CustomModelConfig>,
    /// Configured default provider for `Auto` image-generation targets.
    /// When set, the planner resolves `Auto` against this provider instead of
    /// the session's typed LLM provider identity.
    pub image_generation_provider: Option<Provider>,
    /// Per-build auto-compaction threshold override (tokens, non-zero).
    /// Wins over the global config knob and model-aware scaling.
    pub auto_compact_threshold_override: Option<std::num::NonZeroU64>,
    /// Host-supplied compaction summary curator. When set, the agent loop
    /// asks the curator for the compaction summary instead of running the
    /// summarization LLM call; the compaction trigger and history rebuild
    /// stay with the configured compactor.
    pub compaction_curator_override: Option<Arc<dyn meerkat_core::CompactionCurator>>,
    /// Max tokens per turn. If `None`, uses `Config::max_tokens`.
    pub max_tokens: Option<u32>,
    /// Typed per-request system-prompt policy.
    ///
    /// `Inherit` uses the config/AGENTS/default precedence chain; `Set` wins
    /// outright (skipping config + AGENTS.md); `Disable` suppresses every
    /// prompt source.
    pub system_prompt: crate::SystemPromptOverride,
    /// Optional output schema for structured extraction.
    pub output_schema: Option<OutputSchema>,
    /// Structured-output retry budget *intent*. `None` inherits the canonical
    /// default ([`meerkat_core::config::default_structured_output_retries`]);
    /// the `AgentFactory` seam resolves it. Surfaces must not fabricate one.
    pub structured_output_retries: Option<u32>,
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
    /// Override pre-adapted low-level agent LLM client.
    ///
    /// This preserves full `AssistantBlock` output for compatibility surfaces
    /// that already own the core agent client, while the surrounding factory
    /// still owns provider defaults, hooks, session metadata, and runtime
    /// setup.
    pub agent_llm_client_override: Option<Arc<dyn AgentLlmClient>>,
    /// Optional wrapper applied to the final agent-facing LLM client.
    ///
    /// Runs after provider resolution and after either raw or pre-adapted
    /// explicit overrides have reached the `AgentLlmClient` boundary.
    pub agent_llm_client_decorator: Option<AgentLlmClientDecorator>,
    /// Typed provider-specific parameter overrides (e.g., thinking config,
    /// reasoning effort). Parsed fail-closed at the surface ingress; the
    /// factory never ferries an untyped JSON bag (K2).
    pub provider_params: Option<meerkat_core::lifecycle::run_primitive::ProviderParamsOverride>,
    /// External tool dispatcher to compose with builtins (e.g., MCP callback tools).
    pub external_tools: Option<Arc<dyn AgentToolDispatcher>>,
    /// Declarative MCP server configs materialized by this build into a
    /// session-owned MCP router composed with `external_tools` and builtins.
    pub mcp_servers: Vec<meerkat_core::mcp_config::McpServerConfig>,
    /// Serializable tool definitions that can rebuild recoverable
    /// surface-owned dispatchers after persistence or runtime restart.
    pub recoverable_tool_defs: Option<Vec<meerkat_core::ToolDef>>,
    /// Optional blob store override used for image externalization/hydration.
    pub blob_store_override: Option<Arc<dyn BlobStore>>,
    /// Optional runtime machine handle for the generated-image builtin.
    pub image_generation_machine_override:
        Option<Arc<dyn meerkat_tools::builtin::image_generation::ImageGenerationMachine>>,
    /// Optional provider executor for the generated-image builtin.
    pub image_generation_executor_override:
        Option<Arc<dyn meerkat_llm_core::ImageGenerationExecutor>>,
    /// Optional provider executor for the Meerkat-owned web-search fallback.
    pub web_search_executor_override: Option<Arc<dyn meerkat_llm_core::WebSearchExecutor>>,
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
    /// Per-build override for factory-level `enable_workgraph`.
    /// `Inherit` defers to the factory default.
    pub override_workgraph: ToolCategoryOverride,
    /// Per-build override for factory-level `enable_mob`.
    pub override_mob: ToolCategoryOverride,
    /// Per-build override for factory-level comms tooling.
    /// `Inherit` defers to the factory/runtime default.
    pub override_comms: ToolCategoryOverride,
    /// Per-build override for assistant image generation visibility.
    pub override_image_generation: ToolCategoryOverride,
    /// Per-build override for Meerkat-owned fallback web-search visibility.
    pub override_web_search: ToolCategoryOverride,
    /// Agent-facing scheduler tools supplied by the embedding surface.
    ///
    /// Scheduler is a surface capability. This dispatcher only controls
    /// visibility/composition; schedule execution remains owned by the
    /// schedule service/host chosen by the embedding surface.
    pub schedule_tools: Option<Arc<dyn AgentToolDispatcher>>,
    /// Agent-facing WorkGraph tools supplied by the embedding surface.
    pub workgraph_tools: Option<Arc<dyn AgentToolDispatcher>>,
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
    pub preload_skills: Option<Vec<meerkat_core::skills::SkillKey>>,
    /// Realm identity for cross-surface storage sharing/isolation.
    pub realm_id: Option<RealmId>,
    /// Optional process/agent instance identifier within a realm.
    pub instance_id: Option<String>,
    /// Backend pinned by the realm manifest (e.g. "sqlite", "jsonl", "memory").
    ///
    /// Typed thread-through (`#207`): parsed fail-closed at the surface ingress
    /// and projected to the durable `SessionMetadata.backend` string only at the
    /// metadata write below.
    pub backend: Option<meerkat_core::RecoveryBackendKind>,
    /// Config generation used when this session was created/resumed.
    pub config_generation: Option<u64>,
    /// Realm-scoped auth binding.
    ///
    /// When `Some`, `build_agent()` routes provider/auth resolution through
    /// `ProviderRuntimeRegistry` against the named realm-scoped
    /// `Config.realm[realm_id]` binding. When `None`, resolution falls back to
    /// the realm default binding for the provider (including the synthesized
    /// `env_default` realm for provider-native env keys); self-hosted builds
    /// with no canonical realm binding fail closed. `llm_client_override`
    /// beats both paths.
    pub auth_binding: Option<meerkat_core::AuthBindingRef>,
    /// Typed durable mob-member identity, set by the mob runtime when building a
    /// member's session. Persisted onto `SessionMetadata.mob_member_binding`,
    /// where mob ownership routing reads it directly on resume/restart.
    pub mob_member_binding: Option<meerkat_core::MobMemberBinding>,
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
    /// These are set on the session's internal metadata map before the core
    /// `AgentBuilder` factory-policy seam runs, so they are available for
    /// early-stage recovery (e.g. canonical inherited visibility state).
    /// On resumed sessions, canonical tool visibility metadata is merged into
    /// the durable visibility state instead of replacing it.
    pub initial_metadata_entries: std::collections::BTreeMap<String, serde_json::Value>,
    /// Typed initial visibility handoff to the generated visibility owner.
    ///
    /// This is intentionally not metadata: initial inherited visibility is a
    /// semantic machine fact and must be applied through the core visibility
    /// owner after tool authority catalogs are installed.
    pub initial_tool_visibility_state: Option<meerkat_core::InheritedToolVisibilityAuthority>,
    /// Typed session-local tool filter to apply once the agent's tool catalog
    /// is fully composed.
    ///
    /// This is intentionally a typed carrier, not a metadata-string side
    /// channel: the initial tool filter is build-time session semantics and is
    /// owned directly by the build config (mirroring
    /// `initial_tool_visibility_state`).
    pub initial_tool_filter: Option<meerkat_core::ToolFilter>,
    /// Per-launch call-level tool access policy (the existing
    /// [`meerkat_core::ops::ToolAccessPolicy`] vocabulary).
    ///
    /// Resolved into the sealed [`meerkat_core::ToolExecutionPolicy`] and
    /// applied as the OUTERMOST dispatcher composition: execution of a denied
    /// tool fails with an ordinary `access_denied` tool error while the
    /// LLM-visible tool list (`tools()`/`tool_catalog()`) stays byte-identical
    /// (no prompt-cache prefix change). `Inherit` must be resolved by the
    /// spawn chain before build; an unresolved `Inherit` fails the build
    /// closed with a typed error. The effective policy is persisted into
    /// `SessionMetadata.tooling.tool_access_policy` so children can inherit
    /// it transitively.
    pub tool_access_policy: Option<meerkat_core::ops::ToolAccessPolicy>,
    /// Pre-built session comms runtime, type-erased (multi-host mobs
    /// DEC-P3H-3 — the `llm_client_override` precedent). Produced by
    /// [`crate::encode_session_comms_runtime_override_for_service`]; the
    /// factory's comms arm downcasts fail-closed, verifies the participant
    /// name equals `comms_name`, and skips config-mode construction. Set by
    /// the mob member-host materializer; never by ordinary surfaces.
    pub session_comms_runtime_override: Option<Arc<dyn std::any::Any + Send + Sync>>,
    /// ADJ-17 prompt-section policy: `SpecPinned` (set ONLY by the member
    /// host's spec decompiler, and by revival re-running it) suppresses the
    /// factory-appended skill-inventory section so spec-built prompts are
    /// byte-pinned across placements. `Full` (default) is unchanged.
    pub host_prompt_sections: meerkat_core::service::HostPromptSections,
}

impl std::fmt::Debug for AgentBuildConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentBuildConfig")
            .field("model", &self.model)
            .field("provider", &self.provider)
            .field("self_hosted_server_id", &self.self_hosted_server_id)
            .field("custom_models", &self.custom_models.keys())
            .field("image_generation_provider", &self.image_generation_provider)
            .field(
                "auto_compact_threshold_override",
                &self.auto_compact_threshold_override,
            )
            .field(
                "compaction_curator_override",
                &self.compaction_curator_override.is_some(),
            )
            .field("max_tokens", &self.max_tokens)
            .field("system_prompt", &{
                match &self.system_prompt {
                    crate::SystemPromptOverride::Inherit => "Inherit".to_string(),
                    crate::SystemPromptOverride::Disable => "Disable".to_string(),
                    crate::SystemPromptOverride::Set(s) => {
                        let head: String = s.chars().take(64).collect();
                        format!("Set({head:?})")
                    }
                }
            })
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
            .field(
                "agent_llm_client_override",
                &self.agent_llm_client_override.is_some(),
            )
            .field(
                "agent_llm_client_decorator",
                &self.agent_llm_client_decorator.is_some(),
            )
            .field("provider_params", &self.provider_params.is_some())
            .field("external_tools", &self.external_tools.is_some())
            .field("recoverable_tool_defs", &self.recoverable_tool_defs)
            .field("blob_store_override", &self.blob_store_override.is_some())
            .field(
                "image_generation_machine_override",
                &self.image_generation_machine_override.is_some(),
            )
            .field(
                "image_generation_executor_override",
                &self.image_generation_executor_override.is_some(),
            )
            .field(
                "web_search_executor_override",
                &self.web_search_executor_override.is_some(),
            )
            .field("override_builtins", &self.override_builtins)
            .field("override_shell", &self.override_shell)
            .field("override_memory", &self.override_memory)
            .field("override_schedule", &self.override_schedule)
            .field("override_workgraph", &self.override_workgraph)
            .field("override_mob", &self.override_mob)
            .field("override_comms", &self.override_comms)
            .field("override_image_generation", &self.override_image_generation)
            .field("override_web_search", &self.override_web_search)
            .field("schedule_tools", &self.schedule_tools.is_some())
            .field("workgraph_tools", &self.workgraph_tools.is_some())
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
            .field("mcp_servers", &self.mcp_servers)
            .field("runtime_build_mode", &self.runtime_build_mode)
            .field(
                "initial_tool_visibility_state",
                &self.initial_tool_visibility_state.is_some(),
            )
            .field("initial_tool_filter", &self.initial_tool_filter.is_some())
            .field("tool_access_policy", &self.tool_access_policy)
            .field(
                "session_comms_runtime_override",
                &self.session_comms_runtime_override.is_some(),
            )
            .field("host_prompt_sections", &self.host_prompt_sections)
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
            custom_models: std::collections::BTreeMap::new(),
            image_generation_provider: None,
            auto_compact_threshold_override: None,
            compaction_curator_override: None,
            max_tokens: None,
            system_prompt: crate::SystemPromptOverride::Inherit,
            output_schema: None,
            structured_output_retries: None,
            hooks_override: HookRunOverrides::default(),
            keep_alive: false,
            comms_name: None,
            peer_meta: None,
            resume_session: None,
            budget_limits: None,
            event_tx: None,
            llm_client_override: None,
            agent_llm_client_override: None,
            agent_llm_client_decorator: None,
            provider_params: None,
            external_tools: None,
            recoverable_tool_defs: None,
            blob_store_override: None,
            image_generation_machine_override: None,
            image_generation_executor_override: None,
            web_search_executor_override: None,
            override_builtins: ToolCategoryOverride::Inherit,
            override_shell: ToolCategoryOverride::Inherit,
            override_memory: ToolCategoryOverride::Inherit,
            override_schedule: ToolCategoryOverride::Inherit,
            override_workgraph: ToolCategoryOverride::Inherit,
            override_mob: ToolCategoryOverride::Inherit,
            override_comms: ToolCategoryOverride::Inherit,
            override_image_generation: ToolCategoryOverride::Inherit,
            override_web_search: ToolCategoryOverride::Inherit,
            schedule_tools: None,
            workgraph_tools: None,
            mob_tool_authority_context: None,
            mob_tools: None,
            preload_skills: None,
            realm_id: None,
            instance_id: None,
            backend: None,
            config_generation: None,
            auth_binding: None,
            mob_member_binding: None,
            silent_comms_intents: Vec::new(),
            max_inline_peer_notifications: None,
            tool_dispatcher_override: None,
            session_store_override: None,
            hook_engine_override: None,
            skill_engine_override: None,
            app_context: None,
            additional_instructions: None,
            wait_for_mcp: false,
            mcp_servers: Vec::new(),
            shell_env: None,
            checkpointer: None,
            call_timeout_override: meerkat_core::CallTimeoutOverride::default(),
            resume_override_mask: meerkat_core::service::ResumeOverrideMask::default(),
            runtime_build_mode: meerkat_core::RuntimeBuildMode::StandaloneEphemeral,
            initial_metadata_entries: std::collections::BTreeMap::new(),
            initial_tool_visibility_state: None,
            initial_tool_filter: None,
            tool_access_policy: None,
            session_comms_runtime_override: None,
            host_prompt_sections: meerkat_core::service::HostPromptSections::default(),
        }
    }

    /// Stage a session-local tool filter to apply once the agent's tool
    /// catalog is fully composed.
    pub fn set_initial_tool_filter(&mut self, filter: meerkat_core::ToolFilter) {
        self.initial_tool_filter = Some(filter);
    }

    /// Build config from a service `CreateSessionRequest` + event channel.
    pub fn from_create_session_request(
        req: &CreateSessionRequest,
        event_tx: mpsc::Sender<AgentEvent>,
    ) -> Self {
        let mut build = Self::new(req.model.clone());
        // The request carries the typed tri-state policy end-to-end; no
        // Option<String> projection exists at this boundary.
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
        let (override_mob, authority_context) =
            meerkat_runtime::mob_operator_authority::resolve_mob_operator_access(
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
        self.custom_models = build.custom_models.clone();
        self.image_generation_provider = build.image_generation_provider;
        self.auto_compact_threshold_override = build.auto_compact_threshold_override;
        self.output_schema = build.output_schema.clone();
        self.structured_output_retries = build.structured_output_retries;
        self.hooks_override = build.hooks_override.clone();
        self.comms_name = build.comms_name.clone();
        self.peer_meta = build.peer_meta.clone();
        self.resume_session = build.resume_session.clone();
        self.budget_limits = build.budget_limits.clone();
        self.provider_params = build.provider_params.clone();
        self.external_tools = build.external_tools.clone();
        self.mcp_servers = build.mcp_servers.clone();
        self.recoverable_tool_defs = build.recoverable_tool_defs.clone();
        self.blob_store_override = build.blob_store_override.clone();
        self.llm_client_override = build
            .llm_client_override
            .as_ref()
            .and_then(decode_llm_client_override_from_service);
        self.agent_llm_client_decorator = build.agent_llm_client_decorator.clone();
        self.override_builtins = build.override_builtins;
        self.override_shell = build.override_shell;
        self.override_memory = build.override_memory;
        self.override_schedule = build.override_schedule;
        self.override_workgraph = build.override_workgraph;
        self.override_comms = build.override_comms;
        let (override_mob, mob_tool_authority_context) =
            meerkat_runtime::mob_operator_authority::resolve_mob_operator_access(
                build.override_mob,
                build.mob_tool_authority_context.clone(),
            );
        self.override_mob = override_mob;
        self.override_image_generation = build.override_image_generation;
        self.override_web_search = build.override_web_search;
        self.schedule_tools = build.schedule_tools.clone();
        self.workgraph_tools = build.workgraph_tools.clone();
        self.mob_tool_authority_context = mob_tool_authority_context;
        self.mob_tools = build.mob_tools.clone();
        self.preload_skills = build.preload_skills.clone();
        self.realm_id = build.realm_id.clone();
        self.instance_id = build.instance_id.clone();
        self.backend = build.backend;
        self.config_generation = build.config_generation;
        // Phase 3: auth_binding flows from SessionBuildOptions into
        // AgentBuildConfig so surfaces can drive binding selection per-request.
        self.auth_binding = build.auth_binding.clone();
        self.mob_member_binding = build.mob_member_binding.clone();
        self.keep_alive = build.keep_alive;
        self.silent_comms_intents
            .clone_from(&build.silent_comms_intents);
        self.max_inline_peer_notifications = build.max_inline_peer_notifications;
        self.app_context = build.app_context.clone();
        self.additional_instructions = build.additional_instructions.clone();
        self.initial_metadata_entries = build.initial_metadata_entries.clone();
        self.initial_tool_filter = build.initial_tool_filter.clone();
        self.tool_access_policy = build.tool_access_policy.clone();
        self.shell_env = build.shell_env.clone();
        self.checkpointer = build.checkpointer.clone();
        self.call_timeout_override = build.call_timeout_override.clone();
        self.resume_override_mask = build.resume_override_mask;
        self.runtime_build_mode = build.runtime_build_mode.clone();
        // Carried erased: the comms arm downcasts fail-closed at build time
        // (a foreign payload is a typed build error there, never a silent
        // drop here).
        self.session_comms_runtime_override = build.session_comms_runtime_override.clone();
        self.host_prompt_sections = build.host_prompt_sections;
    }

    /// Convert build options to the service transport representation.
    pub fn to_session_build_options(&self) -> SessionBuildOptions {
        SessionBuildOptions {
            provider: self.provider,
            self_hosted_server_id: self.self_hosted_server_id.clone(),
            custom_models: self.custom_models.clone(),
            image_generation_provider: self.image_generation_provider,
            auto_compact_threshold_override: self.auto_compact_threshold_override,
            output_schema: self.output_schema.clone(),
            structured_output_retries: self.structured_output_retries,
            hooks_override: self.hooks_override.clone(),
            comms_name: self.comms_name.clone(),
            peer_meta: self.peer_meta.clone(),
            resume_session: self.resume_session.clone(),
            budget_limits: self.budget_limits.clone(),
            provider_params: self.provider_params.clone(),
            external_tools: self.external_tools.clone(),
            mcp_servers: self.mcp_servers.clone(),
            recoverable_tool_defs: self.recoverable_tool_defs.clone(),
            blob_store_override: self.blob_store_override.clone(),
            llm_client_override: self
                .llm_client_override
                .clone()
                .map(encode_llm_client_override_for_service),
            agent_llm_client_decorator: self.agent_llm_client_decorator.clone(),
            override_builtins: self.override_builtins,
            override_shell: self.override_shell,
            override_memory: self.override_memory,
            override_schedule: self.override_schedule,
            override_workgraph: self.override_workgraph,
            override_mob: self.override_mob,
            override_image_generation: self.override_image_generation,
            override_web_search: self.override_web_search,
            // The facade build pipeline exposes no comms-override API surface;
            // the per-build comms override (#76) is owned by the recovery path
            // (`build_recovered_session`), so the non-recovery build inherits.
            override_comms: ToolCategoryOverride::Inherit,
            schedule_tools: self.schedule_tools.clone(),
            workgraph_tools: self.workgraph_tools.clone(),
            mob_tool_authority_context: self.mob_tool_authority_context.clone(),
            mob_tools: self.mob_tools.clone(),
            preload_skills: self.preload_skills.clone(),
            realm_id: self.realm_id.clone(),
            instance_id: self.instance_id.clone(),
            backend: self.backend,
            config_generation: self.config_generation,
            auth_binding: self.auth_binding.clone(),
            mob_member_binding: self.mob_member_binding.clone(),
            keep_alive: self.keep_alive,
            silent_comms_intents: self.silent_comms_intents.clone(),
            max_inline_peer_notifications: self.max_inline_peer_notifications,
            app_context: self.app_context.clone(),
            additional_instructions: self.additional_instructions.clone(),
            initial_metadata_entries: self.initial_metadata_entries.clone(),
            initial_tool_filter: self.initial_tool_filter.clone(),
            tool_access_policy: self.tool_access_policy.clone(),
            shell_env: self.shell_env.clone(),
            checkpointer: self.checkpointer.clone(),
            call_timeout_override: self.call_timeout_override.clone(),
            resume_override_mask: self.resume_override_mask,
            runtime_build_mode: self.runtime_build_mode.clone(),
            initial_turn_metadata: None,
            session_comms_runtime_override: self.session_comms_runtime_override.clone(),
            host_prompt_sections: self.host_prompt_sections,
        }
    }
}

fn realm_allows_file_backed_semantic_memory(
    backend: Option<meerkat_core::RecoveryBackendKind>,
) -> bool {
    !matches!(backend, Some(meerkat_core::RecoveryBackendKind::Memory))
}

fn effective_memory_for_realm(
    requested: ToolCategoryOverride,
    factory_enabled: bool,
    backend: Option<meerkat_core::RecoveryBackendKind>,
) -> bool {
    realm_allows_file_backed_semantic_memory(backend) && requested.resolve(factory_enabled)
}

fn metadata_memory_override_for_realm(
    requested: ToolCategoryOverride,
    backend: Option<meerkat_core::RecoveryBackendKind>,
) -> ToolCategoryOverride {
    if realm_allows_file_backed_semantic_memory(backend) {
        requested
    } else {
        ToolCategoryOverride::Disable
    }
}

/// Errors that can occur when building an agent via [`AgentFactory::build_agent()`].
#[derive(Debug, thiserror::Error)]
pub enum BuildAgentError {
    /// Cannot infer provider from the given model name.
    #[error("Cannot infer provider from model '{model}'")]
    UnknownProvider { model: String },

    /// LLM client creation failed.
    ///
    /// Carries the typed [`FactoryError`] cause: realm/binding target
    /// selection (`ConnectionTarget`), provider auth/credential resolution
    /// (`ProviderAuth`), client construction (`ClientBuild`), and TokenStore
    /// availability (`TokenStore`) all propagate typed — never flattened
    /// into a string variant.
    #[error("LLM client creation failed: {0}")]
    LlmClient(#[from] FactoryError),

    /// Tool dispatcher creation failed.
    #[error("Tool dispatcher creation failed: {0}")]
    ToolDispatcher(#[from] CompositeDispatcherError),

    /// Declarative MCP server materialization failed (staging, bring-up, or
    /// `mcp_servers` requested without the `mcp` feature).
    #[error("MCP setup failed: {0}")]
    McpSetup(String),

    /// Comms runtime failed to initialize.
    #[error("Comms runtime failed: {0}")]
    #[cfg(feature = "comms")]
    Comms(String),

    /// Comms construction failed because the canonical session identity claim is live.
    #[error("Session identity already active: {0}")]
    #[cfg(feature = "comms")]
    SessionIdentityInUse(meerkat_core::SessionId),

    /// Configuration error.
    #[error("Config error: {0}")]
    Config(String),

    /// An explicit tool-category `Enable` could not be satisfied.
    ///
    /// Emitted when a caller explicitly enabled a tool capability
    /// (e.g. `override_web_search = Enable`, `override_memory = Enable`)
    /// but the underlying capability could not be provisioned. An explicit
    /// enable must deliver the tool or fail closed; it must never silently
    /// drop to a tool-less build. `Inherit`-default builds that resolve to no
    /// tool are unaffected (they degrade silently by design).
    #[error("Explicit '{capability}' enable could not be satisfied: {reason}")]
    CapabilityUnavailable {
        /// The tool capability that was explicitly enabled.
        capability: &'static str,
        /// Why the capability could not be provisioned.
        reason: String,
    },

    /// `keep_alive` was set but `comms_name` is missing.
    #[error("keep_alive requires comms_name to be set")]
    #[cfg(feature = "comms")]
    KeepAliveRequiresCommsName,
}

#[cfg(feature = "comms")]
impl From<meerkat_comms::CommsRuntimeError> for BuildAgentError {
    fn from(error: meerkat_comms::CommsRuntimeError) -> Self {
        match error {
            meerkat_comms::CommsRuntimeError::SessionIdentityInUse(session_id) => {
                Self::SessionIdentityInUse(session_id)
            }
            other => Self::Comms(other.to_string()),
        }
    }
}

/// Resolver that delegates to [`ModelRegistry`] to look up model-specific
/// operational defaults at call time.
///
/// This struct bridges the dependency gap: `meerkat-core` owns the
/// `ModelOperationalDefaultsResolver` trait, and this facade-layer
/// implementation provides the concrete registry lookup.
struct RegistryBackedDefaultsResolver {
    registry: Arc<ModelRegistry>,
}

impl meerkat_core::ModelOperationalDefaultsResolver for RegistryBackedDefaultsResolver {
    fn call_timeout_for(&self, provider: Provider, model: &str) -> Option<std::time::Duration> {
        self.registry
            .profile_for_provider(provider, model)
            .and_then(|p| p.call_timeout_secs)
            .map(std::time::Duration::from_secs)
    }
}

fn provider_tool_defaults_for(
    provider: Provider,
    config: &Config,
    model_profile: Option<&meerkat_core::model_profile::ModelProfile>,
    web_search_override: ToolCategoryOverride,
) -> Option<meerkat_core::lifecycle::run_primitive::ProviderTag> {
    use meerkat_core::lifecycle::run_primitive::{
        AnthropicProviderTag, GeminiProviderTag, OpaqueProviderBody, OpenAiProviderTag, ProviderTag,
    };
    if matches!(web_search_override, ToolCategoryOverride::Disable) {
        return None;
    }
    if !model_profile.is_some_and(|profile| profile.supports_web_search) {
        return None;
    }

    match provider {
        Provider::Anthropic if config.provider_tools.anthropic.web_search => {
            Some(ProviderTag::Anthropic(AnthropicProviderTag {
                web_search: Some(OpaqueProviderBody::from_value(&serde_json::json!({
                    "type": "web_search_20250305",
                    "name": "web_search"
                }))),
                ..Default::default()
            }))
        }
        Provider::OpenAI if config.provider_tools.openai.web_search => {
            Some(ProviderTag::OpenAi(OpenAiProviderTag {
                web_search: Some(OpaqueProviderBody::from_value(
                    &serde_json::json!({"type": "web_search"}),
                )),
                ..Default::default()
            }))
        }
        Provider::Gemini if config.provider_tools.gemini.google_search => {
            Some(ProviderTag::Gemini(GeminiProviderTag {
                google_search: Some(OpaqueProviderBody::from_value(&serde_json::json!({}))),
                ..Default::default()
            }))
        }
        _ => None,
    }
}

/// Typed create-session model hints supplied by a public surface.
///
/// `None` means omitted by the caller. In particular, this type does not carry
/// fabricated provenance for values that were read from config.
#[derive(Debug, Clone, Default)]
pub struct CreateSessionModelResolutionRequest {
    pub model: Option<String>,
    pub provider: Option<Provider>,
    pub auth_binding: Option<AuthBindingRef>,
}

/// Canonical lowering of create-session model hints into one concrete LLM
/// identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateSessionModelResolution {
    pub model: String,
    pub provider: Provider,
    /// Owner-stamped configured binding when the request named one.
    pub auth_binding: Option<AuthBindingRef>,
}

/// Fail-closed errors from create-session model resolution.
#[derive(Debug, thiserror::Error)]
pub enum CreateSessionModelResolutionError {
    #[error("invalid effective model registry: {0}")]
    Config(#[from] meerkat_core::ConfigError),
    #[error("invalid auth binding: {0}")]
    AuthBinding(#[from] meerkat_core::ConnectionTargetError),
    #[error(
        "auth binding realm '{realm}' binding '{binding}' selects provider {binding_provider:?}, but the request selected provider {explicit_provider:?}"
    )]
    AuthBindingProviderMismatch {
        realm: RealmId,
        binding: meerkat_core::BindingId,
        binding_provider: Provider,
        explicit_provider: Provider,
    },
    #[error("{0}")]
    ProviderModelMismatch(String),
    #[error("invalid configured model/provider identity: {0}")]
    ConfiguredProviderModelMismatch(String),
    #[error("cannot infer provider from model '{model}'; specify a provider or auth binding")]
    UnknownProvider { model: String },
    #[error("cannot infer provider from configured create-session model '{model}'")]
    ConfiguredUnknownProvider { model: String },
    #[error("provider {provider:?} has no configured or catalog default model")]
    MissingProviderDefault { provider: Provider },
    #[error(
        "auth binding realm '{realm}' binding '{binding}' selects provider {provider:?}, which has no configured or catalog default model"
    )]
    BindingMissingProviderDefault {
        realm: RealmId,
        binding: meerkat_core::BindingId,
        provider: Provider,
    },
    #[error("explicit model must not be empty")]
    EmptyExplicitModel,
    #[error("auth binding realm '{realm}' binding '{binding}' has an empty default_model")]
    EmptyBindingDefaultModel {
        realm: RealmId,
        binding: meerkat_core::BindingId,
    },
}

impl CreateSessionModelResolutionError {
    /// Whether this failure comes from server-owned effective configuration
    /// rather than caller-supplied create-session hints.
    ///
    /// Public surfaces use this typed classification to keep malformed host
    /// configuration on their internal/configuration error channel instead of
    /// blaming the request as invalid parameters. Keep the match exhaustive so
    /// every future resolver error makes an explicit fault-ownership decision.
    #[must_use]
    pub const fn is_configuration_fault(&self) -> bool {
        match self {
            Self::Config(_)
            | Self::ConfiguredProviderModelMismatch(_)
            | Self::ConfiguredUnknownProvider { .. }
            | Self::EmptyBindingDefaultModel { .. }
            | Self::BindingMissingProviderDefault { .. } => true,
            Self::AuthBinding(source) => Self::auth_binding_is_configuration_fault(source),
            Self::AuthBindingProviderMismatch { .. }
            | Self::ProviderModelMismatch(_)
            | Self::UnknownProvider { .. }
            | Self::MissingProviderDefault { .. }
            | Self::EmptyExplicitModel => false,
        }
    }

    const fn auth_binding_is_configuration_fault(
        source: &meerkat_core::ConnectionTargetError,
    ) -> bool {
        match source {
            meerkat_core::ConnectionTargetError::MissingRealm
            | meerkat_core::ConnectionTargetError::UnknownRealm(_)
            | meerkat_core::ConnectionTargetError::InvalidRealmId { .. }
            | meerkat_core::ConnectionTargetError::InvalidBindingId { .. }
            | meerkat_core::ConnectionTargetError::BindingInvalid {
                source: meerkat_core::ProviderBindingError::UnknownBinding(_),
                ..
            } => false,
            meerkat_core::ConnectionTargetError::MissingDefaultBinding { .. }
            | meerkat_core::ConnectionTargetError::RealmConfigInvalid { .. }
            | meerkat_core::ConnectionTargetError::BindingInvalid {
                source:
                    meerkat_core::ProviderBindingError::UnknownBackend(_)
                    | meerkat_core::ProviderBindingError::UnknownAuth(_)
                    | meerkat_core::ProviderBindingError::ProviderMismatch { .. }
                    | meerkat_core::ProviderBindingError::UnknownProviderName(_)
                    | meerkat_core::ProviderBindingError::InvalidRealmId { .. },
                ..
            }
            | meerkat_core::ConnectionTargetError::ProviderMismatch { .. }
            | meerkat_core::ConnectionTargetError::RealmChain(_) => true,
        }
    }
}

fn configured_model_for_provider(config: &Config, provider: Provider) -> Option<String> {
    let configured = match provider {
        Provider::Anthropic => Some(&config.models.anthropic),
        Provider::OpenAI => Some(&config.models.openai),
        Provider::Gemini => Some(&config.models.gemini),
        Provider::SelfHosted | Provider::Other => None,
    }?;
    (!configured.is_empty()).then(|| configured.clone())
}

fn resolve_provider_constrained_create_model(
    config: &Config,
    registry: &ModelRegistry,
    provider: Provider,
) -> Result<String, CreateSessionModelResolutionError> {
    if !config.agent.model.is_empty() {
        match registry.entry(&config.agent.model) {
            Some(entry) if entry.provider == provider => return Ok(config.agent.model.clone()),
            // A registered model owned by another provider cannot satisfy the
            // constraint. Continue to the provider-specific default.
            Some(_) => {}
            // An uncatalogued global model remains valid operator intent when
            // a provider or binding supplies its typed owner.
            None => return Ok(config.agent.model.clone()),
        }
    }
    if let Some(model) = configured_model_for_provider(config, provider) {
        return Ok(model);
    }
    registry
        .default_model(provider)
        .map(str::to_string)
        .ok_or(CreateSessionModelResolutionError::MissingProviderDefault { provider })
}

/// Resolve create-session model/provider/binding precedence once for every
/// surface.
///
/// Precedence is explicit model; a named auth binding's model default; then
/// configured global, per-provider, and catalog defaults constrained by the
/// explicit provider or binding provider. A named auth binding is resolved
/// through the effective realm config before model selection, which both
/// infers its provider and validates inherited owner/profile facts. Known
/// provider/model mismatches fail closed.
pub fn resolve_create_session_model(
    config: &Config,
    request: CreateSessionModelResolutionRequest,
) -> Result<CreateSessionModelResolution, CreateSessionModelResolutionError> {
    let registry = config.model_registry(meerkat_models::canonical())?;
    let model_was_explicit = request.model.is_some();
    if request.model.as_ref().is_some_and(String::is_empty) {
        return Err(CreateSessionModelResolutionError::EmptyExplicitModel);
    }

    let resolved_binding = request
        .auth_binding
        .as_ref()
        .map(|binding| meerkat_core::resolve_explicit_auth_binding_target(config, binding))
        .transpose()?;
    let binding_provider = resolved_binding
        .as_ref()
        .map(|target| target.backend.provider);
    if let (Some(explicit_provider), Some(binding_provider), Some(binding)) = (
        request.provider,
        binding_provider,
        resolved_binding.as_ref(),
    ) && explicit_provider != binding_provider
    {
        return Err(
            CreateSessionModelResolutionError::AuthBindingProviderMismatch {
                realm: binding.auth_binding.realm.clone(),
                binding: binding.auth_binding.binding.clone(),
                binding_provider,
                explicit_provider,
            },
        );
    }

    let constrained_provider = request.provider.or(binding_provider);
    let (model, provider_hint) = if let Some(model) = request.model {
        (model, constrained_provider)
    } else if let Some(target) = resolved_binding.as_ref()
        && let Some(model) = target.binding.default_model.clone()
    {
        if model.is_empty() {
            return Err(
                CreateSessionModelResolutionError::EmptyBindingDefaultModel {
                    realm: target.auth_binding.realm.clone(),
                    binding: target.auth_binding.binding.clone(),
                },
            );
        }
        (model, Some(target.backend.provider))
    } else if let Some(provider) = constrained_provider {
        let model = resolve_provider_constrained_create_model(config, &registry, provider)
            .map_err(|error| match (error, resolved_binding.as_ref()) {
                (
                    CreateSessionModelResolutionError::MissingProviderDefault { provider },
                    Some(target),
                ) => CreateSessionModelResolutionError::BindingMissingProviderDefault {
                    realm: target.auth_binding.realm.clone(),
                    binding: target.auth_binding.binding.clone(),
                    provider,
                },
                (error, _) => error,
            })?;
        (model, Some(provider))
    } else if !config.agent.model.is_empty() {
        (config.agent.model.clone(), None)
    } else if let Some((provider, model)) =
        meerkat_models::provider_priority()
            .iter()
            .find_map(|provider| {
                configured_model_for_provider(config, *provider).map(|m| (*provider, m))
            })
    {
        (model, Some(provider))
    } else {
        (meerkat_models::global_default_model().to_string(), None)
    };

    let provider = provider_hint
        .or_else(|| registry.entry(&model).map(|entry| entry.provider))
        .ok_or_else(|| {
            if model_was_explicit {
                CreateSessionModelResolutionError::UnknownProvider {
                    model: model.clone(),
                }
            } else {
                CreateSessionModelResolutionError::ConfiguredUnknownProvider {
                    model: model.clone(),
                }
            }
        })?;
    if let Some(reason) = registry.provider_override_mismatch_reason(provider, &model) {
        return Err(if model_was_explicit {
            CreateSessionModelResolutionError::ProviderModelMismatch(reason)
        } else {
            CreateSessionModelResolutionError::ConfiguredProviderModelMismatch(reason)
        });
    }

    Ok(CreateSessionModelResolution {
        model,
        provider,
        auth_binding: resolved_binding.map(|target| target.auth_binding),
    })
}

/// Resolve the no-hint create-session default model.
///
/// A nonempty `config.agent.model` is explicit operator intent, including a
/// still-supported model that happened to be a historical template default.
/// The default template is now empty, so no frozen string list is consulted.
#[must_use]
pub fn resolve_create_session_default_model(config: &Config) -> String {
    if !config.agent.model.is_empty() {
        return config.agent.model.clone();
    }
    resolve_provider_catalog_default_model(config)
}

/// Tiers 2–3 of [`resolve_create_session_default_model`]: the configured
/// per-provider default walked in catalog [`provider_priority`] order, then the
/// catalog [`global_default_model`] terminal fallback — *without* consulting
/// `config.agent.model`.
///
/// This is the seam for code that already decided to bypass the global knob.
/// Create-session callers should use [`resolve_create_session_model`] so model,
/// provider, and auth-binding precedence remain coupled.
///
/// [`provider_priority`]: meerkat_models::provider_priority
/// [`global_default_model`]: meerkat_models::global_default_model
#[must_use]
pub fn resolve_provider_catalog_default_model(config: &Config) -> String {
    let from_provider_priority = meerkat_models::provider_priority()
        .iter()
        .find_map(|provider| {
            let configured = match provider {
                Provider::Anthropic => &config.models.anthropic,
                Provider::OpenAI => &config.models.openai,
                Provider::Gemini => &config.models.gemini,
                _ => return None,
            };
            (!configured.is_empty()).then(|| configured.clone())
        });
    if let Some(model) = from_provider_priority {
        return model;
    }
    meerkat_models::global_default_model().to_string()
}

#[cfg(any(feature = "session-compaction", test))]
fn model_aware_compaction_config(
    config: &Config,
    registry: &ModelRegistry,
    provider: Provider,
    model: &str,
    build_threshold_override: Option<std::num::NonZeroU64>,
) -> meerkat_core::CompactionConfig {
    let mut compaction: meerkat_core::CompactionConfig = config.compaction.clone().into();
    // The per-build override (e.g. a mob profile's `auto_compact_threshold`)
    // wins over the global config knob and model-aware scaling.
    if let Some(threshold) = build_threshold_override {
        compaction.auto_compact_threshold = threshold.get();
        return compaction;
    }
    let default_threshold = meerkat_core::CompactionConfig::default().auto_compact_threshold;
    if config.compaction.auto_compact_threshold_explicit
        || compaction.auto_compact_threshold != default_threshold
    {
        return compaction;
    }

    if let Some(context_window) = registry
        .entry_for_provider(provider, model)
        .and_then(|entry| entry.context_window)
    {
        // The static default is intentionally conservative for unknown models.
        // Cataloged large-context models should compact near the model window,
        // with enough headroom for the active turn's output.
        let context_window = u64::from(context_window);
        if context_window > 0 {
            compaction.auto_compact_threshold = context_window.saturating_mul(4) / 5;
        }
    }

    compaction
}

#[cfg(not(target_arch = "wasm32"))]
fn provider_web_search_enabled(config: &Config, provider: Provider) -> bool {
    match provider {
        Provider::Anthropic => config.provider_tools.anthropic.web_search,
        Provider::OpenAI => config.provider_tools.openai.web_search,
        Provider::Gemini => config.provider_tools.gemini.google_search,
        _ => false,
    }
}

/// Construct a [`ProviderRuntimeRegistry`] populated with the
/// feature-gated per-provider runtimes from the per-provider crates.
/// Private to the factory — callers use the registry `AgentFactory`
/// owns on itself. Not exposed as a free helper (dogma §65 forbids
/// a canonical-seam-bypass "populate on demand" path).
fn build_provider_registry() -> meerkat_llm_core::provider_runtime::ProviderRuntimeRegistry {
    #[allow(unused_mut)]
    let mut r = meerkat_llm_core::provider_runtime::ProviderRuntimeRegistry::empty();
    #[cfg(feature = "anthropic")]
    {
        r = r.with_runtime(Arc::new(meerkat_anthropic::AnthropicProviderRuntime));
    }
    #[cfg(feature = "openai")]
    {
        r = r.with_runtime(Arc::new(meerkat_openai::OpenAiProviderRuntime));
        r = r.with_runtime(Arc::new(meerkat_providers::SelfHostedProviderRuntime));
    }
    #[cfg(feature = "gemini")]
    {
        r = r.with_runtime(Arc::new(meerkat_gemini::GoogleProviderRuntime));
    }
    r
}

/// Return the canonical string key for a provider.
pub fn provider_key(provider: Provider) -> &'static str {
    provider.as_str()
}

#[cfg(not(target_arch = "wasm32"))]
struct UnavailableImageGenerationExecutor;

#[cfg(not(target_arch = "wasm32"))]
#[async_trait::async_trait]
impl meerkat_llm_core::ImageGenerationExecutor for UnavailableImageGenerationExecutor {
    async fn execute_image_generation(
        &self,
        _request: meerkat_llm_core::ProviderImageGenerationRequest,
    ) -> Result<meerkat_llm_core::ProviderImageGenerationOutput, meerkat_llm_core::LlmError> {
        Err(meerkat_llm_core::LlmError::InvalidRequest {
            message: "image generation is available, but no image provider credential or executor \
                      could be resolved for this session; configure an OpenAI or Gemini image \
                      binding, or pass an image-generation executor override"
                .to_string(),
        })
    }
}

#[cfg(feature = "openai")]
struct SelfHostedClientSpec {
    server_id: String,
    mode: OpenAiCompatibleMode,
    remote_model: String,
    base_url: String,
    supports_temperature: bool,
    supports_thinking: bool,
    supports_reasoning: bool,
    supports_image_tool_results: bool,
}

struct SelfHostedClientBuild {
    client: Arc<dyn LlmClient>,
    durable_auth_binding: Option<AuthBindingRef>,
}

#[cfg(not(target_arch = "wasm32"))]
struct RoutingImageGenerationExecutor {
    executors: BTreeMap<String, Arc<dyn meerkat_llm_core::ImageGenerationExecutor>>,
}

#[cfg(not(target_arch = "wasm32"))]
impl RoutingImageGenerationExecutor {
    fn new(
        executors: BTreeMap<String, Arc<dyn meerkat_llm_core::ImageGenerationExecutor>>,
    ) -> Self {
        Self { executors }
    }

    fn provider_key(plan: &meerkat_core::GenerateImageExecutionPlan) -> &str {
        plan.provider.0.as_str()
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[async_trait::async_trait]
impl meerkat_llm_core::ImageGenerationExecutor for RoutingImageGenerationExecutor {
    async fn execute_image_generation(
        &self,
        request: meerkat_llm_core::ProviderImageGenerationRequest,
    ) -> Result<meerkat_llm_core::ProviderImageGenerationOutput, meerkat_llm_core::LlmError> {
        let provider = Self::provider_key(&request.execution_plan);
        let executor = self.executors.get(provider).ok_or_else(|| {
            meerkat_llm_core::LlmError::InvalidRequest {
                message: format!("no image generation executor configured for provider {provider}"),
            }
        })?;
        executor.execute_image_generation(request).await
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod image_generation_executor_routing_tests {
    use super::RoutingImageGenerationExecutor;
    use async_trait::async_trait;
    use meerkat_llm_core::{
        ImageGenerationExecutor, LlmError, ProviderImageGenerationOutput,
        ProviderImageGenerationRequest,
    };
    use std::collections::BTreeMap;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    struct CountingImageExecutor {
        calls: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl ImageGenerationExecutor for CountingImageExecutor {
        async fn execute_image_generation(
            &self,
            _request: ProviderImageGenerationRequest,
        ) -> Result<ProviderImageGenerationOutput, LlmError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Err(LlmError::InvalidRequest {
                message: "executor should not be called for another provider".to_string(),
            })
        }
    }

    fn hosted_openai_request() -> ProviderImageGenerationRequest {
        serde_json::from_value(serde_json::json!({
            "operation_id": "00000000-0000-0000-0000-000000000101",
            "model": "gpt-5.4",
            "generate_request": {
                "intent": {
                    "intent": "generate",
                    "prompt": {"content": "draw a square"},
                    "prompt_source": {
                        "source": "user_provided",
                        "message_id": "00000000-0000-0000-0000-000000000102"
                    },
                    "reference_images": []
                },
                "target": {"target": "auto"},
                "size": {"size": "square1024"},
                "quality": "low",
                "format": "png",
                "count": 1
            },
            "execution_plan": {
                "provider": "openai",
                "backend": "hosted_tool",
                "max_count": 1,
                "capabilities": {
                    "hosted_image_generation_tool": true,
                    "native_image_output": false,
                    "custom_tools": false,
                    "image_search_grounding": false,
                    "image_continuity_tokens": "unsupported"
                },
                "requires_scoped_override": false,
                "provider_plan": {
                    "tool_name": "image_generation",
                    "model": "gpt-image-2",
                    "output": {
                        "size": "square1024",
                        "quality": "low",
                        "output_format": "png"
                    },
                    "provider_params": {}
                }
            },
            "projected_messages": []
        }))
        .expect("hosted OpenAI image request")
    }

    #[tokio::test]
    async fn single_provider_router_does_not_dispatch_openai_plan_to_gemini_executor() {
        let calls = Arc::new(AtomicUsize::new(0));
        let mut executors: BTreeMap<String, Arc<dyn ImageGenerationExecutor>> = BTreeMap::new();
        executors.insert(
            "gemini".to_string(),
            Arc::new(CountingImageExecutor {
                calls: Arc::clone(&calls),
            }),
        );
        let router = RoutingImageGenerationExecutor::new(executors);

        let err = router
            .execute_image_generation(hosted_openai_request())
            .await
            .expect_err("OpenAI plan should not dispatch to a Gemini-only executor");

        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert!(
            err.to_string()
                .contains("no image generation executor configured for provider openai"),
            "unexpected error: {err}"
        );
    }
}

#[cfg(not(target_arch = "wasm32"))]
struct CompositeImageGenerationPlanner {
    profiles: Vec<Arc<dyn meerkat_core::ImageGenerationProviderProfile>>,
    /// Configured default provider for `Auto` targets. When set, `Auto`
    /// resolves against this provider instead of the session's typed LLM
    /// provider identity (`SessionModelRoutingStatus.session_provider`).
    auto_target_provider: Option<meerkat_core::Provider>,
}

#[cfg(not(target_arch = "wasm32"))]
impl CompositeImageGenerationPlanner {
    fn new(profiles: Vec<Arc<dyn meerkat_core::ImageGenerationProviderProfile>>) -> Self {
        Self {
            profiles,
            auto_target_provider: None,
        }
    }

    fn with_auto_target_provider(
        mut self,
        auto_target_provider: Option<meerkat_core::Provider>,
    ) -> Self {
        self.auto_target_provider = auto_target_provider;
        self
    }

    fn profile_for_provider(
        &self,
        provider: meerkat_core::Provider,
    ) -> Option<&Arc<dyn meerkat_core::ImageGenerationProviderProfile>> {
        self.profiles
            .iter()
            .find(|profile| profile.canonical_provider() == provider)
    }

    fn profile_for_provider_id(
        &self,
        provider: &str,
    ) -> Option<(
        meerkat_core::Provider,
        &Arc<dyn meerkat_core::ImageGenerationProviderProfile>,
    )> {
        self.profiles
            .iter()
            .find(|profile| profile.matches_provider_id(provider))
            .map(|profile| (profile.canonical_provider(), profile))
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl meerkat_core::ImageGenerationPlanner for CompositeImageGenerationPlanner {
    fn resolve_image_generation_plan(
        &self,
        status: &meerkat_core::SessionModelRoutingStatus,
        operation_id: meerkat_core::ImageOperationId,
        request: &meerkat_core::GenerateImageRequest,
    ) -> Result<meerkat_core::ImageGenerationResolvedPlan, meerkat_core::ImageOperationDenialReason>
    {
        use meerkat_core::{
            ImageContinuityTokenSupport, ImageGenerationTargetCapabilities,
            ImageGenerationTargetPreference, ImageOperationDenialReason,
        };

        let capabilities = ImageGenerationTargetCapabilities {
            hosted_image_generation_tool: true,
            native_image_output: true,
            custom_tools: false,
            image_search_grounding: false,
            image_continuity_tokens: ImageContinuityTokenSupport::Unsupported,
        };
        let one = std::num::NonZeroU32::MIN;
        if request.count > one {
            return Err(ImageOperationDenialReason::UnsupportedCount);
        }

        let (target_provider, profile) = match &request.target {
            ImageGenerationTargetPreference::Auto => {
                // A configured image-generation provider is the declared
                // `Auto` default; only absent that does `Auto` fall back to
                // the session's typed LLM provider identity. The provider
                // fact is owned by the resolved `SessionLlmIdentity` and
                // projected into the routing status by the machine (hydrated
                // at construction, recommitted on hot-swap) — it is never
                // re-derived here from the model string through the built-in
                // catalog, which has no row for `ModelRegistry`-owned custom
                // models.
                let provider = self
                    .auto_target_provider
                    .or(status.session_provider)
                    .ok_or(ImageOperationDenialReason::UnsupportedTarget)?;
                let profile = self
                    .profile_for_provider(provider)
                    .ok_or(ImageOperationDenialReason::UnsupportedTarget)?;
                (provider, profile)
            }
            ImageGenerationTargetPreference::ProviderDefault { provider }
            | ImageGenerationTargetPreference::Model { provider, .. } => self
                .profile_for_provider_id(provider.0.as_str())
                .ok_or(ImageOperationDenialReason::UnsupportedTarget)?,
        };
        let requested_model = match &request.target {
            ImageGenerationTargetPreference::Model { model, .. } => {
                meerkat_models::image_generation_model(target_provider, model.as_str())
                    .ok_or(ImageOperationDenialReason::UnsupportedTarget)?
            }
            _ => meerkat_models::default_image_generation_model(target_provider)
                .ok_or(ImageOperationDenialReason::UnsupportedTarget)?,
        };

        let resolution = profile.resolve_execution_plan(
            operation_id,
            &requested_model,
            request,
            capabilities,
            one,
        )?;
        let requires_scoped_override = resolution.execution_plan.requires_scoped_override();
        let machine_routing_realtime_capable = if requires_scoped_override {
            model_realtime_capable(target_provider, resolution.provider_call_model.as_str())
        } else {
            status
                .session_provider
                .map(|provider| model_realtime_capable(provider, status.effective_model.as_str()))
                .unwrap_or(false)
        };
        let machine_routing_model = if resolution.execution_plan.requires_scoped_override() {
            resolution.provider_call_model.clone()
        } else {
            status.effective_model.clone()
        };
        Ok(meerkat_core::ImageGenerationResolvedPlan {
            provider_model: resolution.provider_call_model,
            machine_routing_model,
            machine_routing_realtime_capable,
            execution_plan: resolution.execution_plan,
            projected_messages: image_projection_messages(request),
        })
    }

    fn infer_provider_for_model(&self, model: &str) -> Option<meerkat_core::ProviderId> {
        let provider = meerkat_models::image_generation_provider_for_model(model)?;
        self.profile_for_provider(provider)?;
        Some(meerkat_core::ProviderId::new(provider.as_str()))
    }

    fn provider_documentation(&self) -> Vec<String> {
        self.profiles
            .iter()
            .filter_map(|profile| profile.image_generation_documentation())
            .map(str::to_owned)
            .collect()
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn image_projection_messages(
    request: &meerkat_core::GenerateImageRequest,
) -> Vec<meerkat_core::Message> {
    let text = match &request.intent {
        meerkat_core::ImageGenerationIntent::Generate { prompt, .. } => prompt.content.clone(),
        meerkat_core::ImageGenerationIntent::Edit { instruction, .. } => {
            instruction.content.clone()
        }
    };
    vec![meerkat_core::Message::User(
        meerkat_core::UserMessage::text(text),
    )]
}

#[cfg(not(target_arch = "wasm32"))]
fn model_realtime_capable(provider: meerkat_core::Provider, model: &str) -> bool {
    meerkat_models::capabilities_for(provider, model)
        .map(|caps| caps.realtime)
        .unwrap_or(false)
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
    meerkat_models::capabilities_for(meerkat_core::Provider::OpenAI, model)
        .map(|caps| caps.realtime)
        .unwrap_or(false)
}

/// Typed attachment state of the factory's provider-auth persistence.
///
/// One owner for the three construction outcomes: no store (detached),
/// an open store (attached), or a default store whose open FAILED. The
/// open failure is a fault, not an absence of credentials — it is held
/// typed and propagated at the resolution seam
/// ([`AgentFactory::resolution_token_store`]) instead of being collapsed
/// to "no store" and laundered into `AuthError::InteractiveLoginRequired`.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone)]
enum ProviderAuthPersistenceAttachment {
    /// No persistence attached (minimal builds, callers that deliberately
    /// resolve without persisted credentials).
    Detached,
    /// The complete token-vault + refresh-authority capability is attached.
    Attached(meerkat_providers::auth_store::ProviderAuthPersistence),
    /// The default token store failed to open at factory construction.
    OpenFailed(Arc<meerkat_providers::auth_store::TokenStoreError>),
}

/// Open the native provider-auth persistence pair from one backend decision.
///
/// A persisted token store and its refresh coordinator are one capability:
/// deriving both from the same
/// [`meerkat_providers::auth_store::TokenStoreBackend`] keeps every factory that
/// can consume rotating OAuth tokens on the canonical cross-process lock
/// authority. A backend-open fault remains attached and is surfaced at
/// resolution time; it is never converted into process-local refresh.
#[cfg(not(target_arch = "wasm32"))]
fn open_provider_auth_persistence(
    backend: Result<
        meerkat_providers::auth_store::TokenStoreBackend,
        meerkat_providers::auth_store::TokenStoreError,
    >,
) -> ProviderAuthPersistenceAttachment {
    match backend.and_then(|backend| backend.open_with_refresh_authority()) {
        Ok(persistence) => ProviderAuthPersistenceAttachment::Attached(persistence),
        Err(error) => ProviderAuthPersistenceAttachment::OpenFailed(Arc::new(error)),
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
    pub enable_workgraph: bool,
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
    /// Provider-auth persistence attachment used by the provider-runtime registry
    /// when resolving OAuth-backed bindings (Claude.ai / ChatGPT / Google
    /// Code Assist). When detached, OAuth bindings surface
    /// `AuthError::InteractiveLoginRequired` — CLI / REST / RPC surfaces
    /// attach an `AutoTokenStore` during construction so the whole stack
    /// reads the same persisted credentials. When the default store failed
    /// to open at construction, the typed open fault is held here and
    /// propagates at the first provider resolution instead of being
    /// laundered into a missing-credential outcome.
    #[cfg(not(target_arch = "wasm32"))]
    provider_auth_persistence: ProviderAuthPersistenceAttachment,
    /// External auth resolvers keyed by their typed
    /// [`meerkat_core::ExternalResolverId`] identity. Merged into
    /// `ResolverEnvironment.external_resolvers` during `build_agent`.
    /// The WASM runtime registers a `WasmExternalAuthResolver` under its
    /// typed resolver id so realm bindings configured with
    /// `CredentialSourceSpec::ExternalResolver { handle }` delegate
    /// credential resolution to the JS host's OAuth flow.
    pub external_auth_resolvers: BTreeMap<
        meerkat_core::ExternalResolverId,
        Arc<dyn meerkat_providers::ExternalAuthResolverHandle>,
    >,
    /// Provider runtime registry, owned by the factory. Populated with
    /// the feature-gated per-provider runtimes
    /// (`AnthropicProviderRuntime`, `OpenAiProviderRuntime`,
    /// `GoogleProviderRuntime`) at construction so dispatch is
    /// deterministic and off the canonical-seam-bypass path
    /// (dogma §65).
    provider_registry: Arc<meerkat_llm_core::provider_runtime::ProviderRuntimeRegistry>,
    /// Default machine handle for generated-image planning/routing.
    pub image_generation_machine:
        Option<Arc<dyn meerkat_tools::builtin::image_generation::ImageGenerationMachine>>,
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
            .field("enable_workgraph", &self.enable_workgraph)
            .field("enable_mob", &self.enable_mob);
        #[cfg(feature = "comms")]
        d.field("enable_comms", &self.enable_comms);
        #[cfg(feature = "skills")]
        d.field("skill_source", &self.skill_source.as_ref().map(|_| ".."));
        d.field("custom_store", &self.custom_store.as_ref().map(|_| ".."));
        d.field("mob_tools", &self.mob_tools.is_some());
        d.field(
            "image_generation_machine",
            &self.image_generation_machine.is_some(),
        );
        #[cfg(feature = "comms")]
        d.field("comms_runtime", &self.comms_runtime.is_some());
        d.finish()
    }
}

impl AgentFactory {
    fn resolve_realm_binding_for_provider(
        config: &Config,
        provider: Provider,
        auth_binding: Option<&AuthBindingRef>,
        preferred_realm: Option<&RealmId>,
    ) -> Result<(RealmConnectionSet, String, AuthBindingRef), meerkat_core::ConnectionTargetError>
    {
        let target = meerkat_core::resolve_auth_binding_or_default_for_provider(
            config,
            provider,
            auth_binding,
            preferred_realm,
            true,
        )?;
        Ok((
            target.realm,
            target.auth_binding.binding.to_string(),
            target.auth_binding,
        ))
    }

    fn resolve_realm_binding_candidates_for_provider(
        config: &Config,
        provider: Provider,
        auth_binding: Option<&AuthBindingRef>,
        preferred_realm: Option<&RealmId>,
    ) -> Result<Vec<meerkat_core::ResolvedConnectionTarget>, meerkat_core::ConnectionTargetError>
    {
        meerkat_core::resolve_auth_binding_candidates_for_provider(
            config,
            provider,
            auth_binding,
            preferred_realm,
            true,
        )
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn selected_binding_id_for_provider(
        realm: &RealmConnectionSet,
        provider: Provider,
        purpose: &str,
        default_label: &str,
    ) -> Result<String, String> {
        let mut provider_bindings = Vec::new();
        // The per-provider default is the binding carrying the typed
        // `provider_default` marker — not a `default_<provider>` name match.
        let mut typed_provider_default: Option<&str> = None;
        for (binding_id, binding) in &realm.bindings {
            let backend = realm
                .backends
                .get(&binding.backend_profile)
                .ok_or_else(|| {
                    format!(
                        "{purpose} for provider '{}' is unavailable in selected realm '{}': binding '{}:{}' references unknown backend '{}'",
                        provider.as_str(),
                        realm.realm_id,
                        realm.realm_id,
                        binding_id,
                        binding.backend_profile,
                    )
                })?;
            let auth = realm
                .auth_profiles
                .get(&binding.auth_profile)
                .ok_or_else(|| {
                    format!(
                        "{purpose} for provider '{}' is unavailable in selected realm '{}': binding '{}:{}' references unknown auth '{}'",
                        provider.as_str(),
                        realm.realm_id,
                        realm.realm_id,
                        binding_id,
                        binding.auth_profile,
                    )
                })?;
            if backend.provider == provider && auth.provider == provider {
                provider_bindings.push(binding_id.as_str());
                if binding.provider_default && typed_provider_default.is_none() {
                    typed_provider_default = Some(binding_id.as_str());
                }
            }
        }

        if let Some(default_binding) = realm.default_binding.as_deref()
            && provider_bindings.contains(&default_binding)
        {
            return Ok(default_binding.to_string());
        }

        if let Some(typed_default) = typed_provider_default {
            return Ok(typed_default.to_string());
        }

        match provider_bindings.as_slice() {
            [binding_id] => Ok((*binding_id).to_string()),
            [] => {
                if let Some(default_binding) = realm.default_binding.as_deref() {
                    match realm.lookup_binding(default_binding) {
                        Ok((_binding, backend, auth)) => Err(format!(
                            "{purpose} for provider '{}' is unavailable in selected realm '{}': binding '{}:{}' resolves backend={:?} auth={:?}, expected provider {:?}",
                            provider.as_str(),
                            realm.realm_id,
                            realm.realm_id,
                            default_binding,
                            backend.provider,
                            auth.provider,
                            provider,
                        )),
                        Err(source) => Err(format!(
                            "{purpose} for provider '{}' is unavailable in selected realm '{}': selected realm default binding '{}' is invalid: {source}",
                            provider.as_str(),
                            realm.realm_id,
                            default_binding,
                        )),
                    }
                } else {
                    Err(format!(
                        "{purpose} for provider '{}' is unavailable in selected realm '{}': selected realm has no default binding and no binding for provider '{}'",
                        provider.as_str(),
                        realm.realm_id,
                        provider.as_str(),
                    ))
                }
            }
            many => Err(format!(
                "{purpose} for provider '{}' is unavailable in selected realm '{}': selected realm has multiple bindings for provider '{}' ({}) and no unambiguous {default_label} default",
                provider.as_str(),
                realm.realm_id,
                provider.as_str(),
                many.join(", "),
            )),
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn selected_image_binding_id_for_provider(
        realm: &RealmConnectionSet,
        provider: Provider,
    ) -> Result<String, String> {
        Self::selected_binding_id_for_provider(realm, provider, "image credential binding", "image")
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn selected_web_search_binding_id_for_provider(
        realm: &RealmConnectionSet,
        provider: Provider,
    ) -> Result<String, String> {
        Self::selected_binding_id_for_provider(
            realm,
            provider,
            "web_search credential binding",
            "web_search",
        )
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn resolve_selected_binding_for_provider(
        config: &Config,
        provider: Provider,
        selected_realm: RealmId,
        purpose: &str,
        // The image/web_search selector is no longer threaded here: the
        // chain-aware connection resolver applies the one canonical
        // default-selection policy (default_binding > provider_default >
        // single) per chain member, so a separate purpose-labelled selector
        // would be a second policy. Kept in the signature to preserve the
        // image/web_search call sites unchanged.
        _select_binding_id: fn(&RealmConnectionSet, Provider) -> Result<String, String>,
    ) -> Result<(RealmConnectionSet, String, AuthBindingRef), String> {
        // Walk the selected realm's parent chain (explicit realm => head
        // required, fail-closed, NO env fallback) and stamp the OWNING realm,
        // so an image/web_search binding inherited from a parent/global
        // resolves at its owner (decision A) instead of the consuming realm.
        let target = meerkat_core::resolve_realm_binding_target_for_provider(
            config,
            provider,
            Some(&selected_realm),
            None,
            None,
            None,
            /* allow_env_default = */ false,
        )
        .map_err(|err| {
            format!(
                "{purpose} for provider '{}' is unavailable in selected realm '{}': {err}",
                provider.as_str(),
                selected_realm.as_str(),
            )
        })?;
        let binding_id = target.auth_binding.binding.as_str().to_string();
        Ok((target.realm, binding_id, target.auth_binding))
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn resolve_selected_image_binding_for_provider(
        config: &Config,
        provider: Provider,
        selected_realm: RealmId,
    ) -> Result<(RealmConnectionSet, String, AuthBindingRef), String> {
        Self::resolve_selected_binding_for_provider(
            config,
            provider,
            selected_realm,
            "image credential binding",
            Self::selected_image_binding_id_for_provider,
        )
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn resolve_selected_web_search_binding_for_provider(
        config: &Config,
        provider: Provider,
        selected_realm: RealmId,
    ) -> Result<(RealmConnectionSet, String, AuthBindingRef), String> {
        Self::resolve_selected_binding_for_provider(
            config,
            provider,
            selected_realm,
            "web_search credential binding",
            Self::selected_web_search_binding_id_for_provider,
        )
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn resolve_image_binding_for_provider(
        config: &Config,
        provider: Provider,
        selected_realm: Option<&RealmId>,
    ) -> Result<(RealmConnectionSet, String, AuthBindingRef), String> {
        // The `String` error here is a display-only reason that feeds the
        // fail-closed `CapabilityUnavailable` / silent-inherit policy of the
        // image executor path; the typed `ConnectionTargetError` is lowered
        // exactly once at this seam.
        let Some(selected_realm) = selected_realm else {
            return Self::resolve_realm_binding_for_provider(config, provider, None, None)
                .map_err(|e| e.to_string());
        };
        if selected_realm.is_env_default() {
            return Self::resolve_realm_binding_for_provider(config, provider, None, None)
                .map_err(|e| e.to_string());
        }
        if !config.realm.contains_key(selected_realm.as_str()) {
            return Self::resolve_realm_binding_for_provider(
                config,
                provider,
                None,
                Some(selected_realm),
            )
            .map_err(|e| e.to_string());
        }
        Self::resolve_selected_image_binding_for_provider(config, provider, selected_realm.clone())
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn resolve_web_search_binding_for_provider(
        config: &Config,
        provider: Provider,
        selected_realm: Option<&RealmId>,
    ) -> Result<(RealmConnectionSet, String, AuthBindingRef), String> {
        // The `String` error here is a display-only reason that feeds the
        // fail-closed `CapabilityUnavailable` / silent-inherit policy of the
        // web-search executor path; the typed `ConnectionTargetError` is
        // lowered exactly once at this seam.
        let Some(selected_realm) = selected_realm else {
            return Self::resolve_realm_binding_for_provider(config, provider, None, None)
                .map_err(|e| e.to_string());
        };
        if selected_realm.is_env_default() {
            return Self::resolve_realm_binding_for_provider(config, provider, None, None)
                .map_err(|e| e.to_string());
        }
        if !config.realm.contains_key(selected_realm.as_str()) {
            return Self::resolve_realm_binding_for_provider(
                config,
                provider,
                None,
                Some(selected_realm),
            )
            .map_err(|e| e.to_string());
        }
        Self::resolve_selected_web_search_binding_for_provider(
            config,
            provider,
            selected_realm.clone(),
        )
    }

    /// Build the facade-owned OpenAI realtime session factory.
    ///
    /// The returned factory holds NO credential material: on every
    /// `open_session` / `open_live_adapter` / `attach_external_session` it
    /// reads the CURRENT config from `config_source`, resolves the owning
    /// session's `SessionLlmIdentity.auth_binding` through the same
    /// owning-realm resolver seam as the text path
    /// ([`Self::build_llm_client_for_identity_with_auth_lease`]), and mints
    /// the provider adapter via the registry-owned
    /// `ProviderRuntime::build_realtime_session_factory` seam. Resolution or
    /// gating failures fail the open closed with a typed error — there is no
    /// fallback to a process-default credential.
    #[cfg(all(
        feature = "openai",
        feature = "openai-realtime",
        not(target_arch = "wasm32")
    ))]
    pub fn build_openai_realtime_session_factory(
        &self,
        config_source: Arc<
            dyn crate::session_runtime::realtime_credentials::RealtimeCurrentConfigSource,
        >,
    ) -> Arc<dyn meerkat_client::RealtimeSessionFactory> {
        Arc::new(
            crate::session_runtime::realtime_credentials::PerOpenCredentialRealtimeSessionFactory::new(
                self.clone(),
                config_source,
            ),
        )
    }

    /// Resolve the provider-owned realtime session factory for `identity`
    /// against the CURRENT `config`, honoring the identity's `auth_binding`
    /// through the owning resolver seam — the realtime mirror of
    /// [`Self::build_llm_client_for_identity_with_auth_lease`].
    ///
    /// Credential material never surfaces here: the provider runtime applies
    /// its realtime backend/auth gating and constructs the concrete adapter
    /// behind `ProviderRuntimeRegistry::build_realtime_session_factory`.
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn resolve_realtime_session_factory_for_identity(
        &self,
        config: &Config,
        identity: &SessionLlmIdentity,
    ) -> Result<Arc<dyn meerkat_client::RealtimeSessionFactory>, FactoryError> {
        let (realm, _binding_id, auth_binding) = Self::resolve_realm_binding_for_provider(
            config,
            identity.provider,
            identity.auth_binding.as_ref(),
            None,
        )
        .map_err(FactoryError::ConnectionTarget)?;
        let mut env = meerkat_providers::ResolverEnvironment::with_process_env();
        if let Some(persistence) = self.resolution_provider_auth_persistence()? {
            env = env.with_provider_auth_persistence(persistence);
        }
        for (handle, resolver) in &self.external_auth_resolvers {
            env = env.with_external_resolver(handle.clone(), resolver.clone());
        }
        let connection = self
            .provider_registry
            .resolve(&realm, &auth_binding, &env)
            .await
            .map_err(FactoryError::ProviderAuth)?;
        self.provider_registry
            .build_realtime_session_factory(connection)
            .map_err(FactoryError::ClientBuild)
    }

    /// Build a fallback web-search executor for a provider whose active model
    /// lacks native search.
    ///
    /// `explicit` carries the resolved tool-category intent: when the caller
    /// explicitly enabled web search (`override_web_search == Enable`), every
    /// branch that cannot provision the tool fails closed with
    /// [`BuildAgentError::CapabilityUnavailable`] rather than silently
    /// returning `Ok(None)`. On an `Inherit`-default build (`explicit == false`)
    /// the same branches degrade to `Ok(None)` by design.
    #[cfg(not(target_arch = "wasm32"))]
    async fn build_web_search_executor(
        &self,
        config: &Config,
        registry: &ModelRegistry,
        search_provider: Provider,
        selected_realm: Option<&RealmId>,
        runtime_build_mode: &RuntimeBuildMode,
        explicit: bool,
    ) -> Result<Option<Arc<dyn meerkat_llm_core::WebSearchExecutor>>, BuildAgentError> {
        // Fail closed on an explicit enable; degrade silently on inherit.
        let unavailable = |reason: String| -> Result<
            Option<Arc<dyn meerkat_llm_core::WebSearchExecutor>>,
            BuildAgentError,
        > {
            if explicit {
                Err(BuildAgentError::CapabilityUnavailable {
                    capability: "web_search",
                    reason,
                })
            } else {
                Ok(None)
            }
        };
        if !provider_web_search_enabled(config, search_provider) {
            return unavailable(format!(
                "provider {search_provider:?} web search is not enabled in config"
            ));
        }
        let Some(model) = registry.default_model(search_provider).map(str::to_string) else {
            return unavailable(format!("no default model for provider {search_provider:?}"));
        };
        let Some(profile) = registry.profile_for_provider(search_provider, &model) else {
            return unavailable(format!(
                "no profile for provider {search_provider:?} model '{model}'"
            ));
        };
        if !profile.supports_web_search {
            return unavailable(format!(
                "model '{model}' for provider {search_provider:?} does not support web search"
            ));
        }

        let Ok((realm, _binding_id, auth_binding)) =
            Self::resolve_web_search_binding_for_provider(config, search_provider, selected_realm)
        else {
            return unavailable(format!(
                "failed to resolve web-search auth binding for provider {search_provider:?}"
            ));
        };
        let mut env = meerkat_providers::ResolverEnvironment::with_process_env();
        // A faulted credential backend is a hard typed fault, never folded
        // into the inherit-mode `Ok(None)` degradation.
        if let Some(persistence) = self
            .resolution_provider_auth_persistence()
            .map_err(BuildAgentError::LlmClient)?
        {
            env = env.with_provider_auth_persistence(persistence);
        }
        if let RuntimeBuildMode::SessionOwned(bindings) = runtime_build_mode
            && !auth_binding.is_env_default()
        {
            env = env.with_auth_lease_handle(bindings.auth_lease().clone());
        }
        for (handle, resolver) in &self.external_auth_resolvers {
            env = env.with_external_resolver(handle.clone(), resolver.clone());
        }
        let connection = match self
            .provider_registry
            .resolve(&realm, &auth_binding, &env)
            .await
        {
            Ok(connection) => connection,
            Err(err) => {
                return unavailable(format!(
                    "web-search connection resolution failed for provider {search_provider:?}: {err}"
                ));
            }
        };
        if let RuntimeBuildMode::SessionOwned(bindings) = runtime_build_mode
            && !auth_binding.is_env_default()
        {
            Self::publish_auth_lease(bindings.auth_lease(), &auth_binding, &connection)
                .map_err(BuildAgentError::LlmClient)?;
        }
        let client = match self.provider_registry.build_client(connection) {
            Ok(client) => client,
            Err(err) => {
                return unavailable(format!(
                    "web-search client build failed for provider {search_provider:?}: {err}"
                ));
            }
        };
        let adapted: Arc<dyn AgentLlmClient> = Arc::new(
            LlmClientAdapter::try_for_provider_identity(client, model.clone(), search_provider)
                .map_err(|error| BuildAgentError::Config(error.to_string()))?,
        );

        let executor: Arc<dyn meerkat_llm_core::WebSearchExecutor> = match search_provider {
            #[cfg(feature = "openai")]
            Provider::OpenAI => {
                Arc::new(meerkat_openai::OpenAiWebSearchExecutor::new(model, adapted))
            }
            #[cfg(feature = "gemini")]
            Provider::Gemini => {
                Arc::new(meerkat_gemini::GeminiWebSearchExecutor::new(model, adapted))
            }
            #[cfg(feature = "anthropic")]
            Provider::Anthropic => Arc::new(meerkat_anthropic::AnthropicWebSearchExecutor::new(
                model, adapted,
            )),
            _ => {
                return unavailable(format!(
                    "no web-search executor adapter for provider {search_provider:?}"
                ));
            }
        };
        Ok(Some(executor))
    }

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
            enable_workgraph: false,
            enable_mob: false,
            #[cfg(feature = "skills")]
            skill_source: None,
            custom_store: None,
            provider_registry: Arc::new(build_provider_registry()),
            mob_tools: None,
            #[cfg(feature = "comms")]
            comms_runtime: None,
            #[cfg(not(target_arch = "wasm32"))]
            provider_auth_persistence: ProviderAuthPersistenceAttachment::Detached,
            external_auth_resolvers: BTreeMap::new(),
            image_generation_machine: None,
        }
    }

    /// Create a new factory with the required session store path.
    ///
    /// The default file-backed TokenStore is attached when it opens cleanly.
    /// Its backend-derived file-lock refresh coordinator is attached in the
    /// same construction step, so separate native processes never fall back to
    /// independent in-memory refresh ownership for those persisted tokens.
    /// OAuth-backed bindings read persisted tokens written by `rkat auth login`
    /// and REST/RPC OAuth completion handlers out of the box. If the default
    /// TokenStore fails to open (for example a corrupt credential backend),
    /// the typed open fault is retained and the first provider resolution
    /// fails with `FactoryError::TokenStore` — never silently degraded to
    /// `InteractiveLoginRequired`.
    pub fn new(store_path: impl Into<PathBuf>) -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        let provider_auth_persistence = open_provider_auth_persistence(
            meerkat_providers::auth_store::TokenStoreBackend::default_auto(),
        );
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
            enable_workgraph: false,
            enable_mob: false,
            #[cfg(feature = "skills")]
            skill_source: None,
            custom_store: None,
            mob_tools: None,
            #[cfg(feature = "comms")]
            comms_runtime: None,
            #[cfg(not(target_arch = "wasm32"))]
            provider_auth_persistence,
            external_auth_resolvers: BTreeMap::new(),
            provider_registry: Arc::new(build_provider_registry()),
            image_generation_machine: None,
        }
    }

    /// Attach one backend-opened provider-auth persistence capability.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn with_provider_auth_persistence(
        mut self,
        persistence: meerkat_providers::auth_store::ProviderAuthPersistence,
    ) -> Self {
        self.provider_auth_persistence = ProviderAuthPersistenceAttachment::Attached(persistence);
        self
    }

    /// Detach any persistent `TokenStore` from this factory. Callers that
    /// deliberately resolve without persisted credentials (e.g. tests
    /// pinning the no-store path) use this instead of poking the typed
    /// attachment state directly.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn without_provider_auth_persistence(mut self) -> Self {
        self.provider_auth_persistence = ProviderAuthPersistenceAttachment::Detached;
        self
    }

    /// Resolve the complete persistence capability attached to this factory.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn resolution_provider_auth_persistence(
        &self,
    ) -> Result<Option<meerkat_providers::auth_store::ProviderAuthPersistence>, FactoryError> {
        match &self.provider_auth_persistence {
            ProviderAuthPersistenceAttachment::Detached => Ok(None),
            ProviderAuthPersistenceAttachment::Attached(persistence) => {
                Ok(Some(persistence.clone()))
            }
            ProviderAuthPersistenceAttachment::OpenFailed(err) => {
                Err(FactoryError::TokenStore(Arc::clone(err)))
            }
        }
    }

    /// Resolve the token store to attach to a `ResolverEnvironment`.
    ///
    /// `Detached` → `Ok(None)`, `Attached` → `Ok(Some(store))`, and a
    /// retained default-store open failure propagates as the typed
    /// [`FactoryError::TokenStore`] fault — provider resolution with a
    /// faulted credential backend fails closed (mirroring the REST
    /// surface, which fails startup on the same condition) instead of
    /// proceeding with silently-absent persisted-auth truth.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn resolution_token_store(
        &self,
    ) -> Result<Option<Arc<dyn meerkat_providers::auth_store::TokenStore>>, FactoryError> {
        Ok(self
            .resolution_provider_auth_persistence()?
            .map(|persistence| persistence.token_store()))
    }

    pub fn provider_runtime_registry(
        &self,
    ) -> Arc<meerkat_llm_core::provider_runtime::ProviderRuntimeRegistry> {
        Arc::clone(&self.provider_registry)
    }

    pub fn with_image_generation_machine(
        mut self,
        machine: Arc<dyn meerkat_tools::builtin::image_generation::ImageGenerationMachine>,
    ) -> Self {
        self.image_generation_machine = Some(machine);
        self
    }

    /// Register an external auth resolver that surfaces bindings whose
    /// `CredentialSourceSpec::ExternalResolver { handle }` matches the
    /// supplied typed [`meerkat_core::ExternalResolverId`]. Used by WASM
    /// hosts (browser OAuth flow owned by the page) and by SDK users
    /// embedding meerkat inside an existing auth story.
    pub fn with_external_auth_resolver(
        mut self,
        handle: impl Into<meerkat_core::ExternalResolverId>,
        resolver: Arc<dyn meerkat_providers::ExternalAuthResolverHandle>,
    ) -> Self {
        self.external_auth_resolvers.insert(handle.into(), resolver);
        self
    }

    /// Convenience: attach the default file-backed `TokenStore` at the user's
    /// default credential directory ($XDG_CONFIG_HOME/meerkat/credentials).
    /// Surfaces (CLI / REST / RPC) call this during factory construction.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn with_default_token_store(
        self,
    ) -> Result<Self, meerkat_providers::auth_store::TokenStoreError> {
        let persistence = meerkat_providers::auth_store::TokenStoreBackend::default_auto()?
            .open_with_refresh_authority()?;
        Ok(self.with_provider_auth_persistence(persistence))
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

    /// Enable or disable WorkGraph tools.
    pub fn workgraph(mut self, enabled: bool) -> Self {
        self.enable_workgraph = enabled;
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
        // A factory-owned runtime can later be wired into `SessionOwned`
        // builds, so local compatibility classification must be inert as soon
        // as the runtime enters this surface.
        runtime.require_peer_comms_machine_authority();
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
    ) -> Vec<meerkat_core::skills::CapabilityId> {
        let registered: BTreeSet<meerkat_capabilities::CapabilityId> =
            meerkat_capabilities::build_capabilities()
                .into_iter()
                .map(|registration| registration.id)
                .collect();
        let mut capabilities: BTreeSet<meerkat_capabilities::CapabilityId> =
            meerkat_capabilities::available_capabilities(config)
                .into_iter()
                .collect();

        let set_tool_capability =
            |capabilities: &mut BTreeSet<meerkat_capabilities::CapabilityId>,
             capability: meerkat_capabilities::CapabilityId,
             enabled: bool| {
                if enabled && registered.contains(&capability) {
                    capabilities.insert(capability);
                } else {
                    capabilities.remove(&capability);
                }
            };

        let builtins_enabled = build_config
            .map(|build| build.override_builtins.resolve(self.enable_builtins))
            .unwrap_or(self.enable_builtins);
        set_tool_capability(
            &mut capabilities,
            meerkat_capabilities::CapabilityId::Builtins,
            builtins_enabled,
        );

        let shell_enabled = build_config
            .map(|build| build.override_shell.resolve(self.enable_shell))
            .unwrap_or(self.enable_shell);
        set_tool_capability(
            &mut capabilities,
            meerkat_capabilities::CapabilityId::Shell,
            shell_enabled,
        );

        let memory_enabled = build_config
            .map(|build| {
                effective_memory_for_realm(build.override_memory, self.enable_memory, build.backend)
            })
            .unwrap_or(self.enable_memory);
        set_tool_capability(
            &mut capabilities,
            meerkat_capabilities::CapabilityId::MemoryStore,
            memory_enabled,
        );

        let schedule_enabled = build_config
            .map(|build| build.override_schedule.resolve(self.enable_schedule))
            .unwrap_or(self.enable_schedule);
        set_tool_capability(
            &mut capabilities,
            meerkat_capabilities::CapabilityId::Schedule,
            schedule_enabled,
        );

        let workgraph_enabled = build_config
            .map(|build| build.override_workgraph.resolve(self.enable_workgraph))
            .unwrap_or(self.enable_workgraph);
        set_tool_capability(
            &mut capabilities,
            meerkat_capabilities::CapabilityId::WorkGraph,
            workgraph_enabled,
        );

        #[cfg(feature = "comms")]
        {
            let build_requests_comms =
                build_config.is_some_and(|build| build.comms_name.is_some() || build.keep_alive);
            let has_comms_runtime = self.comms_runtime.is_some();
            set_tool_capability(
                &mut capabilities,
                meerkat_capabilities::CapabilityId::Comms,
                self.enable_comms || build_requests_comms || has_comms_runtime,
            );
        }

        // Project the capability-registry `CapabilityId` enum to the typed
        // core skill capability slug that `DefaultSkillEngine` expects.
        // The stringify path is stable by construction — the enum
        // `Display` impl emits the canonical snake_case slug which
        // `core::skills::CapabilityId::parse` accepts.
        capabilities
            .into_iter()
            .map(|cap| cap.to_string())
            .filter_map(|cap| meerkat_core::skills::CapabilityId::parse(&cap).ok())
            .collect()
    }

    #[cfg(feature = "skills")]
    pub async fn build_skill_runtime(
        &self,
        config: &Config,
    ) -> Result<Option<Arc<meerkat_core::skills::SkillRuntime>>, BuildAgentError> {
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
                            // A repository-resolution failure must degrade
                            // locally, not disable the whole skill runtime: the
                            // embedded skill source is always available and must
                            // survive a failing configured repository.
                            tracing::warn!(
                                "Failed to resolve skill repositories: {e}; \
                                 degrading to embedded-only skill source"
                            );
                            // `embedded_only_skill_source` already yields
                            // `Option<Arc<..>>`; do not re-wrap in Arc.
                            Self::embedded_only_skill_source(&config.skills)?
                        }
                    }
                }
                #[cfg(target_arch = "wasm32")]
                None
            };

        skill_source
            .map(|source| {
                let available_caps = self.effective_skill_capabilities(config, None);
                let registry = Arc::new(config.skills.build_source_identity_registry().map_err(
                    |e| {
                        BuildAgentError::Config(format!(
                            "failed to build skill source identity registry: {e}"
                        ))
                    },
                )?);
                let engine = meerkat_skills::DefaultSkillEngine::new(source, available_caps)
                    .with_inventory_threshold(config.skills.inventory_threshold)
                    .with_max_injection_bytes(config.skills.max_injection_bytes)
                    .with_source_identity_registry(registry);
                let engine = Arc::new(engine);
                Ok::<Arc<meerkat_core::skills::SkillRuntime>, BuildAgentError>(Arc::new(
                    meerkat_core::skills::SkillRuntime::new(engine),
                ))
            })
            .transpose()
    }

    /// Build an embedded-only composite skill source as the fail-soft fallback
    /// when configured repositories fail to resolve.
    ///
    /// The embedded source is always available regardless of repository
    /// configuration, so a failing repository degrades locally to embedded
    /// skills instead of disabling the whole skill runtime. The source-identity
    /// registry is attached so canonical loads still resolve fail-closed.
    #[cfg(all(feature = "skills", not(target_arch = "wasm32")))]
    fn embedded_only_skill_source(
        skills_config: &meerkat_core::skills_config::SkillsConfig,
    ) -> Result<Option<Arc<meerkat_skills::CompositeSkillSource>>, BuildAgentError> {
        let registry = Arc::new(
            skills_config
                .build_source_identity_registry()
                .map_err(|e| {
                    BuildAgentError::Config(format!(
                        "failed to build embedded skill source identity registry: {e}"
                    ))
                })?,
        );
        let builtin_uuid = meerkat_core::skills::SourceUuid::builtin();
        let builtin_identity = skills_config
            .source_identity_records()
            .into_iter()
            .find(|record| record.source_uuid == builtin_uuid)
            .ok_or_else(|| {
                BuildAgentError::Config(
                    "embedded skill source identity record is missing".to_string(),
                )
            })?;
        let source = meerkat_skills::CompositeSkillSource::from_named_with_registry(
            vec![meerkat_skills::NamedSource::new(
                builtin_identity,
                meerkat_skills::source::SourceNode::Embedded(
                    meerkat_skills::EmbeddedSkillSource::new(),
                ),
            )],
            registry,
        );
        Ok(Some(Arc::new(source)))
    }

    /// Override the default session store.
    ///
    /// When set, `build_agent()` uses this store instead of the feature-flag-based
    /// default (jsonl, memory, or ephemeral). The store is wrapped in `StoreAdapter`
    /// and passed to the core `AgentBuilder` factory-policy seam.
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
    ) -> Result<Option<SessionMetadata>, BuildAgentError> {
        let Some(session) = build_config.resume_session.as_ref() else {
            return Ok(None);
        };
        let Some(metadata) = session.session_metadata() else {
            if session.messages().is_empty()
                && matches!(
                    &build_config.runtime_build_mode,
                    meerkat_core::RuntimeBuildMode::SessionOwned(bindings)
                        if bindings.session_id() == session.id()
                            && meerkat_runtime::session_runtime_bindings_have_machine_authority(bindings)
                )
            {
                return Ok(None);
            }
            return Err(BuildAgentError::Config(format!(
                "resumed session {} is missing durable session metadata",
                session.id()
            )));
        };

        let mask = build_config.resume_override_mask;

        if !mask.model {
            build_config.model = metadata.model.clone();
        }
        if !mask.max_tokens {
            build_config.max_tokens = Some(metadata.max_tokens);
        }
        if !mask.structured_output_retries {
            build_config.structured_output_retries = Some(metadata.structured_output_retries);
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
        // Phase 3: propagate persisted auth_binding so resume re-resolves
        // through the same realm binding. The caller keeps precedence —
        // if build_config already carries an auth binding reference, leave it.
        if !mask.auth_binding && build_config.auth_binding.is_none() {
            build_config.auth_binding = metadata
                .auth_binding
                .clone()
                .filter(|auth_binding| !auth_binding.is_env_default());
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
        if !mask.override_schedule {
            build_config.override_schedule = metadata.tooling.schedule;
        }
        if !mask.override_workgraph {
            build_config.override_workgraph = metadata.tooling.workgraph;
        }
        if !mask.override_comms {
            build_config.override_comms = metadata.tooling.comms;
        }
        if !mask.override_mob {
            let persisted_mob_authority = build_config
                .resume_session
                .as_ref()
                .and_then(Session::mob_tool_authority_context);
            build_config.mob_tool_authority_context =
                persisted_mob_authority.and_then(|authority_context| {
                    match meerkat_runtime::mob_operator_authority::restore_mob_operator_authority(
                        &authority_context,
                    ) {
                        Ok(restored) => Some(restored),
                        Err(error) => {
                            tracing::warn!(
                                error = %error,
                                "generated mob operator authority rejected resumed context"
                            );
                            None
                        }
                    }
                });
            // Resumed metadata is a compatibility mirror, not authority. A
            // persisted Disable can keep failing closed, but Enable must come
            // from a restored generated context or an explicit fresh override.
            build_config.override_mob = if build_config.mob_tool_authority_context.is_some() {
                ToolCategoryOverride::Enable
            } else if matches!(metadata.tooling.mob, ToolCategoryOverride::Disable) {
                ToolCategoryOverride::Disable
            } else {
                ToolCategoryOverride::Inherit
            };
        }
        if !mask.override_image_generation {
            build_config.override_image_generation = metadata.tooling.image_generation;
        }
        if !mask.override_web_search {
            build_config.override_web_search = metadata.tooling.web_search;
        }
        if !mask.preload_skills {
            build_config.preload_skills = metadata.tooling.active_skills.clone();
        }
        // Effective call-level tool access policy: restore the persisted
        // policy unless the caller supplied an explicit one — a restricted
        // session must not escape its execution gate by being resumed.
        if !mask.tool_access_policy && build_config.tool_access_policy.is_none() {
            build_config.tool_access_policy = metadata.tooling.tool_access_policy.clone();
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
        // Durable mob-member identity: preserve the persisted typed binding on
        // resume unless the build already carries one. This is identity, not a
        // turn override, so it has no mask bit; the caller keeps precedence.
        if build_config.mob_member_binding.is_none() {
            build_config.mob_member_binding = metadata.mob_member_binding.clone();
        }

        Ok(Some(metadata))
    }

    fn apply_initial_metadata_entries(
        session: &mut Session,
        entries: &BTreeMap<String, serde_json::Value>,
    ) -> Result<(), BuildAgentError> {
        for (key, value) in entries {
            session
                .try_set_metadata(key, value.clone())
                .map_err(|err| {
                    BuildAgentError::Config(format!("invalid initial metadata entry: {err}"))
                })?;
        }
        Ok(())
    }

    /// Build an LLM adapter for the provided client/model.
    pub async fn build_llm_adapter(
        &self,
        client: Arc<dyn LlmClient>,
        model: impl Into<String>,
    ) -> LlmClientAdapter {
        LlmClientAdapter::new(client, model.into())
    }

    /// Build an adapter whose facade identity is owned by the canonical
    /// session identity rather than by replaceable raw transport mechanics.
    pub async fn build_llm_adapter_for_identity(
        &self,
        client: Arc<dyn LlmClient>,
        identity: &SessionLlmIdentity,
    ) -> Result<LlmClientAdapter, BuildAgentError> {
        LlmClientAdapter::try_for_provider_identity(
            client,
            identity.model.clone(),
            identity.provider,
        )
        .map_err(|error| BuildAgentError::Config(error.to_string()))
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

    /// Apply an optional provider-agnostic wrapper to the final agent LLM client.
    pub fn decorate_agent_llm_client(
        client: Arc<dyn AgentLlmClient>,
        decorator: Option<&AgentLlmClientDecorator>,
    ) -> Arc<dyn AgentLlmClient> {
        match decorator {
            Some(decorator) => decorator(client),
            None => client,
        }
    }

    pub async fn build_llm_client_for_identity(
        &self,
        config: &Config,
        identity: &SessionLlmIdentity,
    ) -> Result<Arc<dyn LlmClient>, FactoryError> {
        self.build_llm_client_for_identity_with_auth_lease(config, identity, None)
            .await
    }

    pub async fn build_llm_client_for_identity_with_auth_lease(
        &self,
        config: &Config,
        identity: &SessionLlmIdentity,
        auth_lease_handle: Option<meerkat_core::handles::GeneratedAuthLeaseHandle>,
    ) -> Result<Arc<dyn LlmClient>, FactoryError> {
        let registry = config
            .model_registry(meerkat_models::canonical())
            .map_err(|err| FactoryError::ClientCreationFailed(err.to_string()))?;
        if matches!(identity.provider, Provider::SelfHosted) {
            return self
                .build_self_hosted_client_for_identity(
                    config,
                    &registry,
                    identity,
                    auth_lease_handle,
                    None,
                )
                .await
                .map(|resolved| resolved.client);
        }

        // Test-mode shim: hot-swap never needs real credentials under
        // RKAT_TEST_CLIENT=1.
        if std::env::var("RKAT_TEST_CLIENT").ok().as_deref() == Some("1") {
            return Ok(Arc::new(meerkat_client::TestClient::for_provider(
                identity.provider,
            )));
        }

        let (realm, _binding_id, auth_binding) = Self::resolve_realm_binding_for_provider(
            config,
            identity.provider,
            identity.auth_binding.as_ref(),
            None,
        )
        .map_err(FactoryError::ConnectionTarget)?;
        let lease_auth_binding = if auth_binding.is_env_default()
            && identity
                .auth_binding
                .as_ref()
                .map(AuthBindingRef::is_env_default)
                .unwrap_or(true)
        {
            None
        } else {
            Some(auth_binding.clone())
        };

        #[allow(unused_mut)]
        let mut env = meerkat_providers::ResolverEnvironment::with_process_env();
        #[cfg(not(target_arch = "wasm32"))]
        {
            if let Some(persistence) = self.resolution_provider_auth_persistence()? {
                env = env.with_provider_auth_persistence(persistence);
            }
        }
        for (handle, resolver) in &self.external_auth_resolvers {
            env = env.with_external_resolver(handle.clone(), resolver.clone());
        }
        if lease_auth_binding.is_some()
            && let Some(handle) = auth_lease_handle.clone()
        {
            env = env.with_auth_lease_handle(handle);
        }
        let provider_registry = Arc::clone(&self.provider_registry);
        let connection = provider_registry
            .resolve(&realm, &auth_binding, &env)
            .await
            .map_err(FactoryError::ProviderAuth)?;
        if let (Some(handle), Some(lease_auth_binding)) =
            (auth_lease_handle, lease_auth_binding.as_ref())
        {
            Self::publish_auth_lease(&handle, lease_auth_binding, &connection)?;
        }
        provider_registry
            .build_client(connection)
            .map_err(FactoryError::ClientBuild)
    }

    /// Build the per-request LLM policy for a (re)configured identity.
    ///
    /// `web_search` carries the session's persisted web-search disable intent
    /// (`SessionMetadata.tooling.web_search`). It MUST be threaded through so a
    /// model/provider hot-swap preserves a session's `--no-web-search` disable
    /// instead of silently re-enabling the provider-native web-search body.
    pub fn request_policy_for_llm_identity(
        &self,
        config: &Config,
        identity: &SessionLlmIdentity,
        web_search: ToolCategoryOverride,
    ) -> Result<meerkat_core::SessionLlmRequestPolicy, FactoryError> {
        let registry = config
            .model_registry(meerkat_models::canonical())
            .map_err(|err| FactoryError::ClientCreationFailed(err.to_string()))?;
        let model_profile = registry.profile_for_provider(identity.provider, &identity.model);
        Ok(meerkat_core::SessionLlmRequestPolicy {
            model: identity.model.clone(),
            provider_params: identity.provider_params.clone(),
            provider_tool_defaults: provider_tool_defaults_for(
                identity.provider,
                config,
                model_profile.as_ref(),
                web_search,
            ),
        })
    }

    fn model_registry(&self, config: &Config) -> Result<ModelRegistry, BuildAgentError> {
        config
            .model_registry(meerkat_models::canonical())
            .map_err(|err| BuildAgentError::Config(err.to_string()))
    }

    fn resolve_provider_from_registry(
        &self,
        registry: &ModelRegistry,
        build_config: &AgentBuildConfig,
    ) -> Result<(Provider, Option<String>), BuildAgentError> {
        if let Some(provider) = build_config.provider {
            if let Some(reason) =
                registry.provider_override_mismatch_reason(provider, &build_config.model)
            {
                return Err(BuildAgentError::Config(reason));
            }

            return match provider {
                Provider::SelfHosted => {
                    let entry = registry
                        .entry_for_provider(Provider::SelfHosted, &build_config.model)
                        .ok_or_else(|| {
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

        if let Some(client) = build_config.llm_client_override.as_ref() {
            return Ok((client.provider(), None));
        }
        if let Some(client) = build_config.agent_llm_client_override.as_ref() {
            // `AgentLlmClient::provider()` is typed at the trait seam: a
            // caller-supplied custom client (the bring-your-own-client
            // extension point) declares its own `Provider` variant directly —
            // no string parse-back, no minted catalog identity.
            return Ok((client.provider(), None));
        }

        Err(BuildAgentError::UnknownProvider {
            model: build_config.model.clone(),
        })
    }

    fn model_fallback_identities(
        &self,
        config: &Config,
        registry: &ModelRegistry,
        current: &SessionLlmIdentity,
    ) -> Result<Vec<SessionLlmIdentity>, BuildAgentError> {
        if !config.model_fallback.enabled {
            return Ok(Vec::new());
        }

        let catalog_default_chain = config.model_fallback.chain.is_empty();
        let preferred_realm = current
            .auth_binding
            .as_ref()
            .filter(|auth_binding| !auth_binding.is_env_default())
            .map(|auth_binding| auth_binding.realm.clone());
        let mut targets: Vec<(String, Option<Provider>, Option<AuthBindingRef>)> =
            if catalog_default_chain {
                let mut defaults = Vec::new();
                for provider in meerkat_models::provider_priority() {
                    let model = match provider {
                        Provider::Anthropic => config.models.anthropic.clone(),
                        Provider::OpenAI => config.models.openai.clone(),
                        Provider::Gemini => config.models.gemini.clone(),
                        _ => String::new(),
                    };
                    let model = if model.is_empty() {
                        meerkat_models::default_model(*provider)
                            .map(str::to_string)
                            .unwrap_or_default()
                    } else {
                        model
                    };
                    if !model.is_empty() {
                        defaults.push((model, Some(*provider), None));
                    }
                }
                defaults.push((
                    meerkat_models::global_default_model().to_string(),
                    None,
                    None,
                ));
                defaults
            } else {
                config
                    .model_fallback
                    .chain
                    .iter()
                    .map(|target| {
                        (
                            target.model.clone(),
                            target.provider,
                            target.auth_binding.clone(),
                        )
                    })
                    .collect()
            };

        let mut identities = Vec::new();
        for (model, provider_override, auth_binding) in targets.drain(..) {
            let provider = if let Some(provider) = provider_override {
                if let Some(reason) = registry.provider_override_mismatch_reason(provider, &model) {
                    return Err(BuildAgentError::Config(reason));
                }
                provider
            } else {
                registry
                    .entry(&model)
                    .map(|entry| entry.provider)
                    .ok_or_else(|| BuildAgentError::UnknownProvider {
                        model: model.clone(),
                    })?
            };
            let self_hosted_server_id = if matches!(provider, Provider::SelfHosted) {
                registry
                    .entry_for_provider(Provider::SelfHosted, &model)
                    .and_then(|entry| entry.self_hosted.as_ref())
                    .map(|server| server.server_id.clone())
            } else {
                None
            };
            let auth_binding = match (auth_binding, preferred_realm.as_ref()) {
                (Some(auth_binding), _) => Some(auth_binding),
                (None, Some(realm)) => {
                    let resolved = Self::resolve_realm_binding_candidates_for_provider(
                        config,
                        provider,
                        None,
                        Some(realm),
                    )
                    .ok()
                    .and_then(|candidates| {
                        candidates
                            .into_iter()
                            .find(|target| {
                                if catalog_default_chain {
                                    // Candidates are already chain-scoped
                                    // (head -> ancestors -> global), in
                                    // nearest-child-wins order. Accept the first
                                    // configured one — an inherited binding is
                                    // owner-stamped (owner != preferred realm),
                                    // so a realm-equality filter would wrongly
                                    // drop it.
                                    !target.auth_binding.is_env_default()
                                } else {
                                    true
                                }
                            })
                            .map(|target| target.auth_binding)
                    });
                    if catalog_default_chain && resolved.is_none() {
                        continue;
                    }
                    resolved
                }
                (None, None) => None,
            };
            let identity = SessionLlmIdentity {
                model,
                provider,
                self_hosted_server_id,
                provider_params: None,
                auth_binding,
            };
            if identity.model == current.model
                && identity.provider == current.provider
                && identity.self_hosted_server_id == current.self_hosted_server_id
                && identity.auth_binding == current.auth_binding
            {
                continue;
            }
            if identities.iter().any(|seen: &SessionLlmIdentity| {
                seen.model == identity.model
                    && seen.provider == identity.provider
                    && seen.self_hosted_server_id == identity.self_hosted_server_id
                    && seen.auth_binding == identity.auth_binding
            }) {
                continue;
            }
            identities.push(identity);
        }

        Ok(identities)
    }

    #[cfg(feature = "openai")]
    fn self_hosted_client_spec_for_identity(
        &self,
        registry: &ModelRegistry,
        identity: &SessionLlmIdentity,
    ) -> Result<SelfHostedClientSpec, FactoryError> {
        let entry = registry
            .entry_for_provider(Provider::SelfHosted, &identity.model)
            .ok_or_else(|| {
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
        let profile = registry
            .profile_for_provider(Provider::SelfHosted, &identity.model)
            .ok_or_else(|| {
                FactoryError::ClientCreationFailed(format!(
                    "missing provider-aware profile for self-hosted model '{}'",
                    identity.model
                ))
            })?;

        Ok(SelfHostedClientSpec {
            server_id: self_hosted.server_id.clone(),
            mode,
            remote_model: self_hosted.remote_model.clone(),
            base_url: self_hosted.base_url.clone(),
            supports_temperature: profile.supports_temperature,
            supports_thinking: profile.supports_thinking,
            supports_reasoning: profile.supports_reasoning,
            supports_image_tool_results: profile.image_tool_results,
        })
    }

    async fn build_self_hosted_client_for_identity(
        &self,
        config: &Config,
        registry: &ModelRegistry,
        identity: &SessionLlmIdentity,
        auth_lease_handle: Option<meerkat_core::handles::GeneratedAuthLeaseHandle>,
        preferred_realm: Option<&RealmId>,
    ) -> Result<SelfHostedClientBuild, FactoryError> {
        #[cfg(not(feature = "openai"))]
        {
            let _ = (
                config,
                registry,
                identity,
                auth_lease_handle,
                preferred_realm,
            );
            Err(FactoryError::UnsupportedProvider(
                "self_hosted requires the openai feature".to_string(),
            ))
        }

        #[cfg(feature = "openai")]
        {
            let spec = self.self_hosted_client_spec_for_identity(registry, identity)?;
            let (realm, auth_binding, durable_auth_binding) =
                self.resolve_self_hosted_connection(config, identity, &spec, preferred_realm)?;

            #[allow(unused_mut)]
            let mut env = meerkat_providers::ResolverEnvironment::with_process_env();
            #[cfg(not(target_arch = "wasm32"))]
            {
                if let Some(persistence) = self.resolution_provider_auth_persistence()? {
                    env = env.with_provider_auth_persistence(persistence);
                }
            }
            if let Some(handle) = auth_lease_handle.clone() {
                env = env.with_auth_lease_handle(handle);
            }
            for (handle, resolver) in &self.external_auth_resolvers {
                env = env.with_external_resolver(handle.clone(), resolver.clone());
            }

            let provider_registry = Arc::clone(&self.provider_registry);
            let connection = provider_registry
                .resolve(&realm, &auth_binding, &env)
                .await
                .map_err(FactoryError::ProviderAuth)?;

            if let Some(handle) = auth_lease_handle {
                Self::publish_auth_lease(&handle, &auth_binding, &connection)?;
            }

            if connection.resolved_authorizer().is_some() {
                return Err(FactoryError::ClientCreationFailed(
                    "self-hosted OpenAI-compatible clients do not support dynamic authorizer-backed auth"
                        .to_string(),
                ));
            }
            let bearer_token = connection.resolved_secret();
            let base_url = connection
                .backend_profile
                .base_url
                .clone()
                .unwrap_or(spec.base_url);
            Ok(SelfHostedClientBuild {
                client: Arc::new(OpenAiCompatibleClient::new_with_options(
                    spec.mode,
                    spec.remote_model,
                    base_url,
                    bearer_token,
                    OpenAiCompatibleClientOptions {
                        supports_temperature: spec.supports_temperature,
                        supports_thinking: spec.supports_thinking,
                        supports_reasoning: spec.supports_reasoning,
                        supports_image_tool_results: spec.supports_image_tool_results,
                    },
                )),
                durable_auth_binding,
            })
        }
    }

    #[cfg(feature = "openai")]
    fn resolve_self_hosted_connection(
        &self,
        config: &Config,
        identity: &SessionLlmIdentity,
        spec: &SelfHostedClientSpec,
        preferred_realm: Option<&RealmId>,
    ) -> Result<(RealmConnectionSet, AuthBindingRef, Option<AuthBindingRef>), FactoryError> {
        if let Some(auth_binding) = identity.auth_binding.as_ref() {
            let (realm, _binding_id, resolved_auth_binding) =
                Self::resolve_realm_binding_for_provider(
                    config,
                    Provider::SelfHosted,
                    Some(auth_binding),
                    None,
                )
                .map_err(FactoryError::ConnectionTarget)?;
            return Ok((
                realm,
                resolved_auth_binding.clone(),
                Some(resolved_auth_binding),
            ));
        }

        if let Some((realm, resolved_auth_binding)) =
            Self::configured_self_hosted_connection(config, preferred_realm)?
        {
            return Ok((
                realm,
                resolved_auth_binding.clone(),
                Some(resolved_auth_binding),
            ));
        }

        // No transient `[self_hosted]` migration realm is synthesized: the
        // canonical `RealmConnectionSet` (from `auth_binding` or a configured
        // realm default binding resolved above) is the sole owner of the
        // self-hosted connection. A `[self_hosted]` server with no canonical
        // realm binding fails closed rather than fabricating a parallel
        // `self_hosted_legacy` realm + FNV-hashed transient binding id.
        Err(FactoryError::ClientCreationFailed(format!(
            "self-hosted server '{}' has no canonical realm binding; define a realm with a self_hosted backend + binding and select it with auth_binding or a realm default binding",
            spec.server_id
        )))
    }

    #[cfg(feature = "openai")]
    fn configured_self_hosted_connection(
        config: &Config,
        preferred_realm: Option<&RealmId>,
    ) -> Result<Option<(RealmConnectionSet, AuthBindingRef)>, FactoryError> {
        if let Some(realm_id) = preferred_realm
            && config.realm.contains_key(realm_id.as_str())
        {
            let target = meerkat_core::resolve_realm_binding_target_for_provider(
                config,
                Provider::SelfHosted,
                Some(realm_id),
                None,
                None,
                None,
                false,
            )
            .map_err(|err| {
                FactoryError::ClientCreationFailed(format!(
                    "selected realm '{}' self_hosted credential binding is unavailable: {err}",
                    realm_id.as_str()
                ))
            })?;
            return Ok(Some((target.realm, target.auth_binding)));
        }

        match meerkat_core::resolve_auth_binding_or_default_for_provider(
            config,
            Provider::SelfHosted,
            None,
            preferred_realm,
            false,
        ) {
            Ok(target) => Ok(Some((target.realm, target.auth_binding))),
            Err(err) => {
                tracing::debug!(
                    error = %err,
                    "self-hosted connection seam lookup did not find a configured realm binding; resolution will fail closed"
                );
                Ok(None)
            }
        }
    }

    fn publish_auth_lease(
        handle: &meerkat_core::handles::GeneratedAuthLeaseHandle,
        auth_binding: &AuthBindingRef,
        connection: &meerkat_llm_core::provider_runtime::ResolvedConnection,
    ) -> Result<(), FactoryError> {
        if matches!(
            connection.auth_lease.kind(),
            meerkat_core::ResolvedAuthKind::None
        ) {
            return Ok(());
        }
        let lease_key = meerkat_core::handles::LeaseKey::from_auth_binding(auth_binding);
        let expires_at = connection
            .auth_lease
            .expires_at()
            .map(|ts| ts.timestamp().max(0) as u64)
            .unwrap_or(u64::MAX);
        let snapshot = handle.snapshot(&lease_key);
        let snapshot_expires_at = snapshot.expires_at.unwrap_or(u64::MAX);
        if snapshot.credential_present
            && snapshot_expires_at == expires_at
            && matches!(
                snapshot.phase,
                Some(
                    meerkat_core::handles::AuthLeasePhase::Valid
                        | meerkat_core::handles::AuthLeasePhase::Expiring
                )
            )
        {
            return Ok(());
        }
        handle.acquire_lease(&lease_key, expires_at).map_err(|e| {
            FactoryError::ClientCreationFailed(format!("AuthMachine lifecycle acquire failed: {e}"))
        })?;
        Ok(())
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
    ) -> Result<CompositeDispatcher, CompositeDispatcherError> {
        CompositeDispatcher::new_with_ops_lifecycle(
            store,
            config,
            project_root,
            shell_config,
            external,
            session_id,
            ops_lifecycle,
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
            None,
            None,
            None,
            None,
            ToolCategoryOverride::Inherit,
            None,
            ToolCategoryOverride::Inherit,
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
        image_generation_machine: Option<
            Arc<dyn meerkat_tools::builtin::image_generation::ImageGenerationMachine>,
        >,
        image_generation_executor: Option<Arc<dyn meerkat_llm_core::ImageGenerationExecutor>>,
        image_generation_planner: Option<Arc<dyn meerkat_core::ImageGenerationPlanner>>,
        image_generation_blob_store: Option<Arc<dyn BlobStore>>,
        image_generation_visibility: ToolCategoryOverride,
        web_search_executor: Option<Arc<dyn meerkat_llm_core::WebSearchExecutor>>,
        web_search_visibility: ToolCategoryOverride,
    ) -> Result<Arc<dyn AgentToolDispatcher>, CompositeDispatcherError> {
        let BuiltinDispatcherConfig {
            store,
            config,
            project_root,
            shell_config,
            external,
            session_id,
            ops_lifecycle,
        } = BuiltinDispatcherConfig {
            store,
            config,
            project_root,
            shell_config,
            external,
            session_id,
            ops_lifecycle,
        };

        // dogma #299: the composite no longer launders the ambient process CWD
        // into the project root. Re-supply a concrete, caller-owned fallback at
        // this single chokepoint that every factory-built dispatcher
        // (`build_agent` plus all SDK `create_*` helpers) funnels through, so
        // none can reach the composite with an unresolved root. Precedence
        // mirrors the composite's own order with the removed CWD step replaced
        // by the factory's store-parent authority: explicit `project_root`, then
        // any `shell_config` root, then `shell_project_root()`.
        let project_root = project_root
            .or_else(|| shell_config.as_ref().map(|cfg| cfg.project_root.clone()))
            .or_else(|| Some(self.shell_project_root()));

        #[cfg_attr(not(feature = "skills"), allow(unused_mut))]
        let mut composite = self
            .build_composite_dispatcher(
                store,
                &config,
                project_root,
                shell_config,
                external,
                session_id.clone(),
                ops_lifecycle,
            )
            .await?;

        #[cfg(feature = "skills")]
        if let Some(engine) = skill_engine {
            composite
                .register_skill_tools(meerkat_tools::builtin::skills::SkillToolSet::new(engine));
        }

        if let Some(blob_store) = image_generation_blob_store.clone() {
            composite.register_blob_file_tools(blob_store);
        }

        if let Some(executor) = web_search_executor {
            composite.register_web_search_tool(executor, web_search_visibility);
        }

        if let (Some(session_id), Some(machine), Some(executor), Some(planner), Some(blob_store)) = (
            session_id
                .as_deref()
                .and_then(|id| SessionId::parse(id).ok()),
            image_generation_machine,
            image_generation_executor,
            image_generation_planner,
            image_generation_blob_store,
        ) {
            composite.register_image_generation_tool(
                meerkat_tools::builtin::image_generation::ImageGenerationToolRuntime {
                    session_id,
                    machine,
                    planner,
                    blob_store,
                    executor,
                },
                image_generation_visibility,
            );
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
        build_config.resume_override_mask.override_schedule |= !matches!(
            build_config.override_schedule,
            ToolCategoryOverride::Inherit
        );
        build_config.resume_override_mask.override_workgraph |= !matches!(
            build_config.override_workgraph,
            ToolCategoryOverride::Inherit
        );
        build_config.resume_override_mask.override_comms |=
            !matches!(build_config.override_comms, ToolCategoryOverride::Inherit);
        let has_explicit_mob_authority_context = build_config
            .mob_tool_authority_context
            .as_ref()
            .is_some_and(
                meerkat_core::service::MobToolAuthorityContext::is_generated_authority_context,
            );
        // A live generated authority context is an explicit composition
        // handoff. Resumed metadata must not overwrite it with a projection.
        build_config.resume_override_mask.override_mob |= has_explicit_mob_authority_context
            || !matches!(build_config.override_mob, ToolCategoryOverride::Inherit);
        build_config.resume_override_mask.override_image_generation |= !matches!(
            build_config.override_image_generation,
            ToolCategoryOverride::Inherit
        );
        build_config.resume_override_mask.override_web_search |= !matches!(
            build_config.override_web_search,
            ToolCategoryOverride::Inherit
        );
        build_config.resume_override_mask.tool_access_policy |=
            build_config.tool_access_policy.is_some();

        let explicit_mob_override =
            !matches!(build_config.override_mob, ToolCategoryOverride::Inherit);
        let resumed_session_metadata = Self::apply_resumed_session_metadata(&mut build_config)?;
        let mut session = build_config.resume_session.clone().unwrap_or_default();
        if let RuntimeBuildMode::SessionOwned(bindings) = &build_config.runtime_build_mode {
            if !meerkat_runtime::session_runtime_bindings_have_machine_authority(bindings) {
                return Err(BuildAgentError::Config(
                    "SessionRuntimeBindings were not prepared by MeerkatMachine; \
                     session-owned runtime builds must use MeerkatMachine-prepared bindings"
                        .to_string(),
                ));
            }
            if bindings.session_id() != session.id() {
                return Err(BuildAgentError::Config(format!(
                    "SessionRuntimeBindings.session_id ({}) does not match session ({}); \
                     bindings may have been prepared for a different session",
                    bindings.session_id(),
                    session.id(),
                )));
            }
        }

        if let Some(authority_context) = build_config.mob_tool_authority_context.take() {
            build_config.mob_tool_authority_context =
                match meerkat_runtime::mob_operator_authority::restore_mob_operator_authority(
                    &authority_context,
                ) {
                    Ok(restored) => Some(restored),
                    Err(error) => {
                        tracing::warn!(
                            error = %error,
                            "generated mob operator authority rejected build context"
                        );
                        None
                    }
                };
        }
        if build_config.mob_tool_authority_context.is_some()
            && matches!(build_config.override_mob, ToolCategoryOverride::Inherit)
        {
            // Successful generated authority restore is the typed handoff that
            // moves the visibility mirror to active mob operator intent.
            build_config.override_mob = ToolCategoryOverride::Enable;
        }

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

        let registry = if build_config.custom_models.is_empty() {
            self.model_registry(config)?
        } else {
            ModelRegistry::from_config_with_models(
                config,
                &build_config.custom_models,
                meerkat_models::canonical(),
            )
            .map_err(|err| BuildAgentError::Config(err.to_string()))?
        };
        #[cfg(not(target_arch = "wasm32"))]
        let image_generation_planner: Option<
            Arc<dyn meerkat_core::ImageGenerationPlanner>,
        > = {
            let profiles = self.provider_registry.image_generation_profiles();
            (!profiles.is_empty()).then(|| {
                Arc::new(
                    CompositeImageGenerationPlanner::new(profiles)
                        .with_auto_target_provider(build_config.image_generation_provider),
                ) as Arc<dyn meerkat_core::ImageGenerationPlanner>
            })
        };

        // 2. Resolve provider and any self-hosted server binding.
        let resumed_self_hosted_server_id = resumed_session_metadata
            .as_ref()
            .and_then(|metadata| metadata.self_hosted_server_id.clone());
        let (provider, resolved_self_hosted_server_id) =
            self.resolve_provider_from_registry(&registry, &build_config)?;
        if let Some(client) = build_config.llm_client_override.as_ref() {
            let claimed_provider = client.provider();
            if !matches!(claimed_provider, Provider::Other) && claimed_provider != provider {
                return Err(BuildAgentError::Config(format!(
                    "raw LLM client override claims provider '{}' but canonical model '{}' belongs to '{}'",
                    claimed_provider.as_str(),
                    build_config.model,
                    provider.as_str(),
                )));
            }
        }
        if let Some(client) = build_config.agent_llm_client_override.as_ref()
            && (client.provider() != provider || client.model() != build_config.model)
        {
            return Err(BuildAgentError::Config(format!(
                "agent LLM client override identity '{}:{}' does not match canonical session identity '{}:{}'",
                client.provider().as_str(),
                client.model(),
                provider.as_str(),
                build_config.model,
            )));
        }
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
        #[cfg(not(target_arch = "wasm32"))]
        let mut auto_image_generation_executor: Option<
            Arc<dyn meerkat_llm_core::ImageGenerationExecutor>,
        > = None;
        let llm_client: Option<Arc<dyn LlmClient>> = if build_config
            .agent_llm_client_override
            .is_some()
        {
            None
        } else {
            Some(match build_config.llm_client_override.as_ref() {
                Some(client) => Arc::clone(client),
                None if std::env::var("RKAT_TEST_CLIENT").ok().as_deref() == Some("1") => {
                    // Test shim: when RKAT_TEST_CLIENT=1 is set by integration
                    // tests, short-circuit to an in-process TestClient so tests
                    // don't need real provider credentials.
                    Arc::new(meerkat_client::TestClient::for_provider(provider))
                }
                None => {
                    if matches!(provider, Provider::SelfHosted) {
                        let auth_lease_handle = if let RuntimeBuildMode::SessionOwned(bindings) =
                            &build_config.runtime_build_mode
                        {
                            Some(bindings.auth_lease().clone())
                        } else {
                            None
                        };
                        let resolved = self
                            .build_self_hosted_client_for_identity(
                                config,
                                &registry,
                                &SessionLlmIdentity {
                                    model: build_config.model.clone(),
                                    provider,
                                    self_hosted_server_id: self_hosted_server_id.clone(),
                                    provider_params: build_config.provider_params.clone(),
                                    auth_binding: build_config.auth_binding.clone(),
                                },
                                auth_lease_handle,
                                build_config.realm_id.as_ref(),
                            )
                            .await
                            .map_err(BuildAgentError::LlmClient)?;
                        build_config.auth_binding = resolved.durable_auth_binding;
                        resolved.client
                    } else {
                        // Provider-runtime registry needs the OAuth-backed
                        // TokenStore attached so persisted tokens (written by
                        // `rkat auth login`, server-side OAuth completion, etc.)
                        // are read during resolve_binding.
                        #[allow(unused_mut)]
                        let mut env = meerkat_providers::ResolverEnvironment::with_process_env();
                        #[cfg(not(target_arch = "wasm32"))]
                        {
                            if let Some(persistence) = self
                                .resolution_provider_auth_persistence()
                                .map_err(BuildAgentError::LlmClient)?
                            {
                                env = env.with_provider_auth_persistence(persistence);
                            }
                        }
                        for (handle, resolver) in &self.external_auth_resolvers {
                            env = env.with_external_resolver(handle.clone(), resolver.clone());
                        }
                        let explicit_auth_binding = build_config.auth_binding.is_some();
                        let provider_registry = Arc::clone(&self.provider_registry);
                        let mut first_resolution_error: Option<
                            meerkat_llm_core::provider_runtime::ProviderAuthError,
                        > = None;
                        let mut resolved = None;
                        let candidates = Self::resolve_realm_binding_candidates_for_provider(
                            config,
                            provider,
                            build_config.auth_binding.as_ref(),
                            build_config.realm_id.as_ref(),
                        )
                        .map_err(|e| {
                            BuildAgentError::LlmClient(FactoryError::ConnectionTarget(e))
                        })?;
                        for target in candidates {
                            let resolved_auth_binding = target.auth_binding.clone();
                            let lease_auth_binding = if resolved_auth_binding.is_env_default()
                                && !explicit_auth_binding
                            {
                                None
                            } else {
                                Some(resolved_auth_binding.clone())
                            };
                            let mut candidate_env = env.clone();
                            if let RuntimeBuildMode::SessionOwned(bindings) =
                                &build_config.runtime_build_mode
                                && lease_auth_binding.is_some()
                            {
                                candidate_env = candidate_env
                                    .with_auth_lease_handle(bindings.auth_lease().clone());
                            }
                            match provider_registry
                                .resolve(&target.realm, &resolved_auth_binding, &candidate_env)
                                .await
                            {
                                Ok(connection) => {
                                    if connection.provider != provider {
                                        return Err(BuildAgentError::LlmClient(
                                            FactoryError::ProviderAuth(
                                                meerkat_llm_core::provider_runtime::ProviderAuthError::ResolvedProviderMismatch {
                                                    expected: provider,
                                                    resolved: connection.provider,
                                                },
                                            ),
                                        ));
                                    }
                                    // `build_config.model` is already the
                                    // canonical surface resolution. Binding
                                    // selection owns credentials only; it must
                                    // not silently replace that final model
                                    // merely because the original caller
                                    // omitted an explicit model hint.
                                    let resolved_model = build_config.model.clone();
                                    resolved = Some((
                                        connection,
                                        resolved_auth_binding,
                                        lease_auth_binding,
                                        resolved_model,
                                    ));
                                    break;
                                }
                                Err(err) => {
                                    first_resolution_error.get_or_insert(err);
                                    if explicit_auth_binding {
                                        break;
                                    }
                                }
                            }
                        }
                        let (connection, resolved_auth_binding, lease_auth_binding, resolved_model) =
                            resolved.ok_or_else(|| {
                                BuildAgentError::LlmClient(FactoryError::ProviderAuth(
                                    first_resolution_error.unwrap_or_else(|| {
                                        meerkat_llm_core::provider_runtime::ProviderAuthError::SourceResolutionFailed(
                                            format!(
                                                "no auth binding candidates resolved for provider '{}'",
                                                provider.as_str()
                                            ),
                                        )
                                    }),
                                ))
                            })?;
                        if resolved_model.is_empty() {
                            return Err(BuildAgentError::Config(format!(
                                "auth binding realm '{}' binding '{}' has an empty default_model",
                                resolved_auth_binding.realm, resolved_auth_binding.binding,
                            )));
                        }
                        if let Some(reason) =
                            registry.provider_override_mismatch_reason(provider, &resolved_model)
                        {
                            return Err(BuildAgentError::Config(reason));
                        }
                        build_config.model = resolved_model;

                        // Publish immediately after resolve. Provider resolution can refresh and
                        // persist OAuth token bytes; AuthMachine must observe the returned lease
                        // before any later build step can fail.
                        if let RuntimeBuildMode::SessionOwned(bindings) =
                            &build_config.runtime_build_mode
                            && let Some(lease_auth_binding) = lease_auth_binding.as_ref()
                        {
                            Self::publish_auth_lease(
                                bindings.auth_lease(),
                                lease_auth_binding,
                                &connection,
                            )
                            .map_err(BuildAgentError::LlmClient)?;
                        }

                        #[cfg(not(target_arch = "wasm32"))]
                        {
                            auto_image_generation_executor = provider_registry
                                .build_image_generation_executor(connection.clone())
                                .map_err(|e| {
                                    BuildAgentError::LlmClient(FactoryError::ClientBuild(e))
                                })?;
                        }

                        if lease_auth_binding.is_some() {
                            build_config.auth_binding = Some(resolved_auth_binding);
                        } else {
                            build_config.auth_binding = None;
                        }

                        // Realtime-capable OpenAI models (e.g. gpt-realtime-2)
                        // cannot go through the Responses API — POST /v1/responses
                        // returns 404 model_not_found. Route those through the
                        // OpenAI Realtime WebSocket via `OpenAiRealtimeTextAdapter`.
                        // Capability-driven routing owns this decision at the
                        // composition seam (dogma §9).
                        let realtime_route = matches!(provider, Provider::OpenAI)
                            && is_openai_realtime_capable(&build_config.model);
                        #[cfg(not(feature = "openai-realtime"))]
                        if realtime_route {
                            return Err(BuildAgentError::LlmClient(FactoryError::ClientBuild(
                                meerkat_llm_core::provider_runtime::ProviderClientError::MissingFeature(
                                    "openai-realtime",
                                ),
                            )));
                        }
                        #[cfg(feature = "openai-realtime")]
                        if realtime_route {
                            provider_registry
                                .build_realtime_text_client(connection)
                                .map_err(|e| {
                                    BuildAgentError::LlmClient(FactoryError::ClientBuild(e))
                                })?
                        } else {
                            provider_registry.build_client(connection).map_err(|e| {
                                BuildAgentError::LlmClient(FactoryError::ClientBuild(e))
                            })?
                        }
                        #[cfg(not(feature = "openai-realtime"))]
                        {
                            provider_registry.build_client(connection).map_err(|e| {
                                BuildAgentError::LlmClient(FactoryError::ClientBuild(e))
                            })?
                        }
                    }
                }
            })
        };
        #[cfg(not(target_arch = "wasm32"))]
        if build_config.image_generation_executor_override.is_none() {
            let mut executors: BTreeMap<
                String,
                Arc<dyn meerkat_llm_core::ImageGenerationExecutor>,
            > = BTreeMap::new();
            if let Some(executor) = auto_image_generation_executor.take() {
                executors.insert(provider_key(provider).to_string(), executor);
            }
            for image_provider in [Provider::OpenAI, Provider::Gemini] {
                let key = provider_key(image_provider).to_string();
                if executors.contains_key(&key) {
                    continue;
                }
                let Ok((realm, _binding_id, auth_binding)) =
                    Self::resolve_image_binding_for_provider(
                        config,
                        image_provider,
                        build_config.realm_id.as_ref(),
                    )
                else {
                    continue;
                };
                #[allow(unused_mut)]
                let mut env = meerkat_providers::ResolverEnvironment::with_process_env();
                #[cfg(not(target_arch = "wasm32"))]
                {
                    if let Some(persistence) = self
                        .resolution_provider_auth_persistence()
                        .map_err(BuildAgentError::LlmClient)?
                    {
                        env = env.with_provider_auth_persistence(persistence);
                    }
                }
                if let RuntimeBuildMode::SessionOwned(bindings) = &build_config.runtime_build_mode
                    && !auth_binding.is_env_default()
                {
                    env = env.with_auth_lease_handle(bindings.auth_lease().clone());
                }
                for (handle, resolver) in &self.external_auth_resolvers {
                    env = env.with_external_resolver(handle.clone(), resolver.clone());
                }
                let Ok(connection) = self
                    .provider_registry
                    .resolve(&realm, &auth_binding, &env)
                    .await
                else {
                    continue;
                };
                if let RuntimeBuildMode::SessionOwned(bindings) = &build_config.runtime_build_mode
                    && !auth_binding.is_env_default()
                {
                    Self::publish_auth_lease(bindings.auth_lease(), &auth_binding, &connection)
                        .map_err(BuildAgentError::LlmClient)?;
                }
                let Ok(Some(executor)) = self
                    .provider_registry
                    .build_image_generation_executor(connection)
                else {
                    continue;
                };
                executors.insert(key, executor);
            }
            auto_image_generation_executor = match executors.len() {
                0 => None,
                _ => Some(Arc::new(RoutingImageGenerationExecutor::new(executors))
                    as Arc<dyn meerkat_llm_core::ImageGenerationExecutor>),
            };
        }

        // 4. Create LLM adapter (with optional provider_params, event channel, and shared event tap)
        let model = build_config.model.clone();
        let model_profile = registry.profile_for_provider(provider, &model);
        let capability_base_filter_override = model_profile.as_ref().map(|profile| {
            meerkat_core::capability_base_filter_for_image_tool_results(profile.image_tool_results)
        });
        let resolved_llm_identity = SessionLlmIdentity {
            model: model.clone(),
            provider,
            self_hosted_server_id: self_hosted_server_id.clone(),
            provider_params: build_config.provider_params.clone(),
            auth_binding: build_config.auth_binding.clone(),
        };
        #[cfg(not(target_arch = "wasm32"))]
        let auto_web_search_executor: Option<Arc<dyn meerkat_llm_core::WebSearchExecutor>> = {
            let active_model_has_native_search = model_profile
                .as_ref()
                .is_some_and(|profile| profile.supports_web_search);
            if build_config.web_search_executor_override.is_none()
                && !active_model_has_native_search
                && matches!(
                    build_config.override_web_search,
                    ToolCategoryOverride::Enable
                )
            {
                // The guard above already requires override_web_search ==
                // Enable, so this fallback is an explicit enable: it must
                // deliver a tool or fail closed (no silent Ok(None)).
                self.build_web_search_executor(
                    config,
                    &registry,
                    provider,
                    build_config.realm_id.as_ref(),
                    &build_config.runtime_build_mode,
                    true,
                )
                .await?
            } else {
                None
            }
        };
        if let meerkat_core::RuntimeBuildMode::SessionOwned(bindings) =
            &build_config.runtime_build_mode
        {
            let capability_base_filter_for_machine = capability_base_filter_override
                .clone()
                .unwrap_or(ToolFilter::All);
            bindings
                .model_routing()
                .set_baseline(
                    meerkat_core::lifecycle::run_primitive::ModelId::new(model.clone()),
                    model_profile
                        .as_ref()
                        .is_some_and(|profile| profile.realtime),
                )
                .map_err(|err| BuildAgentError::Config(format!("model routing baseline: {err}")))?;
            bindings
                .model_routing()
                .hydrate_llm_capability_surface(
                    &resolved_llm_identity,
                    model_profile.as_ref(),
                    &capability_base_filter_for_machine,
                )
                .map_err(|err| {
                    BuildAgentError::Config(format!("session LLM capability hydration: {err}"))
                })?;
        }
        let event_tap = meerkat_core::new_event_tap();
        let agent_llm_client_was_overridden = build_config.agent_llm_client_override.is_some();
        let raw_llm_client_was_overridden = build_config.llm_client_override.is_some();
        let llm_adapter: Arc<dyn AgentLlmClient> = if let Some(agent_client) =
            build_config.agent_llm_client_override.take()
        {
            agent_client
        } else {
            let llm_client = llm_client.ok_or_else(|| {
                BuildAgentError::Config(
                    "internal error: missing LLM client for adapter build".to_string(),
                )
            })?;
            let mut llm_adapter_inner = match build_config.event_tx.clone() {
                Some(tx) => LlmClientAdapter::try_with_event_channel_for_provider_identity(
                    llm_client,
                    model.clone(),
                    provider,
                    tx,
                )
                .map_err(|error| BuildAgentError::Config(error.to_string()))?,
                None => {
                    LlmClientAdapter::try_for_provider_identity(llm_client, model.clone(), provider)
                        .map_err(|error| BuildAgentError::Config(error.to_string()))?
                }
            };
            llm_adapter_inner = llm_adapter_inner.with_event_tap(event_tap.clone());
            // K2: the build config carries the typed `ProviderParamsOverride`
            // end-to-end; the adapter default is its provider tag (no legacy
            // JSON-bag projection at this seam).
            if let Some(typed_tag) = build_config
                .provider_params
                .as_ref()
                .and_then(|params| params.provider_tag.clone())
            {
                llm_adapter_inner = llm_adapter_inner.with_provider_params(Some(typed_tag));
            }
            Arc::new(llm_adapter_inner)
        };
        let llm_adapter = Self::decorate_agent_llm_client(
            llm_adapter,
            build_config.agent_llm_client_decorator.as_ref(),
        );
        let llm_adapter = if !agent_llm_client_was_overridden
            && !raw_llm_client_was_overridden
            && config.model_fallback.enabled
        {
            let explicit_fallback_chain = !config.model_fallback.chain.is_empty();
            let primary_request_policy = self.request_policy_for_llm_identity(
                config,
                &resolved_llm_identity,
                build_config.override_web_search,
            )?;
            let primary_target_profile = registry.profile_witness_for_provider(
                resolved_llm_identity.provider,
                &resolved_llm_identity.model,
            ).ok_or_else(|| {
                BuildAgentError::Config(format!(
                    "model fallback cannot bind active model '{}:{}' because it is absent from the effective model registry",
                    resolved_llm_identity.provider.as_str(),
                    resolved_llm_identity.model
                ))
            })?;
            let mut candidates = vec![ModelFallbackCandidate {
                identity: resolved_llm_identity.clone(),
                request_policy: primary_request_policy,
                client: Arc::clone(&llm_adapter),
                target_profile: primary_target_profile,
            }];
            let auth_lease_handle = match &build_config.runtime_build_mode {
                RuntimeBuildMode::SessionOwned(bindings) => Some(bindings.auth_lease().clone()),
                RuntimeBuildMode::StandaloneEphemeral => None,
            };
            for identity in
                self.model_fallback_identities(config, &registry, &resolved_llm_identity)?
            {
                let target_profile = match registry
                    .profile_witness_for_provider(identity.provider, &identity.model)
                {
                    Some(profile) => profile,
                    None => {
                        return Err(BuildAgentError::Config(format!(
                            "model fallback target '{}:{}' is absent from the effective model registry",
                            identity.provider.as_str(),
                            identity.model
                        )));
                    }
                };
                let raw_client = match self
                    .build_llm_client_for_identity_with_auth_lease(
                        config,
                        &identity,
                        auth_lease_handle.clone(),
                    )
                    .await
                {
                    Ok(client) => client,
                    Err(error) if explicit_fallback_chain => {
                        return Err(BuildAgentError::LlmClient(error));
                    }
                    Err(error) => {
                        tracing::debug!(
                            model = %identity.model,
                            provider = %identity.provider.as_str(),
                            error = %error,
                            "skipping unavailable catalog-default model fallback candidate"
                        );
                        continue;
                    }
                };
                let mut adapter = match build_config.event_tx.clone() {
                    Some(tx) => LlmClientAdapter::try_with_event_channel_for_provider_identity(
                        raw_client,
                        identity.model.clone(),
                        identity.provider,
                        tx,
                    )
                    .map_err(|error| BuildAgentError::Config(error.to_string()))?,
                    None => LlmClientAdapter::try_for_provider_identity(
                        raw_client,
                        identity.model.clone(),
                        identity.provider,
                    )
                    .map_err(|error| BuildAgentError::Config(error.to_string()))?,
                };
                adapter = adapter.with_event_tap(event_tap.clone());
                let decorated = Self::decorate_agent_llm_client(
                    Arc::new(adapter),
                    build_config.agent_llm_client_decorator.as_ref(),
                );
                let request_policy = self.request_policy_for_llm_identity(
                    config,
                    &identity,
                    build_config.override_web_search,
                )?;
                candidates.push(ModelFallbackCandidate {
                    target_profile,
                    request_policy,
                    identity,
                    client: decorated,
                });
            }
            match ModelFallbackClient::new(candidates) {
                Some(client) => Arc::new(client) as Arc<dyn AgentLlmClient>,
                None => llm_adapter,
            }
        } else {
            llm_adapter
        };

        // 5. Resolve max_tokens (config field is Option for realm-inheritance
        // presence; resolve the operative value here at point-of-use).
        let max_tokens = build_config
            .max_tokens
            .unwrap_or_else(|| config.resolved_max_tokens());
        let _realm_scope_root = self.realm_scope_root(&build_config);
        let _conventions_context_root = self
            .context_root
            .as_deref()
            .or(self.project_root.as_deref());
        let _conventions_user_root = self.user_config_root.as_deref();

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
                                    tracing::warn!(
                                        "Failed to resolve skill repositories: {e}; \
                                         degrading to embedded-only skill source"
                                    );
                                    Self::embedded_only_skill_source(&config.skills)?
                                }
                            }
                        }
                        #[cfg(target_arch = "wasm32")]
                        None
                    };

                skill_source
                    .map(|source| {
                        let available_caps =
                            self.effective_skill_capabilities(config, Some(&build_config));
                        let registry =
                            Arc::new(config.skills.build_source_identity_registry().map_err(
                                |e| {
                                    BuildAgentError::Config(format!(
                                        "failed to build skill source identity registry: {e}"
                                    ))
                                },
                            )?);
                        let engine =
                            meerkat_skills::DefaultSkillEngine::new(source, available_caps)
                                .with_inventory_threshold(config.skills.inventory_threshold)
                                .with_max_injection_bytes(config.skills.max_injection_bytes)
                                .with_source_identity_registry(registry);
                        let engine = Arc::new(engine);
                        Ok::<Arc<meerkat_core::skills::SkillRuntime>, BuildAgentError>(Arc::new(
                            meerkat_core::skills::SkillRuntime::new(engine),
                        ))
                    })
                    .transpose()?
            }; // end else (filesystem resolution fallthrough)
        #[cfg(not(feature = "skills"))]
        let skill_engine: Option<Arc<meerkat_core::skills::SkillRuntime>> = None;

        // 6b. Build tool dispatcher (with optional external tools, per-build overrides, skill tools)
        //
        // The typed per-request policy is persisted as-is on
        // `SessionBuildState.system_prompt` (the tri-state round-trips
        // losslessly, including `Disable`). The override itself is taken
        // (leaving `Inherit`) for prompt assembly below.
        let persisted_system_prompt = build_config.system_prompt.clone();
        let prompt_override = std::mem::take(&mut build_config.system_prompt);
        let effective_builtins = build_config.override_builtins.resolve(self.enable_builtins);
        #[allow(unused_variables)] // only consumed by non-wasm32 tool dispatcher
        let effective_shell = build_config.override_shell.resolve(self.enable_shell);
        let initial_tool_filter = build_config.initial_tool_filter.take();
        let _session_id = session.id().to_string();

        let resolved_mode = &build_config.runtime_build_mode;
        // Inject pre-resolved metadata entries after runtime binding
        // validation and before the builder reads metadata for early-stage
        // recovery. Canonical visibility facts are not accepted through this
        // map; inherited visibility uses the typed authority handoff below.
        Self::apply_initial_metadata_entries(&mut session, &build_config.initial_metadata_entries)?;
        let initial_visibility_state = build_config.initial_tool_visibility_state.take();

        #[cfg(feature = "comms")]
        {
            let mob_operator_tools_available = build_config.mob_tool_authority_context.is_some()
                && (build_config.mob_tools.is_some() || self.mob_tools.is_some());
            if mob_operator_tools_available
                && self.enable_comms
                && self.comms_runtime.is_none()
                && build_config.comms_name.is_none()
            {
                build_config.comms_name = Some(format!("mob-owner/{}", session.id()));
            }
        }

        // 6b. Create comms runtime before tool wiring.
        // A per-session override (DEC-P3H-3: the mob member-host
        // materializer pre-builds the member runtime with a durable
        // session-scoped keypair and the machine-claimed session identity)
        // wins outright: downcast fail-closed, participant name verified
        // against comms_name, config-mode construction skipped. peer_meta /
        // blob-store were already applied by the materializer.
        #[cfg(feature = "comms")]
        let session_comms_runtime_override = resolve_session_comms_runtime_override(
            build_config.session_comms_runtime_override.take(),
            build_config.comms_name.as_deref(),
        )?;
        // If the factory has a pre-built runtime (surface with stable identity),
        // use it directly. Otherwise create a per-session runtime from config.
        #[cfg(all(feature = "comms", not(target_arch = "wasm32")))]
        let comms_runtime = if let Some(runtime) = session_comms_runtime_override {
            Some(runtime)
        } else if let Some(ref shared) = self.comms_runtime
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
                        Arc::clone(bindings.session_claim_handle())
                    }
                    meerkat_core::RuntimeBuildMode::StandaloneEphemeral => {
                        meerkat_core::handles::DefaultSessionClaimRegistry::global()
                            as Arc<dyn meerkat_core::handles::SessionClaimHandle>
                    }
                };
            let mut runtime =
                crate::sdk::build_session_scoped_comms_runtime_from_config_scoped_with_silent_intents_typed(
                    config,
                    _realm_scope_root.as_path(),
                    self.user_config_root.as_deref(),
                    comms_name,
                    build_config.peer_meta.clone(),
                    // Realm ID is the comms inproc namespace boundary (transport
                    // string, not a typed realm carrier).
                    build_config
                        .realm_id
                        .as_ref()
                        .map(|realm| realm.as_str().to_string()),
                    session.id(),
                    silent_intents,
                    session_claim_handle,
                )
                .await
                ?;
            if let Some(blob_store) = build_config.blob_store_override.clone() {
                runtime.set_blob_store(blob_store);
            }
            Some(Arc::new(runtime))
        } else {
            None
        };
        #[cfg(all(feature = "comms", target_arch = "wasm32"))]
        let comms_runtime = if let Some(runtime) = session_comms_runtime_override {
            Some(runtime)
        } else if let Some(ref shared) = self.comms_runtime
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
                build_config
                    .realm_id
                    .as_ref()
                    .map(|realm| realm.as_str().to_string()),
                silent_intents,
            )
            .map_err(BuildAgentError::from)?;
            runtime.require_peer_comms_machine_authority();
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
        if build_config.session_comms_runtime_override.take().is_some() {
            // Fail closed: a pre-built comms runtime cannot be honored
            // without the comms feature; ignoring it would build the member
            // under the wrong (absent) identity.
            return Err(BuildAgentError::Config(
                "session comms runtime override requires the comms feature".to_string(),
            ));
        }
        #[cfg(not(feature = "comms"))]
        #[allow(clippy::no_effect_underscore_binding)]
        let _comms_runtime: Option<()> = None;

        #[cfg(feature = "comms")]
        if let RuntimeBuildMode::SessionOwned(bindings) = resolved_mode
            && let Some(runtime) = &comms_runtime
        {
            // Close the runtime-backed ingress window before any later build
            // work can interleave with already-started shared listeners.
            runtime.require_peer_comms_machine_authority();
            bindings
                .install_peer_comms_on(runtime.as_ref())
                .map_err(BuildAgentError::Comms)?;
        }

        if model_profile.is_some() {
            session.try_tool_visibility_state().map_err(|err| {
                BuildAgentError::Config(format!("invalid canonical tool visibility state: {err}"))
            })?;
        }
        // Resolve ops lifecycle registry via RuntimeBuildMode.
        #[allow(unused_variables)]
        let (ops_lifecycle, concrete_ops_lifecycle): (
            Arc<dyn OpsLifecycleRegistry>,
            Option<Arc<RuntimeOpsLifecycleRegistry>>,
        ) = match resolved_mode {
            RuntimeBuildMode::SessionOwned(bindings) => {
                (Arc::clone(bindings.ops_lifecycle()), None)
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

        // 6d. Materialize declarative MCP servers into a session-owned
        // router composed with any caller-supplied external tools. The
        // router is constructed against the build mode's canonical
        // external-tool surface authority, so the session-time
        // bind_external_tool_surface_handle below is a ptr-eq no-op instead
        // of a poisoning rebind; connection waits ride the unconditional
        // `wait_for_mcp` poll loop like every other external dispatcher.
        #[cfg(all(feature = "mcp", not(target_arch = "wasm32")))]
        if !build_config.mcp_servers.is_empty() {
            let surface_handle: Arc<dyn meerkat_core::ExternalToolSurfaceHandle> =
                match resolved_mode {
                    RuntimeBuildMode::SessionOwned(bindings) => {
                        Arc::clone(bindings.external_tool_surface())
                    }
                    _ => Arc::new(
                        meerkat_runtime::handles::RuntimeExternalToolSurfaceHandle::ephemeral(),
                    ),
                };
            let mut router = meerkat_mcp::McpRouter::new_with_surface_handle(surface_handle);
            for server in &build_config.mcp_servers {
                router.stage_add(server.clone()).map_err(|error| {
                    BuildAgentError::McpSetup(format!(
                        "stage MCP server '{}': {error}",
                        server.name
                    ))
                })?;
            }
            router.apply_staged().await.map_err(|error| {
                BuildAgentError::McpSetup(format!("apply staged MCP servers: {error}"))
            })?;
            let adapter = meerkat_mcp::McpRouterAdapter::new(router);
            adapter.refresh_tools().await.map_err(|error| {
                BuildAgentError::McpSetup(format!("refresh MCP tools: {error}"))
            })?;
            let adapter: Arc<dyn AgentToolDispatcher> = Arc::new(adapter);
            build_config.external_tools = Some(match build_config.external_tools.take() {
                Some(existing) => Arc::new(meerkat_core::gateway::DynamicToolComposite::new(vec![
                    existing, adapter,
                ])) as Arc<dyn AgentToolDispatcher>,
                None => adapter,
            });
        }
        #[cfg(not(all(feature = "mcp", not(target_arch = "wasm32"))))]
        if !build_config.mcp_servers.is_empty() {
            // Fail closed: silently dropping requested MCP servers would be
            // a capability downgrade the caller cannot observe.
            return Err(BuildAgentError::McpSetup(
                "mcp_servers were requested but this build lacks the 'mcp' feature (or targets wasm32)"
                    .to_string(),
            ));
        }

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
                        build_config.external_tools.take(),
                        effective_builtins,
                        effective_shell,
                        skill_engine.clone(),
                        build_config.shell_env.clone(),
                        _session_id.clone(),
                        Arc::clone(&ops_lifecycle),
                        build_config
                            .image_generation_machine_override
                            .clone()
                            .or_else(|| self.image_generation_machine.clone()),
                        build_config
                            .image_generation_executor_override
                            .clone()
                            .or_else(|| auto_image_generation_executor.clone())
                            .or_else(|| {
                                image_generation_planner.as_ref().map(|_| {
                                    Arc::new(UnavailableImageGenerationExecutor)
                                        as Arc<dyn meerkat_llm_core::ImageGenerationExecutor>
                                })
                            }),
                        image_generation_planner.clone(),
                        build_config.blob_store_override.clone(),
                        build_config.override_image_generation,
                        build_config
                            .web_search_executor_override
                            .clone()
                            .or_else(|| auto_web_search_executor.clone()),
                        if model_profile
                            .as_ref()
                            .is_some_and(|profile| profile.supports_web_search)
                        {
                            ToolCategoryOverride::Disable
                        } else {
                            build_config.override_web_search
                        },
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
            tools.bind_external_tool_surface_handle(Arc::clone(bindings.external_tool_surface()));
            tools.bind_mcp_server_lifecycle_handle(Arc::clone(bindings.mcp_server_lifecycle()));
            // W1-A/U6: install the typed machine authority required before
            // comms can emit semantic peer request/response receipts.
            #[cfg(feature = "comms")]
            if let Some(runtime) = &comms_runtime {
                runtime.install_peer_request_response_authority(
                    meerkat_comms::PeerRequestResponseAuthority::new(
                        Arc::clone(bindings.peer_interaction()),
                        Arc::clone(bindings.interaction_stream()),
                    ),
                );
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
            // Scheduled spawn/fork-helper targets fire later under host
            // authority; the creating session's effective tool access policy
            // must be applied at creation or a gated agent could schedule a
            // helper that escapes its own containment.
            let creator_tool_access_policy = build_config.tool_access_policy.clone();
            let schedule_dispatcher = match build_config.mob_member_binding.clone() {
                Some(binding) => Arc::new(
                    meerkat_schedule::CurrentSessionScheduleToolDispatcher::new_with_resolver(
                        schedule_dispatcher,
                        session.id().clone(),
                        Arc::new(
                            crate::surface::MobMemberCurrentSessionScheduleResolver::new(binding),
                        ),
                    )
                    .with_creator_tool_access_policy(creator_tool_access_policy),
                ) as Arc<dyn AgentToolDispatcher>,
                None => Arc::new(
                    meerkat_schedule::CurrentSessionScheduleToolDispatcher::new(
                        schedule_dispatcher,
                        session.id().clone(),
                    )
                    .with_creator_tool_access_policy(creator_tool_access_policy),
                ) as Arc<dyn AgentToolDispatcher>,
            };
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

        // 9c. Compose tools with WorkGraph surface (after scheduler, before mob).
        let effective_workgraph = build_config
            .override_workgraph
            .resolve(self.enable_workgraph);
        if effective_workgraph {
            let workgraph_dispatcher = match build_config.workgraph_tools.take() {
                Some(dispatcher) => dispatcher,
                None => {
                    #[cfg(not(target_arch = "wasm32"))]
                    {
                        return Err(BuildAgentError::Config(
                            "WorkGraph tools enabled without a supplied dispatcher; non-wasm \
                             surfaces must install WorkGraph tools from the persistence bundle"
                                .to_string(),
                        ));
                    }
                    #[cfg(target_arch = "wasm32")]
                    {
                        // Wasm has no host SQLite persistence bundle; keep the
                        // explicitly documented in-memory fallback scoped by the
                        // typed realm identity.
                        let Some(realm_id) = build_config.realm_id.as_ref() else {
                            return Err(BuildAgentError::Config(
                                "WorkGraph tools enabled with no supplied dispatcher and no realm \
                                 identity; supply a WorkGraph dispatcher or a realm-scoped build"
                                    .to_string(),
                            ));
                        };
                        let realm_scope = realm_id.as_str().to_string();
                        meerkat_workgraph::wire_workgraph_tools(
                            meerkat_workgraph::WorkGraphService::with_scope(
                                Arc::new(meerkat_workgraph::MemoryWorkGraphStore::new()),
                                realm_scope,
                                meerkat_workgraph::WorkNamespace::default(),
                            ),
                        )
                    }
                }
            };
            let workgraph_usage = render_tool_usage_instructions(workgraph_dispatcher.as_ref());
            tools = Arc::new(meerkat_core::DynamicToolComposite::new(vec![
                tools,
                workgraph_dispatcher,
            ]));
            if !workgraph_usage.is_empty() {
                if !tool_usage_instructions.is_empty() {
                    tool_usage_instructions.push_str("\n\n");
                }
                tool_usage_instructions.push_str(&workgraph_usage);
            }
        }

        tracing::debug!(
            tool_count_after_workgraph = tools.tools().len(),
            effective_workgraph,
            "tool composition: after workgraph gateway"
        );

        // 9d. Compose tools with mob surface (after comms/scheduler/WorkGraph, so mob
        // gateway wraps the already-composed base capability stack).
        let effective_mob = build_config
            .mob_tool_authority_context
            .as_ref()
            .is_some_and(
                meerkat_core::service::MobToolAuthorityContext::is_generated_authority_context,
            );
        let mob_factory = build_config
            .mob_tools
            .take()
            .or_else(|| self.mob_tools.clone());
        // Shared mob authority handle — created inside the mob block, hoisted
        // out so the agent builder can reference it after the block.
        let mut hoisted_mob_authority_handle: Option<
            Arc<std::sync::RwLock<meerkat_core::service::MobToolAuthorityContext>>,
        > = None;
        // Hoisted so the final built ToolScope can become the parent
        // composition authority's live generated visibility source.
        let mut hoisted_parent_tool_authority: Option<
            meerkat_core::service::ParentToolCompositionAuthority,
        > = None;
        if effective_mob && let Some(mob_factory) = mob_factory {
            // Build comms runtime arg: clone from the comms phase if available.
            #[cfg(feature = "comms")]
            let mob_comms: Option<Arc<dyn meerkat_core::agent::CommsRuntime>> = comms_runtime
                .as_ref()
                .map(|r| Arc::clone(r) as Arc<dyn meerkat_core::agent::CommsRuntime>);
            #[cfg(not(feature = "comms"))]
            let mob_comms: Option<Arc<dyn meerkat_core::agent::CommsRuntime>> = None;
            #[cfg(feature = "comms")]
            let mob_comms_name = build_config
                .comms_name
                .clone()
                .or_else(|| mob_comms.as_ref().and_then(|runtime| runtime.comms_name()));
            #[cfg(not(feature = "comms"))]
            let mob_comms_name = build_config.comms_name.clone();

            // Create the shared effective-authority handle. The agent owns the
            // write side (via apply_session_effects); mob tools get a read view.
            let mob_authority_handle = build_config
                .mob_tool_authority_context
                .as_ref()
                .map(|ctx| Arc::new(std::sync::RwLock::new(ctx.clone())));
            hoisted_mob_authority_handle = mob_authority_handle.clone();

            // Use an AgentFactory-minted parent composition authority: created
            // before mob composition so it can be passed into MobToolsBuildArgs.
            // The built agent's final ToolScope is installed after all tool
            // composition and initial visibility staging, so InheritParent reads
            // generated visibility authority rather than the raw dispatcher.
            // SAFETY: this is the canonical facade AgentFactory composition
            // after generated mob authority is present; core validates the
            // facade bridge token before minting the parent authority.
            #[allow(unsafe_code)]
            let parent_tool_authority = unsafe {
                core_agent_factory_parent_tool_composition_authority_new(
                    agent_factory_policy_bridge_token(),
                )
            }
            .map_err(|err| {
                BuildAgentError::Config(format!(
                    "Parent tool composition authority creation failed: {err}"
                ))
            })?;
            let mob_args = meerkat_core::service::MobToolsBuildArgs {
                session_id: session.id().clone(),
                model: model.clone(),
                authority_context: build_config.mob_tool_authority_context.clone(),
                effective_authority: mob_authority_handle.clone(),
                comms_name: mob_comms_name,
                comms_runtime: mob_comms,
                snapshot_context: meerkat_core::service::MobToolSnapshotContext::ParentOwned(
                    parent_tool_authority.clone(),
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
            hoisted_parent_tool_authority = Some(parent_tool_authority);
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
                // Generate inventory section for system prompt. A generation
                // fault must propagate a typed build error — never be lowered
                // into fabricated `<available_skills state="unavailable">`
                // prompt content. Skills are enabled here (skill_engine is
                // Some), so an inventory failure fails the build closed.
                let inventory = engine.inventory_section().await.map_err(|e| {
                    BuildAgentError::Config(format!("skill inventory generation failed: {e}"))
                })?;

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
                    // An inventory-listing fault is an infrastructure failure,
                    // not per-skill absence: collapsing it to an empty set
                    // would mislabel every persisted skill as NotFound and
                    // silently strip prompt activation. Fail the build closed.
                    let available: std::collections::HashSet<_> = engine
                        .list_skills(&meerkat_core::skills::SkillFilter::default())
                        .await
                        .map(|descs| descs.into_iter().map(|desc| desc.key).collect())
                        .map_err(|e| {
                            BuildAgentError::Config(format!(
                                "failed to list skills while resolving persisted active skills: {e}"
                            ))
                        })?;
                    let requested_ids = std::mem::take(ids);
                    let mut retained = std::collections::HashSet::new();
                    // Track dropped persisted skills as typed (key, reason) pairs
                    // so the caller receives a structured recovery signal rather
                    // than a silently-stripped prompt activation.
                    let mut dropped: Vec<(
                        meerkat_core::skills::SkillKey,
                        meerkat_core::SkillResolutionFailureReason,
                    )> = Vec::new();
                    for requested in requested_ids {
                        match engine.canonical_skill_key(&requested).await {
                            Ok(canonical) if available.contains(&canonical) => {
                                if retained.insert(canonical.clone()) {
                                    ids.push(canonical);
                                }
                            }
                            Ok(canonical) => {
                                // Resolved to a canonical key that this surface
                                // does not expose: the requested skill is not
                                // available on resume.
                                dropped.push((
                                    requested,
                                    meerkat_core::SkillResolutionFailureReason::NotFound {
                                        key: canonical,
                                    },
                                ));
                            }
                            Err(err) => {
                                dropped.push((
                                    requested,
                                    meerkat_core::SkillResolutionFailureReason::Load {
                                        message: err.to_string(),
                                    },
                                ));
                            }
                        }
                    }
                    if !dropped.is_empty() {
                        // Surface a typed recovery signal: the caller decides how
                        // to react to persisted skills that no longer resolve on
                        // this surface. We do not silently strip prompt activation.
                        if let Some(ref tx) = build_config.event_tx {
                            for (requested, reason) in &dropped {
                                let _ = tx
                                    .send(AgentEvent::SkillResolutionFailed {
                                        skill_key: Some(requested.clone()),
                                        reason: reason.clone(),
                                    })
                                    .await;
                            }
                        }
                        tracing::warn!(
                            dropped_skill_count = dropped.len(),
                            "persisted active skills are unavailable on the current surface; \
                             emitted typed SkillResolutionFailed recovery signal"
                        );
                    }
                    if ids.is_empty() {
                        preload = None;
                    }
                }

                // Pre-load requested skills into system prompt (Level 2)
                let mut preloaded_sections = Vec::new();
                let mut skill_ids = None;
                if let Some(ref ids) = preload {
                    match engine.resolve_and_render(ids).await {
                        Ok(resolved) => {
                            let mut resolved_ids = Vec::with_capacity(resolved.len());
                            for skill in &resolved {
                                resolved_ids.push(skill.key.clone());
                                preloaded_sections.push(skill.rendered_body.clone());
                            }
                            if !resolved_ids.is_empty() {
                                skill_ids = Some(resolved_ids);
                            }
                        }
                        Err(e) => {
                            return Err(BuildAgentError::Config(format!(
                                "Failed to preload skill: {e}"
                            )));
                        }
                    }
                }

                // Persist the canonical skills actually activated for this session.

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
            Option<Vec<meerkat_core::skills::SkillKey>>,
        ) = (String::new(), Vec::new(), None);
        let (inventory_section, preloaded_skill_sections, active_skill_ids) =
            skill_inventory_section;

        // 12. Build system prompt (single canonical path)
        let mut extra_sections: Vec<&str> = Vec::new();
        // Only inject skill inventory (with tool guidance) when builtins are
        // enabled — otherwise browse_skills/load_skill don't exist — and only
        // for `Full` prompt-section builds (A1/ADJ-17: SpecPinned mob-
        // materialized builds keep spec-built prompts byte-pinned).
        if !inventory_section.is_empty()
            && effective_builtins
            && skill_inventory_prompt_section_enabled(build_config.host_prompt_sections)
        {
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
        let resume_session_is_precreated_empty = build_config
            .resume_session
            .as_ref()
            .is_some_and(|session| session.messages().is_empty());
        // An explicit per-request policy (`Set` or `Disable`) forces prompt
        // (re)assembly even on resume; `Inherit` only assembles for fresh or
        // pre-created-empty sessions.
        let should_apply_system_prompt = build_config.resume_session.is_none()
            || resume_session_is_precreated_empty
            || prompt_override.is_explicit();
        #[cfg(not(target_arch = "wasm32"))]
        let system_prompt = if should_apply_system_prompt {
            Some(
                crate::assemble_system_prompt(
                    config,
                    &prompt_override,
                    _conventions_context_root,
                    &extra_sections,
                    &tool_usage_instructions,
                )
                .await
                .map_err(|err| BuildAgentError::Config(err.to_string()))?,
            )
        } else {
            None
        };
        #[cfg(target_arch = "wasm32")]
        let system_prompt = if should_apply_system_prompt {
            Some(
                crate::prompt_policy::assemble_system_prompt_without_filesystem(
                    config,
                    &prompt_override,
                    &extra_sections,
                    &tool_usage_instructions,
                ),
            )
        } else {
            None
        };

        // 11f. Wait for pending MCP connections when requested.
        //
        // poll_external_updates() is forwarded through the full dispatcher
        // chain (CompositeDispatcher → ToolGateway → McpRouterAdapter), so
        // this drains background MCP connection results regardless of how
        // many dispatchers are composed.
        //
        // The contract is unconditional: `wait_for_mcp` blocks until every
        // server finishes connecting (success or failure). Connection-timeout
        // policy is owned by the per-server `connect_timeout_secs` in the MCP
        // client — the factory does NOT own a second, hard-coded timeout that
        // silently releases the first turn while servers are still pending.
        if build_config.wait_for_mcp {
            loop {
                let update = tools.poll_external_updates().await;
                if update.pending.is_empty() {
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
            // Record the exact assembled base-prompt bytes (or carry the
            // prior build's record through an Inherit resume) so a later
            // resume can split the persisted System content into base +
            // runtime-appended tail byte-exactly.
            assembled_system_prompt: system_prompt.clone().or_else(|| {
                build_config
                    .resume_session
                    .as_ref()
                    .and_then(|session| session.build_state())
                    .and_then(|state| state.assembled_system_prompt)
            }),
        };

        // Resolve the structured-output retry budget exactly once, here at the
        // AgentFactory seam: build options carry intent (`Option<u32>`, `None`
        // = inherit). The canonical owner is the deployment config field
        // `config.agent.structured_output_retries` (whose own serde default is
        // `meerkat_core::config::default_structured_output_retries`); surfaces
        // never fabricate this default.
        let resolved_structured_output_retries = build_config
            .structured_output_retries
            .unwrap_or(config.agent.structured_output_retries);

        // Resolve the per-launch tool access policy into the sealed execution
        // form BEFORE session metadata is persisted: an unresolved `Inherit`
        // is a wiring fault (the spawn chain owns Inherit resolution — a child
        // inherits its parent's persisted effective policy; host launches with
        // no parent resolve to unrestricted) and must fail the build closed
        // rather than persist or silently un-gate the session.
        let tool_execution_policy = build_config
            .tool_access_policy
            .clone()
            .map(meerkat_core::ToolExecutionPolicy::resolve)
            .transpose()
            .map_err(|err| BuildAgentError::Config(format!("Tool access policy: {err}")))?;

        // Persist the *override intent* (Inherit/Enable/Disable), not the resolved
        // effective bool. This ensures Inherit survives across save/resume cycles so
        // the session continues to follow future runtime defaults.
        let factory_metadata = if let Some(mut metadata) = resumed_session_metadata {
            metadata.model = model.clone();
            metadata.max_tokens = max_tokens;
            metadata.structured_output_retries = resolved_structured_output_retries;
            metadata.provider = provider;
            metadata.self_hosted_server_id = self_hosted_server_id.clone();
            metadata.provider_params = build_config.provider_params.clone();
            metadata.tooling.builtins = build_config.override_builtins;
            metadata.tooling.shell = build_config.override_shell;
            // Apply an explicit comms override on resume; preserve the persisted
            // value when the build carries no explicit comms intent (Inherit) so
            // a prior explicit Enable/Disable survives across resumes.
            if !matches!(build_config.override_comms, ToolCategoryOverride::Inherit) {
                metadata.tooling.comms = build_config.override_comms;
            }
            metadata.tooling.mob = build_config.override_mob;
            metadata.tooling.memory = metadata_memory_override_for_realm(
                build_config.override_memory,
                build_config.backend,
            );
            metadata.tooling.schedule = build_config.override_schedule;
            metadata.tooling.workgraph = build_config.override_workgraph;
            metadata.tooling.image_generation = build_config.override_image_generation;
            metadata.tooling.web_search = build_config.override_web_search;
            // Effective (resolved) policy only — an unresolved `Inherit`
            // failed the build above, so `Inherit` can never persist here.
            metadata.tooling.tool_access_policy = build_config.tool_access_policy.clone();
            if build_config.resume_override_mask.preload_skills || active_skill_ids.is_some() {
                metadata.tooling.active_skills = active_skill_ids.clone();
            }
            metadata.keep_alive = build_config.keep_alive;
            metadata.comms_name = build_config.comms_name.clone();
            metadata.peer_meta = build_config.peer_meta.clone();
            metadata.realm_id = build_config.realm_id.clone();
            metadata.instance_id = build_config.instance_id.clone();
            metadata.backend = build_config.backend.map(|b| b.as_str().to_string());
            metadata.config_generation = build_config.config_generation;
            metadata.auth_binding = build_config.auth_binding.clone();
            metadata.mob_member_binding = build_config.mob_member_binding.clone();
            metadata
        } else {
            SessionMetadata {
                schema_version: meerkat_core::SESSION_METADATA_SCHEMA_VERSION,
                model: model.clone(),
                max_tokens,
                structured_output_retries: resolved_structured_output_retries,
                provider,
                self_hosted_server_id: self_hosted_server_id.clone(),
                provider_params: build_config.provider_params.clone(),
                tooling: SessionTooling {
                    builtins: build_config.override_builtins,
                    shell: build_config.override_shell,
                    comms: build_config.override_comms,
                    mob: build_config.override_mob,
                    memory: metadata_memory_override_for_realm(
                        build_config.override_memory,
                        build_config.backend,
                    ),
                    schedule: build_config.override_schedule,
                    workgraph: build_config.override_workgraph,
                    image_generation: build_config.override_image_generation,
                    web_search: build_config.override_web_search,
                    // Effective (resolved) policy only — an unresolved
                    // `Inherit` failed the build above.
                    tool_access_policy: build_config.tool_access_policy.clone(),
                    active_skills: active_skill_ids.clone(),
                },
                keep_alive: build_config.keep_alive,
                comms_name: build_config.comms_name.clone(),
                peer_meta: build_config.peer_meta.clone(),
                realm_id: build_config.realm_id.clone(),
                instance_id: build_config.instance_id.clone(),
                backend: build_config.backend.map(|b| b.as_str().to_string()),
                config_generation: build_config.config_generation,
                auth_binding: build_config.auth_binding.clone(),
                mob_member_binding: build_config.mob_member_binding.clone(),
            }
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
        let effective_model_registry = Arc::new(registry.clone());

        let mut builder = AgentBuilder::new()
            .model(model.clone())
            .max_tokens_per_turn(max_tokens)
            .budget(budget_limits)
            .structured_output_retries(resolved_structured_output_retries)
            .retry_policy(meerkat_core::retry::RetryPolicy {
                call_timeout: None,
                ..config.retry.clone().into()
            })
            .with_hook_run_overrides(build_config.hooks_override)
            .with_model_defaults_resolver(Arc::new(RegistryBackedDefaultsResolver {
                registry: Arc::clone(&effective_model_registry),
            }))
            .with_effective_model_registry(effective_model_registry)
            // Config-owned tool execution policy (per-call timeout, per-tool
            // overrides, dispatch concurrency) reaches the agent loop through
            // this seam; without it every factory-built agent silently ran on
            // `ToolsConfig::default()`.
            .with_tools_config(config.tools.clone())
            .with_call_timeout_override(effective_call_timeout_override);

        if let Some(defaults) = provider_tool_defaults_for(
            provider,
            config,
            model_profile.as_ref(),
            build_config.override_web_search,
        ) {
            builder = builder.provider_tool_defaults(defaults);
        }
        if let Some(params) = build_config.provider_params.clone() {
            builder = builder.provider_params(params);
        }
        if let Some(capability_base_filter) = capability_base_filter_override.clone() {
            builder = builder.with_capability_base_filter(capability_base_filter);
        }
        if let Some(state) = initial_visibility_state {
            builder = builder.with_initial_tool_visibility_state(state);
        }
        // Resume continuity: an explicit per-request prompt on a resumed,
        // established transcript must not blind-replace the persisted System
        // message — that discards runtime-applied system context and produces
        // a projection that is no longer a continuation of the persisted
        // transcript revision, so the append-only continuity guard rejects
        // the very first post-resume persist and the live session is
        // discarded (cold-restart transcript loss for runtime-backed hosts).
        // Reconcile instead: a base the persisted prompt already carries is
        // preserved byte-for-byte; a genuinely changed base is committed as a
        // typed transcript rewrite so the persist proves a graph edge from
        // the persisted head.
        let system_prompt = match system_prompt {
            Some(assembled)
                if build_config.resume_session.is_some() && !resume_session_is_precreated_empty =>
            {
                let reconciliation = session
                    .reconcile_resumed_system_prompt(
                        assembled,
                        Some("agent-factory/resume".to_string()),
                    )
                    .map_err(|err| {
                        BuildAgentError::Config(format!(
                            "failed to reconcile resumed system prompt: {err}"
                        ))
                    })?;
                tracing::debug!(
                    session_id = %session.id(),
                    ?reconciliation,
                    "reconciled explicit system prompt against resumed transcript"
                );
                None
            }
            other => other,
        };
        if let Some(system_prompt) = system_prompt {
            builder = builder.system_prompt(system_prompt);
        }

        if let Some(schema) = build_config.output_schema {
            builder = builder.output_schema(schema);
        }
        let _is_resumed = build_config.resume_session.is_some();
        let session_id = session.id().clone();
        session
            .set_session_metadata(factory_metadata)
            .map_err(|err| {
                BuildAgentError::Config(format!(
                    "Failed to store session metadata before core build: {err}"
                ))
            })?;
        session
            .set_build_state(persisted_build_state)
            .map_err(|err| {
                BuildAgentError::Config(format!(
                    "Failed to store session build state before core build: {err}"
                ))
            })?;
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
        let effective_memory = effective_memory_for_realm(
            build_config.override_memory,
            self.enable_memory,
            build_config.backend,
        );
        #[cfg(feature = "memory-store-session")]
        if effective_memory {
            let memory_dir = self.store_path.join("memory");
            match meerkat_memory::HnswMemoryStore::open(&memory_dir) {
                Ok(store) => {
                    let store = Arc::new(store) as Arc<dyn meerkat_core::memory::MemoryStore>;
                    builder = builder.memory_store(Arc::clone(&store));

                    // Compose memory_search tool into the dispatcher
                    let memory_scope =
                        meerkat_core::memory::MemorySearchScope::for_session(session_id.clone());
                    let memory_dispatcher = meerkat_memory::MemorySearchDispatcher::new(
                        Arc::clone(&store),
                        memory_scope,
                    );
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
                    // Memory is only effective when enabled explicitly — by a
                    // per-build `Enable` override or by the surface-level
                    // factory enable (which carries config truth). An enabled
                    // capability must deliver memory_search or fail closed;
                    // building an agent without the promised tool would
                    // silently misreport the session's capability truth.
                    return Err(BuildAgentError::CapabilityUnavailable {
                        capability: "memory",
                        reason: format!(
                            "failed to open HnswMemoryStore at {}: {e}",
                            memory_dir.display()
                        ),
                    });
                }
            }
        }
        #[cfg(not(feature = "memory-store-session"))]
        if effective_memory {
            // Mirror of the open-failure arm above: an effective memory
            // enable on a build without the `memory-store-session` feature
            // can never deliver the promised `memory_search` tool. Fail
            // closed instead of silently producing a memory-less agent that
            // misreports the session's capability truth.
            return Err(BuildAgentError::CapabilityUnavailable {
                capability: "memory",
                reason: "meerkat was built without the `memory-store-session` feature".to_string(),
            });
        }

        // 12c. Wire compactor (when session-compaction is enabled)
        #[cfg(feature = "session-compaction")]
        {
            let compactor = Arc::new(meerkat_session::DefaultCompactor::new(
                model_aware_compaction_config(
                    config,
                    &registry,
                    provider,
                    &model,
                    build_config.auto_compact_threshold_override,
                ),
            ));
            builder = builder.compactor(compactor);
        }
        // Host-supplied summary curator: substitutes summary content
        // production only; the trigger/rebuild stay with the compactor above.
        if let Some(curator) = build_config.compaction_curator_override {
            builder = builder.compaction_curator(curator);
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
        match resolved_mode {
            RuntimeBuildMode::SessionOwned(bindings) => {
                builder = builder.with_epoch_cursor_state(Arc::clone(bindings.cursor_state()));
                builder =
                    builder.with_tool_visibility_owner(bindings.tool_visibility_owner().clone());
                builder = builder.with_turn_state_handle(Arc::clone(bindings.turn_state()));
                builder = builder.with_model_routing_handle(Arc::clone(bindings.model_routing()));
                builder = builder.with_sticky_model_fallback_commit_coordinator(Arc::clone(
                    bindings.sticky_model_fallback_commit_coordinator(),
                ));
                builder = builder.with_compaction_commit_coordinator(Arc::clone(
                    bindings.compaction_commit_coordinator(),
                ));
                builder = builder.require_runtime_execution_kind_stamp();
                builder = builder.with_external_tool_surface_handle(Arc::clone(
                    bindings.external_tool_surface(),
                ));
                builder = builder.with_auth_lease_handle(bindings.auth_lease().clone());
                builder = builder
                    .with_mcp_server_lifecycle_handle(Arc::clone(bindings.mcp_server_lifecycle()));
            }
            RuntimeBuildMode::StandaloneEphemeral => {
                let capability_base_filter_for_machine = capability_base_filter_override
                    .clone()
                    .unwrap_or(ToolFilter::All);
                let authorities = meerkat_runtime::standalone_session_runtime_authorities(
                    &session_id,
                    &resolved_llm_identity,
                    model_profile.as_ref(),
                    &capability_base_filter_for_machine,
                )
                .map_err(|err| {
                    BuildAgentError::Config(format!(
                        "Failed to prepare standalone session authority: {err}"
                    ))
                })?;
                builder = builder
                    .with_turn_state_handle(Arc::clone(authorities.turn_state()))
                    .with_model_routing_handle(Arc::clone(authorities.model_routing()))
                    .with_tool_visibility_owner(authorities.tool_visibility_owner().clone());
            }
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

        // 12i. Call-level tool execution gate — the OUTERMOST composition.
        // Applied after every gateway/memory/catalog-control composition so
        // memory_search and every gateway-composed tool is gated. The gate
        // forwards `tools()`/`tool_catalog()` byte-identically (the
        // LLM-visible list — and therefore the prompt-cache prefix — is
        // unchanged); a denied call surfaces as an ordinary `access_denied`
        // tool error that the agent loop converts into an `is_error` tool
        // result, and the run continues. The ops-lifecycle binding (step 9d)
        // ran on the inner dispatcher before this wrap, and the wrapper
        // forwards `bind_mcp_server_lifecycle_handle` /
        // `bind_external_tool_surface_handle` so later handle binds still
        // reach the inner dispatcher. Provider-native server tools never
        // traverse the dispatcher and cannot be gated here; hosts that need
        // them gated must disable the native capability on gated builds.
        if let Some(policy) = tool_execution_policy {
            tools = Arc::new(meerkat_core::ExecutionPolicyGatedDispatcher::new(
                tools, policy,
            ));
        }

        // 13. Build agent. AgentFactory owns the policy composition above; core
        // validates that the durable policy metadata/runtime handle exists
        // before constructing the agent.
        // SAFETY: this is the canonical facade factory after provider,
        // runtime, auth, session, tool, and metadata policy composition above.
        #[allow(unsafe_code)]
        let mut agent = unsafe {
            core_agent_factory_policy_build(
                agent_factory_policy_bridge_token(),
                builder,
                llm_adapter,
                tools,
                store_adapter,
            )
            .await
        }
        .map_err(|err| {
            BuildAgentError::Config(format!("AgentFactory policy validation failed: {err}"))
        })?;

        if let Some(provider) = hoisted_control_visibility_provider {
            provider.set_scope(agent.tool_scope().clone());
        }

        if let Some(filter) = initial_tool_filter {
            agent
                .stage_external_tool_filter(filter)
                .map_err(|err| BuildAgentError::Config(err.to_string()))?;
            let base_tools = agent
                .tool_scope()
                .base_tools_snapshot()
                .map_err(|err| BuildAgentError::Config(err.to_string()))?;
            agent
                .tool_scope()
                .apply_staged(base_tools)
                .map_err(|err| BuildAgentError::Config(err.to_string()))?;
        }

        if let Some(ref authority) = hoisted_parent_tool_authority {
            // SAFETY: same AgentFactory bridge token as creation; core validates
            // the token before accepting the live ToolScope as the parent-owned
            // generated visibility source.
            #[allow(unsafe_code)]
            unsafe {
                core_agent_factory_parent_tool_composition_authority_set_tool_scope(
                    agent_factory_policy_bridge_token(),
                    authority,
                    agent.tool_scope(),
                )
            }
            .map_err(|err| {
                BuildAgentError::Config(format!(
                    "Parent tool composition authority update failed: {err}"
                ))
            })?;
        }

        // Wire mob authority handle into agent for session-effect application.
        if let Some(handle) = hoisted_mob_authority_handle {
            agent.set_mob_authority_handle(handle);
        }

        Ok(agent)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use meerkat_core::config::ModelFallbackTarget;
    #[cfg(feature = "skills")]
    use meerkat_core::skills::{
        SkillDocument, SkillKey, SkillKeyRemap, SkillName, SkillRuntime, SourceIdentityLineage,
        SourceIdentityLineageEvent, SourceIdentityRecord, SourceIdentityRegistry,
        SourceIdentityStatus, SourceTransportKind, SourceUuid,
    };
    use meerkat_core::types::ToolCallView;
    use meerkat_core::{
        AuthProfileConfig, BackendProfileConfig, BindingId, BindingOrigin, BlobId, BlobPayload,
        BlobRef, BlobStoreError, CredentialSourceSpec, ProviderBindingConfig, RealmConfigSection,
        SelfHostedApiStyle, SelfHostedModelConfig, SelfHostedServerConfig, SelfHostedTransport,
    };
    use std::collections::HashMap;
    use tokio::sync::Mutex;

    #[test]
    fn mob_member_current_session_schedule_resolver_persists_role_free_identity_target() {
        let mob_binding = meerkat_core::MobMemberBinding {
            mob_id: "ops".to_string(),
            role: "old-profile".to_string(),
            member: "deploy-monitor".to_string(),
        };
        let resolver =
            crate::surface::MobMemberCurrentSessionScheduleResolver::new(mob_binding.clone());
        let target =
            meerkat_schedule::CurrentSessionScheduleTargetResolver::resolve_current_session_target(
                &resolver,
                &SessionId::new(),
                meerkat_schedule::ScheduledSessionAction::Prompt {
                    prompt: meerkat_core::ContentInput::Text("check".to_string()),
                    system_prompt: None,
                    render_metadata: None,
                    skill_refs: Vec::new(),
                    additional_instructions: Vec::new(),
                },
            );

        let meerkat_schedule::TargetBinding::Identity(binding) = target else {
            panic!("mob-member-backed current_session should persist an identity target");
        };
        assert_eq!(
            binding.identity(),
            crate::surface::mob_member_schedule_identity(&mob_binding)
        );
        assert!(
            !binding.identity().contains("old-profile"),
            "role/profile must not be part of the durable schedule identity"
        );
    }

    /// A faulted default TokenStore must surface its typed open fault at the
    /// resolution seam — never collapse to "no store attached" (which would
    /// launder the fault into `AuthError::InteractiveLoginRequired`).
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn faulted_default_token_store_propagates_typed_open_fault() {
        let temp = tempfile::tempdir().unwrap();
        let mut factory = AgentFactory::new(temp.path().join("sessions"));
        factory.provider_auth_persistence = ProviderAuthPersistenceAttachment::OpenFailed(
            Arc::new(meerkat_providers::auth_store::TokenStoreError::Unavailable(
                "injected open failure".into(),
            )),
        );

        let Err(err) = factory.resolution_token_store() else {
            panic!("a faulted credential backend must fail resolution typed");
        };
        assert!(
            matches!(err, FactoryError::TokenStore(_)),
            "expected FactoryError::TokenStore, got: {err:?}"
        );

        // Deliberately-detached factories resolve cleanly to "no store".
        let detached = AgentFactory::minimal();
        assert!(detached.resolution_token_store().unwrap().is_none());
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn provider_auth_persistence_replacement_and_detach_are_atomic() {
        let temp = tempfile::tempdir().unwrap();
        let factory = AgentFactory::new(temp.path().join("sessions"));
        assert!(
            factory
                .resolution_provider_auth_persistence()
                .unwrap()
                .is_some()
        );

        let persistence = meerkat_providers::auth_store::ProviderAuthPersistence::new(
            Arc::new(meerkat_providers::auth_store::EphemeralTokenStore::new()),
            Arc::new(meerkat_providers::auth_store::InMemoryCoordinator::new()),
        );
        let factory = factory.with_provider_auth_persistence(persistence);
        assert!(
            factory
                .resolution_provider_auth_persistence()
                .unwrap()
                .is_some(),
            "replacement attaches one complete persistence capability"
        );

        let factory = factory.without_provider_auth_persistence();
        assert!(
            factory
                .resolution_provider_auth_persistence()
                .unwrap()
                .is_none()
        );
    }

    /// Every shipping native surface (CLI, REST, RPC, and MCP server) enters
    /// provider auth through `AgentFactory::new`. Two independently-created
    /// factories over one persisted backend must therefore serialize the
    /// credential mutation itself, not merely deduplicate within one process
    /// or one factory instance.
    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn persisted_factories_share_file_lock_refresh_authority() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let temp = tempfile::tempdir().unwrap();
        let default_factory = AgentFactory::new(temp.path().join("sessions"));
        assert!(
            default_factory
                .resolution_provider_auth_persistence()
                .unwrap()
                .is_some(),
            "the production factory constructor must attach refresh authority"
        );
        let backend = meerkat_providers::auth_store::TokenStoreBackend::File {
            root: temp.path().join("credentials"),
        };

        let mut factories = Vec::new();
        for _ in 0..2 {
            let persistence = backend.clone().open_with_refresh_authority().unwrap();
            let factory = AgentFactory::minimal().with_provider_auth_persistence(persistence);
            assert!(factory.resolution_token_store().unwrap().is_some());
            assert!(
                factory
                    .resolution_provider_auth_persistence()
                    .unwrap()
                    .is_some(),
                "persisted provider auth must install cross-process refresh authority"
            );
            factories.push(factory);
        }

        let key = meerkat_providers::auth_store::TokenKey::parse("shared", "oauth").unwrap();
        let start = Arc::new(tokio::sync::Barrier::new(factories.len()));
        let in_flight = Arc::new(AtomicUsize::new(0));
        let max_in_flight = Arc::new(AtomicUsize::new(0));
        let mut tasks = Vec::new();

        for factory in factories {
            let coordinator = factory
                .resolution_provider_auth_persistence()
                .unwrap()
                .unwrap()
                .refresh_coordinator();
            let key = key.clone();
            let start = Arc::clone(&start);
            let in_flight = Arc::clone(&in_flight);
            let max_in_flight = Arc::clone(&max_in_flight);
            tasks.push(tokio::spawn(async move {
                start.wait().await;
                coordinator
                    .with_exclusive_mutation(
                        key,
                        Box::new(move || {
                            Box::pin(async move {
                                let current = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                                max_in_flight.fetch_max(current, Ordering::SeqCst);
                                tokio::time::sleep(std::time::Duration::from_millis(40)).await;
                                in_flight.fetch_sub(1, Ordering::SeqCst);
                                Ok::<_, meerkat_providers::auth_store::CredentialMutationError>(
                                    meerkat_providers::auth_store::CredentialMutationOutcome::Persisted(
                                        meerkat_providers::auth_store::PersistedTokens::api_key("ok"),
                                    ),
                                )
                            })
                        }),
                    )
                    .await
            }));
        }

        for task in tasks {
            task.await.unwrap().unwrap();
        }
        assert_eq!(
            max_in_flight.load(Ordering::SeqCst),
            1,
            "independent persisted factories must share one file-lock refresh authority"
        );
    }

    fn session_with_raw_metadata(
        session: Session,
        key: &'static str,
        value: serde_json::Value,
    ) -> Session {
        let mut raw = serde_json::to_value(session).expect("session should serialize");
        raw.get_mut("metadata")
            .and_then(serde_json::Value::as_object_mut)
            .expect("session metadata should be an object")
            .insert(key.to_string(), value);
        serde_json::from_value(raw).expect("session should deserialize with raw metadata")
    }

    fn visibility_tool(name: &str) -> Arc<meerkat_core::types::ToolDef> {
        Arc::new(
            meerkat_core::types::ToolDef::new(
                name,
                format!("test tool {name}"),
                serde_json::json!({ "type": "object" }),
            )
            .with_provenance(meerkat_core::types::ToolProvenance {
                kind: meerkat_core::types::ToolSourceKind::Callback,
                source_id: name.into(),
            }),
        )
    }

    fn inherited_visibility_authority(
        filter: meerkat_core::tool_scope::ToolFilter,
        tool_names: &[&str],
    ) -> (
        meerkat_core::InheritedToolVisibilityAuthority,
        BTreeMap<meerkat_core::ToolName, meerkat_core::ToolVisibilityWitness>,
    ) {
        let tools = Arc::from(
            tool_names
                .iter()
                .map(|name| visibility_tool(name))
                .collect::<Vec<_>>(),
        );
        let tool_scope = meerkat_core::ToolScope::new(tools);
        // SAFETY: factory tests use the same facade bridge token as production
        // AgentFactory to exercise the sealed parent composition authority path.
        #[allow(unsafe_code)]
        let authority = unsafe {
            core_agent_factory_parent_tool_composition_authority_new(
                agent_factory_policy_bridge_token(),
            )
        }
        .expect("test parent tool composition authority should mint through AgentFactory bridge");
        // SAFETY: same sealed facade bridge token as authority creation.
        #[allow(unsafe_code)]
        unsafe {
            core_agent_factory_parent_tool_composition_authority_set_tool_scope(
                agent_factory_policy_bridge_token(),
                &authority,
                &tool_scope,
            )
        }
        .expect("test parent tool composition authority should accept ToolScope");
        let authority = authority
            .authorize_inherited_tool_visibility(filter)
            .expect("test visibility authority should mint from snapshot");
        let witnesses = authority.witnesses().clone();
        (authority, witnesses)
    }

    struct NeverLlmClient;

    #[async_trait]
    impl LlmClient for NeverLlmClient {
        fn project_replay_messages(
            &self,
            messages: &[meerkat_core::Message],
        ) -> Result<Vec<meerkat_core::Message>, meerkat_client::LlmError> {
            Ok(messages.to_vec())
        }

        fn stream<'a>(
            &'a self,
            _request: &'a meerkat_client::LlmRequest,
        ) -> std::pin::Pin<
            Box<
                dyn futures::Stream<
                        Item = Result<meerkat_client::LlmEvent, meerkat_client::LlmError>,
                    > + Send
                    + 'a,
            >,
        > {
            Box::pin(futures::stream::empty())
        }

        fn provider(&self) -> meerkat_core::Provider {
            meerkat_core::Provider::Other
        }

        async fn health_check(&self) -> Result<(), meerkat_client::LlmError> {
            Ok(())
        }
    }

    struct TestSessionStore;

    #[async_trait]
    impl SessionStore for TestSessionStore {
        async fn save(&self, _session: &Session) -> Result<(), meerkat_store::SessionStoreError> {
            Ok(())
        }

        async fn load(
            &self,
            _id: &meerkat_core::SessionId,
        ) -> Result<Option<Session>, meerkat_store::SessionStoreError> {
            Ok(None)
        }

        async fn list(
            &self,
            _filter: meerkat_store::SessionFilter,
        ) -> Result<Vec<meerkat_core::SessionMeta>, meerkat_store::SessionStoreError> {
            Ok(Vec::new())
        }

        async fn delete(
            &self,
            _id: &meerkat_core::SessionId,
        ) -> Result<(), meerkat_store::SessionStoreError> {
            Ok(())
        }

        async fn delete_if_current_revision(
            &self,
            _id: &meerkat_core::SessionId,
            _expected_current_revision: &str,
        ) -> Result<bool, meerkat_store::SessionStoreError> {
            Ok(false)
        }
    }

    #[derive(Default)]
    struct TestBlobStore {
        blobs: Mutex<HashMap<BlobId, BlobPayload>>,
    }

    #[async_trait]
    impl BlobStore for TestBlobStore {
        async fn put_image(&self, media_type: &str, data: &str) -> Result<BlobRef, BlobStoreError> {
            let blob_id = BlobId::new(format!("sha256:{media_type}:{data}"));
            self.blobs.lock().await.insert(
                blob_id.clone(),
                BlobPayload {
                    blob_id: blob_id.clone(),
                    media_type: media_type.to_string(),
                    data: data.to_string(),
                },
            );
            Ok(BlobRef {
                blob_id,
                media_type: media_type.to_string(),
            })
        }

        async fn get(&self, blob_id: &BlobId) -> Result<BlobPayload, BlobStoreError> {
            self.blobs
                .lock()
                .await
                .get(blob_id)
                .cloned()
                .ok_or_else(|| BlobStoreError::NotFound(blob_id.clone()))
        }

        async fn delete(&self, blob_id: &BlobId) -> Result<(), BlobStoreError> {
            self.blobs.lock().await.remove(blob_id);
            Ok(())
        }

        fn is_persistent(&self) -> bool {
            false
        }
    }

    #[test]
    fn registry_backed_defaults_resolver_respects_provider_identity() {
        let registry = ModelRegistry::from_config(&Config::default(), meerkat_models::canonical())
            .expect("registry");
        let resolver = RegistryBackedDefaultsResolver {
            registry: Arc::new(registry),
        };

        assert_eq!(
            meerkat_core::ModelOperationalDefaultsResolver::call_timeout_for(
                &resolver,
                Provider::OpenAI,
                "gpt-5.4"
            ),
            Some(std::time::Duration::from_secs(600))
        );
        assert_eq!(
            meerkat_core::ModelOperationalDefaultsResolver::call_timeout_for(
                &resolver,
                Provider::Anthropic,
                "gpt-5.4"
            ),
            None,
            "provider-aware default lookup must not reuse OpenAI defaults for Anthropic"
        );
    }

    #[tokio::test]
    async fn core_agentbuilder_factory_policy_rejects_missing_metadata() {
        let temp = tempfile::tempdir().unwrap();
        let factory = AgentFactory::new(temp.path().join("sessions"));
        let llm_adapter = Arc::new(
            factory
                .build_llm_adapter(Arc::new(NeverLlmClient), "mock-model")
                .await,
        );
        let tools = Arc::new(EmptyToolDispatcher);
        let store_adapter = Arc::new(
            factory
                .build_store_adapter(Arc::new(TestSessionStore))
                .await,
        );

        let builder = AgentBuilder::new()
            .model("mock-model")
            .max_tokens_per_turn(64)
            .with_turn_state_handle(Arc::new(RuntimeTurnStateHandle::ephemeral()));
        // SAFETY: this test exercises the canonical facade/core bridge after
        // intentionally omitting required factory metadata.
        #[allow(unsafe_code)]
        let result = unsafe {
            core_agent_factory_policy_build(
                agent_factory_policy_bridge_token(),
                builder,
                llm_adapter,
                tools,
                store_adapter,
            )
            .await
        };

        assert!(
            matches!(
                result,
                Err(meerkat_core::AgentBuildPolicyError::MissingSession)
            ),
            "core factory-policy build must reject missing factory metadata"
        );
    }

    #[test]
    fn default_provider_resolution_uses_catalog_owner_without_explicit_override() {
        let temp = tempfile::tempdir().unwrap();
        let factory = AgentFactory::new(temp.path().join("sessions"));
        let registry = factory
            .model_registry(&Config::default())
            .expect("registry");
        let build = AgentBuildConfig::new("gpt-5.4");

        let (provider, server_id) = factory
            .resolve_provider_from_registry(&registry, &build)
            .expect("catalog model should infer provider");

        assert_eq!(provider, Provider::OpenAI);
        assert_eq!(server_id, None);
    }

    #[test]
    fn default_compaction_threshold_scales_with_large_context_model() {
        let config = Config::default();
        let registry = config
            .model_registry(meerkat_models::canonical())
            .expect("registry");

        let compaction =
            model_aware_compaction_config(&config, &registry, Provider::OpenAI, "gpt-5.5", None);

        assert_eq!(compaction.auto_compact_threshold, 840_000);
    }

    #[test]
    fn create_session_default_model_resolves_to_catalog_seam() {
        // A default config pins no model anywhere (core embeds no provider
        // data), so the resolver must terminate at the catalog-owned global
        // default — the SAME value every surface gets. An operator-set
        // non-legacy `config.agent.model` outranks it (tier 1).
        let config = Config::default();
        assert!(config.agent.model.is_empty());
        assert_eq!(
            resolve_create_session_default_model(&config),
            meerkat_models::global_default_model(),
        );

        let mut pinned = Config::default();
        pinned.agent.model = "operator-pinned-model".to_string();
        assert_eq!(
            resolve_create_session_default_model(&pinned),
            "operator-pinned-model",
        );
    }

    #[test]
    fn create_session_default_model_preserves_supported_historical_pin() {
        let mut config = Config::default();
        config.agent.model = "claude-opus-4-7".to_string();
        config.models.anthropic = "custom-anthropic".to_string();
        config.models.openai = "gpt-5.5-custom".to_string();

        assert_eq!(
            resolve_create_session_default_model(&config),
            "claude-opus-4-7"
        );
        let resolved =
            resolve_create_session_model(&config, CreateSessionModelResolutionRequest::default())
                .expect("supported operator pin resolves");
        assert_eq!(resolved.model, "claude-opus-4-7");
        assert_eq!(resolved.provider, Provider::Anthropic);
    }

    #[test]
    fn create_session_model_provider_mismatch_uses_provider_default() {
        let mut config = Config::default();
        config.agent.model = "gpt-5.5".to_string();
        config.models.anthropic = "claude-sonnet-4-6".to_string();

        let resolved = resolve_create_session_model(
            &config,
            CreateSessionModelResolutionRequest {
                provider: Some(Provider::Anthropic),
                ..Default::default()
            },
        )
        .expect("provider constraint selects its configured default");
        assert_eq!(resolved.model, "claude-sonnet-4-6");
        assert_eq!(resolved.provider, Provider::Anthropic);
    }

    fn create_model_configured_auth_binding(
        provider: Provider,
        default_model: Option<&str>,
    ) -> (Config, AuthBindingRef) {
        let mut config = Config::default();
        let mut section = RealmConfigSection::default();
        section.backend.insert(
            "backend".to_string(),
            BackendProfileConfig {
                provider: provider.as_str().to_string(),
                backend_kind: "test_backend".to_string(),
                base_url: None,
                options: serde_json::Value::Null,
            },
        );
        section.auth.insert(
            "auth".to_string(),
            AuthProfileConfig {
                provider: provider.as_str().to_string(),
                auth_method: "test_auth".to_string(),
                source: CredentialSourceSpec::ManagedStore,
                constraints: Default::default(),
                metadata_defaults: Default::default(),
            },
        );
        section.binding.insert(
            "primary".to_string(),
            ProviderBindingConfig {
                backend_profile: "backend".to_string(),
                auth_profile: "auth".to_string(),
                default_model: default_model.map(str::to_string),
                policy: Default::default(),
                provider_default: true,
            },
        );
        config.realm.insert("dev".to_string(), section);
        (
            config,
            AuthBindingRef {
                realm: RealmId::parse("dev").expect("realm"),
                binding: BindingId::parse("primary").expect("binding"),
                profile: None,
                origin: BindingOrigin::Configured,
            },
        )
    }

    #[test]
    fn create_session_model_binding_default_wins_when_model_omitted() {
        let (mut config, auth_binding) = create_model_configured_auth_binding(
            Provider::Gemini,
            Some("gemini-3.1-flash-lite-preview"),
        );
        config.agent.model = "claude-opus-4-7".to_string();

        let resolved = resolve_create_session_model(
            &config,
            CreateSessionModelResolutionRequest {
                auth_binding: Some(auth_binding),
                ..Default::default()
            },
        )
        .expect("binding default resolves");
        assert_eq!(resolved.model, "gemini-3.1-flash-lite-preview");
        assert_eq!(resolved.provider, Provider::Gemini);
        assert!(resolved.auth_binding.is_some());
    }

    #[test]
    fn create_session_model_explicit_model_wins_over_binding_default() {
        let (config, auth_binding) = create_model_configured_auth_binding(
            Provider::Gemini,
            Some("gemini-3.1-flash-lite-preview"),
        );

        let resolved = resolve_create_session_model(
            &config,
            CreateSessionModelResolutionRequest {
                model: Some("gemini-3.5-flash".to_string()),
                auth_binding: Some(auth_binding),
                ..Default::default()
            },
        )
        .expect("explicit same-provider model wins");
        assert_eq!(resolved.model, "gemini-3.5-flash");
        assert_eq!(resolved.provider, Provider::Gemini);
    }

    #[test]
    fn create_session_model_binding_provider_without_default_constrains_ladder() {
        let (mut config, auth_binding) =
            create_model_configured_auth_binding(Provider::Gemini, None);
        config.agent.model = "claude-opus-4-7".to_string();
        config.models.gemini = "gemini-3.1-pro-preview".to_string();

        let resolved = resolve_create_session_model(
            &config,
            CreateSessionModelResolutionRequest {
                auth_binding: Some(auth_binding),
                ..Default::default()
            },
        )
        .expect("binding provider constrains defaults");
        assert_eq!(resolved.model, "gemini-3.1-pro-preview");
        assert_eq!(resolved.provider, Provider::Gemini);
    }

    #[test]
    fn create_session_model_malformed_or_mismatched_binding_fails_closed() {
        let (mut malformed, auth_binding) =
            create_model_configured_auth_binding(Provider::Gemini, None);
        malformed
            .realm
            .get_mut("dev")
            .expect("realm")
            .auth
            .get_mut("auth")
            .expect("auth")
            .provider = "openai".to_string();
        let malformed_error = resolve_create_session_model(
            &malformed,
            CreateSessionModelResolutionRequest {
                auth_binding: Some(auth_binding),
                ..Default::default()
            },
        )
        .expect_err("backend/auth mismatch must fail");
        assert!(matches!(
            &malformed_error,
            CreateSessionModelResolutionError::AuthBinding(_)
        ));
        assert!(
            malformed_error.is_configuration_fault(),
            "malformed server-owned binding config is an internal fault"
        );

        let (config, auth_binding) = create_model_configured_auth_binding(Provider::Gemini, None);
        let mismatch_error = resolve_create_session_model(
            &config,
            CreateSessionModelResolutionRequest {
                provider: Some(Provider::OpenAI),
                auth_binding: Some(auth_binding),
                ..Default::default()
            },
        )
        .expect_err("explicit provider mismatch must fail");
        assert!(matches!(
            &mismatch_error,
            CreateSessionModelResolutionError::AuthBindingProviderMismatch { .. }
        ));
        assert!(
            !mismatch_error.is_configuration_fault(),
            "an explicit provider that conflicts with a valid named binding is caller-owned"
        );
    }

    #[test]
    fn create_session_model_empty_binding_default_is_configuration_fault() {
        let (config, auth_binding) =
            create_model_configured_auth_binding(Provider::Gemini, Some(""));

        let error = resolve_create_session_model(
            &config,
            CreateSessionModelResolutionRequest {
                auth_binding: Some(auth_binding),
                ..Default::default()
            },
        )
        .expect_err("empty configured binding default must fail");

        assert!(matches!(
            &error,
            CreateSessionModelResolutionError::EmptyBindingDefaultModel { .. }
        ));
        assert!(error.is_configuration_fault());
    }

    #[test]
    fn create_session_model_missing_default_distinguishes_binding_from_explicit_provider() {
        let (binding_config, auth_binding) =
            create_model_configured_auth_binding(Provider::SelfHosted, None);
        let binding_error = resolve_create_session_model(
            &binding_config,
            CreateSessionModelResolutionRequest {
                auth_binding: Some(auth_binding),
                ..Default::default()
            },
        )
        .expect_err("a self-hosted binding without any model default must fail");
        assert!(matches!(
            &binding_error,
            CreateSessionModelResolutionError::BindingMissingProviderDefault { .. }
        ));
        assert!(binding_error.is_configuration_fault());

        let provider_error = resolve_create_session_model(
            &Config::default(),
            CreateSessionModelResolutionRequest {
                provider: Some(Provider::SelfHosted),
                ..Default::default()
            },
        )
        .expect_err("an explicit self-hosted provider requires a model");
        assert!(matches!(
            &provider_error,
            CreateSessionModelResolutionError::MissingProviderDefault { .. }
        ));
        assert!(!provider_error.is_configuration_fault());
    }

    #[test]
    fn create_session_model_unknown_owner_distinguishes_config_from_request() {
        let model = "uncatalogued-without-provider";
        let mut configured = Config::default();
        configured.agent.model = model.to_string();

        let configured_error = resolve_create_session_model(
            &configured,
            CreateSessionModelResolutionRequest::default(),
        )
        .expect_err("uncatalogued configured model without an owner must fail");
        assert!(matches!(
            &configured_error,
            CreateSessionModelResolutionError::ConfiguredUnknownProvider { .. }
        ));
        assert!(configured_error.is_configuration_fault());

        let request_error = resolve_create_session_model(
            &Config::default(),
            CreateSessionModelResolutionRequest {
                model: Some(model.to_string()),
                ..Default::default()
            },
        )
        .expect_err("uncatalogued explicit model without an owner must fail");
        assert!(matches!(
            &request_error,
            CreateSessionModelResolutionError::UnknownProvider { .. }
        ));
        assert!(!request_error.is_configuration_fault());
    }

    #[test]
    fn create_session_model_error_ownership_distinguishes_config_from_request() {
        fn assert_fault(error: CreateSessionModelResolutionError, expected: bool) {
            assert_eq!(
                error.is_configuration_fault(),
                expected,
                "unexpected fault ownership for {error}"
            );
        }

        let realm = RealmId::parse("dev").expect("realm");
        let binding = BindingId::parse("primary").expect("binding");

        assert_fault(
            CreateSessionModelResolutionError::Config(meerkat_core::ConfigError::Validation(
                "broken model registry".to_string(),
            )),
            true,
        );
        assert_fault(
            CreateSessionModelResolutionError::ConfiguredProviderModelMismatch(
                "configured mismatch".to_string(),
            ),
            true,
        );
        assert_fault(
            CreateSessionModelResolutionError::ConfiguredUnknownProvider {
                model: "configured-unknown".to_string(),
            },
            true,
        );
        assert_fault(
            CreateSessionModelResolutionError::EmptyBindingDefaultModel {
                realm: realm.clone(),
                binding: binding.clone(),
            },
            true,
        );
        assert_fault(
            CreateSessionModelResolutionError::BindingMissingProviderDefault {
                realm: realm.clone(),
                binding: binding.clone(),
                provider: Provider::SelfHosted,
            },
            true,
        );

        for source in [
            meerkat_core::ConnectionTargetError::MissingRealm,
            meerkat_core::ConnectionTargetError::UnknownRealm("missing".to_string()),
            meerkat_core::ConnectionTargetError::InvalidRealmId {
                realm: "bad realm".to_string(),
                source: meerkat_core::IdentityError::InvalidChar(' '),
            },
            meerkat_core::ConnectionTargetError::InvalidBindingId {
                binding: "bad binding".to_string(),
                source: meerkat_core::IdentityError::InvalidChar(' '),
            },
            meerkat_core::ConnectionTargetError::BindingInvalid {
                realm: "dev".to_string(),
                binding: "missing".to_string(),
                source: meerkat_core::ProviderBindingError::UnknownBinding("missing".to_string()),
            },
        ] {
            assert_fault(
                CreateSessionModelResolutionError::AuthBinding(source),
                false,
            );
        }

        for source in [
            meerkat_core::ConnectionTargetError::MissingDefaultBinding {
                realm: "dev".to_string(),
            },
            meerkat_core::ConnectionTargetError::RealmConfigInvalid {
                realm: "dev".to_string(),
                source: meerkat_core::ProviderBindingError::UnknownBackend("missing".to_string()),
            },
            meerkat_core::ConnectionTargetError::BindingInvalid {
                realm: "dev".to_string(),
                binding: "primary".to_string(),
                source: meerkat_core::ProviderBindingError::UnknownAuth("missing".to_string()),
            },
            meerkat_core::ConnectionTargetError::BindingInvalid {
                realm: "dev".to_string(),
                binding: "primary".to_string(),
                source: meerkat_core::ProviderBindingError::ProviderMismatch {
                    binding: "primary".to_string(),
                    backend: Provider::OpenAI,
                    auth: Provider::Anthropic,
                },
            },
            meerkat_core::ConnectionTargetError::BindingInvalid {
                realm: "dev".to_string(),
                binding: "primary".to_string(),
                source: meerkat_core::ProviderBindingError::UnknownProviderName(
                    "mystery".to_string(),
                ),
            },
            meerkat_core::ConnectionTargetError::BindingInvalid {
                realm: "dev".to_string(),
                binding: "primary".to_string(),
                source: meerkat_core::ProviderBindingError::InvalidRealmId {
                    realm: "bad realm".to_string(),
                    source: meerkat_core::IdentityError::InvalidChar(' '),
                },
            },
            meerkat_core::ConnectionTargetError::ProviderMismatch {
                realm: "dev".to_string(),
                binding: "primary".to_string(),
                expected: Provider::Gemini,
                backend: Provider::OpenAI,
                auth: Provider::Anthropic,
            },
            meerkat_core::ConnectionTargetError::RealmChain(
                meerkat_core::connection::RealmChainError::Cycle {
                    chain: vec!["dev".to_string(), "dev".to_string()],
                },
            ),
        ] {
            assert_fault(CreateSessionModelResolutionError::AuthBinding(source), true);
        }

        for error in [
            CreateSessionModelResolutionError::AuthBindingProviderMismatch {
                realm,
                binding,
                binding_provider: Provider::Gemini,
                explicit_provider: Provider::OpenAI,
            },
            CreateSessionModelResolutionError::ProviderModelMismatch(
                "request mismatch".to_string(),
            ),
            CreateSessionModelResolutionError::UnknownProvider {
                model: "request-unknown".to_string(),
            },
            CreateSessionModelResolutionError::MissingProviderDefault {
                provider: Provider::SelfHosted,
            },
            CreateSessionModelResolutionError::EmptyExplicitModel,
        ] {
            assert_fault(error, false);
        }
    }

    #[test]
    fn create_session_default_model_falls_back_to_global_default() {
        // With no configured per-provider or agent defaults, the resolver must
        // terminate at the catalog global default rather than an empty string.
        let mut config = Config::default();
        config.models.anthropic = String::new();
        config.models.openai = String::new();
        config.models.gemini = String::new();
        config.agent.model = String::new();
        assert_eq!(
            resolve_create_session_default_model(&config),
            meerkat_models::global_default_model()
        );
        let resolved =
            resolve_create_session_model(&config, CreateSessionModelResolutionRequest::default())
                .expect("no-hint ladder resolves");
        assert_eq!(resolved.model, meerkat_models::global_default_model());
        assert_eq!(
            resolved.provider,
            meerkat_models::canonical()
                .infer_provider(meerkat_models::global_default_model())
                .expect("global default has a catalog owner")
        );
    }

    #[test]
    fn explicit_compaction_threshold_is_preserved() {
        let mut config = Config::default();
        config.compaction.auto_compact_threshold = 42_000;
        let registry = config
            .model_registry(meerkat_models::canonical())
            .expect("registry");

        let compaction =
            model_aware_compaction_config(&config, &registry, Provider::OpenAI, "gpt-5.5", None);

        assert_eq!(compaction.auto_compact_threshold, 42_000);
    }

    #[test]
    fn explicit_default_compaction_threshold_is_preserved() {
        let mut config = Config::default();
        config.compaction.auto_compact_threshold = 100_000;
        config.compaction.auto_compact_threshold_explicit = true;
        let registry = config
            .model_registry(meerkat_models::canonical())
            .expect("registry");

        let compaction =
            model_aware_compaction_config(&config, &registry, Provider::OpenAI, "gpt-5.5", None);

        assert_eq!(compaction.auto_compact_threshold, 100_000);
    }

    #[test]
    fn explicit_provider_override_rejects_catalog_owner_contradiction() {
        let temp = tempfile::tempdir().unwrap();
        let factory = AgentFactory::new(temp.path().join("sessions"));
        let registry = factory
            .model_registry(&Config::default())
            .expect("registry");
        let mut build = AgentBuildConfig::new("gpt-5.4");
        build.provider = Some(Provider::Anthropic);

        let err = factory
            .resolve_provider_from_registry(&registry, &build)
            .expect_err("explicit provider must match catalog ownership");
        assert!(
            err.to_string().contains("registered for provider 'openai'")
                && err.to_string().contains("not provider 'anthropic'")
                && err.to_string().contains("gpt-5.4"),
            "error should identify the rejected provider/model pair: {err}"
        );
    }

    #[test]
    fn explicit_provider_override_allows_uncatalogued_model_without_catalog_owner() {
        let temp = tempfile::tempdir().unwrap();
        let factory = AgentFactory::new(temp.path().join("sessions"));
        let registry = factory
            .model_registry(&Config::default())
            .expect("registry");
        let mut build = AgentBuildConfig::new("custom-openai-compatible");
        build.provider = Some(Provider::OpenAI);

        let (provider, server_id) = factory
            .resolve_provider_from_registry(&registry, &build)
            .expect("uncatalogued explicit provider has no catalog owner to contradict");

        assert_eq!(provider, Provider::OpenAI);
        assert_eq!(server_id, None);
    }

    #[test]
    fn request_policy_tool_defaults_require_provider_owned_profile() {
        let temp = tempfile::tempdir().unwrap();
        let factory = AgentFactory::new(temp.path().join("sessions"));
        let config = Config::default();
        let openai_identity = SessionLlmIdentity {
            model: "gpt-5.4".to_string(),
            provider: Provider::OpenAI,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: None,
        };
        let mismatched_identity = SessionLlmIdentity {
            provider: Provider::Anthropic,
            ..openai_identity.clone()
        };

        let openai_policy = factory
            .request_policy_for_llm_identity(
                &config,
                &openai_identity,
                ToolCategoryOverride::Inherit,
            )
            .expect("OpenAI request policy");
        assert_eq!(
            openai_policy.provider_tool_defaults,
            Some(meerkat_core::lifecycle::run_primitive::ProviderTag::OpenAi(
                meerkat_core::lifecycle::run_primitive::OpenAiProviderTag {
                    web_search: Some(
                        meerkat_core::lifecycle::run_primitive::OpaqueProviderBody::from_value(
                            &serde_json::json!({"type": "web_search"}),
                        ),
                    ),
                    ..Default::default()
                },
            ),),
            "owned OpenAI web-search defaults should still resolve"
        );

        let mismatched_policy = factory
            .request_policy_for_llm_identity(
                &config,
                &mismatched_identity,
                ToolCategoryOverride::Inherit,
            )
            .expect("mismatched request policy should fail closed, not error");
        assert!(
            mismatched_policy.provider_tool_defaults.is_none(),
            "Anthropic policy must not reuse OpenAI model defaults from model id alone"
        );

        // A session that persisted a web-search disable intent must keep it
        // across a model reconfigure: the provider-native body is suppressed
        // even though the model profile would otherwise advertise it.
        let disabled_policy = factory
            .request_policy_for_llm_identity(
                &config,
                &openai_identity,
                ToolCategoryOverride::Disable,
            )
            .expect("OpenAI request policy with web-search disabled");
        assert!(
            disabled_policy.provider_tool_defaults.is_none(),
            "persisted web-search Disable must survive reconfigure and suppress the native body"
        );
    }

    #[test]
    fn provider_tool_defaults_suppressed_on_web_search_disable() {
        let config = Config::default();
        // Profile advertises web-search support and config enables the provider
        // tool — without the override this would resolve the native body.
        let profile = meerkat_core::model_profile::ModelProfile {
            provider: Provider::OpenAI,
            model_family: "gpt-5".to_string(),
            supports_temperature: false,
            supports_thinking: false,
            supports_reasoning: false,
            inline_video: false,
            vision: false,
            image_input: false,
            image_tool_results: false,
            realtime: false,
            supports_web_search: true,
            image_generation: false,
            params_schema: serde_json::json!({}),
            beta_headers: Vec::new(),
            call_timeout_secs: None,
        };

        let enabled = provider_tool_defaults_for(
            Provider::OpenAI,
            &config,
            Some(&profile),
            ToolCategoryOverride::Inherit,
        );
        assert_eq!(
            enabled,
            Some(meerkat_core::lifecycle::run_primitive::ProviderTag::OpenAi(
                meerkat_core::lifecycle::run_primitive::OpenAiProviderTag {
                    web_search: Some(
                        meerkat_core::lifecycle::run_primitive::OpaqueProviderBody::from_value(
                            &serde_json::json!({"type": "web_search"}),
                        ),
                    ),
                    ..Default::default()
                },
            ),),
            "Inherit must leave owned web-search defaults intact"
        );

        let disabled = provider_tool_defaults_for(
            Provider::OpenAI,
            &config,
            Some(&profile),
            ToolCategoryOverride::Disable,
        );
        assert!(
            disabled.is_none(),
            "Disable must suppress the native web-search body even when profile+config would enable it"
        );
    }

    #[cfg(feature = "skills")]
    fn test_skill_key(source_uuid: &str, skill_name: &str) -> SkillKey {
        SkillKey::new(
            SourceUuid::parse(source_uuid).expect("valid source uuid"),
            SkillName::parse(skill_name).expect("valid skill name"),
        )
    }

    #[cfg(feature = "skills")]
    fn remapped_skill_keys() -> (SkillKey, SkillKey) {
        (
            test_skill_key("00000000-0000-4b11-8111-0000000000a1", "legacy-skill"),
            test_skill_key("00000000-0000-4b11-8111-0000000000b2", "modern-skill"),
        )
    }

    #[cfg(feature = "skills")]
    fn identity_record(key: &SkillKey, display_name: &str) -> SourceIdentityRecord {
        SourceIdentityRecord {
            source_uuid: key.source_uuid.clone(),
            display_name: display_name.to_string(),
            transport_kind: SourceTransportKind::Filesystem,
            fingerprint: display_name.to_string(),
            status: SourceIdentityStatus::Active,
        }
    }

    #[cfg(feature = "skills")]
    fn remapping_skill_runtime(requested: SkillKey, canonical: SkillKey) -> Arc<SkillRuntime> {
        let source = meerkat_skills::InMemorySkillSource::new(vec![SkillDocument {
            descriptor: meerkat_core::skills::SkillDescriptor::new(
                canonical.clone(),
                "modern-skill",
                "Modern remapped skill",
            ),
            body: format!("modern body for {canonical}"),
            extensions: Default::default(),
        }]);
        let registry = SourceIdentityRegistry::build(
            vec![
                identity_record(&requested, "legacy-source"),
                identity_record(&canonical, "modern-source"),
            ],
            vec![SourceIdentityLineage {
                event_id: "test-remap".to_string(),
                recorded_at_unix_secs: 0,
                required_from_skills: Vec::new(),
                event: SourceIdentityLineageEvent::RenameOrRelocate {
                    from: requested.source_uuid.clone(),
                    to: canonical.source_uuid.clone(),
                },
            }],
            vec![SkillKeyRemap {
                from: requested,
                to: canonical,
                reason: Some("test remap".to_string()),
            }],
            Vec::new(),
        )
        .expect("source identity registry");
        let engine = meerkat_skills::DefaultSkillEngine::new(source, Vec::new())
            .with_source_identity_registry(Arc::new(registry));

        Arc::new(SkillRuntime::new(Arc::new(engine)))
    }

    #[cfg(feature = "skills")]
    fn metadata_with_active_skills(active_skills: Option<Vec<SkillKey>>) -> SessionMetadata {
        let tooling = SessionTooling {
            builtins: ToolCategoryOverride::Disable,
            active_skills,
            ..Default::default()
        };

        SessionMetadata {
            schema_version: meerkat_core::SESSION_METADATA_SCHEMA_VERSION,
            model: "claude-sonnet-4-5".to_string(),
            max_tokens: 8192,
            structured_output_retries: 2,
            provider: Provider::Anthropic,
            self_hosted_server_id: None,
            provider_params: None,
            tooling,
            keep_alive: false,
            comms_name: None,
            peer_meta: None,
            realm_id: None,
            instance_id: None,
            backend: None,
            config_generation: None,
            auth_binding: None,
            mob_member_binding: None,
        }
    }

    fn inline_realm_section(entries: &[(&str, &str)]) -> RealmConfigSection {
        RealmConfigSection::from_inline_api_keys(entries)
    }

    fn configured_auth_binding(realm: &str, binding: &str) -> AuthBindingRef {
        AuthBindingRef {
            realm: RealmId::parse(realm).expect("valid realm"),
            binding: BindingId::parse(binding).expect("valid binding"),
            profile: None,
            origin: meerkat_core::BindingOrigin::Configured,
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    struct ScriptedStickyFallbackClient {
        provider: Provider,
        fail_recoverably: bool,
        calls: Arc<std::sync::atomic::AtomicUsize>,
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[async_trait]
    impl LlmClient for ScriptedStickyFallbackClient {
        fn project_replay_messages(
            &self,
            messages: &[meerkat_core::Message],
        ) -> Result<Vec<meerkat_core::Message>, meerkat_client::LlmError> {
            Ok(messages.to_vec())
        }

        fn stream<'a>(
            &'a self,
            _request: &'a meerkat_client::LlmRequest,
        ) -> std::pin::Pin<
            Box<
                dyn futures::Stream<
                        Item = Result<meerkat_client::LlmEvent, meerkat_client::LlmError>,
                    > + Send
                    + 'a,
            >,
        > {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if self.fail_recoverably {
                Box::pin(futures::stream::iter(vec![Err(
                    meerkat_client::LlmError::ServerOverloaded,
                )]))
            } else {
                Box::pin(futures::stream::iter(vec![
                    Ok(meerkat_client::LlmEvent::TextDelta {
                        delta: "backup success".to_string(),
                        meta: None,
                    }),
                    Ok(meerkat_client::LlmEvent::Done {
                        outcome: meerkat_client::LlmDoneOutcome::Success {
                            stop_reason: meerkat_core::StopReason::EndTurn,
                        },
                    }),
                ]))
            }
        }

        fn provider(&self) -> Provider {
            self.provider
        }

        async fn health_check(&self) -> Result<(), meerkat_client::LlmError> {
            Ok(())
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    struct ScriptedStickyFallbackProviderRuntime {
        provider: Provider,
        client: Arc<dyn LlmClient>,
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[async_trait]
    impl meerkat_llm_core::provider_runtime::ProviderRuntime for ScriptedStickyFallbackProviderRuntime {
        fn provider_id(&self) -> Provider {
            self.provider
        }

        async fn resolve_binding(
            &self,
            binding: &meerkat_llm_core::provider_runtime::ValidatedBinding,
            _env: &meerkat_llm_core::provider_runtime::ResolverEnvironment,
        ) -> Result<
            meerkat_llm_core::provider_runtime::ResolvedConnection,
            meerkat_llm_core::provider_runtime::ProviderAuthError,
        > {
            Ok(meerkat_llm_core::provider_runtime::ResolvedConnection {
                provider: self.provider,
                backend: binding.backend(),
                backend_profile: Arc::clone(binding.backend_profile()),
                auth_lease: Arc::new(
                    meerkat_llm_core::provider_runtime::StaticLease::inline_secret(
                        format!("test-{}-key", self.provider.as_str()),
                        meerkat_core::AuthMetadata::default(),
                        None,
                        format!("sticky-fallback-test:{}", self.provider.as_str()),
                    ),
                ),
            })
        }

        fn build_client(
            &self,
            _connection: meerkat_llm_core::provider_runtime::ResolvedConnection,
        ) -> Result<Arc<dyn LlmClient>, meerkat_llm_core::provider_runtime::ProviderClientError>
        {
            Ok(Arc::clone(&self.client))
        }
    }

    #[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
    struct StickyFallbackVisibilityDispatcher {
        tools: Arc<[Arc<meerkat_core::ToolDef>]>,
    }

    #[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
    #[async_trait]
    impl AgentToolDispatcher for StickyFallbackVisibilityDispatcher {
        fn tools(&self) -> Arc<[Arc<meerkat_core::ToolDef>]> {
            Arc::clone(&self.tools)
        }

        async fn dispatch(
            &self,
            call: ToolCallView<'_>,
        ) -> Result<meerkat_core::ToolDispatchOutcome, meerkat_core::ToolError> {
            Ok(
                meerkat_core::ToolResult::new(call.id.to_string(), call.name.to_string(), false)
                    .into(),
            )
        }
    }

    #[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
    fn sticky_fallback_visibility_dispatcher(tool_name: &str) -> Arc<dyn AgentToolDispatcher> {
        Arc::new(StickyFallbackVisibilityDispatcher {
            tools: Arc::from([visibility_tool(tool_name)]),
        })
    }

    #[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
    fn scripted_sticky_fallback_factory(
        store_path: PathBuf,
        primary_calls: Arc<std::sync::atomic::AtomicUsize>,
        backup_calls: Arc<std::sync::atomic::AtomicUsize>,
    ) -> AgentFactory {
        let primary_client: Arc<dyn LlmClient> = Arc::new(ScriptedStickyFallbackClient {
            provider: Provider::Anthropic,
            fail_recoverably: true,
            calls: primary_calls,
        });
        let backup_client: Arc<dyn LlmClient> = Arc::new(ScriptedStickyFallbackClient {
            provider: Provider::OpenAI,
            fail_recoverably: false,
            calls: backup_calls,
        });
        let mut factory = AgentFactory::new(store_path)
            .without_provider_auth_persistence()
            .builtins(false);
        factory.provider_registry = Arc::new(
            meerkat_llm_core::provider_runtime::ProviderRuntimeRegistry::empty()
                .with_runtime(Arc::new(ScriptedStickyFallbackProviderRuntime {
                    provider: Provider::Anthropic,
                    client: primary_client,
                }))
                .with_runtime(Arc::new(ScriptedStickyFallbackProviderRuntime {
                    provider: Provider::OpenAI,
                    client: backup_client,
                })),
        );
        factory
    }

    #[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
    fn sticky_fallback_test_config(backup_model: &str, backup_binding: AuthBindingRef) -> Config {
        let mut config = Config::default();
        config.realm.insert(
            "fallback_test".to_string(),
            inline_realm_section(&[
                ("anthropic", "test-primary-key"),
                ("openai", "test-backup-key"),
            ]),
        );
        config.models.custom.insert(
            backup_model.to_string(),
            meerkat_core::config::CustomModelConfig {
                provider: Provider::OpenAI,
                display_name: Some("Text-only backup".to_string()),
                context_window: Some(32_000),
                max_output_tokens: Some(4_096),
                vision: Some(false),
                web_search: Some(false),
                call_timeout_secs: None,
            },
        );
        config.model_fallback.enabled = true;
        config.model_fallback.chain = vec![ModelFallbackTarget {
            model: backup_model.to_string(),
            provider: Some(Provider::OpenAI),
            auth_binding: Some(backup_binding),
        }];
        config.retry.max_retries = 1;
        config.retry.initial_delay = std::time::Duration::ZERO;
        config.retry.max_delay = std::time::Duration::ZERO;
        config.retry.multiplier = 1.0;
        config
    }

    #[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
    fn sticky_fallback_runtime_request(
        primary_model: &str,
        external_tools: Arc<dyn AgentToolDispatcher>,
    ) -> CreateSessionRequest {
        CreateSessionRequest {
            injected_context: Vec::new(),
            model: primary_model.to_string(),
            prompt: meerkat_core::ContentInput::Text(String::new()),
            system_prompt: crate::SystemPromptOverride::Set(
                "persistent sticky fallback regression".to_string(),
            ),
            max_tokens: None,
            event_tx: None,
            initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
            deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
            build: Some(SessionBuildOptions {
                provider: Some(Provider::Anthropic),
                realm_id: Some(RealmId::parse("fallback_test").expect("test realm")),
                external_tools: Some(external_tools),
                override_builtins: ToolCategoryOverride::Disable,
                ..Default::default()
            }),
            labels: None,
        }
    }

    #[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
    fn assert_persisted_sticky_fallback_state(
        session: &Session,
        backup_model: &str,
        backup_binding: &AuthBindingRef,
        external_filter: &ToolFilter,
        original_prompt: &str,
    ) {
        let identity = session
            .session_metadata()
            .expect("fallback session must retain canonical metadata")
            .llm_identity();
        assert_eq!(identity.model, backup_model);
        assert_eq!(identity.provider, Provider::OpenAI);
        assert_eq!(identity.auth_binding.as_ref(), Some(backup_binding));

        let visibility = session
            .try_tool_visibility_state()
            .expect("canonical visibility metadata should decode")
            .expect("runtime-backed session should persist visibility metadata");
        let expected_capability_filter = ToolFilter::Deny(
            [meerkat_core::VIEW_IMAGE_TOOL_NAME.to_string()]
                .into_iter()
                .collect(),
        );
        assert_eq!(
            visibility.capability_base_filter, expected_capability_filter,
            "durable visibility must follow the fallback model profile"
        );
        assert_eq!(&visibility.active_filter, external_filter);
        assert_eq!(&visibility.staged_filter, external_filter);
        assert_eq!(visibility.active_revision, visibility.staged_revision);
        assert!(
            visibility.active_revision > 0,
            "the staged external filter must cross the real turn boundary"
        );
        if let ToolFilter::Allow(names) | ToolFilter::Deny(names) = external_filter {
            for name in names {
                assert!(
                    visibility
                        .filter_witnesses
                        .get(name)
                        .is_some_and(meerkat_core::ToolVisibilityWitness::has_identity_witness),
                    "durable filter tool {name} must retain its provenance witness"
                );
            }
        }

        let user_texts = session
            .messages()
            .iter()
            .filter_map(|message| match message {
                meerkat_core::Message::User(user) => Some(user.text_content()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            user_texts,
            vec![original_prompt.to_string()],
            "fallback details must never be flattened into a fabricated user prompt"
        );
        let assistant_texts = session
            .messages()
            .iter()
            .filter_map(|message| match message {
                meerkat_core::Message::BlockAssistant(assistant) => {
                    Some(assistant.text_blocks().collect::<Vec<_>>().join("\n"))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            assistant_texts,
            vec!["backup success".to_string()],
            "fallback details must remain typed notice data, not assistant prose"
        );
        let typed_fallback_notices = session
            .messages()
            .iter()
            .filter_map(|message| match message {
                meerkat_core::Message::SystemNotice(notice)
                    if notice.blocks.iter().any(|block| {
                        matches!(
                            block,
                            meerkat_core::SystemNoticeBlock::RuntimeNotice { category, .. }
                                if category == "model_fallback"
                        )
                    }) =>
                {
                    Some(notice)
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(typed_fallback_notices.len(), 1);
        assert_eq!(
            typed_fallback_notices[0].kind,
            meerkat_core::SystemNoticeKind::Generic
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn session_owned_no_store_sticky_fallback_updates_binding_authorities() {
        use meerkat_runtime::SessionServiceRuntimeExt as _;

        let primary_model = "claude-sonnet-4-5";
        let backup_model = "custom-text-only-backup";
        let primary_binding = configured_auth_binding("fallback_test", "default_anthropic");
        let backup_binding = configured_auth_binding("fallback_test", "default_openai");
        let primary_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let backup_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let primary_client: Arc<dyn LlmClient> = Arc::new(ScriptedStickyFallbackClient {
            provider: Provider::Anthropic,
            fail_recoverably: true,
            calls: Arc::clone(&primary_calls),
        });
        let backup_client: Arc<dyn LlmClient> = Arc::new(ScriptedStickyFallbackClient {
            provider: Provider::OpenAI,
            fail_recoverably: false,
            calls: Arc::clone(&backup_calls),
        });

        let temp = tempfile::tempdir().unwrap();
        let mut factory = AgentFactory::new(temp.path().join("sessions"))
            .without_provider_auth_persistence()
            .builtins(false);
        factory.provider_registry = Arc::new(
            meerkat_llm_core::provider_runtime::ProviderRuntimeRegistry::empty()
                .with_runtime(Arc::new(ScriptedStickyFallbackProviderRuntime {
                    provider: Provider::Anthropic,
                    client: primary_client,
                }))
                .with_runtime(Arc::new(ScriptedStickyFallbackProviderRuntime {
                    provider: Provider::OpenAI,
                    client: backup_client,
                })),
        );

        let mut config = Config::default();
        config.realm.insert(
            "fallback_test".to_string(),
            inline_realm_section(&[
                ("anthropic", "test-primary-key"),
                ("openai", "test-backup-key"),
            ]),
        );
        config.models.custom.insert(
            backup_model.to_string(),
            meerkat_core::config::CustomModelConfig {
                provider: Provider::OpenAI,
                display_name: Some("Text-only backup".to_string()),
                context_window: Some(32_000),
                max_output_tokens: Some(4_096),
                vision: Some(false),
                web_search: Some(false),
                call_timeout_secs: None,
            },
        );
        config.model_fallback.enabled = true;
        config.model_fallback.chain = vec![ModelFallbackTarget {
            model: backup_model.to_string(),
            provider: Some(Provider::OpenAI),
            auth_binding: Some(backup_binding.clone()),
        }];
        config.retry.max_retries = 1;
        config.retry.initial_delay = std::time::Duration::ZERO;
        config.retry.max_delay = std::time::Duration::ZERO;
        config.retry.multiplier = 1.0;

        let runtime = meerkat_runtime::MeerkatMachine::ephemeral();
        let session = Session::new();
        let session_id = session.id().clone();
        let bindings = runtime
            .prepare_bindings(session_id.clone())
            .await
            .expect("ephemeral machine should prepare real session-owned bindings");
        let mut build = AgentBuildConfig::new(primary_model);
        build.provider = Some(Provider::Anthropic);
        build.realm_id = Some(RealmId::parse("fallback_test").expect("test realm"));
        build.auth_binding = Some(primary_binding.clone());
        build.resume_session = Some(session);
        build.runtime_build_mode = RuntimeBuildMode::SessionOwned(bindings.clone());
        build.override_builtins = ToolCategoryOverride::Disable;

        let mut agent = factory
            .build_agent(build, &config)
            .await
            .expect("real factory should build the session-owned fallback chain");
        let initial_visibility = bindings
            .tool_visibility_owner()
            .visibility_state()
            .expect("initial machine visibility");
        let initial_routing = runtime
            .session_model_routing_status(&session_id)
            .await
            .expect("initial model-routing status");
        assert_eq!(initial_visibility.capability_base_filter, ToolFilter::All);
        assert_eq!(initial_routing.baseline_model.as_str(), primary_model);
        assert_eq!(initial_routing.session_provider, Some(Provider::Anthropic));

        agent.set_runtime_execution_kind(Some(
            meerkat_core::lifecycle::RuntimeExecutionKind::ContentTurn,
        ));
        let result = agent
            .run(meerkat_core::ContentInput::Text(
                "exercise fallback".to_string(),
            ))
            .await
            .expect("recoverable primary failure should retry successfully on backup");
        assert_eq!(result.text, "backup success");
        assert_eq!(primary_calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(backup_calls.load(std::sync::atomic::Ordering::SeqCst), 1);

        let active_identity = agent
            .session()
            .session_metadata()
            .expect("fallback should retain canonical session metadata")
            .llm_identity();
        assert_eq!(active_identity.model, backup_model);
        assert_eq!(active_identity.provider, Provider::OpenAI);
        assert_eq!(active_identity.auth_binding, Some(backup_binding));

        let fallback_visibility = bindings
            .tool_visibility_owner()
            .visibility_state()
            .expect("fallback machine visibility");
        let expected_backup_filter = ToolFilter::Deny(
            [meerkat_core::VIEW_IMAGE_TOOL_NAME.to_string()]
                .into_iter()
                .collect(),
        );
        assert_eq!(
            fallback_visibility.capability_base_filter, expected_backup_filter,
            "prepared binding visibility must publish the backup profile"
        );
        let fallback_routing = runtime
            .session_model_routing_status(&session_id)
            .await
            .expect("fallback model-routing status");
        assert_eq!(fallback_routing.baseline_model.as_str(), backup_model);
        assert_eq!(fallback_routing.effective_model.as_str(), backup_model);
        assert_eq!(fallback_routing.session_provider, Some(Provider::OpenAI));
    }

    #[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
    #[tokio::test]
    async fn runtime_backed_persistent_sticky_fallback_survives_authoritative_reload() {
        use meerkat_core::service::SessionService as _;
        use meerkat_runtime::completion::CompletionOutcome;
        use meerkat_runtime::{RuntimeStore as _, SessionServiceRuntimeExt as _};

        const PRIMARY_MODEL: &str = "claude-sonnet-4-5";
        const BACKUP_MODEL: &str = "custom-text-only-backup";
        const EXTERNAL_TOOL: &str = "known_external_tool";
        const ORIGINAL_PROMPT: &str = "persist this exact user prompt";

        let primary_binding = configured_auth_binding("fallback_test", "default_anthropic");
        let backup_binding = configured_auth_binding("fallback_test", "default_openai");
        let config = sticky_fallback_test_config(BACKUP_MODEL, backup_binding.clone());
        let temp = tempfile::tempdir().expect("persistent fallback tempdir");
        let sqlite_store = Arc::new(
            meerkat_store::SqliteSessionStore::open(temp.path().join("sessions.sqlite3"))
                .expect("open sqlite session store"),
        );
        let runtime_store = Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());
        let blob_store = Arc::new(meerkat_store::MemoryBlobStore::new());
        let external_filter = ToolFilter::Deny([EXTERNAL_TOOL.to_string()].into_iter().collect());
        let runtime_id;
        let session_id;

        {
            let primary_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let backup_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let factory = scripted_sticky_fallback_factory(
                temp.path().join("first-factory-sessions"),
                Arc::clone(&primary_calls),
                Arc::clone(&backup_calls),
            );
            let builder = crate::FactoryAgentBuilder::new(factory, config.clone());
            assert!(
                builder.default_llm_client.is_none(),
                "the regression must resolve both clients through the real provider registry"
            );
            let persistence = crate::PersistenceBundle::new(
                Arc::clone(&sqlite_store) as Arc<dyn SessionStore>,
                Arc::clone(&runtime_store) as Arc<dyn meerkat_runtime::RuntimeStore>,
                Arc::clone(&blob_store) as Arc<dyn BlobStore>,
            );
            let (service, adapter) =
                crate::surface::build_runtime_backed_service(builder, 4, persistence);
            let service = Arc::new(service);
            let session = Session::new();
            session_id = session.id().clone();
            runtime_id = meerkat_runtime::LogicalRuntimeId::for_session(&session_id);
            let request = sticky_fallback_runtime_request(
                PRIMARY_MODEL,
                sticky_fallback_visibility_dispatcher(EXTERNAL_TOOL),
            );
            let result = Box::pin(crate::surface::materialize_session(
                &service,
                &adapter,
                session,
                request,
                {
                    let service = Arc::clone(&service);
                    let adapter = Arc::clone(&adapter);
                    move |session_id| {
                        crate::surface::default_persistent_executor(service, adapter, session_id)
                    }
                },
            ))
            .await
            .expect("materialize real runtime-backed fallback session");
            assert_eq!(result.session_id, session_id);

            let initial_snapshot_bytes = runtime_store
                .load_session_snapshot(&runtime_id)
                .await
                .expect("load initial runtime snapshot")
                .expect("runtime-backed create must materialize an initial snapshot");
            let initial_snapshot: Session = serde_json::from_slice(&initial_snapshot_bytes)
                .expect("initial runtime snapshot should deserialize");
            assert_eq!(
                initial_snapshot
                    .try_tool_visibility_state()
                    .expect("initial visibility metadata should decode"),
                Some(SessionToolVisibilityState::default()),
                "runtime-backed creation must seal the all-default visibility parent before staging"
            );
            let initial_identity = initial_snapshot
                .session_metadata()
                .expect("initial runtime snapshot should carry session metadata")
                .llm_identity();
            assert_eq!(initial_identity.model, PRIMARY_MODEL);
            assert_eq!(initial_identity.provider, Provider::Anthropic);
            assert_eq!(initial_identity.auth_binding, Some(primary_binding.clone()));

            service
                .set_session_tool_filter(&session_id, external_filter.clone())
                .await
                .expect("stage known external visibility filter");
            let staged_session = service
                .load_authoritative_session(&session_id)
                .await
                .expect("load staged authoritative session")
                .expect("staged authoritative session should exist");
            let staged_visibility = staged_session
                .try_tool_visibility_state()
                .expect("staged visibility metadata should decode")
                .expect("staged visibility metadata should exist");
            assert_eq!(staged_visibility.active_filter, ToolFilter::All);
            assert_eq!(staged_visibility.staged_filter, external_filter);
            assert!(
                staged_visibility
                    .filter_witnesses
                    .get(EXTERNAL_TOOL)
                    .is_some_and(meerkat_core::ToolVisibilityWitness::has_identity_witness),
                "staged filter must carry the real external tool's provenance witness"
            );

            let (_accept, completion) = adapter
                .accept_input_with_completion(
                    &session_id,
                    meerkat_runtime::Input::Prompt(meerkat_runtime::PromptInput::new(
                        ORIGINAL_PROMPT,
                        None,
                    )),
                )
                .await
                .expect("accept real runtime prompt");
            let completion = completion.expect("runtime prompt should provide a completion handle");
            let completion =
                tokio::time::timeout(std::time::Duration::from_secs(10), completion.wait())
                    .await
                    .expect("runtime prompt should complete before timeout")
                    .expect("runtime completion waiter should resolve");
            assert!(
                matches!(
                    completion,
                    CompletionOutcome::Completed(ref run) if run.text == "backup success"
                ),
                "unexpected fallback completion: {completion:?}"
            );
            assert_eq!(primary_calls.load(std::sync::atomic::Ordering::SeqCst), 1);
            assert_eq!(backup_calls.load(std::sync::atomic::Ordering::SeqCst), 1);

            let live_identity = service
                .live_session_llm_identity(&session_id)
                .await
                .expect("read live identity watch");
            assert_eq!(live_identity.model, BACKUP_MODEL);
            assert_eq!(live_identity.provider, Provider::OpenAI);
            assert_eq!(live_identity.auth_binding.as_ref(), Some(&backup_binding));
            let live_routing = adapter
                .session_model_routing_status(&session_id)
                .await
                .expect("read live fallback routing");
            assert_eq!(live_routing.baseline_model.as_str(), BACKUP_MODEL);
            assert_eq!(live_routing.effective_model.as_str(), BACKUP_MODEL);
            assert_eq!(live_routing.session_provider, Some(Provider::OpenAI));

            let authoritative = service
                .load_authoritative_session(&session_id)
                .await
                .expect("load durable authoritative fallback session")
                .expect("durable authoritative fallback session should exist");
            assert_persisted_sticky_fallback_state(
                &authoritative,
                BACKUP_MODEL,
                &backup_binding,
                &external_filter,
                ORIGINAL_PROMPT,
            );
            let runtime_snapshot_bytes = runtime_store
                .load_session_snapshot(&runtime_id)
                .await
                .expect("load fallback runtime snapshot")
                .expect("fallback runtime snapshot should exist");
            let runtime_snapshot: Session = serde_json::from_slice(&runtime_snapshot_bytes)
                .expect("fallback runtime snapshot should deserialize");
            assert_persisted_sticky_fallback_state(
                &runtime_snapshot,
                BACKUP_MODEL,
                &backup_binding,
                &external_filter,
                ORIGINAL_PROMPT,
            );
        }

        let restart_primary_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let restart_backup_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let restart_factory = scripted_sticky_fallback_factory(
            temp.path().join("restarted-factory-sessions"),
            Arc::clone(&restart_primary_calls),
            Arc::clone(&restart_backup_calls),
        );
        let restart_builder = crate::FactoryAgentBuilder::new(restart_factory, config);
        assert!(restart_builder.default_llm_client.is_none());
        let restart_persistence = crate::PersistenceBundle::new(
            Arc::clone(&sqlite_store) as Arc<dyn SessionStore>,
            Arc::clone(&runtime_store) as Arc<dyn meerkat_runtime::RuntimeStore>,
            Arc::clone(&blob_store) as Arc<dyn BlobStore>,
        );
        let (restart_service, restart_adapter) =
            crate::surface::build_runtime_backed_service(restart_builder, 4, restart_persistence);
        let restart_service = Arc::new(restart_service);
        assert!(
            !restart_adapter.contains_session(&session_id).await,
            "fresh runtime adapter must start without an in-memory registration"
        );
        let resume_session = restart_service
            .load_authoritative_session(&session_id)
            .await
            .expect("load fallback target from retained persistence")
            .expect("retained fallback session should exist");
        assert_persisted_sticky_fallback_state(
            &resume_session,
            BACKUP_MODEL,
            &backup_binding,
            &external_filter,
            ORIGINAL_PROMPT,
        );
        let restart_request = sticky_fallback_runtime_request(
            PRIMARY_MODEL,
            sticky_fallback_visibility_dispatcher(EXTERNAL_TOOL),
        );
        let restart_result = Box::pin(crate::surface::materialize_session(
            &restart_service,
            &restart_adapter,
            resume_session,
            restart_request,
            {
                let service = Arc::clone(&restart_service);
                let adapter = Arc::clone(&restart_adapter);
                move |session_id| {
                    crate::surface::default_persistent_executor(service, adapter, session_id)
                }
            },
        ))
        .await
        .expect("rematerialize retained fallback session");
        assert_eq!(restart_result.session_id, session_id);
        assert_eq!(
            restart_primary_calls.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "reload must not probe or replay the failed primary"
        );
        assert_eq!(
            restart_backup_calls.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "reload must not fabricate a new fallback turn"
        );

        let reloaded_live_identity = restart_service
            .live_session_llm_identity(&session_id)
            .await
            .expect("read reloaded live identity watch");
        assert_eq!(reloaded_live_identity.model, BACKUP_MODEL);
        assert_eq!(reloaded_live_identity.provider, Provider::OpenAI);
        assert_eq!(
            reloaded_live_identity.auth_binding.as_ref(),
            Some(&backup_binding)
        );
        let reloaded_routing = restart_adapter
            .session_model_routing_status(&session_id)
            .await
            .expect("read reloaded fallback routing");
        assert_eq!(reloaded_routing.baseline_model.as_str(), BACKUP_MODEL);
        assert_eq!(reloaded_routing.effective_model.as_str(), BACKUP_MODEL);
        assert_eq!(reloaded_routing.session_provider, Some(Provider::OpenAI));
        let reloaded_authoritative = restart_service
            .load_authoritative_session(&session_id)
            .await
            .expect("load rematerialized authoritative session")
            .expect("rematerialized authoritative session should exist");
        assert_persisted_sticky_fallback_state(
            &reloaded_authoritative,
            BACKUP_MODEL,
            &backup_binding,
            &external_filter,
            ORIGINAL_PROMPT,
        );
        let reloaded_snapshot_bytes = runtime_store
            .load_session_snapshot(&runtime_id)
            .await
            .expect("load rematerialized runtime snapshot")
            .expect("rematerialized runtime snapshot should exist");
        let reloaded_snapshot: Session = serde_json::from_slice(&reloaded_snapshot_bytes)
            .expect("rematerialized runtime snapshot should deserialize");
        assert_persisted_sticky_fallback_state(
            &reloaded_snapshot,
            BACKUP_MODEL,
            &backup_binding,
            &external_filter,
            ORIGINAL_PROMPT,
        );
    }

    fn openai_realm_with_bindings(bindings: &[(&str, &str)]) -> RealmConfigSection {
        let mut section = RealmConfigSection::default();
        section.backend.insert(
            "openai_api".to_string(),
            BackendProfileConfig {
                provider: "openai".to_string(),
                backend_kind: "openai_api".to_string(),
                base_url: None,
                options: serde_json::Value::Null,
            },
        );
        for (binding_id, secret) in bindings {
            let auth_id = format!("{binding_id}_auth");
            section.auth.insert(
                auth_id.clone(),
                meerkat_core::AuthProfileConfig {
                    provider: "openai".to_string(),
                    auth_method: "api_key".to_string(),
                    source: CredentialSourceSpec::InlineSecret {
                        secret: (*secret).to_string(),
                    },
                    constraints: Default::default(),
                    metadata_defaults: Default::default(),
                },
            );
            section.binding.insert(
                (*binding_id).to_string(),
                ProviderBindingConfig {
                    backend_profile: "openai_api".to_string(),
                    auth_profile: auth_id,
                    default_model: None,
                    policy: Default::default(),
                    provider_default: false,
                },
            );
        }
        section
    }

    #[cfg(all(
        feature = "openai",
        feature = "openai-realtime",
        not(target_arch = "wasm32")
    ))]
    mod realtime_per_open_credentials {
        use super::*;
        use crate::session_runtime::realtime_credentials::{
            PerOpenCredentialRealtimeSessionFactory, RealtimeCurrentConfigSource,
        };
        use meerkat_contracts::RealtimeTurningMode;
        use meerkat_llm_core::LlmError;
        use meerkat_llm_core::provider_runtime::{
            ProviderAuthError, ProviderClientError, ProviderRuntime, ProviderRuntimeRegistry,
            ResolvedConnection, ResolverEnvironment, StaticLease, ValidatedBinding,
        };
        use meerkat_llm_core::realtime_session::{
            RealtimeExternalSessionTarget, RealtimeSession, RealtimeSessionFactory,
            RealtimeSessionOpenConfig,
        };

        #[derive(Debug, Clone, PartialEq)]
        struct RecordedRealtimeOpen {
            secret: String,
            identity: SessionLlmIdentity,
            via: &'static str,
        }

        type OpenRecorder = Arc<std::sync::Mutex<Vec<RecordedRealtimeOpen>>>;

        /// Provider-runtime double that resolves the binding's inline secret
        /// and mints a mock realtime factory carrying that secret, so tests
        /// can observe which credential each open would authenticate with.
        struct RealtimeRecordingOpenAiRuntime {
            opens: OpenRecorder,
        }

        #[async_trait]
        impl ProviderRuntime for RealtimeRecordingOpenAiRuntime {
            fn provider_id(&self) -> Provider {
                Provider::OpenAI
            }

            async fn resolve_binding(
                &self,
                binding: &ValidatedBinding,
                _env: &ResolverEnvironment,
            ) -> Result<ResolvedConnection, ProviderAuthError> {
                let secret = match &binding.auth_profile().source {
                    CredentialSourceSpec::InlineSecret { secret } => secret.clone(),
                    other => {
                        return Err(ProviderAuthError::SourceResolutionFailed(format!(
                            "test runtime only resolves inline secrets, got {other:?}"
                        )));
                    }
                };
                Ok(ResolvedConnection {
                    provider: Provider::OpenAI,
                    backend: binding.backend(),
                    backend_profile: Arc::clone(binding.backend_profile()),
                    auth_lease: Arc::new(StaticLease::inline_secret(
                        secret,
                        meerkat_core::AuthMetadata::default(),
                        None,
                        "test-realtime",
                    )),
                })
            }

            fn build_client(
                &self,
                _connection: ResolvedConnection,
            ) -> Result<Arc<dyn meerkat_llm_core::LlmClient>, ProviderClientError> {
                Ok(Arc::new(meerkat_client::TestClient::default()))
            }

            fn build_realtime_session_factory(
                &self,
                connection: ResolvedConnection,
            ) -> Result<Arc<dyn RealtimeSessionFactory>, ProviderClientError> {
                let secret = connection
                    .resolved_secret()
                    .ok_or(ProviderClientError::NoCredentialMaterial)?;
                Ok(Arc::new(RecordingProviderRealtimeFactory {
                    secret,
                    opens: Arc::clone(&self.opens),
                }))
            }
        }

        /// Minted-per-open mock: records the credential it was minted with
        /// plus the identity of every open, then fails with a sentinel so no
        /// real socket is attempted.
        struct RecordingProviderRealtimeFactory {
            secret: String,
            opens: OpenRecorder,
        }

        impl RecordingProviderRealtimeFactory {
            fn record(&self, identity: &SessionLlmIdentity, via: &'static str) {
                self.opens.lock().unwrap().push(RecordedRealtimeOpen {
                    secret: self.secret.clone(),
                    identity: identity.clone(),
                    via,
                });
            }
        }

        #[async_trait]
        impl RealtimeSessionFactory for RecordingProviderRealtimeFactory {
            fn capabilities(&self) -> meerkat_contracts::RealtimeCapabilities {
                meerkat_contracts::RealtimeCapabilities::default()
            }

            fn supports_provider(&self, provider: Provider) -> bool {
                provider == Provider::OpenAI
            }

            async fn open_session(
                &self,
                open_config: &RealtimeSessionOpenConfig,
            ) -> Result<Box<dyn RealtimeSession>, LlmError> {
                self.record(&open_config.llm_identity, "open_session");
                Err(LlmError::ConnectionReset)
            }

            async fn attach_external_session(
                &self,
                _target: &RealtimeExternalSessionTarget,
                open_config: &RealtimeSessionOpenConfig,
            ) -> Result<Box<dyn RealtimeSession>, LlmError> {
                self.record(&open_config.llm_identity, "attach_external_session");
                Err(LlmError::ConnectionReset)
            }

            async fn open_live_adapter(
                &self,
                open_config: &RealtimeSessionOpenConfig,
            ) -> Result<Arc<dyn meerkat_core::live_adapter::LiveAdapter>, LlmError> {
                self.record(&open_config.llm_identity, "open_live_adapter");
                Err(LlmError::ConnectionReset)
            }
        }

        struct FixedRealtimeConfigSource(Config);

        #[async_trait]
        impl RealtimeCurrentConfigSource for FixedRealtimeConfigSource {
            async fn current_config(&self) -> Result<Config, meerkat_core::ConfigError> {
                Ok(self.0.clone())
            }
        }

        struct FailingRealtimeConfigSource;

        #[async_trait]
        impl RealtimeCurrentConfigSource for FailingRealtimeConfigSource {
            async fn current_config(&self) -> Result<Config, meerkat_core::ConfigError> {
                Err(meerkat_core::ConfigError::MissingField(
                    "injected current-config failure".to_string(),
                ))
            }
        }

        fn realtime_identity(auth_binding: Option<AuthBindingRef>) -> SessionLlmIdentity {
            SessionLlmIdentity {
                model: "gpt-realtime-2".to_string(),
                provider: Provider::OpenAI,
                self_hosted_server_id: None,
                provider_params: None,
                auth_binding,
            }
        }

        fn open_config(identity: SessionLlmIdentity) -> RealtimeSessionOpenConfig {
            RealtimeSessionOpenConfig::new(
                RealtimeTurningMode::ProviderManaged,
                identity,
                Vec::new(),
                Vec::new(),
            )
        }

        fn recording_wrapper(
            config: Config,
        ) -> (PerOpenCredentialRealtimeSessionFactory, OpenRecorder) {
            let temp = std::env::temp_dir().join("meerkat-realtime-per-open-tests");
            let opens: OpenRecorder = Arc::new(std::sync::Mutex::new(Vec::new()));
            let mut factory = AgentFactory::new(temp).without_provider_auth_persistence();
            factory.provider_registry = Arc::new(ProviderRuntimeRegistry::empty().with_runtime(
                Arc::new(RealtimeRecordingOpenAiRuntime {
                    opens: Arc::clone(&opens),
                }),
            ));
            let wrapper = PerOpenCredentialRealtimeSessionFactory::new(
                factory,
                Arc::new(FixedRealtimeConfigSource(config)),
            );
            (wrapper, opens)
        }

        #[tokio::test]
        async fn open_session_resolves_the_session_auth_binding_secret() {
            let mut config = Config::default();
            config.realm.insert(
                "dev".to_string(),
                openai_realm_with_bindings(&[
                    ("primary_openai", "sk-primary"),
                    ("secondary_openai", "sk-secondary"),
                ]),
            );
            let (wrapper, opens) = recording_wrapper(config);
            let identity =
                realtime_identity(Some(configured_auth_binding("dev", "secondary_openai")));

            let result = wrapper.open_session(&open_config(identity.clone())).await;

            assert!(
                matches!(result, Err(LlmError::ConnectionReset)),
                "mock provider factory should have been reached"
            );
            let opens = opens.lock().unwrap();
            assert_eq!(
                *opens,
                vec![RecordedRealtimeOpen {
                    secret: "sk-secondary".to_string(),
                    identity,
                    via: "open_session",
                }],
                "the socket must authenticate with the SESSION binding's secret, not a default"
            );
        }

        #[tokio::test]
        async fn open_live_adapter_and_attach_resolve_per_open_under_the_session_binding() {
            let mut config = Config::default();
            config.realm.insert(
                "dev".to_string(),
                openai_realm_with_bindings(&[
                    ("primary_openai", "sk-primary"),
                    ("secondary_openai", "sk-secondary"),
                ]),
            );
            let (wrapper, opens) = recording_wrapper(config);
            let identity =
                realtime_identity(Some(configured_auth_binding("dev", "primary_openai")));

            let adapter = wrapper
                .open_live_adapter(&open_config(identity.clone()))
                .await;
            assert!(matches!(adapter, Err(LlmError::ConnectionReset)));

            let attached = wrapper
                .attach_external_session(
                    &RealtimeExternalSessionTarget::new("call_1").expect("target"),
                    &open_config(identity.clone()),
                )
                .await;
            assert!(matches!(attached, Err(LlmError::ConnectionReset)));

            let opens = opens.lock().unwrap();
            assert_eq!(
                *opens,
                vec![
                    RecordedRealtimeOpen {
                        secret: "sk-primary".to_string(),
                        identity: identity.clone(),
                        via: "open_live_adapter",
                    },
                    RecordedRealtimeOpen {
                        secret: "sk-primary".to_string(),
                        identity,
                        via: "attach_external_session",
                    },
                ],
                "every open path must re-resolve under the session binding"
            );
        }

        #[tokio::test]
        async fn open_session_without_binding_resolves_the_default_binding() {
            let mut config = Config::default();
            config.realm.insert(
                "global".to_string(),
                openai_realm_with_bindings(&[("default_openai", "sk-default")]),
            );
            let (wrapper, opens) = recording_wrapper(config);
            let identity = realtime_identity(None);

            let result = wrapper.open_session(&open_config(identity.clone())).await;

            assert!(matches!(result, Err(LlmError::ConnectionReset)));
            let opens = opens.lock().unwrap();
            assert_eq!(
                *opens,
                vec![RecordedRealtimeOpen {
                    secret: "sk-default".to_string(),
                    identity,
                    via: "open_session",
                }],
                "a binding-less session identity resolves the configured default binding"
            );
        }

        #[tokio::test]
        async fn open_fails_closed_on_unresolvable_binding() {
            let mut config = Config::default();
            config.realm.insert(
                "dev".to_string(),
                openai_realm_with_bindings(&[("primary_openai", "sk-primary")]),
            );
            let (wrapper, opens) = recording_wrapper(config);
            let identity =
                realtime_identity(Some(configured_auth_binding("dev", "missing_binding")));

            let error = wrapper
                .open_session(&open_config(identity))
                .await
                .err()
                .expect("unresolvable binding must fail the open closed");

            assert!(
                matches!(error, LlmError::InvalidConfig { .. }),
                "expected a typed channel-terminal error, got {error:?}"
            );
            assert!(
                opens.lock().unwrap().is_empty(),
                "no provider factory may be minted for an unresolvable binding"
            );
        }

        #[tokio::test]
        async fn open_fails_closed_when_current_config_cannot_be_read() {
            let temp = std::env::temp_dir().join("meerkat-realtime-per-open-tests");
            let factory = AgentFactory::new(temp).without_provider_auth_persistence();
            let wrapper = PerOpenCredentialRealtimeSessionFactory::new(
                factory,
                Arc::new(FailingRealtimeConfigSource),
            );

            let error = wrapper
                .open_session(&open_config(realtime_identity(None)))
                .await
                .err()
                .expect("a config-source fault must fail the open closed");

            assert!(matches!(error, LlmError::InvalidConfig { .. }));
        }

        /// End-to-end through the REAL OpenAI provider runtime: a session
        /// bound to an Azure OpenAI binding is admitted nowhere near a
        /// realtime socket — the provider runtime's mint-time gate rejects
        /// the backend with its typed error instead of falling back to a
        /// process-default credential.
        #[tokio::test]
        async fn open_fails_closed_on_realtime_unsupported_backend_binding() {
            let mut section = RealmConfigSection::default();
            section.backend.insert(
                "azure".to_string(),
                BackendProfileConfig {
                    provider: "openai".to_string(),
                    backend_kind: "azure_openai".to_string(),
                    base_url: Some("https://example.openai.azure.com".to_string()),
                    options: serde_json::Value::Null,
                },
            );
            section.auth.insert(
                "azure_auth".to_string(),
                meerkat_core::AuthProfileConfig {
                    provider: "openai".to_string(),
                    auth_method: "azure_api_key".to_string(),
                    source: CredentialSourceSpec::InlineSecret {
                        secret: "azure-key".to_string(),
                    },
                    constraints: Default::default(),
                    metadata_defaults: Default::default(),
                },
            );
            section.binding.insert(
                "azure_openai".to_string(),
                ProviderBindingConfig {
                    backend_profile: "azure".to_string(),
                    auth_profile: "azure_auth".to_string(),
                    default_model: None,
                    policy: Default::default(),
                    provider_default: false,
                },
            );
            let mut config = Config::default();
            config.realm.insert("dev".to_string(), section);

            let temp = std::env::temp_dir().join("meerkat-realtime-per-open-tests");
            let factory = AgentFactory::new(temp).without_provider_auth_persistence();
            let wrapper = PerOpenCredentialRealtimeSessionFactory::new(
                factory,
                Arc::new(FixedRealtimeConfigSource(config)),
            );
            let identity = realtime_identity(Some(configured_auth_binding("dev", "azure_openai")));

            let error = wrapper
                .open_session(&open_config(identity))
                .await
                .err()
                .expect("realtime-unsupported backend binding must fail the open closed");

            match error {
                LlmError::InvalidConfig { message } => assert!(
                    message.contains("openai-realtime-azure-openai"),
                    "rejection must carry the provider runtime's typed gate: {message}"
                ),
                other => panic!("expected InvalidConfig, got {other:?}"),
            }
        }
    }

    #[test]
    fn model_fallback_keeps_same_model_provider_with_different_auth_binding() {
        let factory = AgentFactory::new(std::env::temp_dir().join("meerkat-test-sessions"));
        let mut config = Config::default();
        let primary = configured_auth_binding("dev", "primary_openai");
        let secondary = configured_auth_binding("dev", "secondary_openai");
        config.model_fallback.chain = vec![ModelFallbackTarget {
            model: "gpt-5.5".to_string(),
            provider: Some(Provider::OpenAI),
            auth_binding: Some(secondary.clone()),
        }];
        let registry = factory.model_registry(&config).expect("model registry");
        let current = SessionLlmIdentity {
            model: "gpt-5.5".to_string(),
            provider: Provider::OpenAI,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: Some(primary),
        };

        let identities = factory
            .model_fallback_identities(&config, &registry, &current)
            .expect("fallback identities");

        assert_eq!(identities.len(), 1);
        assert_eq!(identities[0].model, "gpt-5.5");
        assert_eq!(identities[0].provider, Provider::OpenAI);
        assert_eq!(identities[0].auth_binding, Some(secondary));
    }

    #[test]
    fn catalog_model_fallback_uses_only_available_bindings_in_selected_realm() {
        let factory = AgentFactory::new(std::env::temp_dir().join("meerkat-test-sessions"));
        let mut config = Config::default();
        config.realm.insert(
            "team".to_string(),
            inline_realm_section(&[("openai", "sk-test-openai"), ("anthropic", "sk-ant-test")]),
        );
        let registry = factory.model_registry(&config).expect("model registry");
        let current = SessionLlmIdentity {
            model: meerkat_models::default_model(Provider::OpenAI)
                .expect("OpenAI catalog default")
                .to_string(),
            provider: Provider::OpenAI,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: Some(configured_auth_binding("team", "default_openai")),
        };

        let identities = factory
            .model_fallback_identities(&config, &registry, &current)
            .expect("catalog fallback identities");

        assert!(
            identities.iter().any(|identity| {
                identity.provider == Provider::Anthropic
                    && identity.auth_binding
                        == Some(configured_auth_binding("team", "default_anthropic"))
            }),
            "catalog fallback should include providers registered in the selected realm: {identities:#?}"
        );
        assert!(
            identities.iter().all(|identity| {
                identity
                    .auth_binding
                    .as_ref()
                    .is_some_and(|auth_binding| auth_binding.realm.as_str() == "team")
            }),
            "catalog fallback must not bleed into env/default realms when selected realm auth is in use"
        );
        assert!(
            identities
                .iter()
                .all(|identity| identity.provider != Provider::Gemini),
            "providers not registered in the selected realm should be skipped for the catalog chain"
        );
    }

    #[cfg(all(feature = "openai", not(target_arch = "wasm32")))]
    struct RecordingOpenAiRuntime {
        calls: Arc<std::sync::Mutex<Vec<AuthBindingRef>>>,
    }

    #[cfg(all(feature = "openai", not(target_arch = "wasm32")))]
    #[async_trait::async_trait]
    impl meerkat_llm_core::provider_runtime::ProviderRuntime for RecordingOpenAiRuntime {
        fn provider_id(&self) -> Provider {
            Provider::OpenAI
        }

        async fn resolve_binding(
            &self,
            binding: &meerkat_llm_core::provider_runtime::ValidatedBinding,
            _env: &meerkat_llm_core::provider_runtime::ResolverEnvironment,
        ) -> Result<
            meerkat_llm_core::provider_runtime::ResolvedConnection,
            meerkat_llm_core::provider_runtime::ProviderAuthError,
        > {
            self.calls
                .lock()
                .unwrap()
                .push(binding.auth_binding_ref().clone());
            Ok(meerkat_llm_core::provider_runtime::ResolvedConnection {
                provider: Provider::OpenAI,
                backend: binding.backend(),
                backend_profile: Arc::clone(binding.backend_profile()),
                auth_lease: Arc::new(
                    meerkat_llm_core::provider_runtime::StaticLease::inline_secret(
                        "sk-test-openai".to_string(),
                        meerkat_core::AuthMetadata::default(),
                        None,
                        "test-openai",
                    ),
                ),
            })
        }

        fn build_client(
            &self,
            _connection: meerkat_llm_core::provider_runtime::ResolvedConnection,
        ) -> Result<Arc<dyn LlmClient>, meerkat_llm_core::provider_runtime::ProviderClientError>
        {
            Ok(Arc::new(meerkat_client::TestClient::default()))
        }
    }

    #[cfg(all(feature = "openai", not(target_arch = "wasm32")))]
    #[tokio::test]
    async fn explicit_model_fallback_resolves_secondary_auth_binding_for_same_model() {
        let temp = tempfile::tempdir().unwrap();
        let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut factory = AgentFactory::new(temp.path().join("sessions")).builtins(false);
        factory.provider_registry = Arc::new(
            meerkat_llm_core::provider_runtime::ProviderRuntimeRegistry::empty().with_runtime(
                Arc::new(RecordingOpenAiRuntime {
                    calls: Arc::clone(&calls),
                }),
            ),
        );

        let primary = configured_auth_binding("dev", "primary_openai");
        let secondary = configured_auth_binding("dev", "secondary_openai");
        let mut config = Config::default();
        config.realm.insert(
            "dev".to_string(),
            openai_realm_with_bindings(&[
                ("primary_openai", "sk-primary"),
                ("secondary_openai", "sk-secondary"),
            ]),
        );
        config.model_fallback.chain = vec![ModelFallbackTarget {
            model: "gpt-5.5".to_string(),
            provider: Some(Provider::OpenAI),
            auth_binding: Some(secondary.clone()),
        }];

        let mut build = AgentBuildConfig::new("gpt-5.5");
        build.provider = Some(Provider::OpenAI);
        build.auth_binding = Some(primary.clone());
        build.override_builtins = ToolCategoryOverride::Disable;

        let _agent = factory
            .build_agent(build, &config)
            .await
            .expect("agent should build primary plus fallback clients");

        let calls = calls.lock().unwrap().clone();
        assert!(
            calls.contains(&primary),
            "primary binding should be resolved for the active client"
        );
        assert!(
            calls.contains(&secondary),
            "same model/provider fallback must still resolve the secondary auth binding"
        );
    }

    #[test]
    fn missing_auth_binding_synthesizes_env_default_binding_for_resolved_provider() {
        let config = Config::default();
        let (realm, binding_id, auth_binding) = AgentFactory::resolve_realm_binding_for_provider(
            &config,
            Provider::Anthropic,
            None,
            None,
        )
        .expect("env default binding should synthesize");

        assert_eq!(realm.realm_id.as_str(), "env_default");
        assert_eq!(binding_id, "default");
        assert_eq!(auth_binding.realm.as_str(), "env_default");
        assert_eq!(auth_binding.binding.as_str(), "default");
        let (_binding, backend, auth) = realm.lookup_binding("default").unwrap();
        assert_eq!(backend.provider, Provider::Anthropic);
        assert!(matches!(
            &auth.source,
            CredentialSourceSpec::Env { env, fallback }
                if env == "ANTHROPIC_API_KEY" && fallback.is_empty()
        ));
    }

    #[test]
    fn explicit_env_default_auth_binding_is_not_rehydrated_as_durable_identity() {
        let config = Config::default();
        let auth_binding = AuthBindingRef {
            realm: RealmId::parse("env_default").expect("valid realm"),
            binding: BindingId::parse("default").expect("valid binding"),
            profile: None,
            origin: meerkat_core::BindingOrigin::SyntheticEnvDefault,
        };
        let err = AgentFactory::resolve_realm_binding_for_provider(
            &config,
            Provider::OpenAI,
            Some(&auth_binding),
            None,
        )
        .expect_err("env_default is a synthetic fallback, not a durable identity");

        assert!(
            matches!(
                &err,
                meerkat_core::ConnectionTargetError::UnknownRealm(realm)
                    if realm == "env_default"
            ),
            "typed error should name the rejected synthetic realm: {err}"
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn env_default_fallback_does_not_admit_auth_lease_identity() {
        use meerkat_llm_core::provider_runtime::{
            ProviderAuthError, ProviderClientError, ProviderRuntime, ProviderRuntimeRegistry,
            ResolvedConnection, ResolverEnvironment, StaticLease, ValidatedBinding,
        };

        struct RecordingOpenAiRuntime {
            auth_lease_handle_seen: Arc<std::sync::atomic::AtomicBool>,
        }

        #[async_trait::async_trait]
        impl ProviderRuntime for RecordingOpenAiRuntime {
            fn provider_id(&self) -> Provider {
                Provider::OpenAI
            }

            async fn resolve_binding(
                &self,
                binding: &ValidatedBinding,
                env: &ResolverEnvironment,
            ) -> Result<ResolvedConnection, ProviderAuthError> {
                assert_eq!(binding.auth_binding_ref().realm.as_str(), "env_default");
                self.auth_lease_handle_seen.fetch_or(
                    env.auth_lease_handle.is_some(),
                    std::sync::atomic::Ordering::SeqCst,
                );
                Ok(ResolvedConnection {
                    provider: Provider::OpenAI,
                    backend: binding.backend(),
                    backend_profile: Arc::clone(binding.backend_profile()),
                    auth_lease: Arc::new(StaticLease::inline_secret(
                        "test-openai-key".to_string(),
                        meerkat_core::AuthMetadata::default(),
                        None,
                        "test",
                    )),
                })
            }

            fn build_client(
                &self,
                _connection: ResolvedConnection,
            ) -> Result<Arc<dyn LlmClient>, ProviderClientError> {
                Ok(Arc::new(meerkat_client::TestClient::default()))
            }
        }

        let temp = tempfile::tempdir().unwrap();
        let auth_lease_handle_seen = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let provider_registry =
            ProviderRuntimeRegistry::empty().with_runtime(Arc::new(RecordingOpenAiRuntime {
                auth_lease_handle_seen: Arc::clone(&auth_lease_handle_seen),
            }));
        let mut factory = AgentFactory::new(temp.path().join("sessions")).builtins(false);
        factory.provider_registry = Arc::new(provider_registry);

        let session = Session::new();
        let runtime = meerkat_runtime::MeerkatMachine::ephemeral();
        let bindings = runtime
            .prepare_bindings(session.id().clone())
            .await
            .expect("session runtime bindings");
        let mut build = AgentBuildConfig::new("gpt-5.4");
        build.provider = Some(Provider::OpenAI);
        build.resume_session = Some(session);
        build.runtime_build_mode = meerkat_core::RuntimeBuildMode::SessionOwned(bindings.clone());
        build.override_builtins = ToolCategoryOverride::Disable;

        let agent = factory
            .build_agent(build, &Config::default())
            .await
            .unwrap();

        assert!(
            agent
                .session()
                .session_metadata()
                .unwrap()
                .auth_binding
                .is_none(),
            "env-default fallback must not become durable session identity"
        );
        assert!(
            !auth_lease_handle_seen.load(std::sync::atomic::Ordering::SeqCst),
            "env-default fallback resolution must not receive an AuthMachine lease handle"
        );
        let env_default_auth_binding = AuthBindingRef {
            realm: RealmId::parse("env_default").expect("valid realm"),
            binding: BindingId::parse("default").expect("valid binding"),
            profile: None,
            origin: meerkat_core::BindingOrigin::SyntheticEnvDefault,
        };
        let lease_key =
            meerkat_core::handles::LeaseKey::from_auth_binding(&env_default_auth_binding);
        let snapshot = bindings.auth_lease().snapshot(&lease_key);
        assert_eq!(
            snapshot.phase, None,
            "synthetic env-default identity must not be admitted to AuthMachine lease truth"
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn hot_swap_env_default_fallback_does_not_publish_auth_lease() {
        use meerkat_llm_core::provider_runtime::{
            ProviderAuthError, ProviderClientError, ProviderRuntime, ProviderRuntimeRegistry,
            ResolvedConnection, ResolverEnvironment, StaticLease, ValidatedBinding,
        };

        struct RecordingOpenAiRuntime {
            auth_lease_handle_seen: Arc<std::sync::atomic::AtomicBool>,
        }

        #[async_trait::async_trait]
        impl ProviderRuntime for RecordingOpenAiRuntime {
            fn provider_id(&self) -> Provider {
                Provider::OpenAI
            }

            async fn resolve_binding(
                &self,
                binding: &ValidatedBinding,
                env: &ResolverEnvironment,
            ) -> Result<ResolvedConnection, ProviderAuthError> {
                assert_eq!(binding.auth_binding_ref().realm.as_str(), "env_default");
                self.auth_lease_handle_seen.fetch_or(
                    env.auth_lease_handle.is_some(),
                    std::sync::atomic::Ordering::SeqCst,
                );
                Ok(ResolvedConnection {
                    provider: Provider::OpenAI,
                    backend: binding.backend(),
                    backend_profile: Arc::clone(binding.backend_profile()),
                    auth_lease: Arc::new(StaticLease::inline_secret(
                        "test-openai-key".to_string(),
                        meerkat_core::AuthMetadata::default(),
                        None,
                        "test",
                    )),
                })
            }

            fn build_client(
                &self,
                _connection: ResolvedConnection,
            ) -> Result<Arc<dyn LlmClient>, ProviderClientError> {
                Ok(Arc::new(meerkat_client::TestClient::default()))
            }
        }

        let temp = tempfile::tempdir().unwrap();
        let auth_lease_handle_seen = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let provider_registry =
            ProviderRuntimeRegistry::empty().with_runtime(Arc::new(RecordingOpenAiRuntime {
                auth_lease_handle_seen: Arc::clone(&auth_lease_handle_seen),
            }));
        let mut factory = AgentFactory::new(temp.path().join("sessions")).builtins(false);
        factory.provider_registry = Arc::new(provider_registry);

        let runtime = meerkat_runtime::MeerkatMachine::ephemeral();
        let bindings = runtime
            .prepare_bindings(SessionId::new())
            .await
            .expect("session runtime bindings");
        let identity = SessionLlmIdentity {
            model: "gpt-5.4".into(),
            provider: Provider::OpenAI,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: None,
        };

        let _client = factory
            .build_llm_client_for_identity_with_auth_lease(
                &Config::default(),
                &identity,
                Some(bindings.auth_lease().clone()),
            )
            .await
            .expect("env-default hot-swap client should still build");

        assert!(
            !auth_lease_handle_seen.load(std::sync::atomic::Ordering::SeqCst),
            "env-default hot-swap fallback resolution must not receive an AuthMachine lease handle"
        );
        let env_default_auth_binding = AuthBindingRef {
            realm: RealmId::parse("env_default").expect("valid realm"),
            binding: BindingId::parse("default").expect("valid binding"),
            profile: None,
            origin: meerkat_core::BindingOrigin::SyntheticEnvDefault,
        };
        let lease_key =
            meerkat_core::handles::LeaseKey::from_auth_binding(&env_default_auth_binding);
        let snapshot = bindings.auth_lease().snapshot(&lease_key);
        assert_eq!(
            snapshot.phase, None,
            "synthetic env-default hot-swap identity must not be admitted to AuthMachine lease truth"
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn publish_auth_lease_does_not_reacquire_existing_matching_credential() {
        use meerkat_llm_core::provider_runtime::{
            NormalizedBackendKind, ResolvedConnection, StaticLease,
        };

        let runtime = meerkat_runtime::MeerkatMachine::ephemeral();
        let session = Session::new();
        let bindings = runtime
            .prepare_bindings(session.id().clone())
            .await
            .expect("runtime bindings");
        let auth_binding = AuthBindingRef {
            realm: RealmId::parse("session_a").expect("valid realm"),
            binding: meerkat_core::BindingId::parse("default_openai").expect("valid binding"),
            profile: None,
            origin: meerkat_core::BindingOrigin::Configured,
        };
        let expires_at = chrono::Utc::now() + chrono::Duration::hours(1);
        let connection = ResolvedConnection {
            provider: Provider::OpenAI,
            backend: NormalizedBackendKind::OpenAi(
                meerkat_core::provider_matrix::OpenAiBackendKind::OpenAiApi,
            ),
            backend_profile: Arc::new(meerkat_core::BackendProfile {
                id: "openai_api".into(),
                provider: Provider::OpenAI,
                backend_kind: "openai_api".into(),
                base_url: None,
                options: serde_json::Value::Null,
            }),
            auth_lease: Arc::new(StaticLease::inline_secret(
                "managed-oauth-access".into(),
                meerkat_core::AuthMetadata::default(),
                Some(expires_at),
                "openai:managed_chatgpt_oauth",
            )),
        };

        AgentFactory::publish_auth_lease(bindings.auth_lease(), &auth_binding, &connection)
            .expect("first publication");
        let lease_key = meerkat_core::handles::LeaseKey::from_auth_binding(&auth_binding);
        let first_snapshot = bindings.auth_lease().snapshot(&lease_key);

        AgentFactory::publish_auth_lease(bindings.auth_lease(), &auth_binding, &connection)
            .expect("second matching publication should be idempotent");
        let second_snapshot = bindings.auth_lease().snapshot(&lease_key);

        assert_eq!(
            second_snapshot, first_snapshot,
            "factory must not advance AuthMachine publication for an already-visible matching credential"
        );
    }

    #[test]
    fn realm_default_binding_is_explicit_config_auth_source() {
        let mut config = Config::default();
        let mut section = RealmConfigSection {
            default_binding: Some("default_anthropic".to_string()),
            ..Default::default()
        };
        section.backend.insert(
            "anthropic_backend".to_string(),
            BackendProfileConfig {
                provider: "anthropic".to_string(),
                backend_kind: "anthropic_api".to_string(),
                base_url: None,
                options: serde_json::Value::Null,
            },
        );
        section.auth.insert(
            "anthropic_env".to_string(),
            meerkat_core::AuthProfileConfig {
                provider: "anthropic".to_string(),
                auth_method: "api_key".to_string(),
                source: CredentialSourceSpec::Env {
                    env: "ANTHROPIC_API_KEY".to_string(),
                    fallback: Vec::new(),
                },
                constraints: Default::default(),
                metadata_defaults: Default::default(),
            },
        );
        section.binding.insert(
            "default_anthropic".to_string(),
            ProviderBindingConfig {
                backend_profile: "anthropic_backend".to_string(),
                auth_profile: "anthropic_env".to_string(),
                default_model: None,
                policy: Default::default(),
                provider_default: false,
            },
        );
        config.realm.insert("dev".to_string(), section);

        let (_realm, binding_id, auth_binding) = AgentFactory::resolve_realm_binding_for_provider(
            &config,
            Provider::Anthropic,
            None,
            Some(&RealmId::parse("dev").unwrap()),
        )
        .expect("configured default binding should resolve");

        assert_eq!(binding_id, "default_anthropic");
        assert_eq!(auth_binding.realm.as_str(), "dev");
        assert_eq!(auth_binding.binding.as_str(), "default_anthropic");
    }

    // RCT-22: build_agent with an omitted auth_binding and a head realm that
    // does not itself define a provider binding INHERITS the binding from the
    // reserved `global` realm (implicit chain tail) and resolves it at its
    // OWNING realm (global), beating the env fallback. (Replaces the old
    // flat-scan-of-unrelated-realms behavior, which is gone.)
    #[cfg(all(feature = "openai", not(target_arch = "wasm32")))]
    #[tokio::test]
    async fn build_agent_without_auth_binding_inherits_global_realm_binding() {
        let temp = tempfile::tempdir().unwrap();
        let factory = AgentFactory::new(temp.path().join("sessions"));
        let mut config = Config::default();
        let mut section = RealmConfigSection::default();
        section.backend.insert(
            "openai_api".to_string(),
            BackendProfileConfig {
                provider: "openai".to_string(),
                backend_kind: "openai_api".to_string(),
                base_url: None,
                options: serde_json::Value::Null,
            },
        );
        section.auth.insert(
            "openai_key".to_string(),
            meerkat_core::AuthProfileConfig {
                provider: "openai".to_string(),
                auth_method: "api_key".to_string(),
                source: CredentialSourceSpec::InlineSecret {
                    secret: "sk-test-openai".to_string(),
                },
                constraints: Default::default(),
                metadata_defaults: Default::default(),
            },
        );
        section.binding.insert(
            "openai_oauth".to_string(),
            ProviderBindingConfig {
                backend_profile: "openai_api".to_string(),
                auth_profile: "openai_key".to_string(),
                default_model: Some("gpt-5.5".to_string()),
                policy: Default::default(),
                provider_default: false,
            },
        );
        // The binding lives in `global`; the head realm inherits it via the
        // implicit tail.
        config.realm.insert("global".to_string(), section);

        let mut build = AgentBuildConfig::new("gpt-5.4");
        build.realm_id = Some(RealmId::parse("missing").unwrap());
        assert!(build.auth_binding.is_none());

        let agent = factory
            .build_agent(build, &config)
            .await
            .expect("inherited global binding should beat env fallback");
        let metadata = agent
            .session()
            .session_metadata()
            .expect("metadata written");

        assert_eq!(metadata.provider, Provider::OpenAI);
        assert_eq!(
            metadata.auth_binding.as_ref().map(|auth_binding| {
                (
                    auth_binding.realm.as_str().to_string(),
                    auth_binding.binding.as_str().to_string(),
                )
            }),
            Some(("global".to_string(), "openai_oauth".to_string())),
            "inherited binding is owner-stamped at global, not the consuming head"
        );
    }

    #[cfg(all(feature = "openai", not(target_arch = "wasm32")))]
    #[tokio::test]
    async fn omitted_model_surface_resolution_survives_implicit_binding_default_selection() {
        let temp = tempfile::tempdir().unwrap();
        let factory = AgentFactory::new(temp.path().join("sessions"));
        let mut config = Config::default();
        config.agent.model.clear();
        config.models.openai = "gpt-5.4".to_string();
        let mut section = RealmConfigSection::default();
        section.backend.insert(
            "openai_api".to_string(),
            BackendProfileConfig {
                provider: "openai".to_string(),
                backend_kind: "openai_api".to_string(),
                base_url: None,
                options: serde_json::Value::Null,
            },
        );
        section.auth.insert(
            "openai_key".to_string(),
            meerkat_core::AuthProfileConfig {
                provider: "openai".to_string(),
                auth_method: "api_key".to_string(),
                source: CredentialSourceSpec::InlineSecret {
                    secret: "sk-test-openai".to_string(),
                },
                constraints: Default::default(),
                metadata_defaults: Default::default(),
            },
        );
        section.binding.insert(
            "implicit-openai".to_string(),
            ProviderBindingConfig {
                backend_profile: "openai_api".to_string(),
                auth_profile: "openai_key".to_string(),
                default_model: Some("gpt-5.5".to_string()),
                policy: Default::default(),
                provider_default: true,
            },
        );
        config.realm.insert("global".to_string(), section);

        let resolution = resolve_create_session_model(
            &config,
            CreateSessionModelResolutionRequest {
                model: None,
                provider: Some(Provider::OpenAI),
                auth_binding: None,
            },
        )
        .expect("surface model resolution");
        assert_ne!(
            resolution.model, "gpt-5.5",
            "fixture must distinguish surface default from binding default"
        );
        let mut build = AgentBuildConfig::new(resolution.model.clone());
        build.provider = Some(resolution.provider);
        build.auth_binding = resolution.auth_binding.clone();
        assert!(
            !build.resume_override_mask.model,
            "omitted caller model remains omitted provenance"
        );

        let agent = factory
            .build_agent(build, &config)
            .await
            .expect("implicit credential binding should build");
        let metadata = agent
            .session()
            .session_metadata()
            .expect("metadata written");
        assert_eq!(metadata.model, resolution.model);
        assert_eq!(metadata.provider, resolution.provider);
        assert_eq!(
            metadata
                .auth_binding
                .as_ref()
                .map(|binding| binding.binding.as_str()),
            Some("implicit-openai")
        );
    }

    // RCT-20: an image-credential binding inherited from `global` by a child
    // realm that exists but defines no image binding resolves at its OWNING
    // realm (global) — the second stamping site walks the chain too.
    #[cfg(all(feature = "openai", not(target_arch = "wasm32")))]
    #[test]
    fn image_binding_inherited_from_global_stamps_owner() {
        let mut config = Config::default();
        config.realm.insert(
            "global".to_string(),
            openai_realm_with_bindings(&[("primary", "sk-img")]),
        );
        let child = RealmConfigSection {
            parent: Some(RealmId::global()),
            ..Default::default()
        };
        config.realm.insert("child".to_string(), child);

        let child_id = RealmId::parse("child").unwrap();
        let (realm, _binding_id, auth_binding) = AgentFactory::resolve_image_binding_for_provider(
            &config,
            Provider::OpenAI,
            Some(&child_id),
        )
        .expect("image binding inherits from global");
        assert_eq!(
            auth_binding.realm.as_str(),
            "global",
            "image binding is owner-stamped at the defining realm"
        );
        assert_eq!(
            realm.realm_id.as_str(),
            auth_binding.realm.as_str(),
            "registry equality holds for the inherited image binding"
        );
    }

    #[cfg(all(feature = "openai", not(target_arch = "wasm32")))]
    #[tokio::test]
    async fn explicit_model_beats_binding_default_model() {
        let temp = tempfile::tempdir().unwrap();
        let factory = AgentFactory::new(temp.path().join("sessions"));
        let mut config = Config::default();
        let mut section = RealmConfigSection::default();
        section.backend.insert(
            "openai_api".to_string(),
            BackendProfileConfig {
                provider: "openai".to_string(),
                backend_kind: "openai_api".to_string(),
                base_url: None,
                options: serde_json::Value::Null,
            },
        );
        section.auth.insert(
            "openai_key".to_string(),
            meerkat_core::AuthProfileConfig {
                provider: "openai".to_string(),
                auth_method: "api_key".to_string(),
                source: CredentialSourceSpec::InlineSecret {
                    secret: "sk-test-openai".to_string(),
                },
                constraints: Default::default(),
                metadata_defaults: Default::default(),
            },
        );
        section.binding.insert(
            "openai_oauth".to_string(),
            ProviderBindingConfig {
                backend_profile: "openai_api".to_string(),
                auth_profile: "openai_key".to_string(),
                default_model: Some("gpt-5.5".to_string()),
                policy: Default::default(),
                provider_default: false,
            },
        );
        config.realm.insert("dev".to_string(), section);

        let mut build = AgentBuildConfig::new("gpt-5.5");
        build.provider = Some(Provider::OpenAI);
        build.auth_binding = Some(AuthBindingRef {
            realm: meerkat_core::RealmId::parse("dev").unwrap(),
            binding: meerkat_core::BindingId::parse("openai_oauth").unwrap(),
            profile: None,
            origin: meerkat_core::BindingOrigin::Configured,
        });
        build.resume_override_mask.model = true;

        let agent = factory
            .build_agent(build, &config)
            .await
            .expect("explicit model should resolve through explicit binding");
        let metadata = agent
            .session()
            .session_metadata()
            .expect("metadata written");

        assert_eq!(metadata.provider, Provider::OpenAI);
        assert_eq!(metadata.model, "gpt-5.5");
        assert_eq!(
            metadata.auth_binding.as_ref().map(|auth_binding| {
                (
                    auth_binding.realm.as_str().to_string(),
                    auth_binding.binding.as_str().to_string(),
                )
            }),
            Some(("dev".to_string(), "openai_oauth".to_string()))
        );
    }

    #[cfg(all(feature = "anthropic", feature = "openai", not(target_arch = "wasm32")))]
    #[tokio::test]
    async fn build_agent_without_provider_or_auth_binding_does_not_probe_other_providers() {
        use async_trait::async_trait;
        use meerkat_llm_core::provider_runtime::{
            ProviderAuthError, ProviderClientError, ProviderRuntime, ProviderRuntimeRegistry,
            ResolvedConnection, ResolverEnvironment, StaticLease, ValidatedBinding,
        };

        struct FailingOpenAiRuntime;

        #[async_trait]
        impl ProviderRuntime for FailingOpenAiRuntime {
            fn provider_id(&self) -> Provider {
                Provider::OpenAI
            }

            async fn resolve_binding(
                &self,
                _binding: &ValidatedBinding,
                _env: &ResolverEnvironment,
            ) -> Result<ResolvedConnection, ProviderAuthError> {
                Err(ProviderAuthError::Auth(
                    meerkat_core::AuthError::MissingSecret,
                ))
            }

            fn build_client(
                &self,
                _connection: ResolvedConnection,
            ) -> Result<Arc<dyn LlmClient>, ProviderClientError> {
                unreachable!("failing runtime never resolves")
            }
        }

        struct SucceedingAnthropicRuntime;

        #[async_trait]
        impl ProviderRuntime for SucceedingAnthropicRuntime {
            fn provider_id(&self) -> Provider {
                Provider::Anthropic
            }

            async fn resolve_binding(
                &self,
                binding: &ValidatedBinding,
                _env: &ResolverEnvironment,
            ) -> Result<ResolvedConnection, ProviderAuthError> {
                Ok(ResolvedConnection {
                    provider: Provider::Anthropic,
                    backend: binding.backend(),
                    backend_profile: Arc::clone(binding.backend_profile()),
                    auth_lease: Arc::new(StaticLease::inline_secret(
                        "sk-ant-test".to_string(),
                        meerkat_core::AuthMetadata::default(),
                        None,
                        "anthropic:test",
                    )),
                })
            }

            fn build_client(
                &self,
                _connection: ResolvedConnection,
            ) -> Result<Arc<dyn LlmClient>, ProviderClientError> {
                Ok(Arc::new(meerkat_client::TestClient::default()))
            }
        }

        let temp = tempfile::tempdir().unwrap();
        let mut factory = AgentFactory::new(temp.path().join("sessions"));
        factory.provider_registry = Arc::new(
            ProviderRuntimeRegistry::empty()
                .with_runtime(Arc::new(FailingOpenAiRuntime))
                .with_runtime(Arc::new(SucceedingAnthropicRuntime)),
        );
        let mut config = Config::default();
        let mut section = RealmConfigSection::default();
        section.backend.insert(
            "anthropic_api".to_string(),
            BackendProfileConfig {
                provider: "anthropic".to_string(),
                backend_kind: "anthropic_api".to_string(),
                base_url: None,
                options: serde_json::Value::Null,
            },
        );
        section.auth.insert(
            "anthropic_key".to_string(),
            meerkat_core::AuthProfileConfig {
                provider: "anthropic".to_string(),
                auth_method: "api_key".to_string(),
                source: CredentialSourceSpec::InlineSecret {
                    secret: "sk-ant-test".to_string(),
                },
                constraints: Default::default(),
                metadata_defaults: Default::default(),
            },
        );
        section.binding.insert(
            "default_anthropic".to_string(),
            ProviderBindingConfig {
                backend_profile: "anthropic_api".to_string(),
                auth_profile: "anthropic_key".to_string(),
                default_model: Some("claude-opus-4-8".to_string()),
                policy: Default::default(),
                provider_default: false,
            },
        );
        config.realm.insert("dev".to_string(), section);

        let mut build = AgentBuildConfig::new("gpt-5.4");
        build.realm_id = Some(RealmId::parse("missing").unwrap());
        assert!(build.provider.is_none());
        assert!(build.auth_binding.is_none());

        let err = match factory.build_agent(build, &config).await {
            Ok(_) => {
                panic!("OpenAI model resolution must not silently probe Anthropic credentials")
            }
            Err(err) => err,
        };
        assert!(
            err.to_string().contains("openai")
                || err.to_string().contains("OpenAI")
                || err.to_string().contains("missing secret"),
            "error should stay on selected OpenAI path, not switch providers: {err}"
        );
    }

    #[test]
    fn selected_realm_image_binding_does_not_cross_to_default_realm() {
        let mut config = Config::default();
        let mut session_section = RealmConfigSection {
            default_binding: Some("default_anthropic".to_string()),
            ..Default::default()
        };
        session_section.backend.insert(
            "anthropic_backend".to_string(),
            BackendProfileConfig {
                provider: "anthropic".to_string(),
                backend_kind: "anthropic_api".to_string(),
                base_url: None,
                options: serde_json::Value::Null,
            },
        );
        session_section.auth.insert(
            "anthropic_env".to_string(),
            meerkat_core::AuthProfileConfig {
                provider: "anthropic".to_string(),
                auth_method: "api_key".to_string(),
                source: CredentialSourceSpec::Env {
                    env: "ANTHROPIC_API_KEY".to_string(),
                    fallback: Vec::new(),
                },
                constraints: Default::default(),
                metadata_defaults: Default::default(),
            },
        );
        session_section.binding.insert(
            "default_anthropic".to_string(),
            ProviderBindingConfig {
                backend_profile: "anthropic_backend".to_string(),
                auth_profile: "anthropic_env".to_string(),
                default_model: None,
                policy: Default::default(),
                provider_default: false,
            },
        );
        config
            .realm
            .insert("session_a".to_string(), session_section);

        let mut default_section = RealmConfigSection {
            default_binding: Some("default_openai".to_string()),
            ..Default::default()
        };
        default_section.backend.insert(
            "openai_backend".to_string(),
            BackendProfileConfig {
                provider: "openai".to_string(),
                backend_kind: "openai_responses".to_string(),
                base_url: None,
                options: serde_json::Value::Null,
            },
        );
        default_section.auth.insert(
            "openai_env".to_string(),
            meerkat_core::AuthProfileConfig {
                provider: "openai".to_string(),
                auth_method: "api_key".to_string(),
                source: CredentialSourceSpec::Env {
                    env: "OPENAI_API_KEY".to_string(),
                    fallback: Vec::new(),
                },
                constraints: Default::default(),
                metadata_defaults: Default::default(),
            },
        );
        default_section.binding.insert(
            "default_openai".to_string(),
            ProviderBindingConfig {
                backend_profile: "openai_backend".to_string(),
                auth_profile: "openai_env".to_string(),
                default_model: None,
                policy: Default::default(),
                provider_default: false,
            },
        );
        // 'default' is now just an unrelated sibling realm (no longer a magic
        // fallback) — session_a does not parent to it, so it must not be crossed.
        config.realm.insert("default".to_string(), default_section);

        let err = AgentFactory::resolve_image_binding_for_provider(
            &config,
            Provider::OpenAI,
            Some(&RealmId::parse("session_a").unwrap()),
        )
        .expect_err("image lookup must stay on the selected realm's chain");

        // Fails closed for OpenAI on session_a's chain; never crosses to the
        // unrelated 'default' realm's openai binding.
        assert!(
            err.contains("session_a") && !err.contains("default_openai"),
            "image lookup must not cross to the unrelated 'default' realm: {err}"
        );
    }

    #[test]
    fn unconfigured_storage_realm_can_still_use_configured_global_realm() {
        let mut config = Config::default();
        config.realm.insert(
            "global".to_string(),
            inline_realm_section(&[("openai", "default-openai-key")]),
        );

        let (realm, binding_id, auth_binding) = AgentFactory::resolve_image_binding_for_provider(
            &config,
            Provider::OpenAI,
            Some(&RealmId::parse("session_missing").unwrap()),
        )
        .expect("unconfigured storage realm inherits the global realm credentials");

        assert_eq!(realm.realm_id.as_str(), "global");
        assert_eq!(binding_id, "default_openai");
        assert_eq!(auth_binding.realm.as_str(), "global");
        assert_eq!(auth_binding.binding.as_str(), "default_openai");
    }

    #[test]
    fn unscoped_image_binding_can_still_synthesize_env_default() {
        let config = Config::default();
        let (realm, binding_id, auth_binding) =
            AgentFactory::resolve_image_binding_for_provider(&config, Provider::Gemini, None)
                .expect("unscoped image lookup may use env_default");

        assert_eq!(realm.realm_id.as_str(), "env_default");
        assert_eq!(binding_id, "default");
        assert_eq!(auth_binding.realm.as_str(), "env_default");
        assert_eq!(auth_binding.binding.as_str(), "default");
    }

    #[test]
    fn selected_env_default_image_binding_can_synthesize_env_default() {
        let config = Config::default();
        let (realm, binding_id, auth_binding) = AgentFactory::resolve_image_binding_for_provider(
            &config,
            Provider::Gemini,
            Some(&RealmId::parse("env_default").unwrap()),
        )
        .expect("explicit env_default image lookup may use env_default credentials");

        assert_eq!(realm.realm_id.as_str(), "env_default");
        assert_eq!(binding_id, "default");
        assert_eq!(auth_binding.realm.as_str(), "env_default");
        assert_eq!(auth_binding.binding.as_str(), "default");
    }

    #[test]
    fn unscoped_image_binding_can_still_use_configured_global_realm() {
        let mut config = Config::default();
        config.realm.insert(
            "global".to_string(),
            inline_realm_section(&[("openai", "default-openai-key")]),
        );

        // No selected realm: the head defaults to the reserved `global` root.
        let (realm, binding_id, auth_binding) =
            AgentFactory::resolve_image_binding_for_provider(&config, Provider::OpenAI, None)
                .expect("unscoped image lookup resolves the global realm");

        assert_eq!(realm.realm_id.as_str(), "global");
        assert_eq!(binding_id, "default_openai");
        assert_eq!(auth_binding.realm.as_str(), "global");
        assert_eq!(auth_binding.binding.as_str(), "default_openai");
    }

    #[test]
    fn unconfigured_storage_realm_can_still_synthesize_env_default_image_binding() {
        let config = Config::default();
        let (realm, binding_id, auth_binding) = AgentFactory::resolve_image_binding_for_provider(
            &config,
            Provider::OpenAI,
            Some(&RealmId::parse("workspace_derived").unwrap()),
        )
        .expect("workspace storage realm without credential config may use env_default");

        assert_eq!(realm.realm_id.as_str(), "env_default");
        assert_eq!(binding_id, "default");
        assert_eq!(auth_binding.realm.as_str(), "env_default");
        assert_eq!(auth_binding.binding.as_str(), "default");
    }

    #[cfg(all(feature = "openai", not(target_arch = "wasm32")))]
    #[derive(Clone, Debug)]
    struct RecordedSelfHostedResolve {
        auth_binding: AuthBindingRef,
        backend_base_url: Option<String>,
        auth_source: CredentialSourceSpec,
        token_store_present: bool,
        auth_lease_handle_present: bool,
    }

    #[cfg(all(feature = "openai", not(target_arch = "wasm32")))]
    struct RecordingSelfHostedRuntime {
        calls: Arc<std::sync::Mutex<Vec<RecordedSelfHostedResolve>>>,
        authless: bool,
    }

    #[cfg(all(feature = "openai", not(target_arch = "wasm32")))]
    #[async_trait::async_trait]
    impl meerkat_llm_core::provider_runtime::ProviderRuntime for RecordingSelfHostedRuntime {
        fn provider_id(&self) -> Provider {
            Provider::SelfHosted
        }

        async fn resolve_binding(
            &self,
            binding: &meerkat_llm_core::provider_runtime::ValidatedBinding,
            env: &meerkat_llm_core::provider_runtime::ResolverEnvironment,
        ) -> Result<
            meerkat_llm_core::provider_runtime::ResolvedConnection,
            meerkat_llm_core::provider_runtime::ProviderAuthError,
        > {
            self.calls.lock().unwrap().push(RecordedSelfHostedResolve {
                auth_binding: binding.auth_binding_ref().clone(),
                backend_base_url: binding.backend_profile().base_url.clone(),
                auth_source: binding.auth_profile().source.clone(),
                token_store_present: env.provider_auth_persistence().is_some(),
                auth_lease_handle_present: env.auth_lease_handle.is_some(),
            });
            let auth_lease: Arc<dyn meerkat_core::AuthLease> = if self.authless {
                Arc::new(
                    meerkat_llm_core::provider_runtime::StaticLease::empty_lease(
                        meerkat_core::AuthMetadata::default(),
                        "test-self-hosted-none",
                    ),
                )
            } else {
                Arc::new(
                    meerkat_llm_core::provider_runtime::StaticLease::inline_secret(
                        "resolved-self-hosted-token".to_string(),
                        meerkat_core::AuthMetadata::default(),
                        None,
                        "test-self-hosted",
                    ),
                )
            };
            Ok(meerkat_llm_core::provider_runtime::ResolvedConnection {
                provider: Provider::SelfHosted,
                backend: binding.backend(),
                backend_profile: Arc::clone(binding.backend_profile()),
                auth_lease,
            })
        }

        fn build_client(
            &self,
            _connection: meerkat_llm_core::provider_runtime::ResolvedConnection,
        ) -> Result<Arc<dyn LlmClient>, meerkat_llm_core::provider_runtime::ProviderClientError>
        {
            Ok(Arc::new(meerkat_client::TestClient::default()))
        }
    }

    #[cfg(all(feature = "openai", not(target_arch = "wasm32")))]
    fn install_recording_self_hosted_runtime(
        factory: &mut AgentFactory,
    ) -> Arc<std::sync::Mutex<Vec<RecordedSelfHostedResolve>>> {
        let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
        factory.provider_registry = Arc::new(
            meerkat_llm_core::provider_runtime::ProviderRuntimeRegistry::empty().with_runtime(
                Arc::new(RecordingSelfHostedRuntime {
                    calls: Arc::clone(&calls),
                    authless: false,
                }),
            ),
        );
        calls
    }

    #[cfg(all(feature = "openai", not(target_arch = "wasm32")))]
    fn install_authless_self_hosted_runtime(
        factory: &mut AgentFactory,
    ) -> Arc<std::sync::Mutex<Vec<RecordedSelfHostedResolve>>> {
        let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
        factory.provider_registry = Arc::new(
            meerkat_llm_core::provider_runtime::ProviderRuntimeRegistry::empty().with_runtime(
                Arc::new(RecordingSelfHostedRuntime {
                    calls: Arc::clone(&calls),
                    authless: true,
                }),
            ),
        );
        calls
    }

    #[cfg(all(feature = "openai", not(target_arch = "wasm32")))]
    fn config_with_self_hosted_server(server: SelfHostedServerConfig) -> Config {
        config_with_self_hosted_server_id("local", server)
    }

    #[cfg(all(feature = "openai", not(target_arch = "wasm32")))]
    fn config_with_self_hosted_server_id(
        server_id: &str,
        server: SelfHostedServerConfig,
    ) -> Config {
        let mut config = Config::default();
        config
            .self_hosted
            .servers
            .insert(server_id.to_string(), server);
        config.self_hosted.models.insert(
            "gemma-4-e2b".to_string(),
            SelfHostedModelConfig {
                server: server_id.to_string(),
                remote_model: "gemma4:e2b".to_string(),
                display_name: "Gemma 4 E2B".into(),
                family: "gemma-4".to_string(),
                tier: meerkat_core::model_profile::catalog::ModelTier::Supported,
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
        config
    }

    #[cfg(all(feature = "openai", not(target_arch = "wasm32")))]
    fn insert_self_hosted_realm_binding(
        config: &mut Config,
        realm_id: &str,
        binding_id: &str,
        base_url: &str,
        source: CredentialSourceSpec,
    ) {
        let backend_id = format!("{binding_id}_backend");
        let auth_id = format!("{binding_id}_auth");
        let mut realm = RealmConfigSection {
            default_binding: Some(binding_id.to_string()),
            ..Default::default()
        };
        realm.backend.insert(
            backend_id.clone(),
            BackendProfileConfig {
                provider: "self_hosted".to_string(),
                backend_kind: "self_hosted".to_string(),
                base_url: Some(base_url.to_string()),
                options: serde_json::Value::Null,
            },
        );
        realm.auth.insert(
            auth_id.clone(),
            meerkat_core::AuthProfileConfig {
                provider: "self_hosted".to_string(),
                auth_method: "static_bearer".to_string(),
                source,
                constraints: Default::default(),
                metadata_defaults: Default::default(),
            },
        );
        realm.binding.insert(
            binding_id.to_string(),
            ProviderBindingConfig {
                backend_profile: backend_id,
                auth_profile: auth_id,
                default_model: Some("gemma4:e2b".to_string()),
                policy: Default::default(),
                provider_default: false,
            },
        );
        config.realm.insert(realm_id.to_string(), realm);
    }

    #[cfg(all(feature = "openai", not(target_arch = "wasm32")))]
    #[tokio::test]
    async fn self_hosted_server_without_canonical_realm_binding_fails_closed() {
        let temp = tempfile::tempdir().unwrap();
        let mut factory = AgentFactory::new(temp.path().join("sessions")).builtins(false);
        let calls = install_recording_self_hosted_runtime(&mut factory);

        // A `[self_hosted]` server with a non-slug id and no canonical realm
        // binding must fail closed. No transient `self_hosted_legacy` realm is
        // synthesized and no FNV-hashed transient binding id is fabricated from
        // the server id string.
        let config = config_with_self_hosted_server_id(
            "localhost:11434",
            SelfHostedServerConfig {
                transport: SelfHostedTransport::OpenAiCompatible,
                base_url: "http://localhost:11434".to_string(),
                api_style: SelfHostedApiStyle::ChatCompletions,
            },
        );
        let mut build = AgentBuildConfig::new("gemma-4-e2b");
        build.provider = Some(Provider::SelfHosted);
        build.override_builtins = ToolCategoryOverride::Disable;

        let err = match factory.build_agent(build, &config).await {
            Ok(_) => panic!("self-hosted server without canonical realm binding must fail closed"),
            Err(err) => err,
        };

        assert!(
            calls.lock().unwrap().is_empty(),
            "missing canonical realm binding must not synthesize a transient legacy connection"
        );
        assert!(
            err.to_string().contains("self-hosted")
                && err.to_string().contains("no canonical realm binding"),
            "unexpected error: {err}"
        );
    }

    #[cfg(all(feature = "openai", not(target_arch = "wasm32")))]
    #[tokio::test]
    async fn self_hosted_realm_runtime_resolution_publishes_auth_lease_visibility() {
        let temp = tempfile::tempdir().unwrap();
        let mut factory = AgentFactory::new(temp.path().join("sessions")).builtins(false);
        let calls = install_recording_self_hosted_runtime(&mut factory);
        let mut config = config_with_self_hosted_server(SelfHostedServerConfig {
            transport: SelfHostedTransport::OpenAiCompatible,
            base_url: "http://127.0.0.1:11434".to_string(),
            api_style: SelfHostedApiStyle::ChatCompletions,
        });
        insert_self_hosted_realm_binding(
            &mut config,
            "global",
            "default_local",
            "http://realm.example/v1",
            CredentialSourceSpec::InlineSecret {
                secret: "realm-token".to_string(),
            },
        );

        let session = Session::new();
        let runtime = meerkat_runtime::MeerkatMachine::ephemeral();
        let bindings = runtime
            .prepare_bindings(session.id().clone())
            .await
            .unwrap();
        let mut build = AgentBuildConfig::new("gemma-4-e2b");
        build.provider = Some(Provider::SelfHosted);
        build.resume_session = Some(session);
        build.runtime_build_mode = meerkat_core::RuntimeBuildMode::SessionOwned(bindings.clone());
        build.override_builtins = ToolCategoryOverride::Disable;

        factory.build_agent(build, &config).await.unwrap();

        let expected_filter = meerkat_core::tool_scope::ToolFilter::Deny(
            [meerkat_core::VIEW_IMAGE_TOOL_NAME.to_string()]
                .into_iter()
                .collect(),
        );
        assert_eq!(
            bindings
                .tool_visibility_owner()
                .visibility_state()
                .unwrap()
                .capability_base_filter,
            expected_filter,
            "Chat Completions self-hosted models must hide view_image even when raw config requests image tool results"
        );

        let calls = calls.lock().unwrap();
        assert_eq!(
            calls.len(),
            1,
            "self-hosted must resolve through ProviderRuntimeRegistry"
        );
        assert!(
            calls[0].auth_lease_handle_present,
            "session-owned auth lease handle must be passed into runtime resolution"
        );
        let lease_key = meerkat_core::handles::LeaseKey::from_auth_binding(&calls[0].auth_binding);
        let snapshot = bindings.auth_lease().snapshot(&lease_key);
        assert_eq!(
            snapshot.phase,
            Some(meerkat_core::handles::AuthLeasePhase::Valid),
            "self-hosted runtime resolution must publish the resolved lease to AuthMachine"
        );
    }

    #[cfg(all(feature = "openai", not(target_arch = "wasm32")))]
    #[tokio::test]
    async fn self_hosted_authless_realm_resolution_does_not_publish_credential_lifecycle() {
        let temp = tempfile::tempdir().unwrap();
        let mut factory = AgentFactory::new(temp.path().join("sessions")).builtins(false);
        let calls = install_authless_self_hosted_runtime(&mut factory);
        let mut config = config_with_self_hosted_server(SelfHostedServerConfig {
            transport: SelfHostedTransport::OpenAiCompatible,
            base_url: "http://127.0.0.1:11434".to_string(),
            api_style: SelfHostedApiStyle::ChatCompletions,
        });
        insert_self_hosted_realm_binding(
            &mut config,
            "dev",
            "dev_local",
            "http://realm.example/v1",
            CredentialSourceSpec::ManagedStore,
        );
        config
            .realm
            .get_mut("dev")
            .unwrap()
            .auth
            .get_mut("dev_local_auth")
            .unwrap()
            .auth_method = "none".to_string();

        let session = Session::new();
        let runtime = meerkat_runtime::MeerkatMachine::ephemeral();
        let bindings = runtime
            .prepare_bindings(session.id().clone())
            .await
            .unwrap();
        let mut build = AgentBuildConfig::new("gemma-4-e2b");
        build.provider = Some(Provider::SelfHosted);
        build.realm_id = Some(RealmId::parse("dev").unwrap());
        build.resume_session = Some(session);
        build.runtime_build_mode = meerkat_core::RuntimeBuildMode::SessionOwned(bindings.clone());
        build.override_builtins = ToolCategoryOverride::Disable;

        factory.build_agent(build, &config).await.unwrap();

        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        let lease_key = meerkat_core::handles::LeaseKey::from_auth_binding(&calls[0].auth_binding);
        let snapshot = bindings.auth_lease().snapshot(&lease_key);
        assert_eq!(snapshot.phase, None);
        assert!(!snapshot.credential_present);
        assert_eq!(snapshot.generation, 0);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn factory_accepts_local_machine_bindings_for_pre_authoritative_mob_resources() {
        let temp = tempfile::tempdir().unwrap();
        let factory = AgentFactory::new(temp.path().join("sessions")).builtins(false);
        let session = Session::new();
        let runtime = meerkat_runtime::MeerkatMachine::ephemeral();
        let bindings = runtime
            .prepare_local_session_bindings(session.id().clone())
            .await
            .expect("local session runtime bindings");
        let mut build = AgentBuildConfig::new("claude-sonnet-4-5");
        build.provider = Some(Provider::Anthropic);
        build.llm_client_override = Some(Arc::new(meerkat_client::TestClient::default()));
        build.resume_session = Some(session);
        build.runtime_build_mode = meerkat_core::RuntimeBuildMode::SessionOwned(bindings.clone());
        build.override_builtins = ToolCategoryOverride::Disable;

        let agent = factory
            .build_agent(build, &Config::default())
            .await
            .unwrap();

        assert_eq!(agent.session().id(), bindings.session_id());
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn factory_capability_filter_rejects_malformed_canonical_visibility_state() {
        let temp = tempfile::tempdir().unwrap();
        let factory = AgentFactory::new(temp.path().join("sessions")).builtins(false);
        let session = session_with_raw_metadata(
            Session::new(),
            meerkat_core::SESSION_TOOL_VISIBILITY_STATE_KEY,
            serde_json::json!("not-a-visibility-state"),
        );
        let runtime = meerkat_runtime::MeerkatMachine::ephemeral();
        let bindings = runtime
            .prepare_bindings(session.id().clone())
            .await
            .expect("session runtime bindings");
        let mut build = AgentBuildConfig::new("claude-sonnet-4-5");
        build.provider = Some(Provider::Anthropic);
        build.llm_client_override = Some(Arc::new(meerkat_client::TestClient::default()));
        build.resume_session = Some(session);
        build.runtime_build_mode = meerkat_core::RuntimeBuildMode::SessionOwned(bindings.clone());
        build.override_builtins = ToolCategoryOverride::Disable;

        let err = match factory.build_agent(build, &Config::default()).await {
            Ok(_) => panic!("malformed canonical visibility metadata must fail closed"),
            Err(err) => err,
        };

        assert!(
            err.to_string()
                .contains("invalid canonical tool visibility state"),
            "unexpected error: {err}"
        );
        assert_eq!(
            bindings.tool_visibility_owner().visibility_state().unwrap(),
            meerkat_core::SessionToolVisibilityState::default(),
            "factory failure must not install default visibility through the machine owner"
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn factory_capability_filter_installs_through_runtime_visibility_owner() {
        let temp = tempfile::tempdir().unwrap();
        let factory = AgentFactory::new(temp.path().join("sessions")).builtins(false);
        let session = Session::new();
        let runtime = meerkat_runtime::MeerkatMachine::ephemeral();
        let bindings = runtime
            .prepare_bindings(session.id().clone())
            .await
            .expect("session runtime bindings");
        let mut build = AgentBuildConfig::new("gpt-5.4");
        build.provider = Some(Provider::OpenAI);
        build.llm_client_override = Some(Arc::new(meerkat_client::TestClient::default()));
        build.resume_session = Some(session);
        build.runtime_build_mode = meerkat_core::RuntimeBuildMode::SessionOwned(bindings.clone());
        build.override_builtins = ToolCategoryOverride::Disable;

        let agent = factory
            .build_agent(build, &Config::default())
            .await
            .unwrap();

        let expected_filter = meerkat_core::tool_scope::ToolFilter::All;
        let owner_state = bindings.tool_visibility_owner().visibility_state().unwrap();
        assert_eq!(&owner_state.capability_base_filter, &expected_filter);
        assert_eq!(
            agent
                .session()
                .try_tool_visibility_state()
                .expect("parse visibility"),
            Some(owner_state),
            "runtime-backed sessions must persist the generated-owner visibility projection as a rebuildable CAS parent"
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn factory_resumed_initial_visibility_merge_preserves_canonical_state() {
        let temp = tempfile::tempdir().unwrap();
        let factory = AgentFactory::new(temp.path().join("sessions")).builtins(false);
        let session = Session::new();
        let original_state = SessionToolVisibilityState {
            inherited_base_filter: meerkat_core::tool_scope::ToolFilter::Deny(
                ["old_parent".to_string()].into_iter().collect(),
            ),
            active_filter: meerkat_core::tool_scope::ToolFilter::Deny(
                ["active_secret".to_string()].into_iter().collect(),
            ),
            staged_filter: meerkat_core::tool_scope::ToolFilter::Allow(
                ["staged_visible".to_string()].into_iter().collect(),
            ),
            active_requested_deferred_names: ["deferred_existing".into()].into_iter().collect(),
            active_revision: 11,
            staged_revision: 13,
            requested_witnesses: [(
                "deferred_existing".into(),
                meerkat_core::ToolVisibilityWitness {
                    last_seen_provenance: Some(meerkat_core::ToolProvenance {
                        kind: meerkat_core::ToolSourceKind::Callback,
                        source_id: "deferred_existing".into(),
                    }),
                },
            )]
            .into_iter()
            .collect(),
            filter_witnesses: [
                (
                    "active_secret".into(),
                    meerkat_core::ToolVisibilityWitness {
                        last_seen_provenance: Some(meerkat_core::ToolProvenance {
                            kind: meerkat_core::ToolSourceKind::Callback,
                            source_id: "active_secret".into(),
                        }),
                    },
                ),
                (
                    "staged_visible".into(),
                    meerkat_core::ToolVisibilityWitness {
                        last_seen_provenance: Some(meerkat_core::ToolProvenance {
                            kind: meerkat_core::ToolSourceKind::Callback,
                            source_id: "staged_visible".into(),
                        }),
                    },
                ),
            ]
            .into_iter()
            .collect(),
            ..Default::default()
        };
        let session = session_with_raw_metadata(
            session,
            meerkat_core::SESSION_TOOL_VISIBILITY_STATE_KEY,
            serde_json::to_value(original_state.clone()).expect("visibility state"),
        );
        let runtime = meerkat_runtime::MeerkatMachine::ephemeral();
        let bindings = runtime
            .prepare_bindings(session.id().clone())
            .await
            .expect("session runtime bindings");
        let inherited_filter = meerkat_core::tool_scope::ToolFilter::Deny(
            ["parent_shell".to_string()].into_iter().collect(),
        );
        let (inherited_authority, inherited_filter_witnesses) =
            inherited_visibility_authority(inherited_filter.clone(), &["parent_shell"]);
        let mut build = AgentBuildConfig::new("claude-sonnet-4-5");
        build.provider = Some(Provider::Anthropic);
        build.llm_client_override = Some(Arc::new(meerkat_client::TestClient::default()));
        build.resume_session = Some(session);
        build.runtime_build_mode = meerkat_core::RuntimeBuildMode::SessionOwned(bindings.clone());
        build.override_builtins = ToolCategoryOverride::Disable;
        build.initial_tool_visibility_state = Some(inherited_authority);
        let agent = factory
            .build_agent(build, &Config::default())
            .await
            .unwrap();

        let visibility_state = agent
            .session()
            .try_tool_visibility_state()
            .expect("parse visibility")
            .expect("visibility state");
        assert_eq!(visibility_state.inherited_base_filter, inherited_filter);
        assert_eq!(visibility_state.active_filter, original_state.active_filter);
        assert_eq!(visibility_state.staged_filter, original_state.staged_filter);
        assert_eq!(
            visibility_state.active_revision,
            original_state.active_revision
        );
        assert_eq!(
            visibility_state.staged_revision,
            original_state.staged_revision
        );
        assert_eq!(
            visibility_state.requested_witnesses,
            original_state.requested_witnesses
        );
        let mut expected_filter_witnesses = original_state.filter_witnesses.clone();
        expected_filter_witnesses.extend(inherited_filter_witnesses);
        assert_eq!(visibility_state.filter_witnesses, expected_filter_witnesses);
        let owner_state = bindings.tool_visibility_owner().visibility_state().unwrap();
        assert_eq!(owner_state.active_filter, original_state.active_filter);
        assert_eq!(owner_state.staged_filter, original_state.staged_filter);
        assert_eq!(owner_state.active_revision, original_state.active_revision);
        assert_eq!(owner_state.staged_revision, original_state.staged_revision);
        assert_eq!(
            owner_state.requested_witnesses,
            original_state.requested_witnesses
        );
        assert_eq!(owner_state.filter_witnesses, expected_filter_witnesses);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn factory_initial_visibility_handoff_applies_for_fresh_session() {
        let temp = tempfile::tempdir().unwrap();
        let factory = AgentFactory::new(temp.path().join("sessions")).builtins(false);
        let inherited_filter = meerkat_core::tool_scope::ToolFilter::Deny(
            ["parent_shell".to_string()].into_iter().collect(),
        );
        let (inherited_authority, inherited_filter_witnesses) =
            inherited_visibility_authority(inherited_filter.clone(), &["parent_shell"]);
        let mut build = AgentBuildConfig::new("claude-sonnet-4-5");
        build.provider = Some(Provider::Anthropic);
        build.llm_client_override = Some(Arc::new(meerkat_client::TestClient::default()));
        build.override_builtins = ToolCategoryOverride::Disable;
        build.initial_tool_visibility_state = Some(inherited_authority);

        let agent = factory
            .build_agent(build, &Config::default())
            .await
            .unwrap();

        let visibility_state = agent
            .session()
            .try_tool_visibility_state()
            .expect("parse visibility")
            .expect("visibility state");
        assert_eq!(visibility_state.inherited_base_filter, inherited_filter);
        assert_eq!(
            visibility_state.filter_witnesses,
            inherited_filter_witnesses
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn factory_resumed_initial_visibility_rejects_malformed_canonical_state() {
        let temp = tempfile::tempdir().unwrap();
        let factory = AgentFactory::new(temp.path().join("sessions")).builtins(false);
        let session = session_with_raw_metadata(
            Session::new(),
            meerkat_core::SESSION_TOOL_VISIBILITY_STATE_KEY,
            serde_json::json!("not-a-visibility-state"),
        );
        let runtime = meerkat_runtime::MeerkatMachine::ephemeral();
        let bindings = runtime
            .prepare_bindings(session.id().clone())
            .await
            .expect("session runtime bindings");
        let mut build = AgentBuildConfig::new("claude-sonnet-4-5");
        build.provider = Some(Provider::Anthropic);
        build.llm_client_override = Some(Arc::new(meerkat_client::TestClient::default()));
        build.resume_session = Some(session);
        build.runtime_build_mode = meerkat_core::RuntimeBuildMode::SessionOwned(bindings.clone());
        build.override_builtins = ToolCategoryOverride::Disable;
        build.initial_tool_visibility_state = Some(
            inherited_visibility_authority(
                meerkat_core::tool_scope::ToolFilter::Deny(
                    ["parent_shell".to_string()].into_iter().collect(),
                ),
                &["parent_shell"],
            )
            .0,
        );

        let err = match factory.build_agent(build, &Config::default()).await {
            Ok(_) => panic!("malformed resumed canonical visibility must fail closed"),
            Err(err) => err,
        };

        assert!(
            err.to_string()
                .contains("invalid canonical tool visibility state"),
            "unexpected error: {err}"
        );
        assert_eq!(
            bindings.tool_visibility_owner().visibility_state().unwrap(),
            SessionToolVisibilityState::default(),
            "factory failure must not overwrite malformed state and install default visibility"
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn factory_initial_visibility_rejects_raw_canonical_metadata_handoff() {
        let temp = tempfile::tempdir().unwrap();
        let factory = AgentFactory::new(temp.path().join("sessions")).builtins(false);
        let session = Session::new();
        let runtime = meerkat_runtime::MeerkatMachine::ephemeral();
        let bindings = runtime
            .prepare_bindings(session.id().clone())
            .await
            .expect("session runtime bindings");
        let mut build = AgentBuildConfig::new("claude-sonnet-4-5");
        build.provider = Some(Provider::Anthropic);
        build.llm_client_override = Some(Arc::new(meerkat_client::TestClient::default()));
        build.resume_session = Some(session);
        build.runtime_build_mode = meerkat_core::RuntimeBuildMode::SessionOwned(bindings.clone());
        build.override_builtins = ToolCategoryOverride::Disable;
        build.initial_metadata_entries.insert(
            meerkat_core::SESSION_TOOL_VISIBILITY_STATE_KEY.to_string(),
            serde_json::to_value(SessionToolVisibilityState {
                active_filter: meerkat_core::tool_scope::ToolFilter::Deny(
                    ["active_secret".to_string()].into_iter().collect(),
                ),
                ..Default::default()
            })
            .expect("visibility state"),
        );

        let err = match factory.build_agent(build, &Config::default()).await {
            Ok(_) => {
                panic!("initial metadata must not install raw visibility authority")
            }
            Err(err) => err,
        };

        assert!(
            err.to_string()
                .contains("metadata key `session_tool_visibility_state_v1` is reserved"),
            "unexpected error: {err}"
        );
        assert_eq!(
            bindings.tool_visibility_owner().visibility_state().unwrap(),
            SessionToolVisibilityState::default()
        );
    }

    #[cfg(all(feature = "openai", not(target_arch = "wasm32")))]
    #[tokio::test]
    async fn self_hosted_absent_auth_binding_uses_selected_realm_default() {
        let temp = tempfile::tempdir().unwrap();
        let mut factory = AgentFactory::new(temp.path().join("sessions")).builtins(false);
        let calls = install_recording_self_hosted_runtime(&mut factory);
        let mut config = config_with_self_hosted_server(SelfHostedServerConfig {
            transport: SelfHostedTransport::OpenAiCompatible,
            base_url: "http://server.example/v1".to_string(),
            api_style: SelfHostedApiStyle::ChatCompletions,
        });
        insert_self_hosted_realm_binding(
            &mut config,
            "dev",
            "dev_local",
            "http://realm.example/v1",
            CredentialSourceSpec::InlineSecret {
                secret: "realm-token".to_string(),
            },
        );
        let mut build = AgentBuildConfig::new("gemma-4-e2b");
        build.provider = Some(Provider::SelfHosted);
        build.realm_id = Some(RealmId::parse("dev").unwrap());
        build.override_builtins = ToolCategoryOverride::Disable;

        let agent = factory.build_agent(build, &config).await.unwrap();

        let calls = calls.lock().unwrap();
        assert_eq!(
            calls.len(),
            1,
            "self-hosted realm default must resolve through ProviderRuntimeRegistry"
        );
        let call = &calls[0];
        assert_eq!(call.auth_binding.realm.as_str(), "dev");
        assert_eq!(call.auth_binding.binding.as_str(), "dev_local");
        assert_eq!(
            call.backend_base_url.as_deref(),
            Some("http://realm.example/v1")
        );
        assert!(
            matches!(
                &call.auth_source,
                CredentialSourceSpec::InlineSecret { secret } if secret == "realm-token"
            ),
            "selected realm auth must resolve through the realm binding"
        );
        assert_eq!(
            agent
                .session()
                .session_metadata()
                .unwrap()
                .auth_binding
                .as_ref()
                .map(|auth_binding| (
                    auth_binding.realm.as_str().to_string(),
                    auth_binding.binding.as_str().to_string()
                )),
            Some(("dev".to_string(), "dev_local".to_string())),
            "selected realm default binding must become durable session identity"
        );
    }

    #[cfg(all(feature = "openai", not(target_arch = "wasm32")))]
    #[tokio::test]
    async fn self_hosted_absent_auth_binding_uses_global_realm() {
        let temp = tempfile::tempdir().unwrap();
        let mut factory = AgentFactory::new(temp.path().join("sessions")).builtins(false);
        let calls = install_recording_self_hosted_runtime(&mut factory);
        let mut config = config_with_self_hosted_server(SelfHostedServerConfig {
            transport: SelfHostedTransport::OpenAiCompatible,
            base_url: "http://server.example/v1".to_string(),
            api_style: SelfHostedApiStyle::ChatCompletions,
        });
        insert_self_hosted_realm_binding(
            &mut config,
            "global",
            "default_local",
            "http://global-realm.example/v1",
            CredentialSourceSpec::InlineSecret {
                secret: "global-realm-token".to_string(),
            },
        );
        let mut build = AgentBuildConfig::new("gemma-4-e2b");
        build.provider = Some(Provider::SelfHosted);
        build.override_builtins = ToolCategoryOverride::Disable;

        let agent = factory.build_agent(build, &config).await.unwrap();

        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        let call = &calls[0];
        assert_eq!(call.auth_binding.realm.as_str(), "global");
        assert_eq!(call.auth_binding.binding.as_str(), "default_local");
        assert_eq!(
            call.backend_base_url.as_deref(),
            Some("http://global-realm.example/v1")
        );
        assert!(
            matches!(
                &call.auth_source,
                CredentialSourceSpec::InlineSecret { secret } if secret == "global-realm-token"
            ),
            "global realm auth must resolve through the realm binding"
        );
        assert_eq!(
            agent
                .session()
                .session_metadata()
                .unwrap()
                .auth_binding
                .as_ref()
                .map(|auth_binding| (
                    auth_binding.realm.as_str().to_string(),
                    auth_binding.binding.as_str().to_string()
                )),
            Some(("global".to_string(), "default_local".to_string())),
            "global realm binding must become durable session identity"
        );
    }

    #[cfg(all(feature = "openai", not(target_arch = "wasm32")))]
    #[tokio::test]
    async fn self_hosted_selected_realm_without_binding_fails_closed() {
        let temp = tempfile::tempdir().unwrap();
        let mut factory = AgentFactory::new(temp.path().join("sessions")).builtins(false);
        let calls = install_recording_self_hosted_runtime(&mut factory);
        let mut config = config_with_self_hosted_server(SelfHostedServerConfig {
            transport: SelfHostedTransport::OpenAiCompatible,
            base_url: "http://server.example/v1".to_string(),
            api_style: SelfHostedApiStyle::ChatCompletions,
        });
        config
            .realm
            .insert("dev".to_string(), RealmConfigSection::default());
        let mut build = AgentBuildConfig::new("gemma-4-e2b");
        build.provider = Some(Provider::SelfHosted);
        build.realm_id = Some(RealmId::parse("dev").unwrap());
        build.override_builtins = ToolCategoryOverride::Disable;

        let err = match factory.build_agent(build, &config).await {
            Ok(_) => panic!("selected realm without self-hosted binding must fail closed"),
            Err(err) => err,
        };

        assert!(
            calls.lock().unwrap().is_empty(),
            "selected realm failure must not fall back to any other resolution path"
        );
        assert!(
            err.to_string().contains("dev") && err.to_string().contains("self_hosted"),
            "unexpected error: {err}"
        );
    }

    #[cfg(all(feature = "openai", not(target_arch = "wasm32")))]
    #[tokio::test]
    async fn self_hosted_without_credentials_or_realm_binding_fails_closed() {
        let temp = tempfile::tempdir().unwrap();
        let factory = AgentFactory::new(temp.path().join("sessions")).builtins(false);
        // A credential-free `[self_hosted]` server is no longer silently
        // resolved through a synthesized transient `self_hosted_legacy` realm:
        // without a canonical realm binding it fails closed, leaving the
        // canonical `RealmConnectionSet` as the sole owner of the connection.
        let config = config_with_self_hosted_server(SelfHostedServerConfig {
            transport: SelfHostedTransport::OpenAiCompatible,
            base_url: "http://127.0.0.1:11434".to_string(),
            api_style: SelfHostedApiStyle::ChatCompletions,
        });
        let mut build = AgentBuildConfig::new("gemma-4-e2b");
        build.provider = Some(Provider::SelfHosted);
        build.override_builtins = ToolCategoryOverride::Disable;

        let err = match factory.build_agent(build, &config).await {
            Ok(_) => {
                panic!(
                    "authless self-hosted server without a canonical realm binding must fail closed"
                )
            }
            Err(err) => err,
        };

        assert!(
            err.to_string().contains("self-hosted")
                && err.to_string().contains("no canonical realm binding"),
            "unexpected error: {err}"
        );
    }

    #[cfg(all(feature = "openai", not(target_arch = "wasm32")))]
    #[tokio::test]
    async fn self_hosted_explicit_auth_binding_uses_realm_auth_and_persists_identity() {
        let temp = tempfile::tempdir().unwrap();
        let factory = AgentFactory::new(temp.path().join("sessions")).builtins(false);
        let mut config = config_with_self_hosted_server(SelfHostedServerConfig {
            transport: SelfHostedTransport::OpenAiCompatible,
            base_url: "http://127.0.0.1:11434".to_string(),
            api_style: SelfHostedApiStyle::ChatCompletions,
        });
        let mut realm = RealmConfigSection {
            default_binding: Some("local_binding".to_string()),
            ..Default::default()
        };
        realm.backend.insert(
            "local_backend".to_string(),
            BackendProfileConfig {
                provider: "self_hosted".to_string(),
                backend_kind: "self_hosted".to_string(),
                base_url: Some("http://127.0.0.1:9999/v1".to_string()),
                options: serde_json::Value::Null,
            },
        );
        realm.auth.insert(
            "local_auth".to_string(),
            meerkat_core::AuthProfileConfig {
                provider: "self_hosted".to_string(),
                auth_method: "static_bearer".to_string(),
                source: CredentialSourceSpec::InlineSecret {
                    secret: "realm-token".to_string(),
                },
                constraints: Default::default(),
                metadata_defaults: Default::default(),
            },
        );
        realm.binding.insert(
            "local_binding".to_string(),
            ProviderBindingConfig {
                backend_profile: "local_backend".to_string(),
                auth_profile: "local_auth".to_string(),
                default_model: Some("gemma4:e2b".to_string()),
                policy: Default::default(),
                provider_default: false,
            },
        );
        config.realm.insert("dev".to_string(), realm);
        let auth_binding = AuthBindingRef {
            realm: RealmId::parse("dev").unwrap(),
            binding: BindingId::parse("local_binding").unwrap(),
            profile: None,
            origin: meerkat_core::BindingOrigin::Configured,
        };
        let mut build = AgentBuildConfig::new("gemma-4-e2b");
        build.provider = Some(Provider::SelfHosted);
        build.auth_binding = Some(auth_binding.clone());
        build.override_builtins = ToolCategoryOverride::Disable;

        let agent = factory.build_agent(build, &config).await.unwrap();

        assert_eq!(
            agent.session().session_metadata().unwrap().auth_binding,
            Some(auth_binding),
            "explicit realm auth_binding must remain the durable session identity"
        );
    }

    #[test]
    fn configured_selected_realm_without_provider_config_rejects_env_default_image_binding() {
        let mut config = Config::default();
        config
            .realm
            .insert("default".to_string(), RealmConfigSection::default());

        let err = AgentFactory::resolve_image_binding_for_provider(
            &config,
            Provider::OpenAI,
            Some(&RealmId::parse("default").unwrap()),
        )
        .expect_err("configured selected image lookup must not synthesize env_default");

        assert!(
            err.contains("provider 'openai'")
                && err.contains("selected realm 'default'")
                && err.contains("has no default binding"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn selected_image_binding_uses_provider_binding_over_text_default() {
        let mut config = Config::default();
        let mut selected = inline_realm_section(&[
            ("anthropic", "text-anthropic-key"),
            ("openai", "image-openai-key"),
        ]);
        selected
            .binding
            .get_mut("default_openai")
            .expect("openai binding")
            .policy
            .require_metadata_account = true;
        config.realm.insert("session_a".to_string(), selected);
        config.realm.insert(
            "default".to_string(),
            inline_realm_section(&[("openai", "default-openai-key")]),
        );

        let (realm, binding_id, auth_binding) = AgentFactory::resolve_image_binding_for_provider(
            &config,
            Provider::OpenAI,
            Some(&RealmId::parse("session_a").unwrap()),
        )
        .expect("selected image lookup should pick the selected realm's OpenAI binding");

        assert_eq!(realm.realm_id.as_str(), "session_a");
        assert_eq!(binding_id, "default_openai");
        assert_eq!(auth_binding.realm.as_str(), "session_a");
        assert_eq!(auth_binding.binding.as_str(), "default_openai");
        let (binding, backend, auth) = realm.lookup_auth_binding(&auth_binding).unwrap();
        assert_eq!(backend.provider, Provider::OpenAI);
        assert_eq!(auth.provider, Provider::OpenAI);
        assert!(
            binding.policy.require_metadata_account,
            "image binding policy must stay attached to the provider-specific selected binding"
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn selected_image_binding_auth_policy_failure_keeps_typed_shape() {
        use meerkat_llm_core::provider_runtime::{
            ProviderAuthError, ProviderBindingError, ProviderClientError, ProviderRuntime,
            ProviderRuntimeRegistry, ResolvedConnection, ResolverEnvironment, ValidatedBinding,
        };

        struct PolicyRejectingOpenAiRuntime;

        #[async_trait::async_trait]
        impl ProviderRuntime for PolicyRejectingOpenAiRuntime {
            fn provider_id(&self) -> Provider {
                Provider::OpenAI
            }

            async fn resolve_binding(
                &self,
                binding: &ValidatedBinding,
                _env: &ResolverEnvironment,
            ) -> Result<ResolvedConnection, ProviderAuthError> {
                assert_eq!(binding.auth_binding_ref().realm.as_str(), "session_a");
                assert_eq!(
                    binding.auth_binding_ref().binding.as_str(),
                    "default_openai"
                );
                assert_eq!(binding.provider(), Provider::OpenAI);
                assert!(binding.policy().require_metadata_account);
                Err(ProviderAuthError::Binding(
                    ProviderBindingError::MissingRequiredDefault("metadata_account"),
                ))
            }

            fn build_client(
                &self,
                _connection: ResolvedConnection,
            ) -> Result<Arc<dyn LlmClient>, ProviderClientError> {
                unreachable!("test only resolves the selected image binding")
            }
        }

        let mut config = Config::default();
        let mut selected = inline_realm_section(&[
            ("anthropic", "text-anthropic-key"),
            ("openai", "image-openai-key"),
        ]);
        selected
            .binding
            .get_mut("default_openai")
            .expect("openai binding")
            .policy
            .require_metadata_account = true;
        config.realm.insert("session_a".to_string(), selected);

        let (realm, _binding_id, auth_binding) = AgentFactory::resolve_image_binding_for_provider(
            &config,
            Provider::OpenAI,
            Some(&RealmId::parse("session_a").unwrap()),
        )
        .expect("selected image lookup should resolve the selected OpenAI binding");
        let registry =
            ProviderRuntimeRegistry::empty().with_runtime(Arc::new(PolicyRejectingOpenAiRuntime));

        let err = registry
            .resolve(&realm, &auth_binding, &ResolverEnvironment::testing())
            .await
            .expect_err("auth-policy validation should fail in the provider-runtime domain");

        assert!(matches!(
            err,
            ProviderAuthError::Binding(ProviderBindingError::MissingRequiredDefault(
                "metadata_account"
            ))
        ));
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn selected_image_binding_incompatible_auth_fails_without_default_fallback() {
        use meerkat_llm_core::provider_runtime::{
            ProviderAuthError, ProviderBindingError, ProviderClientError, ProviderRuntime,
            ProviderRuntimeRegistry, ResolvedConnection, ResolverEnvironment, ValidatedBinding,
        };

        struct CountingOpenAiRuntime {
            resolve_calls: Arc<std::sync::atomic::AtomicUsize>,
        }

        #[async_trait::async_trait]
        impl ProviderRuntime for CountingOpenAiRuntime {
            fn provider_id(&self) -> Provider {
                Provider::OpenAI
            }

            async fn resolve_binding(
                &self,
                _binding: &ValidatedBinding,
                _env: &ResolverEnvironment,
            ) -> Result<ResolvedConnection, ProviderAuthError> {
                self.resolve_calls
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Err(ProviderAuthError::SourceResolutionFailed(
                    "catalog should reject before runtime dispatch".into(),
                ))
            }

            fn build_client(
                &self,
                _connection: ResolvedConnection,
            ) -> Result<Arc<dyn LlmClient>, ProviderClientError> {
                unreachable!("test only resolves the selected image binding")
            }
        }

        let mut config = Config::default();
        let mut selected = inline_realm_section(&[
            ("anthropic", "text-anthropic-key"),
            ("openai", "image-openai-key"),
        ]);
        selected
            .auth
            .get_mut("default_openai")
            .expect("openai auth")
            .auth_method = "managed_chatgpt_oauth".to_string();
        config.realm.insert("session_a".to_string(), selected);
        config.realm.insert(
            "default".to_string(),
            inline_realm_section(&[("openai", "default-openai-key")]),
        );

        let (realm, binding_id, auth_binding) = AgentFactory::resolve_image_binding_for_provider(
            &config,
            Provider::OpenAI,
            Some(&RealmId::parse("session_a").unwrap()),
        )
        .expect("selected image lookup should not fall back before validation");
        assert_eq!(realm.realm_id.as_str(), "session_a");
        assert_eq!(binding_id, "default_openai");

        let resolve_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let registry =
            ProviderRuntimeRegistry::empty().with_runtime(Arc::new(CountingOpenAiRuntime {
                resolve_calls: Arc::clone(&resolve_calls),
            }));

        let err = registry
            .resolve(&realm, &auth_binding, &ResolverEnvironment::testing())
            .await
            .expect_err("catalog should reject the selected incompatible image binding");

        assert!(matches!(
            err,
            ProviderAuthError::Binding(ProviderBindingError::UnsupportedCombination { .. })
        ));
        assert_eq!(
            resolve_calls.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "invalid selected binding must not dispatch or fall back to the default realm"
        );
    }

    #[test]
    fn selected_empty_realm_image_binding_rejects_env_default_image_binding() {
        let mut config = Config::default();
        config
            .realm
            .insert("default".to_string(), RealmConfigSection::default());

        let err = AgentFactory::resolve_image_binding_for_provider(
            &config,
            Provider::Gemini,
            Some(&RealmId::parse("default").unwrap()),
        )
        .expect_err("empty selected image realm must not synthesize env_default");

        assert!(
            err.contains("provider 'gemini'")
                && err.contains("selected realm 'default'")
                && err.contains("has no default binding"),
            "unexpected error: {err}"
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn selected_realm_reuses_active_same_provider_image_executor() {
        use meerkat_llm_core::provider_runtime::{
            ProviderAuthError, ProviderClientError, ProviderRuntime, ProviderRuntimeRegistry,
            ResolvedConnection, ResolverEnvironment, StaticLease, ValidatedBinding,
        };
        use meerkat_llm_core::{
            ImageGenerationExecutor, LlmError, ProviderImageGenerationOutput,
            ProviderImageGenerationRequest,
        };

        struct RecordingOpenAiRuntime {
            image_executor_builds: Arc<std::sync::atomic::AtomicUsize>,
        }

        struct UnusedImageExecutor;

        #[async_trait::async_trait]
        impl ImageGenerationExecutor for UnusedImageExecutor {
            async fn execute_image_generation(
                &self,
                _request: ProviderImageGenerationRequest,
            ) -> Result<ProviderImageGenerationOutput, LlmError> {
                Err(LlmError::InvalidRequest {
                    message: "unused test executor".to_string(),
                })
            }
        }

        #[async_trait::async_trait]
        impl ProviderRuntime for RecordingOpenAiRuntime {
            fn provider_id(&self) -> Provider {
                Provider::OpenAI
            }

            async fn resolve_binding(
                &self,
                binding: &ValidatedBinding,
                _env: &ResolverEnvironment,
            ) -> Result<ResolvedConnection, ProviderAuthError> {
                assert_eq!(binding.auth_binding_ref().realm.as_str(), "env_default");
                Ok(ResolvedConnection {
                    provider: Provider::OpenAI,
                    backend: binding.backend(),
                    backend_profile: Arc::clone(binding.backend_profile()),
                    auth_lease: Arc::new(StaticLease::inline_secret(
                        "test-openai-key".to_string(),
                        meerkat_core::AuthMetadata::default(),
                        None,
                        "test",
                    )),
                })
            }

            fn build_client(
                &self,
                _connection: ResolvedConnection,
            ) -> Result<Arc<dyn LlmClient>, ProviderClientError> {
                Ok(Arc::new(meerkat_client::TestClient::default()))
            }

            fn build_image_generation_executor(
                &self,
                _connection: ResolvedConnection,
            ) -> Result<Option<Arc<dyn ImageGenerationExecutor>>, ProviderClientError> {
                self.image_executor_builds
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(Some(Arc::new(UnusedImageExecutor)))
            }
        }

        let temp = tempfile::tempdir().unwrap();
        let image_executor_builds = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let provider_registry =
            ProviderRuntimeRegistry::empty().with_runtime(Arc::new(RecordingOpenAiRuntime {
                image_executor_builds: Arc::clone(&image_executor_builds),
            }));
        let mut factory = AgentFactory::new(temp.path().join("sessions")).builtins(false);
        factory.provider_registry = Arc::new(provider_registry);

        let mut build = AgentBuildConfig::new("gpt-5.4");
        build.provider = Some(Provider::OpenAI);
        build.realm_id = Some(RealmId::parse("default").unwrap());
        build.override_builtins = ToolCategoryOverride::Disable;
        let mut config = Config::default();
        config
            .realm
            .insert("default".to_string(), RealmConfigSection::default());

        factory.build_agent(build, &config).await.unwrap();

        assert_eq!(
            image_executor_builds.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "same-provider image setup should reuse the already-authorized active LLM connection"
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn image_generation_visibility_does_not_enable_general_builtins() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = Arc::new(meerkat_runtime::MeerkatMachine::ephemeral());
        let blob_store: Arc<dyn BlobStore> = Arc::new(meerkat_store::MemoryBlobStore::default());
        let factory = AgentFactory::new(temp.path().join("sessions"))
            .builtins(false)
            .with_image_generation_machine(runtime);

        let mut build = AgentBuildConfig::new("claude-sonnet-4-5");
        build.provider = Some(Provider::Anthropic);
        build.llm_client_override = Some(Arc::new(meerkat_client::TestClient::default()));
        build.override_builtins = ToolCategoryOverride::Disable;
        build.override_image_generation = ToolCategoryOverride::Enable;
        build.blob_store_override = Some(blob_store);

        let agent = factory
            .build_agent(build, &Config::default())
            .await
            .unwrap();
        let visible_names = agent.tool_scope().visible_tool_names().unwrap();

        assert!(
            visible_names.contains("generate_image"),
            "image_generation must own generate_image visibility independently of builtins"
        );
        assert!(
            !visible_names.contains("task_list"),
            "enabling image_generation must not expose general task builtins"
        );
        assert!(
            !visible_names.contains("apply_patch"),
            "enabling image_generation must not expose project mutation builtins"
        );
        assert!(
            !visible_names.contains("browse_skills"),
            "enabling image_generation must not expose skill browsing tools"
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn session_owned_image_executor_resolution_publishes_auth_lease_visibility() {
        use meerkat_llm_core::provider_runtime::{
            ProviderAuthError, ProviderClientError, ProviderRuntime, ProviderRuntimeRegistry,
            ResolvedConnection, ResolverEnvironment, StaticLease, ValidatedBinding,
        };
        use meerkat_llm_core::{
            ImageGenerationExecutor, LlmError, ProviderImageGenerationOutput,
            ProviderImageGenerationRequest,
        };

        struct TextAnthropicRuntime;

        struct PublishingImageOpenAiRuntime {
            expires_at: chrono::DateTime<chrono::Utc>,
            saw_auth_lease_handle: Arc<std::sync::atomic::AtomicBool>,
            image_executor_builds: Arc<std::sync::atomic::AtomicUsize>,
        }

        struct TestImageExecutor;

        #[async_trait::async_trait]
        impl ImageGenerationExecutor for TestImageExecutor {
            async fn execute_image_generation(
                &self,
                _request: ProviderImageGenerationRequest,
            ) -> Result<ProviderImageGenerationOutput, LlmError> {
                Err(LlmError::InvalidRequest {
                    message: "unused test executor".to_string(),
                })
            }
        }

        #[async_trait::async_trait]
        impl ProviderRuntime for TextAnthropicRuntime {
            fn provider_id(&self) -> Provider {
                Provider::Anthropic
            }

            async fn resolve_binding(
                &self,
                binding: &ValidatedBinding,
                _env: &ResolverEnvironment,
            ) -> Result<ResolvedConnection, ProviderAuthError> {
                Ok(ResolvedConnection {
                    provider: Provider::Anthropic,
                    backend: binding.backend(),
                    backend_profile: Arc::clone(binding.backend_profile()),
                    auth_lease: Arc::new(StaticLease::inline_secret(
                        "sk-ant-test".to_string(),
                        meerkat_core::AuthMetadata::default(),
                        None,
                        "test",
                    )),
                })
            }

            fn build_client(
                &self,
                _connection: ResolvedConnection,
            ) -> Result<Arc<dyn LlmClient>, ProviderClientError> {
                Ok(Arc::new(meerkat_client::TestClient::default()))
            }
        }

        #[async_trait::async_trait]
        impl ProviderRuntime for PublishingImageOpenAiRuntime {
            fn provider_id(&self) -> Provider {
                Provider::OpenAI
            }

            async fn resolve_binding(
                &self,
                binding: &ValidatedBinding,
                env: &ResolverEnvironment,
            ) -> Result<ResolvedConnection, ProviderAuthError> {
                assert_eq!(binding.auth_binding_ref().realm.as_str(), "session_a");
                assert_eq!(
                    binding.auth_binding_ref().binding.as_str(),
                    "default_openai"
                );
                self.saw_auth_lease_handle.store(
                    env.auth_lease_handle.is_some(),
                    std::sync::atomic::Ordering::SeqCst,
                );
                Ok(ResolvedConnection {
                    provider: Provider::OpenAI,
                    backend: binding.backend(),
                    backend_profile: Arc::clone(binding.backend_profile()),
                    auth_lease: Arc::new(StaticLease::inline_secret(
                        "sk-image-test".to_string(),
                        meerkat_core::AuthMetadata::default(),
                        Some(self.expires_at),
                        "test",
                    )),
                })
            }

            fn build_client(
                &self,
                _connection: ResolvedConnection,
            ) -> Result<Arc<dyn LlmClient>, ProviderClientError> {
                Ok(Arc::new(meerkat_client::TestClient::default()))
            }

            fn build_image_generation_executor(
                &self,
                _connection: ResolvedConnection,
            ) -> Result<Option<Arc<dyn ImageGenerationExecutor>>, ProviderClientError> {
                self.image_executor_builds
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(Some(Arc::new(TestImageExecutor)))
            }
        }

        let temp = tempfile::tempdir().unwrap();
        let expires_at = chrono::Utc::now() + chrono::Duration::hours(2);
        let saw_auth_lease_handle = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let image_executor_builds = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let provider_registry = ProviderRuntimeRegistry::empty()
            .with_runtime(Arc::new(TextAnthropicRuntime))
            .with_runtime(Arc::new(PublishingImageOpenAiRuntime {
                expires_at,
                saw_auth_lease_handle: Arc::clone(&saw_auth_lease_handle),
                image_executor_builds: Arc::clone(&image_executor_builds),
            }));
        let mut factory = AgentFactory::new(temp.path().join("sessions")).builtins(false);
        factory.provider_registry = Arc::new(provider_registry);

        let mut config = Config::default();
        config.realm.insert(
            "session_a".to_string(),
            inline_realm_section(&[
                ("anthropic", "text-anthropic-key"),
                ("openai", "image-openai-key"),
            ]),
        );
        let session = Session::new();
        let runtime = meerkat_runtime::MeerkatMachine::ephemeral();
        let bindings = runtime
            .prepare_bindings(session.id().clone())
            .await
            .expect("session runtime bindings");
        let mut build = AgentBuildConfig::new("claude-sonnet-4-5");
        build.provider = Some(Provider::Anthropic);
        build.realm_id = Some(RealmId::parse("session_a").unwrap());
        build.resume_session = Some(session);
        build.runtime_build_mode = meerkat_core::RuntimeBuildMode::SessionOwned(bindings.clone());
        build.override_builtins = ToolCategoryOverride::Disable;

        factory.build_agent(build, &config).await.unwrap();

        assert!(
            saw_auth_lease_handle.load(std::sync::atomic::Ordering::SeqCst),
            "session-owned image resolution must receive the AuthMachine authority handle"
        );
        assert_eq!(
            image_executor_builds.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "test should exercise the selected OpenAI image executor path"
        );
        let auth_binding = AuthBindingRef {
            realm: RealmId::parse("session_a").unwrap(),
            binding: BindingId::parse("default_openai").unwrap(),
            profile: None,
            origin: meerkat_core::BindingOrigin::Configured,
        };
        let snapshot =
            bindings
                .auth_lease()
                .snapshot(&meerkat_core::handles::LeaseKey::from_auth_binding(
                    &auth_binding,
                ));
        assert_eq!(
            snapshot.phase,
            Some(meerkat_core::handles::AuthLeasePhase::Valid)
        );
        assert!(snapshot.credential_present);
        assert_eq!(snapshot.expires_at, Some(expires_at.timestamp() as u64));
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn session_owned_llm_resolution_publishes_auth_lease_before_image_executor_failure() {
        use meerkat_llm_core::ImageGenerationExecutor;
        use meerkat_llm_core::provider_runtime::{
            ProviderAuthError, ProviderClientError, ProviderRuntime, ProviderRuntimeRegistry,
            ResolvedConnection, ResolverEnvironment, StaticLease, ValidatedBinding,
        };

        struct FailingImageOpenAiRuntime {
            expires_at: chrono::DateTime<chrono::Utc>,
            saw_auth_lease_handle: Arc<std::sync::atomic::AtomicBool>,
        }

        #[async_trait::async_trait]
        impl ProviderRuntime for FailingImageOpenAiRuntime {
            fn provider_id(&self) -> Provider {
                Provider::OpenAI
            }

            async fn resolve_binding(
                &self,
                binding: &ValidatedBinding,
                env: &ResolverEnvironment,
            ) -> Result<ResolvedConnection, ProviderAuthError> {
                self.saw_auth_lease_handle.store(
                    env.auth_lease_handle.is_some(),
                    std::sync::atomic::Ordering::SeqCst,
                );
                Ok(ResolvedConnection {
                    provider: Provider::OpenAI,
                    backend: binding.backend(),
                    backend_profile: Arc::clone(binding.backend_profile()),
                    auth_lease: Arc::new(StaticLease::inline_secret(
                        "sk-openai-test".to_string(),
                        meerkat_core::AuthMetadata::default(),
                        Some(self.expires_at),
                        "test",
                    )),
                })
            }

            fn build_client(
                &self,
                _connection: ResolvedConnection,
            ) -> Result<Arc<dyn LlmClient>, ProviderClientError> {
                Ok(Arc::new(meerkat_client::TestClient::default()))
            }

            fn build_image_generation_executor(
                &self,
                _connection: ResolvedConnection,
            ) -> Result<Option<Arc<dyn ImageGenerationExecutor>>, ProviderClientError> {
                Err(ProviderClientError::ClientInit(
                    "image executor init failed".to_string(),
                ))
            }
        }

        let temp = tempfile::tempdir().unwrap();
        let expires_at = chrono::Utc::now() + chrono::Duration::hours(2);
        let saw_auth_lease_handle = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let provider_registry =
            ProviderRuntimeRegistry::empty().with_runtime(Arc::new(FailingImageOpenAiRuntime {
                expires_at,
                saw_auth_lease_handle: Arc::clone(&saw_auth_lease_handle),
            }));
        let mut factory = AgentFactory::new(temp.path().join("sessions")).builtins(false);
        factory.provider_registry = Arc::new(provider_registry);

        let mut config = Config::default();
        config.realm.insert(
            "session_a".to_string(),
            inline_realm_section(&[("openai", "text-openai-key")]),
        );
        let session = Session::new();
        let runtime = meerkat_runtime::MeerkatMachine::ephemeral();
        let bindings = runtime
            .prepare_bindings(session.id().clone())
            .await
            .expect("session runtime bindings");
        let mut build = AgentBuildConfig::new("gpt-5.4");
        build.provider = Some(Provider::OpenAI);
        build.realm_id = Some(RealmId::parse("session_a").unwrap());
        build.resume_session = Some(session);
        build.runtime_build_mode = meerkat_core::RuntimeBuildMode::SessionOwned(bindings.clone());
        build.override_builtins = ToolCategoryOverride::Disable;

        let err = match factory.build_agent(build, &config).await {
            Ok(_) => panic!("image executor init should fail after provider resolve"),
            Err(err) => err,
        };
        assert!(
            err.to_string().contains("image executor init failed"),
            "unexpected error: {err}"
        );
        assert!(
            saw_auth_lease_handle.load(std::sync::atomic::Ordering::SeqCst),
            "session-owned LLM resolution must receive the AuthMachine authority handle"
        );
        let auth_binding = AuthBindingRef {
            realm: RealmId::parse("session_a").unwrap(),
            binding: BindingId::parse("default_openai").unwrap(),
            profile: None,
            origin: meerkat_core::BindingOrigin::Configured,
        };
        let snapshot =
            bindings
                .auth_lease()
                .snapshot(&meerkat_core::handles::LeaseKey::from_auth_binding(
                    &auth_binding,
                ));
        assert_eq!(
            snapshot.phase,
            Some(meerkat_core::handles::AuthLeasePhase::Valid)
        );
        assert!(snapshot.credential_present);
        assert_eq!(snapshot.expires_at, Some(expires_at.timestamp() as u64));
    }

    #[cfg(all(feature = "skills", feature = "comms"))]
    #[tokio::test]
    async fn default_skill_runtime_resolves_embedded_multi_agent_comms_skill() {
        let temp = tempfile::tempdir().unwrap();
        let factory = AgentFactory::new(temp.path().join("sessions")).comms(true);
        let runtime = factory
            .build_skill_runtime(&Config::default())
            .await
            .expect("skill runtime build should not fail")
            .expect("skills-enabled config should build a runtime");
        let skill_key = meerkat_core::skills::SkillKey::builtin(
            meerkat_core::skills::SkillName::parse("multi-agent-comms").unwrap(),
        );

        let resolved = runtime
            .resolve_and_render(std::slice::from_ref(&skill_key))
            .await
            .expect("embedded comms skill should resolve");

        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].key, skill_key);
        assert_eq!(resolved[0].name, "multi-agent-comms");
        assert!(resolved[0].rendered_body.contains("Multi-Agent Comms"));
    }

    #[cfg(not(feature = "skills"))]
    #[test]
    fn skills_capability_is_absent_when_facade_skills_feature_is_disabled() {
        let caps = meerkat_capabilities::available_capabilities(&Config::default());

        assert!(
            !caps.contains(&meerkat_capabilities::CapabilityId::Skills),
            "skills capability must reflect composed runtime support, not merely the linked skill substrate"
        );
    }

    #[cfg(feature = "skills")]
    #[tokio::test]
    async fn preloaded_remapped_skill_persists_resolved_canonical_key() {
        let temp = tempfile::tempdir().unwrap();
        let (requested, canonical) = remapped_skill_keys();
        let mut build = AgentBuildConfig::new("claude-sonnet-4-5");
        build.llm_client_override = Some(Arc::new(meerkat_client::TestClient::default()));
        build.override_builtins = ToolCategoryOverride::Disable;
        build.preload_skills = Some(vec![requested.clone()]);
        build.skill_engine_override = Some(remapping_skill_runtime(
            requested.clone(),
            canonical.clone(),
        ));

        let agent = AgentFactory::new(temp.path().join("sessions"))
            .builtins(false)
            .build_agent(build, &Config::default())
            .await
            .unwrap();

        let metadata = agent
            .session()
            .session_metadata()
            .expect("session metadata should be persisted");
        assert_eq!(
            metadata.tooling.active_skills,
            Some(vec![canonical.clone()])
        );
        let Some(meerkat_core::Message::System(message)) = agent.session().messages().first()
        else {
            unreachable!("expected system prompt");
        };
        assert!(message.content.contains(&canonical.to_string()));
        assert!(!message.content.contains(&requested.to_string()));
    }

    #[cfg(feature = "skills")]
    #[tokio::test]
    async fn resumed_remapped_active_skill_is_canonicalized_before_persisting() {
        let temp = tempfile::tempdir().unwrap();
        let (requested, canonical) = remapped_skill_keys();
        let mut resumed = Session::new();
        resumed
            .set_session_metadata(metadata_with_active_skills(Some(vec![
                requested.clone(),
                canonical.clone(),
            ])))
            .expect("resume metadata");

        let mut build = AgentBuildConfig::new("claude-sonnet-4-5");
        build.llm_client_override = Some(Arc::new(meerkat_client::TestClient::default()));
        build.resume_session = Some(resumed);
        build.skill_engine_override = Some(remapping_skill_runtime(requested, canonical.clone()));

        let agent = AgentFactory::new(temp.path().join("sessions"))
            .builtins(false)
            .build_agent(build, &Config::default())
            .await
            .unwrap();

        let metadata = agent
            .session()
            .session_metadata()
            .expect("session metadata should be persisted");
        assert_eq!(metadata.tooling.active_skills, Some(vec![canonical]));
    }

    #[cfg(all(feature = "skills", feature = "comms"))]
    #[test]
    fn per_session_comms_build_keeps_comms_skill_capability() {
        let temp = tempfile::tempdir().unwrap();
        let factory = AgentFactory::new(temp.path().join("sessions"));
        let mut build = AgentBuildConfig::new("gpt-5.4");
        build.comms_name = Some("mob:test/analyst/a-1".to_string());

        let caps = factory.effective_skill_capabilities(&Config::default(), Some(&build));

        assert!(
            caps.iter().any(|cap| cap.as_str() == "comms"),
            "per-session comms identity should expose comms capability to skills"
        );
    }

    #[cfg(feature = "skills")]
    #[test]
    fn build_override_can_expose_schedule_skill_capability() {
        let temp = tempfile::tempdir().unwrap();
        let factory = AgentFactory::new(temp.path().join("sessions"));
        let mut config = Config::default();
        config.tools.schedule_enabled = false;
        let mut build = AgentBuildConfig::new("gpt-5.4");
        build.override_schedule = ToolCategoryOverride::Enable;

        let caps = factory.effective_skill_capabilities(&config, Some(&build));

        assert!(
            caps.iter().any(|cap| cap.as_str() == "schedule"),
            "per-build schedule enable should expose schedule capability to skills"
        );
    }

    #[cfg(all(feature = "comms", not(target_arch = "wasm32")))]
    #[tokio::test]
    async fn session_owned_shared_comms_runtime_is_marked_machine_required() {
        let temp = tempfile::tempdir().unwrap();
        let shared_name = format!("shared-session-owned-{}", meerkat_core::SessionId::new());
        let shared_runtime =
            Arc::new(meerkat_comms::CommsRuntime::inproc_only(&shared_name).unwrap());
        assert!(!shared_runtime.peer_comms_machine_authority_required());
        assert!(shared_runtime.peer_comms_handle().is_none());

        let session = Session::new();
        let runtime = meerkat_runtime::MeerkatMachine::ephemeral();
        let bindings = runtime
            .prepare_bindings(session.id().clone())
            .await
            .expect("session runtime bindings");
        let mut build = AgentBuildConfig::new("gpt-5.4");
        build.llm_client_override = Some(Arc::new(meerkat_client::TestClient::default()));
        build.resume_session = Some(session);
        build.runtime_build_mode = meerkat_core::RuntimeBuildMode::SessionOwned(bindings);
        build.override_builtins = ToolCategoryOverride::Disable;

        let factory = AgentFactory::new(temp.path().join("sessions"))
            .builtins(false)
            .with_comms_runtime(Arc::clone(&shared_runtime));
        assert!(
            shared_runtime.peer_comms_machine_authority_required(),
            "factory attachment must fail closed before a session-owned build can install machine authority"
        );

        let _agent = factory
            .build_agent(build, &Config::default())
            .await
            .expect("session-owned build should succeed");

        assert!(
            shared_runtime.peer_comms_machine_authority_required(),
            "shared runtime must fail closed before any session-owned ingress can use local classifier authority"
        );
        assert!(
            shared_runtime.peer_comms_handle().is_some(),
            "session-owned shared runtime should install the PeerComms machine handle"
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn image_planner_derives_scoped_target_realtime_capability() {
        use meerkat_core::ImageGenerationPlanner as _;

        struct NativeImageProfile;

        impl meerkat_core::ImageGenerationProviderProfile for NativeImageProfile {
            fn canonical_provider(&self) -> meerkat_core::Provider {
                meerkat_core::Provider::Gemini
            }

            fn resolve_execution_plan(
                &self,
                _operation_id: meerkat_core::ImageOperationId,
                model: &meerkat_core::model_profile::catalog::ImageGenerationModelProfile,
                _request: &meerkat_core::GenerateImageRequest,
                capabilities: meerkat_core::ImageGenerationTargetCapabilities,
                max_count: std::num::NonZeroU32,
            ) -> Result<
                meerkat_core::ImageGenerationProviderResolution,
                meerkat_core::ImageOperationDenialReason,
            > {
                Ok(meerkat_core::ImageGenerationProviderResolution {
                    provider_call_model: meerkat_core::lifecycle::run_primitive::ModelId::new(
                        model.model_id,
                    ),
                    execution_plan: meerkat_core::GenerateImageExecutionPlan {
                        provider: meerkat_core::ProviderId::new("gemini"),
                        backend: meerkat_core::ImageGenerationBackendKind::NativeModel,
                        max_count,
                        capabilities,
                        requires_scoped_override: true,
                        provider_plan: serde_json::Value::Null,
                    },
                })
            }
        }

        let planner = CompositeImageGenerationPlanner::new(vec![Arc::new(NativeImageProfile)]);
        let status = meerkat_core::SessionModelRoutingStatus::new(
            meerkat_core::lifecycle::run_primitive::ModelId::new("gpt-realtime-2"),
            None,
            None,
            None,
        );
        let request = meerkat_core::GenerateImageRequest::new(
            meerkat_core::ImageGenerationIntent::Generate {
                prompt: meerkat_core::PromptText::new("draw a cat").unwrap(),
                prompt_source: meerkat_core::PromptSource::ModelDistilled {
                    tool_call_id: meerkat_core::ToolCallId::new("tool-call"),
                },
                reference_images: Vec::new(),
            },
            meerkat_core::ImageGenerationTargetPreference::ProviderDefault {
                provider: meerkat_core::ProviderId::new("gemini"),
            },
            meerkat_core::ImageSizePreference::Square1024,
            meerkat_core::ImageQualityPreference::Auto,
            meerkat_core::ImageFormatPreference::Png,
            std::num::NonZeroU32::MIN,
        )
        .unwrap();

        let plan = planner
            .resolve_image_generation_plan(
                &status,
                serde_json::from_str("\"00000000-0000-0000-0000-000000000000\"").unwrap(),
                &request,
            )
            .unwrap();

        assert_eq!(
            plan.machine_routing_model.as_str(),
            "gemini-3.1-flash-image-preview"
        );
        assert!(
            !plan.machine_routing_realtime_capable,
            "uncatalogued Gemini image targets must not be treated as realtime-capable"
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn image_planner_rejects_uncatalogued_model_before_provider_execution() {
        use meerkat_core::ImageGenerationPlanner as _;

        struct PanickingGeminiProfile;

        impl meerkat_core::ImageGenerationProviderProfile for PanickingGeminiProfile {
            fn canonical_provider(&self) -> meerkat_core::Provider {
                meerkat_core::Provider::Gemini
            }

            fn resolve_execution_plan(
                &self,
                _operation_id: meerkat_core::ImageOperationId,
                _model: &meerkat_core::model_profile::catalog::ImageGenerationModelProfile,
                _request: &meerkat_core::GenerateImageRequest,
                _capabilities: meerkat_core::ImageGenerationTargetCapabilities,
                _max_count: std::num::NonZeroU32,
            ) -> Result<
                meerkat_core::ImageGenerationProviderResolution,
                meerkat_core::ImageOperationDenialReason,
            > {
                panic!("provider profile must not receive uncatalogued image models")
            }
        }

        let planner = CompositeImageGenerationPlanner::new(vec![Arc::new(PanickingGeminiProfile)]);
        let status = meerkat_core::SessionModelRoutingStatus::new(
            meerkat_core::lifecycle::run_primitive::ModelId::new("gpt-5.5"),
            None,
            None,
            None,
        );
        let request = meerkat_core::GenerateImageRequest::new(
            meerkat_core::ImageGenerationIntent::Generate {
                prompt: meerkat_core::PromptText::new("draw a cat").unwrap(),
                prompt_source: meerkat_core::PromptSource::ModelDistilled {
                    tool_call_id: meerkat_core::ToolCallId::new("tool-call"),
                },
                reference_images: Vec::new(),
            },
            meerkat_core::ImageGenerationTargetPreference::Model {
                provider: meerkat_core::ProviderId::new("gemini"),
                model: meerkat_core::lifecycle::run_primitive::ModelId::new(
                    "gemini-unknown-image-preview",
                ),
            },
            meerkat_core::ImageSizePreference::Square1024,
            meerkat_core::ImageQualityPreference::Auto,
            meerkat_core::ImageFormatPreference::Png,
            std::num::NonZeroU32::MIN,
        )
        .unwrap();

        let result = planner.resolve_image_generation_plan(
            &status,
            serde_json::from_str("\"00000000-0000-0000-0000-000000000000\"").unwrap(),
            &request,
        );

        assert!(matches!(
            result,
            Err(meerkat_core::ImageOperationDenialReason::UnsupportedTarget)
        ));
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn image_planner_rejects_unknown_provider_without_alias_fallback() {
        use meerkat_core::ImageGenerationPlanner as _;

        let planner = CompositeImageGenerationPlanner::new(Vec::new());
        let status = meerkat_core::SessionModelRoutingStatus::new(
            meerkat_core::lifecycle::run_primitive::ModelId::new("gpt-5.5"),
            None,
            None,
            None,
        );
        let request = meerkat_core::GenerateImageRequest::new(
            meerkat_core::ImageGenerationIntent::Generate {
                prompt: meerkat_core::PromptText::new("draw a cat").unwrap(),
                prompt_source: meerkat_core::PromptSource::ModelDistilled {
                    tool_call_id: meerkat_core::ToolCallId::new("tool-call"),
                },
                reference_images: Vec::new(),
            },
            meerkat_core::ImageGenerationTargetPreference::ProviderDefault {
                provider: meerkat_core::ProviderId::new("unknown-provider"),
            },
            meerkat_core::ImageSizePreference::Square1024,
            meerkat_core::ImageQualityPreference::Auto,
            meerkat_core::ImageFormatPreference::Png,
            std::num::NonZeroU32::MIN,
        )
        .unwrap();

        let result = planner.resolve_image_generation_plan(
            &status,
            serde_json::from_str("\"00000000-0000-0000-0000-000000000000\"").unwrap(),
            &request,
        );

        assert!(matches!(
            result,
            Err(meerkat_core::ImageOperationDenialReason::UnsupportedTarget)
        ));
    }

    #[cfg(not(target_arch = "wasm32"))]
    struct AutoTargetGeminiProfile;

    #[cfg(not(target_arch = "wasm32"))]
    impl meerkat_core::ImageGenerationProviderProfile for AutoTargetGeminiProfile {
        fn canonical_provider(&self) -> meerkat_core::Provider {
            meerkat_core::Provider::Gemini
        }

        fn resolve_execution_plan(
            &self,
            _operation_id: meerkat_core::ImageOperationId,
            model: &meerkat_core::model_profile::catalog::ImageGenerationModelProfile,
            _request: &meerkat_core::GenerateImageRequest,
            capabilities: meerkat_core::ImageGenerationTargetCapabilities,
            max_count: std::num::NonZeroU32,
        ) -> Result<
            meerkat_core::ImageGenerationProviderResolution,
            meerkat_core::ImageOperationDenialReason,
        > {
            Ok(meerkat_core::ImageGenerationProviderResolution {
                provider_call_model: meerkat_core::lifecycle::run_primitive::ModelId::new(
                    model.model_id,
                ),
                execution_plan: meerkat_core::GenerateImageExecutionPlan {
                    provider: meerkat_core::ProviderId::new("gemini"),
                    backend: meerkat_core::ImageGenerationBackendKind::NativeModel,
                    max_count,
                    capabilities,
                    requires_scoped_override: true,
                    provider_plan: serde_json::Value::Null,
                },
            })
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn auto_target_image_request() -> meerkat_core::GenerateImageRequest {
        meerkat_core::GenerateImageRequest::new(
            meerkat_core::ImageGenerationIntent::Generate {
                prompt: meerkat_core::PromptText::new("draw a cat").unwrap(),
                prompt_source: meerkat_core::PromptSource::ModelDistilled {
                    tool_call_id: meerkat_core::ToolCallId::new("tool-call"),
                },
                reference_images: Vec::new(),
            },
            meerkat_core::ImageGenerationTargetPreference::Auto,
            meerkat_core::ImageSizePreference::Square1024,
            meerkat_core::ImageQualityPreference::Auto,
            meerkat_core::ImageFormatPreference::Png,
            std::num::NonZeroU32::MIN,
        )
        .unwrap()
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn image_planner_auto_uses_configured_image_generation_provider() {
        use meerkat_core::ImageGenerationPlanner as _;

        // The session's typed provider identity is Anthropic — a provider
        // with no image-generation profile — yet the configured default must
        // take precedence and let `Auto` resolve against Gemini.
        let planner = CompositeImageGenerationPlanner::new(vec![Arc::new(AutoTargetGeminiProfile)])
            .with_auto_target_provider(Some(meerkat_core::Provider::Gemini));
        let status = meerkat_core::SessionModelRoutingStatus::new(
            meerkat_core::lifecycle::run_primitive::ModelId::new("claude-opus-4-8"),
            None,
            None,
            None,
        )
        .with_session_provider(Some(meerkat_core::Provider::Anthropic));

        let plan = planner
            .resolve_image_generation_plan(
                &status,
                serde_json::from_str("\"00000000-0000-0000-0000-000000000000\"").unwrap(),
                &auto_target_image_request(),
            )
            .expect("configured image_generation_provider must satisfy Auto targets");
        assert_eq!(
            plan.provider_model.as_str(),
            "gemini-3.1-flash-image-preview"
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn image_planner_auto_without_configured_provider_fails_closed_for_anthropic() {
        use meerkat_core::ImageGenerationPlanner as _;

        // Without a configured default, `Auto` resolves via the session's
        // typed provider identity; Anthropic has no image profile, so the
        // typed denial is preserved (no silent fallback).
        let planner = CompositeImageGenerationPlanner::new(vec![Arc::new(AutoTargetGeminiProfile)]);
        let status = meerkat_core::SessionModelRoutingStatus::new(
            meerkat_core::lifecycle::run_primitive::ModelId::new("claude-opus-4-8"),
            None,
            None,
            None,
        )
        .with_session_provider(Some(meerkat_core::Provider::Anthropic));

        let result = planner.resolve_image_generation_plan(
            &status,
            serde_json::from_str("\"00000000-0000-0000-0000-000000000000\"").unwrap(),
            &auto_target_image_request(),
        );
        assert!(matches!(
            result,
            Err(meerkat_core::ImageOperationDenialReason::UnsupportedTarget)
        ));
    }

    /// Regression for the dogma row "Image auto-routing loses session
    /// provider identity": a session on a `ModelRegistry`-owned custom model
    /// (unknown to the built-in catalog, so `ModelCatalog::infer_provider`
    /// returns `None`) must auto-plan against the session provider's
    /// registered image default WITHOUT any separate image-provider config.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn image_planner_auto_follows_session_provider_for_custom_models() {
        use meerkat_core::ImageGenerationPlanner as _;

        let planner = CompositeImageGenerationPlanner::new(vec![Arc::new(AutoTargetGeminiProfile)]);
        let status = meerkat_core::SessionModelRoutingStatus::new(
            meerkat_core::lifecycle::run_primitive::ModelId::new("my-custom-gemini"),
            None,
            None,
            None,
        )
        .with_session_provider(Some(meerkat_core::Provider::Gemini));

        let plan = planner
            .resolve_image_generation_plan(
                &status,
                serde_json::from_str("\"00000000-0000-0000-0000-000000000000\"").unwrap(),
                &auto_target_image_request(),
            )
            .expect("Auto must resolve through the session's typed provider identity");
        assert_eq!(
            plan.provider_model.as_str(),
            "gemini-3.1-flash-image-preview",
            "the session provider's registered image default must be selected"
        );
    }

    /// After a live LLM identity hot-swap, the routing status carries the
    /// swapped provider (recommitted by the machine), and `Auto` planning
    /// follows it — not the construction-time identity.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn image_planner_auto_follows_swapped_session_provider() {
        use meerkat_core::ImageGenerationPlanner as _;

        let planner = CompositeImageGenerationPlanner::new(vec![Arc::new(AutoTargetGeminiProfile)]);
        let operation_id: meerkat_core::ImageOperationId =
            serde_json::from_str("\"00000000-0000-0000-0000-000000000000\"").unwrap();
        let status_for = |provider: meerkat_core::Provider| {
            meerkat_core::SessionModelRoutingStatus::new(
                meerkat_core::lifecycle::run_primitive::ModelId::new("my-custom-model"),
                None,
                None,
                None,
            )
            .with_session_provider(Some(provider))
        };

        // Pre-swap identity: a provider with no image profile is denied.
        let result = planner.resolve_image_generation_plan(
            &status_for(meerkat_core::Provider::Anthropic),
            operation_id,
            &auto_target_image_request(),
        );
        assert!(matches!(
            result,
            Err(meerkat_core::ImageOperationDenialReason::UnsupportedTarget)
        ));

        // Post-swap identity: the same planner resolves against the swapped
        // provider's registered image default.
        let plan = planner
            .resolve_image_generation_plan(
                &status_for(meerkat_core::Provider::Gemini),
                operation_id,
                &auto_target_image_request(),
            )
            .expect("Auto must follow the swapped session provider");
        assert_eq!(
            plan.provider_model.as_str(),
            "gemini-3.1-flash-image-preview"
        );
    }

    /// One condition, one path: the planner must NOT silently re-derive a
    /// provider from the effective model string through the built-in catalog.
    /// Even with a catalogued Gemini text model, an unhydrated session
    /// provider plus no configured image provider is a typed denial.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn image_planner_auto_does_not_rederive_provider_from_model_string() {
        use meerkat_core::ImageGenerationPlanner as _;

        let planner = CompositeImageGenerationPlanner::new(vec![Arc::new(AutoTargetGeminiProfile)]);
        // "gemini-3.5-flash" IS in the built-in catalog; the old
        // `ModelCatalog::infer_provider` path would have resolved Gemini here.
        let status = meerkat_core::SessionModelRoutingStatus::new(
            meerkat_core::lifecycle::run_primitive::ModelId::new("gemini-3.5-flash"),
            None,
            None,
            None,
        );

        let result = planner.resolve_image_generation_plan(
            &status,
            serde_json::from_str("\"00000000-0000-0000-0000-000000000000\"").unwrap(),
            &auto_target_image_request(),
        );
        assert!(
            matches!(
                result,
                Err(meerkat_core::ImageOperationDenialReason::UnsupportedTarget)
            ),
            "missing session identity must fail closed, not fall back to model-string inference"
        );
    }

    /// The non-scoped realtime capability check pairs the session's typed
    /// provider with the effective model — `(session_provider, model)` is the
    /// same pair the LLM client actually runs with.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn image_planner_unscoped_realtime_capability_follows_session_provider() {
        use meerkat_core::ImageGenerationPlanner as _;

        struct HostedOpenAiProfile;

        impl meerkat_core::ImageGenerationProviderProfile for HostedOpenAiProfile {
            fn canonical_provider(&self) -> meerkat_core::Provider {
                meerkat_core::Provider::OpenAI
            }

            fn resolve_execution_plan(
                &self,
                _operation_id: meerkat_core::ImageOperationId,
                model: &meerkat_core::model_profile::catalog::ImageGenerationModelProfile,
                _request: &meerkat_core::GenerateImageRequest,
                capabilities: meerkat_core::ImageGenerationTargetCapabilities,
                max_count: std::num::NonZeroU32,
            ) -> Result<
                meerkat_core::ImageGenerationProviderResolution,
                meerkat_core::ImageOperationDenialReason,
            > {
                Ok(meerkat_core::ImageGenerationProviderResolution {
                    provider_call_model: meerkat_core::lifecycle::run_primitive::ModelId::new(
                        model.model_id,
                    ),
                    execution_plan: meerkat_core::GenerateImageExecutionPlan {
                        provider: meerkat_core::ProviderId::new("openai"),
                        backend: meerkat_core::ImageGenerationBackendKind::HostedTool,
                        max_count,
                        capabilities,
                        requires_scoped_override: false,
                        provider_plan: serde_json::Value::Null,
                    },
                })
            }
        }

        let planner = CompositeImageGenerationPlanner::new(vec![Arc::new(HostedOpenAiProfile)]);
        let operation_id: meerkat_core::ImageOperationId =
            serde_json::from_str("\"00000000-0000-0000-0000-000000000000\"").unwrap();
        let request = meerkat_core::GenerateImageRequest::new(
            meerkat_core::ImageGenerationIntent::Generate {
                prompt: meerkat_core::PromptText::new("draw a cat").unwrap(),
                prompt_source: meerkat_core::PromptSource::ModelDistilled {
                    tool_call_id: meerkat_core::ToolCallId::new("tool-call"),
                },
                reference_images: Vec::new(),
            },
            meerkat_core::ImageGenerationTargetPreference::ProviderDefault {
                provider: meerkat_core::ProviderId::new("openai"),
            },
            meerkat_core::ImageSizePreference::Square1024,
            meerkat_core::ImageQualityPreference::Auto,
            meerkat_core::ImageFormatPreference::Png,
            std::num::NonZeroU32::MIN,
        )
        .unwrap();

        let base_status = meerkat_core::SessionModelRoutingStatus::new(
            meerkat_core::lifecycle::run_primitive::ModelId::new("gpt-realtime-2"),
            None,
            None,
            None,
        );

        let with_identity = planner
            .resolve_image_generation_plan(
                &base_status
                    .clone()
                    .with_session_provider(Some(meerkat_core::Provider::OpenAI)),
                operation_id,
                &request,
            )
            .expect("hosted plan with hydrated identity");
        assert!(
            with_identity.machine_routing_realtime_capable,
            "(openai, gpt-realtime-2) is realtime-capable in the catalog"
        );

        let without_identity = planner
            .resolve_image_generation_plan(&base_status, operation_id, &request)
            .expect("hosted plan without hydrated identity");
        assert!(
            !without_identity.machine_routing_realtime_capable,
            "no hydrated session provider must mean no realtime claim — never \
             a provider re-derived from the model string"
        );
    }

    #[test]
    fn per_build_auto_compact_threshold_override_wins_over_config_and_scaling() {
        let mut config = Config::default();
        config.compaction.auto_compact_threshold = 42_000;
        config.compaction.auto_compact_threshold_explicit = true;
        let registry = config
            .model_registry(meerkat_models::canonical())
            .expect("registry");

        let compaction = model_aware_compaction_config(
            &config,
            &registry,
            Provider::OpenAI,
            "gpt-5.5",
            std::num::NonZeroU64::new(9_000),
        );

        assert_eq!(
            compaction.auto_compact_threshold, 9_000,
            "per-build override must beat the explicit config knob and model-aware scaling"
        );
    }

    #[test]
    fn build_options_roundtrip_custom_model_and_image_provider_and_threshold() {
        let mut build = AgentBuildConfig::new("claude-internal-preview");
        build.custom_models.insert(
            "claude-internal-preview".to_string(),
            meerkat_core::config::CustomModelConfig {
                provider: Provider::Anthropic,
                display_name: None,
                context_window: Some(400_000),
                max_output_tokens: None,
                vision: None,
                web_search: None,
                call_timeout_secs: None,
            },
        );
        build.image_generation_provider = Some(Provider::Gemini);
        build.auto_compact_threshold_override = std::num::NonZeroU64::new(50_000);

        let options = build.to_session_build_options();
        let mut rebuilt = AgentBuildConfig::new("claude-internal-preview");
        rebuilt.apply_session_build_options(&options);

        assert_eq!(
            rebuilt
                .custom_models
                .get("claude-internal-preview")
                .map(|model| model.provider),
            Some(Provider::Anthropic),
            "custom models must survive deferred-session option materialization"
        );
        assert_eq!(rebuilt.image_generation_provider, Some(Provider::Gemini));
        assert_eq!(
            rebuilt.auto_compact_threshold_override,
            std::num::NonZeroU64::new(50_000)
        );
    }

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
            },
        );
        config.self_hosted.models.insert(
            "gemma-4-e2b".to_string(),
            SelfHostedModelConfig {
                server: "local".to_string(),
                remote_model: "gemma4:e2b".to_string(),
                display_name: "Gemma 4 E2B".into(),
                family: "gemma-4".to_string(),
                tier: meerkat_core::model_profile::catalog::ModelTier::Supported,
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
                auth_binding: None,
                mob_member_binding: None,
            })
            .expect("resume metadata");

        let mut build = AgentBuildConfig::new("gemma-4-e2b");
        build.resume_session = Some(resumed);
        let _ = AgentFactory::apply_resumed_session_metadata(&mut build).expect("resume metadata");

        let registry = factory.model_registry(&config).expect("registry");
        let (provider, server_id) = factory
            .resolve_provider_from_registry(&registry, &build)
            .expect("resolved provider");

        assert_eq!(provider, Provider::SelfHosted);
        assert_eq!(server_id.as_deref(), Some("other"));
    }

    #[test]
    fn resumed_session_without_metadata_fails_closed_when_not_precreated() {
        let mut resumed = Session::new();
        resumed.push(meerkat_core::Message::User(
            meerkat_core::types::UserMessage::text("existing turn"),
        ));

        let mut build = AgentBuildConfig::new("gpt-5.4");
        build.resume_session = Some(resumed);

        let err = AgentFactory::apply_resumed_session_metadata(&mut build)
            .expect_err("resumed sessions without durable metadata must fail closed");

        assert!(
            matches!(&err, BuildAgentError::Config(message) if message.contains("missing durable session metadata")),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn empty_session_without_metadata_requires_machine_precreated_bindings() {
        let mut build = AgentBuildConfig::new("gpt-5.4");
        build.resume_session = Some(Session::new());

        let err = AgentFactory::apply_resumed_session_metadata(&mut build).expect_err(
            "empty sessions without generated runtime bindings must not synthesize metadata",
        );

        assert!(
            matches!(&err, BuildAgentError::Config(message) if message.contains("missing durable session metadata")),
            "unexpected error: {err}"
        );
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

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn factory_exposes_blob_tools_when_blob_store_is_wired() {
        let temp = tempfile::tempdir().unwrap();
        let project_root = temp.path().join("workspace");
        tokio::fs::create_dir_all(&project_root).await.unwrap();
        tokio::fs::write(
            project_root.join("source.png"),
            [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A],
        )
        .await
        .unwrap();
        let blob_store = Arc::new(TestBlobStore::default());
        let factory = AgentFactory::new(temp.path().join("sessions")).project_root(&project_root);
        let ops_lifecycle: Arc<dyn OpsLifecycleRegistry> =
            Arc::new(RuntimeOpsLifecycleRegistry::new());

        let (dispatcher, usage) = factory
            .build_tool_dispatcher_for_agent_with_overrides(
                &Config::default(),
                None,
                true,
                false,
                None,
                None,
                SessionId::new().to_string(),
                ops_lifecycle,
                None,
                None,
                None,
                Some(blob_store.clone()),
                ToolCategoryOverride::Inherit,
                None,
                ToolCategoryOverride::Inherit,
            )
            .await
            .expect("dispatcher should build with blob store");

        for expected in ["blob_save_file", "blob_load_file", "blob_inspect"] {
            assert!(
                dispatcher.tools().iter().any(|tool| tool.name == expected),
                "{expected} should be visible through factory-built builtins"
            );
            assert!(
                usage.contains(expected),
                "{expected} should appear in usage instructions"
            );
        }

        let call_json =
            serde_json::value::RawValue::from_string(r#"{"path":"source.png"}"#.to_string())
                .unwrap();
        let call = ToolCallView {
            id: "blob-load",
            name: "blob_load_file",
            args: &call_json,
        };
        let outcome = dispatcher
            .dispatch(call)
            .await
            .expect("blob_load_file dispatch should succeed");
        let payload: serde_json::Value =
            serde_json::from_str(&outcome.result.text_content()).unwrap();
        let blob_id = BlobId::new(payload["blob_id"].as_str().unwrap());
        assert_eq!(
            blob_store.get(&blob_id).await.unwrap().media_type,
            "image/png"
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn factory_does_not_expose_blob_tools_without_blob_store() {
        let temp = tempfile::tempdir().unwrap();
        let project_root = temp.path().join("workspace");
        tokio::fs::create_dir_all(&project_root).await.unwrap();
        let factory = AgentFactory::new(temp.path().join("sessions")).project_root(&project_root);
        let ops_lifecycle: Arc<dyn OpsLifecycleRegistry> =
            Arc::new(RuntimeOpsLifecycleRegistry::new());

        let (dispatcher, usage) = factory
            .build_tool_dispatcher_for_agent_with_overrides(
                &Config::default(),
                None,
                true,
                false,
                None,
                None,
                SessionId::new().to_string(),
                ops_lifecycle,
                None,
                None,
                None,
                None,
                ToolCategoryOverride::Inherit,
                None,
                ToolCategoryOverride::Inherit,
            )
            .await
            .expect("dispatcher should build without blob store");

        assert!(
            dispatcher
                .tools()
                .iter()
                .all(|tool| !tool.name.starts_with("blob_")),
            "blob tools should be absent without a session blob store"
        );
        assert!(
            !usage.contains("blob_save_file"),
            "blob tool usage should be absent without a session blob store"
        );
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
        image_generation_machine: Option<
            Arc<dyn meerkat_tools::builtin::image_generation::ImageGenerationMachine>,
        >,
        image_generation_executor: Option<Arc<dyn meerkat_llm_core::ImageGenerationExecutor>>,
        image_generation_planner: Option<Arc<dyn meerkat_core::ImageGenerationPlanner>>,
        image_generation_blob_store: Option<Arc<dyn BlobStore>>,
        image_generation_visibility: ToolCategoryOverride,
        web_search_executor: Option<Arc<dyn meerkat_llm_core::WebSearchExecutor>>,
        web_search_visibility: ToolCategoryOverride,
    ) -> Result<(Arc<dyn AgentToolDispatcher>, String), BuildAgentError> {
        let compose_image_generation =
            image_generation_visibility.resolve(false) && image_generation_machine.is_some();
        if !effective_builtins && !compose_image_generation {
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

        // Create builtin tool config. When a non-builtin capability such as
        // image generation needs the composite dispatcher, keep the general
        // builtin namespace closed and let the capability registration add its
        // own tool(s). This keeps profile category intent one-owner: enabling
        // `image_generation` must not also expose task/patch/skill utilities.
        let builtin_config = if effective_builtins {
            if effective_shell {
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
            }
        } else {
            BuiltinToolConfig {
                policy: ToolPolicyLayer::new().with_mode(ToolMode::AllowList),
                ..Default::default()
            }
        };
        let skill_engine = effective_builtins.then_some(skill_engine).flatten();

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
                image_generation_machine,
                image_generation_executor,
                image_generation_planner,
                image_generation_blob_store,
                image_generation_visibility,
                web_search_executor,
                web_search_visibility,
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
        AgentToolDispatcher, Config, Message, ToolCatalogCapabilities, ToolCatalogEntry,
        ToolCategoryOverride,
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
                            meerkat_core::ToolProvenance {
                                kind: meerkat_core::ToolSourceKind::Callback,
                                source_id: "registered".into(),
                            },
                        )
                    } else {
                        ToolCatalogEntry::session_inline(Arc::clone(tool), true)
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
        fn project_replay_messages(
            &self,
            messages: &[meerkat_core::Message],
        ) -> Result<Vec<meerkat_core::Message>, meerkat_client::LlmError> {
            Ok(messages.to_vec())
        }

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

        fn provider(&self) -> meerkat_core::Provider {
            meerkat_core::Provider::Other
        }

        async fn health_check(&self) -> Result<(), LlmError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn memory_realm_backend_suppresses_file_backed_semantic_memory() {
        let temp = tempfile::tempdir().unwrap();
        let store_path = temp.path().join("sessions");
        let factory = AgentFactory::new(store_path.clone())
            .builtins(false)
            .memory(true);
        let mut build_config = AgentBuildConfig::new("claude-sonnet-4-5");
        build_config.llm_client_override = Some(Arc::new(PromptTestClient));
        build_config.override_memory = ToolCategoryOverride::Enable;
        build_config.backend = Some(meerkat_core::RecoveryBackendKind::Memory);

        let agent = factory
            .build_agent(build_config, &Config::default())
            .await
            .unwrap();

        assert!(
            !store_path.join("memory").exists(),
            "memory realm backend must not open the file-backed semantic memory store"
        );
        assert!(
            !agent
                .tool_scope()
                .visible_tool_names()
                .unwrap()
                .contains("memory_search"),
            "memory_search must not be exposed when its only available store would be file-backed"
        );
        assert_eq!(
            agent.session().session_metadata().unwrap().tooling.memory,
            ToolCategoryOverride::Disable
        );
    }

    fn tools(names: &[&str]) -> Arc<[Arc<ToolDef>]> {
        names
            .iter()
            .map(|name| {
                Arc::new(ToolDef {
                    name: (*name).into(),
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
            name: "secret_lookup".into(),
            description: "Look up a secret value".to_string(),
            input_schema: serde_json::json!({"type":"object"}),
            provenance: None,
        });
        let secret_audit = Arc::new(ToolDef {
            name: "secret_audit".into(),
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
            name: "visible".into(),
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
            name: "secret_lookup".into(),
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

#[cfg(all(test, feature = "comms"))]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod session_comms_override_tests {
    use super::*;

    fn scratch_runtime(name: &str) -> Arc<meerkat_comms::CommsRuntime> {
        Arc::new(
            meerkat_comms::CommsRuntime::inproc_only_with_keypair_and_silent_intents(
                name,
                None,
                meerkat_comms::Keypair::generate(),
                Arc::new(std::collections::HashSet::new()),
            )
            .expect("scratch runtime"),
        )
    }

    // T22 — foreign payload: typed reject, never a silent fallback.
    #[test]
    fn foreign_override_payload_is_a_typed_build_error() {
        let payload: Arc<dyn std::any::Any + Send + Sync> = Arc::new(42u32);
        let Err(err) = resolve_session_comms_runtime_override(Some(payload), Some("mob-1/w/a"))
        else {
            panic!("foreign payload must fail closed");
        };
        assert!(matches!(err, BuildAgentError::Config(_)));
    }

    // T22 — comms-name mismatch: typed reject.
    #[test]
    fn override_comms_name_mismatch_is_a_typed_build_error() {
        let runtime = scratch_runtime("mob-1/worker/w-1");
        let payload = crate::encode_session_comms_runtime_override_for_service(runtime);
        let Err(err) =
            resolve_session_comms_runtime_override(Some(payload), Some("mob-1/worker/OTHER"))
        else {
            panic!("name mismatch must fail closed");
        };
        assert!(matches!(err, BuildAgentError::Config(_)));
    }

    // T22 — matching override resolves to the SAME runtime (no second
    // construction; pointer identity is the proof).
    #[test]
    fn matching_override_resolves_to_the_injected_runtime() {
        let runtime = scratch_runtime("mob-1/worker/w-2");
        let payload =
            crate::encode_session_comms_runtime_override_for_service(Arc::clone(&runtime));
        let resolved =
            resolve_session_comms_runtime_override(Some(payload), Some("mob-1/worker/w-2"))
                .expect("resolves")
                .expect("present");
        assert!(Arc::ptr_eq(&resolved, &runtime));
    }

    #[test]
    fn absent_override_resolves_to_none() {
        assert!(
            resolve_session_comms_runtime_override(None, None)
                .expect("resolves")
                .is_none()
        );
    }
}

#[cfg(test)]
mod host_prompt_sections_tests {
    use super::*;

    // T23 — the A1 switch: SpecPinned suppresses the skill-inventory
    // section; Full (default) keeps it.
    #[test]
    fn spec_pinned_suppresses_the_skill_inventory_section() {
        assert!(skill_inventory_prompt_section_enabled(
            meerkat_core::service::HostPromptSections::Full
        ));
        assert!(!skill_inventory_prompt_section_enabled(
            meerkat_core::service::HostPromptSections::SpecPinned
        ));
        assert_eq!(
            meerkat_core::service::HostPromptSections::default(),
            meerkat_core::service::HostPromptSections::Full,
            "local builds default to Full"
        );
    }
}
