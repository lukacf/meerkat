//! SessionService trait — canonical lifecycle abstraction.
//!
//! All surfaces (CLI, REST, MCP Server, JSON-RPC) route through `SessionService`.
//! Implementations may be ephemeral (in-memory only) or persistent (backed by a store).

pub mod transport;

use crate::event::AgentEvent;
use crate::event::EventEnvelope;
use crate::lifecycle::run_primitive::RuntimeTurnMetadata;
use crate::session::{PendingSystemContextAppend, SystemContextStageError};
use crate::time_compat::SystemTime;
#[cfg(target_arch = "wasm32")]
use crate::tokio;
use crate::types::{
    ContentInput, HandlingMode, Message, RenderMetadata, RunResult, SessionId, ToolDef, Usage,
};
use crate::{
    AgentToolDispatcher, BudgetLimits, HookRunOverrides, OutputSchema, PeerMeta, Provider, Session,
    SessionLlmIdentity, ToolCategoryOverride,
};
use crate::{EventStream, StreamError};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::sync::Arc;
use tokio::sync::mpsc;

pub use crate::session::{TranscriptEditError, TranscriptReplacement};

/// Controls whether `create_session()` should execute an initial turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InitialTurnPolicy {
    /// Run the initial turn immediately as part of session creation.
    RunImmediately,
    /// Register the session and return without running an initial turn.
    ///
    /// `CreateSessionRequest::deferred_prompt_policy` determines whether the
    /// create-time prompt is discarded or staged for the first later turn.
    Defer,
}

/// How a deferred create request treats its create-time prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum DeferredPromptPolicy {
    /// Register the session only; the caller will supply the first runtime input separately.
    #[default]
    Discard,
    /// Persist the create-time prompt and merge it into the first later turn.
    Stage,
}

/// Errors returned by `SessionService` methods.
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    /// The requested session does not exist.
    #[error("session not found: {id}")]
    NotFound { id: SessionId },

    /// A turn is already in progress on this session.
    #[error("session is busy: {id}")]
    Busy { id: SessionId },

    /// The operation requires persistence but the `session-store` feature is disabled.
    #[error("session persistence is disabled")]
    PersistenceDisabled,

    /// The operation requires compaction but the `session-compaction` feature is disabled.
    #[error("session compaction is disabled")]
    CompactionDisabled,

    /// No turn is currently running on this session.
    #[error("no turn running on session: {id}")]
    NotRunning { id: SessionId },

    /// A session store operation failed.
    #[error("store error: {0}")]
    Store(#[source] Box<dyn std::error::Error + Send + Sync>),

    /// An agent-level error occurred during execution.
    #[error("agent error: {0}")]
    Agent(#[from] crate::error::AgentError),

    /// The operation failed with structured error data for protocol surfaces.
    #[error("{message}")]
    FailedWithData {
        message: String,
        data: serde_json::Value,
    },

    /// The requested operation is not supported by this session service.
    #[error("unsupported: {0}")]
    Unsupported(String),
}

impl SessionError {
    /// Return a stable error code string for wire formats.
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotFound { .. } => "SESSION_NOT_FOUND",
            Self::Busy { .. } => "SESSION_BUSY",
            Self::PersistenceDisabled => "SESSION_PERSISTENCE_DISABLED",
            Self::CompactionDisabled => "SESSION_COMPACTION_DISABLED",
            Self::NotRunning { .. } => "SESSION_NOT_RUNNING",
            Self::Store(_) => "SESSION_STORE_ERROR",
            Self::Unsupported(_) => "SESSION_UNSUPPORTED",
            Self::Agent(_) => "AGENT_ERROR",
            Self::FailedWithData { .. } => "SESSION_ERROR",
        }
    }

    pub fn structured_data(&self) -> Option<serde_json::Value> {
        match self {
            Self::FailedWithData { data, .. } => Some(data.clone()),
            _ => None,
        }
    }
}

/// Errors returned by session control-plane mutation methods.
#[derive(Debug, thiserror::Error)]
pub enum SessionControlError {
    /// A lifecycle/session-store error occurred while handling the control request.
    #[error(transparent)]
    Session(#[from] SessionError),

    /// The control request was malformed.
    #[error("invalid system-context request: {message}")]
    InvalidRequest { message: String },

    /// The idempotency key was replayed with different request content.
    #[error(
        "system-context idempotency conflict on session {id}: key '{key}' already maps to different content"
    )]
    Conflict { id: SessionId, key: String },
}

impl SessionControlError {
    /// Return a stable error code string for wire formats.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Session(err) => err.code(),
            Self::InvalidRequest { .. } => "INVALID_PARAMS",
            Self::Conflict { .. } => "SESSION_SYSTEM_CONTEXT_CONFLICT",
        }
    }
}

impl SystemContextStageError {
    /// Convert a stage-time state conflict into a surface-level control error.
    pub fn into_control_error(self, id: &SessionId) -> SessionControlError {
        match self {
            Self::InvalidRequest(message) => SessionControlError::InvalidRequest { message },
            Self::Conflict { key, .. } => SessionControlError::Conflict {
                id: id.clone(),
                key,
            },
        }
    }
}

/// Request to create a new session and run the first turn.
#[derive(Debug)]
pub struct CreateSessionRequest {
    /// Model name (e.g. "claude-opus-4-6").
    pub model: String,
    /// Initial user prompt (text or multimodal).
    pub prompt: ContentInput,
    /// Optional normalized rendering metadata for the initial prompt.
    pub render_metadata: Option<RenderMetadata>,
    /// Optional system prompt override.
    pub system_prompt: Option<String>,
    /// Max tokens per LLM turn.
    pub max_tokens: Option<u32>,
    /// Channel for streaming events during the turn.
    pub event_tx: Option<mpsc::Sender<EventEnvelope<AgentEvent>>>,
    /// Canonical SkillKeys to resolve and inject for the first turn.
    pub skill_references: Option<Vec<crate::skills::SkillKey>>,
    /// Initial turn behavior for this session creation call.
    pub initial_turn: InitialTurnPolicy,
    /// How to treat `prompt` when `initial_turn == Defer`.
    pub deferred_prompt_policy: DeferredPromptPolicy,
    /// Optional extended build options for factory-backed builders.
    pub build: Option<SessionBuildOptions>,
    /// Optional key-value labels attached at session creation.
    pub labels: Option<BTreeMap<String, String>>,
}

