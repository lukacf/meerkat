//! SessionService trait — canonical lifecycle abstraction.
//!
//! All surfaces (CLI, REST, MCP Server, JSON-RPC) route through `SessionService`.
//! Implementations may be ephemeral (in-memory only) or persistent (backed by a store).

pub mod transport;

use crate::event::AgentEvent;
use crate::types::{RunResult, SessionId, Usage};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::SystemTime;
use tokio::sync::mpsc;

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
    /// Skill IDs to resolve and inject for the first turn.
    pub skill_references: Option<Vec<crate::skills::SkillId>>,
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
    /// Skill IDs to resolve and inject for this turn.
    pub skill_references: Option<Vec<crate::skills::SkillId>>,
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
}

/// Extension trait for `Arc<dyn SessionService>` to allow calling methods directly.
impl dyn SessionService {
    /// Wrap self in an Arc.
    pub fn into_arc(self: Box<Self>) -> Arc<dyn SessionService> {
        Arc::from(self)
    }
}
