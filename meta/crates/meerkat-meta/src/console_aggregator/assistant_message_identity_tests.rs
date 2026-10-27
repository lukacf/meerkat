use super::*;

const RUNTIME: &str = "assistant-identity-runtime";
const IDENTITY: &str = "worker";
const SESSION_A: &str = "00000000-0000-4000-8000-000000000101";
const SESSION_B: &str = "00000000-0000-4000-8000-000000000102";
const MESSAGE_A: &str = "00000000-0000-4000-8000-000000000201";
const MESSAGE_B: &str = "00000000-0000-4000-8000-000000000202";

fn assistant_row(id: Option<&str>, blocks: Vec<Value>) -> Value {
    let mut row = json!({
        "role": "block_assistant", "blocks": blocks, "stop_reason": "end_turn",
        "created_at": "1970-01-01T00:00:00.100Z",
    });
    if let Some(id) = id {
        row["assistant_message_id"] = json!(id);
    }
    row
}

fn text_block(text: &str) -> Value {
    json!({"block_type": "text", "data": {"text": text}})
}

fn tool_block(id: &str, name: &str, args: Value) -> Value {
    json!({"block_type": "tool_use", "data": {"id": id, "name": name, "args": args}})
}

fn image_block() -> Value {
    json!({"block_type": "image", "data": {
        "image_id": "00000000-0000-4000-8000-000000000301",
        "blob_ref": {"blob_id": "sha256:assistant-image", "media_type": "image/png"},
        "media_type": "image/png", "width": 16, "height": 16,
        "revised_prompt": {"disposition": "not_requested"},
        "meta": {"provider": "not_emitted"},
    }})
}

fn project(session: &str, offset: usize, message: Value) -> Vec<NewConsoleFrame> {
    frames_from_session_history_message(RUNTIME, IDENTITY, session, offset, message)
}

#[test]
fn assistant_identity_history_keeps_exact_text_blocks_and_session_scope() -> ConsoleLogResult<()> {
    let blocks = vec![
        text_block("  First"),
        json!({"block_type": "reasoning", "data": {"text": "private reasoning"}}),
        json!({"block_type":"transcript","data":{"text":"\n","source":"spoken"}}),
        text_block("A\u{030a} and \u{00e5}.  "),
        tool_block(
            "peer-call",
            "send_message",
            json!({"peer_id": "peer", "body": "hello"}),
        ),
    ];
    let row = assistant_row(Some(MESSAGE_A), blocks.clone());
    // Parsing through the actual upstream type is part of the contract test.
    let _: Message = serde_json::from_value(row.clone())?;
    let parent = project(SESSION_A, 3, row.clone());
    let child = project(SESSION_B, 3, row.clone());
    assert_eq!(parent.len(), 2, "text and outgoing peer-call siblings");
    for (frames, session) in [(&parent, SESSION_A), (&child, SESSION_B)] {
        for frame in frames {
            assert_eq!(frame.session_id.as_deref(), Some(session));
            assert_eq!(frame.payload["assistant_message_id"], MESSAGE_A);
            assert_ne!(frame.kind, "interaction_complete");
            assert_ne!(frame.kind, "run_completed");
        }
        let text = frames
            .iter()
            .find(|frame| frame.kind == "text_complete")
            .ok_or("canonical assistant text missing")?;
        assert_eq!(text.payload["text"], "  First\nA\u{030a} and \u{00e5}.  ");
        assert_eq!(text.payload["result"], text.payload["text"]);
        assert_eq!(text.payload["message"], row);
        assert_eq!(text.payload["message"]["blocks"], json!(blocks));
    }
    assert_ne!(
        parent[0].dedupe_key, child[0].dedupe_key,
        "fork-inherited IDs are scoped to the actual session"
    );
    let different_id = project(SESSION_A, 4, assistant_row(Some(MESSAGE_B), blocks));
    assert_ne!(
        parent[0].payload["assistant_message_id"],
        different_id[0].payload["assistant_message_id"]
    );
    assert_ne!(parent[0].dedupe_key, different_id[0].dedupe_key);
    Ok(())
}

#[test]
fn assistant_identity_history_stamps_tool_siblings_without_claiming_child_messages()
-> ConsoleLogResult<()> {
    let mut row = assistant_row(
        Some(MESSAGE_A),
        vec![
            tool_block("lookup", "peers", json!({})),
            tool_block(
                "send",
                "send_message",
                json!({"peer_id": "peer", "body": "hello"}),
            ),
            tool_block(
                "spawn",
                "mob_spawn_member",
                json!({"member_id": "child", "initial_message": "Inspect."}),
            ),
        ],
    );
    row["stop_reason"] = json!("tool_use");
    let frames = project(SESSION_A, 5, row);
    let calls = frames
        .iter()
        .filter(|frame| frame.kind == "tool_call_requested")
        .collect::<Vec<_>>();
    assert_eq!(calls.len(), 3);
    for frame in calls {
        assert_eq!(frame.payload["assistant_message_id"], MESSAGE_A);
        assert_eq!(frame.session_id.as_deref(), Some(SESSION_A));
        assert_ne!(
            frame.payload["tool_call_id"],
            frame.payload["assistant_message_id"]
        );
    }
    let child = frames
        .iter()
        .find(|frame| frame.kind == "user_input")
        .ok_or("spawn child input missing")?;
    assert_eq!(child.identity, "child");
    assert_eq!(child.session_id, None);
    assert!(
        child.payload.get("assistant_message_id").is_none(),
        "the parent's assistant row is not the child's user message identity"
    );
    assert!(
        frames
            .iter()
            .all(|frame| frame.kind != "interaction_complete" && frame.kind != "run_completed")
    );
    Ok(())
}

#[test]
fn assistant_identity_legacy_null_and_malformed_history_are_not_promoted() -> ConsoleLogResult<()> {
    for id in [None, Some(Value::Null)] {
        let mut row = assistant_row(None, vec![text_block("Legacy text")]);
        if let Some(id) = id {
            row["assistant_message_id"] = id;
        }
        let frames = project(SESSION_A, 0, row);
        assert_eq!(frames.len(), 1);
        assert!(frames[0].payload.get("assistant_message_id").is_none());
    }
    for malformed in [
        json!(""),
        json!("not-an-id"),
        json!(12),
        json!({"id": MESSAGE_A}),
    ] {
        let mut row = assistant_row(None, vec![text_block("Must not become authority")]);
        row["assistant_message_id"] = malformed;
        assert!(serde_json::from_value::<Message>(row.clone()).is_err());
        assert!(project(SESSION_A, 0, row).is_empty());
    }
    for row in [
        json!({"role":"user","content":"Human text","assistant_message_id":MESSAGE_A}),
        json!({"role":"tool_results","results":[{"tool_use_id":"tool-a","content":"Result","is_error":false}],"assistant_message_id":MESSAGE_A}),
    ] {
        let frames = project(SESSION_A, 0, row);
        assert!(!frames.is_empty());
        assert!(
            frames
                .iter()
                .all(|frame| frame.payload.get("assistant_message_id").is_none())
        );
    }
    Ok(())
}

#[test]
fn assistant_identity_rich_only_history_has_a_canonical_nonterminal_carrier() -> ConsoleLogResult<()>
{
    for block in [
        json!({"block_type":"reasoning","data":{"text":"Reasoning only"}}),
        json!({"block_type":"server_tool_content","data":{"id":"server-item","kind":{"kind":"web_search"},"content":{"status":"completed"}}}),
        image_block(),
    ] {
        let row = assistant_row(Some(MESSAGE_A), vec![block]);
        let _: Message = serde_json::from_value(row.clone())?;
        let frames = project(SESSION_A, 0, row.clone());
        let canonical = frames
            .iter()
            .find(|frame| frame.kind == "assistant_message")
            .ok_or("rich-only assistant history lost its canonical row")?;
        assert_eq!(canonical.payload["assistant_message_id"], MESSAGE_A);
        assert_eq!(canonical.payload["message"], row);
        assert_eq!(canonical.session_id.as_deref(), Some(SESSION_A));
        assert!(frames.iter().all(|frame| !matches!(
            frame.kind.as_str(),
            "text_complete" | "interaction_complete" | "run_completed"
        )));
    }
    let empty = project(SESSION_A, 0, assistant_row(Some(MESSAGE_A), Vec::new()));
    assert_eq!(empty.len(), 1);
    assert_eq!(empty[0].kind, "assistant_message");
    assert_eq!(empty[0].payload["assistant_message_id"], MESSAGE_A);
    assert!(project(SESSION_A, 0, assistant_row(None, Vec::new())).is_empty());
    Ok(())
}

#[tokio::test]
async fn assistant_identity_survives_policy_storage_reopen_and_nested_update()
-> ConsoleLogResult<()> {
    struct RedactText;
    impl ConsoleVisibilityPolicy for RedactText {
        fn redact_payload(&self, frame: &NewConsoleFrame) -> Option<Value> {
            let mut payload = frame.payload.clone();
            payload["text"] = json!("[redacted]");
            payload["result"] = json!("[redacted]");
            payload.as_object_mut()?.remove("message");
            Some(payload)
        }
    }
    let temp = tempfile::tempdir()?;
    let path = temp.path().join("assistant-history.sqlite");
    let store = Arc::new(SqliteConsoleLogStore::open(&path)?);
    let aggregator = MetaConsoleAggregator::new(store.clone());
    let frame = project(
        SESSION_A,
        0,
        assistant_row(Some(MESSAGE_A), vec![text_block("Private")]),
    )
    .into_iter()
    .next()
    .ok_or("history frame missing")?;
    let outcome =
        append_and_emit_with_policy(&aggregator.inner, frame, Arc::new(RedactText)).await?;
    assert_eq!(outcome.frame.status, ConsoleFrameStatus::Redacted);
    assert_eq!(outcome.frame.payload["assistant_message_id"], MESSAGE_A);
    assert!(outcome.frame.payload.get("message").is_none());
    let updated = update_frame_status_and_emit(
        &aggregator.inner,
        &outcome.frame.id,
        ConsoleFrameStatus::Completed,
    )
    .await?
    .ok_or("stored frame not updated")?;
    assert_eq!(updated.payload["assistant_message_id"], MESSAGE_A);
    let rows = store.query_frames(ConsoleTimelineQuery::default()).await?;
    let marker = rows
        .frames
        .iter()
        .find(|frame| frame.kind == "frame_updated")
        .ok_or("update marker missing")?;
    assert_eq!(
        marker.payload["frame"]["payload"]["assistant_message_id"],
        MESSAGE_A
    );
    assert_eq!(marker.payload["frame"]["session_id"], SESSION_A);
    assert!(
        serde_json::to_value(&updated)?
            .get("assistant_message_id")
            .is_none(),
        "assistant identity stays payload metadata, not a duplicated frame column"
    );
    drop(aggregator);
    drop(store);
    let reopened = SqliteConsoleLogStore::open(&path)?;
    let persisted = reopened
        .frame_by_dedupe_key(&updated.dedupe_key)
        .await?
        .ok_or("reopened frame missing")?;
    assert_eq!(persisted, updated);
    let marker = reopened
        .frame_by_dedupe_key(&marker.dedupe_key)
        .await?
        .ok_or("reopened marker missing")?;
    assert_eq!(
        marker.payload["frame"]["payload"]["assistant_message_id"],
        MESSAGE_A
    );
    Ok(())
}

