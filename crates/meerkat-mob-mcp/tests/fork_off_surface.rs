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
use meerkat_core::service::MobIncarnationId;
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
use support::{
    CouncilFixture, GatedClaimCouncilStore, ScriptedTurn, TurnGate, council_definition,
    last_user_text, role_in_request, user_text,
};

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
#[derive(Clone)]
struct RequestLog {
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
    /// Requests recorded so far; a watch, so waiters wake on each record.
    recorded: Arc<tokio::sync::watch::Sender<usize>>,
}

impl Default for RequestLog {
    fn default() -> Self {
        Self {
            requests: Arc::default(),
            recorded: Arc::new(tokio::sync::watch::Sender::new(0)),
        }
    }
}

impl RequestLog {
    fn record(&self, request: &LlmRequest) {
        self.requests.lock().unwrap().push(RecordedRequest {
            last_user: last_user_text(request),
            rendered: format!("{:?}", request.messages),
        });
        self.recorded
            .send_modify(|recorded| *recorded = recorded.saturating_add(1));
    }

    /// Every recorded request matching `filter`.
    fn matching(&self, filter: impl Fn(&RecordedRequest) -> bool) -> Vec<RecordedRequest> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|request| filter(request))
            .cloned()
            .collect()
    }

    /// Wait until at least one recorded request matches `filter`, woken by
    /// each record; the bound is a hang guard only.
    async fn wait_matching(
        &self,
        filter: impl Fn(&RecordedRequest) -> bool,
        what: &str,
    ) -> Vec<RecordedRequest> {
        let mut recorded = self.recorded.subscribe();
        let reached = tokio::time::timeout(
            Duration::from_secs(120),
            recorded.wait_for(|_| !self.matching(&filter).is_empty()),
        )
        .await;
        assert!(matches!(reached, Ok(Ok(_))), "{what}");
        self.matching(filter)
    }

    /// The single request whose last user message carries `prompt`.
    fn request_for(&self, prompt: &str) -> RecordedRequest {
        let matching = self.matching(|request| request.last_user.contains(prompt));
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

/// The incarnation the live mob `mob_id` runs at in `state`, as its handle's
/// witness: the binding a grant over that mob names.
async fn live_incarnation(
    state: &MobMcpState,
    mob_id: &str,
) -> meerkat_core::service::LiveMobIncarnation {
    state
        .handle_for(&meerkat_mob::MobId::from(mob_id))
        .await
        .expect("live mob handle")
        .live_incarnation()
}

/// A member's fork authority: it may spawn its own role, nothing else. No
/// manage scope, so every observation of another member goes through
/// ownership admission.
fn forker_authority(
    mob_id: &str,
    incarnation: meerkat_core::service::LiveMobIncarnation,
) -> meerkat_core::service::MobToolAuthorityContext {
    let authority = meerkat_runtime::mob_operator_authority::create_only_mob_operator_authority()
        .expect("generated authority");
    let authority =
        meerkat_runtime::mob_operator_authority::set_create_authority(&authority, false)
            .expect("no create scope");
    meerkat_runtime::mob_operator_authority::grant_spawn_profile_in_mob(
        &authority,
        mob_id,
        incarnation,
        "participant",
    )
    .expect("spawnable participant profile, no manage scope")
}

/// A convener's authority: create scope (councils) and manage scope over the
/// source mob whose members participate.
fn convener_authority(
    mob_id: &str,
    incarnation: meerkat_core::service::LiveMobIncarnation,
) -> meerkat_core::service::MobToolAuthorityContext {
    let authority = meerkat_runtime::mob_operator_authority::create_only_mob_operator_authority()
        .expect("generated authority");
    meerkat_runtime::mob_operator_authority::grant_manage_mob(&authority, mob_id, incarnation)
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
        forker_authority(
            fixture.source_mob_id().as_str(),
            live_incarnation(&fixture.state, fixture.source_mob_id().as_str()).await,
        ),
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
/// Only for lifecycle calls on a freshly restored mob, whose resume may still
/// be admitting. Never use it for member-status reads: a status read must
/// succeed on its first attempt (see
/// `forker_checks_its_idle_child_while_an_operator_reads_the_busy_forker`),
/// and a retry loop would hide exactly that failure. Any other error ends the
/// retries.
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
    assert!(
        started.get("cache_inheritance").is_none(),
        "prompt-cache accounting is host-facing, not in the model's fork_off result: {started}"
    );
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
    let is_wake = |request: &RecordedRequest| {
        request.rendered.contains(&job_id) && !request.last_user.contains("FOLLOW-UP")
    };
    let wake_requests = || log.matching(is_wake);
    log.wait_matching(
        is_wake,
        "the idle forker was never woken by its child's completion",
    )
    .await;
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
        convener_authority(&mob_id, live_incarnation(&fixture.state, &mob_id).await),
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
    assert!(
        result.get("cache_inheritance").is_none(),
        "prompt-cache accounting is host-facing, not in the model's fork_off result: {result}"
    );
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
        convener_authority(&mob_id, live_incarnation(&fixture.state, &mob_id).await),
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

/// A council seats forks of the source mob its convener was admitted to, or
/// nothing: when that mob is destroyed and another is created under its id
/// with members of the same identities between the admission and the
/// seating, the council refuses the participant, typed, and never forks a
/// member of the new mob.
#[tokio::test(flavor = "multi_thread")]
async fn a_council_never_seats_a_member_of_a_mob_recreated_under_its_source_id() {
    let gate = TurnGate::new();
    let _open_on_drop = OpenOnDrop(Arc::clone(&gate));
    let council_store: Arc<dyn meerkat_mob::store::TemporaryCouncilStore> =
        Arc::new(GatedClaimCouncilStore::new(
            Arc::new(meerkat_mob::store::InMemoryTemporaryCouncilStore::new()),
            Arc::clone(&gate),
        ));
    let fixture = CouncilFixture::new_runtime_backed_with(
        routed_script(RequestLog::default(), Vec::new()),
        move |state, _root| state.with_temporary_council_store(council_store),
    );
    fixture.seed_source_mob(&["alice", "bob"]).await;
    fixture
        .state
        .set_detached_completion_delivery(DetachedCompletionDelivery::Unavailable);
    let mob_id = fixture.source_mob_id();
    let convener = bind_surface(
        &fixture.state,
        SessionId::new(),
        convener_authority(
            mob_id.as_str(),
            live_incarnation(&fixture.state, mob_id.as_str()).await,
        ),
    );

    let council = dispatch(
        &convener.surface,
        "council",
        council_args(&fixture, Some("recreated-source")),
    );
    let recreate = async {
        gate.wait_entered(1).await;
        fixture
            .state
            .mob_destroy(&mob_id)
            .await
            .expect("the console destroys the admitted source mob");
        fixture.seed_source_mob(&["alice", "bob"]).await;
        gate.open();
    };
    let (outcome, ()) = tokio::join!(council, recreate);

    let result = result_json(&outcome.expect("the council returns its sealed result"));
    let rendered = result.to_string();
    for member in ["alice", "bob"] {
        let replacement = member_session(&fixture, member).await;
        assert!(
            !rendered.contains(&replacement.to_string()),
            "the council forked {member} of the mob created under its source id: {result}"
        );
    }
    assert_eq!(
        result["result"]["exit_reason"]["reason"], "participant_source_replaced",
        "{result}"
    );
    assert_eq!(
        result["result"]["exit_reason"]["participant_order"], 0,
        "{result}"
    );
    let participants = result["result"]["participants"]
        .as_array()
        .expect("participants");
    assert!(
        participants.iter().all(|participant| {
            participant["seated"] == false && participant.get("capability").is_none()
        }),
        "no participant holds a capability or a seat: {result}"
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
        convener_authority(&mob_id, live_incarnation(&fixture.state, &mob_id).await),
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
// Implicit runtime delivery and owners that are not live
// ===========================================================================

/// Omitting an explicit adapter uses the persistent service's retained owner.
/// Both detached tools record their outcome once and preserve it for later turns.
#[tokio::test(flavor = "multi_thread")]
async fn an_implicit_service_runtime_delivers_fork_and_council_completions() {
    let log = RequestLog::default();
    let fixture = CouncilFixture::new_with_implicit_runtime_adapter(routed_script(
        log.clone(),
        vec![(CHILD_TASK, ChildReply::Text(CHILD_REPLY))],
    ));
    assert!(
        fixture.runtime_adapter.is_none(),
        "no explicit adapter supplied"
    );
    fixture.seed_source_mob(&["forker", "alice", "bob"]).await;
    assert_eq!(fixture.state.detached_delivery_blocked_because(), None);
    let runtime = <PersistentService as meerkat_mob::MobSessionService>::acquire_runtime_adapter(
        fixture.service.as_ref(),
        None,
    )
    .expect("acquire the service's retained runtime")
    .expect("persistent service has a runtime owner");
    let mob_id = fixture.source_mob_id().to_string();
    let forker = member_surface(&fixture, "forker").await;

    let fork_job = start_detached_fork(
        &forker,
        "implicit-owner-child",
        fork_args("implicit-owner-child", CHILD_TASK),
    )
    .await;
    let fork_record = wait_for_completion(&fixture, &forker.session, &fork_job).await;
    assert_eq!(
        fork_record.status,
        BackgroundJobTerminalStatus::Completed,
        "{fork_record:?}"
    );

    let convener = bind_surface(
        &fixture.state,
        forker.session.clone(),
        convener_authority(&mob_id, live_incarnation(&fixture.state, &mob_id).await),
    );
    let council = call(
        &convener.surface,
        "council",
        council_args(&fixture, Some("implicit-owner")),
    )
    .await
    .expect("council starts detached");
    assert_eq!(council["status"], "running", "{council}");
    assert!(council.get("blocked_because").is_none(), "{council}");
    let council_job = council["job_id"].as_str().expect("council job id");
    assert_ne!(fork_job, council_job);
    let council_record = wait_for_completion(&fixture, &convener.session, council_job).await;
    assert_eq!(
        council_record.status,
        BackgroundJobTerminalStatus::Completed,
        "{council_record:?}"
    );

    let prompt = "FOLLOW-UP-I what did the fork and council find?";
    assert_eq!(
        drive_turn(&fixture, "forker", prompt).await,
        FOLLOW_UP_REPLY
    );
    let request = log.request_for(prompt);
    assert!(
        request.rendered.contains(&fork_job)
            && request.rendered.contains(CHILD_REPLY)
            && request.rendered.contains(council_job)
            && request.rendered.contains(COUNCIL_SUMMARY),
        "the implicit owner carries both completions into its next model request: {}",
        request.rendered
    );
    assert!(
        runtime.contains_session(&forker.session).await,
        "the service's retained owner runs the member"
    );
    let messages = persisted_messages(fixture.service.as_ref(), &forker.session).await;
    assert_one_completion_record(&messages, "fork_off", &fork_job, CHILD_REPLY);
    assert_one_completion_record(&messages, "council", council_job, COUNCIL_SUMMARY);
    assert_eq!(any_completion_records(&messages), 2);
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
    log.wait_matching(
        |request| request.rendered.contains(&job_id) && !request.last_user.contains("FOLLOW-UP-A"),
        "the revived owner was never woken",
    )
    .await;
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
        convener_authority(&mob_id, live_incarnation(&fixture.state, &mob_id).await),
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
    let woken = log
        .wait_matching(
            |request| {
                request.rendered.contains(&job_id) && !request.last_user.contains("FOLLOW-UP-C")
            },
            "the revived convener was never woken",
        )
        .await
        .len();
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

/// The live custodian's completion outlives an owner that cannot be served
/// when the job ends: the convener is not live and its mob is stopped, so it
/// cannot be revived. The outcome is committed and submitted durably, nothing
/// is recorded while the mob is stopped, and once the mob runs again the
/// completion is applied exactly once, with no restart (P3-B B1 and B2: the
/// live path waited on a bounded revival loop and then dropped the outcome
/// until the next restart).
#[tokio::test(flavor = "multi_thread")]
async fn a_live_completion_for_an_unservable_owner_is_applied_once_it_can_be_served() {
    let log = RequestLog::default();
    let gate = TurnGate::new();
    let _release_on_exit = OpenOnDrop(gate.clone());
    let inner = routed_script(log.clone(), Vec::new());
    let gate_for_script = gate.clone();
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
        convener_authority(&mob_id, live_incarnation(&fixture.state, &mob_id).await),
    );
    let started = call(&convener.surface, "council", council_args(&fixture, None))
        .await
        .expect("council starts");
    let job_id = started["job_id"].as_str().expect("job id").to_string();
    gate.wait_entered(1).await;

    // The convener's mob stops and its executor is retired while the council
    // runs: when the council ends, the convener cannot be revived.
    let handle = source_handle(&fixture).await;
    handle.stop().await.expect("stop the convener's mob");
    let runtime = fixture
        .runtime_adapter
        .clone()
        .expect("runtime-backed fixture");
    runtime
        .unregister_session(&convener.session)
        .await
        .expect("the convener is not live");
    gate.open();

    // The council's terminal is committed on its custody record.
    let council_store = fixture.state.temporary_council_store_for_tests();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    loop {
        let committed = council_store
            .list_all()
            .await
            .expect("council records")
            .into_iter()
            .any(|record| {
                record
                    .detached_job
                    .is_some_and(|job| job.job_id == job_id && job.terminal.is_some())
            });
        if committed {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the council's outcome was never committed"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        completion_records(
            &persisted_messages(fixture.service.as_ref(), &convener.session).await,
            &job_id
        )
        .is_empty(),
        "nothing is recorded while the convener cannot be served"
    );

    handle.resume().await.expect("the mob runs again");
    // Stopping the mob also ends the council's participants, so the council
    // itself may fail; what is delivered is whatever outcome it committed.
    wait_for_completion(&fixture, &convener.session, &job_id).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        completion_records(
            &persisted_messages(fixture.service.as_ref(), &convener.session).await,
            &job_id
        )
        .len(),
        1,
        "applied exactly once"
    );
    fixture.teardown().await;
}

/// P3-B B.4 at the producer: a completion the job's owner session already
/// took under `{tool}:{job_id}` (delivered live before an upgrade, or before
/// the member was repointed to another session) is not submitted again,
/// wherever the owner serves now: no continuation is committed and nothing
/// reaches the owner's current session. A completion not yet taken is
/// submitted, and once applied a repeat submission is skipped too.
#[tokio::test(flavor = "multi_thread")]
async fn the_sink_skips_a_completion_the_owner_session_already_took_even_after_a_repoint() {
    let fixture =
        CouncilFixture::new_runtime_backed(routed_script(RequestLog::default(), Vec::new()));
    fixture.seed_source_mob(&["forker", "successor"]).await;
    let handle = source_handle(&fixture).await;
    let taken = member_session(&fixture, "forker").await;
    let current = member_session(&fixture, "successor").await;
    let runtime = fixture
        .runtime_adapter
        .clone()
        .expect("runtime-backed fixture");
    let outcome = json!({"status": "completed", "text": CHILD_REPLY});
    support::admit_pre_upgrade_completion(
        &runtime,
        &handle,
        &AgentIdentity::from("forker"),
        &taken,
        "fork_off",
        "job-b4",
        BackgroundJobTerminalStatus::Completed,
        outcome.clone(),
    )
    .await
    .expect("the job's session takes the completion");
    wait_for_completion(&fixture, &taken, "job-b4").await;

    // The owner now serves on another session (as after a repoint).
    let skipped = fixture
        .state
        .submit_detached_completion_for_tests(
            Some((handle.clone(), AgentIdentity::from("successor"))),
            &taken,
            "fork_off",
            "job-b4",
            BackgroundJobTerminalStatus::Completed,
            outcome.clone(),
        )
        .await
        .expect("submit");
    assert_eq!(
        skipped,
        meerkat_mob_mcp::DetachedCompletionDelivered::AlreadyDelivered
    );
    let continuations = fixture.continuations.as_ref().expect("continuations");
    let status_reader = meerkat::ContinuationOwnerService::new(
        continuations.inbox.clone(),
        continuations.bindings.resolver().expect("bound resolver"),
        Arc::clone(&runtime),
    );
    assert_eq!(
        status_reader
            .continuation_status(
                &meerkat::ContinuationOwner::Member {
                    mob_id: fixture.source_mob_id().to_string(),
                    identity: "successor".to_string(),
                },
                &meerkat::ContinuationKey::new("fork_off:job-b4").expect("key"),
            )
            .await
            .expect("status"),
        meerkat::ContinuationStatus::NotCommitted,
        "nothing is committed for a completion already taken"
    );
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        completion_records(
            &persisted_messages(fixture.service.as_ref(), &current).await,
            "job-b4"
        )
        .is_empty(),
        "the owner's current session never sees it"
    );

    // A completion not yet taken is submitted, applied once, and a repeat
    // submission after it was applied is skipped.
    let submit = || {
        fixture.state.submit_detached_completion_for_tests(
            Some((handle.clone(), AgentIdentity::from("successor"))),
            &current,
            "fork_off",
            "job-b4-new",
            BackgroundJobTerminalStatus::Completed,
            outcome.clone(),
        )
    };
    assert_eq!(
        submit().await.expect("submit"),
        meerkat_mob_mcp::DetachedCompletionDelivered::Delivered
    );
    wait_for_completion(&fixture, &current, "job-b4-new").await;
    assert_eq!(
        submit().await.expect("submit again"),
        meerkat_mob_mcp::DetachedCompletionDelivered::AlreadyDelivered
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        completion_records(
            &persisted_messages(fixture.service.as_ref(), &current).await,
            "job-b4-new"
        )
        .len(),
        1
    );
    fixture.teardown().await;
}

/// The job owner confirms a fork_off outcome only from the mob that ran the
/// job, and a restarted host binds its continuations before it restores its
/// mobs. Until the restore is done a job it cannot find waits (a governed
/// completion stays pending instead of being refused for a mob that comes
/// back); once the mob set is complete the job is found, and a job no mob
/// holds is absent for good.
#[tokio::test(flavor = "multi_thread")]
async fn the_job_owner_waits_for_the_restore_before_calling_a_fork_job_absent() {
    let fixture = CouncilFixture::new_runtime_backed(routed_script(
        RequestLog::default(),
        vec![(CHILD_TASK, ChildReply::Text(CHILD_REPLY))],
    ));
    fixture.seed_source_mob(&["forker"]).await;
    let forker = member_surface(&fixture, "forker").await;
    let job_id = start_detached_fork(
        &forker,
        "restore-child",
        fork_args("restore-child", CHILD_TASK),
    )
    .await;
    wait_for_completion(&fixture, &forker.session, &job_id).await;

    let bindings = Arc::clone(
        &fixture
            .continuations
            .as_ref()
            .expect("continuations")
            .bindings,
    );
    let lookup = |job_id: &str| {
        let (bindings, job_id) = (Arc::clone(&bindings), job_id.to_string());
        async move {
            bindings
                .job_source()
                .expect("a bound job owner")
                .retained_job(&meerkat::ContinuationProducer::ForkOff, &job_id)
                .await
        }
    };
    assert!(matches!(
        lookup(&job_id).await,
        meerkat::RetainedJobLookup::Found(_)
    ));

    // The process dies; the restarted host binds before anything restores.
    fixture.shut_down_predecessor().await;
    let restarted = fixture.restart_state();
    assert!(
        matches!(
            lookup(&job_id).await,
            meerkat::RetainedJobLookup::Unavailable(_)
        ),
        "a job in a mob not restored yet waits"
    );
    assert!(matches!(
        lookup("never-forked").await,
        meerkat::RetainedJobLookup::Unavailable(_)
    ));

    // Any mob verb restores the mobs, and the set is complete.
    restarted
        .mob_handles_snapshot()
        .await
        .expect("restore the mobs");
    match lookup(&job_id).await {
        meerkat::RetainedJobLookup::Found(facts) => {
            assert_eq!(facts.owner_session_id, forker.session);
        }
        _ => panic!("the restored mob confirms its job"),
    }
    assert!(
        matches!(
            lookup("never-forked").await,
            meerkat::RetainedJobLookup::Absent
        ),
        "a job no mob holds once the set is complete is absent"
    );
    fixture.teardown().await;
}

/// A continuation owed to a member of a mob this host does not manage waits
/// (the host may insert the mob's handle later), but only while the mob can
/// still come back. A mob this state destroyed, or one missing once the mob
/// set is known complete (persistent restore finished, or the host declared
/// its own restore done), is retired, so its rows strand instead of waiting
/// for good.
#[tokio::test(flavor = "multi_thread")]
async fn a_member_address_of_a_mob_gone_for_good_is_retired() {
    let fixture =
        CouncilFixture::new_runtime_backed(routed_script(RequestLog::default(), Vec::new()));
    fixture.seed_source_mob(&["worker"]).await;
    let handle = source_handle(&fixture).await;
    let generation = handle
        .roster()
        .await
        .get_by_identity(&AgentIdentity::from("worker"))
        .expect("worker")
        .generation
        .get();
    let mob_id = fixture.source_mob_id();
    let address = meerkat::member_delivery_address(
        mob_id.as_str(),
        handle.incarnation(),
        "worker",
        generation,
    )
    .expect("address");
    let bindings = Arc::clone(
        &fixture
            .continuations
            .as_ref()
            .expect("continuations")
            .bindings,
    );
    assert!(matches!(
        bindings.resolve_address(&address).await,
        meerkat::AddressResolution::Session(_)
    ));

    // A handle kept here would own the mob database's observer, and the
    // destroy would leave its storage cleanup outstanding.
    drop(handle);
    fixture
        .state
        .mob_destroy(&mob_id)
        .await
        .expect("destroy the mob");
    assert_eq!(
        bindings.resolve_address(&address).await,
        meerkat::AddressResolution::Retired,
        "a mob this state destroyed never serves again"
    );
    let never =
        meerkat::member_delivery_address("never-existed", MobIncarnationId::mint(), "worker", 1)
            .expect("address");
    assert_eq!(
        bindings.resolve_address(&never).await,
        meerkat::AddressResolution::Retired,
        "a mob missing after the persistent restore is gone for good"
    );

    // A host that inserts restored handles itself (no persistent root): an
    // unknown mob waits until the host declares its restore done.
    let inserting = MobMcpState::new_with_runtime_adapter(
        fixture.service.clone(),
        fixture.runtime_adapter.clone(),
        meerkat_mob::MobControlPrincipal::Owner,
    )
    .expect("construct runtime authority")
    .into_shared();
    fixture.bind_continuations(&inserting);
    let late = meerkat::member_delivery_address("late-mob", MobIncarnationId::mint(), "worker", 1)
        .expect("address");
    assert_eq!(
        bindings.resolve_address(&late).await,
        meerkat::AddressResolution::NotServed,
        "its handle may still be inserted"
    );
    inserting.declare_mob_set_restored();
    assert_eq!(
        bindings.resolve_address(&late).await,
        meerkat::AddressResolution::Retired
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

    let outsider = bind_surface(
        &fixture.state,
        SessionId::new(),
        forker_authority(&mob_id, live_incarnation(&fixture.state, &mob_id).await),
    );
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

fn assert_member_absence(
    result: Result<Value, ToolError>,
    kind: &str,
    member: &str,
    context: &str,
) {
    match result {
        Err(ToolError::ExecutionFailedWithData { data, .. }) => {
            assert_eq!(data["kind"], kind, "{context}: {data}");
            assert_eq!(data["member_id"], member, "{context}: {data}");
            assert_eq!(data["retryable"], false, "{context}: {data}");
        }
        other => panic!("{context}: expected a typed {kind} error, got {other:?}"),
    }
}

/// #1234: a retired or missing target is typed not-found before ownership,
/// for manage-scope and non-manage callers alike. The spawner of a retired
/// child gets member_retired (the event log records the retirement), an
/// identity the mob never held gets member_not_found, and access_denied is
/// kept for a present member the caller does not own.
#[tokio::test(flavor = "multi_thread")]
async fn owned_member_tools_type_a_retired_or_missing_target_as_not_found() {
    let fixture = CouncilFixture::new_runtime_backed(routed_script(
        RequestLog::default(),
        vec![(CHILD_TASK, ChildReply::Text(CHILD_REPLY))],
    ));
    fixture.seed_source_mob(&["forker", "bystander"]).await;
    let mob_id = fixture.source_mob_id().to_string();
    let forker = member_surface(&fixture, "forker").await;
    let bystander = member_surface(&fixture, "bystander").await;
    let manager = bind_surface(
        &fixture.state,
        SessionId::new(),
        convener_authority(&mob_id, live_incarnation(&fixture.state, &mob_id).await),
    );

    let job_id =
        start_detached_fork(&forker, "gone-child", fork_args("gone-child", CHILD_TASK)).await;
    wait_for_completion(&fixture, &forker.session, &job_id).await;
    call(
        &forker.surface,
        "mob_check_member",
        json!({"mob_id": mob_id, "member_id": "gone-child"}),
    )
    .await
    .expect("the forker checks its present child");

    // Retired out from under its spawner, as by the idle sweep.
    let handle = source_handle(&fixture).await;
    handle
        .retire(AgentIdentity::from("gone-child"))
        .await
        .expect("retire the child");
    assert_not_seated(&handle, "gone-child").await;

    for tool in ["mob_check_member", "mob_retire_member"] {
        for (surface, who) in [
            (&forker.surface, "its spawner"),
            (&bystander.surface, "a non-owner"),
            (&manager.surface, "a manage-scope caller"),
        ] {
            assert_member_absence(
                call(
                    surface,
                    tool,
                    json!({"mob_id": mob_id, "member_id": "gone-child"}),
                )
                .await,
                "member_retired",
                "gone-child",
                &format!("{tool} on a retired child by {who}"),
            );
        }
        for (surface, who) in [
            (&forker.surface, "a non-manage member"),
            (&manager.surface, "a manage-scope caller"),
        ] {
            assert_member_absence(
                call(
                    surface,
                    tool,
                    json!({"mob_id": mob_id, "member_id": "never-seated"}),
                )
                .await,
                "member_not_found",
                "never-seated",
                &format!("{tool} on an unknown identity by {who}"),
            );
        }
        assert!(
            matches!(
                call(
                    &forker.surface,
                    tool,
                    json!({"mob_id": mob_id, "member_id": "bystander"}),
                )
                .await,
                Err(ToolError::AccessDenied { .. })
            ),
            "{tool}: a present member the caller does not own stays access_denied"
        );
    }
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
        forker_authority(mob_id.as_str(), restored.live_incarnation()),
    );
    // The restored mob's fork re-link pass reads its children's status
    // concurrently; the forker's check neither waits on it nor is refused.
    call(
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

/// Marker of a forker turn held open at its model call.
const FORKER_HOLD: &str = "FORKER-HOLD-3K keep this turn open";

/// The field incident: an operator harness polled the forker's status
/// while the forker was mid-turn, each read of the busy forker held the mob's
/// single status permit, and the forker's own `mob_check_member` on its idle
/// fork child was refused (`observation_lane_saturated`). The check must
/// succeed on its first attempt, with no retry loop to hide a refusal.
#[tokio::test(flavor = "multi_thread")]
async fn forker_checks_its_idle_child_while_an_operator_reads_the_busy_forker() {
    let gate = TurnGate::new();
    let _release_on_exit = OpenOnDrop(gate.clone());
    let fixture = CouncilFixture::new_runtime_backed(routed_script(
        RequestLog::default(),
        vec![
            (CHILD_TASK, ChildReply::Text(CHILD_REPLY)),
            (
                FORKER_HOLD,
                ChildReply::Gated(gate.clone(), FOLLOW_UP_REPLY),
            ),
        ],
    ));
    fixture.seed_source_mob(&["forker"]).await;
    let mob_id = fixture.source_mob_id();
    let forker = member_surface(&fixture, "forker").await;
    let job_id =
        start_detached_fork(&forker, "idle-child", fork_args("idle-child", CHILD_TASK)).await;
    wait_for_completion(&fixture, &forker.session, &job_id).await;

    // The forker is mid-turn: its session task serves no status request.
    let handle = source_handle(&fixture).await;
    let spec = BoundedResultSpec::new("held", 4096).expect("bounded result spec");
    let held_turn = handle
        .start_work_for_identity_bounded(
            AgentIdentity::from("forker"),
            WorkSpec::new(
                ContentInput::Text(FORKER_HOLD.to_string()),
                WorkOrigin::Internal,
            ),
            HandlingMode::Queue,
            spec.clone(),
        )
        .await
        .expect("start the held forker turn");
    gate.wait_entered(1).await;

    // An operator keeps reading the busy forker's status, as the harness's
    // once-a-second identity inspection did; each read of a busy member runs
    // for at least the execution-snapshot bound.
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let operator = {
        let state = Arc::clone(&fixture.state);
        let mob_id = mob_id.clone();
        let stop = Arc::clone(&stop);
        tokio::spawn(async move {
            let mut longest = Duration::ZERO;
            while !stop.load(std::sync::atomic::Ordering::SeqCst) {
                let started = std::time::Instant::now();
                let _ = state
                    .mob_member_status(&mob_id, &AgentIdentity::from("forker"))
                    .await;
                longest = longest.max(started.elapsed());
            }
            longest
        })
    };
    tokio::time::sleep(Duration::from_millis(50)).await;

    let checked = tokio::time::timeout(
        Duration::from_secs(10),
        call(
            &forker.surface,
            "mob_check_member",
            json!({"mob_id": mob_id.as_str(), "member_id": "idle-child"}),
        ),
    )
    .await
    .expect("the check does not wait on the operator's read of the forker")
    .expect("the forker's check of its idle child succeeds on the first attempt");
    assert_eq!(checked["output_preview"], CHILD_REPLY, "{checked}");
    assert!(
        checked.get("preview_unavailable").is_none(),
        "the idle child's preview is observed: {checked}"
    );

    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    let longest_operator_read = operator.await.expect("operator poller");
    assert!(
        longest_operator_read >= Duration::from_millis(200),
        "the operator's reads of the busy forker were held open: {longest_operator_read:?}"
    );
    gate.open();
    tokio::time::timeout(Duration::from_secs(60), held_turn.wait_bounded(spec))
        .await
        .expect("the held forker turn finishes once released")
        .expect("the held forker turn completes");
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
        convener_authority(&mob_id, live_incarnation(&fixture.state, &mob_id).await),
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
        convener_authority(&mob_id, live_incarnation(&fixture.state, &mob_id).await),
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
        convener_authority(&mob_id, live_incarnation(&fixture.state, &mob_id).await),
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

/// An owner hook that counts its calls and cannot make anything live.
#[derive(Default)]
struct CountingOwnerHost(std::sync::atomic::AtomicUsize);

#[async_trait::async_trait]
impl meerkat_mob_mcp::DetachedOwnerHost for CountingOwnerHost {
    async fn ensure_owner_live(
        &self,
        _session_id: &SessionId,
    ) -> Result<(), meerkat_mob_mcp::DetachedOwnerError> {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Err(meerkat_mob_mcp::DetachedOwnerError::Failed {
            detail: "this hook is not the way to revive a mob member".to_string(),
        })
    }
}

/// On a host with an owner hook, a member convener is still revived through
/// its mob: the route's one membership read decides the custodian's owner,
/// and the hook is never asked (lifecycle review: the gate skipped the read
/// when a hook existed, and a second read decided the owner separately).
#[tokio::test(flavor = "multi_thread")]
async fn a_hooked_host_revives_a_member_convener_through_its_mob() {
    let gate = TurnGate::new();
    let _release_on_exit = OpenOnDrop(gate.clone());
    let inner = routed_script(RequestLog::default(), Vec::new());
    let gate_for_script = gate.clone();
    let fixture = CouncilFixture::new_runtime_backed(move |request: &LlmRequest| {
        if !support::user_text(request).contains("bounded plain-text summary")
            && let Some(role) = support::role_in_request(request)
        {
            return ScriptedTurn::Gated(gate_for_script.clone(), format!("position from {role}"));
        }
        inner(request)
    });
    fixture.seed_source_mob(&["convener", "alice", "bob"]).await;
    let host = Arc::new(CountingOwnerHost::default());
    fixture.state.set_detached_owner_host(Some(host.clone()));
    let mob_id = fixture.source_mob_id().to_string();
    let convener = bind_surface(
        &fixture.state,
        member_session(&fixture, "convener").await,
        convener_authority(&mob_id, live_incarnation(&fixture.state, &mob_id).await),
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
    gate.open();

    let record = wait_for_completion(&fixture, &convener.session, &job_id).await;
    assert_eq!(
        record.status,
        BackgroundJobTerminalStatus::Completed,
        "{record:?}"
    );
    assert_eq!(
        host.0.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "the owner hook is not asked for a mob member"
    );
    fixture.teardown().await;
}

// ===========================================================================
// A status read observes the owner; only delivery serves it
// ===========================================================================

/// The committed owner of one host setup result, in its own namespace.
struct OneSetupJob(meerkat::RetainedJobFacts);

#[async_trait::async_trait]
impl meerkat::RetainedJobSource for OneSetupJob {
    async fn retained_job(
        &self,
        producer: &meerkat::ContinuationProducer,
        job_id: &str,
    ) -> meerkat::RetainedJobLookup {
        if *producer == setup_producer() && job_id == "setup-1" {
            meerkat::RetainedJobLookup::Found(self.0.clone())
        } else {
            meerkat::RetainedJobLookup::Absent
        }
    }
}

fn setup_producer() -> meerkat::ContinuationProducer {
    meerkat::ContinuationProducer::Host {
        namespace: "status-read".to_string(),
    }
}

/// The host setup result owed to its owner as a continuation.
fn setup_completion() -> meerkat::ContinuationDelivery {
    meerkat::ContinuationDelivery {
        key: meerkat::ContinuationKey::new("setup:setup-1").expect("key"),
        result: meerkat::ContinuationResultRef {
            producer: setup_producer(),
            producer_id: "setup-1".to_string(),
            result_digest: "sha256:status-read".to_string(),
            summary: None,
        },
        body: "the setup finished".into(),
        handling: meerkat::ContinuationHandling::Queue,
    }
}

/// An owner hook whose first ask, the delivery owner's own resolution,
/// parks until the test releases it (`true`: the owner is made live). Every
/// later ask is counted and refused at once.
struct ParkedOwnerHost {
    asks: std::sync::atomic::AtomicUsize,
    entered: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    release: tokio::sync::Mutex<Option<tokio::sync::oneshot::Receiver<bool>>>,
}

impl ParkedOwnerHost {
    fn new() -> (
        Arc<Self>,
        tokio::sync::oneshot::Receiver<()>,
        tokio::sync::oneshot::Sender<bool>,
    ) {
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        (
            Arc::new(Self {
                asks: std::sync::atomic::AtomicUsize::new(0),
                entered: Mutex::new(Some(entered_tx)),
                release: tokio::sync::Mutex::new(Some(release_rx)),
            }),
            entered_rx,
            release_tx,
        )
    }

    fn asks(&self) -> usize {
        self.asks.load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl meerkat_mob_mcp::DetachedOwnerHost for ParkedOwnerHost {
    async fn ensure_owner_live(
        &self,
        _session_id: &SessionId,
    ) -> Result<(), meerkat_mob_mcp::DetachedOwnerError> {
        if self.asks.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
            if let Some(entered) = self.entered.lock().expect("entered").take() {
                let _ = entered.send(());
            }
            let release = self.release.lock().await.take();
            if let Some(release) = release
                && release.await == Ok(true)
            {
                return Ok(());
            }
        }
        Err(meerkat_mob_mcp::DetachedOwnerError::Failed {
            detail: "the owner is not live".to_string(),
        })
    }
}

/// Reading a continuation's status never makes its owner live. The delivery
/// owner's own resolution of the row is parked inside the owner hook, an
/// acknowledged cutpoint after which no delivery work can ask the hook
/// again. Through the bound service (which outlives its binding), the status
/// read and the read-only retained observation report the row pending and
/// never ask the hook. Delivery still serves the owner: released as live,
/// the parked resolution applies the completion to its exact session, and
/// the hook was asked that one time only.
#[tokio::test(flavor = "multi_thread")]
async fn a_status_read_never_revives_the_owner_and_delivery_still_does() {
    let fixture =
        CouncilFixture::new_runtime_backed(routed_script(RequestLog::default(), Vec::new()));
    fixture.seed_source_mob(&["forker"]).await;
    let session_id = member_session(&fixture, "forker").await;
    let (host, entered, release) = ParkedOwnerHost::new();
    fixture.state.set_detached_owner_host(Some(host.clone()));
    let continuations = fixture
        .state
        .continuation_service()
        .expect("the bound continuation service");
    let owner = meerkat::ContinuationOwner::Session {
        session_id: session_id.clone(),
    };
    let jobs = OneSetupJob(meerkat::RetainedJobFacts {
        owner_session_id: session_id.clone(),
        retained_work: None,
        result_digest: "sha256:status-read".to_string(),
    });
    let record = meerkat::RetainedJobRecord::from_owner(&jobs, setup_producer(), "setup-1")
        .await
        .expect("read the setup owner")
        .expect("the setup result is committed");
    let delivery = setup_completion();
    continuations
        .submit_retained_completion(&owner, delivery.clone(), &record, 1)
        .await
        .expect("commit the completion");
    tokio::time::timeout(Duration::from_secs(60), entered)
        .await
        .expect("the delivery owner resolves the row")
        .expect("the hook remains owned");

    let status = continuations
        .continuation_status(&owner, &delivery.key)
        .await
        .expect("status");
    assert!(
        matches!(
            status,
            meerkat::ContinuationStatus::Pending { admitted: None, .. }
        ),
        "{status:?}"
    );
    let observed = continuations
        .observe_retained_completion(&owner, &delivery, &record)
        .await
        .expect("observe");
    assert!(
        matches!(
            observed,
            meerkat::ContinuationObservation::Matched {
                status: meerkat::ContinuationStatus::Pending { admitted: None, .. },
                ..
            }
        ),
        "{observed:?}"
    );
    assert_eq!(
        host.asks(),
        1,
        "only the delivery owner's resolution asked the owner hook"
    );

    release
        .send(true)
        .expect("release the parked resolution as live");
    let applied = tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            if let meerkat::ContinuationStatus::Applied { session, .. } = continuations
                .continuation_status(&owner, &delivery.key)
                .await
                .expect("status")
            {
                break session;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("delivery applies the completion");
    assert_eq!(applied, session_id, "applied to its exact owner session");
    assert_eq!(
        host.asks(),
        1,
        "delivery served the owner once; no status read asked"
    );
    fixture.teardown().await;
}

/// A row for a member of a mob this host does not manage, committed through
/// a service that resolves only that member's address.
struct ForeignMember(meerkat_runtime::LogicalRuntimeId);

#[async_trait::async_trait]
impl meerkat::ContinuationAddressResolver for ForeignMember {
    async fn current_address(
        &self,
        _owner: &meerkat::ContinuationOwner,
    ) -> Result<Option<meerkat_runtime::LogicalRuntimeId>, String> {
        Ok(Some(self.0.clone()))
    }

    async fn resolve_address(
        &self,
        _address: &meerkat_runtime::LogicalRuntimeId,
    ) -> Result<meerkat::AddressResolution, String> {
        Ok(meerkat::AddressResolution::NotServed)
    }
}

/// A status read strands a row only when the owner's records prove it gone.
/// A plain session that does not read proves nothing (it may not be
/// persisted yet), so its row stays pending. A member of a mob this host
/// does not manage stays pending while the host may still register that
/// mob, and is stranded once the host declared every mob it will have.
#[tokio::test(flavor = "multi_thread")]
async fn a_status_read_strands_only_an_owner_whose_records_prove_it_gone() {
    let fixture =
        CouncilFixture::new_runtime_backed(routed_script(RequestLog::default(), Vec::new()));
    let continuations = fixture
        .state
        .continuation_service()
        .expect("the bound continuation service");

    let absent = meerkat::ContinuationOwner::Session {
        session_id: SessionId::new(),
    };
    let delivery = setup_completion();
    let receipt = continuations
        .submit(&absent, delivery.clone(), 1)
        .await
        .expect("commit the continuation");
    let status = continuations
        .continuation_status(&absent, &delivery.key)
        .await
        .expect("status");
    assert!(
        matches!(
            &status,
            meerkat::ContinuationStatus::Pending { receipt: pending, admitted: None, .. }
                if pending == &receipt
        ),
        "mere absence is not retirement: {status:?}"
    );

    let address =
        meerkat::member_delivery_address("foreign-mob", MobIncarnationId::mint(), "absent", 1)
            .expect("foreign member address");
    let member = meerkat::ContinuationOwner::Member {
        mob_id: "foreign-mob".to_string(),
        identity: "absent".to_string(),
    };
    let foreign = meerkat::ContinuationOwnerService::new(
        fixture
            .continuations
            .as_ref()
            .expect("the fixture's continuations")
            .inbox
            .clone(),
        Arc::new(ForeignMember(address)),
        fixture
            .runtime_adapter
            .clone()
            .expect("runtime-backed fixture"),
    );
    let foreign_receipt = foreign
        .submit(&member, delivery.clone(), 1)
        .await
        .expect("commit the foreign member's continuation");
    let status = continuations
        .continuation_status(&member, &delivery.key)
        .await
        .expect("status");
    assert!(
        matches!(
            &status,
            meerkat::ContinuationStatus::Pending { receipt: pending, admitted: None, .. }
                if pending == &foreign_receipt
        ),
        "the host may still register that mob: {status:?}"
    );
    fixture.state.declare_mob_set_restored();
    assert_eq!(
        continuations
            .continuation_status(&member, &delivery.key)
            .await
            .expect("status"),
        meerkat::ContinuationStatus::Stranded {
            receipt: foreign_receipt,
            cause: meerkat::StrandedCause::OwnerRetired,
        },
        "the host's complete mob set proves that mob gone"
    );
    fixture.teardown().await;
}

/// A mob destroyed and created again under its id is a new mob incarnation
/// whose generations start over. A row still owed to the destroyed mob's
/// member (a detached job's late completion, say) names that mob
/// incarnation, so the delivery pass finds it retired and the row strands:
/// it never reaches the new member under the same identity and generation,
/// whose own rows the pass applies.
#[tokio::test(flavor = "multi_thread")]
async fn a_row_owed_to_a_destroyed_mobs_member_never_reaches_the_mob_created_again() {
    let fixture =
        CouncilFixture::new_runtime_backed(routed_script(RequestLog::default(), Vec::new()));
    fixture.seed_source_mob(&["worker"]).await;
    let mob_id = fixture.source_mob_id();
    let worker = AgentIdentity::from("worker");
    let handle = source_handle(&fixture).await;
    let destroyed = handle.incarnation();
    let generation = handle
        .roster()
        .await
        .get_by_identity(&worker)
        .expect("worker")
        .generation
        .get();
    let owed_address =
        meerkat::member_delivery_address(mob_id.as_str(), destroyed, "worker", generation)
            .expect("address");
    // A handle kept here would own the mob database's observer, and the
    // destroy would leave its storage cleanup outstanding.
    drop(handle);
    fixture
        .state
        .mob_destroy(&mob_id)
        .await
        .expect("destroy the mob");

    fixture.seed_source_mob(&["worker"]).await;
    let handle = source_handle(&fixture).await;
    assert_ne!(
        handle.incarnation(),
        destroyed,
        "a create starts a new mob incarnation"
    );
    assert_eq!(
        handle
            .roster()
            .await
            .get_by_identity(&worker)
            .expect("worker")
            .generation
            .get(),
        generation,
        "the new mob's generations start over"
    );
    drop(handle);
    let successor_session = member_session(&fixture, "worker").await;

    let owner = meerkat::ContinuationOwner::Member {
        mob_id: mob_id.to_string(),
        identity: "worker".to_string(),
    };
    let late = meerkat::ContinuationOwnerService::new(
        fixture
            .continuations
            .as_ref()
            .expect("the fixture's continuations")
            .inbox
            .clone(),
        Arc::new(ForeignMember(owed_address.clone())),
        fixture
            .runtime_adapter
            .clone()
            .expect("runtime-backed fixture"),
    );
    let owed = late
        .submit(&owner, member_result("owed"), 1)
        .await
        .expect("commit the destroyed member's row");
    assert_eq!(owed.address, owed_address.0);
    let continuations = fixture
        .state
        .continuation_service()
        .expect("the bound continuation service");
    let current = continuations
        .submit(&owner, member_result("current"), 2)
        .await
        .expect("commit the new member's row");
    assert_ne!(
        current.address, owed.address,
        "the new member has its own address"
    );

    let applied = tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            if let meerkat::ContinuationStatus::Applied { session, .. } = continuations
                .continuation_status(&owner, &current.key)
                .await
                .expect("status")
            {
                break session;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("the delivery pass applies the new member's row");
    assert_eq!(applied, successor_session);
    assert_eq!(
        continuations
            .continuation_status(&owner, &owed.key)
            .await
            .expect("status"),
        meerkat::ContinuationStatus::Stranded {
            receipt: owed.clone(),
            cause: meerkat::StrandedCause::OwnerRetired,
        },
        "the destroyed member's row is never applied to the new member"
    );
    fixture.teardown().await;
}

/// A host result owed to a mob member under the key `tasks:{key}`.
fn member_result(key: &str) -> meerkat::ContinuationDelivery {
    meerkat::ContinuationDelivery {
        key: meerkat::ContinuationKey::new(format!("tasks:{key}")).expect("key"),
        result: meerkat::ContinuationResultRef {
            producer: meerkat::ContinuationProducer::Host {
                namespace: "tasks".to_string(),
            },
            producer_id: key.to_string(),
            result_digest: format!("sha256:{key}"),
            summary: None,
        },
        body: format!("task {key} finished").into(),
        handling: meerkat::ContinuationHandling::Queue,
    }
}

/// An ephemeral session whose live actor is discarded without archiving
/// reads as absent, and the host restores it under the same id from the
/// snapshot it kept. A retained continuation owed to it stays pending through
/// that absence (its status and its read-only retained observation), never
/// stranded, and the same-id restore succeeds.
#[tokio::test(flavor = "multi_thread")]
async fn a_status_read_keeps_a_discarded_ephemeral_owner_pending_through_its_same_id_restore() {
    use meerkat_core::service::SessionService as _;

    let temp = tempfile::tempdir().expect("temp dir");
    let mut builder = meerkat::FactoryAgentBuilder::new(
        meerkat::AgentFactory::new(temp.path().join("sessions")).builtins(false),
        meerkat::Config::default(),
    );
    builder.default_llm_client = Some(Arc::new(support::ScriptedCouncilClient::new(
        routed_script(RequestLog::default(), Vec::new()),
    )));
    let service = Arc::new(meerkat_session::EphemeralSessionService::new(builder, 4));
    let state = MobMcpState::new_with_runtime_adapter(
        service.clone(),
        None,
        meerkat_mob::MobControlPrincipal::Owner,
    )
    .expect("the ephemeral service supplies its runtime")
    .into_shared();
    let bindings = meerkat::ContinuationHostBindings::default();
    state
        .bind_continuations(
            meerkat_runtime::RuntimeDeliveryInbox::new(Arc::new(
                meerkat_runtime::InMemoryRuntimeStore::new(),
            )),
            &bindings,
        )
        .expect("bind the continuation services");
    let continuations = state
        .continuation_service()
        .expect("the bound continuation service");
    let request = |build| meerkat_core::service::CreateSessionRequest {
        injected_context: Vec::new(),
        model: "claude-haiku-4-5-20251001".to_string(),
        prompt: ContentInput::Text(String::new()),
        system_prompt: meerkat_core::SystemPromptOverride::Inherit,
        max_tokens: None,
        event_tx: None,
        initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
        deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
        build: Some(build),
        labels: None,
    };
    let session_id = service
        .create_session(request(
            meerkat_core::service::SessionBuildOptions::default(),
        ))
        .await
        .expect("create the ephemeral session")
        .session_id;
    let owner = meerkat::ContinuationOwner::Session {
        session_id: session_id.clone(),
    };
    let jobs = OneSetupJob(meerkat::RetainedJobFacts {
        owner_session_id: session_id.clone(),
        retained_work: None,
        result_digest: "sha256:status-read".to_string(),
    });
    let record = meerkat::RetainedJobRecord::from_owner(&jobs, setup_producer(), "setup-1")
        .await
        .expect("read the setup owner")
        .expect("the setup result is committed");
    let delivery = setup_completion();
    let receipt = continuations
        .submit_retained_completion(&owner, delivery.clone(), &record, 1)
        .await
        .expect("commit the retained continuation");

    let snapshot = service
        .export_session(&session_id)
        .await
        .expect("the host keeps the session's snapshot");
    service
        .discard_live_session(&session_id)
        .await
        .expect("discard the live actor without archiving");
    assert!(
        matches!(
            service.read(&session_id).await,
            Err(meerkat_core::service::SessionError::NotFound { .. })
        ),
        "the discarded session reads as absent"
    );
    let pending = |status: &meerkat::ContinuationStatus| {
        matches!(
            status,
            meerkat::ContinuationStatus::Pending { receipt: pending, admitted: None, .. }
                if pending == &receipt
        )
    };
    let status = continuations
        .continuation_status(&owner, &delivery.key)
        .await
        .expect("status");
    assert!(
        pending(&status),
        "a recoverable absence is not retirement: {status:?}"
    );
    let observed = continuations
        .observe_retained_completion(&owner, &delivery, &record)
        .await
        .expect("observe");
    assert!(
        matches!(
            &observed,
            meerkat::ContinuationObservation::Matched { status, .. } if pending(status)
        ),
        "{observed:?}"
    );

    let mut restore = meerkat_core::service::SessionBuildOptions::default();
    restore.resume_existing_session(snapshot);
    let restored = tokio::time::timeout(
        Duration::from_secs(60),
        service.create_session(request(restore)),
    )
    .await
    .expect("the restore is bounded")
    .expect("the session is restored under the same id");
    assert_eq!(restored.session_id, session_id);
    service
        .read(&session_id)
        .await
        .expect("the restored session reads");
    let status = continuations
        .continuation_status(&owner, &delivery.key)
        .await
        .expect("status");
    assert!(
        pending(&status),
        "still owed to the restored session: {status:?}"
    );
}

// ===========================================================================
// Delivery strands a session owner only on its own lifecycle proof
// ===========================================================================

/// An owner hook the test toggles: while the host does not hold the owner it
/// answers "gone", which a host answers for a session it holds no record of
/// right now (an RPC host answers `SESSION_NOT_FOUND` for a temporary absence
/// as for a deleted session); while it holds it, the owner is live. Counts
/// every ask.
struct TogglingOwnerHost {
    holds: std::sync::atomic::AtomicBool,
    asks: std::sync::atomic::AtomicUsize,
}

impl TogglingOwnerHost {
    fn holding(holds: bool) -> Arc<Self> {
        Arc::new(Self {
            holds: std::sync::atomic::AtomicBool::new(holds),
            asks: std::sync::atomic::AtomicUsize::new(0),
        })
    }

    fn hold(&self) {
        self.holds.store(true, std::sync::atomic::Ordering::SeqCst);
    }

    fn asks(&self) -> usize {
        self.asks.load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl meerkat_mob_mcp::DetachedOwnerHost for TogglingOwnerHost {
    async fn ensure_owner_live(
        &self,
        _session_id: &SessionId,
    ) -> Result<(), meerkat_mob_mcp::DetachedOwnerError> {
        self.asks.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if self.holds.load(std::sync::atomic::Ordering::SeqCst) {
            Ok(())
        } else {
            Err(meerkat_mob_mcp::DetachedOwnerError::OwnerGone {
                detail: "the host holds no record of the session right now".to_string(),
            })
        }
    }
}

/// Commit the host setup result owed to `session_id`, and return the row's
/// receipt with the delivery owner's first pass that holds the owner's
/// address blocked: the row is owed, not retired.
async fn owe_setup_result(
    continuations: &meerkat::ContinuationOwnerService,
    passes: &mut tokio::sync::watch::Receiver<meerkat::RuntimeDeliveryPass>,
    session_id: &SessionId,
) -> (meerkat::ContinuationReceipt, meerkat::RuntimeDeliveryPass) {
    let jobs = OneSetupJob(meerkat::RetainedJobFacts {
        owner_session_id: session_id.clone(),
        retained_work: None,
        result_digest: "sha256:status-read".to_string(),
    });
    let record = meerkat::RetainedJobRecord::from_owner(&jobs, setup_producer(), "setup-1")
        .await
        .expect("read the setup owner")
        .expect("the setup result is committed");
    let receipt = continuations
        .submit_retained_completion(
            &meerkat::ContinuationOwner::Session {
                session_id: session_id.clone(),
            },
            setup_completion(),
            &record,
            1,
        )
        .await
        .expect("commit the retained setup result");
    let address = meerkat_runtime::LogicalRuntimeId::for_session(session_id);
    let blocked =
        support::wait_for_pass(passes, "the owed row held blocked, not retired", |pass| {
            pass.blocked_runtimes.contains(&address)
        })
        .await;
    (receipt, blocked)
}

/// Whether `status` is the owed row `receipt`, pending and never admitted.
fn owed(status: &meerkat::ContinuationStatus, receipt: &meerkat::ContinuationReceipt) -> bool {
    matches!(
        status,
        meerkat::ContinuationStatus::Pending { receipt: pending, admitted: None, .. }
            if pending == receipt
    )
}

/// Delivery keeps a retained completion owed to an ephemeral session whose
/// live actor was discarded without archiving: the session reads as absent,
/// which proves no retirement, so the delivery owner holds its runtime
/// blocked instead of dropping it as drained. Restored under the same id from
/// the host's snapshot, the session reads again, and the bound source's hint
/// retries the blocked row, which admits the completion as that session's
/// input exactly once. The owner is a session of the service's own runtime
/// machine, prepared the way a surface prepares one (native bindings, no
/// executor), because only such a session can be admitted a continuation:
/// this proves the admission, not a processed turn. The raw restore commits
/// no attachment, so it proves the restore together with the hint, not a
/// wake from the restore.
#[tokio::test(flavor = "multi_thread")]
async fn delivery_keeps_a_discarded_ephemeral_owner_owed_until_a_hint_after_its_restore() {
    use meerkat_core::service::SessionService as _;

    let temp = tempfile::tempdir().expect("temp dir");
    let mut builder = meerkat::FactoryAgentBuilder::new(
        meerkat::AgentFactory::new(temp.path().join("sessions")).builtins(false),
        meerkat::Config::default(),
    );
    builder.default_llm_client = Some(Arc::new(support::ScriptedCouncilClient::new(
        routed_script(RequestLog::default(), Vec::new()),
    )));
    let service = Arc::new(meerkat_session::EphemeralSessionService::new(builder, 4));
    let state = MobMcpState::new_with_runtime_adapter(
        service.clone(),
        None,
        meerkat_mob::MobControlPrincipal::Owner,
    )
    .expect("the ephemeral service supplies its runtime")
    .into_shared();
    let machine = meerkat_mob::MobSessionService::acquire_runtime_adapter(service.as_ref(), None)
        .expect("acquire the service's machine")
        .expect("an ephemeral service owns its machine");
    let delivery_host = support::FixtureContinuations::arm_over(
        &service,
        &machine,
        meerkat_runtime::RuntimeDeliveryInbox::new(Arc::new(
            meerkat_runtime::InMemoryRuntimeStore::new(),
        )),
        true,
    );
    delivery_host.bind(&state);
    let mut passes = delivery_host.passes.clone();
    let continuations = state
        .continuation_service()
        .expect("the bound continuation service");
    let request = |build| meerkat_core::service::CreateSessionRequest {
        injected_context: Vec::new(),
        model: "claude-haiku-4-5-20251001".to_string(),
        prompt: ContentInput::Text(String::new()),
        system_prompt: meerkat_core::SystemPromptOverride::Inherit,
        max_tokens: None,
        event_tx: None,
        initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
        deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
        build: Some(build),
        labels: None,
    };
    let prepared = meerkat::surface::prepare_surface_session(&machine)
        .await
        .expect("prepare the owner's runtime bindings");
    let mut build = meerkat_core::service::SessionBuildOptions::default();
    build.mint_session_with_id(prepared.session_id.clone());
    build.runtime_build_mode = meerkat_core::RuntimeBuildMode::SessionOwned(prepared.bindings);
    let session_id = service
        .create_session(request(build))
        .await
        .expect("create the owner session in the service's machine")
        .session_id;
    assert_eq!(session_id, prepared.session_id);
    let owner = meerkat::ContinuationOwner::Session {
        session_id: session_id.clone(),
    };
    let address = meerkat_runtime::LogicalRuntimeId::for_session(&session_id);
    let key = setup_completion().key;

    let snapshot = service
        .export_session(&session_id)
        .await
        .expect("the host keeps the session's snapshot");
    service
        .discard_live_session(&session_id)
        .await
        .expect("discard the live actor without archiving");
    assert!(
        matches!(
            service.read(&session_id).await,
            Err(meerkat_core::service::SessionError::NotFound { .. })
        ),
        "the discarded session reads as absent"
    );

    let (receipt, blocked) = owe_setup_result(&continuations, &mut passes, &session_id).await;
    assert_eq!(
        (blocked.applied, blocked.refused),
        (0, 0),
        "never settled: {blocked:?}"
    );
    let status = continuations
        .continuation_status(&owner, &key)
        .await
        .expect("status");
    assert!(owed(&status, &receipt), "{status:?}");

    let mut restore = meerkat_core::service::SessionBuildOptions::default();
    restore.resume_existing_session(snapshot);
    let restored = tokio::time::timeout(
        Duration::from_secs(60),
        service.create_session(request(restore)),
    )
    .await
    .expect("the restore is bounded")
    .expect("the session is restored under the same id");
    assert_eq!(restored.session_id, session_id);
    service
        .read(&session_id)
        .await
        .expect("the restored session reads");

    let hinted = passes.borrow().generation;
    delivery_host.bindings.source_custody_changed();
    let applied =
        support::wait_for_pass(&mut passes, "the hinted retry admitted the row", |pass| {
            pass.applied > 0
        })
        .await;
    assert!(
        applied.generation > hinted,
        "admitted by the owner's retry after the hint: {applied:?}"
    );
    assert_eq!(applied.applied, 1, "admitted exactly once: {applied:?}");
    assert!(
        !applied.blocked_runtimes.contains(&address),
        "nothing stays blocked: {applied:?}"
    );
    let status = continuations
        .continuation_status(&owner, &key)
        .await
        .expect("status");
    assert!(
        matches!(
            &status,
            meerkat::ContinuationStatus::Applied { receipt: applied_receipt, session, .. }
                if applied_receipt == &receipt && session == &session_id
        ),
        "admitted as its exact owner session's input: {status:?}"
    );
}

/// The owner hook's "gone" is not a retirement proof. The owner session
/// reads, but the hook answers it gone (a host answers so for a session
/// absent only for the moment); its lifecycle proves no retirement, so the
/// delivery owner keeps the row owed and its runtime blocked instead of
/// dropping it as drained. Once the host holds the session again, the bound
/// source's hint retries the blocked row, and the hook serves the owner,
/// which is applied the completion exactly once.
#[tokio::test(flavor = "multi_thread")]
async fn an_owner_hook_gone_answer_without_lifecycle_proof_keeps_the_row_owed_until_a_hint() {
    let fixture = CouncilFixture::new_runtime_backed_with_source_hints(
        routed_script(RequestLog::default(), Vec::new()),
        Arc::new(meerkat_runtime::InMemoryRuntimeStore::new()),
    );
    fixture.seed_source_mob(&["forker"]).await;
    let session_id = member_session(&fixture, "forker").await;
    let host = TogglingOwnerHost::holding(false);
    fixture.state.set_detached_owner_host(Some(host.clone()));
    let bound = fixture
        .continuations
        .as_ref()
        .expect("the runtime-backed fixture's continuations");
    let mut passes = bound.passes.clone();
    let continuations = fixture
        .state
        .continuation_service()
        .expect("the bound continuation service");
    let owner = meerkat::ContinuationOwner::Session {
        session_id: session_id.clone(),
    };
    let address = meerkat_runtime::LogicalRuntimeId::for_session(&session_id);
    let key = setup_completion().key;

    let (receipt, blocked) = owe_setup_result(&continuations, &mut passes, &session_id).await;
    assert_eq!(
        (blocked.applied, blocked.refused),
        (0, 0),
        "never settled: {blocked:?}"
    );
    assert!(host.asks() >= 1, "delivery asked the owner hook");
    let status = continuations
        .continuation_status(&owner, &key)
        .await
        .expect("status");
    assert!(owed(&status, &receipt), "{status:?}");

    host.hold();
    let hinted = passes.borrow().generation;
    bound.bindings.source_custody_changed();
    let applied = support::wait_for_pass(&mut passes, "the hinted retry applied the row", |pass| {
        pass.applied > 0
    })
    .await;
    assert!(
        applied.generation > hinted,
        "applied by the owner's retry after the hint: {applied:?}"
    );
    assert_eq!(applied.applied, 1, "applied exactly once: {applied:?}");
    assert!(
        !applied.blocked_runtimes.contains(&address),
        "nothing stays blocked: {applied:?}"
    );
    let status = continuations
        .continuation_status(&owner, &key)
        .await
        .expect("status");
    assert!(
        matches!(
            &status,
            meerkat::ContinuationStatus::Applied { receipt: applied_receipt, session, .. }
                if applied_receipt == &receipt && session == &session_id
        ),
        "applied to its exact owner session: {status:?}"
    );
    fixture.teardown().await;
}

/// A failed read of the owner session answers nothing about the owner:
/// delivery keeps the row owed and its runtime blocked, never asks the owner
/// hook and admits nothing, though the hook would serve the owner. Once the
/// session reads again, the bound source's hint retries the blocked row, the
/// hook serves the owner, and the completion is applied exactly once.
#[tokio::test(flavor = "multi_thread")]
async fn a_failed_owner_read_keeps_the_row_owed_without_asking_the_owner_hook() {
    let store = Arc::new(support::commit_gate::CommitGateRuntimeStore::new());
    let fixture = CouncilFixture::new_runtime_backed_with_source_hints(
        routed_script(RequestLog::default(), Vec::new()),
        store.clone(),
    );
    fixture.seed_source_mob(&["forker"]).await;
    let session_id = member_session(&fixture, "forker").await;
    let host = TogglingOwnerHost::holding(true);
    fixture.state.set_detached_owner_host(Some(host.clone()));
    let bound = fixture
        .continuations
        .as_ref()
        .expect("the runtime-backed fixture's continuations");
    let mut passes = bound.passes.clone();
    let continuations = fixture
        .state
        .continuation_service()
        .expect("the bound continuation service");
    let owner = meerkat::ContinuationOwner::Session {
        session_id: session_id.clone(),
    };
    let address = meerkat_runtime::LogicalRuntimeId::for_session(&session_id);
    let key = setup_completion().key;

    store.fail_session_reads(address.clone());
    let unreadable = fixture.state.session_service().read(&session_id).await;
    assert!(
        matches!(
            &unreadable,
            Err(error) if !matches!(error, meerkat_core::service::SessionError::NotFound { .. })
        ),
        "the owner session cannot be read: {:?}",
        unreadable.map(|_| ())
    );
    let (receipt, blocked) = owe_setup_result(&continuations, &mut passes, &session_id).await;
    assert_eq!(
        (blocked.applied, blocked.refused),
        (0, 0),
        "nothing was admitted: {blocked:?}"
    );
    assert_eq!(
        host.asks(),
        0,
        "the owner hook is not asked for an unreadable owner"
    );
    let status = continuations
        .continuation_status(&owner, &key)
        .await
        .expect("status");
    assert!(owed(&status, &receipt), "{status:?}");

    store.restore_session_reads();
    let hinted = passes.borrow().generation;
    bound.bindings.source_custody_changed();
    let applied = support::wait_for_pass(&mut passes, "the hinted retry applied the row", |pass| {
        pass.applied > 0
    })
    .await;
    assert!(
        applied.generation > hinted,
        "applied by the owner's retry after the hint: {applied:?}"
    );
    assert_eq!(applied.applied, 1, "applied exactly once: {applied:?}");
    assert!(
        host.asks() >= 1,
        "the retry served the owner through the hook"
    );
    let status = continuations
        .continuation_status(&owner, &key)
        .await
        .expect("status");
    assert!(
        matches!(
            &status,
            meerkat::ContinuationStatus::Applied { receipt: applied_receipt, session, .. }
                if applied_receipt == &receipt && session == &session_id
        ),
        "applied to its exact owner session: {status:?}"
    );
    fixture.teardown().await;
}

/// The observer-failure control: a lifecycle that cannot be observed proves
/// nothing. The owner hook answers the owner gone while the owner's resume
/// authority cannot be read, so delivery keeps the row owed and its runtime
/// blocked instead of settling it on a failed observation, and a status read
/// reports the owner unavailable, typed. Once the lifecycle reads again it
/// proves no retirement (the owner is active): the hint's retry still keeps
/// the row owed, and the status reads it pending.
#[tokio::test(flavor = "multi_thread")]
async fn an_unobservable_owner_lifecycle_never_retires_its_row() {
    let store = Arc::new(support::commit_gate::CommitGateRuntimeStore::new());
    let fixture = CouncilFixture::new_runtime_backed_with_source_hints(
        routed_script(RequestLog::default(), Vec::new()),
        store.clone(),
    );
    fixture.seed_source_mob(&["forker"]).await;
    let session_id = member_session(&fixture, "forker").await;
    let host = TogglingOwnerHost::holding(false);
    fixture.state.set_detached_owner_host(Some(host.clone()));
    let bound = fixture
        .continuations
        .as_ref()
        .expect("the runtime-backed fixture's continuations");
    let mut passes = bound.passes.clone();
    let continuations = fixture
        .state
        .continuation_service()
        .expect("the bound continuation service");
    let owner = meerkat::ContinuationOwner::Session {
        session_id: session_id.clone(),
    };
    let address = meerkat_runtime::LogicalRuntimeId::for_session(&session_id);
    let key = setup_completion().key;

    store.fail_resume_observations(address.clone());
    let (receipt, blocked) = owe_setup_result(&continuations, &mut passes, &session_id).await;
    assert_eq!(
        (blocked.applied, blocked.refused),
        (0, 0),
        "never settled: {blocked:?}"
    );
    assert!(host.asks() >= 1, "delivery asked the owner hook");
    let unavailable = continuations.continuation_status(&owner, &key).await;
    assert!(
        matches!(
            unavailable,
            Err(meerkat::ContinuationStatusError::Unavailable(_))
        ),
        "{unavailable:?}"
    );

    store.restore_resume_observations();
    let hinted = passes.borrow().generation;
    let asked = host.asks();
    let retry_host = Arc::clone(&host);
    bound.bindings.source_custody_changed();
    // The retry resolved the row again: the hook was asked once more.
    let retried =
        support::wait_for_pass(&mut passes, "the hinted retry resolved the row", |pass| {
            pass.generation > hinted && retry_host.asks() > asked
        })
        .await;
    assert!(
        retried.blocked_runtimes.contains(&address),
        "an active owner is not retired: the row stays owed: {retried:?}"
    );
    assert_eq!(
        (retried.applied, retried.refused),
        (0, 0),
        "never settled: {retried:?}"
    );
    let status = continuations
        .continuation_status(&owner, &key)
        .await
        .expect("status");
    assert!(owed(&status, &receipt), "{status:?}");
    fixture.teardown().await;
}

// ===========================================================================
// fork_off of a member with an activated host instruction
// ===========================================================================

const POLICY_CHILD_TASK: &str = "policy-child-task-marker";
const POLICY_CHILD_REPLY: &str = "policy child reply";
const STANDING_BODY: &str = "Always state your uncertainty explicitly.";

/// Seed the forker on a model that can represent an ordered mid-conversation
/// System activation, and activate one host instruction on it.
async fn seed_activated_forker(fixture: &CouncilFixture) {
    let mut definition = council_definition(fixture.source_mob_id().as_str());
    for binding in definition.profiles.values_mut() {
        if let Some(profile) = meerkat_mob::ProfileBinding::as_inline_mut(binding) {
            profile.model = "claude-opus-5".to_string();
        }
    }
    fixture.seed_source_mob_from(definition, &["forker"]).await;
    source_handle(fixture)
        .await
        .activate_member_instruction(
            &AgentIdentity::from("forker"),
            meerkat_core::InstructionActivationRequest {
                revision: meerkat_core::InstructionRevisionRef {
                    namespace: meerkat_core::InstructionNamespace::new("host.standing").unwrap(),
                    key: meerkat_core::InstructionKey::new("member").unwrap(),
                    revision_id: meerkat_core::InstructionRevisionId::new("rev-standing-1")
                        .unwrap(),
                    content_sha256: meerkat_core::InstructionContentDigest::for_body(STANDING_BODY),
                },
                activation_id: meerkat_core::InstructionActivationId::new("standing-1").unwrap(),
                expectation: meerkat_core::InstructionActivationExpectation::Absent,
                supersedes: None,
                body: STANDING_BODY.to_string(),
            },
        )
        .await
        .expect("the host activates an instruction on the forker");
}

/// fork_off starts the child's turn itself, so under the default policy the
/// tool call is refused, typed, before any child is committed or seated.
#[tokio::test(flavor = "multi_thread")]
async fn fork_off_of_an_activated_member_is_refused_typed_by_default() {
    let log = RequestLog::default();
    let fixture = CouncilFixture::new_runtime_backed(routed_script(
        log.clone(),
        vec![(POLICY_CHILD_TASK, ChildReply::Text(POLICY_CHILD_REPLY))],
    ));
    seed_activated_forker(&fixture).await;
    fixture
        .state
        .set_detached_completion_delivery(DetachedCompletionDelivery::Unavailable);
    let forker = member_surface(&fixture, "forker").await;

    let outcome = dispatch(
        &forker.surface,
        "fork_off",
        fork_args("policy-child", POLICY_CHILD_TASK),
    )
    .await;
    match outcome {
        Err(ToolError::ExecutionFailedWithData { data, .. }) => {
            assert_eq!(data["kind"], "fork_instruction_refused", "{data}");
            assert_eq!(
                data["fork_instruction_refusal"]["cause"]["kind"], "policy_refuses",
                "{data}"
            );
        }
        other => panic!("expected the typed fork instruction refusal, got {other:?}"),
    }
    assert!(
        log.matching(|request| request.last_user.contains(POLICY_CHILD_TASK))
            .is_empty(),
        "no child turn reached the model"
    );
    assert_not_seated(&source_handle(&fixture).await, "policy-child").await;
    fixture.teardown().await;
}

/// With `ReactivateInherited` the child runs its task with the instruction.
#[tokio::test(flavor = "multi_thread")]
async fn fork_off_of_an_activated_member_runs_when_the_host_opts_in() {
    let log = RequestLog::default();
    let fixture = CouncilFixture::new_runtime_backed_with(
        routed_script(
            log.clone(),
            vec![(POLICY_CHILD_TASK, ChildReply::Text(POLICY_CHILD_REPLY))],
        ),
        |state, _root| {
            state.with_fork_instruction_policy(
                meerkat_core::ForkInstructionPolicy::ReactivateInherited,
            )
        },
    );
    seed_activated_forker(&fixture).await;
    fixture
        .state
        .set_detached_completion_delivery(DetachedCompletionDelivery::Unavailable);
    let forker = member_surface(&fixture, "forker").await;

    let outcome = dispatch(
        &forker.surface,
        "fork_off",
        fork_args("policy-child", POLICY_CHILD_TASK),
    )
    .await
    .unwrap_or_else(|error| panic!("the child runs its task: {error:?}"));
    let result = result_json(&outcome);
    assert_eq!(
        result["bounded_result"]["text"], POLICY_CHILD_REPLY,
        "{result}"
    );
    let child_request = log.request_for(POLICY_CHILD_TASK);
    // The inherited row reaches the model as a header that names the key;
    // the body comes once, from the child's own re-activation.
    assert_eq!(
        child_request.rendered.matches(STANDING_BODY).count(),
        1,
        "the child's request carries the body once, from its own activation"
    );
    assert_eq!(
        child_request
            .rendered
            .matches("[meerkat-inherited-instruction-v2]")
            .count(),
        1,
        "the inherited history row is a header without its body"
    );
    fixture.teardown().await;
}