impl CreateSessionRequest {
    /// Compose the existing service-level labels and build app-context into the
    /// shared surface metadata contract.
    #[must_use]
    pub fn surface_metadata(&self) -> crate::SurfaceMetadata {
        crate::SurfaceMetadata::from_optional_parts(
            self.labels.clone(),
            self.build
                .as_ref()
                .and_then(|build| build.app_context.clone()),
        )
    }
}

/// Optional build-time options used by factory-backed session builders.
#[derive(Clone)]
pub struct SessionBuildOptions {
    pub provider: Option<Provider>,
    pub self_hosted_server_id: Option<String>,
    pub output_schema: Option<OutputSchema>,
    pub structured_output_retries: u32,
    pub hooks_override: HookRunOverrides,
    pub comms_name: Option<String>,
    pub peer_meta: Option<PeerMeta>,
    pub resume_session: Option<Session>,
    pub budget_limits: Option<BudgetLimits>,
    pub provider_params: Option<serde_json::Value>,
    pub external_tools: Option<Arc<dyn AgentToolDispatcher>>,
    /// Serializable tool definitions used to reconstruct recoverable
    /// surface-owned dispatchers during session resume/rebuild.
    pub recoverable_tool_defs: Option<Vec<crate::ToolDef>>,
    /// Blob store used to externalize durable image content and hydrate refs
    /// back to bytes at execution seams.
    pub blob_store_override: Option<Arc<dyn crate::BlobStore>>,
    /// Opaque transport for an optional per-request LLM override.
    ///
    /// Factory builders may downcast this to their concrete client trait.
    pub llm_client_override: Option<Arc<dyn std::any::Any + Send + Sync>>,
    /// Optional wrapper applied to the final agent-facing LLM client.
    ///
    /// This is intentionally provider-agnostic and runs after raw clients are
    /// adapted into [`AgentLlmClient`].
    pub agent_llm_client_decorator: Option<crate::AgentLlmClientDecorator>,
    // NOTE: ops_lifecycle_override was removed in Phase 3.
    // Use runtime_build_mode instead.
    pub override_builtins: ToolCategoryOverride,
    pub override_shell: ToolCategoryOverride,
    pub override_memory: ToolCategoryOverride,
    /// Per-build override for the factory-level scheduler capability.
    pub override_schedule: ToolCategoryOverride,
    pub override_mob: ToolCategoryOverride,
    /// Per-build override for assistant image generation visibility.
    ///
    /// `Inherit` means "visible when the session-owned image-generation
    /// substrate is available"; `Disable` hides the tool even when wired.
    pub override_image_generation: ToolCategoryOverride,
    /// Per-build override for Meerkat-owned fallback web search visibility.
    ///
    /// `Inherit` keeps the fallback hidden. `Enable` explicitly exposes the
    /// fallback when the active model lacks native provider web search.
    pub override_web_search: ToolCategoryOverride,
    /// Agent-facing scheduler tools supplied by the embedding surface.
    ///
    /// Scheduler remains surface-owned. This dispatcher only controls
    /// tool visibility/composition for the built agent.
    pub schedule_tools: Option<Arc<dyn AgentToolDispatcher>>,
    pub preload_skills: Option<Vec<crate::skills::SkillKey>>,
    pub realm_id: Option<String>,
    pub instance_id: Option<String>,
    pub backend: Option<String>,
    pub config_generation: Option<u64>,
    /// Realm-scoped auth binding (Phase 3 provider-auth redesign).
    /// Flows into `AgentBuildConfig.auth_binding` via `FactoryAgentBuilder`.
    pub auth_binding: Option<crate::AuthBindingRef>,
    /// Whether this session runs as a keep-alive (long-running, interrupt-to-stop)
    /// agent. Surfaces use this to decide blocking vs fire-and-return semantics.
    pub keep_alive: bool,
    /// Optional session checkpointer for keep-alive persistence.
    pub checkpointer: Option<std::sync::Arc<dyn crate::checkpoint::SessionCheckpointer>>,
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
    /// Opaque application context passed through to custom `SessionAgentBuilder`
    /// implementations. Not consumed by the standard build pipeline.
    ///
    /// Uses `Value` rather than `Box<RawValue>` because `SessionBuildOptions`
    /// must be `Clone` and `Box<RawValue>` does not implement `Clone`.
    /// Same tradeoff as `provider_params`.
    pub app_context: Option<serde_json::Value>,
    /// Additional instruction sections appended to the system prompt after skill
    /// assembly, before tool instructions. Order preserved.
    pub additional_instructions: Option<Vec<String>>,
    /// Initial canonical session metadata entries applied before agent build.
    ///
    /// Used for surface-supplied runtime state such as session-local tool
    /// visibility. The factory validates special keys before applying them.
    pub initial_metadata_entries: BTreeMap<String, serde_json::Value>,
    /// Environment variables injected into shell tool subprocesses for this agent.
    /// Set by the application's `SessionAgentBuilder` — never by the LLM.
    /// Values are not included in the agent's context window.
    pub shell_env: Option<std::collections::HashMap<String, String>>,
    /// Explicit call-timeout override at the build seam.
    ///
    /// - `Inherit` (default): defer to config override, then profile default
    /// - `Disabled`: explicitly disable call timeout regardless of profile
    /// - `Value(d)`: explicitly set call timeout to `d`
    pub call_timeout_override: crate::CallTimeoutOverride,
    /// Typed explicit-override intent for resumed-session merges.
    ///
    /// Surfaces set bits only for fields they can prove were explicitly
    /// supplied by the caller. Resumed metadata then fills only the
    /// non-explicit fields.
    pub resume_override_mask: ResumeOverrideMask,
    /// Late-binding mob tool factory, called inside `build_agent()` with
    /// session-scoped args to produce the mob tool dispatcher.
    ///
    /// Surfaces that enable mob tools pass an `Arc<dyn MobToolsFactory>` here.
    /// The factory calls [`MobToolsFactory::build_mob_tools`] during agent
    /// construction with the session ID, ops lifecycle registry, and optional
    /// comms runtime — then composes the result into the tool gateway.
    pub mob_tools: Option<Arc<dyn MobToolsFactory>>,
    /// Runtime build mode — determines how the factory resolves the ops lifecycle
    /// registry and completion feed.
    ///
    /// - `SessionOwned(bindings)`: runtime-backed build with epoch-owned
    ///   bindings. Factory validates `bindings.session_id == session.id()`.
    /// - `StandaloneEphemeral`: factory creates local-only ephemeral bindings.
    ///   Suitable for WASM, tests, embedded, and standalone surfaces.
    pub runtime_build_mode: crate::runtime_epoch::RuntimeBuildMode,
    /// Runtime-stamped metadata for an eager first turn.
    ///
    /// Session services only forward this carrier. They must not infer an
    /// execution kind from runtime build mode.
    pub initial_turn_metadata: Option<RuntimeTurnMetadata>,
    /// Runtime-injected mob operator authority context.
    ///
    /// This is the only source of mob operator tool authority. Tool visibility
    /// may depend on this context being present, but dispatch-time
    /// authorization must still re-check the typed create/scope fields on
    /// every operator call.
    pub mob_tool_authority_context: Option<MobToolAuthorityContext>,
}

