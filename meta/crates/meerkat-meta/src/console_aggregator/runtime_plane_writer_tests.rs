//! Member-attributed console frames never carry a runtime-plane identity:
//! the aggregator's frame writers refuse `_system` and `__console__` unless
//! the frame is the runtime's own (a source-gap marker, or a runtime event
//! the runtime's event store accepted under the system identity). The
//! session-history backfill drops a frame it derives under one and goes on
//! with the rest of the member's history; a member that itself holds one
//! (created before every creation path refused them) gets no history pass
//! from any target source.
#![allow(clippy::expect_used)]

use meerkat_core::types::SessionId;
use serde_json::json;

use super::*;

#[tokio::test]
async fn member_attributed_frames_refuse_runtime_plane_identities() {
    let aggregator = MetaConsoleAggregator::in_memory();
    for identity in ["_system", "__console__"] {
        let frame = frame_from_session_history_message(
            "runtime-a",
            identity,
            "session",
            0,
            json!({"role": "user", "content": "member history"}),
        )
        .expect("frame");
        let appended = append_and_emit(&aggregator.inner, frame).await;
        assert!(
            appended.is_err(),
            "a member frame under {identity} is refused"
        );
    }
    assert!(
        aggregator
            .inner
            .store
            .latest_cursor()
            .await
            .expect("latest cursor")
            .is_none(),
        "nothing was stored"
    );
}

#[tokio::test]
async fn console_sends_refuse_runtime_plane_identities() {
    let aggregator = MetaConsoleAggregator::in_memory();
    for identity in ["_system", "__console__"] {
        let request: ConsoleSendRequest = serde_json::from_value(json!({
            "identity": identity,
            "content": "hello",
            "origin": "operator",
            "idempotency_key": format!("send-{identity}"),
        }))
        .expect("send request");
        let refused = aggregator.send(request).await;
        assert!(
            matches!(&refused, Err(ConsoleSendError::InvalidRequest(message)) if message.contains("reserved")),
            "a send to {identity} is refused as reserved before any member lookup"
        );
    }
}

const HISTORY_RUNTIME: &str = "runtime-plane-history";
const HISTORY_MEMBER: &str = "history-worker";
const LATER_TEXT: &str = "Work goes on after the spawn call.";

/// A member's transcript: an assistant step whose one tool call is `tool`
/// with `args`, then a later assistant message.
fn spawn_call_then_later_message(tool: &str, args: Value) -> ConsoleLogResult<Vec<Message>> {
    Ok(vec![
        serde_json::from_value(json!({
            "role": "block_assistant",
            "blocks": [{
                "block_type": "tool_use",
                "data": { "id": "call-spawn", "name": tool, "args": args },
            }],
            "stop_reason": "tool_use",
            "created_at": "1970-01-01T00:00:00.100Z",
        }))?,
        serde_json::from_value(json!({
            "role": "block_assistant",
            "blocks": [{ "block_type": "text", "data": { "text": LATER_TEXT } }],
            "stop_reason": "end_turn",
            "created_at": "1970-01-01T00:00:00.200Z",
        }))?,
    ])
}

/// Backfill `history` as the session of `HISTORY_MEMBER` on a fresh
/// aggregator that registers the runtime with the empty identity namespace,
/// as the default runtime registers. Returns the aggregator, the session id
/// and what the backfill returned.
async fn backfill_member_history(
    runtime: &UnifiedRuntime,
    service: &super::tests::DelayedHistorySessionService,
    history: &[Message],
) -> ConsoleLogResult<(MetaConsoleAggregator, String, ConsoleLogResult<()>)> {
    let aggregator = MetaConsoleAggregator::in_memory();
    let mut entry = super::tests::runtime_entry_for_test(HISTORY_RUNTIME, runtime);
    entry.identity_namespace.clear();
    let session = SessionId::new();
    let mut record = super::tests::identity_record_for_test(HISTORY_MEMBER);
    record.runtime_key = HISTORY_RUNTIME.into();
    record.session_id = Some(session.to_string());
    aggregator
        .inner
        .runtimes
        .write()
        .map_err(|_| std::io::Error::other("runtime registration lock poisoned"))?
        .insert(HISTORY_RUNTIME.into(), entry.clone());
    service.script_history([super::tests::ScriptedHistoryRead {
        page: Some(meerkat_core::service::SessionHistoryPage::from_messages(
            session.clone(),
            history,
            meerkat_core::service::SessionHistoryQuery::default(),
        )),
        gate: None,
    }]);
    let backfill = backfill_one_session_history(
        aggregator.inner.clone(),
        SessionBackfillTarget {
            assistant_refresh: AssistantHistoryRefreshReason::PositiveOnly,
            provenance: None,
            entry,
            record,
            session_id: session.to_string(),
        },
        true,
    )
    .await;
    Ok((aggregator, session.to_string(), backfill))
}

