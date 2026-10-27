use super::*;
use meerkat_core::event::AgentEvent;
use meerkat_core::lifecycle::{InputId, RunId};
use meerkat_core::types::{RuntimeAppendOrigin, SessionId, SystemNoticeKind, SystemNoticeMessage};
use std::sync::atomic::AtomicUsize;

const RUNTIME: &str = "notice-runtime";
const IDENTITY: &str = "notice-worker";

fn notice(session: &SessionId, run: u128, input: u128) -> SystemNoticeMessage {
    let mut notice = SystemNoticeMessage::new(SystemNoticeKind::Generic, "Exact boundary notice.");
    notice.runtime_origin = Some(RuntimeAppendOrigin {
        session_id: session.clone(),
        run_id: RunId(uuid::Uuid::from_u128(run)),
        input_id: InputId(uuid::Uuid::from_u128(input)),
        append_ordinal: 0,
    });
    notice
}

fn frame(key: &str, session: &SessionId, kind: &str, payload: Value) -> NewConsoleFrame {
    NewConsoleFrame {
        id: None,
        dedupe_key: key.to_string(),
        timestamp_ms: 1,
        runtime_key: RUNTIME.to_string(),
        identity: IDENTITY.to_string(),
        conversation_id: Some(IDENTITY.to_string()),
        session_id: Some(session.to_string()),
        kind: kind.to_string(),
        status: ConsoleFrameStatus::Completed,
        payload,
        source: ConsoleFrameSource {
            member_provenance: None,
            kind: ConsoleFrameSourceKind::ConsoleEvent,
            source_cursor: None,
        },
        source_event_id: None,
        interaction_id: None,
        turn_id: None,
        run_id: None,
        parent_frame_id: None,
        caused_by_frame_id: None,
    }
}

fn applied(key: &str, session: &SessionId, run: u128, input: u128) -> NewConsoleFrame {
    let mut frame = frame(
        key,
        session,
        "boundary_append_applied",
        json!(AgentEvent::BoundaryAppendApplied {
            run_id: RunId(uuid::Uuid::from_u128(run)),
            input_id: InputId(uuid::Uuid::from_u128(input)),
            content: "Model compatibility projection.".into(),
            append_count: 1,
            notices: vec![notice(session, run, input)],
            transcript_start: Some(2),
        }),
    );
    frame.run_id = Some(uuid::Uuid::from_u128(run).to_string());
    frame
}

fn attempt(run: u128, input: u128) -> (uuid::Uuid, uuid::Uuid) {
    (uuid::Uuid::from_u128(run), uuid::Uuid::from_u128(input))
}

async fn observe(
    aggregator: &MetaConsoleAggregator,
    session: &SessionId,
) -> ConsoleLogResult<RuntimeNoticeObservation> {
    observe_runtime_notice_attempts(&aggregator.inner, RUNTIME, IDENTITY, &session.to_string())
        .await
}

#[tokio::test]
async fn notice_scan_reaches_exact_attempts_before_and_after_one_thousand_rows()
-> ConsoleLogResult<()> {
    let aggregator = MetaConsoleAggregator::in_memory();
    let session = SessionId::new();
    let mut expected = BTreeSet::new();
    let mut relevant_frontier = 0;
    for index in 0_u128..2_105 {
        let row = if [0, 999, 1_000, 2_000].contains(&index) {
            expected.insert(attempt(10, 100 + index));
            applied(&format!("applied-{index}"), &session, 10, 100 + index)
        } else {
            frame(
                &format!("noise-{index}"),
                &session,
                "text_delta",
                json!({ "delta": "." }),
            )
        };
        let is_notice = row.kind == "boundary_append_applied";
        let saved = aggregator.store().append_if_absent(row).await?.frame;
        if is_notice {
            relevant_frontier = saved.cursor.seq().ok_or("missing assigned cursor")?;
        }
    }
    let observed = observe(&aggregator, &session).await?;
    assert_eq!(observed.observed_through, 2_105);
    assert_eq!(observed.relevant_frontier, relevant_frontier);
    assert_eq!(observed.attempts, expected);
    assert!(!observed.has_notice_history);
    assert!(observed.previous_snapshot.is_none());
    Ok(())
}

#[tokio::test]
async fn notice_scan_rejects_conflicting_source_scope_and_typed_attempt_identity()
-> ConsoleLogResult<()> {
    let aggregator = MetaConsoleAggregator::in_memory();
    let session = SessionId::new();
    let other_session = SessionId::new();
    let accepted = aggregator
        .store()
        .append_if_absent(applied("accepted", &session, 10, 20))
        .await?
        .frame;
    for index in 0..12 {
        let mut invalid = applied(&format!("invalid-{index}"), &session, 30, 40 + index);
        match index {
            0 => invalid.runtime_key = "other-runtime".to_string(),
            1 => invalid.session_id = Some(other_session.to_string()),
            2 => invalid.identity = "other-worker".to_string(),
            3 => invalid.source.kind = ConsoleFrameSourceKind::SessionHistory,
            4 => invalid.source.kind = ConsoleFrameSourceKind::Synthetic,
            5 => invalid.run_id = Some(uuid::Uuid::from_u128(31).to_string()),
            6 => invalid.payload["run_id"] = json!(uuid::Uuid::from_u128(31)),
            7 => invalid.payload["input_id"] = json!(uuid::Uuid::from_u128(99)),
            8 => {
                invalid.payload["notices"][0]["runtime_origin"]["session_id"] =
                    json!(other_session);
            }
            9 => {
                invalid.payload["notices"][0]["runtime_origin"]["run_id"] =
                    json!(uuid::Uuid::from_u128(31));
            }
            10 => {
                invalid.payload["notices"][0]["runtime_origin"]["input_id"] =
                    json!(uuid::Uuid::from_u128(99));
            }
            _ => invalid.payload["notices"][0]["runtime_origin"] = Value::Null,
        }
        aggregator.store().append_if_absent(invalid).await?;
    }
    let observed = observe(&aggregator, &session).await?;
    assert_eq!(observed.attempts, BTreeSet::from([attempt(10, 20)]));
    assert_eq!(
        observed.relevant_frontier,
        accepted.cursor.seq().ok_or("missing assigned cursor")?
    );
    assert!(!observed.has_notice_history);
    assert!(observed.previous_snapshot.is_none());
    Ok(())
}

#[tokio::test]
async fn notice_scan_uses_latest_exact_history_snapshot_and_ignores_foreign_history()
-> ConsoleLogResult<()> {
    let aggregator = MetaConsoleAggregator::in_memory();
    let session = SessionId::new();
    let message = Message::SystemNotice(notice(&session, 10, 20));
    let mut history = frame(
        "history",
        &session,
        "system_notice",
        json!({ "message": message }),
    );
    history.source.kind = ConsoleFrameSourceKind::SessionHistory;
    let history = aggregator.store().append_if_absent(history).await?.frame;
    let first_observed = observe(&aggregator, &session).await?;
    let first = runtime_notice_snapshot_frame(
        RUNTIME,
        IDENTITY,
        &session.to_string(),
        &first_observed,
        &BTreeSet::new(),
        std::slice::from_ref(&message),
    )
    .ok_or("initial snapshot missing")?;
    let first = aggregator.store().append_if_absent(first).await?.frame;
    let observed = observe(&aggregator, &session).await?;
    let removed = runtime_notice_snapshot_frame(
        RUNTIME,
        IDENTITY,
        &session.to_string(),
        &observed,
        &BTreeSet::new(),
        &[],
    )
    .ok_or("empty replacement snapshot missing")?;
    let removed = aggregator.store().append_if_absent(removed).await?.frame;
    for index in 0..3 {
        let mut foreign = frame(
            &format!("foreign-snapshot-{index}"),
            &session,
            "runtime_notice_snapshot",
            first.payload.clone(),
        );
        foreign.source.kind = ConsoleFrameSourceKind::SessionHistory;
        match index {
            0 => foreign.runtime_key = "other-runtime".to_string(),
            1 => foreign.session_id = Some(SessionId::new().to_string()),
            _ => foreign.source.kind = ConsoleFrameSourceKind::ConsoleEvent,
        }
        aggregator.store().append_if_absent(foreign).await?;
    }
    let mut foreign_history = frame(
        "foreign-history",
        &session,
        "system_notice",
        json!({ "message": message }),
    );
    foreign_history.runtime_key = "other-runtime".to_string();
    foreign_history.source.kind = ConsoleFrameSourceKind::SessionHistory;
    aggregator.store().append_if_absent(foreign_history).await?;
    let observed = observe(&aggregator, &session).await?;
    assert!(observed.has_notice_history);
    assert!(observed.attempts.is_empty());
    assert_eq!(
        observed.relevant_frontier,
        history.cursor.seq().ok_or("missing assigned cursor")?
    );
    assert_eq!(
        observed.previous_snapshot.as_ref().map(|row| &row.id),
        Some(&removed.id)
    );
    Ok(())
}

// The rows still live in the real in-memory store. This wrapper injects only a
// read boundary race or a page failure, not a simulated scan result.
struct ObservedStore {
    inner: InMemoryConsoleLogStore,
    append_after_cursor: std::sync::Mutex<Vec<NewConsoleFrame>>,
    query_count: AtomicUsize,
    fail_query: AtomicUsize,
    query_afters: std::sync::Mutex<Vec<Option<u64>>>,
}

impl ObservedStore {
    fn new(append_after_cursor: Vec<NewConsoleFrame>, fail_query: Option<usize>) -> Self {
        Self {
            inner: InMemoryConsoleLogStore::new(),
            append_after_cursor: std::sync::Mutex::new(append_after_cursor),
            query_count: AtomicUsize::new(0),
            fail_query: AtomicUsize::new(fail_query.unwrap_or(0)),
            query_afters: std::sync::Mutex::new(Vec::new()),
        }
    }
}

#[async_trait::async_trait]
impl ConsoleLogStore for ObservedStore {
    async fn append_if_absent(&self, row: NewConsoleFrame) -> ConsoleLogResult<AppendOutcome> {
        self.inner.append_if_absent(row).await
    }

    async fn update_frame_status(
        &self,
        id: &str,
        status: ConsoleFrameStatus,
    ) -> ConsoleLogResult<Option<ConsoleFrame>> {
        self.inner.update_frame_status(id, status).await
    }