/// Opaque principal token carried through mob tool authority and provenance.
///
/// `meerkat-mob` may store or compare this token as an opaque blob, but it
/// must not decode token structure, branch on token contents, or expand scope
/// from it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct OpaquePrincipalToken(String);

impl OpaquePrincipalToken {
    pub fn new(token: impl Into<String>) -> Self {
        Self(token.into())
    }

    pub fn generated() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for OpaquePrincipalToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Runtime-supplied caller provenance carried alongside mob tool authority.
///
/// This is informational/projection-only data. It is not a second authority
/// source and must never be used for policy expansion inside `meerkat-mob`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct MobToolCallerProvenance {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    caller_session_id: Option<crate::SessionId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    caller_mob_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    caller_member_id: Option<String>,
}

impl MobToolCallerProvenance {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_session_id(mut self, session_id: crate::SessionId) -> Self {
        self.caller_session_id = Some(session_id);
        self
    }

    pub fn with_mob_id(mut self, mob_id: impl Into<String>) -> Self {
        self.caller_mob_id = Some(mob_id.into());
        self
    }

    pub fn with_member_id(mut self, member_id: impl Into<String>) -> Self {
        self.caller_member_id = Some(member_id.into());
        self
    }

    pub fn caller_session_id(&self) -> Option<&crate::SessionId> {
        self.caller_session_id.as_ref()
    }

    pub fn caller_mob_id(&self) -> Option<&str> {
        self.caller_mob_id.as_deref()
    }

    pub fn caller_member_id(&self) -> Option<&str> {
        self.caller_member_id.as_deref()
    }
}

/// Typed mob operator authority injected by the host/runtime.
///
/// This is capability-oriented only. It is not an identity or ownership
/// model, and it must never be inferred from mob membership, session shape,
/// `owner_session_id`, or profile flags.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MobToolAuthorityContext {
    principal_token: OpaquePrincipalToken,
    can_create_mobs: bool,
    #[serde(default)]
    can_mutate_profiles: bool,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    managed_mob_scope: BTreeSet<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    caller_provenance: Option<MobToolCallerProvenance>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    audit_invocation_id: Option<String>,
}

impl MobToolAuthorityContext {
    pub fn new(principal_token: OpaquePrincipalToken, can_create_mobs: bool) -> Self {
        Self {
            principal_token,
            can_create_mobs,
            can_mutate_profiles: can_create_mobs,
            managed_mob_scope: BTreeSet::new(),
            caller_provenance: None,
            audit_invocation_id: None,
        }
    }

    pub fn create_only_generated() -> Self {
        Self::new(OpaquePrincipalToken::generated(), true)
    }

    pub fn principal_token(&self) -> &OpaquePrincipalToken {
        &self.principal_token
    }

    pub fn can_create_mobs(&self) -> bool {
        self.can_create_mobs
    }

    pub fn can_mutate_profiles(&self) -> bool {
        self.can_mutate_profiles
    }

    pub fn with_profile_mutation(mut self, allowed: bool) -> Self {
        self.can_mutate_profiles = allowed;
        self
    }

    pub fn managed_mob_scope(&self) -> &BTreeSet<String> {
        &self.managed_mob_scope
    }

    pub fn caller_provenance(&self) -> Option<&MobToolCallerProvenance> {
        self.caller_provenance.as_ref()
    }

    pub fn audit_invocation_id(&self) -> Option<&str> {
        self.audit_invocation_id.as_deref()
    }

    pub fn can_manage_mob(&self, mob_id: &str) -> bool {
        self.managed_mob_scope.contains(mob_id)
    }

    pub fn grant_manage_mob(mut self, mob_id: impl Into<String>) -> Self {
        self.managed_mob_scope.insert(mob_id.into());
        self
    }

    /// Grant management scope for a mob in-place (mutable borrow).
    ///
    /// Used by the turn executor when applying `SessionEffect::GrantManageMob`
    /// effects from tool dispatch.
    pub fn grant_manage_mob_in_place(&mut self, mob_id: String) {
        self.managed_mob_scope.insert(mob_id);
    }

    pub fn with_managed_mob_scope<I, S>(mut self, mob_ids: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.managed_mob_scope = mob_ids.into_iter().map(Into::into).collect();
        self
    }

    pub fn with_caller_provenance(mut self, caller_provenance: MobToolCallerProvenance) -> Self {
        self.caller_provenance = Some(caller_provenance);
        self
    }

    pub fn with_audit_invocation_id(mut self, audit_invocation_id: impl Into<String>) -> Self {
        self.audit_invocation_id = Some(audit_invocation_id.into());
        self
    }
}

/// Shared host/runtime policy for explicit mob-operator enablement.
///
/// When a host/runtime build seam explicitly enables mob operator tools for a
/// session, the default authority shape is create-only. Existing-mob scope
/// must still be injected separately and explicitly.
pub fn generated_create_only_mob_operator_authority(
    enable_mob: ToolCategoryOverride,
) -> Option<MobToolAuthorityContext> {
    matches!(enable_mob, ToolCategoryOverride::Enable)
        .then(MobToolAuthorityContext::create_only_generated)
}

