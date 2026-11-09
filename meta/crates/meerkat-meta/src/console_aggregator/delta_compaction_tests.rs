//! Console projection rows over a real console log: a streamed block's
//! delta rows after the first go once the block's complete row is stored, a
//! removed delta is never stored again, and a restarted runtime's events are
//! all stored.
#![allow(clippy::expect_used)]

use super::*;
use crate::console_contracts::ConsoleIdentityEventEnvelope;

const RUNTIME: &str = "runtime-a";
const IDENTITY: &str = "worker";
const SESSION: &str = "0199f3c2-0000-7000-8000-000000000001";
const MESSAGE_A: &str = "0199f3c2-9b1e-7e2a-8c41-6d2f0a1b3c4d";
const MESSAGE_B: &str = "0199f3c2-9b1e-7e2a-8c41-6d2f0a1b3c4e";
const MESSAGE_C: &str = "0199f3c2-9b1e-7e2a-8c41-6d2f0a1b3c4f";

/// A member console event as the live projection receives it: the member
/// stream's sequence and the assistant message ride in the payload.
fn event(
    event_id: &str,
    kind: &str,
    message: &str,
    sequence: u64,
    text: &str,
) -> ConsoleIdentityEventEnvelope {
    ConsoleIdentityEventEnvelope {
        event_id: event_id.to_string(),
        interaction_id: None,
        identity: IDENTITY.to_string(),
        event_type: kind.to_string(),
        timestamp_ms: 1_000 + sequence,
        data: json!({
            "type": kind,
            "delta": text,
            "content": text,
            "assistant_message_id": message,
            "session_id": SESSION,
            "source_sequence": sequence,
            "source_epoch": "epoch.0.1",
        }),
    }
}

/// A streamed text block: three deltas, then the complete row.
fn text_block(
    prefix: &str,
    message: &str,
    first_sequence: u64,
) -> Vec<ConsoleIdentityEventEnvelope> {
    let mut events: Vec<_> = ["Hello", " streamed", " world."]
        .into_iter()
        .enumerate()
        .map(|(index, chunk)| {
            let sequence = first_sequence + u64::try_from(index).expect("index");
            event(
                &format!("{prefix}-delta-{index}"),
                "text_delta",
                message,
                sequence,
                chunk,
            )
        })
        .collect();
    events.push(event(
        &format!("{prefix}-complete"),
        "text_complete",
        message,
        first_sequence + 3,
        "Hello streamed world.",
    ));
    events
}

struct Fixture {
    _temp: tempfile::TempDir,
    runtime: Arc<UnifiedRuntime>,
    aggregator: MetaConsoleAggregator,
    events: ConsoleEventStore,
}

impl Fixture {
    async fn new(store: Arc<dyn ConsoleLogStore>) -> Self {
        let (temp, runtime, _) = super::tests::build_stress_runtime(0, Duration::ZERO).await;
        let events = ConsoleEventStore::new();
        let mut entry = super::tests::runtime_entry_for_test(RUNTIME, &runtime);
        entry.identity_namespace.clear();
        entry.console_events.clone_from(&events);
        let aggregator = MetaConsoleAggregator::new_with_options(
            store,
            ConsoleAggregatorOptions {
                session_history_backfill_enabled: false,
                ..ConsoleAggregatorOptions::default()
            },
        );
        aggregator
            .inner
            .runtimes
            .write()
            .expect("registry")
            .insert(RUNTIME.into(), entry);
        Self {
            _temp: temp,
            runtime,
            aggregator,
            events,
        }
    }

    /// Buffer the events in the runtime's console event store, as the
    /// runtime does, and project them, as the live projection does.
    async fn stream(&self, events: &[ConsoleIdentityEventEnvelope]) {
        for envelope in events {
            self.events.append_envelope(envelope.clone()).await;
        }
        self.project(events).await;
    }