#[tokio::test]
async fn assistant_identity_live_events_preserve_carriers_without_inheriting_them()
-> ConsoleLogResult<()> {
    use crate::types::{EventEnvelope, UnifiedEvent};
    let (_temp, runtime, _) = super::tests::build_stress_runtime(0, Duration::ZERO).await;
    let entry = super::tests::runtime_entry_for_test(RUNTIME, &runtime);
    let events = ConsoleEventStore::new();
    events
        .register_runtime_identity("rt:worker:1", IDENTITY)
        .await;
    let carriers = vec![
        json!({"type":"turn_started","turn_number":1}),
        json!({"type":"reasoning_delta","delta":"Reason"}),
        json!({"type":"reasoning_complete","content":"Reason"}),
        json!({"type":"text_delta","delta":"candidate"}),
        json!({"type":"text_complete","content":"candidate"}),
        json!({"type":"server_tool_content","id":"provider-item","kind":{"kind":"web_search"},"content":{"status":"completed"}}),
        json!({"type":"assistant_image_appended","image":image_block()["data"]}),
        json!({"type":"turn_completed","stop_reason":"end_turn"}),
        json!({"type":"retrying","retry":{"failure":{"provider":"fixture","kind":"network_timeout","message":"retry"},"plan":{"attempt":1,"max_retries":2,"computed_delay_ms":1,"selected_delay_ms":1,"rate_limit_floor_applied":false,"budget_capped":false}}}),
        json!({"type":"run_completed","session_id":SESSION_A,"result":"hook-rewritten result","usage":{"input_tokens":1,"output_tokens":1,"cache_creation_tokens":null,"cache_read_tokens":null}}),
    ];
    let mut ordered = carriers
        .into_iter()
        .map(|raw| (true, raw))
        .collect::<Vec<_>>();
    ordered.splice(
        4..4,
        [
            json!({"type":"tool_call_requested","id":"tool-1","name":"peers","args":{}}),
            json!({"type":"tool_execution_started","id":"tool-1","name":"peers"}),
            json!({"type":"text_delta","delta":"id-less summary"}),
        ]
        .into_iter()
        .map(|raw| (false, raw)),
    );
    for (index, (has_identity, mut raw)) in ordered.into_iter().enumerate() {
        if has_identity {
            raw["assistant_message_id"] = json!(MESSAGE_A);
        }
        let typed: meerkat_core::AgentEvent = serde_json::from_value(raw)?;
        let mut payload = crate::mob_handle_runtime::console_agent_event_payload(&typed);
        if has_identity {
            assert_eq!(payload["assistant_message_id"], MESSAGE_A);
        } else {
            assert!(payload.get("assistant_message_id").is_none());
        }
        payload["session_id"] = json!(SESSION_A);
        let kind = meerkat_core::event::agent_event_type(&typed);
        events
            .project_unified_event(&EventEnvelope {
                event_id: format!(
                    "{}-{index}",
                    if has_identity { "carrier" } else { "idless" }
                ),
                source: "agent".into(),
                timestamp_ms: index as u64,
                event: UnifiedEvent::Agent {
                    agent_id: "rt:worker:1".into(),
                    event_type: kind.into(),
                    payload: Some(payload),
                },
            })
            .await;
    }
    let replay = events
        .replay_all(None)
        .await
        .map_err(|error| std::io::Error::other(format!("replay: {error:?}")))?
        .into_iter()
        .filter(|event| event.identity == IDENTITY)
        .collect::<Vec<_>>();
    assert_eq!(replay.len(), 13);
    for envelope in replay {
        let frame = frame_from_console_event(&entry, envelope);
        assert_eq!(frame.session_id.as_deref(), Some(SESSION_A));
        if frame
            .id
            .as_deref()
            .is_some_and(|id| id.starts_with("carrier-"))
        {
            assert_eq!(frame.payload["assistant_message_id"], MESSAGE_A);
            if frame.kind == "interaction_complete" {
                assert_eq!(frame.payload["source_event_type"], "run_completed");
                assert_eq!(frame.payload["result"], "hook-rewritten result");
            } else {
                assert_eq!(
                    frame.status,
                    ConsoleFrameStatus::Delivered,
                    "text/turn completion is not canonical history or durable commit"
                );
            }
        } else {
            assert!(frame.payload.get("assistant_message_id").is_none());
        }
        assert_eq!(frame.source.kind, ConsoleFrameSourceKind::ConsoleEvent);
    }
    runtime.mob_handle().stop().await?;
    Ok(())
}

#[tokio::test]
async fn assistant_identity_whole_payload_redaction_does_not_restore_removed_identity()
-> ConsoleLogResult<()> {
    struct RemovePayload;
    impl ConsoleVisibilityPolicy for RemovePayload {
        fn redact_payload(&self, _: &NewConsoleFrame) -> Option<Value> {
            Some(json!({"redacted":true}))
        }
    }
    let aggregator = MetaConsoleAggregator::in_memory();
    let frame = project(
        SESSION_A,
        0,
        assistant_row(Some(MESSAGE_A), vec![text_block("Private")]),
    )
    .into_iter()
    .next()
    .ok_or("history frame missing")?;
    let outcome =
        append_and_emit_with_policy(&aggregator.inner, frame, Arc::new(RemovePayload)).await?;
    assert_eq!(outcome.frame.payload, json!({"redacted":true}));
    assert!(outcome.frame.payload.get("assistant_message_id").is_none());
    Ok(())
}

fn history_page(
    session: &str,
    ids: &[&str],
) -> ConsoleLogResult<meerkat_core::service::SessionHistoryPage> {
    let messages = ids
        .iter()
        .map(|id| serde_json::from_value(assistant_row(Some(id), vec![text_block("Same answer")])))
        .collect::<Result<Vec<Message>, _>>()?;
    Ok(meerkat_core::service::SessionHistoryPage::from_messages(
        meerkat_core::types::SessionId::parse(session)?,
        &messages,
        meerkat_core::service::SessionHistoryQuery::default(),
    ))
}