/// Shared build-seam rule for mob operator access rehydration.
///
/// Explicit disable clears authority. Otherwise, persisted typed authority
/// wins; if none exists, explicit mob enablement falls back to generated
/// create-only authority.
pub fn resolve_mob_operator_access(
    enable_mob: ToolCategoryOverride,
    persisted_authority_context: Option<MobToolAuthorityContext>,
) -> (ToolCategoryOverride, Option<MobToolAuthorityContext>) {
    if matches!(enable_mob, ToolCategoryOverride::Disable) {
        return (ToolCategoryOverride::Disable, None);
    }

    let authority_context = persisted_authority_context
        .or_else(|| generated_create_only_mob_operator_authority(enable_mob));
    let override_mob = if authority_context.is_some() {
        ToolCategoryOverride::Enable
    } else {
        enable_mob
    };

    (override_mob, authority_context)
}

/// Provider of a snapshot of currently visible tools.
///
/// Implemented by the agent's `ToolScope` holder to capture tool visibility
/// at spawn time for inheritance by mob children.
pub trait VisibleToolSnapshotProvider: Send + Sync {
    /// Returns the tool definitions currently visible to the parent agent.
    fn snapshot_visible_tools(&self) -> Vec<Arc<ToolDef>>;
}

/// Context for capturing a parent agent's tool scope snapshot.
///
/// `ParentOwned` carries a provider that can snapshot the parent's visible
/// tools at child spawn time. `Standalone` means no parent scope is available
/// (e.g. top-level agents, tests).
pub enum MobToolSnapshotContext {
    /// Parent agent owns a tool scope; snapshot available on demand.
    ParentOwned(Arc<dyn VisibleToolSnapshotProvider>),
    /// No parent scope available.
    Standalone,
}

/// Session-scoped arguments passed to [`MobToolsFactory::build_mob_tools`].
pub struct MobToolsBuildArgs {
    /// Session ID of the agent being built.
    pub session_id: crate::SessionId,
    /// Model name of the owning agent — inherited by implicit mob helpers.
    pub model: String,
    /// Runtime-injected mob operator authority context.
    ///
    /// Tool visibility may depend on this context being present, but operator
    /// dispatch must still re-check the typed create/scope fields on every
    /// call.
    pub authority_context: Option<MobToolAuthorityContext>,
    /// Shared effective mob authority handle owned by the agent.
    ///
    /// Mob tools read from this handle for authorization checks. The agent
    /// (turn owner) is the sole writer — it updates this handle via
    /// `apply_session_effects` after merging tool-produced `SessionEffect`s.
    /// If `None`, mob tools fall back to `authority_context` as a static snapshot.
    pub effective_authority: Option<Arc<std::sync::RwLock<MobToolAuthorityContext>>>,
    /// Comms name of the owning agent (for building `TrustedPeerDescriptor`).
    pub comms_name: Option<String>,
    /// Optional comms runtime for auto-wiring spawned members.
    pub comms_runtime: Option<Arc<dyn crate::agent::CommsRuntime>>,
    /// Context for capturing a snapshot of the parent agent's visible tools.
    pub snapshot_context: MobToolSnapshotContext,
}

/// Factory trait for late-binding mob tool construction.
///
/// Implementations capture surface-specific state (e.g. `MobMcpState`) and
/// receive session-scoped arguments from `build_agent()` at construction time.
/// This avoids a cyclic dependency between the facade crate and `meerkat-mob-mcp`.
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait MobToolsFactory: Send + Sync {
    /// Build a mob tool dispatcher for the given session.
    async fn build_mob_tools(
        &self,
        args: MobToolsBuildArgs,
    ) -> Result<Arc<dyn AgentToolDispatcher>, Box<dyn std::error::Error + Send + Sync>>;
}

/// Typed explicit-override intent for resumed-session metadata merges.
///
/// This avoids trying to recover caller intent from flattened build config.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ResumeOverrideMask {
    pub model: bool,
    pub provider: bool,
    pub max_tokens: bool,
    pub structured_output_retries: bool,
    pub provider_params: bool,
    pub auth_binding: bool,
    pub override_builtins: bool,
    pub override_shell: bool,
    pub override_memory: bool,
    pub override_mob: bool,
    pub override_image_generation: bool,
    pub override_web_search: bool,
    pub preload_skills: bool,
    pub keep_alive: bool,
    pub comms_name: bool,
    pub peer_meta: bool,
}

impl SessionBuildOptions {
    /// Apply the shared rehydration rule for mob operator access.
    ///
    /// This preserves exact persisted authority when available and otherwise
    /// falls back to generated create-only authority for explicit mob
    /// enablement.
    pub fn apply_persisted_mob_operator_access(
        &mut self,
        enable_mob: ToolCategoryOverride,
        persisted_authority_context: Option<MobToolAuthorityContext>,
    ) {
        let (override_mob, authority_context) =
            resolve_mob_operator_access(enable_mob, persisted_authority_context);
        self.override_mob = override_mob;
        self.mob_tool_authority_context = authority_context;
    }

    /// Apply the shared host/runtime default for explicit mob operator
    /// enablement.
    ///
    /// This keeps `override_mob` and the generated create-only authority
    /// context aligned at the composition seam. Existing-mob scope must be
    /// injected explicitly elsewhere; this helper never infers it.
    pub fn apply_generated_create_only_mob_operator_access(
        &mut self,
        enable_mob: ToolCategoryOverride,
    ) {
        self.apply_persisted_mob_operator_access(enable_mob, None);
    }
}

