//! EventStore trait — append-only event log with monotonic sequence numbers.
//!
//! Gated behind the `session-store` feature.
//!
//! File-backed sequence contract:
//! - Owner: `FileEventStore` allocates event order from a durable per-session
//!   sequence owner under the event-store root.
//! - Bootstrap: when the owner is absent, the canonical event log tail seeds it;
//!   projected `.rkat/sessions/...` files are never consulted.
//! - Staleness: an existing owner behind the event log tail is corruption and
//!   append fails closed.
//! - Failure: allocation errors abort append; the store never falls back to a
//!   process-local counter or projection checkpoint.

use async_trait::async_trait;
use meerkat_core::event::AgentEvent;
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

/// A stored event with sequence metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredEvent {
    /// Monotonically increasing sequence number within a session.
    pub seq: u64,
    /// Schema version for forward compatibility.
    pub schema_version: u32,
    /// When the event was stored.
    pub timestamp: SystemTime,
    /// The event payload.
    pub event: AgentEvent,
}

/// Current schema version for stored events.
pub const EVENT_SCHEMA_VERSION: u32 = 1;

/// Append-only event log.
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait EventStore: Send + Sync {
    /// Append events to the log for a session.
    ///
    /// Returns the sequence number of the last appended event.
    async fn append(
        &self,
        session_id: &SessionId,
        events: &[AgentEvent],
    ) -> Result<u64, EventStoreError>;

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
        let base_seq = match sequence_owner {
            Some(owner_seq) if owner_seq < event_log_tail => {
                return Err(EventStoreError::Store(format!(
                    "durable sequence owner for session {session_id} is stale ({owner_seq}) behind event log tail ({event_log_tail}); refusing to allocate"
                )));
            }
            Some(owner_seq) => owner_seq,
            None => event_log_tail,
        };
        let first_seq = base_seq.checked_add(1).ok_or_else(|| {
            EventStoreError::Store("event sequence overflow while allocating first sequence".into())
        })?;
        let last_seq = base_seq.checked_add(event_count).ok_or_else(|| {
            EventStoreError::Store("event sequence overflow while allocating range".into())
        })?;

        self.write_sequence_owner(session_id, last_seq).await?;
        Ok((first_seq, last_seq))
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg(not(target_arch = "wasm32"))]
impl EventStore for FileEventStore {
    async fn append(
        &self,
        session_id: &SessionId,
        events: &[AgentEvent],
    ) -> Result<u64, EventStoreError> {
        if events.is_empty() {
            return self.last_seq(session_id).await;
        }

        let _guard = self.append_lock.lock().await;
        tokio::fs::create_dir_all(&self.root).await?;
        let _sequence_lock = self.acquire_sequence_lock(session_id).await?;
        let path = self.log_path(session_id);
        let (mut next_seq, last_allocated_seq) = self
            .allocate_sequence_range(session_id, events.len())
            .await?;
        let mut lines = String::new();
        for event in events {
            let stored = StoredEvent {
                seq: next_seq,
                schema_version: EVENT_SCHEMA_VERSION,
                timestamp: SystemTime::now(),
                event: event.clone(),
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
    async fn file_event_store_stale_sequence_owner_fails_closed()
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
                        content: "tail is two".to_string(),
                    },
                ],
            )
            .await?;
        store.write_sequence_owner(&session_id, 1).await?;

        let err = store
            .append(
                &session_id,
                &[AgentEvent::TextComplete {
                    content: "must not reuse sequence".to_string(),
                }],
            )
            .await
            .expect_err("stale durable sequence owner must fail closed");

        assert!(err.to_string().contains("stale"));
        assert_eq!(store.last_seq(&session_id).await?, 2);
        let events = store.read_from(&session_id, 1).await?;
        assert_eq!(events.len(), 2);
        Ok(())
    }
}
