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
    SESSION_TRANSCRIPT_HISTORY_CHECKPOINT_DIGEST_KEY, SESSION_TRANSCRIPT_HISTORY_STATE_KEY,
    SYSTEM_CONTEXT_SEPARATOR, SessionMeta, TranscriptRevisionBody,
};
use crate::time_compat::SystemTime;
use crate::types::{Message, SessionId, SystemMessage, Usage};
use crate::{
    Session, TranscriptHistoryState, TranscriptRewriteCommit, TranscriptRewriteRecord,
    ValidatedTranscriptHistory, transcript_messages_digest,
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

// A process-global Boolean memo of slim-materialization verifications —
// (session id, head revision, message count) triples marked "already
// verified" — used to live here and let `SessionHead::into_session` skip its
// digest check on a tuple hit. It was DELETED as a verification bypass: the
// tuple never bound the ROW BYTES, so "load valid rows once (memo warms) ->
// corrupt a strand row while the head row stays intact (key unchanged) ->
// reload" served the corrupted transcript unverified. Do not reintroduce a
// skip-verification memo keyed on a non-binding tuple; the only sound fast
// path is the byte-exact substitution memo below.

/// Process-global SUBSTITUTION memo for slim materialization: the exact
/// message vector (shared `Arc`, O(1) to record) plus its proven digest
/// midstates, keyed by `(session id, head revision, message count)`.
///
/// Two producers hold the required proof: the head writer proves
/// `digest(messages) == head_revision` when it mints the head row
/// (`from_session`), and a slim reader proves the same equality when its
/// first-sight verification passes (`into_session`). Recording that proven
/// vector lets `into_session` SERVE it on the next materialization of the
/// same `(id, revision, count)` instead of re-hashing the row-assembled
/// vector — substitution, never blessing:
/// a hit discards whatever the rows materialized to and serves content the
/// producer proved, so corrupt rows are displaced, not trusted (the same
/// semantics as the transcript-graph decode memo). Debug builds compare the
/// substituted vector against the materialized rows on every hit. One entry
/// per recently-active session id: retention is the `Arc` the live session
/// already holds, plus at most `SLIM_SNAPSHOT_MEMO_BOUND` documents after
/// their sessions drop. Honors `MEERKAT_DISABLE_GRAPH_DECODE_MEMO`.
const SLIM_SNAPSHOT_MEMO_BOUND: usize = 4;

type SlimSnapshotEntry = (
    String,
    String,
    u64,
    crate::session::SharedTranscriptSnapshot,
);

fn slim_snapshot_memo() -> &'static std::sync::Mutex<std::collections::VecDeque<SlimSnapshotEntry>>
{
    static MEMO: std::sync::OnceLock<
        std::sync::Mutex<std::collections::VecDeque<SlimSnapshotEntry>>,
    > = std::sync::OnceLock::new();
    MEMO.get_or_init(|| std::sync::Mutex::new(std::collections::VecDeque::new()))
}

fn record_slim_materialization_snapshot(
    id: &SessionId,
    revision: &str,
    count: u64,
    snapshot: crate::session::SharedTranscriptSnapshot,
) {
    if std::env::var_os("MEERKAT_DISABLE_GRAPH_DECODE_MEMO").is_some() {
        return;
    }
    let id = id.to_string();
    let mut memo = slim_snapshot_memo()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    memo.retain(|(existing, _, _, _)| existing != &id);
    if memo.len() >= SLIM_SNAPSHOT_MEMO_BOUND {
        memo.pop_front();
    }
    memo.push_back((id, revision.to_string(), count, snapshot));
}

fn slim_materialization_snapshot(
    id: &SessionId,
    revision: &str,
    count: u64,
) -> Option<crate::session::SharedTranscriptSnapshot> {
    if std::env::var_os("MEERKAT_DISABLE_GRAPH_DECODE_MEMO").is_some() {
        return None;
    }
    let id = id.to_string();
    let memo = slim_snapshot_memo()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    memo.iter()
        .find(|(mid, mrev, mcount, snapshot)| {
            mid == &id
                && mrev == revision
                && *mcount == count
                && snapshot.message_count() as u64 == count
        })
        .map(|(_, _, _, snapshot)| snapshot.clone())
}

/// Transcript digests a caller has already proved, handed to a save guard so
/// it does not recompute them.
///
/// Trust model: a witness carries exactly the same authority as
/// [`SessionHead::head_revision`] — caller-attested, audited at the next
/// `commit_rewrite`, and verified fail-closed on every
/// [`SessionHead::into_session`]. Supply one only for a digest you hold
/// durable evidence for (a persisted `head_revision` for the row you loaded,
/// or a digest this process just computed over that exact message vector).
/// Every field is optional and absent fields are computed exactly as before,
/// so `SaveGuardWitness::none()` reproduces the unwitnessed guard verdict.
#[derive(Debug, Clone, Copy, Default)]
pub struct SaveGuardWitness<'a> {
    previous_revision: Option<&'a str>,
    incoming_revision: Option<&'a str>,
}

impl<'a> SaveGuardWitness<'a> {
    /// No caller-proved digests: the guard computes everything itself.
    #[must_use]
    pub fn none() -> Self {
        Self::default()
    }

    /// Record the proved transcript digest of the previously persisted row.
    #[must_use]
    pub fn with_previous_revision(mut self, revision: &'a str) -> Self {
        self.previous_revision = Some(revision);
        self
    }

    /// Record the proved transcript digest of the incoming document.
    #[must_use]
    pub fn with_incoming_revision(mut self, revision: &'a str) -> Self {
        self.incoming_revision = Some(revision);
        self
    }
}

/// Resolve a transcript digest witness-first, then from the session's own
/// incremental accumulator, then by full recompute.
///
/// All three produce the identical format-2 string; only the cost differs.
fn resolve_transcript_revision(
    session: &Session,
    witness: Option<&str>,
) -> Result<String, SessionStoreError> {
    if let Some(revision) = witness {
        return Ok(revision.to_string());
    }
    session
        .transcript_content_digest()
        .map_err(SessionStoreError::from)
}

/// Whether an incoming document without an inline transcript graph is a slim
/// projection CARRYING the previous graph out of line, rather than one erasing
/// it.
///
/// A head-canonical (slim) materialization keeps its revision bodies in
/// separate rows and carries the storage-invariant witness under a reserved
/// metadata key instead of an inline graph — that equivalence is the
/// documented contract of `session_transcript_history_checkpoint_digest`.
/// Without this carve-out every load-from-head -> save round trip reads as
/// "erases the retained history", which is exactly how a head-canonical
/// session wedges itself permanently: the durable head becomes unsaveable and
/// the identity cannot resume.
///
/// Fail-closed: only an EXACT witness match counts. A missing witness, an
/// unreadable one, or one naming a different graph is treated as genuine
/// erasure, so real graph loss still fails. Malformed evidence PROPAGATES as
/// an error and is never reduced to `false`.
///
/// The previous side is taken as the graph VALUE, not as a session, on
/// purpose: the reference witness must be DERIVED from a canonical graph that
/// is actually present. Two matching but unverified carried strings must never
/// establish history equivalence between themselves — that would let a pair of
/// slim projections agree the graph exists while neither holds it.
fn incoming_carries_previous_history_witness(
    incoming: &Session,
    previous_state: &crate::TranscriptHistoryState,
) -> Result<bool, SessionStoreError> {
    let carried =
        crate::checkpoint::session_transcript_history_witness(incoming).map_err(|err| {
            SessionStoreError::InvalidTranscriptRewrite {
                id: incoming.id().clone(),
                reason: format!("incoming transcript history witness is malformed: {err}"),
            }
        })?;
    let Some(carried) = carried else {
        return Ok(false);
    };
    // Derive the reference witness under the format the CARRIER declares:
    // a v3 (revision-identity) carrier over the same graph must match the
    // v3 derivation, not the v2 whole-graph hash it was never computed as.
    // Unknown formats already refused typed at document ingress; deriving
    // refuses them again rather than reducing them to a mismatch.
    let derived = crate::checkpoint::transcript_history_checkpoint_digest_in_format(
        previous_state,
        carried.witness_format(),
    )
    .map_err(|err| SessionStoreError::InvalidTranscriptRewrite {
        id: incoming.id().clone(),
        reason: format!("previous transcript history witness is malformed: {err}"),
    })?;
    Ok(derived == *carried.digest())
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
    append_only_save_guard_with_witness(incoming, previous, SaveGuardWitness::none())
}

