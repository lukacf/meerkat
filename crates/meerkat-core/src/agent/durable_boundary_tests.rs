//! Durable in-turn boundary delivery through the real agent loop.
//!
//! A durable Steer delivery joins the RUNNING turn: the runner writes its
//! typed appends into the Session at the next exact `CallingLlm` boundary, so
//! that request and every later request of the same run see them, and the run
//! commits them like tool results. A request-only delivery is still visible to
//! exactly one request and never becomes Session state.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use crate as meerkat_core;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use meerkat_core::{
    AgentBuilder, AgentError, AgentEvent, AgentLlmClient, AgentSessionStore, AgentToolDispatcher,
    AssistantBlock, LlmStreamResult, Message, StopReason, ToolCallView, ToolDef, ToolResult,
    TurnUsage, Usage,
};
use serde_json::value::RawValue;
use tokio::sync::{Notify, mpsc};

const NOTICE_TOKEN: &str = "BG-DONE-7Q";
const REQUEST_ONLY_TOKEN: &str = "PEER-STEER-3K";

struct RecordingClient {
    requests: Mutex<Vec<Vec<Message>>>,
    next: AtomicUsize,
}

impl RecordingClient {
    fn new() -> Self {
        Self {
            requests: Mutex::new(Vec::new()),
            next: AtomicUsize::new(0),
        }
    }

    fn requests(&self) -> Vec<Vec<Message>> {
        self.requests.lock().unwrap().clone()
    }
}

fn tool_use(id: &str, name: &str) -> LlmStreamResult {
    LlmStreamResult::new(
        vec![AssistantBlock::ToolUse {
            id: id.to_string(),
            name: name.into(),
            args: RawValue::from_string("{}".to_string()).unwrap(),
            meta: None,
        }],
        StopReason::ToolUse,
        TurnUsage::host_declared(
            meerkat_core::Provider::Other,
            "mock-model",
            Usage::default(),
        )
        .into_inner(),
    )
}

#[async_trait]
impl AgentLlmClient for RecordingClient {
    async fn stream_response(
        &self,
        messages: &[Message],
        _tools: &[Arc<ToolDef>],
        _max_tokens: u32,
        _temperature: Option<f32>,
        _provider_params: Option<&meerkat_core::lifecycle::run_primitive::ProviderParamsOverride>,
    ) -> Result<LlmStreamResult, AgentError> {
        self.requests.lock().unwrap().push(messages.to_vec());
        let call = self.next.fetch_add(1, Ordering::SeqCst);
        Ok(match call {
            0 => tool_use("call-block", "probe_block"),
            1 => tool_use("call-step", "probe_step"),
            _ => LlmStreamResult::new(
                vec![AssistantBlock::Text {
                    text: "done".to_string(),
                    meta: None,
                }],
                StopReason::EndTurn,
                TurnUsage::host_declared(
                    meerkat_core::Provider::Other,
                    "mock-model",
                    Usage::default(),
                )
                .into_inner(),
            ),
        })
    }

    fn provider(&self) -> meerkat_core::Provider {
        meerkat_core::Provider::Other
    }

    fn model(&self) -> &'static str {
        "mock-model"
    }
}

/// `probe_block` parks until the test releases it, so the post-tool model
/// boundary is open while a delivery is prepared.
struct GatedTools {
    tools: Arc<[Arc<ToolDef>]>,
    entered: Arc<Notify>,
    release: Arc<Notify>,
}

impl GatedTools {
    fn new(entered: Arc<Notify>, release: Arc<Notify>) -> Self {
        let schema = serde_json::json!({ "type": "object" });
        Self {
            tools: Arc::from([
                Arc::new(ToolDef::new("probe_block", "blocks", schema.clone())),
                Arc::new(ToolDef::new("probe_step", "returns", schema)),
            ]),
            entered,
            release,
        }
    }
}

#[async_trait]
impl AgentToolDispatcher for GatedTools {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::clone(&self.tools)
    }

    async fn dispatch(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<meerkat_core::ToolDispatchOutcome, meerkat_core::ToolError> {
        if call.name == "probe_block" {
            self.entered.notify_one();
            self.release.notified().await;
        }
        Ok(ToolResult::new(call.id.to_string(), format!("{} ok", call.name), false).into())
    }
}

