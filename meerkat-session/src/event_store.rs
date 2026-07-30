//! EventStore trait — append-only event log with monotonic sequence numbers.
//!
//! Gated behind the `session-store` feature.
//!
//! File-backed sequence contract:
//! - Owner: one atomic per-session event-log head binds sequence high-water,
//!   byte length, native log identity, final-row anchor, and optional rewrite
//!   prefix authority. There is no parallel rewrite sidecar/sequence owner.
//! - Bootstrap: a missing/stale head is rebuilt from the canonical JSONL. The
//!   old `.seq` file is a one-time migration hint only; projected
//!   `.rkat/sessions/...` files are never consulted.
//! - Durable ordering: JSONL bytes are flushed and `fsync`ed before the head is
//!   atomically replaced. A crash can leave the head trailing canonical bytes,
//!   never ahead of them, so no durable sequence is reused or overwritten.
//! - Failure: allocation errors abort append; the store never falls back to a
//!   process-local counter or projection checkpoint.

use async_trait::async_trait;
use meerkat_core::event::{AgentEvent, EventEnvelope, EventSourceIdentity};
use meerkat_core::interaction::InteractionId;
use meerkat_core::time_compat::SystemTime;
use meerkat_core::types::SessionId;
use meerkat_core::{
    TranscriptRewriteAuditReceiptBatch, TranscriptRewriteCommit, TranscriptRewritePrefixAccumulator,
};
use serde::{Deserialize, Serialize};
#[cfg(not(target_arch = "wasm32"))]
use sha2::{Digest, Sha256};
use std::collections::HashSet;
#[cfg(not(target_arch = "wasm32"))]
use std::collections::{BTreeMap, HashMap};
#[cfg(not(target_arch = "wasm32"))]
use std::io::SeekFrom;
#[cfg(all(unix, not(target_arch = "wasm32")))]
use std::os::unix::fs::MetadataExt;
#[cfg(not(target_arch = "wasm32"))]
use std::path::{Path, PathBuf};
#[cfg(not(target_arch = "wasm32"))]
use std::sync::Arc;
#[cfg(all(test, not(target_arch = "wasm32")))]
use std::sync::atomic::{AtomicUsize, Ordering};
#[cfg(not(target_arch = "wasm32"))]
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncSeekExt, AsyncWriteExt, BufReader};
#[cfg(not(target_arch = "wasm32"))]
use tokio::sync::Mutex;

fn transcript_rewrite_event_parts(
    event: &AgentEvent,
) -> Option<(&SessionId, &[TranscriptRewriteCommit])> {
    match event {
        AgentEvent::TranscriptRewriteCommitted { session_id, record } => {
            Some((session_id, std::slice::from_ref(&record.commit)))
        }
        AgentEvent::TranscriptRewriteAuditReceiptCommitted {
            session_id,
            receipt,
            ..
        } => Some((session_id, receipt.commits())),
        _ => None,
    }
}

fn transcript_rewrite_receipt_event_parts(
    event: &AgentEvent,
) -> Option<(
    &SessionId,
    &TranscriptRewriteAuditReceiptBatch,
    &Option<String>,
)> {
    let AgentEvent::TranscriptRewriteAuditReceiptCommitted {
        session_id,
        receipt,
        final_assistant_text,
    } = event
    else {
        return None;
    };
    Some((session_id, receipt, final_assistant_text))
}

/// A stored event with sequence metadata and canonical stream-envelope identity.
///
/// The durable log preserves the originating [`EventEnvelope`] identity (typed
/// `source`, `mob_id`, and the original stream `stream_seq`) so replay can
/// rehydrate the real envelope instead of fabricating a session-scoped one. Only
/// `seq` is store-assigned; the remaining identity is carried verbatim from the
/// envelope that produced the event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredEvent {
    /// Monotonically increasing sequence number within a session.
    pub seq: u64,
    /// Schema version for forward compatibility.
    pub schema_version: u32,
    /// When the event was stored.
    pub timestamp: SystemTime,
    /// Canonical typed source identity of the originating stream envelope.
    ///
    /// `serde(default)` exists ONLY so a pre-bump (v1) row — which lacked this
    /// field — still parses far enough to be rejected by the typed
    /// [`EventStoreError::SchemaVersionMismatch`] gate in
    /// [`FileEventStore::read_from`], rather than surfacing an opaque
    /// deserialization error. It is never a substantive fallback: any row
    /// carrying it is fail-closed on the schema-version check before use.
    #[serde(default = "stored_event_legacy_source")]
    pub source: EventSourceIdentity,
    /// Mob the originating envelope belonged to, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mob_id: Option<String>,
    /// Original stream sequence carried by the originating envelope.
    #[serde(default)]
    pub stream_seq: u64,
    /// The event payload.
    pub event: AgentEvent,
}

/// One durable row whose payload has not been parsed.
///
/// Carries only what a caller can act on without the payload: the durable
/// sequence, and the payload's own bytes. The schema-version gate that
/// [`StoredEvent`] decoding applies is applied here too, before a row is
/// handed out — an unparsed payload is not an unchecked one.
#[derive(Debug, Clone)]
pub struct RawStoredEvent {
    /// Monotonically increasing sequence number within a session.
    pub seq: u64,
    /// The event payload, exactly as stored.
    pub event: Box<serde_json::value::RawValue>,
}

/// Store-owned proof of one canonical transcript-rewrite audit prefix.
///
/// This is an output receipt, not a consumer cursor. File offsets, log
/// generations, fingerprints, and boundary anchors remain private to the
/// backend that minted it. The semantic accumulator is independently bound by
/// the sealed transcript graph; matching those two facts is what authorizes a
/// backend-local tail seek.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptRewritePrefixReceipt {
    session_id: SessionId,
    through_log_seq: u64,
    accumulator: TranscriptRewritePrefixAccumulator,
    last_commit: Option<TranscriptRewriteCommit>,
    /// Opaque one-use handle for a backend-private candidate that must not
    /// become skip authority until the consumer has validated and applied the
    /// exact rows. Public/custom receipts do not need one.
    finalization_id: Option<String>,
}

impl TranscriptRewritePrefixReceipt {
    /// Construct a semantic receipt minted at one backend-stable high-water.
    ///
    /// Storage-specific proof remains the implementing store's obligation;
    /// this constructor prevents downstream backends from depending on private
    /// FileEventStore token bytes.
    pub fn new(
        session_id: SessionId,
        through_log_seq: u64,
        accumulator: TranscriptRewritePrefixAccumulator,
        last_commit: Option<TranscriptRewriteCommit>,
    ) -> Result<Self, EventStoreError> {
        validate_accumulator_last_commit(&accumulator, last_commit.as_ref())?;
        Ok(Self {
            session_id,
            through_log_seq,
            accumulator,
            last_commit,
            finalization_id: None,
        })
    }

    /// Session whose canonical event log supplied this receipt.
    #[must_use]
    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    /// Inclusive event-log sequence covered by this receipt.
    #[must_use]
    pub fn through_log_seq(&self) -> u64 {
        self.through_log_seq
    }

    /// Canonical ordered occurrence prefix bound at this high-water.
    #[must_use]
    pub fn accumulator(&self) -> &TranscriptRewritePrefixAccumulator {
        &self.accumulator
    }

    /// Last occurrence fact bound by the receipt, if the prefix is nonempty.
    #[must_use]
    pub fn last_commit(&self) -> Option<&TranscriptRewriteCommit> {
        self.last_commit.as_ref()
    }
}

/// One transcript-rewrite row returned by the combined audit read.
///
/// The wrapper is typed as a rewrite event, while retaining the exact payload
/// bytes so commit-only coverage and full record materialization cannot race
/// through two different reads.
#[derive(Debug)]
pub struct RawTranscriptRewriteEvent {
    seq: u64,
    event: Box<serde_json::value::RawValue>,
}

impl RawTranscriptRewriteEvent {
    /// Validate and wrap exact stored rewrite-event payload bytes.
    pub fn new(seq: u64, event: Box<serde_json::value::RawValue>) -> Result<Self, EventStoreError> {
        match meerkat_core::event::transcript_rewrite_commits_from_payload(&event)
            .map_err(|error| EventStoreError::Serialization(error.to_string()))?
        {
            Some(_) => Ok(Self { seq, event }),
            None => Err(EventStoreError::Store(
                "raw transcript rewrite row carries a different AgentEvent variant".to_string(),
            )),
        }
    }

    /// Durable event-log sequence of this row.
    #[must_use]
    pub fn seq(&self) -> u64 {
        self.seq
    }

    /// Exact stored `AgentEvent` payload bytes.
    #[must_use]
    pub fn event(&self) -> &serde_json::value::RawValue {
        &self.event
    }

    /// Consume the wrapper while preserving the exact stored payload bytes.
    #[must_use]
    pub fn into_event(self) -> Box<serde_json::value::RawValue> {
        self.event
    }
}

/// Rows and high-water returned by one stable transcript-rewrite audit read.
#[derive(Debug)]
pub struct TranscriptRewriteAuditRows {
    session_id: SessionId,
    observed_through_log_seq: u64,
    receipt: Option<TranscriptRewritePrefixReceipt>,
    rewrite_rows: Vec<RawTranscriptRewriteEvent>,
}

impl TranscriptRewriteAuditRows {
    /// Session whose canonical audit log supplied these rows.
    #[must_use]
    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    /// Exact stable event-log high-water, including ordinary rows.
    #[must_use]
    pub fn observed_through_log_seq(&self) -> u64 {
        self.observed_through_log_seq
    }

    /// Prefix receipt at the stable high-water observed by this read.
    ///
    /// Full reconciliation may have no canonical receipt yet (for example
    /// while graph-authorized repair still owes a missing generation).
    #[must_use]
    pub fn receipt(&self) -> Option<&TranscriptRewritePrefixReceipt> {
        self.receipt.as_ref()
    }

    /// Exact rewrite-event rows read after the proved prefix.
    #[must_use]
    pub fn rewrite_rows(&self) -> &[RawTranscriptRewriteEvent] {
        &self.rewrite_rows
    }

    /// Consume the result and transfer its exact rewrite payload rows.
    #[must_use]
    pub fn into_rewrite_rows(self) -> Vec<RawTranscriptRewriteEvent> {
        self.rewrite_rows
    }
}

/// Result scope of one combined store-owned rewrite audit read.
///
/// An authorized tail begins strictly after a backend-private byte/log
/// boundary whose semantic prefix exactly matched the supplied graph
/// accumulator. Full reconciliation starts at the canonical log beginning and
/// carries no skip authorization.
#[derive(Debug)]
pub enum TranscriptRewriteAuditRead {
    /// Backend proved the supplied prefix and returned only its delta.
    AuthorizedTail(TranscriptRewriteAuditRows),
    /// Backend could not prove a prefix and returned all rewrite rows.
    FullReconciliation(TranscriptRewriteAuditRows),
}

impl TranscriptRewriteAuditRead {
    /// Construct a backend-authorized delta after validating its public shape.
    pub fn authorized_tail(
        expected_prefix: &TranscriptRewritePrefixAccumulator,
        expected_last_commit: Option<&TranscriptRewriteCommit>,
        receipt: TranscriptRewritePrefixReceipt,
        rewrite_rows: Vec<RawTranscriptRewriteEvent>,
    ) -> Result<Self, EventStoreError> {
        let session_id = receipt.session_id.clone();
        let observed_through_log_seq = receipt.through_log_seq;
        validate_public_rewrite_audit_rows(&session_id, observed_through_log_seq, &rewrite_rows)?;
        let read = Self::AuthorizedTail(TranscriptRewriteAuditRows {
            session_id,
            observed_through_log_seq,
            receipt: Some(receipt),
            rewrite_rows,
        });
        read.verify_authorized_tail(expected_prefix, expected_last_commit)?;
        Ok(read)
    }

    /// Construct a full reconciliation result after validating session,
    /// high-water, receipt, ordering, and exact rewrite payload shape.
    pub fn full_reconciliation(
        session_id: SessionId,
        observed_through_log_seq: u64,
        receipt: Option<TranscriptRewritePrefixReceipt>,
        rewrite_rows: Vec<RawTranscriptRewriteEvent>,
    ) -> Result<Self, EventStoreError> {
        if receipt.as_ref().is_some_and(|receipt| {
            receipt.session_id != session_id || receipt.through_log_seq != observed_through_log_seq
        }) {
            return Err(EventStoreError::Store(
                "rewrite audit receipt does not bind the result session/high-water".to_string(),
            ));
        }
        validate_public_rewrite_audit_rows(&session_id, observed_through_log_seq, &rewrite_rows)?;
        Ok(Self::FullReconciliation(TranscriptRewriteAuditRows {
            session_id,
            observed_through_log_seq,
            receipt,
            rewrite_rows,
        }))
    }

    /// Independently bind an authorized tail to the caller's checkpoint-proved
    /// start prefix.
    ///
    /// Consumers must call this even for in-tree stores: a trait implementer
    /// is trusted to prove storage position, but an end digest alone cannot
    /// prove that it did not omit a row. This verifier enforces strict
    /// next-generation extension, exact same-generation retries, and an exact
    /// fold to the returned receipt.
    pub fn verify_authorized_tail(
        &self,
        expected_prefix: &TranscriptRewritePrefixAccumulator,
        expected_last_commit: Option<&TranscriptRewriteCommit>,
    ) -> Result<(), EventStoreError> {
        let Self::AuthorizedTail(rows) = self else {
            return Ok(());
        };
        let receipt = rows.receipt.as_ref().ok_or_else(|| {
            EventStoreError::Store("authorized rewrite tail has no end receipt".to_string())
        })?;
        if receipt.session_id != rows.session_id
            || receipt.through_log_seq != rows.observed_through_log_seq
        {
            return Err(EventStoreError::Store(
                "authorized rewrite tail receipt does not bind the result session/high-water"
                    .to_string(),
            ));
        }
        validate_accumulator_last_commit(&receipt.accumulator, receipt.last_commit.as_ref())?;
        validate_public_rewrite_audit_rows(
            &rows.session_id,
            rows.observed_through_log_seq,
            &rows.rewrite_rows,
        )?;
        validate_accumulator_last_commit(expected_prefix, expected_last_commit)?;
        let mut accumulator = expected_prefix.clone();
        let mut last_commit = expected_last_commit.cloned();
        for row in &rows.rewrite_rows {
            let Some((row_session_id, commits)) =
                meerkat_core::event::transcript_rewrite_commits_from_payload(&row.event)
                    .map_err(|error| EventStoreError::Serialization(error.to_string()))?
            else {
                return Err(EventStoreError::Store(
                    "authorized rewrite tail contains a non-rewrite payload".to_string(),
                ));
            };
            if row_session_id != rows.session_id {
                return Err(EventStoreError::Store(format!(
                    "authorized rewrite tail row belongs to session {row_session_id}, expected {}",
                    rows.session_id
                )));
            }
            for commit in commits {
                let current_generation = accumulator.occurrence_count();
                if commit.rewrite_generation == current_generation {
                    if last_commit.as_ref() != Some(&commit) {
                        return Err(EventStoreError::Store(format!(
                            "authorized rewrite tail conflicts with occurrence generation {}",
                            commit.rewrite_generation
                        )));
                    }
                    continue;
                }
                if current_generation.checked_add(1) != Some(commit.rewrite_generation) {
                    return Err(EventStoreError::Store(format!(
                        "authorized rewrite tail jumps from occurrence generation {current_generation} to {}",
                        commit.rewrite_generation
                    )));
                }
                accumulator = accumulator
                    .extend(&commit)
                    .map_err(|error| EventStoreError::Serialization(error.to_string()))?;
                last_commit = Some(commit);
            }
        }
        if accumulator != receipt.accumulator || last_commit != receipt.last_commit {
            return Err(EventStoreError::Store(
                "authorized rewrite tail does not fold exactly to its end receipt".to_string(),
            ));
        }
        Ok(())
    }
}

fn validate_accumulator_last_commit(
    accumulator: &TranscriptRewritePrefixAccumulator,
    last_commit: Option<&TranscriptRewriteCommit>,
) -> Result<(), EventStoreError> {
    match last_commit {
        Some(commit) if commit.rewrite_generation == accumulator.occurrence_count() => Ok(()),
        None if accumulator.occurrence_count() == 0 => Ok(()),
        Some(commit) => Err(EventStoreError::Store(format!(
            "rewrite receipt last generation {} does not equal prefix occurrence count {}",
            commit.rewrite_generation,
            accumulator.occurrence_count()
        ))),
        None => Err(EventStoreError::Store(
            "nonempty rewrite prefix has no last occurrence fact".to_string(),
        )),
    }
}

fn validate_public_rewrite_audit_rows(
    session_id: &SessionId,
    observed_through_log_seq: u64,
    rows: &[RawTranscriptRewriteEvent],
) -> Result<(), EventStoreError> {
    let mut previous_seq = 0_u64;
    for row in rows {
        if row.seq <= previous_seq || row.seq > observed_through_log_seq {
            return Err(EventStoreError::Store(format!(
                "rewrite audit row sequence {} is outside the strict ordered high-water {}",
                row.seq, observed_through_log_seq
            )));
        }
        previous_seq = row.seq;
        let Some((row_session_id, _)) =
            meerkat_core::event::transcript_rewrite_commits_from_payload(&row.event)
                .map_err(|error| EventStoreError::Serialization(error.to_string()))?
        else {
            return Err(EventStoreError::Store(
                "raw transcript rewrite row carries a different AgentEvent variant".to_string(),
            ));
        };
        if row_session_id != *session_id {
            return Err(EventStoreError::Store(format!(
                "rewrite audit row belongs to session {row_session_id}, expected {session_id}"
            )));
        }
    }
    Ok(())
}

/// Semantic expectation supplied to the combined audit read.
///
/// The current variant is O(1) by construction. Only the explicit one-time
/// 0.8.10 variant exposes the already-proved ordered graph vector needed to
/// map generation-zero physical rows without granting those rows order
/// authority.
#[derive(Clone, Copy)]
pub enum TranscriptRewriteAuditExpectation<'a> {
    /// Current checkpoint-bound occurrence prefix.
    Current(&'a TranscriptRewritePrefixAccumulator),
    /// One-time migration of generation-zero 0.8.10 audit rows.
    LegacyGenerationZero {
        /// Prefix derived from the normalized, checkpoint-proved graph vector.
        expected_prefix: &'a TranscriptRewritePrefixAccumulator,
        /// Same graph vector in semantic occurrence order.
        ordered_commits: &'a [TranscriptRewriteCommit],
    },
}

impl TranscriptRewriteAuditExpectation<'_> {
    fn expected_prefix(&self) -> &TranscriptRewritePrefixAccumulator {
        match self {
            Self::Current(prefix) => prefix,
            Self::LegacyGenerationZero {
                expected_prefix, ..
            } => expected_prefix,
        }
    }
}

/// The [`StoredEvent`] fields a raw read needs, with the payload left alone.
#[derive(Deserialize)]
struct RawStoredEventWire {
    seq: u64,
    schema_version: u32,
    event: Box<serde_json::value::RawValue>,
}

/// Compact row shape used while reconstructing the sparse EventStore index.
///
/// The index needs durable sequence, typed source, and at most the small
/// transcript-rewrite commit projection. It must not materialize the two full
/// transcript bodies merely to recover sequence or exact-interaction
/// occupancy.
#[derive(Deserialize)]
struct IndexedStoredEventWire {
    seq: u64,
    schema_version: u32,
    #[serde(default = "stored_event_legacy_source")]
    source: EventSourceIdentity,
    event: Box<serde_json::value::RawValue>,
}

/// Placeholder source used only to let a pre-bump row deserialize so the typed
/// schema-version gate can reject it (see [`StoredEvent::source`]).
fn stored_event_legacy_source() -> EventSourceIdentity {
    EventSourceIdentity::external("legacy-pre-schema-v2")
}

impl StoredEvent {
    /// Rehydrate the canonical [`EventEnvelope`] this row was persisted from.
    ///
    /// This is the inverse of the persist path: it returns the original typed
    /// source/mob_id/stream sequence rather than a fabricated session-scoped
    /// envelope. The envelope `seq` is the original stream sequence; the durable
    /// store sequence is [`StoredEvent::seq`].
    #[must_use]
    pub fn to_envelope(&self) -> EventEnvelope<AgentEvent> {
        EventEnvelope::new_with_source(
            self.source.clone(),
            self.stream_seq,
            self.mob_id.clone(),
            self.event.clone(),
        )
    }
}

/// Result of an exact interaction-terminal append.
///
/// Both variants carry the canonical durable row. A replay deliberately
/// returns the already-stored row (including its original `stream_seq`) rather
/// than reflecting caller-supplied envelope metadata from the retry.
#[derive(Debug, Clone)]
pub enum ExactInteractionAppend {
    /// No row existed for the interaction, so this call durably inserted one.
    Inserted(StoredEvent),
    /// One semantically identical row already existed and was reused.
    Replayed(StoredEvent),
}

/// Result of an exact receipt-only transcript-rewrite append.
///
/// A replay returns the one canonical stored row, so projection recovery can
/// resume from its durable sequence without emitting another audit row.
#[derive(Debug, Clone)]
pub enum ExactTranscriptRewriteReceiptAppend {
    /// No identical receipt row existed, so this call durably inserted one.
    Inserted(StoredEvent),
    /// One identical receipt row already existed and was reused.
    Replayed(StoredEvent),
}

impl ExactTranscriptRewriteReceiptAppend {
    /// The canonical durable row selected by the exact append.
    #[must_use]
    pub fn stored_event(&self) -> &StoredEvent {
        match self {
            Self::Inserted(event) | Self::Replayed(event) => event,
        }
    }

    /// Whether this call added a physical event row.
    #[must_use]
    pub fn inserted(&self) -> bool {
        matches!(self, Self::Inserted(_))
    }
}

/// Maximum number of exact interaction terminals accepted by one durable
/// batch. The bound keeps prevalidation, receipt materialization, and the
/// single-fsync write buffer predictably bounded.
pub const MAX_EXACT_INTERACTION_TERMINAL_BATCH: usize = 256;

impl ExactInteractionAppend {
    /// The canonical durable row selected by the exact append.
    #[must_use]
    pub fn stored_event(&self) -> &StoredEvent {
        match self {
            Self::Inserted(event) | Self::Replayed(event) => event,
        }
    }

    /// Consume the result and return its canonical durable row.
    #[must_use]
    pub fn into_stored_event(self) -> StoredEvent {
        match self {
            Self::Inserted(event) | Self::Replayed(event) => event,
        }
    }
}

fn interaction_terminal_payload_id(event: &AgentEvent) -> Option<InteractionId> {
    match event {
        AgentEvent::InteractionComplete { interaction_id, .. }
        | AgentEvent::InteractionCallbackPending { interaction_id, .. }
        | AgentEvent::InteractionFailed { interaction_id, .. } => Some(*interaction_id),
        _ => None,
    }
}

