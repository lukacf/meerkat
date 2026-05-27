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

use crate::session::{SYSTEM_CONTEXT_SEPARATOR, SessionMeta};
use crate::time_compat::SystemTime;
use crate::types::{Message, SessionId};
use crate::{
    Session, TranscriptHistoryState, TranscriptRewriteCommit, TranscriptRewriteSelection,
    transcript_messages_digest,
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
         persisted {prev_len} without transcript-continuity proof"
    )]
    MonotonicityViolation {
        id: SessionId,
        prev_len: usize,
        new_len: usize,
    },

    #[error(
        "session {id} save rejected: incoming transcript is not a continuation of persisted revision {previous_revision}"
    )]
    TranscriptContinuityViolation {
        id: SessionId,
        previous_revision: String,
        incoming_revision: String,
        reason: String,
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
/// message count is strictly smaller than the previously persisted one
/// without a transcript graph edge that proves a core-owned mutation.
///
/// The guard also rejects equal/longer saves whose retained prefix no longer
/// matches the persisted transcript. A plain save may append or update
/// metadata; same-session replacement must go through
/// [`transcript_rewrite_save_guard`].
pub fn append_only_save_guard(
    incoming: &Session,
    previous: Option<&Session>,
) -> Result<(), SessionStoreError> {
    let Some(previous) = previous else {
        return Ok(());
    };

    incoming
        .validate_transcript_history_state()
        .map_err(|err| SessionStoreError::InvalidTranscriptRewrite {
            id: incoming.id().clone(),
            reason: format!("incoming transcript history state is malformed: {err}"),
        })?;
    let previous_had_history = previous
        .transcript_history_state()
        .map_err(|err| SessionStoreError::InvalidTranscriptRewrite {
            id: incoming.id().clone(),
            reason: format!("previous transcript history state is malformed: {err}"),
        })?
        .is_some();
    let incoming_has_history = incoming
        .transcript_history_state()
        .map_err(|err| SessionStoreError::InvalidTranscriptRewrite {
            id: incoming.id().clone(),
            reason: format!("incoming transcript history state is malformed: {err}"),
        })?
        .is_some();
    if previous_had_history && !incoming_has_history {
        return Err(SessionStoreError::InvalidTranscriptRewrite {
            id: incoming.id().clone(),
            reason: "incoming save would erase retained transcript history state".to_string(),
        });
    }

    let previous_revision =
        transcript_messages_digest(previous.messages()).map_err(SessionStoreError::from)?;
    let incoming_revision =
        transcript_messages_digest(incoming.messages()).map_err(SessionStoreError::from)?;
    if previous_revision == incoming_revision {
        return Ok(());
    }

    let prev_len = previous.messages().len();
    let new_len = incoming.messages().len();
    if new_len >= prev_len {
        let incoming_prefix_revision = transcript_messages_digest(&incoming.messages()[..prev_len])
            .map_err(SessionStoreError::from)?;
        if incoming_prefix_revision == previous_revision {
            return Ok(());
        }
    }
    if incoming_preserves_conversation_tail_with_system_context_append(incoming, previous)? {
        return Ok(());
    }
    if incoming_preserves_prefix_after_transient_notice_cleanup(incoming, previous)? {
        return Ok(());
    }
    if new_len < prev_len {
        return Err(SessionStoreError::MonotonicityViolation {
            id: incoming.id().clone(),
            prev_len,
            new_len,
        });
    }

    Err(SessionStoreError::TranscriptContinuityViolation {
        id: incoming.id().clone(),
        previous_revision,
        incoming_revision,
        reason: "incoming transcript neither preserves the persisted prefix nor records a graph edge from the persisted head".to_string(),
    })
}

fn incoming_preserves_conversation_tail_with_system_context_append(
    incoming: &Session,
    previous: &Session,
) -> Result<bool, SessionStoreError> {
    let (previous_system, previous_tail) = split_single_leading_system(previous.messages());
    let (incoming_system, incoming_tail) = split_single_leading_system(incoming.messages());
    let Some(incoming_system) = incoming_system else {
        return Ok(false);
    };
    if !system_context_is_append(previous_system, incoming_system) {
        return Ok(false);
    }
    if incoming_tail.len() < previous_tail.len() {
        return Ok(false);
    }
    let previous_tail_revision =
        transcript_messages_digest(previous_tail).map_err(SessionStoreError::from)?;
    let incoming_tail_prefix_revision =
        transcript_messages_digest(&incoming_tail[..previous_tail.len()])
            .map_err(SessionStoreError::from)?;
    Ok(previous_tail_revision == incoming_tail_prefix_revision)
}

fn split_single_leading_system(messages: &[Message]) -> (Option<&str>, &[Message]) {
    match messages.first() {
        Some(Message::System(system)) => (Some(system.content.as_str()), &messages[1..]),
        _ => (None, messages),
    }
}

fn system_context_is_append(previous: Option<&str>, incoming: &str) -> bool {
    let appended = match previous {
        Some(previous) if incoming == previous => return true,
        Some(previous) if incoming.starts_with(previous) => {
            let appended = &incoming[previous.len()..];
            appended.strip_prefix(SYSTEM_CONTEXT_SEPARATOR)
        }
        Some(_) => None,
        None => Some(incoming),
    };
    appended.is_some_and(|appended| appended.starts_with("[Runtime System Context]"))
}

fn incoming_preserves_prefix_after_transient_notice_cleanup(
    incoming: &Session,
    previous: &Session,
) -> Result<bool, SessionStoreError> {
    let previous_without_transient = previous
        .messages()
        .iter()
        .filter(|message| !is_transient_system_notice(message))
        .cloned()
        .collect::<Vec<_>>();
    if previous_without_transient.len() == previous.messages().len()
        || incoming.messages().len() < previous_without_transient.len()
    {
        return Ok(false);
    }
    let previous_revision =
        transcript_messages_digest(&previous_without_transient).map_err(SessionStoreError::from)?;
    let incoming_prefix_revision =
        transcript_messages_digest(&incoming.messages()[..previous_without_transient.len()])
            .map_err(SessionStoreError::from)?;
    Ok(previous_revision == incoming_prefix_revision)
}

fn is_transient_system_notice(message: &Message) -> bool {
    let Message::SystemNotice(notice) = message else {
        return false;
    };
    notice.kind == crate::types::SystemNoticeKind::McpPending
        && notice.blocks.iter().all(|block| {
            matches!(
                block,
                crate::types::SystemNoticeBlock::Mcp {
                    persisted: false,
                    ..
                }
            )
        })
}

/// Validate a runtime run-boundary snapshot.
///
/// Runtime turns normally append to the transcript, but core-owned turn
/// mechanics such as compaction can also produce an audited internal rewrite.
/// Runtime stores use this guard inside their atomic boundary commit: plain
/// replacement is rejected, while an incoming snapshot carrying a typed rewrite
/// commit from the currently persisted head is accepted through the same
/// rewrite validator as [`SessionStore::save_transcript_rewrite`].
pub fn run_boundary_snapshot_save_guard(
    incoming: &Session,
    previous: Option<&Session>,
) -> Result<(), SessionStoreError> {
    match append_only_save_guard(incoming, previous) {
        Ok(()) => Ok(()),
        Err(append_error) => {
            let Some(previous) = previous else {
                return Err(append_error);
            };
            let previous_revision =
                transcript_messages_digest(previous.messages()).map_err(SessionStoreError::from)?;
            let incoming_revision =
                transcript_messages_digest(incoming.messages()).map_err(SessionStoreError::from)?;
            let Some(state) = incoming.transcript_history_state().map_err(|err| {
                SessionStoreError::InvalidTranscriptRewrite {
                    id: incoming.id().clone(),
                    reason: format!("incoming transcript history state is malformed: {err}"),
                }
            })?
            else {
                return Err(append_error);
            };
            let Some(commits) = find_transcript_rewrite_commit_chain_extending(
                &state,
                &previous_revision,
                &incoming_revision,
            ) else {
                return Err(append_error);
            };
            let Some(commit) = commits.first() else {
                return Err(append_error);
            };
            transcript_rewrite_bridge_save_guard(incoming, commit, &state, &incoming_revision)?;
            for commit in commits.iter().skip(1) {
                validate_transcript_rewrite_commit_bodies(incoming, commit, &state)?;
            }
            Ok(())
        }
    }
}

/// Find the rewrite commit that authorizes replacing `previous_revision`,
/// allowing the incoming head to extend the rewrite via normal append bodies.
pub fn find_transcript_rewrite_commit_extending<'a>(
    state: &'a TranscriptHistoryState,
    previous_revision: &str,
    incoming_revision: &str,
) -> Option<&'a TranscriptRewriteCommit> {
    find_transcript_rewrite_commit_chain_extending(state, previous_revision, incoming_revision)
        .and_then(|commits| commits.into_iter().next())
}

