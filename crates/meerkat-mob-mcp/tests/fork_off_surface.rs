//! fork_off and council through the agent-facing surface, over the production
//! persistent session service with a scripted provider.
//!
//! These pin the headline paths of the detached contract: a fork_off returns
//! promptly and its outcome reaches the forker as one durable completion
//! record (a persisted `BackgroundJob` system notice) that wakes an idle
//! forker; one-shot hosts block for the result; the forker can observe and
//! retire its own children without manage scope; ownership follows the spawn
//! tree and retirement cascades down it.
//!
//! Only the LLM is scripted. Each scripted reply is chosen from the last
//! user message of the request, and every request is recorded, so a test can
//! read exactly what a member's next model call carried.
#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

mod support;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use meerkat_client::LlmRequest;
use meerkat_core::agent::AgentToolDispatcher;
use meerkat_core::event::BackgroundJobTerminalStatus;
use meerkat_core::types::{SystemNoticeBlock, SystemNoticeKind, ToolCallView};
use meerkat_core::{
    ContentInput, HandlingMode, Message, SessionId, ToolDispatchOutcome, ToolError,
};
use meerkat_mob::{
    AgentIdentity, BoundedResultSpec, MobHandle, SpawnMemberSpec, WorkOrigin, WorkSpec,
};
use meerkat_mob_mcp::{
    AgentMobToolSurface, DetachedCompletionDelivery, DetachedDeliveryUnavailable, MobMcpState,
};
use serde_json::{Value, json};
use support::{CouncilFixture, ScriptedTurn, TurnGate, last_user_text, role_in_request, user_text};

/// Marker in every child task, so the script can tell a child's turn from
/// the forker's own turns.
const CHILD_TASK: &str = "CHILD-TASK-4X reply with the token";
const CHILD_REPLY: &str = "FORKED-RESULT-7Q";
const FOLLOW_UP_REPLY: &str = "FOLLOW-UP-ACK";
const COUNCIL_SUMMARY: &str = "COUNCIL-SUMMARY-9Z the council agreed";

// ===========================================================================
// Request log + script
// ===========================================================================

#[derive(Debug, Clone)]
struct RecordedRequest {
    last_user: String,
    rendered: String,
}

/// Every provider request, in issue order.
#[derive(Clone, Default)]
struct RequestLog(Arc<Mutex<Vec<RecordedRequest>>>);

impl RequestLog {
    fn record(&self, request: &LlmRequest) {
        self.0.lock().unwrap().push(RecordedRequest {
            last_user: last_user_text(request),
            rendered: format!("{:?}", request.messages),
        });
    }

    /// The single request whose last user message carries `prompt`.
    fn request_for(&self, prompt: &str) -> RecordedRequest {
        let matching: Vec<_> = self
            .0
            .lock()
            .unwrap()
            .iter()
            .filter(|request| request.last_user.contains(prompt))
            .cloned()
            .collect();
        assert_eq!(
            matching.len(),
            1,
            "expected exactly one model request for prompt {prompt:?}, got {matching:#?}"
        );
        matching.into_iter().next().unwrap()
    }
}

/// Opens a gate when dropped, so a failing test that unwinds does not leave
/// scripted turns blocked forever while the runtime shuts down.
struct OpenOnDrop(Arc<TurnGate>);

impl Drop for OpenOnDrop {
    fn drop(&mut self) {
        self.0.open();
    }
}