/// Prevalidate a complete exact-terminal batch before any store lookup or
/// write. In particular, a repeated identity is rejected even when the two
/// payloads are byte-identical: one batch item must map to one receipt.
pub(crate) fn validate_exact_interaction_terminal_batch(
    terminals: &[(InteractionId, EventEnvelope<AgentEvent>)],
) -> Result<(), EventStoreError> {
    if terminals.len() > MAX_EXACT_INTERACTION_TERMINAL_BATCH {
        return Err(EventStoreError::InvalidExactInteractionTerminalBatch {
            reason: format!(
                "batch contains {} terminals, exceeding the maximum of {MAX_EXACT_INTERACTION_TERMINAL_BATCH}",
                terminals.len()
            ),
        });
    }

    let mut identities = HashSet::with_capacity(terminals.len());
    for (interaction_id, envelope) in terminals {
        validate_exact_interaction_terminal(*interaction_id, envelope)?;
        if !identities.insert(*interaction_id) {
            return Err(EventStoreError::InvalidExactInteractionTerminalBatch {
                reason: format!(
                    "interaction {interaction_id} occurs more than once in the same batch"
                ),
            });
        }
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub(crate) enum ExactInteractionOccupancy {
    Empty,
    One(StoredEvent),
    Multiple { first: StoredEvent, count: usize },
}

/// Validate durable occupants for a batch and return the length of its
/// canonical replay prefix.
///
/// Existing rows may form only a prefix. This makes crash recovery converge
/// (replay prefix + append missing suffix) while rejecting arbitrary holes
/// that could otherwise append a lower stream sequence after a later durable
/// terminal. Every replay-prefix row must also be semantically identical and
/// carry a contiguous, non-zero stream sequence.
pub(crate) fn validate_exact_interaction_terminal_replay_prefix(
    session_id: &SessionId,
    terminals: &[(InteractionId, EventEnvelope<AgentEvent>)],
    occupants: &[ExactInteractionOccupancy],
) -> Result<usize, EventStoreError> {
    if occupants.len() != terminals.len() {
        return Err(EventStoreError::Store(format!(
            "exact interaction batch occupancy count {} does not match terminal count {}",
            occupants.len(),
            terminals.len()
        )));
    }

    let mut prefix_len = 0_usize;
    let mut previous_stream_seq: Option<u64> = None;
    let mut observed_missing = false;
    for ((interaction_id, envelope), occupant) in terminals.iter().zip(occupants) {
        match occupant {
            ExactInteractionOccupancy::Empty => observed_missing = true,
            ExactInteractionOccupancy::One(existing) => {
                if observed_missing {
                    return Err(EventStoreError::InvalidExactInteractionTerminalBatch {
                        reason: format!(
                            "durable interaction {interaction_id} appears after a missing batch item; existing rows must form one canonical prefix"
                        ),
                    });
                }
                if existing.mob_id != envelope.mob_id
                    || !interaction_terminal_events_semantically_equal(
                        &existing.event,
                        &envelope.payload,
                    )
                {
                    return Err(EventStoreError::ExactInteractionTerminalConflict {
                        session_id: session_id.clone(),
                        interaction_id: *interaction_id,
                        existing_count: 1,
                        reason: format!(
                            "stored mob/event {:?}/{:?} does not match incoming mob/event {:?}/{:?}",
                            existing.mob_id, existing.event, envelope.mob_id, envelope.payload
                        ),
                    });
                }
                if existing.stream_seq == 0 {
                    return Err(EventStoreError::ExactInteractionTerminalConflict {
                        session_id: session_id.clone(),
                        interaction_id: *interaction_id,
                        existing_count: 1,
                        reason: "stored replay-prefix row has zero stream sequence".to_string(),
                    });
                }
                if let Some(previous) = previous_stream_seq {
                    let expected = previous.checked_add(1).ok_or_else(|| {
                        EventStoreError::InvalidExactInteractionTerminalBatch {
                            reason: "durable replay prefix stream sequence overflow".to_string(),
                        }
                    })?;
                    if existing.stream_seq != expected {
                        return Err(EventStoreError::InvalidExactInteractionTerminalBatch {
                            reason: format!(
                                "durable replay prefix is not stream-contiguous: interaction {interaction_id} has stream sequence {}, expected {expected}",
                                existing.stream_seq
                            ),
                        });
                    }
                }
                previous_stream_seq = Some(existing.stream_seq);
                prefix_len = prefix_len.saturating_add(1);
            }
            ExactInteractionOccupancy::Multiple { first, count } => {
                return Err(EventStoreError::ExactInteractionTerminalConflict {
                    session_id: session_id.clone(),
                    interaction_id: *interaction_id,
                    existing_count: *count,
                    reason: format!(
                        "multiple durable rows already claim the exact interaction identity; first row was {:?}",
                        first.event
                    ),
                });
            }
        }
    }
    Ok(prefix_len)
}

pub(crate) fn interaction_related_envelope_id(
    envelope: &EventEnvelope<AgentEvent>,
) -> Option<InteractionId> {
    match &envelope.source {
        EventSourceIdentity::Interaction { interaction_id } => Some(*interaction_id),
        _ => interaction_terminal_payload_id(&envelope.payload),
    }
}

pub(crate) fn validate_exact_interaction_terminal(
    interaction_id: InteractionId,
    envelope: &EventEnvelope<AgentEvent>,
) -> Result<(), EventStoreError> {
    match &envelope.source {
        EventSourceIdentity::Interaction {
            interaction_id: source_id,
        } if *source_id == interaction_id => {}
        EventSourceIdentity::Interaction {
            interaction_id: source_id,
        } => {
            return Err(EventStoreError::InvalidExactInteractionTerminal {
                interaction_id,
                reason: format!(
                    "envelope source interaction id {source_id} does not match the exact append key"
                ),
            });
        }
        source => {
            return Err(EventStoreError::InvalidExactInteractionTerminal {
                interaction_id,
                reason: format!("envelope source must be Interaction, got {source:?}"),
            });
        }
    }

    let Some(payload_id) = interaction_terminal_payload_id(&envelope.payload) else {
        return Err(EventStoreError::InvalidExactInteractionTerminal {
            interaction_id,
            reason: "payload must be InteractionComplete, InteractionCallbackPending, or InteractionFailed"
                .to_string(),
        });
    };
    if payload_id != interaction_id {
        return Err(EventStoreError::InvalidExactInteractionTerminal {
            interaction_id,
            reason: format!(
                "payload interaction id {payload_id} does not match the exact append key"
            ),
        });
    }
    Ok(())
}

pub(crate) fn interaction_terminal_events_semantically_equal(
    existing: &AgentEvent,
    incoming: &AgentEvent,
) -> bool {
    match (existing, incoming) {
        (
            AgentEvent::InteractionComplete {
                interaction_id: left_id,
                result: left_result,
                structured_output: left_structured,
            },
            AgentEvent::InteractionComplete {
                interaction_id: right_id,
                result: right_result,
                structured_output: right_structured,
            },
        ) => {
            left_id == right_id
                && left_result == right_result
                && left_structured == right_structured
        }
        (
            AgentEvent::InteractionCallbackPending {
                interaction_id: left_id,
                tool_name: left_tool,
                args: left_args,
                pending_tool_calls: left_pending,
            },
            AgentEvent::InteractionCallbackPending {
                interaction_id: right_id,
                tool_name: right_tool,
                args: right_args,
                pending_tool_calls: right_pending,
            },
        ) => {
            left_id == right_id
                && left_tool == right_tool
                && left_args == right_args
                && left_pending == right_pending
        }
        (
            AgentEvent::InteractionFailed {
                interaction_id: left_id,
                reason: left_reason,
            },
            AgentEvent::InteractionFailed {
                interaction_id: right_id,
                reason: right_reason,
            },
        ) => left_id == right_id && left_reason == right_reason,
        _ => false,
    }
}

/// Current schema version for stored events.
///
/// Bumped to `2` when [`StoredEvent`] gained canonical envelope identity
/// (`source`/`mob_id`/`stream_seq`). [`FileEventStore::read_from`] fails closed on
/// any row whose `schema_version` does not match this constant.
pub const EVENT_SCHEMA_VERSION: u32 = 2;

/// Durable marker that a session's detached event projection halted.
///
/// The event log remains the replay authority, but a projection task that
/// observes an append failure has no caller to return to. File-backed stores
/// persist this marker beside the log so restarted services keep failing replay
/// closed instead of forgetting the halt in a process-local map.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventProjectionHaltMarker {
    pub session_id: SessionId,
    pub reason: String,
    pub recorded_at: SystemTime,
}

/// Append-only event log.
///
/// The canonical append surface is [`EventStore::append_envelopes`], which
/// preserves the originating [`EventEnvelope`] identity (typed source, `mob_id`,
/// and the original stream sequence). [`EventStore::append`] is a thin reduction
/// for callers that genuinely produce session-scoped events (a session event IS a
/// session-sourced envelope); it is not a lossy fallback.
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait EventStore: Send + Sync {
    /// Append canonical stream envelopes to the durable log for a session.
    ///
    /// The store assigns durable [`StoredEvent::seq`] values but preserves each
    /// envelope's typed `source`, `mob_id`, and original `seq` (persisted as
    /// [`StoredEvent::stream_seq`]). Returns the durable sequence number of the
    /// last appended event. Interaction-sourced or interaction-terminal
    /// envelopes occupy an exact identity keyspace: implementations must accept
    /// them only as one canonical terminal routed through
    /// [`EventStore::append_interaction_terminal_exact`], and reject mixed or
    /// multi-row batches containing them.
    async fn append_envelopes(
        &self,
        session_id: &SessionId,
        envelopes: &[EventEnvelope<AgentEvent>],
    ) -> Result<u64, EventStoreError>;

    /// Durably append exactly one terminal for `interaction_id`, or replay the
    /// canonical row already stored for that identity.
    ///
    /// Implementations must make the exact-source lookup and append one atomic
    /// operation. Zero rows inserts, one semantically identical terminal
    /// replays, and a mismatching row or duplicate exact-source rows fail
    /// closed. Envelope metadata such as `stream_seq` is not part of replay
    /// equivalence; the returned row is always the canonical stored row.
    async fn append_interaction_terminal_exact(
        &self,
        session_id: &SessionId,
        interaction_id: InteractionId,
        envelope: &EventEnvelope<AgentEvent>,
    ) -> Result<ExactInteractionAppend, EventStoreError> {
        let _ = (session_id, interaction_id, envelope);
        Err(EventStoreError::Store(
            "this EventStore does not implement exact interaction terminal publication".to_string(),
        ))
    }

    /// Durably append or replay an ordered batch of exact interaction
    /// terminals under one store transaction/critical section.
    ///
    /// `stream_seq_floor` is the live session sequencer's last allocated
    /// sequence. Implementations must first validate the complete batch and
    /// every durable occupant. Existing matching rows may form only a
    /// contiguous prefix; missing rows are one suffix stamped strictly after
    /// both that prefix and `stream_seq_floor`. A conflict or non-prefix
    /// partial set must fail before any new row is appended. Results preserve
    /// input order and carry one canonical durable row per item.
    async fn append_interaction_terminals_exact_batch(
        &self,
        session_id: &SessionId,
        stream_seq_floor: u64,
        terminals: &[(InteractionId, EventEnvelope<AgentEvent>)],
    ) -> Result<Vec<ExactInteractionAppend>, EventStoreError> {
        validate_exact_interaction_terminal_batch(terminals)?;
        match terminals {
            [] => Ok(Vec::new()),
            [(interaction_id, envelope)] => {
                let mut canonical = envelope.clone();
                canonical.seq = stream_seq_floor.checked_add(1).ok_or_else(|| {
                    EventStoreError::InvalidExactInteractionTerminalBatch {
                        reason: "session event stream sequence overflow".to_string(),
                    }
                })?;
                self.append_interaction_terminal_exact(session_id, *interaction_id, &canonical)
                    .await
                    .map(|append| vec![append])
            }
            _ => Err(EventStoreError::Store(
                "this EventStore does not implement atomic exact interaction terminal batches"
                    .to_string(),
            )),
        }
    }

    /// Durably append one receipt-only transcript-rewrite batch, or return the
    /// canonical identical row already stored for the exact receipt identity.
    ///
    /// Implementations must make the receipt lookup and optional append one
    /// atomic operation. The final assistant text is a derived, delta-sized
    /// projection fact; a retry with different projection text conflicts.
    async fn append_transcript_rewrite_receipt_exact(
        &self,
        session_id: &SessionId,
        receipt: &TranscriptRewriteAuditReceiptBatch,
        final_assistant_text: Option<&str>,
    ) -> Result<ExactTranscriptRewriteReceiptAppend, EventStoreError>;

    /// Append bare session-scoped events to the log for a session.
    ///
    /// Each event is reduced to a session-sourced envelope before being handed to
    /// [`EventStore::append_envelopes`]. Interaction terminal events therefore
    /// do not belong on this reduction; callers must supply their canonical
    /// interaction source through [`EventStore::append_interaction_terminal_exact`].
    /// Returns the sequence number of the last appended event.
    async fn append(
        &self,
        session_id: &SessionId,
        events: &[AgentEvent],
    ) -> Result<u64, EventStoreError> {
        if events.is_empty() {
            return self.last_seq(session_id).await;
        }
        let envelopes: Vec<EventEnvelope<AgentEvent>> = events
            .iter()
            .map(|event| {
                EventEnvelope::new_with_source(
                    EventSourceIdentity::session(session_id.clone()),
                    0,
                    None,
                    event.clone(),
                )
            })
            .collect();
        self.append_envelopes(session_id, &envelopes).await
    }

    /// Persist a fail-closed marker after detached event projection halts.
    async fn record_projection_halt(
        &self,
        session_id: &SessionId,
        reason: &str,
    ) -> Result<(), EventStoreError>;

    /// Read a durable projection-halt marker.
    async fn projection_halt(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<EventProjectionHaltMarker>, EventStoreError>;

    /// Read events from a given sequence number onward.
    async fn read_from(
        &self,
        session_id: &SessionId,
        from_seq: u64,
    ) -> Result<Vec<StoredEvent>, EventStoreError>;

    /// [`Self::read_from`] with each row's payload left unparsed.
    ///
    /// A transcript-rewrite row carries two FULL transcript bodies, and the
    /// replay's coverage decision reads one short field of it. Materializing
    /// those bodies to reach that field is the dominant cost of an
    /// authoritative load on a session with a long rewrite history, and a
    /// typed read cannot avoid it: by the time the caller holds a
    /// [`StoredEvent`] the bodies are already built.
    ///
    /// `None` means this store has no rawer read than the typed one. The
    /// default returns it, so a store that does not implement this keeps
    /// behaving exactly as it did — the caller falls back to
    /// [`Self::read_from`] and pays what every load used to pay.
    async fn read_raw_from(
        &self,
        session_id: &SessionId,
        from_seq: u64,
    ) -> Result<Option<Vec<RawStoredEvent>>, EventStoreError> {
        let _ = (session_id, from_seq);
        Ok(None)
    }

    /// Read transcript-rewrite audit rows through one store-owned authority
    /// boundary.
    ///
    /// [`TranscriptRewriteAuditExpectation::Current`] is O(1) by type.
    /// [`TranscriptRewriteAuditExpectation::LegacyGenerationZero`] alone may
    /// expose the already-proved graph vector during one full 0.8.10
    /// reconciliation, because semantic occurrence order cannot be
    /// rediscovered from revision reachability. Consumer positions, file
    /// offsets, and log identity never enter this API.
    ///
    /// `None` means this store does not implement the capability. Callers must
    /// then use their existing full typed audit path. The default is
    /// deliberately safe-slow so a custom store cannot accidentally authorize
    /// a skip.
    async fn read_transcript_rewrite_audit(
        &self,
        session_id: &SessionId,
        expectation: TranscriptRewriteAuditExpectation<'_>,
    ) -> Result<Option<TranscriptRewriteAuditRead>, EventStoreError> {
        let _ = (session_id, expectation);
        Ok(None)
    }

    /// Publish backend-private tail/reconciliation authority only after the
    /// consumer has validated the exact returned bodies and successfully
    /// applied their semantic replay.
    ///
    /// The receipt carries no file offset, fingerprint, or log generation.
    /// Implementations that stage a private candidate bind it to the receipt's
    /// opaque one-use handle; unknown/already-finalized receipts are idempotent
    /// no-ops. The safe default has no persisted marker authority to publish.
    async fn finalize_transcript_rewrite_audit(
        &self,
        receipt: &TranscriptRewritePrefixReceipt,
    ) -> Result<(), EventStoreError> {
        let _ = receipt;
        Ok(())
    }

    /// Read at most `max_rows` events from a sequence floor. Production
    /// stores override this to avoid materializing an unbounded backlog;
    /// the default preserves compatibility for small test stores.
    async fn read_from_bounded(
        &self,
        session_id: &SessionId,
        from_seq: u64,
        max_rows: usize,
    ) -> Result<Vec<StoredEvent>, EventStoreError> {
        let mut rows = self.read_from(session_id, from_seq).await?;
        rows.truncate(max_rows);
        Ok(rows)
    }

    /// Compatibility page API used by session surfaces. The default delegates
    /// to the bounded read so production stores have one bounded-query seam.
    async fn read_page(
        &self,
        session_id: &SessionId,
        from_seq: u64,
        limit: usize,
    ) -> Result<Vec<StoredEvent>, EventStoreError> {
        self.read_from_bounded(session_id, from_seq, limit).await
    }

    /// Get the latest sequence number for a session (0 if empty).
    async fn last_seq(&self, session_id: &SessionId) -> Result<u64, EventStoreError>;
}

/// Errors from event store operations.
#[derive(Debug, thiserror::Error)]
pub enum EventStoreError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    Serialization(String),

    #[error("Store error: {0}")]
    Store(String),

    #[error("invalid exact interaction terminal for {interaction_id}: {reason}")]
    InvalidExactInteractionTerminal {
        interaction_id: InteractionId,
        reason: String,
    },

    #[error("invalid exact interaction terminal batch: {reason}")]
    InvalidExactInteractionTerminalBatch { reason: String },

    #[error(
        "exact interaction terminal conflict for session {session_id}, interaction {interaction_id}: found {existing_count} exact-source row(s): {reason}"
    )]
    ExactInteractionTerminalConflict {
        session_id: SessionId,
        interaction_id: InteractionId,
        existing_count: usize,
        reason: String,
    },

    #[error(
        "transcript rewrite generation conflict for session {session_id}, generation {generation}: \
         row {first_seq} and row {conflicting_seq} carry different commit facts"
    )]
    TranscriptRewriteGenerationConflict {
        session_id: SessionId,
        generation: u64,
        first_seq: u64,
        conflicting_seq: u64,
    },

    #[error(
        "exact transcript rewrite receipt conflict for session {session_id}: found {existing_count} exact row(s): {reason}"
    )]
    ExactTranscriptRewriteReceiptConflict {
        session_id: SessionId,
        existing_count: usize,
        reason: String,
    },

    #[error(
        "event log schema version mismatch: stored row has schema_version {found}, \
         runtime expects {expected}; refusing to project an unknown schema"
    )]
    SchemaVersionMismatch { expected: u32, found: u32 },
}

/// Filesystem-backed [`EventStore`] with one JSONL log per session.
///
/// This store is intentionally simple: it is the canonical append-only source
/// for the derived [`crate::projector::SessionProjector`] files, while session
/// snapshots remain owned by `SessionStore`/`RuntimeStore`.
#[derive(Debug, Clone)]
#[cfg(not(target_arch = "wasm32"))]
pub struct FileEventStore {
    root: PathBuf,
    append_lock: Arc<Mutex<()>>,
    index_registry: Arc<Mutex<EventLogIndexRegistry>>,
    pending_rewrite_heads: Arc<Mutex<BTreeMap<String, PendingTranscriptRewriteHead>>>,
    #[cfg(test)]
    decoded_rows: Arc<AtomicUsize>,
}

/// Number of event rows between byte-offset checkpoints. A warmed page read
/// decodes at most this many rows before reaching its requested sequence floor.
#[cfg(not(target_arch = "wasm32"))]
const EVENT_LOG_INDEX_STRIDE: u64 = 64;

/// Bound reconstructable index state independently of the number of sessions
/// a long-lived service has ever touched. Eviction is safe because an in-flight
/// reader retains its own `Arc`; a later lookup simply rebuilds from JSONL.
#[cfg(not(target_arch = "wasm32"))]
const EVENT_LOG_INDEX_CACHE_CAPACITY: usize = 256;

/// Pending audit proofs are reconstructable and safe to evict: an evicted
/// finalization becomes a no-op and the next load performs reconciliation
/// again. Bounding them avoids failed/cancelled loads accumulating one entry
/// per session for process lifetime.
#[cfg(not(target_arch = "wasm32"))]
const PENDING_REWRITE_HEAD_CAPACITY: usize = 256;

/// Prefix/suffix bytes sampled from the final indexed row when validating a
/// fingerprint hit. This catches replaced tails without making a very large
/// event row an unbounded per-page read.
#[cfg(not(target_arch = "wasm32"))]
const EVENT_LOG_ANCHOR_SAMPLE_BYTES: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg(not(target_arch = "wasm32"))]
struct EventLogFingerprint {
    len: u64,
    modified: Option<SystemTime>,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(unix)]
    ctime_seconds: i64,
    #[cfg(unix)]
    ctime_nanoseconds: i64,
}

#[derive(Debug, Clone, Copy)]
#[cfg(not(target_arch = "wasm32"))]
struct EventLogCheckpoint {
    seq: u64,
    byte_offset: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[cfg(not(target_arch = "wasm32"))]
struct EventLogLineAnchor {
    byte_offset: u64,
    byte_len: u64,
    prefix_hash: u64,
    suffix_hash: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg(not(target_arch = "wasm32"))]
struct DurableEventLogHeadBody {
    schema_version: u32,
    session_id: SessionId,
    /// Exact event-log sequence reached after the covered append fsync.
    through_log_seq: u64,
    /// Exact event-log byte length reached after that same fsync.
    covered_log_len: u64,
    /// Native file identity/timestamps at the covered boundary. An unchanged
    /// length must match this exactly; a longer cooperative append is instead
    /// validated through the preserved boundary-row anchor below.
    covered_log_fingerprint: EventLogFingerprint,
    /// Last row ending at `covered_log_len`; absent only for an empty log.
    last_line: Option<EventLogLineAnchor>,
    /// Sequence of the last semantic rewrite occurrence in the prefix.
    /// Every requested sequence from here through `through_log_seq` therefore
    /// names the same canonical rewrite prefix.
    last_distinct_rewrite_seq: u64,
    /// Last semantic occurrence generation bound by `rewrite_prefix`.
    last_rewrite_generation: u64,
    /// Full last occurrence fact. This lets a direct-tail reader classify an
    /// exact retry of the boundary generation without retaining the historical
    /// commit vector or accepting digest-only equality.
    last_rewrite_commit: Option<TranscriptRewriteCommit>,
    /// Checkpoint-comparable semantic prefix. Backend-local log identity and
    /// offsets never escape this event-log head.
    rewrite_prefix: Option<TranscriptRewritePrefixAccumulator>,
    /// This boundary was produced by matching generation-zero 0.8.10 rows
    /// against the already-proved ordered graph vector, never by physical row
    /// order or revision reachability.
    legacy_generation_zero_normalized: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg(not(target_arch = "wasm32"))]
struct DurableEventLogHead {
    body: DurableEventLogHeadBody,
    /// SHA-256 over the exact serialized body with a domain separator.
    ///
    /// The canonical JSONL remains authority. This checksum rejects torn or
    /// independently-corrupted heads before their bounded log-tail binding
    /// is considered.
    checksum: String,
}

#[cfg(not(target_arch = "wasm32"))]
const EVENT_LOG_HEAD_SCHEMA_VERSION: u32 = 1;

/// Validated sparse projection of one canonical JSONL log.
///
/// This is deliberately reconstructable cache state, never a second event
/// authority. Only an append performed under this store's shared append lock
/// and durable sequence lock may extend it mechanically. Any independently
/// observed file change rebuilds from byte zero and re-runs schema/sequence
/// validation. On Unix, warm reuse additionally requires the full native file
/// identity/timestamp fingerprint and a bounded final-row sample; non-Unix
/// platforms conservatively rebuild because they lack the same identity proof.
///
/// This is a cooperative-writer contract, not a defense against a privileged
/// process that mutates bytes while spoofing filesystem identity metadata
/// between the before/after checks. The JSONL owner remains append-only, and
/// refresh rejects a file that changes observably while it is being scanned.
#[derive(Debug, Default)]
#[cfg(not(target_arch = "wasm32"))]
struct EventLogIndex {
    checkpoints: Vec<EventLogCheckpoint>,
    row_count: u64,
    last_seq: u64,
    fingerprint: Option<EventLogFingerprint>,
    last_line: Option<EventLogLineAnchor>,
    /// Reconstructable exact-source occupancy index. Every durable row whose
    /// typed source is `Interaction(id)` occupies that identity, even when its
    /// payload is nonterminal/corrupt. Keeping only the first row's compact
    /// byte locator plus a count lets exact appends distinguish zero/one/many
    /// in O(1) memory; the one replayed row is sought and decoded on demand.
    exact_interaction_occupants: HashMap<InteractionId, ExactInteractionOccupant>,
    /// Exact receipt-only rows keyed by the canonical serialized receipt
    /// (without the derived summary projection).
    transcript_rewrite_receipt_occupants: HashMap<Vec<u8>, ExactTranscriptRewriteReceiptOccupant>,
    /// Current rewrite occurrences keyed by serialized generation.
    ///
    /// Physical row order is deliberately irrelevant: late audit repair may
    /// append an older occurrence after a newer one. Same-generation retries
    /// collapse only when every commit fact is equal; a conflicting fact is
    /// corruption.
    transcript_rewrite_commits: BTreeMap<u64, SequencedTranscriptRewriteCommit>,
    /// Generation-zero rows written by 0.8.10. Their physical
    /// order has no semantic authority. A one-time reconciliation maps them
    /// against the already-proved graph vector under 0.8.10 equality semantics
    /// before a normalized event-log head may be minted.
    legacy_transcript_rewrite_commits: Vec<SequencedTranscriptRewriteCommit>,
    /// O(1)-extendable current occurrence prefix. Missing generations or
    /// unresolved legacy rows leave this absent and force full reconciliation.
    transcript_rewrite_prefix: Option<TranscriptRewritePrefixEvidence>,
}

#[derive(Debug, Clone)]
#[cfg(not(target_arch = "wasm32"))]
struct ExactInteractionOccupant {
    first: EventLogRowLocator,
    count: usize,
}

#[derive(Debug, Clone)]
#[cfg(not(target_arch = "wasm32"))]
struct ExactTranscriptRewriteReceiptOccupant {
    first_seq: u64,
    count: usize,
    final_assistant_text: Option<String>,
}

#[derive(Debug, Clone, Copy)]
#[cfg(not(target_arch = "wasm32"))]
struct EventLogRowLocator {
    seq: u64,
    byte_offset: u64,
    byte_len: u64,
}

#[derive(Debug, Clone)]
#[cfg(not(target_arch = "wasm32"))]
struct ResolvedExactInteractionOccupant {
    first: StoredEvent,
    count: usize,
}

#[derive(Debug, Clone)]
#[cfg(not(target_arch = "wasm32"))]
struct SequencedTranscriptRewriteCommit {
    seq: u64,
    commit: TranscriptRewriteCommit,
}

#[derive(Debug, Clone)]
#[cfg(not(target_arch = "wasm32"))]
struct TranscriptRewritePrefixEvidence {
    accumulator: TranscriptRewritePrefixAccumulator,
    last_generation: u64,
    last_distinct_rewrite_seq: u64,
    last_commit: Option<TranscriptRewriteCommit>,
}

/// Immutable O(1)-size view captured while the shared index mutex is held.
/// Page callers receive only their chosen checkpoint, never a clone of the
/// full sparse checkpoint vector; warmed `last_seq` therefore remains O(1).
#[derive(Debug, Clone, Copy)]
#[cfg(not(target_arch = "wasm32"))]
struct EventLogIndexSnapshot {
    row_count: u64,
    last_seq: u64,
    fingerprint: Option<EventLogFingerprint>,
    byte_offset: Option<u64>,
}

#[derive(Debug)]
#[cfg(not(target_arch = "wasm32"))]
struct EventLogIndexRegistryEntry {
    index: Arc<Mutex<EventLogIndex>>,
    last_access: u64,
}

#[derive(Debug, Default)]
#[cfg(not(target_arch = "wasm32"))]
struct EventLogIndexRegistry {
    entries: BTreeMap<String, EventLogIndexRegistryEntry>,
    access_clock: u64,
}

#[derive(Debug, Clone, Copy)]
#[cfg(not(target_arch = "wasm32"))]
struct AppendedIndexRow {
    seq: u64,
    relative_offset: u64,
    byte_len: u64,
}

#[cfg(not(target_arch = "wasm32"))]
struct EventLogAppend<'a> {
    session_id: &'a SessionId,
    pre_fingerprint: EventLogFingerprint,
    post_fingerprint: EventLogFingerprint,
    bytes: &'a [u8],
    rows: &'a [AppendedIndexRow],
    stored_events: &'a [StoredEvent],
}

#[cfg(not(target_arch = "wasm32"))]
struct ReconciledEventLogHead {
    session_id: SessionId,
    fingerprint: EventLogFingerprint,
    through_log_seq: u64,
    last_line: Option<EventLogLineAnchor>,
    last_distinct_rewrite_seq: u64,
    legacy_generation_zero_normalized: bool,
}

#[derive(Debug)]
#[cfg(not(target_arch = "wasm32"))]
struct FullTranscriptRewriteAuditScan {
    fingerprint: Option<EventLogFingerprint>,
    last_line: Option<EventLogLineAnchor>,
    observed_through_log_seq: u64,
    index: EventLogIndex,
    rewrite_rows: Vec<RawTranscriptRewriteEvent>,
}

#[derive(Debug, Clone)]
#[cfg(not(target_arch = "wasm32"))]
struct PendingTranscriptRewriteHead {
    receipt: TranscriptRewritePrefixReceipt,
    body: DurableEventLogHeadBody,
}

#[cfg(not(target_arch = "wasm32"))]
impl EventLogIndex {
    fn note_exact_interaction_occupant(
        &mut self,
        source: &EventSourceIdentity,
        seq: u64,
        byte_offset: u64,
        byte_len: u64,
    ) {
        let EventSourceIdentity::Interaction { interaction_id } = source else {
            return;
        };
        self.exact_interaction_occupants
            .entry(*interaction_id)
            .and_modify(|occupant| occupant.count = occupant.count.saturating_add(1))
            .or_insert_with(|| ExactInteractionOccupant {
                first: EventLogRowLocator {
                    seq,
                    byte_offset,
                    byte_len,
                },
                count: 1,
            });
    }

    fn note_transcript_rewrite_commit(
        &mut self,
        session_id: &SessionId,
        event: &StoredEvent,
    ) -> Result<(), EventStoreError> {
        let Some((event_session_id, commits)) = transcript_rewrite_event_parts(&event.event) else {
            return Ok(());
        };
        if event_session_id != session_id {
            return Err(EventStoreError::Store(format!(
                "transcript rewrite event for session {event_session_id} is stored in session {session_id}'s log"
            )));
        }
        if let Some((_, receipt, final_assistant_text)) =
            transcript_rewrite_receipt_event_parts(&event.event)
        {
            let identity = serde_json::to_vec(receipt)
                .map_err(|error| EventStoreError::Serialization(error.to_string()))?;
            match self.transcript_rewrite_receipt_occupants.get_mut(&identity) {
                Some(occupant) if occupant.final_assistant_text == *final_assistant_text => {
                    occupant.count = occupant.count.saturating_add(1);
                }
                Some(occupant) => {
                    return Err(EventStoreError::ExactTranscriptRewriteReceiptConflict {
                        session_id: session_id.clone(),
                        existing_count: occupant.count,
                        reason: format!(
                            "row {} carries a different terminal assistant-text projection",
                            occupant.first_seq
                        ),
                    });
                }
                None => {
                    self.transcript_rewrite_receipt_occupants.insert(
                        identity,
                        ExactTranscriptRewriteReceiptOccupant {
                            first_seq: event.seq,
                            count: 1,
                            final_assistant_text: final_assistant_text.clone(),
                        },
                    );
                }
            }
        }
        for commit in commits {
            self.note_transcript_rewrite_occurrence(session_id, event.seq, commit)?;
        }
        Ok(())
    }

    fn note_transcript_rewrite_payload(
        &mut self,
        session_id: &SessionId,
        seq: u64,
        payload: &serde_json::value::RawValue,
    ) -> Result<(), EventStoreError> {
        let decoded = meerkat_core::event::transcript_rewrite_commits_from_payload(payload)
            .map_err(|error| EventStoreError::Serialization(error.to_string()))?;
        let Some((event_session_id, commits)) = decoded else {
            return Ok(());
        };
        if event_session_id != *session_id {
            return Err(EventStoreError::Store(format!(
                "transcript rewrite event for session {event_session_id} is stored in session {session_id}'s log"
            )));
        }
        if let Some((receipt_session_id, receipt, final_assistant_text)) =
            meerkat_core::event::transcript_rewrite_audit_receipt_from_payload(payload)
                .map_err(|error| EventStoreError::Serialization(error.to_string()))?
        {
            if receipt_session_id != *session_id {
                return Err(EventStoreError::Store(format!(
                    "transcript rewrite receipt for session {receipt_session_id} is stored in session {session_id}'s log"
                )));
            }
            let identity = serde_json::to_vec(&receipt)
                .map_err(|error| EventStoreError::Serialization(error.to_string()))?;
            match self.transcript_rewrite_receipt_occupants.get_mut(&identity) {
                Some(occupant) if occupant.final_assistant_text == final_assistant_text => {
                    occupant.count = occupant.count.saturating_add(1);
                }
                Some(occupant) => {
                    return Err(EventStoreError::ExactTranscriptRewriteReceiptConflict {
                        session_id: session_id.clone(),
                        existing_count: occupant.count,
                        reason: format!(
                            "row {} carries a different terminal assistant-text projection",
                            occupant.first_seq
                        ),
                    });
                }
                None => {
                    self.transcript_rewrite_receipt_occupants.insert(
                        identity,
                        ExactTranscriptRewriteReceiptOccupant {
                            first_seq: seq,
                            count: 1,
                            final_assistant_text,
                        },
                    );
                }
            }
        }
        for commit in &commits {
            self.note_transcript_rewrite_occurrence(session_id, seq, commit)?;
        }
        Ok(())
    }

    fn note_transcript_rewrite_occurrence(
        &mut self,
        session_id: &SessionId,
        seq: u64,
        commit: &TranscriptRewriteCommit,
    ) -> Result<(), EventStoreError> {
        if commit.rewrite_generation == 0 {
            // Retain physical multiplicity. A proved 0.8.10 graph may carry
            // byte-equal semantic occurrences; one physical row cannot prove
            // both. Counts beyond graph multiplicity are retries.
            self.legacy_transcript_rewrite_commits
                .push(SequencedTranscriptRewriteCommit {
                    seq,
                    commit: commit.clone(),
                });
            self.transcript_rewrite_prefix = None;
            return Ok(());
        }

        if let Some(existing) = self
            .transcript_rewrite_commits
            .get(&commit.rewrite_generation)
        {
            if existing.commit == *commit {
                // A retry reuses the occurrence generation and every fact.
                return Ok(());
            }
            return Err(EventStoreError::TranscriptRewriteGenerationConflict {
                session_id: session_id.clone(),
                generation: commit.rewrite_generation,
                first_seq: existing.seq,
                conflicting_seq: seq,
            });
        }

        let can_extend = self.legacy_transcript_rewrite_commits.is_empty()
            && self
                .transcript_rewrite_prefix
                .as_ref()
                .is_some_and(|prefix| {
                    prefix.last_generation.checked_add(1) == Some(commit.rewrite_generation)
                });
        self.transcript_rewrite_commits.insert(
            commit.rewrite_generation,
            SequencedTranscriptRewriteCommit {
                seq,
                commit: commit.clone(),
            },
        );

        if can_extend {
            let Some(prefix) = self.transcript_rewrite_prefix.as_mut() else {
                return Err(EventStoreError::Store(
                    "event-log index lost its transcript rewrite prefix while extending it"
                        .to_string(),
                ));
            };
            prefix.accumulator = prefix
                .accumulator
                .extend(commit)
                .map_err(|error| EventStoreError::Serialization(error.to_string()))?;
            prefix.last_generation = commit.rewrite_generation;
            prefix.last_distinct_rewrite_seq = seq;
            prefix.last_commit = Some(commit.clone());
        } else {
            self.rebuild_current_transcript_rewrite_prefix()?;
        }
        Ok(())
    }

    fn rebuild_current_transcript_rewrite_prefix(&mut self) -> Result<(), EventStoreError> {
        self.transcript_rewrite_prefix = None;
        if !self.legacy_transcript_rewrite_commits.is_empty() {
            return Ok(());
        }
        let mut accumulator = TranscriptRewritePrefixAccumulator::empty();
        let mut expected_generation = 1_u64;
        let mut last_distinct_rewrite_seq = 0_u64;
        for (&generation, row) in &self.transcript_rewrite_commits {
            if generation != expected_generation {
                // Missing audit occurrences are recoverable by the session
                // graph. They deny a prefix receipt but do not make the whole
                // event log unreadable.
                return Ok(());
            }
            accumulator = accumulator
                .extend(&row.commit)
                .map_err(|error| EventStoreError::Serialization(error.to_string()))?;
            last_distinct_rewrite_seq = last_distinct_rewrite_seq.max(row.seq);
            expected_generation = expected_generation.checked_add(1).ok_or_else(|| {
                EventStoreError::Store(
                    "transcript rewrite generation overflow while rebuilding prefix".to_string(),
                )
            })?;
        }
        self.transcript_rewrite_prefix = Some(TranscriptRewritePrefixEvidence {
            accumulator,
            last_generation: expected_generation.saturating_sub(1),
            last_distinct_rewrite_seq,
            last_commit: self
                .transcript_rewrite_commits
                .last_key_value()
                .map(|(_, row)| row.commit.clone()),
        });
        Ok(())
    }

    fn current_transcript_rewrite_prefix_receipt(
        &self,
        session_id: &SessionId,
        through_log_seq: u64,
    ) -> Option<TranscriptRewritePrefixReceipt> {
        match self.transcript_rewrite_prefix.as_ref() {
            Some(evidence) if evidence.last_distinct_rewrite_seq <= through_log_seq => {
                Some(TranscriptRewritePrefixReceipt {
                    session_id: session_id.clone(),
                    through_log_seq,
                    accumulator: evidence.accumulator.clone(),
                    last_commit: evidence.last_commit.clone(),
                    finalization_id: None,
                })
            }
            None if self.transcript_rewrite_commits.is_empty()
                && self.legacy_transcript_rewrite_commits.is_empty() =>
            {
                Some(TranscriptRewritePrefixReceipt {
                    session_id: session_id.clone(),
                    through_log_seq,
                    accumulator: TranscriptRewritePrefixAccumulator::empty(),
                    last_commit: None,
                    finalization_id: None,
                })
            }
            _ => None,
        }
    }

