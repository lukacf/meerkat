//! EventStore trait — append-only event log with monotonic sequence numbers.
//!
//! Gated behind the `session-store` feature.
//!
//! File-backed sequence contract:
//! - Owner: `FileEventStore` allocates event order from the canonical event log
//!   tail, reconciled against a durable per-session sequence owner hint.
//! - Bootstrap: when the owner is absent, the canonical event log tail seeds it;
//!   projected `.rkat/sessions/...` files are never consulted.
//! - Durable ordering: the sequence owner is advanced only AFTER the log bytes
//!   are flushed and `fsync`ed (see [`FileEventStore::append`]). A crash between
//!   allocation and the owner write therefore leaves the owner trailing the log
//!   tail — a benign stale hint that the tail authoritatively overrides — never a
//!   forward gap or a reused/overwritten sequence.
//! - Failure: allocation errors abort append; the store never falls back to a
//!   process-local counter or projection checkpoint.

use async_trait::async_trait;
use meerkat_core::event::{AgentEvent, EventEnvelope, EventSourceIdentity};
use meerkat_core::interaction::InteractionId;
use meerkat_core::time_compat::SystemTime;
use meerkat_core::types::SessionId;
use serde::{Deserialize, Serialize};
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
            },
            AgentEvent::InteractionCallbackPending {
                interaction_id: right_id,
                tool_name: right_tool,
                args: right_args,
            },
        ) => left_id == right_id && left_tool == right_tool && left_args == right_args,
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

/// Prefix/suffix bytes sampled from the final indexed row when validating a
/// fingerprint hit. This catches replaced tails without making a very large
/// event row an unbounded per-page read.
#[cfg(not(target_arch = "wasm32"))]
const EVENT_LOG_ANCHOR_SAMPLE_BYTES: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

#[derive(Debug, Clone, Copy)]
#[cfg(not(target_arch = "wasm32"))]
struct EventLogLineAnchor {
    byte_offset: u64,
    byte_len: u64,
    prefix_hash: u64,
    suffix_hash: u64,
}

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
    /// payload is nonterminal/corrupt. Keeping the first canonical row plus a
    /// count lets exact appends distinguish zero/one/many in O(1) after the
    /// validated log index is warm.
    exact_interaction_occupants: HashMap<InteractionId, ExactInteractionOccupant>,
}

