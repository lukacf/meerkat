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

use crate::session::{
    SESSION_TRANSCRIPT_HISTORY_STATE_KEY, SESSION_TRANSCRIPT_REWRITE_PREFIX_AUTHORITY_KEY,
    SessionHeadMetadataIdentity, SessionHeadMetadataProjection, SessionMeta,
    TranscriptRevisionBody,
};
use crate::time_compat::SystemTime;
use crate::types::{Message, SessionId, Usage};
use crate::{
    ComponentEventPrefixAuthority, PreparedComponentEventSuffix, Session, SessionComponentKind,
    TranscriptGraphPrefixAccumulator, TranscriptHistoryState, TranscriptRevisionEdge,
    TranscriptRewriteCommit, TranscriptRewritePrefixAccumulator, TranscriptRewriteRecord,
    ValidatedTranscriptHistory, VerifiedComponentEventSequence, transcript_messages_digest,
};
#[cfg(test)]
use crate::{TranscriptRewriteParentTransition, TranscriptRewriteSelection};

fn session_realtime_component_root(
    session: &Session,
) -> Result<ComponentEventPrefixAuthority, SessionStoreError> {
    session.realtime_component_event_prefix().map_err(|error| {
        SessionStoreError::Serialization(format!(
            "failed to derive realtime component root: {error}"
        ))
    })
}

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

// Slim materialization deliberately has no process-global substitution or
// verification memo. Durable message-row bytes are the authority: every load
// verifies their exact row-prefix commitment when carried and always verifies
// the semantic transcript digest before returning a `Session`. A cache keyed by
// projections of those rows can otherwise replace the bytes being verified and
// hide durable corruption.

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

/// Prove coherence between a validated audited graph and the live Session.
///
/// Equality covers rewrite boundaries. After ordinary appends the graph
/// remains at its latest audited rewrite endpoint, so the only admissible
/// relaxation is a content-addressed prefix proof from that retained endpoint
/// to the live transcript. Commitless compatibility graphs keep exact-only
/// semantics inside `Session::live_transcript_extends_history_head`.
fn validate_live_transcript_history_head_coherence(
    session: &Session,
    state: &TranscriptHistoryState,
    live_revision: &str,
    subject: &str,
) -> Result<(), SessionStoreError> {
    let coherent = session
        .live_transcript_extends_history_head(state, live_revision)
        .map_err(|error| SessionStoreError::InvalidTranscriptRewrite {
            id: session.id().clone(),
            reason: format!("{subject} transcript history state is malformed: {error}"),
        })?;
    if coherent {
        return Ok(());
    }
    Err(SessionStoreError::InvalidTranscriptRewrite {
        id: session.id().clone(),
        reason: format!(
            "{subject} transcript graph audited head {} is neither the exact live revision \
             {live_revision} nor its retained prefix ancestor",
            state.head()
        ),
    })
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
    let _digest_site = crate::digest_observability::enter_digest_site(
        crate::digest_observability::DIGEST_SITE_APPEND_GUARD,
    );
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
    if let Some(state) = incoming_state.as_deref() {
        validate_live_transcript_history_head_coherence(
            incoming,
            state,
            &incoming_revision,
            "incoming",
        )?;
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
    if previous_state.is_some() && !incoming_has_history {
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
        return Err(SessionStoreError::InvalidTranscriptRewrite {
            id: incoming.id().clone(),
            reason: "incoming append-only save would erase retained transcript history state"
                .to_string(),
        });
    };
    if incoming_state.commit_count() != previous_state.commit_count()
        || !incoming_state.extends_exact_graph(previous_state)
    {
        return Err(SessionStoreError::InvalidTranscriptRewrite {
            id: incoming.id().clone(),
            reason:
                "incoming append-only save would change retained compact transcript graph authority"
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
    validate_live_transcript_history_head_coherence(
        previous,
        previous_state,
        &previous_revision,
        "previous",
    )?;
    validate_live_transcript_history_head_coherence(
        incoming,
        incoming_state,
        incoming_revision,
        "incoming append-only save",
    )?;

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
    if !incoming_state.extends_exact_graph(previous_state) {
        return Err(SessionStoreError::InvalidTranscriptRewrite {
            id: incoming.id().clone(),
            reason:
                "incoming rewrite save would change retained compact transcript graph authority"
                    .to_string(),
        });
    }
    Ok(())
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
    let _digest_site = crate::digest_observability::enter_digest_site(
        crate::digest_observability::DIGEST_SITE_BOUNDARY_GUARD,
    );
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
                    && sealed.commit_count() != 0
                {
                    validate_live_transcript_history_head_coherence(
                        incoming,
                        sealed.state(),
                        &incoming_revision,
                        "incoming adopted",
                    )?;
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
                state.head(),
            )?;
            if commits.is_none()
                && run_boundary_context_summary_retained_source_projection_save_guard(
                    incoming, previous, &sealed,
                )?
            {
                return Ok(());
            }
            let Some(commits) = commits else {
                return Err(append_error);
            };
            let Some(commit) = commits.first() else {
                if state.commit_count() == 0 {
                    return Err(append_error);
                }
                // Empty chain: the persisted row is already at (or past) the
                // latest audited rewrite and the live transcript extends that
                // endpoint by plain appends. Every retained commit and body is
                // covered by the seal; prove only the audited-head/live-tail
                // relation that is outside the graph.
                validate_live_transcript_history_head_coherence(
                    incoming,
                    state,
                    &incoming_revision,
                    "incoming",
                )?;
                return Ok(());
            };
            transcript_rewrite_bridge_save_guard(incoming, commit, &sealed, &incoming_revision)?;
            // Trailing rebookkept commits beyond the walked chain stay
            // digest-consistent by the same seal; no per-commit re-proof.
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
    let Some(history) = incoming
        .validated_transcript_history_state()
        .map_err(|err| SessionStoreError::InvalidTranscriptRewrite {
            id: incoming.id().clone(),
            reason: format!("incoming transcript history state is malformed: {err}"),
        })?
    else {
        return Ok(());
    };
    let incoming_revision = incoming
        .transcript_content_digest()
        .map_err(SessionStoreError::from)?;
    validate_live_transcript_history_head_coherence(
        incoming,
        history.state(),
        &incoming_revision,
        "incoming",
    )
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
    if state.commit_count() != 0 {
        return Ok(false);
    }

    let incoming_revision =
        transcript_messages_digest(incoming.messages()).map_err(SessionStoreError::from)?;
    if state.head() != incoming_revision || !state.contains_revision(&incoming_revision) {
        return Ok(false);
    }

    let mut projection_without_history = incoming.clone();
    projection_without_history.clear_transcript_history_state();
    if append_only_save_guard(&projection_without_history, previous).is_err() {
        return Ok(false);
    }

    let Some(previous) = previous else {
        return Ok(state.commit_count() == 0);
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

    // The append-only guard above already proved the exact previous rows are
    // the incoming prefix. A zero-edge compact graph has no occurrence beyond
    // its anchor from which to derive a separate digest ancestry relation.
    Ok(true)
}

fn run_boundary_context_summary_retained_source_projection_save_guard(
    incoming: &Session,
    previous: &Session,
    state: &ValidatedTranscriptHistory,
) -> Result<bool, SessionStoreError> {
    if state.commit_count() == 0 {
        return Ok(false);
    }

    if incoming.messages().len() <= previous.messages().len() {
        return Ok(false);
    }

    // This projection is exactly one typed compaction-summary insertion into
    // the previously persisted ordered transcript, followed by an optional
    // newly generated suffix. Derive the insertion boundary from the first
    // unequal source row; no role or row position receives special treatment.
    let insertion_offset = incoming
        .messages()
        .iter()
        .zip(previous.messages())
        .take_while(|(incoming, previous)| incoming == previous)
        .count();
    let Some(Message::User(summary)) = incoming.messages().get(insertion_offset) else {
        return Ok(false);
    };
    // Typed marker, not content classification: the runtime compaction producer
    // stamps the rebuilt-transcript boundary message with the
    // `CompactionSummary` transcript role. The save-guard admits the divergent
    // rewrite parent only when that typed fact is present.
    if !summary.transcript_role.is_compaction_summary() {
        return Ok(false);
    }

    let retained_end = insertion_offset
        .checked_add(1)
        .and_then(|after_summary| {
            after_summary.checked_add(previous.messages().len() - insertion_offset)
        })
        .ok_or_else(|| SessionStoreError::Corrupted(incoming.id().clone()))?;
    let Some(retained_suffix) = incoming.messages().get(insertion_offset + 1..retained_end) else {
        return Ok(false);
    };
    if retained_suffix != &previous.messages()[insertion_offset..] {
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
            return state
                .unique_revision_position(cursor)
                .is_some()
                .then_some(chain);
        }
        if !visited.insert(cursor.to_string()) {
            return None;
        }
        let commit = state.commits().find(|commit| {
            (commit.parent_revision == cursor
                || state.revision_extends(&commit.parent_revision, cursor))
                && (incoming_revision == state.head()
                    || state.revision_extends(incoming_revision, &commit.revision))
        });
        let Some(commit) = commit else {
            return state
                .revision_extends(incoming_revision, cursor)
                .then_some(chain);
        };
        cursor = &commit.revision;
        chain.push(commit);
    }
}

/// Per-save-operation observability for rewrite-chain searches.
///
/// Compact graphs carry exact occurrence edges and row-prefix lineage, so the
/// search no longer caches materialized historical bodies or their digests.
/// The counter remains as the stable caller-facing measurement surface:
/// compact core searches add zero, while adapters may record explicit external
/// digest passes.
#[derive(Debug, Default)]
pub struct RewriteChainSearchMemo {
    /// Full content-digest passes computed by searches under this memo.
    digests_computed: u64,
}

impl RewriteChainSearchMemo {
    /// Full content-digest passes computed by searches under this memo so
    /// far. Observability for the `MEERKAT_TRACE_REWRITE_MATERIALIZE` trace
    /// lines; a memo hit costs zero.
    #[must_use]
    pub fn digests_computed(&self) -> u64 {
        self.digests_computed
    }

    /// Count digest passes performed by a caller-side (non-core) predicate
    /// evaluation against this memo's total.
    pub fn record_external_digests(&mut self, passes: u64) {
        self.digests_computed = self.digests_computed.saturating_add(passes);
    }
}

/// Find a rewrite chain whose first parent may be an append-only continuation
/// of a previously persisted snapshot.
///
/// Runtime-backed sessions can append messages in the runtime store before a
/// core-owned compaction rewrite reaches the compatibility `SessionStore`. In
/// that case the first rewrite commit's parent revision is
/// not equal to the persisted row's digest, but its retained parent body proves
/// a normal append path from that persisted row.
///
/// The graph arrives already proved. Exact graph occurrence order handles
/// audited endpoints; if the saved snapshot falls inside a later rewrite
/// parent's append suffix, exact durable-row lineage proves that relation
/// without materializing either historical document.
pub fn find_transcript_rewrite_commit_chain_extending_session<'a>(
    state: &'a ValidatedTranscriptHistory,
    previous: &Session,
    incoming_revision: &str,
) -> Result<Option<Vec<&'a TranscriptRewriteCommit>>, SessionStoreError> {
    let mut memo = RewriteChainSearchMemo::default();
    find_transcript_rewrite_commit_chain_extending_session_with_memo(
        state,
        previous,
        incoming_revision,
        &mut memo,
    )
}

/// [`find_transcript_rewrite_commit_chain_extending_session`] with a
/// caller-owned [`RewriteChainSearchMemo`]. Compact graph searches themselves
/// do not compute semantic digests; the memo remains the accounting surface for
/// explicit caller-side work.
///
/// # Errors
///
/// Propagates exact durable-row lineage serialization failures.
pub fn find_transcript_rewrite_commit_chain_extending_session_with_memo<'a>(
    state: &'a ValidatedTranscriptHistory,
    previous: &Session,
    incoming_revision: &str,
    _memo: &mut RewriteChainSearchMemo,
) -> Result<Option<Vec<&'a TranscriptRewriteCommit>>, SessionStoreError> {
    let _digest_site = crate::digest_observability::enter_digest_site(
        crate::digest_observability::DIGEST_SITE_REWRITE_CHAIN_WALK,
    );
    let state = state.state();
    let previous_revision = previous
        .transcript_content_digest()
        .map_err(SessionStoreError::from)?;
    if incoming_revision == state.head() {
        let previous_history = previous.transcript_history_state().map_err(|error| {
            SessionStoreError::InvalidTranscriptRewrite {
                id: previous.id().clone(),
                reason: format!("previous transcript history state is malformed: {error}"),
            }
        })?;
        if let Some(previous_history) = previous_history.as_ref() {
            if state.extends_exact_graph(previous_history) {
                return Ok(Some(
                    state
                        .commits()
                        .skip(previous_history.commit_count())
                        .collect(),
                ));
            }
        } else {
            let previous_count = u64::try_from(previous.messages().len())
                .map_err(|_| SessionStoreError::Corrupted(previous.id().clone()))?;
            let previous_prefix = match previous.exact_message_row_prefix_at(previous_count) {
                Some(prefix) => prefix,
                None => SessionMessageRowPrefixAccumulator::from_messages(previous.messages())?,
            };
            if previous_count == state.anchor().row_prefix().row_count()
                && previous_prefix == *state.anchor().row_prefix()
            {
                return Ok(Some(state.commits().collect()));
            }
        }
    }
    let mut chain = Vec::new();
    let mut cursor = previous_revision.as_str();
    let mut visited = std::collections::BTreeSet::new();
    loop {
        if incoming_revision == cursor {
            return Ok(state
                .unique_revision_position(cursor)
                .is_some()
                .then_some(chain));
        }
        if !visited.insert(cursor.to_string()) {
            return Ok(None);
        }

        // Exact graph edges are authoritative: a commit recorded directly
        // against this cursor advances the walk (and keeps that commit on
        // the audited persistence chain). A commit whose revision is the
        // cursor itself, or any revision this walk already visited, cannot
        // make progress and is never selected.
        let mut selected = None;
        for commit in state.commits() {
            if commit.revision == cursor || visited.contains(&commit.revision) {
                continue;
            }
            if incoming_revision != state.head()
                && !state.revision_extends(incoming_revision, &commit.revision)
            {
                continue;
            }
            if commit.parent_revision == cursor
                || state.revision_extends(&commit.parent_revision, cursor)
            {
                selected = Some(commit);
                break;
            }
        }

        // The persisted snapshot may lie between an audited endpoint and the
        // parent of the next rewrite. Prove that exceptional relation from the
        // edge's exact row-lineage transition. This scans only compact edges
        // and the relevant parent delta; it never reconstructs a full body.
        if selected.is_none() && chain.is_empty() && cursor == previous_revision {
            for index in 0..state.commit_count() {
                let edge = state
                    .edge(index)
                    .ok_or_else(|| SessionStoreError::Corrupted(previous.id().clone()))?;
                if incoming_revision != state.head()
                    && !state.revision_extends(incoming_revision, edge.revision())
                {
                    continue;
                }
                if compact_edge_parent_extends_session(state, index, previous)? {
                    selected = Some(edge.commit());
                    break;
                }
            }
        }

        let Some(commit) = selected else {
            return Ok(state
                .revision_extends(incoming_revision, cursor)
                .then_some(chain));
        };
        cursor = &commit.revision;
        chain.push(commit);
    }
}

fn compact_edge_parent_extends_session(
    state: &TranscriptHistoryState,
    edge_index: usize,
    previous: &Session,
) -> Result<bool, SessionStoreError> {
    let edge = state
        .edge(edge_index)
        .ok_or_else(|| SessionStoreError::Corrupted(previous.id().clone()))?;
    let previous_count = u64::try_from(previous.messages().len())
        .map_err(|_| SessionStoreError::Corrupted(previous.id().clone()))?;
    let base_prefix = if edge_index == 0 {
        state.anchor().row_prefix()
    } else {
        state
            .edge(edge_index - 1)
            .map(TranscriptRevisionEdge::result_witness)
            .map(|witness| witness.row_prefix())
            .ok_or_else(|| SessionStoreError::Corrupted(previous.id().clone()))?
    };
    if base_prefix.row_count() != edge.messages_before_base() as u64
        || previous_count < base_prefix.row_count()
        || previous_count > edge.messages_before() as u64
    {
        return Ok(false);
    }
    let previous_prefix = match previous.exact_message_row_prefix_at(previous_count) {
        Some(prefix) => prefix,
        None => SessionMessageRowPrefixAccumulator::from_messages(previous.messages())?,
    };
    if previous_count == base_prefix.row_count() {
        return Ok(previous_prefix == *base_prefix);
    }
    if previous_count == edge.messages_before() as u64 {
        return Ok(previous_prefix == *edge.parent_row_prefix());
    }

    let mut derived = base_prefix.clone();
    if let Some((at, replacement)) = edge.parent_advance().exact_splice() {
        let rows = replacement
            .iter()
            .map(|message| serde_json::to_vec(message).map_err(SessionStoreError::from))
            .collect::<Result<Vec<_>, _>>()?;
        let start =
            u64::try_from(at).map_err(|_| SessionStoreError::Corrupted(previous.id().clone()))?;
        let end = start
            .checked_add(
                u64::try_from(replacement.len())
                    .map_err(|_| SessionStoreError::Corrupted(previous.id().clone()))?,
            )
            .ok_or_else(|| SessionStoreError::Corrupted(previous.id().clone()))?;
        derived = derived.replace_serialized_range(start, end, &rows)?;
    }
    let appended_count = usize::try_from(previous_count - base_prefix.row_count())
        .map_err(|_| SessionStoreError::Corrupted(previous.id().clone()))?;
    let Some(appended) = edge.parent_advance().appended().get(..appended_count) else {
        return Ok(false);
    };
    let serialized = appended
        .iter()
        .map(|message| serde_json::to_vec(message).map_err(SessionStoreError::from))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(derived.extend_serialized_rows(&serialized)? == previous_prefix)
}