    fn legacy_prefix_reconciled_by_expected_graph(
        &self,
        session_id: &SessionId,
        through_log_seq: u64,
        expected_prefix: &TranscriptRewritePrefixAccumulator,
        expected_commits: &[TranscriptRewriteCommit],
        rewrite_rows: &[RawTranscriptRewriteEvent],
    ) -> Result<Option<TranscriptRewritePrefixReceipt>, EventStoreError> {
        if self.legacy_transcript_rewrite_commits.is_empty() {
            return Ok(None);
        }
        let rebuilt = TranscriptRewritePrefixAccumulator::from_commits(expected_commits)
            .map_err(|error| EventStoreError::Serialization(error.to_string()))?;
        if &rebuilt != expected_prefix {
            return Ok(None);
        }
        for (index, expected) in expected_commits.iter().enumerate() {
            let expected_generation = u64::try_from(index)
                .ok()
                .and_then(|index| index.checked_add(1))
                .ok_or_else(|| {
                    EventStoreError::Store(
                        "expected transcript rewrite vector exceeds u64 generations".to_string(),
                    )
                })?;
            if expected.rewrite_generation != expected_generation {
                return Ok(None);
            }
        }

        if self
            .transcript_rewrite_commits
            .iter()
            .any(|(&generation, row)| {
                let Ok(index) = usize::try_from(generation.saturating_sub(1)) else {
                    return true;
                };
                expected_commits.get(index) != Some(&row.commit)
            })
        {
            // An explicit current generation is occurrence identity. A
            // matching generation-zero fact must never mask a conflicting
            // fact under that generation.
            return Ok(None);
        }

        let mut allowed_legacy_counts = HashMap::<Vec<u8>, u64>::new();
        let mut required_legacy_counts = HashMap::<Vec<u8>, u64>::new();
        for expected in expected_commits {
            let mut legacy_fact = expected.clone();
            legacy_fact.rewrite_generation = 0;
            let identity = serde_json::to_vec(&legacy_fact)
                .map_err(|error| EventStoreError::Serialization(error.to_string()))?;
            *allowed_legacy_counts.entry(identity.clone()).or_default() += 1;
            if !self
                .transcript_rewrite_commits
                .contains_key(&expected.rewrite_generation)
            {
                *required_legacy_counts.entry(identity).or_default() += 1;
            }
        }
        // The commit-only index deliberately does not materialize rewrite
        // bodies, so it also cannot perform body-authorized compatibility
        // healing. A legacy graph has already applied those same heals while
        // becoming checkpoint proof. Decode only generation-zero rows here so
        // comparison uses the exact post-heal commit the consumer will later
        // validate from these same payload bytes. The resulting head remains a
        // private candidate until that validation and replay succeed.
        let mut physical_legacy_counts = HashMap::<Vec<u8>, u64>::new();
        let mut healed_legacy_count = 0_usize;
        for row in rewrite_rows {
            let Some((_, indexed_commits)) =
                meerkat_core::event::transcript_rewrite_commits_from_payload(&row.event)
                    .map_err(|error| EventStoreError::Serialization(error.to_string()))?
            else {
                return Err(EventStoreError::Store(
                    "full rewrite reconciliation contains a non-rewrite payload".to_string(),
                ));
            };
            let [indexed_commit] = indexed_commits.as_slice() else {
                if indexed_commits
                    .iter()
                    .any(|commit| commit.rewrite_generation == 0)
                {
                    return Err(EventStoreError::Store(
                        "generation-zero compatibility evidence must be one released singleton row"
                            .to_string(),
                    ));
                }
                continue;
            };
            if indexed_commit.rewrite_generation != 0 {
                continue;
            }
            let event: AgentEvent = serde_json::from_str(row.event.get())
                .map_err(|error| EventStoreError::Serialization(error.to_string()))?;
            let AgentEvent::TranscriptRewriteCommitted {
                session_id: event_session_id,
                record,
            } = event
            else {
                return Err(EventStoreError::Store(
                    "full rewrite reconciliation contains a non-rewrite payload".to_string(),
                ));
            };
            if event_session_id != *session_id {
                return Err(EventStoreError::Store(format!(
                    "transcript rewrite event for session {event_session_id} is stored in session {session_id}'s log"
                )));
            }
            if record.commit.rewrite_generation != 0 {
                return Err(EventStoreError::Store(
                    "generation-zero rewrite changed occurrence identity during compatibility decode"
                        .to_string(),
                ));
            }
            healed_legacy_count = healed_legacy_count.saturating_add(1);
            let identity = serde_json::to_vec(&record.commit)
                .map_err(|error| EventStoreError::Serialization(error.to_string()))?;
            if !allowed_legacy_counts.contains_key(&identity) {
                // A generation-zero fact foreign to the sealed graph cannot
                // be hidden behind a migration marker.
                return Ok(None);
            }
            *physical_legacy_counts.entry(identity).or_default() += 1;
        }
        if healed_legacy_count != self.legacy_transcript_rewrite_commits.len() {
            return Err(EventStoreError::Store(
                "full rewrite reconciliation did not retain every indexed generation-zero row"
                    .to_string(),
            ));
        }
        if required_legacy_counts.iter().any(|(identity, required)| {
            physical_legacy_counts.get(identity).copied().unwrap_or(0) < *required
        }) {
            // The caller's existing audit-repair lane may append these missing
            // occurrences with current generation identity. Until it does, no
            // skip authority is minted. Extra physical rows are old retries.
            return Ok(None);
        }

        Ok(Some(TranscriptRewritePrefixReceipt {
            session_id: session_id.clone(),
            through_log_seq,
            accumulator: expected_prefix.clone(),
            last_commit: expected_commits.last().cloned(),
            finalization_id: None,
        }))
    }
}

#[cfg(not(target_arch = "wasm32"))]
struct SequenceAllocationLock {
    _lock: std::fs::File,
}