impl Default for SessionBuildOptions {
    fn default() -> Self {
        Self {
            provider: None,
            self_hosted_server_id: None,
            output_schema: None,
            structured_output_retries: 2,
            hooks_override: HookRunOverrides::default(),
            comms_name: None,
            peer_meta: None,
            resume_session: None,
            // Phase 3 field — default None keeps the legacy flat path.
            // Populated by surfaces that accept realm/binding inputs.
            budget_limits: None,
            provider_params: None,
            external_tools: None,
            recoverable_tool_defs: None,
            blob_store_override: None,
            llm_client_override: None,
            agent_llm_client_decorator: None,
            override_builtins: ToolCategoryOverride::Inherit,
            override_shell: ToolCategoryOverride::Inherit,
            override_memory: ToolCategoryOverride::Inherit,
            override_schedule: ToolCategoryOverride::Inherit,
            override_mob: ToolCategoryOverride::Inherit,
            override_image_generation: ToolCategoryOverride::Inherit,
            override_web_search: ToolCategoryOverride::Inherit,
            schedule_tools: None,
            preload_skills: None,
            realm_id: None,
            instance_id: None,
            backend: None,
            config_generation: None,
            auth_binding: None,
            keep_alive: false,
            checkpointer: None,
            silent_comms_intents: Vec::new(),
            max_inline_peer_notifications: None,
            app_context: None,
            additional_instructions: None,
            initial_metadata_entries: BTreeMap::new(),
            shell_env: None,
            call_timeout_override: crate::CallTimeoutOverride::Inherit,
            resume_override_mask: ResumeOverrideMask::default(),
            mob_tools: None,
            runtime_build_mode: crate::runtime_epoch::RuntimeBuildMode::StandaloneEphemeral,
            initial_turn_metadata: None,
            mob_tool_authority_context: None,
        }
    }
}

impl std::fmt::Debug for SessionBuildOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionBuildOptions")
            .field("provider", &self.provider)
            .field("output_schema", &self.output_schema.is_some())
            .field("structured_output_retries", &self.structured_output_retries)
            .field("hooks_override", &self.hooks_override)
            .field("comms_name", &self.comms_name)
            .field("peer_meta", &self.peer_meta)
            .field("resume_session", &self.resume_session.is_some())
            .field("budget_limits", &self.budget_limits)
            .field("provider_params", &self.provider_params.is_some())
            .field("external_tools", &self.external_tools.is_some())
            .field("recoverable_tool_defs", &self.recoverable_tool_defs)
            .field("blob_store_override", &self.blob_store_override.is_some())
            .field("llm_client_override", &self.llm_client_override.is_some())
            .field(
                "agent_llm_client_decorator",
                &self.agent_llm_client_decorator.is_some(),
            )
            .field("override_builtins", &self.override_builtins)
            .field("override_shell", &self.override_shell)
            .field("override_memory", &self.override_memory)
            .field("override_schedule", &self.override_schedule)
            .field("override_mob", &self.override_mob)
            .field("schedule_tools", &self.schedule_tools.is_some())
            .field("preload_skills", &self.preload_skills)
            .field("realm_id", &self.realm_id)
            .field("instance_id", &self.instance_id)
            .field("backend", &self.backend)
            .field("config_generation", &self.config_generation)
            .field("keep_alive", &self.keep_alive)
            .field("checkpointer", &self.checkpointer.is_some())
            .field("silent_comms_intents", &self.silent_comms_intents)
            .field(
                "max_inline_peer_notifications",
                &self.max_inline_peer_notifications,
            )
            .field("app_context", &self.app_context.is_some())
            .field("additional_instructions", &self.additional_instructions)
            .field("initial_metadata_entries", &self.initial_metadata_entries)
            .field("call_timeout_override", &self.call_timeout_override)
            .field("resume_override_mask", &self.resume_override_mask)
            .field("mob_tools", &self.mob_tools.is_some())
            .field("runtime_build_mode", &self.runtime_build_mode)
            .field(
                "initial_turn_metadata",
                &self.initial_turn_metadata.is_some(),
            )
            .field(
                "mob_tool_authority_context",
                &self.mob_tool_authority_context.is_some(),
            )
            .field("runtime_build_mode", &self.runtime_build_mode)
            .finish()
    }
}

/// Runtime/session semantic carrier for starting a turn.
///
/// The session service forwards this as one machine/composition-owned bundle.
/// It must not split handling mode, tool overlays, context appends, or runtime
/// metadata back into service-level request fields.
#[derive(Debug)]
pub struct StartTurnRuntimeSemantics {
    /// Optional normalized rendering metadata for this turn prompt.
    pub render_metadata: Option<RenderMetadata>,
    /// Handling mode for this turn's ordinary content-bearing work.
    ///
    /// This is a **runtime-owned semantic**: the runtime routes Queue/Steer
    /// before calling the executor. The session service passes this through
    /// to the `SessionAgent` but does not act on it. Non-Queue handling
    /// only works correctly on runtime-backed surfaces.
    pub handling_mode: HandlingMode,
    /// Canonical SkillKeys to resolve and inject for this turn.
    pub skill_references: Option<Vec<crate::skills::SkillKey>>,
    /// Optional per-turn flow tool overlay (ephemeral, non-persistent).
    pub flow_tool_overlay: Option<TurnToolOverlay>,
    /// Runtime-owned system-context appends that must be applied at this
    /// turn boundary before the model run starts.
    pub pre_turn_context_appends: Vec<PendingSystemContextAppend>,
    /// Canonical runtime-authored metadata for this turn.
    ///
    /// Runtime-backed callers populate this once at the machine boundary and
    /// the session layer derives per-turn policy from this typed carrier
    /// instead of re-inferring or dropping fields.
    pub turn_metadata: Option<RuntimeTurnMetadata>,
}

impl Default for StartTurnRuntimeSemantics {
    fn default() -> Self {
        Self {
            render_metadata: None,
            handling_mode: HandlingMode::Queue,
            skill_references: None,
            flow_tool_overlay: None,
            pre_turn_context_appends: Vec::new(),
            turn_metadata: None,
        }
    }
}

impl StartTurnRuntimeSemantics {
    #[must_use]
    pub fn new(
        render_metadata: Option<RenderMetadata>,
        handling_mode: HandlingMode,
        skill_references: Option<Vec<crate::skills::SkillKey>>,
        flow_tool_overlay: Option<TurnToolOverlay>,
        pre_turn_context_appends: Vec<PendingSystemContextAppend>,
        turn_metadata: Option<RuntimeTurnMetadata>,
    ) -> Self {
        Self {
            render_metadata,
            handling_mode,
            skill_references,
            flow_tool_overlay,
            pre_turn_context_appends,
            turn_metadata,
        }
    }

