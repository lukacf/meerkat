//! SessionService trait — canonical lifecycle abstraction.
//!
//! All surfaces (CLI, REST, MCP Server, JSON-RPC) route through `SessionService`.
//! Implementations may be ephemeral (in-memory only) or persistent (backed by a store).

pub mod transport;

use crate::event::{AgentEvent, ScopedAgentEvent, StreamScopeFrame};
use crate::types::{RunResult, SessionId, Usage};
use crate::{
    AgentToolDispatcher, BudgetLimits, HookRunOverrides, OutputSchema, PeerMeta, Provider, Session,
};
use crate::{EventStream, StreamError};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::SystemTime;
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
            Self::Agent(_) => "AGENT_ERROR",
        }
    }
}

/// Request to create a new session and run the first turn.
#[derive(Debug)]
pub struct CreateSessionRequest {
    /// Model name (e.g. "claude-opus-4-6").
    pub model: String,
    /// Initial user prompt.
    pub prompt: String,
    /// Optional system prompt override.
    pub system_prompt: Option<String>,
    /// Max tokens per LLM turn.
    pub max_tokens: Option<u32>,
    /// Channel for streaming events during the turn.
    pub event_tx: Option<mpsc::Sender<AgentEvent>>,
    /// Run in host mode: process prompt then stay alive for comms messages.
    /// This is a session-level property applied to all turns.
    pub host_mode: bool,
    /// Canonical SkillKeys to resolve and inject for the first turn.
    pub skill_references: Option<Vec<crate::skills::SkillKey>>,
    /// Initial turn behavior for this session creation call.
    pub initial_turn: InitialTurnPolicy,
    /// Optional extended build options for factory-backed builders.
    pub build: Option<SessionBuildOptions>,
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
    /// Optional scoped stream sink for attributed multi-agent events.
    pub scoped_event_tx: Option<mpsc::Sender<ScopedAgentEvent>>,
    /// Base scope path for attributed events emitted by nested sub-agents.
    pub scoped_event_path: Option<Vec<StreamScopeFrame>>,
    pub override_builtins: Option<bool>,
    pub override_shell: Option<bool>,
    pub override_subagents: Option<bool>,
    pub override_memory: Option<bool>,
    pub preload_skills: Option<Vec<crate::skills::SkillId>>,
    pub realm_id: Option<String>,
    pub instance_id: Option<String>,
    pub backend: Option<String>,
    pub config_generation: Option<u64>,
    /// Optional session checkpointer for host-mode persistence.
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
            scoped_event_tx: None,
            scoped_event_path: None,
            override_builtins: None,
            override_shell: None,
            override_subagents: None,
            override_memory: None,
            preload_skills: None,
            realm_id: None,
            instance_id: None,
            backend: None,
            config_generation: None,
            checkpointer: None,
            silent_comms_intents: Vec::new(),
            max_inline_peer_notifications: None,
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
            .field("scoped_event_tx", &self.scoped_event_tx.is_some())
            .field("scoped_event_path", &self.scoped_event_path.is_some())
            .field("override_builtins", &self.override_builtins)
            .field("override_shell", &self.override_shell)
            .field("override_subagents", &self.override_subagents)
            .field("override_memory", &self.override_memory)
            .field("preload_skills", &self.preload_skills)
            .field("realm_id", &self.realm_id)
            .field("instance_id", &self.instance_id)
            .field("backend", &self.backend)
            .field("config_generation", &self.config_generation)
            .field("checkpointer", &self.checkpointer.is_some())
            .field("silent_comms_intents", &self.silent_comms_intents)
            .field(
                "max_inline_peer_notifications",
                &self.max_inline_peer_notifications,
            )
            .finish()
    }
}

/// Request to start a new turn on an existing session.
#[derive(Debug)]
pub struct StartTurnRequest {
    /// User prompt for this turn.
    pub prompt: String,
    /// Channel for streaming events during the turn.
    pub event_tx: Option<mpsc::Sender<AgentEvent>>,
    /// Run this turn in host mode.
    pub host_mode: bool,
    /// Canonical SkillKeys to resolve and inject for this turn.
    pub skill_references: Option<Vec<crate::skills::SkillKey>>,
}

/// Query parameters for listing sessions.
#[derive(Debug, Default)]
pub struct SessionQuery {
    /// Maximum number of results.
    pub limit: Option<usize>,
    /// Offset for pagination.
    pub offset: Option<usize>,
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
}

/// Detailed view of a session's state and history metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
    pub session_id: SessionId,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    pub message_count: usize,
    pub is_active: bool,
    pub last_assistant_text: Option<String>,
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

/// Canonical session lifecycle abstraction.
///
/// All surfaces delegate to this trait. Implementations control persistence,
/// compaction, and event logging behavior.
#[async_trait]
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
        Err(StreamError::NotFound(format!("session {}", id)))
    }
}

/// Extension trait for `Arc<dyn SessionService>` to allow calling methods directly.
impl dyn SessionService {
    /// Wrap self in an Arc.
    pub fn into_arc(self: Box<Self>) -> Arc<dyn SessionService> {
        Arc::from(self)
    }
}
