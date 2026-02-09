//! RedbEventStore — redb-backed event store implementation.
//!
//! Gated behind the `session-store` feature.
//!
//! Tables:
//! - `events`: session-UUID-bytes + seq-u64 (24 bytes) → event JSON
//! - `event_seqs`: session-UUID-bytes (16 bytes) → last seq u64

use async_trait::async_trait;
use meerkat_core::event::AgentEvent;
use meerkat_core::types::SessionId;
use redb::{Database, ReadableTable, TableDefinition};
use std::path::Path;
use std::sync::Arc;
use std::time::SystemTime;

use crate::event_store::{EventStore, EventStoreError, StoredEvent, EVENT_SCHEMA_VERSION};

const EVENTS_TABLE: TableDefinition<&[u8], &[u8]> = TableDefinition::new("session_events");
const EVENT_SEQS_TABLE: TableDefinition<&[u8], u64> = TableDefinition::new("session_event_seqs");

fn event_key(session_id: &SessionId, seq: u64) -> [u8; 24] {
    let mut key = [0u8; 24];
    key[..16].copy_from_slice(session_id.0.as_bytes());
    key[16..].copy_from_slice(&seq.to_be_bytes());
    key
}

fn session_key(session_id: &SessionId) -> [u8; 16] {
    *session_id.0.as_bytes()
}

/// Event store backed by redb.
pub struct RedbEventStore {
    db: Arc<Database>,
}

impl RedbEventStore {
    /// Open or create an event store at the given path.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, EventStoreError> {
        let db = Database::create(path)
            .map_err(|e| EventStoreError::Store(format!("failed to open db: {e}")))?;

        // Ensure tables exist
        let write_txn = db
            .begin_write()
            .map_err(|e| EventStoreError::Store(format!("begin_write failed: {e}")))?;
        {
            let _ = write_txn
                .open_table(EVENTS_TABLE)
                .map_err(|e| EventStoreError::Store(format!("open events table: {e}")))?;
            let _ = write_txn
                .open_table(EVENT_SEQS_TABLE)
                .map_err(|e| EventStoreError::Store(format!("open seqs table: {e}")))?;
        }
        write_txn
            .commit()
            .map_err(|e| EventStoreError::Store(format!("commit failed: {e}")))?;

        Ok(Self { db: Arc::new(db) })
    }
}

#[async_trait]
impl EventStore for RedbEventStore {
    async fn append(
        &self,
        session_id: &SessionId,
        events: &[AgentEvent],
    ) -> Result<u64, EventStoreError> {
        if events.is_empty() {
            return self.last_seq(session_id).await;
        }

        let sk = session_key(session_id);

        let write_txn = self
            .db
            .begin_write()
            .map_err(|e| EventStoreError::Store(format!("begin_write: {e}")))?;

        let last_seq;
        {
            let mut events_table = write_txn
                .open_table(EVENTS_TABLE)
                .map_err(|e| EventStoreError::Store(format!("open events: {e}")))?;
            let mut seqs_table = write_txn
                .open_table(EVENT_SEQS_TABLE)
                .map_err(|e| EventStoreError::Store(format!("open seqs: {e}")))?;

            // Get current seq
            let mut current_seq = seqs_table
                .get(sk.as_slice())
                .map_err(|e| EventStoreError::Store(format!("get seq: {e}")))?
                .map(|v| v.value())
                .unwrap_or(0);

            let now = SystemTime::now();

            for event in events {
                current_seq += 1;
                let stored = StoredEvent {
                    seq: current_seq,
                    schema_version: EVENT_SCHEMA_VERSION,
                    timestamp: now,
                    event: event.clone(),
                };
                let json = serde_json::to_vec(&stored)
                    .map_err(|e| EventStoreError::Serialization(e.to_string()))?;
                let ek = event_key(session_id, current_seq);
                events_table
                    .insert(ek.as_slice(), json.as_slice())
                    .map_err(|e| EventStoreError::Store(format!("insert event: {e}")))?;
            }

            // Update last seq
            seqs_table
                .insert(sk.as_slice(), current_seq)
                .map_err(|e| EventStoreError::Store(format!("update seq: {e}")))?;

            last_seq = current_seq;
        }

        write_txn
            .commit()
            .map_err(|e| EventStoreError::Store(format!("commit: {e}")))?;

        Ok(last_seq)
    }

