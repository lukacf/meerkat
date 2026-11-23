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

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use meerkat_client::TestClient;
use meerkat_core::approval::decision::{
    ApproverId, DECISION_PAGE_MAX, DecisionHoldRequest, DecisionKind, DecisionListFilter,
    DecisionOperationId, DecisionPhase, DecisionPrincipalKey, DecisionRenderer, DecisionRendering,
    DecisionRequestBody, DecisionSubjectKey, Sha256Digest, StoredDecision,
};
use meerkat_core::ask_user::AskUserQuestionSet;
use meerkat_core::{PrincipalKind, PrincipalRef, RunId, TrustDomainId};
use meerkat_meta::{
    AccessControlConfig, AccessController, DecisionNoticeSink, StdioDecisionConnection,
    UnifiedRuntime,
};
use meerkat_mob::MobDefinition;
use meerkat_runtime::ask_user::{ASK_USER_QUESTIONS_KIND, AskUserHold, hold_ask_user};
use meerkat_runtime::decision::{DecisionWithdrawer, DurableDecisionService};
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
                u64::MAX / 2,
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
