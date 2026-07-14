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
//!
//! Projection contract:
//! - Owner: `EventStore` owns event order and durable sequence state.
//! - Rebuild trigger: missing checkpoints resume from the event log start;
//!   invalid, unreadable, or ahead-of-log checkpoints force full replay.
//! - Staleness policy: checkpoints are cursors over derived output only and
//!   are stale when they exceed `EventStore::last_seq`.
//! - Failure behavior: event-store read failures propagate; projection never
//!   invents or allocates event sequences.

use crate::event_store::EventStore;
use meerkat_core::event::AgentEvent;
use meerkat_core::types::SessionId;
use std::cmp::Ordering;
use std::path::{Path, PathBuf};

/// Projector that materializes session files from the event store.
#[derive(Debug, Clone)]
pub struct SessionProjector {
    output_dir: PathBuf,
}

/// Derived-commit cursor persisted in `checkpoint`.
///
/// Owns BOTH facts of a committed projection: the last projected event
/// sequence AND the committed `events.jsonl` byte length. A crash between the
/// `events.jsonl` append and the checkpoint write leaves a partial tail past
/// `events_len`; resume truncates back to the committed length before
/// projecting incrementally, so derived rows can never duplicate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct ProjectionCheckpoint {
    last_seq: u64,
    events_len: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CheckpointState {
    Valid(ProjectionCheckpoint),
    Missing,
    Invalid,
}