    async fn read_from(
        &self,
        session_id: &SessionId,
        from_seq: u64,
    ) -> Result<Vec<StoredEvent>, EventStoreError> {
        let read_txn = self
            .db
            .begin_read()
            .map_err(|e| EventStoreError::Store(format!("begin_read: {e}")))?;
        let events_table = read_txn
            .open_table(EVENTS_TABLE)
            .map_err(|e| EventStoreError::Store(format!("open events: {e}")))?;

        let start_key = event_key(session_id, from_seq);
        // End key: same session, max seq
        let end_key = event_key(session_id, u64::MAX);

        let range = events_table
            .range(start_key.as_slice()..=end_key.as_slice())
            .map_err(|e| EventStoreError::Store(format!("range query: {e}")))?;

        let mut results = Vec::new();
        for entry in range {
            let (_, value_guard) =
                entry.map_err(|e| EventStoreError::Store(format!("iter: {e}")))?;
            let stored: StoredEvent = serde_json::from_slice(value_guard.value())
                .map_err(|e| EventStoreError::Serialization(e.to_string()))?;
            // Skip events with unrecognized schema versions (forward-compatible)
            if stored.schema_version <= EVENT_SCHEMA_VERSION {
                results.push(stored);
            }
        }

        Ok(results)
    }

    async fn last_seq(&self, session_id: &SessionId) -> Result<u64, EventStoreError> {
        let sk = session_key(session_id);
        let read_txn = self
            .db
            .begin_read()
            .map_err(|e| EventStoreError::Store(format!("begin_read: {e}")))?;
        let seqs_table = read_txn
            .open_table(EVENT_SEQS_TABLE)
            .map_err(|e| EventStoreError::Store(format!("open seqs: {e}")))?;

        Ok(seqs_table
            .get(sk.as_slice())
            .map_err(|e| EventStoreError::Store(format!("get seq: {e}")))?
            .map(|v| v.value())
            .unwrap_or(0))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use meerkat_core::types::Usage;
    use tempfile::TempDir;

    fn temp_event_store() -> (TempDir, RedbEventStore) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("events.redb");
        let store = RedbEventStore::open(&path).unwrap();
        (dir, store)
    }

    #[tokio::test]
    async fn test_event_log_append_and_read() {
        let (_dir, store) = temp_event_store();
        let session_id = SessionId::new();

        let events = vec![
            AgentEvent::RunStarted {
                session_id: session_id.clone(),
                prompt: "Hello".to_string(),
            },
            AgentEvent::TurnStarted { turn_number: 0 },
            AgentEvent::RunCompleted {
                session_id: session_id.clone(),
                result: "Done".to_string(),
                usage: Usage::default(),
            },
        ];

        let last_seq = store.append(&session_id, &events).await.unwrap();
        assert_eq!(last_seq, 3);

        let stored = store.read_from(&session_id, 1).await.unwrap();
        assert_eq!(stored.len(), 3);
        assert_eq!(stored[0].seq, 1);
        assert_eq!(stored[1].seq, 2);
        assert_eq!(stored[2].seq, 3);
    }

    #[tokio::test]
    async fn test_event_log_monotonic_sequence() {
        let (_dir, store) = temp_event_store();
        let session_id = SessionId::new();

        // Append batch 1
        let events1 = vec![AgentEvent::TurnStarted { turn_number: 0 }];
        let seq1 = store.append(&session_id, &events1).await.unwrap();
        assert_eq!(seq1, 1);

        // Append batch 2
        let events2 = vec![
            AgentEvent::TurnStarted { turn_number: 1 },
            AgentEvent::TurnStarted { turn_number: 2 },
        ];
        let seq2 = store.append(&session_id, &events2).await.unwrap();
        assert_eq!(seq2, 3);

        // Read all
        let stored = store.read_from(&session_id, 1).await.unwrap();
        assert_eq!(stored.len(), 3);
        assert_eq!(stored[0].seq, 1);
        assert_eq!(stored[1].seq, 2);
        assert_eq!(stored[2].seq, 3);
    }

    #[tokio::test]
    async fn test_event_log_read_from_mid() {
        let (_dir, store) = temp_event_store();
        let session_id = SessionId::new();

        let events = vec![
            AgentEvent::TurnStarted { turn_number: 0 },
            AgentEvent::TurnStarted { turn_number: 1 },
            AgentEvent::TurnStarted { turn_number: 2 },
        ];
        store.append(&session_id, &events).await.unwrap();

        // Read from seq 2 onward
        let stored = store.read_from(&session_id, 2).await.unwrap();
        assert_eq!(stored.len(), 2);
        assert_eq!(stored[0].seq, 2);
        assert_eq!(stored[1].seq, 3);
    }

    #[tokio::test]
    async fn test_event_log_empty_session() {
        let (_dir, store) = temp_event_store();
        let session_id = SessionId::new();

        let last = store.last_seq(&session_id).await.unwrap();
        assert_eq!(last, 0);

        let stored = store.read_from(&session_id, 1).await.unwrap();
        assert!(stored.is_empty());
    }

    #[tokio::test]
    async fn test_event_log_empty_append() {
        let (_dir, store) = temp_event_store();
        let session_id = SessionId::new();

        let seq = store.append(&session_id, &[]).await.unwrap();
        assert_eq!(seq, 0);
    }
}
