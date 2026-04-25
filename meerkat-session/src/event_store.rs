//! EventStore trait — append-only event log with monotonic sequence numbers.
//!
//! Gated behind the `session-store` feature.

use async_trait::async_trait;
use meerkat_core::event::AgentEvent;
use meerkat_core::types::SessionId;
use serde::{Deserialize, Serialize};
#[cfg(not(target_arch = "wasm32"))]
use std::path::{Path, PathBuf};
#[cfg(not(target_arch = "wasm32"))]
use std::sync::Arc;
use std::time::SystemTime;
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
        let path = self.log_path(session_id);
        let mut next_seq = self.last_seq(session_id).await? + 1;
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
        Ok(next_seq - 1)
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
}