#[cfg(not(target_arch = "wasm32"))]
impl FileEventStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            append_lock: Arc::new(Mutex::new(())),
            index_registry: Arc::new(Mutex::new(EventLogIndexRegistry::default())),
            pending_rewrite_heads: Arc::new(Mutex::new(BTreeMap::new())),
            #[cfg(test)]
            decoded_rows: Arc::new(AtomicUsize::new(0)),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn log_path(&self, session_id: &SessionId) -> PathBuf {
        self.root.join(format!("{session_id}.jsonl"))
    }

    fn event_log_head_dir(&self) -> PathBuf {
        self.root.join(".event-log-head")
    }

    fn event_log_head_path(&self, session_id: &SessionId) -> PathBuf {
        self.event_log_head_dir().join(format!("{session_id}.json"))
    }

    fn durable_event_log_head_checksum(
        body: &DurableEventLogHeadBody,
    ) -> Result<String, EventStoreError> {
        let bytes = serde_json::to_vec(body)
            .map_err(|error| EventStoreError::Serialization(error.to_string()))?;
        let mut hasher = Sha256::new();
        hasher.update(b"meerkat.file-event-store.event-log-head.v1\0");
        hasher.update(bytes);
        Ok(format!("sha256:{:x}", hasher.finalize()))
    }

    async fn read_durable_event_log_head(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<DurableEventLogHeadBody>, EventStoreError> {
        let head_path = self.event_log_head_path(session_id);
        let bytes = match tokio::fs::read(&head_path).await {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(EventStoreError::Io(error)),
        };
        let head: DurableEventLogHead = match serde_json::from_slice(&bytes) {
            Ok(head) => head,
            Err(_) => return Ok(None),
        };
        let expected_checksum = Self::durable_event_log_head_checksum(&head.body)?;
        if head.checksum != expected_checksum
            || head.body.schema_version != EVENT_LOG_HEAD_SCHEMA_VERSION
            || head.body.session_id != *session_id
            || head.body.last_distinct_rewrite_seq > head.body.through_log_seq
            || match &head.body.rewrite_prefix {
                Some(prefix) => {
                    head.body.last_rewrite_generation != prefix.occurrence_count()
                        || match &head.body.last_rewrite_commit {
                            Some(commit) => {
                                commit.rewrite_generation != head.body.last_rewrite_generation
                            }
                            None => head.body.last_rewrite_generation != 0,
                        }
                }
                None => {
                    head.body.last_rewrite_generation != 0
                        || head.body.last_rewrite_commit.is_some()
                }
            }
        {
            return Ok(None);
        }
        Ok(Some(head.body))
    }

    #[cfg(test)]
    async fn read_exact_event_log_head(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<DurableEventLogHeadBody>, EventStoreError> {
        let Some(head) = self.read_durable_event_log_head(session_id).await? else {
            return Ok(None);
        };
        let path = self.log_path(session_id);
        let mut file = match tokio::fs::File::open(&path).await {
            Ok(file) => file,
            Err(error)
                if error.kind() == std::io::ErrorKind::NotFound && head.covered_log_len == 0 =>
            {
                return Ok(Some(head));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(EventStoreError::Io(error)),
        };
        let before = Self::event_log_fingerprint_from_metadata(&file.metadata().await?);
        if before != head.covered_log_fingerprint || before.len != head.covered_log_len {
            return Ok(None);
        }
        match head.last_line {
            Some(anchor)
                if anchor.byte_offset.checked_add(anchor.byte_len)
                    == Some(head.covered_log_len)
                    && Self::tail_anchor_matches(&mut file, anchor).await? => {}
            None if head.covered_log_len == 0 && head.through_log_seq == 0 => {}
            _ => return Ok(None),
        }
        let after = Self::event_log_fingerprint_from_metadata(&file.metadata().await?);
        Ok((after == before).then_some(head))
    }

    /// Validate the durable head and advance it in memory across a stable
    /// append-only suffix. Callers that are about to append persist the final
    /// advanced head once after their own JSONL fsync.
    async fn read_current_event_log_head(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<DurableEventLogHeadBody>, EventStoreError> {
        let Some(mut head) = self.read_durable_event_log_head(session_id).await? else {
            return Ok(None);
        };
        let path = self.log_path(session_id);
        let mut file = match tokio::fs::File::open(&path).await {
            Ok(file) => file,
            Err(error)
                if error.kind() == std::io::ErrorKind::NotFound && head.covered_log_len == 0 =>
            {
                return Ok(Some(head));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(EventStoreError::Io(error)),
        };
        let target = Self::event_log_fingerprint_from_metadata(&file.metadata().await?);
        if target.len < head.covered_log_len {
            return Ok(None);
        }
        if target.len == head.covered_log_len && target != head.covered_log_fingerprint {
            return Ok(None);
        }
        #[cfg(unix)]
        if target.len > head.covered_log_len
            && (target.device != head.covered_log_fingerprint.device
                || target.inode != head.covered_log_fingerprint.inode)
        {
            return Ok(None);
        }
        #[cfg(not(unix))]
        if target.len > head.covered_log_len {
            return Ok(None);
        }
        match head.last_line {
            Some(anchor)
                if anchor.byte_offset.checked_add(anchor.byte_len)
                    == Some(head.covered_log_len)
                    && Self::tail_anchor_matches(&mut file, anchor).await? => {}
            None if head.covered_log_len == 0 && head.through_log_seq == 0 => {}
            _ => return Ok(None),
        }
        let after_anchor = Self::event_log_fingerprint_from_metadata(&file.metadata().await?);
        if after_anchor != target {
            return Ok(None);
        }
        if target.len == head.covered_log_len {
            return Ok(Some(head));
        }

        file.seek(SeekFrom::Start(head.covered_log_len)).await?;
        let mut lines =
            BufReader::new((&mut file).take(target.len.saturating_sub(head.covered_log_len)));
        let mut line = String::new();
        let mut offset = head.covered_log_len;
        let mut observed_seq = head.through_log_seq;
        while offset < target.len {
            line.clear();
            let bytes_read = lines.read_line(&mut line).await?;
            if bytes_read == 0 || !line.ends_with('\n') {
                return Err(EventStoreError::Store(format!(
                    "event log '{}' has a torn row after durable head byte {}",
                    path.display(),
                    head.covered_log_len
                )));
            }
            let byte_len = u64::try_from(bytes_read).map_err(|_| {
                EventStoreError::Store(format!(
                    "event log '{}' contains an address-unrepresentable row",
                    path.display()
                ))
            })?;
            let line_start = offset;
            offset = offset.checked_add(byte_len).ok_or_else(|| {
                EventStoreError::Store(format!(
                    "event log '{}' byte offset overflow",
                    path.display()
                ))
            })?;
            let row = self.decode_raw_event_line(&line)?;
            if row.seq <= observed_seq {
                return Err(EventStoreError::Store(format!(
                    "event log '{}' sequence {} is not strictly greater than durable head {}",
                    path.display(),
                    row.seq,
                    observed_seq
                )));
            }
            observed_seq = row.seq;
            head.last_line = Some(Self::event_log_line_anchor(
                line_start,
                byte_len,
                line.as_bytes(),
            ));
            let Some((raw_rewrite, commits)) = Self::transcript_rewrite_row(session_id, row)?
            else {
                continue;
            };
            let Some(_) = head.rewrite_prefix.as_ref() else {
                continue;
            };
            if let Some((_, receipt, _)) =
                meerkat_core::event::transcript_rewrite_audit_receipt_from_payload(
                    &raw_rewrite.event,
                )
                .map_err(|error| EventStoreError::Serialization(error.to_string()))?
            {
                let current_prefix = head.rewrite_prefix.as_ref().ok_or_else(|| {
                    EventStoreError::Store(
                        "rewrite-prefix authority disappeared while applying a receipt".to_string(),
                    )
                })?;
                if receipt.end_prefix() == current_prefix {
                    if head.last_rewrite_commit.as_ref() != receipt.commits().last() {
                        return Err(EventStoreError::TranscriptRewriteGenerationConflict {
                            session_id: session_id.clone(),
                            generation: head.last_rewrite_generation,
                            first_seq: head.last_distinct_rewrite_seq,
                            conflicting_seq: raw_rewrite.seq,
                        });
                    }
                    continue;
                }
                if receipt.start_prefix() == current_prefix {
                    let last = receipt.commits().last().cloned().ok_or_else(|| {
                        EventStoreError::Store(
                            "validated transcript rewrite receipt is empty".to_string(),
                        )
                    })?;
                    head.rewrite_prefix = Some(receipt.end_prefix().clone());
                    head.last_rewrite_generation = last.rewrite_generation;
                    head.last_rewrite_commit = Some(last);
                    head.last_distinct_rewrite_seq = raw_rewrite.seq;
                    continue;
                }
            }
            if let [commit] = commits.as_slice()
                && commit.rewrite_generation == head.last_rewrite_generation
                && head.last_rewrite_commit.as_ref() != Some(commit)
            {
                return Err(EventStoreError::TranscriptRewriteGenerationConflict {
                    session_id: session_id.clone(),
                    generation: commit.rewrite_generation,
                    first_seq: head.last_distinct_rewrite_seq,
                    conflicting_seq: raw_rewrite.seq,
                });
            }
            // This suffix was recovered from disk, not from the current
            // receipt path. Released singleton bodies still require one-time
            // compatibility reconciliation before they can advance semantic
            // skip authority.
            head.rewrite_prefix = None;
            head.last_rewrite_generation = 0;
            head.last_rewrite_commit = None;
            head.legacy_generation_zero_normalized = false;
        }
        drop(lines);
        let after = Self::event_log_fingerprint_from_metadata(&file.metadata().await?);
        if after != target {
            return Ok(None);
        }
        head.through_log_seq = observed_seq;
        head.covered_log_len = target.len;
        head.covered_log_fingerprint = target;
        Ok(Some(head))
    }

    async fn write_event_log_head(
        &self,
        session_id: &SessionId,
        body: DurableEventLogHeadBody,
    ) -> Result<(), EventStoreError> {
        let head = DurableEventLogHead {
            checksum: Self::durable_event_log_head_checksum(&body)?,
            body,
        };
        let bytes = serde_json::to_vec(&head)
            .map_err(|error| EventStoreError::Serialization(error.to_string()))?;
        let directory = self.event_log_head_dir();
        tokio::fs::create_dir_all(&directory).await?;
        let destination = self.event_log_head_path(session_id);
        let temporary = directory.join(format!(
            ".{session_id}.tmp.{}",
            meerkat_core::time_compat::new_uuid_v7()
        ));
        let result = async {
            let mut file = tokio::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)
                .await?;
            file.write_all(&bytes).await?;
            file.flush().await?;
            file.sync_all().await?;
            drop(file);
            tokio::fs::rename(&temporary, &destination).await?;
            let sync_directory = directory.clone();
            tokio::task::spawn_blocking(move || -> Result<(), std::io::Error> {
                std::fs::File::open(sync_directory)?.sync_all()
            })
            .await
            .map_err(|error| {
                EventStoreError::Store(format!(
                    "event-log head directory sync task failed: {error}"
                ))
            })??;
            Ok::<(), EventStoreError>(())
        }
        .await;
        if result.is_err() {
            let _ = tokio::fs::remove_file(&temporary).await;
        }
        result
    }

    /// Build one fixed-size durable prefix snapshot from the validated index.
    ///
    /// The caller holds the durable per-session sequence lock. `fingerprint`
    /// and `through_log_seq` are the post-fsync event-log state; a concurrent
    /// reader may have rebuilt the same state, but no writer can advance it.
    async fn persist_event_log_head_from_index(
        &self,
        session_id: &SessionId,
        fingerprint: EventLogFingerprint,
        through_log_seq: u64,
        trusted_empty_base: bool,
    ) -> Result<(), EventStoreError> {
        let shared = self.event_log_index(session_id).await;
        let body = {
            let index = shared.lock().await;
            if index.fingerprint != Some(fingerprint) || index.last_seq != through_log_seq {
                return Ok(());
            }
            let receipt =
                index.current_transcript_rewrite_prefix_receipt(session_id, through_log_seq);
            let evidence = index.transcript_rewrite_prefix.as_ref();
            let last_distinct_rewrite_seq = evidence
                .map(|evidence| evidence.last_distinct_rewrite_seq)
                .unwrap_or_else(|| {
                    index
                        .legacy_transcript_rewrite_commits
                        .iter()
                        .chain(index.transcript_rewrite_commits.values())
                        .map(|row| row.seq)
                        .max()
                        .unwrap_or(0)
                });
            let (last_rewrite_generation, last_rewrite_commit, rewrite_prefix) =
                if trusted_empty_base {
                    match (receipt, evidence) {
                        (Some(receipt), Some(evidence)) => (
                            receipt.accumulator.occurrence_count(),
                            evidence.last_commit.clone(),
                            Some(receipt.accumulator),
                        ),
                        (Some(receipt), None) if receipt.accumulator.occurrence_count() == 0 => {
                            (0, None, Some(receipt.accumulator))
                        }
                        _ => (0, None, None),
                    }
                } else if index.transcript_rewrite_commits.is_empty()
                    && index.legacy_transcript_rewrite_commits.is_empty()
                {
                    // A rebuilt index may publish positional high-water, but
                    // rows read from disk have not passed exact-body replay.
                    // Only the provably empty rewrite prefix is safe before a
                    // consumer-staged receipt is finalized.
                    (0, None, Some(TranscriptRewritePrefixAccumulator::empty()))
                } else {
                    (0, None, None)
                };
            DurableEventLogHeadBody {
                schema_version: EVENT_LOG_HEAD_SCHEMA_VERSION,
                session_id: session_id.clone(),
                through_log_seq,
                covered_log_len: fingerprint.len,
                covered_log_fingerprint: fingerprint,
                last_line: index.last_line,
                last_distinct_rewrite_seq,
                last_rewrite_generation,
                last_rewrite_commit,
                rewrite_prefix,
                legacy_generation_zero_normalized: false,
            }
        };
        self.write_event_log_head(session_id, body).await
    }

    async fn stage_event_log_head(
        &self,
        mut receipt: TranscriptRewritePrefixReceipt,
        body: DurableEventLogHeadBody,
    ) -> Result<TranscriptRewritePrefixReceipt, EventStoreError> {
        if receipt.session_id != body.session_id
            || receipt.through_log_seq != body.through_log_seq
            || body.rewrite_prefix.as_ref() != Some(&receipt.accumulator)
            || body.last_rewrite_commit != receipt.last_commit
            || body.last_rewrite_generation != receipt.accumulator.occurrence_count()
        {
            return Err(EventStoreError::Store(
                "cannot stage an event-log head that disagrees with its rewrite receipt"
                    .to_string(),
            ));
        }
        let finalization_id = meerkat_core::time_compat::new_uuid_v7().to_string();
        receipt.finalization_id = Some(finalization_id.clone());
        let mut pending = self.pending_rewrite_heads.lock().await;
        if pending.len() == PENDING_REWRITE_HEAD_CAPACITY
            && let Some(evicted) = pending.keys().next().cloned()
        {
            pending.remove(&evicted);
        }
        pending.insert(
            finalization_id,
            PendingTranscriptRewriteHead {
                receipt: receipt.clone(),
                body,
            },
        );
        Ok(receipt)
    }

    async fn stage_reconciled_event_log_head(
        &self,
        reconciled: ReconciledEventLogHead,
        receipt: TranscriptRewritePrefixReceipt,
    ) -> Result<TranscriptRewritePrefixReceipt, EventStoreError> {
        let last_rewrite_generation = receipt.accumulator.occurrence_count();
        let body = DurableEventLogHeadBody {
            schema_version: EVENT_LOG_HEAD_SCHEMA_VERSION,
            session_id: reconciled.session_id,
            through_log_seq: reconciled.through_log_seq,
            covered_log_len: reconciled.fingerprint.len,
            covered_log_fingerprint: reconciled.fingerprint,
            last_line: reconciled.last_line,
            last_distinct_rewrite_seq: reconciled.last_distinct_rewrite_seq,
            last_rewrite_generation,
            last_rewrite_commit: receipt.last_commit.clone(),
            rewrite_prefix: Some(receipt.accumulator.clone()),
            legacy_generation_zero_normalized: reconciled.legacy_generation_zero_normalized,
        };
        self.stage_event_log_head(receipt, body).await
    }

    async fn finalize_pending_rewrite_head(
        &self,
        receipt: &TranscriptRewritePrefixReceipt,
    ) -> Result<(), EventStoreError> {
        let Some(finalization_id) = receipt.finalization_id.as_ref() else {
            return Ok(());
        };
        let candidate = {
            let pending = self.pending_rewrite_heads.lock().await;
            let Some(candidate) = pending.get(finalization_id) else {
                // Already finalized or safely evicted.
                return Ok(());
            };
            if candidate.receipt != *receipt {
                return Err(EventStoreError::Store(
                    "rewrite audit finalization receipt does not bind its private candidate"
                        .to_string(),
                ));
            }
            candidate.clone()
        };

        let session_id = &candidate.body.session_id;
        let _sequence_lock = self.acquire_sequence_lock(session_id).await?;
        let path = self.log_path(session_id);
        let current = match tokio::fs::metadata(&path).await {
            Ok(metadata) => Self::event_log_fingerprint_from_metadata(&metadata),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                self.pending_rewrite_heads
                    .lock()
                    .await
                    .remove(finalization_id);
                return Ok(());
            }
            Err(error) => return Err(EventStoreError::Io(error)),
        };
        if current != candidate.body.covered_log_fingerprint {
            self.pending_rewrite_heads
                .lock()
                .await
                .remove(finalization_id);
            return Ok(());
        }

        self.write_event_log_head(session_id, candidate.body.clone())
            .await?;
        let after = match tokio::fs::metadata(&path).await {
            Ok(metadata) => Self::event_log_fingerprint_from_metadata(&metadata),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                self.pending_rewrite_heads
                    .lock()
                    .await
                    .remove(finalization_id);
                return Ok(());
            }
            Err(error) => return Err(EventStoreError::Io(error)),
        };
        if after == candidate.body.covered_log_fingerprint {
            self.pending_rewrite_heads
                .lock()
                .await
                .remove(finalization_id);
        }
        Ok(())
    }

    async fn event_log_index(&self, session_id: &SessionId) -> Arc<Mutex<EventLogIndex>> {
        let key = session_id.to_string();
        let mut registry = self.index_registry.lock().await;
        let access = registry.access_clock;
        registry.access_clock = registry.access_clock.saturating_add(1);
        if let Some(entry) = registry.entries.get_mut(&key) {
            entry.last_access = access;
            return Arc::clone(&entry.index);
        }
        if registry.entries.len() == EVENT_LOG_INDEX_CACHE_CAPACITY
            && let Some(evicted) = registry
                .entries
                .iter()
                .min_by(|(left_key, left), (right_key, right)| {
                    left.last_access
                        .cmp(&right.last_access)
                        .then_with(|| left_key.cmp(right_key))
                })
                .map(|(key, _)| key.clone())
        {
            registry.entries.remove(&evicted);
        }
        let index = Arc::new(Mutex::new(EventLogIndex::default()));
        registry.entries.insert(
            key,
            EventLogIndexRegistryEntry {
                index: Arc::clone(&index),
                last_access: access,
            },
        );
        index
    }

    fn event_log_index_snapshot(
        index: &EventLogIndex,
        from_seq: Option<u64>,
    ) -> EventLogIndexSnapshot {
        let byte_offset = from_seq.and_then(|from_seq| {
            if index.row_count == 0 {
                return None;
            }
            let checkpoint_index = index
                .checkpoints
                .partition_point(|checkpoint| checkpoint.seq <= from_seq)
                .saturating_sub(1);
            index
                .checkpoints
                .get(checkpoint_index)
                .map(|checkpoint| checkpoint.byte_offset)
        });
        EventLogIndexSnapshot {
            row_count: index.row_count,
            last_seq: index.last_seq,
            fingerprint: index.fingerprint,
            byte_offset,
        }
    }

    #[cfg(test)]
    async fn event_log_fingerprint(
        path: &Path,
    ) -> Result<Option<EventLogFingerprint>, EventStoreError> {
        let metadata = match tokio::fs::metadata(path).await {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(EventStoreError::Io(error)),
        };
        Ok(Some(Self::event_log_fingerprint_from_metadata(&metadata)))
    }

    fn event_log_fingerprint_from_metadata(metadata: &std::fs::Metadata) -> EventLogFingerprint {
        EventLogFingerprint {
            len: metadata.len(),
            modified: metadata.modified().ok(),
            #[cfg(unix)]
            device: metadata.dev(),
            #[cfg(unix)]
            inode: metadata.ino(),
            #[cfg(unix)]
            ctime_seconds: metadata.ctime(),
            #[cfg(unix)]
            ctime_nanoseconds: metadata.ctime_nsec(),
        }
    }

    #[cfg(unix)]
    fn cached_fingerprint_is_reusable(
        cached: Option<EventLogFingerprint>,
        observed: EventLogFingerprint,
    ) -> bool {
        cached == Some(observed)
    }

    #[cfg(not(unix))]
    fn cached_fingerprint_is_reusable(
        _cached: Option<EventLogFingerprint>,
        _observed: EventLogFingerprint,
    ) -> bool {
        false
    }

    fn event_log_line_hash(bytes: &[u8]) -> u64 {
        // Deterministic FNV-1a. This is a bounded tail-corruption check on an
        // otherwise exact native-fingerprint cache hit, not a cryptographic
        // authenticity primitive (the canonical JSONL remains the authority).
        let mut hash = 0xcbf2_9ce4_8422_2325_u64;
        for byte in bytes {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        hash
    }

    fn event_log_line_anchor(byte_offset: u64, byte_len: u64, bytes: &[u8]) -> EventLogLineAnchor {
        let sample_len = bytes.len().min(EVENT_LOG_ANCHOR_SAMPLE_BYTES);
        EventLogLineAnchor {
            byte_offset,
            byte_len,
            prefix_hash: Self::event_log_line_hash(&bytes[..sample_len]),
            suffix_hash: Self::event_log_line_hash(&bytes[bytes.len() - sample_len..]),
        }
    }

    fn decode_event_line(&self, line: &str) -> Result<StoredEvent, EventStoreError> {
        #[cfg(test)]
        self.decoded_rows.fetch_add(1, Ordering::Relaxed);
        let event: StoredEvent = serde_json::from_str(line)
            .map_err(|error| EventStoreError::Serialization(error.to_string()))?;
        if event.schema_version != EVENT_SCHEMA_VERSION {
            return Err(EventStoreError::SchemaVersionMismatch {
                expected: EVENT_SCHEMA_VERSION,
                found: event.schema_version,
            });
        }
        Ok(event)
    }

    /// [`Self::decode_event_line`] without building the payload.
    ///
    /// Applies the same schema-version gate on the same field; only the
    /// payload is left as stored bytes.
    fn decode_raw_event_line(&self, line: &str) -> Result<RawStoredEvent, EventStoreError> {
        #[cfg(test)]
        self.decoded_rows.fetch_add(1, Ordering::Relaxed);
        let wire: RawStoredEventWire = serde_json::from_str(line)
            .map_err(|error| EventStoreError::Serialization(error.to_string()))?;
        if wire.schema_version != EVENT_SCHEMA_VERSION {
            return Err(EventStoreError::SchemaVersionMismatch {
                expected: EVENT_SCHEMA_VERSION,
                found: wire.schema_version,
            });
        }
        Ok(RawStoredEvent {
            seq: wire.seq,
            event: wire.event,
        })
    }

    /// Decode only the row metadata retained by the sparse index.
    fn decode_index_event_line(
        &self,
        line: &str,
    ) -> Result<IndexedStoredEventWire, EventStoreError> {
        #[cfg(test)]
        self.decoded_rows.fetch_add(1, Ordering::Relaxed);
        let wire: IndexedStoredEventWire = serde_json::from_str(line)
            .map_err(|error| EventStoreError::Serialization(error.to_string()))?;
        if wire.schema_version != EVENT_SCHEMA_VERSION {
            return Err(EventStoreError::SchemaVersionMismatch {
                expected: EVENT_SCHEMA_VERSION,
                found: wire.schema_version,
            });
        }
        Ok(wire)
    }

    async fn tail_anchor_matches(
        file: &mut tokio::fs::File,
        anchor: EventLogLineAnchor,
    ) -> Result<bool, EventStoreError> {
        file.seek(SeekFrom::Start(anchor.byte_offset)).await?;
        let Ok(byte_len) = usize::try_from(anchor.byte_len) else {
            return Ok(false);
        };
        let sample_len = byte_len.min(EVENT_LOG_ANCHOR_SAMPLE_BYTES);
        let mut prefix = vec![0; sample_len];
        if file.read_exact(&mut prefix).await.is_err() {
            return Ok(false);
        }
        let Ok(sample_len_u64) = u64::try_from(sample_len) else {
            return Ok(false);
        };
        let Some(suffix_offset) = anchor
            .byte_offset
            .checked_add(anchor.byte_len.saturating_sub(sample_len_u64))
        else {
            return Ok(false);
        };
        file.seek(SeekFrom::Start(suffix_offset)).await?;
        let mut suffix = vec![0; sample_len];
        if file.read_exact(&mut suffix).await.is_err() {
            return Ok(false);
        }
        Ok(Self::event_log_line_hash(&prefix) == anchor.prefix_hash
            && Self::event_log_line_hash(&suffix) == anchor.suffix_hash)
    }

    fn transcript_rewrite_row(
        session_id: &SessionId,
        row: RawStoredEvent,
    ) -> Result<Option<(RawTranscriptRewriteEvent, Vec<TranscriptRewriteCommit>)>, EventStoreError>
    {
        let decoded = meerkat_core::event::transcript_rewrite_commits_from_payload(&row.event)
            .map_err(|error| EventStoreError::Serialization(error.to_string()))?;
        let Some((event_session_id, commits)) = decoded else {
            return Ok(None);
        };
        if event_session_id != *session_id {
            return Err(EventStoreError::Store(format!(
                "transcript rewrite event for session {event_session_id} is stored in session {session_id}'s log"
            )));
        }
        Ok(Some((
            RawTranscriptRewriteEvent {
                seq: row.seq,
                event: row.event,
            },
            commits,
        )))
    }

    /// Prove one durable event-log head boundary and read only bytes appended after
    /// it. The durable sequence lock makes this atomic against cooperative
    /// writers; native file identity, the boundary anchor, and the stable
    /// before/after fingerprint fail closed against replacement/truncation.
    async fn read_authorized_transcript_rewrite_tail(
        &self,
        session_id: &SessionId,
        expected_prefix: &TranscriptRewritePrefixAccumulator,
    ) -> Result<Option<TranscriptRewriteAuditRows>, EventStoreError> {
        let _sequence_lock = self.acquire_sequence_lock(session_id).await?;
        let Some(sidecar) = self.read_durable_event_log_head(session_id).await? else {
            return Ok(None);
        };
        if sidecar.rewrite_prefix.as_ref() != Some(expected_prefix) {
            return Ok(None);
        }
        let authorized_prefix = sidecar.rewrite_prefix.clone().ok_or_else(|| {
            EventStoreError::Store(
                "authorized event-log head lost its transcript rewrite prefix".to_string(),
            )
        })?;

        let path = self.log_path(session_id);
        let mut file = match tokio::fs::File::open(&path).await {
            Ok(file) => file,
            Err(error)
                if error.kind() == std::io::ErrorKind::NotFound && sidecar.covered_log_len == 0 =>
            {
                return Ok(Some(TranscriptRewriteAuditRows {
                    session_id: session_id.clone(),
                    observed_through_log_seq: 0,
                    receipt: Some(TranscriptRewritePrefixReceipt {
                        session_id: session_id.clone(),
                        through_log_seq: 0,
                        accumulator: authorized_prefix.clone(),
                        last_commit: sidecar.last_rewrite_commit,
                        finalization_id: None,
                    }),
                    rewrite_rows: Vec::new(),
                }));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(EventStoreError::Io(error)),
        };
        let target = Self::event_log_fingerprint_from_metadata(&file.metadata().await?);
        if target.len < sidecar.covered_log_len {
            return Ok(None);
        }
        if target.len == sidecar.covered_log_len && target != sidecar.covered_log_fingerprint {
            return Ok(None);
        }
        #[cfg(unix)]
        if target.len > sidecar.covered_log_len
            && (target.device != sidecar.covered_log_fingerprint.device
                || target.inode != sidecar.covered_log_fingerprint.inode)
        {
            return Ok(None);
        }
        #[cfg(not(unix))]
        if target.len > sidecar.covered_log_len {
            // Without a stable native file identity, a longer replacement
            // cannot be distinguished from an append by a bounded check.
            return Ok(None);
        }
        match sidecar.last_line {
            Some(anchor)
                if anchor.byte_offset.checked_add(anchor.byte_len)
                    == Some(sidecar.covered_log_len)
                    && Self::tail_anchor_matches(&mut file, anchor).await? => {}
            None if sidecar.covered_log_len == 0 => {}
            _ => return Ok(None),
        }
        let after_anchor = Self::event_log_fingerprint_from_metadata(&file.metadata().await?);
        if after_anchor != target {
            return Ok(None);
        }

        if target.len == sidecar.covered_log_len {
            return Ok(Some(TranscriptRewriteAuditRows {
                session_id: session_id.clone(),
                observed_through_log_seq: sidecar.through_log_seq,
                receipt: Some(TranscriptRewritePrefixReceipt {
                    session_id: session_id.clone(),
                    through_log_seq: sidecar.through_log_seq,
                    accumulator: authorized_prefix.clone(),
                    last_commit: sidecar.last_rewrite_commit,
                    finalization_id: None,
                }),
                rewrite_rows: Vec::new(),
            }));
        }

        file.seek(SeekFrom::Start(sidecar.covered_log_len)).await?;
        let mut lines =
            BufReader::new((&mut file).take(target.len.saturating_sub(sidecar.covered_log_len)));
        let mut line = String::new();
        let mut offset = sidecar.covered_log_len;
        let mut observed_seq = sidecar.through_log_seq;
        let mut accumulator = authorized_prefix;
        let mut last_generation = sidecar.last_rewrite_generation;
        let mut last_commit = sidecar.last_rewrite_commit.clone();
        let mut last_distinct_rewrite_seq = sidecar.last_distinct_rewrite_seq;
        let mut last_line = sidecar.last_line;
        let mut rewrite_rows = Vec::new();
        while offset < target.len {
            line.clear();
            let bytes_read = lines.read_line(&mut line).await?;
            if bytes_read == 0 || !line.ends_with('\n') {
                return Err(EventStoreError::Store(format!(
                    "event log '{}' has a torn appended row after byte {}",
                    path.display(),
                    sidecar.covered_log_len
                )));
            }
            let byte_len = u64::try_from(bytes_read).map_err(|_| {
                EventStoreError::Store(format!(
                    "event log '{}' contains an address-unrepresentable row",
                    path.display()
                ))
            })?;
            let line_start = offset;
            offset = offset.checked_add(byte_len).ok_or_else(|| {
                EventStoreError::Store(format!(
                    "event log '{}' byte offset overflow",
                    path.display()
                ))
            })?;
            let row = self.decode_raw_event_line(&line)?;
            if row.seq <= observed_seq {
                return Err(EventStoreError::Store(format!(
                    "event log '{}' sequence {} is not strictly greater than authorized high-water {}",
                    path.display(),
                    row.seq,
                    observed_seq
                )));
            }
            observed_seq = row.seq;
            last_line = Some(Self::event_log_line_anchor(
                line_start,
                byte_len,
                line.as_bytes(),
            ));
            let Some((raw_rewrite, commits)) = Self::transcript_rewrite_row(session_id, row)?
            else {
                continue;
            };
            for commit in commits {
                if commit.rewrite_generation == 0 {
                    // A normalized boundary may never be followed by a newly
                    // written generation-zero fact.
                    return Ok(None);
                }
                if commit.rewrite_generation == last_generation {
                    if last_commit.as_ref() != Some(&commit) {
                        return Err(EventStoreError::TranscriptRewriteGenerationConflict {
                            session_id: session_id.clone(),
                            generation: commit.rewrite_generation,
                            first_seq: last_distinct_rewrite_seq,
                            conflicting_seq: raw_rewrite.seq,
                        });
                    }
                    continue;
                }
                if last_generation.checked_add(1) != Some(commit.rewrite_generation) {
                    // A gap or late repair older than the bounded last
                    // occurrence needs the full generation map.
                    return Ok(None);
                }
                accumulator = accumulator
                    .extend(&commit)
                    .map_err(|error| EventStoreError::Serialization(error.to_string()))?;
                last_generation = commit.rewrite_generation;
                last_commit = Some(commit);
                last_distinct_rewrite_seq = raw_rewrite.seq;
            }
            rewrite_rows.push(raw_rewrite);
        }
        drop(lines);
        if offset != target.len {
            return Err(EventStoreError::Store(format!(
                "event log '{}' tail ended at byte {offset}, expected {}",
                path.display(),
                target.len
            )));
        }
        let after = Self::event_log_fingerprint_from_metadata(&file.metadata().await?);
        if after != target {
            return Ok(None);
        }

        let receipt = TranscriptRewritePrefixReceipt {
            session_id: session_id.clone(),
            through_log_seq: observed_seq,
            accumulator: accumulator.clone(),
            last_commit: last_commit.clone(),
            finalization_id: None,
        };
        let advanced = DurableEventLogHeadBody {
            schema_version: EVENT_LOG_HEAD_SCHEMA_VERSION,
            session_id: session_id.clone(),
            through_log_seq: observed_seq,
            covered_log_len: target.len,
            covered_log_fingerprint: target,
            last_line,
            last_distinct_rewrite_seq,
            last_rewrite_generation: last_generation,
            last_rewrite_commit: last_commit,
            rewrite_prefix: Some(accumulator),
            legacy_generation_zero_normalized: sidecar.legacy_generation_zero_normalized,
        };
        let final_fingerprint = Self::event_log_fingerprint_from_metadata(&file.metadata().await?);
        if final_fingerprint != target {
            return Ok(None);
        }
        // Reading proves a candidate; it does not publish skip authority.
        // The consumer must validate the exact bodies and successfully apply
        // semantic replay before finalizing this private head.
        let receipt = self.stage_event_log_head(receipt, advanced).await?;
        Ok(Some(TranscriptRewriteAuditRows {
            session_id: session_id.clone(),
            observed_through_log_seq: observed_seq,
            receipt: Some(receipt),
            rewrite_rows,
        }))
    }

    async fn full_transcript_rewrite_audit_scan(
        &self,
        session_id: &SessionId,
    ) -> Result<FullTranscriptRewriteAuditScan, EventStoreError> {
        const MAX_STABILITY_ATTEMPTS: usize = 3;

        let path = self.log_path(session_id);
        for _ in 0..MAX_STABILITY_ATTEMPTS {
            let mut file = match tokio::fs::File::open(&path).await {
                Ok(file) => file,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    let mut index = EventLogIndex::default();
                    index.rebuild_current_transcript_rewrite_prefix()?;
                    return Ok(FullTranscriptRewriteAuditScan {
                        fingerprint: None,
                        last_line: None,
                        observed_through_log_seq: 0,
                        index,
                        rewrite_rows: Vec::new(),
                    });
                }
                Err(error) => return Err(EventStoreError::Io(error)),
            };
            let target = Self::event_log_fingerprint_from_metadata(&file.metadata().await?);
            file.seek(SeekFrom::Start(0)).await?;
            let mut lines = BufReader::new((&mut file).take(target.len));
            let mut line = String::new();
            let mut offset = 0_u64;
            let mut observed_seq = 0_u64;
            let mut last_line = None;
            let mut index = EventLogIndex::default();
            let mut rewrite_rows = Vec::new();
            while offset < target.len {
                line.clear();
                let bytes_read = lines.read_line(&mut line).await?;
                if bytes_read == 0 || !line.ends_with('\n') {
                    return Err(EventStoreError::Store(format!(
                        "event log '{}' has a torn row during full rewrite reconciliation",
                        path.display()
                    )));
                }
                let byte_len = u64::try_from(bytes_read).map_err(|_| {
                    EventStoreError::Store(format!(
                        "event log '{}' contains an address-unrepresentable row",
                        path.display()
                    ))
                })?;
                let line_start = offset;
                offset = offset.checked_add(byte_len).ok_or_else(|| {
                    EventStoreError::Store(format!(
                        "event log '{}' byte offset overflow",
                        path.display()
                    ))
                })?;
                let row = self.decode_raw_event_line(&line)?;
                if row.seq <= observed_seq {
                    return Err(EventStoreError::Store(format!(
                        "event log '{}' sequence {} is not strictly greater than {}",
                        path.display(),
                        row.seq,
                        observed_seq
                    )));
                }
                observed_seq = row.seq;
                last_line = Some(Self::event_log_line_anchor(
                    line_start,
                    byte_len,
                    line.as_bytes(),
                ));
                let Some((raw_rewrite, _commits)) = Self::transcript_rewrite_row(session_id, row)?
                else {
                    continue;
                };
                index.note_transcript_rewrite_payload(
                    session_id,
                    raw_rewrite.seq,
                    &raw_rewrite.event,
                )?;
                rewrite_rows.push(raw_rewrite);
            }
            drop(lines);
            let after = Self::event_log_fingerprint_from_metadata(&file.metadata().await?);
            if after == target {
                return Ok(FullTranscriptRewriteAuditScan {
                    fingerprint: Some(target),
                    last_line,
                    observed_through_log_seq: observed_seq,
                    index,
                    rewrite_rows,
                });
            }
        }
        Err(EventStoreError::Store(format!(
            "event log '{}' did not remain stable during full rewrite reconciliation",
            path.display()
        )))
    }

    async fn rebuild_event_log_index(
        &self,
        session_id: &SessionId,
        file: &mut tokio::fs::File,
        path: &Path,
        target: EventLogFingerprint,
    ) -> Result<EventLogIndex, EventStoreError> {
        let mut index = EventLogIndex::default();
        let start = 0;
        file.seek(SeekFrom::Start(start)).await?;
        let mut lines = BufReader::new((&mut *file).take(target.len - start));
        let mut line = String::new();
        let mut offset = start;
        while offset < target.len {
            line.clear();
            let bytes_read = lines.read_line(&mut line).await?;
            if bytes_read == 0 {
                return Err(EventStoreError::Store(format!(
                    "event log '{}' changed while its sparse index was being built",
                    path.display()
                )));
            }
            let line_start = offset;
            let byte_len = u64::try_from(bytes_read).map_err(|_| {
                EventStoreError::Store(format!(
                    "event log '{}' contains an address-unrepresentable row",
                    path.display()
                ))
            })?;
            offset = offset.checked_add(byte_len).ok_or_else(|| {
                EventStoreError::Store(format!(
                    "event log '{}' byte offset overflow",
                    path.display()
                ))
            })?;
            if line.trim().is_empty() {
                continue;
            }
            let event = self.decode_index_event_line(&line)?;
            if index.row_count > 0 && event.seq <= index.last_seq {
                return Err(EventStoreError::Store(format!(
                    "event log '{}' sequence {} is not strictly greater than {}",
                    path.display(),
                    event.seq,
                    index.last_seq
                )));
            }
            if index.row_count.is_multiple_of(EVENT_LOG_INDEX_STRIDE) {
                index.checkpoints.push(EventLogCheckpoint {
                    seq: event.seq,
                    byte_offset: line_start,
                });
            }
            index.note_exact_interaction_occupant(&event.source, event.seq, line_start, byte_len);
            index.note_transcript_rewrite_payload(session_id, event.seq, &event.event)?;
            index.row_count = index.row_count.checked_add(1).ok_or_else(|| {
                EventStoreError::Store(format!("event log '{}' row count overflow", path.display()))
            })?;
            index.last_seq = event.seq;
            index.last_line = Some(Self::event_log_line_anchor(
                line_start,
                byte_len,
                line.as_bytes(),
            ));
        }
        index.fingerprint = Some(target);
        Ok(index)
    }

    async fn refresh_event_log_index(
        &self,
        session_id: &SessionId,
        from_seq: Option<u64>,
    ) -> Result<(EventLogIndexSnapshot, Option<tokio::fs::File>), EventStoreError> {
        const MAX_STABILITY_ATTEMPTS: usize = 3;

        let path = self.log_path(session_id);
        let shared = self.event_log_index(session_id).await;
        let mut cached = shared.lock().await;

        for _ in 0..MAX_STABILITY_ATTEMPTS {
            let mut file = match tokio::fs::File::open(&path).await {
                Ok(file) => file,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    *cached = EventLogIndex::default();
                    return Ok((Self::event_log_index_snapshot(&cached, from_seq), None));
                }
                Err(error) => return Err(EventStoreError::Io(error)),
            };
            let before = Self::event_log_fingerprint_from_metadata(&file.metadata().await?);
            if Self::cached_fingerprint_is_reusable(cached.fingerprint, before) {
                let anchor_matches = match cached.last_line {
                    Some(anchor) => Self::tail_anchor_matches(&mut file, anchor).await?,
                    None => cached.row_count == 0,
                };
                let after = Self::event_log_fingerprint_from_metadata(&file.metadata().await?);
                if anchor_matches && after == before {
                    return Ok((
                        Self::event_log_index_snapshot(&cached, from_seq),
                        Some(file),
                    ));
                }
            }

            let candidate = match self
                .rebuild_event_log_index(session_id, &mut file, &path, before)
                .await
            {
                Ok(candidate) => candidate,
                Err(error) => {
                    *cached = EventLogIndex::default();
                    return Err(error);
                }
            };
            let after = Self::event_log_fingerprint_from_metadata(&file.metadata().await?);
            if after == before {
                let snapshot = Self::event_log_index_snapshot(&candidate, from_seq);
                *cached = candidate;
                return Ok((snapshot, Some(file)));
            }
        }

        Err(EventStoreError::Store(format!(
            "event log '{}' did not remain stable while its sparse index was refreshed",
            path.display()
        )))
    }

    async fn read_indexed(
        &self,
        session_id: &SessionId,
        from_seq: u64,
        max_rows: Option<usize>,
    ) -> Result<Vec<StoredEvent>, EventStoreError> {
        self.read_indexed_with(session_id, from_seq, max_rows, |store, line| {
            store.decode_event_line(line).map(|row| (row.seq, row))
        })
        .await
    }

    /// [`Self::read_indexed`] over rows whose payloads are left unparsed.
    async fn read_raw_indexed(
        &self,
        session_id: &SessionId,
        from_seq: u64,
        max_rows: Option<usize>,
    ) -> Result<Vec<RawStoredEvent>, EventStoreError> {
        self.read_indexed_with(session_id, from_seq, max_rows, |store, line| {
            store.decode_raw_event_line(line).map(|row| (row.seq, row))
        })
        .await
    }

    /// The indexed read, parameterized by how a row line becomes a row.
    ///
    /// The index refresh, the byte-offset seek, the read-stability retry and
    /// the fingerprint recheck are the same work whichever shape the caller
    /// wants back; only the per-line decode differs. Two copies of this loop
    /// would be two chances for the raw read to disagree with the typed one
    /// about which rows a log holds.
    async fn read_indexed_with<T>(
        &self,
        session_id: &SessionId,
        from_seq: u64,
        max_rows: Option<usize>,
        decode: impl Fn(&Self, &str) -> Result<(u64, T), EventStoreError>,
    ) -> Result<Vec<T>, EventStoreError> {
        if max_rows == Some(0) {
            return Ok(Vec::new());
        }
        const MAX_STABILITY_ATTEMPTS: usize = 3;
        let path = self.log_path(session_id);
        for _ in 0..MAX_STABILITY_ATTEMPTS {
            let (snapshot, file) = self
                .refresh_event_log_index(session_id, Some(from_seq))
                .await?;
            if snapshot.row_count == 0 {
                return Ok(Vec::new());
            }
            let Some(mut file) = file else {
                return Err(EventStoreError::Store(format!(
                    "event log '{}' vanished after a nonempty index was validated",
                    path.display()
                )));
            };
            let expected = snapshot.fingerprint.ok_or_else(|| {
                EventStoreError::Store(format!(
                    "event log '{}' has a nonempty index without a fingerprint",
                    path.display()
                ))
            })?;
            let (rows, after) = self
                .read_index_snapshot(&path, &mut file, snapshot, from_seq, max_rows, &decode)
                .await?;
            if after == expected {
                return Ok(rows);
            }
        }
        Err(EventStoreError::Store(format!(
            "event log '{}' did not remain stable during an indexed read",
            path.display()
        )))
    }

    /// Read exact interaction-source occupancy from the validated,
    /// reconstructable event-log index.
    ///
    /// Exact append callers hold the durable per-session sequence lock across
    /// this lookup and any following write. `refresh_event_log_index` therefore
    /// validates the current fingerprint after all earlier cooperative writers
    /// and no later writer can race the zero/one/many verdict.
    async fn exact_interaction_occupant(
        &self,
        session_id: &SessionId,
        interaction_id: InteractionId,
    ) -> Result<Option<ResolvedExactInteractionOccupant>, EventStoreError> {
        let mut occupants = self
            .exact_interaction_batch_occupants(session_id, &[interaction_id])
            .await?;
        match occupants.pop() {
            Some(ExactInteractionOccupancy::Empty) | None => Ok(None),
            Some(ExactInteractionOccupancy::One(first)) => {
                Ok(Some(ResolvedExactInteractionOccupant { first, count: 1 }))
            }
            Some(ExactInteractionOccupancy::Multiple { first, count }) => {
                Ok(Some(ResolvedExactInteractionOccupant { first, count }))
            }
        }
    }

    /// Capture all requested exact-source occupants from one validated index
    /// snapshot. Exact batch callers hold the durable sequence lock, so the
    /// prefix verdict remains stable through the following append.
    async fn exact_interaction_batch_occupants(
        &self,
        session_id: &SessionId,
        interaction_ids: &[InteractionId],
    ) -> Result<Vec<ExactInteractionOccupancy>, EventStoreError> {
        let _ = self.refresh_event_log_index(session_id, None).await?;
        let shared = self.event_log_index(session_id).await;
        let (locators, expected) = {
            let index = shared.lock().await;
            let locators = interaction_ids
                .iter()
                .map(|interaction_id| {
                    index
                        .exact_interaction_occupants
                        .get(interaction_id)
                        .cloned()
                })
                .collect::<Vec<_>>();
            (locators, index.fingerprint)
        };
        if locators.iter().all(Option::is_none) {
            return Ok(vec![
                ExactInteractionOccupancy::Empty;
                interaction_ids.len()
            ]);
        }
        let expected = expected.ok_or_else(|| {
            EventStoreError::Store(
                "nonempty exact-interaction index has no event-log fingerprint".to_string(),
            )
        })?;
        let path = self.log_path(session_id);
        let mut file = tokio::fs::File::open(&path).await?;
        let before = Self::event_log_fingerprint_from_metadata(&file.metadata().await?);
        if before != expected {
            return Err(EventStoreError::Store(format!(
                "event log '{}' changed after exact-interaction index validation",
                path.display()
            )));
        }
        let mut resolved = Vec::with_capacity(locators.len());
        for locator in locators {
            let Some(ExactInteractionOccupant { first, count }) = locator else {
                resolved.push(ExactInteractionOccupancy::Empty);
                continue;
            };
            let stored = self.read_event_log_row_at(&path, &mut file, first).await?;
            resolved.push(if count == 1 {
                ExactInteractionOccupancy::One(stored)
            } else {
                ExactInteractionOccupancy::Multiple {
                    first: stored,
                    count,
                }
            });
        }
        let after = Self::event_log_fingerprint_from_metadata(&file.metadata().await?);
        if after != expected {
            return Err(EventStoreError::Store(format!(
                "event log '{}' changed while exact-interaction occupants were read",
                path.display()
            )));
        }
        Ok(resolved)
    }

    async fn read_event_log_row_at(
        &self,
        path: &Path,
        file: &mut tokio::fs::File,
        locator: EventLogRowLocator,
    ) -> Result<StoredEvent, EventStoreError> {
        let byte_len = usize::try_from(locator.byte_len).map_err(|_| {
            EventStoreError::Store(format!(
                "event log '{}' exact-interaction row is too large to address",
                path.display()
            ))
        })?;
        file.seek(SeekFrom::Start(locator.byte_offset)).await?;
        let mut bytes = vec![0_u8; byte_len];
        file.read_exact(&mut bytes).await?;
        let line = std::str::from_utf8(&bytes).map_err(|error| {
            EventStoreError::Serialization(format!(
                "event log '{}' exact-interaction row is not UTF-8: {error}",
                path.display()
            ))
        })?;
        let stored = self.decode_event_line(line)?;
        if stored.seq != locator.seq {
            return Err(EventStoreError::Store(format!(
                "event log '{}' exact-interaction locator expected sequence {}, found {}",
                path.display(),
                locator.seq,
                stored.seq
            )));
        }
        Ok(stored)
    }

    async fn read_index_snapshot<T>(
        &self,
        path: &Path,
        file: &mut tokio::fs::File,
        snapshot: EventLogIndexSnapshot,
        from_seq: u64,
        max_rows: Option<usize>,
        decode: &impl Fn(&Self, &str) -> Result<(u64, T), EventStoreError>,
    ) -> Result<(Vec<T>, EventLogFingerprint), EventStoreError> {
        let byte_offset = snapshot.byte_offset.ok_or_else(|| {
            EventStoreError::Store(format!(
                "event log '{}' has a nonempty index without a page checkpoint",
                path.display()
            ))
        })?;
        let indexed_len = snapshot
            .fingerprint
            .map_or(0, |fingerprint| fingerprint.len);
        file.seek(SeekFrom::Start(byte_offset)).await?;
        let byte_budget = indexed_len.checked_sub(byte_offset).ok_or_else(|| {
            EventStoreError::Store(format!(
                "event log '{}' index checkpoint exceeds its validated length",
                path.display()
            ))
        })?;
        let mut lines = BufReader::new((&mut *file).take(byte_budget));
        let mut line = String::new();
        let mut consumed = 0_u64;
        // `EventStore` is a public library trait: callers may supply an
        // arbitrary `usize`. Do not let an untrusted page hint trigger a
        // capacity-overflow panic or eager giant allocation.
        let mut rows = Vec::new();
        while consumed < byte_budget {
            line.clear();
            let bytes_read = lines.read_line(&mut line).await?;
            if bytes_read == 0 {
                return Err(EventStoreError::Store(format!(
                    "event log '{}' changed during an indexed read",
                    path.display()
                )));
            }
            consumed = consumed
                .checked_add(u64::try_from(bytes_read).map_err(|_| {
                    EventStoreError::Store(format!(
                        "event log '{}' contains an address-unrepresentable row",
                        path.display()
                    ))
                })?)
                .ok_or_else(|| {
                    EventStoreError::Store(format!(
                        "event log '{}' byte offset overflow",
                        path.display()
                    ))
                })?;
            if line.trim().is_empty() {
                continue;
            }
            let (seq, row) = decode(self, &line)?;
            if seq >= from_seq {
                rows.push(row);
                if max_rows.is_some_and(|limit| rows.len() == limit) {
                    break;
                }
            }
        }
        drop(lines);
        let after = Self::event_log_fingerprint_from_metadata(&file.metadata().await?);
        Ok((rows, after))
    }

    async fn note_appended_rows(&self, append: &EventLogAppend<'_>) {
        let shared = self.event_log_index(append.session_id).await;
        let mut index = shared.lock().await;
        let Ok(appended_len) = u64::try_from(append.bytes.len()) else {
            *index = EventLogIndex::default();
            return;
        };
        let Some(expected_post_len) = append.pre_fingerprint.len.checked_add(appended_len) else {
            *index = EventLogIndex::default();
            return;
        };
        // A reader may have rebuilt this shared cache to the post-append
        // fingerprint after fsync but before this cooperative update acquired
        // the mutex. Never erase that newer validated snapshot. Mechanical
        // extension is legal only from the exact pre-append state.
        if index.fingerprint != Some(append.pre_fingerprint) {
            // The first append creates the JSONL after `last_seq()` validated
            // its absence. Bind that still-empty index to the newly-created
            // zero-byte file before mechanically extending it. No other
            // mismatch is legal.
            if index.fingerprint.is_none()
                && index.row_count == 0
                && index.last_seq == 0
                && append.pre_fingerprint.len == 0
            {
                index.fingerprint = Some(append.pre_fingerprint);
            } else {
                return;
            }
        }
        if append.post_fingerprint.len != expected_post_len {
            *index = EventLogIndex::default();
            return;
        }
        let append_start = append.pre_fingerprint.len;
        if append.rows.len() != append.stored_events.len() {
            *index = EventLogIndex::default();
            return;
        }
        for (row, event) in append.rows.iter().zip(append.stored_events) {
            if index.row_count > 0 && row.seq <= index.last_seq {
                *index = EventLogIndex::default();
                return;
            }
            let Some(absolute_offset) = append_start.checked_add(row.relative_offset) else {
                *index = EventLogIndex::default();
                return;
            };
            let Ok(relative_start) = usize::try_from(row.relative_offset) else {
                *index = EventLogIndex::default();
                return;
            };
            let Some(relative_end_u64) = row.relative_offset.checked_add(row.byte_len) else {
                *index = EventLogIndex::default();
                return;
            };
            let Ok(relative_end) = usize::try_from(relative_end_u64) else {
                *index = EventLogIndex::default();
                return;
            };
            let Some(line) = append.bytes.get(relative_start..relative_end) else {
                *index = EventLogIndex::default();
                return;
            };
            if index.row_count.is_multiple_of(EVENT_LOG_INDEX_STRIDE) {
                index.checkpoints.push(EventLogCheckpoint {
                    seq: row.seq,
                    byte_offset: absolute_offset,
                });
            }
            index.row_count = match index.row_count.checked_add(1) {
                Some(row_count) => row_count,
                None => {
                    *index = EventLogIndex::default();
                    return;
                }
            };
            index.last_seq = row.seq;
            index.last_line = Some(Self::event_log_line_anchor(
                absolute_offset,
                row.byte_len,
                line,
            ));
            index.note_exact_interaction_occupant(
                &event.source,
                event.seq,
                absolute_offset,
                row.byte_len,
            );
            if index
                .note_transcript_rewrite_commit(append.session_id, event)
                .is_err()
            {
                *index = EventLogIndex::default();
                return;
            }
        }
        index.fingerprint = Some(append.post_fingerprint);
    }

    #[cfg(test)]
    fn reset_decoded_rows(&self) {
        self.decoded_rows.store(0, Ordering::Relaxed);
    }

    #[cfg(test)]
    fn decoded_rows(&self) -> usize {
        self.decoded_rows.load(Ordering::Relaxed)
    }

    #[cfg(test)]
    async fn index_registry_len(&self) -> usize {
        self.index_registry.lock().await.entries.len()
    }

    #[cfg(test)]
    async fn index_registry_contains(&self, session_id: &SessionId) -> bool {
        self.index_registry
            .lock()
            .await
            .entries
            .contains_key(&session_id.to_string())
    }

    fn sequence_dir(&self) -> PathBuf {
        self.root.join(".sequence")
    }

    fn sequence_path(&self, session_id: &SessionId) -> PathBuf {
        self.sequence_dir().join(format!("{session_id}.seq"))
    }

    fn sequence_lock_path(&self, session_id: &SessionId) -> PathBuf {
        self.sequence_dir().join(format!("{session_id}.lock"))
    }

    fn projection_halt_dir(&self) -> PathBuf {
        self.root.join(".projection-halts")
    }

    fn projection_halt_path(&self, session_id: &SessionId) -> PathBuf {
        self.projection_halt_dir()
            .join(format!("{session_id}.json"))
    }

    async fn acquire_sequence_lock(
        &self,
        session_id: &SessionId,
    ) -> Result<SequenceAllocationLock, EventStoreError> {
        tokio::fs::create_dir_all(self.sequence_dir()).await?;
        let lock_path = self.sequence_lock_path(session_id);
        Self::lock_sequence_file(lock_path).await
    }

    async fn lock_sequence_file(
        lock_path: PathBuf,
    ) -> Result<SequenceAllocationLock, EventStoreError> {
        let display_path = lock_path.display().to_string();
        let lock =
            tokio::task::spawn_blocking(move || -> Result<std::fs::File, EventStoreError> {
                let file = std::fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .create(true)
                    .truncate(false)
                    .open(&lock_path)?;
                file.lock().map_err(|err| {
                    EventStoreError::Store(format!(
                        "failed to acquire durable sequence lock '{display_path}': {err}"
                    ))
                })?;
                Ok(file)
            })
            .await
            .map_err(|err| {
                EventStoreError::Store(format!("durable sequence lock task failed: {err}"))
            })??;

        Ok(SequenceAllocationLock { _lock: lock })
    }

    async fn read_sequence_owner(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<u64>, EventStoreError> {
        let path = self.sequence_path(session_id);
        let contents = match tokio::fs::read_to_string(&path).await {
            Ok(contents) => contents,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(EventStoreError::Io(err)),
        };
        let trimmed = contents.trim();
        if trimmed.is_empty() {
            return Err(EventStoreError::Store(format!(
                "durable sequence owner '{}' is empty",
                path.display()
            )));
        }
        trimmed.parse::<u64>().map(Some).map_err(|err| {
            EventStoreError::Store(format!(
                "durable sequence owner '{}' is invalid: {err}",
                path.display()
            ))
        })
    }

    #[cfg(test)]
    async fn write_sequence_owner(
        &self,
        session_id: &SessionId,
        seq: u64,
    ) -> Result<(), EventStoreError> {
        let path = self.sequence_path(session_id);
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&path)
            .await?;
        file.write_all(format!("{seq}\n").as_bytes()).await?;
        file.flush().await?;
        file.sync_all().await?;
        Ok(())
    }

    fn event_log_head_after_append(
        &self,
        mut head: DurableEventLogHeadBody,
        append: &EventLogAppend<'_>,
    ) -> Result<DurableEventLogHeadBody, EventStoreError> {
        if head.covered_log_fingerprint != append.pre_fingerprint
            || head.covered_log_len != append.pre_fingerprint.len
            || append.rows.len() != append.stored_events.len()
        {
            return Err(EventStoreError::Store(
                "validated event-log head no longer matches append pre-state".to_string(),
            ));
        }

        let mut rewrite_prefix = head.rewrite_prefix.take();
        let mut last_generation = head.last_rewrite_generation;
        let mut last_commit = head.last_rewrite_commit.take();
        let mut last_distinct_rewrite_seq = head.last_distinct_rewrite_seq;
        for event in append.stored_events {
            let Some((event_session_id, commits)) = transcript_rewrite_event_parts(&event.event)
            else {
                continue;
            };
            if event_session_id != append.session_id {
                continue;
            }
            let Some(accumulator) = rewrite_prefix.as_mut() else {
                continue;
            };
            if let Some((_, receipt, _)) = transcript_rewrite_receipt_event_parts(&event.event) {
                if receipt.end_prefix() == accumulator {
                    if last_commit.as_ref() != receipt.commits().last() {
                        return Err(EventStoreError::TranscriptRewriteGenerationConflict {
                            session_id: append.session_id.to_owned(),
                            generation: last_generation,
                            first_seq: last_distinct_rewrite_seq,
                            conflicting_seq: event.seq,
                        });
                    }
                    continue;
                }
                if receipt.start_prefix() != accumulator {
                    return Err(EventStoreError::Store(format!(
                        "transcript rewrite receipt starts at occurrence {}, but the durable head is at {}",
                        receipt.start_prefix().occurrence_count(),
                        accumulator.occurrence_count()
                    )));
                }
            }
            let mut invalid_legacy_transition = false;
            for commit in commits {
                if commit.rewrite_generation == last_generation {
                    if last_commit.as_ref() != Some(commit) {
                        return Err(EventStoreError::TranscriptRewriteGenerationConflict {
                            session_id: append.session_id.to_owned(),
                            generation: commit.rewrite_generation,
                            first_seq: last_distinct_rewrite_seq,
                            conflicting_seq: event.seq,
                        });
                    }
                    continue;
                }
                if last_generation.checked_add(1) == Some(commit.rewrite_generation) {
                    *accumulator = accumulator
                        .extend(commit)
                        .map_err(|error| EventStoreError::Serialization(error.to_string()))?;
                    last_generation = commit.rewrite_generation;
                    last_commit = Some(commit.clone());
                    last_distinct_rewrite_seq = event.seq;
                    continue;
                }
                invalid_legacy_transition = true;
                break;
            }
            if !invalid_legacy_transition {
                continue;
            }
            // A generation-zero append, a gap, or a late older repair needs
            // the full occurrence map. Keep the exact log head/high-water but
            // remove semantic skip authority until reconciliation.
            rewrite_prefix = None;
            last_generation = 0;
            last_commit = None;
            head.legacy_generation_zero_normalized = false;
        }

        let Some(last_row) = append.rows.last() else {
            return Err(EventStoreError::Store(
                "nonempty append has no event-log head row".to_string(),
            ));
        };
        let Some(relative_end) = last_row
            .relative_offset
            .checked_add(last_row.byte_len)
            .and_then(|end| usize::try_from(end).ok())
        else {
            return Err(EventStoreError::Store(
                "appended event-log head row exceeds addressable bytes".to_string(),
            ));
        };
        let Some(relative_start) = usize::try_from(last_row.relative_offset).ok() else {
            return Err(EventStoreError::Store(
                "appended event-log head row has an unaddressable offset".to_string(),
            ));
        };
        let Some(line) = append.bytes.get(relative_start..relative_end) else {
            return Err(EventStoreError::Store(
                "appended event-log head row is outside serialized bytes".to_string(),
            ));
        };
        let absolute_offset = append
            .pre_fingerprint
            .len
            .checked_add(last_row.relative_offset)
            .ok_or_else(|| {
                EventStoreError::Store(
                    "event-log head byte offset overflow after append".to_string(),
                )
            })?;
        head.through_log_seq = last_row.seq;
        head.covered_log_len = append.post_fingerprint.len;
        head.covered_log_fingerprint = append.post_fingerprint;
        head.last_line = Some(Self::event_log_line_anchor(
            absolute_offset,
            last_row.byte_len,
            line,
        ));
        head.last_distinct_rewrite_seq = last_distinct_rewrite_seq;
        head.last_rewrite_generation = last_generation;
        head.last_rewrite_commit = last_commit;
        head.rewrite_prefix = rewrite_prefix;
        Ok(head)
    }

    fn rewrite_batch_is_head_local(
        session_id: &SessionId,
        head: Option<&DurableEventLogHeadBody>,
        events: &[StoredEvent],
    ) -> bool {
        let Some(head) = head else {
            return false;
        };
        let Some(mut prefix) = head.rewrite_prefix.clone() else {
            return false;
        };
        let mut last_generation = head.last_rewrite_generation;
        let mut last_commit = head.last_rewrite_commit.clone();
        for event in events {
            let Some((event_session_id, commits)) = transcript_rewrite_event_parts(&event.event)
            else {
                continue;
            };
            if event_session_id != session_id {
                continue;
            }
            if let Some((_, receipt, _)) = transcript_rewrite_receipt_event_parts(&event.event) {
                if receipt.end_prefix() == &prefix {
                    if last_commit.as_ref() != receipt.commits().last() {
                        return true;
                    }
                    continue;
                }
                if receipt.start_prefix() != &prefix {
                    return false;
                }
            }
            for commit in commits {
                if commit.rewrite_generation == last_generation {
                    // Equal is a local retry; unequal is a local conflict.
                    if last_commit.as_ref() != Some(commit) {
                        return true;
                    }
                    continue;
                }
                if last_generation.checked_add(1) != Some(commit.rewrite_generation) {
                    return false;
                }
                let Ok(next_prefix) = prefix.extend(commit) else {
                    return true;
                };
                prefix = next_prefix;
                last_generation = commit.rewrite_generation;
                last_commit = Some(commit.clone());
            }
        }
        true
    }

    fn validate_transcript_rewrite_events_before_append(
        session_id: &SessionId,
        envelopes: &[EventEnvelope<AgentEvent>],
    ) -> Result<(), EventStoreError> {
        for envelope in envelopes {
            match &envelope.payload {
                AgentEvent::TranscriptRewriteCommitted { .. } => {
                    return Err(EventStoreError::Store(
                        "current EventStore writers refuse full-body TranscriptRewriteCommitted rows; use exact receipt-only publication"
                            .to_string(),
                    ));
                }
                AgentEvent::TranscriptRewriteAuditReceiptCommitted {
                    session_id: event_session_id,
                    ..
                } if event_session_id != session_id => {
                    return Err(EventStoreError::Store(format!(
                        "refusing to append a transcript rewrite receipt for session {event_session_id} to session {session_id}'s log"
                    )));
                }
                _ => {}
            }
        }
        Ok(())
    }

    async fn validate_rewrite_append_against_full_index(
        &self,
        session_id: &SessionId,
        events: &[StoredEvent],
    ) -> Result<(), EventStoreError> {
        let _ = self.refresh_event_log_index(session_id, None).await?;
        let shared = self.event_log_index(session_id).await;
        let index = shared.lock().await;
        let mut batch = BTreeMap::<u64, (&TranscriptRewriteCommit, u64)>::new();
        for event in events {
            let Some((event_session_id, commits)) = transcript_rewrite_event_parts(&event.event)
            else {
                continue;
            };
            if event_session_id != session_id {
                continue;
            }
            for commit in commits {
                if commit.rewrite_generation == 0 {
                    return Err(EventStoreError::Store(
                        "current EventStore writer refuses a generation-zero transcript rewrite"
                            .to_string(),
                    ));
                }
                if let Some(existing) = index
                    .transcript_rewrite_commits
                    .get(&commit.rewrite_generation)
                {
                    if existing.commit != *commit {
                        return Err(EventStoreError::TranscriptRewriteGenerationConflict {
                            session_id: session_id.clone(),
                            generation: commit.rewrite_generation,
                            first_seq: existing.seq,
                            conflicting_seq: event.seq,
                        });
                    }
                    continue;
                }
                if let Some((existing, first_seq)) = batch.get(&commit.rewrite_generation) {
                    if *existing != commit {
                        return Err(EventStoreError::TranscriptRewriteGenerationConflict {
                            session_id: session_id.clone(),
                            generation: commit.rewrite_generation,
                            first_seq: *first_seq,
                            conflicting_seq: event.seq,
                        });
                    }
                    continue;
                }
                batch.insert(commit.rewrite_generation, (commit, event.seq));
            }
        }
        Ok(())
    }

    /// Allocate a contiguous sequence range from the one durable event-log
    /// head. A validated exact head is O(1) and is returned so the append can
    /// advance the same metadata record after the JSONL fsync.
    ///
    /// Missing/stale heads pay one canonical-log reconciliation. The old
    /// truncate-in-place `.seq` file is read only on that migration fallback;
    /// current writers never update it or maintain parallel authority.
    async fn allocate_sequence_range(
        &self,
        session_id: &SessionId,
        event_count: usize,
    ) -> Result<(u64, u64, Option<DurableEventLogHeadBody>, bool), EventStoreError> {
        let event_count = u64::try_from(event_count).map_err(|_| {
            EventStoreError::Store("event batch is too large to allocate a sequence range".into())
        })?;
        if event_count == 0 {
            return Err(EventStoreError::Store(
                "cannot allocate an empty event sequence range".into(),
            ));
        }

        let current_head = self.read_current_event_log_head(session_id).await?;
        let (base_seq, trusted_empty_base) = if let Some(head) = current_head.as_ref() {
            (head.through_log_seq, false)
        } else {
            let (snapshot, _file) = self.refresh_event_log_index(session_id, None).await?;
            let legacy_sequence_owner = self.read_sequence_owner(session_id).await?;
            (
                legacy_sequence_owner
                    .unwrap_or(snapshot.last_seq)
                    .max(snapshot.last_seq),
                snapshot.row_count == 0,
            )
        };
        let first_seq = base_seq.checked_add(1).ok_or_else(|| {
            EventStoreError::Store("event sequence overflow while allocating first sequence".into())
        })?;
        let last_seq = base_seq.checked_add(event_count).ok_or_else(|| {
            EventStoreError::Store("event sequence overflow while allocating range".into())
        })?;

        Ok((first_seq, last_seq, current_head, trusted_empty_base))
    }

    /// Append envelopes while the caller holds both `append_lock` and the
    /// durable per-session sequence lock.
    async fn append_envelopes_locked(
        &self,
        session_id: &SessionId,
        envelopes: &[EventEnvelope<AgentEvent>],
    ) -> Result<Vec<StoredEvent>, EventStoreError> {
        debug_assert!(!envelopes.is_empty());

        // A typed AgentEvent is not a proof that a rewrite record is valid:
        // TranscriptRewriteRecord's fields are public. No semantic head may be
        // extended from commit-only metadata until the exact full bodies pass
        // the same constructor validation used by the session graph.
        Self::validate_transcript_rewrite_events_before_append(session_id, envelopes)?;
        let path = self.log_path(session_id);
        let (mut next_seq, last_allocated_seq, exact_pre_head, trusted_empty_base) = self
            .allocate_sequence_range(session_id, envelopes.len())
            .await?;
        let mut lines = String::new();
        let mut appended_index_rows = Vec::with_capacity(envelopes.len());
        let mut stored_events = Vec::with_capacity(envelopes.len());
        for envelope in envelopes {
            let relative_offset = u64::try_from(lines.len()).map_err(|_| {
                EventStoreError::Store(
                    "serialized event batch is too large to index in memory".to_string(),
                )
            })?;
            let stored = StoredEvent {
                seq: next_seq,
                schema_version: EVENT_SCHEMA_VERSION,
                timestamp: SystemTime::now(),
                source: envelope.source.clone(),
                mob_id: envelope.mob_id.clone(),
                stream_seq: envelope.seq,
                event: envelope.payload.clone(),
            };
            lines.push_str(
                &serde_json::to_string(&stored)
                    .map_err(|err| EventStoreError::Serialization(err.to_string()))?,
            );
            lines.push('\n');
            let relative_end = u64::try_from(lines.len()).map_err(|_| {
                EventStoreError::Store(
                    "serialized event batch is too large to index in memory".to_string(),
                )
            })?;
            appended_index_rows.push(AppendedIndexRow {
                seq: next_seq,
                relative_offset,
                byte_len: relative_end - relative_offset,
            });
            stored_events.push(stored);
            if stored_events.len() < envelopes.len() {
                next_seq = next_seq.checked_add(1).ok_or_else(|| {
                    EventStoreError::Store("event sequence overflow after allocation".to_string())
                })?;
            }
        }
        let has_session_rewrite = stored_events.iter().any(|event| {
            transcript_rewrite_event_parts(&event.event)
                .is_some_and(|(event_session_id, _)| event_session_id == session_id)
        });
        if has_session_rewrite
            && !Self::rewrite_batch_is_head_local(
                session_id,
                exact_pre_head.as_ref(),
                &stored_events,
            )
        {
            self.validate_rewrite_append_against_full_index(session_id, &stored_events)
                .await?;
        }
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .await?;
        let pre_fingerprint = Self::event_log_fingerprint_from_metadata(&file.metadata().await?);
        // Validate and advance semantic head facts before the authority write:
        // a same-generation conflict must fail without appending a corrupt row.
        let prospective_append = EventLogAppend {
            session_id,
            pre_fingerprint,
            post_fingerprint: pre_fingerprint,
            bytes: lines.as_bytes(),
            rows: &appended_index_rows,
            stored_events: &stored_events,
        };
        let mut prospective_head = exact_pre_head
            .map(|head| self.event_log_head_after_append(head, &prospective_append))
            .transpose()?;
        file.write_all(lines.as_bytes()).await?;
        file.flush().await?;
        file.sync_all().await?;
        let post_fingerprint = Self::event_log_fingerprint_from_metadata(&file.metadata().await?);
        self.note_appended_rows(&EventLogAppend {
            session_id,
            pre_fingerprint,
            post_fingerprint,
            bytes: lines.as_bytes(),
            rows: &appended_index_rows,
            stored_events: &stored_events,
        })
        .await;
        // JSONL fsync is the authority boundary. Exactly one atomic metadata
        // replacement follows it: sequence allocation and rewrite-tail
        // authorization share this event-log head instead of maintaining a
        // `.seq` file plus a second sidecar.
        let head_result = if let Some(head) = prospective_head.as_mut() {
            head.covered_log_len = post_fingerprint.len;
            head.covered_log_fingerprint = post_fingerprint;
            self.write_event_log_head(session_id, head.clone()).await
        } else {
            self.persist_event_log_head_from_index(
                session_id,
                post_fingerprint,
                last_allocated_seq,
                trusted_empty_base,
            )
            .await
        };
        if let Err(error) = head_result {
            tracing::warn!(
                session_id = %session_id,
                error = %error,
                "failed to update event-log head; canonical JSONL reconciliation will rebuild it"
            );
        }
        Ok(stored_events)
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg(not(target_arch = "wasm32"))]
impl EventStore for FileEventStore {
    async fn append_envelopes(
        &self,
        session_id: &SessionId,
        envelopes: &[EventEnvelope<AgentEvent>],
    ) -> Result<u64, EventStoreError> {
        if envelopes.is_empty() {
            return self.last_seq(session_id).await;
        }

        let mut receipt_related = envelopes
            .iter()
            .filter_map(|envelope| transcript_rewrite_receipt_event_parts(&envelope.payload));
        if let Some((event_session_id, receipt, final_assistant_text)) = receipt_related.next() {
            if envelopes.len() != 1 || receipt_related.next().is_some() {
                return Err(EventStoreError::Store(
                    "generic event batches containing transcript rewrite receipts are forbidden; publish exactly one receipt"
                        .to_string(),
                ));
            }
            if event_session_id != session_id {
                return Err(EventStoreError::Store(format!(
                    "transcript rewrite receipt for session {event_session_id} cannot be stored in session {session_id}'s log"
                )));
            }
            return self
                .append_transcript_rewrite_receipt_exact(
                    session_id,
                    receipt,
                    final_assistant_text.as_deref(),
                )
                .await
                .map(|result| result.stored_event().seq);
        }

        // Interaction source identity is an exactly-one terminal keyspace, not
        // a generic batch lane. The persistent projection drains one envelope
        // at a time, and a runtime-owned terminal may already have been
        // synchronously appended before broadcast. Route that one canonical
        // envelope through exact append; reject every mixed/multi-row or
        // nonterminal Interaction-source batch before it can bypass uniqueness.
        let mut interaction_related = envelopes.iter().filter_map(|envelope| {
            interaction_related_envelope_id(envelope).map(|id| (id, envelope))
        });
        if let Some((interaction_id, envelope)) = interaction_related.next() {
            if envelopes.len() != 1 || interaction_related.next().is_some() {
                return Err(EventStoreError::InvalidExactInteractionTerminal {
                    interaction_id,
                    reason: "generic event batches containing interaction-sourced or interaction-terminal envelopes are forbidden; append exactly one canonical terminal"
                        .to_string(),
                });
            }
            return self
                .append_interaction_terminal_exact(session_id, interaction_id, envelope)
                .await
                .map(|result| result.stored_event().seq);
        }

        let _guard = self.append_lock.lock().await;
        tokio::fs::create_dir_all(&self.root).await?;
        let _sequence_lock = self.acquire_sequence_lock(session_id).await?;
        let stored = self.append_envelopes_locked(session_id, envelopes).await?;
        stored.last().map(|event| event.seq).ok_or_else(|| {
            EventStoreError::Store("nonempty append produced no durable events".to_string())
        })
    }

    async fn append_interaction_terminal_exact(
        &self,
        session_id: &SessionId,
        interaction_id: InteractionId,
        envelope: &EventEnvelope<AgentEvent>,
    ) -> Result<ExactInteractionAppend, EventStoreError> {
        validate_exact_interaction_terminal(interaction_id, envelope)?;

        let _guard = self.append_lock.lock().await;
        tokio::fs::create_dir_all(&self.root).await?;
        // This durable lock extends exact-ID atomicity across separately
        // constructed FileEventStore instances and processes, not only clones
        // that share the process-local append lock.
        let _sequence_lock = self.acquire_sequence_lock(session_id).await?;
        let occupant = self
            .exact_interaction_occupant(session_id, interaction_id)
            .await?;

        match occupant {
            None => {
                let mut inserted = self
                    .append_envelopes_locked(session_id, std::slice::from_ref(envelope))
                    .await?;
                let stored = inserted.pop().ok_or_else(|| {
                    EventStoreError::Store(
                        "exact interaction append produced no durable event".to_string(),
                    )
                })?;
                Ok(ExactInteractionAppend::Inserted(stored))
            }
            Some(ResolvedExactInteractionOccupant { first, count: 1 })
                if first.mob_id == envelope.mob_id
                    && interaction_terminal_events_semantically_equal(
                        &first.event,
                        &envelope.payload,
                    ) =>
            {
                Ok(ExactInteractionAppend::Replayed(first))
            }
            Some(ResolvedExactInteractionOccupant { first, count: 1 }) => {
                Err(EventStoreError::ExactInteractionTerminalConflict {
                    session_id: session_id.clone(),
                    interaction_id,
                    existing_count: 1,
                    reason: format!(
                        "stored mob/event {:?}/{:?} does not match incoming mob/event {:?}/{:?}",
                        first.mob_id, first.event, envelope.mob_id, envelope.payload
                    ),
                })
            }
            Some(ResolvedExactInteractionOccupant { count, .. }) => {
                Err(EventStoreError::ExactInteractionTerminalConflict {
                    session_id: session_id.clone(),
                    interaction_id,
                    existing_count: count,
                    reason: "multiple durable rows already claim the exact interaction identity"
                        .to_string(),
                })
            }
        }
    }

    async fn append_interaction_terminals_exact_batch(
        &self,
        session_id: &SessionId,
        stream_seq_floor: u64,
        terminals: &[(InteractionId, EventEnvelope<AgentEvent>)],
    ) -> Result<Vec<ExactInteractionAppend>, EventStoreError> {
        validate_exact_interaction_terminal_batch(terminals)?;
        if terminals.is_empty() {
            return Ok(Vec::new());
        }

        let _guard = self.append_lock.lock().await;
        tokio::fs::create_dir_all(&self.root).await?;
        // One durable lock covers the complete occupant verdict and the one
        // optional suffix append across independent store instances/processes.
        let _sequence_lock = self.acquire_sequence_lock(session_id).await?;
        let interaction_ids: Vec<_> = terminals.iter().map(|(id, _)| *id).collect();
        let occupants = self
            .exact_interaction_batch_occupants(session_id, &interaction_ids)
            .await?;
        let prefix_len =
            validate_exact_interaction_terminal_replay_prefix(session_id, terminals, &occupants)?;

        let mut results = Vec::with_capacity(terminals.len());
        let mut replay_tail = 0_u64;
        for occupant in occupants.iter().take(prefix_len) {
            let ExactInteractionOccupancy::One(stored) = occupant else {
                return Err(EventStoreError::Store(
                    "validated exact interaction replay prefix lost its canonical row".to_string(),
                ));
            };
            replay_tail = stored.stream_seq;
            results.push(ExactInteractionAppend::Replayed(stored.clone()));
        }

        if prefix_len == terminals.len() {
            return Ok(results);
        }

        // The store stamps the missing suffix after both the live sequencer's
        // floor and any recovered prefix. This lets a restarted actor recover a
        // prefix whose canonical stream tail is ahead of its local counter,
        // without ever appending a lower sequence after a higher durable row.
        let mut next_stream_seq = stream_seq_floor.max(replay_tail);
        let mut missing_envelopes = Vec::with_capacity(terminals.len() - prefix_len);
        for (_, envelope) in &terminals[prefix_len..] {
            next_stream_seq = next_stream_seq.checked_add(1).ok_or_else(|| {
                EventStoreError::InvalidExactInteractionTerminalBatch {
                    reason: "session event stream sequence overflow while stamping missing suffix"
                        .to_string(),
                }
            })?;
            let mut canonical = envelope.clone();
            canonical.seq = next_stream_seq;
            missing_envelopes.push(canonical);
        }

        // One JSONL write/flush/fsync for the entire missing suffix. All
        // conflicts and prefix-shape checks have completed before this point.
        let inserted = self
            .append_envelopes_locked(session_id, &missing_envelopes)
            .await?;
        if inserted.len() != missing_envelopes.len() {
            return Err(EventStoreError::Store(format!(
                "exact interaction batch appended {} rows for a {}-row suffix",
                inserted.len(),
                missing_envelopes.len()
            )));
        }
        results.extend(inserted.into_iter().map(ExactInteractionAppend::Inserted));
        Ok(results)
    }

    async fn append_transcript_rewrite_receipt_exact(
        &self,
        session_id: &SessionId,
        receipt: &TranscriptRewriteAuditReceiptBatch,
        final_assistant_text: Option<&str>,
    ) -> Result<ExactTranscriptRewriteReceiptAppend, EventStoreError> {
        let identity = serde_json::to_vec(receipt)
            .map_err(|error| EventStoreError::Serialization(error.to_string()))?;
        let incoming_summary = final_assistant_text.map(ToOwned::to_owned);

        let _guard = self.append_lock.lock().await;
        tokio::fs::create_dir_all(&self.root).await?;
        let _sequence_lock = self.acquire_sequence_lock(session_id).await?;
        let _ = self.refresh_event_log_index(session_id, None).await?;
        let shared = self.event_log_index(session_id).await;
        let occupant = {
            let index = shared.lock().await;
            index
                .transcript_rewrite_receipt_occupants
                .get(&identity)
                .cloned()
        };

        match occupant {
            None => {
                let event = AgentEvent::TranscriptRewriteAuditReceiptCommitted {
                    session_id: session_id.clone(),
                    receipt: receipt.clone(),
                    final_assistant_text: incoming_summary,
                };
                let envelope = EventEnvelope::new_with_source(
                    EventSourceIdentity::session(session_id.clone()),
                    0,
                    None,
                    event,
                );
                let mut inserted = self
                    .append_envelopes_locked(session_id, std::slice::from_ref(&envelope))
                    .await?;
                let stored = inserted.pop().ok_or_else(|| {
                    EventStoreError::Store(
                        "exact transcript rewrite receipt append produced no durable event"
                            .to_string(),
                    )
                })?;
                Ok(ExactTranscriptRewriteReceiptAppend::Inserted(stored))
            }
            Some(ExactTranscriptRewriteReceiptOccupant {
                first_seq,
                count: 1,
                final_assistant_text: existing_summary,
            }) if existing_summary == incoming_summary => {
                let mut rows = self.read_indexed(session_id, first_seq, Some(1)).await?;
                let stored = rows.pop().ok_or_else(|| {
                    EventStoreError::Store(format!(
                        "exact transcript rewrite receipt index points to missing row {first_seq}"
                    ))
                })?;
                let Some((stored_session_id, stored_receipt, stored_summary)) =
                    transcript_rewrite_receipt_event_parts(&stored.event)
                else {
                    return Err(EventStoreError::ExactTranscriptRewriteReceiptConflict {
                        session_id: session_id.clone(),
                        existing_count: 1,
                        reason: format!(
                            "indexed exact receipt row {first_seq} changed event variant"
                        ),
                    });
                };
                if stored.seq != first_seq
                    || stored_session_id != session_id
                    || stored_receipt != receipt
                    || *stored_summary != incoming_summary
                {
                    return Err(EventStoreError::ExactTranscriptRewriteReceiptConflict {
                        session_id: session_id.clone(),
                        existing_count: 1,
                        reason: format!(
                            "indexed exact receipt row {first_seq} does not match its canonical identity"
                        ),
                    });
                }
                Ok(ExactTranscriptRewriteReceiptAppend::Replayed(stored))
            }
            Some(ExactTranscriptRewriteReceiptOccupant {
                first_seq,
                count,
                final_assistant_text: existing_summary,
            }) => Err(EventStoreError::ExactTranscriptRewriteReceiptConflict {
                session_id: session_id.clone(),
                existing_count: count,
                reason: format!(
                    "row {first_seq} carries terminal assistant text {existing_summary:?}, incoming is {incoming_summary:?}"
                ),
            }),
        }
    }

    async fn record_projection_halt(
        &self,
        session_id: &SessionId,
        reason: &str,
    ) -> Result<(), EventStoreError> {
        let marker = EventProjectionHaltMarker {
            session_id: session_id.clone(),
            reason: reason.to_string(),
            recorded_at: SystemTime::now(),
        };
        let path = self.projection_halt_path(session_id);
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let bytes = serde_json::to_vec_pretty(&marker)
            .map_err(|err| EventStoreError::Serialization(err.to_string()))?;
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&path)
            .await?;
        file.write_all(&bytes).await?;
        file.write_all(b"\n").await?;
        file.flush().await?;
        file.sync_all().await?;
        Ok(())
    }

    async fn projection_halt(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<EventProjectionHaltMarker>, EventStoreError> {
        let path = self.projection_halt_path(session_id);
        let contents = match tokio::fs::read_to_string(path).await {
            Ok(contents) => contents,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(EventStoreError::Io(err)),
        };
        serde_json::from_str::<EventProjectionHaltMarker>(&contents)
            .map(Some)
            .map_err(|err| EventStoreError::Serialization(err.to_string()))
    }

    async fn read_from(
        &self,
        session_id: &SessionId,
        from_seq: u64,
    ) -> Result<Vec<StoredEvent>, EventStoreError> {
        self.read_indexed(session_id, from_seq, None).await
    }

    async fn read_raw_from(
        &self,
        session_id: &SessionId,
        from_seq: u64,
    ) -> Result<Option<Vec<RawStoredEvent>>, EventStoreError> {
        self.read_raw_indexed(session_id, from_seq, None)
            .await
            .map(Some)
    }

    async fn read_transcript_rewrite_audit(
        &self,
        session_id: &SessionId,
        expectation: TranscriptRewriteAuditExpectation<'_>,
    ) -> Result<Option<TranscriptRewriteAuditRead>, EventStoreError> {
        let expected_prefix = expectation.expected_prefix();
        if let Some(tail) = self
            .read_authorized_transcript_rewrite_tail(session_id, expected_prefix)
            .await?
        {
            return Ok(Some(TranscriptRewriteAuditRead::AuthorizedTail(tail)));
        }

        let scan = self.full_transcript_rewrite_audit_scan(session_id).await?;
        let current_receipt = scan
            .index
            .current_transcript_rewrite_prefix_receipt(session_id, scan.observed_through_log_seq);
        let legacy_receipt = match expectation {
            TranscriptRewriteAuditExpectation::Current(_) => None,
            TranscriptRewriteAuditExpectation::LegacyGenerationZero {
                ordered_commits, ..
            } => scan.index.legacy_prefix_reconciled_by_expected_graph(
                session_id,
                scan.observed_through_log_seq,
                expected_prefix,
                ordered_commits,
                &scan.rewrite_rows,
            )?,
        };
        let legacy_normalized = legacy_receipt.is_some();
        let mut receipt = legacy_receipt.or(current_receipt);
        if let Some(fingerprint) = scan.fingerprint {
            if let Some(candidate_receipt) = receipt.take() {
                let last_distinct_rewrite_seq = scan
                    .index
                    .transcript_rewrite_prefix
                    .as_ref()
                    .map(|prefix| prefix.last_distinct_rewrite_seq)
                    .unwrap_or_else(|| {
                        scan.index
                            .legacy_transcript_rewrite_commits
                            .iter()
                            .chain(scan.index.transcript_rewrite_commits.values())
                            .map(|row| row.seq)
                            .max()
                            .unwrap_or(0)
                    });
                receipt = Some(
                    self.stage_reconciled_event_log_head(
                        ReconciledEventLogHead {
                            session_id: session_id.clone(),
                            fingerprint,
                            through_log_seq: scan.observed_through_log_seq,
                            last_line: scan.last_line,
                            last_distinct_rewrite_seq,
                            legacy_generation_zero_normalized: legacy_normalized,
                        },
                        candidate_receipt,
                    )
                    .await?,
                );
            }
        } else if scan.fingerprint.is_none() && expected_prefix.occurrence_count() == 0 {
            return Ok(Some(TranscriptRewriteAuditRead::AuthorizedTail(
                TranscriptRewriteAuditRows {
                    session_id: session_id.clone(),
                    observed_through_log_seq: 0,
                    receipt,
                    rewrite_rows: Vec::new(),
                },
            )));
        }

        // A full read always returns every exact row it observed. Its staged
        // head remains private and inert until the consumer validates the
        // bodies, applies replay, and explicitly finalizes the receipt.
        Ok(Some(TranscriptRewriteAuditRead::FullReconciliation(
            TranscriptRewriteAuditRows {
                session_id: session_id.clone(),
                observed_through_log_seq: scan.observed_through_log_seq,
                receipt,
                rewrite_rows: scan.rewrite_rows,
            },
        )))
    }

    async fn finalize_transcript_rewrite_audit(
        &self,
        receipt: &TranscriptRewritePrefixReceipt,
    ) -> Result<(), EventStoreError> {
        self.finalize_pending_rewrite_head(receipt).await
    }

    async fn read_from_bounded(
        &self,
        session_id: &SessionId,
        from_seq: u64,
        max_rows: usize,
    ) -> Result<Vec<StoredEvent>, EventStoreError> {
        self.read_indexed(session_id, from_seq, Some(max_rows))
            .await
    }

    async fn read_page(
        &self,
        session_id: &SessionId,
        from_seq: u64,
        limit: usize,
    ) -> Result<Vec<StoredEvent>, EventStoreError> {
        self.read_indexed(session_id, from_seq, Some(limit)).await
    }

    async fn last_seq(&self, session_id: &SessionId) -> Result<u64, EventStoreError> {
        if let Some(head) = self.read_current_event_log_head(session_id).await? {
            return Ok(head.through_log_seq);
        }
        let (snapshot, _file) = self.refresh_event_log_index(session_id, None).await?;
        Ok(self
            .read_sequence_owner(session_id)
            .await?
            .unwrap_or(snapshot.last_seq)
            .max(snapshot.last_seq))
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    struct LegacyEventStore;

    #[async_trait]
    impl EventStore for LegacyEventStore {
        async fn append_envelopes(
            &self,
            _session_id: &SessionId,
            _envelopes: &[EventEnvelope<AgentEvent>],
        ) -> Result<u64, EventStoreError> {
            Ok(0)
        }

        async fn append_transcript_rewrite_receipt_exact(
            &self,
            _session_id: &SessionId,
            _receipt: &TranscriptRewriteAuditReceiptBatch,
            _final_assistant_text: Option<&str>,
        ) -> Result<ExactTranscriptRewriteReceiptAppend, EventStoreError> {
            Err(EventStoreError::Store(
                "LegacyEventStore does not support exact transcript-rewrite receipt publication"
                    .to_string(),
            ))
        }

        async fn record_projection_halt(
            &self,
            _session_id: &SessionId,
            _reason: &str,
        ) -> Result<(), EventStoreError> {
            Err(EventStoreError::Store(
                "LegacyEventStore does not support durable projection-halt markers".to_string(),
            ))
        }

        async fn projection_halt(
            &self,
            _session_id: &SessionId,
        ) -> Result<Option<EventProjectionHaltMarker>, EventStoreError> {
            Err(EventStoreError::Store(
                "LegacyEventStore does not support durable projection-halt markers".to_string(),
            ))
        }

        async fn read_from(
            &self,
            _session_id: &SessionId,
            _from_seq: u64,
        ) -> Result<Vec<StoredEvent>, EventStoreError> {
            Ok(Vec::new())
        }

        async fn last_seq(&self, _session_id: &SessionId) -> Result<u64, EventStoreError> {
            Ok(0)
        }
    }

    fn new_interaction_id() -> InteractionId {
        InteractionId(meerkat_core::time_compat::new_uuid_v7())
    }

    fn completed_interaction_envelope(
        interaction_id: InteractionId,
        stream_seq: u64,
        mob_id: Option<&str>,
        result: &str,
    ) -> EventEnvelope<AgentEvent> {
        EventEnvelope::new_with_source(
            EventSourceIdentity::interaction(interaction_id),
            stream_seq,
            mob_id.map(ToOwned::to_owned),
            AgentEvent::InteractionComplete {
                interaction_id,
                result: result.to_string(),
                structured_output: None,
            },
        )
    }

    fn completed_interaction_batch(
        interaction_ids: &[InteractionId],
    ) -> Vec<(InteractionId, EventEnvelope<AgentEvent>)> {
        interaction_ids
            .iter()
            .enumerate()
            .map(|(index, interaction_id)| {
                (
                    *interaction_id,
                    completed_interaction_envelope(
                        *interaction_id,
                        10_000 + index as u64,
                        Some("mob-batch"),
                        &format!("result-{index}"),
                    ),
                )
            })
            .collect()
    }

    fn transcript_body(
        text: &str,
        parent_revision: Option<String>,
    ) -> meerkat_core::TranscriptRevisionBody {
        let messages = vec![meerkat_core::Message::User(
            meerkat_core::types::UserMessage::text(text),
        )];
        meerkat_core::TranscriptRevisionBody {
            revision: meerkat_core::transcript_messages_digest(&messages)
                .expect("test transcript body must digest"),
            parent_revision,
            messages,
            created_at: SystemTime::UNIX_EPOCH,
        }
    }

    fn transcript_rewrite_record(
        rewrite_generation: u64,
        parent_text: &str,
        revision_text: &str,
        parent_parent_revision: Option<String>,
        reason: &str,
    ) -> meerkat_core::TranscriptRewriteRecord {
        let parent_body = transcript_body(parent_text, parent_parent_revision);
        let revision_body = transcript_body(revision_text, Some(parent_body.revision.clone()));
        let commit = TranscriptRewriteCommit {
            rewrite_generation,
            parent_revision: parent_body.revision.clone(),
            revision: revision_body.revision.clone(),
            selection: meerkat_core::TranscriptRewriteSelection::MessageRange { start: 0, end: 1 },
            original_span_digest: meerkat_core::transcript_messages_digest(&parent_body.messages)
                .expect("test original span must digest"),
            replacement_digest: meerkat_core::transcript_messages_digest(&revision_body.messages)
                .expect("test replacement span must digest"),
            messages_before: parent_body.messages.len(),
            messages_after: revision_body.messages.len(),
            reason: meerkat_core::TranscriptRewriteReason::new(reason),
            actor: Some("event-store-receipt-test".to_string()),
            committed_at: SystemTime::UNIX_EPOCH,
        };
        meerkat_core::TranscriptRewriteRecord::new(commit, parent_body, revision_body)
            .expect("test rewrite record must validate")
    }

    fn transcript_rewrite_event(
        session_id: &SessionId,
        record: meerkat_core::TranscriptRewriteRecord,
    ) -> AgentEvent {
        AgentEvent::TranscriptRewriteCommitted {
            session_id: session_id.clone(),
            record,
        }
    }

    fn transcript_rewrite_receipt(
        start_prefix: TranscriptRewritePrefixAccumulator,
        commits: &[TranscriptRewriteCommit],
    ) -> TranscriptRewriteAuditReceiptBatch {
        let mut end_prefix = start_prefix.clone();
        for commit in commits {
            end_prefix = end_prefix
                .extend(commit)
                .expect("test rewrite receipt must fold");
        }
        TranscriptRewriteAuditReceiptBatch::new(start_prefix, commits.to_vec(), end_prefix)
            .expect("test rewrite receipt must validate")
    }

    fn assert_invalid_exact_terminal(error: EventStoreError, interaction_id: InteractionId) {
        assert!(
            matches!(
                error,
                EventStoreError::InvalidExactInteractionTerminal {
                    interaction_id: actual,
                    ..
                } if actual == interaction_id
            ),
            "expected invalid exact interaction terminal, got {error:?}"
        );
    }

    #[tokio::test]
    async fn legacy_event_store_implementors_compile_and_exact_publication_fails_closed() {
        let store = LegacyEventStore;
        let session_id = SessionId::new();
        let interaction_id = new_interaction_id();
        let envelope = completed_interaction_envelope(interaction_id, 1, None, "done");

        let error = store
            .append_interaction_terminal_exact(&session_id, interaction_id, &envelope)
            .await
            .expect_err("legacy stores cannot silently claim exact terminal publication");
        assert!(
            matches!(
                &error,
                EventStoreError::Store(message)
                    if message.contains("does not implement exact interaction terminal publication")
            ),
            "default exact-terminal capability must fail closed, got {error:?}"
        );
        assert!(
            store
                .read_transcript_rewrite_audit(
                    &session_id,
                    TranscriptRewriteAuditExpectation::Current(
                        &TranscriptRewritePrefixAccumulator::empty(),
                    ),
                )
                .await
                .expect("the default receipt capability must not fail")
                .is_none(),
            "a custom store must not silently claim rewrite-prefix authority"
        );
    }

    #[test]
    fn public_authorized_tail_constructor_rejects_omission_and_conflicting_retry()
    -> Result<(), Box<dyn std::error::Error>> {
        let session_id = SessionId::new();
        let first = transcript_rewrite_record(1, "A", "B", None, "first");
        let second = transcript_rewrite_record(
            2,
            "B",
            "C",
            Some(first.commit.parent_revision.clone()),
            "second",
        );
        let first_prefix =
            TranscriptRewritePrefixAccumulator::from_commits(std::slice::from_ref(&first.commit))?;
        let second_prefix = TranscriptRewritePrefixAccumulator::from_commits(&[
            first.commit.clone(),
            second.commit.clone(),
        ])?;
        let second_receipt = TranscriptRewritePrefixReceipt::new(
            session_id.clone(),
            2,
            second_prefix,
            Some(second.commit.clone()),
        )?;

        let omission = TranscriptRewriteAuditRead::authorized_tail(
            &first_prefix,
            Some(&first.commit),
            second_receipt,
            Vec::new(),
        )
        .expect_err("an end receipt cannot authorize omitted exact tail rows");
        assert!(
            matches!(
                &omission,
                EventStoreError::Store(message)
                    if message.contains("does not fold exactly")
            ),
            "expected exact-fold rejection, got {omission:?}"
        );

        let conflicting = transcript_rewrite_record(1, "A", "X", None, "conflicting-first");
        let conflicting_row = RawTranscriptRewriteEvent::new(
            2,
            serde_json::value::to_raw_value(&transcript_rewrite_event(&session_id, conflicting))?,
        )?;
        let first_receipt = TranscriptRewritePrefixReceipt::new(
            session_id,
            2,
            first_prefix.clone(),
            Some(first.commit.clone()),
        )?;
        let conflict = TranscriptRewriteAuditRead::authorized_tail(
            &first_prefix,
            Some(&first.commit),
            first_receipt,
            vec![conflicting_row],
        )
        .expect_err("same-generation unequal facts are corruption, not retries");
        assert!(
            matches!(
                &conflict,
                EventStoreError::Store(message)
                    if message.contains("conflicts with occurrence generation 1")
            ),
            "expected generation-conflict rejection, got {conflict:?}"
        );
        Ok(())
    }

    #[tokio::test]
    async fn rewrite_generation_conflict_is_rejected_before_jsonl_write_without_a_head()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let root = temp.path().join("events");
        let store = FileEventStore::new(&root);
        let session_id = SessionId::new();
        let first = transcript_rewrite_record(1, "A", "B", None, "first");
        let first_receipt = transcript_rewrite_receipt(
            TranscriptRewritePrefixAccumulator::empty(),
            std::slice::from_ref(&first.commit),
        );
        store
            .append_transcript_rewrite_receipt_exact(&session_id, &first_receipt, None)
            .await?;

        tokio::fs::remove_file(store.event_log_head_path(&session_id)).await?;
        let log_path = store.log_path(&session_id);
        let before = tokio::fs::read(&log_path).await?;
        let conflicting = transcript_rewrite_record(1, "A", "X", None, "conflicting-first");
        let conflicting_receipt = transcript_rewrite_receipt(
            TranscriptRewritePrefixAccumulator::empty(),
            std::slice::from_ref(&conflicting.commit),
        );
        let error = store
            .append_transcript_rewrite_receipt_exact(&session_id, &conflicting_receipt, None)
            .await
            .expect_err("a conflicting occurrence must fail before its JSONL append");
        assert!(
            matches!(
                error,
                EventStoreError::TranscriptRewriteGenerationConflict { generation: 1, .. }
            ),
            "expected typed generation conflict, got {error:?}"
        );
        assert_eq!(
            tokio::fs::read(&log_path).await?,
            before,
            "conflict preflight must leave canonical JSONL byte-for-byte unchanged"
        );
        Ok(())
    }

    #[tokio::test]
    async fn full_body_rewrite_writer_is_rejected_before_write_with_empty_or_existing_head()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let root = temp.path().join("events");
        let store = FileEventStore::new(&root);
        let session_id = SessionId::new();

        let first = transcript_rewrite_record(1, "A", "B", None, "first");
        let empty_error = store
            .append(
                &session_id,
                &[transcript_rewrite_event(&session_id, first.clone())],
            )
            .await
            .expect_err("current writers must not seed a full-body rewrite row");
        assert!(
            matches!(
                &empty_error,
                EventStoreError::Store(message)
                    if message.contains("refuse full-body")
            ),
            "expected full-body writer rejection, got {empty_error:?}"
        );
        assert!(
            !store.log_path(&session_id).exists(),
            "full-body writer rejection must precede canonical JSONL creation"
        );

        let first_receipt = transcript_rewrite_receipt(
            TranscriptRewritePrefixAccumulator::empty(),
            std::slice::from_ref(&first.commit),
        );
        store
            .append_transcript_rewrite_receipt_exact(&session_id, &first_receipt, None)
            .await?;
        let log_path = store.log_path(&session_id);
        let before = tokio::fs::read(&log_path).await?;
        let second = transcript_rewrite_record(
            2,
            "B",
            "C",
            Some(first.commit.parent_revision.clone()),
            "second",
        );
        let existing_error = store
            .append(
                &session_id,
                &[transcript_rewrite_event(&session_id, second)],
            )
            .await
            .expect_err("a full-body successor must not extend receipt authority");
        assert!(
            matches!(
                &existing_error,
                EventStoreError::Store(message)
                    if message.contains("refuse full-body")
            ),
            "expected full-body writer rejection, got {existing_error:?}"
        );
        assert_eq!(
            tokio::fs::read(&log_path).await?,
            before,
            "rejected full-body successor must leave canonical JSONL byte-for-byte unchanged"
        );
        let expected =
            TranscriptRewritePrefixAccumulator::from_commits(std::slice::from_ref(&first.commit))?;
        assert_eq!(
            store
                .read_durable_event_log_head(&session_id)
                .await?
                .expect("valid first append has a head")
                .rewrite_prefix
                .as_ref(),
            Some(&expected),
            "invalid successor must not advance semantic head authority"
        );
        Ok(())
    }

    #[tokio::test]
    async fn append_fallback_does_not_authorize_unvalidated_historical_rewrite_bodies()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let root = temp.path().join("events");
        let store = FileEventStore::new(&root);
        let session_id = SessionId::new();
        let mut corrupt = transcript_rewrite_record(1, "A", "B", None, "first");
        corrupt
            .revision_body
            .messages
            .push(meerkat_core::Message::User(
                meerkat_core::types::UserMessage::text("corrupt-on-disk-body"),
            ));
        let commit = corrupt.commit.clone();
        append_raw_test_rows(
            &store,
            &session_id,
            &[StoredEvent {
                seq: 1,
                schema_version: EVENT_SCHEMA_VERSION,
                timestamp: SystemTime::now(),
                source: EventSourceIdentity::session(session_id.clone()),
                mob_id: None,
                stream_seq: 1,
                event: transcript_rewrite_event(&session_id, corrupt),
            }],
        )
        .await?;

        store
            .append(&session_id, &[AgentEvent::TurnStarted { turn_number: 1 }])
            .await?;
        let head = store
            .read_durable_event_log_head(&session_id)
            .await?
            .expect("ordinary append publishes positional high-water");
        assert_eq!(head.through_log_seq, 2);
        assert!(
            head.rewrite_prefix.is_none(),
            "index-only fallback must not mint semantic authority over historical bodies"
        );

        let expected =
            TranscriptRewritePrefixAccumulator::from_commits(std::slice::from_ref(&commit))?;
        let audit = store
            .read_transcript_rewrite_audit(
                &session_id,
                TranscriptRewriteAuditExpectation::Current(&expected),
            )
            .await?
            .expect("file store supports combined audit");
        assert!(
            matches!(audit, TranscriptRewriteAuditRead::FullReconciliation(_)),
            "unvalidated historical bodies must remain visible to a full reconciliation"
        );

        let anchored_session = SessionId::new();
        store
            .append(
                &anchored_session,
                &[AgentEvent::TurnStarted { turn_number: 1 }],
            )
            .await?;
        let mut anchored_corrupt = transcript_rewrite_record(1, "A", "B", None, "anchored-first");
        anchored_corrupt
            .revision_body
            .messages
            .push(meerkat_core::Message::User(
                meerkat_core::types::UserMessage::text("corrupt-anchored-suffix"),
            ));
        append_raw_test_rows(
            &store,
            &anchored_session,
            &[StoredEvent {
                seq: 2,
                schema_version: EVENT_SCHEMA_VERSION,
                timestamp: SystemTime::now(),
                source: EventSourceIdentity::session(anchored_session.clone()),
                mob_id: None,
                stream_seq: 2,
                event: transcript_rewrite_event(&anchored_session, anchored_corrupt),
            }],
        )
        .await?;
        store
            .append(
                &anchored_session,
                &[AgentEvent::TurnStarted { turn_number: 2 }],
            )
            .await?;
        assert!(
            store
                .read_durable_event_log_head(&anchored_session)
                .await?
                .expect("ordinary append advances positional high-water")
                .rewrite_prefix
                .is_none(),
            "an append must not publish semantic authority over an unvalidated suffix after an exact older head"
        );
        Ok(())
    }

    #[tokio::test]
    async fn authorized_rewrite_tail_binds_loop_occurrences_and_cold_no_tail_decodes_zero_rows()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let root = temp.path().join("events");
        let store = FileEventStore::new(&root);
        let session_id = SessionId::new();
        let a_to_b = transcript_rewrite_record(1, "A", "B", None, "a-to-b");
        let b_to_a = transcript_rewrite_record(
            2,
            "B",
            "A",
            Some(a_to_b.commit.parent_revision.clone()),
            "b-to-a",
        );

        let empty_prefix = TranscriptRewritePrefixAccumulator::empty();
        let first_receipt =
            transcript_rewrite_receipt(empty_prefix, std::slice::from_ref(&a_to_b.commit));
        store
            .append_transcript_rewrite_receipt_exact(&session_id, &first_receipt, None)
            .await?;
        let before_prefix = first_receipt.end_prefix().clone();
        let before_loop = store
            .read_transcript_rewrite_audit(
                &session_id,
                TranscriptRewriteAuditExpectation::Current(&before_prefix),
            )
            .await?
            .expect("file store implements combined rewrite audit");
        let TranscriptRewriteAuditRead::AuthorizedTail(before_loop) = before_loop else {
            return Err(std::io::Error::other("current prefix must authorize a tail").into());
        };

        let second_receipt =
            transcript_rewrite_receipt(before_prefix.clone(), std::slice::from_ref(&b_to_a.commit));
        store
            .append_transcript_rewrite_receipt_exact(&session_id, &second_receipt, None)
            .await?;
        let replayed = store
            .append_transcript_rewrite_receipt_exact(&session_id, &first_receipt, None)
            .await?;
        assert!(
            matches!(replayed, ExactTranscriptRewriteReceiptAppend::Replayed(_)),
            "an exact older receipt retry must reuse its row"
        );
        let expected = TranscriptRewritePrefixAccumulator::from_commits(&[
            a_to_b.commit.clone(),
            b_to_a.commit.clone(),
        ])?;
        let after_loop = store
            .read_transcript_rewrite_audit(
                &session_id,
                TranscriptRewriteAuditExpectation::Current(&expected),
            )
            .await?
            .expect("file store implements combined rewrite audit");
        let TranscriptRewriteAuditRead::AuthorizedTail(after_loop) = after_loop else {
            return Err(
                std::io::Error::other("exact receipt replay must retain O(1) authority").into(),
            );
        };
        let after_receipt = after_loop.receipt().expect("authorized tail has a receipt");
        let before_receipt = before_loop
            .receipt()
            .expect("authorized tail has a receipt");

        assert_eq!(after_receipt.session_id(), &session_id);
        assert_eq!(after_receipt.through_log_seq(), 2);
        assert_eq!(
            after_receipt.accumulator().occurrence_count(),
            2,
            "the exact retry must not become a third occurrence"
        );
        assert_eq!(after_receipt.accumulator(), &expected);
        assert_ne!(
            after_receipt.accumulator(),
            before_receipt.accumulator(),
            "the B -> A return must change the prefix despite the exact A -> B replay"
        );
        store
            .finalize_transcript_rewrite_audit(after_receipt)
            .await?;

        store
            .append(&session_id, &[AgentEvent::TurnStarted { turn_number: 1 }])
            .await?;
        let restarted = FileEventStore::new(store.root());
        restarted.reset_decoded_rows();
        let ordinary_high_water = restarted
            .read_transcript_rewrite_audit(
                &session_id,
                TranscriptRewriteAuditExpectation::Current(&expected),
            )
            .await?
            .expect("the event-log head binds an ordinary-event high-water");
        let TranscriptRewriteAuditRead::AuthorizedTail(ordinary_high_water) = ordinary_high_water
        else {
            return Err(std::io::Error::other("cold exact head must authorize a tail").into());
        };
        assert_eq!(ordinary_high_water.observed_through_log_seq(), 3);
        assert!(ordinary_high_water.rewrite_rows().is_empty());
        assert_eq!(
            ordinary_high_water
                .receipt()
                .expect("authorized tail has a receipt")
                .accumulator(),
            &expected
        );
        assert_eq!(
            restarted.decoded_rows(),
            0,
            "fresh-store combined no-tail read must not build an index or decode a JSONL row"
        );

        let a_to_c =
            transcript_rewrite_record(3, "A", "C", Some(a_to_b.commit.revision.clone()), "a-to-c");
        append_raw_test_rows(
            &restarted,
            &session_id,
            &[
                StoredEvent {
                    seq: 4,
                    schema_version: EVENT_SCHEMA_VERSION,
                    timestamp: SystemTime::now(),
                    source: EventSourceIdentity::session(session_id.clone()),
                    mob_id: None,
                    stream_seq: 4,
                    event: AgentEvent::TurnStarted { turn_number: 2 },
                },
                StoredEvent {
                    seq: 5,
                    schema_version: EVENT_SCHEMA_VERSION,
                    timestamp: SystemTime::now(),
                    source: EventSourceIdentity::session(session_id.clone()),
                    mob_id: None,
                    stream_seq: 5,
                    event: transcript_rewrite_event(&session_id, a_to_c.clone()),
                },
            ],
        )
        .await?;
        let tail_store = FileEventStore::new(&root);
        tail_store.reset_decoded_rows();
        let tail = tail_store
            .read_transcript_rewrite_audit(
                &session_id,
                TranscriptRewriteAuditExpectation::Current(&expected),
            )
            .await?
            .expect("stale event-log head still authorizes its exact suffix");
        tail.verify_authorized_tail(&expected, Some(&b_to_a.commit))?;
        let TranscriptRewriteAuditRead::AuthorizedTail(tail) = tail else {
            return Err(std::io::Error::other("append-only suffix must stay a tail read").into());
        };
        let expected_after_tail = TranscriptRewritePrefixAccumulator::from_commits(&[
            a_to_b.commit,
            b_to_a.commit,
            a_to_c.commit,
        ])?;
        assert_eq!(tail.observed_through_log_seq(), 5);
        assert_eq!(tail.rewrite_rows().len(), 1);
        assert_eq!(
            tail.receipt()
                .expect("authorized tail has a receipt")
                .accumulator(),
            &expected_after_tail
        );
        assert_eq!(
            tail_store.decoded_rows(),
            2,
            "direct seek must decode only the two appended rows"
        );
        assert!(
            !tail_store.index_registry_contains(&session_id).await,
            "authorized direct-tail read must not construct the broad event-log index"
        );
        assert_eq!(
            tail_store
                .read_durable_event_log_head(&session_id)
                .await?
                .expect("the earlier head remains durable until validation")
                .through_log_seq,
            3,
            "reading a tail must not publish its high-water before body validation"
        );
        tail_store
            .finalize_transcript_rewrite_audit(
                tail.receipt()
                    .expect("validated authorized tail has a receipt"),
            )
            .await?;
        assert_eq!(
            tail_store
                .read_durable_event_log_head(&session_id)
                .await?
                .expect("finalization publishes the validated head")
                .through_log_seq,
            5
        );
        Ok(())
    }

    #[tokio::test]
    async fn rewrite_receipt_repair_requires_one_exact_ordered_missing_suffix()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let root = temp.path().join("events");
        let store = FileEventStore::new(&root);
        let session_id = SessionId::new();
        let a_to_b = transcript_rewrite_record(1, "A", "B", None, "a-to-b");
        let b_to_c = transcript_rewrite_record(
            2,
            "B",
            "C",
            Some(a_to_b.commit.parent_revision.clone()),
            "b-to-c",
        );

        let first_prefix =
            TranscriptRewritePrefixAccumulator::from_commits(std::slice::from_ref(&a_to_b.commit))?;
        let descendant_only =
            transcript_rewrite_receipt(first_prefix, std::slice::from_ref(&b_to_c.commit));
        store
            .append_transcript_rewrite_receipt_exact(&session_id, &descendant_only, None)
            .await
            .expect_err("a descendant-only receipt cannot skip its missing ancestor");
        assert!(
            !store.log_path(&session_id).exists(),
            "rejected descendant-only evidence must not create canonical JSONL"
        );

        // Graph repair publishes the complete missing semantic suffix in one
        // receipt row, preserving order without body rows or physical-row
        // sorting.
        let repaired_receipt = transcript_rewrite_receipt(
            TranscriptRewritePrefixAccumulator::empty(),
            &[a_to_b.commit.clone(), b_to_c.commit.clone()],
        );
        store
            .append_transcript_rewrite_receipt_exact(&session_id, &repaired_receipt, None)
            .await?;
        let expected = TranscriptRewritePrefixAccumulator::from_commits(&[
            a_to_b.commit.clone(),
            b_to_c.commit.clone(),
        ])?;
        assert_eq!(
            store
                .read_durable_event_log_head(&session_id)
                .await?
                .expect("exact repair receipt advances semantic authority")
                .rewrite_prefix
                .as_ref(),
            Some(&expected),
            "one exact missing-suffix receipt must publish its proved end prefix"
        );

        let warm = store
            .read_transcript_rewrite_audit(
                &session_id,
                TranscriptRewriteAuditExpectation::Current(&expected),
            )
            .await?
            .expect("warm store supports combined audit");
        let TranscriptRewriteAuditRead::AuthorizedTail(warm) = warm else {
            return Err(std::io::Error::other(
                "exact receipt repair must stay on the O(1) read path",
            )
            .into());
        };
        assert!(warm.rewrite_rows().is_empty());
        assert_eq!(
            warm.receipt()
                .expect("authorized tail has a receipt")
                .accumulator(),
            &expected
        );
        let restarted = FileEventStore::new(&root);
        restarted.reset_decoded_rows();
        let rebuilt = restarted
            .read_transcript_rewrite_audit(
                &session_id,
                TranscriptRewriteAuditExpectation::Current(&expected),
            )
            .await?
            .expect("cold store supports combined audit");
        let TranscriptRewriteAuditRead::AuthorizedTail(rebuilt) = rebuilt else {
            return Err(std::io::Error::other("durable head must bind repaired prefix").into());
        };
        assert_eq!(
            rebuilt
                .receipt()
                .expect("authorized tail has a receipt")
                .accumulator(),
            &expected
        );
        assert_eq!(
            restarted.decoded_rows(),
            0,
            "a valid event-log head must avoid a cold JSONL rebuild"
        );
        Ok(())
    }

    #[tokio::test]
    async fn legacy_generation_zero_receipt_uses_the_same_body_authorized_heal_as_replay()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let root = temp.path().join("events");
        let store = FileEventStore::new(&root);
        let session_id = SessionId::new();
        let parent_messages = vec![
            meerkat_core::Message::User(meerkat_core::types::UserMessage::text("old one")),
            meerkat_core::Message::User(meerkat_core::types::UserMessage::text("old two")),
        ];
        let revision_messages = vec![meerkat_core::Message::User(
            meerkat_core::types::UserMessage::compaction_summary("summary"),
        )];
        let parent_body = meerkat_core::TranscriptRevisionBody {
            revision: meerkat_core::transcript_messages_digest(&parent_messages)?,
            parent_revision: None,
            messages: parent_messages,
            created_at: SystemTime::UNIX_EPOCH,
        };
        let revision_body = meerkat_core::TranscriptRevisionBody {
            revision: meerkat_core::transcript_messages_digest(&revision_messages)?,
            parent_revision: Some(parent_body.revision.clone()),
            messages: revision_messages,
            created_at: SystemTime::UNIX_EPOCH,
        };
        let commit = TranscriptRewriteCommit {
            rewrite_generation: 1,
            parent_revision: parent_body.revision.clone(),
            revision: revision_body.revision.clone(),
            selection: meerkat_core::TranscriptRewriteSelection::MessageRange {
                start: 0,
                end: parent_body.messages.len(),
            },
            original_span_digest: meerkat_core::transcript_messages_digest(&parent_body.messages)?,
            replacement_digest: meerkat_core::transcript_messages_digest(&revision_body.messages)?,
            messages_before: parent_body.messages.len(),
            messages_after: revision_body.messages.len(),
            reason: meerkat_core::TranscriptRewriteReason::new("legacy-compaction"),
            actor: None,
            committed_at: SystemTime::UNIX_EPOCH,
        };
        let mut legacy_record =
            meerkat_core::TranscriptRewriteRecord::new(commit, parent_body, revision_body)?;
        legacy_record.commit.rewrite_generation = 0;
        let legacy_event = transcript_rewrite_event(&session_id, legacy_record);

        // Full payload decode owns compatibility healing because the retained
        // bodies are the authority for classifying a legacy compaction.
        let encoded = serde_json::to_string(&legacy_event)?;
        let AgentEvent::TranscriptRewriteCommitted {
            record: healed_record,
            ..
        } = serde_json::from_str::<AgentEvent>(&encoded)?
        else {
            return Err(std::io::Error::other("legacy event changed variant").into());
        };
        assert!(matches!(
            healed_record.commit.selection,
            meerkat_core::TranscriptRewriteSelection::CompactionMessageRange { .. }
        ));
        let mut expected_commit = healed_record.commit;
        expected_commit.rewrite_generation = 1;
        let expected_commits = vec![expected_commit.clone()];
        let expected_prefix = TranscriptRewritePrefixAccumulator::from_commits(&expected_commits)?;

        append_raw_test_rows(
            &store,
            &session_id,
            &[StoredEvent {
                seq: 1,
                schema_version: EVENT_SCHEMA_VERSION,
                timestamp: SystemTime::now(),
                source: EventSourceIdentity::session(session_id.clone()),
                mob_id: None,
                stream_seq: 1,
                event: legacy_event,
            }],
        )
        .await?;

        let migrated = store
            .read_transcript_rewrite_audit(
                &session_id,
                TranscriptRewriteAuditExpectation::LegacyGenerationZero {
                    expected_prefix: &expected_prefix,
                    ordered_commits: &expected_commits,
                },
            )
            .await?
            .expect("file store supports generation-zero migration");
        let TranscriptRewriteAuditRead::FullReconciliation(migrated) = migrated else {
            return Err(
                std::io::Error::other("legacy healing requires one full reconciliation").into(),
            );
        };
        let receipt = migrated
            .receipt()
            .expect("healed exact payloads must produce a staged receipt");
        assert_eq!(receipt.accumulator(), &expected_prefix);
        store.finalize_transcript_rewrite_audit(receipt).await?;
        assert!(
            store
                .read_durable_event_log_head(&session_id)
                .await?
                .expect("validated migration publishes a durable head")
                .legacy_generation_zero_normalized
        );

        let reopened = FileEventStore::new(&root);
        reopened.reset_decoded_rows();
        let steady = reopened
            .read_transcript_rewrite_audit(
                &session_id,
                TranscriptRewriteAuditExpectation::Current(&expected_prefix),
            )
            .await?
            .expect("normalized head authorizes the current prefix");
        let TranscriptRewriteAuditRead::AuthorizedTail(steady) = steady else {
            return Err(std::io::Error::other(
                "normalized legacy head must avoid repeated full reconciliation",
            )
            .into());
        };
        assert!(steady.rewrite_rows().is_empty());
        assert_eq!(
            reopened.decoded_rows(),
            0,
            "a normalized legacy head must not decode historical JSONL rows"
        );
        Ok(())
    }

    #[tokio::test]
    async fn corrupt_event_log_head_falls_back_and_repairs_from_jsonl()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let root = temp.path().join("events");
        let store = FileEventStore::new(&root);
        let session_id = SessionId::new();
        let a_to_b = transcript_rewrite_record(1, "A", "B", None, "a-to-b");
        let receipt = transcript_rewrite_receipt(
            TranscriptRewritePrefixAccumulator::empty(),
            std::slice::from_ref(&a_to_b.commit),
        );
        store
            .append_transcript_rewrite_receipt_exact(&session_id, &receipt, None)
            .await?;
        let expected = receipt.end_prefix().clone();

        let sidecar_path = store.event_log_head_path(&session_id);
        let mut sidecar: serde_json::Value =
            serde_json::from_slice(&tokio::fs::read(&sidecar_path).await?)?;
        sidecar["checksum"] = serde_json::json!("sha256:corrupt");
        tokio::fs::write(&sidecar_path, serde_json::to_vec(&sidecar)?).await?;

        let rebuilding = FileEventStore::new(&root);
        rebuilding.reset_decoded_rows();
        let repaired = rebuilding
            .read_transcript_rewrite_audit(
                &session_id,
                TranscriptRewriteAuditExpectation::Current(&expected),
            )
            .await?
            .expect("canonical JSONL reconciliation supplies the result");
        let TranscriptRewriteAuditRead::FullReconciliation(repaired) = repaired else {
            return Err(std::io::Error::other(
                "an invalid sidecar must force one full receipt-log reconciliation",
            )
            .into());
        };
        assert!(
            repaired.rewrite_rows().is_empty(),
            "current receipt-only reconciliation must not expose rewrite bodies"
        );
        assert_eq!(
            repaired
                .receipt()
                .expect("authorized tail has a receipt")
                .accumulator(),
            &expected
        );
        assert_eq!(
            rebuilding.decoded_rows(),
            1,
            "an invalid sidecar must decode the one receipt row exactly once"
        );
        assert!(
            rebuilding
                .read_durable_event_log_head(&session_id)
                .await?
                .is_none(),
            "a read must not publish skip authority before receipt validation succeeds"
        );
        rebuilding
            .finalize_transcript_rewrite_audit(
                repaired
                    .receipt()
                    .expect("validated full reconciliation has a receipt"),
            )
            .await?;
        rebuilding
            .finalize_transcript_rewrite_audit(
                repaired
                    .receipt()
                    .expect("validated full reconciliation has a receipt"),
            )
            .await?;

        let reopened = FileEventStore::new(&root);
        reopened.reset_decoded_rows();
        let durable = reopened
            .read_transcript_rewrite_audit(
                &session_id,
                TranscriptRewriteAuditExpectation::Current(&expected),
            )
            .await?
            .expect("the fallback rebuild repairs the durable event-log head");
        let TranscriptRewriteAuditRead::AuthorizedTail(durable) = durable else {
            return Err(std::io::Error::other("repaired head must authorize a tail").into());
        };
        assert_eq!(
            durable
                .receipt()
                .expect("authorized tail has a receipt")
                .accumulator(),
            &expected
        );
        assert_eq!(
            reopened.decoded_rows(),
            0,
            "the repaired event-log head must make the next cold lookup bounded"
        );
        Ok(())
    }

    async fn append_raw_test_rows(
        store: &FileEventStore,
        session_id: &SessionId,
        rows: &[StoredEvent],
    ) -> Result<(), Box<dyn std::error::Error>> {
        tokio::fs::create_dir_all(store.root()).await?;
        let path = store.log_path(session_id);
        let mut bytes = Vec::new();
        for row in rows {
            serde_json::to_writer(&mut bytes, row)?;
            bytes.push(b'\n');
        }
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .await?;
        file.write_all(&bytes).await?;
        file.flush().await?;
        file.sync_all().await?;
        if let Some(last) = rows.last() {
            store.write_sequence_owner(session_id, last.seq).await?;
        }
        Ok(())
    }

    #[tokio::test]
    async fn exact_interaction_append_rejects_non_interaction_source()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let store = FileEventStore::new(temp.path().join("events"));
        let session_id = SessionId::new();
        let interaction_id = new_interaction_id();
        let envelope = EventEnvelope::new_with_source(
            EventSourceIdentity::session(session_id.clone()),
            1,
            None,
            AgentEvent::InteractionComplete {
                interaction_id,
                result: "done".to_string(),
                structured_output: None,
            },
        );

        let error = store
            .append_interaction_terminal_exact(&session_id, interaction_id, &envelope)
            .await
            .expect_err("session source must not enter the exact interaction keyspace");
        assert_invalid_exact_terminal(error, interaction_id);
        assert_eq!(store.last_seq(&session_id).await?, 0);
        Ok(())
    }

    #[tokio::test]
    async fn exact_interaction_append_rejects_payload_id_mismatch()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let store = FileEventStore::new(temp.path().join("events"));
        let session_id = SessionId::new();
        let interaction_id = new_interaction_id();
        let other_id = new_interaction_id();
        let envelope = EventEnvelope::new_with_source(
            EventSourceIdentity::interaction(interaction_id),
            1,
            None,
            AgentEvent::InteractionComplete {
                interaction_id: other_id,
                result: "wrong identity".to_string(),
                structured_output: None,
            },
        );

        let error = store
            .append_interaction_terminal_exact(&session_id, interaction_id, &envelope)
            .await
            .expect_err("payload identity mismatch must fail before append");
        assert_invalid_exact_terminal(error, interaction_id);
        assert_eq!(store.last_seq(&session_id).await?, 0);
        Ok(())
    }

    #[tokio::test]
    async fn exact_interaction_append_rejects_nonterminal_payload()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let store = FileEventStore::new(temp.path().join("events"));
        let session_id = SessionId::new();
        let interaction_id = new_interaction_id();
        let envelope = EventEnvelope::new_with_source(
            EventSourceIdentity::interaction(interaction_id),
            1,
            None,
            AgentEvent::TextComplete {
                content: "not terminal".to_string(),
            },
        );

        let error = store
            .append_interaction_terminal_exact(&session_id, interaction_id, &envelope)
            .await
            .expect_err("nonterminal payload must fail before append");
        assert_invalid_exact_terminal(error, interaction_id);
        assert_eq!(store.last_seq(&session_id).await?, 0);
        Ok(())
    }

    #[tokio::test]
    async fn exact_interaction_append_replays_canonical_row_ignoring_stream_seq()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let store = FileEventStore::new(temp.path().join("events"));
        let session_id = SessionId::new();
        let interaction_id = new_interaction_id();
        let first = completed_interaction_envelope(interaction_id, 7, Some("mob-a"), "done");
        let replay = completed_interaction_envelope(interaction_id, 99, Some("mob-a"), "done");

        let inserted = store
            .append_interaction_terminal_exact(&session_id, interaction_id, &first)
            .await?;
        assert!(matches!(inserted, ExactInteractionAppend::Inserted(_)));
        let replayed = store
            .append_interaction_terminal_exact(&session_id, interaction_id, &replay)
            .await?;
        let ExactInteractionAppend::Replayed(canonical) = replayed else {
            return Err(
                std::io::Error::other("identical terminal must replay the canonical row").into(),
            );
        };
        assert_eq!(canonical.seq, 1);
        assert_eq!(canonical.stream_seq, 7, "caller retry sequence is ignored");
        assert_eq!(canonical.mob_id.as_deref(), Some("mob-a"));
        assert_eq!(store.read_from(&session_id, 0).await?.len(), 1);
        Ok(())
    }

    #[tokio::test]
    async fn generic_single_terminal_append_replays_the_exact_row()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let store = FileEventStore::new(temp.path().join("events"));
        let session_id = SessionId::new();
        let interaction_id = new_interaction_id();
        let first = completed_interaction_envelope(interaction_id, 7, None, "done");
        let replay = completed_interaction_envelope(interaction_id, 88, None, "done");

        let inserted = store
            .append_interaction_terminal_exact(&session_id, interaction_id, &first)
            .await?;
        assert!(matches!(inserted, ExactInteractionAppend::Inserted(_)));
        let replay_seq = store.append_envelopes(&session_id, &[replay]).await?;

        assert_eq!(replay_seq, 1);
        assert_eq!(store.read_from(&session_id, 0).await?.len(), 1);
        Ok(())
    }

    #[tokio::test]
    async fn generic_interaction_batch_cannot_bypass_exact_uniqueness()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let store = FileEventStore::new(temp.path().join("events"));
        let session_id = SessionId::new();
        let interaction_id = new_interaction_id();
        let terminal = completed_interaction_envelope(interaction_id, 2, None, "done");
        let ordinary = EventEnvelope::new_with_source(
            EventSourceIdentity::session(session_id.clone()),
            1,
            None,
            AgentEvent::TurnStarted { turn_number: 1 },
        );

        let error = store
            .append_envelopes(&session_id, &[ordinary, terminal])
            .await
            .expect_err("mixed generic batch must not enter the exact interaction keyspace");
        assert_invalid_exact_terminal(error, interaction_id);
        assert_eq!(store.last_seq(&session_id).await?, 0);

        let nonterminal = EventEnvelope::new_with_source(
            EventSourceIdentity::interaction(interaction_id),
            3,
            None,
            AgentEvent::TextComplete {
                content: "not a terminal".to_string(),
            },
        );
        let error = store
            .append_envelopes(&session_id, &[nonterminal])
            .await
            .expect_err("Interaction source must not carry a generic nonterminal event");
        assert_invalid_exact_terminal(error, interaction_id);
        assert_eq!(store.last_seq(&session_id).await?, 0);
        Ok(())
    }

    #[tokio::test]
    async fn exact_interaction_append_rejects_payload_or_mob_mismatch()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let store = FileEventStore::new(temp.path().join("events"));
        let session_id = SessionId::new();
        let first_id = new_interaction_id();
        let first = completed_interaction_envelope(first_id, 1, Some("mob-a"), "done");
        store
            .append_interaction_terminal_exact(&session_id, first_id, &first)
            .await?;

        let payload_mismatch =
            completed_interaction_envelope(first_id, 2, Some("mob-a"), "different");
        let error = store
            .append_interaction_terminal_exact(&session_id, first_id, &payload_mismatch)
            .await
            .expect_err("same identity with a different terminal payload must conflict");
        assert!(matches!(
            error,
            EventStoreError::ExactInteractionTerminalConflict {
                interaction_id: actual,
                existing_count: 1,
                ..
            } if actual == first_id
        ));

        let second_id = new_interaction_id();
        let second = completed_interaction_envelope(second_id, 3, Some("mob-a"), "done");
        store
            .append_interaction_terminal_exact(&session_id, second_id, &second)
            .await?;
        let cross_mob = completed_interaction_envelope(second_id, 4, Some("mob-b"), "done");
        let error = store
            .append_interaction_terminal_exact(&session_id, second_id, &cross_mob)
            .await
            .expect_err("same payload under a different mob identity must conflict");
        assert!(matches!(
            error,
            EventStoreError::ExactInteractionTerminalConflict {
                interaction_id: actual,
                existing_count: 1,
                ..
            } if actual == second_id
        ));
        Ok(())
    }

    #[tokio::test]
    async fn exact_interaction_append_rejects_corrupt_or_duplicate_occupants()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let store = FileEventStore::new(temp.path().join("events"));
        let session_id = SessionId::new();
        let corrupt_id = new_interaction_id();
        let duplicate_id = new_interaction_id();
        append_raw_test_rows(
            &store,
            &session_id,
            &[
                StoredEvent {
                    seq: 1,
                    schema_version: EVENT_SCHEMA_VERSION,
                    timestamp: SystemTime::now(),
                    source: EventSourceIdentity::interaction(corrupt_id),
                    mob_id: None,
                    stream_seq: 1,
                    event: AgentEvent::TextComplete {
                        content: "corrupt occupant".to_string(),
                    },
                },
                StoredEvent {
                    seq: 2,
                    schema_version: EVENT_SCHEMA_VERSION,
                    timestamp: SystemTime::now(),
                    source: EventSourceIdentity::interaction(duplicate_id),
                    mob_id: None,
                    stream_seq: 2,
                    event: AgentEvent::InteractionComplete {
                        interaction_id: duplicate_id,
                        result: "same".to_string(),
                        structured_output: None,
                    },
                },
                StoredEvent {
                    seq: 3,
                    schema_version: EVENT_SCHEMA_VERSION,
                    timestamp: SystemTime::now(),
                    source: EventSourceIdentity::interaction(duplicate_id),
                    mob_id: None,
                    stream_seq: 3,
                    event: AgentEvent::InteractionComplete {
                        interaction_id: duplicate_id,
                        result: "same".to_string(),
                        structured_output: None,
                    },
                },
            ],
        )
        .await?;
        let valid = completed_interaction_envelope(corrupt_id, 2, None, "done");
        let error = store
            .append_interaction_terminal_exact(&session_id, corrupt_id, &valid)
            .await
            .expect_err("a corrupt exact-source occupant must block replacement");
        assert!(matches!(
            error,
            EventStoreError::ExactInteractionTerminalConflict {
                interaction_id: actual,
                existing_count: 1,
                ..
            } if actual == corrupt_id
        ));

        let replay = completed_interaction_envelope(duplicate_id, 4, None, "same");
        let error = store
            .append_interaction_terminal_exact(&session_id, duplicate_id, &replay)
            .await
            .expect_err("multiple exact-source rows must never collapse to a replay");
        assert!(matches!(
            error,
            EventStoreError::ExactInteractionTerminalConflict {
                interaction_id: actual,
                existing_count: 2,
                ..
            } if actual == duplicate_id
        ));
        Ok(())
    }

    #[tokio::test]
    async fn concurrent_exact_interaction_appends_insert_once_and_replay_the_rest()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let root = temp.path().join("events");
        let session_id = SessionId::new();
        let interaction_id = new_interaction_id();
        let mut tasks = Vec::new();
        for stream_seq in 1..=8 {
            let root = root.clone();
            let session_id = session_id.clone();
            tasks.push(tokio::spawn(async move {
                let store = FileEventStore::new(root);
                let envelope = completed_interaction_envelope(
                    interaction_id,
                    stream_seq,
                    Some("mob-a"),
                    "done",
                );
                store
                    .append_interaction_terminal_exact(&session_id, interaction_id, &envelope)
                    .await
            }));
        }

        let mut inserted = 0;
        let mut replayed = 0;
        let mut canonical_seqs = Vec::new();
        for task in tasks {
            match task.await?? {
                ExactInteractionAppend::Inserted(row) => {
                    inserted += 1;
                    canonical_seqs.push(row.seq);
                }
                ExactInteractionAppend::Replayed(row) => {
                    replayed += 1;
                    canonical_seqs.push(row.seq);
                }
            }
        }
        assert_eq!(inserted, 1);
        assert_eq!(replayed, 7);
        assert!(canonical_seqs.iter().all(|seq| *seq == 1));

        let store = FileEventStore::new(root);
        let rows = store.read_from(&session_id, 0).await?;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].seq, 1);
        Ok(())
    }

    #[tokio::test]
    async fn exact_interaction_batch_accepts_256_and_rejects_larger_or_duplicate_input()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let store = FileEventStore::new(temp.path().join("events"));
        let session_id = SessionId::new();
        let ids: Vec<_> = (0..MAX_EXACT_INTERACTION_TERMINAL_BATCH)
            .map(|_| new_interaction_id())
            .collect();
        let batch = completed_interaction_batch(&ids);

        let results = store
            .append_interaction_terminals_exact_batch(&session_id, 7, &batch)
            .await?;
        assert_eq!(results.len(), MAX_EXACT_INTERACTION_TERMINAL_BATCH);
        assert!(
            results
                .iter()
                .all(|result| matches!(result, ExactInteractionAppend::Inserted(_)))
        );
        let rows = store.read_from(&session_id, 0).await?;
        assert_eq!(rows.len(), MAX_EXACT_INTERACTION_TERMINAL_BATCH);
        assert_eq!(rows.first().map(|row| row.stream_seq), Some(8));
        assert_eq!(rows.last().map(|row| row.stream_seq), Some(263));

        let oversized_ids: Vec<_> = (0..=MAX_EXACT_INTERACTION_TERMINAL_BATCH)
            .map(|_| new_interaction_id())
            .collect();
        let oversized = completed_interaction_batch(&oversized_ids);
        let error = store
            .append_interaction_terminals_exact_batch(&session_id, 263, &oversized)
            .await
            .expect_err("257 terminals must fail before any occupant lookup or append");
        assert!(matches!(
            error,
            EventStoreError::InvalidExactInteractionTerminalBatch { .. }
        ));
        assert_eq!(store.last_seq(&session_id).await?, 256);

        let duplicate = vec![batch[0].clone(), batch[0].clone()];
        let error = store
            .append_interaction_terminals_exact_batch(&session_id, 263, &duplicate)
            .await
            .expect_err("duplicate batch identities must fail closed");
        assert!(matches!(
            error,
            EventStoreError::InvalidExactInteractionTerminalBatch { .. }
        ));
        assert_eq!(store.last_seq(&session_id).await?, 256);
        Ok(())
    }

    #[tokio::test]
    async fn exact_interaction_batch_recovers_canonical_prefix_and_rejects_holes()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let root = temp.path().join("events");
        let store = FileEventStore::new(&root);
        let session_id = SessionId::new();
        let ids: Vec<_> = (0..4).map(|_| new_interaction_id()).collect();
        let mut batch = completed_interaction_batch(&ids);
        batch[0].1.seq = 41;
        batch[1].1.seq = 42;
        store
            .append_interaction_terminal_exact(&session_id, ids[0], &batch[0].1)
            .await?;
        store
            .append_interaction_terminal_exact(&session_id, ids[1], &batch[1].1)
            .await?;

        let recovered = store
            .append_interaction_terminals_exact_batch(&session_id, 10, &batch)
            .await?;
        assert!(matches!(recovered[0], ExactInteractionAppend::Replayed(_)));
        assert!(matches!(recovered[1], ExactInteractionAppend::Replayed(_)));
        assert!(matches!(recovered[2], ExactInteractionAppend::Inserted(_)));
        assert!(matches!(recovered[3], ExactInteractionAppend::Inserted(_)));
        let rows = store.read_from(&session_id, 0).await?;
        assert_eq!(rows.len(), 4);
        assert_eq!(
            rows.iter().map(|row| row.stream_seq).collect::<Vec<_>>(),
            vec![41, 42, 43, 44]
        );

        let replayed = store
            .append_interaction_terminals_exact_batch(&session_id, 100, &batch)
            .await?;
        assert!(
            replayed
                .iter()
                .all(|result| matches!(result, ExactInteractionAppend::Replayed(_)))
        );
        assert_eq!(store.read_from(&session_id, 0).await?.len(), 4);

        let hole_session = SessionId::new();
        let hole_ids: Vec<_> = (0..3).map(|_| new_interaction_id()).collect();
        let mut hole_batch = completed_interaction_batch(&hole_ids);
        hole_batch[1].1.seq = 12;
        store
            .append_interaction_terminal_exact(&hole_session, hole_ids[1], &hole_batch[1].1)
            .await?;
        let error = store
            .append_interaction_terminals_exact_batch(&hole_session, 10, &hole_batch)
            .await
            .expect_err("an existing row after a missing item is not a canonical prefix");
        assert!(matches!(
            error,
            EventStoreError::InvalidExactInteractionTerminalBatch { .. }
        ));
        let hole_rows = store.read_from(&hole_session, 0).await?;
        assert_eq!(
            hole_rows.len(),
            1,
            "the missing prefix must not be inserted"
        );
        assert_eq!(
            hole_rows[0].source,
            EventSourceIdentity::interaction(hole_ids[1])
        );
        Ok(())
    }

    #[tokio::test]
    async fn exact_interaction_batch_conflict_or_corrupt_occupant_inserts_no_suffix()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let store = FileEventStore::new(temp.path().join("events"));
        let session_id = SessionId::new();
        let ids: Vec<_> = (0..3).map(|_| new_interaction_id()).collect();
        let mut batch = completed_interaction_batch(&ids);
        batch[0].1.seq = 1;
        store
            .append_interaction_terminal_exact(&session_id, ids[0], &batch[0].1)
            .await?;
        let conflicting =
            completed_interaction_envelope(ids[1], 2, Some("mob-batch"), "conflicting-result");
        store
            .append_interaction_terminal_exact(&session_id, ids[1], &conflicting)
            .await?;

        let error = store
            .append_interaction_terminals_exact_batch(&session_id, 0, &batch)
            .await
            .expect_err("a conflicting prefix occupant must reject the whole batch");
        assert!(matches!(
            error,
            EventStoreError::ExactInteractionTerminalConflict {
                interaction_id,
                existing_count: 1,
                ..
            } if interaction_id == ids[1]
        ));
        assert_eq!(store.read_from(&session_id, 0).await?.len(), 2);

        let corrupt_session = SessionId::new();
        append_raw_test_rows(
            &store,
            &corrupt_session,
            &[StoredEvent {
                seq: 1,
                schema_version: EVENT_SCHEMA_VERSION,
                timestamp: SystemTime::now(),
                source: EventSourceIdentity::interaction(ids[0]),
                mob_id: Some("mob-batch".to_string()),
                stream_seq: 1,
                event: AgentEvent::TextComplete {
                    content: "corrupt occupant".to_string(),
                },
            }],
        )
        .await?;
        let error = store
            .append_interaction_terminals_exact_batch(&corrupt_session, 0, &batch)
            .await
            .expect_err("a corrupt exact-source occupant must reject the whole batch");
        assert!(matches!(
            error,
            EventStoreError::ExactInteractionTerminalConflict {
                interaction_id,
                existing_count: 1,
                ..
            } if interaction_id == ids[0]
        ));
        assert_eq!(store.read_from(&corrupt_session, 0).await?.len(), 1);
        Ok(())
    }

    #[tokio::test]
    async fn concurrent_exact_interaction_batches_are_all_insert_or_all_replay()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let root = temp.path().join("events");
        let session_id = SessionId::new();
        let ids: Vec<_> = (0..32).map(|_| new_interaction_id()).collect();
        let batch = completed_interaction_batch(&ids);
        let mut tasks = Vec::new();
        for _ in 0..2 {
            let root = root.clone();
            let session_id = session_id.clone();
            let batch = batch.clone();
            tasks.push(tokio::spawn(async move {
                FileEventStore::new(root)
                    .append_interaction_terminals_exact_batch(&session_id, 0, &batch)
                    .await
            }));
        }
        let first = tasks.remove(0).await??;
        let second = tasks.remove(0).await??;
        let inserted_counts = [first, second].map(|results| {
            results
                .iter()
                .filter(|result| matches!(result, ExactInteractionAppend::Inserted(_)))
                .count()
        });
        assert!(
            inserted_counts == [32, 0] || inserted_counts == [0, 32],
            "one locked batch must insert every row and the other must replay every row: {inserted_counts:?}"
        );
        let rows = FileEventStore::new(root).read_from(&session_id, 0).await?;
        assert_eq!(rows.len(), 32);
        assert_eq!(rows.first().map(|row| row.stream_seq), Some(1));
        assert_eq!(rows.last().map(|row| row.stream_seq), Some(32));
        Ok(())
    }

    #[tokio::test]
    async fn warm_exact_interaction_index_avoids_full_log_rescans()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let root = temp.path().join("events");
        let session_id = SessionId::new();
        let writer = FileEventStore::new(&root);
        let history: Vec<_> = (0..2_048)
            .map(|turn_number| AgentEvent::TurnStarted { turn_number })
            .collect();
        writer.append(&session_id, &history).await?;

        // A separately constructed store starts with a cold reconstructable
        // index, so the first exact append validates and decodes the history.
        let store = FileEventStore::new(&root);
        store.reset_decoded_rows();
        let first_id = new_interaction_id();
        let first = completed_interaction_envelope(first_id, 1, None, "first");
        store
            .append_interaction_terminal_exact(&session_id, first_id, &first)
            .await?;
        assert!(
            store.decoded_rows() >= 2_048,
            "cold exact lookup must reconstruct the validated occupant index"
        );

        // The second identity lookup is O(1) over the validated occupancy map;
        // it must not decode the long event history again.
        store.reset_decoded_rows();
        let second_id = new_interaction_id();
        let second = completed_interaction_envelope(second_id, 2, None, "second");
        store
            .append_interaction_terminal_exact(&session_id, second_id, &second)
            .await?;
        assert_eq!(
            store.decoded_rows(),
            0,
            "warm exact append must not rescan the canonical log"
        );
        Ok(())
    }

    #[tokio::test]
    async fn file_event_store_appends_and_reads_session_log()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let store = FileEventStore::new(temp.path().join("events"));
        let session_id = SessionId::new();

        let seq = store
            .append(
                &session_id,
                &[
                    AgentEvent::TurnStarted { turn_number: 1 },
                    AgentEvent::TextComplete {
                        content: "durable event".to_string(),
                    },
                ],
            )
            .await?;

        assert_eq!(seq, 2);
        assert_eq!(store.last_seq(&session_id).await?, 2);
        let events = store.read_from(&session_id, 2).await?;
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0].event, AgentEvent::TextComplete { .. }));
        let page = store.read_page(&session_id, 1, 1).await?;
        assert_eq!(page.len(), 1);
        assert_eq!(page[0].seq, 1);
        assert!(store.root().join(format!("{session_id}.jsonl")).exists());
        Ok(())
    }

    /// The raw read and the typed read must agree about which rows a log holds
    /// and in what order: the coverage decision is taken on one of them and the
    /// rebuild runs on the other.
    #[tokio::test]
    async fn file_event_store_raw_read_agrees_with_the_typed_read()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let store = FileEventStore::new(temp.path().join("events"));
        let session_id = SessionId::new();
        store
            .append(
                &session_id,
                &[
                    AgentEvent::TurnStarted { turn_number: 1 },
                    AgentEvent::TextComplete {
                        content: "durable event".to_string(),
                    },
                    AgentEvent::TurnStarted { turn_number: 2 },
                ],
            )
            .await?;

        for from_seq in [0_u64, 1, 2, 3, 4] {
            let typed = store.read_from(&session_id, from_seq).await?;
            let raw = store
                .read_raw_from(&session_id, from_seq)
                .await?
                .expect("a file-backed log reads raw");
            assert_eq!(
                raw.iter().map(|row| row.seq).collect::<Vec<_>>(),
                typed.iter().map(|row| row.seq).collect::<Vec<_>>(),
                "raw and typed reads disagree about the rows from {from_seq}"
            );
            for (raw_row, typed_row) in raw.iter().zip(typed.iter()) {
                let reparsed: AgentEvent = serde_json::from_str(raw_row.event.get())?;
                assert_eq!(
                    serde_json::to_value(&reparsed)?,
                    serde_json::to_value(&typed_row.event)?,
                    "raw payload at seq {} does not carry the typed event",
                    raw_row.seq
                );
            }
        }
        Ok(())
    }

    /// An unparsed payload is not an unchecked row: the raw read applies the
    /// same schema-version gate the typed read does.
    #[tokio::test]
    async fn file_event_store_raw_read_rejects_a_foreign_schema_version()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let root = temp.path().join("events");
        let session_id = SessionId::new();
        let store = FileEventStore::new(&root);
        store
            .append(&session_id, &[AgentEvent::TurnStarted { turn_number: 1 }])
            .await?;

        let path = root.join(format!("{session_id}.jsonl"));
        let contents = tokio::fs::read_to_string(&path).await?;
        let mut row: serde_json::Value = serde_json::from_str(contents.trim())?;
        row["schema_version"] = serde_json::json!(EVENT_SCHEMA_VERSION + 7);
        tokio::fs::write(&path, format!("{row}\n")).await?;

        let error = FileEventStore::new(&root)
            .read_raw_from(&session_id, 0)
            .await
            .expect_err("a foreign schema version must fail the raw read closed");
        assert!(
            matches!(error, EventStoreError::SchemaVersionMismatch { .. }),
            "unexpected error: {error}"
        );
        Ok(())
    }

    /// A store that does not implement the raw read keeps behaving exactly as
    /// it did; callers fall back to the typed read.
    #[tokio::test]
    async fn a_store_without_a_raw_read_reports_absence_rather_than_failing()
    -> Result<(), Box<dyn std::error::Error>> {
        let session_id = SessionId::new();
        assert!(
            LegacyEventStore
                .read_raw_from(&session_id, 0)
                .await?
                .is_none(),
            "the default raw read must report absence, not an error"
        );
        Ok(())
    }

    #[tokio::test]
    async fn file_event_store_arbitrary_page_hint_cannot_force_capacity_panic()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let store = FileEventStore::new(temp.path().join("events"));
        let session_id = SessionId::new();
        store
            .append(&session_id, &[AgentEvent::TurnStarted { turn_number: 1 }])
            .await?;

        let rows = store.read_from_bounded(&session_id, 1, usize::MAX).await?;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].seq, 1);
        Ok(())
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn file_event_store_warmed_pages_decode_only_one_stride_plus_page()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let store = FileEventStore::new(temp.path().join("events"));
        let session_id = SessionId::new();
        let events: Vec<_> = (0..4_096)
            .map(|turn_number| AgentEvent::TurnStarted { turn_number })
            .collect();
        store.append(&session_id, &events).await?;
        assert_eq!(store.last_seq(&session_id).await?, 4_096);

        let page_len = 17;
        let stride = usize::try_from(EVENT_LOG_INDEX_STRIDE)?;
        for from_seq in [1, 1_000, 2_047, 3_777, 4_090] {
            store.reset_decoded_rows();
            let page = store
                .read_from_bounded(&session_id, from_seq, page_len)
                .await?;
            let expected_len = usize::try_from(4_097_u64.saturating_sub(from_seq))?.min(page_len);
            assert_eq!(page.len(), expected_len);
            assert_eq!(page.first().map(|row| row.seq), Some(from_seq));
            assert!(
                store.decoded_rows() <= stride + page_len,
                "warmed page from {from_seq} decoded {} rows (stride={stride}, page={page_len})",
                store.decoded_rows()
            );
        }

        store.reset_decoded_rows();
        assert_eq!(store.last_seq(&session_id).await?, 4_096);
        assert_eq!(
            store.decoded_rows(),
            0,
            "warmed last_seq must be an indexed O(1) read"
        );
        Ok(())
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn file_event_store_restart_rebuilds_once_then_pages_and_tail_are_indexed()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let root = temp.path().join("events");
        let session_id = SessionId::new();
        let store = FileEventStore::new(&root);
        let events: Vec<_> = (0..512)
            .map(|turn_number| AgentEvent::TurnStarted { turn_number })
            .collect();
        store.append(&session_id, &events).await?;

        let restarted = FileEventStore::new(&root);
        restarted.reset_decoded_rows();
        assert_eq!(restarted.last_seq(&session_id).await?, 512);
        assert_eq!(
            restarted.decoded_rows(),
            512,
            "a restarted in-memory index performs one validating rebuild"
        );

        restarted.reset_decoded_rows();
        let page = restarted.read_from_bounded(&session_id, 477, 11).await?;
        assert_eq!(page.first().map(|row| row.seq), Some(477));
        assert_eq!(page.last().map(|row| row.seq), Some(487));
        assert!(restarted.decoded_rows() <= usize::try_from(EVENT_LOG_INDEX_STRIDE)? + 11);
        restarted.reset_decoded_rows();
        assert_eq!(restarted.last_seq(&session_id).await?, 512);
        assert_eq!(restarted.decoded_rows(), 0);
        Ok(())
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn file_event_store_index_tracks_clone_and_independent_append_growth()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let root = temp.path().join("events");
        let session_id = SessionId::new();
        let store = FileEventStore::new(&root);
        let initial: Vec<_> = (0..128)
            .map(|turn_number| AgentEvent::TurnStarted { turn_number })
            .collect();
        store.append(&session_id, &initial).await?;
        assert_eq!(store.last_seq(&session_id).await?, 128);

        let cloned = store.clone();
        let clone_growth: Vec<_> = (128..192)
            .map(|turn_number| AgentEvent::TurnStarted { turn_number })
            .collect();
        store.reset_decoded_rows();
        cloned.append(&session_id, &clone_growth).await?;
        assert_eq!(
            store.decoded_rows(),
            0,
            "a coordinated clone append extends the exact pre-append fingerprint mechanically"
        );
        store.reset_decoded_rows();
        assert_eq!(store.last_seq(&session_id).await?, 192);
        assert_eq!(
            store.decoded_rows(),
            0,
            "clones share and mechanically extend the same sparse index"
        );

        // A separately constructed store models another service instance. Its
        // append cannot share this registry, so the original store must fully
        // revalidate the grown file rather than trust its cached prefix.
        let independent = FileEventStore::new(&root);
        let independent_growth: Vec<_> = (192..200)
            .map(|turn_number| AgentEvent::TurnStarted { turn_number })
            .collect();
        independent.append(&session_id, &independent_growth).await?;

        store.reset_decoded_rows();
        let page = store.read_from_bounded(&session_id, 195, 5).await?;
        assert_eq!(
            page.iter().map(|row| row.seq).collect::<Vec<_>>(),
            vec![195, 196, 197, 198, 199]
        );
        assert_eq!(
            store.decoded_rows(),
            207,
            "independent growth rebuilds all 200 rows, then seeks seven rows from checkpoint 193"
        );
        Ok(())
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn file_event_store_truncation_invalidates_and_rebuilds_warmed_index()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let root = temp.path().join("events");
        let session_id = SessionId::new();
        let store = FileEventStore::new(&root);
        let events: Vec<_> = (0..256)
            .map(|turn_number| AgentEvent::TurnStarted { turn_number })
            .collect();
        store.append(&session_id, &events).await?;
        assert_eq!(store.last_seq(&session_id).await?, 256);

        let path = store.log_path(&session_id);
        let bytes = tokio::fs::read(&path).await?;
        let truncate_at = bytes
            .iter()
            .enumerate()
            .filter_map(|(index, byte)| (*byte == b'\n').then_some(index + 1))
            .nth(79)
            .expect("256-row log has an 80th newline");
        tokio::fs::write(&path, &bytes[..truncate_at]).await?;

        store.reset_decoded_rows();
        assert_eq!(store.last_seq(&session_id).await?, 80);
        assert_eq!(
            store.decoded_rows(),
            80,
            "a shorter canonical log must invalidate and rebuild the stale index"
        );
        store.reset_decoded_rows();
        let page = store.read_from_bounded(&session_id, 70, 5).await?;
        assert_eq!(
            page.iter().map(|row| row.seq).collect::<Vec<_>>(),
            vec![70, 71, 72, 73, 74]
        );
        assert!(store.decoded_rows() <= usize::try_from(EVENT_LOG_INDEX_STRIDE)? + 5);
        Ok(())
    }

    #[tokio::test]
    async fn file_event_store_index_registry_is_bounded_and_eviction_is_reconstructable()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let store = FileEventStore::new(temp.path().join("events"));
        let retained_session = SessionId::new();
        store
            .append(
                &retained_session,
                &[AgentEvent::TurnStarted { turn_number: 1 }],
            )
            .await?;
        assert_eq!(store.last_seq(&retained_session).await?, 1);
        let retained_index = store.event_log_index(&retained_session).await;

        for _ in 0..EVENT_LOG_INDEX_CACHE_CAPACITY {
            assert_eq!(store.last_seq(&SessionId::new()).await?, 0);
        }
        assert_eq!(
            store.index_registry_len().await,
            EVENT_LOG_INDEX_CACHE_CAPACITY
        );
        assert!(
            !store.index_registry_contains(&retained_session).await,
            "the least-recently-used entry should be evicted at the cap"
        );
        assert_eq!(
            retained_index.lock().await.last_seq,
            1,
            "an in-flight Arc remains valid after registry eviction"
        );

        store.reset_decoded_rows();
        assert_eq!(store.last_seq(&retained_session).await?, 1);
        assert_eq!(
            store.decoded_rows(),
            1,
            "an evicted entry reconstructs from the canonical JSONL"
        );
        assert_eq!(
            store.index_registry_len().await,
            EVENT_LOG_INDEX_CACHE_CAPACITY
        );
        Ok(())
    }

    #[tokio::test]
    async fn file_event_store_append_note_never_clobbers_a_newer_validated_index()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let store = FileEventStore::new(temp.path().join("events"));
        let session_id = SessionId::new();
        let initial: Vec<_> = (0..128)
            .map(|turn_number| AgentEvent::TurnStarted { turn_number })
            .collect();
        store.append(&session_id, &initial).await?;
        assert_eq!(store.last_seq(&session_id).await?, 128);

        let path = store.log_path(&session_id);
        let before = FileEventStore::event_log_fingerprint(&path)
            .await?
            .expect("event log fingerprint before independent append");
        let appended = StoredEvent {
            seq: 129,
            schema_version: EVENT_SCHEMA_VERSION,
            timestamp: SystemTime::now(),
            source: EventSourceIdentity::external("cooperative-reader-race"),
            mob_id: None,
            stream_seq: 129,
            event: AgentEvent::TurnStarted { turn_number: 129 },
        };
        let mut appended_bytes = serde_json::to_vec(&appended)?;
        appended_bytes.push(b'\n');
        let mut file = tokio::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .await?;
        file.write_all(&appended_bytes).await?;
        file.flush().await?;
        file.sync_all().await?;
        let after = FileEventStore::event_log_fingerprint_from_metadata(&file.metadata().await?);
        drop(file);

        // Model a reader winning the race between the writer's fsync and its
        // cooperative `note_appended_rows`: it rebuilds the shared cache all
        // the way to the post-append state first.
        let (rebuilt, _) = store.refresh_event_log_index(&session_id, None).await?;
        assert_eq!(rebuilt.last_seq, 129);
        store
            .note_appended_rows(&EventLogAppend {
                session_id: &session_id,
                pre_fingerprint: before,
                post_fingerprint: after,
                bytes: &appended_bytes,
                rows: &[AppendedIndexRow {
                    seq: 129,
                    relative_offset: 0,
                    byte_len: u64::try_from(appended_bytes.len())?,
                }],
                stored_events: std::slice::from_ref(&appended),
            })
            .await;

        store.reset_decoded_rows();
        assert_eq!(store.last_seq(&session_id).await?, 129);
        assert_eq!(
            store.decoded_rows(),
            0,
            "a delayed append note must preserve the newer validated index"
        );
        Ok(())
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn file_event_store_page_scan_stays_on_the_validated_open_file()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let store = FileEventStore::new(temp.path().join("events"));
        let session_id = SessionId::new();
        let events: Vec<_> = (0..128)
            .map(|turn_number| AgentEvent::TurnStarted { turn_number })
            .collect();
        store.append(&session_id, &events).await?;
        assert_eq!(store.last_seq(&session_id).await?, 128);

        let path = store.log_path(&session_id);
        let (snapshot, opened) = store
            .refresh_event_log_index(&session_id, Some(120))
            .await?;
        let mut opened = opened.expect("nonempty validated log has an open file snapshot");
        let expected = snapshot.fingerprint.expect("validated fingerprint");

        // Atomically replace the pathname after validation with a same-length
        // log whose skipped prefix is invalid while retaining the original
        // tail. The in-flight page must read the already-validated descriptor,
        // never seek into this replacement using stale checkpoints.
        let mut replacement_bytes = tokio::fs::read(&path).await?;
        let needle = b"\"schema_version\":2";
        let position = replacement_bytes
            .windows(needle.len())
            .position(|window| window == needle)
            .expect("first row carries current schema version");
        replacement_bytes[position + needle.len() - 1] = b'3';
        let replacement_path = path.with_extension("snapshot-race");
        tokio::fs::write(&replacement_path, &replacement_bytes).await?;
        tokio::fs::rename(&replacement_path, &path).await?;

        let (page, observed) = store
            .read_index_snapshot(
                &path,
                &mut opened,
                snapshot,
                120,
                Some(1),
                &|store, line| store.decode_event_line(line).map(|row| (row.seq, row)),
            )
            .await?;
        assert_eq!(page.first().map(|row| row.seq), Some(120));
        assert_eq!(observed.device, expected.device);
        assert_eq!(observed.inode, expected.inode);
        assert_eq!(observed.len, expected.len);
        assert_eq!(observed.modified, expected.modified);
        // macOS updates the unlinked-but-open inode's ctime when rename
        // replaces its pathname. That intentional fingerprint mismatch makes
        // the production read discard this internally consistent old page and
        // retry against the replacement; identity above proves no stale
        // checkpoint was ever applied to replacement bytes.

        let error = store
            .read_from_bounded(&session_id, 120, 1)
            .await
            .expect_err("the next snapshot must rebuild and inspect the replacement prefix");
        assert!(matches!(
            error,
            EventStoreError::SchemaVersionMismatch {
                expected: EVENT_SCHEMA_VERSION,
                found: 3,
            }
        ));
        Ok(())
    }

    #[tokio::test]
    async fn file_event_store_same_length_prefix_corruption_with_restored_mtime_rebuilds()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let store = FileEventStore::new(temp.path().join("events"));
        let session_id = SessionId::new();
        let events: Vec<_> = (0..128)
            .map(|turn_number| AgentEvent::TurnStarted { turn_number })
            .collect();
        store.append(&session_id, &events).await?;
        assert_eq!(store.last_seq(&session_id).await?, 128);

        let path = store.log_path(&session_id);
        let before = FileEventStore::event_log_fingerprint(&path)
            .await?
            .expect("event log fingerprint");
        let mut bytes = tokio::fs::read(&path).await?;
        let needle = b"\"schema_version\":2";
        let position = bytes
            .windows(needle.len())
            .position(|window| window == needle)
            .expect("first row carries current schema version");
        bytes[position + needle.len() - 1] = b'3';
        tokio::fs::write(&path, &bytes).await?;
        let file = std::fs::OpenOptions::new().write(true).open(&path)?;
        let original_modified = before.modified.expect("test filesystem exposes mtime");
        file.set_times(std::fs::FileTimes::new().set_modified(original_modified))?;
        file.sync_all()?;
        let after = FileEventStore::event_log_fingerprint(&path)
            .await?
            .expect("replacement fingerprint");
        assert_eq!(after.len, before.len, "replacement preserves byte length");
        assert_eq!(after.modified, before.modified, "test restores mtime");
        #[cfg(unix)]
        {
            assert_eq!(after.device, before.device);
            assert_eq!(after.inode, before.inode);
            assert_ne!(
                (after.ctime_seconds, after.ctime_nanoseconds),
                (before.ctime_seconds, before.ctime_nanoseconds),
                "native ctime must expose the in-place corruption"
            );
            assert_ne!(after, before);
        }

        store.reset_decoded_rows();
        let error = store
            .read_from_bounded(&session_id, 120, 1)
            .await
            .expect_err("native fingerprint change must rebuild and inspect the skipped prefix");
        assert!(matches!(
            error,
            EventStoreError::SchemaVersionMismatch {
                expected: EVENT_SCHEMA_VERSION,
                found: 3,
            }
        ));
        assert_eq!(
            store.decoded_rows(),
            1,
            "rebuild fails closed at the first replaced row"
        );
        Ok(())
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn file_event_store_atomic_same_length_replacement_invalidates_native_identity()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let store = FileEventStore::new(temp.path().join("events"));
        let session_id = SessionId::new();
        let events: Vec<_> = (0..128)
            .map(|turn_number| AgentEvent::TurnStarted { turn_number })
            .collect();
        store.append(&session_id, &events).await?;
        assert_eq!(store.last_seq(&session_id).await?, 128);

        let path = store.log_path(&session_id);
        let before = FileEventStore::event_log_fingerprint(&path)
            .await?
            .expect("event log fingerprint");
        let original_modified = before.modified.expect("test filesystem exposes mtime");
        let mut bytes = tokio::fs::read(&path).await?;
        let needle = b"\"schema_version\":2";
        let position = bytes
            .windows(needle.len())
            .position(|window| window == needle)
            .expect("first row carries current schema version");
        bytes[position + needle.len() - 1] = b'3';

        let replacement_path = path.with_extension("replacement");
        let mut replacement = tokio::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&replacement_path)
            .await?;
        replacement.write_all(&bytes).await?;
        replacement.flush().await?;
        replacement.sync_all().await?;
        drop(replacement);
        let replacement = std::fs::OpenOptions::new()
            .write(true)
            .open(&replacement_path)?;
        replacement.set_times(std::fs::FileTimes::new().set_modified(original_modified))?;
        replacement.sync_all()?;
        drop(replacement);
        tokio::fs::rename(&replacement_path, &path).await?;

        let after = FileEventStore::event_log_fingerprint(&path)
            .await?
            .expect("replacement fingerprint");
        assert_eq!(after.len, before.len, "replacement preserves byte length");
        assert_eq!(after.modified, before.modified, "test restores mtime");
        assert_eq!(after.device, before.device);
        assert_ne!(
            after.inode, before.inode,
            "atomic replacement must change native file identity"
        );
        assert_ne!(after, before);

        store.reset_decoded_rows();
        let error = store
            .read_from_bounded(&session_id, 120, 1)
            .await
            .expect_err("native identity change must rebuild and inspect the skipped prefix");
        assert!(matches!(
            error,
            EventStoreError::SchemaVersionMismatch {
                expected: EVENT_SCHEMA_VERSION,
                found: 3,
            }
        ));
        assert_eq!(
            store.decoded_rows(),
            1,
            "replacement rebuild fails closed at the first row"
        );
        Ok(())
    }

    #[tokio::test]
    async fn file_event_store_prefix_corruption_followed_by_independent_growth_revalidates_from_zero()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let store = FileEventStore::new(temp.path().join("events"));
        let session_id = SessionId::new();
        let events: Vec<_> = (0..128)
            .map(|turn_number| AgentEvent::TurnStarted { turn_number })
            .collect();
        store.append(&session_id, &events).await?;
        assert_eq!(store.last_seq(&session_id).await?, 128);

        let path = store.log_path(&session_id);
        let before = FileEventStore::event_log_fingerprint(&path)
            .await?
            .expect("event log fingerprint");
        let mut bytes = tokio::fs::read(&path).await?;
        let needle = b"\"schema_version\":2";
        let position = bytes
            .windows(needle.len())
            .position(|window| window == needle)
            .expect("first row carries current schema version");
        bytes[position + needle.len() - 1] = b'3';
        tokio::fs::write(&path, &bytes).await?;
        if let Some(original_modified) = before.modified {
            let file = std::fs::OpenOptions::new().write(true).open(&path)?;
            file.set_times(std::fs::FileTimes::new().set_modified(original_modified))?;
            file.sync_all()?;
        }

        // Model a writer that owns the same canonical log but does not share
        // this process's reconstructable index registry.
        let independent_row = StoredEvent {
            seq: 129,
            schema_version: EVENT_SCHEMA_VERSION,
            timestamp: SystemTime::now(),
            source: EventSourceIdentity::external("independent-test-writer"),
            mob_id: None,
            stream_seq: 129,
            event: AgentEvent::TurnStarted { turn_number: 129 },
        };
        let mut independent_bytes = serde_json::to_vec(&independent_row)?;
        independent_bytes.push(b'\n');
        let mut independent = tokio::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .await?;
        independent.write_all(&independent_bytes).await?;
        independent.flush().await?;
        independent.sync_all().await?;
        drop(independent);

        store.reset_decoded_rows();
        let error = store
            .read_from_bounded(&session_id, 129, 1)
            .await
            .expect_err("independent growth must revalidate the cached prefix from byte zero");
        assert!(matches!(
            error,
            EventStoreError::SchemaVersionMismatch {
                expected: EVENT_SCHEMA_VERSION,
                found: 3,
            }
        ));
        assert_eq!(
            store.decoded_rows(),
            1,
            "full revalidation fails closed at the corrupted first row"
        );
        Ok(())
    }

    #[tokio::test]
    async fn file_event_store_persists_projection_halt_marker()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let root = temp.path().join("events");
        let store = FileEventStore::new(&root);
        let session_id = SessionId::new();

        store
            .record_projection_halt(&session_id, "synthetic append failure")
            .await?;

        let restarted = FileEventStore::new(&root);
        let marker = restarted
            .projection_halt(&session_id)
            .await?
            .expect("halt marker should survive store restart");
        assert_eq!(marker.session_id, session_id);
        assert_eq!(marker.reason, "synthetic append failure");
        Ok(())
    }

    #[tokio::test]
    async fn file_event_store_round_trips_tool_config_changed_event()
    -> Result<(), Box<dyn std::error::Error>> {
        // Regression guard for the AgentEvent log replay path. A persisted
        // ToolConfigChanged event (the current `status_info`-bearing shape) must
        // survive the JSONL append -> read_from round trip. Pre-`status_info`
        // (v0.4-v0.5) logs that recorded only the legacy `status` string are
        // intentionally NOT resumable (a clean pre-1.0 break documented in the
        // CHANGELOG); this pins that the CURRENT shape replays cleanly and does
        // not silently regress.
        let temp = tempfile::tempdir()?;
        let store = FileEventStore::new(temp.path().join("events"));
        let session_id = SessionId::new();

        let payload = meerkat_core::ToolConfigChangedPayload::new(
            meerkat_core::ToolConfigChangeOperation::Add,
            "shell",
            meerkat_core::ToolConfigChangeStatus::boundary_applied(true, false, 7),
            true,
        );
        store
            .append(
                &session_id,
                &[
                    AgentEvent::TurnStarted { turn_number: 1 },
                    AgentEvent::ToolConfigChanged {
                        payload: payload.clone(),
                    },
                ],
            )
            .await?;

        let events = store.read_from(&session_id, 1).await?;
        let replayed = events
            .iter()
            .find_map(|entry| match &entry.event {
                AgentEvent::ToolConfigChanged { payload } => Some(payload.clone()),
                _ => None,
            })
            .expect("ToolConfigChanged event must round-trip through the event log");
        assert_eq!(
            replayed, payload,
            "current ToolConfigChanged shape must replay byte-for-byte"
        );
        Ok(())
    }

    #[tokio::test]
    async fn file_event_store_restart_continues_from_atomic_event_log_head()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let root = temp.path().join("events");
        let session_id = SessionId::new();
        let store = FileEventStore::new(&root);

        let seq = store
            .append(
                &session_id,
                &[
                    AgentEvent::TurnStarted { turn_number: 1 },
                    AgentEvent::TextComplete {
                        content: "before restart".to_string(),
                    },
                ],
            )
            .await?;
        assert_eq!(seq, 2);
        assert_eq!(
            store
                .read_exact_event_log_head(&session_id)
                .await?
                .expect("append persists an exact event-log head")
                .through_log_seq,
            2
        );
        assert_eq!(
            store.read_sequence_owner(&session_id).await?,
            None,
            "current writers do not maintain a parallel .seq authority"
        );

        let restarted = FileEventStore::new(&root);
        let seq = restarted
            .append(
                &session_id,
                &[AgentEvent::TextComplete {
                    content: "after restart".to_string(),
                }],
            )
            .await?;

        assert_eq!(seq, 3);
        assert_eq!(
            restarted
                .read_exact_event_log_head(&session_id)
                .await?
                .expect("restart advances the event-log head")
                .through_log_seq,
            3
        );
        let sequences: Vec<u64> = restarted
            .read_from(&session_id, 1)
            .await?
            .into_iter()
            .map(|event| event.seq)
            .collect();
        assert_eq!(sequences, vec![1, 2, 3]);
        Ok(())
    }

    #[tokio::test]
    async fn file_event_store_projected_checkpoint_cannot_mint_next_sequence()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let projection_root = temp.path().join(".rkat");
        let store = FileEventStore::new(projection_root.join("events"));
        let projector = crate::projector::SessionProjector::new(&projection_root);
        let session_id = SessionId::new();

        store
            .append(&session_id, &[AgentEvent::TurnStarted { turn_number: 1 }])
            .await?;
        projector.project(&store, &session_id, 1).await?;

        let session_projection_dir = projection_root
            .join("sessions")
            .join(session_id.to_string());
        tokio::fs::write(session_projection_dir.join("checkpoint"), b"500")
            .await
            .unwrap();

        let seq = store
            .append(
                &session_id,
                &[AgentEvent::TextComplete {
                    content: "projection checkpoint is not authority".to_string(),
                }],
            )
            .await?;

        assert_eq!(seq, 2);
        let sequences: Vec<u64> = store
            .read_from(&session_id, 1)
            .await?
            .into_iter()
            .map(|event| event.seq)
            .collect();
        assert_eq!(sequences, vec![1, 2]);

        let projected_seq = projector.resume(&store, &session_id).await?;
        assert_eq!(projected_seq, 2);
        assert_eq!(projector.read_checkpoint(&session_id).await, 2);
        Ok(())
    }

    #[tokio::test]
    async fn file_event_store_missing_head_adopts_legacy_sequence_owner_once()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let store = FileEventStore::new(temp.path().join("events"));
        let session_id = SessionId::new();

        let seq = store
            .append(&session_id, &[AgentEvent::TurnStarted { turn_number: 1 }])
            .await?;
        assert_eq!(seq, 1);

        tokio::fs::remove_file(store.event_log_head_path(&session_id)).await?;
        store.write_sequence_owner(&session_id, 41).await?;
        let seq = store
            .append(
                &session_id,
                &[AgentEvent::TextComplete {
                    content: "legacy owner is migrated".to_string(),
                }],
            )
            .await?;

        assert_eq!(seq, 42);
        let sequences: Vec<u64> = store
            .read_from(&session_id, 1)
            .await?
            .into_iter()
            .map(|event| event.seq)
            .collect();
        assert_eq!(sequences, vec![1, 42]);
        Ok(())
    }

    #[tokio::test]
    async fn file_event_store_exact_head_ignores_corrupt_legacy_sequence_file()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let store = FileEventStore::new(temp.path().join("events"));
        let session_id = SessionId::new();

        store
            .append(&session_id, &[AgentEvent::TurnStarted { turn_number: 1 }])
            .await?;
        tokio::fs::write(store.sequence_path(&session_id), b"not-a-sequence").await?;

        let seq = store
            .append(
                &session_id,
                &[AgentEvent::TextComplete {
                    content: "must not be minted".to_string(),
                }],
            )
            .await?;

        assert_eq!(seq, 2);
        assert_eq!(store.last_seq(&session_id).await?, 2);
        let events = store.read_from(&session_id, 1).await?;
        assert_eq!(events.len(), 2);
        Ok(())
    }

    #[tokio::test]
    async fn file_event_store_legacy_owner_trailing_exact_head_is_ignored()
    -> Result<(), Box<dyn std::error::Error>> {
        // A stale legacy `.seq` file has no authority once an exact atomic
        // event-log head exists.
        let temp = tempfile::tempdir()?;
        let store = FileEventStore::new(temp.path().join("events"));
        let session_id = SessionId::new();

        store
            .append(
                &session_id,
                &[
                    AgentEvent::TurnStarted { turn_number: 1 },
                    AgentEvent::TextComplete {
                        content: "tail is two".to_string(),
                    },
                ],
            )
            .await?;
        store.write_sequence_owner(&session_id, 1).await?;

        let seq = store
            .append(
                &session_id,
                &[AgentEvent::TextComplete {
                    content: "continues from tail, no reuse".to_string(),
                }],
            )
            .await?;

        assert_eq!(seq, 3, "tail (2) reconciles the trailing owner; no reuse");
        let sequences: Vec<u64> = store
            .read_from(&session_id, 1)
            .await?
            .into_iter()
            .map(|event| event.seq)
            .collect();
        assert_eq!(sequences, vec![1, 2, 3]);
        assert_eq!(
            store.read_sequence_owner(&session_id).await?,
            Some(1),
            "current writers leave the legacy file untouched"
        );
        assert_eq!(
            store
                .read_exact_event_log_head(&session_id)
                .await?
                .expect("current append advances the atomic head")
                .through_log_seq,
            3
        );
        Ok(())
    }

    #[tokio::test]
    async fn file_event_store_head_advances_only_after_jsonl_fsync_no_forward_gap()
    -> Result<(), Box<dyn std::error::Error>> {
        // The JSONL is fsynced before the atomic event-log head replacement.
        // A crash can therefore leave the head trailing canonical bytes, never
        // ahead of them.
        let temp = tempfile::tempdir()?;
        let store = FileEventStore::new(temp.path().join("events"));
        let session_id = SessionId::new();

        let seq = store
            .append(&session_id, &[AgentEvent::TurnStarted { turn_number: 1 }])
            .await?;
        assert_eq!(seq, 1);
        assert_eq!(
            store
                .read_exact_event_log_head(&session_id)
                .await?
                .expect("append persists an exact head")
                .through_log_seq,
            1
        );
        assert_eq!(store.read_sequence_owner(&session_id).await?, None);
        assert_eq!(store.last_seq(&session_id).await?, 1);

        // The next append therefore reuses first_seq == tail + 1 with no gap.
        let seq = store
            .append(
                &session_id,
                &[AgentEvent::TextComplete {
                    content: "no forward gap".to_string(),
                }],
            )
            .await?;
        assert_eq!(seq, 2);
        let sequences: Vec<u64> = store
            .read_from(&session_id, 1)
            .await?
            .into_iter()
            .map(|event| event.seq)
            .collect();
        assert_eq!(sequences, vec![1, 2], "contiguous sequence, no forward gap");
        Ok(())
    }

    #[tokio::test]
    async fn file_event_store_preserves_envelope_identity_round_trip()
    -> Result<(), Box<dyn std::error::Error>> {
        // Rows #164/#265: appending a canonical envelope with a non-session
        // source + mob_id must persist that identity and rehydrate the original
        // envelope on replay (not a fabricated session-scoped one).
        let temp = tempfile::tempdir()?;
        let store = FileEventStore::new(temp.path().join("events"));
        let session_id = SessionId::new();

        let envelope = EventEnvelope::new_with_source(
            EventSourceIdentity::runtime("rt-7"),
            42,
            Some("mob-abc".to_string()),
            AgentEvent::TextComplete {
                content: "from a mob runtime".to_string(),
            },
        );
        let last_seq = store
            .append_envelopes(&session_id, std::slice::from_ref(&envelope))
            .await?;
        assert_eq!(last_seq, 1);

        let stored = store.read_from(&session_id, 1).await?;
        assert_eq!(stored.len(), 1);
        let row = &stored[0];
        assert_eq!(row.seq, 1, "store-assigned durable sequence");
        assert_eq!(row.stream_seq, 42, "original stream seq preserved");
        assert_eq!(row.mob_id.as_deref(), Some("mob-abc"));
        assert_eq!(row.source, EventSourceIdentity::runtime("rt-7"));
        assert_eq!(row.schema_version, EVENT_SCHEMA_VERSION);

        // Rehydrate the canonical envelope; it must equal the original identity,
        // not a session-scoped fabrication.
        let rebuilt = row.to_envelope();
        assert_eq!(rebuilt.source, EventSourceIdentity::runtime("rt-7"));
        assert_eq!(rebuilt.mob_id.as_deref(), Some("mob-abc"));
        assert_eq!(rebuilt.seq, 42);
        assert_ne!(
            rebuilt.source,
            EventSourceIdentity::session(session_id.clone()),
            "must not fabricate a session-scoped source"
        );
        Ok(())
    }

    #[tokio::test]
    async fn file_event_store_read_from_fails_closed_on_schema_version_mismatch()
    -> Result<(), Box<dyn std::error::Error>> {
        // Row #265 gate (fails-old / passes-new): the OLD code wrote
        // `schema_version` but never read it, silently projecting any shape.
        // The fix makes `read_from` fail closed with a typed
        // `SchemaVersionMismatch` when a row's version differs from the runtime
        // constant.
        let temp = tempfile::tempdir()?;
        let store = FileEventStore::new(temp.path().join("events"));
        let session_id = SessionId::new();

        store
            .append(&session_id, &[AgentEvent::TurnStarted { turn_number: 1 }])
            .await?;

        // Hand-write a row carrying a future/unknown schema version.
        let future = StoredEvent {
            seq: 2,
            schema_version: EVENT_SCHEMA_VERSION + 1,
            timestamp: SystemTime::now(),
            source: EventSourceIdentity::session(session_id.clone()),
            mob_id: None,
            stream_seq: 0,
            event: AgentEvent::TextComplete {
                content: "future schema".to_string(),
            },
        };
        let mut line = serde_json::to_string(&future)?;
        line.push('\n');
        let log_path = store.root().join(format!("{session_id}.jsonl"));
        let mut file = tokio::fs::OpenOptions::new()
            .append(true)
            .open(&log_path)
            .await?;
        file.write_all(line.as_bytes()).await?;
        file.flush().await?;
        file.sync_all().await?;

        let err = store
            .read_from(&session_id, 1)
            .await
            .expect_err("schema-version drift must fail closed");
        assert!(
            matches!(
                err,
                EventStoreError::SchemaVersionMismatch {
                    expected,
                    found,
                } if expected == EVENT_SCHEMA_VERSION && found == EVENT_SCHEMA_VERSION + 1
            ),
            "unexpected error: {err}"
        );
        Ok(())
    }

    #[tokio::test]
    async fn file_event_store_rejects_pre_bump_v1_row_with_typed_schema_error()
    -> Result<(), Box<dyn std::error::Error>> {
        // Rows #164/#265: a pre-bump (v1) row lacks `source`/`stream_seq`. It
        // must still parse (via the documented parse-bridge defaults) so the
        // typed SchemaVersionMismatch gate rejects it — NOT surface an opaque
        // serialization error, and never be silently projected as the v2 shape.
        let temp = tempfile::tempdir()?;
        let store = FileEventStore::new(temp.path().join("events"));
        let session_id = SessionId::new();

        // Exact pre-bump v1 on-disk shape: seq/schema_version/timestamp/event,
        // with NO source/mob_id/stream_seq fields. The event payload is encoded
        // from a real AgentEvent so the shape can't silently drift.
        let timestamp = serde_json::to_value(SystemTime::now())?;
        let event = serde_json::to_value(AgentEvent::TurnStarted { turn_number: 1 })?;
        let v1_line = serde_json::to_string(&serde_json::json!({
            "seq": 1,
            "schema_version": 1,
            "timestamp": timestamp,
            "event": event,
        }))?;
        let log_path = store.root().join(format!("{session_id}.jsonl"));
        if let Some(parent) = log_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(&log_path, format!("{v1_line}\n")).await?;

        let err = store
            .read_from(&session_id, 1)
            .await
            .expect_err("pre-bump v1 row must fail closed");
        assert!(
            matches!(
                err,
                EventStoreError::SchemaVersionMismatch {
                    expected,
                    found,
                } if expected == EVENT_SCHEMA_VERSION && found == 1
            ),
            "pre-bump row must surface the typed schema mismatch, got: {err}"
        );
        Ok(())
    }
}
