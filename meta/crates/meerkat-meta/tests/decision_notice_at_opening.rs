#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

//! The stdio host is told of an open decision without asking for it.
//!
//! A connection that follows its runtime's decision owner
//! (`StdioDecisionConnection::notice_openings`) writes the `held` notice of
//! a decision when the runtime opens it, and once for each decision already
//! open when the connection starts: a host that never lists its decisions
//! still hears of a question set a member asked. The notice names a hold
//! the store already keeps. A decision is pushed at most once per
//! connection; the host's own list is what it was, so it writes its `held`
//! once more for a decision the connection only pushed. Nothing is presented
//! to the stdio operator by a push: that stays with the host's list.
//!
//! The decisions already open are read at the connection's start
//! (`DecisionOpeningsAtStart::read`, which a gateway makes before it reports
//! that it is ready) and noticed from that reading
//! (`StdioDecisionConnection::notice_openings_from`): a connection that
//! serves lists nothing at its start. It reads each decision once more
//! right before its notice and notices only one that is still open; for a
//! decision of the reading that read records nothing. The rows after the
//! launch rows count the calls of a store in memory to pin that.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use meerkat_client::TestClient;
use meerkat_core::approval::ApprovalStoreError;
use meerkat_core::approval::decision::{
    ApproverId, DECISION_PAGE_MAX, DecisionHoldRequest, DecisionId, DecisionKind,
    DecisionListFilter, DecisionOperationId, DecisionPage, DecisionPhase, DecisionPrincipalKey,
    DecisionRenderer, DecisionRendering, DecisionRequestBody, DecisionRow, DecisionStore,
    DecisionSubjectKey, InMemoryDecisionStore, Sha256Digest, StoredDecision,
};
use meerkat_core::ask_user::AskUserQuestionSet;
use meerkat_core::{PrincipalKind, PrincipalRef, RunId, TrustDomainId};
use meerkat_meta::{
    AccessControlConfig, AccessController, DecisionNoticeSink, DecisionOpeningsAtStart,
    StdioDecisionConnection, UnifiedRuntime,
};
use meerkat_mob::MobDefinition;
use meerkat_runtime::ask_user::{ASK_USER_QUESTIONS_KIND, AskUserHold, hold_ask_user};
use meerkat_runtime::decision::{
    DecisionClock, DecisionClockError, DecisionWithdrawer, DurableDecisionService,
};
use serde_json::{Value, json};
use tokio::task::JoinHandle;

const RENDERING: &str = "Run the test kind's operation once.";

/// How long a row waits for the notices it expects.
const WAIT: Duration = Duration::from_secs(30);

/// How long a row waits before it says that no further notice came.
const QUIET: Duration = Duration::from_millis(300);

/// The map that lets the stdio operator decide for `team:owners`.
const OPERATOR_MAP: &str = r#"
enabled = false

[approvers."team:owners"]
subjects = ["owner@example.org"]
ingress_subjects = [{ ingress = "operator" }]
"#;

fn now_ms() -> u64 {
    u64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_millis(),
    )
    .expect("ms")
}

/// A persistent runtime on `state`, with the approver map `map` when given.
/// It keeps durable decisions: without a persistent state directory nothing
/// is held and there is nothing to notice.
async fn runtime(state: &Path, map: Option<&str>) -> UnifiedRuntime {
    let mut builder = UnifiedRuntime::builder()
        .definition(
            MobDefinition::from_toml(
                r#"
[mob]
id = "decision-notice-at-opening"

[profiles.lead]
model = "gpt-5.5"
"#,
            )
            .expect("definition"),
        )
        .persistent_state(state)
        .default_llm_client(Arc::new(TestClient::default()));
    if let Some(map) = map {
        let config: AccessControlConfig = toml::from_str(map).expect("map");
        builder = builder.access_controller(AccessController::new(config).expect("controller"));
    }
    Box::pin(builder.build()).await.expect("runtime")
}

/// The decision owner of the state directory `state`, as another holder on
/// the store opens it.
fn owner_of(state: &Path) -> DurableDecisionService {
    meerkat::ApprovalIssuerSelection::Persistent {
        store_path: state.to_path_buf(),
    }
    .open_decisions()
}

fn agent_requester(name: &str) -> DecisionPrincipalKey {
    DecisionPrincipalKey::of(
        &PrincipalRef::in_domain(
            PrincipalKind::SharedAgent,
            name,
            TrustDomainId::new("example.org").expect("domain"),
        )
        .expect("principal"),
    )
    .expect("key")
}

/// Hold one decision of the test kind for `team:owners`.
fn hold(owner: &DurableDecisionService, subject: &str) -> StoredDecision {
    hold_until(owner, subject, u64::MAX / 2)
}

/// Hold one decision of the test kind for `team:owners` that waits until
/// `deadline_at_ms`.
fn hold_until(
    owner: &DurableDecisionService,
    subject: &str,
    deadline_at_ms: u64,
) -> StoredDecision {
    owner
        .hold(
            DecisionHoldRequest::new(
                DecisionKind::new("release").expect("kind"),
                DecisionSubjectKey::new(subject).expect("subject"),
                DecisionOperationId::new(format!("operation:{subject}")).expect("operation"),
                Sha256Digest::of(b"{}"),
                DecisionRequestBody::from_value(&json!({})).expect("body"),
                agent_requester("builder"),
                ApproverId::new("team:owners").expect("approver"),
                deadline_at_ms,
                DecisionRendering::new(RENDERING).expect("rendering"),
                DecisionRenderer::new("render.v1").expect("renderer"),
            ),
            false,
        )
        .expect("hold")
}

/// How long a question set held by `hold_question_set` waits for its
/// answers, in seconds. The seed step writes the same number into its carrier
/// file as the set's expiry.
const QUESTION_SET_WAITS_SECONDS: u64 = 3600;

/// Hold one question set a member asked, as the stock tool's host does.
fn hold_question_set(owner: &DurableDecisionService, call_id: &str) -> StoredDecision {
    let questions: AskUserQuestionSet = serde_json::from_value(json!({
        "title": "Release plan",
        "questions": [{
            "id": "q1",
            "prompt": "Which day?",
            "choices": [{"id": "a", "label": "Monday"}, {"id": "b", "label": "Friday"}],
        }],
    }))
    .expect("question set");
    hold_ask_user(
        owner,
        now_ms(),
        AskUserHold {
            asker: "agent-a".to_string(),
            requester: agent_requester("agent-a"),
            questions,
            timeout: Duration::from_secs(QUESTION_SET_WAITS_SECONDS),
            run_id: RunId::new(),
            call_id: call_id.to_string(),
        },
    )
    .expect("the set is held")
}