/// Find the contiguous rewrite commits that connect `previous_revision` to the
/// incoming head, allowing normal append bodies after the final rewrite.
pub fn find_transcript_rewrite_commit_chain_extending<'a>(
    state: &'a TranscriptHistoryState,
    previous_revision: &str,
    incoming_revision: &str,
) -> Option<Vec<&'a TranscriptRewriteCommit>> {
    let mut chain = Vec::new();
    let mut cursor = previous_revision;
    let mut visited = std::collections::BTreeSet::new();
    loop {
        if incoming_revision == cursor {
            return Some(chain);
        }
        if !visited.insert(cursor.to_string()) {
            return None;
        }
        let commit = state.commits.iter().find(|commit| {
            (commit.parent_revision == cursor
                || transcript_history_revision_extends(state, &commit.parent_revision, cursor))
                && transcript_history_revision_extends(state, incoming_revision, &commit.revision)
        });
        let Some(commit) = commit else {
            return transcript_history_revision_extends(state, incoming_revision, cursor)
                .then_some(chain);
        };
        cursor = &commit.revision;
        chain.push(commit);
    }
}

fn transcript_history_revision_extends(
    state: &TranscriptHistoryState,
    descendant: &str,
    ancestor: &str,
) -> bool {
    if descendant == ancestor {
        return true;
    }
    let mut cursor = descendant;
    while let Some(body) = state.revisions.iter().find(|body| body.revision == cursor) {
        let Some(parent) = body.parent_revision.as_deref() else {
            return false;
        };
        if parent == ancestor {
            return true;
        }
        cursor = parent;
    }
    false
}

