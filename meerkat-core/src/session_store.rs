//! SessionStore trait — canonical session persistence contract.
//!
//! This trait lives in `meerkat-core` so that custom storage implementations
//! (Postgres, DynamoDB, etc.) can be written without depending on `meerkat-store`.
//!
//! # Snapshot = projection
//!
//! The `Session` row a `SessionStore` persists is a **projection of the
//! canonical event log**. The event log (`EventStore`) is append-only at
//! the trait level; the snapshot is a rebuildable materialization of
//! replaying that log. Deleting a `.rkat/sessions/<id>/session.json` and
//! replaying the event store produces an identical snapshot (the
//! `CLAUDE.md` invariant).
//!
//! Wave-c C-H1 (F1 closure from the state-scope-audit) makes the
//! append-only nature of that projection enforceable at the
//! `SessionStore::save` boundary — see the trait docs on
//! [`SessionStore`] and the [`append_only_save_guard`] helper.

use async_trait::async_trait;

use crate::Session;
use crate::session::SessionMeta;
use crate::time_compat::SystemTime;
use crate::types::SessionId;

/// Filter for listing sessions.
#[derive(Debug, Clone, Default)]
pub struct SessionFilter {
    /// Only sessions created after this time.
    pub created_after: Option<SystemTime>,
    /// Only sessions updated after this time.
    pub updated_after: Option<SystemTime>,
    /// Maximum number of results.
    pub limit: Option<usize>,
    /// Offset for pagination.
    pub offset: Option<usize>,
}

/// Errors from session store operations.
///
/// Backend-specific details (rusqlite, filesystem, etc.) are erased to strings
/// so that the trait contract carries no I/O dependencies.
#[derive(Debug, thiserror::Error)]
pub enum SessionStoreError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    Serialization(String),

    #[error("Session not found: {0}")]
    NotFound(SessionId),

    #[error("Session corrupted: {0}")]
    Corrupted(SessionId),

    #[error(
        "session {id} save rejected: new message count {new_len} is shorter than previously \
         persisted {prev_len} — shrink operations must go through Session::fork_at (F1 \
         closure, wave-c C-H1)"
    )]
    MonotonicityViolation {
        id: SessionId,
        prev_len: usize,
        new_len: usize,
    },

    #[error("Internal error: {0}")]
    Internal(String),
}

/// Shared append-only guard for `SessionStore::save` implementations.
///
/// Backends call this at the top of their `save` method with the new
/// session and the previously persisted row (or `None` if no prior row
/// exists). Returns
/// [`SessionStoreError::MonotonicityViolation`] when the new row's
/// message count is strictly smaller than the previously persisted one.
///
/// Backends are free to additionally short-circuit on `Equal` /
/// `Greater` outcomes — the guard only rejects strict decreases.
pub fn append_only_save_guard(
    incoming: &Session,
    previous: Option<&Session>,
) -> Result<(), SessionStoreError> {
    if let Some(prev) = previous {
        let prev_len = prev.messages().len();
        let new_len = incoming.messages().len();
        if new_len < prev_len {
            return Err(SessionStoreError::MonotonicityViolation {
                id: incoming.id().clone(),
                prev_len,
                new_len,
            });
        }
    }
    Ok(())
}

impl From<serde_json::Error> for SessionStoreError {
    fn from(e: serde_json::Error) -> Self {
        Self::Serialization(e.to_string())
    }
}

/// Abstraction over session storage backends.
///
/// All methods take `&self` — implementations must handle interior mutability.
/// Object-safe: consumed as `Arc<dyn SessionStore>` throughout the system.
///
/// # Append-only contract (F1 closure, wave-c C-H1)
///
/// The snapshot written by [`save`](Self::save) is a **projection of the
/// canonical event log** ([`crate::session_store`] doc: "snapshot =
/// projection"). Implementations that persist across calls MUST enforce
/// that the message vector stored for a given `SessionId` is monotonically
/// non-shrinking — a subsequent `save()` for the same id must not have a
/// smaller `messages().len()` than the previously persisted row.
///
/// Callers that need to produce a session with a shorter history must go
/// through [`Session::fork_at`], which rotates `SessionId` — a fork is a
/// new identity on a new event log, not a same-session truncation.
///
/// Backends are encouraged to assert this invariant in their `save`
/// implementation and return
/// [`SessionStoreError::MonotonicityViolation`] when a caller tries to
/// shrink a snapshot. The default implementations in `meerkat-store`
/// (`SqliteSessionStore`, `JsonlStore`, `MemoryStore`) all go through
/// the [`append_only_save_guard`] helper.
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait SessionStore: Send + Sync {
    /// Save a session (create or extend).
    ///
    /// Implementations MUST reject a save whose message history is
    /// shorter than the previously persisted row for the same `SessionId`
    /// — see the trait-level doc on the append-only contract.
    async fn save(&self, session: &Session) -> Result<(), SessionStoreError>;

    /// Load a session by ID.
    async fn load(&self, id: &SessionId) -> Result<Option<Session>, SessionStoreError>;

    /// List sessions matching filter.
    async fn list(&self, filter: SessionFilter) -> Result<Vec<SessionMeta>, SessionStoreError>;

    /// Delete a session.
    async fn delete(&self, id: &SessionId) -> Result<(), SessionStoreError>;

    /// Check if a session exists.
    async fn exists(&self, id: &SessionId) -> Result<bool, SessionStoreError> {
        Ok(self.load(id).await?.is_some())
    }
}