    #[must_use]
    pub fn runtime_metadata(turn_metadata: RuntimeTurnMetadata) -> Self {
        Self {
            turn_metadata: Some(turn_metadata),
            ..Self::default()
        }
    }
}

/// Request to start a new turn on an existing session.
#[derive(Debug)]
pub struct StartTurnRequest {
    /// User prompt for this turn (text or multimodal).
    pub prompt: ContentInput,
    /// Optional system prompt override for a deferred session's first turn.
    ///
    /// This is only supported before the session has any conversation history.
    /// Materialized sessions with existing messages must reject it.
    pub system_prompt: Option<String>,
    /// Channel for streaming events during the turn.
    pub event_tx: Option<mpsc::Sender<EventEnvelope<AgentEvent>>>,
    /// Single runtime/session semantic carrier for this turn.
    pub runtime: StartTurnRuntimeSemantics,
}

/// Request to append runtime system context to an existing session.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AppendSystemContextRequest {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
}

/// Result of appending runtime system context to a session.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AppendSystemContextResult {
    pub status: AppendSystemContextStatus,
}

/// Request to stage callback tool results for the next turn.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StageToolResultsRequest {
    pub results: Vec<crate::ToolResult>,
}

/// Result of staging callback tool results for the next turn.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StageToolResultsResult {
    pub accepted_result_count: usize,
}

/// Outcome of an append-system-context request.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AppendSystemContextStatus {
    Applied,
    Staged,
    Duplicate,
}

/// Ephemeral per-turn tool overlay for flow-dispatched turns.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct TurnToolOverlay {
    /// Optional allow-list for this turn.
    #[serde(default)]
    pub allowed_tools: Option<Vec<String>>,
    /// Optional deny-list for this turn.
    #[serde(default)]
    pub blocked_tools: Option<Vec<String>>,
}

/// Query parameters for listing sessions.
#[derive(Debug, Default)]
pub struct SessionQuery {
    /// Maximum number of results.
    pub limit: Option<usize>,
    /// Offset for pagination.
    pub offset: Option<usize>,
    /// Filters sessions where all specified k/v pairs match.
    pub labels: Option<BTreeMap<String, String>>,
}

/// Summary of a session (for list results).
///
/// Kept lightweight — no billing data. Use `read()` for full details.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSummary {
    pub session_id: SessionId,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    pub message_count: usize,
    pub total_tokens: u64,
    pub is_active: bool,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub labels: BTreeMap<String, String>,
}

/// Detailed view of a session's state and history metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
    pub session_id: SessionId,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    pub message_count: usize,
    pub is_active: bool,
    pub model: String,
    pub provider: Provider,
    pub last_assistant_text: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub labels: BTreeMap<String, String>,
}

/// Billing/usage data for a session, returned separately from state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionUsage {
    pub total_tokens: u64,
    pub usage: Usage,
}

/// Combined session view (state + usage). Convenience wrapper used by
/// `SessionService::read()` to avoid requiring two calls.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionView {
    pub state: SessionInfo,
    pub billing: SessionUsage,
}

impl SessionView {
    /// Convenience: session ID from the state.
    pub fn session_id(&self) -> &SessionId {
        &self.state.session_id
    }
}

/// Query parameters for reading session history.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionHistoryQuery {
    /// Number of messages to skip from the start of the transcript.
    pub offset: usize,
    /// Maximum number of messages to return.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

/// Paginated transcript page for a session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionHistoryPage {
    pub session_id: SessionId,
    pub message_count: usize,
    pub offset: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
    pub has_more: bool,
    pub messages: Vec<Message>,
}

impl SessionHistoryPage {
    /// Build a transcript page from the full ordered message list.
    pub fn from_messages(
        session_id: SessionId,
        messages: &[Message],
        query: SessionHistoryQuery,
    ) -> Self {
        let message_count = messages.len();
        let start = query.offset.min(message_count);
        let end = match query.limit {
            Some(limit) => start.saturating_add(limit).min(message_count),
            None => message_count,
        };
        Self {
            session_id,
            message_count,
            offset: start,
            limit: query.limit,
            has_more: end < message_count,
            messages: messages[start..end].to_vec(),
        }
    }
}

/// Explicit behavior for transcript fork/edit requests when the source
/// session has active work.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum TranscriptEditRunningBehavior {
    /// Reject the request while the source session is active.
    #[default]
    Reject,
}

/// Request to fork a session at a transcript message index.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SessionForkAtRequest {
    pub message_index: usize,
    #[serde(default)]
    pub running_behavior: TranscriptEditRunningBehavior,
}

/// Request to fork a session and apply a typed transcript replacement.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SessionForkReplaceRequest {
    pub message_index: usize,
    #[cfg_attr(feature = "schema", schemars(with = "serde_json::Value"))]
    pub replacement: TranscriptReplacement,
    #[serde(default)]
    pub running_behavior: TranscriptEditRunningBehavior,
}

/// Result of creating an edited transcript branch.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SessionForkResult {
    #[cfg_attr(feature = "schema", schemars(with = "String"))]
    pub source_session_id: SessionId,
    #[cfg_attr(feature = "schema", schemars(with = "String"))]
    pub session_id: SessionId,
    pub message_count: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_ref: Option<String>,
}

impl TranscriptEditError {
    /// Convert a typed edit validation failure into a surface-friendly session
    /// error without widening the `SessionService` error enum.
    pub fn into_session_error(self) -> SessionError {
        SessionError::Agent(crate::error::AgentError::ConfigError(self.to_string()))
    }
}