    async fn project(&self, events: &[ConsoleIdentityEventEnvelope]) {
        for envelope in events {
            project_console_event(self.aggregator.inner.clone(), RUNTIME, envelope.clone())
                .await
                .expect("project");
        }
    }

    /// The identity's rows: (frame id, kind).
    async fn rows(&self) -> Vec<(String, String)> {
        self.aggregator
            .store()
            .query_windowed_frames(ConsoleTimelineWindowQuery {
                identity: Some(IDENTITY.into()),
                limit: 1_000,
                ..ConsoleTimelineWindowQuery::default()
            })
            .await
            .expect("read")
            .frames
            .into_iter()
            .map(|frame| (frame.id, frame.kind))
            .collect()
    }

    async fn stop(self) {
        let _ = self.runtime.mob_handle().stop().await;
    }
}

fn row(id: &str, kind: &str) -> (String, String) {
    (id.to_string(), kind.to_string())
}

fn stores() -> (tempfile::TempDir, [Arc<dyn ConsoleLogStore>; 2]) {
    let dir = tempfile::tempdir().expect("tempdir");
    let sqlite: Arc<dyn ConsoleLogStore> = Arc::new(
        SqliteConsoleLogStore::open(dir.path().join("console.sqlite3")).expect("sqlite store"),
    );
    let memory: Arc<dyn ConsoleLogStore> = Arc::new(InMemoryConsoleLogStore::new());
    (dir, [sqlite, memory])
}

#[tokio::test]
async fn after_the_complete_row_the_deltas_are_gone_and_the_timeline_shows_the_complete_text() {
    let (_dir, stores) = stores();
    for store in stores {
        let fixture = Fixture::new(store.clone()).await;
        let block = text_block("a", MESSAGE_A, 1);
        // While the block streams, every delta is stored and delivered live.
        let mut live = fixture.aggregator.subscribe();
        fixture.stream(&block[..3]).await;
        assert_eq!(fixture.rows().await.len(), 3);
        fixture.stream(&block[3..]).await;
        // The first delta stays and holds the block's place.
        assert_eq!(
            fixture.rows().await,
            [
                row("a-delta-0", "text_delta"),
                row("a-complete", "text_complete")
            ]
        );
        let page = store
            .query_windowed_frames(ConsoleTimelineWindowQuery {
                identity: Some(IDENTITY.into()),
                limit: 1_000,
                ..ConsoleTimelineWindowQuery::default()
            })
            .await
            .expect("read");
        assert_eq!(page.frames[1].payload["content"], "Hello streamed world.");
        let mut delivered = Vec::new();
        while let Ok(ConsoleTimelineEvent::ConsoleFrame { frame }) = live.try_recv() {
            delivered.push(frame.kind);
        }
        assert_eq!(
            delivered,
            ["text_delta", "text_delta", "text_delta", "text_complete"]
        );
        fixture.stop().await;
    }
}

#[tokio::test]
async fn an_interrupted_message_keeps_its_deltas() {
    let (_dir, stores) = stores();
    for store in stores {
        let fixture = Fixture::new(store).await;
        // The block never completes; another one does.
        let interrupted = text_block("a", MESSAGE_A, 1);
        fixture.stream(&interrupted[..2]).await;
        fixture.stream(&text_block("b", MESSAGE_B, 10)).await;
        assert_eq!(
            fixture.rows().await,
            [
                row("a-delta-0", "text_delta"),
                row("a-delta-1", "text_delta"),
                row("b-delta-0", "text_delta"),
                row("b-complete", "text_complete"),
            ]
        );
        fixture.stop().await;
    }
}