fn observed_live_frame(session: &str, event: &str, id: &str) -> NewConsoleFrame {
    NewConsoleFrame {
        id: None,
        dedupe_key: format!("live:{session}:{event}:{id}"),
        timestamp_ms: 1,
        runtime_key: RUNTIME.into(),
        identity: IDENTITY.into(),
        conversation_id: Some(IDENTITY.into()),
        session_id: Some(session.into()),
        kind: event.into(),
        status: ConsoleFrameStatus::Delivered,
        payload: json!({"assistant_message_id":id,"session_id":session,"delta":"provisional"}),
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

#[tokio::test]
async fn assistant_identity_complete_snapshot_covers_only_prior_events_and_coalesces_after_reopen()
-> ConsoleLogResult<()> {
    let temp = tempfile::tempdir()?;
    let path = temp.path().join("snapshot.sqlite");
    let store = Arc::new(SqliteConsoleLogStore::open(&path)?);
    let aggregator = MetaConsoleAggregator::new(store.clone());
    let covered = store
        .append_if_absent(observed_live_frame(SESSION_A, "text_delta", MESSAGE_A))
        .await?
        .frame;
    let observation =
        observe_runtime_notice_attempts(&aggregator.inner, RUNTIME, IDENTITY, SESSION_A).await?;
    let page = complete_current_history_page(SESSION_A, history_page(SESSION_A, &[MESSAGE_A])?)
        .ok_or("complete page rejected")?;
    let snapshot = assistant_history_snapshot_frame(
        RUNTIME,
        IDENTITY,
        SESSION_A,
        &observation,
        &page.messages,
    )
    .ok_or("complete assistant snapshot missing")?;
    assert_eq!(snapshot.kind, "assistant_history_snapshot");
    assert_eq!(snapshot.source.kind, ConsoleFrameSourceKind::SessionHistory);
    assert_eq!(snapshot.session_id.as_deref(), Some(SESSION_A));
    assert_eq!(snapshot.payload["complete"], true);
    assert_eq!(
        snapshot.payload["assistant_message_ids"],
        json!([MESSAGE_A])
    );
    assert_eq!(
        snapshot.payload["observed_through"],
        serde_json::to_value(&covered.cursor)?
    );
    let later = store
        .append_if_absent(observed_live_frame(SESSION_A, "text_delta", MESSAGE_B))
        .await?
        .frame;
    let snapshot = store.append_if_absent(snapshot).await?.frame;
    assert!(later.cursor.seq() > covered.cursor.seq());
    assert_eq!(
        snapshot.payload["observed_through"],
        serde_json::to_value(&covered.cursor)?
    );
    let next =
        observe_runtime_notice_attempts(&aggregator.inner, RUNTIME, IDENTITY, SESSION_A).await?;
    let advanced =
        assistant_history_snapshot_frame(RUNTIME, IDENTITY, SESSION_A, &next, &page.messages)
            .ok_or("later live event needs a fresh coverage snapshot")?;
    store.append_if_absent(advanced).await?;
    let stable =
        observe_runtime_notice_attempts(&aggregator.inner, RUNTIME, IDENTITY, SESSION_A).await?;
    assert!(
        assistant_history_snapshot_frame(RUNTIME, IDENTITY, SESSION_A, &stable, &page.messages)
            .is_none(),
        "snapshot appends must not trigger endless fresh snapshots"
    );
    assert!(
        stable.canonical_frames.is_empty(),
        "control markers are not canonical message rows"
    );
    drop(aggregator);
    drop(store);
    let reopened = MetaConsoleAggregator::new(Arc::new(SqliteConsoleLogStore::open(&path)?));
    let stable =
        observe_runtime_notice_attempts(&reopened.inner, RUNTIME, IDENTITY, SESSION_A).await?;
    assert!(
        assistant_history_snapshot_frame(RUNTIME, IDENTITY, SESSION_A, &stable, &page.messages)
            .is_none()
    );
    Ok(())
}

#[tokio::test]
async fn assistant_identity_complete_snapshot_distinguishes_empty_head_and_restored_ids()
-> ConsoleLogResult<()> {
    let aggregator = MetaConsoleAggregator::in_memory();
    let empty_observation = RuntimeNoticeObservation::default();
    assert!(
        assistant_history_snapshot_frame(RUNTIME, IDENTITY, SESSION_A, &empty_observation, &[])
            .is_none(),
        "an untouched legacy session needs no assistant identity marker"
    );
    aggregator
        .store()
        .append_if_absent(observed_live_frame(SESSION_A, "text_delta", MESSAGE_A))
        .await?;
    let first = history_page(SESSION_A, &[MESSAGE_A])?;
    let mut last_key = None;
    for messages in [&first.messages[..], &[][..], &first.messages[..]] {
        let observation =
            observe_runtime_notice_attempts(&aggregator.inner, RUNTIME, IDENTITY, SESSION_A)
                .await?;
        let snapshot =
            assistant_history_snapshot_frame(RUNTIME, IDENTITY, SESSION_A, &observation, messages)
                .ok_or("changed head must publish a fresh complete snapshot")?;
        assert_ne!(last_key.as_ref(), Some(&snapshot.dedupe_key));
        assert_eq!(
            snapshot.payload["assistant_message_ids"]
                .as_array()
                .map(Vec::len),
            Some(messages.len())
        );
        last_key = Some(snapshot.dedupe_key.clone());
        aggregator.store().append_if_absent(snapshot).await?;
    }
    let mut partial = first.clone();
    partial.has_more = true;
    assert!(complete_current_history_page(SESSION_A, partial).is_none());
    let mut short = first.clone();
    short.message_count += 1;
    assert!(complete_current_history_page(SESSION_A, short).is_none());
    assert!(complete_current_history_page(SESSION_B, first).is_none());
    Ok(())
}

#[tokio::test]
async fn assistant_identity_snapshot_rejects_reset_and_rebuilds_same_cursor_prefix()
-> ConsoleLogResult<()> {
    let store = Arc::new(InMemoryConsoleLogStore::new());
    let aggregator = MetaConsoleAggregator::new(store.clone());
    let mut live = observed_live_frame(SESSION_A, "text_delta", MESSAGE_A);
    live.id = Some("reused-live-frame-id".into());
    store.append_if_absent(live.clone()).await?;
    let observation =
        observe_runtime_notice_attempts(&aggregator.inner, RUNTIME, IDENTITY, SESSION_A).await?;
    let mut snapshot =
        assistant_history_snapshot_frame(RUNTIME, IDENTITY, SESSION_A, &observation, &[])
            .ok_or("snapshot missing")?;
    snapshot.id = Some("reused-snapshot-frame-id".into());
    store.append_if_absent(snapshot.clone()).await?;
    let before =
        observe_runtime_notice_attempts(&aggregator.inner, RUNTIME, IDENTITY, SESSION_A).await?;
    store.clear_frames().await?;
    // Reuse the old tail and cursors but replace an earlier canonical row.
    // A tail-only cache check cannot detect this reset.
    let replacement = project(
        SESSION_A,
        0,
        assistant_row(Some(MESSAGE_B), vec![text_block("Replacement")]),
    )
    .into_iter()
    .next()
    .ok_or("replacement missing")?;
    store.append_if_absent(replacement).await?;
    store.append_if_absent(snapshot).await?;
    assert!(!assistant_history_prefix_is_current(&aggregator.inner, IDENTITY, &before).await?);
    let rebuilt =
        observe_runtime_notice_attempts(&aggregator.inner, RUNTIME, IDENTITY, SESSION_A).await?;
    assert_eq!(rebuilt.canonical_frames.len(), 1);
    assert_eq!(
        rebuilt.assistant_frontier, 0,
        "old live frontier must not survive reset"
    );
    assert!(rebuilt.has_assistant_history);
    Ok(())
}

#[tokio::test]
async fn assistant_identity_redacted_snapshots_keep_coalescing_without_restoring_payload()
-> ConsoleLogResult<()> {
    struct RedactSnapshot;
    impl ConsoleVisibilityPolicy for RedactSnapshot {
        fn redact_payload(&self, _: &NewConsoleFrame) -> Option<Value> {
            Some(json!({"redacted":true}))
        }
    }
    let aggregator = MetaConsoleAggregator::in_memory();
    aggregator
        .store()
        .append_if_absent(observed_live_frame(SESSION_A, "text_delta", MESSAGE_A))
        .await?;
    let observation =
        observe_runtime_notice_attempts(&aggregator.inner, RUNTIME, IDENTITY, SESSION_A).await?;
    let page = history_page(SESSION_A, &[MESSAGE_A])?;
    let snapshot = assistant_history_snapshot_frame(
        RUNTIME,
        IDENTITY,
        SESSION_A,
        &observation,
        &page.messages,
    )
    .ok_or("snapshot missing")?;
    let saved = append_and_emit_with_policy(&aggregator.inner, snapshot, Arc::new(RedactSnapshot))
        .await?
        .frame;
    assert_eq!(saved.payload, json!({"redacted":true}));
    let observation =
        observe_runtime_notice_attempts(&aggregator.inner, RUNTIME, IDENTITY, SESSION_A).await?;
    assert!(
        assistant_history_snapshot_frame(
            RUNTIME,
            IDENTITY,
            SESSION_A,
            &observation,
            &page.messages
        )
        .is_none()
    );
    assert!(observation.canonical_frames.is_empty());
    Ok(())
}

struct NoRevisionStore {
    inner: InMemoryConsoleLogStore,
    scans: std::sync::atomic::AtomicUsize,
    fail_scan: std::sync::atomic::AtomicBool,
}

#[async_trait::async_trait]
impl ConsoleLogStore for NoRevisionStore {
    async fn append_if_absent(&self, frame: NewConsoleFrame) -> ConsoleLogResult<AppendOutcome> {
        self.inner.append_if_absent(frame).await
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
        self.scans.fetch_add(1, Ordering::SeqCst);
        if self.fail_scan.load(Ordering::SeqCst) {
            return Err(std::io::Error::other("injected assistant prefix scan failure").into());
        }
        self.inner.query_frames(query).await
    }
    async fn frame_by_dedupe_key(&self, key: &str) -> ConsoleLogResult<Option<ConsoleFrame>> {
        self.inner.frame_by_dedupe_key(key).await
    }
    async fn latest_cursor(&self) -> ConsoleLogResult<Option<ConsoleCursor>> {
        self.inner.latest_cursor().await
    }
    async fn clear_frames(&self) -> ConsoleLogResult<()> {
        self.inner.clear_frames().await
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
async fn assistant_identity_custom_store_checks_a_fresh_complete_prefix_and_reports_failure()
-> ConsoleLogResult<()> {
    let store = Arc::new(NoRevisionStore {
        inner: InMemoryConsoleLogStore::new(),
        scans: 0.into(),
        fail_scan: false.into(),
    });
    let aggregator = MetaConsoleAggregator::new(store.clone());
    store
        .append_if_absent(observed_live_frame(SESSION_A, "text_delta", MESSAGE_A))
        .await?;
    let observation =
        observe_runtime_notice_attempts(&aggregator.inner, RUNTIME, IDENTITY, SESSION_A).await?;
    assert!(assistant_history_prefix_is_current(&aggregator.inner, IDENTITY, &observation).await?);
    let scans = store.scans.load(Ordering::SeqCst);
    let next =
        observe_runtime_notice_attempts(&aggregator.inner, RUNTIME, IDENTITY, SESSION_A).await?;
    assert!(
        store.scans.load(Ordering::SeqCst) > scans,
        "None revision must not reuse a cached absence proof"
    );
    store.fail_scan.store(true, Ordering::SeqCst);
    assert!(
        assistant_history_prefix_is_current(&aggregator.inner, IDENTITY, &next)
            .await
            .is_err()
    );
    store.fail_scan.store(false, Ordering::SeqCst);
    store.clear_frames().await?;
    store
        .append_if_absent(observed_live_frame(SESSION_A, "text_delta", MESSAGE_B))
        .await?;
    assert!(!assistant_history_prefix_is_current(&aggregator.inner, IDENTITY, &next).await?);
    Ok(())
}

#[tokio::test]
async fn assistant_identity_known_history_is_not_deduplicated_by_identical_text()
-> ConsoleLogResult<()> {
    let aggregator = MetaConsoleAggregator::in_memory();
    for (offset, id) in [MESSAGE_A, MESSAGE_B].into_iter().enumerate() {
        let frame = project(
            SESSION_A,
            offset,
            assistant_row(Some(id), vec![text_block("Same answer")]),
        )
        .into_iter()
        .next()
        .ok_or("canonical row missing")?;
        assert!(
            !history_frame_has_existing_counterpart(&aggregator.inner, &frame).await?,
            "a different assistant ID must not be swallowed by legacy content matching"
        );
        aggregator.store().append_if_absent(frame).await?;
    }
    assert_eq!(
        aggregator
            .store()
            .query_frames(ConsoleTimelineQuery::default())
            .await?
            .frames
            .len(),
        2
    );
    Ok(())
}

#[tokio::test]
async fn assistant_identity_append_only_runs_do_not_repeat_full_position_maps()
-> ConsoleLogResult<()> {
    let (_temp, runtime, service) = super::tests::build_stress_runtime(1, Duration::ZERO).await;
    let store = Arc::new(InMemoryConsoleLogStore::new());
    let aggregator = MetaConsoleAggregator::new(store.clone());
    let mut entry = super::tests::runtime_entry_for_test(RUNTIME, &runtime);
    entry.identity_namespace.clear();
    let member = runtime
        .mob_handle()
        .list_members_observation_snapshot()
        .await
        .into_iter()
        .next()
        .ok_or("fixture member missing")?;
    let record = identity_record_for_member(&entry, &runtime.mob_handle(), &member)
        .await
        .ok_or("fixture identity missing")?;
    let session_id = record.session_id.clone().ok_or("fixture session missing")?;
    let session = meerkat_core::types::SessionId::parse(&session_id)?;
    // Spawn acknowledges admission. Settle the initial turn before scripting
    // history so the real refresh gate can authorize each complete image.
    let handle = entry.runtime.handle();
    let member_id =
        crate::member_comms_id::roster_member_id_for_supplied_id(&record.runtime_member_id);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let mut last_observation = String::from("startup observation not completed");
    tokio::time::timeout_at(deadline, async {
        loop {
            let execution =
                meerkat_mob::MobSessionService::execution_snapshot(&service, &session).await?;
            let status = crate::member_status_observation::observe_member_status_until(
                &handle, &member_id, deadline,
            )
            .await?;
            let pending = entry.runtime.session_commit_pending(&session_id).await;
            last_observation = format!(
                "execution={execution:?}, session={:?}, progress={:?}, commit_pending={pending:?}",
                status.current_session_id, status.progress,
            );
            if execution.as_ref().is_some_and(|snapshot| {
                snapshot.turn_terminal && snapshot.terminal_run_id.is_some()
            }) && status.current_session_id.as_ref() == Some(&session)
                && status.progress.as_ref().is_some_and(|progress| {
                    progress.run_state == meerkat_mob::MemberRunState::Idle
                        && progress.in_flight_work == 0
                })
                && pending == Some(false)
            {
                return Ok::<(), ConsoleLogError>(());
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .map_err(|_| {
        std::io::Error::other(format!(
            "fixture initial turn did not commit before scripted history: {last_observation}"
        ))
    })??;
    let target = SessionBackfillTarget {
        assistant_refresh: assistant_history_refresh::AssistantHistoryRefreshReason::Recovery,
        provenance: None,
        entry: entry.clone(),
        record,
        session_id: session_id.clone(),
    };
    aggregator
        .inner
        .runtimes
        .write()
        .map_err(|_| std::io::Error::other("runtime fixture lock"))?
        .insert(RUNTIME.into(), entry);

    let assistant = |ordinal: u128| -> ConsoleLogResult<Message> {
        Ok(serde_json::from_value(assistant_row(
            Some(&uuid::Uuid::from_u128(ordinal + 1_000).to_string()),
            vec![text_block("Same answer")],
        ))?)
    };
    let notice = |ordinal| {
        let mut notice = meerkat_core::types::SystemNoticeMessage::new(
            meerkat_core::types::SystemNoticeKind::Generic,
            "New peer notice.",
        );
        notice.runtime_origin = Some(meerkat_core::types::RuntimeAppendOrigin {
            session_id: session.clone(),
            run_id: meerkat_core::lifecycle::RunId(uuid::Uuid::from_u128(ordinal)),
            input_id: meerkat_core::lifecycle::InputId(uuid::Uuid::from_u128(ordinal + 100)),
            append_ordinal: 0,
        });
        Message::SystemNotice(notice)
    };
    let mut messages = (0..64)
        .map(assistant)
        .collect::<ConsoleLogResult<Vec<_>>>()?;
    let mut images = vec![(messages.clone(), 1)];
    for ordinal in 64..68 {
        messages.push(assistant(ordinal)?);
        // Ordinary appended notices change the notice image too. They must
        // not force the unchanged history-position prefix onto the wire.
        messages.push(notice(ordinal));
        images.push((messages.clone(), 1));
    }
    let before_compaction = messages.clone();
    messages.drain(..32);
    images.push((messages.clone(), 2));
    for ordinal in 68..72 {
        messages.push(assistant(ordinal)?);
        messages.push(notice(ordinal));
        images.push((messages.clone(), 2));
    }
    images.push((before_compaction.clone(), 3));
    // Isolate restoration from relocation and removal: truncate a suffix,
    // then restore it while all surviving rows retain their coordinates.
    images.push((before_compaction[..32].to_vec(), 4));
    images.push((before_compaction, 5));
    let reads = service.read_calls();
    let image_count = images.len();
    for (index, (messages, expected_full_maps)) in images.into_iter().enumerate() {
        if index % 2 == 1 {
            // Exercise replay of full and sparse maps as well as the warm
            // observation cache. Both must retain prior moved coordinates.
            aggregator
                .inner
                .notice_observations
                .lock()
                .map_err(|_| std::io::Error::other("observation cache lock"))?
                .entries
                .clear();
        }
        let mut live = observed_live_frame(
            &session_id,
            "turn_completed",
            &uuid::Uuid::from_u128(10_000 + index as u128).to_string(),
        );
        live.identity = target.record.identity.clone();
        live.conversation_id = Some(live.identity.clone());
        let observed_through = store.append_if_absent(live).await?.frame.cursor;
        service.script_history([super::tests::ScriptedHistoryRead {
            page: Some(meerkat_core::service::SessionHistoryPage::from_messages(
                session.clone(),
                &messages,
                meerkat_core::service::SessionHistoryQuery::default(),
            )),
            gate: None,
        }]);
        backfill_one_session_history(aggregator.inner.clone(), target.clone(), true).await?;
        let rows = store
            .query_frames(ConsoleTimelineQuery {
                limit: 1_000,
                ..Default::default()
            })
            .await?
            .frames;
        let full_maps = rows
            .iter()
            .filter(|frame| {
                frame.kind == "runtime_notice_snapshot"
                    && frame.payload.get("history_positions_mode").is_none()
            })
            .count();
        assert_eq!(
            full_maps, expected_full_maps,
            "image {index}: full maps are only initial, compaction, and restoration"
        );
        let latest = rows
            .iter()
            .rev()
            .find(|frame| frame.kind == "assistant_history_snapshot")
            .ok_or("assistant image missing")?;
        assert_eq!(latest.runtime_key, RUNTIME);
        assert_eq!(latest.identity, target.record.identity);
        assert_eq!(latest.session_id.as_deref(), Some(session_id.as_str()));
        assert_eq!(latest.source.kind, ConsoleFrameSourceKind::SessionHistory);
        assert_eq!(latest.payload["session_id"], session_id);
        assert_eq!(latest.payload["complete"], true);
        assert_eq!(
            latest.payload["observed_through"],
            json!(observed_through),
            "image {index}: the published assistant snapshot covers this exact prefix"
        );
        let ids: Vec<_> = messages
            .iter()
            .filter_map(|message| {
                let Message::BlockAssistant(assistant) = message else {
                    return None;
                };
                assistant.assistant_message_id.as_ref()
            })
            .collect();
        assert_eq!(latest.payload["assistant_message_ids"], json!(ids));
        assert!(latest.payload.get("history_positions").is_none());
        for sparse in rows.iter().filter(|frame| {
            frame.kind == "runtime_notice_snapshot"
                && frame.payload["history_positions_mode"] == "sparse"
        }) {
            assert_eq!(sparse.payload["history_positions"], json!([]));
            assert_eq!(sparse.payload["removed_history_frame_ids"], json!([]));
        }
        if index == image_count - 1 {
            let full = rows
                .iter()
                .rev()
                .find(|frame| frame.kind == "runtime_notice_snapshot")
                .ok_or("restoration image missing")?;
            assert_eq!(
                full.payload["history_positions"].as_array().map(Vec::len),
                Some(68)
            );
            let expected: BTreeMap<_, _> = messages
                .iter()
                .enumerate()
                .filter_map(|(offset, message)| {
                    let Message::BlockAssistant(assistant) = message else {
                        return None;
                    };
                    Some((
                        assistant.assistant_message_id.as_ref()?.to_string(),
                        format!("{session_id}:{offset}"),
                    ))
                })
                .collect();
            let actual: BTreeMap<_, _> = full.payload["history_positions"]
                .as_array()
                .ok_or("positions missing")?
                .iter()
                .map(|position| {
                    let row = rows
                        .iter()
                        .find(|row| row.id == position["frame_id"])
                        .ok_or("restored frame missing")?;
                    Ok((
                        row.payload["assistant_message_id"]
                            .as_str()
                            .ok_or("restored ID missing")?
                            .to_string(),
                        position["source_cursor"]
                            .as_str()
                            .ok_or("restored cursor missing")?
                            .to_string(),
                    ))
                })
                .collect::<ConsoleLogResult<_>>()?;
            assert_eq!(
                actual, expected,
                "restoration reuses every original assistant frame at its exact coordinate"
            );
        }
    }
    assert_eq!(service.read_calls(), reads + image_count);
    runtime.mob_handle().stop().await?;
    Ok(())
}

/// Wait until the fixture member's initial run (its spawn prompt) is done and
/// the assistant-history refresh gate reads `Settled`: fresh typed idle status
/// for this exact session and no uncommitted run input.
///
/// Idle with nothing admitted is not enough: under load the member can read
/// idle before its kickoff input is admitted, then open that run while the
/// test backfills, and a backfill over a `Pending` gate correctly publishes no
/// snapshot. A kickoff that reached `Started` has been admitted, so from there
/// a `Settled` gate means that run completed and committed. Both signals are
/// typed and re-read only after a machine-state change (kickoff transitions)
/// or a member event (run and commit progress), subscribed before the first
/// read; the deadline only bounds a broken fixture and fails with a clear
/// message.
#[allow(clippy::panic)]
async fn await_initial_run_settled(
    handle: &MobHandle,
    member: &AgentIdentity,
    entry: &RuntimeEntry,
    record: &ConsoleIdentityRecord,
    session_id: &str,
) {
    use futures::StreamExt as _;
    let mut member_events = handle
        .subscribe_agent_events(member)
        .await
        .unwrap_or_else(|error| panic!("subscribe to the fixture member's events: {error}"));
    let mut changes = handle.machine_state_changes();
    let settled = async {
        loop {
            let kickoff = handle
                .list_members_observation_snapshot()
                .await
                .into_iter()
                .find(|entry| &entry.agent_identity == member)
                .unwrap_or_else(|| panic!("the fixture member left the roster"))
                .kickoff
                .map(|kickoff| kickoff.phase);
            let admitted = match kickoff {
                Some(
                    meerkat_mob::MobMemberKickoffPhase::Failed
                    | meerkat_mob::MobMemberKickoffPhase::Cancelled,
                ) => {
                    panic!("the fixture member's initial run ended {kickoff:?}")
                }
                Some(meerkat_mob::MobMemberKickoffPhase::Started) | None => true,
                Some(_) => false,
            };
            if admitted
                && assistant_history_refresh::observe(entry, record, session_id, true).await
                    == assistant_history_refresh::AssistantHistoryRefreshGate::Settled
            {
                return;
            }
            tokio::select! {
                event = member_events.next() => assert!(
                    event.is_some(),
                    "the fixture member's event stream ended before its initial run settled"
                ),
                changed = changes.changed() => assert!(
                    changed.is_ok(),
                    "the mob actor stopped before the fixture member's initial run settled"
                ),
            }
        }
    };
    assert!(
        tokio::time::timeout(Duration::from_mins(1), settled)
            .await
            .is_ok(),
        "the fixture member's initial run did not complete and settle (typed idle, no uncommitted run input) within 60 s"
    );
}

#[tokio::test]
async fn assistant_identity_backfill_publishes_only_complete_current_images_and_restores_rows()
-> ConsoleLogResult<()> {
    let (_temp, runtime, service) = super::tests::build_stress_runtime(1, Duration::ZERO).await;
    let store = Arc::new(InMemoryConsoleLogStore::new());
    let aggregator = MetaConsoleAggregator::new(store.clone());
    let mut entry = super::tests::runtime_entry_for_test(RUNTIME, &runtime);
    entry.identity_namespace.clear();
    let member = runtime
        .mob_handle()
        .list_members_observation_snapshot()
        .await
        .into_iter()
        .next()
        .ok_or("fixture member missing")?;
    let record = identity_record_for_member(&entry, &runtime.mob_handle(), &member)
        .await
        .ok_or("fixture identity missing")?;
    let session_id = record.session_id.clone().ok_or("fixture session missing")?;
    await_initial_run_settled(
        &runtime.mob_handle(),
        &member.agent_identity,
        &entry,
        &record,
        &session_id,
    )
    .await;
    let mob_id = entry.runtime.handle().mob_id().to_string();
    let provenance = ConsoleFrameMemberProvenance {
        identity: record.clone(),
        member: ConsoleMember {
            agent_identity: record.identity.clone(),
            role: "worker".into(),
            state: "running".into(),
            model_capabilities: Default::default(),
            runtime_mode: None,
            session_id: record.session_id.clone(),
            wired_to: Vec::new(),
            labels: BTreeMap::new(),
            progress: None,
        },
        primary_mob_id: mob_id.clone(),
        source_mob_id: mob_id,
        lineage: Vec::new(),
    };
    let target = SessionBackfillTarget {
        assistant_refresh: assistant_history_refresh::AssistantHistoryRefreshReason::Recovery,
        provenance: Some(provenance),
        entry: entry.clone(),
        record,
        session_id: session_id.clone(),
    };
    aggregator
        .inner
        .runtimes
        .write()
        .map_err(|_| std::io::Error::other("runtime fixture lock"))?
        .insert(RUNTIME.into(), entry);
    let script =
        |page| service.script_history([super::tests::ScriptedHistoryRead { page, gate: None }]);
    let first = history_page(&session_id, &[MESSAGE_A])?;
    let mut partial = first.clone();
    partial.has_more = true;
    for page in [
        None,
        Some(partial),
        Some(history_page(SESSION_B, &[MESSAGE_A])?),
    ] {
        script(page);
        backfill_one_session_history(aggregator.inner.clone(), target.clone(), true).await?;
        assert!(
            store
                .query_frames(ConsoleTimelineQuery::default())
                .await?
                .frames
                .iter()
                .all(|frame| frame.kind != "assistant_history_snapshot")
        );
    }
    let reads = service.read_calls();
    let replacement: Message = serde_json::from_value(assistant_row(
        None,
        vec![text_block("Replacement without identity")],
    ))?;
    let rewritten = meerkat_core::service::SessionHistoryPage::from_messages(
        meerkat_core::types::SessionId::parse(&session_id)?,
        &[replacement],
        meerkat_core::service::SessionHistoryQuery::default(),
    );
    let images = [
        first.clone(),
        history_page(&session_id, &[MESSAGE_B])?,
        rewritten,
        history_page(&session_id, &[])?,
        first.clone(),
    ];
    for page in images {
        let expected_messages = serde_json::to_value(&page.messages)?;
        let ids: Vec<_> = page
            .messages
            .iter()
            .filter_map(|message| {
                let Message::BlockAssistant(row) = message else {
                    return None;
                };
                row.assistant_message_id.as_ref()
            })
            .collect();
        let expected_ids = serde_json::to_value(ids)?;
        script(Some(page));
        backfill_one_session_history(aggregator.inner.clone(), target.clone(), true).await?;
        let rows = store
            .query_frames(ConsoleTimelineQuery::default())
            .await?
            .frames;
        let latest = rows
            .iter()
            .rev()
            .find(|frame| frame.kind == "assistant_history_snapshot")
            .ok_or("complete backfill snapshot missing")?;
        assert_eq!(latest.payload["assistant_message_ids"], expected_ids);
        let position_map = rows
            .iter()
            .rev()
            .find(|frame| frame.kind == "runtime_notice_snapshot")
            .ok_or("paired position map missing")?;
        assert_eq!(
            position_map.payload["observed_through"],
            latest.payload["observed_through"]
        );
        assert!(
            position_map.payload.get("history_positions_mode").is_none(),
            "identity projection uses a complete position map"
        );
        let boundary = ConsoleCursor::from(
            position_map.payload["observed_through"]
                .as_str()
                .ok_or("cursor missing")?,
        )
        .seq()
        .ok_or("invalid cursor")?;
        let positions = position_map.payload["history_positions"]
            .as_array()
            .ok_or("positions missing")?;
        let mut visible: Vec<_> = rows
            .iter()
            .filter(|frame| {
                frame.source.kind == ConsoleFrameSourceKind::SessionHistory
                    && frame.payload["message"]["role"] == "block_assistant"
                    && (frame
                        .cursor
                        .seq()
                        .is_some_and(|sequence| sequence > boundary)
                        || positions
                            .iter()
                            .any(|position| position["frame_id"] == frame.id))
            })
            .collect();
        visible.sort_by_key(|frame| frame.source.source_cursor.clone());
        assert_eq!(
            json!(
                visible
                    .iter()
                    .map(|frame| &frame.payload["message"])
                    .collect::<Vec<_>>()
            ),
            expected_messages,
            "complete maps must remove ID rows, show no-ID rewrites, and restore original rows without runtime notices"
        );
    }
    assert_eq!(
        service.read_calls(),
        reads + 5,
        "one owner history read per complete image"
    );
    let before = store.latest_cursor().await?;
    script(Some(first));
    backfill_one_session_history(aggregator.inner.clone(), target, true).await?;
    assert_eq!(
        store.latest_cursor().await?,
        before,
        "unchanged history must not grow snapshots"
    );
    runtime.mob_handle().stop().await?;
    Ok(())
}

#[tokio::test]
async fn assistant_identity_pending_refresh_retries_unchanged_head_through_discovery()
-> ConsoleLogResult<()> {
    let (_temp, runtime, service) = super::tests::build_stress_runtime(1, Duration::ZERO).await;
    let store = Arc::new(InMemoryConsoleLogStore::new());
    let aggregator = MetaConsoleAggregator::new(store.clone());
    let mut entry = super::tests::runtime_entry_for_test(RUNTIME, &runtime);
    entry.identity_namespace.clear();
    let member = runtime
        .mob_handle()
        .list_members_observation_snapshot()
        .await
        .into_iter()
        .next()
        .ok_or("fixture member missing")?;
    let record = identity_record_for_member(&entry, &runtime.mob_handle(), &member)
        .await
        .ok_or("fixture identity missing")?;
    let session_id = record.session_id.clone().ok_or("fixture session missing")?;
    let machine = meerkat_mob::MobSessionService::acquire_runtime_adapter(&service, None)?
        .ok_or("fixture must expose the real runtime machine")?;
    let typed_session_id = meerkat_core::SessionId::parse(&session_id)?;
    assert!(
        machine.contains_session(&typed_session_id).await,
        "fixture session must be registered with its real machine"
    );
    // Spawn acknowledges admission, so await the fresh fixture's initial
    // turn before testing recovery. A terminal run witness also rules out
    // the idle gap before queued input starts; commit status alone cannot.
    let handle = entry.runtime.handle();
    let member_id =
        crate::member_comms_id::roster_member_id_for_supplied_id(&record.runtime_member_id);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let mut last_observation = String::from("startup observation not completed");
    tokio::time::timeout_at(deadline, async {
        loop {
            let execution =
                meerkat_mob::MobSessionService::execution_snapshot(&service, &typed_session_id)
                    .await?;
            let status = crate::member_status_observation::observe_member_status_until(
                &handle, &member_id, deadline,
            )
            .await?;
            let pending = entry.runtime.session_commit_pending(&session_id).await;
            last_observation = format!(
                "execution={execution:?}, session={:?}, progress={:?}, commit_pending={pending:?}",
                status.current_session_id, status.progress,
            );
            if execution.as_ref().is_some_and(|snapshot| {
                snapshot.turn_terminal && snapshot.terminal_run_id.is_some()
            }) && status.current_session_id.as_ref() == Some(&typed_session_id)
                && status.progress.as_ref().is_some_and(|progress| {
                    progress.run_state == meerkat_mob::MemberRunState::Idle
                        && progress.in_flight_work == 0
                })
                && pending == Some(false)
            {
                return Ok::<(), ConsoleLogError>(());
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .map_err(|_| {
        std::io::Error::other(format!(
            "fixture initial turn did not commit before recovery: {last_observation}"
        ))
    })??;
    assert_eq!(
        entry.runtime.session_commit_pending(&session_id).await,
        Some(false),
        "settled retries must observe the real runtime commit authority"
    );
    let mut target = SessionBackfillTarget {
        assistant_refresh: assistant_history_refresh::AssistantHistoryRefreshReason::Recovery,
        provenance: None,
        entry: entry.clone(),
        record: record.clone(),
        session_id: session_id.clone(),
    };
    aggregator
        .inner
        .runtimes
        .write()
        .map_err(|_| std::io::Error::other("runtime fixture lock"))?
        .insert(RUNTIME.into(), entry);
    // Pending and unknown commit observations both map to Pending in the guard
    // unit tests. Exercise that result at the orchestration boundary here, then
    // use the production observer for settlement with either cache state.
    for (index, evict_cache) in [false, true].into_iter().enumerate() {
        let mut live = observed_live_frame(&session_id, "text_delta", MESSAGE_A);
        live.identity = record.identity.clone();
        live.conversation_id = Some(record.identity.clone());
        live.dedupe_key = format!("failed-attempt-{index}");
        store.append_if_absent(live.clone()).await?;
        let before = store
            .query_frames(ConsoleTimelineQuery::default())
            .await?
            .frames
            .iter()
            .filter(|frame| frame.kind == "assistant_history_snapshot")
            .count();
        target.assistant_refresh =
            assistant_history_refresh::AssistantHistoryRefreshReason::Recovery;
        service.script_history([super::tests::ScriptedHistoryRead {
            page: Some(history_page(&session_id, &[])?),
            gate: None,
        }]);
        let mut pending_observations = 0;
        let reads_before_pending = service.read_calls();
        backfill_one_session_history_with_refresh_observer(
            aggregator.inner.clone(),
            target.clone(),
            true,
            |observed_entry, observed_record, observed_session, requested| {
                assert!(requested, "recovery must request a settlement observation");
                assert_eq!(observed_entry.registration_id, target.entry.registration_id);
                assert_eq!(observed_record.identity, record.identity);
                assert_eq!(observed_session, session_id);
                pending_observations += 1;
                Box::pin(std::future::ready(
                    assistant_history_refresh::AssistantHistoryRefreshGate::Pending,
                ))
            },
        )
        .await?;
        assert_eq!(pending_observations, 1);
        assert_eq!(service.read_calls(), reads_before_pending + 1);
        assert_eq!(
            store
                .query_frames(ConsoleTimelineQuery::default())
                .await?
                .frames
                .iter()
                .filter(|frame| frame.kind == "assistant_history_snapshot")
                .count(),
            before
        );
        let key = (
            RUNTIME.to_string(),
            record.identity.clone(),
            session_id.clone(),
        );
        let retry_key = (
            target.entry.registration_id,
            RUNTIME.to_string(),
            record.identity.clone(),
            session_id.clone(),
        );
        assert!(
            aggregator
                .inner
                .assistant_history_retries
                .lock()
                .map_err(|_| std::io::Error::other("fixture retry lock"))?
                .contains(&retry_key)
        );
        {
            let mut cache = aggregator
                .inner
                .notice_observations
                .lock()
                .map_err(|_| std::io::Error::other("fixture cache lock"))?;
            let observation = cache
                .entries
                .get_mut(&key)
                .ok_or("pending retry observation missing")?;
            assert!(!observation.verified_assistant_settled);
            // Exercise the bounded frontier cache path as well as ordinary
            // freshness gating, with identical history and live frontier.
            observation.history_index_complete = false;
            observation.canonical_frames.clear();
            if evict_cache {
                for other in 0..17 {
                    cache.insert(
                        (
                            RUNTIME.to_string(),
                            format!("other-{other}"),
                            format!("other-session-{other}"),
                        ),
                        RuntimeNoticeObservation::default(),
                    );
                }
                assert!(
                    !cache.entries.contains_key(&key),
                    "fixture must evict the pending session projection cache"
                );
            }
        }
        if evict_cache {
            store.clear_frames().await?;
            store.append_if_absent(live).await?;
        }
        target.assistant_refresh =
            assistant_history_refresh::AssistantHistoryRefreshReason::PositiveOnly;
        let watermark_runtime_key = session_history_watermark_runtime_key(RUNTIME, &session_id);
        record_session_history_watermark(&aggregator.inner, &watermark_runtime_key, &session_id, 0)
            .await?;
        let watermark = store
            .source_watermark(
                &watermark_runtime_key,
                ConsoleFrameSourceKind::SessionHistory,
            )
            .await?
            .ok_or("fresh retry watermark missing")?;
        assert!(session_history_watermark_is_fresh(
            &watermark,
            &session_id,
            current_time_ms(),
        ));
        let reads = service.read_calls();
        service.script_history([super::tests::ScriptedHistoryRead {
            page: Some(history_page(&session_id, &[])?),
            gate: None,
        }]);
        backfill_one_session_history(aggregator.inner.clone(), target.clone(), false).await?;
        let rows = store
            .query_frames(ConsoleTimelineQuery::default())
            .await?
            .frames;
        let latest = rows
            .iter()
            .rev()
            .find(|frame| frame.kind == "assistant_history_snapshot")
            .ok_or("settled retry snapshot missing")?;
        assert_eq!(latest.payload["assistant_message_ids"], json!([]));
        let position_map = rows
            .iter()
            .rev()
            .find(|frame| frame.kind == "runtime_notice_snapshot")
            .ok_or("settled retry position map missing")?;
        let cutoff = |frame: &ConsoleFrame| -> ConsoleLogResult<u64> {
            ConsoleCursor::from(
                frame.payload["observed_through"]
                    .as_str()
                    .ok_or("missing cutoff")?,
            )
            .seq()
            .ok_or_else(|| "invalid cutoff".into())
        };
        assert!(
            cutoff(position_map)? <= cutoff(latest)?,
            "settled retry authorizes the unchanged prior position image"
        );
        assert_eq!(
            rows.iter()
                .filter(|frame| frame.kind == "runtime_notice_snapshot")
                .count(),
            1,
            "settling an unchanged empty head must not repeat its full map"
        );
        assert!(position_map.payload.get("history_positions_mode").is_none());
        assert_eq!(position_map.payload["history_positions"], json!([]));
        assert_eq!(
            rows.iter()
                .filter(|frame| frame.kind == "assistant_history_snapshot")
                .count(),
            if evict_cache { 1 } else { before + 1 }
        );
        assert_eq!(
            service.read_calls(),
            reads + 1,
            "pending request must bypass freshness without rereading unchanged owner history"
        );
        assert!(
            !aggregator
                .inner
                .assistant_history_retries
                .lock()
                .map_err(|_| std::io::Error::other("fixture cache lock"))?
                .contains(&retry_key)
        );
    }
    runtime.mob_handle().stop().await?;
    Ok(())
}

/// Boot-time staged-turn shape (a downstream app, 0.8.45: a 453 s first turn after a
/// cold boot, 6-7 minutes of it staged). A member whose turn is staged or
/// running is not idle, so its recovery refresh stays unsettled and the
/// member stays on the retry list. Every 5 s discovery pass used to re-read
/// the member's whole session document even though nothing durable changed.
/// An unsettled pass over an unchanged committed boundary authority now
/// skips that read. The pass that observes the member settled still reads,
/// and so does any pass after a new boundary commits.
#[tokio::test]
async fn unsettled_recovery_refresh_does_not_reread_an_unchanged_document() -> ConsoleLogResult<()>
{
    let durable: Arc<dyn meerkat_runtime::RuntimeStore> =
        Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());
    let (_temp, runtime, service) = super::tests::build_stress_runtime_with_durable_runtime_store(
        1,
        Duration::ZERO,
        Some(durable.clone()),
    )
    .await;
    let store = Arc::new(InMemoryConsoleLogStore::new());
    let aggregator = MetaConsoleAggregator::new(store.clone());
    let mut entry = super::tests::runtime_entry_for_test(RUNTIME, &runtime);
    entry.identity_namespace.clear();
    let member = runtime
        .mob_handle()
        .list_members_observation_snapshot()
        .await
        .into_iter()
        .next()
        .ok_or("fixture member missing")?;
    let record = identity_record_for_member(&entry, &runtime.mob_handle(), &member)
        .await
        .ok_or("fixture identity missing")?;
    let session_id = record.session_id.clone().ok_or("fixture session missing")?;
    super::tests::commit_fixture_session_boundary(durable.as_ref(), &session_id, "committed").await;
    assert!(
        entry
            .runtime
            .session_boundary_authority(&session_id)
            .await
            .is_some(),
        "precondition: the session's committed boundary authority is readable"
    );
    aggregator
        .inner
        .runtimes
        .write()
        .map_err(|_| std::io::Error::other("runtime fixture lock"))?
        .insert(RUNTIME.into(), entry.clone());
    let target = SessionBackfillTarget {
        assistant_refresh: assistant_history_refresh::AssistantHistoryRefreshReason::Recovery,
        provenance: None,
        entry: entry.clone(),
        record: record.clone(),
        session_id: session_id.clone(),
    };
    let retry_key = (
        entry.registration_id,
        RUNTIME.to_string(),
        record.identity.clone(),
        session_id.clone(),
    );
    let pass = |gate: assistant_history_refresh::AssistantHistoryRefreshGate| {
        backfill_one_session_history_with_refresh_observer(
            aggregator.inner.clone(),
            target.clone(),
            false,
            move |_, _, _, _| Box::pin(std::future::ready(gate)),
        )
    };

    let reads = service.read_calls();
    service.script_history([super::tests::ScriptedHistoryRead {
        page: Some(history_page(&session_id, &[])?),
        gate: None,
    }]);
    pass(assistant_history_refresh::AssistantHistoryRefreshGate::Pending).await?;
    assert_eq!(
        service.read_calls(),
        reads + 1,
        "the first unsettled pass reads the current document"
    );

    for _ in 0..3 {
        pass(assistant_history_refresh::AssistantHistoryRefreshGate::Pending).await?;
    }
    assert_eq!(
        service.read_calls(),
        reads + 1,
        "unsettled passes over an unchanged committed boundary must not re-read the whole \
         document"
    );
    assert!(
        aggregator
            .inner
            .assistant_history_retries
            .lock()
            .map_err(|_| std::io::Error::other("fixture retry lock"))?
            .contains(&retry_key),
        "a skipped unsettled pass keeps the member on the retry list"
    );

    service.script_history([super::tests::ScriptedHistoryRead {
        page: Some(history_page(&session_id, &[])?),
        gate: None,
    }]);
    pass(assistant_history_refresh::AssistantHistoryRefreshGate::Settled).await?;
    assert_eq!(
        service.read_calls(),
        reads + 2,
        "the pass that observes the member settled still performs its read"
    );
    runtime.mob_handle().stop().await?;
    Ok(())
}

#[test]
fn assistant_identity_history_refreshes_after_extraction_compaction_and_truncation() {
    for kind in [
        "compaction_completed",
        "extraction_succeeded",
        "extraction_failed",
        "stream_truncated",
    ] {
        let mut frame = observed_live_frame(SESSION_A, kind, MESSAGE_A);
        frame.payload = json!({"session_id":SESSION_A});
        assert!(
            console_event_should_refresh_session_history(&frame),
            "{kind} must refresh complete history"
        );
    }
}

/// Wait until the assistant-history refresh gate reads `Settled` for the
/// member, re-reading only after one of its events or a machine-state change
/// (both subscribed before the first read). The deadline only bounds a broken
/// fixture and fails with a clear message.
#[allow(clippy::panic)]
async fn await_refresh_gate_settled(
    handle: &MobHandle,
    member: &AgentIdentity,
    entry: &RuntimeEntry,
    record: &ConsoleIdentityRecord,
    session_id: &str,
) {
    use futures::StreamExt as _;
    let mut member_events = handle
        .subscribe_agent_events(member)
        .await
        .unwrap_or_else(|error| panic!("subscribe to the fixture member's events: {error}"));
    let mut changes = handle.machine_state_changes();
    let settled = async {
        while assistant_history_refresh::observe(entry, record, session_id, true).await
            != assistant_history_refresh::AssistantHistoryRefreshGate::Settled
        {
            tokio::select! {
                event = member_events.next() => assert!(
                    event.is_some(),
                    "the fixture member's event stream ended before its refresh gate settled"
                ),
                changed = changes.changed() => assert!(
                    changed.is_ok(),
                    "the mob actor stopped before the fixture member's refresh gate settled"
                ),
            }
        }
    };
    assert!(
        tokio::time::timeout(Duration::from_secs(30), settled)
            .await
            .is_ok(),
        "the fixture member's refresh gate did not settle within 30 s"
    );
}

/// A recovery refresh whose gate read found no fresh typed status
/// (`StatusUnknown`: the bounded status read did not answer, as under
/// contention) while the member is idle stays on the retry list, and the
/// member's next typed change, not a discovery tick, re-drives it to
/// publication. The runtime is registered directly, so no discovery loop
/// runs: the re-drive is the only path that can publish.
#[tokio::test]
async fn status_unknown_refresh_is_redriven_by_the_next_typed_change_not_a_tick()
-> ConsoleLogResult<()> {
    let (_temp, runtime, service) = super::tests::build_stress_runtime(0, Duration::ZERO).await;
    // No initial message: no kickoff run, so the member is idle once seated.
    runtime
        .spawn(meerkat_mob::SpawnMemberSpec::from_wire(
            "worker".to_string(),
            "agent-0".to_string(),
            None,
            None,
            None,
        ))
        .await?;
    let store = Arc::new(InMemoryConsoleLogStore::new());
    let aggregator = MetaConsoleAggregator::new(store.clone());
    let mut entry = super::tests::runtime_entry_for_test(RUNTIME, &runtime);
    entry.identity_namespace.clear();
    let member = runtime
        .mob_handle()
        .list_members_observation_snapshot()
        .await
        .into_iter()
        .next()
        .ok_or("fixture member missing")?;
    let record = identity_record_for_member(&entry, &runtime.mob_handle(), &member)
        .await
        .ok_or("fixture identity missing")?;
    let session_id = record.session_id.clone().ok_or("fixture session missing")?;
    aggregator
        .inner
        .runtimes
        .write()
        .map_err(|_| std::io::Error::other("runtime fixture lock"))?
        .insert(RUNTIME.into(), entry.clone());
    let retry_key = (
        entry.registration_id,
        RUNTIME.to_string(),
        record.identity.clone(),
        session_id.clone(),
    );
    let target = SessionBackfillTarget {
        assistant_refresh: assistant_history_refresh::AssistantHistoryRefreshReason::Recovery,
        provenance: None,
        entry: entry.clone(),
        record: record.clone(),
        session_id: session_id.clone(),
    };
    // The seated member's session input commits just after spawn. Wait for
    // the real gate to read `Settled` first, so the only later typed change is
    // the one this test makes.
    await_refresh_gate_settled(
        &runtime.mob_handle(),
        &member.agent_identity,
        &entry,
        &record,
        &session_id,
    )
    .await;
    let snapshots = || async {
        store
            .query_frames(ConsoleTimelineQuery::default())
            .await
            .map(|rows| {
                rows.frames
                    .into_iter()
                    .filter(|frame| frame.kind == "assistant_history_snapshot")
                    .collect::<Vec<_>>()
            })
    };
    // The unsettled pass and every re-driven pass each read the document.
    let pages = (0..24)
        .map(|_| {
            history_page(&session_id, &[MESSAGE_A]).map(|page| super::tests::ScriptedHistoryRead {
                page: Some(page),
                gate: None,
            })
        })
        .collect::<ConsoleLogResult<Vec<_>>>()?;
    service.script_history(pages);

    backfill_one_session_history_with_refresh_observer(
        aggregator.inner.clone(),
        target,
        false,
        |_, _, _, _| {
            Box::pin(std::future::ready(
                assistant_history_refresh::AssistantHistoryRefreshGate::StatusUnknown,
            ))
        },
    )
    .await?;
    assert!(
        snapshots().await?.is_empty(),
        "an unsettled refresh publishes no assistant snapshot"
    );
    assert!(
        aggregator
            .inner
            .assistant_history_retries
            .lock()
            .map_err(|_| std::io::Error::other("fixture retry lock"))?
            .contains(&retry_key),
        "the status-unknown member stays on the retry list"
    );
    // Each typed change runs the armed re-drive. Its pass reads the real
    // gate: when that status read also fails to answer (as it may under
    // heavy load) the pass re-arms for the next typed change instead of
    // publishing, so the test makes another change until one re-driven pass
    // observes the member settled. No discovery loop runs here.
    let mut published = Vec::new();
    for seat in 1..=20 {
        let mut redrives = std::mem::take(
            &mut *aggregator
                .inner
                .status_unknown_redrive_tasks
                .lock()
                .map_err(|_| std::io::Error::other("fixture re-drive lock"))?,
        );
        assert_eq!(
            redrives.len(),
            1,
            "exactly one re-drive is armed for the retry key before typed change {seat}"
        );
        let redrive = redrives.remove(0);
        if seat == 1 {
            assert!(
                !redrive.is_finished(),
                "the re-drive waits for a typed change instead of firing at once"
            );
        }
        // The next typed change: another member is seated (a roster mutation
        // publishes machine state).
        runtime
            .spawn(meerkat_mob::SpawnMemberSpec::from_wire(
                "worker".to_string(),
                format!("agent-{seat}"),
                None,
                None,
                None,
            ))
            .await?;
        assert!(
            tokio::time::timeout(Duration::from_secs(30), redrive)
                .await
                .map_err(|_| std::io::Error::other(
                    "the re-drive did not complete within 30 s of the typed change"
                ))?
                .is_ok(),
            "the re-drive task completed"
        );
        published = snapshots().await?;
        if !published.is_empty() {
            break;
        }
    }
    assert_eq!(
        published.len(),
        1,
        "a typed change re-drove the retry to publication"
    );
    assert_eq!(
        published[0].payload["assistant_message_ids"],
        json!([MESSAGE_A])
    );
    assert!(
        !aggregator
            .inner
            .status_unknown_redrives
            .lock()
            .map_err(|_| std::io::Error::other("fixture re-drive lock"))?
            .contains(&retry_key),
        "a fired re-drive releases its key"
    );
    runtime.mob_handle().stop().await?;
    Ok(())
}

/// One seated, idle member whose session has a committed boundary authority
/// in the composition's durable runtime store, registered directly (no
/// discovery loop runs, so only the code under test can start a pass).
struct BoundaryFixture {
    _temp: tempfile::TempDir,
    runtime: Arc<UnifiedRuntime>,
    service: super::tests::DelayedHistorySessionService,
    durable: Arc<dyn meerkat_runtime::RuntimeStore>,
    aggregator: MetaConsoleAggregator,
    entry: RuntimeEntry,
    record: ConsoleIdentityRecord,
    session_id: String,
    retry_key: (uuid::Uuid, String, String, String),
}

impl BoundaryFixture {
    async fn seated() -> ConsoleLogResult<Self> {
        let durable: Arc<dyn meerkat_runtime::RuntimeStore> =
            Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());
        let (temp, runtime, service) =
            super::tests::build_stress_runtime_with_durable_runtime_store(
                0,
                Duration::ZERO,
                Some(durable.clone()),
            )
            .await;
        runtime
            .spawn(meerkat_mob::SpawnMemberSpec::from_wire(
                "worker".to_string(),
                "agent-0".to_string(),
                None,
                None,
                None,
            ))
            .await?;
        let aggregator = MetaConsoleAggregator::new(Arc::new(InMemoryConsoleLogStore::new()));
        let mut entry = super::tests::runtime_entry_for_test(RUNTIME, &runtime);
        entry.identity_namespace.clear();
        let member = runtime
            .mob_handle()
            .list_members_observation_snapshot()
            .await
            .into_iter()
            .next()
            .ok_or("fixture member missing")?;
        let record = identity_record_for_member(&entry, &runtime.mob_handle(), &member)
            .await
            .ok_or("fixture identity missing")?;
        let session_id = record.session_id.clone().ok_or("fixture session missing")?;
        super::tests::commit_fixture_session_boundary(durable.as_ref(), &session_id, "boundary 1")
            .await;
        assert!(
            entry
                .runtime
                .session_boundary_authority(&session_id)
                .await
                .is_some(),
            "precondition: the session's committed boundary authority is readable"
        );
        aggregator
            .inner
            .runtimes
            .write()
            .map_err(|_| std::io::Error::other("runtime fixture lock"))?
            .insert(RUNTIME.into(), entry.clone());
        await_refresh_gate_settled(
            &runtime.mob_handle(),
            &member.agent_identity,
            &entry,
            &record,
            &session_id,
        )
        .await;
        let retry_key = (
            entry.registration_id,
            RUNTIME.to_string(),
            record.identity.clone(),
            session_id.clone(),
        );
        Ok(Self {
            _temp: temp,
            runtime,
            service,
            durable,
            aggregator,
            entry,
            record,
            session_id,
            retry_key,
        })
    }

    fn target(&self) -> SessionBackfillTarget {
        SessionBackfillTarget {
            assistant_refresh: assistant_history_refresh::AssistantHistoryRefreshReason::Recovery,
            provenance: None,
            entry: self.entry.clone(),
            record: self.record.clone(),
            session_id: self.session_id.clone(),
        }
    }

    /// Commit the session's next boundary: its committed authority moves.
    async fn commit_boundary(&self, note: &str) {
        super::tests::commit_fixture_session_boundary(
            self.durable.as_ref(),
            &self.session_id,
            note,
        )
        .await;
    }

    fn take_tasks(
        tasks: &std::sync::Mutex<Vec<tokio::task::JoinHandle<()>>>,
    ) -> ConsoleLogResult<Vec<tokio::task::JoinHandle<()>>> {
        Ok(std::mem::take(
            &mut *tasks
                .lock()
                .map_err(|_| std::io::Error::other("fixture task lock"))?,
        ))
    }
}

/// Downstream app and idle_cpu_gate shape: a recovery refresh lands while the
/// member is idle between runs of a queued burst, its later inputs and their
/// commits still landing. The gate reads `Draining`: the pass skips its
/// whole-document read, keeps the retry, and arms one re-drive. That re-drive
/// waits for the member's next typed change, not a discovery tick, and the
/// re-driven refresh reads.
#[tokio::test]
async fn draining_refresh_skips_its_read_and_is_redriven_by_the_next_typed_change()
-> ConsoleLogResult<()> {
    let fixture = BoundaryFixture::seated().await?;
    let inner = fixture.aggregator.inner.clone();
    let pages = (0..24)
        .map(|_| {
            history_page(&fixture.session_id, &[MESSAGE_A]).map(|page| {
                super::tests::ScriptedHistoryRead {
                    page: Some(page),
                    gate: None,
                }
            })
        })
        .collect::<ConsoleLogResult<Vec<_>>>()?;
    fixture.service.script_history(pages);
    let reads = fixture.service.read_calls();

    backfill_one_session_history_with_refresh_observer(
        inner.clone(),
        fixture.target(),
        false,
        |_, _, _, _| {
            Box::pin(std::future::ready(
                assistant_history_refresh::AssistantHistoryRefreshGate::Draining,
            ))
        },
    )
    .await?;
    assert_eq!(
        fixture.service.read_calls(),
        reads,
        "a draining pass skips its whole-document read"
    );
    assert!(
        inner
            .assistant_history_retries
            .lock()
            .map_err(|_| std::io::Error::other("fixture retry lock"))?
            .contains(&fixture.retry_key),
        "the draining member stays on the retry list"
    );
    // Each typed change runs the armed re-drive. Its pass reads the real
    // gate: when the bounded status read does not answer (as it may under
    // heavy load) the pass re-arms for the next typed change instead of
    // reading, so the test makes another change until a re-driven pass reads.
    for seat in 1..=20 {
        let mut redrives = BoundaryFixture::take_tasks(&inner.status_unknown_redrive_tasks)?;
        assert_eq!(
            redrives.len(),
            1,
            "exactly one re-drive is armed for the retry key before typed change {seat}"
        );
        let redrive = redrives.remove(0);
        if seat == 1 {
            assert!(
                !redrive.is_finished(),
                "the re-drive waits for a typed change instead of firing at once"
            );
            assert_eq!(
                fixture.service.read_calls(),
                reads,
                "no read before the member's next typed change"
            );
        }
        // The next typed change: another member is seated (a roster mutation
        // publishes machine state).
        fixture
            .runtime
            .spawn(meerkat_mob::SpawnMemberSpec::from_wire(
                "worker".to_string(),
                format!("agent-{seat}"),
                None,
                None,
                None,
            ))
            .await?;
        tokio::time::timeout(Duration::from_secs(30), redrive)
            .await
            .map_err(|_| {
                std::io::Error::other("the re-drive did not complete within 30 s of the change")
            })?
            .map_err(|error| std::io::Error::other(format!("re-drive task: {error}")))?;
        if fixture.service.read_calls() > reads {
            break;
        }
    }
    assert_eq!(
        fixture.service.read_calls(),
        reads + 1,
        "a typed change re-drove the refresh to exactly one read"
    );
    assert!(
        !inner
            .status_unknown_redrives
            .lock()
            .map_err(|_| std::io::Error::other("fixture re-drive lock"))?
            .contains(&fixture.retry_key),
        "the fired re-drive disarms"
    );
    fixture.runtime.mob_handle().stop().await?;
    Ok(())
}

/// A pass whose session's committed boundary moves during its read verified
/// an image already behind the durable document. It re-drives the session
/// exactly once, at once, instead of waiting for a discovery tick and
/// watermark expiry. The re-driven pass ends at the boundary it read and
/// queues nothing, and a later pass over that unchanged boundary skips its
/// read: no loop.
#[tokio::test]
async fn a_boundary_that_moves_mid_read_redrives_exactly_once() -> ConsoleLogResult<()> {
    let fixture = BoundaryFixture::seated().await?;
    let inner = fixture.aggregator.inner.clone();
    let gate = super::tests::HistoryReadGate::new();
    fixture.service.script_history([
        super::tests::ScriptedHistoryRead {
            page: Some(history_page(&fixture.session_id, &[MESSAGE_A])?),
            gate: Some(gate.clone()),
        },
        super::tests::ScriptedHistoryRead {
            page: Some(history_page(&fixture.session_id, &[MESSAGE_A])?),
            gate: None,
        },
    ]);
    let reads = fixture.service.read_calls();
    let pass = tokio::spawn(backfill_one_session_history_with_refresh_observer(
        inner.clone(),
        fixture.target(),
        false,
        |_, _, _, _| {
            Box::pin(std::future::ready(
                assistant_history_refresh::AssistantHistoryRefreshGate::Settled,
            ))
        },
    ));
    gate.wait_until_entered().await;
    // A new boundary commits while the pass reads.
    fixture.commit_boundary("boundary 2").await;
    gate.release();
    pass.await
        .map_err(|error| std::io::Error::other(format!("pass task: {error}")))??;
    // The re-drive is spawned as the pass ends and may already be reading.
    let mut redrives = BoundaryFixture::take_tasks(&inner.moved_boundary_redrive_tasks)?;
    assert_eq!(
        redrives.len(),
        1,
        "the moved boundary re-drives the session exactly once"
    );
    tokio::time::timeout(Duration::from_secs(30), redrives.remove(0))
        .await
        .map_err(|_| std::io::Error::other("the moved-boundary re-drive did not finish"))?
        .map_err(|error| std::io::Error::other(format!("moved-boundary re-drive task: {error}")))?;
    assert_eq!(
        fixture.service.read_calls(),
        reads + 2,
        "one read by the pass and one by the re-drive at the new boundary"
    );
    assert!(
        BoundaryFixture::take_tasks(&inner.moved_boundary_redrive_tasks)?.is_empty(),
        "a pass that ends at the boundary it read queues no further re-drive"
    );

    // Converged: an ordinary pass over the unchanged boundary reads nothing.
    let mut positive = fixture.target();
    positive.assistant_refresh =
        assistant_history_refresh::AssistantHistoryRefreshReason::PositiveOnly;
    backfill_one_session_history(inner.clone(), positive, false).await?;
    assert_eq!(
        fixture.service.read_calls(),
        reads + 2,
        "an unchanged boundary is not read again"
    );
    assert!(BoundaryFixture::take_tasks(&inner.moved_boundary_redrive_tasks)?.is_empty());
    fixture.runtime.mob_handle().stop().await?;
    Ok(())
}

/// Liveness: a member can leave the draining state without its queue ever
/// draining, for example its mob is stopped with inputs still active. The
/// re-drive also wakes on the mob's lifecycle change and hands the decision
/// back to the refresh gate, which reads what is durable, so the session's
/// history is still read and no waiter stays armed: the console converges.
#[tokio::test]
async fn a_member_stopped_while_draining_is_refreshed_and_converges() -> ConsoleLogResult<()> {
    let fixture = BoundaryFixture::seated().await?;
    let inner = fixture.aggregator.inner.clone();
    fixture
        .service
        .script_history([super::tests::ScriptedHistoryRead {
            page: Some(history_page(&fixture.session_id, &[MESSAGE_A])?),
            gate: None,
        }]);
    let reads = fixture.service.read_calls();

    backfill_one_session_history_with_refresh_observer(
        inner.clone(),
        fixture.target(),
        false,
        |_, _, _, _| {
            Box::pin(std::future::ready(
                assistant_history_refresh::AssistantHistoryRefreshGate::Draining,
            ))
        },
    )
    .await?;
    let mut redrives = BoundaryFixture::take_tasks(&inner.status_unknown_redrive_tasks)?;
    assert_eq!(redrives.len(), 1, "exactly one re-drive is armed");
    let redrive = redrives.remove(0);
    assert!(
        !redrive.is_finished(),
        "the re-drive waits for the member's next typed change"
    );
    assert_eq!(
        fixture.service.read_calls(),
        reads,
        "no read while draining"
    );

    // The mob stops with the inputs still active: no drain will ever come.
    fixture.runtime.mob_handle().stop().await?;
    tokio::time::timeout(Duration::from_secs(30), redrive)
        .await
        .map_err(|_| std::io::Error::other("the re-drive did not fire after the stop"))?
        .map_err(|error| std::io::Error::other(format!("re-drive task: {error}")))?;
    assert_eq!(
        fixture.service.read_calls(),
        reads + 1,
        "the stopped member's durable history is read once"
    );
    assert!(
        !inner
            .status_unknown_redrives
            .lock()
            .map_err(|_| std::io::Error::other("fixture re-drive lock"))?
            .contains(&fixture.retry_key),
        "no waiter stays armed for the stopped member"
    );
    tokio::time::timeout(
        Duration::from_secs(30),
        fixture.aggregator.history_backfill_converged(),
    )
    .await
    .map_err(|_| std::io::Error::other("the console did not converge after the stop"))?;
    Ok(())
}

/// A refresh that lands while a run is open on the session (`Running`) still
/// restores positive frames with one read, as before, and also arms a
/// re-drive on the member's next typed change, so the settled refresh follows
/// the run's end instead of a discovery tick (idle_cpu_gate caught the tick
/// doing that read inside its measured window).
#[tokio::test]
async fn running_refresh_reads_once_and_arms_a_typed_change_redrive() -> ConsoleLogResult<()> {
    let fixture = BoundaryFixture::seated().await?;
    let inner = fixture.aggregator.inner.clone();
    fixture
        .service
        .script_history([super::tests::ScriptedHistoryRead {
            page: Some(history_page(&fixture.session_id, &[MESSAGE_A])?),
            gate: None,
        }]);
    let reads = fixture.service.read_calls();
    backfill_one_session_history_with_refresh_observer(
        inner.clone(),
        fixture.target(),
        false,
        |_, _, _, _| {
            Box::pin(std::future::ready(
                assistant_history_refresh::AssistantHistoryRefreshGate::Running,
            ))
        },
    )
    .await?;
    assert_eq!(
        fixture.service.read_calls(),
        reads + 1,
        "a running pass still restores positive frames"
    );
    let redrives = BoundaryFixture::take_tasks(&inner.status_unknown_redrive_tasks)?;
    assert_eq!(redrives.len(), 1, "and arms exactly one re-drive");
    assert!(
        !redrives[0].is_finished(),
        "which waits for the member's next typed change"
    );
    fixture.runtime.mob_handle().stop().await?;
    Ok(())
}