/// Canonical session lifecycle abstraction.
///
/// All surfaces delegate to this trait. Implementations control persistence,
/// compaction, and event logging behavior.
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait SessionService: Send + Sync {
    /// Create a new session and run the first turn.
    async fn create_session(&self, req: CreateSessionRequest) -> Result<RunResult, SessionError>;

    /// Start a new turn on an existing session.
    async fn start_turn(
        &self,
        id: &SessionId,
        req: StartTurnRequest,
    ) -> Result<RunResult, SessionError>;

    /// Cancel an in-flight turn.
    ///
    /// Returns `NotRunning` if no turn is active.
    async fn interrupt(&self, id: &SessionId) -> Result<(), SessionError>;

    /// Cancel an in-flight turn once it reaches the next boundary.
    ///
    /// Returns `NotRunning` if no turn is active. Unsupported by default.
    async fn cancel_after_boundary(&self, _id: &SessionId) -> Result<(), SessionError> {
        Err(SessionError::Unsupported(
            "cancel_after_boundary".to_string(),
        ))
    }

    /// Replace the LLM client on a live session.
    ///
    /// Enables mid-session model/provider hot-swap without rebuilding the
    /// agent. The new client takes effect on the next turn. Returns
    /// `Unsupported` by default; session services that support live agents
    /// override this.
    async fn set_session_client(
        &self,
        _id: &SessionId,
        _client: std::sync::Arc<dyn crate::AgentLlmClient>,
    ) -> Result<(), SessionError> {
        Err(SessionError::Unsupported("set_session_client".to_string()))
    }

    /// Atomically replace the live session client and the session's durable
    /// LLM identity.
    ///
    /// This is the canonical seam for materialized-session hot-swap semantics.
    /// Implementations should apply both updates together so future turns and
    /// resume/recovery see the same model/provider/provider_params identity.
    async fn hot_swap_session_llm_identity(
        &self,
        _id: &SessionId,
        _client: std::sync::Arc<dyn crate::AgentLlmClient>,
        _identity: SessionLlmIdentity,
        _request_policy: crate::SessionLlmRequestPolicy,
    ) -> Result<(), SessionError> {
        Err(SessionError::Unsupported(
            "hot_swap_session_llm_identity".to_string(),
        ))
    }

    /// Replace the canonical tool visibility state carried by the live session.
    ///
    /// This seam is live-only and must not perform its own durable write. The
    /// caller owns any surrounding transactional persistence and rollback.
    async fn set_session_tool_visibility_state(
        &self,
        _id: &SessionId,
        _state: Option<crate::SessionToolVisibilityState>,
    ) -> Result<(), SessionError> {
        Err(SessionError::Unsupported(
            "set_session_tool_visibility_state".to_string(),
        ))
    }

    /// Update the `keep_alive` flag on a live session's durable metadata.
    ///
    /// Called by the runtime when an explicit override changes the session's
    /// keep-alive intent so that subsequent inheriting calls observe the
    /// updated value. Returns `Unsupported` by default.
    async fn update_session_keep_alive(
        &self,
        _id: &SessionId,
        _keep_alive: bool,
    ) -> Result<(), SessionError> {
        Err(SessionError::Unsupported(
            "update_session_keep_alive".to_string(),
        ))
    }

    /// Update the session's canonical mob operator authority context.
    ///
    /// This is the only supported seam for widening or narrowing exact mob
    /// management scope after session creation so recovery and live runtime
    /// state stay aligned.
    async fn update_session_mob_authority_context(
        &self,
        _id: &SessionId,
        _authority_context: Option<MobToolAuthorityContext>,
    ) -> Result<(), SessionError> {
        Err(SessionError::Unsupported(
            "update_session_mob_authority_context".to_string(),
        ))
    }

    /// Whether a live in-memory session bridge currently exists for `id`.
    ///
    /// This is intentionally distinct from `list()` / `SessionSummary`:
    /// persisted-only summaries must not count as live, and idle live sessions
    /// must still count as live even when no turn is running.
    async fn has_live_session(&self, _id: &SessionId) -> Result<bool, SessionError> {
        Err(SessionError::Unsupported("has_live_session".to_string()))
    }

    /// Stage an external tool visibility filter on a live session.
    ///
    /// Used to dynamically hide/show tools (e.g., `view_image`) after a
    /// model hot-swap changes capability support. Returns `Unsupported`
    /// by default.
    async fn set_session_tool_filter(
        &self,
        _id: &SessionId,
        _filter: crate::ToolFilter,
    ) -> Result<(), SessionError> {
        Err(SessionError::Unsupported(
            "set_session_tool_filter".to_string(),
        ))
    }

    /// Read the current state of a session.
    async fn read(&self, id: &SessionId) -> Result<SessionView, SessionError>;

    /// List sessions matching the query.
    async fn list(&self, query: SessionQuery) -> Result<Vec<SessionSummary>, SessionError>;

    /// Archive (remove) a session.
    async fn archive(&self, id: &SessionId) -> Result<(), SessionError>;

    /// Subscribe to session-wide events regardless of triggering interaction.
    ///
    /// Services that do not support this capability return `StreamError::NotFound`.
    async fn subscribe_session_events(&self, id: &SessionId) -> Result<EventStream, StreamError> {
        Err(StreamError::NotFound(format!("session {id}")))
    }
}

/// Optional comms/control-plane extension for `SessionService`.
///
/// Base lifecycle operations stay on `SessionService`; advanced surfaces
/// (RPC/REST/mob orchestration) can use this trait when they need direct
/// access to comms runtime and injector handles.
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait SessionServiceCommsExt: SessionService {
    /// Get the comms runtime for a session, if available.
    async fn comms_runtime(
        &self,
        _session_id: &SessionId,
    ) -> Option<Arc<dyn crate::agent::CommsRuntime>> {
        None
    }

    /// Get the event injector for a session, if available.
    async fn event_injector(
        &self,
        session_id: &SessionId,
    ) -> Option<Arc<dyn crate::EventInjector>> {
        self.comms_runtime(session_id)
            .await
            .and_then(|runtime| runtime.event_injector())
    }

    /// Internal runtime seam for interaction-scoped injection.
    #[doc(hidden)]
    async fn interaction_event_injector(
        &self,
        session_id: &SessionId,
    ) -> Option<Arc<dyn crate::event_injector::SubscribableInjector>> {
        self.comms_runtime(session_id)
            .await
            .and_then(|runtime| runtime.interaction_event_injector())
    }
}