fn is_later_message(frame: &ConsoleFrame) -> bool {
    frame.identity == HISTORY_MEMBER
        && frame.kind == "text_complete"
        && frame.payload["text"] == LATER_TEXT
}

fn is_member_spawn_call(frame: &ConsoleFrame) -> bool {
    frame.identity == HISTORY_MEMBER
        && frame.kind == "tool_call_requested"
        && frame.payload["tool_call_id"] == "call-spawn"
}

/// One spawn call in the member's transcript and the identity the history
/// projection derives its initial-message frame under.
struct SpawnCallRow {
    label: &'static str,
    tool: &'static str,
    args: Value,
    target: &'static str,
}

/// What a backfill of `row`'s transcript left in the console; `Err` names
/// the first expectation that does not hold.
async fn check_spawn_call_row(
    runtime: &UnifiedRuntime,
    service: &super::tests::DelayedHistorySessionService,
    row: &SpawnCallRow,
) -> Result<(), String> {
    let history =
        spawn_call_then_later_message(row.tool, row.args.clone()).map_err(|err| err.to_string())?;
    let (aggregator, session, backfill) = backfill_member_history(runtime, service, &history)
        .await
        .map_err(|err| err.to_string())?;
    backfill.map_err(|err| format!("the backfill failed: {err}"))?;
    let frames = aggregator
        .inner
        .store
        .query_frames(ConsoleTimelineQuery {
            limit: 100,
            ..Default::default()
        })
        .await
        .map_err(|err| err.to_string())?
        .frames;
    if let Some(frame) = frames
        .iter()
        .find(|frame| crate::console_contracts::is_runtime_plane_identity(&frame.identity))
    {
        return Err(format!(
            "a {} frame is stored under {}",
            frame.kind, frame.identity
        ));
    }
    if !crate::console_contracts::is_runtime_plane_identity(row.target)
        && !frames.iter().any(|frame| {
            frame.identity == row.target
                && frame.kind == "user_input"
                && frame.payload["tool_call_id"] == "call-spawn"
        })
    {
        return Err(format!(
            "the initial message for {} is not stored",
            row.target
        ));
    }
    if !frames.iter().any(is_member_spawn_call) {
        return Err("the member's own spawn call frame is not stored".into());
    }
    if !frames.iter().any(is_later_message) {
        return Err("the later message's frame is not stored".into());
    }
    let watermark = aggregator
        .inner
        .store
        .source_watermark(
            &session_history_watermark_runtime_key(HISTORY_RUNTIME, &session),
            ConsoleFrameSourceKind::SessionHistory,
        )
        .await
        .map_err(|err| err.to_string())?;
    let reached = watermark
        .as_deref()
        .and_then(|watermark| parse_session_history_watermark(watermark, &session));
    if reached != Some(history.len()) {
        return Err(format!(
            "the watermark stands at {reached:?}, not at the end ({})",
            history.len()
        ));
    }
    Ok(())
}

#[tokio::test]
async fn history_backfill_skips_frames_under_runtime_plane_identities_and_goes_on()
-> ConsoleLogResult<()> {
    let (_temp, runtime, service) = super::tests::build_stress_runtime(0, Duration::ZERO).await;
    let rows = [
        SpawnCallRow {
            label: "control: spawn_member names an ordinary member",
            tool: "spawn_member",
            args: json!({"member_id": "helper-b", "initial_message": "x"}),
            target: "helper-b",
        },
        SpawnCallRow {
            label: "spawn_member member_id _system",
            tool: "spawn_member",
            args: json!({"member_id": "_system", "initial_message": "x"}),
            target: "_system",
        },
        SpawnCallRow {
            label: "spawn_member member_id __console__",
            tool: "spawn_member",
            args: json!({"member_id": "__console__", "initial_message": "x"}),
            target: "__console__",
        },
        SpawnCallRow {
            label: "spawn_member agent_identity _system",
            tool: "spawn_member",
            args: json!({"agent_identity": "_system", "initial_message": "x"}),
            target: "_system",
        },
        SpawnCallRow {
            label: "mob_spawn_member member_id _system",
            tool: "mob_spawn_member",
            args: json!({"member_id": "_system", "initial_message": "x"}),
            target: "_system",
        },
        SpawnCallRow {
            label: "mob_spawn_member specs agent_identity __console__ with a task",
            tool: "mob_spawn_member",
            args: json!({"specs": [{"agent_identity": "__console__", "task": "x"}]}),
            target: "__console__",
        },
    ];
    let mut failures = Vec::new();
    for row in &rows {
        if let Err(failure) = check_spawn_call_row(&runtime, &service, row).await {
            failures.push(format!("{}: {failure}", row.label));
        }
    }
    let _ = runtime.mob_handle().stop().await;
    assert!(failures.is_empty(), "rows that failed: {failures:#?}");
    Ok(())
}