/// Whether the committed `events.jsonl` prefix recorded by the checkpoint is
/// intact (after healing any partial tail), or the derived output diverged in
/// a way only a full replay can rebuild.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DerivedTail {
    Committed,
    RequiresRebuild,
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

    async fn project_with_mode<E: EventStore + ?Sized>(
        &self,
        event_store: &E,
        session_id: &SessionId,
        from_seq: u64,
        replace_existing: bool,
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
        let mut summary_updated = false;
        let mut last_assistant_text: Option<String> = None;

        for stored in &events {
            let line = serde_json::to_string(&stored)
                .map_err(|e| ProjectionError::Serialization(e.to_string()))?;
            lines.push_str(&line);
            lines.push('\n');
            last_seq = stored.seq;

            // Track last assistant text for summary
            if let AgentEvent::TextComplete { content } = &stored.event
                && !content.is_empty()
            {
                summary_updated = true;
                last_assistant_text = Some(content.clone());
            } else if let AgentEvent::TranscriptRewriteCommitted { record, .. } = &stored.event {
                summary_updated = true;
                last_assistant_text =
                    last_assistant_text_from_messages(&record.revision_body.messages);
            }
        }

        // Append to events.jsonl (not overwrite)
        use tokio::io::AsyncWriteExt;
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .append(!replace_existing)
            .truncate(replace_existing)
            .open(&events_path)
            .await
            .map_err(ProjectionError::Io)?;
        file.write_all(lines.as_bytes())
            .await
            .map_err(ProjectionError::Io)?;
        // Tokio file writes complete on background blocking tasks; flush before
        // dropping so immediate replay readers observe the finished contents.
        file.flush().await.map_err(ProjectionError::Io)?;
        let events_len = file.metadata().await.map_err(ProjectionError::Io)?.len();
        drop(file);

        // Rewrite commits can replace or remove prior assistant text, so keep
        // the derived summary aligned with the retained revision body.
        if summary_updated {
            let summary_path = dir.join("summary.txt");
            if let Some(text) = last_assistant_text {
                tokio::fs::write(&summary_path, text.as_bytes())
                    .await
                    .map_err(ProjectionError::Io)?;
            } else if let Err(err) = tokio::fs::remove_file(&summary_path).await
                && err.kind() != std::io::ErrorKind::NotFound
            {
                return Err(ProjectionError::Io(err));
            }
        }

        // Write the checkpoint cursor: it commits both the last projected
        // sequence and the events.jsonl byte length so resume can truncate any
        // partial tail left by a crash before this write.
        let checkpoint_path = dir.join("checkpoint");
        let checkpoint = ProjectionCheckpoint {
            last_seq,
            events_len,
        };
        let payload = serde_json::to_string(&checkpoint)
            .map_err(|e| ProjectionError::Serialization(e.to_string()))?;
        tokio::fs::write(&checkpoint_path, payload.as_bytes())
            .await
            .map_err(ProjectionError::Io)?;

        Ok(last_seq)
    }

    /// Project events for a session from the given checkpoint.
    ///
    /// Reads events from `from_seq` onward and writes/appends to output files.
    /// Returns the sequence number of the last processed event.
    pub async fn project<E: EventStore + ?Sized>(
        &self,
        event_store: &E,
        session_id: &SessionId,
        from_seq: u64,
    ) -> Result<u64, ProjectionError> {
        self.project_with_mode(event_store, session_id, from_seq, false)
            .await
    }

    /// Read the last checkpoint seq for a session (0 if no checkpoint).
    pub async fn read_checkpoint(&self, session_id: &SessionId) -> u64 {
        match self.read_checkpoint_state(session_id).await {
            CheckpointState::Valid(checkpoint) => checkpoint.last_seq,
            CheckpointState::Missing | CheckpointState::Invalid => 0,
        }
    }

    async fn read_checkpoint_state(&self, session_id: &SessionId) -> CheckpointState {
        let path = self.session_dir(session_id).join("checkpoint");
        match tokio::fs::read_to_string(&path).await {
            Ok(s) => match serde_json::from_str::<ProjectionCheckpoint>(s.trim()) {
                Ok(checkpoint) => CheckpointState::Valid(checkpoint),
                Err(err) => {
                    tracing::warn!(
                        checkpoint = ?path,
                        "invalid checkpoint contents: {err}"
                    );
                    CheckpointState::Invalid
                }
            },
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => CheckpointState::Missing,
            Err(err) => {
                tracing::warn!(
                    checkpoint = ?path,
                    "failed to read checkpoint: {err}"
                );
                CheckpointState::Invalid
            }
        }
    }

    /// Heal the crash window between the `events.jsonl` append and the
    /// checkpoint write: truncate `events.jsonl` back to the byte length the
    /// checkpoint committed, dropping any partial tail a crashed projection
    /// appended. Returns [`DerivedTail::RequiresRebuild`] when the derived file
    /// is shorter than the committed cursor (only a full replay can rebuild).
    async fn heal_events_tail(
        &self,
        session_id: &SessionId,
        committed_len: u64,
    ) -> Result<DerivedTail, ProjectionError> {
        let events_path = self.session_dir(session_id).join("events.jsonl");
        let current_len = match tokio::fs::metadata(&events_path).await {
            Ok(metadata) => metadata.len(),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Ok(if committed_len == 0 {
                    DerivedTail::Committed
                } else {
                    DerivedTail::RequiresRebuild
                });
            }
            Err(err) => return Err(ProjectionError::Io(err)),
        };
        match current_len.cmp(&committed_len) {
            Ordering::Equal => Ok(DerivedTail::Committed),
            Ordering::Less => Ok(DerivedTail::RequiresRebuild),
            Ordering::Greater => {
                tracing::warn!(
                    session_id = %session_id,
                    committed_len,
                    current_len,
                    "events.jsonl has a partial tail past the committed checkpoint; truncating"
                );
                let file = tokio::fs::OpenOptions::new()
                    .write(true)
                    .open(&events_path)
                    .await
                    .map_err(ProjectionError::Io)?;
                file.set_len(committed_len)
                    .await
                    .map_err(ProjectionError::Io)?;
                file.sync_all().await.map_err(ProjectionError::Io)?;
                Ok(DerivedTail::Committed)
            }
        }
    }

    /// Replay from checkpoint 0, producing identical output to a full projection.
    pub async fn replay<E: EventStore + ?Sized>(
        &self,
        event_store: &E,
        session_id: &SessionId,
    ) -> Result<u64, ProjectionError> {
        let dir = self.session_dir(session_id);
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(ProjectionError::Io)?;

        // Remove derived files that may no longer be rewritten by the replay.
        for file_name in ["events.jsonl", "summary.txt", "checkpoint"] {
            let path = dir.join(file_name);
            remove_derived_path(&path).await?;
        }

        // Replay from seq 1, replacing any previous derived output.
        self.project_with_mode(event_store, session_id, 1, true)
            .await
    }

    /// Resume projection from the last checkpoint.
    pub async fn resume<E: EventStore + ?Sized>(
        &self,
        event_store: &E,
        session_id: &SessionId,
    ) -> Result<u64, ProjectionError> {
        match self.read_checkpoint_state(session_id).await {
            CheckpointState::Valid(checkpoint) => {
                let durable_last_seq = event_store
                    .last_seq(session_id)
                    .await
                    .map_err(|e| ProjectionError::EventStore(e.to_string()))?;
                match checkpoint.last_seq.cmp(&durable_last_seq) {
                    Ordering::Greater => {
                        tracing::warn!(
                            session_id = %session_id,
                            checkpoint_last_seq = checkpoint.last_seq,
                            durable_last_seq,
                            "projection checkpoint is ahead of durable event store; replaying derived files"
                        );
                        self.replay(event_store, session_id).await
                    }
                    Ordering::Equal => {
                        match self
                            .heal_events_tail(session_id, checkpoint.events_len)
                            .await?
                        {
                            DerivedTail::Committed => Ok(checkpoint.last_seq),
                            DerivedTail::RequiresRebuild => {
                                self.replay(event_store, session_id).await
                            }
                        }
                    }
                    Ordering::Less => {
                        // A crash inside a previous incremental projection can
                        // leave appended rows past the committed cursor with
                        // the checkpoint still valid-but-stale. Truncate back
                        // to the committed byte length before appending, so
                        // the same derived events are never duplicated.
                        match self
                            .heal_events_tail(session_id, checkpoint.events_len)
                            .await?
                        {
                            DerivedTail::Committed => {
                                self.project(event_store, session_id, checkpoint.last_seq + 1)
                                    .await
                            }
                            DerivedTail::RequiresRebuild => {
                                self.replay(event_store, session_id).await
                            }
                        }
                    }
                }
            }
            // A missing checkpoint with a possibly-present events.jsonl is the
            // crash window between the events.jsonl write and the checkpoint
            // write (project_with_mode writes the checkpoint last). Projecting
            // from seq 1 here APPENDS to that partial events.jsonl, duplicating
            // derived rows. Rebuild via replay() (truncate-and-rebuild, removing
            // derived files first) exactly like the Invalid arm.
            CheckpointState::Missing | CheckpointState::Invalid => {
                self.replay(event_store, session_id).await
            }
        }
    }
}