/// Optional control-plane extension for `SessionService`.
///
/// Keeps the base lifecycle contract minimal while exposing first-class
/// session mutation operations shared across external surfaces.
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait SessionServiceControlExt: SessionService {
    /// Append runtime system context to a session.
    ///
    /// The request is idempotent per `(session_id, idempotency_key)`. When a
    /// turn is active, implementations may stage the append for application at
    /// the next LLM boundary rather than mutating in-flight request state.
    async fn append_system_context(
        &self,
        id: &SessionId,
        req: AppendSystemContextRequest,
    ) -> Result<AppendSystemContextResult, SessionControlError>;

    /// Stage callback tool results for application on the next turn seam.
    ///
    /// Implementations must persist the staged results durably before a live
    /// session can observe them so a failed call never leaves hidden pending
    /// transcript mutations behind.
    async fn stage_tool_results(
        &self,
        id: &SessionId,
        req: StageToolResultsRequest,
    ) -> Result<StageToolResultsResult, SessionError> {
        let _ = (id, req);
        Err(SessionError::Unsupported("stage_tool_results".to_string()))
    }
}

/// Optional history-read extension for `SessionService`.
///
/// Keeps the base lifecycle contract lightweight while allowing surfaces to
/// fetch full transcript contents when they explicitly opt in.
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait SessionServiceHistoryExt: SessionService {
    /// Read the committed transcript for a session.
    ///
    /// Implementations may return `PersistenceDisabled` if they cannot provide
    /// authoritative history for the requested lifecycle state.
    async fn read_history(
        &self,
        id: &SessionId,
        query: SessionHistoryQuery,
    ) -> Result<SessionHistoryPage, SessionError>;
}

/// Optional typed transcript fork/edit extension for `SessionService`.
///
/// Implementations must create a new session identity for every operation.
/// Source history is never mutated in place.
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait SessionServiceTranscriptEditExt: SessionService {
    /// Fork a session at a message index.
    async fn fork_session_at(
        &self,
        id: &SessionId,
        req: SessionForkAtRequest,
    ) -> Result<SessionForkResult, SessionError> {
        let _ = (id, req);
        Err(SessionError::Unsupported("fork_session_at".to_string()))
    }

    /// Fork a session and apply a typed transcript replacement.
    async fn fork_session_replace(
        &self,
        id: &SessionId,
        req: SessionForkReplaceRequest,
    ) -> Result<SessionForkResult, SessionError> {
        let _ = (id, req);
        Err(SessionError::Unsupported(
            "fork_session_replace".to_string(),
        ))
    }
}

/// Extension trait for `Arc<dyn SessionService>` to allow calling methods directly.
impl dyn SessionService {
    /// Wrap self in an Arc.
    pub fn into_arc(self: Box<Self>) -> Arc<dyn SessionService> {
        Arc::from(self)
    }
}

#[cfg(test)]
#[allow(
    clippy::unimplemented,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]
mod tests {
    use super::*;

    struct UnsupportedSessionService;

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl SessionService for UnsupportedSessionService {
        async fn create_session(
            &self,
            _req: CreateSessionRequest,
        ) -> Result<RunResult, SessionError> {
            unimplemented!()
        }

        async fn start_turn(
            &self,
            _id: &SessionId,
            _req: StartTurnRequest,
        ) -> Result<RunResult, SessionError> {
            unimplemented!()
        }

        async fn interrupt(&self, _id: &SessionId) -> Result<(), SessionError> {
            unimplemented!()
        }

        async fn read(&self, _id: &SessionId) -> Result<SessionView, SessionError> {
            unimplemented!()
        }

        async fn list(&self, _query: SessionQuery) -> Result<Vec<SessionSummary>, SessionError> {
            unimplemented!()
        }

        async fn archive(&self, _id: &SessionId) -> Result<(), SessionError> {
            unimplemented!()
        }
    }

    #[tokio::test]
    async fn has_live_session_defaults_to_unsupported() {
        let service = UnsupportedSessionService;
        let err = service
            .has_live_session(&SessionId::new())
            .await
            .expect_err("default implementation should fail loudly");
        assert!(matches!(err, SessionError::Unsupported(name) if name == "has_live_session"));
    }

    #[test]
    fn grant_manage_mob_in_place_adds_mob_id() {
        let mut ctx = MobToolAuthorityContext::create_only_generated();
        ctx.grant_manage_mob_in_place("mob-1".into());
        assert!(ctx.managed_mob_scope.contains("mob-1"));
    }

    #[test]
    fn grant_manage_mob_in_place_is_idempotent() {
        let mut ctx = MobToolAuthorityContext::create_only_generated();
        ctx.grant_manage_mob_in_place("mob-1".into());
        ctx.grant_manage_mob_in_place("mob-1".into());
        assert_eq!(ctx.managed_mob_scope.len(), 1);
    }

    #[test]
    fn grant_manage_mob_in_place_accumulates() {
        let mut ctx = MobToolAuthorityContext::create_only_generated();
        ctx.grant_manage_mob_in_place("mob-1".into());
        ctx.grant_manage_mob_in_place("mob-2".into());
        assert!(ctx.managed_mob_scope.contains("mob-1"));
        assert!(ctx.managed_mob_scope.contains("mob-2"));
        assert_eq!(ctx.managed_mob_scope.len(), 2);
    }

    struct MockSnapshotProvider {
        tools: Vec<Arc<ToolDef>>,
    }

    impl VisibleToolSnapshotProvider for MockSnapshotProvider {
        fn snapshot_visible_tools(&self) -> Vec<Arc<ToolDef>> {
            self.tools.clone()
        }
    }

    #[test]
    fn mob_tool_snapshot_context_standalone() {
        let ctx = MobToolSnapshotContext::Standalone;
        assert!(matches!(ctx, MobToolSnapshotContext::Standalone));
    }

    #[test]
    fn mob_tool_snapshot_context_parent_owned_returns_tools() {
        let tools = vec![Arc::new(ToolDef {
            name: "test_tool".into(),
            description: "a test".to_string(),
            input_schema: serde_json::json!({"type": "object"}),
            provenance: None,
        })];
        let provider = Arc::new(MockSnapshotProvider { tools });
        let ctx = MobToolSnapshotContext::ParentOwned(provider);
        match ctx {
            MobToolSnapshotContext::ParentOwned(p) => {
                let snapshot = p.snapshot_visible_tools();
                assert_eq!(snapshot.len(), 1);
                assert_eq!(snapshot[0].name, "test_tool");
            }
            MobToolSnapshotContext::Standalone => panic!("expected ParentOwned"),
        }
    }
}