    async fn query_frames(
        &self,
        query: ConsoleTimelineQuery,
    ) -> ConsoleLogResult<ConsoleTimelinePage> {
        let call = self.query_count.fetch_add(1, Ordering::SeqCst) + 1;
        self.query_afters
            .lock()
            .map_err(|_| std::io::Error::other("query cursor log lock poisoned"))?
            .push(query.after.as_ref().and_then(ConsoleCursor::seq));
        if self.fail_query.load(Ordering::SeqCst) == call {
            return Err(std::io::Error::other("injected later scan page failure").into());
        }
        self.inner.query_frames(query).await
    }

    async fn frame_by_dedupe_key(&self, key: &str) -> ConsoleLogResult<Option<ConsoleFrame>> {
        self.inner.frame_by_dedupe_key(key).await
    }

    async fn latest_cursor(&self) -> ConsoleLogResult<Option<ConsoleCursor>> {
        let observed = self.inner.latest_cursor().await?;
        let rows = std::mem::take(
            &mut *self
                .append_after_cursor
                .lock()
                .map_err(|_| std::io::Error::other("test row lock poisoned"))?,
        );
        for row in rows {
            self.inner.append_if_absent(row).await?;
        }
        Ok(observed)
    }

    async fn clear_frames(&self) -> ConsoleLogResult<()> {
        self.inner.clear_frames().await
    }

    async fn history_prefix_revision(&self) -> ConsoleLogResult<Option<String>> {
        self.inner.history_prefix_revision().await
    }

    async fn record_source_watermark(
        &self,
        runtime: &str,
        kind: ConsoleFrameSourceKind,
        cursor: &str,
    ) -> ConsoleLogResult<()> {
        self.inner
            .record_source_watermark(runtime, kind, cursor)
            .await
    }

    async fn source_watermark(
        &self,
        runtime: &str,
        kind: ConsoleFrameSourceKind,
    ) -> ConsoleLogResult<Option<String>> {
        self.inner.source_watermark(runtime, kind).await
    }
}

#[tokio::test]
async fn notice_scan_observed_through_excludes_rows_appended_between_cursor_and_scan()
-> ConsoleLogResult<()> {
    let session = SessionId::new();
    let mut late_history = frame(
        "late-history",
        &session,
        "system_notice",
        json!({ "message": Message::SystemNotice(notice(&session, 11, 21)) }),
    );
    late_history.source.kind = ConsoleFrameSourceKind::SessionHistory;
    let mut late_snapshot = frame(
        "late-snapshot",
        &session,
        "runtime_notice_snapshot",
        json!({}),
    );
    late_snapshot.source.kind = ConsoleFrameSourceKind::SessionHistory;
    let store = Arc::new(ObservedStore::new(
        vec![
            applied("late-application", &session, 11, 21),
            late_history,
            late_snapshot,
        ],
        None,
    ));
    let aggregator = MetaConsoleAggregator::new(store.clone());
    store
        .append_if_absent(applied("observed-application", &session, 10, 20))
        .await?;
    let observed = observe(&aggregator, &session).await?;
    assert_eq!(observed.observed_through, 1);
    assert_eq!(observed.relevant_frontier, 1);
    assert_eq!(observed.attempts, BTreeSet::from([attempt(10, 20)]));
    assert!(!observed.has_notice_history);
    assert!(observed.previous_snapshot.is_none());
    assert_eq!(
        store
            .inner
            .latest_cursor()
            .await?
            .and_then(|cursor| cursor.seq()),
        Some(4)
    );
    let snapshot = runtime_notice_snapshot_frame(
        RUNTIME,
        IDENTITY,
        &session.to_string(),
        &observed,
        &observed.attempt_strings(),
        &[],
    )
    .ok_or("settled snapshot missing")?;
    assert_eq!(snapshot.payload["observed_through"], "console:1");
    assert_eq!(
        snapshot.payload["settled_attempts"],
        json!([{
            "run_id": uuid::Uuid::from_u128(10), "input_id": uuid::Uuid::from_u128(20),
        }])
    );
    Ok(())
}

#[tokio::test]
async fn notice_scan_late_page_failure_returns_no_partial_observation() -> ConsoleLogResult<()> {
    let session = SessionId::new();
    let store = Arc::new(ObservedStore::new(Vec::new(), Some(2)));
    let aggregator = MetaConsoleAggregator::new(store.clone());
    store
        .append_if_absent(applied("old-application", &session, 10, 20))
        .await?;
    for index in 0..1_005 {
        store
            .append_if_absent(frame(
                &format!("noise-{index}"),
                &session,
                "text_delta",
                json!({ "delta": "." }),
            ))
            .await?;
    }
    let result = observe(&aggregator, &session).await;
    assert!(
        result.is_err(),
        "a partial scan must not become a complete observation"
    );
    assert_eq!(store.query_count.load(Ordering::SeqCst), 2);
    let first_page = store
        .inner
        .query_frames(ConsoleTimelineQuery {
            limit: 1_000,
            ..Default::default()
        })
        .await?;
    let last_page = store
        .inner
        .query_frames(ConsoleTimelineQuery {
            after: first_page.frames.last().map(|row| row.cursor.clone()),
            limit: 1_000,
            ..Default::default()
        })
        .await?;
    let rows: Vec<_> = first_page
        .frames
        .into_iter()
        .chain(last_page.frames)
        .collect();
    assert_eq!(rows.len(), 1_006);
    assert!(rows.iter().all(|row| row.kind != "runtime_notice_snapshot"));
    assert!(rows.iter().any(|row| row.dedupe_key == "old-application"));
    Ok(())
}

#[tokio::test]
async fn notice_snapshot_rescan_publishes_changed_origin_offset_and_body_but_coalesces_identical_images()
-> ConsoleLogResult<()> {
    let aggregator = MetaConsoleAggregator::in_memory();
    let session = SessionId::new();
    let original = notice(&session, 10, 20);
    let mut changed_origin = original.clone();
    changed_origin
        .runtime_origin
        .as_mut()
        .ok_or("test origin missing")?
        .input_id = InputId(uuid::Uuid::from_u128(21));
    let mut changed_body = changed_origin.clone();
    changed_body.body = Some("Canonical text changed without changing identity.".to_string());
    let legacy = Message::SystemNotice(SystemNoticeMessage::new(
        SystemNoticeKind::Generic,
        "Legacy context.",
    ));
    let images = [
        vec![Message::SystemNotice(original)],
        vec![Message::SystemNotice(changed_origin.clone())],
        vec![legacy.clone(), Message::SystemNotice(changed_origin)],
        vec![legacy, Message::SystemNotice(changed_body)],
    ];
    let mut previous_payload = None;
    for (index, messages) in images.iter().enumerate() {
        let observed = observe(&aggregator, &session).await?;
        let snapshot = runtime_notice_snapshot_frame(
            RUNTIME,
            IDENTITY,
            &session.to_string(),
            &observed,
            &BTreeSet::new(),
            messages,
        )
        .ok_or("changed image did not publish")?;
        assert_ne!(
            previous_payload.as_ref(),
            Some(&snapshot.payload["notices"])
        );
        assert_eq!(
            snapshot.payload["notices"][0]["offset"],
            usize::from(index >= 2)
        );
        previous_payload = Some(snapshot.payload["notices"].clone());
        let saved = aggregator.store().append_if_absent(snapshot).await?;
        assert_eq!(saved.disposition, AppendDisposition::Inserted);
        let rescanned = observe(&aggregator, &session).await?;
        assert_eq!(
            rescanned.previous_snapshot.as_ref().map(|row| &row.id),
            Some(&saved.frame.id)
        );
        assert!(
            runtime_notice_snapshot_frame(
                RUNTIME,
                IDENTITY,
                &session.to_string(),
                &rescanned,
                &BTreeSet::new(),
                messages
            )
            .is_none(),
            "unchanged image should coalesce"
        );
    }
    let rows = aggregator
        .store()
        .query_frames(ConsoleTimelineQuery {
            limit: 20,
            ..Default::default()
        })
        .await?;
    assert_eq!(rows.frames.len(), 4);
    Ok(())
}

async fn append_canonical_message(
    aggregator: &MetaConsoleAggregator,
    session: &SessionId,
    offset: usize,
    message: &Message,
) -> ConsoleLogResult<Vec<ConsoleFrame>> {
    let mut saved = Vec::new();
    for row in frames_from_session_history_message_with_namespace(
        RUNTIME,
        IDENTITY,
        "",
        &session.to_string(),
        offset,
        serde_json::to_value(message)?,
    ) {
        saved.push(aggregator.store().append_if_absent(row).await?.frame);
    }
    Ok(saved)
}

fn assistant_message(text: &str) -> ConsoleLogResult<Message> {
    Ok(serde_json::from_value(json!({
        "role": "block_assistant",
        "blocks": [{ "block_type": "text", "data": { "text": text } }],
        "identity": {
            "interaction_id": uuid::Uuid::from_u128(50),
            "run_id": uuid::Uuid::from_u128(10),
        },
        "stop_reason": "end_turn",
        "created_at": "1970-01-01T00:00:00.100Z",
    }))?)
}

#[tokio::test]
async fn notice_snapshot_append_only_history_does_not_repeat_full_images() -> ConsoleLogResult<()> {
    let aggregator = MetaConsoleAggregator::in_memory();
    let session = SessionId::new();
    let mut messages = vec![Message::SystemNotice(notice(&session, 10, 20))];
    let observed = observe(&aggregator, &session).await?;
    let snapshot = runtime_notice_snapshot_frame(
        RUNTIME,
        IDENTITY,
        &session.to_string(),
        &observed,
        &BTreeSet::new(),
        &messages,
    )
    .ok_or("initial snapshot missing")?;
    aggregator.store().append_if_absent(snapshot).await?;
    for index in 0..32 {
        let message = assistant_message(&format!("Completed answer {index}."))?;
        append_canonical_message(&aggregator, &session, messages.len(), &message).await?;
        messages.push(message);
        let observed = observe(&aggregator, &session).await?;
        assert!(
            runtime_notice_snapshot_frame(
                RUNTIME,
                IDENTITY,
                &session.to_string(),
                &observed,
                &BTreeSet::new(),
                &messages,
            )
            .is_none(),
            "append-only history must not republish old notices and positions"
        );
    }
    Ok(())
}

