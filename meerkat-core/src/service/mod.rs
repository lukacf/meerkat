//! SessionService trait — canonical lifecycle abstraction.
//!
//! All surfaces (CLI, REST, MCP Server, JSON-RPC) route through `SessionService`.
//! Implementations may be ephemeral (in-memory only) or persistent (backed by a store).

pub mod transport;

use crate::event::AgentEvent;
use crate::event::EventEnvelope;
use crate::ops_lifecycle::OpsLifecycleRegistry;
use crate::session::SystemContextStageError;
use crate::time_compat::SystemTime;
#[cfg(target_arch = "wasm32")]
use crate::tokio;
use crate::types::{
    ContentInput, HandlingMode, Message, RenderMetadata, RunResult, SessionId, Usage,
};
use crate::{
    AgentToolDispatcher, BudgetLimits, HookRunOverrides, OutputSchema, PeerMeta, Provider, Session,
    SessionLlmIdentity,
};
use crate::{EventStream, StreamError};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;
use tokio::sync::mpsc;

/// Controls whether `create_session()` should execute an initial turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InitialTurnPolicy {
    /// Run the initial turn immediately as part of session creation.
    RunImmediately,
    /// Register the session and return without running an initial turn.
    Defer,
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
    /// Optional extended build options for factory-backed builders.
    pub build: Option<SessionBuildOptions>,
    /// Optional key-value labels attached at session creation.
    pub labels: Option<BTreeMap<String, String>>,
}

/// Optional build-time options used by factory-backed session builders.
#[derive(Clone)]
pub struct SessionBuildOptions {
    pub provider: Option<Provider>,
    pub output_schema: Option<OutputSchema>,
    pub structured_output_retries: u32,
    pub hooks_override: HookRunOverrides,
    pub comms_name: Option<String>,
    pub peer_meta: Option<PeerMeta>,
    pub resume_session: Option<Session>,
    pub budget_limits: Option<BudgetLimits>,
    pub provider_params: Option<serde_json::Value>,
    pub external_tools: Option<Arc<dyn AgentToolDispatcher>>,
    /// Opaque transport for an optional per-request LLM override.
    ///
    /// Factory builders may downcast this to their concrete client trait.
    pub llm_client_override: Option<Arc<dyn std::any::Any + Send + Sync>>,
    /// Canonical async-op registry for the owning session.
    ///
    /// Runtime-backed surfaces should provide the real per-session registry
    /// from the runtime adapter rather than letting deeper layers allocate a
    /// fresh local registry.
    pub ops_lifecycle_override: Option<Arc<dyn OpsLifecycleRegistry>>,
    pub override_builtins: Option<bool>,
    pub override_shell: Option<bool>,
    pub override_memory: Option<bool>,
    pub override_mob: Option<bool>,
    pub preload_skills: Option<Vec<crate::skills::SkillId>>,
    pub realm_id: Option<String>,
    pub instance_id: Option<String>,
    pub backend: Option<String>,
    pub config_generation: Option<u64>,
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
}

impl Default for SessionBuildOptions {
    fn default() -> Self {
        Self {
            provider: None,
            output_schema: None,
            structured_output_retries: 2,
            hooks_override: HookRunOverrides::default(),
            comms_name: None,
            peer_meta: None,
            resume_session: None,
            budget_limits: None,
            provider_params: None,
            external_tools: None,
            llm_client_override: None,
            ops_lifecycle_override: None,
            override_builtins: None,
            override_shell: None,
            override_memory: None,
            override_mob: None,
            preload_skills: None,
            realm_id: None,
            instance_id: None,
            backend: None,
            config_generation: None,
            keep_alive: false,
            checkpointer: None,
            silent_comms_intents: Vec::new(),
            max_inline_peer_notifications: None,
            app_context: None,
            additional_instructions: None,
            shell_env: None,
            call_timeout_override: crate::CallTimeoutOverride::Inherit,
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
            .field("llm_client_override", &self.llm_client_override.is_some())
            .field(
                "ops_lifecycle_override",
                &self.ops_lifecycle_override.is_some(),
            )
            .field("override_builtins", &self.override_builtins)
            .field("override_shell", &self.override_shell)
            .field("override_memory", &self.override_memory)
            .field("override_mob", &self.override_mob)
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
            .field("call_timeout_override", &self.call_timeout_override)
            .finish()
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
    /// Optional normalized rendering metadata for this turn prompt.
    pub render_metadata: Option<RenderMetadata>,
    /// Handling mode for this turn's ordinary content-bearing work.
    ///
    /// This is a **runtime-owned semantic**: the runtime routes Queue/Steer
    /// before calling the executor. The session service passes this through
    /// to the `SessionAgent` but does not act on it. Non-Queue handling
    /// only works correctly on runtime-backed surfaces.
    pub handling_mode: HandlingMode,
    /// Channel for streaming events during the turn.
    pub event_tx: Option<mpsc::Sender<EventEnvelope<AgentEvent>>>,
    /// Canonical SkillKeys to resolve and inject for this turn.
    pub skill_references: Option<Vec<crate::skills::SkillKey>>,
    /// Optional per-turn flow tool overlay (ephemeral, non-persistent).
    pub flow_tool_overlay: Option<TurnToolOverlay>,
    /// Optional additional instructions prepended as `[SYSTEM NOTICE: ...]` to the user prompt.
    ///
    /// Unlike `SessionBuildOptions.additional_instructions` (which are appended to the
    /// system prompt as extra sections at session creation), turn-level instructions
    /// are prepended to the user message as `[SYSTEM NOTICE: {instruction}]` blocks.
    /// This distinction means create-time instructions persist across turns (system prompt)
    /// while turn-level instructions are per-turn only (conversation history).
    pub additional_instructions: Option<Vec<String>>,
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
    ) -> Result<(), SessionError> {
        Err(SessionError::Unsupported(
            "hot_swap_session_llm_identity".to_string(),
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

/// Extension trait for `Arc<dyn SessionService>` to allow calling methods directly.
impl dyn SessionService {
    /// Wrap self in an Arc.
    pub fn into_arc(self: Box<Self>) -> Arc<dyn SessionService> {
        Arc::from(self)
    }
}

#[cfg(test)]
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
}