fn transcript_rewrite_bridge_save_guard(
    incoming: &Session,
    commit: &TranscriptRewriteCommit,
    incoming_state: &ValidatedTranscriptHistory,
    incoming_message_digest: &str,
) -> Result<(), SessionStoreError> {
    validate_transcript_rewrite_commit_bodies(incoming, commit, incoming_state)?;
    validate_live_transcript_history_head_coherence(
        incoming,
        incoming_state.state(),
        incoming_message_digest,
        "incoming",
    )?;
    if !incoming_state.contains_exact_commit(commit) {
        return Err(SessionStoreError::InvalidTranscriptRewrite {
            id: incoming.id().clone(),
            reason: format!(
                "incoming audited transcript head {} does not extend rewrite revision {}",
                incoming_state.head(),
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
    // A rewrite save adopts THIS occurrence, not merely any content-equal
    // revision retained somewhere in a valid graph. Content revisions can
    // recur (A -> B -> A), so `head == commit.revision` alone cannot identify
    // the occurrence; the ordered commit tail is the occurrence authority.
    // Without both checks, a caller could pair the live body at an older
    // rewrite with a valid graph that already contains later commits, persist
    // graph/live incoherence, and still pass the membership check below.
    if incoming_state.last_commit() != Some(commit) || incoming_state.head() != commit.revision {
        return Err(SessionStoreError::InvalidTranscriptRewrite {
            id: incoming.id().clone(),
            reason: format!(
                "incoming rewrite graph does not end at the supplied audited occurrence \
                 (graph head {}, supplied revision {}, graph tail generation {:?}, supplied generation {})",
                incoming_state.head(),
                commit.revision,
                incoming_state
                    .last_commit()
                    .map(|latest| latest.rewrite_generation),
                commit.rewrite_generation
            ),
        });
    }
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
        .commits()
        .any(|persisted| persisted == commit)
    {
        return Err(SessionStoreError::InvalidTranscriptRewrite {
            id: incoming.id().clone(),
            reason: format!(
                "incoming rewrite did not persist the rewrite commit in the transcript graph (wanted {} -> {}, graph commits: {:?})",
                commit.parent_revision,
                commit.revision,
                incoming_state
                    .commits()
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

    /// The strand created by one exact rewrite occurrence.
    ///
    /// Content revisions may recur (`A -> B -> A`), so the revision digest
    /// alone is not a durable strand identity. The specialized
    /// HeadCanonical rewrite carrier uses the graph's contiguous occurrence
    /// generation to keep every transition addressable without copying a
    /// whole revision body.
    pub fn from_rewrite_occurrence(commit: &TranscriptRewriteCommit) -> Self {
        Self(format!(
            "rewrite:{}:{}",
            commit.rewrite_generation, commit.revision
        ))
    }

    /// The exact non-prefix parent bridge preceding one rewrite occurrence.
    pub fn from_rewrite_parent_occurrence(commit: &TranscriptRewriteCommit) -> Self {
        Self(format!(
            "rewrite-parent:{}:{}",
            commit.rewrite_generation, commit.parent_revision
        ))
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

fn transcript_rewrite_prefix_is_default(prefix: &TranscriptRewritePrefixAccumulator) -> bool {
    prefix == &TranscriptRewritePrefixAccumulator::default()
}

fn transcript_rewrite_prefix_is_canonical(prefix: &TranscriptRewritePrefixAccumulator) -> bool {
    prefix
        .digest()
        .strip_prefix("sha256:")
        .is_some_and(|digest| {
            digest.len() == 64
                && digest
                    .as_bytes()
                    .iter()
                    .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        })
}

fn proved_session_rewrite_prefix_authority(
    session: &Session,
) -> Result<(Option<TranscriptRewritePrefixAccumulator>, bool), SessionStoreError> {
    let explicit = session.transcript_rewrite_prefix_authority();
    let carries_explicit = session
        .metadata()
        .contains_key(SESSION_TRANSCRIPT_REWRITE_PREFIX_AUTHORITY_KEY);
    if carries_explicit && explicit.is_none() {
        return Err(SessionStoreError::InvalidTranscriptRewrite {
            id: session.id().clone(),
            reason: "transcript rewrite-prefix authority is malformed".to_string(),
        });
    }
    let graph_authority = session
        .already_validated_transcript_history_state()
        .map_err(|error| SessionStoreError::InvalidTranscriptRewrite {
            id: session.id().clone(),
            reason: format!(
                "failed to read the already-validated transcript rewrite-prefix authority: {error}"
            ),
        })?
        .map(|history| history.rewrite_prefix().clone());
    if let (Some(explicit), Some(graph)) = (explicit.as_ref(), graph_authority.as_ref())
        && explicit != graph
    {
        return Err(SessionStoreError::InvalidTranscriptRewrite {
            id: session.id().clone(),
            reason: "session-bound rewrite-prefix authority disagrees with the validated graph"
                .to_string(),
        });
    }
    let authority = explicit.or(graph_authority);
    if let Some(prefix) = authority.as_ref()
        && !transcript_rewrite_prefix_is_canonical(prefix)
    {
        return Err(SessionStoreError::InvalidTranscriptRewrite {
            id: session.id().clone(),
            reason: "transcript rewrite-prefix authority has a non-canonical digest".to_string(),
        });
    }
    Ok((authority, carries_explicit))
}

const SESSION_MESSAGE_ROW_PREFIX_VERSION: u16 = 1;
const SESSION_MESSAGE_ROW_PREFIX_DIGEST_PREFIX: &str = "row-lineage-v1-sha256:";

fn session_message_row_prefix_empty_digest() -> [u8; 32] {
    Sha256::digest(b"meerkat.session-message-row-lineage.v1.empty\0").into()
}

fn encode_session_message_row_prefix_digest(digest: &[u8; 32]) -> String {
    let mut encoded = String::with_capacity(64);
    for byte in digest {
        std::fmt::Write::write_fmt(&mut encoded, format_args!("{byte:02x}"))
            .expect("writing to a String cannot fail");
    }
    format!("{SESSION_MESSAGE_ROW_PREFIX_DIGEST_PREFIX}{encoded}")
}

fn decode_session_message_row_prefix_digest(value: &str) -> Option<[u8; 32]> {
    let hex = value.strip_prefix(SESSION_MESSAGE_ROW_PREFIX_DIGEST_PREFIX)?;
    if hex.len() != 64
        || !hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return None;
    }
    let mut digest = [0_u8; 32];
    for (index, byte) in digest.iter_mut().enumerate() {
        let start = index * 2;
        *byte = u8::from_str_radix(&hex[start..start + 2], 16).ok()?;
    }
    Some(digest)
}

fn session_message_row_append_step(
    previous: [u8; 32],
    row_count: u64,
    row: &[u8],
) -> Result<[u8; 32], SessionStoreError> {
    let row_len = u64::try_from(row.len()).map_err(|_| {
        SessionStoreError::Serialization(
            "serialized session message row exceeds the durable u64 range".to_string(),
        )
    })?;
    let mut hasher = Sha256::new();
    hasher.update(b"meerkat.session-message-row-lineage.v1.append\0");
    hasher.update(previous);
    hasher.update(row_count.to_be_bytes());
    hasher.update(row_len.to_be_bytes());
    hasher.update(row);
    Ok(hasher.finalize().into())
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
struct SessionMessageRowPrefixAccumulatorWire {
    version: u16,
    row_count: u64,
    digest: String,
}

/// Exact occurrence-aware lineage of durable serialized message rows.
///
/// Unlike the semantic transcript digest, this accumulator binds every byte
/// in every ordered `message_json` row, including fields such as run
/// identities and timestamps that semantic conversation identity may
/// intentionally erase. It is deliberately history-dependent rather than a
/// flat content root: append and splice are separately domain-framed lineage
/// operations. That makes an arbitrary rewrite result mechanically derivable
/// from its exact parent, bounds, and replacement rows in O(delta), without
/// retaining removed rows or rescanning the unchanged document.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SessionMessageRowPrefixAccumulator {
    version: u16,
    row_count: u64,
    digest: String,
}

impl SessionMessageRowPrefixAccumulator {
    /// Empty prefix under the current row-commitment format.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            version: SESSION_MESSAGE_ROW_PREFIX_VERSION,
            row_count: 0,
            digest: encode_session_message_row_prefix_digest(
                &session_message_row_prefix_empty_digest(),
            ),
        }
    }

    /// Number of ordered durable rows committed by this accumulator.
    #[must_use]
    pub const fn row_count(&self) -> u64 {
        self.row_count
    }

    /// Stable digest string carried by the durable head.
    #[must_use]
    pub fn digest(&self) -> &str {
        &self.digest
    }

    fn validate(&self) -> Result<[u8; 32], String> {
        if self.version != SESSION_MESSAGE_ROW_PREFIX_VERSION {
            return Err(format!(
                "unsupported session message-row prefix version {}",
                self.version
            ));
        }
        decode_session_message_row_prefix_digest(&self.digest)
            .ok_or_else(|| "session message-row prefix carries a non-canonical digest".to_string())
    }

    /// Extend this exact prefix with an ordered suffix of already serialized
    /// durable row bytes.
    pub fn extend_serialized_rows(&self, rows: &[Vec<u8>]) -> Result<Self, SessionStoreError> {
        let mut digest = self.validate().map_err(|reason| {
            SessionStoreError::Serialization(format!(
                "invalid session message-row prefix: {reason}"
            ))
        })?;
        let mut row_count = self.row_count;
        for row in rows {
            digest = session_message_row_append_step(digest, row_count, row)?;
            row_count = row_count.checked_add(1).ok_or_else(|| {
                SessionStoreError::Serialization(
                    "session message-row prefix count overflow".to_string(),
                )
            })?;
        }
        Ok(Self {
            version: SESSION_MESSAGE_ROW_PREFIX_VERSION,
            row_count,
            digest: encode_session_message_row_prefix_digest(&digest),
        })
    }

    /// Derive the exact successor lineage for one typed range replacement.
    ///
    /// The unchanged bytes are already bound by `self`; only the replacement
    /// rows are serialized into this transition. `start..end` uses the parent
    /// row coordinates and may also describe insertion (`start == end`) or
    /// deletion (`rows.is_empty()`).
    pub fn replace_serialized_range(
        &self,
        start: u64,
        end: u64,
        rows: &[Vec<u8>],
    ) -> Result<Self, SessionStoreError> {
        let parent = self.validate().map_err(|reason| {
            SessionStoreError::Serialization(format!(
                "invalid session message-row prefix: {reason}"
            ))
        })?;
        if start > end || end > self.row_count {
            return Err(SessionStoreError::Serialization(
                "session message-row lineage splice is outside its parent row range".to_string(),
            ));
        }
        let replacement_count = u64::try_from(rows.len()).map_err(|_| {
            SessionStoreError::Serialization(
                "session message-row replacement count exceeds u64".to_string(),
            )
        })?;
        let removed = end - start;
        let row_count = self
            .row_count
            .checked_sub(removed)
            .and_then(|count| count.checked_add(replacement_count))
            .ok_or_else(|| {
                SessionStoreError::Serialization(
                    "session message-row lineage splice count overflow".to_string(),
                )
            })?;
        let mut hasher = Sha256::new();
        hasher.update(b"meerkat.session-message-row-lineage.v1.splice\0");
        hasher.update(parent);
        hasher.update(self.row_count.to_be_bytes());
        hasher.update(start.to_be_bytes());
        hasher.update(end.to_be_bytes());
        hasher.update(replacement_count.to_be_bytes());
        for row in rows {
            let row_len = u64::try_from(row.len()).map_err(|_| {
                SessionStoreError::Serialization(
                    "serialized session message row exceeds the durable u64 range".to_string(),
                )
            })?;
            hasher.update(row_len.to_be_bytes());
            hasher.update(row);
        }
        let digest: [u8; 32] = hasher.finalize().into();
        Ok(Self {
            version: SESSION_MESSAGE_ROW_PREFIX_VERSION,
            row_count,
            digest: encode_session_message_row_prefix_digest(&digest),
        })
    }

    /// Derive an exact commitment from an entire serialized row vector.
    pub fn from_serialized_rows(rows: &[Vec<u8>]) -> Result<Self, SessionStoreError> {
        Self::empty().extend_serialized_rows(rows)
    }

    pub(crate) fn from_messages(messages: &[Message]) -> Result<Self, SessionStoreError> {
        let rows = messages
            .iter()
            .map(|message| serde_json::to_vec(message).map_err(SessionStoreError::from))
            .collect::<Result<Vec<_>, _>>()?;
        Self::from_serialized_rows(&rows)
    }
}

impl Serialize for SessionMessageRowPrefixAccumulator {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.validate().map_err(serde::ser::Error::custom)?;
        SessionMessageRowPrefixAccumulatorWire {
            version: self.version,
            row_count: self.row_count,
            digest: self.digest.clone(),
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for SessionMessageRowPrefixAccumulator {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = SessionMessageRowPrefixAccumulatorWire::deserialize(deserializer)?;
        let accumulator = Self {
            version: wire.version,
            row_count: wire.row_count,
            digest: wire.digest,
        };
        accumulator.validate().map_err(serde::de::Error::custom)?;
        Ok(accumulator)
    }
}

/// Maximum number of rewrite occurrences retained after one settled row
/// origin before a prepared successor rotates to a new current anchor.
///
/// The store treats that rotation as the sole authority to materialize the
/// successor strand directly and retire its active overlay edge. Ordinary
/// cold resume therefore pays for the live document plus fewer than this many
/// post-anchor deltas, never the session's accumulated rewrite history.
pub const SESSION_ROW_LINEAGE_REBASE_INTERVAL: u64 = 32;

/// Constant-size cold-replay origin for an operation-lineage row commitment.
///
/// A row-lineage token produced by a splice is deliberately not a flat hash
/// of the resulting document. Cold HeadCanonical materialization must start
/// from this exact topology point and replay the durable rewrite/append
/// transitions after it; rebuilding the final token from final rows would
/// prove a different history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct SessionRowLineageAnchor {
    rewrite_count: u64,
    /// Exact rewrite-prefix authority already proved before this bounded row
    /// origin. Ordinary cold materialization starts from this accumulator and
    /// must not decode older rewrite rows again.
    #[serde(default, skip_serializing_if = "transcript_rewrite_prefix_is_default")]
    rewrite_prefix: TranscriptRewritePrefixAccumulator,
    strand: TranscriptStrandId,
    message_count: u64,
    /// Flat byte commitment to the physical rows at the bounded origin.
    ///
    /// `prefix` below is operation-lineage state and can therefore differ after
    /// a splice. This separate current-row commitment lets cold materialization
    /// verify the anchor document in O(document) without replaying how it was
    /// reached.
    materialized_prefix: SessionMessageRowPrefixAccumulator,
    /// Operation-lineage accumulator from which post-anchor deltas continue.
    prefix: SessionMessageRowPrefixAccumulator,
}

impl SessionRowLineageAnchor {
    fn current(
        rewrite_count: u64,
        rewrite_prefix: TranscriptRewritePrefixAccumulator,
        strand: TranscriptStrandId,
        materialized_prefix: SessionMessageRowPrefixAccumulator,
        prefix: SessionMessageRowPrefixAccumulator,
    ) -> Self {
        Self {
            rewrite_count,
            rewrite_prefix,
            strand,
            message_count: prefix.row_count(),
            materialized_prefix,
            prefix,
        }
    }

    fn validate_for_head(&self, head: &SessionHead) -> Result<(), SessionStoreError> {
        if self.prefix.row_count() != self.message_count
            || self.materialized_prefix.row_count() != self.message_count
            || self.rewrite_prefix.occurrence_count() != self.rewrite_count
            || self.rewrite_count > head.rewrite_count
            || (self.rewrite_count == head.rewrite_count && self.strand != head.strand)
        {
            return Err(SessionStoreError::Corrupted(head.id.clone()));
        }
        Ok(())
    }

    #[must_use]
    pub const fn rewrite_count(&self) -> u64 {
        self.rewrite_count
    }

    #[must_use]
    pub fn rewrite_prefix(&self) -> &TranscriptRewritePrefixAccumulator {
        &self.rewrite_prefix
    }

    #[must_use]
    pub fn strand(&self) -> &TranscriptStrandId {
        &self.strand
    }

    #[must_use]
    pub const fn message_count(&self) -> u64 {
        self.message_count
    }

    #[must_use]
    pub fn materialized_prefix(&self) -> &SessionMessageRowPrefixAccumulator {
        &self.materialized_prefix
    }

    #[must_use]
    pub fn prefix(&self) -> &SessionMessageRowPrefixAccumulator {
        &self.prefix
    }
}

/// Stateful verifier for the exact durable transitions after a head's
/// bounded row-lineage anchor.
///
/// The methods accept the bytes read from physical rows. `finish` is the only
/// constructor for [`VerifiedSessionRowLineageReplay`], and binds the replayed
/// topology and token to one exact current head.
#[derive(Debug)]
pub struct SessionRowLineageReplay {
    session_id: SessionId,
    rewrite_count: u64,
    strand: TranscriptStrandId,
    message_count: u64,
    prefix: SessionMessageRowPrefixAccumulator,
}

impl SessionRowLineageReplay {
    #[doc(hidden)]
    pub fn append_serialized_rows(
        &mut self,
        strand: &TranscriptStrandId,
        rows: &[Vec<u8>],
    ) -> Result<(), SessionStoreError> {
        if strand != &self.strand {
            return Err(SessionStoreError::Corrupted(self.session_id.clone()));
        }
        self.prefix = self.prefix.extend_serialized_rows(rows)?;
        self.message_count = self.prefix.row_count();
        Ok(())
    }

    #[doc(hidden)]
    pub fn replace_serialized_range(
        &mut self,
        successor_strand: TranscriptStrandId,
        start: u64,
        end: u64,
        rows: &[Vec<u8>],
    ) -> Result<(), SessionStoreError> {
        self.prefix = self.prefix.replace_serialized_range(start, end, rows)?;
        self.message_count = self.prefix.row_count();
        self.strand = successor_strand;
        Ok(())
    }

    /// Verify and apply the rewrite splice committed by one strict compact
    /// edge after the store has replayed its parent advance bytes.
    #[doc(hidden)]
    pub fn apply_rewrite_edge(
        &mut self,
        successor_strand: TranscriptStrandId,
        edge: &TranscriptRevisionEdge,
    ) -> Result<(), SessionStoreError> {
        let expected_generation = self.rewrite_count.checked_add(1).ok_or_else(|| {
            SessionStoreError::InvalidTranscriptRewrite {
                id: self.session_id.clone(),
                reason: "row-lineage replay rewrite generation overflow".to_string(),
            }
        })?;
        if edge.rewrite_generation() != expected_generation
            || edge.messages_before() as u64 != self.message_count
            || edge.parent_row_prefix() != &self.prefix
        {
            return Err(SessionStoreError::InvalidTranscriptRewrite {
                id: self.session_id.clone(),
                reason: "compact edge does not bind the replayed parent row lineage".to_string(),
            });
        }
        let (start, end) = edge.commit().selection.bounds();
        let start = u64::try_from(start)
            .map_err(|_| SessionStoreError::Corrupted(self.session_id.clone()))?;
        let end = u64::try_from(end)
            .map_err(|_| SessionStoreError::Corrupted(self.session_id.clone()))?;
        let replacement = edge
            .rewrite()
            .replacement()
            .iter()
            .map(|message| serde_json::to_vec(message).map_err(SessionStoreError::from))
            .collect::<Result<Vec<_>, _>>()?;
        let result = self
            .prefix
            .replace_serialized_range(start, end, &replacement)?;
        if result != *edge.result_witness().row_prefix()
            || result.row_count()
                != u64::try_from(edge.messages_after())
                    .map_err(|_| SessionStoreError::Corrupted(self.session_id.clone()))?
        {
            return Err(SessionStoreError::InvalidTranscriptRewrite {
                id: self.session_id.clone(),
                reason: "compact edge result witness differs from replayed exact rows".to_string(),
            });
        }
        self.prefix = result;
        self.message_count = self.prefix.row_count();
        self.rewrite_count = expected_generation;
        self.strand = successor_strand;
        Ok(())
    }

    #[doc(hidden)]
    pub fn finish(
        self,
        head: &SessionHead,
    ) -> Result<VerifiedSessionRowLineageReplay, SessionStoreError> {
        if self.session_id != head.id
            || self.rewrite_count != head.rewrite_count
            || self.strand != head.strand
            || self.message_count != head.message_count
            || head.message_row_prefix.as_ref() != Some(&self.prefix)
        {
            return Err(SessionStoreError::Corrupted(head.id.clone()));
        }
        Ok(VerifiedSessionRowLineageReplay {
            head_token: session_head_cas_token(head)?,
            prefix: self.prefix,
        })
    }
}

/// Opaque proof that exact physical transitions reproduce one current head's
/// operation-lineage row token.
#[derive(Debug)]
pub struct VerifiedSessionRowLineageReplay {
    head_token: String,
    prefix: SessionMessageRowPrefixAccumulator,
}

/// Small durable head row: the whole session EXCEPT message bodies and
/// retained revision bodies.
#[derive(Debug, Clone, Serialize, Deserialize)]
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
    /// Exact ordered commitment to the serialized durable message rows.
    ///
    /// `None` means a pre-0.8.11 head whose row identity has not yet been
    /// proved. Absence is intentionally not treated as the empty prefix:
    /// built-in stores must run the explicit full-verification conversion
    /// before this head can authorize an ordinary append.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_row_prefix: Option<SessionMessageRowPrefixAccumulator>,
    /// Exact bounded origin from which cold materialization replays row
    /// lineage transitions.
    ///
    /// `None` is accepted only as an unactivated released-0.8.10 shape. A
    /// current ordinary mutation or rewritten-head materialization requires a
    /// proved anchor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub row_lineage_anchor: Option<SessionRowLineageAnchor>,
    /// ADOPTED rewrite commits recorded for this session.
    pub rewrite_count: u64,
    /// Ordered exact rewrite-commit prefix bound into the head CAS.
    ///
    /// Heads written before this field default to the empty prefix. A
    /// non-empty `rewrite_count` paired with that default cannot enter the
    /// ordinary prepared path and must be reconciled through the full
    /// rewrite-aware lane once.
    #[serde(default, skip_serializing_if = "transcript_rewrite_prefix_is_default")]
    pub rewrite_prefix: TranscriptRewritePrefixAccumulator,
    /// Rolling identity of the exact compact transcript anchor and ordered
    /// occurrence-edge sequence.
    ///
    /// `None` is valid only when `rewrite_count == 0`. Released heads whose
    /// rewrite graph has not crossed the one-time importer cannot authorize a
    /// current mutation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub graph_prefix: Option<TranscriptGraphPrefixAccumulator>,
    /// Authenticated realtime-transcript component event prefix.
    ///
    /// `None` denotes the supported unactivated inline representation and
    /// requires the WholeBlob projection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub realtime_event_prefix: Option<ComponentEventPrefixAuthority>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    pub usage: Usage,
    /// Identity of the authenticated out-of-line HeadCanonical metadata map.
    ///
    /// `None` identifies a legacy inline head whose `metadata` map still owns
    /// every projected value. New heads carry only bounded authority overlay
    /// values below and require the exact referenced cell state to be attached
    /// before materializing a Session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata_identity: Option<SessionHeadMetadataIdentity>,
    /// Bounded metadata overlay.
    ///
    /// Current digest-addressed heads keep this empty: domain metadata lives in
    /// the authenticated map named by `metadata_identity`. Released inline
    /// heads retain their previous map until the one-time importer adopts them.
    pub metadata: serde_json::Map<String, serde_json::Value>,
    /// Sealed sparse-Merkle transition shared by the actor Session and every
    /// prepared successor. A cold-loaded head carries a verified full snapshot;
    /// an ordinary successor carries only changed cells and proofs. Neither is
    /// serialized inside the head row.
    #[serde(skip)]
    metadata_projection: Option<Arc<SessionHeadMetadataProjection>>,
}

impl PartialEq for SessionHead {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
            && self.version == other.version
            && self.strand == other.strand
            && self.head_revision == other.head_revision
            && self.message_count == other.message_count
            && self.message_row_prefix == other.message_row_prefix
            && self.row_lineage_anchor == other.row_lineage_anchor
            && self.rewrite_count == other.rewrite_count
            && self.rewrite_prefix == other.rewrite_prefix
            && self.graph_prefix == other.graph_prefix
            && self.realtime_event_prefix == other.realtime_event_prefix
            && self.created_at == other.created_at
            && self.updated_at == other.updated_at
            && self.usage == other.usage
            && self.metadata_identity == other.metadata_identity
            && self.metadata == other.metadata
    }
}

fn head_metadata_cell_carries_key(key: &str) -> bool {
    !matches!(
        key,
        SESSION_TRANSCRIPT_HISTORY_STATE_KEY
            | SESSION_TRANSCRIPT_REWRITE_PREFIX_AUTHORITY_KEY
            | crate::SESSION_REALTIME_TRANSCRIPT_STATE_KEY
    )
}

fn validate_session_head_component_roots(head: &SessionHead) -> Result<(), SessionStoreError> {
    match head.realtime_event_prefix.as_ref() {
        None => Ok(()),
        Some(realtime)
            if realtime.session_id() == &head.id
                && realtime.component() == SessionComponentKind::Realtime =>
        {
            Ok(())
        }
        Some(_) => Err(SessionStoreError::InvalidTranscriptRewrite {
            id: head.id.clone(),
            reason: "session head component root names the wrong session or component".to_string(),
        }),
    }
}

fn validate_session_head_metadata_identity(head: &SessionHead) -> Result<(), SessionStoreError> {
    let Some(identity) = head.metadata_identity.as_ref() else {
        if head.metadata_projection.is_some() {
            return Err(SessionStoreError::Corrupted(head.id.clone()));
        }
        return Ok(());
    };
    if identity.format_version() != SessionHeadMetadataIdentity::FORMAT_V1
        || !head.metadata.is_empty()
    {
        return Err(SessionStoreError::Corrupted(head.id.clone()));
    }
    if let Some(projection) = head.metadata_projection.as_ref()
        && (projection.identity() != identity
            || projection
                .mutations()
                .iter()
                .any(|mutation| !head_metadata_cell_carries_key(mutation.key())))
    {
        return Err(SessionStoreError::Corrupted(head.id.clone()));
    }
    Ok(())
}

fn session_head_has_component_roots(head: &SessionHead) -> bool {
    head.realtime_event_prefix.is_some()
}

fn validate_session_head_storage_representation(
    head: &SessionHead,
) -> Result<(), SessionStoreError> {
    validate_session_head_component_roots(head)?;
    validate_session_head_metadata_identity(head)?;
    if let Some(anchor) = head.row_lineage_anchor.as_ref() {
        anchor.validate_for_head(head)?;
    }
    match head.graph_prefix.as_ref() {
        Some(prefix) if prefix.occurrence_count() == head.rewrite_count => {}
        None if head.rewrite_count == 0 => {}
        Some(prefix) => {
            return Err(SessionStoreError::InvalidTranscriptRewrite {
                id: head.id.clone(),
                reason: format!(
                    "session head graph prefix covers {} occurrences but rewrite_count is {}",
                    prefix.occurrence_count(),
                    head.rewrite_count
                ),
            });
        }
        None => {
            return Err(SessionStoreError::InvalidTranscriptRewrite {
                id: head.id.clone(),
                reason: "rewritten current head has no compact graph-prefix authority".to_string(),
            });
        }
    }
    match (
        session_head_has_component_roots(head),
        head.metadata_identity.is_some(),
    ) {
        (false, false) | (true, true) => Ok(()),
        (true, false) => Err(SessionStoreError::InvalidTranscriptRewrite {
            id: head.id.clone(),
            reason: "activated HeadCanonical head has a component root but no immutable metadata identity; explicit legacy activation is required"
                .to_string(),
        }),
        (false, true) => Err(SessionStoreError::InvalidTranscriptRewrite {
            id: head.id.clone(),
            reason: "digest-addressed metadata identity requires the HeadCanonical realtime component root"
                .to_string(),
        }),
    }
}

/// Opaque proof that one exact serialized row vector materializes a
/// particular [`SessionHead`].
///
/// Construction performs both the byte-exact row-prefix check and the
/// semantic transcript/envelope verification. Store-owned recovery sources
/// accept this carrier so the two proofs cannot be accidentally split across
/// different reads.
#[derive(Debug, Clone)]
pub struct VerifiedSessionHeadMaterialization {
    head: SessionHead,
    session: Arc<Session>,
}

impl VerifiedSessionHeadMaterialization {
    #[must_use]
    pub fn head(&self) -> &SessionHead {
        &self.head
    }

    #[must_use]
    pub fn session(&self) -> &Arc<Session> {
        &self.session
    }

    /// Install a store-verified ancestor prefix from the same physical row
    /// snapshot as this materialization.
    #[doc(hidden)]
    pub fn with_verified_ancestor_row_prefix(
        self,
        ancestor: SessionMessageRowPrefixAccumulator,
    ) -> Result<Self, SessionStoreError> {
        let current = self.head.message_row_prefix.clone().ok_or_else(|| {
            SessionStoreError::InvalidTranscriptRewrite {
                id: self.head.id.clone(),
                reason: "verified head has no exact current row-prefix authority".to_string(),
            }
        })?;
        if !self
            .session
            .install_exact_message_row_lineage(ancestor, current)
        {
            return Err(SessionStoreError::Corrupted(self.head.id.clone()));
        }
        Ok(self)
    }

    /// Exact row lineage retained by the verified materialized Session.
    #[doc(hidden)]
    pub fn exact_row_prefix_at(
        &self,
        row_count: u64,
    ) -> Option<SessionMessageRowPrefixAccumulator> {
        self.session.exact_message_row_prefix_at(row_count)
    }
}

impl SessionHead {
    /// Begin exact cold replay at this head's bounded lineage origin.
    #[doc(hidden)]
    pub fn begin_row_lineage_replay(&self) -> Result<SessionRowLineageReplay, SessionStoreError> {
        validate_session_head_storage_representation(self)?;
        let anchor = self.row_lineage_anchor.as_ref().ok_or_else(|| {
            SessionStoreError::InvalidTranscriptRewrite {
                id: self.id.clone(),
                reason: "current head has no row-lineage replay anchor".to_string(),
            }
        })?;
        Ok(SessionRowLineageReplay {
            session_id: self.id.clone(),
            rewrite_count: anchor.rewrite_count,
            strand: anchor.strand.clone(),
            message_count: anchor.message_count,
            prefix: anchor.prefix.clone(),
        })
    }

    /// Pair this exact current physical head with a fully hydrated `Session`.
    ///
    /// The supplied session must have been materialized from the durable rows
    /// named by this head. For rooted head-canonical sessions that proof is
    /// carried by the session-local exact row-prefix lineage installed during
    /// store materialization. Re-projecting the session and comparing the
    /// complete head CAS token additionally binds metadata, the component root,
    /// rewrite authority, usage, and timestamps.
    ///
    /// This is the verification half of
    /// [`IncrementalSessionStore::materialize_head`]. Callers must not use it
    /// to bless an arbitrary in-memory session.
    #[doc(hidden)]
    pub fn verify_materialized_session(
        self,
        session: Session,
    ) -> Result<VerifiedSessionHeadMaterialization, SessionStoreError> {
        if session.id() != &self.id {
            return Err(SessionStoreError::Corrupted(self.id.clone()));
        }
        let expected_row_prefix = self.message_row_prefix.clone().ok_or_else(|| {
            SessionStoreError::InvalidTranscriptRewrite {
                id: self.id.clone(),
                reason: "current head has no exact message-row authority and cannot be explicitly materialized"
                    .to_string(),
            }
        })?;
        let actual_row_prefix = session
            .exact_message_row_prefix_at(self.message_count)
            .ok_or_else(|| SessionStoreError::InvalidTranscriptRewrite {
                id: self.id.clone(),
                reason:
                    "materialized session does not carry exact durable-row lineage for the current head"
                        .to_string(),
            })?;
        if actual_row_prefix != expected_row_prefix {
            return Err(SessionStoreError::TranscriptContinuityViolation {
                id: self.id.clone(),
                previous_revision: expected_row_prefix.digest().to_string(),
                incoming_revision: actual_row_prefix.digest().to_string(),
                reason:
                    "materialized session row-prefix authority differs from the current physical head"
                        .to_string(),
            });
        }
        let projected = Self::from_session_with_message_row_prefix(
            &session,
            self.strand.clone(),
            self.rewrite_count,
            expected_row_prefix,
            Some(self.rewrite_prefix.clone()),
            self.row_lineage_anchor.clone(),
            self.realtime_event_prefix.is_some(),
        )?;
        let expected_token = session_head_cas_token(&self)?;
        let actual_token = session_head_cas_token(&projected)?;
        if actual_token != expected_token {
            return Err(SessionStoreError::TranscriptRevisionConflict {
                id: self.id.clone(),
                expected: expected_token,
                actual: actual_token,
            });
        }

        Ok(VerifiedSessionHeadMaterialization {
            head: self,
            session: Arc::new(session),
        })
    }

    /// Project a session onto its durable head row.
    ///
    /// Strips `SESSION_TRANSCRIPT_HISTORY_STATE_KEY` from the metadata —
    /// retained history lives out-of-line in strand rows and rewrite records.
    pub fn from_session(
        session: &Session,
        strand: TranscriptStrandId,
        rewrite_count: u64,
    ) -> Result<Self, SessionStoreError> {
        let message_row_prefix =
            SessionMessageRowPrefixAccumulator::from_messages(session.messages())?;
        Self::from_session_with_message_row_prefix(
            session,
            strand,
            rewrite_count,
            message_row_prefix,
            None,
            None,
            false,
        )
    }

    /// Project a typed Session using exact storage authorities proved by the
    /// caller from durable rows.
    ///
    /// This hidden migration/recovery seam exists for a retained runtime
    /// boundary whose exact row bytes may use an older representation than
    /// reserializing the same typed Messages today. Ordinary callers must use
    /// [`PreparedHeadCanonicalMutation::prepare`].
    #[doc(hidden)]
    pub fn from_session_with_proved_storage_authority(
        session: &Session,
        strand: TranscriptStrandId,
        rewrite_prefix: TranscriptRewritePrefixAccumulator,
        message_row_prefix: SessionMessageRowPrefixAccumulator,
    ) -> Result<Self, SessionStoreError> {
        let rewrite_count = rewrite_prefix.occurrence_count();
        Self::from_session_with_message_row_prefix(
            session,
            strand,
            rewrite_count,
            message_row_prefix,
            Some(rewrite_prefix),
            None,
            true,
        )
    }

    /// Project a released inline session while preserving its unactivated
    /// storage representation. This exists only for the one-time 0.8.10
    /// conversion lane before the realtime component and metadata authorities
    /// are installed atomically.
    #[doc(hidden)]
    pub fn from_session_with_proved_inline_storage_authority(
        session: &Session,
        strand: TranscriptStrandId,
        rewrite_prefix: TranscriptRewritePrefixAccumulator,
        message_row_prefix: SessionMessageRowPrefixAccumulator,
    ) -> Result<Self, SessionStoreError> {
        let rewrite_count = rewrite_prefix.occurrence_count();
        Self::from_session_with_message_row_prefix(
            session,
            strand,
            rewrite_count,
            message_row_prefix,
            Some(rewrite_prefix),
            None,
            false,
        )
    }

    fn from_session_with_message_row_prefix(
        session: &Session,
        strand: TranscriptStrandId,
        rewrite_count: u64,
        message_row_prefix: SessionMessageRowPrefixAccumulator,
        proved_rewrite_prefix_override: Option<TranscriptRewritePrefixAccumulator>,
        preserved_row_lineage_anchor: Option<SessionRowLineageAnchor>,
        head_canonical: bool,
    ) -> Result<Self, SessionStoreError> {
        if message_row_prefix.row_count() != session.messages().len() as u64 {
            return Err(SessionStoreError::InvalidTranscriptRewrite {
                id: session.id().clone(),
                reason: format!(
                    "message-row prefix covers {} rows but the session contains {} messages",
                    message_row_prefix.row_count(),
                    session.messages().len()
                ),
            });
        }
        let head_revision = session
            .transcript_content_digest()
            .map_err(SessionStoreError::from)?;
        let (rewrite_prefix_authority, carries_explicit_rewrite_prefix) =
            proved_session_rewrite_prefix_authority(session)?;
        if let (Some(document), Some(override_prefix)) = (
            rewrite_prefix_authority.as_ref(),
            proved_rewrite_prefix_override.as_ref(),
        ) && document != override_prefix
        {
            return Err(SessionStoreError::InvalidTranscriptRewrite {
                id: session.id().clone(),
                reason: "session rewrite-prefix authority conflicts with the exact observed head"
                    .to_string(),
            });
        }
        let effective_rewrite_prefix = proved_rewrite_prefix_override
            .as_ref()
            .or(rewrite_prefix_authority.as_ref());
        let rewrite_prefix = match effective_rewrite_prefix {
            Some(prefix) => {
                let prefix_count = prefix.occurrence_count();
                if prefix_count == rewrite_count {
                    prefix.clone()
                } else if rewrite_count == 0 {
                    // The incremental rewrite installer seeds the universally
                    // known empty root before adopting the document's proved
                    // commit prefix one edge at a time. No other truncated
                    // prefix can be derived from the O(1) accumulator.
                    TranscriptRewritePrefixAccumulator::default()
                } else {
                    return Err(SessionStoreError::InvalidTranscriptRewrite {
                        id: session.id().clone(),
                        reason: format!(
                            "rewrite-prefix authority covers {prefix_count} commits but the \
                             requested head generation is {rewrite_count}"
                        ),
                    });
                }
            }
            None if rewrite_count == 0 => TranscriptRewritePrefixAccumulator::default(),
            None => {
                return Err(SessionStoreError::InvalidTranscriptRewrite {
                    id: session.id().clone(),
                    reason: format!(
                        "head generation {rewrite_count} has no proved rewrite-prefix authority"
                    ),
                });
            }
        };
        let carry_rewrite_prefix_in_metadata = carries_explicit_rewrite_prefix
            && rewrite_prefix_authority.as_ref() == Some(&rewrite_prefix);
        let graph_prefix = session
            .validated_transcript_history_state()
            .map_err(|error| SessionStoreError::InvalidTranscriptRewrite {
                id: session.id().clone(),
                reason: format!("failed to obtain current compact graph authority: {error}"),
            })?
            .map(|history| history.graph_prefix().clone());
        match graph_prefix.as_ref() {
            Some(prefix) if prefix.occurrence_count() == rewrite_count => {}
            None if rewrite_count == 0 => {}
            Some(prefix) => {
                return Err(SessionStoreError::InvalidTranscriptRewrite {
                    id: session.id().clone(),
                    reason: format!(
                        "session compact graph covers {} occurrences but requested head generation is {rewrite_count}",
                        prefix.occurrence_count()
                    ),
                });
            }
            None => {
                return Err(SessionStoreError::InvalidTranscriptRewrite {
                    id: session.id().clone(),
                    reason: "rewritten session has no validated compact graph authority"
                        .to_string(),
                });
            }
        }
        let realtime_event_prefix = head_canonical
            .then(|| session_realtime_component_root(session))
            .transpose()?;
        let (mut metadata, metadata_identity, metadata_projection) = if head_canonical {
            let projection = session
                .head_canonical_metadata_projection()
                .map_err(SessionStoreError::from)?;
            (
                serde_json::Map::new(),
                Some(projection.identity().clone()),
                Some(projection),
            )
        } else {
            // Legacy inline heads preserve their exact wire contract.
            let metadata = session
                .metadata()
                .iter()
                .filter(|(key, _)| {
                    key.as_str() != SESSION_TRANSCRIPT_HISTORY_STATE_KEY
                        && (key.as_str() != SESSION_TRANSCRIPT_REWRITE_PREFIX_AUTHORITY_KEY
                            || carry_rewrite_prefix_in_metadata)
                })
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect();
            (metadata, None, None)
        };
        if !head_canonical {
            session
                .inject_realtime_whole_blob_projection(&mut metadata)
                .map_err(SessionStoreError::from)?;
        }
        let row_lineage_anchor = match preserved_row_lineage_anchor {
            Some(anchor) => anchor,
            None => SessionRowLineageAnchor::current(
                rewrite_count,
                rewrite_prefix.clone(),
                strand.clone(),
                SessionMessageRowPrefixAccumulator::from_messages(session.messages())?,
                message_row_prefix.clone(),
            ),
        };
        let head = Self {
            id: session.id().clone(),
            version: session.version(),
            strand,
            head_revision,
            message_count: session.messages().len() as u64,
            message_row_prefix: Some(message_row_prefix),
            row_lineage_anchor: Some(row_lineage_anchor),
            rewrite_count,
            rewrite_prefix,
            graph_prefix,
            realtime_event_prefix,
            created_at: session.created_at(),
            updated_at: session.updated_at(),
            usage: session.total_usage(),
            metadata_identity,
            metadata,
            metadata_projection,
        };
        validate_session_head_storage_representation(&head)?;
        Ok(head)
    }

    #[must_use]
    pub fn metadata_identity(&self) -> Option<&SessionHeadMetadataIdentity> {
        self.metadata_identity.as_ref()
    }

    #[must_use]
    pub fn metadata_projection(&self) -> Option<&Arc<SessionHeadMetadataProjection>> {
        self.metadata_projection.as_ref()
    }

    #[must_use]
    /// Materialize the complete Session metadata map for an explicit cold
    /// read/summary boundary.
    ///
    /// Ordinary CAS and runtime-authority comparisons must use
    /// [`Self::metadata_identity`] instead; this method necessarily copies the
    /// out-of-line values.
    pub fn materialized_metadata(
        &self,
    ) -> Result<serde_json::Map<String, serde_json::Value>, SessionStoreError> {
        match (&self.metadata_identity, &self.metadata_projection) {
            (None, None) => Ok(self.metadata.clone()),
            (Some(expected), Some(projection)) if projection.identity() == expected => {
                let mut values = projection
                    .materialized_values()
                    .map_err(|_| SessionStoreError::Corrupted(self.id.clone()))?;
                for (key, value) in &self.metadata {
                    if values.insert(key.clone(), value.clone()).is_some() {
                        return Err(SessionStoreError::Corrupted(self.id.clone()));
                    }
                }
                Ok(values)
            }
            _ => Err(SessionStoreError::Corrupted(self.id.clone())),
        }
    }

    /// Attach and verify the complete authenticated metadata snapshot named by
    /// this compact head. Stores build the projection from exact immutable cell
    /// rows and call this only at an explicit materialization boundary.
    pub fn attach_metadata_projection(
        &mut self,
        projection: Arc<SessionHeadMetadataProjection>,
    ) -> Result<(), SessionStoreError> {
        let expected = self.metadata_identity.clone().ok_or_else(|| {
            SessionStoreError::InvalidTranscriptRewrite {
                id: self.id.clone(),
                reason: "legacy inline head has no metadata digest to attach".to_string(),
            }
        })?;
        if !self.metadata.is_empty() {
            return Err(SessionStoreError::Corrupted(self.id.clone()));
        }
        if projection.identity() != &expected
            || !projection.is_full_snapshot()
            || projection
                .mutations()
                .iter()
                .any(|mutation| !head_metadata_cell_carries_key(mutation.key()))
        {
            return Err(SessionStoreError::Corrupted(self.id.clone()));
        }
        let values = projection
            .materialized_values()
            .map_err(|_| SessionStoreError::Corrupted(self.id.clone()))?;
        if values.keys().any(|key| self.metadata.contains_key(key)) {
            return Err(SessionStoreError::Corrupted(self.id.clone()));
        }
        self.metadata_projection = Some(projection);
        Ok(())
    }

    /// Compare one live Session with the exact metadata identity of this head
    /// without walking either accumulated metadata map on the ordinary path.
    pub fn matches_session_metadata(&self, session: &Session) -> Result<bool, SessionStoreError> {
        if session.id() != &self.id {
            return Ok(false);
        }
        let Some(expected_identity) = self.metadata_identity.as_ref() else {
            return Ok(session.metadata() == &self.metadata);
        };
        let projection = session
            .head_canonical_metadata_projection()
            .map_err(SessionStoreError::from)?;
        if projection.identity() != expected_identity {
            return Ok(false);
        }
        Ok(self.metadata.is_empty())
    }

    /// Rebuild a slim `Session` (no transcript-history metadata) from this
    /// head plus its strand messages.
    ///
    /// Fails closed `Corrupted` if `digest(messages) != head_revision` or
    /// `messages.len() != message_count`. The envelope version is restored
    /// through the generated persistence version authority, exactly like
    /// `Session::deserialize`.
    pub fn into_session(self, messages: Vec<Message>) -> Result<Session, SessionStoreError> {
        let serialized_rows = messages
            .iter()
            .map(|message| serde_json::to_vec(message).map_err(SessionStoreError::from))
            .collect::<Result<Vec<_>, _>>()?;
        self.into_session_with_serialized_rows(messages, &serialized_rows, None, None)
    }

    /// Rebuild a slim session directly from the exact durable message-row
    /// bytes.
    ///
    /// This is the store-facing materialization seam. It verifies the
    /// byte-exact prefix commitment before decoding any row into the semantic
    /// message model, so representation fields erased by the transcript
    /// digest remain protected.
    #[doc(hidden)]
    pub fn into_session_from_serialized_rows(
        self,
        serialized_rows: Vec<Vec<u8>>,
    ) -> Result<Session, SessionStoreError> {
        let messages = serialized_rows
            .iter()
            .map(|bytes| {
                serde_json::from_slice::<Message>(bytes)
                    .map_err(|_| SessionStoreError::Corrupted(self.id.clone()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        self.into_session_with_serialized_rows(messages, &serialized_rows, None, None)
    }

    /// Verify and retain one exact serialized materialization as an opaque
    /// store-owned proof.
    #[doc(hidden)]
    pub fn verify_serialized_rows(
        self,
        serialized_rows: Vec<Vec<u8>>,
    ) -> Result<VerifiedSessionHeadMaterialization, SessionStoreError> {
        let head = self.clone();
        let session = Arc::new(self.into_session_from_serialized_rows(serialized_rows)?);
        Ok(VerifiedSessionHeadMaterialization { head, session })
    }

    /// Verify exact message rows together with the authenticated realtime
    /// component event log and install its reducer.
    #[doc(hidden)]
    pub fn verify_serialized_rows_with_component_sequences(
        self,
        serialized_rows: Vec<Vec<u8>>,
        realtime_sequence: VerifiedComponentEventSequence,
    ) -> Result<VerifiedSessionHeadMaterialization, SessionStoreError> {
        let messages = serialized_rows
            .iter()
            .map(|bytes| {
                serde_json::from_slice::<Message>(bytes)
                    .map_err(|_| SessionStoreError::Corrupted(self.id.clone()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let head = self.clone();
        let session = Arc::new(self.into_session_with_serialized_rows(
            messages,
            &serialized_rows,
            Some(&realtime_sequence),
            None,
        )?);
        Ok(VerifiedSessionHeadMaterialization { head, session })
    }

    /// Materialize a rewritten head only after exact physical row-lineage
    /// transitions have been replayed from its bounded anchor.
    #[doc(hidden)]
    pub fn verify_serialized_rows_with_component_sequences_and_lineage(
        self,
        serialized_rows: Vec<Vec<u8>>,
        realtime_sequence: VerifiedComponentEventSequence,
        lineage: VerifiedSessionRowLineageReplay,
    ) -> Result<VerifiedSessionHeadMaterialization, SessionStoreError> {
        let messages = serialized_rows
            .iter()
            .map(|bytes| {
                serde_json::from_slice::<Message>(bytes)
                    .map_err(|_| SessionStoreError::Corrupted(self.id.clone()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let head = self.clone();
        let session = Arc::new(self.into_session_with_serialized_rows(
            messages,
            &serialized_rows,
            Some(&realtime_sequence),
            Some(&lineage),
        )?);
        Ok(VerifiedSessionHeadMaterialization { head, session })
    }

    fn into_session_with_serialized_rows(
        self,
        messages: Vec<Message>,
        serialized_rows: &[Vec<u8>],
        component_sequence: Option<&VerifiedComponentEventSequence>,
        lineage: Option<&VerifiedSessionRowLineageReplay>,
    ) -> Result<Session, SessionStoreError> {
        validate_session_head_component_roots(&self)?;
        if messages.len() as u64 != self.message_count {
            return Err(SessionStoreError::Corrupted(self.id.clone()));
        }
        if serialized_rows.len() != messages.len() {
            return Err(SessionStoreError::Corrupted(self.id.clone()));
        }
        let verified_row_prefix = self.message_row_prefix.clone();
        if let Some(expected) = verified_row_prefix.as_ref() {
            let actual = match lineage {
                Some(proof)
                    if proof.head_token == session_head_cas_token(&self)?
                        && &proof.prefix == expected =>
                {
                    proof.prefix.clone()
                }
                Some(_) => return Err(SessionStoreError::Corrupted(self.id.clone())),
                None => {
                    if let Some(anchor) = self.row_lineage_anchor.as_ref()
                        && (anchor.rewrite_count != self.rewrite_count
                            || anchor.strand != self.strand
                            || anchor.message_count != self.message_count
                            || &anchor.prefix != expected)
                    {
                        return Err(SessionStoreError::InvalidTranscriptRewrite {
                            id: self.id.clone(),
                            reason:
                                "history-dependent row lineage requires bounded exact cold replay"
                                    .to_string(),
                        });
                    }
                    SessionMessageRowPrefixAccumulator::from_serialized_rows(serialized_rows)?
                }
            };
            if actual != *expected || actual.row_count() != self.message_count {
                return Err(SessionStoreError::Corrupted(self.id.clone()));
            }
        }
        // The head revision IS the transcript content digest; verify it on
        // EVERY row-assembled materialization. Process-global substitution or
        // verification memos must not replace this durable-byte proof.
        let component_root = self.realtime_event_prefix.clone();
        let SessionHead {
            id,
            version,
            head_revision,
            created_at,
            updated_at,
            usage,
            metadata_identity,
            mut metadata,
            metadata_projection,
            message_row_prefix,
            ..
        } = self;
        let installed_metadata_projection = match (metadata_identity, metadata_projection) {
            (Some(expected), Some(projection)) if projection.identity() == &expected => {
                let mut values = projection
                    .materialized_values()
                    .map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
                for (key, value) in metadata {
                    if values.insert(key, value).is_some() {
                        return Err(SessionStoreError::Corrupted(id.clone()));
                    }
                }
                metadata = values;
                Some(projection)
            }
            (Some(_), _) => return Err(SessionStoreError::Corrupted(id.clone())),
            (None, None) => None,
            (None, Some(_)) => return Err(SessionStoreError::Corrupted(id.clone())),
        };
        let mut session = Session::from_head_parts(
            version,
            id.clone(),
            messages,
            message_row_prefix,
            created_at,
            updated_at,
            metadata,
            usage,
            installed_metadata_projection,
        )
        .map_err(|err| {
            SessionStoreError::Serialization(format!(
                "failed to restore session from head row: {err}"
            ))
        })?;
        match (component_root, component_sequence) {
            (None, None) => {}
            (Some(realtime_root), Some(realtime_sequence))
                if realtime_sequence.successor() == &realtime_root =>
            {
                session
                    .install_verified_realtime_component_sequence(realtime_sequence)
                    .map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
            }
            _ => return Err(SessionStoreError::Corrupted(id.clone())),
        }
        session
            .normalize_persisted_transcript_history_ingress()
            .map_err(|error| SessionStoreError::InvalidTranscriptRewrite {
                id: id.clone(),
                reason: format!(
                    "persisted transcript-history ingress normalization failed: {error}"
                ),
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
        Ok(session)
    }
}

/// Stable compare token for a persisted session head row (mirror of
/// [`session_projection_cas_token`] for the incremental contract).
#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
struct DigestAddressedSessionHeadCas<'a> {
    format_version: u16,
    id: &'a SessionId,
    version: u32,
    strand: &'a TranscriptStrandId,
    head_revision: &'a str,
    message_count: u64,
    message_row_prefix: &'a Option<SessionMessageRowPrefixAccumulator>,
    row_lineage_anchor: &'a Option<SessionRowLineageAnchor>,
    rewrite_count: u64,
    rewrite_prefix: &'a TranscriptRewritePrefixAccumulator,
    graph_prefix: &'a Option<TranscriptGraphPrefixAccumulator>,
    realtime_event_prefix: &'a Option<ComponentEventPrefixAuthority>,
    created_at: &'a SystemTime,
    updated_at: &'a SystemTime,
    usage: &'a Usage,
    metadata_identity: &'a SessionHeadMetadataIdentity,
}

pub fn session_head_cas_token(head: &SessionHead) -> Result<String, SessionStoreError> {
    validate_session_head_storage_representation(head)?;
    if let Some(prefix) = head.message_row_prefix.as_ref()
        && prefix.row_count() != head.message_count
    {
        return Err(SessionStoreError::InvalidTranscriptRewrite {
            id: head.id.clone(),
            reason: format!(
                "session head message-row prefix covers {} rows but message_count is {}",
                prefix.row_count(),
                head.message_count
            ),
        });
    }
    let (prefix, bytes) = match head.metadata_identity.as_ref() {
        Some(metadata_identity) => {
            if !head.metadata.is_empty() {
                return Err(SessionStoreError::Corrupted(head.id.clone()));
            }
            let preimage = DigestAddressedSessionHeadCas {
                format_version: 5,
                id: &head.id,
                version: head.version,
                strand: &head.strand,
                head_revision: &head.head_revision,
                message_count: head.message_count,
                message_row_prefix: &head.message_row_prefix,
                row_lineage_anchor: &head.row_lineage_anchor,
                rewrite_count: head.rewrite_count,
                rewrite_prefix: &head.rewrite_prefix,
                graph_prefix: &head.graph_prefix,
                realtime_event_prefix: &head.realtime_event_prefix,
                created_at: &head.created_at,
                updated_at: &head.updated_at,
                usage: &head.usage,
                metadata_identity,
            };
            (
                "head-v5-sha256:",
                serde_json::to_vec(&preimage).map_err(SessionStoreError::from)?,
            )
        }
        None => (
            "head-sha256:",
            serde_json::to_vec(head).map_err(SessionStoreError::from)?,
        ),
    };
    Ok(format!("{prefix}{:x}", Sha256::digest(bytes)))
}

/// CAS expectation for incremental head writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionHeadCas {
    /// No head row may exist yet.
    Create,
    /// The stored row's token must equal this.
    IfToken(String),
}

/// Sealed ordinary create/append mutation for a head-canonical session.
///
/// This carrier is the only public path from a typed [`Session`] to the
/// mechanical rows an incremental backend may install for an ordinary
/// boundary. Its private fields keep the predecessor proof, successor head,
/// exact successor head, and serialized suffix paired as one value.
/// There is deliberately no constructor from raw parts and no serde contract.
#[derive(Debug, Clone)]
pub struct PreparedHeadCanonicalMutation {
    predecessor_head: Option<SessionHead>,
    expected_cas: SessionHeadCas,
    successor_head: SessionHead,
    successor_head_token: String,
    metadata_projection: Arc<SessionHeadMetadataProjection>,
    base_seq: u64,
    serialized_suffix: Vec<Vec<u8>>,
    realtime_suffix: Option<PreparedComponentEventSuffix>,
}

impl PreparedHeadCanonicalMutation {
    /// Prepare a generation-zero HeadCanonical create from domain state.
    pub fn prepare_root(session: &Session) -> Result<Self, SessionStoreError> {
        Self::prepare_current(session, None)
    }

    /// Prepare an ordinary canonical-head create or same-strand append.
    ///
    /// `observed_head` is the exact head observation the caller read. An
    /// absent observation prepares a rewrite-free root create. A present
    /// observation prepares an append on that same strand and preserves its
    /// rewrite generation. The predecessor transcript prefix is verified
    /// before any suffix bytes are admitted.
    ///
    pub fn prepare(
        session: &Session,
        observed_head: Option<SessionHead>,
    ) -> Result<Self, SessionStoreError> {
        Self::prepare_current(session, observed_head)
    }

    /// Prepare an in-run physical projection against store-issued committed and
    /// observed heads. Runtime owns the provisional run identity separately.
    pub fn prepare_intra_turn(
        session: &Session,
        runtime_boundary_head: &SessionHead,
        observed_head: SessionHead,
    ) -> Result<Self, SessionStoreError> {
        validate_store_issued_head_pair(session, runtime_boundary_head, &observed_head)?;
        Self::prepare_current(session, Some(observed_head))
    }

    fn prepare_current(
        session: &Session,
        observed_head: Option<SessionHead>,
    ) -> Result<Self, SessionStoreError> {
        let id = session.id().clone();
        let realtime_suffix =
            session
                .prepare_realtime_component_event_suffix()
                .map_err(|error| SessionStoreError::InvalidTranscriptRewrite {
                    id: session.id().clone(),
                    reason: format!("failed to prepare realtime component suffix: {error}"),
                })?;
        let acknowledged_realtime = session
            .realtime_component_event_acknowledged_prefix()
            .clone();
        let successor_realtime = session.realtime_component_event_prefix().map_err(|error| {
            SessionStoreError::InvalidTranscriptRewrite {
                id: session.id().clone(),
                reason: format!("failed to derive realtime successor root: {error}"),
            }
        })?;

        let (strand, rewrite_count, base_seq, expected_cas) = if let Some(head) =
            observed_head.as_ref()
        {
            validate_session_head_component_roots(head)?;
            if &head.id != session.id() {
                return Err(SessionStoreError::InvalidTranscriptRewrite {
                    id,
                    reason: format!(
                        "observed head belongs to session {}, not {}",
                        head.id,
                        session.id()
                    ),
                });
            }
            if head.realtime_event_prefix.as_ref() != Some(&acknowledged_realtime) {
                return Err(SessionStoreError::TranscriptContinuityViolation {
                    id: session.id().clone(),
                    previous_revision: "observed-component-roots".to_string(),
                    incoming_revision: "tracker-acknowledged-component-roots".to_string(),
                    reason:
                        "incoming realtime component tracker does not extend the exact observed head root"
                            .to_string(),
                });
            }
            let Some(message_row_prefix) = head.message_row_prefix.as_ref() else {
                return Err(SessionStoreError::InvalidTranscriptRewrite {
                    id,
                    reason: "observed head predates exact message-row authority; run the explicit head-canonical conversion before ordinary append"
                        .to_string(),
                });
            };
            if message_row_prefix.row_count() != head.message_count {
                return Err(SessionStoreError::Corrupted(session.id().clone()));
            }
            let session_prefix = session
                .exact_message_row_prefix_at(head.message_count)
                .ok_or_else(|| SessionStoreError::InvalidTranscriptRewrite {
                    id: session.id().clone(),
                    reason: "incoming session has no exact durable-row lineage for the observed head; rematerialize it from the canonical rows before append"
                        .to_string(),
                })?;
            if &session_prefix != message_row_prefix {
                return Err(SessionStoreError::TranscriptContinuityViolation {
                    id: session.id().clone(),
                    previous_revision: message_row_prefix.digest().to_string(),
                    incoming_revision: session_prefix.digest().to_string(),
                    reason: "incoming session's exact serialized row prefix differs from the observed canonical head"
                        .to_string(),
                });
            }
            let base = usize::try_from(head.message_count)
                .map_err(|_| SessionStoreError::Corrupted(session.id().clone()))?;
            if session.messages().len() < base {
                return Err(SessionStoreError::MonotonicityViolation {
                    id,
                    prev_len: base,
                    new_len: session.messages().len(),
                });
            }
            let actual_prefix = session
                .transcript_prefix_digest(base)
                .map_err(|error| SessionStoreError::Serialization(error.to_string()))?;
            if actual_prefix != head.head_revision {
                let incoming_revision = session
                    .transcript_content_digest()
                    .map_err(|error| SessionStoreError::Serialization(error.to_string()))?;
                return Err(SessionStoreError::TranscriptContinuityViolation {
                    id,
                    previous_revision: head.head_revision.clone(),
                    incoming_revision,
                    reason: format!(
                        "incoming transcript prefix at observed message count {} does not match the observed head",
                        head.message_count
                    ),
                });
            }
            (
                head.strand.clone(),
                head.rewrite_count,
                head.message_count,
                SessionHeadCas::IfToken(session_head_cas_token(head)?),
            )
        } else {
            if acknowledged_realtime.event_count() != 0 {
                return Err(SessionStoreError::InvalidTranscriptRewrite {
                    id: session.id().clone(),
                    reason:
                        "head-canonical create has a non-empty acknowledged realtime component predecessor"
                            .to_string(),
                });
            }
            (TranscriptStrandId::root(), 0, 0, SessionHeadCas::Create)
        };

        let suffix_start = usize::try_from(base_seq)
            .map_err(|_| SessionStoreError::Corrupted(session.id().clone()))?;
        let serialized_suffix = session.messages()[suffix_start..]
            .iter()
            .map(|message| serde_json::to_vec(message).map_err(SessionStoreError::from))
            .collect::<Result<Vec<_>, _>>()?;
        let successor_message_row_prefix = match observed_head.as_ref() {
            Some(predecessor) => predecessor
                .message_row_prefix
                .as_ref()
                .ok_or_else(|| SessionStoreError::InvalidTranscriptRewrite {
                    id: session.id().clone(),
                    reason: "observed head has no exact message-row prefix authority".to_string(),
                })?
                .extend_serialized_rows(&serialized_suffix)?,
            None => SessionMessageRowPrefixAccumulator::empty()
                .extend_serialized_rows(&serialized_suffix)?,
        };
        if successor_message_row_prefix.row_count() != session.messages().len() as u64 {
            return Err(SessionStoreError::InvalidTranscriptRewrite {
                id: session.id().clone(),
                reason: "successor exact row prefix does not cover the typed session".to_string(),
            });
        }
        let successor_head = SessionHead::from_session_with_message_row_prefix(
            session,
            strand,
            rewrite_count,
            successor_message_row_prefix,
            observed_head
                .as_ref()
                .map(|head| head.rewrite_prefix.clone()),
            observed_head
                .as_ref()
                .and_then(|head| head.row_lineage_anchor.clone()),
            true,
        )?;
        if successor_head.realtime_event_prefix.as_ref() != Some(&successor_realtime) {
            return Err(SessionStoreError::InvalidTranscriptRewrite {
                id: session.id().clone(),
                reason: "projected successor head does not bind the exact prepared realtime component root"
                    .to_string(),
            });
        }
        match realtime_suffix.as_ref() {
            Some(suffix)
                if suffix.predecessor() == &acknowledged_realtime
                    && suffix.successor() == &successor_realtime => {}
            None if acknowledged_realtime == successor_realtime => {}
            _ => {
                return Err(SessionStoreError::InvalidTranscriptRewrite {
                    id: session.id().clone(),
                    reason: "realtime suffix does not bridge the acknowledged and successor roots"
                        .to_string(),
                });
            }
        }
        let incoming_rewrite_prefix = proved_session_rewrite_prefix_authority(session)?
            .0
            .or_else(|| {
                observed_head
                    .as_ref()
                    .map(|head| head.rewrite_prefix.clone())
            })
            .unwrap_or_default();
        let prefix_count = incoming_rewrite_prefix.occurrence_count();
        if prefix_count != rewrite_count {
            return Err(SessionStoreError::InvalidTranscriptRewrite {
                id: session.id().clone(),
                reason: format!(
                    "ordinary head-canonical mutation carries rewrite-prefix authority for \
                     {prefix_count} commits but the observed head generation is {rewrite_count}"
                ),
            });
        }
        if successor_head.rewrite_prefix != incoming_rewrite_prefix {
            return Err(SessionStoreError::InvalidTranscriptRewrite {
                id: session.id().clone(),
                reason: "ordinary head-canonical mutation did not preserve the session's exact rewrite-prefix authority"
                    .to_string(),
            });
        }
        match observed_head.as_ref() {
            Some(predecessor) => {
                if successor_head.rewrite_prefix != predecessor.rewrite_prefix {
                    return Err(SessionStoreError::InvalidTranscriptRewrite {
                        id: session.id().clone(),
                        reason: "ordinary head-canonical append changed the durable rewrite-prefix authority"
                            .to_string(),
                    });
                }
                if successor_head.graph_prefix != predecessor.graph_prefix {
                    return Err(SessionStoreError::InvalidTranscriptRewrite {
                        id: session.id().clone(),
                        reason: "ordinary head-canonical append changed the compact graph-prefix authority"
                            .to_string(),
                    });
                }
            }
            None => {
                if successor_head.graph_prefix.is_some() {
                    return Err(SessionStoreError::InvalidTranscriptRewrite {
                        id: session.id().clone(),
                        reason: "ordinary head-canonical create cannot introduce a retained transcript graph"
                            .to_string(),
                    });
                }
            }
        }
        let suffix_len = u64::try_from(serialized_suffix.len()).map_err(|_| {
            SessionStoreError::Internal(format!(
                "session {} ordinary suffix exceeds the durable u64 row range",
                session.id()
            ))
        })?;
        if base_seq.checked_add(suffix_len) != Some(successor_head.message_count) {
            return Err(SessionStoreError::InvalidTranscriptRewrite {
                id: session.id().clone(),
                reason: format!(
                    "prepared suffix at base {base_seq} with {suffix_len} rows does not reach successor count {}",
                    successor_head.message_count
                ),
            });
        }
        let metadata_projection = successor_head
            .metadata_projection
            .as_ref()
            .cloned()
            .ok_or_else(|| SessionStoreError::InvalidTranscriptRewrite {
                id: session.id().clone(),
                reason: "prepared HeadCanonical successor has no sealed metadata transition"
                    .to_string(),
            })?;
        match observed_head.as_ref() {
            Some(predecessor)
                if metadata_projection.predecessor_identity()
                    == predecessor.metadata_identity.as_ref() => {}
            None if metadata_projection.predecessor_identity().is_none()
                && metadata_projection.is_full_snapshot() => {}
            Some(_) => {
                return Err(SessionStoreError::InvalidTranscriptRewrite {
                    id: session.id().clone(),
                    reason:
                        "prepared metadata transition does not extend the observed head identity"
                            .to_string(),
                });
            }
            None => {
                return Err(SessionStoreError::InvalidTranscriptRewrite {
                    id: session.id().clone(),
                    reason:
                        "HeadCanonical root requires one complete metadata snapshot from the empty map"
                            .to_string(),
                });
            }
        }
        let successor_head_token = session_head_cas_token(&successor_head)?;

        Ok(Self {
            predecessor_head: observed_head,
            expected_cas,
            successor_head,
            successor_head_token,
            metadata_projection,
            base_seq,
            serialized_suffix,
            realtime_suffix,
        })
    }

    /// Session whose canonical head this mutation advances.
    #[must_use]
    pub fn session_id(&self) -> &SessionId {
        &self.successor_head.id
    }

    /// Exact predecessor head observed during preparation, if this is append.
    #[must_use]
    pub fn predecessor_head(&self) -> Option<&SessionHead> {
        self.predecessor_head.as_ref()
    }

    /// Compare-and-swap expectation derived from the exact predecessor head.
    #[must_use]
    pub fn expected_cas(&self) -> &SessionHeadCas {
        &self.expected_cas
    }

    /// Predecessor head token for append, or `None` for create.
    #[must_use]
    pub fn predecessor_head_token(&self) -> Option<&str> {
        match &self.expected_cas {
            SessionHeadCas::Create => None,
            SessionHeadCas::IfToken(token) => Some(token),
        }
    }

    /// Fully derived successor head.
    #[must_use]
    pub fn successor_head(&self) -> &SessionHead {
        &self.successor_head
    }

    /// Stable token for [`Self::successor_head`].
    #[must_use]
    pub fn successor_head_token(&self) -> &str {
        &self.successor_head_token
    }

    /// Authenticated per-key metadata transition sealed with this successor.
    #[must_use]
    pub fn metadata_projection(&self) -> &Arc<SessionHeadMetadataProjection> {
        &self.metadata_projection
    }

    /// Unchanged live strand on which the ordinary suffix is appended.
    #[must_use]
    pub fn strand(&self) -> &TranscriptStrandId {
        &self.successor_head.strand
    }

    /// First durable sequence represented by [`Self::serialized_suffix`].
    #[must_use]
    pub const fn base_seq(&self) -> u64 {
        self.base_seq
    }

    /// Message bytes serialized directly from the typed successor's suffix.
    #[must_use]
    pub fn serialized_suffix(&self) -> &[Vec<u8>] {
        &self.serialized_suffix
    }

    /// Sealed realtime component suffix, when this boundary changes it.
    #[must_use]
    pub fn realtime_suffix(&self) -> Option<&PreparedComponentEventSuffix> {
        self.realtime_suffix.as_ref()
    }

    pub(crate) fn validate_live_successor(
        &self,
        session: &Session,
    ) -> Result<(), SessionStoreError> {
        let invalid = |reason: String| SessionStoreError::InvalidTranscriptRewrite {
            id: self.successor_head.id.clone(),
            reason,
        };
        if session.id() != &self.successor_head.id
            || session.version() != self.successor_head.version
            || session.messages().len() as u64 != self.successor_head.message_count
            || session.created_at() != self.successor_head.created_at
            || session.updated_at() != self.successor_head.updated_at
            || session.total_usage() != self.successor_head.usage
        {
            return Err(invalid(
                "live Session envelope changed after prepared successor was sealed".to_string(),
            ));
        }
        let live_revision = session
            .transcript_content_digest()
            .map_err(SessionStoreError::from)?;
        if live_revision != self.successor_head.head_revision {
            return Err(invalid(
                "live transcript changed after prepared successor was sealed".to_string(),
            ));
        }
        let live_graph_prefix = session
            .validated_transcript_history_state()
            .map_err(|error| invalid(format!("live compact graph is invalid: {error}")))?
            .map(|history| history.graph_prefix().clone());
        if live_graph_prefix != self.successor_head.graph_prefix {
            return Err(invalid(
                "live compact graph changed after prepared successor was sealed".to_string(),
            ));
        }
        let metadata_projection = session
            .head_canonical_metadata_projection()
            .map_err(SessionStoreError::from)?;
        if self.successor_head.metadata_identity.as_ref() != Some(metadata_projection.identity())
            || metadata_projection.as_ref() != self.metadata_projection.as_ref()
        {
            return Err(invalid(
                "live metadata transition changed after prepared successor was sealed".to_string(),
            ));
        }
        let realtime = session
            .realtime_component_event_prefix()
            .map_err(|error| invalid(format!("live realtime root is invalid: {error}")))?;
        if self.successor_head.realtime_event_prefix.as_ref() != Some(&realtime) {
            return Err(invalid(
                "live realtime component root changed after prepared successor was sealed"
                    .to_string(),
            ));
        }
        Ok(())
    }

    /// Acknowledge this exact durable successor on the actor-owned Session.
    ///
    /// All fallible identity/content checks run before authority is installed.
    /// Once they pass, continuation-state installation and exact row-prefix
    /// adoption are the paired in-memory realization of the one CAS-acknowledged
    /// successor. Callers must not install either raw authority separately.
    pub fn acknowledge_session(
        &self,
        session: &mut Session,
        committed_head_token: &str,
    ) -> Result<(), SessionStoreError> {
        self.acknowledge_projection(session, committed_head_token)
    }

    /// Acknowledge only the physical continuation authorities of an exact
    /// durable intra-turn successor.
    ///
    /// RuntimeStore retains the run-scoped provisional authority. This only
    /// advances actor-local row, component, and metadata continuation state.
    pub fn acknowledge_physical_projection(
        &self,
        session: &mut Session,
        committed_head_token: &str,
    ) -> Result<(), SessionStoreError> {
        self.acknowledge_projection(session, committed_head_token)
    }

    fn acknowledge_projection(
        &self,
        session: &mut Session,
        committed_head_token: &str,
    ) -> Result<(), SessionStoreError> {
        self.validate_live_successor(session)?;
        if committed_head_token != self.successor_head_token {
            return Err(SessionStoreError::TranscriptRevisionConflict {
                id: self.successor_head.id.clone(),
                expected: self.successor_head_token.clone(),
                actual: committed_head_token.to_string(),
            });
        }
        if session.id() != &self.successor_head.id {
            return Err(SessionStoreError::InvalidTranscriptRewrite {
                id: session.id().clone(),
                reason: format!(
                    "acknowledged successor belongs to session {}, not {}",
                    self.successor_head.id,
                    session.id()
                ),
            });
        }
        if session.messages().len() as u64 != self.successor_head.message_count {
            return Err(SessionStoreError::InvalidTranscriptRewrite {
                id: session.id().clone(),
                reason: format!(
                    "actor session has {} messages but acknowledged successor covers {}",
                    session.messages().len(),
                    self.successor_head.message_count
                ),
            });
        }
        let live_revision = session
            .transcript_content_digest()
            .map_err(SessionStoreError::from)?;
        if live_revision != self.successor_head.head_revision {
            return Err(SessionStoreError::TranscriptRevisionConflict {
                id: session.id().clone(),
                expected: self.successor_head.head_revision.clone(),
                actual: live_revision,
            });
        }
        let successor_prefix =
            self.successor_head
                .message_row_prefix
                .as_ref()
                .ok_or_else(|| SessionStoreError::InvalidTranscriptRewrite {
                    id: session.id().clone(),
                    reason: "acknowledged successor has no exact row-prefix authority".to_string(),
                })?;
        if successor_prefix.row_count() != self.successor_head.message_count {
            return Err(SessionStoreError::InvalidTranscriptRewrite {
                id: session.id().clone(),
                reason: "acknowledged successor row-prefix count differs from its head count"
                    .to_string(),
            });
        }
        let committed_realtime_root = self
            .successor_head
            .realtime_event_prefix
            .as_ref()
            .ok_or_else(|| SessionStoreError::InvalidTranscriptRewrite {
                id: session.id().clone(),
                reason: "acknowledged successor has no realtime component root".to_string(),
            })?
            .clone();
        let live_realtime_root = session.realtime_component_event_prefix().map_err(|error| {
            SessionStoreError::InvalidTranscriptRewrite {
                id: session.id().clone(),
                reason: format!("actor realtime root changed before acknowledgement: {error}"),
            }
        })?;
        if self.successor_head.realtime_event_prefix.as_ref() != Some(&live_realtime_root) {
            return Err(SessionStoreError::InvalidTranscriptRewrite {
                id: session.id().clone(),
                reason: "actor realtime component root changed before acknowledgement".to_string(),
            });
        }
        let live_realtime_suffix =
            session
                .prepare_realtime_component_event_suffix()
                .map_err(|error| SessionStoreError::InvalidTranscriptRewrite {
                    id: session.id().clone(),
                    reason: format!(
                        "actor realtime suffix changed before acknowledgement: {error}"
                    ),
                })?;
        let realtime_suffix_matches =
            match (live_realtime_suffix.as_ref(), self.realtime_suffix.as_ref()) {
                (Some(live), Some(prepared)) => live == prepared,
                (None, Some(prepared)) => prepared.successor() == &live_realtime_root,
                (None, None) => true,
                (Some(_), None) => false,
            };
        if !realtime_suffix_matches {
            return Err(SessionStoreError::InvalidTranscriptRewrite {
                id: session.id().clone(),
                reason: "actor realtime suffix changed before acknowledgement".to_string(),
            });
        }
        session
            .validate_head_canonical_metadata_acknowledgement(&self.metadata_projection)
            .map_err(|reason| SessionStoreError::InvalidTranscriptRewrite {
                id: session.id().clone(),
                reason,
            })?;
        // These operations are infallible after the count/digest preflight;
        // prefix installation can fail only on the count already checked.
        if !session.install_exact_message_row_prefix(successor_prefix.clone()) {
            return Err(SessionStoreError::Internal(format!(
                "acknowledged successor prefix count changed after preflight for session {}",
                session.id()
            )));
        }
        if let Some(prepared) = self.realtime_suffix.as_ref() {
            session
                .acknowledge_realtime_component_event_suffix(prepared, &committed_realtime_root)
                .map_err(|error| {
                    SessionStoreError::Internal(format!(
                        "preflighted realtime acknowledgement failed for session {}: {error}",
                        session.id()
                    ))
                })?;
        }
        session
            .acknowledge_head_canonical_metadata_projection(&self.metadata_projection)
            .map_err(|error| {
                SessionStoreError::Internal(format!(
                    "preflighted metadata acknowledgement failed for session {}: {error}",
                    session.id()
                ))
            })?;
        Ok(())
    }
}

/// One validated historical same-cardinality splice between imported rewrite
/// occurrences.
///
/// The replacement rows are stored on the bridge and every row outside the
/// splice resolves through `source_strand`. Any later parent append is carried
/// by the rewrite step's ordinary parent suffix.
#[derive(Debug, Clone)]
pub struct PreparedHeadCanonicalParentSplice {
    source_strand: TranscriptStrandId,
    link_splice: StrandSplice,
    serialized_replacement: Vec<Vec<u8>>,
}

impl PreparedHeadCanonicalParentSplice {
    #[must_use]
    pub fn source_strand(&self) -> &TranscriptStrandId {
        &self.source_strand
    }

    #[must_use]
    pub const fn link_splice(&self) -> StrandSplice {
        self.link_splice
    }

    #[must_use]
    pub fn serialized_replacement(&self) -> &[Vec<u8>] {
        &self.serialized_replacement
    }
}

/// Exact physical relationship between the preceding rewrite endpoint and the
/// next commit's audited parent.
///
/// Construction is private to the sealed graph consumer below. `ExactAppend`
/// means every already-addressed row is byte-identical and only a suffix must
/// be added. `ExactSplice` materializes a frozen same-cardinality 0.8.10 edge
/// on a dedicated bridge. Current writers never construct it.
#[derive(Debug, Clone)]
pub enum PreparedHeadCanonicalParentTransition {
    ExactAppend,
    ExactSplice(PreparedHeadCanonicalParentSplice),
}

/// One exact transcript-rewrite occurrence in a sealed HeadCanonical delta.
///
/// The successor strand is represented as a splice over `parent_strand`.
/// Only the append bridge that completes the parent and the replacement span
/// that distinguishes the successor are serialized into this carrier. Shared
/// prefix/suffix rows remain addressed through the parent strand.
#[derive(Debug, Clone)]
pub struct PreparedHeadCanonicalRewriteStep {
    commit: TranscriptRewriteCommit,
    serialized_graph_edge: Vec<u8>,
    parent_strand: TranscriptStrandId,
    parent_base_seq: u64,
    serialized_parent_suffix: Vec<Vec<u8>>,
    strand: TranscriptStrandId,
    link_splice: StrandSplice,
    serialized_replacement: Vec<Vec<u8>>,
    parent_transition: PreparedHeadCanonicalParentTransition,
}

impl PreparedHeadCanonicalRewriteStep {
    /// Exact audited rewrite occurrence installed by this step.
    #[must_use]
    pub fn commit(&self) -> &TranscriptRewriteCommit {
        &self.commit
    }

    /// Exact validated compact edge persisted beside this physical delta.
    #[must_use]
    pub fn serialized_graph_edge(&self) -> &[u8] {
        &self.serialized_graph_edge
    }

    /// Physical/logical strand containing the commit parent.
    #[must_use]
    pub fn parent_strand(&self) -> &TranscriptStrandId {
        &self.parent_strand
    }

    /// First parent sequence represented by [`Self::serialized_parent_suffix`].
    #[must_use]
    pub const fn parent_base_seq(&self) -> u64 {
        self.parent_base_seq
    }

    /// Exact bytes that bridge the prior rewrite endpoint to this parent.
    #[must_use]
    pub fn serialized_parent_suffix(&self) -> &[Vec<u8>] {
        &self.serialized_parent_suffix
    }

    /// Typed, exact-byte relationship to the preceding rewrite endpoint.
    #[must_use]
    pub fn parent_transition(&self) -> &PreparedHeadCanonicalParentTransition {
        &self.parent_transition
    }

    /// Occurrence-unique strand created by this rewrite.
    #[must_use]
    pub fn strand(&self) -> &TranscriptStrandId {
        &self.strand
    }

    /// Descriptor expressing this new strand as a delta over its parent.
    ///
    /// In the descriptor's generic vocabulary, the new strand is `strand`
    /// and `parent_strand` is its row source (`successor`). Consequently
    /// `retained_span()` is exactly the replacement range carried below.
    #[must_use]
    pub const fn link_splice(&self) -> StrandSplice {
        self.link_splice
    }

    /// Exact replacement bytes stored on the new strand's retained span.
    #[must_use]
    pub fn serialized_replacement(&self) -> &[Vec<u8>] {
        &self.serialized_replacement
    }
}

/// Sealed pending-rewrite proof retained across tail-only media
/// externalization.
#[derive(Debug, Clone)]
pub struct PreparedHeadCanonicalRewritePreflight {
    observed_head_token: String,
    history: ValidatedTranscriptHistory,
    pending_edges: Vec<Arc<TranscriptRevisionEdge>>,
    live_tail_base: usize,
    serialized_tail: Option<Vec<Vec<u8>>>,
}

impl PreparedHeadCanonicalRewritePreflight {
    /// Prove the pending graph suffix once before a checkpointer scans the
    /// mutable live tail for media.
    ///
    /// Final route preparation consumes this after tail-only externalization
    /// and refuses if the Session changed graph authority in between.
    pub fn prepare(
        session: &Session,
        observed_head: &SessionHead,
    ) -> Result<Option<Self>, SessionStoreError> {
        if &observed_head.id != session.id() {
            return Err(SessionStoreError::InvalidTranscriptRewrite {
                id: session.id().clone(),
                reason: "rewrite preflight head belongs to another session".to_string(),
            });
        }
        let Some(history) = session
            .already_validated_transcript_history_state()
            .map_err(|error| SessionStoreError::InvalidTranscriptRewrite {
                id: session.id().clone(),
                reason: format!("failed to read validated rewrite graph: {error}"),
            })?
        else {
            if observed_head.rewrite_count != 0
                || observed_head.rewrite_prefix != TranscriptRewritePrefixAccumulator::default()
            {
                return Err(SessionStoreError::InvalidTranscriptRewrite {
                    id: session.id().clone(),
                    reason: "physical head carries rewrite authority absent from the live session"
                        .to_string(),
                });
            }
            return Ok(None);
        };
        if observed_head.rewrite_prefix.occurrence_count() != observed_head.rewrite_count {
            return Err(SessionStoreError::Corrupted(session.id().clone()));
        }
        let pending = history
            .prove_commit_suffix_after(&observed_head.rewrite_prefix)
            .map_err(|error| SessionStoreError::InvalidTranscriptRewrite {
                id: session.id().clone(),
                reason: format!("failed to prove pending rewrite suffix: {error}"),
            })?;
        if pending.edges().is_empty() {
            return Ok(None);
        }
        let live_tail_base = pending
            .edges()
            .last()
            .map(|edge| edge.messages_after())
            .ok_or_else(|| SessionStoreError::Corrupted(session.id().clone()))?;
        if live_tail_base > session.messages().len() {
            return Err(SessionStoreError::InvalidTranscriptRewrite {
                id: session.id().clone(),
                reason: "final audited rewrite endpoint exceeds the live transcript".to_string(),
            });
        }
        let pending_edges = pending.edges().to_vec();
        Ok(Some(Self {
            observed_head_token: session_head_cas_token(observed_head)?,
            history,
            pending_edges,
            live_tail_base,
            serialized_tail: None,
        }))
    }

    #[must_use]
    pub const fn live_tail_base(&self) -> usize {
        self.live_tail_base
    }

    /// Externalize and seal exactly the live tail authorized by this proof.
    ///
    /// The resulting serialized suffix and row lineage are retained inside
    /// the preflight, so final preparation neither scans nor serializes the
    /// tail a second time.
    pub async fn externalize_live_tail(
        mut self,
        session: &mut Session,
        blob_store: &dyn crate::BlobStore,
    ) -> Result<Self, crate::blob::BlobStoreError> {
        let live_history = session
            .already_validated_transcript_history_state()
            .map_err(|error| crate::blob::BlobStoreError::Internal(error.to_string()))?
            .ok_or_else(|| {
                crate::blob::BlobStoreError::Internal(
                    "preflighted rewrite graph disappeared before media externalization"
                        .to_string(),
                )
            })?;
        if !live_history.shares_exact_state_with(&self.history) {
            return Err(crate::blob::BlobStoreError::Internal(
                "rewrite graph changed before media externalization".to_string(),
            ));
        }
        session
            .externalize_media(blob_store, self.live_tail_base)
            .await?;
        let serialized_tail = session.messages()[self.live_tail_base..]
            .iter()
            .map(serde_json::to_vec)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| crate::blob::BlobStoreError::Internal(error.to_string()))?;
        let endpoint_prefix = self
            .pending_edges
            .last()
            .map(|edge| edge.result_witness().row_prefix().clone())
            .ok_or_else(|| {
                crate::blob::BlobStoreError::Internal(
                    "rewrite preflight lost its final endpoint".to_string(),
                )
            })?;
        let current_prefix = endpoint_prefix
            .extend_serialized_rows(&serialized_tail)
            .map_err(|error| crate::blob::BlobStoreError::Internal(error.to_string()))?;
        if !session.install_exact_message_row_lineage(endpoint_prefix, current_prefix) {
            return Err(crate::blob::BlobStoreError::Internal(
                "externalized rewrite tail changed row count during sealing".to_string(),
            ));
        }
        self.serialized_tail = Some(serialized_tail);
        Ok(self)
    }
}

/// Sealed same-session HeadCanonical transcript rewrite.
///
/// Unlike [`PreparedHeadCanonicalMutation`], this carrier may change strand
/// and rewrite generation. It contains one whole-document *proof pass* but no
/// whole-document row vector: durable work is the sum of append bridges,
/// replacement spans, and the final live tail. Private construction binds
/// those deltas to a validated transcript graph, exact predecessor CAS, exact
/// successor head, and typed row-lineage authority.
#[derive(Debug, Clone)]
pub struct PreparedHeadCanonicalRewriteMutation {
    predecessor_head: SessionHead,
    predecessor_head_token: String,
    common: PreparedHeadCanonicalMutation,
    steps: Vec<PreparedHeadCanonicalRewriteStep>,
    tail_base_seq: u64,
    serialized_tail: Vec<Vec<u8>>,
}

impl PreparedHeadCanonicalRewriteMutation {
    /// Return the sealed final-audited live-tail base when `session` contains
    /// rewrite occurrences beyond the exact observed physical head.
    ///
    /// This is the single proof-owning machine verdict for both routing and
    /// the checkpointer's O(delta) media scan. The returned row is derived
    /// from the final commit carried by the same validated graph that proves
    /// the pending occurrence prefix; callers must not re-derive either fact
    /// from the live document.
    pub fn pending_live_tail_base(
        session: &Session,
        observed_head: &SessionHead,
    ) -> Result<Option<usize>, SessionStoreError> {
        PreparedHeadCanonicalRewritePreflight::prepare(session, observed_head)
            .map(|preflight| preflight.map(|proof| proof.live_tail_base()))
    }

    /// Decide whether `session` contains rewrite occurrences beyond the exact
    /// observed physical head.
    ///
    /// Kept as the boolean convenience over [`Self::pending_live_tail_base`]
    /// so every caller shares the same sealed routing verdict.
    pub fn is_required(
        session: &Session,
        observed_head: &SessionHead,
    ) -> Result<bool, SessionStoreError> {
        Self::pending_live_tail_base(session, observed_head).map(|base| base.is_some())
    }

    /// Prepare a rewrite successor from an exact physical head.
    pub fn prepare(
        session: &Session,
        observed_head: SessionHead,
    ) -> Result<Self, SessionStoreError> {
        Self::require_pending(
            Self::try_prepare_current(session, observed_head, None)?,
            session.id(),
        )
    }

    /// Prepare a store-authorized rewrite successor from the exact committed
    /// boundary and observed physical head.
    pub fn prepare_successor(
        session: &Session,
        runtime_boundary_head: &SessionHead,
        observed_head: SessionHead,
    ) -> Result<Self, SessionStoreError> {
        validate_store_issued_head_pair(session, runtime_boundary_head, &observed_head)?;
        Self::require_pending(
            Self::try_prepare_current(session, observed_head, None)?,
            session.id(),
        )
    }

    /// Prepare an in-run physical rewrite projection.
    pub fn prepare_intra_turn(
        session: &Session,
        runtime_boundary_head: &SessionHead,
        observed_head: SessionHead,
    ) -> Result<Self, SessionStoreError> {
        Self::prepare_successor(session, runtime_boundary_head, observed_head)
    }

    fn require_pending(
        prepared: Option<Self>,
        session_id: &SessionId,
    ) -> Result<Self, SessionStoreError> {
        prepared.ok_or_else(|| SessionStoreError::InvalidTranscriptRewrite {
            id: session_id.clone(),
            reason: "specialized rewrite carrier requires an unpersisted occurrence".to_string(),
        })
    }

    fn try_prepare_current(
        session: &Session,
        observed_head: SessionHead,
        preflight: Option<PreparedHeadCanonicalRewritePreflight>,
    ) -> Result<Option<Self>, SessionStoreError> {
        let id = session.id().clone();
        validate_session_head_storage_representation(&observed_head)?;
        if observed_head.id != id {
            return Err(SessionStoreError::InvalidTranscriptRewrite {
                id,
                reason: "observed rewrite head belongs to another session".to_string(),
            });
        }
        let observed_message_prefix =
            observed_head.message_row_prefix.as_ref().ok_or_else(|| {
                SessionStoreError::InvalidTranscriptRewrite {
                    id: session.id().clone(),
                    reason: "observed rewrite head has no exact message-row authority".to_string(),
                }
            })?;
        if observed_message_prefix.row_count() != observed_head.message_count
            || observed_head.rewrite_prefix.occurrence_count() != observed_head.rewrite_count
        {
            return Err(SessionStoreError::Corrupted(session.id().clone()));
        }
        let observed_count = usize::try_from(observed_head.rewrite_count)
            .map_err(|_| SessionStoreError::Corrupted(session.id().clone()))?;
        let (history, pending_edges, preflight_serialized_tail) = match preflight {
            Some(preflight) => {
                if preflight.observed_head_token != session_head_cas_token(&observed_head)? {
                    return Err(SessionStoreError::TranscriptRevisionConflict {
                        id: session.id().clone(),
                        expected: preflight.observed_head_token,
                        actual: session_head_cas_token(&observed_head)?,
                    });
                }
                let live_history = session
                    .already_validated_transcript_history_state()
                    .map_err(|error| SessionStoreError::InvalidTranscriptRewrite {
                        id: session.id().clone(),
                        reason: format!("failed to read preflighted rewrite graph: {error}"),
                    })?
                    .ok_or_else(|| SessionStoreError::InvalidTranscriptRewrite {
                        id: session.id().clone(),
                        reason: "preflighted rewrite graph disappeared before preparation"
                            .to_string(),
                    })?;
                if !live_history.shares_exact_state_with(&preflight.history) {
                    return Err(SessionStoreError::InvalidTranscriptRewrite {
                        id: session.id().clone(),
                        reason: "rewrite graph changed after media-scan preflight".to_string(),
                    });
                }
                let serialized_tail = preflight.serialized_tail.ok_or_else(|| {
                    SessionStoreError::InvalidTranscriptRewrite {
                        id: session.id().clone(),
                        reason: "rewrite preflight was not sealed by tail media externalization"
                            .to_string(),
                    }
                })?;
                (
                    preflight.history,
                    preflight.pending_edges,
                    Some(serialized_tail),
                )
            }
            None => {
                let history = session
                    .validated_transcript_history_state()
                    .map_err(|error| SessionStoreError::InvalidTranscriptRewrite {
                        id: session.id().clone(),
                        reason: format!("failed to validate transcript rewrite graph: {error}"),
                    })?
                    .ok_or_else(|| SessionStoreError::InvalidTranscriptRewrite {
                        id: session.id().clone(),
                        reason: "same-session rewrite has no validated transcript graph"
                            .to_string(),
                    })?;
                if observed_count > history.commit_count() {
                    return Err(SessionStoreError::InvalidTranscriptRewrite {
                        id: session.id().clone(),
                        reason: "observed rewrite generation exceeds the proved graph".to_string(),
                    });
                }
                let pending_edges = history
                    .prove_commit_suffix_after(&observed_head.rewrite_prefix)
                    .map_err(|error| SessionStoreError::InvalidTranscriptRewrite {
                        id: session.id().clone(),
                        reason: format!("failed to prove pending rewrite suffix: {error}"),
                    })?
                    .edges()
                    .to_vec();
                (history, pending_edges, None)
            }
        };
        let expected_observed_graph_prefix =
            history.state().graph_prefix_at(observed_count).cloned();
        if observed_head.graph_prefix != expected_observed_graph_prefix {
            return Err(SessionStoreError::TranscriptContinuityViolation {
                id: session.id().clone(),
                previous_revision: observed_head.graph_prefix.as_ref().map_or_else(
                    || "graph-root".to_string(),
                    |prefix| prefix.digest().to_string(),
                ),
                incoming_revision: expected_observed_graph_prefix.as_ref().map_or_else(
                    || "graph-root".to_string(),
                    |prefix| prefix.digest().to_string(),
                ),
                reason: "observed physical graph prefix differs from the retained Session graph"
                    .to_string(),
            });
        }
        if pending_edges.is_empty() {
            return Ok(None);
        }

        let mut current_strand = observed_head.strand.clone();
        let mut current_len = usize::try_from(observed_head.message_count)
            .map_err(|_| SessionStoreError::Corrupted(session.id().clone()))?;
        let mut current_prefix = observed_message_prefix.clone();
        let mut steps = Vec::with_capacity(pending_edges.len());

        for (pending_index, edge) in pending_edges.iter().enumerate() {
            let commit = edge.commit();
            let base_witness = if pending_index == 0 {
                if observed_count == 0 {
                    None
                } else {
                    history
                        .state()
                        .edge(observed_count - 1)
                        .map(|edge| edge.result_witness())
                }
            } else {
                pending_edges
                    .get(pending_index - 1)
                    .map(|edge| edge.result_witness())
            };
            let base_count = match base_witness {
                Some(witness) => witness.message_count(),
                None => history.anchor().messages().len(),
            };
            let base_prefix = match base_witness {
                Some(witness) => witness.row_prefix(),
                None => history.anchor().row_prefix(),
            };
            if base_count != edge.messages_before_base()
                || current_len < base_count
                || current_len > edge.messages_before()
            {
                return Err(SessionStoreError::InvalidTranscriptRewrite {
                    id: session.id().clone(),
                    reason: format!(
                        "rewrite occurrence {} cannot extend the observed physical endpoint",
                        edge.rewrite_generation()
                    ),
                });
            }

            let appended = edge.parent_advance().appended();
            let already_appended = current_len - base_count;
            if already_appended > appended.len() {
                return Err(SessionStoreError::InvalidTranscriptRewrite {
                    id: session.id().clone(),
                    reason: "observed physical endpoint exceeds the proved parent advance"
                        .to_string(),
                });
            }
            let serialized_already_appended = appended[..already_appended]
                .iter()
                .map(|message| serde_json::to_vec(message).map_err(SessionStoreError::from))
                .collect::<Result<Vec<_>, _>>()?;
            let expected_current_prefix =
                base_prefix.extend_serialized_rows(&serialized_already_appended)?;
            if expected_current_prefix != current_prefix {
                return Err(SessionStoreError::TranscriptContinuityViolation {
                    id: session.id().clone(),
                    previous_revision: current_prefix.digest().to_string(),
                    incoming_revision: expected_current_prefix.digest().to_string(),
                    reason: "observed rows do not match the compact parent-advance lineage"
                        .to_string(),
                });
            }
            let mut advanced_base_prefix = base_prefix.clone();
            let parent_transition =
                if let Some((at, replacement)) = edge.parent_advance().exact_splice() {
                    let replacement_rows = replacement
                        .iter()
                        .map(|message| serde_json::to_vec(message).map_err(SessionStoreError::from))
                        .collect::<Result<Vec<_>, _>>()?;
                    let end = at
                        .checked_add(replacement.len())
                        .ok_or_else(|| SessionStoreError::Corrupted(session.id().clone()))?;
                    advanced_base_prefix = advanced_base_prefix.replace_serialized_range(
                        u64::try_from(at)
                            .map_err(|_| SessionStoreError::Corrupted(session.id().clone()))?,
                        u64::try_from(end)
                            .map_err(|_| SessionStoreError::Corrupted(session.id().clone()))?,
                        &replacement_rows,
                    )?;
                    let bridge_strand = TranscriptStrandId::from_rewrite_parent_occurrence(commit);
                    let splice = PreparedHeadCanonicalParentSplice {
                        source_strand: current_strand.clone(),
                        link_splice: StrandSplice {
                            strand_len: u64::try_from(current_len)
                                .map_err(|_| SessionStoreError::Corrupted(session.id().clone()))?,
                            splice_start: u64::try_from(at)
                                .map_err(|_| SessionStoreError::Corrupted(session.id().clone()))?,
                            splice_end: u64::try_from(end)
                                .map_err(|_| SessionStoreError::Corrupted(session.id().clone()))?,
                            successor_end: u64::try_from(end)
                                .map_err(|_| SessionStoreError::Corrupted(session.id().clone()))?,
                        },
                        serialized_replacement: replacement_rows,
                    };
                    current_strand = bridge_strand;
                    PreparedHeadCanonicalParentTransition::ExactSplice(splice)
                } else {
                    PreparedHeadCanonicalParentTransition::ExactAppend
                };
            let serialized_parent_suffix = appended[already_appended..]
                .iter()
                .map(|message| serde_json::to_vec(message).map_err(SessionStoreError::from))
                .collect::<Result<Vec<_>, _>>()?;
            let serialized_all_appended = appended
                .iter()
                .map(|message| serde_json::to_vec(message).map_err(SessionStoreError::from))
                .collect::<Result<Vec<_>, _>>()?;
            let parent_prefix =
                advanced_base_prefix.extend_serialized_rows(&serialized_all_appended)?;
            if parent_prefix != *edge.parent_row_prefix() {
                return Err(SessionStoreError::InvalidTranscriptRewrite {
                    id: session.id().clone(),
                    reason: "compact edge parent lineage differs from its exact delta".to_string(),
                });
            }
            let serialized_replacement = edge
                .rewrite()
                .replacement()
                .iter()
                .map(|message| serde_json::to_vec(message).map_err(SessionStoreError::from))
                .collect::<Result<Vec<_>, _>>()?;
            if crate::image_content::messages_have_inline_media(&appended[already_appended..])
                || crate::image_content::messages_have_inline_media(edge.rewrite().replacement())
                || edge
                    .parent_advance()
                    .exact_splice()
                    .is_some_and(|(_, replacement)| {
                        crate::image_content::messages_have_inline_media(replacement)
                    })
            {
                return Err(SessionStoreError::InvalidTranscriptRewrite {
                    id: session.id().clone(),
                    reason: "compact rewrite delta carries inline media".to_string(),
                });
            }
            let (start, end) = commit.selection.bounds();
            let replacement_end = start
                .checked_add(serialized_replacement.len())
                .ok_or_else(|| SessionStoreError::Corrupted(session.id().clone()))?;
            let parent_base_seq = u64::try_from(current_len)
                .map_err(|_| SessionStoreError::Corrupted(session.id().clone()))?;
            let strand = TranscriptStrandId::from_rewrite_occurrence(commit);
            let link_splice = StrandSplice {
                strand_len: u64::try_from(edge.messages_after())
                    .map_err(|_| SessionStoreError::Corrupted(session.id().clone()))?,
                splice_start: u64::try_from(start)
                    .map_err(|_| SessionStoreError::Corrupted(session.id().clone()))?,
                splice_end: u64::try_from(replacement_end)
                    .map_err(|_| SessionStoreError::Corrupted(session.id().clone()))?,
                successor_end: u64::try_from(end)
                    .map_err(|_| SessionStoreError::Corrupted(session.id().clone()))?,
            };
            if link_splice.retained_rows()
                != u64::try_from(serialized_replacement.len())
                    .map_err(|_| SessionStoreError::Corrupted(session.id().clone()))?
            {
                return Err(SessionStoreError::InvalidTranscriptRewrite {
                    id: session.id().clone(),
                    reason: "compact rewrite replacement does not match its physical splice"
                        .to_string(),
                });
            }
            steps.push(PreparedHeadCanonicalRewriteStep {
                commit: commit.clone(),
                serialized_graph_edge: edge.to_replay_bytes().map_err(SessionStoreError::from)?,
                parent_strand: current_strand.clone(),
                parent_base_seq,
                serialized_parent_suffix,
                strand: strand.clone(),
                link_splice,
                serialized_replacement,
                parent_transition,
            });
            current_strand = strand;
            current_len = edge.messages_after();
            current_prefix = edge.result_witness().row_prefix().clone();
        }

        if current_len > session.messages().len() {
            return Err(SessionStoreError::MonotonicityViolation {
                id: session.id().clone(),
                prev_len: current_len,
                new_len: session.messages().len(),
            });
        }
        let tail = &session.messages()[current_len..];
        if crate::image_content::messages_have_inline_media(tail) {
            return Err(SessionStoreError::InvalidTranscriptRewrite {
                id: session.id().clone(),
                reason: "post-rewrite live tail carries inline media".to_string(),
            });
        }
        let serialized_tail = match preflight_serialized_tail {
            Some(serialized_tail) => serialized_tail,
            None => tail
                .iter()
                .map(|message| serde_json::to_vec(message).map_err(SessionStoreError::from))
                .collect::<Result<Vec<_>, _>>()?,
        };
        if serialized_tail.len() != tail.len() {
            return Err(SessionStoreError::InvalidTranscriptRewrite {
                id: session.id().clone(),
                reason: "live tail changed after media externalization was sealed".to_string(),
            });
        }
        let successor_message_row_prefix =
            current_prefix.extend_serialized_rows(&serialized_tail)?;
        let live_prefix = session
            .exact_message_row_prefix_at(
                u64::try_from(session.messages().len())
                    .map_err(|_| SessionStoreError::Corrupted(session.id().clone()))?,
            )
            .ok_or_else(|| SessionStoreError::InvalidTranscriptRewrite {
                id: session.id().clone(),
                reason: "live session has no exact row-lineage authority".to_string(),
            })?;
        if successor_message_row_prefix != live_prefix {
            return Err(SessionStoreError::TranscriptContinuityViolation {
                id: session.id().clone(),
                previous_revision: successor_message_row_prefix.digest().to_string(),
                incoming_revision: live_prefix.digest().to_string(),
                reason: "live rows do not extend the final compact occurrence".to_string(),
            });
        }

        let realtime_suffix =
            session
                .prepare_realtime_component_event_suffix()
                .map_err(|error| SessionStoreError::InvalidTranscriptRewrite {
                    id: session.id().clone(),
                    reason: format!("failed to prepare realtime suffix: {error}"),
                })?;
        let acknowledged_realtime = session
            .realtime_component_event_acknowledged_prefix()
            .clone();
        if observed_head.realtime_event_prefix.as_ref() != Some(&acknowledged_realtime) {
            return Err(SessionStoreError::TranscriptContinuityViolation {
                id: session.id().clone(),
                previous_revision: "observed-component-roots".to_string(),
                incoming_revision: "tracker-acknowledged-component-roots".to_string(),
                reason: "rewrite realtime component tracker does not extend observed root"
                    .to_string(),
            });
        }
        let successor_realtime = session.realtime_component_event_prefix().map_err(|error| {
            SessionStoreError::InvalidTranscriptRewrite {
                id: session.id().clone(),
                reason: format!("failed to derive realtime root: {error}"),
            }
        })?;
        match realtime_suffix.as_ref() {
            Some(suffix)
                if suffix.predecessor() == &acknowledged_realtime
                    && suffix.successor() == &successor_realtime => {}
            None if acknowledged_realtime == successor_realtime => {}
            _ => {
                return Err(SessionStoreError::InvalidTranscriptRewrite {
                    id: session.id().clone(),
                    reason: "realtime suffix does not bridge rewrite roots".to_string(),
                });
            }
        }
        let successor_rewrite_count = u64::try_from(history.commit_count())
            .map_err(|_| SessionStoreError::Corrupted(session.id().clone()))?;
        let preserved_row_lineage_anchor =
            observed_head.row_lineage_anchor.clone().filter(|anchor| {
                successor_rewrite_count
                    .checked_sub(anchor.rewrite_count())
                    .is_some_and(|delta| delta < SESSION_ROW_LINEAGE_REBASE_INTERVAL)
            });
        let successor_head = SessionHead::from_session_with_message_row_prefix(
            session,
            current_strand,
            successor_rewrite_count,
            successor_message_row_prefix,
            Some(history.rewrite_prefix().clone()),
            preserved_row_lineage_anchor,
            true,
        )?;
        if successor_head.realtime_event_prefix.as_ref() != Some(&successor_realtime) {
            return Err(SessionStoreError::InvalidTranscriptRewrite {
                id: session.id().clone(),
                reason: "rewrite successor head does not bind prepared realtime component root"
                    .to_string(),
            });
        }
        let successor_head_token = session_head_cas_token(&successor_head)?;
        let predecessor_head_token = session_head_cas_token(&observed_head)?;
        let metadata_projection = successor_head
            .metadata_projection
            .as_ref()
            .cloned()
            .ok_or_else(|| SessionStoreError::InvalidTranscriptRewrite {
                id: session.id().clone(),
                reason: "prepared rewrite successor has no sealed metadata transition".to_string(),
            })?;
        if metadata_projection.predecessor_identity() != observed_head.metadata_identity.as_ref() {
            return Err(SessionStoreError::InvalidTranscriptRewrite {
                id: session.id().clone(),
                reason: "prepared rewrite metadata does not extend the observed head".to_string(),
            });
        }
        let tail_base_seq = u64::try_from(current_len)
            .map_err(|_| SessionStoreError::Corrupted(session.id().clone()))?;
        let common = PreparedHeadCanonicalMutation {
            predecessor_head: Some(observed_head.clone()),
            expected_cas: SessionHeadCas::IfToken(predecessor_head_token.clone()),
            successor_head,
            successor_head_token,
            metadata_projection,
            base_seq: tail_base_seq,
            serialized_suffix: Vec::new(),
            realtime_suffix,
        };
        common.validate_live_successor(session)?;
        Ok(Some(Self {
            predecessor_head: observed_head,
            predecessor_head_token,
            common,
            steps,
            tail_base_seq,
            serialized_tail,
        }))
    }

    #[must_use]
    pub fn session_id(&self) -> &SessionId {
        self.common.session_id()
    }

    #[must_use]
    pub fn predecessor_head(&self) -> &SessionHead {
        &self.predecessor_head
    }

    #[must_use]
    pub fn expected_cas(&self) -> &SessionHeadCas {
        self.common.expected_cas()
    }

    #[must_use]
    pub fn predecessor_head_token(&self) -> &str {
        &self.predecessor_head_token
    }

    #[must_use]
    pub fn successor_head(&self) -> &SessionHead {
        self.common.successor_head()
    }

    #[must_use]
    pub fn successor_head_token(&self) -> &str {
        self.common.successor_head_token()
    }

    #[must_use]
    pub fn steps(&self) -> &[PreparedHeadCanonicalRewriteStep] {
        &self.steps
    }

    #[must_use]
    pub const fn tail_base_seq(&self) -> u64 {
        self.tail_base_seq
    }

    #[must_use]
    pub fn serialized_tail(&self) -> &[Vec<u8>] {
        &self.serialized_tail
    }

    #[must_use]
    pub fn realtime_suffix(&self) -> Option<&PreparedComponentEventSuffix> {
        self.common.realtime_suffix()
    }

    pub(crate) fn validate_live_successor(
        &self,
        session: &Session,
    ) -> Result<(), SessionStoreError> {
        self.common.validate_live_successor(session)
    }

    pub fn acknowledge_session(
        &self,
        session: &mut Session,
        committed_head_token: &str,
    ) -> Result<(), SessionStoreError> {
        self.common
            .acknowledge_session(session, committed_head_token)
    }

    pub fn acknowledge_physical_projection(
        &self,
        session: &mut Session,
        committed_head_token: &str,
    ) -> Result<(), SessionStoreError> {
        self.common
            .acknowledge_physical_projection(session, committed_head_token)
    }
}

/// Singular machine-authorized HeadCanonical persistence route.
///
/// Construction attempts the sealed rewrite suffix exactly once. A pending
/// occurrence returns `Rewrite`; absence returns the ordinary same-strand
/// mutation. Callers must match this enum instead of running `is_required` and
/// then proving the graph again through a second prepare call.
#[derive(Debug, Clone)]
pub enum PreparedHeadCanonicalMutationRoute {
    Ordinary(PreparedHeadCanonicalMutation),
    Rewrite(PreparedHeadCanonicalRewriteMutation),
}

impl PreparedHeadCanonicalMutationRoute {
    pub fn prepare(
        session: &Session,
        observed_head: Option<SessionHead>,
    ) -> Result<Self, SessionStoreError> {
        let Some(observed_head) = observed_head else {
            return PreparedHeadCanonicalMutation::prepare(session, None).map(Self::Ordinary);
        };
        Self::prepare_observed(session, observed_head)
    }

    pub fn prepare_successor(
        session: &Session,
        runtime_boundary_head: &SessionHead,
        observed_head: SessionHead,
    ) -> Result<Self, SessionStoreError> {
        validate_store_issued_head_pair(session, runtime_boundary_head, &observed_head)?;
        Self::prepare_observed(session, observed_head)
    }

    pub fn prepare_intra_turn(
        session: &Session,
        runtime_boundary_head: &SessionHead,
        observed_head: SessionHead,
    ) -> Result<Self, SessionStoreError> {
        Self::prepare_successor(session, runtime_boundary_head, observed_head)
    }

    /// Finalize an in-run route after media externalization consumed the exact
    /// preflight tail proof.
    pub fn prepare_intra_turn_after_preflight(
        session: &Session,
        runtime_boundary_head: &SessionHead,
        observed_head: SessionHead,
        preflight: Option<PreparedHeadCanonicalRewritePreflight>,
    ) -> Result<Self, SessionStoreError> {
        validate_store_issued_head_pair(session, runtime_boundary_head, &observed_head)?;
        let Some(preflight) = preflight else {
            return PreparedHeadCanonicalMutation::prepare(session, Some(observed_head))
                .map(Self::Ordinary);
        };
        let prepared = PreparedHeadCanonicalRewriteMutation::try_prepare_current(
            session,
            observed_head,
            Some(preflight),
        )?;
        PreparedHeadCanonicalRewriteMutation::require_pending(prepared, session.id())
            .map(Self::Rewrite)
    }

    fn prepare_observed(
        session: &Session,
        observed_head: SessionHead,
    ) -> Result<Self, SessionStoreError> {
        match PreparedHeadCanonicalRewriteMutation::try_prepare_current(
            session,
            observed_head.clone(),
            None,
        )? {
            Some(rewrite) => Ok(Self::Rewrite(rewrite)),
            None => PreparedHeadCanonicalMutation::prepare(session, Some(observed_head))
                .map(Self::Ordinary),
        }
    }

    #[must_use]
    pub fn ordinary(&self) -> Option<&PreparedHeadCanonicalMutation> {
        match self {
            Self::Ordinary(mutation) => Some(mutation),
            Self::Rewrite(_) => None,
        }
    }

    #[must_use]
    pub fn rewrite(&self) -> Option<&PreparedHeadCanonicalRewriteMutation> {
        match self {
            Self::Ordinary(_) => None,
            Self::Rewrite(mutation) => Some(mutation),
        }
    }
}

fn validate_store_issued_head_pair(
    session: &Session,
    runtime_boundary_head: &SessionHead,
    observed_head: &SessionHead,
) -> Result<(), SessionStoreError> {
    validate_session_head_storage_representation(runtime_boundary_head)?;
    validate_session_head_storage_representation(observed_head)?;
    if &runtime_boundary_head.id != session.id() || &observed_head.id != session.id() {
        return Err(SessionStoreError::InvalidTranscriptRewrite {
            id: session.id().clone(),
            reason: "store-issued committed or observed head belongs to another session"
                .to_string(),
        });
    }
    if runtime_boundary_head.row_lineage_anchor != observed_head.row_lineage_anchor {
        return Err(SessionStoreError::TranscriptContinuityViolation {
            id: session.id().clone(),
            previous_revision: session_head_cas_token(runtime_boundary_head)?,
            incoming_revision: session_head_cas_token(observed_head)?,
            reason: "observed physical head does not share the committed row-lineage origin"
                .to_string(),
        });
    }
    let boundary_prefix = runtime_boundary_head
        .message_row_prefix
        .as_ref()
        .ok_or_else(|| SessionStoreError::InvalidTranscriptRewrite {
            id: session.id().clone(),
            reason: "committed boundary has no exact message-row prefix".to_string(),
        })?;
    let live_boundary_prefix = session
        .exact_message_row_prefix_at(runtime_boundary_head.message_count)
        .ok_or_else(|| SessionStoreError::InvalidTranscriptRewrite {
            id: session.id().clone(),
            reason: "live session does not retain the committed boundary row prefix".to_string(),
        })?;
    if boundary_prefix != &live_boundary_prefix {
        return Err(SessionStoreError::TranscriptContinuityViolation {
            id: session.id().clone(),
            previous_revision: boundary_prefix.digest().to_string(),
            incoming_revision: live_boundary_prefix.digest().to_string(),
            reason: "live session does not continue the exact committed store boundary".to_string(),
        });
    }
    Ok(())
}

/// Capability trait for O(delta) session persistence.
///
/// Every retained transcript body is addressed by a strand delta: an exact
/// append parent extends the preceding endpoint strand directly, while an
/// imported 0.8.10 exact splice first creates a same-cardinality bridge over
/// that strand. The revision body of commit `k` is then a splice over its
/// exact parent strand. Compaction therefore persists O(live-after) instead
/// of a superset blob.
///
/// # Storage bound (the contract, not merely an implementation note)
///
/// Prefix addressing alone does NOT bound total storage: successive strands
/// are separate address spaces, so a rewrite that shares no *prefix* with its
/// parent — including a released 0.8.10 non-append parent shape —
/// costs a full transcript of fresh rows, per rewrite, forever. Measured in the
/// field: 98 rewrites of one 371-message transcript accumulated 16,672 strand
/// rows.
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

    /// Atomically install one sealed HeadCanonical create/append mutation.
    ///
    /// Implementations must revalidate the exact predecessor CAS, reconcile
    /// the pre-serialized message suffix and realtime component-event suffix,
    /// persist the metadata projection named by the successor, and publish
    /// the successor head as one transaction. An exact already-applied
    /// successor is an idempotent success only after all named rows and
    /// sidecars are reverified. The returned token must be the exact durable
    /// successor head token.
    ///
    /// The compatibility default refuses. A backend may still implement all
    /// legacy incremental verbs without claiming the atomic HeadCanonical
    /// prepared-mutation capability.
    async fn apply_prepared_head_canonical_mutation(
        &self,
        mutation: &PreparedHeadCanonicalMutation,
    ) -> Result<String, SessionStoreError> {
        Err(SessionStoreError::Internal(format!(
            "incremental store does not implement atomic prepared HeadCanonical mutation for session {}",
            mutation.session_id()
        )))
    }

    /// Atomically install one sealed same-session HeadCanonical rewrite.
    ///
    /// Implementations must write only the carrier's bridge/replacement/tail
    /// deltas, record every rewrite occurrence, reconcile the realtime component
    /// suffix and metadata lineage, and publish the exact successor head in
    /// one transaction. The successor strand of each step resolves shared
    /// prefix/suffix rows through its parent. Only a successor whose sealed
    /// head rotates to a current [`SessionRowLineageAnchor`] may be settled
    /// into one full direct strand; materializing any intermediate or
    /// un-authorized successor violates this capability's O(delta) contract.
    /// Exact retries must reverify every named row, occurrence and sidecar,
    /// plus either the named final link or the rotated direct-anchor bytes,
    /// before returning the sealed successor token.
    async fn apply_prepared_head_canonical_rewrite_mutation(
        &self,
        mutation: &PreparedHeadCanonicalRewriteMutation,
    ) -> Result<String, SessionStoreError> {
        Err(SessionStoreError::Internal(format!(
            "incremental store does not implement atomic prepared HeadCanonical rewrite for session {}",
            mutation.session_id()
        )))
    }

    /// Materialize and verify the current physical head.
    ///
    /// Ordinary callers must use [`Self::load_head`], whose head row stays
    /// compact and does not resolve transcript, metadata, or component
    /// sidecars. This exceptional seam is for compatibility/rebase guards
    /// that genuinely need the previous typed session.
    ///
    /// `expected` must still be the store's current physical head.
    /// Implementations with split head/blob/component storage must compare
    /// its exact [`session_head_cas_token`] to the current row and resolve all
    /// named data in one read snapshot. A stale or unreachable head must fail
    /// closed; this read never makes historical metadata reachable and never
    /// becomes a retention owner.
    ///
    /// The safe default rechecks the compact current head, performs the
    /// backend's canonical full load, and then proves that the hydrated
    /// session re-projects to `expected`. Backends can override this to keep
    /// the current-head check and all sidecar reads in one transaction. A
    /// backend whose canonical full load does not install exact row lineage
    /// and verified component reducers is refused by the default and must
    /// provide that stronger override.
    async fn materialize_head(
        &self,
        expected: &SessionHead,
    ) -> Result<VerifiedSessionHeadMaterialization, SessionStoreError> {
        let current = self
            .load_head(&expected.id)
            .await?
            .ok_or_else(|| SessionStoreError::NotFound(expected.id.clone()))?;
        let expected_token = session_head_cas_token(expected)?;
        let current_token = session_head_cas_token(&current)?;
        if current_token != expected_token {
            return Err(SessionStoreError::TranscriptRevisionConflict {
                id: expected.id.clone(),
                expected: expected_token,
                actual: current_token,
            });
        }
        let session = self
            .load(&expected.id)
            .await?
            .ok_or_else(|| SessionStoreError::NotFound(expected.id.clone()))?;
        expected.clone().verify_materialized_session(session)
    }

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
/// Admits: metadata-only update, an exact prefix-preserving append, and
/// transient-notice cleanup. Incoming history state, if present, must
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
    let stored_prefix =
        TranscriptRewritePrefixAccumulator::from_commits(stored_commits).map_err(|error| {
            SessionStoreError::InvalidTranscriptRewrite {
                id: incoming.id().clone(),
                reason: format!("stored rewrite prefix is malformed: {error}"),
            }
        })?;
    head_canonical_plain_save_guard_with_prefix_witness(
        incoming,
        previous_slim,
        u64::try_from(stored_commits.len())
            .map_err(|_| SessionStoreError::Corrupted(incoming.id().clone()))?,
        &stored_prefix,
        witness,
    )
}

/// O(1)-authority variant of the HeadCanonical plain-save guard.
///
/// Stores already persist the exact adopted occurrence count and rolling
/// prefix in [`SessionHead`]; passing those authorities avoids cloning or
/// serializing the accumulated commit vector on every ordinary save.
pub fn head_canonical_plain_save_guard_with_prefix_witness(
    incoming: &Session,
    previous_slim: &Session,
    stored_rewrite_count: u64,
    stored_rewrite_prefix: &TranscriptRewritePrefixAccumulator,
    witness: SaveGuardWitness<'_>,
) -> Result<(), SessionStoreError> {
    if stored_rewrite_prefix.occurrence_count() != stored_rewrite_count {
        return Err(SessionStoreError::Corrupted(incoming.id().clone()));
    }
    incoming
        .validate_transcript_history_state()
        .map_err(|err| SessionStoreError::InvalidTranscriptRewrite {
            id: incoming.id().clone(),
            reason: format!("incoming transcript history state is malformed: {err}"),
        })?;
    let incoming_revision = resolve_transcript_revision(incoming, witness.incoming_revision)?;
    let incoming_state = incoming
        .validated_transcript_history_state()
        .map_err(|err| SessionStoreError::InvalidTranscriptRewrite {
            id: incoming.id().clone(),
            reason: format!("incoming transcript history state is malformed: {err}"),
        })?;
    if let Some(state) = incoming_state.as_ref() {
        validate_live_transcript_history_head_coherence(
            incoming,
            state.state(),
            &incoming_revision,
            "incoming",
        )?;
        let incoming_count = u64::try_from(state.commit_count())
            .map_err(|_| SessionStoreError::Corrupted(incoming.id().clone()))?;
        if incoming_count != stored_rewrite_count || state.rewrite_prefix() != stored_rewrite_prefix
        {
            if incoming_count > stored_rewrite_count {
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
                 matches the typed transient-notice cleanup shape"
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
    validate_session_head_storage_representation(head)?;
    if session_head_has_component_roots(head) {
        return Err(SessionStoreError::InvalidTranscriptRewrite {
            id: head.id.clone(),
            reason: "activated HeadCanonical heads must be committed through PreparedHeadCanonicalMutation so metadata and the realtime component-event suffix are installed atomically"
                .to_string(),
        });
    }
    if head
        .metadata
        .contains_key(SESSION_TRANSCRIPT_HISTORY_STATE_KEY)
    {
        return Err(SessionStoreError::InvalidTranscriptRewrite {
            id: head.id.clone(),
            reason: "session head must not inline transcript history state metadata".to_string(),
        });
    }
    if !transcript_rewrite_prefix_is_canonical(&head.rewrite_prefix) {
        return Err(SessionStoreError::InvalidTranscriptRewrite {
            id: head.id.clone(),
            reason: "session head carries a non-canonical rewrite-prefix digest".to_string(),
        });
    }
    let rewrite_prefix_count = head.rewrite_prefix.occurrence_count();
    if rewrite_prefix_count != head.rewrite_count {
        return Err(SessionStoreError::InvalidTranscriptRewrite {
            id: head.id.clone(),
            reason: format!(
                "session head rewrite-prefix authority covers {rewrite_prefix_count} commits but \
                 rewrite_count is {}",
                head.rewrite_count
            ),
        });
    }
    let Some(message_row_prefix) = head.message_row_prefix.as_ref() else {
        return Err(SessionStoreError::InvalidTranscriptRewrite {
            id: head.id.clone(),
            reason: "session head has no exact message-row prefix authority; explicit conversion is required"
                .to_string(),
        });
    };
    if message_row_prefix.row_count() != head.message_count {
        return Err(SessionStoreError::InvalidTranscriptRewrite {
            id: head.id.clone(),
            reason: format!(
                "session head message-row prefix covers {} rows but message_count is {}",
                message_row_prefix.row_count(),
                head.message_count
            ),
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
        validate_session_head_component_roots(stored_head)?;
        validate_session_head_metadata_identity(stored_head)?;
        if stored_head.metadata_identity.is_some() && !session_head_has_component_roots(stored_head)
        {
            return Err(SessionStoreError::Corrupted(stored_head.id.clone()));
        }
        let stored_was_activated =
            stored_head.realtime_event_prefix.is_some() || stored_head.metadata_identity.is_some();
        if stored_was_activated && !session_head_has_component_roots(head) {
            return Err(SessionStoreError::InvalidTranscriptRewrite {
                id: head.id.clone(),
                reason:
                    "session head transition would downgrade activated HeadCanonical authority to a legacy inline representation"
                        .to_string(),
            });
        }
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
    let Some(stored_message_row_prefix) = stored.message_row_prefix.as_ref() else {
        return Err(SessionStoreError::InvalidTranscriptRewrite {
            id: id.clone(),
            reason: "stored head predates exact message-row authority; explicit conversion is required before rewrite"
                .to_string(),
        });
    };
    if stored_message_row_prefix.row_count() != stored.message_count {
        return Err(SessionStoreError::Corrupted(id.clone()));
    }
    let stored_prefix_count = stored.rewrite_prefix.occurrence_count();
    if stored_prefix_count != stored.rewrite_count {
        return Err(SessionStoreError::InvalidTranscriptRewrite {
            id: id.clone(),
            reason: format!(
                "stored rewrite-prefix authority covers {stored_prefix_count} commits but the \
                 stored head generation is {}",
                stored.rewrite_count
            ),
        });
    }
    let rewrite_count = stored.rewrite_count.checked_add(1).ok_or_else(|| {
        SessionStoreError::InvalidTranscriptRewrite {
            id: id.clone(),
            reason: "stored rewrite generation overflow".to_string(),
        }
    })?;
    let message_row_prefix =
        SessionMessageRowPrefixAccumulator::from_messages(&record.revision_body.messages)?;
    Ok(SessionHead {
        id: id.clone(),
        version: stored.version,
        strand: TranscriptStrandId::from_rewrite(&record.commit),
        head_revision: record.commit.revision.clone(),
        message_count: record.commit.messages_after as u64,
        message_row_prefix: Some(message_row_prefix),
        row_lineage_anchor: stored.row_lineage_anchor.clone(),
        rewrite_count,
        rewrite_prefix: stored
            .rewrite_prefix
            .extend(&record.commit)
            .map_err(SessionStoreError::from)?,
        // Record-only rewrite APIs cannot derive the compact edge commitment
        // and therefore cannot authorize a current rewritten head. Built-in
        // stores require the prepared graph-edge route instead.
        graph_prefix: None,
        realtime_event_prefix: stored.realtime_event_prefix.clone(),
        created_at: stored.created_at,
        updated_at: record.commit.committed_at,
        usage: stored.usage.clone(),
        metadata_identity: stored.metadata_identity.clone(),
        metadata: stored.metadata.clone(),
        metadata_projection: stored.metadata_projection.clone(),
    })
}

/// One rewrite edge in a [`StrandLayout`].
#[derive(Debug, Clone)]
pub struct StrandRewriteLayout {
    pub commit: TranscriptRewriteCommit,
    pub serialized_graph_edge: Vec<u8>,
    pub parent_strand: TranscriptStrandId,
    pub parent_base_seq: u64,
    pub serialized_parent_suffix: Vec<Vec<u8>>,
    pub parent_transition: PreparedHeadCanonicalParentTransition,
    pub strand: TranscriptStrandId,
    pub link_splice: StrandSplice,
    pub serialized_replacement: Vec<Vec<u8>>,
}

/// Deterministic strand layout of a session's retained transcript history:
/// the shared pure function behind read-only head synthesis and the one-time
/// blob-to-head-canonical migration.
#[derive(Debug, Clone)]
pub struct StrandLayout {
    /// The graph anchor is the only full historical row vector.
    pub anchor_strand: TranscriptStrandId,
    pub serialized_anchor: Vec<Vec<u8>>,
    /// Adopted rewrites, in commit order (`rewrite_idx` = position).
    pub rewrites: Vec<StrandRewriteLayout>,
    pub tail_base_seq: u64,
    pub serialized_tail: Vec<Vec<u8>>,
    pub head_strand: TranscriptStrandId,
    pub head_len: u64,
}

/// Lay out a session's retained transcript history as append-only strands.
///
/// Root strand → `from_rewrite` chain per adopted commit; rebookkept parents
/// get their own `rebase:` strands from their retained bodies; the live
/// vector must extend the final strand exactly. Frozen historical edges may
/// still materialize their recorded exact splice, but current live-tail
/// divergence fails closed. Pure — shared by read-only head synthesis and the
/// in-transaction migration write.
pub fn strand_layout_for_history(
    session: &Session,
    history: Option<&ValidatedTranscriptHistory>,
) -> Result<StrandLayout, SessionStoreError> {
    let id = session.id();
    let root = TranscriptStrandId::root();
    let Some(history) = history else {
        let serialized_anchor = session
            .messages()
            .iter()
            .map(|message| serde_json::to_vec(message).map_err(SessionStoreError::from))
            .collect::<Result<Vec<_>, _>>()?;
        return Ok(StrandLayout {
            anchor_strand: root.clone(),
            serialized_anchor,
            rewrites: Vec::new(),
            tail_base_seq: u64::try_from(session.messages().len())
                .map_err(|_| SessionStoreError::Corrupted(id.clone()))?,
            serialized_tail: Vec::new(),
            head_strand: root,
            head_len: u64::try_from(session.messages().len())
                .map_err(|_| SessionStoreError::Corrupted(id.clone()))?,
        });
    };
    let state = history.state();
    let serialized_anchor = state
        .anchor()
        .messages()
        .iter()
        .map(|message| serde_json::to_vec(message).map_err(SessionStoreError::from))
        .collect::<Result<Vec<_>, _>>()?;
    let mut current_strand = root.clone();
    let mut current_len = state.anchor().messages().len();
    let mut rewrites = Vec::with_capacity(state.commit_count());
    for edge in state.edges() {
        if edge.messages_before_base() != current_len {
            return Err(SessionStoreError::InvalidTranscriptRewrite {
                id: id.clone(),
                reason: "compact migration edge base count is not contiguous".to_string(),
            });
        }
        let parent_base_seq =
            u64::try_from(current_len).map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
        let mut parent_strand = current_strand.clone();
        let parent_transition =
            if let Some((at, replacement)) = edge.parent_advance().exact_splice() {
                let serialized_replacement = replacement
                    .iter()
                    .map(|message| serde_json::to_vec(message).map_err(SessionStoreError::from))
                    .collect::<Result<Vec<_>, _>>()?;
                let end = at
                    .checked_add(replacement.len())
                    .ok_or_else(|| SessionStoreError::Corrupted(id.clone()))?;
                let bridge = TranscriptStrandId::from_rewrite_parent_occurrence(edge.commit());
                let splice = PreparedHeadCanonicalParentSplice {
                    source_strand: current_strand.clone(),
                    link_splice: StrandSplice {
                        strand_len: parent_base_seq,
                        splice_start: u64::try_from(at)
                            .map_err(|_| SessionStoreError::Corrupted(id.clone()))?,
                        splice_end: u64::try_from(end)
                            .map_err(|_| SessionStoreError::Corrupted(id.clone()))?,
                        successor_end: u64::try_from(end)
                            .map_err(|_| SessionStoreError::Corrupted(id.clone()))?,
                    },
                    serialized_replacement,
                };
                parent_strand = bridge;
                PreparedHeadCanonicalParentTransition::ExactSplice(splice)
            } else {
                PreparedHeadCanonicalParentTransition::ExactAppend
            };
        let serialized_parent_suffix = edge
            .parent_advance()
            .appended()
            .iter()
            .map(|message| serde_json::to_vec(message).map_err(SessionStoreError::from))
            .collect::<Result<Vec<_>, _>>()?;
        let serialized_replacement = edge
            .rewrite()
            .replacement()
            .iter()
            .map(|message| serde_json::to_vec(message).map_err(SessionStoreError::from))
            .collect::<Result<Vec<_>, _>>()?;
        if crate::image_content::messages_have_inline_media(edge.parent_advance().appended())
            || crate::image_content::messages_have_inline_media(edge.rewrite().replacement())
            || edge
                .parent_advance()
                .exact_splice()
                .is_some_and(|(_, replacement)| {
                    crate::image_content::messages_have_inline_media(replacement)
                })
        {
            return Err(SessionStoreError::InvalidTranscriptRewrite {
                id: id.clone(),
                reason: "compact rewrite delta carries inline media".to_string(),
            });
        }
        let (start, end) = edge.commit().selection.bounds();
        let replacement_end = start
            .checked_add(serialized_replacement.len())
            .ok_or_else(|| SessionStoreError::Corrupted(id.clone()))?;
        let strand = TranscriptStrandId::from_rewrite_occurrence(edge.commit());
        let link_splice = StrandSplice {
            strand_len: u64::try_from(edge.messages_after())
                .map_err(|_| SessionStoreError::Corrupted(id.clone()))?,
            splice_start: u64::try_from(start)
                .map_err(|_| SessionStoreError::Corrupted(id.clone()))?,
            splice_end: u64::try_from(replacement_end)
                .map_err(|_| SessionStoreError::Corrupted(id.clone()))?,
            successor_end: u64::try_from(end)
                .map_err(|_| SessionStoreError::Corrupted(id.clone()))?,
        };
        rewrites.push(StrandRewriteLayout {
            commit: edge.commit().clone(),
            serialized_graph_edge: edge.to_replay_bytes().map_err(SessionStoreError::from)?,
            parent_strand,
            parent_base_seq,
            serialized_parent_suffix,
            parent_transition,
            strand: strand.clone(),
            link_splice,
            serialized_replacement,
        });
        current_strand = strand;
        current_len = edge.messages_after();
    }
    if current_len > session.messages().len() {
        return Err(SessionStoreError::Corrupted(id.clone()));
    }
    let current_prefix = session
        .exact_message_row_prefix_at(
            u64::try_from(session.messages().len())
                .map_err(|_| SessionStoreError::Corrupted(id.clone()))?,
        )
        .ok_or_else(|| SessionStoreError::Corrupted(id.clone()))?;
    if !session
        .live_transcript_extends_history_head(state, "")
        .map_err(|error| SessionStoreError::InvalidTranscriptRewrite {
            id: id.clone(),
            reason: format!("failed to prove compact migration live tail: {error}"),
        })?
    {
        return Err(SessionStoreError::InvalidTranscriptRewrite {
            id: id.clone(),
            reason: "live rows do not extend compact migration history".to_string(),
        });
    }
    let endpoint = state.final_endpoint_witness().ok_or_else(|| {
        SessionStoreError::InvalidTranscriptRewrite {
            id: id.clone(),
            reason: "compact migration graph has no endpoint witness".to_string(),
        }
    })?;
    let tail = &session.messages()[current_len..];
    let serialized_tail = tail
        .iter()
        .map(|message| serde_json::to_vec(message).map_err(SessionStoreError::from))
        .collect::<Result<Vec<_>, _>>()?;
    let exact_append = endpoint
        .row_prefix()
        .extend_serialized_rows(&serialized_tail)?;
    let tail_base_seq =
        u64::try_from(current_len).map_err(|_| SessionStoreError::Corrupted(id.clone()))?;
    if exact_append != current_prefix {
        return Err(SessionStoreError::InvalidTranscriptRewrite {
            id: id.clone(),
            reason: "live rows do not exactly append to compact migration history".to_string(),
        });
    }
    Ok(StrandLayout {
        anchor_strand: root,
        serialized_anchor,
        rewrites,
        tail_base_seq,
        serialized_tail,
        head_strand: current_strand,
        head_len: u64::try_from(session.messages().len())
            .map_err(|_| SessionStoreError::Corrupted(id.clone()))?,
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
        let commit = state
            .commit(0)
            .expect("one rewrite mints one compact edge")
            .clone();
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

    /// A first-boundary adoption graph assembled by grafting an unrelated
    /// compact edge must be rejected. Opaque graph construction prevents this
    /// shape in memory; corrupt durable JSON is the adversarial ingress.
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
        let mut spliced = serde_json::to_value(&state_a)?;
        let unrelated = serde_json::to_value(&state_b)?;
        let unrelated_edge = unrelated["edges"]
            .as_array()
            .and_then(|edges| edges.first())
            .cloned()
            .ok_or_else(|| std::io::Error::other("lineage B edge missing"))?;
        spliced["edges"]
            .as_array_mut()
            .ok_or_else(|| std::io::Error::other("lineage A edge array missing"))?
            .push(unrelated_edge);

        // The live transcript comes from the grafted lineage. Strict graph
        // decode/validation must reject the unrelated occurrence rather than
        // treating its content revision as portable authority.
        let mut incoming = lineage_b.clone();
        incoming.set_metadata_unchecked_for_test(
            crate::session::SESSION_TRANSCRIPT_HISTORY_STATE_KEY,
            spliced,
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

    #[test]
    fn append_only_guard_rejects_non_append_message_replacement() {
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
    fn append_only_guard_accepts_an_ordinary_system_append()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut previous = Session::new();
        previous.push(Message::System(SystemMessage::new("base system")));
        previous.push(Message::User(UserMessage::text("hello".to_string())));

        let mut incoming = previous.clone();
        incoming.append_system_message("extra context".to_string());

        assert!(append_only_save_guard(&incoming, Some(&previous)).is_ok());
        Ok(())
    }

    #[test]
    fn append_only_guard_accepts_ordinary_system_append_after_rewrite()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut previous = Session::new();
        previous.push(Message::System(SystemMessage::new("base system")));
        previous.push(Message::User(UserMessage::text("hello".to_string())));
        previous.commit_transcript_rewrite(
            TranscriptRewriteSelection::MessageRange { start: 1, end: 2 },
            vec![Message::User(UserMessage::text(
                "hello, compacted".to_string(),
            ))],
            crate::TranscriptRewriteReason::new("unit-test"),
            Some("unit-test".to_string()),
            None,
        )?;

        let mut incoming = previous.clone();
        incoming.append_system_message("extra context".to_string());

        assert!(append_only_save_guard(&incoming, Some(&previous)).is_ok());
        assert!(run_boundary_snapshot_save_guard(&incoming, Some(&previous)).is_ok());
        Ok(())
    }

    #[test]
    fn append_only_guard_accepts_system_append_without_content_folklore() {
        let mut previous = Session::new();
        previous.push(Message::System(SystemMessage::new("base system")));
        previous.push(Message::User(UserMessage::text("hello".to_string())));

        let mut incoming = previous.clone();
        incoming.append_system_message("arbitrary system content".to_string());

        assert!(append_only_save_guard(&incoming, Some(&previous)).is_ok());
    }

    #[test]
    fn append_only_guard_accepts_system_timestamp_refresh_without_content_change() {
        let mut previous = Session::new();
        previous.push(Message::System(SystemMessage::new("base system")));

        let mut incoming = previous.clone();
        incoming.append_system_message("base system".to_string());

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
        parent.append_system_message("refreshed runtime system projection".to_string());
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
        parent.append_system_message("refreshed runtime system projection".to_string());
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
        incoming.append_system_message("forged replacement system".to_string());
        let incoming_revision = incoming.transcript_revision()?;
        let history = TranscriptHistoryState {
            digest_format: 0,
            head: incoming_revision.clone(),
            commits: Vec::new(),
            parent_transitions: Vec::new(),
            rewrite_prefix: Default::default(),
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
                head: poisoned_revision.clone(),
                commits: Vec::new(),
                parent_transitions: Vec::new(),
                rewrite_prefix: Default::default(),
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
                head: incoming_revision.clone(),
                rewrite_prefix: crate::TranscriptRewritePrefixAccumulator::from_commits(
                    std::slice::from_ref(&commit),
                )
                .expect("rewrite prefix"),
                commits: vec![commit],
                parent_transitions: vec![TranscriptRewriteParentTransition::ExactAppend],
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
        let poisoned_state = first
            .transcript_history_state()?
            .ok_or_else(|| "second rewrite should retain history state".to_string())?;
        let mut poisoned_wire = serde_json::to_value(poisoned_state)?;
        poisoned_wire["edges"][1]["commit"]["revision"] =
            serde_json::Value::String(first_commit.revision.clone());

        let mut poisoned = first_snapshot;
        poisoned.set_metadata_unchecked_for_test(
            crate::session::SESSION_TRANSCRIPT_HISTORY_STATE_KEY,
            poisoned_wire,
        );

        assert!(matches!(
            transcript_rewrite_save_guard(&poisoned, Some(&previous), &first_commit),
            Err(SessionStoreError::InvalidTranscriptRewrite { reason, .. })
                if reason.contains("incoming transcript history state is malformed")
        ));
        Ok(())
    }

    #[test]
    fn transcript_rewrite_guard_rejects_valid_graph_past_supplied_occurrence()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut previous = Session::new();
        previous.push(Message::User(UserMessage::text("persisted".to_string())));
        let parent_revision = previous.transcript_revision()?;

        let mut first = previous.clone();
        let first_commit = first.commit_transcript_rewrite(
            TranscriptRewriteSelection::MessageRange { start: 0, end: 1 },
            vec![Message::User(UserMessage::text(
                "first audited projection".to_string(),
            ))],
            crate::TranscriptRewriteReason::new("first"),
            Some("unit-test".to_string()),
            Some(parent_revision),
        )?;
        let mut incoming = first.clone();

        let mut later = first;
        let second_commit = later.commit_transcript_rewrite(
            TranscriptRewriteSelection::MessageRange { start: 0, end: 1 },
            vec![Message::User(UserMessage::text(
                "later audited projection".to_string(),
            ))],
            crate::TranscriptRewriteReason::new("second"),
            Some("unit-test".to_string()),
            Some(first_commit.revision.clone()),
        )?;
        let later_state = later
            .transcript_history_state()?
            .ok_or_else(|| "second rewrite should retain history state".to_string())?;
        assert_eq!(later_state.head(), second_commit.revision);

        // Keep the live transcript at the first rewrite but substitute the
        // independently valid graph through the second rewrite. The supplied
        // first commit is a member of this graph and its revision still equals
        // the live digest; only the graph's ordered tail exposes the mismatch.
        incoming.set_metadata_unchecked_for_test(
            crate::session::SESSION_TRANSCRIPT_HISTORY_STATE_KEY,
            serde_json::to_value(later_state)?,
        );
        incoming
            .validate_transcript_history_state()
            .expect("trailing-commit graph is independently valid");

        assert!(matches!(
            transcript_rewrite_save_guard(&incoming, Some(&previous), &first_commit),
            Err(SessionStoreError::InvalidTranscriptRewrite { reason, .. })
                if reason.contains("does not end at the supplied audited occurrence")
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
                head: incoming_revision.clone(),
                commits: Vec::new(),
                parent_transitions: Vec::new(),
                rewrite_prefix: Default::default(),
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
        // already carries a typed rewrite graph. The commits are the audit:
        // the run-boundary guard accepts the validated graph, while the plain trait-level
        // `SessionStore::save` contract keeps rejecting first-save seeds.
        let mut incoming = Session::new();
        incoming.push(Message::User(UserMessage::text("old row".to_string())));
        incoming.push(Message::User(UserMessage::text("hello".to_string())));
        incoming.commit_transcript_rewrite(
            crate::TranscriptRewriteSelection::MessageRange { start: 0, end: 1 },
            vec![Message::User(UserMessage::text("new row".to_string()))],
            crate::TranscriptRewriteReason::new("unit-test"),
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
    fn run_boundary_guard_accepts_plain_append_after_head_rewrite()
    -> Result<(), Box<dyn std::error::Error>> {
        // The empty-chain acceptance the cycle-skip exists for: the persisted
        // row is already AT the rewrite revision and the incoming snapshot
        // extends it by ordinary appends.
        let mut previous = Session::new();
        previous.push(Message::User(UserMessage::text("original row".to_string())));
        previous.push(Message::User(UserMessage::text("hello".to_string())));
        previous.commit_transcript_rewrite(
            crate::TranscriptRewriteSelection::MessageRange { start: 0, end: 1 },
            vec![Message::User(UserMessage::text(
                "rewritten row".to_string(),
            ))],
            crate::TranscriptRewriteReason::new("unit-test"),
            None,
            None,
        )?;

        let mut incoming = previous.clone();
        incoming.push(Message::User(UserMessage::text(
            "post-rewrite turn".to_string(),
        )));

        assert!(append_only_save_guard(&incoming, Some(&previous)).is_ok());
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
                head: incoming_revision.clone(),
                commits: Vec::new(),
                parent_transitions: Vec::new(),
                rewrite_prefix: Default::default(),
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
                head: incoming_revision.clone(),
                commits: Vec::new(),
                parent_transitions: Vec::new(),
                rewrite_prefix: Default::default(),
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
                head: incoming_revision.clone(),
                commits: Vec::new(),
                parent_transitions: Vec::new(),
                rewrite_prefix: Default::default(),
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
                head: incoming_revision.clone(),
                commits: Vec::new(),
                parent_transitions: Vec::new(),
                rewrite_prefix: Default::default(),
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
    fn append_only_guard_rejects_new_rewrite_commits_on_system_append()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut previous = Session::new();
        previous.push(Message::System(SystemMessage::new("base system")));
        previous.push(Message::User(UserMessage::text("persisted".to_string())));
        let mut incoming = previous.clone();
        incoming.append_system_message("extra context".to_string());
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
                head: incoming_revision.clone(),
                commits: vec![TranscriptRewriteCommit {
                    rewrite_generation: 1,
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
                parent_transitions: vec![TranscriptRewriteParentTransition::ExactAppend],
                rewrite_prefix: Default::default(),
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
                head: incoming_revision.clone(),
                commits: vec![TranscriptRewriteCommit {
                    rewrite_generation: 1,
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
                parent_transitions: vec![TranscriptRewriteParentTransition::ExactAppend],
                rewrite_prefix: Default::default(),
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
    fn append_only_guard_accepts_tail_background_notice_refresh_after_history()
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
        let audited_graph = previous
            .metadata()
            .get(crate::session::SESSION_TRANSCRIPT_HISTORY_STATE_KEY)
            .cloned()
            .expect("audited graph");

        let mut incoming = previous.clone();
        incoming.replace_synthetic_notices(SystemNoticeKind::BackgroundJob, Vec::new())?;
        incoming.push(Message::User(UserMessage::text("next turn".to_string())));

        append_only_save_guard(&incoming, Some(&previous))?;
        assert_eq!(incoming.transcript_rewrite_generation()?, 1);
        assert_eq!(
            incoming
                .metadata()
                .get(crate::session::SESSION_TRANSCRIPT_HISTORY_STATE_KEY),
            Some(&audited_graph),
            "tail-only notice refresh must not rewrite audited graph metadata"
        );
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
    fn run_boundary_guard_rejects_mutated_compact_edge_metadata()
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
        let state = incoming
            .transcript_history_state()?
            .ok_or_else(|| std::io::Error::other("incoming history missing"))?;
        let mut state_wire = serde_json::to_value(state)?;
        state_wire["edges"][0]["parent_created_at"] =
            serde_json::to_value(crate::time_compat::UNIX_EPOCH)?;
        incoming.set_metadata_unchecked_for_test(
            crate::session::SESSION_TRANSCRIPT_HISTORY_STATE_KEY,
            state_wire,
        );

        assert!(matches!(
            run_boundary_snapshot_save_guard(&incoming, Some(&previous)),
            Err(SessionStoreError::InvalidTranscriptRewrite { .. })
        ));
        Ok(())
    }

    /// Compact graph authority binds exact anchor and edge bytes, including
    /// bookkeeping that the semantic transcript digest deliberately erases.
    /// A caller cannot re-project historical messages under fresh identities
    /// while retaining the old graph-prefix authority.
    #[test]
    fn append_only_guard_rejects_digest_equal_compact_anchor_with_rebuilt_bookkeeping()
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
        let state = incoming
            .transcript_history_state()?
            .ok_or_else(|| std::io::Error::other("incoming history missing"))?;
        let original_anchor = state.anchor().messages().to_vec();
        let mut rebuilt_anchor = original_anchor.clone();
        let mut rebuilt_any = false;
        for message in &mut rebuilt_anchor {
            if let Message::User(user) = message {
                user.identity = user.identity.with_run_id(crate::lifecycle::RunId::new());
                user.created_at = chrono::Utc::now();
                rebuilt_any = true;
            }
        }
        assert!(rebuilt_any, "fixture must rebuild at least one anchor row");
        assert_ne!(original_anchor, rebuilt_anchor);
        assert_eq!(
            transcript_messages_digest(&original_anchor)?,
            transcript_messages_digest(&rebuilt_anchor)?
        );
        let mut state_wire = serde_json::to_value(state)?;
        state_wire["anchor"]["messages"] = serde_json::to_value(&rebuilt_anchor)?;
        incoming.set_metadata_unchecked_for_test(
            crate::session::SESSION_TRANSCRIPT_HISTORY_STATE_KEY,
            state_wire,
        );
        incoming.push(Message::User(UserMessage::text(
            "ordinary append".to_string(),
        )));

        assert!(matches!(
            append_only_save_guard(&incoming, Some(&previous)),
            Err(SessionStoreError::InvalidTranscriptRewrite { .. })
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
            "runtime system before context refresh",
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
    fn run_boundary_guard_rejects_context_summary_that_replaces_ordered_system()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut previous = Session::new();
        previous.push(Message::System(SystemMessage::new("ordered system A")));
        previous.push(Message::User(UserMessage::text("Turn 1 request")));

        let mut incoming = Session::with_id(previous.id().clone());
        incoming.push(Message::System(SystemMessage::new("ordered system B")));
        incoming.push(Message::User(UserMessage::text(
            "Verbose context that will be compacted",
        )));
        incoming.push(previous.messages()[1].clone());
        let parent_revision = incoming.transcript_revision()?;
        incoming.commit_transcript_rewrite(
            TranscriptRewriteSelection::MessageRange { start: 1, end: 2 },
            vec![Message::User(UserMessage::compaction_summary(
                "[Context compacted] Earlier runtime context",
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
            "runtime system before context refresh",
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
            "runtime system before context refresh",
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
            head: parent_revision.clone(),
            commits: Vec::new(),
            parent_transitions: Vec::new(),
            rewrite_prefix: Default::default(),
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
            head: forged_parent_revision.clone(),
            commits: Vec::new(),
            parent_transitions: Vec::new(),
            rewrite_prefix: Default::default(),
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
    fn recurrence_requires_occurrence_authority_for_rewrite_chain_selection()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut session = Session::new();
        session.push(Message::User(UserMessage::text("A".to_string())));
        let initial = session.clone();
        let original_message = session.messages()[0].clone();
        let a = session.transcript_revision()?;

        let first = session.commit_transcript_rewrite(
            TranscriptRewriteSelection::MessageRange { start: 0, end: 1 },
            vec![Message::User(UserMessage::text("B".to_string()))],
            crate::TranscriptRewriteReason::new("recurrence-test"),
            Some("unit-test".to_string()),
            Some(a.clone()),
        )?;
        let after_first = session.clone();
        let second = session.commit_transcript_rewrite(
            TranscriptRewriteSelection::MessageRange { start: 0, end: 1 },
            vec![original_message],
            crate::TranscriptRewriteReason::new("recurrence-test"),
            Some("unit-test".to_string()),
            Some(first.revision.clone()),
        )?;
        assert_eq!(second.revision, a, "the graph must be A -> B -> A");

        let state = session
            .transcript_history_state()?
            .ok_or_else(|| std::io::Error::other("missing recurrence graph"))?;
        assert!(
            find_transcript_rewrite_commit_chain_extending(&state, &a, &a).is_none(),
            "digest-only chain selection must fail closed across recurring A"
        );

        let validated = session
            .validated_transcript_history_state()?
            .ok_or_else(|| std::io::Error::other("missing validated recurrence graph"))?;
        let from_initial = find_transcript_rewrite_commit_chain_extending_session(
            &validated,
            &initial,
            state.head(),
        )?
        .ok_or_else(|| std::io::Error::other("exact anchor should select recurrence chain"))?;
        assert_eq!(
            from_initial
                .iter()
                .map(|commit| commit.rewrite_generation)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );

        let from_first = find_transcript_rewrite_commit_chain_extending_session(
            &validated,
            &after_first,
            state.head(),
        )?
        .ok_or_else(|| std::io::Error::other("exact graph prefix should select recurrence tail"))?;
        assert_eq!(
            from_first
                .iter()
                .map(|commit| commit.rewrite_generation)
                .collect::<Vec<_>>(),
            vec![2]
        );
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
        incoming.commit_transcript_rewrite(
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
        let state = incoming
            .transcript_history_state()?
            .ok_or_else(|| std::io::Error::other("incoming rewrite should retain history"))?;
        let mut state_wire = serde_json::to_value(state)?;
        let edges = state_wire["edges"]
            .as_array_mut()
            .ok_or_else(|| std::io::Error::other("compact graph edge array missing"))?;
        if edges.len() != 2 {
            return Err(std::io::Error::other("fixture expected exactly two compact edges").into());
        }
        edges.remove(0);
        incoming.set_metadata_unchecked_for_test(
            crate::session::SESSION_TRANSCRIPT_HISTORY_STATE_KEY,
            state_wire,
        );

        assert!(matches!(
            run_boundary_snapshot_save_guard(&incoming, Some(&previous)),
            Err(SessionStoreError::InvalidTranscriptRewrite { .. })
        ));
        Ok(())
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
    fn session_head_projection_strips_inline_history_and_round_trips_token() {
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
    }

    #[test]
    #[allow(clippy::expect_used)]
    fn prepared_successor_borrows_then_acknowledges_actor_session() {
        let (mut live, observed) = acknowledged_head_canonical_root_with_metadata();
        live.push(Message::User(UserMessage::text(
            "successor delta".to_string(),
        )));
        let metadata_before_prepare = live.metadata().clone();

        let mutation = PreparedHeadCanonicalMutation::prepare(&live, Some(observed))
            .expect("prepare borrowed successor");
        assert_eq!(
            live.metadata(),
            &metadata_before_prepare,
            "preparation must not install successor continuation state"
        );
        assert!(matches!(
            mutation.acknowledge_session(&mut live, "wrong-head-token"),
            Err(SessionStoreError::TranscriptRevisionConflict { .. })
        ));
        assert_eq!(
            live.metadata(),
            &metadata_before_prepare,
            "failed acknowledgement must leave authority untouched"
        );

        mutation
            .acknowledge_session(&mut live, mutation.successor_head_token())
            .expect("acknowledge exact successor");
        assert_eq!(
            live.exact_message_row_prefix_at(live.messages().len() as u64)
                .as_ref(),
            mutation.successor_head().message_row_prefix.as_ref(),
            "acknowledgement installs the exact durable row-prefix authority"
        );
    }

    #[allow(clippy::expect_used)]
    fn acknowledged_head_canonical_root_with_metadata() -> (Session, SessionHead) {
        let mut session = Session::new();
        session.set_metadata("a", serde_json::json!(1));
        session.set_metadata("z", serde_json::json!(2));
        let mutation = PreparedHeadCanonicalMutation::prepare_root(&session)
            .expect("prepare HeadCanonical root");
        let head = mutation.successor_head().clone();
        mutation
            .acknowledge_session(&mut session, mutation.successor_head_token())
            .expect("acknowledge HeadCanonical root");
        (session, head)
    }

    #[test]
    #[allow(clippy::expect_used)]
    fn intra_turn_acknowledges_exact_physical_prefix_without_domain_authority() {
        let (mut session, runtime_head) = acknowledged_head_canonical_root_with_metadata();
        session.push(Message::User(UserMessage::text(
            "first physical delta".to_string(),
        )));

        let first = PreparedHeadCanonicalMutation::prepare_intra_turn(
            &session,
            &runtime_head,
            runtime_head.clone(),
        )
        .expect("prepare first intra-turn projection");
        let first_head = first.successor_head().clone();
        first
            .acknowledge_physical_projection(&mut session, first.successor_head_token())
            .expect("acknowledge first physical projection");
        assert_eq!(
            session
                .exact_message_row_prefix_at(session.messages().len() as u64)
                .as_ref(),
            first_head.message_row_prefix.as_ref(),
            "physical acknowledgement installs exact row continuation authority"
        );

        session.push(Message::User(UserMessage::text(
            "second physical delta".to_string(),
        )));
        let replacement = PreparedHeadCanonicalMutation::prepare_intra_turn(
            &session,
            &runtime_head,
            first_head.clone(),
        )
        .expect("prepare second intra-turn projection");
        assert_eq!(
            replacement.predecessor_head(),
            Some(&first_head),
            "the second projection must extend the exact observed physical head"
        );
        replacement
            .acknowledge_physical_projection(&mut session, replacement.successor_head_token())
            .expect("acknowledge replacement physical projection");

        let runtime_successor = PreparedHeadCanonicalMutation::prepare_intra_turn(
            &session,
            &runtime_head,
            replacement.successor_head().clone(),
        )
        .expect("prepare exact boundary adoption");
        runtime_successor
            .acknowledge_session(&mut session, runtime_successor.successor_head_token())
            .expect("acknowledge authoritative runtime successor");
        assert_eq!(
            session
                .exact_message_row_prefix_at(session.messages().len() as u64)
                .as_ref(),
            runtime_successor
                .successor_head()
                .message_row_prefix
                .as_ref()
        );
    }

    #[test]
    #[allow(clippy::expect_used)]
    fn rewrite_carrier_keeps_recurrent_revisions_occurrence_unique_and_delta_bounded() {
        let (mut session, runtime_head) = acknowledged_head_canonical_root_with_metadata();

        session.push(Message::User(UserMessage::text("alpha".to_string())));
        let alpha_revision = session.transcript_content_digest().expect("alpha revision");
        let alpha_to_beta = session
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange { start: 0, end: 1 },
                vec![Message::User(UserMessage::text("beta".to_string()))],
                crate::TranscriptRewriteReason::new("test recurrence"),
                Some("unit-test".to_string()),
                None,
            )
            .expect("rewrite alpha to beta");

        let first = PreparedHeadCanonicalRewriteMutation::prepare_intra_turn(
            &session,
            &runtime_head,
            runtime_head.clone(),
        )
        .expect("prepare first specialized rewrite");
        assert_eq!(first.steps().len(), 1);
        assert_eq!(first.steps()[0].parent_base_seq(), 0);
        assert_eq!(first.steps()[0].serialized_parent_suffix().len(), 1);
        assert_eq!(first.steps()[0].serialized_replacement().len(), 1);
        assert!(first.serialized_tail().is_empty());
        let beta_head = first.successor_head().clone();
        let beta_strand = first.steps()[0].strand().clone();
        first
            .acknowledge_physical_projection(&mut session, first.successor_head_token())
            .expect("acknowledge beta physical projection");

        let beta_to_alpha = session
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange { start: 0, end: 1 },
                vec![Message::User(UserMessage::text("alpha".to_string()))],
                crate::TranscriptRewriteReason::new("test recurrence"),
                Some("unit-test".to_string()),
                None,
            )
            .expect("rewrite beta back to alpha");
        assert_eq!(beta_to_alpha.revision, alpha_revision);
        assert_ne!(
            alpha_to_beta.rewrite_generation,
            beta_to_alpha.rewrite_generation
        );

        let second = PreparedHeadCanonicalRewriteMutation::prepare_intra_turn(
            &session,
            &runtime_head,
            beta_head,
        )
        .expect("prepare recurrent specialized rewrite");
        assert_eq!(second.steps().len(), 1);
        assert_eq!(second.steps()[0].parent_strand(), &beta_strand);
        assert_eq!(second.steps()[0].parent_base_seq(), 1);
        assert!(second.steps()[0].serialized_parent_suffix().is_empty());
        assert_eq!(second.steps()[0].serialized_replacement().len(), 1);
        assert_ne!(second.steps()[0].strand(), &beta_strand);
        assert_ne!(second.steps()[0].strand(), &TranscriptStrandId::root());
        assert_eq!(second.successor_head().head_revision, alpha_revision);
        second
            .acknowledge_physical_projection(&mut session, second.successor_head_token())
            .expect("acknowledge recurrent physical projection");

        assert!(
            !PreparedHeadCanonicalRewriteMutation::is_required(&session, second.successor_head())
                .expect("classify fully persisted rewrite prefix")
        );
        let runtime_successor = PreparedHeadCanonicalMutation::prepare_intra_turn(
            &session,
            &runtime_head,
            second.successor_head().clone(),
        )
        .expect("prepare authoritative runtime adoption after physical rewrites");
        assert!(runtime_successor.serialized_suffix().is_empty());
        assert_eq!(
            runtime_successor.predecessor_head(),
            Some(second.successor_head())
        );
    }

    #[test]
    #[allow(clippy::expect_used)]
    fn save_head_requires_metadata_identity_for_activated_component_roots() {
        let (_, mut head) = acknowledged_head_canonical_root_with_metadata();
        head.metadata_identity = None;
        head.metadata_projection = None;

        assert!(matches!(
            validate_save_head_transition(
                &head,
                None,
                &SessionHeadCas::Create,
                head.message_count,
                head.rewrite_count,
            ),
            Err(SessionStoreError::InvalidTranscriptRewrite { reason, .. })
                if reason.contains("no immutable metadata identity")
        ));
    }

    #[test]
    #[allow(clippy::expect_used)]
    fn legacy_save_head_rejects_activated_head_canonical_authority() {
        let (_, head) = acknowledged_head_canonical_root_with_metadata();

        assert!(matches!(
            validate_save_head_transition(
                &head,
                None,
                &SessionHeadCas::Create,
                head.message_count,
                head.rewrite_count,
            ),
            Err(SessionStoreError::InvalidTranscriptRewrite { reason, .. })
                if reason.contains("PreparedHeadCanonicalMutation")
        ));
    }

    #[test]
    #[allow(clippy::expect_used)]
    fn save_head_forbids_downgrade_from_activated_to_legacy_representation() {
        let (_, stored) = acknowledged_head_canonical_root_with_metadata();
        let stored_token = session_head_cas_token(&stored).expect("stored token");
        let mut downgraded = stored.clone();
        downgraded.realtime_event_prefix = None;
        downgraded.metadata_identity = None;
        downgraded.metadata_projection = None;

        assert!(matches!(
            validate_save_head_transition(
                &downgraded,
                Some((&stored, &stored_token)),
                &SessionHeadCas::IfToken(stored_token.clone()),
                downgraded.message_count,
                downgraded.rewrite_count,
            ),
            Err(SessionStoreError::InvalidTranscriptRewrite { reason, .. })
                if reason.contains("downgrade activated HeadCanonical authority")
        ));
    }

    #[test]
    #[allow(clippy::expect_used)]
    fn compact_head_verifies_only_its_exact_hydrated_session() {
        let (session, head) = acknowledged_head_canonical_root_with_metadata();
        let compact: SessionHead =
            serde_json::from_slice(&serde_json::to_vec(&head).expect("serialize compact head"))
                .expect("deserialize compact head");
        assert!(
            compact.metadata_projection().is_none(),
            "ordinary compact head reads must stay unhydrated"
        );

        let verified = compact
            .verify_materialized_session(session)
            .expect("exact hydrated session must verify");
        assert_eq!(
            verified.head().metadata_identity(),
            head.metadata_identity()
        );
        assert_eq!(
            verified.session().metadata().get("a"),
            Some(&serde_json::json!(1))
        );

        let (mut changed, head) = acknowledged_head_canonical_root_with_metadata();
        changed.set_metadata("a", serde_json::json!(9));
        assert!(
            head.verify_materialized_session(changed).is_err(),
            "a session that no longer re-projects to the exact head must fail closed"
        );
    }

    #[test]
    #[allow(clippy::expect_used)]
    fn unchanged_ordinary_successor_seals_zero_cell_mutations_without_canonicalization() {
        let (mut session, observed) = acknowledged_head_canonical_root_with_metadata();
        let observed_identity = observed
            .metadata_identity()
            .expect("root carries metadata identity")
            .clone();
        crate::session::reset_session_head_metadata_canonicalization_count();
        session.push(Message::User(UserMessage::text("delta".to_string())));

        let mutation = PreparedHeadCanonicalMutation::prepare(&session, Some(observed))
            .expect("prepare ordinary successor");
        let successor_projection = mutation
            .successor_head()
            .metadata_projection()
            .expect("successor carries metadata projection");
        assert_eq!(successor_projection.identity(), &observed_identity);
        assert!(successor_projection.mutations().is_empty());
        assert_eq!(
            crate::session::session_head_metadata_canonicalization_count(),
            0,
            "message-only successor must not rebuild metadata"
        );
    }

    #[test]
    #[allow(clippy::expect_used)]
    fn caller_metadata_mutation_rebuilds_projection_exactly_once() {
        let (mut session, observed) = acknowledged_head_canonical_root_with_metadata();
        let previous_identity = observed
            .metadata_identity()
            .expect("root metadata identity")
            .clone();
        crate::session::reset_session_head_metadata_canonicalization_count();
        session.set_metadata("caller", serde_json::json!({"changed": true}));

        let mutation = PreparedHeadCanonicalMutation::prepare(&session, Some(observed))
            .expect("prepare metadata successor");
        assert_ne!(
            mutation
                .successor_head()
                .metadata_identity()
                .expect("successor identity"),
            &previous_identity
        );
        assert_eq!(
            crate::session::session_head_metadata_canonicalization_count(),
            1,
            "one metadata change must build one exact byte-derived projection"
        );
        let _ = session
            .head_canonical_metadata_projection()
            .expect("reuse projection");
        assert_eq!(
            crate::session::session_head_metadata_canonicalization_count(),
            1
        );
    }

    #[test]
    #[allow(clippy::expect_used)]
    fn compact_head_serde_omits_values_and_attach_restores_exact_metadata() {
        let (session, head) = acknowledged_head_canonical_root_with_metadata();
        let snapshot =
            std::sync::Arc::clone(head.metadata_projection().expect("head metadata snapshot"));
        let encoded = serde_json::to_vec(&head).expect("serialize compact head");
        let encoded_text = std::str::from_utf8(&encoded).expect("head JSON");
        assert!(!encoded_text.contains("\"a\":1"));
        assert!(!encoded_text.contains("\"z\":2"));

        let mut decoded: SessionHead =
            serde_json::from_slice(&encoded).expect("deserialize compact head");
        assert!(decoded.metadata_projection().is_none());
        decoded
            .attach_metadata_projection(snapshot)
            .expect("attach exact metadata snapshot");
        assert_eq!(
            decoded
                .materialized_metadata()
                .expect("materialize metadata"),
            session.metadata().clone()
        );
    }

    #[test]
    #[allow(clippy::expect_used)]
    fn metadata_attach_rejects_a_different_authenticated_snapshot() {
        let (_, head) = acknowledged_head_canonical_root_with_metadata();
        let encoded = serde_json::to_vec(&head).expect("serialize compact head");
        let mut compact: SessionHead =
            serde_json::from_slice(&encoded).expect("deserialize compact head");
        let (mut other, _) = acknowledged_head_canonical_root_with_metadata();
        other.set_metadata("a", serde_json::json!(9));
        let mismatch = other
            .head_canonical_metadata_projection()
            .expect("different snapshot");
        assert!(compact.attach_metadata_projection(mismatch).is_err());
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

    /// A same-shape durable-row corruption after a valid materialization must
    /// still fail closed instead of being replaced by process-global content.
    #[test]
    #[allow(clippy::expect_used)]
    fn session_head_rejects_same_shape_corrupt_row_after_valid_materialization() {
        let mut session = Session::new();
        session.push(Message::System(SystemMessage::new("base system")));
        session.push(Message::User(UserMessage::text("hello".to_string())));
        session.push(Message::User(UserMessage::text("world".to_string())));
        let head = SessionHead::from_session(&session, TranscriptStrandId::root(), 0)
            .expect("head projection");
        let valid_rows = session
            .messages()
            .iter()
            .map(|message| serde_json::to_vec(message).expect("serialize durable row"))
            .collect::<Vec<_>>();

        // Warm the formerly process-global fast path with a valid durable
        // materialization of this exact head.
        let loaded = head
            .clone()
            .into_session_from_serialized_rows(valid_rows.clone())
            .expect("valid rows must materialize");
        assert_eq!(loaded.messages(), session.messages());

        // Change one durable row without changing its type, serialized length,
        // row count, or the head row.
        let mut corrupted_rows = valid_rows;
        let corrupt_message = Message::User(UserMessage::text("jello".to_string()));
        let corrupt_row = serde_json::to_vec(&corrupt_message).expect("serialize corrupt row");
        assert_eq!(corrupt_row.len(), corrupted_rows[1].len());
        corrupted_rows[1] = corrupt_row;
        assert_eq!(corrupted_rows.len() as u64, head.message_count);

        assert!(matches!(
            head.into_session_from_serialized_rows(corrupted_rows),
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

        // Every ordinary ordered System append is admitted.
        let mut context_appended = previous.clone();
        context_appended.append_system_message("extra context".to_string());
        head_canonical_plain_save_guard(&context_appended, &previous, &[])
            .expect("ordinary system append admitted");
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
        let history = compacted
            .validated_transcript_history_state()
            .expect("state read")
            .expect("state present");
        let layout = strand_layout_for_history(&compacted, Some(&history)).expect("layout");
        assert_eq!(layout.rewrites.len(), 1);
        assert_eq!(layout.head_strand, layout.rewrites[0].strand);
        assert_eq!(layout.head_len, compacted.messages().len() as u64);
        // Root strand holds the parent body; the rewrite strand holds the
        // revision body extended by the live tail.
        assert_eq!(
            layout.serialized_anchor.len() as u64,
            layout.rewrites[0].parent_base_seq
        );
        assert_eq!(layout.serialized_tail.len(), 1);
        assert_eq!(layout.rewrites[0].commit, commit);
    }

    #[test]
    #[allow(clippy::expect_used)]
    fn strand_layout_rejects_current_non_append_live_tail() {
        let mut session = Session::new();
        session.append_system_message("historical system");
        session.push(Message::User(UserMessage::text("question")));
        session
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange { start: 1, end: 2 },
                vec![Message::User(UserMessage::text("edited question"))],
                crate::TranscriptRewriteReason::new("unit-test"),
                Some("unit-test".to_string()),
                None,
            )
            .expect("seed audited endpoint");
        let history = session
            .validated_transcript_history_state()
            .expect("state read")
            .expect("state present");
        let endpoint = history
            .state()
            .materialize_revision(history.state().head())
            .expect("materialize audited endpoint");

        let mut divergent = Session::with_id(session.id().clone());
        divergent.append_system_message("replacement system");
        for message in endpoint.messages.iter().skip(1) {
            divergent.push(message.clone());
        }
        let endpoint_prefix = history
            .state()
            .final_endpoint_witness()
            .expect("endpoint witness")
            .row_prefix()
            .clone();
        let current_prefix =
            SessionMessageRowPrefixAccumulator::from_messages(divergent.messages())
                .expect("current row prefix");
        assert!(
            divergent.install_exact_message_row_lineage(endpoint_prefix, current_prefix),
            "test fixture must model an externally asserted live-row lineage"
        );

        let error = strand_layout_for_history(&divergent, Some(&history))
            .expect_err("current live-tail divergence must not mint a rebase strand");
        assert!(
            matches!(error, SessionStoreError::InvalidTranscriptRewrite { ref reason, .. }
                if reason.contains("do not exactly append")),
            "unexpected error: {error}"
        );
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