#[tokio::test]
async fn redacted_append_only_history_keeps_exact_dedupe_coordinates() -> ConsoleLogResult<()> {
    let aggregator = MetaConsoleAggregator::in_memory();
    let session = SessionId::new();
    let mut messages = vec![Message::SystemNotice(notice(&session, 10, 20))];
    let first = runtime_notice_snapshot_frame(
        RUNTIME,
        IDENTITY,
        &session.to_string(),
        &observe(&aggregator, &session).await?,
        &BTreeSet::new(),
        &messages,
    )
    .ok_or("initial snapshot")?;
    aggregator.store().append_if_absent(first).await?;
    for _ in 0..8 {
        let message = assistant_message("Repeated private answer.")?;
        for mut row in frames_from_session_history_message_with_namespace(
            RUNTIME,
            IDENTITY,
            "",
            &session.to_string(),
            messages.len(),
            serde_json::to_value(&message)?,
        ) {
            row.payload = json!({"text":"[host redacted]"});
            row.status = ConsoleFrameStatus::Redacted;
            aggregator.store().append_if_absent(row).await?;
        }
        messages.push(message);
        assert!(
            runtime_notice_snapshot_frame(
                RUNTIME,
                IDENTITY,
                &session.to_string(),
                &observe(&aggregator, &session).await?,
                &BTreeSet::new(),
                &messages
            )
            .is_none(),
            "redacted unchanged source rows need no removal snapshots"
        );
    }
    Ok(())
}

#[tokio::test]
async fn exact_survivor_cannot_lend_its_coordinate_to_earlier_repeated_row() -> ConsoleLogResult<()>
{
    let aggregator = MetaConsoleAggregator::in_memory();
    let session = SessionId::new();
    let message = assistant_message("Repeated answer.")?;
    let older = append_canonical_message(&aggregator, &session, 0, &message)
        .await?
        .remove(0);
    let survivor = append_canonical_message(&aggregator, &session, 2, &message)
        .await?
        .remove(0);
    let filler = Message::SystemNotice(SystemNoticeMessage::new(
        SystemNoticeKind::Generic,
        "Context.",
    ));
    let (positions, removed) = current_history_position_changes(
        RUNTIME,
        IDENTITY,
        &session.to_string(),
        &observe(&aggregator, &session).await?,
        &[filler.clone(), filler, message],
    );
    assert!(
        positions.is_empty(),
        "exact survivor stays at its original coordinate"
    );
    assert_eq!(removed, vec![older.id]);
    assert!(!removed.contains(&survivor.id));
    Ok(())
}

#[tokio::test]
async fn complete_notice_positions_remove_and_restore_original_coordinates() -> ConsoleLogResult<()>
{
    let aggregator = MetaConsoleAggregator::in_memory();
    let session = SessionId::new();
    let durable = Message::SystemNotice(notice(&session, 10, 20));
    let answer = assistant_message("Retained answer.")?;
    let original = append_canonical_message(&aggregator, &session, 1, &answer)
        .await?
        .into_iter()
        .next()
        .ok_or("answer frame")?;
    let observed = observe(&aggregator, &session).await?;
    let snapshot = runtime_notice_snapshot_frame(
        RUNTIME,
        IDENTITY,
        &session.to_string(),
        &observed,
        &BTreeSet::new(),
        std::slice::from_ref(&durable),
    )
    .ok_or("removal snapshot")?;
    assert!(snapshot.payload.get("history_positions_mode").is_none());
    assert_eq!(snapshot.payload["history_positions"], json!([]));
    assert!(snapshot.payload.get("removed_history_frame_ids").is_none());
    aggregator.store().append_if_absent(snapshot).await?;
    let mut observed = observe(&aggregator, &session).await?;
    let messages = [durable, answer];
    restore_current_history_frames(
        &aggregator.inner,
        RUNTIME,
        IDENTITY,
        &session.to_string(),
        &mut observed,
        &messages,
        None,
    )
    .await?;
    let restored = runtime_notice_snapshot_frame(
        RUNTIME,
        IDENTITY,
        &session.to_string(),
        &observed,
        &BTreeSet::new(),
        &messages,
    )
    .ok_or("restoration snapshot")?;
    assert_eq!(
        restored.payload["history_positions"],
        json!([{ "frame_id": original.id, "source_cursor": format!("{session}:1") }]),
        "a complete map explicitly restores the original coordinate"
    );
    assert!(restored.payload.get("removed_history_frame_ids").is_none());
    assert_eq!(
        aggregator
            .store()
            .frame_by_dedupe_key(&original.dedupe_key)
            .await?
            .ok_or("immutable frame")?,
        original
    );
    Ok(())
}

#[tokio::test]
async fn notice_scan_reuses_complete_frontier_and_only_reads_new_rows() -> ConsoleLogResult<()> {
    let session = SessionId::new();
    let store = Arc::new(ObservedStore::new(Vec::new(), None));
    let aggregator = MetaConsoleAggregator::new(store.clone());
    for index in 0..2_005 {
        store
            .append_if_absent(frame(
                &format!("noise-{index}"),
                &session,
                "text_delta",
                json!({"delta":"."}),
            ))
            .await?;
    }
    let first = observe(&aggregator, &session).await?;
    let initial_queries = store.query_count.load(Ordering::SeqCst);
    assert_eq!(initial_queries, 3);
    assert_eq!(
        observe(&aggregator, &session).await?.observed_through,
        first.observed_through
    );
    assert_eq!(
        store.query_count.load(Ordering::SeqCst),
        initial_queries,
        "unchanged frontier must not deserialize old identity frames again"
    );
    store
        .append_if_absent(applied("new-application", &session, 10, 20))
        .await?;
    assert_eq!(
        observe(&aggregator, &session).await?.attempts,
        BTreeSet::from([attempt(10, 20)])
    );
    assert_eq!(
        store.query_count.load(Ordering::SeqCst),
        initial_queries + 1,
        "new frontier must scan only the appended suffix"
    );
    aggregator.clear_timeline_frames().await?;
    assert!(
        observe(&aggregator, &session).await?.attempts.is_empty(),
        "clearing the store must invalidate cached observations"
    );
    Ok(())
}

#[tokio::test]
async fn notice_snapshot_shrinking_history_moves_retained_tool_notice_and_answer_together()
-> ConsoleLogResult<()> {
    let aggregator = MetaConsoleAggregator::in_memory();
    let session = SessionId::new();
    let call: Message = serde_json::from_value(json!({
        "role": "block_assistant",
        "blocks": [{ "block_type": "tool_use", "data": {
            "id": "retained-tool", "name": "lookup_delivery", "args": { "request": "plan" },
        }}],
        "identity": {
            "interaction_id": uuid::Uuid::from_u128(50),
            "run_id": uuid::Uuid::from_u128(10),
        },
        "stop_reason": "tool_use",
        "created_at": "1970-01-01T00:00:00.100Z",
    }))?;
    let result: Message = serde_json::from_value(json!({
        "role": "tool_results",
        "results": [{ "tool_use_id": "retained-tool", "content": "Delivery is ready.", "is_error": false }],
        "created_at": "1970-01-01T00:00:00.200Z",
    }))?;
    let notice = Message::SystemNotice(notice(&session, 10, 20));
    let answer = assistant_message("The delivery plan is ready.")?;
    let tail = [call, result, notice, answer];
    let mut retained = Vec::new();
    for (index, message) in tail.iter().enumerate() {
        retained
            .extend(append_canonical_message(&aggregator, &session, index + 10, message).await?);
    }
    assert_eq!(
        retained.len(),
        4,
        "fixture must exercise actual call/result/notice/answer projections"
    );
    let original_frames = retained.clone();
    let mut current = vec![Message::SystemNotice(SystemNoticeMessage::new(
        SystemNoticeKind::Generic,
        "Compacted context.",
    ))];
    current.extend(tail);
    let observed = observe(&aggregator, &session).await?;
    let snapshot = runtime_notice_snapshot_frame(
        RUNTIME,
        IDENTITY,
        &session.to_string(),
        &observed,
        &BTreeSet::new(),
        &current,
    )
    .ok_or("shrinking image snapshot missing")?;
    let positions = snapshot.payload["history_positions"]
        .as_array()
        .ok_or("positions missing")?;
    assert_eq!(positions.len(), 3);
    let mut current_order = Vec::new();
    for (row, old_offset, new_offset) in [
        (&retained[0], 10, 1),
        (&retained[1], 11, 2),
        (&retained[3], 13, 4),
    ] {
        assert!(
            row.source
                .source_cursor
                .as_deref()
                .is_some_and(|cursor| cursor.starts_with(&format!("{session}:{old_offset}")))
        );
        let position = positions
            .iter()
            .find(|position| position["frame_id"] == row.id)
            .ok_or("retained row lost its exact position mapping")?;
        let cursor = position["source_cursor"]
            .as_str()
            .ok_or("mapped cursor missing")?;
        let expected_suffix = if row.kind == "tool_call_requested" {
            ":tool-call:0"
        } else if row.kind == "tool_execution_completed" {
            ":0"
        } else {
            ""
        };
        assert_eq!(cursor, format!("{session}:{new_offset}{expected_suffix}"));
        current_order.push((new_offset, row.kind.as_str()));
    }
    assert_eq!(snapshot.payload["notices"][0]["offset"], 3);
    assert_eq!(
        snapshot.payload["notices"][0]["message"],
        retained[2].payload["message"]
    );
    current_order.push((3, "system_notice"));
    current_order.sort_unstable();
    assert_eq!(
        current_order,
        vec![
            (1, "tool_call_requested"),
            (2, "tool_execution_completed"),
            (3, "system_notice"),
            (4, "text_complete")
        ]
    );
    let saved_snapshot = aggregator.store().append_if_absent(snapshot).await?.frame;
    let rows = aggregator
        .store()
        .query_frames(ConsoleTimelineQuery {
            limit: 20,
            ..Default::default()
        })
        .await?;
    assert_eq!(
        &rows.frames[..4],
        original_frames.as_slice(),
        "position observations do not rewrite canonical rows"
    );
    let rescanned = observe(&aggregator, &session).await?;
    assert_eq!(
        rescanned.previous_snapshot.as_ref().map(|row| &row.id),
        Some(&saved_snapshot.id)
    );
    assert!(
        runtime_notice_snapshot_frame(
            RUNTIME,
            IDENTITY,
            &session.to_string(),
            &rescanned,
            &BTreeSet::new(),
            &current,
        )
        .is_none(),
        "an unchanged compacted image should coalesce after reconnect rescan"
    );
    Ok(())
}