#[tokio::test]
async fn an_operator_sees_a_members_history_after_a_spawn_call_naming_a_runtime_plane_identity()
-> ConsoleLogResult<()> {
    let (_temp, runtime, service) = super::tests::build_stress_runtime(0, Duration::ZERO).await;
    let history = spawn_call_then_later_message(
        "spawn_member",
        json!({"member_id": "_system", "initial_message": "x"}),
    )?;
    let (aggregator, _session, backfill) =
        backfill_member_history(&runtime, &service, &history).await?;
    let backfill = backfill.map_err(|err| err.to_string());
    let operator = aggregator.policy_view(Arc::new(AllowAllConsoleVisibilityPolicy));
    let member_view = operator
        .query_timeline(ConsoleTimelineQuery {
            identity: Some(HISTORY_MEMBER.into()),
            limit: 100,
            ..Default::default()
        })
        .await?
        .frames;
    let global_view = operator
        .query_timeline(ConsoleTimelineQuery {
            limit: 100,
            ..Default::default()
        })
        .await?
        .frames;
    let _ = runtime.mob_handle().stop().await;
    assert_eq!(backfill, Ok(()), "the member's backfill completes");
    assert!(
        member_view.iter().any(is_member_spawn_call),
        "the operator sees the member's spawn call: {member_view:#?}"
    );
    assert!(
        member_view.iter().any(is_later_message),
        "the operator sees what the member did after the spawn call: {member_view:#?}"
    );
    assert!(
        global_view
            .iter()
            .all(|frame| !crate::console_contracts::is_runtime_plane_identity(&frame.identity)),
        "the operator sees no member frame under a runtime-plane identity: {global_view:#?}"
    );
    Ok(())
}

/// A transcript whose pass publishes both history snapshots: a runtime
/// notice and an assistant message that carries an assistant message id.
fn snapshot_history(session: &SessionId) -> ConsoleLogResult<Vec<Message>> {
    let mut notice = meerkat_core::types::SystemNoticeMessage::new(
        meerkat_core::types::SystemNoticeKind::Generic,
        "Runtime notice.",
    );
    notice.runtime_origin = Some(meerkat_core::types::RuntimeAppendOrigin {
        session_id: session.clone(),
        run_id: meerkat_core::lifecycle::RunId(uuid::Uuid::from_u128(10)),
        input_id: meerkat_core::lifecycle::InputId(uuid::Uuid::from_u128(20)),
        append_ordinal: 0,
    });
    Ok(vec![
        Message::SystemNotice(notice),
        serde_json::from_value(json!({
            "role": "block_assistant",
            "blocks": [{ "block_type": "text", "data": { "text": "Legacy answer." } }],
            "stop_reason": "end_turn",
            "created_at": "1970-01-01T00:00:00.300Z",
            "assistant_message_id": "00000000-0000-4000-8000-000000000401",
        }))?,
    ])
}

fn script_page(
    service: &super::tests::DelayedHistorySessionService,
    session: &SessionId,
    history: &[Message],
) {
    service.script_history([super::tests::ScriptedHistoryRead {
        page: Some(meerkat_core::service::SessionHistoryPage::from_messages(
            session.clone(),
            history,
            meerkat_core::service::SessionHistoryQuery::default(),
        )),
        gate: None,
    }]);
}

async fn backfills_converged(aggregator: &MetaConsoleAggregator) -> Result<(), String> {
    tokio::time::timeout(
        Duration::from_secs(20),
        aggregator.history_backfill_converged(),
    )
    .await
    .map_err(|_| "the history backfills did not converge".to_string())
}

/// Records a failure when the source `what` names read the history since
/// `before`.
fn note_history_reads(
    failures: &mut Vec<String>,
    service: &super::tests::DelayedHistorySessionService,
    before: usize,
    what: &str,
) {
    let reads = service.read_calls() - before;
    if reads != 0 {
        failures.push(format!("{what} read its history {reads} time(s)"));
    }
}