/// What one stdio connection wrote to its host.
struct Notices {
    /// The notices that reached the pipe, in order.
    seen: Mutex<Vec<Value>>,
    /// Whether a line reaches the pipe.
    on_pipe: AtomicBool,
    /// How many lines the connection tried to write.
    attempts: AtomicUsize,
    /// The state directory, for a row that asks what the store keeps when
    /// a notice is written.
    store: Option<PathBuf>,
    /// For each notice, whether another owner on the store read its
    /// decision open when the notice was written.
    kept_open: Mutex<Vec<bool>>,
}

impl Notices {
    fn new(store: Option<&Path>) -> Self {
        Self {
            seen: Mutex::new(Vec::new()),
            on_pipe: AtomicBool::new(true),
            attempts: AtomicUsize::new(0),
            store: store.map(Path::to_path_buf),
            kept_open: Mutex::new(Vec::new()),
        }
    }

    fn all(&self) -> Vec<Value> {
        self.seen
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// The events of the notices written for `decision`, in order.
    fn events_of(&self, decision: &StoredDecision) -> Vec<String> {
        self.all()
            .iter()
            .filter(|notice| notice["decision_id"] == json!(decision.decision_id.to_string()))
            .map(|notice| notice["event"].as_str().expect("event").to_string())
            .collect()
    }

    /// The notices, once `count` of them were written or the wait ran out.
    async fn wait_for(&self, count: usize) -> Vec<Value> {
        let deadline = Instant::now() + WAIT;
        loop {
            let seen = self.all();
            if seen.len() >= count || Instant::now() >= deadline {
                return seen;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// The notices after a quiet moment: what came, and nothing after it.
    async fn settled(&self) -> Vec<Value> {
        tokio::time::sleep(QUIET).await;
        self.all()
    }
}

#[async_trait]
impl DecisionNoticeSink for Notices {
    async fn deliver(&self, line: String) -> bool {
        self.attempts.fetch_add(1, Ordering::SeqCst);
        if !self.on_pipe.load(Ordering::SeqCst) {
            return false;
        }
        let line: Value = serde_json::from_str(&line).expect("a notice line is JSON");
        let notice = line["params"].clone();
        if let Some(store) = &self.store {
            let open = owner_of(store)
                .list(
                    &DecisionListFilter::default().with_phase(DecisionPhase::Open),
                    None,
                    DECISION_PAGE_MAX,
                )
                .expect("another owner lists the open decisions");
            let kept = open
                .decisions
                .iter()
                .any(|row| json!(row.decision_id.to_string()) == notice["decision_id"]);
            self.kept_open
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(kept);
        }
        self.seen
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(notice);
        true
    }
}

/// A stdio connection of `runtime`, as the gateway opens one per process,
/// and what it writes.
fn open_connection(
    runtime: &UnifiedRuntime,
    store: Option<&Path>,
) -> (Arc<StdioDecisionConnection>, Arc<Notices>) {
    let notices = Arc::new(Notices::new(store));
    let sink: Arc<dyn DecisionNoticeSink> = notices.clone();
    (
        Arc::new(StdioDecisionConnection::new(
            runtime.durable_decisions().realm(),
            sink,
        )),
        notices,
    )
}

/// The connection follows the runtime's decision owner, as the gateway
/// starts it once the connection serves.
fn follow(connection: &Arc<StdioDecisionConnection>, runtime: &UnifiedRuntime) -> JoinHandle<()> {
    tokio::spawn(
        Arc::clone(connection).notice_openings(runtime.durable_decisions().owner().clone()),
    )
}

/// One `meta/decisions/*` call on `connection`.
async fn call(
    connection: &StdioDecisionConnection,
    runtime: &UnifiedRuntime,
    method: &str,
    params: Value,
) -> Value {
    let line = json!({"jsonrpc": "2.0", "id": "decisions", "method": method, "params": params})
        .to_string();
    let answer = connection
        .handle_line(runtime, &line)
        .await
        .expect("a decision method answers");
    serde_json::from_str(&answer).expect("answer JSON")
}

/// How often each decision was noticed.
fn times_noticed(notices: &[Value]) -> BTreeMap<String, usize> {
    let mut times = BTreeMap::new();
    for notice in notices {
        let id = notice["decision_id"].as_str().expect("decision id");
        *times.entry(id.to_string()).or_insert(0) += 1;
    }
    times
}

/// A member's question set, held on a runtime whose host never lists its
/// decisions, is noticed to that host once: `held`, of the question set's
/// kind, with nothing of its content.
#[tokio::test]
async fn a_question_set_is_noticed_held_to_a_host_that_never_lists() {
    let dir = tempfile::tempdir().expect("dir");
    let state = dir.path().join("state");
    let runtime = runtime(&state, None).await;
    let (connection, notices) = open_connection(&runtime, None);
    let following = follow(&connection, &runtime);

    let held = hold_question_set(runtime.durable_decisions().owner(), "call-1");

    let seen = notices.wait_for(1).await;
    assert_eq!(seen.len(), 1, "one notice for the set: {seen:?}");
    let notice = &seen[0];
    assert_eq!(notice["decision_id"], json!(held.decision_id.to_string()));
    assert_eq!(notice["event"], json!("held"));
    assert_eq!(notice["kind"], json!(ASK_USER_QUESTIONS_KIND));
    assert_eq!(notice["phase"], json!("open"));
    assert_eq!(
        notice["approver_id"],
        json!(held.descriptor.approver_id.as_str())
    );
    assert_eq!(
        notice["rendering_ref"],
        json!(held.descriptor.subject_digest.to_string())
    );
    assert!(
        notice.get("presentation").is_none_or(Value::is_null),
        "a held notice shows nothing: {notice}"
    );
    let after = notices.settled().await;
    assert_eq!(after.len(), 1, "the set is pushed once: {after:?}");

    following.abort();
    runtime.shutdown().await;
}

/// A notice is written only for a hold the store already keeps: when each
/// notice reaches the pipe, another owner on the same store reads its
/// decision open.
#[tokio::test]
async fn a_pushed_notice_names_a_hold_the_store_already_keeps() {
    let dir = tempfile::tempdir().expect("dir");
    let state = dir.path().join("state");
    let runtime = runtime(&state, None).await;
    let (connection, notices) = open_connection(&runtime, Some(&state));
    let following = follow(&connection, &runtime);

    let owner = runtime.durable_decisions().owner();
    let _set = hold_question_set(owner, "call-1");
    let _first = hold(owner, "operation-1");
    let _second = hold(owner, "operation-2");

    let seen = notices.wait_for(3).await;
    assert_eq!(seen.len(), 3, "one notice per hold: {seen:?}");
    let kept = notices
        .kept_open
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    assert_eq!(
        kept,
        [true, true, true],
        "each notice names a decision the store keeps open"
    );

    following.abort();
    runtime.shutdown().await;
}

/// The host's list is what it was: a decision the connection only pushed is
/// noticed once more at the host's first list of it, and at no later list.
#[tokio::test]
async fn a_pushed_decision_is_noticed_once_more_at_the_hosts_first_list_only() {
    let dir = tempfile::tempdir().expect("dir");
    let state = dir.path().join("state");
    let runtime = runtime(&state, None).await;
    let (connection, notices) = open_connection(&runtime, None);
    let following = follow(&connection, &runtime);

    let held = hold(runtime.durable_decisions().owner(), "operation-1");
    let pushed = notices.wait_for(1).await;
    assert_eq!(pushed.len(), 1, "the decision is pushed: {pushed:?}");
    assert_eq!(notices.events_of(&held), ["held"]);

    let listed = call(&connection, &runtime, "meta/decisions/list", json!({})).await;
    assert!(listed["error"].is_null(), "{listed}");
    assert_eq!(
        notices.events_of(&held),
        ["held", "held"],
        "the host's first list writes its own notice"
    );

    let again = call(&connection, &runtime, "meta/decisions/list", json!({})).await;
    assert!(again["error"].is_null(), "{again}");
    let after = notices.settled().await;
    assert_eq!(after.len(), 2, "no third notice: {after:?}");

    following.abort();
    runtime.shutdown().await;
}

/// After a restart, the new connection is told once of each decision that
/// was open before it, with no list: the notice is owed again on every
/// connection, and nothing about it is stored.
#[tokio::test]
async fn decisions_open_before_a_restart_are_noticed_once_when_the_connection_starts() {
    let dir = tempfile::tempdir().expect("dir");
    let state = dir.path().join("state");
    let before = runtime(&state, None).await;
    let set = hold_question_set(before.durable_decisions().owner(), "call-1");
    let other = hold(before.durable_decisions().owner(), "operation-1");
    before.shutdown().await;
    drop(before);

    let runtime = runtime(&state, None).await;
    let (connection, notices) = open_connection(&runtime, None);
    let following = follow(&connection, &runtime);

    let seen = notices.wait_for(2).await;
    assert_eq!(seen.len(), 2, "one notice per open decision: {seen:?}");
    assert_eq!(notices.events_of(&set), ["held"]);
    assert_eq!(notices.events_of(&other), ["held"]);
    let after = notices.settled().await;
    assert_eq!(after.len(), 2, "each is pushed once: {after:?}");

    following.abort();
    runtime.shutdown().await;
}

/// More decisions than the owner keeps for a subscriber that fell behind
/// (256) are each noticed once: the connection catches up from the open
/// decisions.
#[tokio::test]
async fn a_connection_that_fell_behind_notices_each_open_decision_once() {
    let dir = tempfile::tempdir().expect("dir");
    let state = dir.path().join("state");
    let runtime = runtime(&state, None).await;
    let (connection, notices) = open_connection(&runtime, None);
    let following = follow(&connection, &runtime);

    // The connection is following: its first notice is on the pipe.
    let owner = runtime.durable_decisions().owner();
    let first = hold(owner, "operation-first");
    let pushed = notices.wait_for(1).await;
    assert_eq!(pushed.len(), 1, "the connection follows: {pushed:?}");

    // The row's own thread holds these without yielding, so the connection
    // reads none of them until all are held.
    let mut held = vec![first];
    for index in 0..260 {
        held.push(hold(owner, &format!("operation-{index}")));
    }

    let seen = notices.wait_for(held.len()).await;
    let after = notices.settled().await;
    assert_eq!(seen.len(), held.len(), "every decision is noticed");
    assert_eq!(after.len(), held.len(), "and none twice");
    let times = times_noticed(&after);
    for decision in &held {
        assert_eq!(
            times.get(&decision.decision_id.to_string()),
            Some(&1),
            "{} is noticed once",
            decision.decision_id
        );
    }

    following.abort();
    runtime.shutdown().await;
}

/// A decision that ended before the connection came to its opening is not
/// noticed: the connection reads the decision again and writes only for one
/// that is still open.
#[tokio::test]
async fn a_decision_that_ended_before_its_notice_is_not_noticed() {
    let dir = tempfile::tempdir().expect("dir");
    let state = dir.path().join("state");
    let runtime = runtime(&state, None).await;
    let (connection, notices) = open_connection(&runtime, None);
    let following = follow(&connection, &runtime);

    let owner = runtime.durable_decisions().owner();
    let first = hold(owner, "operation-first");
    let pushed = notices.wait_for(1).await;
    assert_eq!(pushed.len(), 1, "the connection follows: {pushed:?}");

    // Held and withdrawn before the connection reads the opening, then one
    // more that stays open.
    let ended = hold(owner, "operation-ended");
    owner
        .withdraw(&ended.decision_id, &DecisionWithdrawer::Host)
        .expect("withdraw");
    let last = hold(owner, "operation-last");

    let seen = notices.wait_for(2).await;
    assert_eq!(seen.len(), 2, "the two open decisions: {seen:?}");
    let after = notices.settled().await;
    assert_eq!(notices.events_of(&first), ["held"]);
    assert_eq!(notices.events_of(&last), ["held"]);
    assert!(
        notices.events_of(&ended).is_empty(),
        "an ended decision is not noticed: {after:?}"
    );

    following.abort();
    runtime.shutdown().await;
}

/// A push presents nothing to the stdio operator. A decision the operator
/// may decide is pushed `held`; the operator is not eligible on that alone;
/// the host's list then presents it, as it always did.
#[tokio::test]
async fn a_push_presents_nothing_to_the_operator_and_the_list_still_does() {
    let dir = tempfile::tempdir().expect("dir");
    let state = dir.path().join("state");
    let runtime = runtime(&state, Some(OPERATOR_MAP)).await;
    let (connection, notices) = open_connection(&runtime, None);
    let following = follow(&connection, &runtime);

    let held = hold(runtime.durable_decisions().owner(), "operation-7");
    let pushed = notices.wait_for(1).await;
    assert_eq!(pushed.len(), 1, "the decision is pushed: {pushed:?}");
    assert_eq!(notices.events_of(&held), ["held"]);
    assert!(
        pushed[0].get("presentation").is_none_or(Value::is_null),
        "a push shows nothing: {pushed:?}"
    );

    let approve = json!({
        "decision_id": held.decision_id.to_string(),
        "approve": true,
        "presented_digest": held.descriptor.subject_digest.to_string(),
    });
    let early = call(
        &connection,
        &runtime,
        "meta/decisions/decide",
        approve.clone(),
    )
    .await;
    assert_eq!(
        early["error"]["data"]["decision_refusal"]["kind"],
        json!("not_eligible"),
        "a pushed notice is no presentation: {early}"
    );

    let listed = call(&connection, &runtime, "meta/decisions/list", json!({})).await;
    assert!(listed["error"].is_null(), "{listed}");
    assert_eq!(notices.events_of(&held), ["held", "presented"]);
    let decided = call(&connection, &runtime, "meta/decisions/decide", approve).await;
    assert!(
        decided["error"].is_null(),
        "the operator decides what the list presented: {decided}"
    );

    following.abort();
    runtime.shutdown().await;
}

/// A notice that does not reach the pipe ends the following: the connection
/// writes nothing more on its own, and the decision is not counted as told.
#[tokio::test]
async fn a_notice_that_does_not_reach_the_pipe_ends_the_following() {
    let dir = tempfile::tempdir().expect("dir");
    let state = dir.path().join("state");
    let runtime = runtime(&state, None).await;
    let (connection, notices) = open_connection(&runtime, None);
    notices.on_pipe.store(false, Ordering::SeqCst);
    let following = follow(&connection, &runtime);

    let held = hold(runtime.durable_decisions().owner(), "operation-1");

    let deadline = Instant::now() + WAIT;
    while notices.attempts.load(Ordering::SeqCst) == 0 && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(
        notices.attempts.load(Ordering::SeqCst),
        1,
        "the connection tried to write the notice once"
    );
    let deadline = Instant::now() + WAIT;
    while !following.is_finished() && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(following.is_finished(), "the following ended");
    let _second = hold(runtime.durable_decisions().owner(), "operation-2");
    tokio::time::sleep(QUIET).await;
    assert_eq!(
        notices.attempts.load(Ordering::SeqCst),
        1,
        "nothing more is written on the connection's own"
    );

    // The pipe is back for the host's own list: the decision was not
    // counted as told, so the list writes its notice.
    notices.on_pipe.store(true, Ordering::SeqCst);
    let listed = call(&connection, &runtime, "meta/decisions/list", json!({})).await;
    assert!(listed["error"].is_null(), "{listed}");
    assert_eq!(notices.events_of(&held), ["held"]);

    runtime.shutdown().await;
}

/// Whether `following` ended within the wait.
async fn ended(following: &JoinHandle<()>) -> bool {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !following.is_finished() && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    following.is_finished()
}

/// A candidate launch acts nowhere: its connection follows nothing, even
/// with a decision open in the store it reads. The host's own list is what
/// tells it there.
#[tokio::test]
async fn a_candidate_launch_follows_nothing() {
    let dir = tempfile::tempdir().expect("dir");
    let state = dir.path().join("state");
    let runtime = runtime(&state, None).await;
    let owner = runtime.durable_decisions().owner();
    let held = hold(owner, "operation-1");
    let (connection, notices) = open_connection(&runtime, None);

    let following =
        tokio::spawn(Arc::clone(&connection).notice_openings(owner.for_candidate_launch()));

    assert!(ended(&following).await, "the following ends at once");
    let after = notices.settled().await;
    assert!(
        notices.events_of(&held).is_empty(),
        "a candidate launch pushes nothing: {after:?}"
    );

    following.abort();
    runtime.shutdown().await;
}

/// A launch that keeps no durable decisions has none to tell of: its
/// connection follows nothing and ends at once.
#[tokio::test]
async fn a_launch_without_durable_decisions_follows_nothing() {
    let runtime = Box::pin(
        UnifiedRuntime::builder()
            .definition(
                MobDefinition::from_toml(
                    r#"
[mob]
id = "decision-notice-no-store"

[profiles.lead]
model = "gpt-5.5"
"#,
                )
                .expect("definition"),
            )
            .default_llm_client(Arc::new(TestClient::default()))
            .build(),
    )
    .await
    .expect("runtime");
    assert!(
        !runtime.durable_decisions().is_available(),
        "a launch without a persistent state directory keeps no durable decisions"
    );
    let (connection, notices) = open_connection(&runtime, None);

    let following = follow(&connection, &runtime);

    assert!(ended(&following).await, "the following ends at once");
    let after = notices.settled().await;
    assert!(after.is_empty(), "nothing is pushed: {after:?}");

    following.abort();
    runtime.shutdown().await;
}

/// A decision store that counts the calls made of it, over rows kept in
/// memory. A row can script it to refuse its next lists as a store does
/// that another owner holds.
#[derive(Default)]
struct CountingStore {
    rows: InMemoryDecisionStore,
    calls: AtomicUsize,
    /// How many of the calls wrote, or tried to write, a row.
    writes: AtomicUsize,
    /// How many of the calls were lists.
    lists: AtomicUsize,
    /// How many of the next reads of one decision answer that the store
    /// is held.
    gets_to_refuse: AtomicUsize,
    /// How many of the next lists answer that the store is held.
    lists_to_refuse: AtomicUsize,
}

impl CountingStore {
    /// How many calls the store has answered.
    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    /// How many of them were writes.
    fn writes(&self) -> usize {
        self.writes.load(Ordering::SeqCst)
    }

    /// How many of them were lists.
    fn lists(&self) -> usize {
        self.lists.load(Ordering::SeqCst)
    }

    fn called(&self) {
        self.calls.fetch_add(1, Ordering::SeqCst);
    }

    fn wrote(&self) {
        self.called();
        self.writes.fetch_add(1, Ordering::SeqCst);
    }
}

impl DecisionStore for CountingStore {
    fn get(&self, decision_id: &DecisionId) -> Result<Option<DecisionRow>, ApprovalStoreError> {
        self.called();
        let refuse = self
            .gets_to_refuse
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                left.checked_sub(1)
            })
            .is_ok();
        if refuse {
            return Err(ApprovalStoreError::Contended);
        }
        self.rows.get(decision_id)
    }

    fn insert_if_absent(&self, row: &StoredDecision) -> Result<(), ApprovalStoreError> {
        self.wrote();
        self.rows.insert_if_absent(row)
    }

    fn replace_if_revision(
        &self,
        expected_revision: u64,
        row: &StoredDecision,
    ) -> Result<(), ApprovalStoreError> {
        self.wrote();
        self.rows.replace_if_revision(expected_revision, row)
    }

    fn list(
        &self,
        filter: &DecisionListFilter,
        after: Option<&str>,
        limit: usize,
    ) -> Result<DecisionPage, ApprovalStoreError> {
        self.called();
        self.lists.fetch_add(1, Ordering::SeqCst);
        let refuse = self
            .lists_to_refuse
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                left.checked_sub(1)
            })
            .is_ok();
        if refuse {
            return Err(ApprovalStoreError::Contended);
        }
        self.rows.list(filter, after, limit)
    }