struct NoopStore;

#[async_trait]
impl AgentSessionStore for NoopStore {
    async fn save(&self, _session: &meerkat_core::Session) -> Result<(), AgentError> {
        Ok(())
    }

    async fn load(&self, _id: &str) -> Result<Option<meerkat_core::Session>, AgentError> {
        Ok(None)
    }
}

async fn gated_agent(
    client: Arc<RecordingClient>,
    entered: Arc<Notify>,
    release: Arc<Notify>,
) -> meerkat_core::Agent<RecordingClient, GatedTools, NoopStore> {
    let mut session = meerkat_core::Session::new();
    session
        .set_build_state(meerkat_core::SessionBuildState::default())
        .expect("test session build state should serialize");
    AgentBuilder::new()
        .resume_session(session)
        .with_turn_state_handle(Arc::new(
            crate::agent::test_turn_state_handle::TestTurnStateHandle::new(),
        ))
        .with_runtime_execution_kind_for_test(
            meerkat_core::lifecycle::RuntimeExecutionKind::ContentTurn,
        )
        .build_standalone(
            client,
            Arc::new(GatedTools::new(entered, release)),
            Arc::new(NoopStore),
        )
        .await
}

fn durable_notice() -> meerkat_core::lifecycle::TurnBoundaryDelivery {
    meerkat_core::lifecycle::TurnBoundaryDelivery::DurableAppends(
        meerkat_core::lifecycle::DurableTurnBoundaryAppends::try_new(
            meerkat_core::lifecycle::InputId::new(),
            vec![meerkat_core::lifecycle::ConversationAppend {
                runtime_source: None,
                role: meerkat_core::lifecycle::ConversationAppendRole::SystemNotice,
                content: meerkat_core::lifecycle::CoreRenderable::SystemNotice {
                    kind: meerkat_core::types::SystemNoticeKind::Generic,
                    body: Some(NOTICE_TOKEN.to_string()),
                    blocks: Vec::new(),
                },
                identity: None,
            }],
            None,
        )
        .expect("eligible durable notice"),
    )
}

fn carries_notice(message: &Message) -> bool {
    matches!(message, Message::SystemNotice(notice)
        if notice.body.as_deref() == Some(NOTICE_TOKEN))
}

fn mentions(messages: &[Message], token: &str) -> bool {
    messages.iter().any(|message| match message {
        Message::User(user) => user.text_content().contains(token),
        Message::SystemNotice(notice) => notice.body.as_deref() == Some(token),
        _ => false,
    })
}

