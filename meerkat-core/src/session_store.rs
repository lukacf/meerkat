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

use crate::session::SessionMeta;
use crate::time_compat::SystemTime;
use crate::types::SessionId;
use crate::{
    Session, TranscriptRewriteCommit, TranscriptRewriteSelection, transcript_messages_digest,
};

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

    #[error(
        "session {id} rewrite rejected: previous transcript revision {actual} did not match commit parent {expected}"
    )]
    TranscriptRevisionConflict {
        id: SessionId,
        expected: String,
        actual: String,
    },

    #[error("session {id} rewrite rejected: {reason}")]
    InvalidTranscriptRewrite { id: SessionId, reason: String },

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

/// Validate that a same-session shrink/replace save is backed by a typed
/// transcript rewrite commit.
pub fn transcript_rewrite_save_guard(
    incoming: &Session,
    previous: Option<&Session>,
    commit: &TranscriptRewriteCommit,
) -> Result<(), SessionStoreError> {
    let Some(previous) = previous else {
        return Err(SessionStoreError::InvalidTranscriptRewrite {
            id: incoming.id().clone(),
            reason: "rewrite target has no previously persisted session".to_string(),
        });
    };
    if incoming.id() != previous.id() {
        return Err(SessionStoreError::InvalidTranscriptRewrite {
            id: incoming.id().clone(),
            reason: format!(
                "incoming session id {} differs from previous session id {}",
                incoming.id(),
                previous.id()
            ),
        });
    }
    let previous_revision = previous.transcript_revision().map_err(|err| {
        SessionStoreError::InvalidTranscriptRewrite {
            id: incoming.id().clone(),
            reason: format!("previous transcript revision is malformed: {err}"),
        }
    })?;
    if previous_revision != commit.parent_revision {
        return Err(SessionStoreError::TranscriptRevisionConflict {
            id: incoming.id().clone(),
            expected: commit.parent_revision.clone(),
            actual: previous_revision,
        });
    }
    let previous_message_digest =
        transcript_messages_digest(previous.messages()).map_err(|err| {
            SessionStoreError::InvalidTranscriptRewrite {
                id: incoming.id().clone(),
                reason: format!("previous current transcript is not digestible: {err}"),
            }
        })?;
    if previous_message_digest != commit.parent_revision {
        return Err(SessionStoreError::InvalidTranscriptRewrite {
            id: incoming.id().clone(),
            reason: format!(
                "previous current transcript digest {previous_message_digest} does not match commit parent {}",
                commit.parent_revision
            ),
        });
    }
    let incoming_revision = incoming.transcript_revision().map_err(|err| {
        SessionStoreError::InvalidTranscriptRewrite {
            id: incoming.id().clone(),
            reason: format!("incoming transcript revision is malformed: {err}"),
        }
    })?;
    if incoming_revision != commit.revision {
        return Err(SessionStoreError::InvalidTranscriptRewrite {
            id: incoming.id().clone(),
            reason: format!(
                "incoming transcript revision {incoming_revision} does not match commit revision {}",
                commit.revision
            ),
        });
    }
    let incoming_message_digest =
        transcript_messages_digest(incoming.messages()).map_err(|err| {
            SessionStoreError::InvalidTranscriptRewrite {
                id: incoming.id().clone(),
                reason: format!("incoming current transcript is not digestible: {err}"),
            }
        })?;
    if incoming_message_digest != commit.revision {
        return Err(SessionStoreError::InvalidTranscriptRewrite {
            id: incoming.id().clone(),
            reason: format!(
                "incoming current transcript digest {incoming_message_digest} does not match commit revision {}",
                commit.revision
            ),
        });
    }
    if previous.messages().len() != commit.messages_before
        || incoming.messages().len() != commit.messages_after
    {
        return Err(SessionStoreError::InvalidTranscriptRewrite {
            id: incoming.id().clone(),
            reason: format!(
                "commit message counts {} -> {} do not match persisted rewrite {} -> {}",
                commit.messages_before,
                commit.messages_after,
                previous.messages().len(),
                incoming.messages().len()
            ),
        });
    }
    let incoming_state = incoming.transcript_history_state().map_err(|err| {
        SessionStoreError::InvalidTranscriptRewrite {
            id: incoming.id().clone(),
            reason: format!("incoming transcript history state is malformed: {err}"),
        }
    })?;
    let Some(incoming_state) = incoming_state else {
        return Err(SessionStoreError::InvalidTranscriptRewrite {
            id: incoming.id().clone(),
            reason: "incoming rewrite did not persist a transcript revision graph".to_string(),
        });
    };
    let Some(parent_body) = incoming_state
        .revisions
        .iter()
        .find(|body| body.revision == commit.parent_revision)
    else {
        return Err(SessionStoreError::InvalidTranscriptRewrite {
            id: incoming.id().clone(),
            reason: format!(
                "incoming rewrite omitted parent revision body {}",
                commit.parent_revision
            ),
        });
    };
    let Some(revision_body) = incoming_state
        .revisions
        .iter()
        .find(|body| body.revision == commit.revision)
    else {
        return Err(SessionStoreError::InvalidTranscriptRewrite {
            id: incoming.id().clone(),
            reason: format!(
                "incoming rewrite omitted new revision body {}",
                commit.revision
            ),
        });
    };
    let parent_body_revision =
        transcript_messages_digest(&parent_body.messages).map_err(|err| {
            SessionStoreError::InvalidTranscriptRewrite {
                id: incoming.id().clone(),
                reason: format!("parent revision body is not digestible: {err}"),
            }
        })?;
    if parent_body_revision != commit.parent_revision {
        return Err(SessionStoreError::InvalidTranscriptRewrite {
            id: incoming.id().clone(),
            reason: format!(
                "parent revision body digest {parent_body_revision} does not match commit parent {}",
                commit.parent_revision
            ),
        });
    }
    let (start, end) = match &commit.selection {
        TranscriptRewriteSelection::MessageRange { start, end } => (*start, *end),
    };
    if start > end || end > parent_body.messages.len() {
        return Err(SessionStoreError::InvalidTranscriptRewrite {
            id: incoming.id().clone(),
            reason: format!(
                "commit selection {start}..{end} is invalid for parent revision with {} messages",
                parent_body.messages.len()
            ),
        });
    }
    let original_span_digest = transcript_messages_digest(&parent_body.messages[start..end])
        .map_err(|err| SessionStoreError::InvalidTranscriptRewrite {
            id: incoming.id().clone(),
            reason: format!("original span body is not digestible: {err}"),
        })?;
    if original_span_digest != commit.original_span_digest {
        return Err(SessionStoreError::InvalidTranscriptRewrite {
            id: incoming.id().clone(),
            reason: format!(
                "original span digest {original_span_digest} does not match commit digest {}",
                commit.original_span_digest
            ),
        });
    }
    let revision_body_digest =
        transcript_messages_digest(&revision_body.messages).map_err(|err| {
            SessionStoreError::InvalidTranscriptRewrite {
                id: incoming.id().clone(),
                reason: format!("new revision body is not digestible: {err}"),
            }
        })?;
    if revision_body_digest != commit.revision {
        return Err(SessionStoreError::InvalidTranscriptRewrite {
            id: incoming.id().clone(),
            reason: format!(
                "new revision body digest {revision_body_digest} does not match commit revision {}",
                commit.revision
            ),
        });
    }
    let removed_len = end - start;
    let retained_len = commit
        .messages_before
        .checked_sub(removed_len)
        .ok_or_else(|| SessionStoreError::InvalidTranscriptRewrite {
            id: incoming.id().clone(),
            reason: "commit removed more messages than it recorded before rewrite".to_string(),
        })?;
    let replacement_len = commit
        .messages_after
        .checked_sub(retained_len)
        .ok_or_else(|| SessionStoreError::InvalidTranscriptRewrite {
            id: incoming.id().clone(),
            reason: "commit message counts cannot describe a replacement span".to_string(),
        })?;
    let replacement_end = start.checked_add(replacement_len).ok_or_else(|| {
        SessionStoreError::InvalidTranscriptRewrite {
            id: incoming.id().clone(),
            reason: "replacement span end overflowed".to_string(),
        }
    })?;
    if replacement_end > revision_body.messages.len() {
        return Err(SessionStoreError::InvalidTranscriptRewrite {
            id: incoming.id().clone(),
            reason: format!(
                "replacement span {start}..{replacement_end} is invalid for revision with {} messages",
                revision_body.messages.len()
            ),
        });
    }
    let parent_prefix_digest =
        transcript_messages_digest(&parent_body.messages[..start]).map_err(|err| {
            SessionStoreError::InvalidTranscriptRewrite {
                id: incoming.id().clone(),
                reason: format!("parent prefix body is not digestible: {err}"),
            }
        })?;
    let revision_prefix_digest = transcript_messages_digest(&revision_body.messages[..start])
        .map_err(|err| SessionStoreError::InvalidTranscriptRewrite {
            id: incoming.id().clone(),
            reason: format!("revision prefix body is not digestible: {err}"),
        })?;
    if parent_prefix_digest != revision_prefix_digest {
        return Err(SessionStoreError::InvalidTranscriptRewrite {
            id: incoming.id().clone(),
            reason: "rewrite revision changed messages before the selected span".to_string(),
        });
    }
    let parent_suffix_digest =
        transcript_messages_digest(&parent_body.messages[end..]).map_err(|err| {
            SessionStoreError::InvalidTranscriptRewrite {
                id: incoming.id().clone(),
                reason: format!("parent suffix body is not digestible: {err}"),
            }
        })?;
    let revision_suffix_digest =
        transcript_messages_digest(&revision_body.messages[replacement_end..]).map_err(|err| {
            SessionStoreError::InvalidTranscriptRewrite {
                id: incoming.id().clone(),
                reason: format!("revision suffix body is not digestible: {err}"),
            }
        })?;
    if parent_suffix_digest != revision_suffix_digest {
        return Err(SessionStoreError::InvalidTranscriptRewrite {
            id: incoming.id().clone(),
            reason: "rewrite revision changed messages after the selected span".to_string(),
        });
    }
    let replacement_digest = transcript_messages_digest(
        &revision_body.messages[start..replacement_end],
    )
    .map_err(|err| SessionStoreError::InvalidTranscriptRewrite {
        id: incoming.id().clone(),
        reason: format!("replacement span body is not digestible: {err}"),
    })?;
    if replacement_digest != commit.replacement_digest {
        return Err(SessionStoreError::InvalidTranscriptRewrite {
            id: incoming.id().clone(),
            reason: format!(
                "replacement span digest {replacement_digest} does not match commit digest {}",
                commit.replacement_digest
            ),
        });
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

    /// Save a same-SessionId transcript rewrite.
    ///
    /// This is the only `SessionStore` path allowed to replace or shrink the
    /// current message projection. Implementations must validate `commit`
    /// against the previously persisted head before writing `session`.
    async fn save_transcript_rewrite(
        &self,
        session: &Session,
        commit: &TranscriptRewriteCommit,
    ) -> Result<(), SessionStoreError> {
        let _ = (session, commit);
        Err(SessionStoreError::Internal(
            "save_transcript_rewrite is not supported by this SessionStore".to_string(),
        ))
    }

    /// Save a compatibility projection after a separate authority has already
    /// committed the session snapshot.
    ///
    /// This method is for runtime-backed services only: the runtime snapshot
    /// has already accepted the semantic mutation, and the `SessionStore` row is
    /// a rebuildable projection. Normal callers must use [`SessionStore::save`]
    /// or [`SessionStore::save_transcript_rewrite`] so the store boundary keeps
    /// enforcing append-only/CAS semantics.
    async fn save_authoritative_projection(
        &self,
        session: &Session,
    ) -> Result<(), SessionStoreError> {
        self.save(session).await
    }

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
