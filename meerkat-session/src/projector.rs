//! SessionProjector — materializes CLI-friendly files from the event store.
//!
//! Gated behind the `session-store` feature.
//!
//! Produces:
//! ```text
//! .rkat/sessions/{session_id}/
//!   session.json    # snapshot
//!   events.jsonl    # human-readable event log
//!   summary.txt     # last assistant text
//! ```
//!
//! Idempotent: replaying from checkpoint produces identical output.
//! `.rkat/` files are derived output, never canonical source for resume.

use crate::event_store::EventStore;
use meerkat_core::event::AgentEvent;
use meerkat_core::types::SessionId;
use std::path::{Path, PathBuf};

/// Projector that materializes session files from the event store.
pub struct SessionProjector {
    output_dir: PathBuf,
}

impl SessionProjector {
    /// Create a new projector targeting the given output directory.
    pub fn new(output_dir: impl Into<PathBuf>) -> Self {
        Self {
            output_dir: output_dir.into(),
        }
    }

    /// Get the output directory.
    pub fn output_dir(&self) -> &Path {
        &self.output_dir
    }

    /// Session directory path: `{output_dir}/sessions/{session_id}/`
    fn session_dir(&self, session_id: &SessionId) -> PathBuf {
        self.output_dir
            .join("sessions")
            .join(session_id.to_string())
    }

    /// Project events for a session from the given checkpoint.
    ///
    /// Reads events from `from_seq` onward and writes/appends to output files.
    /// Returns the sequence number of the last processed event.
    pub async fn project<E: EventStore>(
        &self,
        event_store: &E,
        session_id: &SessionId,
        from_seq: u64,
    ) -> Result<u64, ProjectionError> {
        let events = event_store
            .read_from(session_id, from_seq)
            .await
            .map_err(|e| ProjectionError::EventStore(e.to_string()))?;

        if events.is_empty() {
            return Ok(from_seq.saturating_sub(1));
        }

        let dir = self.session_dir(session_id);
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(ProjectionError::Io)?;

        // Append events to events.jsonl
        let events_path = dir.join("events.jsonl");
        let mut lines = String::new();
        let mut last_seq = from_seq.saturating_sub(1);
        let mut last_assistant_text: Option<String> = None;

        for stored in &events {
            let line = serde_json::to_string(&stored)
                .map_err(|e| ProjectionError::Serialization(e.to_string()))?;
            lines.push_str(&line);
            lines.push('\n');
            last_seq = stored.seq;

            // Track last assistant text for summary
            if let AgentEvent::TextComplete { content } = &stored.event {
                if !content.is_empty() {
                    last_assistant_text = Some(content.clone());
                }
            }
        }

        // Append to events.jsonl (not overwrite)
        use tokio::io::AsyncWriteExt;
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&events_path)
            .await
            .map_err(ProjectionError::Io)?;
        file.write_all(lines.as_bytes())
            .await
            .map_err(ProjectionError::Io)?;

        // Write summary.txt if we have assistant text
        if let Some(text) = last_assistant_text {
            let summary_path = dir.join("summary.txt");
            tokio::fs::write(&summary_path, text.as_bytes())
                .await
                .map_err(ProjectionError::Io)?;
        }

        // Write checkpoint
        let checkpoint_path = dir.join("checkpoint");
        tokio::fs::write(&checkpoint_path, last_seq.to_string().as_bytes())
            .await
            .map_err(ProjectionError::Io)?;

        Ok(last_seq)
    }

    /// Read the last checkpoint seq for a session (0 if no checkpoint).
    pub async fn read_checkpoint(&self, session_id: &SessionId) -> u64 {
        let path = self.session_dir(session_id).join("checkpoint");
        match tokio::fs::read_to_string(&path).await {
            Ok(s) => match s.trim().parse() {
                Ok(seq) => seq,
                Err(err) => {
                    tracing::warn!(
                        checkpoint = ?path,
                        "invalid checkpoint contents (defaulting to 0): {err}"
                    );
                    0
                }
            },
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => 0,
            Err(err) => {
                tracing::warn!(
                    checkpoint = ?path,
                    "failed to read checkpoint (defaulting to 0): {err}"
                );
                0
            }
        }
    }

    /// Replay from checkpoint 0, producing identical output to a full projection.
    pub async fn replay<E: EventStore>(
        &self,
        event_store: &E,
        session_id: &SessionId,
    ) -> Result<u64, ProjectionError> {
        // Delete existing output
        let dir = self.session_dir(session_id);
        if dir.exists() {
            tokio::fs::remove_dir_all(&dir)
                .await
                .map_err(ProjectionError::Io)?;
        }

        // Replay from seq 1
        self.project(event_store, session_id, 1).await
    }

    /// Resume projection from the last checkpoint.
    pub async fn resume<E: EventStore>(
        &self,
        event_store: &E,
        session_id: &SessionId,
    ) -> Result<u64, ProjectionError> {
        let checkpoint = self.read_checkpoint(session_id).await;
        self.project(event_store, session_id, checkpoint + 1).await
    }
}