    fn owed(&self, after: Option<&str>, limit: usize) -> Result<DecisionPage, ApprovalStoreError> {
        self.called();
        self.rows.owed(after, limit)
    }

    fn due(
        &self,
        now_ms: u64,
        after: Option<&str>,
        limit: usize,
    ) -> Result<DecisionPage, ApprovalStoreError> {
        self.called();
        self.rows.due(now_ms, after, limit)
    }

    fn count(&self, filter: &DecisionListFilter) -> Result<u64, ApprovalStoreError> {
        self.called();
        self.rows.count(filter)
    }

    fn is_persistent(&self) -> bool {
        self.rows.is_persistent()
    }
}

/// A decision owner over a counting store, a connection that writes to
/// `Notices`, and both.
fn counted() -> (
    Arc<CountingStore>,
    DurableDecisionService,
    Arc<StdioDecisionConnection>,
    Arc<Notices>,
) {
    let store = Arc::new(CountingStore::default());
    let backing: Arc<dyn DecisionStore> = store.clone();
    let notices = Arc::new(Notices::new(None));
    let sink: Arc<dyn DecisionNoticeSink> = notices.clone();
    (
        store,
        DurableDecisionService::new(backing),
        Arc::new(StdioDecisionConnection::new(None, sink)),
        notices,
    )
}