#[tokio::test]
async fn notice_history_positions_pair_repeated_exact_occurrences_without_borrowing_lineage()
-> ConsoleLogResult<()> {
    let aggregator = MetaConsoleAggregator::in_memory();
    let session = SessionId::new();
    let message = assistant_message("Repeated authored answer.")?;
    let mut originals = Vec::new();
    // Arrival order deliberately disagrees with canonical order.
    for offset in [30, 10, 20] {
        originals.extend(append_canonical_message(&aggregator, &session, offset, &message).await?);
    }
    assert_eq!(originals.len(), 3);
    for index in 0..4 {
        let mut conflicting = frames_from_session_history_message_with_namespace(
            RUNTIME,
            IDENTITY,
            "",
            &session.to_string(),
            index,
            serde_json::to_value(&message)?,
        )
        .into_iter()
        .next()
        .ok_or("assistant fixture did not project")?;
        conflicting.dedupe_key = format!("conflict-{index}");
        match index {
            0 => conflicting.run_id = Some(uuid::Uuid::from_u128(11).to_string()),
            1 => conflicting.interaction_id = Some(uuid::Uuid::from_u128(51).to_string()),
            2 => conflicting.kind = "user_input".to_string(),
            _ => conflicting.payload["text"] = json!("Different canonical text."),
        }
        aggregator.store().append_if_absent(conflicting).await?;
    }
    let filler = Message::SystemNotice(SystemNoticeMessage::new(
        SystemNoticeKind::Generic,
        "Context.",
    ));
    let current = [filler.clone(), message.clone(), filler, message];
    let observed = observe(&aggregator, &session).await?;
    let positions =
        current_history_positions(RUNTIME, IDENTITY, &session.to_string(), &observed, &current);
    assert_eq!(
        positions,
        vec![
            json!({ "frame_id": originals[1].id, "source_cursor": format!("{session}:1") }),
            json!({ "frame_id": originals[2].id, "source_cursor": format!("{session}:3") }),
        ],
        "only the two exact occurrences can claim current coordinates"
    );
    Ok(())
}

#[test]
fn notice_history_refresh_avoids_forcing_reads_for_stream_bursts_but_keeps_owner_boundaries() {
    let session = SessionId::new();
    let mut forced = 0;
    for index in 0..2_000 {
        let kind = match index % 4 {
            0 => "text_delta",
            1 => "reasoning_delta",
            2 => "tool_input_delta",
            _ => "tool_execution_started",
        };
        let mut streamed = frame(
            &format!("stream-{index}"),
            &session,
            kind,
            json!({ "delta": "streamed content", "source_sequence": index }),
        );
        streamed.source.source_cursor = Some(format!("runtime:{index}"));
        streamed.run_id = Some(uuid::Uuid::from_u128(10).to_string());
        forced += usize::from(console_event_should_refresh_session_history(&streamed));
    }
    assert_eq!(
        forced, 0,
        "the real source session must not force a full history read per delta"
    );
    for kind in [
        "interaction_complete",
        "interaction_failed",
        "message_delivery_failed",
        "run_completed",
        "run_failed",
        "run_cancelled",
        "boundary_append_applied",
        "boundary_appends_discarded",
        "transcript_rewrite_committed",
        "transcript_rewrite_audit_receipt_committed",
    ] {
        assert!(
            console_event_should_refresh_session_history(&frame(kind, &session, kind, json!({}))),
            "{kind} must retain its authoritative refresh trigger"
        );
    }
}

#[tokio::test]
async fn ordinary_notice_reanchors_before_durable_notice_and_answer_after_shrink()
-> ConsoleLogResult<()> {
    let aggregator = MetaConsoleAggregator::in_memory();
    let session = SessionId::new();
    let ordinary = Message::SystemNotice(SystemNoticeMessage::new(
        SystemNoticeKind::Generic,
        "Keep the operator's constraints in view.",
    ));
    let durable = Message::SystemNotice(notice(&session, 10, 20));
    let answer = assistant_message("Answer following both notices.")?;
    let mut retained = Vec::new();
    for (index, message) in [&ordinary, &durable, &answer].into_iter().enumerate() {
        retained
            .extend(append_canonical_message(&aggregator, &session, index + 10, message).await?);
    }
    assert_eq!(retained.len(), 3);
    let current = [
        Message::SystemNotice(SystemNoticeMessage::new(
            SystemNoticeKind::Generic,
            "Compacted context.",
        )),
        ordinary,
        durable,
        answer,
    ];
    let observed = observe(&aggregator, &session).await?;
    let snapshot = runtime_notice_snapshot_frame(
        RUNTIME,
        IDENTITY,
        &session.to_string(),
        &observed,
        &BTreeSet::new(),
        &current,
    )
    .ok_or("current notice image missing")?;
    assert_eq!(
        snapshot.payload["history_positions"],
        json!([
            { "frame_id": retained[0].id, "source_cursor": format!("{session}:1") },
            { "frame_id": retained[2].id, "source_cursor": format!("{session}:3") },
        ])
    );
    assert_eq!(
        snapshot.payload["notices"]
            .as_array()
            .ok_or("notice rows missing")?
            .len(),
        1
    );
    assert_eq!(snapshot.payload["notices"][0]["offset"], 2);
    assert_eq!(
        snapshot.payload["notices"][0]["message"],
        retained[1].payload["message"]
    );
    assert!(
        retained[0].payload["message"]
            .get("runtime_origin")
            .is_none(),
        "the legacy notice is reanchored without manufacturing a runtime origin"
    );
    Ok(())
}

#[tokio::test]
async fn history_position_occurrences_order_tool_result_subindices_numerically()
-> ConsoleLogResult<()> {
    let aggregator = MetaConsoleAggregator::in_memory();
    let session = SessionId::new();
    let repeated = json!({ "tool_use_id": "reused-result", "content": "Same exact result.", "is_error": false });
    let old_results: Vec<_> = (0..11).map(|index| {
        if [2, 10].contains(&index) { repeated.clone() }
        else { json!({ "tool_use_id": format!("other-{index}"), "content": "Other result.", "is_error": false }) }
    }).collect();
    let old: Message = serde_json::from_value(json!({
        "role": "tool_results", "results": old_results, "created_at": "1970-01-01T00:00:00.100Z",
    }))?;
    let projected = frames_from_session_history_message_with_namespace(
        RUNTIME,
        IDENTITY,
        "",
        &session.to_string(),
        10,
        serde_json::to_value(old)?,
    );
    assert_eq!(projected.len(), 11);
    let mut selected = Vec::new();
    // A string sort and arrival-order sort would both choose index 10 first.
    for index in [10, 2] {
        selected.push(
            aggregator
                .store()
                .append_if_absent(projected[index].clone())
                .await?
                .frame,
        );
    }
    let current_results: Message = serde_json::from_value(json!({
        "role": "tool_results", "results": [repeated.clone(), repeated],
        "created_at": "1970-01-01T00:00:00.100Z",
    }))?;
    let current = [
        Message::SystemNotice(SystemNoticeMessage::new(
            SystemNoticeKind::Generic,
            "Context.",
        )),
        current_results,
    ];
    let observed = observe(&aggregator, &session).await?;
    let positions =
        current_history_positions(RUNTIME, IDENTITY, &session.to_string(), &observed, &current);
    assert_eq!(
        positions,
        vec![
            json!({ "frame_id": selected[1].id, "source_cursor": format!("{session}:1:0") }),
            json!({ "frame_id": selected[0].id, "source_cursor": format!("{session}:1:1") }),
        ]
    );
    Ok(())
}

#[tokio::test]
async fn oversized_history_bodies_keep_the_cached_notice_frontier() -> ConsoleLogResult<()> {
    let session = SessionId::new();
    let store = Arc::new(ObservedStore::new(Vec::new(), None));
    let aggregator = MetaConsoleAggregator::new(store.clone());
    // Exceed the old payload cache cap with real serialized history bodies.
    for offset in 0..10 {
        append_canonical_message(
            &aggregator,
            &session,
            offset,
            &assistant_message(&"large exact answer ".repeat(64 * 1024))?,
        )
        .await?;
    }
    let initial = observe(&aggregator, &session).await?;
    let queries = store.query_count.load(Ordering::SeqCst);
    assert_eq!(initial.canonical_frames.len(), 10);
    assert!(initial.retained_bytes < NOTICE_OBSERVATION_MAX_BYTES);
    assert_eq!(
        observe(&aggregator, &session).await?.observed_through,
        initial.observed_through
    );
    assert_eq!(
        store.query_count.load(Ordering::SeqCst),
        queries,
        "large bodies must not permanently evict an unchanged scan frontier"
    );
    store
        .append_if_absent(applied("suffix-notice", &session, 10, 20))
        .await?;
    assert_eq!(
        observe(&aggregator, &session).await?.attempts,
        BTreeSet::from([attempt(10, 20)])
    );
    assert_eq!(store.query_count.load(Ordering::SeqCst), queries + 1);
    Ok(())
}

#[tokio::test]
async fn successive_compactions_publish_survivors_without_cumulative_removals()
-> ConsoleLogResult<()> {
    let aggregator = MetaConsoleAggregator::in_memory();
    let session = SessionId::new();
    let durable = Message::SystemNotice(notice(&session, 10, 20));
    let a = assistant_message("First canonical answer.")?;
    let b = assistant_message("Second canonical answer.")?;
    let first = append_canonical_message(&aggregator, &session, 1, &a)
        .await?
        .remove(0);
    let second = append_canonical_message(&aggregator, &session, 2, &b)
        .await?
        .remove(0);
    let mut observed = observe(&aggregator, &session).await?;
    let snapshot = runtime_notice_snapshot_frame(
        RUNTIME,
        IDENTITY,
        &session.to_string(),
        &observed,
        &BTreeSet::new(),
        &[durable.clone(), b.clone()],
    )
    .ok_or("first compaction")?;
    assert!(snapshot.payload.get("history_positions_mode").is_none());
    assert!(snapshot.payload.get("removed_history_frame_ids").is_none());
    assert_eq!(
        snapshot.payload["history_positions"],
        json!([
            { "frame_id": second.id, "source_cursor": format!("{session}:1") }
        ])
    );
    aggregator.store().append_if_absent(snapshot).await?;
    observed = observe(&aggregator, &session).await?;
    assert_eq!(
        observed.canonical_frames.len(),
        1,
        "removed descriptors must leave the cache"
    );
    let final_image = [durable.clone()];
    let snapshot = runtime_notice_snapshot_frame(
        RUNTIME,
        IDENTITY,
        &session.to_string(),
        &observed,
        &BTreeSet::new(),
        &final_image,
    )
    .ok_or("second compaction")?;
    assert_eq!(snapshot.payload["history_positions"], json!([]));
    assert!(snapshot.payload.get("removed_history_frame_ids").is_none());
    assert!(!serde_json::to_string(&snapshot.payload)?.contains(&first.id));
    aggregator.store().append_if_absent(snapshot).await?;
    observed = observe(&aggregator, &session).await?;
    assert!(observed.canonical_frames.is_empty());
    assert!(
        runtime_notice_snapshot_frame(
            RUNTIME,
            IDENTITY,
            &session.to_string(),
            &observed,
            &BTreeSet::new(),
            &final_image
        )
        .is_none()
    );
    // Restoring an original source coordinate must recover its durable id even
    // after its compact cache descriptor was removed.
    let restored = [durable, a, b];
    restore_current_history_frames(
        &aggregator.inner,
        RUNTIME,
        IDENTITY,
        &session.to_string(),
        &mut observed,
        &restored,
        None,
    )
    .await?;
    let snapshot = runtime_notice_snapshot_frame(
        RUNTIME,
        IDENTITY,
        &session.to_string(),
        &observed,
        &BTreeSet::new(),
        &restored,
    )
    .ok_or("restored image")?;
    assert_eq!(
        snapshot.payload["history_positions"],
        json!([
            { "frame_id": first.id, "source_cursor": format!("{session}:1") },
            { "frame_id": second.id, "source_cursor": format!("{session}:2") }
        ])
    );
    Ok(())
}

