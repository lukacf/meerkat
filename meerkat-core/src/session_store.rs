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
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;

#[cfg(test)]
use crate::TranscriptRewriteSelection;
use crate::session::{
    SESSION_TRANSCRIPT_HISTORY_STATE_KEY, SYSTEM_CONTEXT_SEPARATOR, SessionMeta,
    TranscriptRevisionBody,
};
use crate::time_compat::SystemTime;
use crate::types::{Message, SessionId, SystemMessage, Usage};
use crate::{
    Session, TranscriptHistoryState, TranscriptRewriteCommit, TranscriptRewriteRecord,
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

/// Stable compare token for a full persisted session projection row.
pub fn session_projection_cas_token(session: &Session) -> Result<String, SessionStoreError> {
    let bytes = serde_json::to_vec(session).map_err(|err| {
        SessionStoreError::Serialization(format!(
            "failed to serialize session projection CAS token: {err}"
        ))
    })?;
    Ok(format!("row-sha256:{:x}", Sha256::digest(bytes)))
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
    incoming
        .validate_transcript_history_state()
        .map_err(|err| SessionStoreError::InvalidTranscriptRewrite {
            id: incoming.id().clone(),
            reason: format!("incoming transcript history state is malformed: {err}"),
        })?;
    let incoming_revision =
        transcript_messages_digest(incoming.messages()).map_err(SessionStoreError::from)?;
    let incoming_state = incoming.transcript_history_state().map_err(|err| {
        SessionStoreError::InvalidTranscriptRewrite {
            id: incoming.id().clone(),
            reason: format!("incoming transcript history state is malformed: {err}"),
        }
    })?;
    if let Some(state) = incoming_state.as_ref()
        && state.head != incoming_revision
    {
        return Err(SessionStoreError::InvalidTranscriptRewrite {
            id: incoming.id().clone(),
            reason: format!(
                "incoming transcript graph head {} does not match current message digest {incoming_revision}",
                state.head
            ),
        });
    }

    let Some(previous) = previous else {
        if incoming_state.is_some() {
            return Err(SessionStoreError::InvalidTranscriptRewrite {
                id: incoming.id().clone(),
                reason: "incoming first save would seed transcript history state outside the rewrite/audit path"
                    .to_string(),
            });
        }
        validate_plain_save_transcript_history_preservation(
            incoming,
            None,
            None,
            incoming_state.as_ref(),
        )?;
        return Ok(());
    };
    let previous_state = previous.transcript_history_state().map_err(|err| {
        SessionStoreError::InvalidTranscriptRewrite {
            id: incoming.id().clone(),
            reason: format!("previous transcript history state is malformed: {err}"),
        }
    })?;
    let previous_had_history = previous_state.is_some();
    let incoming_has_history = incoming_state.is_some();
    if previous_had_history && !incoming_has_history {
        return Err(SessionStoreError::InvalidTranscriptRewrite {
            id: incoming.id().clone(),
            reason: "incoming save would erase retained transcript history state".to_string(),
        });
    }
    let previous_revision =
        transcript_messages_digest(previous.messages()).map_err(SessionStoreError::from)?;
    if previous_revision == incoming_revision {
        validate_plain_save_transcript_history_preservation(
            incoming,
            Some(previous),
            previous_state.as_ref(),
            incoming_state.as_ref(),
        )?;
        return Ok(());
    }

    let prev_len = previous.messages().len();
    let new_len = incoming.messages().len();
    if new_len >= prev_len {
        let incoming_prefix_revision = transcript_messages_digest(&incoming.messages()[..prev_len])
            .map_err(SessionStoreError::from)?;
        if incoming_prefix_revision == previous_revision {
            validate_plain_save_transcript_history_preservation(
                incoming,
                Some(previous),
                previous_state.as_ref(),
                incoming_state.as_ref(),
            )?;
            return Ok(());
        }
    }
    if incoming_preserves_conversation_tail_with_system_context_append(incoming, previous)? {
        validate_plain_save_transcript_history_preservation(
            incoming,
            Some(previous),
            previous_state.as_ref(),
            incoming_state.as_ref(),
        )?;
        return Ok(());
    }
    if incoming_preserves_prefix_after_synthetic_notice_refresh(incoming, previous)? {
        validate_plain_save_transcript_history_preservation(
            incoming,
            Some(previous),
            previous_state.as_ref(),
            incoming_state.as_ref(),
        )?;
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

fn validate_plain_save_transcript_history_preservation(
    incoming: &Session,
    previous: Option<&Session>,
    previous_state: Option<&TranscriptHistoryState>,
    incoming_state: Option<&TranscriptHistoryState>,
) -> Result<(), SessionStoreError> {
    let Some(previous) = previous else {
        if incoming_state.is_some() {
            return Err(SessionStoreError::InvalidTranscriptRewrite {
                id: incoming.id().clone(),
                reason: "incoming first save would seed transcript history state outside the rewrite/audit path"
                    .to_string(),
            });
        }
        return Ok(());
    };
    if previous_state.is_none() && incoming_state.is_some() {
        return Err(SessionStoreError::InvalidTranscriptRewrite {
            id: incoming.id().clone(),
            reason: "incoming append-only save would seed transcript history state outside the rewrite/audit path"
                .to_string(),
        });
    }
    let Some(previous_state) = previous_state else {
        return Ok(());
    };
    let Some(incoming_state) = incoming_state else {
        return Err(SessionStoreError::InvalidTranscriptRewrite {
            id: incoming.id().clone(),
            reason: "incoming append-only save would erase retained transcript history state"
                .to_string(),
        });
    };
    let previous_commits = previous_state.commits.as_slice();
    let incoming_commits = incoming_state.commits.as_slice();
    if incoming_commits != previous_commits {
        return Err(SessionStoreError::InvalidTranscriptRewrite {
            id: incoming.id().clone(),
            reason: "incoming append-only save would change retained transcript rewrite commits"
                .to_string(),
        });
    }
    incoming
        .validate_transcript_history_state()
        .map_err(|err| SessionStoreError::InvalidTranscriptRewrite {
            id: incoming.id().clone(),
            reason: format!("incoming transcript history state is malformed: {err}"),
        })?;
    let incoming_revision =
        transcript_messages_digest(incoming.messages()).map_err(SessionStoreError::from)?;
    let previous_revision =
        transcript_messages_digest(previous.messages()).map_err(SessionStoreError::from)?;
    if previous_state.head != previous_revision {
        return Err(SessionStoreError::InvalidTranscriptRewrite {
            id: incoming.id().clone(),
            reason: "previous transcript history head does not match persisted message digest"
                .to_string(),
        });
    }
    if incoming_state.head != incoming_revision {
        return Err(SessionStoreError::InvalidTranscriptRewrite {
            id: incoming.id().clone(),
            reason: "incoming append-only save history head does not match the current transcript"
                .to_string(),
        });
    }

    let mut canonical_revisions = std::collections::BTreeSet::from([incoming_state.head.clone()]);
    for commit in &incoming_state.commits {
        canonical_revisions.insert(commit.parent_revision.clone());
        canonical_revisions.insert(commit.revision.clone());
    }
    let mut seen = std::collections::BTreeSet::new();
    if incoming_state.revisions.iter().any(|body| {
        !canonical_revisions.contains(&body.revision) || !seen.insert(body.revision.clone())
    }) {
        return Err(SessionStoreError::InvalidTranscriptRewrite {
            id: incoming.id().clone(),
            reason: "incoming append-only save carries non-canonical mechanical revision bodies"
                .to_string(),
        });
    }

    validate_audited_revision_bodies_preserved(incoming, previous_state, incoming_state)
}

fn validate_audited_revision_bodies_preserved(
    incoming: &Session,
    previous_state: &TranscriptHistoryState,
    incoming_state: &TranscriptHistoryState,
) -> Result<(), SessionStoreError> {
    let mut audited_revisions = std::collections::BTreeSet::new();
    for commit in &previous_state.commits {
        audited_revisions.insert(commit.parent_revision.as_str());
        audited_revisions.insert(commit.revision.as_str());
    }
    for revision in audited_revisions {
        let previous_body = previous_state
            .revisions
            .iter()
            .find(|body| body.revision == revision)
            .ok_or_else(|| SessionStoreError::InvalidTranscriptRewrite {
                id: incoming.id().clone(),
                reason: format!("previous transcript history omits audited body {revision}"),
            })?;
        let incoming_body = incoming_state
            .revisions
            .iter()
            .find(|body| body.revision == revision)
            .ok_or_else(|| SessionStoreError::InvalidTranscriptRewrite {
                id: incoming.id().clone(),
                reason: format!("incoming append-only save drops audited body {revision}"),
            })?;
        if previous_body.parent_revision != incoming_body.parent_revision
            || previous_body.created_at != incoming_body.created_at
            || transcript_messages_digest(&previous_body.messages)
                .map_err(SessionStoreError::from)?
                != transcript_messages_digest(&incoming_body.messages)
                    .map_err(SessionStoreError::from)?
        {
            return Err(SessionStoreError::InvalidTranscriptRewrite {
                id: incoming.id().clone(),
                reason: format!(
                    "incoming append-only save changes audited transcript body {revision}"
                ),
            });
        }
    }
    Ok(())
}

fn validate_rewrite_save_retains_previous_commits(
    incoming: &Session,
    previous: &Session,
    incoming_state: &TranscriptHistoryState,
) -> Result<(), SessionStoreError> {
    let previous_state = previous.transcript_history_state().map_err(|err| {
        SessionStoreError::InvalidTranscriptRewrite {
            id: incoming.id().clone(),
            reason: format!("previous transcript history state is malformed: {err}"),
        }
    })?;
    let Some(previous_state) = previous_state.as_ref() else {
        return Ok(());
    };
    if incoming_state.commits.len() < previous_state.commits.len()
        || incoming_state.commits[..previous_state.commits.len()] != previous_state.commits
    {
        return Err(SessionStoreError::InvalidTranscriptRewrite {
            id: incoming.id().clone(),
            reason: "incoming rewrite save would drop retained transcript rewrite commits"
                .to_string(),
        });
    }
    validate_audited_revision_bodies_preserved(incoming, previous_state, incoming_state)
}

/// Validate that an authoritative projection write still targets the row that
/// the caller proved continuity against.
pub fn authoritative_projection_current_revision_guard(
    incoming: &Session,
    previous: Option<&Session>,
    expected_current_revision: Option<&str>,
) -> Result<(), SessionStoreError> {
    let previous_token = previous.map(session_projection_cas_token).transpose()?;
    if previous_token.as_deref() == expected_current_revision {
        return Ok(());
    }
    let incoming_revision =
        transcript_messages_digest(incoming.messages()).map_err(SessionStoreError::from)?;
    Err(SessionStoreError::TranscriptContinuityViolation {
        id: incoming.id().clone(),
        previous_revision: previous_token.unwrap_or_else(|| "<missing>".to_string()),
        incoming_revision,
        reason: format!(
            "authoritative projection expected persisted projection token {}, but current row has diverged",
            expected_current_revision.unwrap_or("<missing>")
        ),
    })
}

fn incoming_preserves_conversation_tail_with_system_context_append(
    incoming: &Session,
    previous: &Session,
) -> Result<bool, SessionStoreError> {
    messages_preserve_conversation_tail_with_system_context_append(
        incoming.messages(),
        previous.messages(),
    )
}

fn messages_preserve_conversation_tail_with_system_context_append(
    incoming: &[Message],
    previous: &[Message],
) -> Result<bool, SessionStoreError> {
    let (previous_system, previous_tail) = split_single_leading_system(previous);
    let (incoming_system, incoming_tail) = split_single_leading_system(incoming);
    let Some(incoming_system) = incoming_system else {
        return Ok(false);
    };
    if !system_context_is_append(previous_system, incoming_system)? {
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

fn split_single_leading_system(messages: &[Message]) -> (Option<&SystemMessage>, &[Message]) {
    match messages.first() {
        Some(Message::System(system)) => (Some(system), &messages[1..]),
        _ => (None, messages),
    }
}

/// Decide whether `incoming` is a continuation of `previous` produced by a
/// runtime system-context append.
///
/// The structural part — identical content, or `incoming = previous +
/// separator + suffix` — is a transcript-continuity proof (content equality of
/// the retained prefix), not classification. The SEMANTIC append-admission
/// verdict ("is this incoming persisted prompt an admissible
/// runtime-context-append continuation of the persisted one") is owned by the
/// canonical [`SessionDocumentMachine`] — the same machine the staging path
/// already drives for the four-way append disposition — not a handwritten shell
/// reducer. This function extracts only the pure structural observations plus
/// the typed [`SystemPromptMutationKind`] runtime-context-append marker, drives
/// the machine's `ResolveSystemContextPersistAppendAdmission` input, and mirrors
/// the emitted verdict (`Admit` -> `true`, `Reject` -> `false`). It fails closed
/// if the machine refuses or emits no verdict.
fn system_context_is_append(
    previous: Option<&SystemMessage>,
    incoming: &SystemMessage,
) -> Result<bool, SessionStoreError> {
    // Pure structural observations the shell computes; NO semantic decision.
    let has_previous = previous.is_some();
    let content_identical = previous.is_some_and(|previous| incoming.content == previous.content);
    let content_extends_previous =
        previous.is_some_and(|previous| incoming.content.starts_with(&previous.content));
    let appended_starts_with_separator = previous.is_some_and(|previous| {
        incoming
            .content
            .get(previous.content.len()..)
            .is_some_and(|appended| appended.starts_with(SYSTEM_CONTEXT_SEPARATOR))
    });
    let incoming_is_runtime_context_append = incoming.mutation_kind.is_runtime_context_append();

    let mut authority = crate::session_document::SessionDocumentMachineAuthority::new();
    let effects = authority
        .resolve_system_context_persist_append_admission(
            has_previous,
            content_identical,
            content_extends_previous,
            appended_starts_with_separator,
            incoming_is_runtime_context_append,
        )
        .map_err(|err| {
            SessionStoreError::Internal(format!(
                "session document authority refused persist-time system-context append admission: {err}"
            ))
        })?;
    effects
        .into_iter()
        .find_map(|effect| match effect {
            crate::session_document::SessionDocumentEffect::SystemContextPersistAppendAdmissionResolved {
                admission,
            } => Some(matches!(
                admission,
                crate::session_document::SystemContextPersistAppendAdmission::Admit
            )),
            _ => None,
        })
        .ok_or_else(|| {
            SessionStoreError::Internal(
                "session document authority emitted no persist-time system-context append admission verdict".to_string(),
            )
        })
}

fn incoming_preserves_prefix_after_synthetic_notice_refresh(
    incoming: &Session,
    previous: &Session,
) -> Result<bool, SessionStoreError> {
    let previous_without_synthetic = previous
        .messages()
        .iter()
        .filter(|message| !is_synthetic_refresh_projection(message))
        .cloned()
        .collect::<Vec<_>>();
    if previous_without_synthetic.len() == previous.messages().len() {
        return Ok(false);
    }
    let incoming_without_synthetic = incoming
        .messages()
        .iter()
        .filter(|message| !is_synthetic_refresh_projection(message))
        .cloned()
        .collect::<Vec<_>>();
    if incoming_without_synthetic.len() < previous_without_synthetic.len() {
        return Ok(false);
    }
    let previous_revision =
        transcript_messages_digest(&previous_without_synthetic).map_err(SessionStoreError::from)?;
    let incoming_prefix_revision =
        transcript_messages_digest(&incoming_without_synthetic[..previous_without_synthetic.len()])
            .map_err(SessionStoreError::from)?;
    Ok(previous_revision == incoming_prefix_revision)
}

fn is_synthetic_refresh_projection(message: &Message) -> bool {
    let Message::SystemNotice(notice) = message else {
        return false;
    };
    notice.is_synthetic_refresh_projection()
}

/// Validate a runtime run-boundary snapshot.
///
/// Runtime turns normally append to the transcript, but core-owned turn
/// mechanics such as compaction can also produce an audited internal rewrite.
/// Runtime stores use this guard inside their atomic boundary commit: plain
/// replacement is rejected, while an incoming snapshot carrying a typed rewrite
/// commit from the currently persisted head is accepted through the same
/// rewrite validator as [`SessionStore::save_transcript_rewrite`].
///
/// Runtime stores that have independently proved exact persisted-byte identity
/// can use [`run_boundary_snapshot_head_coherence_guard`] instead: byte
/// identity proves continuity and retained-history preservation, leaving only
/// the incoming graph/live-message coherence invariant to check.
pub fn run_boundary_snapshot_save_guard(
    incoming: &Session,
    previous: Option<&Session>,
) -> Result<(), SessionStoreError> {
    match append_only_save_guard(incoming, previous) {
        Ok(()) => Ok(()),
        Err(append_error) => {
            if run_boundary_commitless_history_projection_save_guard(incoming, previous)? {
                return Ok(());
            }
            let Some(previous) = previous else {
                // First runtime-boundary commit for a session this authority
                // has never snapshotted: adoption of a resumed/imported
                // session. A typed rewrite graph carried in is audited by its
                // own commits — validate every one against its retained
                // bodies and require the graph head to match the incoming
                // digest. Plain `SessionStore::save` keeps rejecting such
                // seeds (the trait-level append-only contract); adoption is a
                // runtime-authority decision, not an ordinary row write.
                let incoming_revision = transcript_messages_digest(incoming.messages())
                    .map_err(SessionStoreError::from)?;
                if let Some(state) = incoming.transcript_history_state().map_err(|err| {
                    SessionStoreError::InvalidTranscriptRewrite {
                        id: incoming.id().clone(),
                        reason: format!("incoming transcript history state is malformed: {err}"),
                    }
                })? && !state.commits.is_empty()
                    && state.head == incoming_revision
                {
                    for commit in &state.commits {
                        validate_transcript_rewrite_commit_bodies(incoming, commit, &state)?;
                    }
                    return Ok(());
                }
                return Err(append_error);
            };
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
            // append_only_save_guard's digest validation of the incoming
            // history state was discarded with its error above; a
            // digest-inconsistent witness body must not be able to prove a
            // fork as a plain append on this branch either.
            incoming
                .validate_transcript_history_state()
                .map_err(|err| SessionStoreError::InvalidTranscriptRewrite {
                    id: incoming.id().clone(),
                    reason: format!("incoming transcript history state is malformed: {err}"),
                })?;
            validate_rewrite_save_retains_previous_commits(incoming, previous, &state)?;
            let commits = find_transcript_rewrite_commit_chain_extending_session(
                &state,
                previous,
                &incoming_revision,
            )?;
            if commits.is_none()
                && run_boundary_context_summary_tail_projection_save_guard(
                    incoming, previous, &state,
                )?
            {
                return Ok(());
            }
            let Some(commits) = commits else {
                return Err(append_error);
            };
            let Some(commit) = commits.first() else {
                if state.commits.is_empty() {
                    return Err(append_error);
                }
                // Empty chain: the persisted row is already at (or past) the
                // last rewrite and the incoming head extends it by plain
                // appends. Unlike the non-empty chain below, no bridge guard
                // runs here, so re-check the graph-head/message-digest
                // agreement explicitly before accepting.
                if state.head != incoming_revision {
                    return Err(SessionStoreError::InvalidTranscriptRewrite {
                        id: incoming.id().clone(),
                        reason: format!(
                            "incoming transcript graph head {} does not match current message digest {incoming_revision}",
                            state.head
                        ),
                    });
                }
                for commit in &state.commits {
                    validate_transcript_rewrite_commit_bodies(incoming, commit, &state)?;
                }
                return Ok(());
            };
            transcript_rewrite_bridge_save_guard(incoming, commit, &state, &incoming_revision)?;
            // Validate every retained commit's recorded bodies, not only the
            // walked chain: a plain-continuation proof can legitimately end
            // the walk before trailing rebookkept commits, and those must
            // stay digest-consistent to ride along (mirrors the empty-chain
            // arm above).
            for commit in &state.commits {
                validate_transcript_rewrite_commit_bodies(incoming, commit, &state)?;
            }
            Ok(())
        }
    }
}

/// Validate the invariant that a typed Session's live transcript matches its
/// retained graph head, without materializing the full history document.
///
/// This is the narrow residual guard for an exact-byte replay of an already
/// persisted runtime snapshot. It does not itself prove byte identity; callers
/// must establish that against their canonical stored row before using it in
/// place of [`run_boundary_snapshot_save_guard`]. Session deserialization (or
/// an invalidated in-memory cache) has already validated every retained graph
/// body, so only one live-transcript digest remains necessary here.
pub fn run_boundary_snapshot_head_coherence_guard(
    incoming: &Session,
) -> Result<(), SessionStoreError> {
    let Some(head) = incoming
        .validated_transcript_history_head()
        .map_err(|err| SessionStoreError::InvalidTranscriptRewrite {
            id: incoming.id().clone(),
            reason: format!("incoming transcript history state is malformed: {err}"),
        })?
    else {
        return Ok(());
    };
    let incoming_revision =
        transcript_messages_digest(incoming.messages()).map_err(SessionStoreError::from)?;
    if head != incoming_revision {
        return Err(SessionStoreError::InvalidTranscriptRewrite {
            id: incoming.id().clone(),
            reason: format!(
                "incoming transcript graph head {head} does not match current message digest {incoming_revision}"
            ),
        });
    }
    Ok(())
}

fn run_boundary_commitless_history_projection_save_guard(
    incoming: &Session,
    previous: Option<&Session>,
) -> Result<bool, SessionStoreError> {
    let Some(state) = incoming.transcript_history_state().map_err(|err| {
        SessionStoreError::InvalidTranscriptRewrite {
            id: incoming.id().clone(),
            reason: format!("incoming transcript history state is malformed: {err}"),
        }
    })?
    else {
        return Ok(false);
    };
    if !state.commits.is_empty() {
        return Ok(false);
    }

    let incoming_revision =
        transcript_messages_digest(incoming.messages()).map_err(SessionStoreError::from)?;
    if state.head != incoming_revision
        || !state
            .revisions
            .iter()
            .any(|body| body.revision == incoming_revision)
    {
        return Ok(false);
    }

    let mut projection_without_history = incoming.clone();
    projection_without_history.clear_transcript_history_state();
    if append_only_save_guard(&projection_without_history, previous).is_err() {
        return Ok(false);
    }

    let Some(previous) = previous else {
        return Ok(state.commits.is_empty());
    };
    if previous
        .transcript_history_state()
        .map_err(|err| SessionStoreError::InvalidTranscriptRewrite {
            id: incoming.id().clone(),
            reason: format!("previous transcript history state is malformed: {err}"),
        })?
        .is_some()
    {
        return Ok(false);
    }

    let previous_revision =
        transcript_messages_digest(previous.messages()).map_err(SessionStoreError::from)?;
    Ok(incoming_revision == previous_revision
        || transcript_history_revision_extends(&state, &incoming_revision, &previous_revision))
}

fn run_boundary_context_summary_tail_projection_save_guard(
    incoming: &Session,
    previous: &Session,
    state: &TranscriptHistoryState,
) -> Result<bool, SessionStoreError> {
    if state.commits.is_empty() {
        return Ok(false);
    }
    incoming
        .validate_transcript_history_state()
        .map_err(|err| SessionStoreError::InvalidTranscriptRewrite {
            id: incoming.id().clone(),
            reason: format!("incoming transcript history state is malformed: {err}"),
        })?;

    let (incoming_system, incoming_tail) = match incoming.messages().split_first() {
        Some((Message::System(system), tail)) => (Some(system), tail),
        _ => (None, incoming.messages()),
    };
    let (previous_system, previous_tail) = match previous.messages().split_first() {
        Some((Message::System(system), tail)) => (Some(system), tail),
        _ => (None, previous.messages()),
    };
    if incoming_system.is_some() != previous_system.is_some()
        || incoming_tail.len() <= previous_tail.len()
    {
        return Ok(false);
    }
    let Some(Message::User(summary)) = incoming_tail.first() else {
        return Ok(false);
    };
    // Typed marker, not content classification: the runtime compaction producer
    // stamps the rebuilt-transcript boundary message with the
    // `CompactionSummary` transcript role. The save-guard admits the divergent
    // rewrite parent only when that typed fact is present.
    if !summary.transcript_role.is_compaction_summary() {
        return Ok(false);
    }

    let retained_end = 1 + previous_tail.len();
    let retained = &incoming_tail[1..retained_end];
    let retained_revision =
        transcript_messages_digest(retained).map_err(SessionStoreError::from)?;
    let previous_revision =
        transcript_messages_digest(previous_tail).map_err(SessionStoreError::from)?;
    if retained_revision != previous_revision {
        return Ok(false);
    }

    for commit in &state.commits {
        validate_transcript_rewrite_commit_bodies(incoming, commit, state)?;
    }
    Ok(true)
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
    if crate::session::validate_transcript_history_state(state).is_err() {
        return None;
    }
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

/// Find a rewrite chain whose first parent may be an append-only continuation
/// of a previously persisted snapshot.
///
/// Runtime-backed sessions can append messages in the runtime store before a
/// core-owned compaction rewrite is checkpointed to the compatibility
/// `SessionStore`. In that case the first rewrite commit's parent revision is
/// not equal to the persisted row's digest, but its retained parent body proves
/// a normal append path from that persisted row.
pub fn find_transcript_rewrite_commit_chain_extending_session<'a>(
    state: &'a TranscriptHistoryState,
    previous: &Session,
    incoming_revision: &str,
) -> Result<Option<Vec<&'a TranscriptRewriteCommit>>, SessionStoreError> {
    crate::session::validate_transcript_history_state(state).map_err(|error| {
        SessionStoreError::InvalidTranscriptRewrite {
            id: previous.id().clone(),
            reason: format!("incoming transcript history state is malformed: {error}"),
        }
    })?;
    let previous_revision =
        transcript_messages_digest(previous.messages()).map_err(SessionStoreError::from)?;
    let mut chain = Vec::new();
    let mut cursor = previous_revision.as_str();
    let mut visited = std::collections::BTreeSet::new();
    loop {
        if incoming_revision == cursor {
            return Ok(Some(chain));
        }
        if !visited.insert(cursor.to_string()) {
            return Ok(None);
        }

        let Some(cursor_messages) = transcript_history_messages_for_revision(
            state,
            cursor,
            &previous_revision,
            previous.messages(),
        ) else {
            return Ok(None);
        };

        // Exact graph edges are authoritative: a commit recorded directly
        // against this cursor advances the walk (and keeps that commit on
        // the audited persistence chain). A commit whose revision is the
        // cursor itself, or any revision this walk already visited, cannot
        // make progress and is never selected.
        let mut selected = None;
        for commit in &state.commits {
            if commit.revision == cursor || visited.contains(&commit.revision) {
                continue;
            }
            if !transcript_history_revision_extends(state, incoming_revision, &commit.revision) {
                continue;
            }
            if commit.parent_revision == cursor {
                selected = Some(commit);
                break;
            }
        }

        // With no exact edge, a plain append continuation from this cursor
        // completes the proof: the incoming transcript preserves the
        // cursor's content and no further rewrite edge is needed. Proving
        // this BEFORE the equivalence-based selection below is load-bearing:
        // once the graph retains SEVERAL chained system-prompt-refresh
        // commits, the refresh equivalence makes every retained refresh
        // commit's parent body "extend" the cursor, so selection would walk
        // an OLDER refresh commit forward onto the revision the cursor
        // already reached and abort as a cycle — rejecting a valid append
        // (chained resume refreshes with no turn in between, the idle mob
        // member roster-drift shape).
        if selected.is_none() {
            if revision_body_preserves_append_continuation_prefix(
                state,
                incoming_revision,
                cursor_messages,
                cursor,
                false,
            )? {
                return Ok(Some(chain));
            }
            // Only when neither an exact edge nor a plain continuation
            // exists, fall back to the system-refresh equivalence: a refresh
            // commit recorded against a rebookkept parent (the resume-time
            // shape) whose parent body still extends the cursor.
            for commit in &state.commits {
                if commit.revision == cursor || visited.contains(&commit.revision) {
                    continue;
                }
                if !transcript_history_revision_extends(state, incoming_revision, &commit.revision)
                {
                    continue;
                }
                if revision_body_preserves_append_continuation_prefix(
                    state,
                    &commit.parent_revision,
                    cursor_messages,
                    cursor,
                    true,
                )? {
                    selected = Some(commit);
                    break;
                }
            }
        }

        let Some(commit) = selected else {
            return Ok(None);
        };
        cursor = &commit.revision;
        chain.push(commit);
    }
}

fn transcript_history_messages_for_revision<'a>(
    state: &'a TranscriptHistoryState,
    revision: &str,
    previous_revision: &str,
    previous_messages: &'a [Message],
) -> Option<&'a [Message]> {
    if revision == previous_revision {
        return Some(previous_messages);
    }
    state
        .revisions
        .iter()
        .find(|body| body.revision == revision)
        .map(|body| body.messages.as_slice())
}

fn revision_body_preserves_append_continuation_prefix(
    state: &TranscriptHistoryState,
    revision: &str,
    ancestor_messages: &[Message],
    ancestor_revision: &str,
    allow_leading_system_refresh: bool,
) -> Result<bool, SessionStoreError> {
    if revision == ancestor_revision {
        return Ok(true);
    }
    let Some(body) = state
        .revisions
        .iter()
        .find(|body| body.revision == revision)
    else {
        return Ok(false);
    };
    if body.messages.len() >= ancestor_messages.len() {
        let prefix_revision = transcript_messages_digest(&body.messages[..ancestor_messages.len()])
            .map_err(SessionStoreError::from)?;
        if prefix_revision == ancestor_revision {
            return Ok(true);
        }
    }
    if messages_preserve_conversation_tail_with_system_context_append(
        &body.messages,
        ancestor_messages,
    )? {
        return Ok(true);
    }
    // The untyped leading-System-refresh equivalence bridges bookkeeping
    // divergence between a persisted row and a rewrite commit's recorded
    // PARENT body only. It must not prove the final plain-append
    // continuation: that would admit an unaudited System replacement (a
    // recorded refresh body with no typed commit) as an ordinary append.
    Ok(allow_leading_system_refresh
        && messages_preserve_tail_after_leading_system_refresh(&body.messages, ancestor_messages)?)
}

fn messages_preserve_tail_after_leading_system_refresh(
    incoming: &[Message],
    previous: &[Message],
) -> Result<bool, SessionStoreError> {
    let (Some(Message::System(_)), Some(Message::System(_))) = (incoming.first(), previous.first())
    else {
        return Ok(false);
    };
    if incoming.len() < previous.len() {
        return Ok(false);
    }
    let previous_tail_len = previous.len().saturating_sub(1);
    if previous_tail_len == 0 {
        return Ok(true);
    }
    let previous_tail_revision =
        transcript_messages_digest(&previous[1..]).map_err(SessionStoreError::from)?;
    let incoming_tail = &incoming[1..];
    if incoming_tail.len() < previous_tail_len {
        return Ok(false);
    }
    let incoming_tail_prefix_revision =
        transcript_messages_digest(&incoming_tail[..previous_tail_len])
            .map_err(SessionStoreError::from)?;
    Ok(incoming_tail_prefix_revision == previous_tail_revision)
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
    // Parent pointers are metadata, not digest-covered: bound the walk so a
    // crafted cyclic revision-parent chain fails closed instead of hanging.
    let mut visited = std::collections::BTreeSet::new();
    while let Some(body) = state.revisions.iter().find(|body| body.revision == cursor) {
        if !visited.insert(body.revision.clone()) {
            return false;
        }
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
    incoming
        .validate_transcript_history_state()
        .map_err(|err| SessionStoreError::InvalidTranscriptRewrite {
            id: incoming.id().clone(),
            reason: format!("incoming transcript history state is malformed: {err}"),
        })?;
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
    validate_rewrite_save_retains_previous_commits(incoming, previous, &incoming_state)?;
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
    let (start, end) = commit.selection.bounds();
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

    /// Save an authoritative projection only if the persisted row is still the
    /// revision that the caller already validated.
    async fn save_authoritative_projection_if_current_revision(
        &self,
        session: &Session,
        expected_current_revision: Option<String>,
    ) -> Result<(), SessionStoreError> {
        let _ = (session, expected_current_revision);
        Err(SessionStoreError::Internal(
            "save_authoritative_projection_if_current_revision is not supported by this SessionStore"
                .to_string(),
        ))
    }

    /// Load a session by ID.
    async fn load(&self, id: &SessionId) -> Result<Option<Session>, SessionStoreError>;

    /// List sessions matching filter.
    async fn list(&self, filter: SessionFilter) -> Result<Vec<SessionMeta>, SessionStoreError>;

    /// Load only the summary metadata row for a session.
    ///
    /// Metadata-only read seam (mobkit ask-24 clause 3): callers that need
    /// session-level metadata facts (the reserved `session_*` authority keys
    /// carried on [`SessionMeta::metadata`]) but not the transcript can avoid
    /// materializing the full session document.
    ///
    /// Default: full [`load`](Self::load) projected through
    /// [`SessionMeta::from`] — correct for every backend, with no
    /// partial-read benefit. Backends with a row-level metadata projection
    /// (SQLite) override this with a real partial read that survives a
    /// corrupt or unreadable full session document.
    async fn load_meta(&self, id: &SessionId) -> Result<Option<SessionMeta>, SessionStoreError> {
        Ok(self
            .load(id)
            .await?
            .map(|session| SessionMeta::from(&session)))
    }

    /// Delete a session.
    async fn delete(&self, id: &SessionId) -> Result<(), SessionStoreError>;

    /// Delete a compatibility projection only if it is still the revision that
    /// the caller already validated as unsafe to expose.
    async fn delete_if_current_revision(
        &self,
        id: &SessionId,
        expected_current_revision: &str,
    ) -> Result<bool, SessionStoreError>;

    /// Check if a session exists.
    async fn exists(&self, id: &SessionId) -> Result<bool, SessionStoreError> {
        Ok(self.load(id).await?.is_some())
    }

    /// Typed capability accessor for the incremental persistence contract.
    ///
    /// Delegating wrappers MUST forward this; the default keeps plain
    /// whole-blob stores on the compat path (the runtime silently degrades to
    /// whole-blob persistence when a wrapper swallows the capability).
    fn as_incremental(self: Arc<Self>) -> Option<Arc<dyn IncrementalSessionStore>> {
        None
    }
}

// ---------------------------------------------------------------------------
// Incremental session persistence (OB3 ask 11): O(delta) writes, compaction
// that SHRINKS the persisted head, retained history out-of-line.
// ---------------------------------------------------------------------------

/// Opaque id of an append-only message strand.
///
/// Minting rules:
/// - [`TranscriptStrandId::root`] — a session's first strand;
/// - [`TranscriptStrandId::from_rewrite`] — the strand created by adopting a
///   transcript rewrite commit (named by the commit's revision digest);
/// - [`TranscriptStrandId::rebase`] — `rebase:{digest}` strands minted for
///   compat/equivalence representation rebases and for migrated rebookkept
///   rewrite parents.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TranscriptStrandId(String);

impl TranscriptStrandId {
    /// A session's first strand.
    pub fn root() -> Self {
        Self("root".to_string())
    }

    /// The strand created by adopting a transcript rewrite commit.
    pub fn from_rewrite(commit: &TranscriptRewriteCommit) -> Self {
        Self(commit.revision.clone())
    }

    /// A compat/equivalence representation-rebase strand.
    pub fn rebase(head_revision: &str) -> Self {
        Self(format!("rebase:{head_revision}"))
    }

    /// Restore a persisted strand id (durable-format decoder for store rows).
    pub fn from_persisted(raw: String) -> Self {
        Self(raw)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for TranscriptStrandId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Small durable head row: the whole session EXCEPT message bodies and
/// retained revision bodies.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SessionHead {
    pub id: SessionId,
    /// Session envelope version (`Session::version`).
    pub version: u32,
    pub strand: TranscriptStrandId,
    /// `transcript_messages_digest` of the live messages.
    pub head_revision: String,
    /// Live message count == strand prefix covered by this head.
    pub message_count: u64,
    /// ADOPTED rewrite commits recorded for this session.
    pub rewrite_count: u64,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    pub usage: Usage,
    /// Session metadata WITHOUT `SESSION_TRANSCRIPT_HISTORY_STATE_KEY`
    /// (the constructor strips it; `save_head` rejects heads carrying it).
    pub metadata: serde_json::Map<String, serde_json::Value>,
}

impl SessionHead {
    /// Project a session onto its durable head row.
    ///
    /// Strips `SESSION_TRANSCRIPT_HISTORY_STATE_KEY` from the metadata —
    /// retained history lives out-of-line in strand rows and rewrite records.
    pub fn from_session(
        session: &Session,
        strand: TranscriptStrandId,
        rewrite_count: u64,
    ) -> Result<Self, SessionStoreError> {
        let head_revision =
            transcript_messages_digest(session.messages()).map_err(SessionStoreError::from)?;
        let mut metadata = session.metadata().clone();
        metadata.remove(SESSION_TRANSCRIPT_HISTORY_STATE_KEY);
        Ok(Self {
            id: session.id().clone(),
            version: session.version(),
            strand,
            head_revision,
            message_count: session.messages().len() as u64,
            rewrite_count,
            created_at: session.created_at(),
            updated_at: session.updated_at(),
            usage: session.total_usage(),
            metadata,
        })
    }

    /// Rebuild a slim `Session` (no transcript-history metadata) from this
    /// head plus its strand messages.
    ///
    /// Fails closed `Corrupted` if `digest(messages) != head_revision` or
    /// `messages.len() != message_count`. The envelope version is restored
    /// through the generated persistence version authority, exactly like
    /// `Session::deserialize`.
    pub fn into_session(self, messages: Vec<Message>) -> Result<Session, SessionStoreError> {
        if messages.len() as u64 != self.message_count {
            return Err(SessionStoreError::Corrupted(self.id));
        }
        let digest = transcript_messages_digest(&messages).map_err(SessionStoreError::from)?;
        if digest != self.head_revision {
            return Err(SessionStoreError::Corrupted(self.id));
        }
        let SessionHead {
            id,
            version,
            created_at,
            updated_at,
            usage,
            metadata,
            ..
        } = self;
        Session::from_head_parts(
            version, id, messages, created_at, updated_at, metadata, usage,
        )
        .map_err(|err| {
            SessionStoreError::Serialization(format!(
                "failed to restore session from head row: {err}"
            ))
        })
    }
}

/// Stable compare token for a persisted session head row (mirror of
/// [`session_projection_cas_token`] for the incremental contract).
pub fn session_head_cas_token(head: &SessionHead) -> Result<String, SessionStoreError> {
    let bytes = serde_json::to_vec(head).map_err(|err| {
        SessionStoreError::Serialization(format!(
            "failed to serialize session head CAS token: {err}"
        ))
    })?;
    Ok(format!("head-sha256:{:x}", Sha256::digest(bytes)))
}

/// CAS expectation for incremental head writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionHeadCas {
    /// No head row may exist yet.
    Create,
    /// The stored row's token must equal this.
    IfToken(String),
}

/// Capability trait for O(delta) session persistence.
///
/// Every retained transcript body is a prefix of some strand: the parent body
/// of commit `k` is a prefix of the strand commit `k-1` created (or the root
/// strand), and the revision body of commit `k` is a prefix of the strand it
/// creates. Retained history therefore costs zero duplicate storage on the
/// live path, and compaction persists O(live-after) instead of a superset
/// blob.
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait IncrementalSessionStore: SessionStore {
    /// Append messages to a strand. O(delta).
    ///
    /// Contiguity: `base_seq` must not exceed the strand's current row count
    /// (0 only for a strand with no rows opens the strand). Existing
    /// `(strand, seq)` rows are immutable: identical bytes => idempotent Ok;
    /// different bytes => `TranscriptContinuityViolation`. Shrink is
    /// structurally inexpressible (the `append_only_save_guard` port).
    async fn append_messages(
        &self,
        id: &SessionId,
        strand: &TranscriptStrandId,
        base_seq: u64,
        messages: &[Message],
    ) -> Result<(), SessionStoreError>;

    /// Record a transcript rewrite without advancing the head.
    ///
    /// Validates the record self-consistently (the
    /// `validate_transcript_rewrite_record` semantics carried by
    /// [`TranscriptRewriteRecord`]), CAS-compares `expected` against the
    /// stored head token, requires `record.commit.parent_revision` to equal
    /// the stored `head_revision` (else `TranscriptRevisionConflict` — the
    /// `transcript_rewrite_save_guard` port), verifies the digest of the
    /// parent strand rows `[0..messages_before)` equals `parent_revision`
    /// (O(parent), rewrite-time only), then writes the commit at
    /// `rewrite_idx = stored head.rewrite_count` (replacing any unadopted row
    /// at that idx => idempotent retry) plus the new strand's base rows
    /// (`revision_body.messages` under `from_rewrite(commit)`).
    ///
    /// Does NOT advance the head: adoption = a subsequent [`save_head`] with
    /// `rewrite_count = idx + 1` and `strand = from_rewrite(commit)`. Returns
    /// the implied next head for the caller to adopt.
    ///
    /// [`save_head`]: IncrementalSessionStore::save_head
    async fn commit_rewrite(
        &self,
        id: &SessionId,
        record: &TranscriptRewriteRecord,
        expected: SessionHeadCas,
    ) -> Result<SessionHead, SessionStoreError>;

    /// CAS-guarded small head write.
    ///
    /// Guards: `expected` token match (mismatch =>
    /// `TranscriptRevisionConflict`); metadata must not carry
    /// `SESSION_TRANSCRIPT_HISTORY_STATE_KEY` (=> `InvalidTranscriptRewrite`);
    /// strand rows must cover `[0, message_count)` (the head never points
    /// past persisted rows); same-strand `message_count` must be monotonic
    /// (=> `MonotonicityViolation`); `rewrite_count` may advance by at most
    /// the recorded-but-unadopted commits. Strand-switch saves are the
    /// authoritative-projection analog (CAS-trusted); `head_revision` on
    /// plain appends is caller-attested, audited at the next `commit_rewrite`
    /// and verified fail-closed on every `into_session` load.
    async fn save_head(
        &self,
        head: &SessionHead,
        expected: SessionHeadCas,
    ) -> Result<(), SessionStoreError>;

    async fn load_head(&self, id: &SessionId) -> Result<Option<SessionHead>, SessionStoreError>;

    async fn load_messages(
        &self,
        id: &SessionId,
        strand: &TranscriptStrandId,
        range: std::ops::Range<u64>,
    ) -> Result<Vec<Message>, SessionStoreError>;

    /// Adopted rewrites only (`idx < head.rewrite_count`), reconstructed as
    /// full [`TranscriptRewriteRecord`]s from strand prefix ranges; never
    /// read on resume. Each record must pass `TranscriptRewriteRecord::new`
    /// validation.
    async fn load_rewrites(
        &self,
        id: &SessionId,
    ) -> Result<Vec<TranscriptRewriteRecord>, SessionStoreError>;
}

/// Plain-save guard for head-canonical rows where retained history lives
/// out-of-line: `previous_slim` is the slim materialization of the stored
/// head; `stored_commits` are the adopted commits.
///
/// Admits: metadata-only update, prefix-preserving append, the
/// system-context-append equivalence (driven through the canonical
/// `SessionDocumentMachine` admission, same as [`append_only_save_guard`]),
/// and transient-notice cleanup. Incoming history state, if present, must
/// carry commits equal to `stored_commits` (extra commits =>
/// `InvalidTranscriptRewrite` "route via save_transcript_rewrite") and pass
/// session-level validation on its own bodies. ABSENT incoming state is OK —
/// out-of-line history cannot be erased by a row write (a deliberate delta vs
/// `append_only_save_guard`'s erase check).
pub fn head_canonical_plain_save_guard(
    incoming: &Session,
    previous_slim: &Session,
    stored_commits: &[TranscriptRewriteCommit],
) -> Result<(), SessionStoreError> {
    incoming
        .validate_transcript_history_state()
        .map_err(|err| SessionStoreError::InvalidTranscriptRewrite {
            id: incoming.id().clone(),
            reason: format!("incoming transcript history state is malformed: {err}"),
        })?;
    let incoming_revision =
        transcript_messages_digest(incoming.messages()).map_err(SessionStoreError::from)?;
    let incoming_state = incoming.transcript_history_state().map_err(|err| {
        SessionStoreError::InvalidTranscriptRewrite {
            id: incoming.id().clone(),
            reason: format!("incoming transcript history state is malformed: {err}"),
        }
    })?;
    if let Some(state) = incoming_state.as_ref() {
        if state.head != incoming_revision {
            return Err(SessionStoreError::InvalidTranscriptRewrite {
                id: incoming.id().clone(),
                reason: format!(
                    "incoming transcript graph head {} does not match current message digest {incoming_revision}",
                    state.head
                ),
            });
        }
        if state.commits.as_slice() != stored_commits {
            if state.commits.len() > stored_commits.len()
                && state.commits[..stored_commits.len()] == *stored_commits
            {
                return Err(SessionStoreError::InvalidTranscriptRewrite {
                    id: incoming.id().clone(),
                    reason: "incoming plain save carries unadopted transcript rewrite commits; \
                             route via save_transcript_rewrite"
                        .to_string(),
                });
            }
            return Err(SessionStoreError::InvalidTranscriptRewrite {
                id: incoming.id().clone(),
                reason: "incoming plain save would change adopted transcript rewrite commits"
                    .to_string(),
            });
        }
    }

    let previous_revision =
        transcript_messages_digest(previous_slim.messages()).map_err(SessionStoreError::from)?;
    if previous_revision == incoming_revision {
        return Ok(());
    }
    let prev_len = previous_slim.messages().len();
    let new_len = incoming.messages().len();
    if new_len >= prev_len {
        let incoming_prefix_revision = transcript_messages_digest(&incoming.messages()[..prev_len])
            .map_err(SessionStoreError::from)?;
        if incoming_prefix_revision == previous_revision {
            return Ok(());
        }
    }
    if incoming_preserves_conversation_tail_with_system_context_append(incoming, previous_slim)? {
        return Ok(());
    }
    if incoming_preserves_prefix_after_synthetic_notice_refresh(incoming, previous_slim)? {
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
        reason: "incoming transcript neither preserves the persisted head-strand prefix nor \
                 matches a machine-admitted equivalence shape"
            .to_string(),
    })
}

/// Shared `save_head` transition validator so guard semantics stay uniform
/// across [`IncrementalSessionStore`] backends.
///
/// `stored` is the current row plus its CAS token; `new_strand_len` is the
/// persisted row count of `head.strand`; `recorded_rewrites` is the total
/// number of recorded rewrite rows (adopted + unadopted).
pub fn validate_save_head_transition(
    head: &SessionHead,
    stored: Option<(&SessionHead, &str)>,
    expected: &SessionHeadCas,
    new_strand_len: u64,
    recorded_rewrites: u64,
) -> Result<(), SessionStoreError> {
    if head
        .metadata
        .contains_key(SESSION_TRANSCRIPT_HISTORY_STATE_KEY)
    {
        return Err(SessionStoreError::InvalidTranscriptRewrite {
            id: head.id.clone(),
            reason: "session head must not inline transcript history state metadata".to_string(),
        });
    }
    match (expected, stored) {
        (SessionHeadCas::Create, None) => {}
        (SessionHeadCas::Create, Some((_, token))) => {
            return Err(SessionStoreError::TranscriptRevisionConflict {
                id: head.id.clone(),
                expected: "<create>".to_string(),
                actual: token.to_string(),
            });
        }
        (SessionHeadCas::IfToken(expected_token), Some((_, token))) => {
            if expected_token != token {
                return Err(SessionStoreError::TranscriptRevisionConflict {
                    id: head.id.clone(),
                    expected: expected_token.clone(),
                    actual: token.to_string(),
                });
            }
        }
        (SessionHeadCas::IfToken(expected_token), None) => {
            return Err(SessionStoreError::TranscriptRevisionConflict {
                id: head.id.clone(),
                expected: expected_token.clone(),
                actual: "<missing>".to_string(),
            });
        }
    }
    if head.message_count > new_strand_len {
        return Err(SessionStoreError::InvalidTranscriptRewrite {
            id: head.id.clone(),
            reason: format!(
                "session head covers {} messages but strand {} only persists {new_strand_len} rows",
                head.message_count, head.strand
            ),
        });
    }
    if let Some((stored_head, _)) = stored {
        if stored_head.strand == head.strand && head.message_count < stored_head.message_count {
            return Err(SessionStoreError::MonotonicityViolation {
                id: head.id.clone(),
                prev_len: usize::try_from(stored_head.message_count).unwrap_or(usize::MAX),
                new_len: usize::try_from(head.message_count).unwrap_or(usize::MAX),
            });
        }
        if head.rewrite_count < stored_head.rewrite_count {
            return Err(SessionStoreError::InvalidTranscriptRewrite {
                id: head.id.clone(),
                reason: format!(
                    "session head rewrite_count {} would retract adopted rewrite count {}",
                    head.rewrite_count, stored_head.rewrite_count
                ),
            });
        }
    }
    if head.rewrite_count > recorded_rewrites {
        return Err(SessionStoreError::InvalidTranscriptRewrite {
            id: head.id.clone(),
            reason: format!(
                "session head rewrite_count {} exceeds recorded rewrite commits {recorded_rewrites}",
                head.rewrite_count
            ),
        });
    }
    Ok(())
}

/// Shared `commit_rewrite` transition validator.
///
/// `parent_prefix_digest` is the digest of the stored head strand's rows
/// `[0..record.commit.messages_before)` — computed by the backend from its
/// persisted rows, never caller-attested. Returns the implied next head for
/// the caller to adopt via `save_head`.
pub fn validate_commit_rewrite_transition(
    id: &SessionId,
    record: &TranscriptRewriteRecord,
    stored: &SessionHead,
    stored_token: &str,
    expected: &SessionHeadCas,
    parent_prefix_digest: &str,
) -> Result<SessionHead, SessionStoreError> {
    match expected {
        SessionHeadCas::Create => {
            return Err(SessionStoreError::TranscriptRevisionConflict {
                id: id.clone(),
                expected: "<create>".to_string(),
                actual: stored_token.to_string(),
            });
        }
        SessionHeadCas::IfToken(expected_token) => {
            if expected_token != stored_token {
                return Err(SessionStoreError::TranscriptRevisionConflict {
                    id: id.clone(),
                    expected: expected_token.clone(),
                    actual: stored_token.to_string(),
                });
            }
        }
    }
    TranscriptRewriteRecord::new(
        record.commit.clone(),
        record.parent_body.clone(),
        record.revision_body.clone(),
    )
    .map_err(|err| SessionStoreError::InvalidTranscriptRewrite {
        id: id.clone(),
        reason: format!("transcript rewrite record failed validation: {err}"),
    })?;
    if record.commit.parent_revision != stored.head_revision {
        return Err(SessionStoreError::TranscriptRevisionConflict {
            id: id.clone(),
            expected: record.commit.parent_revision.clone(),
            actual: stored.head_revision.clone(),
        });
    }
    if parent_prefix_digest != record.commit.parent_revision {
        return Err(SessionStoreError::InvalidTranscriptRewrite {
            id: id.clone(),
            reason: format!(
                "persisted parent strand rows digest {parent_prefix_digest} does not match \
                 commit parent revision {}",
                record.commit.parent_revision
            ),
        });
    }
    Ok(SessionHead {
        id: id.clone(),
        version: stored.version,
        strand: TranscriptStrandId::from_rewrite(&record.commit),
        head_revision: record.commit.revision.clone(),
        message_count: record.commit.messages_after as u64,
        rewrite_count: stored.rewrite_count.saturating_add(1),
        created_at: stored.created_at,
        updated_at: record.commit.committed_at,
        usage: stored.usage.clone(),
        metadata: stored.metadata.clone(),
    })
}

/// One rewrite edge in a [`StrandLayout`].
#[derive(Debug, Clone)]
pub struct StrandRewriteLayout {
    pub commit: TranscriptRewriteCommit,
    pub parent_strand: TranscriptStrandId,
    pub parent_len: u64,
    pub strand: TranscriptStrandId,
    pub strand_len: u64,
}

/// Deterministic strand layout of a session's retained transcript history:
/// the shared pure function behind read-only head synthesis and the one-time
/// blob-to-head-canonical migration.
#[derive(Debug, Clone)]
pub struct StrandLayout {
    /// Full (maximal) row vector per strand.
    pub strands: Vec<(TranscriptStrandId, Vec<Message>)>,
    /// Adopted rewrites, in commit order (`rewrite_idx` = position).
    pub rewrites: Vec<StrandRewriteLayout>,
    pub head_strand: TranscriptStrandId,
    pub head_len: u64,
}

fn layout_messages_extend(
    base: &[Message],
    candidate: &[Message],
) -> Result<bool, SessionStoreError> {
    if candidate.len() < base.len() {
        return Ok(false);
    }
    if base.is_empty() {
        return Ok(true);
    }
    let base_digest = transcript_messages_digest(base).map_err(SessionStoreError::from)?;
    let prefix_digest =
        transcript_messages_digest(&candidate[..base.len()]).map_err(SessionStoreError::from)?;
    Ok(base_digest == prefix_digest)
}

fn layout_find_or_insert(
    id: &SessionId,
    strands: &mut Vec<(TranscriptStrandId, Vec<Message>)>,
    strand: TranscriptStrandId,
    rows: Vec<Message>,
) -> Result<usize, SessionStoreError> {
    if let Some(index) = strands.iter().position(|(sid, _)| *sid == strand) {
        if layout_messages_extend(&strands[index].1, &rows)? {
            strands[index].1 = rows;
        } else if !layout_messages_extend(&rows, &strands[index].1)? {
            return Err(SessionStoreError::InvalidTranscriptRewrite {
                id: id.clone(),
                reason: format!(
                    "retained transcript history maps divergent bodies onto strand {strand}"
                ),
            });
        }
        return Ok(index);
    }
    strands.push((strand, rows));
    Ok(strands.len() - 1)
}

/// Lay out a session's retained transcript history as append-only strands.
///
/// Root strand → `from_rewrite` chain per adopted commit; rebookkept parents
/// get their own `rebase:` strands from their retained bodies; the live
/// vector extends the final strand (or, when it provably does not, its own
/// `rebase:` strand). Pure — shared by read-only head synthesis and the
/// in-transaction migration write.
pub fn strand_layout_for_history(
    id: &SessionId,
    state: Option<&TranscriptHistoryState>,
    live_messages: &[Message],
) -> Result<StrandLayout, SessionStoreError> {
    let mut strands: Vec<(TranscriptStrandId, Vec<Message>)> =
        vec![(TranscriptStrandId::root(), Vec::new())];
    let mut rewrites = Vec::new();
    let mut current = 0usize;
    let commits: &[TranscriptRewriteCommit] = state.map(|s| s.commits.as_slice()).unwrap_or(&[]);
    for commit in commits {
        let state = state.ok_or_else(|| SessionStoreError::InvalidTranscriptRewrite {
            id: id.clone(),
            reason: "transcript rewrite commits without retained history state".to_string(),
        })?;
        let parent_body = state
            .revisions
            .iter()
            .find(|body| body.revision == commit.parent_revision)
            .ok_or_else(|| SessionStoreError::InvalidTranscriptRewrite {
                id: id.clone(),
                reason: format!(
                    "retained history omits parent revision body {}",
                    commit.parent_revision
                ),
            })?;
        let (parent_index, parent_len) =
            if layout_messages_extend(&strands[current].1, &parent_body.messages)? {
                strands[current].1 = parent_body.messages.clone();
                (current, parent_body.messages.len())
            } else {
                let rebased = TranscriptStrandId::rebase(&commit.parent_revision);
                let index =
                    layout_find_or_insert(id, &mut strands, rebased, parent_body.messages.clone())?;
                (index, parent_body.messages.len())
            };
        let revision_body = state
            .revisions
            .iter()
            .find(|body| body.revision == commit.revision)
            .ok_or_else(|| SessionStoreError::InvalidTranscriptRewrite {
                id: id.clone(),
                reason: format!(
                    "retained history omits new revision body {}",
                    commit.revision
                ),
            })?;
        let new_strand = TranscriptStrandId::from_rewrite(commit);
        let new_index =
            layout_find_or_insert(id, &mut strands, new_strand, revision_body.messages.clone())?;
        rewrites.push(StrandRewriteLayout {
            commit: commit.clone(),
            parent_strand: strands[parent_index].0.clone(),
            parent_len: parent_len as u64,
            strand: strands[new_index].0.clone(),
            strand_len: revision_body.messages.len() as u64,
        });
        current = new_index;
    }
    if layout_messages_extend(&strands[current].1, live_messages)? {
        strands[current].1 = live_messages.to_vec();
    } else {
        let live_digest =
            transcript_messages_digest(live_messages).map_err(SessionStoreError::from)?;
        let rebased = TranscriptStrandId::rebase(&live_digest);
        current = layout_find_or_insert(id, &mut strands, rebased, live_messages.to_vec())?;
    }
    Ok(StrandLayout {
        head_strand: strands[current].0.clone(),
        head_len: live_messages.len() as u64,
        strands,
        rewrites,
    })
}

/// Reconstruct an adopted rewrite record from strand prefix ranges.
///
/// `parent_messages`/`revision_messages` are the backend's persisted rows
/// `[0..parent_len)` / `[0..strand_len)` for the recorded strands. Body
/// `created_at` is derived from the commit (bodies are content-addressed;
/// the timestamp is bookkeeping).
pub fn reconstruct_rewrite_record(
    id: &SessionId,
    commit: TranscriptRewriteCommit,
    parent_messages: Vec<Message>,
    revision_messages: Vec<Message>,
) -> Result<TranscriptRewriteRecord, SessionStoreError> {
    let parent_body = TranscriptRevisionBody {
        revision: commit.parent_revision.clone(),
        parent_revision: None,
        messages: parent_messages,
        created_at: commit.committed_at,
    };
    let revision_body = TranscriptRevisionBody {
        revision: commit.revision.clone(),
        parent_revision: Some(commit.parent_revision.clone()),
        messages: revision_messages,
        created_at: commit.committed_at,
    };
    TranscriptRewriteRecord::new(commit, parent_body, revision_body).map_err(|err| {
        SessionStoreError::InvalidTranscriptRewrite {
            id: id.clone(),
            reason: format!("persisted rewrite record failed reconstruction: {err}"),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{
        AssistantBlock, BlockAssistantMessage, StopReason, SystemMessage, SystemNoticeBlock,
        SystemNoticeKind, SystemNoticeMessage, UserMessage,
    };

    #[test]
    fn exact_snapshot_head_coherence_guard_rejects_live_transcript_forgery()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut session = Session::new();
        session.push(Message::User(UserMessage::text("seed".to_string())));
        session.commit_transcript_rewrite(
            crate::TranscriptRewriteSelection::MessageRange { start: 0, end: 1 },
            vec![Message::User(UserMessage::text("rewritten".to_string()))],
            crate::TranscriptRewriteReason::new("unit-test-edit"),
            Some("unit-test".to_string()),
            None,
        )?;
        run_boundary_snapshot_head_coherence_guard(&session)?;

        let mut envelope = serde_json::to_value(&session)?;
        envelope["messages"] = serde_json::to_value(vec![Message::User(UserMessage::text(
            "forged live transcript".to_string(),
        ))])?;
        let forged: Session = serde_json::from_value(envelope)?;
        assert!(matches!(
            run_boundary_snapshot_head_coherence_guard(&forged),
            Err(SessionStoreError::InvalidTranscriptRewrite { .. })
        ));
        Ok(())
    }

    /// A lagging persisted row must be walkable across chained refresh
    /// commits whose recorded parents were rebookkept (only the fuzzy
    /// refresh-equivalence edge can advance), without the walk re-selecting
    /// a refresh commit whose revision it already visited: at the later
    /// cursors, every OLDER refresh commit's parent body still "extends" the
    /// cursor under the refresh equivalence, and re-selecting one walks back
    /// onto visited territory and aborts as a cycle. Pins the
    /// visited-revision skip in both selection scans.
    #[test]
    #[allow(clippy::expect_used)]
    fn boundary_commit_walks_lagging_row_across_rebookkept_refresh_chain()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut base = Session::new();
        base.push(Message::System(SystemMessage::new(
            "member prompt roster v1",
        )));
        base.push(Message::User(UserMessage::text(
            "the codeword is birch seventeen".to_string(),
        )));
        // The persisted row lags the whole rewrite graph (written before any
        // refresh boot, carrying no history state).
        let previous = base.clone();
        let v1 = base.transcript_revision()?;

        // Three refresh boots chain commits onto the graph.
        let mut session = base;
        session.commit_transcript_rewrite(
            TranscriptRewriteSelection::MessageRange { start: 0, end: 1 },
            vec![Message::System(SystemMessage::new(
                "member prompt roster v2",
            ))],
            crate::TranscriptRewriteReason::new("resume-system-prompt-refresh"),
            Some("agent-factory/resume".to_string()),
            Some(v1),
        )?;
        let v2 = session.transcript_revision()?;
        session.commit_transcript_rewrite(
            TranscriptRewriteSelection::MessageRange { start: 0, end: 1 },
            vec![Message::System(SystemMessage::new(
                "member prompt roster v3",
            ))],
            crate::TranscriptRewriteReason::new("resume-system-prompt-refresh"),
            Some("agent-factory/resume".to_string()),
            Some(v2.clone()),
        )?;
        let v3 = session.transcript_revision()?;
        session.commit_transcript_rewrite(
            TranscriptRewriteSelection::MessageRange { start: 0, end: 1 },
            vec![Message::System(SystemMessage::new(
                "member prompt roster v4",
            ))],
            crate::TranscriptRewriteReason::new("resume-system-prompt-refresh"),
            Some("agent-factory/resume".to_string()),
            Some(v3.clone()),
        )?;

        // Rebookkeep the recorded parents of the later refresh commits: each
        // now points at an equivalent parent body with re-stamped leading
        // System content (the re-created-authority shape), so no exact edge
        // exists from the walked cursors and only the refresh equivalence
        // can advance.
        let mut state = session
            .transcript_history_state()?
            .expect("chained refreshes retain history state");
        let rebookkeep = |state: &mut TranscriptHistoryState,
                          original_parent: &str,
                          stamp: &str|
         -> Result<String, Box<dyn std::error::Error>> {
            let body = state
                .revisions
                .iter()
                .find(|body| body.revision == original_parent)
                .expect("parent body retained")
                .clone();
            let mut messages = body.messages;
            messages[0] = Message::System(SystemMessage::new(stamp));
            let revision = transcript_messages_digest(&messages)?;
            // The rebookkept body chains off the revision it restamps, so
            // the graph stays a valid extension chain for the validator.
            state
                .revisions
                .push(crate::session::TranscriptRevisionBody {
                    revision: revision.clone(),
                    parent_revision: Some(original_parent.to_string()),
                    messages,
                    created_at: SystemTime::now(),
                });
            Ok(revision)
        };
        let v2_rebookkept = rebookkeep(&mut state, &v2, "member prompt roster v2 restamped")?;
        let v3_rebookkept = rebookkeep(&mut state, &v3, "member prompt roster v3 restamped")?;
        // Every refresh commit in this fixture rewrites message range 0..1,
        // so the rebookkept parent's recorded span is its leading System
        // message.
        let respan = |state: &TranscriptHistoryState,
                      parent: &str|
         -> Result<String, Box<dyn std::error::Error>> {
            let body = state
                .revisions
                .iter()
                .find(|body| body.revision == parent)
                .expect("rebookkept parent body retained");
            Ok(transcript_messages_digest(&body.messages[0..1])?)
        };
        state.commits[1].original_span_digest = respan(&state, &v2_rebookkept)?;
        state.commits[1].parent_revision = v2_rebookkept;
        state.commits[2].original_span_digest = respan(&state, &v3_rebookkept)?;
        state.commits[2].parent_revision = v3_rebookkept;

        // The turn finally runs: two checkpointer-recorded appends.
        let mut incoming = session;
        incoming.set_metadata_unchecked_for_test(
            crate::session::SESSION_TRANSCRIPT_HISTORY_STATE_KEY,
            serde_json::to_value(&state)?,
        );
        let mut state = incoming
            .transcript_history_state()?
            .expect("history state survives rebookkeeping");
        for text in ["what was the codeword?", "birch seventeen"] {
            incoming.push(Message::User(UserMessage::text(text.to_string())));
            let appended_revision = incoming.transcript_revision()?;
            state
                .revisions
                .push(crate::session::TranscriptRevisionBody {
                    revision: appended_revision.clone(),
                    parent_revision: Some(state.head.clone()),
                    messages: incoming.messages().to_vec(),
                    created_at: SystemTime::now(),
                });
            state.head = appended_revision;
        }
        incoming.set_metadata_unchecked_for_test(
            crate::session::SESSION_TRANSCRIPT_HISTORY_STATE_KEY,
            serde_json::to_value(state)?,
        );

        run_boundary_snapshot_save_guard(&incoming, Some(&previous))?;
        Ok(())
    }

    /// Chained system-prompt-refresh commits with NO turn in between: the
    /// rewrite-chain walk must prove the plain append continuation from the
    /// persisted head instead of spuriously selecting an OLDER refresh
    /// commit (whose parent body also "extends" the head under the
    /// system-refresh equivalence), walking back onto its own cursor, and
    /// aborting as a cycle. Field regression (mobkit 0.7.23): idle mob
    /// members whose prompts carry drifting rosters get one refresh rewrite
    /// per boot; after two turn-less boots the next turn's run-boundary
    /// commit was rejected with "incoming append-only save would change
    /// retained transcript revision graph", permanently refusing resume.
    #[test]
    #[allow(clippy::expect_used)]
    fn boundary_commit_accepts_append_after_chained_promptless_system_refreshes()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut base = Session::new();
        base.push(Message::System(SystemMessage::new(
            "member prompt roster v1",
        )));
        base.push(Message::User(UserMessage::text(
            "the codeword is birch seventeen".to_string(),
        )));
        let v1 = base.transcript_revision()?;

        // Boot 1: resume refreshes the system prompt; the host dies before
        // any turn runs.
        let mut refreshed_once = base.clone();
        refreshed_once.commit_transcript_rewrite(
            TranscriptRewriteSelection::MessageRange { start: 0, end: 1 },
            vec![Message::System(SystemMessage::new(
                "member prompt roster v2",
            ))],
            crate::TranscriptRewriteReason::new("resume-system-prompt-refresh"),
            Some("agent-factory/resume".to_string()),
            Some(v1),
        )?;
        let v2 = refreshed_once.transcript_revision()?;

        // Boot 2: another turn-less refresh chains onto the graph.
        let mut previous = refreshed_once.clone();
        previous.commit_transcript_rewrite(
            TranscriptRewriteSelection::MessageRange { start: 0, end: 1 },
            vec![Message::System(SystemMessage::new(
                "member prompt roster v3",
            ))],
            crate::TranscriptRewriteReason::new("resume-system-prompt-refresh"),
            Some("agent-factory/resume".to_string()),
            Some(v2),
        )?;

        // Boot 3: the first turn finally runs. The intra-turn checkpointer
        // records one revision body per save, so the boundary commit's
        // incoming state carries MORE than one appended revision — the plain
        // +1 append validation cannot accept it and continuity must be
        // proven by the rewrite-chain walk.
        let mut incoming = previous.clone();
        let mut state = incoming
            .transcript_history_state()?
            .expect("chained refreshes retain history state");
        for text in ["what was the codeword?", "birch seventeen"] {
            incoming.push(Message::User(UserMessage::text(text.to_string())));
            let appended_revision = incoming.transcript_revision()?;
            state
                .revisions
                .push(crate::session::TranscriptRevisionBody {
                    revision: appended_revision.clone(),
                    parent_revision: Some(state.head.clone()),
                    messages: incoming.messages().to_vec(),
                    created_at: SystemTime::now(),
                });
            state.head = appended_revision;
        }
        incoming.set_metadata_unchecked_for_test(
            crate::session::SESSION_TRANSCRIPT_HISTORY_STATE_KEY,
            serde_json::to_value(state)?,
        );

        run_boundary_snapshot_save_guard(&incoming, Some(&previous))?;
        Ok(())
    }

    /// FOLD C: the canonical SessionDocumentMachine — not a handwritten shell
    /// boolean reducer — owns the live-vs-durable session-document authority
    /// verdict, the precedence (archived > uncommitted transcript > runtime
    /// system-context > stored transcript-revision), and the typed reason. This
    /// drives the classifier directly and asserts every authority/reason outcome
    /// and the precedence ordering.
    #[test]
    #[allow(clippy::expect_used)]
    fn classify_live_session_authority_is_decided_by_machine() {
        use crate::session_document::{
            LiveSessionAuthorityKind, LiveSessionAuthorityReason, SessionDocumentEffect,
            SessionDocumentMachineAuthority,
        };

        fn classify(
            stored_transcript_diverged: bool,
            live_has_uncommitted_transcript: bool,
            runtime_system_context_diverged: bool,
            stored_is_archived: bool,
        ) -> (LiveSessionAuthorityKind, LiveSessionAuthorityReason) {
            let mut authority = SessionDocumentMachineAuthority::new();
            let effects = authority
                .classify_live_session_authority(
                    stored_transcript_diverged,
                    live_has_uncommitted_transcript,
                    runtime_system_context_diverged,
                    stored_is_archived,
                )
                .expect("classifier must resolve a verdict");
            effects
                .iter()
                .find_map(|effect| match effect {
                    SessionDocumentEffect::LiveSessionAuthorityClassified { authority, reason } => {
                        Some((*authority, *reason))
                    }
                    _ => None,
                })
                .expect("classifier must emit a verdict")
        }

        // All four false -> LiveAuthoritative.
        let (kind, _) = classify(false, false, false, false);
        assert_eq!(kind, LiveSessionAuthorityKind::LiveAuthoritative);

        // Each divergence (in isolation) -> DurableAuthoritative with its reason.
        assert_eq!(
            classify(true, false, false, false),
            (
                LiveSessionAuthorityKind::DurableAuthoritative,
                LiveSessionAuthorityReason::StoredTranscriptRevisionDiverged
            ),
        );
        assert_eq!(
            classify(false, true, false, false),
            (
                LiveSessionAuthorityKind::DurableAuthoritative,
                LiveSessionAuthorityReason::LiveUncommittedTranscript
            ),
        );
        assert_eq!(
            classify(false, false, true, false),
            (
                LiveSessionAuthorityKind::DurableAuthoritative,
                LiveSessionAuthorityReason::RuntimeSystemContextDiverged
            ),
        );
        assert_eq!(
            classify(false, false, false, true),
            (
                LiveSessionAuthorityKind::DurableAuthoritative,
                LiveSessionAuthorityReason::StoredArchived
            ),
        );

        // Precedence: archived > uncommitted > system-context > revision.
        // When ALL four diverge, archived wins.
        assert_eq!(
            classify(true, true, true, true),
            (
                LiveSessionAuthorityKind::DurableAuthoritative,
                LiveSessionAuthorityReason::StoredArchived
            ),
        );
        // Not archived, but uncommitted + system-context + revision -> uncommitted.
        assert_eq!(
            classify(true, true, true, false),
            (
                LiveSessionAuthorityKind::DurableAuthoritative,
                LiveSessionAuthorityReason::LiveUncommittedTranscript
            ),
        );
        // Not archived, not uncommitted, but system-context + revision -> system-context.
        assert_eq!(
            classify(true, false, true, false),
            (
                LiveSessionAuthorityKind::DurableAuthoritative,
                LiveSessionAuthorityReason::RuntimeSystemContextDiverged
            ),
        );
    }

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
    fn append_only_guard_accepts_runtime_system_context_append()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut previous = Session::new();
        previous.push(Message::System(SystemMessage::new("base system")));
        previous.push(Message::User(UserMessage::text("hello".to_string())));

        let mut incoming = previous.clone();
        // The typed runtime-context-append producer stamps the system message's
        // mutation_kind so the save-guard admits the divergence from a typed
        // field, not the rendered `[Runtime System Context]` label.
        incoming.set_system_prompt_with_source(
            format!(
                "base system{SYSTEM_CONTEXT_SEPARATOR}[Runtime System Context]\nsource: unit-test\n\nextra context"
            ),
            crate::session_durable_config_authority::SessionSystemPromptSource::RuntimeContextAppend,
        )?;

        assert!(append_only_save_guard(&incoming, Some(&previous)).is_ok());
        Ok(())
    }

    #[test]
    fn append_only_guard_rejects_append_shaped_prompt_without_runtime_context_marker() {
        let mut previous = Session::new();
        previous.push(Message::System(SystemMessage::new("base system")));
        previous.push(Message::User(UserMessage::text("hello".to_string())));

        // Same rendered shape as a runtime context append, but produced via a
        // direct mutation (mutation_kind != RuntimeContextAppend). The typed
        // gate must reject it — content prefix alone is not authority.
        let mut incoming = previous.clone();
        incoming.set_system_prompt(format!(
            "base system{SYSTEM_CONTEXT_SEPARATOR}[Runtime System Context]\nsource: forged\n\nextra context"
        ));

        assert!(matches!(
            append_only_save_guard(&incoming, Some(&previous)),
            Err(SessionStoreError::TranscriptContinuityViolation { .. })
        ));
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
    fn run_boundary_guard_accepts_compaction_after_uncheckpointed_runtime_append()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut previous = Session::new();
        previous.push(Message::System(SystemMessage::new("base system")));
        previous.push(Message::User(UserMessage::text("turn one".to_string())));
        previous.push(Message::BlockAssistant(BlockAssistantMessage {
            blocks: vec![AssistantBlock::Text {
                text: "answer one".to_string(),
                meta: None,
            }],
            stop_reason: StopReason::EndTurn,
            identity: crate::types::TranscriptMessageIdentity::default(),
            created_at: crate::types::message_timestamp_now(),
        }));

        let mut parent = previous.clone();
        parent.set_system_prompt("refreshed runtime system projection".to_string());
        parent.push(Message::User(UserMessage::text(
            "runtime-only turn".to_string(),
        )));
        parent.push(Message::BlockAssistant(BlockAssistantMessage {
            blocks: vec![AssistantBlock::Text {
                text: "runtime-only answer".to_string(),
                meta: None,
            }],
            stop_reason: StopReason::EndTurn,
            identity: crate::types::TranscriptMessageIdentity::default(),
            created_at: crate::types::message_timestamp_now(),
        }));
        let parent_revision = parent.transcript_revision()?;

        let mut incoming = parent.clone();
        let mut replacement = vec![
            parent.messages()[0].clone(),
            Message::User(UserMessage::compaction_summary(
                "[Context compacted] summary".to_string(),
            )),
        ];
        replacement.extend_from_slice(&parent.messages()[1..]);
        incoming.commit_transcript_rewrite(
            TranscriptRewriteSelection::MessageRange {
                start: 0,
                end: parent.messages().len(),
            },
            replacement,
            crate::TranscriptRewriteReason::new("compaction"),
            Some("meerkat-core".to_string()),
            Some(parent_revision),
        )?;

        assert!(matches!(
            append_only_save_guard(&incoming, Some(&previous)),
            Err(SessionStoreError::TranscriptContinuityViolation { .. })
        ));
        assert!(run_boundary_snapshot_save_guard(&incoming, Some(&previous)).is_ok());
        Ok(())
    }

    #[test]
    fn run_boundary_guard_accepts_compaction_with_retained_tail_window()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut previous = Session::new();
        previous.push(Message::System(SystemMessage::new("base system")));
        previous.push(Message::User(UserMessage::text("turn one".to_string())));
        previous.push(Message::BlockAssistant(BlockAssistantMessage::new(
            vec![crate::types::AssistantBlock::Text {
                text: "answer one".to_string(),
                meta: None,
            }],
            StopReason::EndTurn,
        )));

        let mut parent = previous.clone();
        parent.set_system_prompt("refreshed runtime system projection".to_string());
        parent.push(Message::SystemNotice(SystemNoticeMessage::new(
            SystemNoticeKind::Comms,
            "peer response queued",
        )));
        let parent_revision = parent.transcript_revision()?;

        let mut incoming = parent.clone();
        let mut replacement = vec![
            parent.messages()[0].clone(),
            Message::User(UserMessage::compaction_summary(
                "[Context compacted] summary".to_string(),
            )),
        ];
        replacement.extend_from_slice(&parent.messages()[1..]);
        incoming.commit_transcript_rewrite(
            TranscriptRewriteSelection::MessageRange {
                start: 0,
                end: parent.messages().len(),
            },
            replacement,
            crate::TranscriptRewriteReason::new("compaction"),
            Some("meerkat-core".to_string()),
            Some(parent_revision),
        )?;

        assert!(matches!(
            append_only_save_guard(&incoming, Some(&previous)),
            Err(SessionStoreError::TranscriptContinuityViolation { .. })
        ));
        assert!(run_boundary_snapshot_save_guard(&incoming, Some(&previous)).is_ok());
        Ok(())
    }

    #[test]
    fn run_boundary_guard_rejects_commitless_history_parent_edge()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut previous = Session::new();
        previous.push(Message::System(SystemMessage::new("base system")));
        previous.push(Message::User(UserMessage::text("turn one".to_string())));
        let previous_revision = previous.transcript_revision()?;

        let mut incoming = previous.clone();
        incoming.set_system_prompt("forged replacement system".to_string());
        let incoming_revision = incoming.transcript_revision()?;
        let history = TranscriptHistoryState {
            head: incoming_revision.clone(),
            commits: Vec::new(),
            revisions: vec![
                crate::TranscriptRevisionBody {
                    revision: previous_revision,
                    parent_revision: None,
                    messages: previous.messages().to_vec(),
                    created_at: previous.updated_at(),
                },
                crate::TranscriptRevisionBody {
                    revision: incoming_revision,
                    parent_revision: Some(previous.transcript_revision()?),
                    messages: incoming.messages().to_vec(),
                    created_at: incoming.updated_at(),
                },
            ],
        };
        incoming.set_metadata_unchecked_for_test(
            crate::session::SESSION_TRANSCRIPT_HISTORY_STATE_KEY,
            serde_json::to_value(history)?,
        );

        assert!(matches!(
            append_only_save_guard(&incoming, Some(&previous)),
            Err(SessionStoreError::TranscriptContinuityViolation { .. })
        ));
        assert!(matches!(
            run_boundary_snapshot_save_guard(&incoming, Some(&previous)),
            Err(SessionStoreError::TranscriptContinuityViolation { .. }
                | SessionStoreError::MonotonicityViolation { .. })
        ));
        Ok(())
    }

    #[test]
    fn append_only_guard_rejects_history_head_that_does_not_match_current_messages()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut previous = Session::new();
        previous.push(Message::User(UserMessage::text("persisted".to_string())));

        let mut incoming = previous.clone();
        incoming.push(Message::User(UserMessage::text("append".to_string())));
        let poisoned_messages = vec![Message::User(UserMessage::text(
            "unrelated poisoned history".to_string(),
        ))];
        let poisoned_revision = transcript_messages_digest(&poisoned_messages)?;
        incoming.set_metadata_unchecked_for_test(
            crate::session::SESSION_TRANSCRIPT_HISTORY_STATE_KEY,
            serde_json::to_value(TranscriptHistoryState {
                head: poisoned_revision.clone(),
                commits: Vec::new(),
                revisions: vec![crate::TranscriptRevisionBody {
                    revision: poisoned_revision,
                    parent_revision: None,
                    messages: poisoned_messages,
                    created_at: incoming.updated_at(),
                }],
            })?,
        );

        assert!(matches!(
            append_only_save_guard(&incoming, Some(&previous)),
            Err(SessionStoreError::InvalidTranscriptRewrite { .. })
        ));
        assert!(matches!(
            append_only_save_guard(&incoming, None),
            Err(SessionStoreError::InvalidTranscriptRewrite { .. })
        ));
        assert!(matches!(
            run_boundary_snapshot_save_guard(&incoming, Some(&previous)),
            Err(SessionStoreError::InvalidTranscriptRewrite { .. })
        ));
        Ok(())
    }

    #[test]
    fn append_only_guard_rejects_new_rewrite_commits_on_plain_append()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut previous = Session::new();
        previous.push(Message::User(UserMessage::text("persisted".to_string())));
        let previous_revision = previous.transcript_revision()?;

        let mut incoming = previous.clone();
        let appended = Message::BlockAssistant(BlockAssistantMessage {
            blocks: vec![AssistantBlock::Text {
                text: "plain append".to_string(),
                meta: None,
            }],
            stop_reason: StopReason::EndTurn,
            identity: crate::types::TranscriptMessageIdentity::default(),
            created_at: crate::types::message_timestamp_now(),
        });
        incoming.commit_transcript_rewrite(
            TranscriptRewriteSelection::MessageRange { start: 1, end: 1 },
            vec![appended],
            crate::TranscriptRewriteReason::new("forged-append"),
            Some("unit-test".to_string()),
            Some(previous_revision),
        )?;

        assert!(matches!(
            append_only_save_guard(&incoming, Some(&previous)),
            Err(SessionStoreError::InvalidTranscriptRewrite { .. })
        ));
        Ok(())
    }

    #[test]
    fn append_only_guard_rejects_first_save_with_rewrite_commits()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut incoming = Session::new();
        incoming.push(Message::User(UserMessage::text("seed".to_string())));
        let parent_messages = incoming.messages().to_vec();
        let parent_updated_at = incoming.updated_at();
        let parent_revision = incoming.transcript_revision()?;
        let commit = incoming.commit_transcript_rewrite(
            TranscriptRewriteSelection::MessageRange { start: 0, end: 1 },
            vec![Message::User(UserMessage::text(
                "compacted seed".to_string(),
            ))],
            crate::TranscriptRewriteReason::new("compaction"),
            Some("meerkat-core".to_string()),
            Some(parent_revision),
        )?;
        let incoming_revision = incoming.transcript_revision()?;
        let commit_parent_revision = commit.parent_revision.clone();
        incoming.set_metadata_unchecked_for_test(
            crate::session::SESSION_TRANSCRIPT_HISTORY_STATE_KEY,
            serde_json::to_value(TranscriptHistoryState {
                head: incoming_revision.clone(),
                commits: vec![commit],
                revisions: vec![
                    crate::TranscriptRevisionBody {
                        revision: commit_parent_revision.clone(),
                        parent_revision: None,
                        messages: parent_messages,
                        created_at: parent_updated_at,
                    },
                    crate::TranscriptRevisionBody {
                        revision: incoming_revision,
                        parent_revision: Some(commit_parent_revision),
                        messages: incoming.messages().to_vec(),
                        created_at: incoming.updated_at(),
                    },
                ],
            })?,
        );

        assert!(matches!(
            append_only_save_guard(&incoming, None),
            Err(SessionStoreError::InvalidTranscriptRewrite { .. })
        ));
        Ok(())
    }

    #[test]
    fn transcript_rewrite_guard_rejects_poisoned_history_graph()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut previous = Session::new();
        previous.push(Message::User(UserMessage::text("persisted".to_string())));
        let parent_revision = previous.transcript_revision()?;

        let mut first = previous.clone();
        let first_commit = first.commit_transcript_rewrite(
            TranscriptRewriteSelection::MessageRange { start: 0, end: 1 },
            vec![Message::User(UserMessage::text(
                "compacted persisted".to_string(),
            ))],
            crate::TranscriptRewriteReason::new("compaction"),
            Some("unit-test".to_string()),
            Some(parent_revision),
        )?;
        let first_snapshot = first.clone();

        first.commit_transcript_rewrite(
            TranscriptRewriteSelection::MessageRange { start: 0, end: 1 },
            vec![Message::User(UserMessage::text(
                "uncommitted poisoned fork".to_string(),
            ))],
            crate::TranscriptRewriteReason::new("poison"),
            Some("unit-test".to_string()),
            Some(first_commit.revision.clone()),
        )?;
        let mut poisoned_state = first
            .transcript_history_state()?
            .ok_or_else(|| "second rewrite should retain history state".to_string())?;
        poisoned_state.head = first_commit.revision.clone();

        let mut poisoned = first_snapshot;
        poisoned.set_metadata_unchecked_for_test(
            crate::session::SESSION_TRANSCRIPT_HISTORY_STATE_KEY,
            serde_json::to_value(poisoned_state)?,
        );

        assert!(matches!(
            transcript_rewrite_save_guard(&poisoned, Some(&previous), &first_commit),
            Err(SessionStoreError::InvalidTranscriptRewrite { reason, .. })
                if reason.contains("incoming transcript history state is malformed")
        ));
        Ok(())
    }

    #[test]
    fn authoritative_projection_guard_rejects_changed_persisted_revision()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut previous = Session::new();
        previous.push(Message::User(UserMessage::text("persisted A".to_string())));
        let expected_revision = previous.transcript_revision()?;

        let mut current = previous.clone();
        current.push(Message::BlockAssistant(BlockAssistantMessage {
            blocks: vec![AssistantBlock::Text {
                text: "persisted B".to_string(),
                meta: None,
            }],
            stop_reason: StopReason::EndTurn,
            identity: crate::types::TranscriptMessageIdentity::default(),
            created_at: crate::types::message_timestamp_now(),
        }));
        let mut incoming = previous.clone();
        incoming.push(Message::User(UserMessage::text(
            "incoming from A".to_string(),
        )));

        assert!(matches!(
            authoritative_projection_current_revision_guard(
                &incoming,
                Some(&current),
                Some(&expected_revision)
            ),
            Err(SessionStoreError::TranscriptContinuityViolation { .. })
        ));
        Ok(())
    }

    #[test]
    fn append_only_guard_rejects_rewrite_commits_on_first_save()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut incoming = Session::new();
        incoming.push(Message::User(UserMessage::text("persisted".to_string())));
        let parent_revision = incoming.transcript_revision()?;
        incoming.commit_transcript_rewrite(
            TranscriptRewriteSelection::MessageRange { start: 0, end: 1 },
            vec![Message::User(UserMessage::text("rewritten".to_string()))],
            crate::TranscriptRewriteReason::new("forged-first-save"),
            Some("unit-test".to_string()),
            Some(parent_revision),
        )?;

        assert!(matches!(
            append_only_save_guard(&incoming, None),
            Err(SessionStoreError::InvalidTranscriptRewrite { .. })
        ));
        Ok(())
    }

    #[test]
    fn append_only_guard_rejects_commitless_history_on_first_save()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut incoming = Session::new();
        incoming.push(Message::User(UserMessage::text("persisted".to_string())));
        let incoming_revision = incoming.transcript_revision()?;
        incoming.set_metadata_unchecked_for_test(
            crate::session::SESSION_TRANSCRIPT_HISTORY_STATE_KEY,
            serde_json::to_value(TranscriptHistoryState {
                head: incoming_revision.clone(),
                commits: Vec::new(),
                revisions: vec![crate::TranscriptRevisionBody {
                    revision: incoming_revision,
                    parent_revision: None,
                    messages: incoming.messages().to_vec(),
                    created_at: incoming.updated_at(),
                }],
            })?,
        );

        assert!(matches!(
            append_only_save_guard(&incoming, None),
            Err(SessionStoreError::InvalidTranscriptRewrite { reason, .. })
                if reason.contains("first save would seed transcript history state")
        ));
        Ok(())
    }

    #[test]
    fn run_boundary_guard_adopts_commit_carrying_history_on_first_commit()
    -> Result<(), Box<dyn std::error::Error>> {
        // A runtime authority adopting a session it never snapshotted
        // (resume/import over fresh runtime state) may receive a session that
        // already carries a typed rewrite graph — e.g. a resume-time
        // base-prompt refresh. The commits are the audit: the run-boundary
        // guard accepts the validated graph, while the plain trait-level
        // `SessionStore::save` contract keeps rejecting first-save seeds.
        let mut incoming = Session::new();
        incoming.set_system_prompt("old base".to_string());
        incoming.push(Message::User(UserMessage::text("hello".to_string())));
        incoming.commit_transcript_rewrite(
            crate::TranscriptRewriteSelection::MessageRange { start: 0, end: 1 },
            vec![Message::System(SystemMessage::with_mutation_kind(
                "new base".to_string(),
                crate::types::SystemPromptMutationKind::ExplicitBuild,
            ))],
            crate::TranscriptRewriteReason::new("resume-system-prompt-refresh"),
            None,
            None,
        )?;

        assert!(run_boundary_snapshot_save_guard(&incoming, None).is_ok());
        assert!(matches!(
            append_only_save_guard(&incoming, None),
            Err(SessionStoreError::InvalidTranscriptRewrite { .. })
        ));
        Ok(())
    }

    #[test]
    fn run_boundary_guard_rejects_untyped_leading_system_refresh_after_head_rewrite()
    -> Result<(), Box<dyn std::error::Error>> {
        // A same-length rewrite commit sitting exactly at the persisted head
        // (the resume-refresh shape) must not widen acceptance to UNTYPED
        // leading-System replacements: a plain set_system_prompt records a
        // refresh body via the head refresh but carries no commit, and the
        // chain walker's fallback must not admit it as an ordinary append
        // continuation via the leading-system-refresh equivalence.
        let mut previous = Session::new();
        previous.set_system_prompt("original base".to_string());
        previous.push(Message::User(UserMessage::text("hello".to_string())));
        previous.commit_transcript_rewrite(
            crate::TranscriptRewriteSelection::MessageRange { start: 0, end: 1 },
            vec![Message::System(SystemMessage::with_mutation_kind(
                "refreshed base".to_string(),
                crate::types::SystemPromptMutationKind::ExplicitBuild,
            ))],
            crate::TranscriptRewriteReason::new("resume-system-prompt-refresh"),
            None,
            None,
        )?;

        let mut incoming = previous.clone();
        incoming.set_system_prompt("untyped hijack".to_string());

        assert!(matches!(
            run_boundary_snapshot_save_guard(&incoming, Some(&previous)),
            Err(SessionStoreError::TranscriptContinuityViolation { .. })
        ));
        Ok(())
    }

    #[test]
    fn run_boundary_guard_accepts_plain_append_after_head_rewrite()
    -> Result<(), Box<dyn std::error::Error>> {
        // The empty-chain acceptance the cycle-skip exists for: the persisted
        // row is already AT the rewrite revision and the incoming snapshot
        // extends it by ordinary appends.
        let mut previous = Session::new();
        previous.set_system_prompt("original base".to_string());
        previous.push(Message::User(UserMessage::text("hello".to_string())));
        previous.commit_transcript_rewrite(
            crate::TranscriptRewriteSelection::MessageRange { start: 0, end: 1 },
            vec![Message::System(SystemMessage::with_mutation_kind(
                "refreshed base".to_string(),
                crate::types::SystemPromptMutationKind::ExplicitBuild,
            ))],
            crate::TranscriptRewriteReason::new("resume-system-prompt-refresh"),
            None,
            None,
        )?;

        let mut incoming = previous.clone();
        incoming.push(Message::User(UserMessage::text(
            "post-rewrite turn".to_string(),
        )));

        assert!(run_boundary_snapshot_save_guard(&incoming, Some(&previous)).is_ok());
        Ok(())
    }

    #[test]
    fn append_only_guard_rejects_commitless_history_seed_on_plain_append()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut previous = Session::new();
        previous.push(Message::User(UserMessage::text("persisted".to_string())));
        let previous_revision = previous.transcript_revision()?;

        let mut incoming = previous.clone();
        incoming.push(Message::BlockAssistant(BlockAssistantMessage {
            blocks: vec![AssistantBlock::Text {
                text: "plain append".to_string(),
                meta: None,
            }],
            stop_reason: StopReason::EndTurn,
            identity: crate::types::TranscriptMessageIdentity::default(),
            created_at: crate::types::message_timestamp_now(),
        }));
        let incoming_revision = incoming.transcript_revision()?;
        incoming.set_metadata_unchecked_for_test(
            crate::session::SESSION_TRANSCRIPT_HISTORY_STATE_KEY,
            serde_json::to_value(TranscriptHistoryState {
                head: incoming_revision.clone(),
                commits: Vec::new(),
                revisions: vec![
                    crate::TranscriptRevisionBody {
                        revision: previous_revision,
                        parent_revision: None,
                        messages: previous.messages().to_vec(),
                        created_at: previous.updated_at(),
                    },
                    crate::TranscriptRevisionBody {
                        revision: incoming_revision,
                        parent_revision: Some(previous.transcript_revision()?),
                        messages: incoming.messages().to_vec(),
                        created_at: incoming.updated_at(),
                    },
                ],
            })?,
        );

        assert!(matches!(
            append_only_save_guard(&incoming, Some(&previous)),
            Err(SessionStoreError::InvalidTranscriptRewrite { reason, .. })
                if reason.contains("append-only save would seed transcript history state")
        ));
        Ok(())
    }

    #[test]
    fn run_boundary_guard_accepts_commitless_history_seed_on_plain_append()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut previous = Session::new();
        previous.push(Message::User(UserMessage::text("persisted".to_string())));
        let previous_revision = previous.transcript_revision()?;

        let mut incoming = previous.clone();
        incoming.push(Message::BlockAssistant(BlockAssistantMessage {
            blocks: vec![AssistantBlock::Text {
                text: "plain append".to_string(),
                meta: None,
            }],
            stop_reason: StopReason::EndTurn,
            identity: crate::types::TranscriptMessageIdentity::default(),
            created_at: crate::types::message_timestamp_now(),
        }));
        let incoming_revision = incoming.transcript_revision()?;
        incoming.set_metadata_unchecked_for_test(
            crate::session::SESSION_TRANSCRIPT_HISTORY_STATE_KEY,
            serde_json::to_value(TranscriptHistoryState {
                head: incoming_revision.clone(),
                commits: Vec::new(),
                revisions: vec![
                    crate::TranscriptRevisionBody {
                        revision: previous_revision.clone(),
                        parent_revision: None,
                        messages: previous.messages().to_vec(),
                        created_at: previous.updated_at(),
                    },
                    crate::TranscriptRevisionBody {
                        revision: incoming_revision,
                        parent_revision: Some(previous_revision),
                        messages: incoming.messages().to_vec(),
                        created_at: incoming.updated_at(),
                    },
                ],
            })?,
        );

        assert!(append_only_save_guard(&incoming, Some(&previous)).is_err());
        assert!(run_boundary_snapshot_save_guard(&incoming, Some(&previous)).is_ok());
        Ok(())
    }

    #[test]
    fn run_boundary_guard_accepts_retained_history_seed_on_plain_append()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut original = Session::new();
        original.push(Message::User(UserMessage::text("verbose seed".to_string())));
        let original_revision = original.transcript_revision()?;

        let mut previous = original.clone();
        previous.commit_transcript_rewrite(
            TranscriptRewriteSelection::MessageRange { start: 0, end: 1 },
            vec![Message::User(UserMessage::text(
                "compacted seed".to_string(),
            ))],
            crate::TranscriptRewriteReason::new("compaction"),
            Some("meerkat-core".to_string()),
            Some(original_revision),
        )?;
        let previous_with_history = previous.clone();
        previous.clear_transcript_history_state();

        let mut incoming = previous_with_history;
        incoming.push(Message::BlockAssistant(BlockAssistantMessage {
            blocks: vec![AssistantBlock::Text {
                text: "plain append after retained history".to_string(),
                meta: None,
            }],
            stop_reason: StopReason::EndTurn,
            identity: crate::types::TranscriptMessageIdentity::default(),
            created_at: crate::types::message_timestamp_now(),
        }));

        assert!(append_only_save_guard(&incoming, Some(&previous)).is_err());
        assert!(run_boundary_snapshot_save_guard(&incoming, Some(&previous)).is_ok());
        Ok(())
    }

    #[test]
    fn run_boundary_guard_accepts_commitless_history_seed_on_first_snapshot()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut incoming = Session::new();
        incoming.push(Message::User(UserMessage::text("persisted".to_string())));
        let incoming_revision = incoming.transcript_revision()?;
        incoming.set_metadata_unchecked_for_test(
            crate::session::SESSION_TRANSCRIPT_HISTORY_STATE_KEY,
            serde_json::to_value(TranscriptHistoryState {
                head: incoming_revision.clone(),
                commits: Vec::new(),
                revisions: vec![crate::TranscriptRevisionBody {
                    revision: incoming_revision,
                    parent_revision: None,
                    messages: incoming.messages().to_vec(),
                    created_at: incoming.updated_at(),
                }],
            })?,
        );

        assert!(append_only_save_guard(&incoming, None).is_err());
        assert!(run_boundary_snapshot_save_guard(&incoming, None).is_ok());
        Ok(())
    }

    #[test]
    fn run_boundary_guard_accepts_commitless_history_seed_on_initial_multi_revision_snapshot()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut base = Session::new();
        base.push(Message::User(UserMessage::text("first".to_string())));
        let base_revision = base.transcript_revision()?;

        let mut incoming = base.clone();
        incoming.push(Message::BlockAssistant(BlockAssistantMessage {
            blocks: vec![AssistantBlock::Text {
                text: "second".to_string(),
                meta: None,
            }],
            stop_reason: StopReason::EndTurn,
            identity: crate::types::TranscriptMessageIdentity::default(),
            created_at: crate::types::message_timestamp_now(),
        }));
        let incoming_revision = incoming.transcript_revision()?;
        incoming.set_metadata_unchecked_for_test(
            crate::session::SESSION_TRANSCRIPT_HISTORY_STATE_KEY,
            serde_json::to_value(TranscriptHistoryState {
                head: incoming_revision.clone(),
                commits: Vec::new(),
                revisions: vec![
                    crate::TranscriptRevisionBody {
                        revision: base_revision.clone(),
                        parent_revision: None,
                        messages: base.messages().to_vec(),
                        created_at: base.updated_at(),
                    },
                    crate::TranscriptRevisionBody {
                        revision: incoming_revision,
                        parent_revision: Some(base_revision),
                        messages: incoming.messages().to_vec(),
                        created_at: incoming.updated_at(),
                    },
                ],
            })?,
        );

        assert!(append_only_save_guard(&incoming, None).is_err());
        assert!(run_boundary_snapshot_save_guard(&incoming, None).is_ok());
        Ok(())
    }

    #[test]
    fn append_only_guard_rejects_new_rewrite_commits_on_system_context_append()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut previous = Session::new();
        previous.push(Message::System(SystemMessage::new("base system")));
        previous.push(Message::User(UserMessage::text("persisted".to_string())));
        let mut incoming = previous.clone();
        incoming.set_system_prompt_with_source(
            format!(
                "base system{SYSTEM_CONTEXT_SEPARATOR}[Runtime System Context]\nsource: unit-test\n\nextra context"
            ),
            crate::session_durable_config_authority::SessionSystemPromptSource::RuntimeContextAppend,
        )?;
        incoming.push(Message::BlockAssistant(BlockAssistantMessage {
            blocks: vec![AssistantBlock::Text {
                text: "plain append".to_string(),
                meta: None,
            }],
            stop_reason: StopReason::EndTurn,
            identity: crate::types::TranscriptMessageIdentity::default(),
            created_at: crate::types::message_timestamp_now(),
        }));
        let incoming_revision = incoming.transcript_revision()?;
        incoming.set_metadata_unchecked_for_test(
            crate::session::SESSION_TRANSCRIPT_HISTORY_STATE_KEY,
            serde_json::to_value(TranscriptHistoryState {
                head: incoming_revision.clone(),
                commits: vec![TranscriptRewriteCommit {
                    parent_revision: previous.transcript_revision()?,
                    revision: incoming_revision.clone(),
                    selection: TranscriptRewriteSelection::MessageRange { start: 0, end: 0 },
                    original_span_digest: transcript_messages_digest(&[])?,
                    replacement_digest: transcript_messages_digest(&[])?,
                    messages_before: previous.messages().len(),
                    messages_after: incoming.messages().len(),
                    reason: crate::TranscriptRewriteReason::new("forged"),
                    actor: Some("unit-test".to_string()),
                    committed_at: incoming.updated_at(),
                }],
                revisions: vec![crate::TranscriptRevisionBody {
                    revision: incoming_revision,
                    parent_revision: None,
                    messages: incoming.messages().to_vec(),
                    created_at: incoming.updated_at(),
                }],
            })?,
        );

        assert!(matches!(
            append_only_save_guard(&incoming, Some(&previous)),
            Err(SessionStoreError::InvalidTranscriptRewrite { .. })
        ));
        Ok(())
    }

    #[test]
    fn append_only_guard_rejects_new_rewrite_commits_on_transient_notice_cleanup()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut previous = Session::new();
        previous.push(Message::SystemNotice(SystemNoticeMessage::new(
            SystemNoticeKind::Comms,
            "transient peer delivery notice",
        )));
        previous.push(Message::User(UserMessage::text("persisted".to_string())));

        let mut incoming = Session::new();
        incoming.push(Message::User(UserMessage::text("persisted".to_string())));
        incoming.push(Message::BlockAssistant(BlockAssistantMessage {
            blocks: vec![AssistantBlock::Text {
                text: "plain append after notice cleanup".to_string(),
                meta: None,
            }],
            stop_reason: StopReason::EndTurn,
            identity: crate::types::TranscriptMessageIdentity::default(),
            created_at: crate::types::message_timestamp_now(),
        }));
        let incoming_revision = incoming.transcript_revision()?;
        incoming.set_metadata_unchecked_for_test(
            crate::session::SESSION_TRANSCRIPT_HISTORY_STATE_KEY,
            serde_json::to_value(TranscriptHistoryState {
                head: incoming_revision.clone(),
                commits: vec![TranscriptRewriteCommit {
                    parent_revision: previous.transcript_revision()?,
                    revision: incoming_revision.clone(),
                    selection: TranscriptRewriteSelection::MessageRange { start: 0, end: 0 },
                    original_span_digest: transcript_messages_digest(&[])?,
                    replacement_digest: transcript_messages_digest(&[])?,
                    messages_before: previous.messages().len(),
                    messages_after: incoming.messages().len(),
                    reason: crate::TranscriptRewriteReason::new("forged"),
                    actor: Some("unit-test".to_string()),
                    committed_at: incoming.updated_at(),
                }],
                revisions: vec![crate::TranscriptRevisionBody {
                    revision: incoming_revision,
                    parent_revision: None,
                    messages: incoming.messages().to_vec(),
                    created_at: incoming.updated_at(),
                }],
            })?,
        );

        assert!(matches!(
            append_only_save_guard(&incoming, Some(&previous)),
            Err(SessionStoreError::InvalidTranscriptRewrite { .. })
        ));
        Ok(())
    }

    #[test]
    fn append_only_guard_accepts_mechanical_background_notice_refresh_after_history()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut previous = Session::new();
        previous.push(Message::User(UserMessage::text("before".to_string())));
        previous.commit_transcript_rewrite(
            TranscriptRewriteSelection::MessageRange { start: 0, end: 1 },
            vec![Message::User(UserMessage::text("after".to_string()))],
            crate::TranscriptRewriteReason::new("unit-test-edit"),
            Some("unit-test".to_string()),
            None,
        )?;
        previous.replace_synthetic_notices(
            SystemNoticeKind::BackgroundJob,
            vec![Message::SystemNotice(SystemNoticeMessage::new(
                SystemNoticeKind::BackgroundJob,
                "job complete",
            ))],
        )?;

        let mut incoming = previous.clone();
        incoming.replace_synthetic_notices(SystemNoticeKind::BackgroundJob, Vec::new())?;
        incoming.push(Message::User(UserMessage::text("next turn".to_string())));

        append_only_save_guard(&incoming, Some(&previous))?;
        assert_eq!(incoming.transcript_rewrite_generation()?, 1);
        Ok(())
    }

    #[test]
    fn append_only_guard_rejects_removing_persisted_mcp_pending_notice()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut previous = Session::new();
        previous.push(Message::User(UserMessage::text("before".to_string())));
        previous.push(Message::SystemNotice(SystemNoticeMessage::with_block(
            SystemNoticeKind::McpPending,
            Some("persisted pending fact".to_string()),
            SystemNoticeBlock::Mcp {
                server_id: Some("server".to_string()),
                operation: None,
                phase: None,
                persisted: true,
                detail: None,
                pending_sources: Vec::new(),
            },
        )));

        let mut incoming = previous.clone();
        incoming.messages = std::sync::Arc::new(
            previous
                .messages()
                .iter()
                .filter(|message| !matches!(message, Message::SystemNotice(_)))
                .cloned()
                .collect(),
        );
        incoming.push(Message::User(UserMessage::text("after".to_string())));

        assert!(append_only_save_guard(&incoming, Some(&previous)).is_err());
        Ok(())
    }

    #[test]
    fn run_boundary_guard_rejects_mutated_prior_audited_body_metadata()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut previous = Session::new();
        previous.push(Message::User(UserMessage::text("A".to_string())));
        previous.commit_transcript_rewrite(
            TranscriptRewriteSelection::MessageRange { start: 0, end: 1 },
            vec![Message::User(UserMessage::text("B".to_string()))],
            crate::TranscriptRewriteReason::new("first"),
            Some("unit-test".to_string()),
            None,
        )?;

        let mut incoming = previous.clone();
        incoming.push(Message::User(UserMessage::text(
            "ordinary append".to_string(),
        )));
        incoming.commit_transcript_rewrite(
            TranscriptRewriteSelection::MessageRange { start: 1, end: 2 },
            vec![Message::User(UserMessage::text(
                "rewritten append".to_string(),
            ))],
            crate::TranscriptRewriteReason::new("second"),
            Some("unit-test".to_string()),
            None,
        )?;
        let mut state = incoming
            .transcript_history_state()?
            .ok_or_else(|| std::io::Error::other("incoming history missing"))?;
        let old_parent = state.commits[0].parent_revision.clone();
        state
            .revisions
            .iter_mut()
            .find(|body| body.revision == old_parent)
            .ok_or_else(|| std::io::Error::other("old audited parent missing"))?
            .parent_revision = Some("sha256:forged-lineage-parent".to_string());
        incoming.set_metadata_unchecked_for_test(
            crate::session::SESSION_TRANSCRIPT_HISTORY_STATE_KEY,
            serde_json::to_value(state)?,
        );

        assert!(matches!(
            run_boundary_snapshot_save_guard(&incoming, Some(&previous)),
            Err(SessionStoreError::InvalidTranscriptRewrite { reason, .. })
                if reason.contains("changes audited transcript body")
        ));
        Ok(())
    }

    #[test]
    fn run_boundary_guard_accepts_generated_context_summary_before_retained_tail()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut previous = Session::new();
        previous.push(Message::System(SystemMessage::new(
            "runtime system before context refresh",
        )));
        previous.push(Message::User(UserMessage::text(
            "Turn 1 request".to_string(),
        )));
        previous.push(Message::BlockAssistant(BlockAssistantMessage {
            blocks: vec![AssistantBlock::Text {
                text: "Turn 1 answer".to_string(),
                meta: None,
            }],
            stop_reason: StopReason::EndTurn,
            identity: crate::types::TranscriptMessageIdentity::default(),
            created_at: crate::types::message_timestamp_now(),
        }));

        let mut incoming = Session::with_id(previous.id().clone());
        incoming.push(Message::System(SystemMessage::new(
            "runtime system after context refresh",
        )));
        incoming.push(Message::User(UserMessage::text(
            "Verbose context that will be compacted".to_string(),
        )));
        for message in previous.messages()[1..].iter().cloned() {
            incoming.push(message);
        }
        incoming.push(Message::BlockAssistant(BlockAssistantMessage {
            blocks: vec![AssistantBlock::Text {
                text: "Turn 2 generated answer".to_string(),
                meta: None,
            }],
            stop_reason: StopReason::EndTurn,
            identity: crate::types::TranscriptMessageIdentity::default(),
            created_at: crate::types::message_timestamp_now(),
        }));
        let parent_revision = incoming.transcript_revision()?;
        incoming.commit_transcript_rewrite(
            TranscriptRewriteSelection::MessageRange { start: 1, end: 2 },
            vec![Message::User(UserMessage::compaction_summary(
                "[Context compacted] Earlier runtime context".to_string(),
            ))],
            crate::TranscriptRewriteReason::new("compaction"),
            Some("meerkat-core".to_string()),
            Some(parent_revision),
        )?;

        assert!(append_only_save_guard(&incoming, Some(&previous)).is_err());
        assert!(run_boundary_snapshot_save_guard(&incoming, Some(&previous)).is_ok());
        Ok(())
    }

    #[test]
    fn run_boundary_guard_rejects_context_summary_tail_without_compaction_summary_marker()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut previous = Session::new();
        previous.push(Message::System(SystemMessage::new(
            "runtime system before context refresh",
        )));
        previous.push(Message::User(UserMessage::text(
            "Turn 1 request".to_string(),
        )));
        previous.push(Message::BlockAssistant(BlockAssistantMessage {
            blocks: vec![AssistantBlock::Text {
                text: "Turn 1 answer".to_string(),
                meta: None,
            }],
            stop_reason: StopReason::EndTurn,
            identity: crate::types::TranscriptMessageIdentity::default(),
            created_at: crate::types::message_timestamp_now(),
        }));

        let mut incoming = Session::with_id(previous.id().clone());
        incoming.push(Message::System(SystemMessage::new(
            "runtime system after context refresh",
        )));
        incoming.push(Message::User(UserMessage::text(
            "Verbose context that will be compacted".to_string(),
        )));
        for message in previous.messages()[1..].iter().cloned() {
            incoming.push(message);
        }
        incoming.push(Message::BlockAssistant(BlockAssistantMessage {
            blocks: vec![AssistantBlock::Text {
                text: "Turn 2 generated answer".to_string(),
                meta: None,
            }],
            stop_reason: StopReason::EndTurn,
            identity: crate::types::TranscriptMessageIdentity::default(),
            created_at: crate::types::message_timestamp_now(),
        }));
        let parent_revision = incoming.transcript_revision()?;
        // Same rendered shape (content begins with `[Context compacted]`) but the
        // summary message uses the ordinary conversational role. The typed gate
        // must reject it: rendered content alone is not authority.
        incoming.commit_transcript_rewrite(
            TranscriptRewriteSelection::MessageRange { start: 1, end: 2 },
            vec![Message::User(UserMessage::text(
                "[Context compacted] Earlier runtime context".to_string(),
            ))],
            crate::TranscriptRewriteReason::new("compaction"),
            Some("meerkat-core".to_string()),
            Some(parent_revision),
        )?;

        assert!(append_only_save_guard(&incoming, Some(&previous)).is_err());
        assert!(matches!(
            run_boundary_snapshot_save_guard(&incoming, Some(&previous)),
            Err(SessionStoreError::TranscriptContinuityViolation { .. }
                | SessionStoreError::MonotonicityViolation { .. })
        ));
        Ok(())
    }

    /// Ask 1 save-guard invariant: the injected-context transcript role must
    /// NOT satisfy the transcript-continuity save-guard. Only the
    /// runtime-minted `CompactionSummary` role admits a divergent rewrite
    /// parent (`is_compaction_summary()` stays `CompactionSummary`-only).
    #[test]
    fn run_boundary_guard_rejects_context_summary_tail_with_injected_context_marker()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut previous = Session::new();
        previous.push(Message::System(SystemMessage::new(
            "runtime system before context refresh",
        )));
        previous.push(Message::User(UserMessage::text(
            "Turn 1 request".to_string(),
        )));
        previous.push(Message::BlockAssistant(BlockAssistantMessage {
            blocks: vec![AssistantBlock::Text {
                text: "Turn 1 answer".to_string(),
                meta: None,
            }],
            stop_reason: StopReason::EndTurn,
            identity: crate::types::TranscriptMessageIdentity::default(),
            created_at: crate::types::message_timestamp_now(),
        }));

        let mut incoming = Session::with_id(previous.id().clone());
        incoming.push(Message::System(SystemMessage::new(
            "runtime system after context refresh",
        )));
        incoming.push(Message::User(UserMessage::text(
            "Verbose context that will be compacted".to_string(),
        )));
        for message in previous.messages()[1..].iter().cloned() {
            incoming.push(message);
        }
        incoming.push(Message::BlockAssistant(BlockAssistantMessage {
            blocks: vec![AssistantBlock::Text {
                text: "Turn 2 generated answer".to_string(),
                meta: None,
            }],
            stop_reason: StopReason::EndTurn,
            identity: crate::types::TranscriptMessageIdentity::default(),
            created_at: crate::types::message_timestamp_now(),
        }));
        let parent_revision = incoming.transcript_revision()?;
        // Same rendered shape, but the boundary message carries the typed
        // injected-context role instead of the compaction-summary role. The
        // guard reads the typed marker: injected context is host-attached
        // ambient content, not a runtime compaction boundary.
        incoming.commit_transcript_rewrite(
            TranscriptRewriteSelection::MessageRange { start: 1, end: 2 },
            vec![Message::User(UserMessage::injected_context(
                "[Context compacted] Earlier runtime context".to_string(),
            ))],
            crate::TranscriptRewriteReason::new("compaction"),
            Some("meerkat-core".to_string()),
            Some(parent_revision),
        )?;

        assert!(append_only_save_guard(&incoming, Some(&previous)).is_err());
        assert!(matches!(
            run_boundary_snapshot_save_guard(&incoming, Some(&previous)),
            Err(SessionStoreError::TranscriptContinuityViolation { .. }
                | SessionStoreError::MonotonicityViolation { .. })
        ));
        Ok(())
    }

    #[test]
    fn run_boundary_guard_rejects_runtime_parent_with_inserted_message_before_tail()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut previous = Session::new();
        previous.push(Message::System(SystemMessage::new("base system")));
        previous.push(Message::User(UserMessage::text("turn one".to_string())));
        previous.push(Message::BlockAssistant(BlockAssistantMessage {
            blocks: vec![AssistantBlock::Text {
                text: "answer one".to_string(),
                meta: None,
            }],
            stop_reason: StopReason::EndTurn,
            identity: crate::types::TranscriptMessageIdentity::default(),
            created_at: crate::types::message_timestamp_now(),
        }));

        let parent_messages = vec![
            Message::System(SystemMessage::new("refreshed runtime system projection")),
            Message::User(UserMessage::text(
                "injected before retained tail".to_string(),
            )),
            previous.messages()[1].clone(),
            previous.messages()[2].clone(),
        ];
        let parent_revision = transcript_messages_digest(&parent_messages)?;
        let mut parent = previous.clone();
        parent.apply_transcript_history_state(TranscriptHistoryState {
            head: parent_revision.clone(),
            commits: Vec::new(),
            revisions: vec![crate::TranscriptRevisionBody {
                revision: parent_revision,
                parent_revision: None,
                messages: parent_messages,
                created_at: parent.updated_at(),
            }],
        })?;
        let parent_revision = parent.transcript_revision()?;

        let mut incoming = parent.clone();
        incoming.commit_transcript_rewrite(
            TranscriptRewriteSelection::MessageRange {
                start: 0,
                end: parent.messages().len(),
            },
            vec![Message::User(UserMessage::text(
                "[Context compacted] summary".to_string(),
            ))],
            crate::TranscriptRewriteReason::new("compaction"),
            Some("meerkat-core".to_string()),
            Some(parent_revision),
        )?;

        assert!(matches!(
            run_boundary_snapshot_save_guard(&incoming, Some(&previous)),
            Err(SessionStoreError::TranscriptContinuityViolation { .. }
                | SessionStoreError::MonotonicityViolation { .. })
        ));
        Ok(())
    }

    #[test]
    fn run_boundary_guard_rejects_forged_parent_edge_before_real_rewrite_commit()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut previous = Session::new();
        previous.push(Message::System(SystemMessage::new("base system")));
        previous.push(Message::User(UserMessage::text("turn one".to_string())));
        previous.push(Message::BlockAssistant(BlockAssistantMessage {
            blocks: vec![AssistantBlock::Text {
                text: "answer one".to_string(),
                meta: None,
            }],
            stop_reason: StopReason::EndTurn,
            identity: crate::types::TranscriptMessageIdentity::default(),
            created_at: crate::types::message_timestamp_now(),
        }));
        let previous_revision = previous.transcript_revision()?;

        let forged_parent_messages = vec![
            Message::System(SystemMessage::new("refreshed runtime system projection")),
            Message::User(UserMessage::text(
                "forged insertion before retained tail".to_string(),
            )),
            previous.messages()[1].clone(),
            previous.messages()[2].clone(),
        ];
        let forged_parent_revision = transcript_messages_digest(&forged_parent_messages)?;
        let mut forged_parent = previous.clone();
        forged_parent.apply_transcript_history_state(TranscriptHistoryState {
            head: forged_parent_revision.clone(),
            commits: Vec::new(),
            revisions: vec![
                crate::TranscriptRevisionBody {
                    revision: previous_revision.clone(),
                    parent_revision: None,
                    messages: previous.messages().to_vec(),
                    created_at: previous.updated_at(),
                },
                crate::TranscriptRevisionBody {
                    revision: forged_parent_revision.clone(),
                    parent_revision: Some(previous_revision),
                    messages: forged_parent_messages,
                    created_at: forged_parent.updated_at(),
                },
            ],
        })?;

        let mut incoming = forged_parent.clone();
        incoming.commit_transcript_rewrite(
            TranscriptRewriteSelection::MessageRange {
                start: 0,
                end: forged_parent.messages().len(),
            },
            vec![Message::User(UserMessage::text(
                "[Context compacted] forged branch".to_string(),
            ))],
            crate::TranscriptRewriteReason::new("compaction"),
            Some("meerkat-core".to_string()),
            Some(forged_parent_revision),
        )?;

        assert!(matches!(
            run_boundary_snapshot_save_guard(&incoming, Some(&previous)),
            Err(SessionStoreError::TranscriptContinuityViolation { .. }
                | SessionStoreError::MonotonicityViolation { .. })
        ));
        Ok(())
    }

    #[test]
    fn append_only_guard_rejects_transient_mcp_pending_notice_cleanup_with_unaudited_commit()
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

        assert!(matches!(
            append_only_save_guard(&incoming, Some(&previous)),
            Err(SessionStoreError::InvalidTranscriptRewrite { .. })
        ));
        Ok::<(), crate::TranscriptEditError>(())
    }

    #[test]
    fn rewrite_chain_finder_crosses_normal_append_between_rewrites()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut session = Session::new();
        session.push(Message::User(UserMessage::text("first".to_string())));
        session.push(Message::BlockAssistant(BlockAssistantMessage {
            blocks: vec![AssistantBlock::Text {
                text: "verbose first answer".to_string(),
                meta: None,
            }],
            stop_reason: StopReason::EndTurn,
            identity: crate::types::TranscriptMessageIdentity::default(),
            created_at: crate::types::message_timestamp_now(),
        }));

        let original = session.transcript_revision()?;
        let first = session.commit_transcript_rewrite(
            TranscriptRewriteSelection::MessageRange { start: 1, end: 2 },
            vec![Message::BlockAssistant(BlockAssistantMessage {
                blocks: vec![AssistantBlock::Text {
                    text: "compact first answer".to_string(),
                    meta: None,
                }],
                stop_reason: StopReason::EndTurn,
                identity: crate::types::TranscriptMessageIdentity::default(),
                created_at: crate::types::message_timestamp_now(),
            })],
            crate::TranscriptRewriteReason::new("compaction"),
            Some("unit-test".to_string()),
            Some(original.clone()),
        )?;

        session.push(Message::User(UserMessage::text("second".to_string())));
        session.push(Message::BlockAssistant(BlockAssistantMessage {
            blocks: vec![AssistantBlock::Text {
                text: "verbose second answer".to_string(),
                meta: None,
            }],
            stop_reason: StopReason::EndTurn,
            identity: crate::types::TranscriptMessageIdentity::default(),
            created_at: crate::types::message_timestamp_now(),
        }));
        let bridge = session.transcript_revision()?;
        assert_ne!(bridge, first.revision);

        let second = session.commit_transcript_rewrite(
            TranscriptRewriteSelection::MessageRange { start: 3, end: 4 },
            vec![Message::BlockAssistant(BlockAssistantMessage {
                blocks: vec![AssistantBlock::Text {
                    text: "compact second answer".to_string(),
                    meta: None,
                }],
                stop_reason: StopReason::EndTurn,
                identity: crate::types::TranscriptMessageIdentity::default(),
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

    #[test]
    fn run_boundary_guard_rejects_dropped_retained_rewrite_commits()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut base = Session::new();
        base.push(Message::User(UserMessage::text("turn one".to_string())));
        base.push(Message::BlockAssistant(BlockAssistantMessage {
            blocks: vec![AssistantBlock::Text {
                text: "verbose answer".to_string(),
                meta: None,
            }],
            stop_reason: StopReason::EndTurn,
            identity: crate::types::TranscriptMessageIdentity::default(),
            created_at: crate::types::message_timestamp_now(),
        }));
        let base_revision = base.transcript_revision()?;

        let mut previous = base.clone();
        let _retained_commit = previous.commit_transcript_rewrite(
            TranscriptRewriteSelection::MessageRange { start: 1, end: 2 },
            vec![Message::BlockAssistant(BlockAssistantMessage {
                blocks: vec![AssistantBlock::Text {
                    text: "first compact answer".to_string(),
                    meta: None,
                }],
                stop_reason: StopReason::EndTurn,
                identity: crate::types::TranscriptMessageIdentity::default(),
                created_at: crate::types::message_timestamp_now(),
            })],
            crate::TranscriptRewriteReason::new("compaction"),
            Some("unit-test".to_string()),
            Some(base_revision),
        )?;
        let previous_revision = previous.transcript_revision()?;

        let mut incoming = previous.clone();
        let new_commit = incoming.commit_transcript_rewrite(
            TranscriptRewriteSelection::MessageRange { start: 1, end: 2 },
            vec![Message::BlockAssistant(BlockAssistantMessage {
                blocks: vec![AssistantBlock::Text {
                    text: "second compact answer".to_string(),
                    meta: None,
                }],
                stop_reason: StopReason::EndTurn,
                identity: crate::types::TranscriptMessageIdentity::default(),
                created_at: crate::types::message_timestamp_now(),
            })],
            crate::TranscriptRewriteReason::new("compaction"),
            Some("unit-test".to_string()),
            Some(previous_revision),
        )?;
        let mut state = incoming
            .transcript_history_state()?
            .ok_or_else(|| std::io::Error::other("incoming rewrite should retain history"))?;
        state.commits = vec![new_commit];
        incoming.set_metadata_unchecked_for_test(
            crate::session::SESSION_TRANSCRIPT_HISTORY_STATE_KEY,
            serde_json::to_value(state)?,
        );

        assert!(matches!(
            run_boundary_snapshot_save_guard(&incoming, Some(&previous)),
            Err(SessionStoreError::InvalidTranscriptRewrite { reason, .. })
                if reason.contains("drop retained transcript rewrite commits")
        ));
        Ok(())
    }

    // ------------------------------------------------------------------
    // FOLD 2: the persist-time system-context append-admission decision routes
    // through SessionDocumentMachine ResolveSystemContextPersistAppendAdmission
    // (the SAME machine the staging path drives). These tests pin that the
    // persist-time verdict matches a direct machine call for every shape, and
    // that the four admission cases behave exactly as the retired shell reducer.
    // ------------------------------------------------------------------

    fn runtime_append_system(content: &str) -> SystemMessage {
        let mut system = SystemMessage::new(content);
        system.mutation_kind = crate::types::SystemPromptMutationKind::RuntimeContextAppend;
        system
    }

    /// Direct machine call mirroring the persist-time observation extraction —
    /// the persist-time path MUST agree with this for every input shape.
    #[allow(clippy::expect_used)]
    fn machine_persist_append_admits(
        previous: Option<&SystemMessage>,
        incoming: &SystemMessage,
    ) -> bool {
        let has_previous = previous.is_some();
        let content_identical =
            previous.is_some_and(|previous| incoming.content == previous.content);
        let content_extends_previous =
            previous.is_some_and(|previous| incoming.content.starts_with(&previous.content));
        let appended_starts_with_separator = previous.is_some_and(|previous| {
            incoming
                .content
                .get(previous.content.len()..)
                .is_some_and(|appended| appended.starts_with(SYSTEM_CONTEXT_SEPARATOR))
        });
        let incoming_is_runtime_context_append = incoming.mutation_kind.is_runtime_context_append();
        let mut authority = crate::session_document::SessionDocumentMachineAuthority::new();
        let effects = authority
            .resolve_system_context_persist_append_admission(
                has_previous,
                content_identical,
                content_extends_previous,
                appended_starts_with_separator,
                incoming_is_runtime_context_append,
            )
            .expect("machine resolves persist-append admission");
        effects.into_iter().any(|effect| {
            matches!(
                effect,
                crate::session_document::SessionDocumentEffect::SystemContextPersistAppendAdmissionResolved {
                    admission: crate::session_document::SystemContextPersistAppendAdmission::Admit,
                }
            )
        })
    }

    #[allow(clippy::expect_used)]
    fn assert_persist_append_matches_machine(
        previous: Option<&SystemMessage>,
        incoming: &SystemMessage,
        expected: bool,
    ) {
        let verdict =
            system_context_is_append(previous, incoming).expect("persist-time admission resolves");
        assert_eq!(verdict, expected, "persist-time verdict mismatch");
        assert_eq!(
            verdict,
            machine_persist_append_admits(previous, incoming),
            "persist-time verdict diverges from direct machine call"
        );
    }

    #[test]
    fn persist_append_identical_content_admits() {
        let previous = SystemMessage::new("base system");
        let incoming = SystemMessage::new("base system");
        assert_persist_append_matches_machine(Some(&previous), &incoming, true);
    }

    #[test]
    fn persist_append_separator_append_with_marker_admits() {
        let previous = SystemMessage::new("base system");
        let incoming = runtime_append_system(&format!(
            "base system{SYSTEM_CONTEXT_SEPARATOR}[Runtime System Context]\nextra"
        ));
        assert_persist_append_matches_machine(Some(&previous), &incoming, true);
    }

    #[test]
    fn persist_append_shaped_without_marker_rejects() {
        let previous = SystemMessage::new("base system");
        // Append-shaped content but no runtime-context-append provenance marker.
        let incoming = SystemMessage::new(format!(
            "base system{SYSTEM_CONTEXT_SEPARATOR}[Runtime System Context]\nextra"
        ));
        assert_persist_append_matches_machine(Some(&previous), &incoming, false);
    }

    #[test]
    fn persist_append_divergent_content_rejects() {
        let previous = SystemMessage::new("base system");
        let incoming = runtime_append_system("totally different");
        assert_persist_append_matches_machine(Some(&previous), &incoming, false);
    }

    #[test]
    fn persist_append_no_previous_admits_only_with_marker() {
        let with_marker = runtime_append_system("brand new context");
        assert_persist_append_matches_machine(None, &with_marker, true);

        let without_marker = SystemMessage::new("brand new context");
        assert_persist_append_matches_machine(None, &without_marker, false);
    }

    fn assistant_with_bookkeeping(
        text: &str,
        run_id: Option<crate::lifecycle::RunId>,
        created_at: crate::types::MessageTimestamp,
    ) -> Message {
        Message::BlockAssistant(BlockAssistantMessage {
            blocks: vec![AssistantBlock::Text {
                text: text.to_string(),
                meta: None,
            }],
            stop_reason: StopReason::EndTurn,
            identity: crate::types::TranscriptMessageIdentity {
                interaction_id: None,
                run_id,
                objective_id: None,
            },
            created_at,
        })
    }

    /// Cold-restart resume regression (Ask B): a re-created runtime authority
    /// re-stamps run identity and timestamps on the transcript copy it
    /// re-projects. The transcript revision is a content address, so a
    /// bookkeeping-only difference on the shared prefix must not fail
    /// continuity.
    #[test]
    fn append_only_guard_accepts_rebookkept_prefix_identity_and_timestamps() {
        let base_time = crate::types::message_timestamp_now();
        let mut previous = Session::new();
        previous.push(Message::System(SystemMessage::new("base system")));
        previous.push(Message::User(UserMessage::text("turn one".to_string())));
        previous.push(assistant_with_bookkeeping(
            "answer one",
            Some(crate::lifecycle::RunId::new()),
            base_time,
        ));

        let mut incoming = previous.clone();
        let mut rebookkept = previous.messages().to_vec();
        for message in &mut rebookkept {
            match message {
                Message::User(user) => {
                    user.created_at = base_time + chrono::Duration::hours(1);
                }
                Message::BlockAssistant(assistant) => {
                    assistant.identity = crate::types::TranscriptMessageIdentity {
                        interaction_id: None,
                        run_id: Some(crate::lifecycle::RunId::new()),
                        objective_id: None,
                    };
                    assistant.created_at = base_time + chrono::Duration::hours(1);
                }
                _ => {}
            }
        }
        rebookkept.push(Message::User(UserMessage::text("turn two".to_string())));
        incoming.messages = std::sync::Arc::new(rebookkept);

        assert!(
            append_only_save_guard(&incoming, Some(&previous)).is_ok(),
            "bookkeeping-only prefix divergence must not fail continuity"
        );
    }

    #[test]
    fn append_only_guard_rejects_content_divergence_despite_matching_bookkeeping() {
        let base_time = crate::types::message_timestamp_now();
        let run_id = crate::lifecycle::RunId::new();
        let mut previous = Session::new();
        previous.push(Message::User(UserMessage::text("turn one".to_string())));
        previous.push(assistant_with_bookkeeping(
            "answer one",
            Some(run_id),
            base_time,
        ));

        let mut incoming = previous.clone();
        let mut diverged = previous.messages().to_vec();
        if let Message::BlockAssistant(assistant) = &mut diverged[1] {
            assistant.blocks = vec![AssistantBlock::Text {
                text: "a different answer".to_string(),
                meta: None,
            }];
        }
        diverged.push(Message::User(UserMessage::text("turn two".to_string())));
        incoming.messages = std::sync::Arc::new(diverged);

        assert!(matches!(
            append_only_save_guard(&incoming, Some(&previous)),
            Err(SessionStoreError::TranscriptContinuityViolation { .. })
        ));
    }

    // -----------------------------------------------------------------------
    // Incremental session persistence (OB3 ask 11)
    // -----------------------------------------------------------------------

    #[allow(clippy::expect_used)]
    fn compacted_session_fixture() -> (Session, Session, TranscriptRewriteCommit) {
        let mut parent = Session::new();
        parent.push(Message::User(UserMessage::text("turn one".to_string())));
        parent.push(Message::User(UserMessage::text("turn two".to_string())));
        parent.push(Message::User(UserMessage::text("turn three".to_string())));
        let mut compacted = parent.clone();
        let commit = compacted
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange { start: 0, end: 3 },
                vec![Message::User(UserMessage::text(
                    "[Context compacted] summary".to_string(),
                ))],
                crate::TranscriptRewriteReason::new("compaction"),
                Some("unit-test".to_string()),
                None,
            )
            .expect("compaction rewrite should commit");
        (parent, compacted, commit)
    }

    #[test]
    #[allow(clippy::expect_used)]
    fn session_head_from_session_strips_history_state_and_round_trips() {
        let (_, compacted, _) = compacted_session_fixture();
        assert!(
            compacted
                .metadata()
                .contains_key(SESSION_TRANSCRIPT_HISTORY_STATE_KEY),
            "fixture must carry inline history state"
        );
        let head = SessionHead::from_session(&compacted, TranscriptStrandId::root(), 1)
            .expect("head projection");
        assert!(
            !head
                .metadata
                .contains_key(SESSION_TRANSCRIPT_HISTORY_STATE_KEY),
            "SessionHead::from_session must strip the inline history state"
        );
        assert_eq!(head.message_count, compacted.messages().len() as u64);
        assert_eq!(head.rewrite_count, 1);
        assert_eq!(
            head.head_revision,
            transcript_messages_digest(compacted.messages()).expect("digest")
        );

        // CAS token is stable across a serialize round-trip.
        let token = session_head_cas_token(&head).expect("token");
        let round_tripped: SessionHead =
            serde_json::from_slice(&serde_json::to_vec(&head).expect("serialize head"))
                .expect("deserialize head");
        assert_eq!(
            session_head_cas_token(&round_tripped).expect("token"),
            token,
            "session head CAS token must be stable across serde round-trips"
        );

        // Slim rebuild carries no history metadata and preserves the transcript.
        let slim = head
            .into_session(compacted.messages().to_vec())
            .expect("into_session");
        assert!(
            slim.transcript_history_state()
                .expect("state read")
                .is_none(),
            "slim session must not carry transcript history metadata"
        );
        assert_eq!(slim.messages().len(), compacted.messages().len());
        assert_eq!(slim.id(), compacted.id());
    }

    #[test]
    #[allow(clippy::expect_used)]
    fn session_head_into_session_fails_corrupted_on_tamper() {
        let mut session = Session::new();
        session.push(Message::User(UserMessage::text("hello".to_string())));
        let head = SessionHead::from_session(&session, TranscriptStrandId::root(), 0)
            .expect("head projection");

        // Tampered content.
        let tampered = vec![Message::User(UserMessage::text("tampered".to_string()))];
        assert!(matches!(
            head.clone().into_session(tampered),
            Err(SessionStoreError::Corrupted(_))
        ));

        // Wrong count.
        assert!(matches!(
            head.into_session(Vec::new()),
            Err(SessionStoreError::Corrupted(_))
        ));
    }

    #[test]
    #[allow(clippy::expect_used)]
    fn head_canonical_plain_save_guard_shapes() {
        let mut previous = Session::new();
        previous.push(Message::System(SystemMessage::new("base system")));
        previous.push(Message::User(UserMessage::text("hello".to_string())));

        // Plain append is admitted.
        let mut appended = previous.clone();
        appended.push(Message::User(UserMessage::text("more".to_string())));
        head_canonical_plain_save_guard(&appended, &previous, &[]).expect("plain append admitted");

        // Metadata-only update is admitted.
        let mut metadata_only = previous.clone();
        metadata_only.set_metadata("note", serde_json::json!("x"));
        head_canonical_plain_save_guard(&metadata_only, &previous, &[])
            .expect("metadata-only update admitted");

        // Shrink without a commit is a MonotonicityViolation.
        let mut shrunk = Session::with_id(previous.id().clone());
        shrunk.push(Message::System(SystemMessage::new("base system")));
        assert!(matches!(
            head_canonical_plain_save_guard(&shrunk, &previous, &[]),
            Err(SessionStoreError::MonotonicityViolation { .. })
        ));

        // System-context-append equivalence is admitted (machine-driven).
        let mut context_appended = previous.clone();
        context_appended
            .set_system_prompt_with_source(
                format!(
                    "base system{SYSTEM_CONTEXT_SEPARATOR}[Runtime System Context]\nsource: unit-test\n\nextra context"
                ),
                crate::session_durable_config_authority::SessionSystemPromptSource::RuntimeContextAppend,
            )
            .expect("runtime context append");
        head_canonical_plain_save_guard(&context_appended, &previous, &[])
            .expect("system-context append equivalence admitted");
    }

    /// ABSENT incoming history state is admitted by the head-canonical guard
    /// (out-of-line history cannot be erased by a row write) — the deliberate
    /// delta vs `append_only_save_guard`, which rejects the same shape as an
    /// erase. The contrast assertion pins the RED behavior permanently.
    #[test]
    #[allow(clippy::expect_used)]
    fn head_canonical_plain_save_guard_admits_absent_incoming_state() {
        let (_, compacted, commit) = compacted_session_fixture();

        // The slim materialization of the stored head (what an incremental
        // backend reconstructs) plus one plain append with NO inline state.
        let mut slim = Session::with_id(compacted.id().clone());
        for message in compacted.messages() {
            slim.push(message.clone());
        }
        assert!(
            slim.transcript_history_state()
                .expect("state read")
                .is_none()
        );
        let mut incoming = slim.clone();
        incoming.push(Message::User(UserMessage::text("next turn".to_string())));

        head_canonical_plain_save_guard(&incoming, &slim, std::slice::from_ref(&commit))
            .expect("absent incoming state must be admitted for head-canonical rows");

        // RED contrast: append_only_save_guard rejects the equivalent shape
        // when the previous row carried inline history state (the erase
        // check at the whole-blob boundary).
        let err = append_only_save_guard(&incoming, Some(&compacted))
            .expect_err("append_only_save_guard must reject the erase shape");
        assert!(
            matches!(err, SessionStoreError::InvalidTranscriptRewrite { ref reason, .. }
                if reason.contains("erase")),
            "unexpected error: {err}"
        );
    }

    #[test]
    #[allow(clippy::expect_used)]
    fn head_canonical_plain_save_guard_rejects_extra_commit_and_admits_fat_round_trip() {
        let (_, compacted, commit) = compacted_session_fixture();
        let mut slim = Session::with_id(compacted.id().clone());
        for message in compacted.messages() {
            slim.push(message.clone());
        }

        // A fat round-trip (incoming carries exactly the adopted commits) is
        // admitted.
        head_canonical_plain_save_guard(&compacted, &slim, std::slice::from_ref(&commit))
            .expect("commits-preserved fat round-trip admitted");

        // Incoming state with an EXTRA (unadopted) commit must be routed via
        // save_transcript_rewrite.
        let mut extra = compacted;
        extra
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange { start: 0, end: 1 },
                vec![Message::User(UserMessage::text(
                    "[Context compacted] second summary".to_string(),
                ))],
                crate::TranscriptRewriteReason::new("compaction"),
                Some("unit-test".to_string()),
                None,
            )
            .expect("second rewrite");
        let err = head_canonical_plain_save_guard(&extra, &slim, std::slice::from_ref(&commit))
            .expect_err("extra commit must be rejected on the plain path");
        assert!(
            matches!(err, SessionStoreError::InvalidTranscriptRewrite { ref reason, .. }
                if reason.contains("save_transcript_rewrite")),
            "unexpected error: {err}"
        );
    }

    #[test]
    #[allow(clippy::expect_used)]
    fn strand_layout_covers_compaction_chain_and_live_tail() {
        let (_, mut compacted, commit) = compacted_session_fixture();
        compacted.push(Message::User(UserMessage::text(
            "post-compaction turn".to_string(),
        )));
        let state = compacted
            .transcript_history_state()
            .expect("state read")
            .expect("state present");
        let layout = strand_layout_for_history(compacted.id(), Some(&state), compacted.messages())
            .expect("layout");
        assert_eq!(layout.rewrites.len(), 1);
        assert_eq!(
            layout.head_strand,
            TranscriptStrandId::from_rewrite(&commit)
        );
        assert_eq!(layout.head_len, compacted.messages().len() as u64);
        // Root strand holds the parent body; the rewrite strand holds the
        // revision body extended by the live tail.
        let root_rows = &layout
            .strands
            .iter()
            .find(|(sid, _)| *sid == TranscriptStrandId::root())
            .expect("root strand")
            .1;
        assert_eq!(root_rows.len() as u64, layout.rewrites[0].parent_len);
        let head_rows = &layout
            .strands
            .iter()
            .find(|(sid, _)| *sid == layout.head_strand)
            .expect("head strand")
            .1;
        assert_eq!(head_rows.len(), compacted.messages().len());
    }
}