/// How a child whose task carries `marker` replies.
#[derive(Clone)]
enum ChildReply {
    Text(&'static str),
    Gated(Arc<TurnGate>, &'static str),
}

/// Route replies by the last user message: a child task gets its configured
/// reply, council rounds answer by role, anything else is a follow-up turn.
fn routed_script(
    log: RequestLog,
    children: Vec<(&'static str, ChildReply)>,
) -> impl Fn(&LlmRequest) -> ScriptedTurn + Send + Sync + 'static {
    move |request| {
        log.record(request);
        let last = last_user_text(request);
        for (marker, reply) in &children {
            if last.contains(marker) {
                return match reply {
                    ChildReply::Text(text) => ScriptedTurn::Text((*text).to_string()),
                    ChildReply::Gated(gate, text) => {
                        ScriptedTurn::Gated(Arc::clone(gate), (*text).to_string())
                    }
                };
            }
        }
        if user_text(request).contains("bounded plain-text summary") {
            return ScriptedTurn::Text(COUNCIL_SUMMARY.to_string());
        }
        if let Some(role) = role_in_request(request) {
            return ScriptedTurn::Text(format!("position from {role}"));
        }
        ScriptedTurn::Text(FOLLOW_UP_REPLY.to_string())
    }
}

// ===========================================================================
// Surfaces and calls
// ===========================================================================

/// A member's fork authority: it may spawn its own role, nothing else. No
/// manage scope, so every observation of another member goes through
/// ownership admission.
fn forker_authority(mob_id: &str) -> meerkat_core::service::MobToolAuthorityContext {
    let authority = meerkat_runtime::mob_operator_authority::create_only_mob_operator_authority()
        .expect("generated authority");
    let authority =
        meerkat_runtime::mob_operator_authority::set_create_authority(&authority, false)
            .expect("no create scope");
    meerkat_runtime::mob_operator_authority::grant_spawn_profile_in_mob(
        &authority,
        mob_id,
        "participant",
    )
    .expect("spawnable participant profile, no manage scope")
}

/// A convener's authority: create scope (councils) and manage scope over the
/// source mob whose members participate.
fn convener_authority(mob_id: &str) -> meerkat_core::service::MobToolAuthorityContext {
    let authority = meerkat_runtime::mob_operator_authority::create_only_mob_operator_authority()
        .expect("generated authority");
    meerkat_runtime::mob_operator_authority::grant_manage_mob(&authority, mob_id)
        .expect("manage scope over the source mob")
}

struct BoundSurface {
    surface: Arc<dyn AgentToolDispatcher>,
    session: SessionId,
}

/// The agent-facing surface for `session`, bound to an operation registry
/// the way the agent loop binds every dispatcher.
fn bind_surface(
    state: &Arc<MobMcpState>,
    session: SessionId,
    authority: meerkat_core::service::MobToolAuthorityContext,
) -> BoundSurface {
    let registry = Arc::new(meerkat_runtime::ops_lifecycle::RuntimeOpsLifecycleRegistry::new());
    let surface: Arc<dyn AgentToolDispatcher> = Arc::new(AgentMobToolSurface::new(
        Arc::clone(state),
        None,
        authority,
        "claude-sonnet-4-5".to_string(),
        session.clone(),
        None,
        None,
        None,
    ));
    let surface = surface
        .bind_ops_lifecycle(registry, session.clone())
        .expect("bind ops lifecycle")
        .into_dispatcher();
    BoundSurface { surface, session }
}

async fn source_handle(fixture: &CouncilFixture) -> MobHandle {
    fixture
        .state
        .handle_for(&fixture.source_mob_id())
        .await
        .expect("source mob handle")
}

async fn member_session(fixture: &CouncilFixture, member: &str) -> SessionId {
    source_handle(fixture)
        .await
        .resolve_bridge_session_id(&AgentIdentity::from(member))
        .await
        .unwrap_or_else(|| panic!("{member} has a bridge session"))
}

/// The surface of mob member `member`, acting with fork authority only.
async fn member_surface(fixture: &CouncilFixture, member: &str) -> BoundSurface {
    let session = member_session(fixture, member).await;
    bind_surface(
        &fixture.state,
        session,
        forker_authority(fixture.source_mob_id().as_str()),
    )
}

async fn dispatch(
    surface: &Arc<dyn AgentToolDispatcher>,
    name: &'static str,
    args: Value,
) -> Result<ToolDispatchOutcome, ToolError> {
    let raw = serde_json::value::RawValue::from_string(args.to_string()).unwrap();
    surface
        .dispatch(ToolCallView {
            id: "surface-call",
            name,
            args: &raw,
        })
        .await
}

fn result_json(outcome: &ToolDispatchOutcome) -> Value {
    serde_json::from_str(&outcome.result.text_content()).expect("json tool result")
}

async fn call(
    surface: &Arc<dyn AgentToolDispatcher>,
    name: &'static str,
    args: Value,
) -> Result<Value, ToolError> {
    dispatch(surface, name, args)
        .await
        .map(|outcome| result_json(&outcome))
}

/// `call`, retried while the mob reports a retryable pending admission.
///
/// A restored mob runs the fork_off re-link pass, which observes restored
/// children through the mob's single member-status observation lane; a
/// concurrent check is told to retry (`observation_lane_saturated`) rather
/// than queued. Any other error ends the retries.
async fn call_when_admitted(
    surface: &Arc<dyn AgentToolDispatcher>,
    name: &'static str,
    args: Value,
) -> Result<Value, ToolError> {
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        match call(surface, name, args.clone()).await {
            Err(ToolError::ExecutionFailedWithData { data, .. })
                if data["kind"] == "mob_lifecycle_operation_admission_pending"
                    && data["retryable"] == true
                    && std::time::Instant::now() < deadline =>
            {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            other => return other,
        }
    }
}

fn fork_args(member_id: &str, task_marker: &str) -> Value {
    json!({"member_id": member_id, "task": task_marker})
}

/// Start a detached fork_off and return its job id.
async fn start_detached_fork(bound: &BoundSurface, member_id: &str, args: Value) -> String {
    let started = call(&bound.surface, "fork_off", args)
        .await
        .unwrap_or_else(|error| panic!("fork_off {member_id} starts: {error}"));
    assert_eq!(started["status"], "running", "{started}");
    assert_eq!(started["agent_identity"], member_id, "{started}");
    started["job_id"].as_str().expect("job id").to_string()
}

/// One durable completion record of a detached job, as found in the owner's
/// transcript.
#[derive(Debug)]
struct CompletionRecord {
    status: BackgroundJobTerminalStatus,
    detail: String,
    body: String,
}

/// The durable completion records of `job_id`: `BackgroundJob` system
/// notices whose block is `persisted` for that job.
fn completion_records(messages: &[Message], job_id: &str) -> Vec<CompletionRecord> {
    messages
        .iter()
        .filter_map(|message| match message {
            Message::SystemNotice(notice) if notice.kind == SystemNoticeKind::BackgroundJob => {
                notice.blocks.iter().find_map(|block| match block {
                    SystemNoticeBlock::BackgroundJob {
                        job_id: id,
                        status,
                        detail,
                        persisted: true,
                        ..
                    } if id == job_id => Some(CompletionRecord {
                        status: *status,
                        detail: detail.clone().unwrap_or_default(),
                        body: notice.body.clone().unwrap_or_default(),
                    }),
                    _ => None,
                })
            }
            _ => None,
        })
        .collect()
}

/// Every durable completion record in `messages`, whatever the job.
fn any_completion_records(messages: &[Message]) -> usize {
    messages
        .iter()
        .filter(|message| {
            matches!(message, Message::SystemNotice(notice) if notice.blocks.iter().any(|block| {
                matches!(block, SystemNoticeBlock::BackgroundJob { persisted: true, .. })
            }))
        })
        .count()
}

/// Wait until the owner's durable transcript holds the completion record of
/// `job_id`, and check it is recorded exactly once.
async fn wait_for_completion(
    fixture: &CouncilFixture,
    owner: &SessionId,
    job_id: &str,
) -> CompletionRecord {
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    loop {
        let mut records = completion_records(
            &persisted_messages(fixture.service.as_ref(), owner).await,
            job_id,
        );
        assert!(
            records.len() <= 1,
            "job {job_id} recorded more than once: {records:?}"
        );
        if let Some(record) = records.pop() {
            return record;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the owner never received the completion of job {job_id}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// Run one exact turn of `member` with `prompt` and return its reply.
async fn drive_turn(fixture: &CouncilFixture, member: &str, prompt: &str) -> String {
    let handle = source_handle(fixture).await;
    let spec = BoundedResultSpec::new("follow-up", 4096).expect("bounded result spec");
    let work = handle
        .start_work_for_identity_bounded(
            AgentIdentity::from(member),
            WorkSpec::new(ContentInput::Text(prompt.to_string()), WorkOrigin::Internal),
            HandlingMode::Queue,
            spec.clone(),
        )
        .await
        .unwrap_or_else(|error| panic!("start a turn for {member}: {error}"));
    match tokio::time::timeout(Duration::from_secs(60), work.wait_bounded(spec)).await {
        Ok(Ok(result)) => result.result().result().text().to_string(),
        Ok(Err(error)) => panic!("turn for {member} failed: {error:?}"),
        Err(elapsed) => panic!("turn for {member} timed out: {elapsed}"),
    }
}

type PersistentService = meerkat_session::PersistentSessionService<meerkat::FactoryAgentBuilder>;

async fn persisted_messages(service: &PersistentService, session: &SessionId) -> Vec<Message> {
    <PersistentService as meerkat_mob::MobSessionService>::load_persisted_session(service, session)
        .await
        .expect("load persisted session")
        .expect("session exists")
        .messages()
        .to_vec()
}

/// Exactly one durable completion record for `job_id`, naming its tool and
/// carrying `needle` in its typed detail.
fn assert_one_completion_record(messages: &[Message], tool: &str, job_id: &str, needle: &str) {
    let records = completion_records(messages, job_id);
    assert_eq!(
        records.len(),
        1,
        "exactly one durable completion record for job {job_id}, transcript: {messages:#?}"
    );
    let record = &records[0];
    assert!(
        record.body.contains(tool) && record.body.contains(job_id),
        "the record names its tool and job: {record:?}"
    );
    assert!(
        record.detail.contains(needle),
        "the record's detail carries {needle:?}: {record:?}"
    );
}

async fn assert_not_seated(handle: &MobHandle, member: &str) {
    assert!(
        handle
            .get_member(&AgentIdentity::from(member))
            .await
            .expect("get member")
            .is_none(),
        "{member} must be retired"
    );
}

async fn spawned_by(handle: &MobHandle, member: &str) -> Option<AgentIdentity> {
    handle
        .get_member(&AgentIdentity::from(member))
        .await
        .expect("get member")
        .unwrap_or_else(|| panic!("{member} is seated"))
        .spawned_by
}

fn member_names(listed: &Value) -> Vec<String> {
    let mut names: Vec<String> = listed["members"]
        .as_array()
        .expect("members")
        .iter()
        .map(|member| {
            member["agent_identity"]
                .as_str()
                .or_else(|| member["member_id"].as_str())
                .unwrap_or_else(|| panic!("member without identity: {member}"))
                .to_string()
        })
        .collect();
    names.sort();
    names
}

// ===========================================================================
// Detached fork_off: the completion reaches the forker durably
// ===========================================================================

/// fork_off returns while the child is still running. When the child
/// finishes, its outcome is recorded once in the forker's transcript, the
/// idle forker is woken for exactly one turn that sees it, and later model
/// requests keep carrying it. The forker here has never run a turn of its
/// own, so this is also the not-live owner case of a member that never ran.
#[tokio::test(flavor = "multi_thread")]
async fn detached_fork_off_completion_reaches_the_forkers_next_model_request() {
    let log = RequestLog::default();
    let gate = TurnGate::new();
    let fixture = CouncilFixture::new_runtime_backed(routed_script(
        log.clone(),
        vec![(CHILD_TASK, ChildReply::Gated(gate.clone(), CHILD_REPLY))],
    ));
    fixture.seed_source_mob(&["forker", "bystander"]).await;
    let forker = member_surface(&fixture, "forker").await;

    let job_id = start_detached_fork(
        &forker,
        "surface-child",
        fork_args("surface-child", CHILD_TASK),
    )
    .await;
    // The call returned while the child's turn is still held open, and
    // nothing is recorded before the child finishes.
    gate.wait_entered(1).await;
    assert!(
        completion_records(
            &persisted_messages(fixture.service.as_ref(), &forker.session).await,
            &job_id
        )
        .is_empty(),
        "no completion before the child finishes"
    );
    let handle = source_handle(&fixture).await;
    assert_eq!(
        spawned_by(&handle, "surface-child").await,
        Some(AgentIdentity::from("forker")),
        "the forker owns its child"
    );

    gate.open();
    let record = wait_for_completion(&fixture, &forker.session, &job_id).await;
    assert_eq!(
        record.status,
        BackgroundJobTerminalStatus::Completed,
        "{record:?}"
    );
    assert!(
        record.detail.contains(CHILD_REPLY) && record.detail.contains("completed"),
        "the record carries the child's typed outcome: {record:?}"
    );

    // The idle forker is woken once, and that turn's request carries it.
    let wake_requests = || {
        log.0
            .lock()
            .unwrap()
            .iter()
            .filter(|request| {
                request.rendered.contains(&job_id) && !request.last_user.contains("FOLLOW-UP")
            })
            .cloned()
            .collect::<Vec<_>>()
    };
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    while wake_requests().is_empty() {
        assert!(
            std::time::Instant::now() < deadline,
            "the idle forker was never woken by its child's completion"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    let woken = wake_requests();
    assert_eq!(
        woken.len(),
        1,
        "one wake turn for one completion: {woken:#?}"
    );
    assert!(
        woken[0].rendered.contains(CHILD_REPLY),
        "the wake turn's request carries the child's outcome: {}",
        woken[0].rendered
    );

    let prompt = "FOLLOW-UP-1 what did your fork find?";
    assert_eq!(
        drive_turn(&fixture, "forker", prompt).await,
        FOLLOW_UP_REPLY
    );
    let request = log.request_for(prompt);
    assert!(
        request.rendered.contains(&job_id) && request.rendered.contains(CHILD_REPLY),
        "the forker's next model request must carry the child's outcome: {}",
        request.rendered
    );

    // A completed child stays seated for further work.
    assert_eq!(
        spawned_by(&handle, "surface-child").await,
        Some(AgentIdentity::from("forker"))
    );
    fixture.teardown().await;
}

/// The completion is a durable record, not a refresh notice: it is in the
/// forker's model requests on two later turns, written exactly once, and
/// still there when the session is read back by a fresh service over the
/// same stores.
#[tokio::test(flavor = "multi_thread")]
async fn fork_off_completion_is_a_durable_entry_across_turns_and_reload() {
    let log = RequestLog::default();
    let fixture = CouncilFixture::new_runtime_backed(routed_script(
        log.clone(),
        vec![(CHILD_TASK, ChildReply::Text(CHILD_REPLY))],
    ));
    fixture.seed_source_mob(&["forker"]).await;
    let forker = member_surface(&fixture, "forker").await;

    let job_id = start_detached_fork(
        &forker,
        "durable-child",
        fork_args("durable-child", CHILD_TASK),
    )
    .await;
    let record = wait_for_completion(&fixture, &forker.session, &job_id).await;
    assert_eq!(
        record.status,
        BackgroundJobTerminalStatus::Completed,
        "{record:?}"
    );

    for prompt in [
        "FOLLOW-UP-A first later turn",
        "FOLLOW-UP-B second later turn",
    ] {
        assert_eq!(
            drive_turn(&fixture, "forker", prompt).await,
            FOLLOW_UP_REPLY
        );
        let request = log.request_for(prompt);
        assert!(
            request.rendered.contains(&job_id) && request.rendered.contains(CHILD_REPLY),
            "turn {prompt:?} must still carry the child's outcome: {}",
            request.rendered
        );
    }

    let messages = persisted_messages(fixture.service.as_ref(), &forker.session).await;
    assert_one_completion_record(&messages, "fork_off", &job_id, CHILD_REPLY);

    // A fresh service over the same stores has no live actor: what it reads
    // is what was persisted.
    let reopened = fixture.reopen_session_service();
    let reloaded = persisted_messages(reopened.as_ref(), &forker.session).await;
    assert_one_completion_record(&reloaded, "fork_off", &job_id, CHILD_REPLY);
    fixture.teardown().await;
}

/// The result does not depend on the child staying alive: after the forker
/// retires its child, the forker's next request still carries the result.
#[tokio::test(flavor = "multi_thread")]
async fn fork_off_result_outlives_the_retired_child() {
    let log = RequestLog::default();
    let fixture = CouncilFixture::new_runtime_backed(routed_script(
        log.clone(),
        vec![(CHILD_TASK, ChildReply::Text(CHILD_REPLY))],
    ));
    fixture.seed_source_mob(&["forker"]).await;
    let forker = member_surface(&fixture, "forker").await;
    let mob_id = fixture.source_mob_id().to_string();

    let job_id = start_detached_fork(
        &forker,
        "short-lived-child",
        fork_args("short-lived-child", CHILD_TASK),
    )
    .await;
    wait_for_completion(&fixture, &forker.session, &job_id).await;

    call(
        &forker.surface,
        "mob_retire_member",
        json!({"mob_id": mob_id, "member_id": "short-lived-child"}),
    )
    .await
    .expect("the forker retires its own child");
    let handle = source_handle(&fixture).await;
    assert_not_seated(&handle, "short-lived-child").await;

    let prompt = "FOLLOW-UP-R after the child is gone";
    assert_eq!(
        drive_turn(&fixture, "forker", prompt).await,
        FOLLOW_UP_REPLY
    );
    let request = log.request_for(prompt);
    assert!(
        request.rendered.contains(&job_id) && request.rendered.contains(CHILD_REPLY),
        "the result must survive the child's retirement: {}",
        request.rendered
    );
    assert_one_completion_record(
        &persisted_messages(fixture.service.as_ref(), &forker.session).await,
        "fork_off",
        &job_id,
        CHILD_REPLY,
    );
    fixture.teardown().await;
}

// ===========================================================================
// Detached council: the sealed result reaches the convener
// ===========================================================================

/// Council arguments over alice and bob. With `council_id: None` the tool
/// derives the id from the call, which is what a model does by default.
fn council_args(fixture: &CouncilFixture, council_id: Option<&str>) -> Value {
    let mob_id = fixture.source_mob_id();
    let mob_id = mob_id.as_str();
    let mut args = json!({
        "topic": "Should we ship the migration this week?",
        "participants": [
            {"mob_id": mob_id, "member_id": "alice", "role": "analyst"},
            {"mob_id": mob_id, "member_id": "bob", "role": "critic"},
        ],
        "max_rounds": 1,
        "timeout_seconds": 120,
    });
    if let Some(label) = council_id {
        args["council_id"] = json!(fixture.council_id(label).as_str());
    }
    args
}

/// The convener's council runs detached and its sealed result reaches the
/// convener's next model request. The council id is left to the tool, so this
/// also pins that a derived id seats its participants (it once did not: the
/// derived id was not a valid comms name component).
#[tokio::test(flavor = "multi_thread")]
async fn detached_council_completion_reaches_the_convener() {
    let log = RequestLog::default();
    let fixture = CouncilFixture::new_runtime_backed(routed_script(log.clone(), Vec::new()));
    fixture.seed_source_mob(&["convener", "alice", "bob"]).await;
    let mob_id = fixture.source_mob_id().to_string();
    let convener = bind_surface(
        &fixture.state,
        member_session(&fixture, "convener").await,
        convener_authority(&mob_id),
    );

    let started = call(&convener.surface, "council", council_args(&fixture, None))
        .await
        .expect("council starts");
    assert_eq!(started["status"], "running", "{started}");
    assert!(started["council_id"].as_str().is_some(), "{started}");
    let job_id = started["job_id"].as_str().expect("job id").to_string();

    let record = wait_for_completion(&fixture, &convener.session, &job_id).await;
    assert_eq!(
        record.status,
        BackgroundJobTerminalStatus::Completed,
        "a council that seated and merged completes its job: {record:?}"
    );
    assert_one_completion_record(
        &persisted_messages(fixture.service.as_ref(), &convener.session).await,
        "council",
        &job_id,
        "COUNCIL-SUMMARY-9Z",
    );
    let outcome: Value = serde_json::from_str(&record.detail).expect("typed council outcome");
    assert_eq!(
        outcome["result"]["exit_reason"]["reason"], "completed",
        "{outcome}"
    );
    let participants = outcome["result"]["participants"]
        .as_array()
        .expect("participants");
    assert_eq!(participants.len(), 2, "{outcome}");
    assert!(
        participants
            .iter()
            .all(|participant| participant["seated"] == true),
        "every participant of a council with a derived id is seated: {outcome}"
    );
    assert!(
        outcome["result"]["exchanges"]
            .as_array()
            .is_some_and(|exchanges| exchanges.len() >= 2),
        "the participants ran their exchanges: {outcome}"
    );

    let prompt = "FOLLOW-UP-C what did the council decide?";
    assert_eq!(
        drive_turn(&fixture, "convener", prompt).await,
        FOLLOW_UP_REPLY
    );
    let request = log.request_for(prompt);
    assert!(
        request.rendered.contains(&job_id) && request.rendered.contains("COUNCIL-SUMMARY-9Z"),
        "the convener's next model request must carry the council result: {}",
        request.rendered
    );
    fixture.teardown().await;
}

// ===========================================================================
// One-shot hosts: blocking contract, tool-owned deadline
// ===========================================================================

/// Without detached delivery, fork_off waits for the child however long it
/// takes and returns its result in the call itself; no completion record is
/// written for a result the call already returned.
#[tokio::test(flavor = "multi_thread")]
async fn one_shot_hosts_block_for_the_fork_off_result() {
    let log = RequestLog::default();
    let gate = TurnGate::new();
    let fixture = CouncilFixture::new_runtime_backed(routed_script(
        log,
        vec![(CHILD_TASK, ChildReply::Gated(gate.clone(), CHILD_REPLY))],
    ));
    fixture.seed_source_mob(&["forker"]).await;
    fixture
        .state
        .set_detached_completion_delivery(DetachedCompletionDelivery::Unavailable);
    let forker = member_surface(&fixture, "forker").await;

    let surface = Arc::clone(&forker.surface);
    let pending = tokio::spawn(async move {
        dispatch(
            &surface,
            "fork_off",
            fork_args("blocking-child", CHILD_TASK),
        )
        .await
    });
    gate.wait_entered(1).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        !pending.is_finished(),
        "a one-shot host's fork_off must wait for the child's turn"
    );
    gate.open();
    let outcome = tokio::time::timeout(Duration::from_secs(60), pending)
        .await
        .expect("the blocking call returns once the child finishes")
        .expect("join")
        .expect("blocking fork_off returns the result");
    let result = result_json(&outcome);
    assert_eq!(result["agent_identity"], "blocking-child", "{result}");
    assert_eq!(result["bounded_result"]["text"], CHILD_REPLY, "{result}");
    assert_eq!(
        result["blocked_because"], "host_declared_unavailable",
        "the result says, typed, why the call blocked: {result}"
    );
    assert!(
        result.get("job_id").is_none(),
        "no job on the blocking path: {result}"
    );
    assert_eq!(
        any_completion_records(
            &persisted_messages(fixture.service.as_ref(), &forker.session).await
        ),
        0,
        "no completion record on the blocking path"
    );
    fixture.teardown().await;
}

/// max_run_secs still bounds the blocking call: the child is cancelled and
/// retired, and the call reports the elapsed limit instead of hanging.
#[tokio::test(flavor = "multi_thread")]
async fn one_shot_fork_off_honours_max_run_secs() {
    let log = RequestLog::default();
    // Never opened while the test runs: without max_run_secs this child would
    // run forever.
    let gate = TurnGate::new();
    let _release_on_exit = OpenOnDrop(gate.clone());
    let fixture = CouncilFixture::new_runtime_backed(routed_script(
        log,
        vec![(CHILD_TASK, ChildReply::Gated(gate.clone(), CHILD_REPLY))],
    ));
    fixture.seed_source_mob(&["forker"]).await;
    fixture
        .state
        .set_detached_completion_delivery(DetachedCompletionDelivery::Unavailable);
    let forker = member_surface(&fixture, "forker").await;

    let error = tokio::time::timeout(
        Duration::from_secs(60),
        dispatch(
            &forker.surface,
            "fork_off",
            json!({"member_id": "bounded-child", "task": CHILD_TASK, "max_run_secs": 1}),
        ),
    )
    .await
    .expect("max_run_secs must end the blocking call")
    .expect_err("an autokilled child reports an error");
    assert!(
        error.to_string().contains("max_run_elapsed"),
        "the error names the elapsed limit: {error}"
    );
    assert_not_seated(&source_handle(&fixture).await, "bounded-child").await;
    fixture.teardown().await;
}

/// A one-shot host's council returns the sealed result in the call itself.
#[tokio::test(flavor = "multi_thread")]
async fn one_shot_hosts_block_for_the_council_result() {
    let fixture =
        CouncilFixture::new_runtime_backed(routed_script(RequestLog::default(), Vec::new()));
    fixture.seed_source_mob(&["convener", "alice", "bob"]).await;
    fixture
        .state
        .set_detached_completion_delivery(DetachedCompletionDelivery::Unavailable);
    let mob_id = fixture.source_mob_id().to_string();
    let convener = bind_surface(
        &fixture.state,
        member_session(&fixture, "convener").await,
        convener_authority(&mob_id),
    );

    let outcome = dispatch(
        &convener.surface,
        "council",
        council_args(&fixture, Some("blocking")),
    )
    .await
    .expect("blocking council returns the sealed result");
    let result = result_json(&outcome);
    assert!(
        result.get("job_id").is_none(),
        "no job on the blocking path: {result}"
    );
    assert!(
        result["result"].to_string().contains("COUNCIL-SUMMARY-9Z"),
        "the sealed result is in the call: {result}"
    );
    assert_eq!(
        result["blocked_because"], "host_declared_unavailable",
        "the result says, typed, why the call blocked: {result}"
    );
    assert_eq!(
        any_completion_records(
            &persisted_messages(fixture.service.as_ref(), &convener.session).await
        ),
        0,
        "no completion record on the blocking path"
    );
    fixture.teardown().await;
}

/// fork_off and council own their lifetime bound, so the agent loop's
/// default tool deadline (600 s unless configured) must not cut them, while
/// every other tool keeps it. This is the plan the agent loop resolves for a
/// call, both on the bare surface and through the dynamic composite the
/// factory mounts the mob family in.
#[tokio::test(flavor = "multi_thread")]
async fn fork_off_and_council_are_not_cut_by_the_core_tool_deadline() {
    let fixture =
        CouncilFixture::new_runtime_backed(routed_script(RequestLog::default(), Vec::new()));
    fixture.seed_source_mob(&["forker"]).await;
    let forker = member_surface(&fixture, "forker").await;
    let composed: Arc<dyn AgentToolDispatcher> = Arc::new(meerkat_core::DynamicToolComposite::new(
        vec![Arc::clone(&forker.surface)],
    ));
    let core_default = Duration::from_secs(600);
    let resolution = meerkat_core::ToolExecutionResolutionContext::new(
        meerkat_core::ToolDeadlineChain::new(vec![meerkat_core::ToolDeadlineContributor::finite(
            meerkat_core::ToolDeadlineOwner::CoreToolDispatch,
            core_default,
        )])
        .expect("deadline chain"),
    );
    let args = serde_json::value::RawValue::from_string("{}".to_string()).unwrap();
    for dispatcher in [&forker.surface, &composed] {
        for (tool, expected) in [
            ("fork_off", None),
            ("council", None),
            ("mob_list_members", Some(core_default)),
        ] {
            let plan = meerkat_core::resolve_tool_execution_plan_fenced(
                dispatcher,
                ToolCallView {
                    id: "plan",
                    name: tool,
                    args: &args,
                },
                &meerkat_core::ToolDispatchContext::default(),
                &resolution,
            )
            .unwrap_or_else(|error| panic!("resolve {tool}: {error:?}"));
            assert_eq!(
                plan.effective_timeout(),
                expected,
                "{tool}: effective deadline the agent loop enforces"
            );
        }
    }
    fixture.teardown().await;
}

/// A council that fails (every participant's provider call errors) completes
/// its job as failed, and the durable record carries the typed outcome.
#[tokio::test(flavor = "multi_thread")]
async fn a_failing_detached_council_completes_its_job_as_failed() {
    let log = RequestLog::default();
    let recorder = log.clone();
    let fixture = CouncilFixture::new_runtime_backed(move |request| {
        recorder.record(request);
        let council_turn = role_in_request(request).is_some()
            || user_text(request).contains("bounded plain-text summary");
        if council_turn {
            ScriptedTurn::Fail("participant provider unavailable".to_string())
        } else {
            ScriptedTurn::Text(FOLLOW_UP_REPLY.to_string())
        }
    });
    fixture.seed_source_mob(&["convener", "alice", "bob"]).await;
    let mob_id = fixture.source_mob_id().to_string();
    let convener = bind_surface(
        &fixture.state,
        member_session(&fixture, "convener").await,
        convener_authority(&mob_id),
    );

    let started = call(&convener.surface, "council", council_args(&fixture, None))
        .await
        .expect("council starts");
    assert_eq!(started["status"], "running", "{started}");
    let job_id = started["job_id"].as_str().expect("job id").to_string();

    let record = wait_for_completion(&fixture, &convener.session, &job_id).await;
    assert_eq!(
        record.status,
        BackgroundJobTerminalStatus::Failed,
        "a council whose exit is a failure fails its job: {record:?}"
    );
    let outcome: Value = serde_json::from_str(&record.detail).expect("typed council outcome");
    let reason = outcome["result"]["exit_reason"]["reason"]
        .as_str()
        .unwrap_or_else(|| panic!("typed exit reason in the record: {outcome}"));
    assert_ne!(reason, "completed", "{outcome}");
    assert_one_completion_record(
        &persisted_messages(fixture.service.as_ref(), &convener.session).await,
        "council",
        &job_id,
        reason,
    );
    fixture.teardown().await;
}

// ===========================================================================
// Hosts that cannot deliver detached, and owners that are not live
// ===========================================================================

/// A host that declares detached delivery but has no runtime to admit the
/// completion does not pretend: fork_off and council block for their result
/// and say why, typed, in the result.
#[tokio::test(flavor = "multi_thread")]
async fn a_host_declaring_delivery_without_a_runtime_blocks_and_says_why() {
    let fixture = CouncilFixture::new_without_runtime_adapter(routed_script(
        RequestLog::default(),
        vec![(CHILD_TASK, ChildReply::Text(CHILD_REPLY))],
    ));
    fixture.seed_source_mob(&["forker", "alice", "bob"]).await;
    fixture
        .state
        .set_detached_completion_delivery(DetachedCompletionDelivery::Available);
    assert_eq!(
        fixture.state.detached_delivery_blocked_because(),
        Some(DetachedDeliveryUnavailable::NoRuntimeAdapter)
    );
    let mob_id = fixture.source_mob_id().to_string();
    let forker = member_surface(&fixture, "forker").await;

    let forked = call(
        &forker.surface,
        "fork_off",
        fork_args("unroutable-child", CHILD_TASK),
    )
    .await
    .expect("fork_off blocks for its result");
    assert_eq!(forked["bounded_result"]["text"], CHILD_REPLY, "{forked}");
    assert!(forked.get("job_id").is_none(), "{forked}");
    assert_eq!(forked["blocked_because"], "no_runtime_adapter", "{forked}");

    let convener = bind_surface(
        &fixture.state,
        forker.session.clone(),
        convener_authority(&mob_id),
    );
    let council = call(
        &convener.surface,
        "council",
        council_args(&fixture, Some("no-runtime")),
    )
    .await
    .expect("council blocks for its result");
    assert!(council.get("job_id").is_none(), "{council}");
    assert_eq!(
        council["blocked_because"], "no_runtime_adapter",
        "{council}"
    );
    assert_eq!(
        any_completion_records(
            &persisted_messages(fixture.service.as_ref(), &forker.session).await
        ),
        0,
        "nothing is recorded for results the calls already returned"
    );
    fixture.teardown().await;
}

/// The owner has run a turn and the runtime has since retired its executor
/// when the child finishes: the completion still lands once and wakes it.
#[tokio::test(flavor = "multi_thread")]
async fn detached_completion_reaches_an_owner_whose_executor_was_torn_down() {
    let log = RequestLog::default();
    let gate = TurnGate::new();
    let _release_on_exit = OpenOnDrop(gate.clone());
    let fixture = CouncilFixture::new_runtime_backed(routed_script(
        log.clone(),
        vec![(CHILD_TASK, ChildReply::Gated(gate.clone(), CHILD_REPLY))],
    ));
    fixture.seed_source_mob(&["forker"]).await;
    let forker = member_surface(&fixture, "forker").await;
    assert_eq!(
        drive_turn(&fixture, "forker", "FOLLOW-UP-W warm up").await,
        FOLLOW_UP_REPLY
    );

    let job_id =
        start_detached_fork(&forker, "late-child", fork_args("late-child", CHILD_TASK)).await;
    gate.wait_entered(1).await;
    fixture
        .runtime_adapter
        .as_ref()
        .expect("runtime-backed fixture")
        .unregister_session(&forker.session)
        .await
        .expect("the runtime retires the owner's idle executor");
    gate.open();

    let record = wait_for_completion(&fixture, &forker.session, &job_id).await;
    assert_eq!(
        record.status,
        BackgroundJobTerminalStatus::Completed,
        "{record:?}"
    );
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    loop {
        let woken = log
            .0
            .lock()
            .unwrap()
            .iter()
            .filter(|request| {
                request.rendered.contains(&job_id) && !request.last_user.contains("FOLLOW-UP-A")
            })
            .count();
        if woken >= 1 {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the revived owner was never woken"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let prompt = "FOLLOW-UP-A after the revival";
    assert_eq!(
        drive_turn(&fixture, "forker", prompt).await,
        FOLLOW_UP_REPLY
    );
    let request = log.request_for(prompt);
    assert!(
        request.rendered.contains(&job_id) && request.rendered.contains(CHILD_REPLY),
        "{}",
        request.rendered
    );
    assert_one_completion_record(
        &persisted_messages(fixture.service.as_ref(), &forker.session).await,
        "fork_off",
        &job_id,
        CHILD_REPLY,
    );
    fixture.teardown().await;
}

/// A convener that is a mob member has run a turn, and the runtime retires
/// its idle executor while its detached council runs: the council's
/// completion still lands once, revives the convener through its mob, and
/// wakes it for one turn, as for a fork_off owner.
#[tokio::test(flavor = "multi_thread")]
async fn detached_council_completion_revives_a_convener_whose_executor_was_torn_down() {
    let log = RequestLog::default();
    let gate = TurnGate::new();
    let _release_on_exit = OpenOnDrop(gate.clone());
    let inner = routed_script(log.clone(), Vec::new());
    let gate_for_script = gate.clone();
    // Participants' discussion turns wait on the gate, so the convener's
    // executor is retired while the council is still running.
    let fixture = CouncilFixture::new_runtime_backed(move |request: &LlmRequest| {
        if !support::user_text(request).contains("bounded plain-text summary")
            && let Some(role) = support::role_in_request(request)
        {
            return ScriptedTurn::Gated(gate_for_script.clone(), format!("position from {role}"));
        }
        inner(request)
    });
    fixture.seed_source_mob(&["convener", "alice", "bob"]).await;
    let mob_id = fixture.source_mob_id().to_string();
    let convener = bind_surface(
        &fixture.state,
        member_session(&fixture, "convener").await,
        convener_authority(&mob_id),
    );
    assert_eq!(
        drive_turn(&fixture, "convener", "FOLLOW-UP-W warm up").await,
        FOLLOW_UP_REPLY
    );

    let started = call(&convener.surface, "council", council_args(&fixture, None))
        .await
        .expect("council starts");
    let job_id = started["job_id"].as_str().expect("job id").to_string();
    gate.wait_entered(1).await;
    let runtime = fixture
        .runtime_adapter
        .as_ref()
        .expect("runtime-backed fixture");
    runtime
        .unregister_session(&convener.session)
        .await
        .expect("the runtime retires the convener's idle executor");
    assert!(!runtime.contains_session(&convener.session).await);
    gate.open();

    let record = wait_for_completion(&fixture, &convener.session, &job_id).await;
    assert_eq!(
        record.status,
        BackgroundJobTerminalStatus::Completed,
        "{record:?}"
    );
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    let woken = loop {
        let woken = log
            .0
            .lock()
            .unwrap()
            .iter()
            .filter(|request| {
                request.rendered.contains(&job_id) && !request.last_user.contains("FOLLOW-UP-C")
            })
            .count();
        if woken >= 1 {
            break woken;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the revived convener was never woken"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    };
    assert_eq!(woken, 1, "the convener is woken for one turn");
    assert!(
        runtime.contains_session(&convener.session).await,
        "the convener was revived"
    );
    let prompt = "FOLLOW-UP-C after the revival";
    assert_eq!(
        drive_turn(&fixture, "convener", prompt).await,
        FOLLOW_UP_REPLY
    );
    let request = log.request_for(prompt);
    assert!(
        request.rendered.contains(&job_id) && request.rendered.contains("COUNCIL-SUMMARY-9Z"),
        "{}",
        request.rendered
    );
    assert_one_completion_record(
        &persisted_messages(fixture.service.as_ref(), &convener.session).await,
        "council",
        &job_id,
        "COUNCIL-SUMMARY-9Z",
    );
    fixture.teardown().await;
}

/// The live custodian's delivery waits out a deferred owner revival: the
/// owner is not live and its mob is stopped when the job ends, so revival is
/// deferred; once the mob runs again the completion is delivered, exactly
/// once, without a restart (lifecycle review: the single live attempt used to
/// drop it until the next restart).
#[tokio::test(flavor = "multi_thread")]
async fn live_delivery_waits_for_a_stopped_owner_mob_and_delivers_once() {
    let fixture =
        CouncilFixture::new_runtime_backed(routed_script(RequestLog::default(), Vec::new()));
    fixture.seed_source_mob(&["forker"]).await;
    let handle = source_handle(&fixture).await;
    let owner = member_session(&fixture, "forker").await;
    let runtime = fixture
        .runtime_adapter
        .clone()
        .expect("runtime-backed fixture");
    handle.stop().await.expect("stop the mob");
    runtime
        .unregister_session(&owner)
        .await
        .expect("the owner is not live");

    let job_id = "job-live-deferred".to_string();
    let delivery = tokio::spawn({
        let runtime = Arc::clone(&runtime);
        let handle = handle.clone();
        let owner = owner.clone();
        let job_id = job_id.clone();
        async move {
            meerkat_mob_mcp::deliver_detached_completion_to_member_when_revivable(
                &runtime,
                &handle,
                &AgentIdentity::from("forker"),
                &owner,
                "fork_off",
                &job_id,
                BackgroundJobTerminalStatus::Completed,
                json!({"text": CHILD_REPLY}),
            )
            .await
        }
    });
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        !delivery.is_finished(),
        "delivery waits while the mob is stopped"
    );
    assert!(
        completion_records(
            &persisted_messages(fixture.service.as_ref(), &owner).await,
            &job_id
        )
        .is_empty()
    );

    handle.resume().await.expect("the mob runs again");
    let delivered = tokio::time::timeout(Duration::from_secs(60), delivery)
        .await
        .expect("delivery ends once the mob runs")
        .expect("delivery task");
    assert_eq!(
        delivered,
        Ok(meerkat_mob_mcp::DetachedCompletionDelivered::Delivered)
    );
    wait_for_completion(&fixture, &owner, &job_id).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        completion_records(
            &persisted_messages(fixture.service.as_ref(), &owner).await,
            &job_id
        )
        .len(),
        1,
        "delivered exactly once"
    );
    fixture.teardown().await;
}

/// The fixture's role profile defaults to autonomous_host, which cannot run a
/// tracked turn. fork_off still works: the child runs turn-driven.
#[tokio::test(flavor = "multi_thread")]
async fn fork_off_of_an_autonomous_host_role_runs_the_child_turn_driven() {
    let fixture = CouncilFixture::new_runtime_backed(routed_script(
        RequestLog::default(),
        vec![(CHILD_TASK, ChildReply::Text(CHILD_REPLY))],
    ));
    fixture.seed_source_mob(&["forker"]).await;
    let handle = source_handle(&fixture).await;
    let role = handle
        .definition()
        .profiles
        .get(&meerkat_mob::ProfileName::from("participant"))
        .and_then(|binding| binding.as_inline())
        .expect("inline participant profile")
        .clone();
    assert_eq!(
        role.runtime_mode,
        meerkat_mob::MobRuntimeMode::AutonomousHost,
        "precondition: the role defaults to autonomous_host"
    );
    let forker = member_surface(&fixture, "forker").await;

    let job_id =
        start_detached_fork(&forker, "role-child", fork_args("role-child", CHILD_TASK)).await;
    let record = wait_for_completion(&fixture, &forker.session, &job_id).await;
    assert_eq!(
        record.status,
        BackgroundJobTerminalStatus::Completed,
        "{record:?}"
    );
    assert!(record.detail.contains(CHILD_REPLY), "{record:?}");
    let child = handle
        .get_member(&AgentIdentity::from("role-child"))
        .await
        .expect("get member")
        .expect("the completed child stays seated");
    assert_eq!(child.runtime_mode, meerkat_mob::MobRuntimeMode::TurnDriven);
    fixture.teardown().await;
}

// ===========================================================================
// Ownership on the agent surface
// ===========================================================================

/// Without manage scope the forker checks, lists and retires its own
/// children and nothing else; a session that is not a mob member and holds
/// no manage scope is denied outright.
#[tokio::test(flavor = "multi_thread")]
async fn forker_checks_lists_and_retires_only_its_own_children() {
    let fixture = CouncilFixture::new_runtime_backed(routed_script(
        RequestLog::default(),
        vec![(CHILD_TASK, ChildReply::Text(CHILD_REPLY))],
    ));
    fixture.seed_source_mob(&["forker", "bystander"]).await;
    let mob_id = fixture.source_mob_id().to_string();
    let forker = member_surface(&fixture, "forker").await;
    let bystander = member_surface(&fixture, "bystander").await;

    let job_id =
        start_detached_fork(&forker, "owned-child", fork_args("owned-child", CHILD_TASK)).await;
    wait_for_completion(&fixture, &forker.session, &job_id).await;

    let checked = call(
        &forker.surface,
        "mob_check_member",
        json!({"mob_id": mob_id, "member_id": "owned-child"}),
    )
    .await
    .expect("the forker checks its own child");
    assert_eq!(checked["output_preview"], CHILD_REPLY, "{checked}");
    for (surface, target) in [
        (&forker.surface, "bystander"),
        (&bystander.surface, "owned-child"),
    ] {
        for tool in ["mob_check_member", "mob_retire_member"] {
            assert!(
                matches!(
                    call(
                        surface,
                        tool,
                        json!({"mob_id": mob_id, "member_id": target})
                    )
                    .await,
                    Err(ToolError::AccessDenied { .. })
                ),
                "{tool} on {target} must be denied to a non-owner without manage scope"
            );
        }
    }

    let listed = call(
        &forker.surface,
        "mob_list_members",
        json!({"mob_id": mob_id}),
    )
    .await
    .expect("owner view of the member list");
    assert_eq!(member_names(&listed), vec!["owned-child"], "{listed}");
    let listed = call(
        &bystander.surface,
        "mob_list_members",
        json!({"mob_id": mob_id}),
    )
    .await
    .expect("a member without children gets an empty owner view");
    assert!(member_names(&listed).is_empty(), "{listed}");

    let outsider = bind_surface(&fixture.state, SessionId::new(), forker_authority(&mob_id));
    assert!(
        matches!(
            call(
                &outsider.surface,
                "mob_list_members",
                json!({"mob_id": mob_id})
            )
            .await,
            Err(ToolError::AccessDenied { .. })
        ),
        "a session that is not a member and has no manage scope sees nothing"
    );

    call(
        &forker.surface,
        "mob_retire_member",
        json!({"mob_id": mob_id, "member_id": "owned-child"}),
    )
    .await
    .expect("the forker retires its own child");
    let handle = source_handle(&fixture).await;
    assert_not_seated(&handle, "owned-child").await;
    assert!(
        handle
            .get_member(&AgentIdentity::from("bystander"))
            .await
            .unwrap()
            .is_some()
    );
    fixture.teardown().await;
}

/// A forks C, C forks D from inside its own running turn. A owns D through
/// C: it lists and checks D, which a bystander cannot. When C's opt-in
/// max_run elapses, C is cancelled and retired and so is its running child D.
#[tokio::test(flavor = "multi_thread")]
async fn autokill_cascades_down_the_spawn_tree_the_root_forker_owns() {
    const C_TASK: &str = "C-TASK-2K keep working";
    const D_TASK: &str = "D-TASK-8M keep working";
    // Never opened while the test runs: C and D run until something ends them.
    let gate = TurnGate::new();
    let _release_on_exit = OpenOnDrop(gate.clone());
    let fixture = CouncilFixture::new_runtime_backed(routed_script(
        RequestLog::default(),
        vec![
            (D_TASK, ChildReply::Gated(gate.clone(), "D done")),
            (C_TASK, ChildReply::Gated(gate.clone(), "C done")),
        ],
    ));
    fixture.seed_source_mob(&["tree-a", "bystander"]).await;
    let mob_id = fixture.source_mob_id().to_string();
    let a = member_surface(&fixture, "tree-a").await;
    let bystander = member_surface(&fixture, "bystander").await;

    let c_job = start_detached_fork(
        &a,
        "tree-c",
        json!({"member_id": "tree-c", "task": C_TASK, "max_run_secs": 4}),
    )
    .await;
    gate.wait_entered(1).await;
    let c = member_surface(&fixture, "tree-c").await;
    start_detached_fork(&c, "tree-d", fork_args("tree-d", D_TASK)).await;
    gate.wait_entered(2).await;
    let handle = source_handle(&fixture).await;
    assert_eq!(
        spawned_by(&handle, "tree-d").await,
        Some(AgentIdentity::from("tree-c"))
    );

    let listed = call(&a.surface, "mob_list_members", json!({"mob_id": mob_id}))
        .await
        .expect("owner view");
    assert_eq!(
        member_names(&listed),
        vec!["tree-c", "tree-d"],
        "the root forker's view covers its whole subtree: {listed}"
    );
    call(
        &a.surface,
        "mob_check_member",
        json!({"mob_id": mob_id, "member_id": "tree-d"}),
    )
    .await
    .expect("the root forker checks its grandchild");
    assert!(matches!(
        call(
            &bystander.surface,
            "mob_check_member",
            json!({"mob_id": mob_id, "member_id": "tree-d"}),
        )
        .await,
        Err(ToolError::AccessDenied { .. })
    ));

    let record = wait_for_completion(&fixture, &a.session, &c_job).await;
    // An autokill is Terminated, live as after a restart (the re-link).
    assert_eq!(
        record.status,
        BackgroundJobTerminalStatus::Terminated,
        "{record:?}"
    );
    assert!(record.detail.contains("max_run_elapsed"), "{record:?}");
    assert_not_seated(&handle, "tree-c").await;
    assert_not_seated(&handle, "tree-d").await;
    assert!(
        handle
            .get_member(&AgentIdentity::from("tree-a"))
            .await
            .unwrap()
            .is_some(),
        "the root forker is untouched"
    );
    fixture.teardown().await;
}

/// The root forker retires a grandchild directly, and retiring a child
/// through the tool retires the child's own children too.
#[tokio::test(flavor = "multi_thread")]
async fn root_forker_retires_a_grandchild_and_retirement_cascades() {
    let fixture = CouncilFixture::new_runtime_backed(routed_script(
        RequestLog::default(),
        vec![(CHILD_TASK, ChildReply::Text(CHILD_REPLY))],
    ));
    fixture.seed_source_mob(&["tree-a"]).await;
    let mob_id = fixture.source_mob_id().to_string();
    let a = member_surface(&fixture, "tree-a").await;

    let c_job = start_detached_fork(&a, "tree-c", fork_args("tree-c", CHILD_TASK)).await;
    wait_for_completion(&fixture, &a.session, &c_job).await;
    let c = member_surface(&fixture, "tree-c").await;
    for grandchild in ["tree-d1", "tree-d2"] {
        let job = start_detached_fork(&c, grandchild, fork_args(grandchild, CHILD_TASK)).await;
        wait_for_completion(&fixture, &c.session, &job).await;
    }
    let handle = source_handle(&fixture).await;

    call(
        &a.surface,
        "mob_retire_member",
        json!({"mob_id": mob_id, "member_id": "tree-d1"}),
    )
    .await
    .expect("the root forker retires its grandchild");
    assert_not_seated(&handle, "tree-d1").await;
    assert_eq!(
        spawned_by(&handle, "tree-c").await,
        Some(AgentIdentity::from("tree-a")),
        "retiring a grandchild leaves its parent seated"
    );

    call(
        &a.surface,
        "mob_retire_member",
        json!({"mob_id": mob_id, "member_id": "tree-c"}),
    )
    .await
    .expect("the root forker retires its child");
    assert_not_seated(&handle, "tree-c").await;
    assert_not_seated(&handle, "tree-d2").await;
    fixture.teardown().await;
}

/// Ownership is durable: it survives a mob stop/resume, a restart of the
/// mob state over the same durable stores, and a successor-spec respawn.
#[tokio::test(flavor = "multi_thread")]
async fn spawned_by_survives_resume_restart_and_successor_respawn() {
    let fixture = CouncilFixture::new_runtime_backed(routed_script(
        RequestLog::default(),
        vec![(CHILD_TASK, ChildReply::Text(CHILD_REPLY))],
    ));
    fixture.seed_source_mob(&["forker"]).await;
    let mob_id = fixture.source_mob_id();
    let forker = member_surface(&fixture, "forker").await;
    let job = start_detached_fork(&forker, "kept-child", fork_args("kept-child", CHILD_TASK)).await;
    wait_for_completion(&fixture, &forker.session, &job).await;
    let owner = Some(AgentIdentity::from("forker"));

    fixture.state.mob_stop(&mob_id).await.expect("stop");
    fixture.state.mob_resume(&mob_id).await.expect("resume");
    let handle = source_handle(&fixture).await;
    assert_eq!(
        spawned_by(&handle, "kept-child").await,
        owner,
        "after resume"
    );
    call(
        &forker.surface,
        "mob_check_member",
        json!({"mob_id": mob_id.as_str(), "member_id": "kept-child"}),
    )
    .await
    .expect("the forker still owns its child after resume");

    // Restart: quiesce this state's actors, then reopen the same stores.
    handle.shutdown().await.expect("quiesce the source mob");
    let restarted = fixture.restart_state();
    let restored = restarted.handle_for(&mob_id).await.expect("restored mob");
    assert_eq!(
        spawned_by(&restored, "kept-child").await,
        owner,
        "after restart"
    );
    let forker_after_restart = bind_surface(
        &restarted,
        forker.session.clone(),
        forker_authority(mob_id.as_str()),
    );
    call_when_admitted(
        &forker_after_restart.surface,
        "mob_check_member",
        json!({"mob_id": mob_id.as_str(), "member_id": "kept-child"}),
    )
    .await
    .expect("the forker still owns its child after a restart");

    let mut successor = SpawnMemberSpec::new("participant", AgentIdentity::from("kept-child"));
    successor.runtime_mode = Some(meerkat_mob::MobRuntimeMode::TurnDriven);
    restored
        .respawn_with_successor_spec(successor)
        .await
        .expect("successor respawn");
    assert_eq!(
        spawned_by(&restored, "kept-child").await,
        owner,
        "after a successor-spec respawn"
    );
    call_when_admitted(
        &forker_after_restart.surface,
        "mob_retire_member",
        json!({"mob_id": mob_id.as_str(), "member_id": "kept-child"}),
    )
    .await
    .expect("the forker retires its respawned child");
    assert_not_seated(&restored, "kept-child").await;

    let handles = restarted.mob_handles_snapshot().await.unwrap_or_default();
    for (id, handle) in handles {
        if restarted.mob_destroy(&id).await.is_err() {
            let _ = handle.shutdown().await;
        }
    }
    fixture.teardown().await;
}

/// A detached fork_off whose tool call is dropped while the child's handoff
/// is in flight never strands the child (lifecycle review: the relieved
/// task's synchronous tail can finish after the call was dropped). The test
/// stops polling the call once the child is seated, lets the fork finish on
/// its own task, then drops the call: the child's completion must still
/// reach the forker, although the forker never saw the job id.
#[tokio::test(flavor = "multi_thread")]
async fn a_fork_off_call_dropped_mid_handoff_still_delivers_its_child() {
    let fixture = CouncilFixture::new_runtime_backed(routed_script(
        RequestLog::default(),
        vec![(CHILD_TASK, ChildReply::Text(CHILD_REPLY))],
    ));
    fixture.seed_source_mob(&["forker"]).await;
    let forker = member_surface(&fixture, "forker").await;
    let handle = source_handle(&fixture).await;
    let child = AgentIdentity::from("dropped-call-child");
    let raw = serde_json::value::RawValue::from_string(
        fork_args("dropped-call-child", CHILD_TASK).to_string(),
    )
    .unwrap();
    let mut call_future = forker.surface.dispatch(ToolCallView {
        id: "dropped-call",
        name: "fork_off",
        args: &raw,
    });
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        if futures::poll!(call_future.as_mut()).is_ready() {
            // The call finished before it could be dropped mid-handoff; the
            // delivery below must hold either way.
            break;
        }
        if handle.get_member(&child).await.unwrap().is_some() {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the child was never seated"
        );
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    // Stop polling: the fork finishes on its relieved task. Then drop.
    tokio::time::sleep(Duration::from_millis(500)).await;
    drop(call_future);

    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        let messages = persisted_messages(fixture.service.as_ref(), &forker.session).await;
        let delivered = messages.iter().any(|message| {
            matches!(message, Message::SystemNotice(notice) if notice.blocks.iter().any(|block| {
                matches!(
                    block,
                    SystemNoticeBlock::BackgroundJob { persisted: true, detail: Some(detail), .. }
                        if detail.contains(CHILD_REPLY)
                )
            }))
        });
        if delivered {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the dropped call's child was stranded: no completion reached the forker"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    fixture.teardown().await;
}

/// Observation must not wait for the child's turn: the forker checks a child
/// that is still mid-turn and gets its status back promptly. (mob_check_member
/// is how the fork_off description tells the forker to watch a running child.)
#[tokio::test(flavor = "multi_thread")]
async fn forker_observes_a_running_child_without_waiting_for_its_turn() {
    let gate = TurnGate::new();
    let _release_on_exit = OpenOnDrop(gate.clone());
    let fixture = CouncilFixture::new_runtime_backed(routed_script(
        RequestLog::default(),
        vec![(CHILD_TASK, ChildReply::Gated(gate.clone(), CHILD_REPLY))],
    ));
    fixture.seed_source_mob(&["forker"]).await;
    let mob_id = fixture.source_mob_id().to_string();
    let forker = member_surface(&fixture, "forker").await;
    start_detached_fork(&forker, "busy-child", fork_args("busy-child", CHILD_TASK)).await;
    gate.wait_entered(1).await;

    let checked = tokio::time::timeout(
        Duration::from_secs(10),
        call(
            &forker.surface,
            "mob_check_member",
            json!({"mob_id": mob_id, "member_id": "busy-child"}),
        ),
    )
    .await
    .expect("mob_check_member must not wait for the child's running turn")
    .expect("the forker checks its running child");
    assert_eq!(checked["is_final"], false, "{checked}");
    assert_ne!(
        checked["output_preview"], CHILD_REPLY,
        "the child has not replied yet: {checked}"
    );
    // The read says, typed, that the child is mid-turn, and in plain words
    // which fields are as of its last completed turn.
    assert_eq!(checked["progress"]["run_state"], "run_open", "{checked}");
    assert!(
        checked["note"]
            .as_str()
            .is_some_and(|note| note.contains("still running")),
        "{checked}"
    );
    gate.open();
    fixture.teardown().await;
}

// ===========================================================================
// A result is never owed where it cannot be delivered
// ===========================================================================

/// An owner hook that never has to act in these tests: it reports the owner
/// gone if the custodian ever asks.
struct AbsentOwnerHost;

#[async_trait::async_trait]
impl meerkat_mob_mcp::DetachedOwnerHost for AbsentOwnerHost {
    async fn ensure_owner_live(
        &self,
        _session_id: &SessionId,
    ) -> Result<(), meerkat_mob_mcp::DetachedOwnerError> {
        Err(meerkat_mob_mcp::DetachedOwnerError::OwnerGone {
            detail: "a test session with no durable record".to_string(),
        })
    }
}

/// The detached route is decided per caller. On a host without an owner
/// hook, a mob member's call runs detached (its mob revives it to receive the
/// result) and a plain session's call does not (`no_owner_revival_host`);
/// with an owner hook a plain session's call runs detached too. fork_off and
/// council share this decision (fork_off already requires a member caller).
#[tokio::test(flavor = "multi_thread")]
async fn the_detached_route_requires_an_owner_that_can_be_revived() {
    let fixture =
        CouncilFixture::new_runtime_backed(routed_script(RequestLog::default(), Vec::new()));
    fixture.seed_source_mob(&["forker"]).await;
    let member = member_session(&fixture, "forker").await;
    let plain = SessionId::new();

    assert_eq!(
        fixture
            .state
            .detached_delivery_blocked_because_for(&member)
            .await,
        None,
        "a member's result is delivered detached"
    );
    assert_eq!(
        fixture
            .state
            .detached_delivery_blocked_because_for(&plain)
            .await,
        Some(DetachedDeliveryUnavailable::NoOwnerRevivalHost),
        "a plain session on a hookless host is not owed a detached result"
    );
    fixture
        .state
        .set_detached_owner_host(Some(Arc::new(AbsentOwnerHost)));
    assert_eq!(
        fixture
            .state
            .detached_delivery_blocked_because_for(&plain)
            .await,
        None,
        "an owner hook makes a plain session's result deliverable"
    );
    fixture.teardown().await;
}

/// No detached job is bound to any council in the state's council store.
async fn owed_council_jobs(fixture: &CouncilFixture) -> usize {
    fixture
        .state
        .temporary_council_store_for_tests()
        .list_all()
        .await
        .expect("list councils")
        .iter()
        .filter(|record| record.detached_job.is_some())
        .count()
}

/// A plain-session convener (a top-level REST, MCP-server or keep-alive CLI
/// session) on a host without an owner hook: the council runs in the turn and
/// its sealed result comes back in the call, with the typed reason, and no
/// job is owed.
#[tokio::test(flavor = "multi_thread")]
async fn a_plain_session_convener_on_a_hookless_host_gets_the_council_in_turn() {
    let fixture =
        CouncilFixture::new_runtime_backed(routed_script(RequestLog::default(), Vec::new()));
    fixture.seed_source_mob(&["alice", "bob"]).await;
    let mob_id = fixture.source_mob_id().to_string();
    let convener = bind_surface(
        &fixture.state,
        SessionId::new(),
        convener_authority(&mob_id),
    );

    let outcome = dispatch(
        &convener.surface,
        "council",
        council_args(&fixture, Some("plain-in-turn")),
    )
    .await
    .expect("the council returns its sealed result in the call");
    let result = result_json(&outcome);
    assert!(
        result.get("job_id").is_none(),
        "no job for a convener the result could not reach later: {result}"
    );
    assert!(
        result["result"].to_string().contains("COUNCIL-SUMMARY-9Z"),
        "the sealed result is in the call: {result}"
    );
    assert_eq!(
        result["blocked_because"], "no_owner_revival_host",
        "the result says, typed, why the call ran in the turn: {result}"
    );
    assert_eq!(owed_council_jobs(&fixture).await, 0, "no job is owed");
    fixture.teardown().await;
}

/// A mob member convener on the same hookless host runs its council
/// detached: its mob revives it to receive the result.
#[tokio::test(flavor = "multi_thread")]
async fn a_member_convener_on_a_hookless_host_runs_the_council_detached() {
    let fixture =
        CouncilFixture::new_runtime_backed(routed_script(RequestLog::default(), Vec::new()));
    fixture.seed_source_mob(&["convener", "alice", "bob"]).await;
    let mob_id = fixture.source_mob_id().to_string();
    let convener = bind_surface(
        &fixture.state,
        member_session(&fixture, "convener").await,
        convener_authority(&mob_id),
    );

    let started = result_json(
        &dispatch(
            &convener.surface,
            "council",
            council_args(&fixture, Some("member-detached")),
        )
        .await
        .expect("council starts"),
    );
    assert_eq!(started["status"], "running", "{started}");
    assert!(started.get("blocked_because").is_none(), "{started}");
    let job_id = started["job_id"].as_str().expect("job id").to_string();
    wait_for_completion(&fixture, &convener.session, &job_id).await;
    fixture.teardown().await;
}

/// With an owner hook installed, a plain-session convener's council runs
/// detached as well.
#[tokio::test(flavor = "multi_thread")]
async fn an_owner_hook_lets_a_plain_session_convener_run_the_council_detached() {
    let fixture =
        CouncilFixture::new_runtime_backed(routed_script(RequestLog::default(), Vec::new()));
    fixture.seed_source_mob(&["alice", "bob"]).await;
    fixture
        .state
        .set_detached_owner_host(Some(Arc::new(AbsentOwnerHost)));
    let mob_id = fixture.source_mob_id().to_string();
    let convener = bind_surface(
        &fixture.state,
        SessionId::new(),
        convener_authority(&mob_id),
    );

    let started = result_json(
        &dispatch(
            &convener.surface,
            "council",
            council_args(&fixture, Some("hooked-detached")),
        )
        .await
        .expect("council starts"),
    );
    assert_eq!(started["status"], "running", "{started}");
    assert!(started.get("blocked_because").is_none(), "{started}");
    assert!(started["job_id"].as_str().is_some(), "{started}");
    // The council seals on its own task, with the convener's job bound.
    let store = fixture.state.temporary_council_store_for_tests();
    let council_id = fixture.council_id("hooked-detached");
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    while store
        .load(&council_id)
        .await
        .expect("load council")
        .and_then(|record| record.result)
        .is_none()
    {
        assert!(
            std::time::Instant::now() < deadline,
            "the council never sealed"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert_eq!(owed_council_jobs(&fixture).await, 1, "the job is bound");
    fixture.teardown().await;
}