struct RedactNoticeAndHistory;
impl ConsoleVisibilityPolicy for RedactNoticeAndHistory {
    fn redact_payload(&self, row: &NewConsoleFrame) -> Option<Value> {
        (row.source.kind == ConsoleFrameSourceKind::SessionHistory).then(|| {
            let mut payload = row.payload.clone();
            if row.kind == "runtime_notice_snapshot" {
                if let Some(notices) = payload["notices"].as_array_mut() {
                    for notice in notices {
                        notice["message"]["body"] = json!("[host redacted]");
                    }
                }
            } else {
                payload = json!({ "text": "[host redacted]" });
            }
            payload
        })
    }
}

#[tokio::test]
async fn stored_redacted_snapshots_coalesce_after_reopen_and_append_after_compaction()
-> ConsoleLogResult<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("redacted-notices.sqlite");
    let session = SessionId::new();
    let durable = Message::SystemNotice(notice(&session, 10, 20));
    let answer = assistant_message("Private retained answer.")?;
    let policy: Arc<dyn ConsoleVisibilityPolicy> = Arc::new(RedactNoticeAndHistory);
    {
        let aggregator = MetaConsoleAggregator::new(Arc::new(SqliteConsoleLogStore::open(&path)?));
        for row in frames_from_session_history_message_with_namespace(
            RUNTIME,
            IDENTITY,
            "",
            &session.to_string(),
            10,
            serde_json::to_value(&answer)?,
        ) {
            append_and_emit_with_policy(&aggregator.inner, row, policy.clone()).await?;
        }
        let observed = observe(&aggregator, &session).await?;
        let snapshot = runtime_notice_snapshot_frame(
            RUNTIME,
            IDENTITY,
            &session.to_string(),
            &observed,
            &BTreeSet::new(),
            &[durable.clone(), answer.clone()],
        )
        .ok_or("compacted snapshot")?;
        assert_eq!(
            snapshot.payload["history_positions"]
                .as_array()
                .ok_or("positions")?
                .len(),
            1,
            "pre-policy canonical witness must match a moved redacted row"
        );
        let stored = append_and_emit_with_policy(&aggregator.inner, snapshot, policy.clone())
            .await?
            .frame;
        assert_eq!(
            stored.payload["notices"][0]["message"]["body"],
            "[host redacted]"
        );
        assert!(!serde_json::to_string(&stored)?.contains("Exact boundary notice."));
        assert!(!serde_json::to_string(&stored)?.contains("Private retained answer."));
    }
    let aggregator = MetaConsoleAggregator::new(Arc::new(SqliteConsoleLogStore::open(&path)?));
    let mut messages = vec![durable, answer];
    for index in 0..16 {
        let observed = observe(&aggregator, &session).await?;
        assert!(
            runtime_notice_snapshot_frame(
                RUNTIME,
                IDENTITY,
                &session.to_string(),
                &observed,
                &BTreeSet::new(),
                &messages
            )
            .is_none(),
            "stored redaction and appended history must not republish a full image"
        );
        let added = assistant_message(&format!("Appended private answer {index}."))?;
        for row in frames_from_session_history_message_with_namespace(
            RUNTIME,
            IDENTITY,
            "",
            &session.to_string(),
            messages.len(),
            serde_json::to_value(&added)?,
        ) {
            append_and_emit_with_policy(&aggregator.inner, row, policy.clone()).await?;
        }
        messages.push(added);
        aggregator
            .store()
            .append_if_absent(frame(
                &format!("terminal-{index}"),
                &session,
                "run_completed",
                json!({"run_id":uuid::Uuid::from_u128(10)}),
            ))
            .await?;
    }
    let rows = aggregator
        .store()
        .query_frames(ConsoleTimelineQuery {
            limit: 100,
            ..Default::default()
        })
        .await?;
    assert_eq!(
        rows.frames
            .iter()
            .filter(|row| row.kind == "runtime_notice_snapshot")
            .count(),
        1
    );
    assert!(!serde_json::to_string(&rows.frames)?.contains("Private retained answer."));
    Ok(())
}

#[tokio::test]
async fn compacted_payload_history_reclaims_cache_bytes_and_keeps_suffix_scan()
-> ConsoleLogResult<()> {
    let session = SessionId::new();
    let store = Arc::new(ObservedStore::new(Vec::new(), None));
    let aggregator = MetaConsoleAggregator::new(store.clone());
    let mut messages = vec![Message::SystemNotice(notice(&session, 10, 20))];
    for index in 0..12 {
        let message =
            assistant_message(&format!("{index}:{}", "private content ".repeat(64 * 1024)))?;
        append_canonical_message(&aggregator, &session, messages.len(), &message).await?;
        messages.push(message);
    }
    let observed = observe(&aggregator, &session).await?;
    let before_bytes = observed.retained_bytes;
    let current = vec![
        messages[0].clone(),
        messages.pop().ok_or("retained answer")?,
    ];
    let snapshot = runtime_notice_snapshot_frame(
        RUNTIME,
        IDENTITY,
        &session.to_string(),
        &observed,
        &BTreeSet::new(),
        &current,
    )
    .ok_or("compaction snapshot")?;
    aggregator.store().append_if_absent(snapshot).await?;
    let observed = observe(&aggregator, &session).await?;
    assert_eq!(observed.canonical_frames.len(), 1);
    assert!(
        observed.retained_bytes < before_bytes / 4,
        "removed history descriptors must stop consuming the retained-byte budget"
    );
    let queries = store.query_count.load(Ordering::SeqCst);
    assert!(
        runtime_notice_snapshot_frame(
            RUNTIME,
            IDENTITY,
            &session.to_string(),
            &observe(&aggregator, &session).await?,
            &BTreeSet::new(),
            &current
        )
        .is_none()
    );
    assert_eq!(store.query_count.load(Ordering::SeqCst), queries);
    store
        .append_if_absent(applied("post-compaction-notice", &session, 10, 21))
        .await?;
    assert!(
        observe(&aggregator, &session)
            .await?
            .attempts
            .contains(&attempt(10, 21))
    );
    assert_eq!(store.query_count.load(Ordering::SeqCst), queries + 1);
    Ok(())
}

#[tokio::test]
async fn whole_payload_redaction_preserves_snapshot_coalescing_and_changed_notice_detection()
-> ConsoleLogResult<()> {
    struct HidePayload;
    impl ConsoleVisibilityPolicy for HidePayload {
        fn redact_payload(&self, _: &NewConsoleFrame) -> Option<Value> {
            Some(json!({ "redacted": true }))
        }
    }
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("whole-payload-redaction.sqlite");
    let session = SessionId::new();
    let original = Message::SystemNotice(notice(&session, 10, 20));
    {
        let aggregator = MetaConsoleAggregator::new(Arc::new(SqliteConsoleLogStore::open(&path)?));
        let snapshot = runtime_notice_snapshot_frame(
            RUNTIME,
            IDENTITY,
            &session.to_string(),
            &observe(&aggregator, &session).await?,
            &BTreeSet::new(),
            std::slice::from_ref(&original),
        )
        .ok_or("initial snapshot")?;
        let stored =
            append_and_emit_with_policy(&aggregator.inner, snapshot, Arc::new(HidePayload))
                .await?
                .frame;
        assert_eq!(stored.payload, json!({ "redacted": true }));
        assert!(!serde_json::to_string(&stored)?.contains("Exact boundary notice."));
    }
    let aggregator = MetaConsoleAggregator::new(Arc::new(SqliteConsoleLogStore::open(&path)?));
    let observation = observe(&aggregator, &session).await?;
    assert!(
        runtime_notice_snapshot_frame(
            RUNTIME,
            IDENTITY,
            &session.to_string(),
            &observation,
            &BTreeSet::new(),
            std::slice::from_ref(&original)
        )
        .is_none()
    );
    let mut changed = notice(&session, 10, 20);
    changed.body = Some("Changed private body, same exact origin.".into());
    assert!(
        runtime_notice_snapshot_frame(
            RUNTIME,
            IDENTITY,
            &session.to_string(),
            &observation,
            &BTreeSet::new(),
            &[Message::SystemNotice(changed)]
        )
        .is_some(),
        "redaction may hide text but must not hide a canonical image change from the producer"
    );
    Ok(())
}

#[tokio::test]
async fn pruned_history_restoration_recovers_original_id_at_a_new_coordinate()
-> ConsoleLogResult<()> {
    let aggregator = MetaConsoleAggregator::in_memory();
    let session = SessionId::new();
    let durable = Message::SystemNotice(notice(&session, 10, 20));
    let answer = assistant_message("Restore this exact canonical occurrence.")?;
    let original = append_canonical_message(&aggregator, &session, 10, &answer)
        .await?
        .remove(0);
    let observation = observe(&aggregator, &session).await?;
    let snapshot = runtime_notice_snapshot_frame(
        RUNTIME,
        IDENTITY,
        &session.to_string(),
        &observation,
        &BTreeSet::new(),
        std::slice::from_ref(&durable),
    )
    .ok_or("removal snapshot")?;
    aggregator.store().append_if_absent(snapshot).await?;
    let mut observation = observe(&aggregator, &session).await?;
    assert!(observation.canonical_frames.is_empty());
    let current = [durable, answer];
    restore_current_history_frames(
        &aggregator.inner,
        RUNTIME,
        IDENTITY,
        &session.to_string(),
        &mut observation,
        &current,
        None,
    )
    .await?;
    let restored = runtime_notice_snapshot_frame(
        RUNTIME,
        IDENTITY,
        &session.to_string(),
        &observation,
        &BTreeSet::new(),
        &current,
    )
    .ok_or("moved restoration")?;
    assert_eq!(
        restored.payload["history_positions"],
        json!([
            { "frame_id": original.id, "source_cursor": format!("{session}:1") }
        ])
    );
    Ok(())
}

