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
use meerkat_core::time_compat::SystemTime;
use meerkat_core::types::SessionId;
use serde::{Deserialize, Serialize};
#[cfg(not(target_arch = "wasm32"))]
use std::path::{Path, PathBuf};
#[cfg(not(target_arch = "wasm32"))]
use std::sync::Arc;
#[cfg(not(target_arch = "wasm32"))]
use tokio::io::AsyncWriteExt;
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

/// Current schema version for stored events.
///
/// Bumped to `2` when [`StoredEvent`] gained canonical envelope identity
/// (`source`/`mob_id`/`stream_seq`). [`FileEventStore::read_from`] fails closed on
/// any row whose `schema_version` does not match this constant.
pub const EVENT_SCHEMA_VERSION: u32 = 2;

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
    /// last appended event.
    async fn append_envelopes(
        &self,
        session_id: &SessionId,
        envelopes: &[EventEnvelope<AgentEvent>],
    ) -> Result<u64, EventStoreError>;

    /// Append bare session-scoped events to the log for a session.
    ///
    /// Each event is reduced to a session-sourced envelope before being handed to
    /// [`EventStore::append_envelopes`]. Returns the sequence number of the last
    /// appended event.
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

    /// Read events from a given sequence number onward.
    async fn read_from(
        &self,
        session_id: &SessionId,
        from_seq: u64,
    ) -> Result<Vec<StoredEvent>, EventStoreError>;

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
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn log_path(&self, session_id: &SessionId) -> PathBuf {
        self.root.join(format!("{session_id}.jsonl"))
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

        let _guard = self.append_lock.lock().await;
        tokio::fs::create_dir_all(&self.root).await?;
        let _sequence_lock = self.acquire_sequence_lock(session_id).await?;
        let path = self.log_path(session_id);
        let (mut next_seq, last_allocated_seq) = self
            .allocate_sequence_range(session_id, envelopes.len())
            .await?;
        let mut lines = String::new();
        for envelope in envelopes {
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
            next_seq += 1;
        }
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .await?;
        file.write_all(lines.as_bytes()).await?;
        file.flush().await?;
        file.sync_all().await?;
        // Advance the durable sequence owner only AFTER the log bytes are
        // durably persisted (flushed + fsynced). A crash before this point
        // leaves the owner trailing the log tail (a benign stale hint that the
        // tail reconciles on the next allocation), never a forward gap or a
        // reused sequence.
        self.write_sequence_owner(session_id, last_allocated_seq)
            .await?;
        Ok(last_allocated_seq)
    }

    async fn read_from(
        &self,
        session_id: &SessionId,
        from_seq: u64,
    ) -> Result<Vec<StoredEvent>, EventStoreError> {
        let path = self.log_path(session_id);
        let contents = match tokio::fs::read_to_string(path).await {
            Ok(contents) => contents,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(err) => return Err(EventStoreError::Io(err)),
        };

        contents
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| {
                serde_json::from_str::<StoredEvent>(line)
                    .map_err(|err| EventStoreError::Serialization(err.to_string()))
            })
            .filter_map(|event| match event {
                // Fail closed on schema drift: an unknown (future or pre-bump)
                // schema version must not be silently projected as the current
                // shape. The version constant gates a real runtime decision
                // rather than being an inert written field.
                Ok(event) if event.schema_version != EVENT_SCHEMA_VERSION => {
                    Some(Err(EventStoreError::SchemaVersionMismatch {
                        expected: EVENT_SCHEMA_VERSION,
                        found: event.schema_version,
                    }))
                }
                Ok(event) if event.seq >= from_seq => Some(Ok(event)),
                Ok(_) => None,
                Err(err) => Some(Err(err)),
            })
            .collect()
    }

    async fn last_seq(&self, session_id: &SessionId) -> Result<u64, EventStoreError> {
        Ok(self
            .read_from(session_id, 1)
            .await?
            .last()
            .map_or(0, |event| event.seq))
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

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
        assert!(store.root().join(format!("{session_id}.jsonl")).exists());
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
        let checkpoint = tokio::fs::read_to_string(session_projection_dir.join("checkpoint"))
            .await
            .unwrap();
        assert_eq!(checkpoint.trim(), "2");
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