/// Let the other tasks of the row run until each of them waits. A row's
/// runtime has one thread, so a task runs when the row yields and goes on
/// until it waits for something; nothing here waits for time.
async fn let_the_tasks_run() {
    for _ in 0..16 {
        tokio::task::yield_now().await;
    }
}

/// A connection that serves lists nothing at its start. The decisions
/// already open are listed in the reading (`DecisionOpeningsAtStart::read`:
/// a gateway makes it before it reports that it is ready); once it serves,
/// the connection reads each of them once more right before its notice, in
/// a read that records nothing, and makes no other call of the store until
/// its runtime opens a decision. Another owner of the same store that
/// writes its first decision as soon as the gateway is ready so meets no
/// list of the gateway's.
///
/// The store here counts its calls. After the reading: exactly one read for
/// the one decision of the reading, no list and no write; the runtime's own
/// opening is then read again, as before.
#[tokio::test]
async fn a_serving_connection_reads_each_decision_of_the_reading_once_and_lists_nothing() {
    let (store, owner, connection, notices) = counted();
    let before = hold(&owner, "operation-before-the-start");

    // The reading at the start lists the open decisions.
    let before_the_reading = store.calls();
    let start = DecisionOpeningsAtStart::read(owner.clone())
        .await
        .expect("an owner that keeps decisions is read");
    assert!(
        store.calls() > before_the_reading,
        "the reading at the start calls the store"
    );
    let after_the_reading = store.calls();
    let lists_of_the_reading = store.lists();
    let writes_before_the_start = store.writes();

    // The connection serves: the decision of the reading is read once
    // more and noticed. Nothing is listed and nothing is written.
    let following = tokio::spawn(Arc::clone(&connection).notice_openings_from(start));
    let_the_tasks_run().await;
    assert_eq!(notices.events_of(&before), ["held"]);
    assert_eq!(notices.all().len(), 1);
    assert_eq!(
        store.calls(),
        after_the_reading + 1,
        "one read for the one decision of the reading, and no other call"
    );
    assert_eq!(
        store.lists(),
        lists_of_the_reading,
        "a serving connection listed the store"
    );
    assert_eq!(
        store.writes(),
        writes_before_the_start,
        "the read before a notice of the reading wrote a row"
    );

    // The runtime opens a decision: the connection reads it again and
    // notices it; the earlier one is not noticed again.
    let later = hold(&owner, "operation-after-the-start");
    let after_the_hold = store.calls();
    let_the_tasks_run().await;
    assert_eq!(notices.events_of(&later), ["held"]);
    assert_eq!(notices.events_of(&before), ["held"]);
    assert!(
        store.calls() > after_the_hold,
        "a decision the runtime opens is read again before its notice"
    );

    following.abort();
}