async fn wait_until(what: &str, mut ready: impl FnMut() -> bool) {
    for _ in 0..2_000 {
        if ready() {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("{what} did not happen");
}

#[tokio::test]
async fn durable_boundary_appends_join_the_running_turn_and_every_later_request() {
    let client = Arc::new(RecordingClient::new());
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let mut agent = gated_agent(
        Arc::clone(&client),
        Arc::clone(&entered),
        Arc::clone(&release),
    )
    .await;
    let state = agent.transient_turn_context_state();
    let (tx, mut rx) = mpsc::channel(256);
    let run = tokio::spawn(async move {
        let result = agent.run_with_events("start".to_string().into(), tx).await;
        (agent, result)
    });

    tokio::time::timeout(std::time::Duration::from_secs(5), entered.notified())
        .await
        .expect("the gated tool starts");
    let run_id = state.active_run_for_test().expect("the run owns the actor");
    let prepare_state = state.clone();
    let prepare_run_id = run_id.clone();
    let prepare = tokio::spawn(async move {
        prepare_state
            .prepare_active_turn_boundary(&prepare_run_id, durable_notice())
            .await
    });
    wait_until("durable registration", || {
        state.has_registered_durable_for_test()
    })
    .await;
    release.notify_one();

    let stage = tokio::time::timeout(std::time::Duration::from_secs(5), prepare)
        .await
        .expect("the runner parks at the post-tool boundary")
        .expect("prepare task")
        .expect("durable preparation")
        .into_stage_output(None);
    let witness = stage.delivery_witness().cloned().expect("durable witness");
    stage.commit().expect("publish durable appends");

    let (agent, result) = tokio::time::timeout(std::time::Duration::from_secs(5), run)
        .await
        .expect("run finishes")
        .expect("run task");
    result.expect("run succeeds");
    assert_eq!(
        witness.outcome(),
        meerkat_core::lifecycle::CoreBoundaryDeliveryOutcome::Applied
    );

    // The request that crossed the boundary and every later request of the
    // SAME run carry the notice as a transcript row.
    let requests = client.requests();
    assert_eq!(requests.len(), 3, "one run, three model requests");
    assert!(!mentions(&requests[0], NOTICE_TOKEN));
    assert_eq!(requests[1].iter().filter(|m| carries_notice(m)).count(), 1);
    assert_eq!(
        requests[2].iter().filter(|m| carries_notice(m)).count(),
        1,
        "the next boundary's synthetic-notice refresh keeps a durable notice"
    );

    // Exactly one transcript row, after the tool results of the boundary and
    // before the next assistant message.
    let messages = agent.session().messages();
    let positions = messages
        .iter()
        .enumerate()
        .filter(|(_, message)| carries_notice(message))
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    assert_eq!(positions.len(), 1, "the notice is written exactly once");
    let at = positions[0];
    assert!(matches!(messages[at - 1], Message::ToolResults { .. }));
    assert!(matches!(messages[at + 1], Message::BlockAssistant(_)));

    let mut applied = Vec::new();
    while let Ok(event) = rx.try_recv() {
        if let AgentEvent::BoundaryAppendApplied {
            run_id: event_run,
            append_count,
            ..
        } = event
        {
            applied.push((event_run, append_count));
        }
    }
    assert_eq!(applied, vec![(run_id, 1)]);
}

#[tokio::test]
async fn request_only_boundary_context_stays_request_local() {
    let client = Arc::new(RecordingClient::new());
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let mut agent = gated_agent(
        Arc::clone(&client),
        Arc::clone(&entered),
        Arc::clone(&release),
    )
    .await;
    let state = agent.transient_turn_context_state();
    let (tx, mut rx) = mpsc::channel(256);
    let run = tokio::spawn(async move {
        let result = agent.run_with_events("start".to_string().into(), tx).await;
        (agent, result)
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), entered.notified())
        .await
        .expect("the gated tool starts");
    let run_id = state.active_run_for_test().expect("the run owns the actor");
    let prepare_state = state.clone();
    let prepare = tokio::spawn(async move {
        prepare_state
            .prepare_active_turn_boundary(
                &run_id,
                meerkat_core::lifecycle::TurnBoundaryDelivery::RequestOnly(vec![
                    meerkat_core::lifecycle::TurnRequestContext::new(REQUEST_ONLY_TOKEN)
                        .expect("context"),
                ]),
            )
            .await
    });
    wait_until("request-only registration", || {
        state.has_registered_request_only_for_test()
    })
    .await;
    release.notify_one();
    let stage = tokio::time::timeout(std::time::Duration::from_secs(5), prepare)
        .await
        .expect("parked")
        .expect("prepare task")
        .expect("request-only preparation")
        .into_stage_output(None);
    assert!(stage.delivery_witness().is_none());
    stage.commit().expect("publish request-only context");
    let (agent, result) = run.await.expect("run task");
    result.expect("run succeeds");

    let requests = client.requests();
    assert!(mentions(&requests[1], REQUEST_ONLY_TOKEN));
    assert!(
        !mentions(&requests[2], REQUEST_ONLY_TOKEN),
        "request-only context is visible to exactly one request"
    );
    assert!(!mentions(agent.session().messages(), REQUEST_ONLY_TOKEN));
    while let Ok(event) = rx.try_recv() {
        assert!(
            !matches!(event, AgentEvent::BoundaryAppendApplied { .. }),
            "request-only context never announces a durable append"
        );
    }
}

/// Run one gated turn and deliver `delivery` at its post-tool boundary.
/// Returns the agent after the run, the Session before the run, the delivery
/// witness and every event the run published.
async fn run_with_durable_delivery_at_post_tool_boundary(
    delivery: meerkat_core::lifecycle::TurnBoundaryDelivery,
) -> (
    meerkat_core::Agent<RecordingClient, GatedTools, NoopStore>,
    meerkat_core::Session,
    meerkat_core::lifecycle::CoreBoundaryDeliveryWitness,
    Vec<AgentEvent>,
) {
    let client = Arc::new(RecordingClient::new());
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let mut agent = gated_agent(client, Arc::clone(&entered), Arc::clone(&release)).await;
    let pre_run_session = agent.session().clone();
    let state = agent.transient_turn_context_state();
    let (tx, mut rx) = mpsc::channel(256);
    let run = tokio::spawn(async move {
        let result = agent.run_with_events("start".to_string().into(), tx).await;
        (agent, result)
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), entered.notified())
        .await
        .expect("the gated tool starts");
    let run_id = state.active_run_for_test().expect("the run owns the actor");
    let prepare_state = state.clone();
    let prepare = tokio::spawn(async move {
        prepare_state
            .prepare_active_turn_boundary(&run_id, delivery)
            .await
    });
    wait_until("durable registration", || {
        state.has_registered_durable_for_test()
    })
    .await;
    release.notify_one();
    let stage = tokio::time::timeout(std::time::Duration::from_secs(5), prepare)
        .await
        .expect("the runner parks at the post-tool boundary")
        .expect("prepare task")
        .expect("durable preparation")
        .into_stage_output(None);
    let witness = stage.delivery_witness().cloned().expect("durable witness");
    stage.commit().expect("publish durable appends");
    let (agent, result) = tokio::time::timeout(std::time::Duration::from_secs(5), run)
        .await
        .expect("run finishes")
        .expect("run task");
    result.expect("run succeeds");
    let mut events = Vec::new();
    while let Ok(event) = rx.try_recv() {
        events.push(event);
    }
    (agent, pre_run_session, witness, events)
}

fn uncommitted_compaction_rollback(
    agent: &meerkat_core::Agent<RecordingClient, GatedTools, NoopStore>,
    rollback_session: meerkat_core::Session,
    rollback_durable_boundary_apply_ordinal: u64,
) -> crate::agent::CompactionTransaction {
    crate::agent::CompactionTransaction {
        phase: crate::agent::CompactionTransactionPhase::AwaitingRuntimeCommit(Box::new(
            crate::agent::CompactionRollbackState {
                rollback_session,
                rollback_last_input_tokens: agent.last_input_tokens,
                rollback_compaction_cadence: agent.compaction_cadence.clone(),
                rollback_durable_row_floor: agent.durable_row_floor,
                rollback_durable_boundary_apply_ordinal,
            },
        )),
        projections: Vec::new(),
    }
}

#[tokio::test]
async fn compaction_rollback_discards_only_the_durable_appends_applied_after_its_capture() {
    let (mut agent, pre_run_session, witness, _events) =
        run_with_durable_delivery_at_post_tool_boundary(durable_notice()).await;
    assert_eq!(
        witness.outcome(),
        meerkat_core::lifecycle::CoreBoundaryDeliveryOutcome::Applied
    );
    let applied_ordinal = agent.transient_turn_context_state().durable_apply_ordinal();
    assert_eq!(
        applied_ordinal, 1,
        "the runner applied exactly one delivery"
    );

    // A rollback captured AFTER the apply restores an image that still holds
    // the notice: the delivery stays Applied and its input is consumed.
    let post_apply_session = agent.session().clone();
    let original_notice = post_apply_session
        .messages()
        .iter()
        .find(|message| carries_notice(message))
        .expect("applied canonical notice")
        .clone();
    let Message::SystemNotice(notice) = &original_notice else {
        panic!("typed notice");
    };
    assert!(notice.runtime_origin.is_some());
    agent.compaction_transaction = Some(uncommitted_compaction_rollback(
        &agent,
        post_apply_session,
        applied_ordinal,
    ));
    agent
        .abort_uncommitted_compaction_projections()
        .await
        .expect("abort a rollback captured after the apply");
    assert_eq!(
        witness.outcome(),
        meerkat_core::lifecycle::CoreBoundaryDeliveryOutcome::Applied
    );
    assert_eq!(
        agent
            .session()
            .messages()
            .iter()
            .filter(|message| carries_notice(message))
            .count(),
        1
    );
    let retained_notice = agent
        .session()
        .messages()
        .iter()
        .find(|message| carries_notice(message))
        .expect("rollback retained the exact canonical notice");
    assert_eq!(
        serde_json::to_value(retained_notice).unwrap(),
        serde_json::to_value(original_notice).unwrap(),
        "a retained image keeps notice content, timestamp and runtime origin"
    );

    // A rollback captured BEFORE the apply restores an image without the
    // notice: the delivery is Discarded, so the runtime redelivers its input
    // once instead of consuming it with an image that lost it.
    agent.compaction_transaction =
        Some(uncommitted_compaction_rollback(&agent, pre_run_session, 0));
    agent
        .abort_uncommitted_compaction_projections()
        .await
        .expect("abort a rollback captured before the apply");
    assert!(
        !agent.session().messages().iter().any(carries_notice),
        "the restored image no longer holds the notice"
    );
    assert_eq!(
        witness.outcome(),
        meerkat_core::lifecycle::CoreBoundaryDeliveryOutcome::Discarded
    );
}

#[tokio::test]
async fn durable_comms_notice_publishes_peer_content_ingested_in_turn() {
    let comms_notice = meerkat_core::lifecycle::ConversationAppend {
        runtime_source: None,
        role: meerkat_core::lifecycle::ConversationAppendRole::SystemNotice,
        content: meerkat_core::lifecycle::CoreRenderable::SystemNotice {
            kind: meerkat_core::types::SystemNoticeKind::Comms,
            body: None,
            blocks: vec![meerkat_core::types::SystemNoticeBlock::Comms {
                kind: meerkat_core::types::CommsNoticeKind::Request,
                direction: meerkat_core::types::SystemNoticeDirection::Incoming,
                peer: None,
                sender_taint: None,
                request_id: Some("req-1".to_string()),
                intent: None,
                status: None,
                summary: None,
                payload: None,
                content: vec![meerkat_core::types::ContentBlock::Text {
                    text: NOTICE_TOKEN.to_string(),
                }],
            }],
        },
        identity: None,
    };
    let delivery = meerkat_core::lifecycle::TurnBoundaryDelivery::DurableAppends(
        meerkat_core::lifecycle::DurableTurnBoundaryAppends::try_new(
            meerkat_core::lifecycle::InputId::new(),
            vec![comms_notice],
            None,
        )
        .expect("eligible comms notice"),
    );
    let (_agent, _pre_run_session, witness, events) =
        run_with_durable_delivery_at_post_tool_boundary(delivery).await;
    assert_eq!(
        witness.outcome(),
        meerkat_core::lifecycle::CoreBoundaryDeliveryOutcome::Applied
    );
    // The same ingestion fact the turn-start path publishes for a queued
    // peer delivery, right after the append is announced.
    let applied_at = events
        .iter()
        .position(|event| matches!(event, AgentEvent::BoundaryAppendApplied { .. }))
        .expect("the durable append is announced");
    let ingested = events
        .iter()
        .enumerate()
        .filter(|(_, event)| matches!(event, AgentEvent::PeerContentIngested { .. }))
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    assert_eq!(ingested, vec![applied_at + 1], "{events:?}");
    assert!(matches!(
        &events[applied_at + 1],
        AgentEvent::PeerContentIngested { request_id, .. } if request_id.as_deref() == Some("req-1")
    ));
}

// These contract tests deliberately inspect serialized public facts so they
// compile against the pre-origin API and fail on absent behavior, not types.
fn projection_contract_append(
    role: &str,
    content: serde_json::Value,
) -> meerkat_core::lifecycle::ConversationAppend {
    serde_json::from_value(serde_json::json!({
        "role": role,
        "content": content,
    }))
    .expect("existing typed append wire shape")
}

fn projection_contract_notice() -> meerkat_core::lifecycle::ConversationAppend {
    projection_contract_append(
        "system_notice",
        serde_json::json!({
            "type": "system_notice",
            "kind": "generic",
            "body": "  same notice\nsecond paragraph  ",
            "blocks": [{
                "type": "runtime_notice",
                "category": "contract_probe",
                "detail": "exact block text",
                "payload": {"ordinal_is_not_text": true},
            }],
        }),
    )
}

#[tokio::test]
async fn durable_notice_event_rows_equal_applied_transcript_rows() {
    let input_id = meerkat_core::lifecycle::InputId::new();
    let appends = vec![
        projection_contract_append(
            "injected_context",
            serde_json::json!({
                "type": "text", "text": "host context",
            }),
        ),
        projection_contract_notice(),
        projection_contract_append(
            "user",
            serde_json::json!({
                "type": "text", "text": "follow-up instruction",
            }),
        ),
        projection_contract_notice(),
    ];
    let expected_projection = serde_json::to_value(
        meerkat_core::lifecycle::run_primitive::model_projection_content_input_from_conversation_appends(&appends),
    ).unwrap();
    let delivery = meerkat_core::lifecycle::TurnBoundaryDelivery::DurableAppends(
        meerkat_core::lifecycle::DurableTurnBoundaryAppends::try_new(
            input_id.clone(),
            appends,
            None,
        )
        .expect("mixed eligible appends"),
    );
    let (agent, _, witness, events) =
        run_with_durable_delivery_at_post_tool_boundary(delivery).await;
    assert_eq!(
        witness.outcome(),
        meerkat_core::CoreBoundaryDeliveryOutcome::Applied
    );
    let applied = events
        .iter()
        .filter(|event| matches!(event, AgentEvent::BoundaryAppendApplied { .. }))
        .collect::<Vec<_>>();
    assert_eq!(
        applied.len(),
        1,
        "one application event carries both notices"
    );
    let event = serde_json::to_value(applied[0]).unwrap();
    assert_eq!(event["append_count"], 4);
    assert_eq!(
        event["content"], expected_projection,
        "compatibility content remains the model projection"
    );
    let emitted = event["notices"]
        .as_array()
        .expect("typed canonical notice delta");
    assert_eq!(
        emitted.len(),
        2,
        "equal bodies are distinct accepted appends"
    );
    let history = agent.session().messages();
    let rows = history
        .iter()
        .enumerate()
        .filter_map(|(offset, message)| match message {
            Message::SystemNotice(notice)
                if notice.body.as_deref() == Some("  same notice\nsecond paragraph  ") =>
            {
                Some((offset, notice))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(rows.len(), 2);
    let start = event["transcript_start"]
        .as_u64()
        .expect("application image position") as usize;
    for ((offset, notice), ordinal) in rows.iter().zip([1_u64, 3]) {
        let wire = serde_json::to_value(notice).unwrap();
        assert_eq!(
            wire["runtime_origin"],
            serde_json::json!({
                "session_id": agent.session().id(),
                "run_id": event["run_id"],
                "input_id": input_id,
                "append_ordinal": ordinal,
            })
        );
        assert_eq!(*offset, start + ordinal as usize);
        assert_eq!(wire["body"], "  same notice\nsecond paragraph  ");
        assert_eq!(wire["blocks"][0]["payload"]["ordinal_is_not_text"], true);
    }
    let canonical = rows
        .iter()
        .map(|(_, notice)| serde_json::to_value(notice).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        *emitted, canonical,
        "event carries the actual typed rows including timestamps"
    );
    assert!(matches!(&history[start - 1], Message::ToolResults { .. }));
    assert!(matches!(&history[start + 4], Message::BlockAssistant(_)));
    let requests = agent.client.requests();
    assert_eq!(requests.len(), 3);
    for request in &requests[1..] {
        let notices = request
            .iter()
            .filter_map(|message| match message {
                Message::SystemNotice(notice)
                    if notice.body.as_deref() == Some("  same notice\nsecond paragraph  ") =>
                {
                    Some(serde_json::to_value(notice).unwrap())
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            notices, canonical,
            "every later request preserves canonical bytes and provenance"
        );
    }
}

#[tokio::test]
async fn durable_persisted_background_job_keeps_exact_notice_origin_through_refresh() {
    let input_id = meerkat_core::lifecycle::InputId::new();
    let notice = meerkat_core::types::SystemNoticeMessage::persisted_background_job(
        "integration probe",
        "persisted-job-origin-probe",
        meerkat_core::event::BackgroundJobTerminalStatus::Completed,
        "Exact detached result\nwith a second paragraph.".to_string(),
    );
    let expected_body = notice.body.clone();
    let expected_blocks = serde_json::to_value(&notice.blocks).unwrap();
    let delivery = meerkat_core::lifecycle::TurnBoundaryDelivery::DurableAppends(
        meerkat_core::lifecycle::DurableTurnBoundaryAppends::try_new(
            input_id.clone(),
            vec![meerkat_core::lifecycle::ConversationAppend {
                runtime_source: None,
                role: meerkat_core::lifecycle::ConversationAppendRole::SystemNotice,
                content: meerkat_core::lifecycle::CoreRenderable::SystemNotice {
                    kind: notice.kind,
                    body: notice.body,
                    blocks: notice.blocks,
                },
                identity: None,
            }],
            None,
        )
        .expect("typed persisted job is an eligible durable append"),
    );
    let (agent, _, witness, events) =
        run_with_durable_delivery_at_post_tool_boundary(delivery).await;
    assert_eq!(
        witness.outcome(),
        meerkat_core::CoreBoundaryDeliveryOutcome::Applied
    );
    let applied = events
        .iter()
        .enumerate()
        .filter(|(_, event)| matches!(event, AgentEvent::BoundaryAppendApplied { .. }))
        .collect::<Vec<_>>();
    assert_eq!(applied.len(), 1);
    let (applied_at, applied_event) = applied[0];
    let event = serde_json::to_value(applied_event).unwrap();
    assert_eq!(event["append_count"], 1);
    let emitted = event["notices"].as_array().expect("canonical notice rows");
    assert_eq!(emitted.len(), 1);

    let collect_job = |messages: &[Message]| {
        messages
            .iter()
            .filter_map(|message| match message {
                Message::SystemNotice(notice)
                    if notice.persisted_background_job_id()
                        == Some("persisted-job-origin-probe") =>
                {
                    Some(serde_json::to_value(notice).unwrap())
                }
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    let canonical = collect_job(agent.session().messages());
    assert_eq!(canonical.len(), 1, "refresh retains one committed job row");
    assert_eq!(*emitted, canonical, "live event contains the saved row");
    assert_eq!(canonical[0]["kind"], "background_job");
    assert_eq!(
        canonical[0]["body"],
        serde_json::to_value(expected_body).unwrap()
    );
    assert_eq!(canonical[0]["blocks"], expected_blocks);
    assert_eq!(canonical[0]["blocks"][0]["persisted"], true);
    assert_eq!(
        canonical[0]["runtime_origin"],
        serde_json::json!({
            "session_id": agent.session().id(),
            "run_id": event["run_id"],
            "input_id": input_id,
            "append_ordinal": 0,
        })
    );
    let requests = agent.client.requests();
    assert_eq!(requests.len(), 3);
    assert!(collect_job(&requests[0]).is_empty());
    for request in &requests[1..] {
        assert_eq!(
            collect_job(request),
            canonical,
            "later model requests retain exact content and runtime provenance"
        );
    }
    let completed = events
        .iter()
        .enumerate()
        .filter(|(_, event)| matches!(event, AgentEvent::RunCompleted { .. }))
        .collect::<Vec<_>>();
    assert_eq!(
        completed.len(),
        1,
        "job outcome does not mint a run terminal"
    );
    assert!(completed[0].0 > applied_at);
    assert!(matches!(
        completed[0].1,
        AgentEvent::RunCompleted { result, .. } if result == "done"
    ));
}

#[test]
fn legacy_boundary_event_deserializes_without_typed_notice_projection() {
    let old = serde_json::json!({
        "type": "boundary_append_applied",
        "run_id": meerkat_core::lifecycle::RunId::new(),
        "input_id": meerkat_core::lifecycle::InputId::new(),
        "content": "legacy model projection",
        "append_count": 1,
    });
    let event: AgentEvent = serde_json::from_value(old).expect("old event remains supported");
    let current = serde_json::to_value(event).unwrap();
    assert!(
        current
            .get("notices")
            .is_none_or(|rows| rows.as_array().is_some_and(Vec::is_empty))
    );
    assert!(
        current
            .get("transcript_start")
            .is_none_or(serde_json::Value::is_null)
    );
}