#[tokio::test]
async fn oversized_descriptor_cache_keeps_frontier_and_exact_attempts() -> ConsoleLogResult<()> {
    let session = SessionId::new();
    let store = Arc::new(ObservedStore::new(Vec::new(), None));
    let aggregator = MetaConsoleAggregator::new(store.clone());
    store
        .append_if_absent(applied("anchor", &session, 10, 20))
        .await?;
    let mut observation = observe(&aggregator, &session).await?;
    let messages = vec![Message::SystemNotice(notice(&session, 10, 20))];
    observation.verified_history_digest = Some(history_image_digest(&messages));
    observation.verified_relevant_frontier = observation.relevant_frontier;
    // Exercise the actual byte-cap branch without thousands of fixtures.
    observation.retained_bytes = NOTICE_OBSERVATION_MAX_BYTES + 1;
    cache_notice_observation(
        &aggregator.inner,
        (RUNTIME.into(), IDENTITY.into(), session.to_string()),
        &observation,
    );
    let queries = store.query_count.load(Ordering::SeqCst);
    let cached = observe(&aggregator, &session).await?;
    assert_eq!(cached.observed_through, observation.observed_through);
    assert_eq!(store.query_count.load(Ordering::SeqCst), queries);
    assert!(!cached.history_index_complete);
    assert!(cached.canonical_frames.is_empty());
    assert_eq!(
        cached.attempts,
        BTreeSet::from([attempt(10, 20)]),
        "settlement lookup must retain exact attempts"
    );
    assert!(observation_verifies_history(&cached, &messages));
    store
        .append_if_absent(applied("new-after-cap", &session, 11, 21))
        .await?;
    let changed = observe(&aggregator, &session).await?;
    assert_eq!(store.query_count.load(Ordering::SeqCst), queries + 1);
    assert!(
        !observation_verifies_history(&changed, &messages),
        "new notice requires complete reconstruction"
    );
    let rebuilt = observe_runtime_notice_attempts_with_cache(
        &aggregator.inner,
        RUNTIME,
        IDENTITY,
        &session.to_string(),
        false,
    )
    .await?;
    assert!(rebuilt.history_index_complete);
    assert_eq!(
        rebuilt.attempts,
        BTreeSet::from([attempt(10, 20), attempt(11, 21)])
    );
    Ok(())
}

#[tokio::test]
async fn restored_repeated_rows_keep_numeric_source_order_after_pruning() -> ConsoleLogResult<()> {
    let aggregator = MetaConsoleAggregator::in_memory();
    let session = SessionId::new();
    let durable = Message::SystemNotice(notice(&session, 10, 20));
    let answer = assistant_message("Repeated exact canonical answer.")?;
    let mut original = Vec::new();
    for offset in [30, 10, 20] {
        original.push(
            append_canonical_message(&aggregator, &session, offset, &answer)
                .await?
                .remove(0),
        );
    }
    let observed = observe(&aggregator, &session).await?;
    let snapshot = runtime_notice_snapshot_frame(
        RUNTIME,
        IDENTITY,
        &session.to_string(),
        &observed,
        &BTreeSet::new(),
        std::slice::from_ref(&durable),
    )
    .ok_or("removal")?;
    aggregator.store().append_if_absent(snapshot).await?;
    let mut observed = observe(&aggregator, &session).await?;
    assert!(observed.canonical_frames.is_empty());
    let current = [durable.clone(), answer.clone(), durable, answer];
    restore_current_history_frames(
        &aggregator.inner,
        RUNTIME,
        IDENTITY,
        &session.to_string(),
        &mut observed,
        &current,
        None,
    )
    .await?;
    let positions =
        current_history_positions(RUNTIME, IDENTITY, &session.to_string(), &observed, &current);
    assert_eq!(
        positions,
        vec![
            json!({"frame_id":original[1].id,"source_cursor":format!("{session}:1")}),
            json!({"frame_id":original[2].id,"source_cursor":format!("{session}:3")}),
        ]
    );
    assert_eq!(
        observed.canonical_frames.len(),
        2,
        "recovery retains only needed occurrences"
    );
    Ok(())
}

#[tokio::test]
async fn overflow_frontier_invalidates_when_a_known_attempt_settles() -> ConsoleLogResult<()> {
    let session = SessionId::new();
    let aggregator = MetaConsoleAggregator::in_memory();
    aggregator
        .store()
        .append_if_absent(applied("pending", &session, 10, 20))
        .await?;
    let mut observation = observe(&aggregator, &session).await?;
    observation.verified_history_digest = Some(history_image_digest(&[]));
    observation.verified_relevant_frontier = observation.relevant_frontier;
    observation.verified_settled_digest = Some(settled_attempts_digest(&BTreeSet::new()));
    observation.retained_bytes = NOTICE_OBSERVATION_MAX_BYTES + 1;
    cache_notice_observation(
        &aggregator.inner,
        (RUNTIME.into(), IDENTITY.into(), session.to_string()),
        &observation,
    );
    let before = observe(&aggregator, &session).await?;
    assert!(observation_verifies_history(&before, &[]));
    assert!(reuse_verified_history_image(&before, &[], &BTreeSet::new()));
    let settled = BTreeSet::from([(
        uuid::Uuid::from_u128(10).to_string(),
        uuid::Uuid::from_u128(20).to_string(),
    )]);
    assert!(
        !reuse_verified_history_image(&before, &[], &settled),
        "the actual production reuse gate must reject changed settlement with unchanged history"
    );
    aggregator
        .store()
        .append_if_absent(frame("terminal", &session, "run_completed", json!({})))
        .await?;
    let after = observe(&aggregator, &session).await?;
    assert!(
        observation_verifies_history(&after, &[]),
        "ordinary terminal rows must not invalidate unchanged notice images"
    );
    assert_eq!(after.attempts, BTreeSet::from([attempt(10, 20)]));
    assert!(reuse_verified_history_image(&after, &[], &BTreeSet::new()));
    assert!(!reuse_verified_history_image(&after, &[], &settled));
    Ok(())
}

#[tokio::test]
async fn restored_tool_result_occurrences_survive_changed_numeric_subindices()
-> ConsoleLogResult<()> {
    let aggregator = MetaConsoleAggregator::in_memory();
    let session = SessionId::new();
    let repeated = json!({"tool_use_id":"same-tool","content":"Exact result.","is_error":false});
    let results: Vec<_> = (0..11).map(|index| if [2,10].contains(&index) { repeated.clone() }
        else { json!({"tool_use_id":format!("other-{index}"),"content":"Other result.","is_error":false}) }).collect();
    let old: Message = serde_json::from_value(json!({"role":"tool_results","results":results,
        "created_at":"1970-01-01T00:00:00.100Z"}))?;
    let projected = frames_from_session_history_message_with_namespace(
        RUNTIME,
        IDENTITY,
        "",
        &session.to_string(),
        10,
        serde_json::to_value(old)?,
    );
    let mut originals = Vec::new();
    for index in [10, 2] {
        originals.push(
            aggregator
                .store()
                .append_if_absent(projected[index].clone())
                .await?
                .frame,
        );
    }
    let durable = Message::SystemNotice(notice(&session, 10, 20));
    let observation = observe(&aggregator, &session).await?;
    let snapshot = runtime_notice_snapshot_frame(
        RUNTIME,
        IDENTITY,
        &session.to_string(),
        &observation,
        &BTreeSet::new(),
        std::slice::from_ref(&durable),
    )
    .ok_or("removed results")?;
    aggregator.store().append_if_absent(snapshot).await?;
    let mut observation = observe(&aggregator, &session).await?;
    let rewritten: Message = serde_json::from_value(json!({"role":"tool_results",
        "results":[repeated.clone(),repeated],"created_at":"1970-01-01T00:00:00.100Z"}))?;
    let current = [durable, rewritten];
    restore_current_history_frames(
        &aggregator.inner,
        RUNTIME,
        IDENTITY,
        &session.to_string(),
        &mut observation,
        &current,
        None,
    )
    .await?;
    assert_eq!(
        current_history_positions(
            RUNTIME,
            IDENTITY,
            &session.to_string(),
            &observation,
            &current
        ),
        vec![
            json!({"frame_id":originals[1].id,"source_cursor":format!("{session}:1:0")}),
            json!({"frame_id":originals[0].id,"source_cursor":format!("{session}:1:1")}),
        ]
    );
    Ok(())
}

#[test]
fn typed_compaction_summary_never_projects_as_a_human_history_row() -> ConsoleLogResult<()> {
    let session = SessionId::new();
    let summary = Message::User(meerkat_core::types::UserMessage::compaction_summary(
        "Retained exact context.",
    ));
    let ordinary = Message::User(meerkat_core::types::UserMessage::text(
        "Retained exact context.",
    ));
    assert!(
        frames_from_session_history_message(
            RUNTIME,
            IDENTITY,
            &session.to_string(),
            0,
            serde_json::to_value(summary)?
        )
        .is_empty()
    );
    let conversational =
        current_history_projection(RUNTIME, IDENTITY, &session.to_string(), &[ordinary]);
    assert_eq!(
        conversational
            .first()
            .ok_or("conversational user history row missing")?
            .kind,
        "user_input"
    );
    Ok(())
}