/// Over a store that holds no decision the reading at the start is the
/// only call: the connection that serves writes nothing and calls the store
/// for nothing. This is the launch of another owner's first write: the
/// gateway's store is still empty when it is ready.
#[tokio::test]
async fn a_serving_connection_over_an_empty_store_calls_it_for_nothing() {
    let (store, owner, connection, notices) = counted();

    let start = DecisionOpeningsAtStart::read(owner)
        .await
        .expect("an owner that keeps decisions is read");
    let after_the_reading = store.calls();
    assert!(
        after_the_reading > 0,
        "the reading at the start calls the store"
    );

    let following = tokio::spawn(Arc::clone(&connection).notice_openings_from(start));
    let_the_tasks_run().await;
    assert!(
        notices.all().is_empty(),
        "nothing is open: nothing is noticed"
    );
    assert_eq!(
        store.calls(),
        after_the_reading,
        "a serving connection called a store that holds no decision"
    );
    assert!(!following.is_finished(), "the connection goes on following");

    following.abort();
}

/// Move a row's paused clock on, past the waits between a refused read's
/// tries, letting the tasks run after each step. No time passes.
async fn let_the_paused_clock_run() {
    for _ in 0..8 {
        tokio::time::advance(Duration::from_millis(200)).await;
        let_the_tasks_run().await;
    }
}

/// A decision the runtime opens AFTER the reading at the start and BEFORE
/// the connection serves is noticed, once: the reading holds the
/// subscription it took before it listed, so the opening waits there until
/// the task reads it. The decision of the reading is noticed too, once.
#[tokio::test]
async fn a_decision_the_runtime_opens_between_the_reading_and_the_connections_start_is_noticed_once()
 {
    let (_store, owner, connection, notices) = counted();
    let before = hold(&owner, "operation-before-the-reading");
    let start = DecisionOpeningsAtStart::read(owner.clone())
        .await
        .expect("an owner that keeps decisions is read");

    // The gateway is between its reading and its Ready here.
    let between = hold(&owner, "operation-between");

    let following = tokio::spawn(Arc::clone(&connection).notice_openings_from(start));
    let_the_tasks_run().await;
    assert_eq!(notices.events_of(&before), ["held"]);
    assert_eq!(notices.events_of(&between), ["held"]);
    assert_eq!(notices.all().len(), 2, "each once: {:?}", notices.all());

    following.abort();
}