async fn remove_derived_path(path: &Path) -> Result<(), ProjectionError> {
    match tokio::fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::IsADirectory => {
            tokio::fs::remove_dir_all(path)
                .await
                .map_err(ProjectionError::Io)
        }
        Err(err) => {
            // macOS reports remove_file(directory) as PermissionDenied.
            if err.kind() == std::io::ErrorKind::PermissionDenied {
                match tokio::fs::metadata(path).await {
                    Ok(metadata) if metadata.is_dir() => {
                        return tokio::fs::remove_dir_all(path)
                            .await
                            .map_err(ProjectionError::Io);
                    }
                    Ok(_) | Err(_) => {}
                }
            }
            Err(ProjectionError::Io(err))
        }
    }
}

fn last_assistant_text_from_messages(messages: &[meerkat_core::Message]) -> Option<String> {
    messages.iter().rev().find_map(|message| match message {
        meerkat_core::Message::BlockAssistant(message) => {
            let text = message.to_string();
            (!text.is_empty()).then_some(text)
        }
        _ => None,
    })
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
    use crate::event_store::{
        EVENT_SCHEMA_VERSION, EventProjectionHaltMarker, EventStoreError, StoredEvent,
    };
    use meerkat_core::event::EventSourceIdentity;
    use meerkat_core::types::{
        AssistantBlock, BlockAssistantMessage, Message, StopReason, Usage, UserMessage,
    };
    use std::collections::HashMap;
    use std::sync::Mutex;
    use std::time::SystemTime;
    use tempfile::TempDir;

    fn read_checkpoint_cursor(path: &std::path::Path) -> ProjectionCheckpoint {
        let content = std::fs::read_to_string(path).unwrap();
        serde_json::from_str(content.trim()).unwrap()
    }

    /// Simple in-memory event store for testing the projector.
    struct MemEventStore {
        events: Mutex<HashMap<String, Vec<StoredEvent>>>,
        projection_halts: Mutex<HashMap<String, EventProjectionHaltMarker>>,
    }

    impl MemEventStore {
        fn new() -> Self {
            Self {
                events: Mutex::new(HashMap::new()),
                projection_halts: Mutex::new(HashMap::new()),
            }
        }

        fn add_events(&self, session_id: &SessionId, agent_events: &[AgentEvent]) {
            let mut map = self.events.lock().unwrap();
            let entry = map.entry(session_id.to_string()).or_default();
            let base_seq = entry.len() as u64;
            for (i, event) in agent_events.iter().enumerate() {
                entry.push(StoredEvent {
                    seq: base_seq + i as u64 + 1,
                    schema_version: EVENT_SCHEMA_VERSION,
                    timestamp: SystemTime::now(),
                    source: EventSourceIdentity::session(session_id.clone()),
                    mob_id: None,
                    stream_seq: 0,
                    event: event.clone(),
                });
            }
        }
    }

    #[async_trait::async_trait]
    impl EventStore for MemEventStore {
        async fn append_envelopes(
            &self,
            session_id: &SessionId,
            envelopes: &[meerkat_core::event::EventEnvelope<AgentEvent>],
        ) -> Result<u64, EventStoreError> {
            let events: Vec<AgentEvent> = envelopes.iter().map(|env| env.payload.clone()).collect();
            self.add_events(session_id, &events);
            self.last_seq(session_id).await
        }

        async fn record_projection_halt(
            &self,
            session_id: &SessionId,
            reason: &str,
        ) -> Result<(), EventStoreError> {
            self.projection_halts.lock().unwrap().insert(
                session_id.to_string(),
                EventProjectionHaltMarker {
                    session_id: session_id.clone(),
                    reason: reason.to_string(),
                    recorded_at: SystemTime::now(),
                },
            );
            Ok(())
        }

        async fn projection_halt(
            &self,
            session_id: &SessionId,
        ) -> Result<Option<EventProjectionHaltMarker>, EventStoreError> {
            Ok(self
                .projection_halts
                .lock()
                .unwrap()
                .get(&session_id.to_string())
                .cloned())
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
                    input: meerkat_core::types::RunInput::Content {
                        content: meerkat_core::ContentInput::Text("Hello".to_string()),
                    },
                },
                AgentEvent::TextComplete {
                    content: "Hi there!".to_string(),
                },
                AgentEvent::RunCompleted {
                    session_id: sid.clone(),
                    result: "Hi there!".to_string(),
                    structured_output: None,
                    extraction_required: false,
                    usage: Usage::default(),
                    terminal_cause_kind: None,
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

        // Check checkpoint: the cursor commits both the last sequence and the
        // committed events.jsonl byte length.
        let checkpoint = read_checkpoint_cursor(&session_dir.join("checkpoint"));
        assert_eq!(checkpoint.last_seq, 3);
        assert_eq!(
            checkpoint.events_len,
            std::fs::metadata(session_dir.join("events.jsonl"))
                .unwrap()
                .len()
        );
    }

    #[tokio::test]
    async fn test_projector_rewrite_event_refreshes_summary() {
        let dir = TempDir::new().unwrap();
        let projector = SessionProjector::new(dir.path().join(".rkat"));
        let store = MemEventStore::new();
        let sid = SessionId::new();

        let mut session = meerkat_core::Session::with_id(sid.clone());
        session.push(Message::User(UserMessage::text("hello")));
        session.push(Message::BlockAssistant(BlockAssistantMessage {
            blocks: vec![AssistantBlock::Text {
                text: "old summary".to_string(),
                meta: None,
            }],
            stop_reason: StopReason::EndTurn,
            identity: meerkat_core::types::TranscriptMessageIdentity::default(),
            created_at: meerkat_core::types::message_timestamp_now(),
        }));
        let parent_revision = session.transcript_revision().unwrap();
        let commit = session
            .commit_transcript_rewrite(
                meerkat_core::TranscriptRewriteSelection::MessageRange { start: 1, end: 2 },
                Vec::new(),
                meerkat_core::TranscriptRewriteReason::new("compaction"),
                Some("projector-test".to_string()),
                Some(parent_revision.clone()),
            )
            .unwrap();
        let parent_body = session
            .transcript_revision_body(&parent_revision)
            .unwrap()
            .unwrap();
        let revision_body = session
            .transcript_revision_body(&commit.revision)
            .unwrap()
            .unwrap();
        let record =
            meerkat_core::TranscriptRewriteRecord::new(commit, parent_body, revision_body).unwrap();

        store.add_events(
            &sid,
            &[
                AgentEvent::TextComplete {
                    content: "old summary".to_string(),
                },
                AgentEvent::TranscriptRewriteCommitted {
                    session_id: sid.clone(),
                    record,
                },
            ],
        );

        projector.project(&store, &sid, 1).await.unwrap();
        assert!(
            !projector.session_dir(&sid).join("summary.txt").exists(),
            "rewrite removing assistant output must clear stale summary"
        );
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
        let lines: Vec<&str> = events_content.trim().lines().collect();
        assert_eq!(lines.len(), 3);
    }

    #[tokio::test]
    async fn test_projector_resume_rebuilds_after_invalid_checkpoint_without_duplicates() {
        let dir = TempDir::new().unwrap();
        let projector = SessionProjector::new(dir.path().join(".rkat"));
        let store = MemEventStore::new();
        let sid = SessionId::new();

        store.add_events(
            &sid,
            &[
                AgentEvent::TurnStarted { turn_number: 0 },
                AgentEvent::TextComplete {
                    content: "first".to_string(),
                },
            ],
        );
        projector.project(&store, &sid, 1).await.unwrap();

        let session_dir = projector.session_dir(&sid);
        tokio::fs::write(session_dir.join("checkpoint"), b"not-a-seq")
            .await
            .unwrap();
        store.add_events(&sid, &[AgentEvent::TurnStarted { turn_number: 1 }]);

        let seq = projector.resume(&store, &sid).await.unwrap();
        assert_eq!(seq, 3);

        let events_content = std::fs::read_to_string(session_dir.join("events.jsonl")).unwrap();
        let lines: Vec<&str> = events_content.trim().lines().collect();
        assert_eq!(lines.len(), 3);

        let checkpoint = read_checkpoint_cursor(&session_dir.join("checkpoint"));
        assert_eq!(checkpoint.last_seq, 3);
    }

    #[tokio::test]
    async fn test_projector_resume_rebuilds_after_stale_checkpoint_ahead_of_event_store_tail() {
        let dir = TempDir::new().unwrap();
        let projector = SessionProjector::new(dir.path().join(".rkat"));
        let store = MemEventStore::new();
        let sid = SessionId::new();

        store.add_events(
            &sid,
            &[
                AgentEvent::TurnStarted { turn_number: 0 },
                AgentEvent::TextComplete {
                    content: "authoritative tail".to_string(),
                },
            ],
        );
        projector.project(&store, &sid, 1).await.unwrap();

        let session_dir = projector.session_dir(&sid);
        tokio::fs::write(
            session_dir.join("checkpoint"),
            serde_json::to_string(&ProjectionCheckpoint {
                last_seq: 99,
                events_len: 17,
            })
            .unwrap()
            .as_bytes(),
        )
        .await
        .unwrap();
        tokio::fs::write(session_dir.join("events.jsonl"), b"stale projection\n")
            .await
            .unwrap();

        let seq = projector.resume(&store, &sid).await.unwrap();
        assert_eq!(seq, 2);

        let events_content = std::fs::read_to_string(session_dir.join("events.jsonl")).unwrap();
        assert!(!events_content.contains("stale projection"));
        let lines: Vec<&str> = events_content.trim().lines().collect();
        assert_eq!(lines.len(), 2);

        let checkpoint = read_checkpoint_cursor(&session_dir.join("checkpoint"));
        assert_eq!(checkpoint.last_seq, 2);
    }

    #[tokio::test]
    async fn test_projector_resume_missing_checkpoint_rebuilds_without_duplicates() {
        // Row #144 gate (fails-old / passes-new): a crash after events.jsonl is
        // written but before the checkpoint write leaves a partial events.jsonl
        // and NO checkpoint (CheckpointState::Missing). The OLD code called
        // project(from_seq=1), which APPENDS to that partial file, duplicating
        // every derived row. The fix rebuilds via replay (truncate-and-rebuild).
        let dir = TempDir::new().unwrap();
        let projector = SessionProjector::new(dir.path().join(".rkat"));
        let store = MemEventStore::new();
        let sid = SessionId::new();

        store.add_events(
            &sid,
            &[
                AgentEvent::TurnStarted { turn_number: 0 },
                AgentEvent::TextComplete {
                    content: "first".to_string(),
                },
            ],
        );
        projector.project(&store, &sid, 1).await.unwrap();

        let session_dir = projector.session_dir(&sid);
        // Simulate the crash window: events.jsonl exists, checkpoint is gone.
        tokio::fs::remove_file(session_dir.join("checkpoint"))
            .await
            .unwrap();
        assert!(session_dir.join("events.jsonl").exists());

        let seq = projector.resume(&store, &sid).await.unwrap();
        assert_eq!(seq, 2);

        // events.jsonl must be rebuilt (exactly 2 lines), NOT appended (4 lines).
        let events_content = std::fs::read_to_string(session_dir.join("events.jsonl")).unwrap();
        let lines: Vec<&str> = events_content.trim().lines().collect();
        assert_eq!(
            lines.len(),
            2,
            "missing-checkpoint resume must rebuild, not append duplicates"
        );

        let checkpoint = read_checkpoint_cursor(&session_dir.join("checkpoint"));
        assert_eq!(checkpoint.last_seq, 2);
    }

    #[tokio::test]
    async fn test_projector_resume_truncates_partial_tail_behind_valid_stale_checkpoint() {
        // Regression gate (valid-stale-checkpoint crash window): a crash inside
        // an incremental projection AFTER the events.jsonl append but BEFORE
        // the checkpoint write leaves a valid-but-stale checkpoint and a
        // partial tail past its committed byte length. Resume must truncate
        // back to the committed cursor before projecting, never append the
        // same derived events twice.
        let dir = TempDir::new().unwrap();
        let projector = SessionProjector::new(dir.path().join(".rkat"));
        let store = MemEventStore::new();
        let sid = SessionId::new();

        store.add_events(
            &sid,
            &[
                AgentEvent::TurnStarted { turn_number: 0 },
                AgentEvent::TextComplete {
                    content: "first".to_string(),
                },
            ],
        );
        projector.project(&store, &sid, 1).await.unwrap();
        let session_dir = projector.session_dir(&sid);
        let committed = read_checkpoint_cursor(&session_dir.join("checkpoint"));
        assert_eq!(committed.last_seq, 2);

        // Simulate the crashed incremental projection: event 3 lands durably
        // and its derived row is appended, but the crash happens before the
        // checkpoint write (checkpoint stays at the committed seq-2 cursor).
        store.add_events(&sid, &[AgentEvent::TurnStarted { turn_number: 1 }]);
        let partial_row =
            serde_json::to_string(&store.read_from(&sid, 3).await.unwrap().first().unwrap())
                .unwrap();
        use std::io::Write as _;
        let mut events_file = std::fs::OpenOptions::new()
            .append(true)
            .open(session_dir.join("events.jsonl"))
            .unwrap();
        writeln!(events_file, "{partial_row}").unwrap();
        drop(events_file);

        let seq = projector.resume(&store, &sid).await.unwrap();
        assert_eq!(seq, 3);

        // Exactly 3 derived rows: the partial tail was truncated, then event 3
        // was projected once.
        let events_content = std::fs::read_to_string(session_dir.join("events.jsonl")).unwrap();
        let lines: Vec<&str> = events_content.trim().lines().collect();
        assert_eq!(
            lines.len(),
            3,
            "valid-stale-checkpoint resume must truncate the partial tail, not duplicate rows"
        );
        assert_eq!(
            lines
                .iter()
                .filter(|line| line.contains("\"seq\":3"))
                .count(),
            1,
            "the partially appended event must appear exactly once after resume"
        );

        let checkpoint = read_checkpoint_cursor(&session_dir.join("checkpoint"));
        assert_eq!(checkpoint.last_seq, 3);
        assert_eq!(
            checkpoint.events_len,
            std::fs::metadata(session_dir.join("events.jsonl"))
                .unwrap()
                .len()
        );
    }

    #[tokio::test]
    async fn test_projector_resume_rebuilds_when_events_file_shorter_than_committed_cursor() {
        // If events.jsonl is shorter than the committed cursor (deleted or
        // tampered derived output), truncation cannot heal it; resume must
        // rebuild via full replay.
        let dir = TempDir::new().unwrap();
        let projector = SessionProjector::new(dir.path().join(".rkat"));
        let store = MemEventStore::new();
        let sid = SessionId::new();

        store.add_events(
            &sid,
            &[
                AgentEvent::TurnStarted { turn_number: 0 },
                AgentEvent::TextComplete {
                    content: "first".to_string(),
                },
            ],
        );
        projector.project(&store, &sid, 1).await.unwrap();
        let session_dir = projector.session_dir(&sid);
        tokio::fs::write(session_dir.join("events.jsonl"), b"x")
            .await
            .unwrap();
        store.add_events(&sid, &[AgentEvent::TurnStarted { turn_number: 1 }]);

        let seq = projector.resume(&store, &sid).await.unwrap();
        assert_eq!(seq, 3);

        let events_content = std::fs::read_to_string(session_dir.join("events.jsonl")).unwrap();
        let lines: Vec<&str> = events_content.trim().lines().collect();
        assert_eq!(lines.len(), 3);
        assert!(!events_content.starts_with('x'));
    }

    #[tokio::test]
    async fn test_projector_resume_rebuilds_after_unreadable_checkpoint_without_duplicates() {
        let dir = TempDir::new().unwrap();
        let projector = SessionProjector::new(dir.path().join(".rkat"));
        let store = MemEventStore::new();
        let sid = SessionId::new();

        store.add_events(
            &sid,
            &[
                AgentEvent::TurnStarted { turn_number: 0 },
                AgentEvent::TextComplete {
                    content: "first".to_string(),
                },
            ],
        );
        projector.project(&store, &sid, 1).await.unwrap();

        let session_dir = projector.session_dir(&sid);
        let checkpoint_path = session_dir.join("checkpoint");
        tokio::fs::remove_file(&checkpoint_path).await.unwrap();
        tokio::fs::create_dir(&checkpoint_path).await.unwrap();
        store.add_events(&sid, &[AgentEvent::TurnStarted { turn_number: 1 }]);

        let seq = projector.resume(&store, &sid).await.unwrap();
        assert_eq!(seq, 3);

        let events_content = std::fs::read_to_string(session_dir.join("events.jsonl")).unwrap();
        let lines: Vec<&str> = events_content.trim().lines().collect();
        assert_eq!(lines.len(), 3);

        assert!(checkpoint_path.is_file());
        let checkpoint = read_checkpoint_cursor(&checkpoint_path);
        assert_eq!(checkpoint.last_seq, 3);
    }
}