#[tokio::test]
async fn restoration_search_reuses_negative_frontier_and_rechecks_new_rows_and_requests()
-> ConsoleLogResult<()> {
    let session = SessionId::new();
    let store = Arc::new(ObservedStore::new(Vec::new(), None));
    let aggregator = MetaConsoleAggregator::new(store.clone());
    let a = assistant_message("Missing canonical A.")?;
    let b = assistant_message("Previously removed B.")?;
    let original_b = append_canonical_message(&aggregator, &session, 20, &b)
        .await?
        .into_iter()
        .next()
        .ok_or("canonical fixture row missing")?;
    store
        .append_if_absent(applied("search-anchor", &session, 10, 20))
        .await?;
    let mut observed = observe(&aggregator, &session).await?;
    observed.full_history_positions = true;
    observed.canonical_frames.clear();
    observed.retained_bytes = 0;
    let missing = [a.clone()];
    let before = store.query_count.load(Ordering::SeqCst);
    restore_current_history_frames(
        &aggregator.inner,
        RUNTIME,
        IDENTITY,
        &session.to_string(),
        &mut observed,
        &missing,
        None,
    )
    .await?;
    assert_eq!(store.query_count.load(Ordering::SeqCst), before + 1);
    restore_current_history_frames(
        &aggregator.inner,
        RUNTIME,
        IDENTITY,
        &session.to_string(),
        &mut observed,
        &missing,
        None,
    )
    .await?;
    assert_eq!(
        store.query_count.load(Ordering::SeqCst),
        before + 1,
        "same missing row must not rescan the old log"
    );
    let frontier = observed.observed_through;
    let late_a = append_canonical_message(&aggregator, &session, 30, &a)
        .await?
        .into_iter()
        .next()
        .ok_or("canonical fixture row missing")?;
    observed.observed_through = late_a
        .cursor
        .seq()
        .ok_or("stored row missing console sequence")?;
    restore_current_history_frames(
        &aggregator.inner,
        RUNTIME,
        IDENTITY,
        &session.to_string(),
        &mut observed,
        &missing,
        None,
    )
    .await?;
    assert_eq!(
        store
            .query_afters
            .lock()
            .map_err(|_| std::io::Error::other("query cursor log lock poisoned"))?
            .last(),
        Some(&Some(frontier)),
        "later candidates are read from the searched frontier"
    );
    assert!(
        observed
            .canonical_frames
            .iter()
            .any(|row| row.id == late_a.id)
    );
    restore_current_history_frames(
        &aggregator.inner,
        RUNTIME,
        IDENTITY,
        &session.to_string(),
        &mut observed,
        &[a, b],
        None,
    )
    .await?;
    assert_eq!(
        store
            .query_afters
            .lock()
            .map_err(|_| std::io::Error::other("query cursor log lock poisoned"))?
            .last(),
        Some(&None),
        "a different restoration request must search old evidence again"
    );
    assert!(
        observed
            .canonical_frames
            .iter()
            .any(|row| row.id == original_b.id)
    );
    Ok(())
}

#[tokio::test]
async fn failed_restoration_search_does_not_cache_partial_absence() -> ConsoleLogResult<()> {
    let session = SessionId::new();
    let store = Arc::new(ObservedStore::new(Vec::new(), None));
    let aggregator = MetaConsoleAggregator::new(store.clone());
    store
        .append_if_absent(applied("failure-anchor", &session, 10, 20))
        .await?;
    let mut observed = observe(&aggregator, &session).await?;
    observed.full_history_positions = true;
    let current = [assistant_message("Not previously stored.")?];
    let before = store.query_count.load(Ordering::SeqCst);
    store.fail_query.store(before + 1, Ordering::SeqCst);
    assert!(
        restore_current_history_frames(
            &aggregator.inner,
            RUNTIME,
            IDENTITY,
            &session.to_string(),
            &mut observed,
            &current,
            None
        )
        .await
        .is_err()
    );
    restore_current_history_frames(
        &aggregator.inner,
        RUNTIME,
        IDENTITY,
        &session.to_string(),
        &mut observed,
        &current,
        None,
    )
    .await?;
    assert_eq!(
        store.query_count.load(Ordering::SeqCst),
        before + 2,
        "failed search must be retried"
    );
    assert_eq!(
        store
            .query_afters
            .lock()
            .map_err(|_| std::io::Error::other("query cursor log lock poisoned"))?
            .last(),
        Some(&None)
    );
    Ok(())
}

fn backfill_test_page(
    session: &SessionId,
    messages: &[Message],
) -> meerkat_core::service::SessionHistoryPage {
    meerkat_core::service::SessionHistoryPage::from_messages(
        session.clone(),
        messages,
        meerkat_core::service::SessionHistoryQuery::default(),
    )
}

fn script_backfill_pages(
    service: &super::tests::DelayedHistorySessionService,
    pages: impl IntoIterator<Item = Option<meerkat_core::service::SessionHistoryPage>>,
) {
    service.script_history(
        pages
            .into_iter()
            .map(|page| super::tests::ScriptedHistoryRead { page, gate: None }),
    );
}

fn captured_backfill_test_provenance(
    entry: &RuntimeEntry,
    record: &ConsoleIdentityRecord,
) -> ConsoleFrameMemberProvenance {
    // Capture the target's member context so query counts isolate history
    // restoration from the separate legacy provenance recovery path.
    let mob_id = entry.runtime.handle().mob_id().to_string();
    ConsoleFrameMemberProvenance {
        identity: record.clone(),
        member: ConsoleMember {
            agent_identity: record.runtime_member_id.clone(),
            role: "worker".into(),
            state: "running".into(),
            model_capabilities: Default::default(),
            runtime_mode: None,
            session_id: record.session_id.clone(),
            wired_to: record.topology_peers.clone(),
            labels: record.labels.clone(),
            progress: None,
        },
        primary_mob_id: mob_id.clone(),
        source_mob_id: mob_id,
        lineage: Vec::new(),
    }
}

fn force_frontier_only_cache(
    aggregator: &MetaConsoleAggregator,
    session: &SessionId,
) -> ConsoleLogResult<()> {
    let key = (
        RUNTIME.to_string(),
        IDENTITY.to_string(),
        session.to_string(),
    );
    let mut observed = aggregator
        .inner
        .notice_observations
        .lock()
        .map_err(|_| std::io::Error::other("notice observation cache lock poisoned"))?
        .entries
        .get(&key)
        .ok_or("verified observation missing from cache")?
        .clone();
    // Exercise the production cap branch with a scoped cache fixture. No global
    // limit override or several thousand synthetic descriptors are needed.
    observed.retained_bytes = NOTICE_OBSERVATION_MAX_BYTES + 1;
    cache_notice_observation(&aggregator.inner, key, &observed);
    Ok(())
}

#[tokio::test]
async fn production_backfill_reuses_overcap_image_and_rebuilds_changed_image_before_publication()
-> ConsoleLogResult<()> {
    let (_temp, runtime, service) = super::tests::build_stress_runtime(0, Duration::ZERO).await;
    let session = SessionId::new();
    let store = Arc::new(ObservedStore::new(Vec::new(), None));
    let aggregator = MetaConsoleAggregator::new(store.clone());
    let mut entry = super::tests::runtime_entry_for_test(RUNTIME, &runtime);
    entry.identity_namespace.clear();
    let mut record = super::tests::identity_record_for_test(IDENTITY);
    record.runtime_key = RUNTIME.into();
    record.session_id = Some(session.to_string());
    let target = SessionBackfillTarget {
        assistant_refresh: AssistantHistoryRefreshReason::Recovery,
        provenance: Some(captured_backfill_test_provenance(&entry, &record)),
        entry: entry.clone(),
        record,
        session_id: session.to_string(),
    };
    aggregator
        .inner
        .runtimes
        .write()
        .map_err(|_| std::io::Error::other("runtime registration lock poisoned"))?
        .insert(RUNTIME.into(), entry);
    let durable = Message::SystemNotice(notice(&session, 10, 20));
    let a = assistant_message("Restored owner answer.")?;
    let original = append_canonical_message(&aggregator, &session, 10, &a)
        .await?
        .into_iter()
        .next()
        .ok_or("canonical fixture row missing")?;
    append_canonical_message(&aggregator, &session, 11, &durable).await?;
    store
        .record_source_watermark(
            &session_history_watermark_runtime_key(RUNTIME, &session.to_string()),
            ConsoleFrameSourceKind::SessionHistory,
            &format_session_history_watermark(&session.to_string(), 2, 0),
        )
        .await?;
    let summary = Message::User(meerkat_core::types::UserMessage::compaction_summary(
        "Compacted owner context.",
    ));
    let current = [summary.clone(), durable.clone()];
    script_backfill_pages(&service, [Some(backfill_test_page(&session, &current))]);
    backfill_one_session_history(aggregator.inner.clone(), target.clone(), true).await?;
    observe(&aggregator, &session).await?;
    force_frontier_only_cache(&aggregator, &session)?;
    let queries = store.query_count.load(Ordering::SeqCst);
    let reads = service.read_calls();
    let before = store.inner.latest_cursor().await?;
    script_backfill_pages(&service, [Some(backfill_test_page(&session, &current))]);
    backfill_one_session_history(aggregator.inner.clone(), target.clone(), true).await?;
    assert_eq!(
        service.read_calls(),
        reads + 1,
        "verified frontier uses one owner history read"
    );
    assert_eq!(
        store.query_count.load(Ordering::SeqCst),
        queries,
        "verified frontier does not reconstruct the timeline"
    );
    assert_eq!(
        store.inner.latest_cursor().await?,
        before,
        "unchanged image does not republish a snapshot"
    );
    // The real ephemeral runtime reports no durable settlement. Seed an old
    // verified settlement image to ensure its changed owner answer, with the
    // exact same history, forces the production rebuild-and-reread branch.
    {
        let key = (
            RUNTIME.to_string(),
            IDENTITY.to_string(),
            session.to_string(),
        );
        let mut cached = aggregator
            .inner
            .notice_observations
            .lock()
            .map_err(|_| std::io::Error::other("notice observation cache lock poisoned"))?;
        cached
            .entries
            .get_mut(&key)
            .ok_or("verified observation missing from cache")?
            .verified_settled_digest = Some(settled_attempts_digest(&BTreeSet::from([(
            uuid::Uuid::from_u128(10).to_string(),
            uuid::Uuid::from_u128(20).to_string(),
        )])));
    }
    script_backfill_pages(
        &service,
        [
            Some(backfill_test_page(&session, &current)),
            Some(backfill_test_page(&session, &current)),
        ],
    );
    backfill_one_session_history(aggregator.inner.clone(), target.clone(), true).await?;
    assert_eq!(
        service.read_calls(),
        reads + 3,
        "fresh owner settlement invalidates even an unchanged history image"
    );
    assert_eq!(
        store.inner.latest_cursor().await?,
        before,
        "settlement cache invalidation does not manufacture a new snapshot"
    );
    force_frontier_only_cache(&aggregator, &session)?;
    let mut abandoned = notice(&session, 10, 20);
    abandoned.body = Some("First read must never be published.".into());
    let mut final_notice = notice(&session, 10, 20);
    final_notice.body = Some("Fresh owner image after complete reconstruction.".into());
    let final_image = [summary, a, Message::SystemNotice(final_notice)];
    script_backfill_pages(
        &service,
        [
            Some(backfill_test_page(
                &session,
                &[Message::SystemNotice(abandoned.clone())],
            )),
            None,
        ],
    );
    backfill_one_session_history(aggregator.inner.clone(), target.clone(), true).await?;
    let failed_rows = store
        .inner
        .query_frames(ConsoleTimelineQuery {
            limit: 100,
            ..Default::default()
        })
        .await?
        .frames;
    assert_eq!(
        failed_rows
            .iter()
            .filter(|row| row.kind == "runtime_notice_snapshot")
            .count(),
        1,
        "a failed fresh read after rebuilding cannot publish the abandoned image"
    );
    assert!(
        failed_rows
            .iter()
            .any(|row| row.kind == "replay_unavailable")
    );
    force_frontier_only_cache(&aggregator, &session)?;
    script_backfill_pages(
        &service,
        [
            Some(backfill_test_page(
                &session,
                &[Message::SystemNotice(abandoned)],
            )),
            Some(backfill_test_page(&session, &final_image)),
        ],
    );
    backfill_one_session_history(aggregator.inner.clone(), target, true).await?;
    assert_eq!(
        service.read_calls(),
        reads + 7,
        "both failed and successful changed images rebuild then obtain a fresh owner read"
    );
    let rows = store
        .inner
        .query_frames(ConsoleTimelineQuery {
            limit: 100,
            ..Default::default()
        })
        .await?
        .frames;
    let snapshots: Vec<_> = rows
        .iter()
        .filter(|row| row.kind == "runtime_notice_snapshot")
        .collect();
    assert_eq!(snapshots.len(), 2);
    let latest = snapshots.last().ok_or("fresh snapshot missing")?;
    assert_eq!(
        latest.payload["notices"][0]["message"]["body"],
        "Fresh owner image after complete reconstruction."
    );
    assert!(
        latest.payload["history_positions"]
            .as_array()
            .ok_or("snapshot missing full history position array")?
            .iter()
            .any(|position| position["frame_id"] == original.id
                && position["source_cursor"] == format!("{session}:1"))
    );
    assert!(
        rows.iter().any(|row| row.kind == "system_notice"
            && row.payload["message"]["body"]
                == "Fresh owner image after complete reconstruction."
            && row.source.source_cursor.as_deref() == Some(format!("{session}:2").as_str())),
        "ordinary suffix backfill still appends the fresh owner notice"
    );
    assert!(
        !rows.iter().any(|row| row.kind == "user_input"),
        "typed summary is never projected as a human message"
    );
    runtime.mob_handle().stop().await?;
    Ok(())
}