/// A subscription that fell behind between the reading and the
/// connection's start loses nothing: the task reads that it missed
/// openings, lists the open decisions again and notices each of them once,
/// the decision of the reading included. That list is a call of the store
/// after the connection serves.
#[tokio::test]
async fn a_subscription_that_fell_behind_before_the_connections_start_notices_each_decision_once() {
    let (store, owner, connection, notices) = counted();
    let mut held = vec![hold(&owner, "operation-before-the-reading")];
    let start = DecisionOpeningsAtStart::read(owner.clone())
        .await
        .expect("an owner that keeps decisions is read");

    // More openings than the subscription keeps, all before the task reads
    // one of them.
    for index in 0..260 {
        held.push(hold(&owner, &format!("operation-{index}")));
    }
    let before_the_task = store.calls();

    let following = tokio::spawn(Arc::clone(&connection).notice_openings_from(start));
    let_the_tasks_run().await;
    let seen = notices.all();
    assert_eq!(
        seen.len(),
        held.len(),
        "every decision is noticed, and none twice"
    );
    let times = times_noticed(&seen);
    for decision in &held {
        assert_eq!(
            times.get(&decision.decision_id.to_string()),
            Some(&1),
            "{} is noticed once",
            decision.decision_id
        );
    }
    assert!(
        store.calls() > before_the_task,
        "a subscription that fell behind reads the store again"
    );

    following.abort();
}

/// A reading at the start that the store refuses is made again a few times
/// and then succeeds: the decision that was open is noticed. The waits
/// between the tries are the row's paused clock's, so no time passes.
#[tokio::test(start_paused = true)]
async fn a_reading_at_the_start_that_the_store_first_refuses_is_made_again() {
    let (store, owner, connection, notices) = counted();
    let before = hold(&owner, "operation-before-the-reading");
    store.lists_to_refuse.store(2, Ordering::SeqCst);

    let start = DecisionOpeningsAtStart::read(owner.clone())
        .await
        .expect("an owner that keeps decisions is read");
    assert_eq!(store.lists_to_refuse.load(Ordering::SeqCst), 0);

    let following = tokio::spawn(Arc::clone(&connection).notice_openings_from(start));
    let_the_tasks_run().await;
    assert_eq!(notices.events_of(&before), ["held"]);

    following.abort();
}

/// A reading at the start that the store refuses every time ends with
/// nothing read, and the connection starts all the same: nothing is
/// noticed at the opening (the decision that was open then waits for the
/// host's own list), and a decision the runtime opens later is noticed.
#[tokio::test(start_paused = true)]
async fn a_reading_at_the_start_that_the_store_refuses_notices_nothing_and_the_connection_follows()
{
    let (store, owner, connection, notices) = counted();
    let before = hold(&owner, "operation-before-the-reading");
    store.lists_to_refuse.store(usize::MAX, Ordering::SeqCst);

    let start = DecisionOpeningsAtStart::read(owner.clone())
        .await
        .expect("a reading the store refuses is still a reading");
    store.lists_to_refuse.store(0, Ordering::SeqCst);
    let after_the_reading = store.calls();

    let following = tokio::spawn(Arc::clone(&connection).notice_openings_from(start));
    let_the_tasks_run().await;
    assert!(
        notices.all().is_empty(),
        "nothing was read at the start, so nothing is noticed at the opening: {:?}",
        notices.all()
    );
    assert_eq!(
        store.calls(),
        after_the_reading,
        "the connection does not read again what its start could not read"
    );

    let later = hold(&owner, "operation-after-the-start");
    let_the_tasks_run().await;
    assert_eq!(notices.events_of(&later), ["held"]);
    assert!(notices.events_of(&before).is_empty());

    following.abort();
}

/// A decision of the reading that the host withdrew before its notice is
/// not noticed: the connection reads it once more right before the notice
/// and finds it no longer open. The decision that stayed open is noticed.
#[tokio::test]
async fn a_decision_of_the_reading_withdrawn_before_its_notice_is_not_noticed() {
    let (_store, owner, connection, notices) = counted();
    let ended = hold(&owner, "operation-withdrawn-before-its-notice");
    let open = hold(&owner, "operation-still-open");
    let start = DecisionOpeningsAtStart::read(owner.clone())
        .await
        .expect("an owner that keeps decisions is read");

    // Between the reading and the notice: the host withdraws one.
    owner
        .withdraw(&ended.decision_id, &DecisionWithdrawer::Host)
        .expect("withdraw");

    let following = tokio::spawn(Arc::clone(&connection).notice_openings_from(start));
    let_the_tasks_run().await;
    assert_eq!(notices.events_of(&open), ["held"]);
    assert!(
        notices.events_of(&ended).is_empty(),
        "a decision that ended before its notice is not noticed: {:?}",
        notices.all()
    );

    following.abort();
}

/// A clock a row sets.
struct RowClock(AtomicU64);

impl DecisionClock for RowClock {
    fn now_ms(&self) -> Result<u64, DecisionClockError> {
        Ok(self.0.load(Ordering::SeqCst))
    }
}

/// A decision whose deadline passed before the reading at the start is not
/// noticed at the opening, and the reading records nothing for it: the
/// list reads it as expired at its clock and writes no row.
#[tokio::test]
async fn a_decision_past_its_deadline_at_the_reading_is_not_noticed() {
    let store = Arc::new(CountingStore::default());
    let backing: Arc<dyn DecisionStore> = store.clone();
    let clock = Arc::new(RowClock(AtomicU64::new(1_000_000)));
    let owner = DurableDecisionService::with_clock(backing, clock.clone());
    let notices = Arc::new(Notices::new(None));
    let sink: Arc<dyn DecisionNoticeSink> = notices.clone();
    let connection = Arc::new(StdioDecisionConnection::new(None, sink));

    let late = hold_until(&owner, "operation-past-its-deadline", 1_060_000);
    let open = hold(&owner, "operation-still-open");
    clock.0.store(2_000_000, Ordering::SeqCst);
    let writes_before_the_reading = store.writes();

    let start = DecisionOpeningsAtStart::read(owner.clone())
        .await
        .expect("an owner that keeps decisions is read");
    assert_eq!(
        store.writes(),
        writes_before_the_reading,
        "the reading writes nothing, the expiry it observes included"
    );

    let following = tokio::spawn(Arc::clone(&connection).notice_openings_from(start));
    let_the_tasks_run().await;
    assert_eq!(notices.events_of(&open), ["held"]);
    assert!(
        notices.events_of(&late).is_empty(),
        "a decision past its deadline is not noticed: {:?}",
        notices.all()
    );

    following.abort();
}