#[derive(Debug, Clone)]
#[cfg(not(target_arch = "wasm32"))]
struct ExactInteractionOccupant {
    first: StoredEvent,
    count: usize,
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
impl EventLogIndex {
    fn note_exact_interaction_occupant(&mut self, event: &StoredEvent) {
        let EventSourceIdentity::Interaction { interaction_id } = &event.source else {
            return;
        };
        self.exact_interaction_occupants
            .entry(*interaction_id)
            .and_modify(|occupant| occupant.count = occupant.count.saturating_add(1))
            .or_insert_with(|| ExactInteractionOccupant {
                first: event.clone(),
                count: 1,
            });
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

    async fn rebuild_event_log_index(
        &self,
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
            let event = self.decode_event_line(&line)?;
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
            index.note_exact_interaction_occupant(&event);
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

            let candidate = match self.rebuild_event_log_index(&mut file, &path, before).await {
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
                .read_index_snapshot(&path, &mut file, snapshot, from_seq, max_rows)
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
    ) -> Result<Option<ExactInteractionOccupant>, EventStoreError> {
        let _ = self.refresh_event_log_index(session_id, None).await?;
        let shared = self.event_log_index(session_id).await;
        let index = shared.lock().await;
        Ok(index
            .exact_interaction_occupants
            .get(&interaction_id)
            .cloned())
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
        let index = shared.lock().await;
        Ok(interaction_ids
            .iter()
            .map(
                |interaction_id| match index.exact_interaction_occupants.get(interaction_id) {
                    None => ExactInteractionOccupancy::Empty,
                    Some(ExactInteractionOccupant { first, count: 1 }) => {
                        ExactInteractionOccupancy::One(first.clone())
                    }
                    Some(ExactInteractionOccupant { first, count }) => {
                        ExactInteractionOccupancy::Multiple {
                            first: first.clone(),
                            count: *count,
                        }
                    }
                },
            )
            .collect())
    }

    async fn read_index_snapshot(
        &self,
        path: &Path,
        file: &mut tokio::fs::File,
        snapshot: EventLogIndexSnapshot,
        from_seq: u64,
        max_rows: Option<usize>,
    ) -> Result<(Vec<StoredEvent>, EventLogFingerprint), EventStoreError> {
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
            let event = self.decode_event_line(&line)?;
            if event.seq >= from_seq {
                rows.push(event);
                if max_rows.is_some_and(|limit| rows.len() == limit) {
                    break;
                }
            }
        }
        drop(lines);
        let after = Self::event_log_fingerprint_from_metadata(&file.metadata().await?);
        Ok((rows, after))
    }

    async fn note_appended_rows(
        &self,
        session_id: &SessionId,
        pre_fingerprint: EventLogFingerprint,
        post_fingerprint: EventLogFingerprint,
        appended_bytes: &[u8],
        rows: &[AppendedIndexRow],
        stored_events: &[StoredEvent],
    ) {
        let shared = self.event_log_index(session_id).await;
        let mut index = shared.lock().await;
        let Ok(appended_len) = u64::try_from(appended_bytes.len()) else {
            *index = EventLogIndex::default();
            return;
        };
        let Some(expected_post_len) = pre_fingerprint.len.checked_add(appended_len) else {
            *index = EventLogIndex::default();
            return;
        };
        // A reader may have rebuilt this shared cache to the post-append
        // fingerprint after fsync but before this cooperative update acquired
        // the mutex. Never erase that newer validated snapshot. Mechanical
        // extension is legal only from the exact pre-append state.
        if index.fingerprint != Some(pre_fingerprint) {
            return;
        }
        if post_fingerprint.len != expected_post_len {
            *index = EventLogIndex::default();
            return;
        }
        let append_start = pre_fingerprint.len;
        if rows.len() != stored_events.len() {
            *index = EventLogIndex::default();
            return;
        }
        for (row, event) in rows.iter().zip(stored_events) {
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
            let Some(line) = appended_bytes.get(relative_start..relative_end) else {
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
            index.note_exact_interaction_occupant(event);
        }
        index.fingerprint = Some(post_fingerprint);
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

    /// Allocate a contiguous sequence range WITHOUT advancing the durable owner.
    ///
    /// The owner is advanced only after the log bytes are flushed and `fsync`ed
    /// (see [`FileEventStore::append`]). A durable owner that trails the canonical
    /// event log tail is therefore the expected post-crash state — a benign stale
    /// hint — so the tail authoritatively reconciles it (`base = max(owner, tail)`)
    /// rather than being treated as corruption. A durable owner AHEAD of the tail
    /// is an intentional reservation and remains authoritative; the projection
    /// checkpoint is never consulted.
    async fn allocate_sequence_range(
        &self,
        session_id: &SessionId,
        event_count: usize,
    ) -> Result<(u64, u64), EventStoreError> {
        let event_count = u64::try_from(event_count).map_err(|_| {
            EventStoreError::Store("event batch is too large to allocate a sequence range".into())
        })?;
        if event_count == 0 {
            return Err(EventStoreError::Store(
                "cannot allocate an empty event sequence range".into(),
            ));
        }

        let event_log_tail = self.last_seq(session_id).await?;
        let sequence_owner = self.read_sequence_owner(session_id).await?;
        let base_seq = sequence_owner.unwrap_or(event_log_tail).max(event_log_tail);
        let first_seq = base_seq.checked_add(1).ok_or_else(|| {
            EventStoreError::Store("event sequence overflow while allocating first sequence".into())
        })?;
        let last_seq = base_seq.checked_add(event_count).ok_or_else(|| {
            EventStoreError::Store("event sequence overflow while allocating range".into())
        })?;

        Ok((first_seq, last_seq))
    }

    /// Append envelopes while the caller holds both `append_lock` and the
    /// durable per-session sequence lock.
    async fn append_envelopes_locked(
        &self,
        session_id: &SessionId,
        envelopes: &[EventEnvelope<AgentEvent>],
    ) -> Result<Vec<StoredEvent>, EventStoreError> {
        debug_assert!(!envelopes.is_empty());

        let path = self.log_path(session_id);
        let (mut next_seq, last_allocated_seq) = self
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
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .await?;
        let pre_fingerprint = Self::event_log_fingerprint_from_metadata(&file.metadata().await?);
        file.write_all(lines.as_bytes()).await?;
        file.flush().await?;
        file.sync_all().await?;
        let post_fingerprint = Self::event_log_fingerprint_from_metadata(&file.metadata().await?);
        self.note_appended_rows(
            session_id,
            pre_fingerprint,
            post_fingerprint,
            lines.as_bytes(),
            &appended_index_rows,
            &stored_events,
        )
        .await;
        // Advance the durable sequence owner only AFTER the log bytes are
        // durably persisted (flushed + fsynced). A crash before this point
        // leaves the owner trailing the log tail (a benign stale hint that the
        // tail reconciles on the next allocation), never a forward gap or a
        // reused sequence.
        self.write_sequence_owner(session_id, last_allocated_seq)
            .await?;
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
            Some(ExactInteractionOccupant { first, count: 1 })
                if first.mob_id == envelope.mob_id
                    && interaction_terminal_events_semantically_equal(
                        &first.event,
                        &envelope.payload,
                    ) =>
            {
                Ok(ExactInteractionAppend::Replayed(first))
            }
            Some(ExactInteractionOccupant { first, count: 1 }) => {
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
            Some(ExactInteractionOccupant { count, .. }) => {
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
        let (snapshot, _file) = self.refresh_event_log_index(session_id, None).await?;
        Ok(snapshot.last_seq)
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
            .note_appended_rows(
                &session_id,
                before,
                after,
                &appended_bytes,
                &[AppendedIndexRow {
                    seq: 129,
                    relative_offset: 0,
                    byte_len: u64::try_from(appended_bytes.len())?,
                }],
                std::slice::from_ref(&appended),
            )
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
            .read_index_snapshot(&path, &mut opened, snapshot, 120, Some(1))
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
    async fn file_event_store_restart_continues_from_durable_sequence_owner()
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
        assert_eq!(store.read_sequence_owner(&session_id).await?, Some(2));

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
        assert_eq!(restarted.read_sequence_owner(&session_id).await?, Some(3));
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
    async fn file_event_store_process_local_counter_is_inert_when_durable_owner_advances()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let store = FileEventStore::new(temp.path().join("events"));
        let session_id = SessionId::new();

        let seq = store
            .append(&session_id, &[AgentEvent::TurnStarted { turn_number: 1 }])
            .await?;
        assert_eq!(seq, 1);

        store.write_sequence_owner(&session_id, 41).await?;
        let seq = store
            .append(
                &session_id,
                &[AgentEvent::TextComplete {
                    content: "durable owner wins".to_string(),
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
    async fn file_event_store_corrupt_sequence_owner_fails_closed()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let store = FileEventStore::new(temp.path().join("events"));
        let session_id = SessionId::new();

        store
            .append(&session_id, &[AgentEvent::TurnStarted { turn_number: 1 }])
            .await?;
        tokio::fs::write(store.sequence_path(&session_id), b"not-a-sequence").await?;

        let err = store
            .append(
                &session_id,
                &[AgentEvent::TextComplete {
                    content: "must not be minted".to_string(),
                }],
            )
            .await
            .expect_err("corrupt durable sequence owner must fail closed");

        assert!(err.to_string().contains("durable sequence owner"));
        assert_eq!(store.last_seq(&session_id).await?, 1);
        let events = store.read_from(&session_id, 1).await?;
        assert_eq!(events.len(), 1);
        Ok(())
    }

    #[tokio::test]
    async fn file_event_store_owner_trailing_log_tail_reconciles_to_tail()
    -> Result<(), Box<dyn std::error::Error>> {
        // Row #71: with the durable sequence owner advanced only AFTER the log
        // bytes are fsynced, an owner that trails the log tail is the normal
        // post-crash state (bytes synced, owner write lost) — NOT corruption.
        // The canonical event log tail authoritatively reconciles it, so the
        // next append continues contiguously from the tail (no reuse, no gap).
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
        // Simulate a crash that lost the owner write after the log fsync: the
        // owner trails the durable tail of 2.
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
        // The owner is re-advanced past the tail after the successful fsync.
        assert_eq!(store.read_sequence_owner(&session_id).await?, Some(3));
        Ok(())
    }

    #[tokio::test]
    async fn file_event_store_owner_advances_only_after_fsync_no_forward_gap()
    -> Result<(), Box<dyn std::error::Error>> {
        // Row #71 hardening: the OLD code advanced the durable owner BEFORE
        // writing the log bytes, so a write/flush failure after sequence
        // allocation left the owner ahead of the tail, minting a forward gap on
        // the next append. The fix advances the owner only after
        // `file.sync_all()`. This pins the post-commit invariant — owner equals
        // the durable tail (never ahead) — so an interrupted append can only
        // ever leave the owner trailing (reconciled by the tail), never ahead.
        // (The fails-old/passes-new behavioral gate is the trailing-owner
        // reconcile test above.)
        let temp = tempfile::tempdir()?;
        let store = FileEventStore::new(temp.path().join("events"));
        let session_id = SessionId::new();

        let seq = store
            .append(&session_id, &[AgentEvent::TurnStarted { turn_number: 1 }])
            .await?;
        assert_eq!(seq, 1);
        // Post-commit invariant: owner == durable tail (NOT ahead). Under the
        // old "advance-before-write" ordering an interrupted append would leave
        // the owner ahead of the tail here.
        assert_eq!(store.read_sequence_owner(&session_id).await?, Some(1));
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