/// [`append_only_save_guard`] with caller-proved transcript digests.
///
/// Same accept/reject boundary, same errors, same messages — the witness only
/// removes recomputation. Store mirrors that already hold the persisted
/// `head_revision` for `previous` use this to keep a plain save off the
/// O(document) path entirely.
pub fn append_only_save_guard_with_witness(
    incoming: &Session,
    previous: Option<&Session>,
    witness: SaveGuardWitness<'_>,
) -> Result<(), SessionStoreError> {
    let _digest_site =
        crate::checkpoint::enter_digest_site(crate::checkpoint::DIGEST_SITE_APPEND_GUARD);
    incoming
        .validate_transcript_history_state()
        .map_err(|err| SessionStoreError::InvalidTranscriptRewrite {
            id: incoming.id().clone(),
            reason: format!("incoming transcript history state is malformed: {err}"),
        })?;
    let incoming_revision = resolve_transcript_revision(incoming, witness.incoming_revision)?;
    let incoming_state = incoming.transcript_history_state_shared().map_err(|err| {
        SessionStoreError::InvalidTranscriptRewrite {
            id: incoming.id().clone(),
            reason: format!("incoming transcript history state is malformed: {err}"),
        }
    })?;
    if let Some(state) = incoming_state.as_deref()
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
            incoming_state.as_deref(),
            &incoming_revision,
            None,
        )?;
        return Ok(());
    };
    let previous_state = previous.transcript_history_state_shared().map_err(|err| {
        SessionStoreError::InvalidTranscriptRewrite {
            id: incoming.id().clone(),
            reason: format!("previous transcript history state is malformed: {err}"),
        }
    })?;
    let incoming_has_history = incoming_state.is_some();
    if let Some(previous_graph) = previous_state.as_deref()
        && !incoming_has_history
        && !incoming_carries_previous_history_witness(incoming, previous_graph)?
    {
        return Err(SessionStoreError::InvalidTranscriptRewrite {
            id: incoming.id().clone(),
            reason: "incoming save would erase retained transcript history state".to_string(),
        });
    }
    let previous_revision = resolve_transcript_revision(previous, witness.previous_revision)?;
    if previous_revision == incoming_revision {
        validate_plain_save_transcript_history_preservation(
            incoming,
            Some(previous),
            previous_state.as_deref(),
            incoming_state.as_deref(),
            &incoming_revision,
            Some(&previous_revision),
        )?;
        return Ok(());
    }

    let prev_len = previous.messages().len();
    let new_len = incoming.messages().len();
    if new_len >= prev_len {
        let incoming_prefix_revision = incoming
            .transcript_prefix_digest(prev_len)
            .map_err(SessionStoreError::from)?;
        if incoming_prefix_revision == previous_revision {
            validate_plain_save_transcript_history_preservation(
                incoming,
                Some(previous),
                previous_state.as_deref(),
                incoming_state.as_deref(),
                &incoming_revision,
                Some(&previous_revision),
            )?;
            return Ok(());
        }
    }
    if incoming_preserves_conversation_tail_with_system_context_append(incoming, previous)? {
        validate_plain_save_transcript_history_preservation(
            incoming,
            Some(previous),
            previous_state.as_deref(),
            incoming_state.as_deref(),
            &incoming_revision,
            Some(&previous_revision),
        )?;
        return Ok(());
    }
    if incoming_preserves_prefix_after_synthetic_notice_refresh(incoming, previous)? {
        validate_plain_save_transcript_history_preservation(
            incoming,
            Some(previous),
            previous_state.as_deref(),
            incoming_state.as_deref(),
            &incoming_revision,
            Some(&previous_revision),
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

/// `incoming_revision` / `previous_revision` are the digests the calling guard
/// already resolved for these exact documents. They are pass-through
/// parameters, not new evidence: this validator used to recompute both, which
/// doubled every plain save's transcript hashing for no additional proof.
fn validate_plain_save_transcript_history_preservation(
    incoming: &Session,
    previous: Option<&Session>,
    previous_state: Option<&TranscriptHistoryState>,
    incoming_state: Option<&TranscriptHistoryState>,
    incoming_revision: &str,
    previous_revision: Option<&str>,
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
        // Same slim-projection carve-out as the plain-save guard: a
        // head-canonical materialization keeps its revision bodies in
        // separate rows and carries the storage-invariant witness instead of
        // an inline graph. An EXACT witness match proves the graph is carried,
        // not erased; anything else is genuine erasure and still fails.
        if incoming_carries_previous_history_witness(incoming, previous_state)? {
            return Ok(());
        }
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
    let previous_revision = match previous_revision {
        Some(previous_revision) => previous_revision.to_string(),
        None => previous
            .transcript_content_digest()
            .map_err(SessionStoreError::from)?,
    };
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
            || !audited_bodies_are_equivalent(&previous_body.messages, &incoming_body.messages)?
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

/// Whether two audited revision bodies carry the same transcript.
///
/// Structural equality is the fast path: `Message: PartialEq` short-circuits
/// on the first difference and costs no hashing, and byte/structural equality
/// implies digest equality, so an equal verdict is strictly stronger than the
/// digest compare it replaces.
///
/// The digest compare is kept as the FALLBACK, and it is not optional.
/// Canonicalization deliberately erases what `PartialEq` compares: transcript
/// message identity and `created_at` are reset to sentinels, and inline vs
/// blob image forms collapse to one blob identity
/// (`canonicalize_messages_for_digest`). Digest-equal but structurally
/// different audited bodies therefore exist in the wild — the first save after
/// blob-store enablement, a body re-derived through a heal path, a resume that
/// re-projects the same conversation under a new runtime authority. Rejecting
/// those would freeze the session's writes fail-closed, which is exactly the
/// class of bug this guard exists to prevent, not to cause.
fn audited_bodies_are_equivalent(
    previous_body: &[Message],
    incoming_body: &[Message],
) -> Result<bool, SessionStoreError> {
    if previous_body == incoming_body {
        return Ok(true);
    }
    let previous_digest =
        transcript_messages_digest(previous_body).map_err(SessionStoreError::from)?;
    let incoming_digest =
        transcript_messages_digest(incoming_body).map_err(SessionStoreError::from)?;
    Ok(previous_digest == incoming_digest)
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
    let _digest_site =
        crate::checkpoint::enter_digest_site(crate::checkpoint::DIGEST_SITE_BOUNDARY_GUARD);
    match append_only_save_guard(incoming, previous) {
        Ok(()) => Ok(()),
        Err(append_error) => {
            if run_boundary_commitless_history_projection_save_guard(incoming, previous)? {
                return Ok(());
            }
            let Some(previous) = previous else {
                // First runtime-boundary commit for a session this authority
                // has never snapshotted: adoption of a resumed/imported
                // session. A typed rewrite graph carried in is audited by
                // the sealed whole-graph proof — every retained body's
                // digest, every commit's edit-shape relations, AND chain
                // coherence (which the former per-commit loop never checked)
                // — plus the graph-head/live-digest agreement below. Plain
                // `SessionStore::save` keeps rejecting such seeds (the
                // trait-level append-only contract); adoption is a
                // runtime-authority decision, not an ordinary row write.
                let incoming_revision = incoming
                    .transcript_content_digest()
                    .map_err(SessionStoreError::from)?;
                if let Some(sealed) =
                    incoming
                        .validated_transcript_history_state()
                        .map_err(|err| SessionStoreError::InvalidTranscriptRewrite {
                            id: incoming.id().clone(),
                            reason: format!(
                                "incoming transcript history state is malformed: {err}"
                            ),
                        })?
                    && !sealed.commits.is_empty()
                    && sealed.head == incoming_revision
                {
                    return Ok(());
                }
                return Err(append_error);
            };
            let incoming_revision = incoming
                .transcript_content_digest()
                .map_err(SessionStoreError::from)?;
            // append_only_save_guard's digest validation of the incoming
            // history state was discarded with its error above; a
            // digest-inconsistent witness body must not be able to prove a
            // fork as a plain append on this branch either. Sealing the parse
            // keeps that obligation and carries it to the consumers below,
            // which previously each re-established it from scratch.
            let Some(sealed) = incoming
                .validated_transcript_history_state()
                .map_err(|err| SessionStoreError::InvalidTranscriptRewrite {
                    id: incoming.id().clone(),
                    reason: format!("incoming transcript history state is malformed: {err}"),
                })?
            else {
                return Err(append_error);
            };
            let state = sealed.state();
            validate_rewrite_save_retains_previous_commits(incoming, previous, state)?;
            let commits = find_transcript_rewrite_commit_chain_extending_session(
                &sealed,
                previous,
                &incoming_revision,
            )?;
            if commits.is_none()
                && run_boundary_context_summary_tail_projection_save_guard(
                    incoming, previous, &sealed,
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
                // agreement explicitly before accepting. Every retained
                // commit's recorded bodies and edit-shape relations are
                // proven by the seal; re-deriving them per commit repeated
                // the whole-graph pass once per retained commit.
                if state.head != incoming_revision {
                    return Err(SessionStoreError::InvalidTranscriptRewrite {
                        id: incoming.id().clone(),
                        reason: format!(
                            "incoming transcript graph head {} does not match current message digest {incoming_revision}",
                            state.head
                        ),
                    });
                }
                return Ok(());
            };
            transcript_rewrite_bridge_save_guard(incoming, commit, &sealed, &incoming_revision)?;
            // Trailing rebookkept commits beyond the walked chain stay
            // digest-consistent by the same seal; no per-commit re-proof.
            Ok(())
        }
    }
}

/// [`run_boundary_snapshot_save_guard`] accepting caller-threaded graph
/// evidence for the one-time legacy upgrade boundary.
///
/// A pre-0.8.9 runtime snapshot row carries the transcript-history graph
/// INLINE; a 0.8.9 boundary snapshot is slim and carries only the witness.
/// When the graph evolved between the two (a resume reconciliation commit, a
/// turn's compaction), the slim incoming's witness can never equal the
/// witness derived from the previous inline graph, so the erase carve-out
/// refuses every save and the session wedges. The caller — who holds durable
/// access to the evolved graph — threads it here as `evidence`, and this
/// guard verifies (never trusts) it:
///
/// 1. the evidence graph seals ([`ValidatedTranscriptHistory::seal_owned`]:
///    every retained body digest-verified against its revision id, commit
///    edit shapes, chain coherence);
/// 2. the evidence is the exact graph the incoming document commits to: its
///    witness, derived under the format the incoming CARRIER declares,
///    equals the carried digest;
/// 3. the previous inline graph is retained: its commits are a prefix of the
///    evidence commits and every audited body is preserved
///    ([`validate_rewrite_save_retains_previous_commits`] with the evidence
///    standing in for the incoming's absent inline state);
/// 4. the previous row's live transcript reaches the evidence head through
///    digest-proved audited edges
///    ([`find_transcript_rewrite_commit_chain_extending_session`] +
///    [`transcript_rewrite_bridge_save_guard`] — the same validators the
///    guard already runs when an incoming document carries its graph
///    inline);
/// 5. the incoming live transcript continues the evidence head by plain
///    appends, proved by hashing the incoming prefix against the retained
///    head body.
///
/// This is exactly the acceptance boundary the guard has for an inline
/// incoming document, with the caller-threaded graph substituted for the
/// absent inline state and step 2 binding that substitution to the incoming
/// bytes. A fork — a graph whose prefix differs from the previous commits,
/// or a live transcript with no digest-proved path from the previous head —
/// fails the same validators it would fail inline.
///
/// Evidence is consulted ONLY when the unwitnessed guard refuses AND the
/// previous row carries an inline graph AND the incoming is slim with a
/// carried witness. Everything else — including every save against an
/// already-slim previous row — returns the unwitnessed verdict untouched, so
/// this path runs at most once per session: the accepted write replaces the
/// row with the slim representation and the precondition can never hold
/// again. Genuine erasure (no evidence, no witness, unprovable evolution)
/// keeps today's refusal message; malformed evidence propagates as a typed
/// error.
/// The previous-history preservation obligation of the legacy-evidence path.
///
/// The commit log is compared STRICTLY — commits are the digest-carrying
/// audit facts and round-trip exactly through every store — so the previous
/// inline graph's commits must be a prefix of the evidence commits. Audited
/// BODY preservation is proved at content level via
/// [`audited_bodies_are_equivalent`], not by the byte-identical
/// `created_at`/parent-pointer compare the inline path uses: the evidence is
/// reconstructed from durable rewrite records
/// ([`reconstruct_rewrite_record`] re-stamps bodies with `committed_at` and
/// drops parent bookkeeping), which is exactly the "re-projected same
/// conversation" shape that function documents as digest-equal but
/// structurally different. Content is what erasure would lose; content is
/// what this proves.
fn validate_legacy_evidence_retains_previous_history(
    incoming: &Session,
    previous: &Session,
    evidence: &ValidatedTranscriptHistory,
) -> Result<(), SessionStoreError> {
    let previous_state = previous.transcript_history_state_shared().map_err(|err| {
        SessionStoreError::InvalidTranscriptRewrite {
            id: incoming.id().clone(),
            reason: format!("previous transcript history state is malformed: {err}"),
        }
    })?;
    let Some(previous_state) = previous_state else {
        return Ok(());
    };
    let evidence_state = evidence.state();
    if evidence_state.commits.len() < previous_state.commits.len()
        || evidence_state.commits[..previous_state.commits.len()] != previous_state.commits
    {
        return Err(SessionStoreError::InvalidTranscriptRewrite {
            id: incoming.id().clone(),
            reason:
                "legacy upgrade history evidence would drop retained transcript rewrite commits"
                    .to_string(),
        });
    }
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
        let evidence_body = evidence_state
            .revisions
            .iter()
            .find(|body| body.revision == revision)
            .ok_or_else(|| SessionStoreError::InvalidTranscriptRewrite {
                id: incoming.id().clone(),
                reason: format!("legacy upgrade history evidence drops audited body {revision}"),
            })?;
        if !audited_bodies_are_equivalent(&previous_body.messages, &evidence_body.messages)? {
            return Err(SessionStoreError::InvalidTranscriptRewrite {
                id: incoming.id().clone(),
                reason: format!(
                    "legacy upgrade history evidence changes audited transcript body {revision}"
                ),
            });
        }
    }
    Ok(())
}

pub fn run_boundary_snapshot_save_guard_with_legacy_history_evidence(
    incoming: &Session,
    previous: Option<&Session>,
    evidence: Option<&TranscriptHistoryState>,
) -> Result<(), SessionStoreError> {
    let refusal = match run_boundary_snapshot_save_guard(incoming, previous) {
        Ok(()) => return Ok(()),
        Err(refusal) => refusal,
    };
    let (Some(evidence), Some(previous)) = (evidence, previous) else {
        return Err(refusal);
    };
    legacy_inline_history_evolution_guard(incoming, previous, evidence, refusal)
}

fn legacy_inline_history_evolution_guard(
    incoming: &Session,
    previous: &Session,
    evidence: &TranscriptHistoryState,
    refusal: SessionStoreError,
) -> Result<(), SessionStoreError> {
    let _digest_site =
        crate::checkpoint::enter_digest_site(crate::checkpoint::DIGEST_SITE_BOUNDARY_GUARD);
    // Reachability preconditions, all fail-closed to the unwitnessed verdict:
    // previous must still be the legacy inline representation and incoming
    // must be a slim projection carrying a witness to bind against.
    let previous_carries_inline_graph = previous
        .transcript_history_state_shared()
        .map_err(|err| SessionStoreError::InvalidTranscriptRewrite {
            id: incoming.id().clone(),
            reason: format!("previous transcript history state is malformed: {err}"),
        })?
        .is_some();
    if !previous_carries_inline_graph {
        return Err(refusal);
    }
    let incoming_carries_inline_graph = incoming
        .transcript_history_state_shared()
        .map_err(|err| SessionStoreError::InvalidTranscriptRewrite {
            id: incoming.id().clone(),
            reason: format!("incoming transcript history state is malformed: {err}"),
        })?
        .is_some();
    if incoming_carries_inline_graph {
        return Err(refusal);
    }
    let carried =
        crate::checkpoint::session_transcript_history_witness(incoming).map_err(|err| {
            SessionStoreError::InvalidTranscriptRewrite {
                id: incoming.id().clone(),
                reason: format!("incoming transcript history witness is malformed: {err}"),
            }
        })?;
    let Some(carried) = carried else {
        return Err(refusal);
    };

    // 1. Whole-graph proof of the threaded evidence.
    let sealed = ValidatedTranscriptHistory::seal_owned(evidence.clone()).map_err(|err| {
        SessionStoreError::InvalidTranscriptRewrite {
            id: incoming.id().clone(),
            reason: format!("legacy upgrade history evidence is malformed: {err}"),
        }
    })?;
    // 2. Bind the evidence to the incoming document's own carried witness,
    // under the format that carrier declares.
    let derived = crate::checkpoint::transcript_history_checkpoint_digest_in_format(
        sealed.state(),
        carried.witness_format(),
    )
    .map_err(|err| SessionStoreError::InvalidTranscriptRewrite {
        id: incoming.id().clone(),
        reason: format!("legacy upgrade history evidence witness is malformed: {err}"),
    })?;
    if derived != *carried.digest() {
        return Err(SessionStoreError::InvalidTranscriptRewrite {
            id: incoming.id().clone(),
            reason: format!(
                "legacy upgrade history evidence witness {derived} does not match the witness {} carried by the incoming save",
                carried.digest()
            ),
        });
    }
    // 3. The previous inline graph is retained by the evidence graph.
    validate_legacy_evidence_retains_previous_history(incoming, previous, &sealed)?;
    // 4. The previous live transcript reaches the evidence head through
    // digest-proved audited edges.
    let evidence_head = sealed.state().head.as_str();
    let Some(chain) =
        find_transcript_rewrite_commit_chain_extending_session(&sealed, previous, evidence_head)?
    else {
        return Err(refusal);
    };
    if let Some(commit) = chain.first() {
        transcript_rewrite_bridge_save_guard(incoming, commit, &sealed, evidence_head)?;
    }
    // 5. The incoming live transcript continues the evidence head by plain
    // appends: the retained head body's length names the prefix, the digest
    // over the incoming's own messages proves it.
    let incoming_revision = incoming
        .transcript_content_digest()
        .map_err(SessionStoreError::from)?;
    if incoming_revision == evidence_head {
        return Ok(());
    }
    let Some(head_body) = sealed
        .state()
        .revisions
        .iter()
        .find(|body| body.revision == evidence_head)
    else {
        return Err(refusal);
    };
    let head_len = head_body.messages.len();
    if incoming.messages().len() < head_len {
        return Err(refusal);
    }
    let incoming_prefix_revision = incoming
        .transcript_prefix_digest(head_len)
        .map_err(SessionStoreError::from)?;
    if incoming_prefix_revision != evidence_head {
        return Err(refusal);
    }
    Ok(())
}

/// Assemble the caller-threaded evolved-graph evidence for the one-time
/// legacy upgrade boundary from an incremental store's durable records.
///
/// This is the CALLER half of
/// [`run_boundary_snapshot_save_guard_with_legacy_history_evidence`]: rebuild
/// the evolved graph from the store's append-only rewrite records
/// ([`TranscriptHistoryState::from_rewrite_records`]) and, when the last
/// pre-upgrade head write pinned a mechanical live-head body, extend the
/// reconstruction to the durable head row so the graph names the same
/// retained revisions the incoming document's witness was minted over. The
/// extension body is loaded from the head's own strand rows and
/// digest-verified by the guard when it seals the evidence, so nothing
/// returned here is trusted — an imperfect reconstruction can only reproduce
/// the existing refusal, never admit an unproven write.
///
/// Returns `Ok(None)` for every shape the plain guard already decides
/// correctly: an incoming document carrying its graph inline, one carrying
/// no witness, or a store with no adopted rewrites.
pub async fn legacy_upgrade_history_evidence_from_incremental(
    incremental: &dyn IncrementalSessionStore,
    incoming: &Session,
) -> Result<Option<TranscriptHistoryState>, SessionStoreError> {
    if incoming
        .metadata()
        .contains_key(SESSION_TRANSCRIPT_HISTORY_STATE_KEY)
        || !incoming
            .metadata()
            .contains_key(SESSION_TRANSCRIPT_HISTORY_CHECKPOINT_DIGEST_KEY)
    {
        return Ok(None);
    }
    let records = incremental.load_rewrites(incoming.id()).await?;
    let Some(mut state) = TranscriptHistoryState::from_rewrite_records(records).map_err(|err| {
        SessionStoreError::InvalidTranscriptRewrite {
            id: incoming.id().clone(),
            reason: format!("failed to rebuild transcript history for the upgrade boundary: {err}"),
        }
    })?
    else {
        return Ok(None);
    };
    let head = incremental.load_head(incoming.id()).await?;
    if let Some(head) = head
        && head.head_revision != state.head
    {
        if state
            .revisions
            .iter()
            .any(|body| body.revision == head.head_revision)
        {
            state.head = head.head_revision;
        } else {
            let messages = incremental
                .load_messages(incoming.id(), &head.strand, 0..head.message_count)
                .await?;
            state.revisions.push(TranscriptRevisionBody {
                revision: head.head_revision.clone(),
                parent_revision: Some(state.head.clone()),
                messages,
                created_at: head.updated_at,
            });
            state.head = head.head_revision;
        }
    }
    Ok(Some(state))
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
    state: &ValidatedTranscriptHistory,
) -> Result<bool, SessionStoreError> {
    if state.commits.is_empty() {
        return Ok(false);
    }

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

    // Every retained commit's recorded bodies are proven by the sealed graph
    // this guard now demands; the former per-commit loop re-ran the
    // whole-graph pass once per retained commit.
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
///
/// The graph arrives already proved. This walk reads revision strings and
/// retained bodies as authority, so it needs the whole-graph validation to have
/// happened — but every caller stands immediately downstream of a session that
/// already established it, and re-deriving it here cost one full
/// canonicalize-and-hash pass over every retained body per call (the
/// storage-normalization wrapper calls this once per retained commit). Demanding
/// [`ValidatedTranscriptHistory`] keeps the requirement and drops the repetition.
pub fn find_transcript_rewrite_commit_chain_extending_session<'a>(
    state: &'a ValidatedTranscriptHistory,
    previous: &Session,
    incoming_revision: &str,
) -> Result<Option<Vec<&'a TranscriptRewriteCommit>>, SessionStoreError> {
    let _digest_site =
        crate::checkpoint::enter_digest_site(crate::checkpoint::DIGEST_SITE_REWRITE_CHAIN_WALK);
    let state = state.state();
    let previous_revision = previous
        .transcript_content_digest()
        .map_err(SessionStoreError::from)?;
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
    incoming_state: &ValidatedTranscriptHistory,
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
    let previous_message_digest = previous.transcript_content_digest().map_err(|err| {
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
    let incoming_message_digest = incoming.transcript_content_digest().map_err(|err| {
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
    let Some(incoming_state) = incoming
        .validated_transcript_history_state()
        .map_err(|err| SessionStoreError::InvalidTranscriptRewrite {
            id: incoming.id().clone(),
            reason: format!("incoming transcript history state is malformed: {err}"),
        })?
    else {
        return Err(SessionStoreError::InvalidTranscriptRewrite {
            id: incoming.id().clone(),
            reason: "incoming rewrite did not persist a transcript revision graph".to_string(),
        });
    };
    validate_rewrite_save_retains_previous_commits(incoming, previous, incoming_state.state())?;
    validate_transcript_rewrite_commit_bodies(incoming, commit, &incoming_state)
}

/// Require a rewrite commit to be a member of a PROVEN incoming graph.
///
/// This function used to re-derive, per commit, everything the whole-graph
/// validator already proves for every commit in the state: parent/revision
/// body presence, message-count agreement, both body digests, selection
/// bounds, span/prefix/suffix/replacement digests. Demanding
/// [`ValidatedTranscriptHistory`] keeps every one of those checks — the seal
/// cannot exist without them having passed — and reduces this call to the
/// one fact the seal does not state: that THIS commit is in THAT graph.
/// Callers holding an unproven parse must seal it first
/// (`Session::validated_transcript_history_state`), which is exactly one
/// whole-graph pass instead of one per consumer.
fn validate_transcript_rewrite_commit_bodies(
    incoming: &Session,
    commit: &TranscriptRewriteCommit,
    incoming_state: &ValidatedTranscriptHistory,
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
        let head_revision = session
            .transcript_content_digest()
            .map_err(SessionStoreError::from)?;
        // The digest above just proved `digest(messages) == head_revision`
        // for exactly this vector; admit the proven vector (shared Arc, O(1))
        // so the next slim materialization of this head substitutes it
        // instead of re-hashing the row-assembled copy.
        if let Some(snapshot) = session.shared_transcript_snapshot() {
            record_slim_materialization_snapshot(
                session.id(),
                &head_revision,
                session.messages().len() as u64,
                snapshot,
            );
        }
        let history_witness = crate::checkpoint::session_transcript_history_witness(session)
            .map_err(|error| {
                SessionStoreError::Serialization(format!(
                    "failed to derive transcript-history checkpoint witness: {error}"
                ))
            })?;
        // Build the slim metadata WITHOUT cloning the transcript-history
        // graph value: on a compacted session that value carries every
        // retained revision body plus the live head body, so `clone()` then
        // `remove()` was one full O(graph) tree copy per boundary save.
        let mut metadata = session
            .metadata()
            .iter()
            .filter(|(key, _)| {
                key.as_str() != SESSION_TRANSCRIPT_HISTORY_STATE_KEY
                    && key.as_str() != SESSION_TRANSCRIPT_HISTORY_CHECKPOINT_DIGEST_KEY
            })
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect::<serde_json::Map<String, serde_json::Value>>();
        if let Some(history_witness) = history_witness {
            // The typed carrier round-trips the witness FORMAT: v2 stays the
            // bare string every pre-v3 reader understands, v3 persists the
            // object form. A slim projection can never relabel the format —
            // it re-carries exactly what the document's evidence declares.
            metadata.insert(
                SESSION_TRANSCRIPT_HISTORY_CHECKPOINT_DIGEST_KEY.to_string(),
                history_witness.to_carried_value(),
            );
        }
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
        // Equality fast path: the head writer proved this exact
        // (id, revision, count) over the vector it recorded. When the
        // materialized rows are IDENTICAL to that proven vector (a plain
        // memory compare — no canonicalization, no hashing), serving the
        // proven vector with its warm digest midstates is exactly the
        // full verification's outcome at a fraction of the cost. ANY
        // difference — tampered rows, but also legitimate representation
        // deltas the digest erases (externalized media forms) — is NOT a
        // verdict: it falls through to the unchanged first-sight digest
        // verification below, which accepts digest-equal representations
        // and fails tampered rows closed as `Corrupted`.
        if let Some(snapshot) =
            slim_materialization_snapshot(&self.id, &self.head_revision, self.message_count)
            && *snapshot.messages().as_ref() == messages
        {
            let SessionHead {
                id,
                version,
                created_at,
                updated_at,
                usage,
                metadata,
                ..
            } = self;
            let transcript = crate::session::TranscriptMessages::from_shared_snapshot(&snapshot);
            return Session::from_head_parts_with_transcript(
                version, id, transcript, created_at, updated_at, metadata, usage,
            )
            .map_err(|err| {
                SessionStoreError::Serialization(format!(
                    "failed to restore session from head row: {err}"
                ))
            });
        }
        // The head revision IS the transcript content digest; verify it on
        // EVERY row-assembled materialization. A process-global Boolean memo
        // keyed on (session id, head revision, message count) used to skip
        // this hash after one valid load, but that tuple never bound the row
        // BYTES: load valid rows (memo warms), corrupt a strand row while the
        // head row stays intact (same key), reload — the substitution path
        // above misses on bytes, the tuple still hits, and the corrupted
        // transcript is served unverified. A verification bypass keyed on a
        // non-binding tuple is a corruption-blessing device; do not bring it
        // back as an optimization. The sound fast path is the substitution
        // memo above (byte-exact identity with a vector whose digest was
        // proven), which the verification below warms for the next load.
        let SessionHead {
            id,
            version,
            head_revision,
            created_at,
            updated_at,
            usage,
            metadata,
            ..
        } = self;
        let session = Session::from_head_parts(
            version,
            id.clone(),
            messages,
            created_at,
            updated_at,
            metadata,
            usage,
        )
        .map_err(|err| {
            SessionStoreError::Serialization(format!(
                "failed to restore session from head row: {err}"
            ))
        })?;
        // The verification pass is the session's own first digest, so it seeds
        // the incremental accumulator instead of being computed and thrown
        // away: the mandatory first-sight hash now also pays for every later
        // save guard on this in-memory session.
        let digest = session
            .transcript_content_digest()
            .map_err(SessionStoreError::from)?;
        if digest != head_revision {
            return Err(SessionStoreError::Corrupted(id));
        }
        // The digest above just proved `digest(messages) == head_revision`
        // for exactly this vector — the same evidence the head producer holds
        // in `from_session` — so admit it to the substitution memo: the next
        // materialization of identical rows serves it via the byte-exact
        // compare instead of re-hashing (different bytes still fall through
        // to this verification).
        if let Some(snapshot) = session.shared_transcript_snapshot() {
            record_slim_materialization_snapshot(
                session.id(),
                &head_revision,
                session.messages().len() as u64,
                snapshot,
            );
        }
        Ok(session)
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
/// creates. Compaction therefore persists O(live-after) instead of a superset
/// blob.
///
/// # Storage bound (the contract, not merely an implementation note)
///
/// Prefix addressing alone does NOT bound total storage: successive strands
/// are separate address spaces, so a rewrite that shares no *prefix* with its
/// parent — the common shape, e.g. replacing the leading system projection —
/// costs a full transcript of fresh rows, per rewrite, forever. Measured in
/// the field: 98 rewrites of one 371-message transcript accumulated 16,672
/// strand rows.
///
/// A conforming backend's persisted rows MUST stay bounded by
/// `live transcript + Σ retained deltas`, where a retained delta is the span
/// a superseded strand's successor genuinely dropped or replaced
/// ([`StrandSplice`] is the shared vocabulary for that span, derived by
/// comparison and never caller-attested). Concretely: `N` rewrites must not
/// cost `N × transcript`, and a backend must not retain rows no read verb of
/// this trait can reach. The observable contract is unchanged — every verb
/// below still serves exactly the same content — so a backend that keeps
/// whole strands materialized (an in-memory reference store, say) stays
/// conformant on semantics; only durable backends owe the bound.
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

    /// Head row ONLY when head+rows are the session's canonical durable
    /// representation.
    ///
    /// Unlike [`load_head`], which may synthesize a deterministic head for a
    /// legacy blob-only session (an O(document) blob parse), this must return
    /// `None` for absent AND blob-only sessions and must never read the blob.
    /// It is the capability probe for head-trusted range reads: `Some`
    /// promises the returned row is the persisted head row itself and that
    /// [`load_messages`] over `head.strand` serves exactly the rows the head
    /// covers, without materializing the whole document.
    ///
    /// The conservative default returns `None`: a store that does not
    /// override this simply never advertises the canonical head, keeping
    /// every reader on the whole-load path (fallback, never refusal).
    ///
    /// [`load_head`]: IncrementalSessionStore::load_head
    /// [`load_messages`]: IncrementalSessionStore::load_messages
    async fn load_canonical_head(
        &self,
        id: &SessionId,
    ) -> Result<Option<SessionHead>, SessionStoreError> {
        let _ = id;
        Ok(None)
    }

    /// Adopted rewrite COMMITS only (`idx < head.rewrite_count`), oldest
    /// first, without materializing retained revision bodies.
    ///
    /// Must serve exactly the commits of [`load_rewrites`], in the same
    /// order — including the empty set while a recorded rewrite is not yet
    /// adopted. The default derives from `load_rewrites` (always correct,
    /// but O(sum of retained bodies)); overriding stores read the small
    /// commit rows directly.
    ///
    /// [`load_rewrites`]: IncrementalSessionStore::load_rewrites
    async fn load_rewrite_commits(
        &self,
        id: &SessionId,
    ) -> Result<Vec<TranscriptRewriteCommit>, SessionStoreError> {
        Ok(self
            .load_rewrites(id)
            .await?
            .into_iter()
            .map(|record| record.commit)
            .collect())
    }
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
    head_canonical_plain_save_guard_with_witness(
        incoming,
        previous_slim,
        stored_commits,
        SaveGuardWitness::none(),
    )
}

/// [`head_canonical_plain_save_guard`] with caller-proved transcript digests.
///
/// The head-canonical caller normally holds the stored row's `head_revision`
/// already, which is exactly the `previous_slim` digest this guard would
/// otherwise recompute over every strand row it just materialized.
pub fn head_canonical_plain_save_guard_with_witness(
    incoming: &Session,
    previous_slim: &Session,
    stored_commits: &[TranscriptRewriteCommit],
    witness: SaveGuardWitness<'_>,
) -> Result<(), SessionStoreError> {
    incoming
        .validate_transcript_history_state()
        .map_err(|err| SessionStoreError::InvalidTranscriptRewrite {
            id: incoming.id().clone(),
            reason: format!("incoming transcript history state is malformed: {err}"),
        })?;
    let incoming_revision = resolve_transcript_revision(incoming, witness.incoming_revision)?;
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

    let previous_revision = resolve_transcript_revision(previous_slim, witness.previous_revision)?;
    if previous_revision == incoming_revision {
        return Ok(());
    }
    let prev_len = previous_slim.messages().len();
    let new_len = incoming.messages().len();
    if new_len >= prev_len {
        let incoming_prefix_revision = incoming
            .transcript_prefix_digest(prev_len)
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

/// Where one row of a superseded strand physically lives.
///
/// See [`StrandSplice`]: a superseded strand keeps only the rows its
/// successor cannot reproduce; every other row IS a row of the successor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StrandRowSource {
    /// Physically retained by the superseded strand itself, at this index.
    Retained(u64),
    /// Byte-identical to the successor strand's row at this index.
    Successor(u64),
}

/// One contiguous run of a superseded strand's rows, resolved to a source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StrandSegment {
    /// Rows physically retained by the superseded strand (its own indices).
    Retained(std::ops::Range<u64>),
    /// Rows served by the successor strand (successor indices).
    Successor(std::ops::Range<u64>),
}

/// The minimal splice that re-expresses a superseded strand as a delta of
/// the strand that replaced it.
///
/// # Why this exists
///
/// Every strand transition a session takes — adopting a transcript rewrite,
/// rebasing onto an equivalence-admitted projection — produces a new strand
/// whose rows are overwhelmingly the *same rows* as the strand it replaced.
/// Storing each strand as an independent row vector therefore costs
/// `O(transcript)` per transition and grows without bound: a rewrite that
/// edits one message of a 371-message transcript persists 371 fresh rows,
/// forever, for one changed message.
///
/// A splice bounds that: the superseded strand physically retains only
/// `[splice_start, splice_end)` — the rows the successor genuinely dropped
/// or replaced — and every other row resolves to the successor. Total
/// storage becomes `live transcript + Σ retained spans` instead of
/// `revisions × transcript`.
///
/// # Invariants
///
/// With `S` the superseded strand and `N` its successor:
/// - `S[0..splice_start) == N[0..splice_start)` (shared prefix);
/// - `S[splice_end..strand_len) == N[successor_end..successor_len())`
///   (shared suffix);
/// - `S[splice_start..splice_end)` is retained by `S` itself;
/// - `splice_start <= splice_end <= strand_len` and
///   `splice_start <= successor_end`.
///
/// The splice is derived by comparison ([`StrandSplice::between`]), never
/// attested by a caller: a backend can always recompute it from the two row
/// vectors it holds, and a wrong descriptor is structurally inexpressible.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct StrandSplice {
    /// Logical row count of the superseded strand.
    pub strand_len: u64,
    /// First index at which the two strands differ.
    pub splice_start: u64,
    /// End (exclusive, in superseded-strand indices) of the replaced span.
    pub splice_end: u64,
    /// End (exclusive, in successor indices) of the replacement span.
    pub successor_end: u64,
}

impl StrandSplice {
    /// Derive the minimal splice between a superseded strand's rows and its
    /// successor's rows by longest common prefix + longest common suffix.
    ///
    /// Comparison is on whatever row identity `T` provides; backends pass
    /// the exact persisted bytes so "shared" means byte-identical, never
    /// merely digest-equivalent.
    pub fn between<T: PartialEq>(strand_rows: &[T], successor_rows: &[T]) -> Self {
        let overlap = strand_rows.len().min(successor_rows.len());
        let mut prefix = 0usize;
        while prefix < overlap && strand_rows[prefix] == successor_rows[prefix] {
            prefix += 1;
        }
        let mut suffix = 0usize;
        while suffix < overlap - prefix
            && strand_rows[strand_rows.len() - 1 - suffix]
                == successor_rows[successor_rows.len() - 1 - suffix]
        {
            suffix += 1;
        }
        Self {
            strand_len: strand_rows.len() as u64,
            splice_start: prefix as u64,
            splice_end: (strand_rows.len() - suffix) as u64,
            successor_end: (successor_rows.len() - suffix) as u64,
        }
    }

    /// Structural well-formedness of a descriptor read back from storage.
    pub fn is_well_formed(&self) -> bool {
        self.splice_start <= self.splice_end
            && self.splice_end <= self.strand_len
            && self.splice_start <= self.successor_end
    }

    /// Rows the superseded strand must physically retain.
    pub fn retained_span(&self) -> std::ops::Range<u64> {
        self.splice_start..self.splice_end
    }

    /// Number of rows the superseded strand must physically retain.
    pub fn retained_rows(&self) -> u64 {
        self.splice_end.saturating_sub(self.splice_start)
    }

    /// Logical row count the successor must serve for this splice to
    /// resolve.
    pub fn successor_len(&self) -> u64 {
        self.successor_end
            .saturating_add(self.strand_len.saturating_sub(self.splice_end))
    }

    /// Whether the splice actually shares rows. A full-transcript
    /// compaction shares nothing (`retained_rows() == strand_len`): the
    /// superseded strand genuinely IS its own retained delta, and no
    /// encoding can shrink it.
    pub fn shares_rows(&self) -> bool {
        self.retained_rows() < self.strand_len
    }

    /// Resolve one superseded-strand index; `None` past `strand_len`.
    pub fn source(&self, index: u64) -> Option<StrandRowSource> {
        if index >= self.strand_len {
            return None;
        }
        if index < self.splice_start {
            return Some(StrandRowSource::Successor(index));
        }
        if index < self.splice_end {
            return Some(StrandRowSource::Retained(index));
        }
        Some(StrandRowSource::Successor(
            index - self.splice_end + self.successor_end,
        ))
    }

    /// The (at most three) contiguous segments covering `range`, in order.
    ///
    /// `range` must already be within `0..strand_len`; out-of-range reads
    /// are the caller's fail-closed decision, not silently clamped content.
    pub fn segments(&self, range: std::ops::Range<u64>) -> impl Iterator<Item = StrandSegment> {
        let start = range.start.min(self.strand_len);
        let end = range.end.clamp(start, self.strand_len);
        let mut spans: [Option<StrandSegment>; 3] = [None, None, None];
        let lead_end = end.min(self.splice_start);
        if start < lead_end {
            spans[0] = Some(StrandSegment::Successor(start..lead_end));
        }
        let own_start = start.max(self.splice_start);
        let own_end = end.min(self.splice_end);
        if own_start < own_end {
            spans[1] = Some(StrandSegment::Retained(own_start..own_end));
        }
        let tail_start = start.max(self.splice_end);
        if tail_start < end {
            spans[2] = Some(StrandSegment::Successor(
                (tail_start - self.splice_end + self.successor_end)
                    ..(end - self.splice_end + self.successor_end),
            ));
        }
        spans.into_iter().flatten()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{
        AssistantBlock, BlockAssistantMessage, StopReason, SystemMessage, SystemNoticeBlock,
        SystemNoticeKind, SystemNoticeMessage, UserMessage,
    };

    /// Minimal incremental store keeping every default-provided method on its
    /// trait default: pins that the range-read capability verbs are
    /// conservative (`load_canonical_head` never advertises a head;
    /// `load_rewrite_commits` derives exactly from `load_rewrites`).
    struct DefaultVerbIncrementalStore {
        rewrites: Vec<TranscriptRewriteRecord>,
    }

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl SessionStore for DefaultVerbIncrementalStore {
        async fn save(&self, _session: &Session) -> Result<(), SessionStoreError> {
            Err(SessionStoreError::Internal(
                "not exercised by the default-verb pin".to_string(),
            ))
        }

        async fn save_transcript_rewrite(
            &self,
            _session: &Session,
            _commit: &TranscriptRewriteCommit,
        ) -> Result<(), SessionStoreError> {
            Err(SessionStoreError::Internal(
                "not exercised by the default-verb pin".to_string(),
            ))
        }

        async fn save_authoritative_projection(
            &self,
            _session: &Session,
        ) -> Result<(), SessionStoreError> {
            Err(SessionStoreError::Internal(
                "not exercised by the default-verb pin".to_string(),
            ))
        }

        async fn save_authoritative_projection_if_current_revision(
            &self,
            _session: &Session,
            _expected_current_revision: Option<String>,
        ) -> Result<(), SessionStoreError> {
            Err(SessionStoreError::Internal(
                "not exercised by the default-verb pin".to_string(),
            ))
        }

        async fn load(&self, _id: &SessionId) -> Result<Option<Session>, SessionStoreError> {
            Ok(None)
        }

        async fn list(
            &self,
            _filter: SessionFilter,
        ) -> Result<Vec<SessionMeta>, SessionStoreError> {
            Ok(Vec::new())
        }

        async fn load_meta(
            &self,
            _id: &SessionId,
        ) -> Result<Option<SessionMeta>, SessionStoreError> {
            Ok(None)
        }

        async fn delete(&self, _id: &SessionId) -> Result<(), SessionStoreError> {
            Err(SessionStoreError::Internal(
                "not exercised by the default-verb pin".to_string(),
            ))
        }

        async fn delete_if_current_revision(
            &self,
            _id: &SessionId,
            _expected_current_revision: &str,
        ) -> Result<bool, SessionStoreError> {
            Err(SessionStoreError::Internal(
                "not exercised by the default-verb pin".to_string(),
            ))
        }

        async fn exists(&self, _id: &SessionId) -> Result<bool, SessionStoreError> {
            Ok(false)
        }
    }

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl IncrementalSessionStore for DefaultVerbIncrementalStore {
        async fn append_messages(
            &self,
            _id: &SessionId,
            _strand: &TranscriptStrandId,
            _base_seq: u64,
            _messages: &[Message],
        ) -> Result<(), SessionStoreError> {
            Err(SessionStoreError::Internal(
                "not exercised by the default-verb pin".to_string(),
            ))
        }

        async fn commit_rewrite(
            &self,
            _id: &SessionId,
            _record: &TranscriptRewriteRecord,
            _expected: SessionHeadCas,
        ) -> Result<SessionHead, SessionStoreError> {
            Err(SessionStoreError::Internal(
                "not exercised by the default-verb pin".to_string(),
            ))
        }

        async fn save_head(
            &self,
            _head: &SessionHead,
            _expected: SessionHeadCas,
        ) -> Result<(), SessionStoreError> {
            Err(SessionStoreError::Internal(
                "not exercised by the default-verb pin".to_string(),
            ))
        }

        async fn load_head(
            &self,
            _id: &SessionId,
        ) -> Result<Option<SessionHead>, SessionStoreError> {
            Ok(None)
        }

        async fn load_messages(
            &self,
            _id: &SessionId,
            _strand: &TranscriptStrandId,
            _range: std::ops::Range<u64>,
        ) -> Result<Vec<Message>, SessionStoreError> {
            Ok(Vec::new())
        }

        async fn load_rewrites(
            &self,
            _id: &SessionId,
        ) -> Result<Vec<TranscriptRewriteRecord>, SessionStoreError> {
            Ok(self.rewrites.clone())
        }
    }

    #[tokio::test]
    #[allow(clippy::expect_used)]
    async fn range_read_defaults_are_conservative() -> Result<(), Box<dyn std::error::Error>> {
        // A store on the trait defaults never advertises a canonical head —
        // every reader stays on the whole-load path.
        let empty = DefaultVerbIncrementalStore {
            rewrites: Vec::new(),
        };
        let id = SessionId::new();
        assert!(empty.load_canonical_head(&id).await?.is_none());
        assert!(empty.load_rewrite_commits(&id).await?.is_empty());

        // The default commit view derives exactly from load_rewrites: same
        // commits, same order.
        let mut session = Session::new();
        session.push(Message::User(UserMessage::text("seed".to_string())));
        session.commit_transcript_rewrite(
            crate::TranscriptRewriteSelection::MessageRange { start: 0, end: 1 },
            vec![Message::User(UserMessage::text("rewritten".to_string()))],
            crate::TranscriptRewriteReason::new("unit-test-edit"),
            Some("unit-test".to_string()),
            None,
        )?;
        let state = session
            .transcript_history_state()?
            .expect("rewrite mints history state");
        let commit = state.commits[0].clone();
        let parent_body = session
            .transcript_revision_body(&commit.parent_revision)?
            .expect("parent body retained");
        let revision_body = session
            .transcript_revision_body(&commit.revision)?
            .expect("revision body retained");
        let record = TranscriptRewriteRecord::new(commit.clone(), parent_body, revision_body)?;
        let store = DefaultVerbIncrementalStore {
            rewrites: vec![record],
        };
        assert_eq!(store.load_rewrite_commits(&id).await?, vec![commit]);
        assert!(
            store.load_canonical_head(&id).await?.is_none(),
            "the conservative default must never advertise a canonical head"
        );
        Ok(())
    }

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

    /// The pre-0.8.9 upgrade pair: `previous` is the inline (0.8.8-shaped)
    /// runtime row after one audited rewrite; `evolved` is that session after
    /// a resume-time system-prompt rewrite (the "agent-factory/resume" shape
    /// from the production defect).
    fn legacy_upgrade_fixture() -> Result<(Session, Session), Box<dyn std::error::Error>> {
        let mut base = Session::new();
        base.push(Message::System(SystemMessage::new("member prompt v1")));
        base.push(Message::User(UserMessage::text(
            "the codeword is birch seventeen".to_string(),
        )));
        let mut previous = base;
        previous.commit_transcript_rewrite(
            TranscriptRewriteSelection::MessageRange { start: 0, end: 1 },
            vec![Message::System(SystemMessage::new("member prompt v2"))],
            crate::TranscriptRewriteReason::new("unit-test-edit"),
            Some("unit-test".to_string()),
            None,
        )?;
        let mut evolved = previous.clone();
        evolved.commit_transcript_rewrite(
            TranscriptRewriteSelection::MessageRange { start: 0, end: 1 },
            vec![Message::System(SystemMessage::new("member prompt v3"))],
            crate::TranscriptRewriteReason::new("resume-system-prompt-refresh"),
            Some("agent-factory/resume".to_string()),
            None,
        )?;
        Ok((previous, evolved))
    }

    /// The slim 0.8.9 boundary materialization of `session`: no inline graph,
    /// the storage-invariant witness under the reserved carrier key —
    /// produced through the real head-projection seam, not hand-forged JSON.
    fn slim_boundary_materialization(
        session: &Session,
    ) -> Result<Session, Box<dyn std::error::Error>> {
        let rewrite_count = session
            .transcript_history_state()?
            .map(|state| state.commits.len() as u64)
            .unwrap_or(0);
        let head = SessionHead::from_session(session, TranscriptStrandId::root(), rewrite_count)?;
        Ok(head.into_session(session.messages().to_vec())?)
    }

    /// The caller's evidence shape: rebuild the evolved graph from its own
    /// append-only rewrite records (the incremental store's durable truth)
    /// and extend the reconstruction to the pinned live head, exactly like
    /// `legacy_upgrade_boundary_history_evidence` in meerkat-session.
    #[allow(clippy::expect_used)]
    fn rebuilt_history_evidence(
        session: &Session,
    ) -> Result<TranscriptHistoryState, Box<dyn std::error::Error>> {
        let state = session
            .transcript_history_state()?
            .expect("evolved session retains history state");
        let mut records = Vec::new();
        for commit in &state.commits {
            let parent_body = session
                .transcript_revision_body(&commit.parent_revision)?
                .expect("parent body retained");
            let revision_body = session
                .transcript_revision_body(&commit.revision)?
                .expect("revision body retained");
            records.push(TranscriptRewriteRecord::new(
                commit.clone(),
                parent_body,
                revision_body,
            )?);
        }
        let mut rebuilt = TranscriptHistoryState::from_rewrite_records(records)?
            .expect("evolved session has adopted rewrites");
        let live_revision = transcript_messages_digest(session.messages())?;
        if rebuilt.head != live_revision {
            if rebuilt
                .revisions
                .iter()
                .any(|body| body.revision == live_revision)
            {
                rebuilt.head = live_revision;
            } else {
                let parent = rebuilt.head.clone();
                rebuilt.revisions.push(TranscriptRevisionBody {
                    revision: live_revision.clone(),
                    parent_revision: Some(parent),
                    messages: session.messages().to_vec(),
                    created_at: SystemTime::now(),
                });
                rebuilt.head = live_revision;
            }
        }
        Ok(rebuilt)
    }

    /// Pins the production defect: after the graph evolves (resume rewrite),
    /// the slim boundary save is refused against the inline previous row —
    /// and stays refused when no evidence is threaded (fail-closed).
    #[test]
    #[allow(clippy::expect_used)]
    fn legacy_upgrade_slim_save_refused_without_evidence() -> Result<(), Box<dyn std::error::Error>>
    {
        let (previous, evolved) = legacy_upgrade_fixture()?;
        let incoming = slim_boundary_materialization(&evolved)?;
        for verdict in [
            run_boundary_snapshot_save_guard(&incoming, Some(&previous)),
            run_boundary_snapshot_save_guard_with_legacy_history_evidence(
                &incoming,
                Some(&previous),
                None,
            ),
        ] {
            let error = verdict.expect_err("evolved slim save must be refused without evidence");
            assert!(
                error
                    .to_string()
                    .contains("incoming save would erase retained transcript history state"),
                "unexpected refusal: {error}"
            );
        }
        Ok(())
    }

    /// The fix: the same refused save is accepted once the caller threads the
    /// evolved graph and the guard verifies binding + ancestry over it.
    #[test]
    fn legacy_upgrade_slim_save_accepts_verified_evolution_evidence()
    -> Result<(), Box<dyn std::error::Error>> {
        let (previous, evolved) = legacy_upgrade_fixture()?;
        let incoming = slim_boundary_materialization(&evolved)?;
        let evidence = rebuilt_history_evidence(&evolved)?;
        assert!(
            run_boundary_snapshot_save_guard(&incoming, Some(&previous)).is_err(),
            "the unwitnessed guard must still refuse — the evidence path is the only admission"
        );
        run_boundary_snapshot_save_guard_with_legacy_history_evidence(
            &incoming,
            Some(&previous),
            Some(&evidence),
        )?;
        Ok(())
    }

    /// The full production shape: an append between the audited rewrites, a
    /// mechanical live-head pin after the last rewrite, and live appends
    /// after the slim materialization (the first turn's messages). Exercises
    /// the record-chain reconstruction, the head-row extension, and the
    /// guard's digest-proved live continuation.
    #[test]
    fn legacy_upgrade_slim_save_accepts_evolution_with_appends()
    -> Result<(), Box<dyn std::error::Error>> {
        let (previous, _) = legacy_upgrade_fixture()?;
        let mut evolved = previous.clone();
        evolved.push(Message::User(UserMessage::text(
            "resume banner".to_string(),
        )));
        evolved.commit_transcript_rewrite(
            TranscriptRewriteSelection::MessageRange { start: 0, end: 1 },
            vec![Message::System(SystemMessage::new("member prompt v3"))],
            crate::TranscriptRewriteReason::new("resume-system-prompt-refresh"),
            Some("agent-factory/resume".to_string()),
            None,
        )?;
        // A post-rewrite append pins a mechanical live-head body into the
        // graph the slim materialization's witness names.
        evolved.push(Message::User(UserMessage::text(
            "post-rewrite note".to_string(),
        )));
        let mut incoming = slim_boundary_materialization(&evolved)?;
        // The first turn appends past the pinned head before the boundary
        // save; the carried witness still names the pinned graph.
        incoming.push(Message::User(UserMessage::text(
            "what was the codeword?".to_string(),
        )));
        incoming.push(Message::User(UserMessage::text(
            "birch seventeen".to_string(),
        )));
        let evidence = rebuilt_history_evidence(&evolved)?;
        assert!(run_boundary_snapshot_save_guard(&incoming, Some(&previous)).is_err());
        run_boundary_snapshot_save_guard_with_legacy_history_evidence(
            &incoming,
            Some(&previous),
            Some(&evidence),
        )?;
        Ok(())
    }

    /// Fork safety: evidence over a graph whose commit prefix differs from
    /// the previous inline graph is refused even though it is internally
    /// consistent and matches the incoming's own carried witness.
    #[test]
    #[allow(clippy::expect_used)]
    fn legacy_upgrade_slim_save_refuses_forked_evidence() -> Result<(), Box<dyn std::error::Error>>
    {
        let mut base = Session::new();
        base.push(Message::System(SystemMessage::new("member prompt v1")));
        base.push(Message::User(UserMessage::text(
            "the codeword is birch seventeen".to_string(),
        )));
        let mut previous = base.clone();
        previous.commit_transcript_rewrite(
            TranscriptRewriteSelection::MessageRange { start: 0, end: 1 },
            vec![Message::System(SystemMessage::new("member prompt v2"))],
            crate::TranscriptRewriteReason::new("unit-test-edit"),
            Some("unit-test".to_string()),
            None,
        )?;
        let mut forked = base;
        forked.commit_transcript_rewrite(
            TranscriptRewriteSelection::MessageRange { start: 0, end: 1 },
            vec![Message::System(SystemMessage::new(
                "forked prompt that never extended the audited history",
            ))],
            crate::TranscriptRewriteReason::new("unit-test-fork"),
            Some("unit-test".to_string()),
            None,
        )?;
        let incoming = slim_boundary_materialization(&forked)?;
        let evidence = rebuilt_history_evidence(&forked)?;
        let error = run_boundary_snapshot_save_guard_with_legacy_history_evidence(
            &incoming,
            Some(&previous),
            Some(&evidence),
        )
        .expect_err("forked evidence must never be admitted");
        assert!(
            matches!(error, SessionStoreError::InvalidTranscriptRewrite { .. }),
            "unexpected fork verdict: {error}"
        );
        Ok(())
    }

    /// Same-graph slim round-trips keep passing through the existing exact
    /// witness carve-out, and evidence is never consulted for them: even
    /// deliberately poisoned evidence cannot change the verdict.
    #[test]
    fn legacy_upgrade_same_graph_round_trip_still_accepted()
    -> Result<(), Box<dyn std::error::Error>> {
        let (previous, _) = legacy_upgrade_fixture()?;
        let incoming = slim_boundary_materialization(&previous)?;
        run_boundary_snapshot_save_guard(&incoming, Some(&previous))?;
        let poisoned = TranscriptHistoryState {
            head: "sha256:not-a-real-revision".to_string(),
            commits: Vec::new(),
            revisions: Vec::new(),
            digest_format: 0,
            replay_cursor: None,
        };
        run_boundary_snapshot_save_guard_with_legacy_history_evidence(
            &incoming,
            Some(&previous),
            Some(&poisoned),
        )?;
        Ok(())
    }

    /// A v3 (revision-identity) carrier over the SAME graph must round-trip
    /// against an inline previous row: the carve-out derives the reference
    /// witness under the format the carrier declares instead of assuming v2.
    #[test]
    #[allow(clippy::expect_used)]
    fn legacy_upgrade_same_graph_v3_carrier_round_trip_accepted()
    -> Result<(), Box<dyn std::error::Error>> {
        let (previous, _) = legacy_upgrade_fixture()?;
        let mut incoming = slim_boundary_materialization(&previous)?;
        let previous_state = previous
            .transcript_history_state()?
            .expect("previous retains history state");
        let v3 =
            crate::checkpoint::transcript_history_checkpoint_digest_in_format(&previous_state, 3)?;
        incoming.set_metadata_unchecked_for_test(
            SESSION_TRANSCRIPT_HISTORY_CHECKPOINT_DIGEST_KEY,
            serde_json::json!({
                "witness_format": 3,
                "revision_digest_format": 2,
                "digest": v3.as_str(),
            }),
        );
        run_boundary_snapshot_save_guard(&incoming, Some(&previous))?;
        Ok(())
    }

    /// When the previous row is already slim, the legacy path is not
    /// reachable at all: a plain append commits through the ordinary guard
    /// and poisoned evidence is never read.
    #[test]
    fn legacy_upgrade_evidence_unreachable_when_previous_is_slim()
    -> Result<(), Box<dyn std::error::Error>> {
        let (_, evolved) = legacy_upgrade_fixture()?;
        let previous_slim = slim_boundary_materialization(&evolved)?;
        let mut incoming = previous_slim.clone();
        incoming.push(Message::User(UserMessage::text(
            "next turn message".to_string(),
        )));
        let poisoned = TranscriptHistoryState {
            head: "sha256:not-a-real-revision".to_string(),
            commits: Vec::new(),
            revisions: Vec::new(),
            digest_format: 0,
            replay_cursor: None,
        };
        run_boundary_snapshot_save_guard_with_legacy_history_evidence(
            &incoming,
            Some(&previous_slim),
            Some(&poisoned),
        )?;
        Ok(())
    }

    /// A slim incoming with NO carried witness is genuine erasure: evidence
    /// cannot bind to anything and the refusal keeps its exact message.
    #[test]
    #[allow(clippy::expect_used)]
    fn legacy_upgrade_missing_witness_keeps_erasure_refusal()
    -> Result<(), Box<dyn std::error::Error>> {
        let (previous, evolved) = legacy_upgrade_fixture()?;
        let incoming = slim_boundary_materialization(&evolved)?;
        let mut envelope = serde_json::to_value(&incoming)?;
        envelope["metadata"]
            .as_object_mut()
            .expect("metadata object")
            .remove(SESSION_TRANSCRIPT_HISTORY_CHECKPOINT_DIGEST_KEY);
        let incoming: Session = serde_json::from_value(envelope)?;
        let evidence = rebuilt_history_evidence(&evolved)?;
        let error = run_boundary_snapshot_save_guard_with_legacy_history_evidence(
            &incoming,
            Some(&previous),
            Some(&evidence),
        )
        .expect_err("a witnessless slim save is genuine erasure");
        assert!(
            error
                .to_string()
                .contains("incoming save would erase retained transcript history state"),
            "unexpected refusal: {error}"
        );
        Ok(())
    }

    /// Malformed evidence propagates as a typed error instead of being
    /// reduced to acceptance or to the generic erasure refusal.
    #[test]
    #[allow(clippy::expect_used)]
    fn legacy_upgrade_malformed_evidence_propagates_typed_error()
    -> Result<(), Box<dyn std::error::Error>> {
        let (previous, evolved) = legacy_upgrade_fixture()?;
        let incoming = slim_boundary_materialization(&evolved)?;
        let mut evidence = rebuilt_history_evidence(&evolved)?;
        evidence.revisions[0]
            .messages
            .push(Message::User(UserMessage::text(
                "tampered body no longer matching its revision digest".to_string(),
            )));
        let error = run_boundary_snapshot_save_guard_with_legacy_history_evidence(
            &incoming,
            Some(&previous),
            Some(&evidence),
        )
        .expect_err("tampered evidence must refuse typed");
        assert!(
            error
                .to_string()
                .contains("legacy upgrade history evidence"),
            "unexpected malformed-evidence verdict: {error}"
        );
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

    /// Adoption-arm strengthening pin (seal retype, 0.8.9): a first-boundary
    /// adoption graph whose rewrite records are INDIVIDUALLY valid but whose
    /// commit chain does not link must be rejected. The retired per-commit
    /// loop this arm used to run never checked chain linkage, so this shape
    /// was silently accepted; the sealed whole-graph proof the arm now
    /// demands includes the chain walk. Behaviour change in the safe
    /// direction — this test pins it.
    #[test]
    #[allow(clippy::expect_used)]
    fn adoption_arm_rejects_a_graph_whose_commit_chain_does_not_link()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut lineage_a = Session::new();
        lineage_a.push(Message::User(UserMessage::text("a-1".to_string())));
        lineage_a.push(Message::User(UserMessage::text("a-2".to_string())));
        lineage_a.commit_transcript_rewrite(
            crate::TranscriptRewriteSelection::MessageRange { start: 1, end: 2 },
            vec![Message::User(UserMessage::text(
                "a-2-rewritten".to_string(),
            ))],
            crate::TranscriptRewriteReason::new("unit-test"),
            Some("unit-test".to_string()),
            None,
        )?;

        let mut lineage_b = Session::new();
        lineage_b.push(Message::User(UserMessage::text("b-1".to_string())));
        lineage_b.push(Message::User(UserMessage::text("b-2".to_string())));
        lineage_b.commit_transcript_rewrite(
            crate::TranscriptRewriteSelection::MessageRange { start: 1, end: 2 },
            vec![Message::User(UserMessage::text(
                "b-2-rewritten".to_string(),
            ))],
            crate::TranscriptRewriteReason::new("unit-test"),
            Some("unit-test".to_string()),
            None,
        )?;

        let state_a = lineage_a
            .transcript_history_state()?
            .expect("lineage A graph");
        let state_b = lineage_b
            .transcript_history_state()?
            .expect("lineage B graph");
        // Splice: A's record followed by B's record. B's parent is neither
        // A's revision nor an extension of it; every body is present and
        // digest-consistent, so each record validates in isolation.
        let mut spliced = state_a;
        spliced.commits.extend(state_b.commits.iter().cloned());
        for body in &state_b.revisions {
            if !spliced
                .revisions
                .iter()
                .any(|existing| existing.revision == body.revision)
            {
                spliced.revisions.push(body.clone());
            }
        }
        spliced.head = state_b.head.clone();

        // The incoming live transcript matches the spliced head exactly, so
        // the retired per-commit loop's only other check (head == incoming
        // digest) would have passed and the graph would have been adopted.
        let mut incoming = lineage_b.clone();
        incoming.set_metadata_unchecked_for_test(
            crate::session::SESSION_TRANSCRIPT_HISTORY_STATE_KEY,
            serde_json::to_value(&spliced)?,
        );

        let error = run_boundary_snapshot_save_guard(&incoming, None)
            .expect_err("non-linking adoption chain must be rejected");
        let rendered = error.to_string();
        assert!(
            rendered.contains("transcript history state is malformed")
                || rendered.contains("does not extend"),
            "expected the sealed whole-graph proof to reject the non-linking \
             chain, got: {rendered}"
        );
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
            digest_format: 0,
            replay_cursor: None,
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
                digest_format: 0,
                replay_cursor: None,
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
                digest_format: 0,
                replay_cursor: None,
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
                digest_format: 0,
                replay_cursor: None,
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
                digest_format: 0,
                replay_cursor: None,
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
                digest_format: 0,
                replay_cursor: None,
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
                digest_format: 0,
                replay_cursor: None,
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
                digest_format: 0,
                replay_cursor: None,
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
                digest_format: 0,
                replay_cursor: None,
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
                digest_format: 0,
                replay_cursor: None,
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
        incoming.messages.replace(
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

    /// Required pin for the audited-body fast path: canonical digests
    /// deliberately erase transcript message identity and `created_at`, so two
    /// audited bodies can be digest-equal (same transcript) while
    /// `Message: PartialEq` says they differ. The structural compare is only a
    /// FAST PATH; the digest compare must still admit the save. A bare
    /// `PartialEq` here would reject every boundary save of an affected
    /// session fail-closed and freeze its writes.
    #[test]
    fn append_only_guard_admits_digest_equal_audited_body_with_rebuilt_bookkeeping()
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
        let mut state = incoming
            .transcript_history_state()?
            .ok_or_else(|| std::io::Error::other("incoming history missing"))?;
        // Re-project every audited body the way a re-derivation path does:
        // fresh construction bookkeeping, identical conversation content. The
        // revision strings are unchanged because the digest erases exactly
        // these fields.
        let mut rebuilt_any = false;
        for body in &mut state.revisions {
            for message in &mut body.messages {
                if let Message::User(user) = message {
                    user.identity = user.identity.with_run_id(crate::lifecycle::RunId::new());
                    user.created_at = chrono::Utc::now();
                    rebuilt_any = true;
                }
            }
        }
        assert!(rebuilt_any, "fixture must rebuild at least one body");
        incoming.set_metadata_unchecked_for_test(
            crate::session::SESSION_TRANSCRIPT_HISTORY_STATE_KEY,
            serde_json::to_value(state)?,
        );
        incoming.push(Message::User(UserMessage::text(
            "ordinary append".to_string(),
        )));

        // Sanity: the bodies really are structurally different but
        // digest-identical, i.e. the fixture exercises the fallback.
        let previous_state = previous
            .transcript_history_state()?
            .ok_or_else(|| std::io::Error::other("previous history missing"))?;
        let incoming_state = incoming
            .transcript_history_state()?
            .ok_or_else(|| std::io::Error::other("incoming history missing"))?;
        let audited = previous_state.commits[0].parent_revision.clone();
        let previous_body = previous_state
            .revisions
            .iter()
            .find(|body| body.revision == audited)
            .ok_or_else(|| std::io::Error::other("previous audited body missing"))?;
        let incoming_body = incoming_state
            .revisions
            .iter()
            .find(|body| body.revision == audited)
            .ok_or_else(|| std::io::Error::other("incoming audited body missing"))?;
        assert_ne!(previous_body.messages, incoming_body.messages);
        assert_eq!(
            transcript_messages_digest(&previous_body.messages)?,
            transcript_messages_digest(&incoming_body.messages)?
        );
        assert!(audited_bodies_are_equivalent(
            &previous_body.messages,
            &incoming_body.messages
        )?);

        append_only_save_guard(&incoming, Some(&previous))?;
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
            digest_format: 0,
            replay_cursor: None,
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
            digest_format: 0,
            replay_cursor: None,
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
        incoming.messages.replace(rebookkept);

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
        incoming.messages.replace(diverged);

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
        let (_, mut compacted, _) = compacted_session_fixture();
        let stamp = crate::SessionCheckpointStamp::root(
            &compacted,
            crate::SessionCheckpointProvenance::SessionCreated,
        )
        .expect("typed root stamp");
        compacted
            .install_checkpoint_stamp(stamp.clone())
            .expect("install typed root stamp");
        let full_checkpoint_digest =
            crate::session_checkpoint_digest(&compacted).expect("full checkpoint digest");
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
        assert!(
            head.metadata
                .contains_key(SESSION_TRANSCRIPT_HISTORY_CHECKPOINT_DIGEST_KEY),
            "head must retain the semantic history witness"
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
        assert_eq!(
            crate::session_checkpoint_digest(&slim).expect("slim checkpoint digest"),
            full_checkpoint_digest,
            "full and out-of-line history representations must have one checkpoint identity"
        );
        assert_eq!(
            slim.try_checkpoint_state().expect("verify slim stamp"),
            crate::SessionCheckpointState::Verified(stamp)
        );
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

    /// Strand-row tamper AFTER a fully verified load of the same head must
    /// still fail closed `Corrupted`.
    ///
    /// Regression test for a deleted verification bypass: a process-global
    /// Boolean memo keyed on (session id, head revision, message count) let
    /// `into_session` skip digest verification once the tuple had been proven
    /// this process. The tuple never bound the row bytes, so the sequence
    /// "load valid rows (memo warms) -> corrupt a strand row, head row
    /// untouched (key unchanged) -> reload" served the corrupted transcript
    /// unverified. The sibling test above corrupts BEFORE any verified load,
    /// so it never exercised this sequence.
    #[test]
    #[allow(clippy::expect_used)]
    fn session_head_into_session_detects_strand_tamper_after_verified_load() {
        let mut session = Session::new();
        session.push(Message::System(SystemMessage::new("base system")));
        session.push(Message::User(UserMessage::text("hello".to_string())));
        session.push(Message::User(UserMessage::text("world".to_string())));
        let head = SessionHead::from_session(&session, TranscriptStrandId::root(), 0)
            .expect("head projection");

        // Model the fresh-process reader (restart -> resume from durable
        // rows), where the substitution memo is necessarily cold:
        // `from_session` above recorded this session's proven vector in the
        // process-global memo, which would satisfy the valid load below via
        // byte-exact substitution and mask the verification sequence under
        // test. Displace that entry through the memo's own same-id retain
        // with a key no lookup can match.
        if let Some(snapshot) = session.shared_transcript_snapshot() {
            record_slim_materialization_snapshot(
                session.id(),
                "reader-model-displaced",
                u64::MAX,
                snapshot,
            );
        }

        // Full valid load: first-sight verification passes (this is the step
        // that warmed the deleted Boolean memo).
        let loaded = head
            .clone()
            .into_session(session.messages().to_vec())
            .expect("valid rows must materialize");
        assert_eq!(loaded.messages(), session.messages());

        // Corrupt ONE strand row; the head row (id, revision, count) is
        // untouched, so the deleted memo's tuple key still matched.
        let mut corrupted = session.messages().to_vec();
        corrupted[1] = Message::User(UserMessage::text("tampered strand row".to_string()));
        assert_eq!(corrupted.len() as u64, head.message_count);

        // Reload after tamper: the corruption must be DETECTED, not served.
        assert!(matches!(
            head.into_session(corrupted),
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

    // ---------------------------------------------------------------------
    // StrandSplice: the pure delta math behind bounded strand storage.
    // ---------------------------------------------------------------------

    /// Every index of `strand` must resolve to the row `expected` holds
    /// there, sourcing from `strand`'s retained span or from `successor`.
    #[allow(clippy::expect_used)]
    fn assert_splice_reconstructs(strand: &[&str], successor: &[&str]) -> StrandSplice {
        let splice = StrandSplice::between(strand, successor);
        assert!(
            splice.is_well_formed(),
            "derived splice must be well formed: {splice:?}"
        );
        assert_eq!(
            splice.successor_len(),
            successor.len() as u64,
            "splice must imply the successor's true length: {splice:?}"
        );
        for (index, expected) in strand.iter().enumerate() {
            let source = splice
                .source(index as u64)
                .expect("in-range index must resolve");
            let actual = match source {
                StrandRowSource::Retained(at) => {
                    assert!(
                        splice.retained_span().contains(&at),
                        "retained source {at} must fall inside {:?}",
                        splice.retained_span()
                    );
                    strand[at as usize]
                }
                StrandRowSource::Successor(at) => successor[at as usize],
            };
            assert_eq!(actual, *expected, "row {index} resolved to the wrong body");
        }
        assert!(
            splice.source(strand.len() as u64).is_none(),
            "past-the-end index must not resolve"
        );

        // Every sub-range must segment to exactly the same rows, in order.
        for start in 0..=strand.len() as u64 {
            for end in start..=strand.len() as u64 {
                let mut served: Vec<&str> = Vec::new();
                for segment in splice.segments(start..end) {
                    match segment {
                        StrandSegment::Retained(range) => {
                            assert!(
                                range.start >= splice.splice_start
                                    && range.end <= splice.splice_end,
                                "retained segment {range:?} escaped the retained span"
                            );
                            served.extend(strand[range.start as usize..range.end as usize].iter());
                        }
                        StrandSegment::Successor(range) => {
                            served
                                .extend(successor[range.start as usize..range.end as usize].iter());
                        }
                    }
                }
                assert_eq!(
                    served,
                    strand[start as usize..end as usize].to_vec(),
                    "segments of {start}..{end} must serve the superseded strand exactly"
                );
            }
        }
        splice
    }

    #[test]
    fn strand_splice_shares_the_prefix_when_only_the_tail_changed() {
        let splice = assert_splice_reconstructs(&["a", "b", "c"], &["a", "b", "z", "y"]);
        assert_eq!(splice.splice_start, 2);
        assert_eq!(splice.splice_end, 3);
        assert_eq!(splice.retained_rows(), 1);
        assert!(splice.shares_rows());
    }

    /// The production shape: a one-message edit at index 0 of a long
    /// transcript must retain exactly one row, not a whole copy.
    #[test]
    fn strand_splice_leading_edit_retains_one_row_of_a_long_transcript() {
        let old: Vec<String> = (0..64).map(|i| format!("m{i}")).collect();
        let mut new = old.clone();
        new[0] = "system refreshed".to_string();
        let splice = StrandSplice::between(&old, &new);
        assert_eq!(splice.splice_start, 0);
        assert_eq!(splice.splice_end, 1);
        assert_eq!(splice.successor_end, 1);
        assert_eq!(
            splice.retained_rows(),
            1,
            "a one-message leading edit must retain exactly one row"
        );
        assert_eq!(splice.successor_len(), 64);
        let borrowed: Vec<&str> = old.iter().map(String::as_str).collect();
        let borrowed_new: Vec<&str> = new.iter().map(String::as_str).collect();
        assert_splice_reconstructs(&borrowed, &borrowed_new);
    }

    #[test]
    fn strand_splice_shares_the_suffix_when_the_head_changed() {
        let splice = assert_splice_reconstructs(&["a", "b", "c"], &["x", "b", "c"]);
        assert_eq!(splice.splice_start, 0);
        assert_eq!(splice.splice_end, 1);
        assert_eq!(splice.successor_end, 1);
        assert_eq!(splice.retained_rows(), 1);
    }

    #[test]
    fn strand_splice_over_a_pure_append_retains_nothing() {
        let splice = assert_splice_reconstructs(&["a", "b"], &["a", "b", "c"]);
        assert_eq!(splice.retained_rows(), 0);
        assert_eq!(splice.splice_start, 2);
        assert_eq!(splice.splice_end, 2);
        assert_eq!(splice.successor_end, 3);
        assert!(splice.shares_rows());
    }

    #[test]
    fn strand_splice_over_a_truncation_retains_the_dropped_tail() {
        let splice = assert_splice_reconstructs(&["a", "b", "c"], &["a"]);
        assert_eq!(splice.retained_span(), 1..3);
        assert_eq!(splice.successor_end, 1);
    }

    /// A full-transcript compaction shares nothing: the splice must say so
    /// rather than claim a false overlap, and backends must keep the strand
    /// materialized.
    #[test]
    fn strand_splice_over_a_full_replacement_shares_nothing() {
        let splice = assert_splice_reconstructs(&["a", "b", "c"], &["summary"]);
        assert_eq!(splice.retained_span(), 0..3);
        assert_eq!(splice.retained_rows(), 3);
        assert!(!splice.shares_rows());
    }

    #[test]
    fn strand_splice_between_identical_strands_retains_nothing() {
        let splice = assert_splice_reconstructs(&["a", "b"], &["a", "b"]);
        assert_eq!(splice.retained_rows(), 0);
        assert_eq!(splice.splice_start, 2);
        assert_eq!(splice.successor_end, 2);
    }

    /// Repeated rows must not let the prefix and suffix scans overlap and
    /// double-count a shared row.
    #[test]
    fn strand_splice_does_not_overlap_prefix_and_suffix_on_repeated_rows() {
        let splice = assert_splice_reconstructs(&["a", "a", "a"], &["a", "a"]);
        assert_eq!(splice.splice_start, 2);
        assert_eq!(splice.splice_end, 3);
        assert_eq!(splice.successor_end, 2);
        assert_eq!(splice.retained_rows(), 1);
    }

    #[test]
    fn strand_splice_over_an_empty_successor_retains_everything() {
        let splice = assert_splice_reconstructs(&["a", "b"], &[]);
        assert_eq!(splice.retained_span(), 0..2);
        assert_eq!(splice.successor_len(), 0);
        assert!(!splice.shares_rows());
    }

    #[test]
    fn strand_splice_over_an_empty_strand_is_inert() {
        let splice = assert_splice_reconstructs(&[], &["a"]);
        assert_eq!(splice.strand_len, 0);
        assert_eq!(splice.retained_rows(), 0);
        assert!(!splice.shares_rows());
    }

    #[test]
    fn malformed_persisted_splices_are_detectable() {
        assert!(
            !StrandSplice {
                strand_len: 4,
                splice_start: 3,
                splice_end: 2,
                successor_end: 3,
            }
            .is_well_formed(),
            "an inverted span must not pass well-formedness"
        );
        assert!(
            !StrandSplice {
                strand_len: 4,
                splice_start: 1,
                splice_end: 9,
                successor_end: 1,
            }
            .is_well_formed(),
            "a span past strand_len must not pass well-formedness"
        );
        assert!(
            !StrandSplice {
                strand_len: 4,
                splice_start: 2,
                splice_end: 3,
                successor_end: 1,
            }
            .is_well_formed(),
            "a replacement ending before the shared prefix must not pass"
        );
    }
}