/// A decision of the reading whose deadline passed before its notice is not
/// noticed, and the read that finds it expired records nothing: the store's
/// write count does not move. Its expiry is left to whoever records it.
#[tokio::test]
async fn a_decision_of_the_reading_past_its_deadline_before_its_notice_is_not_noticed_or_recorded()
{
    let store = Arc::new(CountingStore::default());
    let backing: Arc<dyn DecisionStore> = store.clone();
    let clock = Arc::new(RowClock(AtomicU64::new(1_000_000)));
    let owner = DurableDecisionService::with_clock(backing, clock.clone());
    let notices = Arc::new(Notices::new(None));
    let sink: Arc<dyn DecisionNoticeSink> = notices.clone();
    let connection = Arc::new(StdioDecisionConnection::new(None, sink));

    let late = hold_until(&owner, "operation-past-its-deadline", 1_060_000);
    let open = hold(&owner, "operation-still-open");
    let start = DecisionOpeningsAtStart::read(owner.clone())
        .await
        .expect("an owner that keeps decisions is read");

    // Between the reading and the notice: the deadline of one passes.
    clock.0.store(2_000_000, Ordering::SeqCst);
    let calls_before_the_start = store.calls();
    let writes_before_the_start = store.writes();

    let following = tokio::spawn(Arc::clone(&connection).notice_openings_from(start));
    let_the_tasks_run().await;
    assert_eq!(notices.events_of(&open), ["held"]);
    assert!(
        notices.events_of(&late).is_empty(),
        "a decision past its deadline is not noticed: {:?}",
        notices.all()
    );
    assert_eq!(
        store.calls(),
        calls_before_the_start + 2,
        "one read for each of the two decisions of the reading"
    );
    assert_eq!(
        store.writes(),
        writes_before_the_start,
        "the read that found the expiry recorded it"
    );

    following.abort();
}

/// A rescan after a lag records nothing either. The store holds a decision
/// past its deadline that nobody recorded (the reading left it out); the
/// subscription falls behind, the task lists the open decisions of the
/// store again, which names that decision by its stored phase, and reads
/// each through the narrowed owner: the expired one is not noticed and the
/// store's write count does not move, while every open decision is noticed
/// once.
#[tokio::test]
async fn a_rescan_after_a_lag_does_not_notice_or_record_a_decision_past_its_deadline() {
    let store = Arc::new(CountingStore::default());
    let backing: Arc<dyn DecisionStore> = store.clone();
    let clock = Arc::new(RowClock(AtomicU64::new(1_000_000)));
    let owner = DurableDecisionService::with_clock(backing, clock.clone());
    let notices = Arc::new(Notices::new(None));
    let sink: Arc<dyn DecisionNoticeSink> = notices.clone();
    let connection = Arc::new(StdioDecisionConnection::new(None, sink));

    let late = hold_until(&owner, "operation-past-its-deadline", 1_060_000);
    clock.0.store(2_000_000, Ordering::SeqCst);
    let start = DecisionOpeningsAtStart::read(owner.clone())
        .await
        .expect("an owner that keeps decisions is read");

    // More openings than the subscription keeps, before the task reads
    // one of them: the task will list the store again.
    let mut held = Vec::new();
    for index in 0..260 {
        held.push(hold(&owner, &format!("operation-{index}")));
    }
    let lists_before_the_task = store.lists();
    let writes_before_the_task = store.writes();

    let following = tokio::spawn(Arc::clone(&connection).notice_openings_from(start));
    let_the_tasks_run().await;
    assert!(
        store.lists() > lists_before_the_task,
        "the subscription fell behind and the store was listed again"
    );
    let seen = notices.all();
    assert_eq!(
        seen.len(),
        held.len(),
        "every open decision is noticed, and none twice"
    );
    assert!(
        notices.events_of(&late).is_empty(),
        "a decision past its deadline is not noticed by a rescan"
    );
    assert_eq!(
        store.writes(),
        writes_before_the_task,
        "a rescan recorded the expiry of a decision the task did not see opened"
    );

    following.abort();
}

/// A read before a notice that the store refuses is made again a few
/// times: a decision of the reading whose read is refused twice is noticed
/// once the store answers. The waits between the tries are on the row's
/// paused clock, which the row moves on; no time passes.
#[tokio::test(start_paused = true)]
async fn a_read_before_a_notice_that_the_store_first_refuses_is_made_again() {
    let (store, owner, connection, notices) = counted();
    let before = hold(&owner, "operation-before-the-reading");
    let start = DecisionOpeningsAtStart::read(owner.clone())
        .await
        .expect("an owner that keeps decisions is read");
    store.gets_to_refuse.store(2, Ordering::SeqCst);

    let following = tokio::spawn(Arc::clone(&connection).notice_openings_from(start));
    let_the_tasks_run().await;
    let_the_paused_clock_run().await;
    assert_eq!(store.gets_to_refuse.load(Ordering::SeqCst), 0);
    assert_eq!(notices.events_of(&before), ["held"]);

    following.abort();
}

/// A decision of the reading whose read before its notice the store
/// refuses every time is not noticed: the task gives it up after its
/// tries, goes on to the next decision and keeps following. Nothing brings
/// it back but the host's own list.
#[tokio::test(start_paused = true)]
async fn a_decision_whose_read_before_its_notice_the_store_refuses_is_not_noticed() {
    let (store, owner, connection, notices) = counted();
    let refused = hold(&owner, "operation-before-the-reading");
    let start = DecisionOpeningsAtStart::read(owner.clone())
        .await
        .expect("an owner that keeps decisions is read");
    // One read and its five further tries.
    store.gets_to_refuse.store(6, Ordering::SeqCst);

    let following = tokio::spawn(Arc::clone(&connection).notice_openings_from(start));
    let_the_tasks_run().await;
    let_the_paused_clock_run().await;
    assert_eq!(
        store.gets_to_refuse.load(Ordering::SeqCst),
        0,
        "the read was made six times"
    );
    assert!(
        notices.all().is_empty(),
        "a decision that could not be read is not noticed: {:?}",
        notices.all()
    );

    // The connection still follows its runtime.
    let later = hold(&owner, "operation-after-the-start");
    let_the_tasks_run().await;
    assert_eq!(notices.events_of(&later), ["held"]);
    assert!(notices.events_of(&refused).is_empty());

    following.abort();
}