/// Every path that delivers console events to the projection again finds
/// the removed deltas recognised: registration replay, lag recovery, the
/// shutdown drain, and a live receiver re-reading its channel.
#[tokio::test]
async fn replays_after_compaction_store_no_duplicate_fragment() {
    let (_dir, stores) = stores();
    for store in stores {
        let fixture = Fixture::new(store.clone()).await;
        let before = event("before", "run_started", MESSAGE_A, 0, "");
        let block = text_block("a", MESSAGE_A, 1);
        fixture.stream(std::slice::from_ref(&before)).await;
        fixture.stream(&block).await;
        let expected = vec![
            row("before", "run_started"),
            row("a-delta-0", "text_delta"),
            row("a-complete", "text_complete"),
        ];
        assert_eq!(fixture.rows().await, expected);

        // A live receiver that lagged gets the same events from its channel.
        fixture.project(&block).await;
        assert_eq!(fixture.rows().await, expected, "channel redelivery");

        // Lag recovery replays every buffered event after the watermark.
        store
            .record_source_watermark(RUNTIME, ConsoleFrameSourceKind::ConsoleEvent, "before")
            .await
            .expect("watermark");
        recover_lagged_source_events(fixture.aggregator.inner.clone(), RUNTIME, &fixture.events)
            .await
            .expect("lag recovery");
        assert_eq!(fixture.rows().await, expected, "lag recovery");

        // The shutdown drain projects every event of its final drains.
        let buffered = fixture.events.replay_all(None).await.expect("buffered");
        fixture
            .aggregator
            .project_runtime_events(RUNTIME, buffered)
            .await
            .expect("shutdown drain");
        assert_eq!(fixture.rows().await, expected, "shutdown drain");

        // Registration replays the whole buffer.
        fixture.aggregator.register_runtime_handles_with_policy(
            RUNTIME,
            "",
            fixture.runtime.mob_runtime().clone(),
            None,
            fixture.events.clone(),
            Arc::new(AllowAllConsoleVisibilityPolicy),
        );
        fixture.aggregator.history_backfill_converged().await;
        assert_eq!(fixture.rows().await, expected, "registration replay");
        fixture.stop().await;
    }
}

#[tokio::test]
async fn a_second_reasoning_block_of_the_message_streams_and_compacts() {
    let (_dir, stores) = stores();
    for store in stores {
        let fixture = Fixture::new(store).await;
        let first = [
            event("r1-delta-0", "reasoning_delta", MESSAGE_A, 1, "First"),
            event("r1-delta-1", "reasoning_delta", MESSAGE_A, 2, " thought."),
            event(
                "r1-complete",
                "reasoning_complete",
                MESSAGE_A,
                3,
                "First thought.",
            ),
        ];
        let second = [
            event("r2-delta-0", "reasoning_delta", MESSAGE_A, 4, "Second"),
            event("r2-delta-1", "reasoning_delta", MESSAGE_A, 5, " thought."),
            event(
                "r2-complete",
                "reasoning_complete",
                MESSAGE_A,
                6,
                "Second thought.",
            ),
        ];
        fixture.stream(&first).await;
        fixture.stream(&second[..2]).await;
        assert_eq!(
            fixture.rows().await,
            [
                row("r1-delta-0", "reasoning_delta"),
                row("r1-complete", "reasoning_complete"),
                row("r2-delta-0", "reasoning_delta"),
                row("r2-delta-1", "reasoning_delta"),
            ],
            "the second block streams after the first one compacted"
        );
        fixture.stream(&second[2..]).await;
        // A replay of either block stores nothing again.
        fixture.project(&first).await;
        fixture.project(&second).await;
        assert_eq!(
            fixture.rows().await,
            [
                row("r1-delta-0", "reasoning_delta"),
                row("r1-complete", "reasoning_complete"),
                row("r2-delta-0", "reasoning_delta"),
                row("r2-complete", "reasoning_complete"),
            ]
        );
        fixture.stop().await;
    }
}