/// Errors from projection operations.
#[derive(Debug, thiserror::Error)]
pub enum ProjectionError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    Serialization(String),

    #[error("Event store error: {0}")]
    EventStore(String),
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::event_store::{EVENT_SCHEMA_VERSION, EventStoreError, StoredEvent};
    use meerkat_core::types::Usage;
    use std::collections::HashMap;
    use std::sync::Mutex;
    use std::time::SystemTime;
    use tempfile::TempDir;

    /// Simple in-memory event store for testing the projector.
    struct MemEventStore {
        events: Mutex<HashMap<String, Vec<StoredEvent>>>,
    }

    impl MemEventStore {
        fn new() -> Self {
            Self {
                events: Mutex::new(HashMap::new()),
            }
        }

        fn add_events(&self, session_id: &SessionId, agent_events: &[AgentEvent]) {
            let mut map = self.events.lock().unwrap();
            let entry = map.entry(session_id.to_string()).or_insert_with(Vec::new);
            let base_seq = entry.len() as u64;
            for (i, event) in agent_events.iter().enumerate() {
                entry.push(StoredEvent {
                    seq: base_seq + i as u64 + 1,
                    schema_version: EVENT_SCHEMA_VERSION,
                    timestamp: SystemTime::now(),
                    event: event.clone(),
                });
            }
        }
    }

    #[async_trait::async_trait]
    impl EventStore for MemEventStore {
        async fn append(
            &self,
            session_id: &SessionId,
            events: &[AgentEvent],
        ) -> Result<u64, EventStoreError> {
            self.add_events(session_id, events);
            self.last_seq(session_id).await
        }

        async fn read_from(
            &self,
            session_id: &SessionId,
            from_seq: u64,
        ) -> Result<Vec<StoredEvent>, EventStoreError> {
            let map = self.events.lock().unwrap();
            let key = session_id.to_string();
            Ok(map
                .get(&key)
                .map(|events| {
                    events
                        .iter()
                        .filter(|e| e.seq >= from_seq)
                        .cloned()
                        .collect()
                })
                .unwrap_or_default())
        }

        async fn last_seq(&self, session_id: &SessionId) -> Result<u64, EventStoreError> {
            let map = self.events.lock().unwrap();
            let key = session_id.to_string();
            Ok(map
                .get(&key)
                .and_then(|events| events.last().map(|e| e.seq))
                .unwrap_or(0))
        }
    }

    #[tokio::test]
    async fn test_projector_materializes_session_files() {
        let dir = TempDir::new().unwrap();
        let projector = SessionProjector::new(dir.path().join(".rkat"));
        let store = MemEventStore::new();
        let sid = SessionId::new();

        store.add_events(
            &sid,
            &[
                AgentEvent::RunStarted {
                    session_id: sid.clone(),
                    prompt: "Hello".to_string(),
                },
                AgentEvent::TextComplete {
                    content: "Hi there!".to_string(),
                },
                AgentEvent::RunCompleted {
                    session_id: sid.clone(),
                    result: "Hi there!".to_string(),
                    usage: Usage::default(),
                },
            ],
        );

        let last_seq = projector.project(&store, &sid, 1).await.unwrap();
        assert_eq!(last_seq, 3);

        // Check files exist
        let session_dir = projector.session_dir(&sid);
        assert!(session_dir.join("events.jsonl").exists());
        assert!(session_dir.join("summary.txt").exists());
        assert!(session_dir.join("checkpoint").exists());

        // Check summary content
        let summary = std::fs::read_to_string(session_dir.join("summary.txt")).unwrap();
        assert_eq!(summary, "Hi there!");

        // Check checkpoint
        let checkpoint = std::fs::read_to_string(session_dir.join("checkpoint")).unwrap();
        assert_eq!(checkpoint.trim(), "3");
    }

    #[tokio::test]
    async fn test_projector_idempotent_replay() {
        let dir = TempDir::new().unwrap();
        let projector = SessionProjector::new(dir.path().join(".rkat"));
        let store = MemEventStore::new();
        let sid = SessionId::new();

        store.add_events(
            &sid,
            &[
                AgentEvent::TurnStarted { turn_number: 0 },
                AgentEvent::TextComplete {
                    content: "Response 1".to_string(),
                },
            ],
        );

        // First projection
        let seq1 = projector.replay(&store, &sid).await.unwrap();

        // Get file content after first projection
        let events_content_1 =
            std::fs::read_to_string(projector.session_dir(&sid).join("events.jsonl")).unwrap();

        // Replay (should produce identical output)
        let seq2 = projector.replay(&store, &sid).await.unwrap();

        let events_content_2 =
            std::fs::read_to_string(projector.session_dir(&sid).join("events.jsonl")).unwrap();

        assert_eq!(seq1, seq2);
        assert_eq!(events_content_1, events_content_2);
    }

    #[tokio::test]
    async fn test_projector_resumes_from_checkpoint() {
        let dir = TempDir::new().unwrap();
        let projector = SessionProjector::new(dir.path().join(".rkat"));
        let store = MemEventStore::new();
        let sid = SessionId::new();

        // Add first batch
        store.add_events(&sid, &[AgentEvent::TurnStarted { turn_number: 0 }]);
        projector.project(&store, &sid, 1).await.unwrap();

        // Verify checkpoint
        let cp = projector.read_checkpoint(&sid).await;
        assert_eq!(cp, 1);

        // Add more events
        store.add_events(
            &sid,
            &[
                AgentEvent::TurnStarted { turn_number: 1 },
                AgentEvent::TextComplete {
                    content: "Done".to_string(),
                },
            ],
        );

        // Resume from checkpoint
        let seq = projector.resume(&store, &sid).await.unwrap();
        assert_eq!(seq, 3);

        // events.jsonl should have all 3 events
        let events_content =
            std::fs::read_to_string(projector.session_dir(&sid).join("events.jsonl")).unwrap();
        let lines: Vec<&str> = events_content.trim().split('\n').collect();
        assert_eq!(lines.len(), 3);
    }
}