/// What the seed step appends to the name of the directory it seeded to name
/// its carrier file.
const SEED_CARRIER_SUFFIX: &str = ".sdk-held-question-seed.json";

/// The carrier file of a seeded directory: the file beside the directory (in
/// the directory's parent, never inside the directory) whose name is the
/// directory's name followed by `SEED_CARRIER_SUFFIX`. The seed step leaves
/// there, for the SDK row to read, the decision id of the held question set,
/// when it was held and after how many seconds it expires. The gateway is
/// given the directory only, so it never meets the file.
fn seed_carrier_of(state: &Path) -> PathBuf {
    let mut name = state
        .file_name()
        .expect("the directory to seed has a name")
        .to_os_string();
    name.push(SEED_CARRIER_SUFFIX);
    state.with_file_name(name)
}

/// The directory the seed step may write: the one
/// `META_SDK_HELD_QUESTION_STATE` names, when it is fresh and the fixture's
/// own. It panics, before anything is written, unless the value is an
/// absolute path that ends in a name, neither the path nor its existing
/// parent holds a symbolic link or a relative part, the directory is absent
/// or empty (so it holds no store of any kind), nothing exists yet at its
/// carrier file's path (`seed_carrier_of`), and the directory lies nowhere in
/// the home directory's tree (the home directory itself and every path under
/// it, compared in resolved form) and in no state or configuration home of
/// the user (`$XDG_STATE_HOME`, `$XDG_CONFIG_HOME`).
fn fresh_seed_directory() -> PathBuf {
    let named = std::env::var_os("META_SDK_HELD_QUESTION_STATE").unwrap_or_default();
    assert!(
        !named.is_empty(),
        "META_SDK_HELD_QUESTION_STATE is unset or empty: it names the fresh directory to seed"
    );
    let state = PathBuf::from(named);
    assert!(
        state.is_absolute(),
        "the directory to seed must be named by an absolute path, and this one is not: {}",
        state.display()
    );
    let is_link =
        std::fs::symlink_metadata(&state).is_ok_and(|found| found.file_type().is_symlink());
    assert!(
        !is_link,
        "the directory to seed must not be a symbolic link, and this one is: {}",
        state.display()
    );
    let parent = state.parent().expect("the directory to seed has a parent");
    let canonical_parent = parent
        .canonicalize()
        .expect("the parent of the directory to seed exists and can be resolved");
    assert!(
        canonical_parent.as_path() == parent,
        "the path of the directory to seed must hold no symbolic link and no relative part: {}",
        state.display()
    );
    if state.exists() {
        assert!(
            state.is_dir(),
            "the path to seed exists and is not a directory, so it is not seeded: {}",
            state.display()
        );
        let holds_something = std::fs::read_dir(&state)
            .expect("the directory to seed can be read")
            .next()
            .is_some();
        assert!(
            !holds_something,
            "the directory to seed must be absent or empty, and this one holds something: {}",
            state.display()
        );
    }
    let carrier = seed_carrier_of(&state);
    assert!(
        std::fs::symlink_metadata(&carrier).is_err(),
        "the carrier file of the directory to seed must not exist yet, and something is there: {}",
        carrier.display()
    );
    // The resolved form of the directory: its resolved parent and its name
    // (the directory is not a symbolic link, and may not exist yet).
    let own_name = state.file_name().expect("the directory to seed has a name");
    let resolved_state = canonical_parent.join(own_name);
    for name in ["HOME", "XDG_STATE_HOME", "XDG_CONFIG_HOME"] {
        let Some(value) = std::env::var_os(name).filter(|value| !value.is_empty()) else {
            continue;
        };
        let location = PathBuf::from(value);
        let resolved_location = location.canonicalize().unwrap_or(location);
        assert!(
            !resolved_state.starts_with(&resolved_location),
            "the directory to seed must lie nowhere in the tree of {name} ({}), and this one does: {}",
            resolved_location.display(),
            state.display()
        );
    }
    state
}

/// The seed step of the SDK row that needs a question set pending before a
/// gateway process starts
/// (`meta/sdk/python/tests/test_held_question_at_opening_real_gateway.py`):
/// no SDK call opens a question set.
///
/// The order is: this step, by name, on a fresh directory; then a gateway
/// built from the same revision; then the SDK row on that directory, before
/// the set expires (`QUESTION_SET_WAITS_SECONDS` after this step).
///
/// It holds one question set, as the stock tool's host does, in the directory
/// `META_SDK_HELD_QUESTION_STATE` names (see `fresh_seed_directory` for what
/// it refuses), through another owner on that store, and leaves it open.
/// Only after the hold succeeded it writes the carrier file beside that
/// directory, not in it (`seed_carrier_of`): the set's decision id, its
/// kind, when it was seeded and after how many seconds it expires. The
/// directory then holds only what the store wrote. It starts no process.
///
/// It is not a row of this file and runs by name only. It shows nothing of a
/// notice, of a restart, of how often a notice is delivered or of what a
/// person is shown.
#[test]
#[ignore = "the seed step of an SDK row: run by name with META_SDK_HELD_QUESTION_STATE set"]
fn seed_a_pending_question_set_for_an_sdk_row() {
    let state = fresh_seed_directory();
    std::fs::create_dir_all(&state).expect("the state directory");
    let seeded_at_ms = now_ms();
    let owner = owner_of(&state);
    let held = hold_question_set(&owner, "sdk-row-call-1");
    assert_eq!(held.descriptor.kind.as_str(), ASK_USER_QUESTIONS_KIND);
    let open = owner
        .list(
            &DecisionListFilter::default().with_phase(DecisionPhase::Open),
            None,
            DECISION_PAGE_MAX,
        )
        .expect("the owner lists the open decisions");
    let kept = open
        .decisions
        .iter()
        .any(|row| row.decision_id.to_string() == held.decision_id.to_string());
    assert!(kept, "the question set is open in the store");
    let carrier = json!({
        "decision_id": held.decision_id.to_string(),
        "kind": ASK_USER_QUESTIONS_KIND,
        "seeded_at_ms": seeded_at_ms,
        "expires_after_seconds": QUESTION_SET_WAITS_SECONDS,
        "seed": "seed_a_pending_question_set_for_an_sdk_row",
    });
    let written = std::fs::write(seed_carrier_of(&state), carrier.to_string());
    written.expect("the seed step writes its carrier file");
}