fn transcript_rewrite_bridge_save_guard(
    incoming: &Session,
    commit: &TranscriptRewriteCommit,
    incoming_state: &TranscriptHistoryState,
    incoming_message_digest: &str,
) -> Result<(), SessionStoreError> {
    validate_transcript_rewrite_commit_bodies(incoming, commit, incoming_state)?;
    if incoming_state.head != incoming_message_digest {
        return Err(SessionStoreError::InvalidTranscriptRewrite {
            id: incoming.id().clone(),
            reason: format!(
                "incoming transcript graph head {} does not match current message digest {incoming_message_digest}",
                incoming_state.head
            ),
        });
    }
    if !transcript_history_revision_extends(
        incoming_state,
        incoming_message_digest,
        &commit.revision,
    ) {
        return Err(SessionStoreError::InvalidTranscriptRewrite {
            id: incoming.id().clone(),
            reason: format!(
                "incoming transcript head {incoming_message_digest} does not extend rewrite revision {}",
                commit.revision
            ),
        });
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
    let Some(incoming_state) = incoming.transcript_history_state().map_err(|err| {
        SessionStoreError::InvalidTranscriptRewrite {
            id: incoming.id().clone(),
            reason: format!("incoming transcript history state is malformed: {err}"),
        }
    })?
    else {
        return Err(SessionStoreError::InvalidTranscriptRewrite {
            id: incoming.id().clone(),
            reason: "incoming rewrite did not persist a transcript revision graph".to_string(),
        });
    };
    validate_transcript_rewrite_commit_bodies(incoming, commit, &incoming_state)
}

fn validate_transcript_rewrite_commit_bodies(
    incoming: &Session,
    commit: &TranscriptRewriteCommit,
    incoming_state: &TranscriptHistoryState,
) -> Result<(), SessionStoreError> {
    if !incoming_state
        .commits
        .iter()
        .any(|persisted| persisted == commit)
    {
        return Err(SessionStoreError::InvalidTranscriptRewrite {
            id: incoming.id().clone(),
            reason: format!(
                "incoming rewrite did not persist the rewrite commit in the transcript graph (wanted {} -> {}, graph commits: {:?})",
                commit.parent_revision,
                commit.revision,
                incoming_state
                    .commits
                    .iter()
                    .map(|commit| (&commit.parent_revision, &commit.revision))
                    .collect::<Vec<_>>()
            ),
        });
    }
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
    if parent_body.messages.len() != commit.messages_before
        || revision_body.messages.len() != commit.messages_after
    {
        return Err(SessionStoreError::InvalidTranscriptRewrite {
            id: incoming.id().clone(),
            reason: format!(
                "commit message counts {} -> {} do not match persisted rewrite {} -> {}",
                commit.messages_before,
                commit.messages_after,
                parent_body.messages.len(),
                revision_body.messages.len()
            ),
        });
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{
        AssistantMessage, BlockAssistantMessage, StopReason, SystemMessage, SystemNoticeBlock,
        SystemNoticeKind, SystemNoticeMessage, Usage, UserMessage,
    };

    #[test]
    fn append_only_guard_rejects_leading_system_message_replacement() {
        let mut previous = Session::new();
        previous.push(Message::System(SystemMessage::new("original system")));
        previous.push(Message::User(UserMessage::text("hello".to_string())));

        let mut incoming = previous.clone();
        let rewrite_result = incoming.replace_messages_internal(
            vec![
                Message::System(SystemMessage::new("rewritten system")),
                Message::User(UserMessage::text("hello".to_string())),
            ],
            crate::TranscriptRewriteReason::new("unit-test"),
        );
        assert!(
            rewrite_result.is_ok(),
            "typed rewrite should be constructible: {rewrite_result:?}"
        );

        assert!(matches!(
            append_only_save_guard(&incoming, Some(&previous)),
            Err(SessionStoreError::TranscriptContinuityViolation { .. })
        ));
    }

    #[test]
    fn append_only_guard_accepts_runtime_system_context_append() {
        let mut previous = Session::new();
        previous.push(Message::System(SystemMessage::new("base system")));
        previous.push(Message::User(UserMessage::text("hello".to_string())));

        let mut incoming = previous.clone();
        incoming.set_system_prompt(format!(
            "base system{SYSTEM_CONTEXT_SEPARATOR}[Runtime System Context]\nsource: unit-test\n\nextra context"
        ));

        assert!(append_only_save_guard(&incoming, Some(&previous)).is_ok());
    }

    #[test]
    fn append_only_guard_accepts_system_timestamp_refresh_without_content_change() {
        let mut previous = Session::new();
        previous.push(Message::System(SystemMessage::new("base system")));

        let mut incoming = previous.clone();
        incoming.set_system_prompt("base system".to_string());

        assert!(append_only_save_guard(&incoming, Some(&previous)).is_ok());
    }

    #[test]
    fn append_only_guard_accepts_transient_mcp_pending_notice_cleanup()
    -> Result<(), crate::TranscriptEditError> {
        let mut previous = Session::new();
        previous.push(Message::User(UserMessage::text("hello".to_string())));
        previous.push(Message::SystemNotice(SystemNoticeMessage {
            kind: SystemNoticeKind::McpPending,
            body: Some("connecting".to_string()),
            blocks: vec![SystemNoticeBlock::Mcp {
                server_id: None,
                operation: None,
                phase: None,
                persisted: false,
                detail: Some("connecting".to_string()),
                pending_sources: vec!["test-server".to_string()],
            }],
            created_at: crate::types::message_timestamp_now(),
        }));
        previous.push(Message::BlockAssistant(BlockAssistantMessage::new(
            vec![crate::types::AssistantBlock::Text {
                text: "answer".to_string(),
                meta: None,
            }],
            StopReason::EndTurn,
        )));

        let mut incoming = previous.clone();
        incoming.replace_messages_internal(
            previous
                .messages()
                .iter()
                .filter(|message| !matches!(message, Message::SystemNotice(_)))
                .cloned()
                .collect(),
            crate::TranscriptRewriteReason::new("unit-test"),
        )?;
        incoming.push(Message::User(UserMessage::text("again".to_string())));

        assert!(append_only_save_guard(&incoming, Some(&previous)).is_ok());
        Ok::<(), crate::TranscriptEditError>(())
    }

    #[test]
    fn rewrite_chain_finder_crosses_normal_append_between_rewrites()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut session = Session::new();
        session.push(Message::User(UserMessage::text("first".to_string())));
        session.push(Message::Assistant(AssistantMessage {
            content: "verbose first answer".to_string(),
            tool_calls: Vec::new(),
            stop_reason: StopReason::EndTurn,
            usage: Usage::default(),
            created_at: crate::types::message_timestamp_now(),
        }));

        let original = session.transcript_revision()?;
        let first = session.commit_transcript_rewrite(
            TranscriptRewriteSelection::MessageRange { start: 1, end: 2 },
            vec![Message::Assistant(AssistantMessage {
                content: "compact first answer".to_string(),
                tool_calls: Vec::new(),
                stop_reason: StopReason::EndTurn,
                usage: Usage::default(),
                created_at: crate::types::message_timestamp_now(),
            })],
            crate::TranscriptRewriteReason::new("compaction"),
            Some("unit-test".to_string()),
            Some(original.clone()),
        )?;

        session.push(Message::User(UserMessage::text("second".to_string())));
        session.push(Message::Assistant(AssistantMessage {
            content: "verbose second answer".to_string(),
            tool_calls: Vec::new(),
            stop_reason: StopReason::EndTurn,
            usage: Usage::default(),
            created_at: crate::types::message_timestamp_now(),
        }));
        let bridge = session.transcript_revision()?;
        assert_ne!(bridge, first.revision);

        let second = session.commit_transcript_rewrite(
            TranscriptRewriteSelection::MessageRange { start: 3, end: 4 },
            vec![Message::Assistant(AssistantMessage {
                content: "compact second answer".to_string(),
                tool_calls: Vec::new(),
                stop_reason: StopReason::EndTurn,
                usage: Usage::default(),
                created_at: crate::types::message_timestamp_now(),
            })],
            crate::TranscriptRewriteReason::new("compaction"),
            Some("unit-test".to_string()),
            Some(bridge),
        )?;
        let state = session
            .transcript_history_state()?
            .ok_or_else(|| std::io::Error::other("missing transcript history state"))?;

        let chain =
            find_transcript_rewrite_commit_chain_extending(&state, &original, &second.revision)
                .ok_or_else(|| {
                    std::io::Error::other(
                        "rewrite chain should extend through normal append bridge",
                    )
                })?;
        assert_eq!(chain.len(), 2);
        assert_eq!(chain[0].revision, first.revision);
        assert_eq!(chain[1].revision, second.revision);
        Ok(())
    }
}