#[tokio::test]
async fn restoration_frontier_invalidates_when_candidate_multiplicity_or_reservations_change()
-> ConsoleLogResult<()> {
    let session = SessionId::new();
    let store = Arc::new(ObservedStore::new(Vec::new(), None));
    let aggregator = MetaConsoleAggregator::new(store.clone());
    let answer = assistant_message("Equal repeated answer.")?;
    let first = append_canonical_message(&aggregator, &session, 10, &answer)
        .await?
        .into_iter()
        .next()
        .ok_or("canonical fixture row missing")?;
    let second = append_canonical_message(&aggregator, &session, 20, &answer)
        .await?
        .into_iter()
        .next()
        .ok_or("canonical fixture row missing")?;
    store
        .append_if_absent(applied("reservation-anchor", &session, 10, 20))
        .await?;
    let mut observed = observe(&aggregator, &session).await?;
    observed.full_history_positions = true;
    let current = [answer.clone(), answer.clone(), answer.clone()];
    restore_current_history_frames(
        &aggregator.inner,
        RUNTIME,
        IDENTITY,
        &session.to_string(),
        &mut observed,
        &current,
        None,
    )
    .await?;
    let searched = store.query_count.load(Ordering::SeqCst);
    restore_current_history_frames(
        &aggregator.inner,
        RUNTIME,
        IDENTITY,
        &session.to_string(),
        &mut observed,
        &current,
        None,
    )
    .await?;
    assert_eq!(store.query_count.load(Ordering::SeqCst), searched);
    observed.canonical_frames.retain(|row| row.id != first.id);
    // Still one unmatched occurrence, but a different reservation set now
    // leaves the original frame eligible within the previously searched prefix.
    restore_current_history_frames(
        &aggregator.inner,
        RUNTIME,
        IDENTITY,
        &session.to_string(),
        &mut observed,
        &[answer.clone(), answer.clone()],
        None,
    )
    .await?;
    assert_eq!(
        store
            .query_afters
            .lock()
            .map_err(|_| std::io::Error::other("query cursor log lock poisoned"))?
            .last(),
        Some(&None)
    );
    assert!(
        observed
            .canonical_frames
            .iter()
            .any(|row| row.id == first.id)
    );
    assert!(
        observed
            .canonical_frames
            .iter()
            .any(|row| row.id == second.id)
    );
    let before = store.query_count.load(Ordering::SeqCst);
    restore_current_history_frames(
        &aggregator.inner,
        RUNTIME,
        IDENTITY,
        &session.to_string(),
        &mut observed,
        &[answer.clone(), answer.clone(), answer.clone(), answer],
        None,
    )
    .await?;
    assert_eq!(
        store.query_count.load(Ordering::SeqCst),
        before + 1,
        "new unmatched multiplicity rechecks historical candidates"
    );
    assert_eq!(
        store
            .query_afters
            .lock()
            .map_err(|_| std::io::Error::other("query cursor log lock poisoned"))?
            .last(),
        Some(&None)
    );
    Ok(())
}

#[tokio::test]
async fn production_backfill_caches_missing_rows_and_skips_sessions_without_notice_images()
-> ConsoleLogResult<()> {
    let (_temp, runtime, service) = super::tests::build_stress_runtime(0, Duration::ZERO).await;
    let session = SessionId::new();
    let store = Arc::new(ObservedStore::new(Vec::new(), None));
    let aggregator = MetaConsoleAggregator::new(store.clone());
    let mut entry = super::tests::runtime_entry_for_test(RUNTIME, &runtime);
    entry.identity_namespace.clear();
    entry.visibility_policy = Arc::new(RedactNoticeAndHistory);
    let mut record = super::tests::identity_record_for_test(IDENTITY);
    record.runtime_key = RUNTIME.into();
    record.session_id = Some(session.to_string());
    let target = SessionBackfillTarget {
        assistant_refresh: AssistantHistoryRefreshReason::Recovery,
        provenance: Some(captured_backfill_test_provenance(&entry, &record)),
        entry: entry.clone(),
        record,
        session_id: session.to_string(),
    };
    aggregator
        .inner
        .runtimes
        .write()
        .map_err(|_| std::io::Error::other("runtime registration lock poisoned"))?
        .insert(RUNTIME.into(), entry);
    let summary = Message::User(meerkat_core::types::UserMessage::compaction_summary(
        "Context from runtime.",
    ));
    let missing = assistant_message("Not previously emitted at any coordinate.")?;
    let current = [summary, missing];
    store
        .append_if_absent(frame(
            "no-notice-anchor",
            &session,
            "run_completed",
            json!({}),
        ))
        .await?;
    let mut observed = observe(&aggregator, &session).await?;
    observed.full_history_positions = true;
    cache_notice_observation(
        &aggregator.inner,
        (RUNTIME.into(), IDENTITY.into(), session.to_string()),
        &observed,
    );
    store
        .record_source_watermark(
            &session_history_watermark_runtime_key(RUNTIME, &session.to_string()),
            ConsoleFrameSourceKind::SessionHistory,
            &format_session_history_watermark(&session.to_string(), 2, 0),
        )
        .await?;
    let queries = store.query_count.load(Ordering::SeqCst);
    script_backfill_pages(&service, [Some(backfill_test_page(&session, &current))]);
    backfill_one_session_history(aggregator.inner.clone(), target.clone(), true).await?;
    assert_eq!(
        store.query_count.load(Ordering::SeqCst),
        queries,
        "no possible notice snapshot needs no restoration search"
    );
    store
        .append_if_absent(applied("now-has-notice", &session, 10, 20))
        .await?;
    script_backfill_pages(&service, [Some(backfill_test_page(&session, &current))]);
    backfill_one_session_history(aggregator.inner.clone(), target.clone(), true).await?;
    observe(&aggregator, &session).await?;
    // The emitted snapshot is a new stored row. One suffix query may advance
    // the prior negative frontier across it, then no old scan is repeated.
    script_backfill_pages(&service, [Some(backfill_test_page(&session, &current))]);
    backfill_one_session_history(aggregator.inner.clone(), target.clone(), true).await?;
    let queries = store.query_count.load(Ordering::SeqCst);
    for _ in 0..3 {
        script_backfill_pages(&service, [Some(backfill_test_page(&session, &current))]);
        backfill_one_session_history(aggregator.inner.clone(), target.clone(), true).await?;
    }
    assert_eq!(
        store.query_count.load(Ordering::SeqCst),
        queries,
        "unchanged missing canonical row reuses successful search after policy-filtered publication"
    );
    let rows = store
        .inner
        .query_frames(ConsoleTimelineQuery {
            limit: 100,
            ..Default::default()
        })
        .await?
        .frames;
    assert_eq!(
        rows.iter()
            .filter(|row| row.kind == "runtime_notice_snapshot")
            .count(),
        1
    );
    runtime.mob_handle().stop().await?;
    Ok(())
}

#[tokio::test]
async fn restoration_search_is_session_scoped_and_store_reset_invalidates_its_frontier()
-> ConsoleLogResult<()> {
    let session = SessionId::new();
    let foreign = SessionId::new();
    let store = Arc::new(ObservedStore::new(Vec::new(), None));
    let aggregator = MetaConsoleAggregator::new(store.clone());
    let current = [assistant_message("Identical bytes across sessions.")?];
    append_canonical_message(&aggregator, &foreign, 10, &current[0]).await?;
    store
        .append_if_absent(applied("session-anchor", &session, 10, 20))
        .await?;
    let mut observed = observe(&aggregator, &session).await?;
    observed.full_history_positions = true;
    restore_current_history_frames(
        &aggregator.inner,
        RUNTIME,
        IDENTITY,
        &session.to_string(),
        &mut observed,
        &current,
        None,
    )
    .await?;
    assert!(
        observed.canonical_frames.is_empty(),
        "foreign equal-content history is not restoration evidence"
    );
    assert!(observed.restoration_search.is_some());
    cache_notice_observation(
        &aggregator.inner,
        (RUNTIME.into(), IDENTITY.into(), session.to_string()),
        &observed,
    );
    store.clear_frames().await?;
    let original = append_canonical_message(&aggregator, &session, 10, &current[0])
        .await?
        .into_iter()
        .next()
        .ok_or("canonical fixture row missing")?;
    store
        .append_if_absent(applied("replacement-anchor", &session, 10, 20))
        .await?;
    let after_reset = observe(&aggregator, &session).await?;
    assert!(
        after_reset.restoration_search.is_none(),
        "the old durable anchor invalidates the cached search even at an equal cursor"
    );
    assert!(
        after_reset
            .canonical_frames
            .iter()
            .any(|row| row.id == original.id)
    );
    Ok(())
}