/// With no room to follow another block from its first delta, compaction
/// stops for the runtime: the blocks it was following keep their deltas.
#[tokio::test]
async fn on_open_block_overflow_the_dropped_blocks_keep_their_deltas() {
    let (_dir, stores) = stores();
    for store in stores {
        let fixture = Fixture::new(store).await;
        *fixture.aggregator.inner.streamed_blocks.lock().await =
            delta_compaction::StreamedBlocks::with_max_blocks(2);
        let a = text_block("a", MESSAGE_A, 1);
        let b = text_block("b", MESSAGE_B, 10);
        let c = text_block("c", MESSAGE_C, 20);
        // Two blocks open, then a third finds no room.
        fixture.stream(&a[..1]).await;
        fixture.stream(&b[..1]).await;
        fixture.stream(&c[..1]).await;
        for block in [&a, &b, &c] {
            fixture.stream(&block[1..]).await;
        }
        let rows = fixture.rows().await;
        for envelope in a.iter().chain(&b).chain(&c) {
            assert!(
                rows.iter().any(|(id, _)| id == &envelope.event_id),
                "{} kept",
                envelope.event_id
            );
        }
        assert_eq!(rows.len(), 12);
        fixture.stop().await;
    }
}

/// A legacy history row, with no typed run, interaction or assistant
/// message id, still finds its live counterpart once the block's deltas
/// after the first are gone: the complete row carries the block's whole
/// text, so no duplicate history row is appended. (A history row that names
/// its assistant message never searches; only blocks that name one are
/// compacted.)
#[tokio::test]
async fn a_legacy_history_row_finds_the_compacted_blocks_complete_row() {
    let fixture = Fixture::new(Arc::new(InMemoryConsoleLogStore::new())).await;
    fixture.stream(&text_block("a", MESSAGE_A, 1)).await;
    assert_eq!(
        fixture.rows().await,
        [
            row("a-delta-0", "text_delta"),
            row("a-complete", "text_complete")
        ]
    );
    let history = NewConsoleFrame {
        id: None,
        dedupe_key: "history-answer".into(),
        timestamp_ms: 2_000,
        runtime_key: RUNTIME.into(),
        identity: IDENTITY.into(),
        conversation_id: Some(IDENTITY.into()),
        session_id: Some(SESSION.into()),
        kind: "text_complete".into(),
        status: ConsoleFrameStatus::Completed,
        payload: json!({ "text": "Hello streamed world." }),
        source: ConsoleFrameSource {
            member_provenance: None,
            kind: ConsoleFrameSourceKind::SessionHistory,
            source_cursor: Some(format!("{SESSION}:1")),
        },
        source_event_id: None,
        interaction_id: None,
        turn_id: None,
        run_id: None,
        parent_frame_id: None,
        caused_by_frame_id: None,
    };
    assert!(
        history_frame_has_existing_counterpart(&fixture.aggregator.inner, &history)
            .await
            .expect("counterpart search")
    );
    fixture.stop().await;
}

/// A restart opens the same persistent log with a new runtime event store:
/// the events that store mints (member lifecycle records, queue overflow
/// failures, memory events) are stored, not dropped as already seen.
#[tokio::test]
async fn a_restarted_runtime_stores_its_own_events_in_a_persistent_log() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("console.sqlite3");
    let mut ids = Vec::new();
    for lifecycle in ["member_spawned", "member_retired"] {
        let store: Arc<dyn ConsoleLogStore> =
            Arc::new(SqliteConsoleLogStore::open(&path).expect("open console log"));
        let fixture = Fixture::new(store).await;
        let envelope = fixture
            .events
            .append(IDENTITY, None, lifecycle, json!({}))
            .await;
        fixture.project(std::slice::from_ref(&envelope)).await;
        ids.push(envelope.event_id);
        fixture.stop().await;
    }
    assert_ne!(ids[0], ids[1]);
    let reopened = SqliteConsoleLogStore::open(&path).expect("reopen console log");
    let kinds: Vec<String> = reopened
        .query_windowed_frames(ConsoleTimelineWindowQuery {
            identity: Some(IDENTITY.into()),
            limit: 1_000,
            ..ConsoleTimelineWindowQuery::default()
        })
        .await
        .expect("read")
        .frames
        .into_iter()
        .map(|frame| frame.kind)
        .collect();
    assert_eq!(kinds, ["member_spawned", "member_retired"]);
}