/// A member created under a runtime-plane identity before every creation
/// path refused them gets no history pass from any target source: the
/// identity list, the targeted runner an identity inspection spawns, and a
/// settled pass all return before reading its history, as discovery does.
#[tokio::test]
async fn every_history_target_source_skips_a_legacy_member_under_a_runtime_plane_identity()
-> ConsoleLogResult<()> {
    let (_temp, runtime, service) = super::tests::build_stress_runtime(0, Duration::ZERO).await;
    let aggregator = MetaConsoleAggregator::in_memory();
    let mut entry = super::tests::runtime_entry_for_test(HISTORY_RUNTIME, &runtime);
    entry.identity_namespace.clear();
    aggregator
        .inner
        .runtimes
        .write()
        .map_err(|_| std::io::Error::other("runtime registration lock poisoned"))?
        .insert(HISTORY_RUNTIME.into(), entry.clone());
    let mut failures = Vec::new();

    // An ordinary member in the same runtime, through the identity list.
    let ordinary_session = SessionId::new();
    let mut ordinary = super::tests::identity_record_for_test(HISTORY_MEMBER);
    ordinary.runtime_key = HISTORY_RUNTIME.into();
    ordinary.session_id = Some(ordinary_session.to_string());
    let ordinary_history = spawn_call_then_later_message(
        "spawn_member",
        json!({"member_id": "helper-b", "initial_message": "x"}),
    )?;
    script_page(&service, &ordinary_session, &ordinary_history);
    spawn_identity_backfills_for_records(aggregator.inner.clone(), std::slice::from_ref(&ordinary));
    if let Err(err) = backfills_converged(&aggregator).await {
        failures.push(format!("{HISTORY_MEMBER}: {err}"));
    }

    for reserved in ["_system", "__console__"] {
        let session = SessionId::new();
        let mut record = super::tests::identity_record_for_test(reserved);
        record.runtime_key = HISTORY_RUNTIME.into();
        record.session_id = Some(session.to_string());
        let history = snapshot_history(&session)?;
        let target = SessionBackfillTarget {
            assistant_refresh: AssistantHistoryRefreshReason::Recovery,
            provenance: None,
            entry: entry.clone(),
            record: record.clone(),
            session_id: session.to_string(),
        };

        // The identity list (`list_identities`) spawns a backfill per record.
        let before = service.read_calls();
        script_page(&service, &session, &history);
        spawn_identity_backfills_for_records(
            aggregator.inner.clone(),
            std::slice::from_ref(&record),
        );
        if let Err(err) = backfills_converged(&aggregator).await {
            failures.push(format!("{reserved}: {err}"));
        }
        note_history_reads(
            &mut failures,
            &service,
            before,
            &format!("{reserved}: the identity list"),
        );

        // The targeted runner an identity inspection spawns.
        let before = service.read_calls();
        script_page(&service, &session, &history);
        if let Err(err) =
            run_targeted_session_history_backfill(aggregator.inner.clone(), target.clone(), true)
                .await
        {
            failures.push(format!("{reserved}: the targeted backfill failed: {err}"));
        }
        note_history_reads(
            &mut failures,
            &service,
            before,
            &format!("{reserved}: the targeted backfill"),
        );

        // A settled pass, which would publish both history snapshots.
        let before = service.read_calls();
        script_page(&service, &session, &history);
        if let Err(err) = backfill_one_session_history_with_refresh_observer(
            aggregator.inner.clone(),
            target,
            true,
            |_, _, _, _| {
                Box::pin(std::future::ready(
                    assistant_history_refresh::AssistantHistoryRefreshGate::Settled,
                ))
            },
        )
        .await
        {
            failures.push(format!("{reserved}: the settled pass failed: {err}"));
        }
        note_history_reads(
            &mut failures,
            &service,
            before,
            &format!("{reserved}: the settled pass"),
        );
    }

    let frames = aggregator
        .inner
        .store
        .query_frames(ConsoleTimelineQuery {
            limit: 200,
            ..Default::default()
        })
        .await?
        .frames;
    let _ = runtime.mob_handle().stop().await;
    for frame in frames
        .iter()
        .filter(|frame| crate::console_contracts::is_runtime_plane_identity(&frame.identity))
    {
        failures.push(format!(
            "a {} frame is stored under {}",
            frame.kind, frame.identity
        ));
    }
    if !frames.iter().any(is_later_message) {
        failures.push("the ordinary member's history is not stored".into());
    }
    assert!(failures.is_empty(), "failures: {failures:#?}");
    Ok(())
}
