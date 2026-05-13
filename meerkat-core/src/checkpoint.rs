//! Session checkpointing trait for periodic persistence.
//!
//! Host-mode agents run indefinitely, so the normal post-turn persistence path
//! (`PersistentSessionService::persist_full_session`) never fires after the
//! initial `create_session`. This trait allows the agent to checkpoint the
//! session after each keep-alive interaction.

use crate::session::Session;
use crate::types::SessionId;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Machine-owned classifier for a failed session checkpoint.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionCheckpointErrorKind {
    BlobExternalization,
    DeferredTurnExternalization,
    DeferredTurnSerialization,
    StoreSave,
}

impl SessionCheckpointErrorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::BlobExternalization => "blob_externalization",
            Self::DeferredTurnExternalization => "deferred_turn_externalization",
            Self::DeferredTurnSerialization => "deferred_turn_serialization",
            Self::StoreSave => "store_save",
        }
    }
}

impl std::fmt::Display for SessionCheckpointErrorKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Failure returned by a session checkpointer.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error("checkpoint failed for session {session_id} during {kind}: {message}")]
pub struct SessionCheckpointError {
    pub session_id: SessionId,
    pub kind: SessionCheckpointErrorKind,
    pub message: String,
}

impl SessionCheckpointError {
    pub fn new(
        session_id: SessionId,
        kind: SessionCheckpointErrorKind,
        message: impl Into<String>,
    ) -> Self {
        Self {
            session_id,
            kind,
            message: message.into(),
        }
    }
}

/// Periodic session persistence hook.
///
/// Implementations clone what they need from the borrowed `Session`.
/// Failures are returned as typed checkpoint errors so callers can report
/// stale durability instead of presenting a successful run.
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait SessionCheckpointer: Send + Sync {
    /// Save a snapshot of the current session state.
    async fn checkpoint(&self, session: &Session) -> Result<(), SessionCheckpointError>;
}
