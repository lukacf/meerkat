//! Question sets on the Console and on the stdio gating methods
//! (`crate::ask_user`): a member's `ask_user` call holds a durable decision
//! of kind `ask_user.questions`, `meta/gating/questions` lists the open
//! sets, and `meta/gating/decide` hands a person's answer to the decision
//! owner for that one kind, under the checks of an approval's decide.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::atomic::{AtomicU64, Ordering};

use meerkat_core::approval::ApprovalAssurance;
use meerkat_core::approval::decision::{
    ApproverId, DECIDED_BY_PEOPLE_ONLY, DecisionAnswer, DecisionEvidence, DecisionHoldRequest,
    DecisionId, DecisionKind, DecisionOperationId, DecisionPhase, DecisionPrincipalKey,
    DecisionRenderer, DecisionRendering, DecisionRequestBody, DecisionSubjectKey, Sha256Digest,
    StoredDecision,
};
use meerkat_core::ask_user::{
    ASK_USER_TOOL, AskUserAnswer, AskUserHost, AskUserOutcome, AskUserQuestionSet, AskUserRequest,
    AskUserResult,
};
use meerkat_core::{PrincipalKind, PrincipalRef, RunId, TrustDomainId};
use meerkat_runtime::ask_user::{ASK_USER_QUESTIONS_KIND, AskUserNotice, AskUserNotices};
use meerkat_runtime::decision::{DecisionClock, DecisionClockError, DurableDecisionService};

use super::console_human_admission_tests::{BOB, state};
use super::console_token_lapse_tests::{
    console_rpc_as, durable_fences, human_claims, now_seconds, token,
};
use super::*;
use crate::ask_user::{
    AskUserDriver, AskUserSlot, AskUserSurface, MemberAskUserHost, ask_user_notice_key,
    ask_user_result_input, spawn_ask_user_driver,
};
use crate::console_schedules::{
    ConsoleScheduleDoor, ConsoleScheduleDoorSlot, ConsoleScheduledSend,
};
use crate::durable_decisions::RuntimeDecisions;

const QUESTIONS: &str = "meta/gating/questions";
const DECIDE: &str = "meta/gating/decide";
const CAROL: &str = "carol@example.test";
const DAVE: &str = "dave@example.test";
const ISSUER: &str = "https://trusted.meta.localhost";
/// The asking member of most rows, and another member.
const ASKER: &str = "agent-a";
const OTHER: &str = "agent-b";
const DECISIONS_FILE: &str = "approval_decisions.sqlite3";

/// A Console send path that sends nothing: installed, it says the runtime
/// serves a Console.
struct NoSends;

#[async_trait::async_trait]
impl ConsoleScheduleDoor for NoSends {
    async fn send(&self, _send: &ConsoleScheduledSend, _delivery_key: &str) -> Result<(), String> {
        Ok(())
    }
}

/// A clock a row moves.
struct RowClock(AtomicU64);

impl DecisionClock for RowClock {
    fn now_ms(&self) -> Result<u64, DecisionClockError> {
        Ok(self.0.load(Ordering::SeqCst))
    }
}

/// The notices the executor handed over, each settled as admitted.
#[derive(Default)]
struct Told {
    notices: std::sync::Mutex<Vec<AskUserNotice>>,
}

impl Told {
    fn all(&self) -> Vec<AskUserNotice> {
        self.notices.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl AskUserNotices for Told {
    async fn deliver(
        &self,
        notice: &AskUserNotice,
    ) -> Option<meerkat_core::approval::ApprovalConsentDeliveryState> {
        self.notices.lock().unwrap().push(notice.clone());
        Some(
            meerkat_core::approval::ApprovalConsentDeliveryState::Admitted {
                session_id: meerkat_core::SessionId::new(),
                input_id: "input-1".to_string(),
            },
        )
    }
}

/// Wait until `done` says so, at most 30 s.
async fn until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while !done() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting until {what}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn decisions_in(state_dir: &std::path::Path, name: &str, speaks: bool) -> RuntimeDecisions {
    RuntimeDecisions::in_state_dir(
        state_dir,
        meerkat_core::RealmId::parse("team").expect("realm"),
        crate::durable_decisions::DecisionIngressDeclarations::default(),
        // The runtime's metadata store, in its state directory: the
        // decide events of the Console are bound there.
        Arc::new(
            crate::runtime::SqliteMetadataStore::open(state_dir.join("metadata.sqlite3"))
                .expect("metadata store"),
        ),
        name,
        speaks,
    )
}

/// What a row's Console is built from.
struct Setup {
    decisions: RuntimeDecisionState,
    fences: crate::auth::session_fence::ConsoleSessionFences,
    access: Option<AccessController>,
}

impl Setup {
    /// An enforced Console (verified tokens only) admitting BOB, CAROL and
    /// DAVE, with an access controller that enforces nothing and knows the
    /// rows' members (a Console that tells its people apart shows a member
    /// it does not know to none of them).
    fn enforced() -> Self {
        Self::bare().access(crate::access::AccessControlConfig::default())
    }

    /// [`Self::enforced`] with no access controller at all.
    fn bare() -> Self {
        let mut decisions = state().decisions;
        decisions
            .auth
            .email_allowlist
            .extend([CAROL.to_string(), DAVE.to_string()]);
        Self {
            decisions,
            fences: crate::auth::session_fence::ConsoleSessionFences::detached(),
            access: None,
        }
    }

    fn access(mut self, config: crate::access::AccessControlConfig) -> Self {
        let access = AccessController::new(config).expect("access controller");
        for member in [ASKER, OTHER] {
            access.record_agent_attributes(crate::access::AgentResourceAttributes {
                identity: member.to_string(),
                agent_id: None,
                role: Some("worker".to_string()),
                labels: BTreeMap::new(),
            });
        }
        self.access = Some(access);
        self
    }
}

/// A Console over a mob runtime whose members' question sets are held in
/// the durable decisions of a state directory.
struct QuestionConsole {
    app: Router,
    runtime: MobRuntime,
    name: String,
    decisions: RuntimeDecisions,
    /// The Console send path the surface reads as "a Console is served".
    doors: ConsoleScheduleDoorSlot,
    _owner: Arc<tokio::sync::Mutex<MetaRuntimeHandle>>,
    _directory: tempfile::TempDir,
}

impl QuestionConsole {
    /// The Console `name` over `decisions`, serving a Console.
    async fn start(name: &str, setup: Setup, decisions: RuntimeDecisions) -> Self {
        let owner = crate::start_meta_runtime(
            crate::MetaConfig {
                modules: vec![],
                discovery: crate::DiscoverySpec {
                    namespace: name.to_string(),
                    modules: vec![],
                },
                pre_spawn: vec![],
            },
            vec![],
            Duration::from_secs(1),
        )
        .expect("gating owner");
        let owner = Arc::new(tokio::sync::Mutex::new(owner));
        let (directory, runtime) = super::tests::build_empty_console_test_runtime(name)
            .await
            .expect("console runtime");
        let doors = ConsoleScheduleDoorSlot::default();
        doors.install(Arc::new(NoSends));
        runtime
            .ask_user_slot()
            .fill(AskUserSurface::new(decisions.clone(), doors.served(), name));
        let app = console_json_router_with_state(ConsoleJsonState {
            decisions: setup.decisions,
            session_fences: setup.fences,
            runtime: Some(runtime.clone()),
            module_runtime: Some(owner.clone()),
            access: setup.access,
            durable_decisions: decisions.clone(),
            ..state()
        });
        Self {
            app,
            runtime,
            name: name.to_string(),
            decisions,
            doors,
            _owner: owner,
            _directory: directory,
        }
    }

    /// The host of the member `asker` of this Console's mob.
    fn host(&self, asker: &str) -> MemberAskUserHost {
        MemberAskUserHost::new(
            self.runtime.ask_user_slot().clone(),
            self.name.clone(),
            asker.to_string(),
        )
    }

    /// `asker` asks the row's question set in the call `call_id`.
    async fn ask(&self, asker: &str, call_id: &str) -> AskUserResult {
        ask(&self.host(asker), call_id, 3600).await
    }

    /// `asker` asks, and the pending id of the set it holds.
    async fn asked(&self, asker: &str, call_id: &str) -> String {
        let result = self.ask(asker, call_id).await;
        assert_eq!(result.outcome, AskUserOutcome::Asked, "{result:?}");
        result.request_id.expect("an asked set has a request id")
    }

    fn row(&self, pending_id: &str) -> StoredDecision {
        self.decisions
            .owner()
            .get(&DecisionId::new(pending_id.to_string()).expect("decision id"))
            .expect("decision row")
    }

    fn digest(&self, pending_id: &str) -> String {
        self.row(pending_id).descriptor.subject_digest.to_string()
    }

    /// The pending ids `token` is listed.
    async fn listed(&self, token: Option<&str>) -> Vec<String> {
        let answer = console_rpc_as(&self.app, token, QUESTIONS, json!({})).await;
        answer["result"]["question_sets"]
            .as_array()
            .unwrap_or_else(|| panic!("a list of question sets: {answer}"))
            .iter()
            .map(|set| set["pending_id"].as_str().expect("pending id").to_string())
            .collect()
    }

    /// Answer `pending_id` as `token`: the first choice of the first
    /// question, the second question skipped.
    async fn answer(&self, token: Option<&str>, pending_id: &str) -> Value {
        self.decide(
            token,
            json!({
                "pending_id": pending_id,
                "decision": "approve",
                "presented_digest": self.digest(pending_id),
                "answers": [choice("q1", "a")],
            }),
        )
        .await
    }

    async fn decide(&self, token: Option<&str>, params: Value) -> Value {
        console_rpc_as(&self.app, token, DECIDE, params).await
    }

    /// Arm the driver of the kind's executor over this Console's decisions,
    /// handing the notices to `told`: it starts at once when the store
    /// holds a set, otherwise at the first set held through the surface.
    fn drive(&self, told: &Arc<Told>) -> AskUserDriver {
        spawn_ask_user_driver(
            &self.decisions,
            Arc::clone(told) as Arc<dyn AskUserNotices>,
            Duration::from_secs(3600),
            self.runtime
                .ask_user_slot()
                .held_signal()
                .expect("the surface is filled"),
        )
        .expect("the driver is armed")
    }

    async fn stop(self) {
        let _ = self.runtime.handle().stop().await;
    }
}

/// The row's question set: one question with a written answer allowed and a
/// recommendation, one without.
fn question_set() -> AskUserQuestionSet {
    serde_json::from_value(json!({
        "title": "Release plan",
        "questions": [
            {
                "id": "q1",
                "prompt": "Which day?",
                "choices": [{"id": "a", "label": "Monday"}, {"id": "b", "label": "Friday"}],
                "allow_free_text": true,
                "recommended": "a",
            },
            {
                "id": "q2",
                "prompt": "Who announces it?",
                "choices": [{"id": "x", "label": "The team"}, {"id": "y", "label": "Nobody"}],
            },
        ],
    }))
    .expect("question set")
}

async fn ask(host: &MemberAskUserHost, call_id: &str, timeout_seconds: u64) -> AskUserResult {
    try_ask(host, call_id, timeout_seconds)
        .await
        .expect("the host takes the call")
}

async fn try_ask(
    host: &MemberAskUserHost,
    call_id: &str,
    timeout_seconds: u64,
) -> Result<AskUserResult, meerkat_core::ask_user::AskUserRefusal> {
    host.ask(AskUserRequest {
        questions: &question_set(),
        timeout: Duration::from_secs(timeout_seconds),
        run_id: &RunId::new(),
        call_id,
    })
    .await
}

fn choice(question: &str, choice: &str) -> Value {
    json!({"question_id": question, "answer": {"kind": "choice", "choice_id": choice}})
}

fn text(question: &str, text: &str) -> Value {
    json!({"question_id": question, "answer": {"kind": "free_text", "text": text}})
}

/// BOB's verified token, with the signed login `sid`.
fn bob() -> String {
    let mut claims = human_claims();
    claims["sid"] = json!("login-7");
    token(claims, Some(now_seconds() + 3600))
}

/// CAROL's verified token.
fn carol() -> String {
    token(
        json!({ "sub": format!("oidc-sub-{CAROL}"), "email": CAROL, "provider": "google_oauth" }),
        Some(now_seconds() + 3600),
    )
}

fn refusal_kind(answer: &Value) -> &Value {
    &answer["error"]["data"]["kind"]
}

fn rule(id: &str, subject: &str, actions: &[&str], agents: &[&str]) -> crate::access::AccessRule {
    crate::access::AccessRule {
        id: id.to_string(),
        subjects: vec![subject.to_string()],
        actions: actions.iter().map(|action| (*action).to_string()).collect(),
        agents: agents.iter().map(|agent| (*agent).to_string()).collect(),
        ..crate::access::AccessRule::default()
    }
}

/// Access control enforced: BOB administers; CAROL reads and decides gating
/// and may view `OTHER` only.
fn carol_sees_other_only() -> crate::access::AccessControlConfig {
    crate::access::AccessControlConfig {
        enabled: true,
        admins: vec![BOB.to_string()],
        rules: vec![
            rule(
                "carol-gating",
                CAROL,
                &[ACTION_GATING_VIEW, ACTION_GATING_DECIDE],
                &[],
            ),
            rule("carol-agents", CAROL, &[ACTION_AGENT_VIEW], &[OTHER]),
        ],
        ..crate::access::AccessControlConfig::default()
    }
}

/// A signed-in person answers a set on the Console: the decision is
/// executed with the typed answer in its human evidence (person
/// authenticated, in the login it was made in, on the ingress `console`),
/// and the asking member is told exactly once, with the typed answers; a
/// second executor pass sends nothing.
#[tokio::test]
async fn a_person_answers_a_set_and_its_asker_is_told_once() {
    let state_dir = tempfile::tempdir().expect("state directory");
    let console = QuestionConsole::start(
        "questions-answer",
        Setup::enforced(),
        decisions_in(state_dir.path(), "questions-answer", true),
    )
    .await;
    // The call and its replay (the same run and call id) hold one set.
    let run_id = RunId::new();
    let set = question_set();
    let call = AskUserRequest {
        questions: &set,
        timeout: Duration::from_secs(3600),
        run_id: &run_id,
        call_id: "call-1",
    };
    let host = console.host(ASKER);
    let asked = host.ask(call).await.expect("the host takes the call");
    let replayed = host.ask(call).await.expect("the host takes the replay");
    let pending_id = asked.request_id.clone().expect("an asked set");
    let listed = console.listed(Some(&bob())).await;
    let decided = console.answer(Some(&bob()), &pending_id).await;
    let told = Arc::new(Told::default());
    let driver = console.drive(&told);
    until("the asker is told", || !told.all().is_empty()).await;
    until("the decision is settled", || {
        console.row(&pending_id).notice_receipt.is_some()
    })
    .await;
    let passes = driver.passes();
    console.decisions.notify_work();
    until("another pass was made", || driver.passes() > passes).await;
    let row = console.row(&pending_id);
    let notices = told.all();
    driver.shutdown().await;
    console.stop().await;

    assert_eq!(asked.outcome, AskUserOutcome::Asked, "{asked:?}");
    assert_eq!(replayed, asked, "a replayed call answers the set it held");
    assert_eq!(listed, vec![pending_id.clone()]);
    assert_eq!(
        decided["result"],
        json!({
            "pending_id": pending_id,
            "kind": "question_set",
            "outcome": "answered",
            "decided_at_ms": row.state.decided_at_ms,
        }),
        "{decided}"
    );
    assert_eq!(row.state.phase, DecisionPhase::Executed, "{row:?}");
    let Some(DecisionEvidence::Human {
        assurance,
        authentication_scope,
        authentication_scope_digest,
        ingress,
        approve,
        reason,
        answer,
        ..
    }) = row.outcome.as_ref().map(|outcome| &outcome.evidence)
    else {
        panic!("human evidence: {row:?}");
    };
    assert_eq!(*assurance, ApprovalAssurance::PersonAuthenticated);
    assert_eq!(ingress.as_str(), "console");
    assert!(*approve);
    assert_eq!(*reason, None);
    assert_eq!(*authentication_scope, None, "the login itself is not kept");
    assert_eq!(
        authentication_scope_digest
            .as_ref()
            .map(ToString::to_string),
        Some(Sha256Digest::of(b"login-7").to_string())
    );
    let Some(DecisionAnswer::AskUser { answers }) = answer else {
        panic!("a typed answer: {row:?}");
    };
    assert_eq!(
        serde_json::to_value(answers).unwrap(),
        json!([choice("q1", "a")])
    );
    assert_eq!(notices.len(), 1, "told exactly once: {notices:?}");
    let notice = &notices[0];
    assert_eq!(notice.asker, ASKER);
    assert_eq!(notice.result.outcome, AskUserOutcome::Answered);
    assert_eq!(
        serde_json::to_value(&notice.result.answers).unwrap(),
        json!([
            choice("q1", "a"),
            {"question_id": "q2", "answer": {"kind": "unanswered"}},
        ])
    );
    // The input the asker receives: the label, then the typed result.
    let input = ask_user_result_input(&notice.result).unwrap();
    let (label, document) = input.split_once('\n').expect("a label line");
    assert_eq!(label, "ask_user_result");
    assert_eq!(
        serde_json::from_str::<AskUserResult>(document).unwrap(),
        notice.result
    );
    assert_eq!(
        ask_user_notice_key(&pending_id),
        format!("ask-user:{pending_id}")
    );
}

/// A dismissal, the deadline and an administrator's withdraw each reach the
/// asker: `dismissed`, `timed_out` with every question unanswered, and
/// `withdrawn`. A set past its deadline is no longer listed, before any
/// pass recorded its expiry.
#[tokio::test]
async fn a_dismissal_the_deadline_and_a_withdraw_reach_the_asker() {
    let state_dir = tempfile::tempdir().expect("state directory");
    let now_ms = now_seconds() * 1000;
    let clock = Arc::new(RowClock(AtomicU64::new(now_ms)));
    let owner = DurableDecisionService::with_clock(
        Arc::new(meerkat_store::SqliteDecisionStore::in_store_path(
            state_dir.path(),
        )),
        clock.clone(),
    );
    let decisions =
        decisions_in(state_dir.path(), "questions-ends", true).with_owner_for_test(owner);
    let console = QuestionConsole::start(
        "questions-ends",
        Setup::enforced().access(crate::access::AccessControlConfig {
            enabled: true,
            admins: vec![BOB.to_string()],
            ..crate::access::AccessControlConfig::default()
        }),
        decisions.clone(),
    )
    .await;
    console.runtime.ask_user_slot().fill(
        AskUserSurface::new(decisions, console.doors.served(), "questions-ends")
            .with_clock(clock.clone()),
    );
    let dismissed = console.asked(ASKER, "dismissed").await;
    let withdrawn = console.asked(ASKER, "withdrawn").await;
    let late = ask(&console.host(ASKER), "late", 60)
        .await
        .request_id
        .expect("asked");
    let dismissal = console
        .decide(
            Some(&bob()),
            json!({
                "pending_id": dismissed,
                "decision": "reject",
                "presented_digest": console.digest(&dismissed),
            }),
        )
        .await;
    let withdrawal = console_rpc_as(
        &console.app,
        Some(&bob()),
        "meta/decisions/withdraw",
        json!({ "decision_id": withdrawn }),
    )
    .await;
    let listed_before = console.listed(Some(&bob())).await;
    clock.0.store(now_ms + 61_000, Ordering::SeqCst);
    let listed_after = console.listed(Some(&bob())).await;
    let told = Arc::new(Told::default());
    let driver = console.drive(&told);
    until("the three askers are told", || told.all().len() == 3).await;
    let notices = told.all();
    driver.shutdown().await;
    console.stop().await;

    assert_eq!(
        dismissal["result"]["outcome"],
        json!("dismissed"),
        "{dismissal}"
    );
    assert_eq!(dismissal["result"]["kind"], json!("question_set"));
    assert!(withdrawal["error"].is_null(), "{withdrawal}");
    assert_eq!(
        listed_before,
        vec![late.clone()],
        "ended sets are not listed"
    );
    assert_eq!(
        listed_after,
        Vec::<String>::new(),
        "a set past its deadline"
    );
    let outcome_of = |pending_id: &str| {
        let notice = notices
            .iter()
            .find(|notice| notice.decision_id.as_str() == pending_id)
            .unwrap_or_else(|| panic!("the notice of {pending_id}: {notices:?}"));
        assert!(
            notice
                .result
                .answers
                .iter()
                .all(|answer| answer.answer == AskUserAnswer::Unanswered),
            "{notice:?}"
        );
        assert_eq!(notice.result.answers.len(), 2);
        notice.result.outcome
    };
    assert_eq!(outcome_of(&dismissed), AskUserOutcome::Dismissed);
    assert_eq!(outcome_of(&withdrawn), AskUserOutcome::Withdrawn);
    assert_eq!(outcome_of(&late), AskUserOutcome::TimedOut);
}

/// A restart. A runtime over a state directory whose store already holds a
/// set starts the executor's driver at once, with no new set held, and its
/// first passes settle what the earlier runtime left: an answer it took and
/// never executed is executed and its asker told. A set the earlier runtime
/// held is listed, answered and its notice delivered by the second.
#[tokio::test]
async fn a_set_survives_a_restart() {
    let state_dir = tempfile::tempdir().expect("state directory");
    let first = QuestionConsole::start(
        "questions-restart-1",
        Setup::enforced(),
        decisions_in(state_dir.path(), "questions-restart-1", true),
    )
    .await;
    let pending_id = first.asked(ASKER, "call-1").await;
    // Answered in the first runtime, which runs no driver: left approved.
    let left_id = first.asked(OTHER, "call-2").await;
    let left = first.answer(Some(&bob()), &left_id).await;
    let left_phase = first.row(&left_id).state.phase;
    first.stop().await;

    let second = QuestionConsole::start(
        "questions-restart-2",
        Setup::enforced(),
        decisions_in(state_dir.path(), "questions-restart-2", true),
    )
    .await;
    let told = Arc::new(Told::default());
    let driver = second.drive(&told);
    until(
        "the driver started at once and settled what was left",
        || driver.passes().is_some() && !told.all().is_empty(),
    )
    .await;
    let settled = told.all();
    let settled_phase = second.row(&left_id).state.phase;
    let listed = second.listed(Some(&bob())).await;
    let decided = second.answer(Some(&bob()), &pending_id).await;
    until("the second asker is told", || told.all().len() == 2).await;
    let notices = told.all();
    driver.shutdown().await;
    second.stop().await;

    assert_eq!(left["result"]["outcome"], json!("answered"), "{left}");
    assert_eq!(left_phase, DecisionPhase::Approved);
    assert_eq!(settled.len(), 1, "{settled:?}");
    assert_eq!(settled[0].decision_id.as_str(), left_id);
    assert_eq!(settled[0].result.outcome, AskUserOutcome::Answered);
    assert_eq!(settled_phase, DecisionPhase::Executed);
    assert_eq!(listed, vec![pending_id.clone()]);
    assert_eq!(decided["result"]["outcome"], json!("answered"), "{decided}");
    assert_eq!(notices.len(), 2, "{notices:?}");
    assert_eq!(notices[1].decision_id.as_str(), pending_id);
    assert_eq!(notices[1].result.outcome, AskUserOutcome::Answered);
}

/// Several first sets arriving together start one driver: every call is
/// answered `asked`, none waits for another, and each set's end reaches its
/// asker exactly once.
#[tokio::test]
async fn first_sets_arriving_together_start_one_driver() {
    const TOGETHER: usize = 8;
    let state_dir = tempfile::tempdir().expect("state directory");
    let console = QuestionConsole::start(
        "questions-together",
        Setup::enforced(),
        decisions_in(state_dir.path(), "questions-together", true),
    )
    .await;
    let told = Arc::new(Told::default());
    let driver = console.drive(&told);
    let passes_before = driver.passes();
    let hosts: Vec<MemberAskUserHost> = (0..TOGETHER)
        .map(|n| console.host(&format!("agent-{n}")))
        .collect();
    let results = tokio::time::timeout(
        Duration::from_secs(30),
        futures::future::join_all(hosts.iter().map(|host| try_ask(host, "call-1", 3600))),
    )
    .await;
    until("the driver started", || driver.passes().is_some()).await;
    // While the one driver runs, its claim of the kind is held: no other
    // executor of the kind can start beside it.
    let second_claim = console
        .decisions
        .owner()
        .claim_executor_kind(&DecisionKind::new(ASK_USER_QUESTIONS_KIND).expect("kind"));
    let results = results.expect("no call waits for another");
    let mut pending_ids = Vec::new();
    for result in &results {
        let result = result.as_ref().expect("the host takes the call");
        pending_ids.push(result.request_id.clone().expect("an asked set"));
    }
    for pending_id in &pending_ids {
        console
            .decisions
            .owner()
            .withdraw(
                &DecisionId::new(pending_id.clone()).expect("decision id"),
                &meerkat_runtime::decision::DecisionWithdrawer::Host,
            )
            .expect("withdraw");
    }
    console.decisions.notify_work();
    until("every asker is told", || told.all().len() >= TOGETHER).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    let notices = told.all();
    driver.shutdown().await;
    console.stop().await;

    assert_eq!(passes_before, None, "armed, not started");
    assert!(
        matches!(
            second_claim,
            Err(meerkat_runtime::decision::DecisionError::ExecutorHeld)
        ),
        "the kind's executor claim is held by the one driver"
    );
    pending_ids.sort();
    pending_ids.dedup();
    assert_eq!(pending_ids.len(), TOGETHER, "one set per call");
    let mut told_ids: Vec<String> = notices
        .iter()
        .map(|notice| notice.decision_id.to_string())
        .collect();
    told_ids.sort();
    assert_eq!(told_ids, pending_ids, "each asker is told exactly once");
    assert!(
        notices
            .iter()
            .all(|notice| notice.result.outcome == AskUserOutcome::Withdrawn),
        "{notices:?}"
    );
}

/// Who may answer. A person who may not view the asking member gets the
/// existing `access_denied` and is not listed the set; a request that names
/// an approver is refused; a service token and an open Console's anonymous
/// caller get `approver_unauthenticated`; a caller without `gating.view`
/// reads no question sets. The set stays open throughout, and a person who
/// may view the member then answers it.
#[tokio::test]
async fn only_a_person_who_may_view_the_asker_answers() {
    let state_dir = tempfile::tempdir().expect("state directory");
    let mut config = carol_sees_other_only();
    config
        .rules
        .push(rule("dave-decides", DAVE, &[ACTION_GATING_DECIDE], &[]));
    let setup = Setup::enforced().access(config);
    let console = QuestionConsole::start(
        "questions-who",
        setup,
        decisions_in(state_dir.path(), "questions-who", true),
    )
    .await;
    let pending_id = console.asked(ASKER, "call-1").await;
    let visible_to_carol = console.asked(OTHER, "call-2").await;
    let carol_listed = console.listed(Some(&carol())).await;
    let carol_decides = console.answer(Some(&carol()), &pending_id).await;
    // A caller who may not view the asking member is refused before
    // anything of its request is read.
    let carol_unread = console
        .decide(
            Some(&carol()),
            json!({ "pending_id": pending_id, "decision": "maybe" }),
        )
        .await;
    let named = console
        .decide(
            Some(&bob()),
            json!({
                "pending_id": pending_id,
                "approver_id": "person:bob",
                "decision": "approve",
                "presented_digest": console.digest(&pending_id),
                "answers": [choice("q1", "a")],
            }),
        )
        .await;
    let dave = token(
        json!({ "sub": "oidc-sub-dave", "email": DAVE, "provider": "google_oauth" }),
        Some(now_seconds() + 3600),
    );
    let dave_reads = console_rpc_as(&console.app, Some(&dave), QUESTIONS, json!({})).await;
    let open_after_refusals = console.row(&pending_id).state.phase;
    let bob_answers = console.answer(Some(&bob()), &pending_id).await;
    console.stop().await;

    // A Console with no access controller: a service token, and, once it
    // is open, its anonymous caller.
    let service_dir = tempfile::tempdir().expect("state directory");
    let service_console = QuestionConsole::start(
        "questions-who-service",
        Setup::bare(),
        decisions_in(service_dir.path(), "questions-who-service", true),
    )
    .await;
    let service_pending = service_console.asked(ASKER, "call-1").await;
    let service = service_console
        .answer(
            Some(&token(
                json!({ "email": "svc:robot" }),
                Some(now_seconds() + 3600),
            )),
            &service_pending,
        )
        .await;
    let service_phase = service_console.row(&service_pending).state.phase;
    service_console.stop().await;
    let open_dir = tempfile::tempdir().expect("state directory");
    let mut open = Setup::bare();
    open.decisions.console.require_app_auth = false;
    let open_console = QuestionConsole::start(
        "questions-who-open",
        open,
        decisions_in(open_dir.path(), "questions-who-open", true),
    )
    .await;
    let open_pending = open_console.asked(ASKER, "call-1").await;
    let anonymous = open_console.answer(None, &open_pending).await;
    let open_phase = open_console.row(&open_pending).state.phase;
    open_console.stop().await;

    assert_eq!(
        refusal_kind(&carol_unread),
        &json!("access_denied"),
        "the origin check precedes the request's parse: {carol_unread}"
    );
    assert_eq!(carol_listed, vec![visible_to_carol]);
    assert_eq!(
        refusal_kind(&carol_decides),
        &json!("access_denied"),
        "{carol_decides}"
    );
    assert_eq!(
        carol_decides["error"]["message"],
        json!("access denied to approval origin"),
        "{carol_decides}"
    );
    assert_eq!(named["error"]["code"], json!(-32602), "{named}");
    assert_eq!(
        refusal_kind(&service),
        &json!("approver_unauthenticated"),
        "{service}"
    );
    assert_eq!(service_phase, DecisionPhase::Open);
    assert_eq!(
        refusal_kind(&dave_reads),
        &json!("access_denied"),
        "{dave_reads}"
    );
    assert_eq!(open_after_refusals, DecisionPhase::Open);
    assert_eq!(
        bob_answers["result"]["outcome"],
        json!("answered"),
        "{bob_answers}"
    );
    assert_eq!(
        refusal_kind(&anonymous),
        &json!("approver_unauthenticated"),
        "{anonymous}"
    );
    assert_eq!(open_phase, DecisionPhase::Open);
}

/// A person whose Console sessions are revoked while the answer waits is
/// refused when the decision commits, and the set stays open.
#[tokio::test]
async fn an_answer_rechecks_its_person_when_it_commits() {
    let (_fence_dir, fences) = durable_fences();
    let state_dir = tempfile::tempdir().expect("state directory");
    let mut setup = Setup::enforced();
    setup.fences = fences.clone();
    let console = QuestionConsole::start(
        "questions-recheck",
        setup,
        decisions_in(state_dir.path(), "questions-recheck", true),
    )
    .await;
    let pending_id = console.asked(ASKER, "call-1").await;
    let hold = super::console_decide_holds::ConsoleDecideHold::install(&pending_id);
    let decide = tokio::spawn({
        let app = console.app.clone();
        let bearer = bob();
        let params = json!({
            "pending_id": pending_id,
            "decision": "approve",
            "presented_digest": console.digest(&pending_id),
            "answers": [choice("q1", "a")],
        });
        async move { console_rpc_as(&app, Some(&bearer), DECIDE, params).await }
    });
    let reached = tokio::time::timeout(Duration::from_secs(30), hold.reached()).await;
    let revoked =
        fences.revoke_sessions(BOB, std::time::SystemTime::now() + Duration::from_secs(5));
    hold.release();
    let answer = tokio::time::timeout(Duration::from_secs(30), decide).await;
    let row = console.row(&pending_id);
    console.stop().await;

    reached.expect("the decide reached its hand-off");
    revoked.expect("revoke");
    let answer = answer
        .expect("the decide answered")
        .expect("the decide task");
    assert_eq!(
        refusal_kind(&answer),
        &json!("approver_unauthenticated"),
        "{answer}"
    );
    assert_eq!(row.state.phase, DecisionPhase::Open);
    assert!(row.outcome.is_none());
}

/// A grant withdrawn while the answer waits is honored when the decision
/// commits: without `gating.decide` it is `access_denied`, and so it is
/// once the person may no longer view the asking member.
#[tokio::test]
async fn an_answer_rechecks_its_grants_when_it_commits() {
    let mut refusals = Vec::new();
    for (name, removed) in [
        ("questions-grant-decide", "carol-gating"),
        ("questions-grant-view", "carol-agents"),
    ] {
        let state_dir = tempfile::tempdir().expect("state directory");
        let mut config = carol_sees_other_only();
        config.rules[1].agents = vec![ASKER.to_string()];
        let setup = Setup::enforced().access(config);
        let access = setup.access.clone().expect("access controller");
        let console =
            QuestionConsole::start(name, setup, decisions_in(state_dir.path(), name, true)).await;
        let pending_id = console.asked(ASKER, "call-1").await;
        let hold = super::console_decide_holds::ConsoleDecideHold::install(&pending_id);
        let decide = tokio::spawn({
            let app = console.app.clone();
            let bearer = carol();
            let params = json!({
                "pending_id": pending_id,
                "decision": "approve",
                "presented_digest": console.digest(&pending_id),
                "answers": [choice("q1", "a")],
            });
            async move { console_rpc_as(&app, Some(&bearer), DECIDE, params).await }
        });
        let reached = tokio::time::timeout(Duration::from_secs(30), hold.reached()).await;
        let replaced = access.upsert_rule(match removed {
            "carol-gating" => rule("carol-gating", CAROL, &[ACTION_GATING_VIEW], &[]),
            _ => rule("carol-agents", CAROL, &[ACTION_AGENT_VIEW], &[OTHER]),
        });
        hold.release();
        let answer = tokio::time::timeout(Duration::from_secs(30), decide).await;
        let phase = console.row(&pending_id).state.phase;
        console.stop().await;
        reached.expect("the decide reached its hand-off");
        replaced.expect("the configuration is replaced");
        refusals.push((
            removed,
            answer.expect("answered").expect("the decide task"),
            phase,
        ));
    }

    // Every case is judged, and each one that is not refused as it should
    // be is named.
    let mut honored = Vec::new();
    for (removed, answer, phase) in refusals {
        let message = match removed {
            "carol-gating" => "access denied to gating decisions",
            _ => "access denied to approval origin",
        };
        if refusal_kind(&answer) != &json!("access_denied")
            || answer["error"]["message"] != json!(message)
            || phase != DecisionPhase::Open
        {
            honored.push(format!("{removed} withdrawn: {phase:?} {answer}"));
        }
    }
    assert!(
        honored.is_empty(),
        "a grant withdrawn while the answer waits is honored: {honored:#?}"
    );
}

/// A member private to a person: its set is not listed for others and not
/// answerable by others; its owner is listed it and answers it.
#[tokio::test]
async fn a_private_members_set_is_its_owners() {
    let state_dir = tempfile::tempdir().expect("state directory");
    let mut setup = Setup::enforced().access(crate::access::AccessControlConfig::default());
    setup
        .access
        .as_ref()
        .expect("access controller")
        .record_agent_attributes(crate::access::AgentResourceAttributes {
            identity: "agent-p".to_string(),
            agent_id: None,
            role: Some("worker".to_string()),
            labels: BTreeMap::from([(
                crate::access::PRIVATE_TO_LABEL.to_string(),
                person_private_owner_key(ISSUER, "oidc-sub-7"),
            )]),
        });
    setup.fences = crate::auth::session_fence::ConsoleSessionFences::detached();
    let console = QuestionConsole::start(
        "questions-private",
        setup,
        decisions_in(state_dir.path(), "questions-private", true),
    )
    .await;
    let pending_id = console.asked("agent-p", "call-1").await;
    let others_list = console.listed(Some(&carol())).await;
    let others_answer = console.answer(Some(&carol()), &pending_id).await;
    let phase_after_other = console.row(&pending_id).state.phase;
    let owners_list = console.listed(Some(&bob())).await;
    let owners_answer = console.answer(Some(&bob()), &pending_id).await;
    console.stop().await;

    assert_eq!(others_list, Vec::<String>::new());
    assert_eq!(
        refusal_kind(&others_answer),
        &json!("access_denied"),
        "{others_answer}"
    );
    assert_eq!(phase_after_other, DecisionPhase::Open);
    assert_eq!(owners_list, vec![pending_id]);
    assert_eq!(
        owners_answer["result"]["outcome"],
        json!("answered"),
        "{owners_answer}"
    );
}

/// An open decision of `kind` in `decisions`, held as its own owner holds
/// one.
fn held_of_kind(decisions: &RuntimeDecisions, kind: &str) -> StoredDecision {
    let requester = DecisionPrincipalKey::of(
        &PrincipalRef::in_domain(
            PrincipalKind::SharedAgent,
            "builder",
            TrustDomainId::new("example.org").expect("domain"),
        )
        .expect("principal"),
    )
    .expect("key");
    decisions
        .owner()
        .hold(
            DecisionHoldRequest::new(
                DecisionKind::new(kind).expect("kind"),
                DecisionSubjectKey::new("subject").expect("subject"),
                DecisionOperationId::new(format!("operation:{kind}")).expect("operation"),
                Sha256Digest::of(br#"{"build":7}"#),
                DecisionRequestBody::from_value(&json!({"build": 7})).expect("body"),
                requester,
                ApproverId::new("team:owners").expect("approver id"),
                u64::MAX / 2,
                DecisionRendering::new("Release build 7.").expect("rendering"),
                DecisionRenderer::new("render.v1").expect("renderer"),
            ),
            false,
        )
        .expect("hold")
}

/// The answer of `answer` with the pending id `pending_id` it names
/// replaced by a placeholder, so two refusals compare.
fn refusal_shape(answer: &Value, pending_id: &str) -> Value {
    json!({
        "result": answer["result"],
        "code": answer["error"]["code"],
        "message": answer["error"]["message"]
            .as_str()
            .map(|message| message.replace(pending_id, "<id>")),
        "data": answer["error"]["data"],
    })
}

/// The hand-off's kind match is closed. The id of an open decision of every
/// other kind people decide, and of a made-up kind, sent to
/// `meta/gating/decide` on the Console and on stdio, is refused exactly as
/// an unknown pending id is, and the decision stays open and unpresented.
#[tokio::test]
async fn a_decision_of_another_kind_is_an_unknown_pending_id() {
    let state_dir = tempfile::tempdir().expect("state directory");
    let console = QuestionConsole::start(
        "questions-closed-kind",
        Setup::enforced(),
        decisions_in(state_dir.path(), "questions-closed-kind", true),
    )
    .await;
    let unknown = format!("decision:{}", "0".repeat(64));
    let params = |pending_id: &str, digest: &str| {
        json!({
            "pending_id": pending_id,
            "decision": "approve",
            "presented_digest": digest,
            "answers": [choice("q1", "a")],
        })
    };
    let zero_digest = Sha256Digest::of(b"").to_string();
    let unknown_refusal = console
        .decide(Some(&bob()), params(&unknown, &zero_digest))
        .await;
    let mut kinds: Vec<&str> = DECIDED_BY_PEOPLE_ONLY
        .iter()
        .copied()
        .filter(|kind| *kind != ASK_USER_QUESTIONS_KIND)
        .collect();
    kinds.push("made_up.kind");
    let mut rows = Vec::new();
    for kind in &kinds {
        let held = held_of_kind(&console.decisions, kind);
        let pending_id = held.decision_id.to_string();
        let digest = held.descriptor.subject_digest.to_string();
        let on_console = console
            .decide(Some(&bob()), params(&pending_id, &digest))
            .await;
        let mut stdio_params = params(&pending_id, &digest);
        stdio_params["approver_id"] = json!("person:bob");
        let names_one = crate::rpc::question_set_methods::gating_decide_names_question_set(
            &console.runtime,
            &stdio_params,
        );
        rows.push((
            *kind,
            pending_id.clone(),
            on_console,
            names_one,
            console.row(&pending_id),
        ));
    }
    // A question set itself is one the stdio branch takes.
    let set = console.asked(ASKER, "call-1").await;
    let names_the_set = crate::rpc::question_set_methods::gating_decide_names_question_set(
        &console.runtime,
        &json!({ "pending_id": set }),
    );
    console.stop().await;

    assert!(
        DECIDED_BY_PEOPLE_ONLY.contains(&ASK_USER_QUESTIONS_KIND),
        "the kind is decided by people only"
    );
    assert!(kinds.len() > 1, "{kinds:?}");
    assert!(
        unknown_refusal["error"]["code"].is_i64(),
        "an unknown pending id is refused: {unknown_refusal}"
    );
    assert!(names_the_set);
    for (kind, pending_id, on_console, names_one, row) in rows {
        assert_eq!(
            refusal_shape(&on_console, &pending_id),
            refusal_shape(&unknown_refusal, &unknown),
            "{kind}: {on_console}"
        );
        assert!(!names_one, "{kind}: the stdio branch leaves it alone");
        assert_eq!(row.state.phase, DecisionPhase::Open, "{kind}");
        assert!(row.outcome.is_none(), "{kind}");
        assert!(row.presentations.is_empty(), "{kind}");
        assert_eq!(row.state.first_presented_to_approver_at_ms, None, "{kind}");
    }
}

/// Requests that do not answer the set are invalid params, and the set
/// stays open: an approval without answers, an unknown question, an unknown
/// choice, a written answer where none is accepted, an over-long written
/// answer, a question answered twice, another digest than the set's, and an
/// escalation.
#[tokio::test]
async fn an_invalid_answer_leaves_the_set_open() {
    let state_dir = tempfile::tempdir().expect("state directory");
    let console = QuestionConsole::start(
        "questions-invalid",
        Setup::enforced(),
        decisions_in(state_dir.path(), "questions-invalid", true),
    )
    .await;
    let pending_id = console.asked(ASKER, "call-1").await;
    let digest = console.digest(&pending_id);
    let approve = |answers: Value| {
        json!({
            "pending_id": pending_id,
            "decision": "approve",
            "presented_digest": digest,
            "answers": answers,
        })
    };
    let requests = vec![
        (
            "no answers",
            json!({ "pending_id": pending_id, "decision": "approve", "presented_digest": digest }),
        ),
        ("empty answers", approve(json!([]))),
        ("unknown question", approve(json!([choice("q9", "a")]))),
        ("unknown choice", approve(json!([choice("q1", "z")]))),
        ("free text not allowed", approve(json!([text("q2", "me")]))),
        (
            "over-long text",
            approve(json!([text("q1", &"x".repeat(281))])),
        ),
        (
            "repeated question",
            approve(json!([choice("q1", "a"), choice("q1", "b")])),
        ),
        (
            "unanswered given",
            approve(json!([{"question_id": "q1", "answer": {"kind": "unanswered"}}])),
        ),
        (
            "wrong digest",
            json!({
                "pending_id": pending_id,
                "decision": "approve",
                "presented_digest": Sha256Digest::of(b"another set").to_string(),
                "answers": [choice("q1", "a")],
            }),
        ),
        (
            "no digest",
            json!({ "pending_id": pending_id, "decision": "reject" }),
        ),
        (
            "escalate",
            json!({ "pending_id": pending_id, "decision": "escalate", "presented_digest": digest }),
        ),
        (
            "another word",
            json!({ "pending_id": pending_id, "decision": "maybe", "presented_digest": digest }),
        ),
    ];
    // A refused request records nothing: the decide event of a request the
    // set would not take is never bound.
    let recorded_before = other_files(state_dir.path());
    let mut answers = Vec::new();
    for (label, params) in requests {
        let answer = console.decide(Some(&bob()), params).await;
        answers.push((label, answer, console.row(&pending_id).state.phase));
    }
    let recorded_after_refusals = other_files(state_dir.path());
    // A trimmed written answer of exactly the bound is taken.
    let trimmed = console
        .decide(
            Some(&bob()),
            approve(json!([text("q1", &format!("  {}  ", "x".repeat(280)))])),
        )
        .await;
    let recorded_after_answer = other_files(state_dir.path());
    let row = console.row(&pending_id);
    console.stop().await;

    for (label, answer, phase) in answers {
        assert_eq!(answer["error"]["code"], json!(-32602), "{label}: {answer}");
        assert!(answer["error"]["data"].is_null(), "{label}: {answer}");
        assert_eq!(phase, DecisionPhase::Open, "{label}");
    }
    assert!(
        recorded_before == recorded_after_refusals,
        "a refused request records nothing outside the decision store"
    );
    assert!(
        recorded_after_refusals != recorded_after_answer,
        "the decide event of the answer that is taken is recorded"
    );
    assert_eq!(trimmed["result"]["outcome"], json!("answered"), "{trimmed}");
    let Some(DecisionEvidence::Human {
        answer: Some(DecisionAnswer::AskUser { answers }),
        ..
    }) = row.outcome.as_ref().map(|outcome| &outcome.evidence)
    else {
        panic!("a typed answer: {row:?}");
    };
    assert_eq!(
        answers[0].answer,
        AskUserAnswer::FreeText {
            text: "x".repeat(280)
        }
    );
}

/// `meta/gating/questions`: the wire object of each open set, exact, the
/// soonest deadline first; the stdio read lists every set.
#[tokio::test]
async fn the_read_lists_open_sets_soonest_deadline_first() {
    let state_dir = tempfile::tempdir().expect("state directory");
    let console = QuestionConsole::start(
        "questions-read",
        Setup::enforced().access(carol_sees_other_only()),
        decisions_in(state_dir.path(), "questions-read", true),
    )
    .await;
    let later = ask(&console.host(ASKER), "later", 7200)
        .await
        .request_id
        .expect("asked");
    let sooner = ask(&console.host(OTHER), "sooner", 600)
        .await
        .request_id
        .expect("asked");
    let read = console_rpc_as(&console.app, Some(&bob()), QUESTIONS, json!({})).await;
    let stdio = crate::rpc::question_set_methods::gating_questions_response(
        Some(&console.decisions),
        &Value::Null,
        json!(1),
    );
    let stdio_invalid = crate::rpc::question_set_methods::gating_questions_response(
        Some(&console.decisions),
        &json!([]),
        json!(2),
    );
    let none_held =
        crate::rpc::question_set_methods::gating_questions_response(None, &json!({}), json!(3));
    let carol_listed = console.listed(Some(&carol())).await;
    let rows = [console.row(&sooner), console.row(&later)];
    console.stop().await;

    let wire = |row: &StoredDecision, identity: &str| {
        json!({
            "pending_id": row.decision_id.to_string(),
            "origin": { "identity": identity },
            "title": "Release plan",
            "questions": [
                {
                    "id": "q1",
                    "prompt": "Which day?",
                    "choices": [{"id": "a", "label": "Monday"}, {"id": "b", "label": "Friday"}],
                    "allow_free_text": true,
                    "recommended": "a",
                },
                {
                    "id": "q2",
                    "prompt": "Who announces it?",
                    "choices": [{"id": "x", "label": "The team"}, {"id": "y", "label": "Nobody"}],
                },
            ],
            "created_at_ms": row.descriptor.created_at_ms,
            "deadline_at_ms": row.descriptor.deadline_at_ms,
            "subject_digest": row.descriptor.subject_digest.to_string(),
        })
    };
    let expected = json!({ "question_sets": [wire(&rows[0], OTHER), wire(&rows[1], ASKER)] });
    assert_eq!(read["result"], expected, "{read}");
    assert_eq!(
        stdio.result,
        Some(expected),
        "the host reads every set: {:?}",
        stdio.error
    );
    assert_eq!(
        stdio_invalid.error.as_ref().map(|error| error.code),
        Some(-32602)
    );
    assert_eq!(none_held.result, Some(json!({ "question_sets": [] })));
    assert_eq!(carol_listed, vec![sooner], "CAROL may view {OTHER} only");
    assert!(rows[0].descriptor.deadline_at_ms < rows[1].descriptor.deadline_at_ms);
    assert!(
        rows[0]
            .descriptor
            .subject_digest
            .to_string()
            .starts_with("sha256:")
    );
}

/// The stdio decide is the host's: `approver_id` is required as for every
/// gating decide; the decision is recorded as the host operator's, device
/// trusted, on the ingress `host`, with who the host vouched for.
#[tokio::test]
async fn the_host_answers_a_set_on_stdio() {
    let state_dir = tempfile::tempdir().expect("state directory");
    let console = QuestionConsole::start(
        "questions-stdio",
        Setup::enforced(),
        decisions_in(state_dir.path(), "questions-stdio", true),
    )
    .await;
    let pending_id = console.asked(ASKER, "call-1").await;
    let dismissed_id = console.asked(ASKER, "call-2").await;
    let mut params = json!({
        "pending_id": pending_id,
        "decision": "approve",
        "presented_digest": console.digest(&pending_id),
        "answers": [choice("q1", "b"), choice("q2", "x")],
    });
    let unnamed = crate::rpc::question_set_methods::gating_decide_question_set(
        &console.runtime,
        &params,
        json!(1),
    );
    let phase_after_unnamed = console.row(&pending_id).state.phase;
    params["approver_id"] = json!("person:dana");
    let decided = crate::rpc::question_set_methods::gating_decide_question_set(
        &console.runtime,
        &params,
        json!(2),
    );
    let again = crate::rpc::question_set_methods::gating_decide_question_set(
        &console.runtime,
        &params,
        json!(3),
    );
    let dismissed = crate::rpc::question_set_methods::gating_decide_question_set(
        &console.runtime,
        &json!({
            "pending_id": dismissed_id,
            "approver_id": "person:dana",
            "decision": "reject",
            "presented_digest": console.digest(&dismissed_id),
        }),
        json!(4),
    );
    // The stdio decide reads its request as every gating decide does:
    // the words it takes are the ones it took before.
    let deny = crate::rpc::question_set_methods::gating_decide_question_set(
        &console.runtime,
        &json!({
            "pending_id": pending_id,
            "approver_id": "person:dana",
            "decision": "deny",
            "presented_digest": console.digest(&pending_id),
        }),
        json!(5),
    );
    let row = console.row(&pending_id);
    console.stop().await;

    let unnamed = unnamed.error.expect("an approver id is required");
    assert_eq!(unnamed.code, -32602);
    assert_eq!(
        deny.error.as_ref().map(|error| error.code),
        Some(-32602),
        "{:?}",
        deny.error
    );
    assert!(unnamed.message.contains("approver_id"), "{unnamed:?}");
    assert_eq!(phase_after_unnamed, DecisionPhase::Open);
    assert_eq!(
        decided.result,
        Some(json!({
            "pending_id": pending_id,
            "kind": "question_set",
            "outcome": "answered",
            "decided_at_ms": row.state.decided_at_ms,
        })),
        "{:?}",
        decided.error
    );
    let again = again.error.expect("an answered set takes no second answer");
    assert_eq!(again.code, -32602);
    assert!(again.message.contains("no longer open"), "{again:?}");
    assert_eq!(
        dismissed.result.as_ref().map(|result| &result["outcome"]),
        Some(&json!("dismissed")),
        "{:?}",
        dismissed.error
    );
    let Some(DecisionEvidence::Human {
        assurance,
        ingress,
        reason,
        answer,
        ..
    }) = row.outcome.as_ref().map(|outcome| &outcome.evidence)
    else {
        panic!("human evidence: {row:?}");
    };
    assert_eq!(*assurance, ApprovalAssurance::DeviceTrusted);
    assert_eq!(ingress.as_str(), "host");
    assert_eq!(reason.as_deref(), Some("the host answered for person:dana"));
    assert!(matches!(answer, Some(DecisionAnswer::AskUser { answers }) if answers.len() == 2));
}

/// The names and sizes of the files under `dir` but the decision store's
/// own.
fn other_files(dir: &std::path::Path) -> BTreeMap<String, (u64, Vec<u8>)> {
    let mut files = BTreeMap::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(next) = pending.pop() {
        for entry in std::fs::read_dir(&next).expect("read the state directory") {
            let path = entry.expect("entry").path();
            if path.is_dir() {
                pending.push(path);
                continue;
            }
            let name = path
                .strip_prefix(dir)
                .expect("under the state directory")
                .display()
                .to_string();
            if name.starts_with("approval_decisions.") {
                continue;
            }
            let bytes = std::fs::read(&path).expect("read a state file");
            files.insert(name, (bytes.len() as u64, bytes));
        }
    }
    files
}

/// Executing an answered set stores no copy of its evidence outside the
/// decision row: no other file of the state directory changes at the
/// execution, and none holds the written answer.
#[tokio::test]
async fn executing_an_answer_writes_nothing_outside_the_decision_store() {
    let (state_dir, fences) = durable_fences();
    let mut setup = Setup::enforced();
    setup.fences = fences;
    let console = QuestionConsole::start(
        "questions-evidence",
        setup,
        decisions_in(state_dir.path(), "questions-evidence", true),
    )
    .await;
    let pending_id = console.asked(ASKER, "call-1").await;
    let decided = console
        .decide(
            Some(&bob()),
            json!({
                "pending_id": pending_id,
                "decision": "approve",
                "presented_digest": console.digest(&pending_id),
                "answers": [text("q1", "a-written-answer-nobody-else-keeps")],
            }),
        )
        .await;
    let before = other_files(state_dir.path());
    let told = Arc::new(Told::default());
    let driver = console.drive(&told);
    until("the answer is executed and its asker told", || {
        console.row(&pending_id).notice_receipt.is_some()
    })
    .await;
    driver.shutdown().await;
    let after = other_files(state_dir.path());
    let phase = console.row(&pending_id).state.phase;
    console.stop().await;

    assert_eq!(decided["result"]["outcome"], json!("answered"), "{decided}");
    assert_eq!(phase, DecisionPhase::Executed);
    assert!(
        state_dir.path().join(DECISIONS_FILE).exists(),
        "the decision store holds the set"
    );
    assert_eq!(
        before.keys().collect::<Vec<_>>(),
        after.keys().collect::<Vec<_>>(),
        "no file appears at the execution"
    );
    assert_eq!(before, after, "no other file changes at the execution");
    for (name, (_, bytes)) in &after {
        assert!(
            !String::from_utf8_lossy(bytes).contains("a-written-answer-nobody-else-keeps"),
            "{name} holds a copy of the answer"
        );
    }
}

/// No person can be asked: the runtime serves no Console, keeps no durable
/// decisions, is a candidate launch, has not filled the surface, or cannot
/// route the answer back to the asker. The call answers `no_human_surface`
/// at once and writes no decision.
#[tokio::test]
async fn a_call_without_a_human_surface_holds_nothing() {
    let state_dir = tempfile::tempdir().expect("state directory");
    let name = "questions-no-surface";
    let doors = ConsoleScheduleDoorSlot::default();
    let served = ConsoleScheduleDoorSlot::default();
    served.install(Arc::new(NoSends));
    let available = decisions_in(state_dir.path(), name, true);
    let surfaces: Vec<(&str, Option<AskUserSurface>, &str, &str)> = vec![
        ("unfilled", None, name, ASKER),
        (
            "no Console served",
            Some(AskUserSurface::new(available.clone(), doors.served(), name)),
            name,
            ASKER,
        ),
        (
            "no decision store",
            Some(AskUserSurface::new(
                RuntimeDecisions::unavailable(),
                served.served(),
                name,
            )),
            name,
            ASKER,
        ),
        (
            "candidate launch",
            Some(AskUserSurface::new(
                decisions_in(state_dir.path(), name, false),
                served.served(),
                name,
            )),
            name,
            ASKER,
        ),
        (
            "a member of another mob",
            Some(AskUserSurface::new(
                available.clone(),
                served.served(),
                name,
            )),
            "a-child-mob",
            ASKER,
        ),
        (
            "a name that is no agent name",
            Some(AskUserSurface::new(
                available.clone(),
                served.served(),
                name,
            )),
            name,
            "not an agent name",
        ),
    ];
    let mut results = Vec::new();
    for (label, surface, mob_id, asker) in surfaces {
        let slot = AskUserSlot::default();
        if let Some(surface) = surface {
            slot.fill(surface);
        }
        let host = MemberAskUserHost::new(slot, mob_id.to_string(), asker.to_string());
        results.push((label, try_ask(&host, "call-1", 3600).await));
    }
    // The same member is asked once the surface is whole, and no more once
    // the Console's send path is cleared or the slot emptied.
    let slot = AskUserSlot::default();
    slot.fill(AskUserSurface::new(
        available.clone(),
        served.served(),
        name,
    ));
    let host = MemberAskUserHost::new(slot.clone(), name.to_string(), ASKER.to_string());
    let file_before = state_dir.path().join(DECISIONS_FILE).exists();
    let whole = ask(&host, "call-1", 3600).await;
    served.clear();
    let cleared = ask(&host, "call-2", 3600).await;
    served.install(Arc::new(NoSends));
    slot.clear();
    let emptied = ask(&host, "call-3", 3600).await;
    let open = crate::ask_user::open_question_sets(&available).expect("read");

    // Every case is judged, and each one that held or failed is named.
    let held: Vec<String> = results
        .iter()
        .filter(|(_, result)| *result != Ok(AskUserResult::no_human_surface()))
        .map(|(label, result)| format!("{label}: {result:?}"))
        .collect();
    assert!(
        held.is_empty(),
        "each case answers no_human_surface: {held:#?}"
    );
    assert!(!file_before, "a refused call writes no decision store");
    assert_eq!(whole.outcome, AskUserOutcome::Asked, "{whole:?}");
    assert_eq!(cleared, AskUserResult::no_human_surface());
    assert_eq!(emptied, AskUserResult::no_human_surface());
    assert_eq!(open.len(), 1, "only the whole surface held a set");
}

/// The names of the decision store's files under `dir`: the store itself
/// and the lock files of its executors.
fn decision_store_files(dir: &std::path::Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .expect("read the state directory")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .filter(|name| name.starts_with("approval_decisions"))
        .collect();
    names.sort();
    names
}

/// Nothing is opened for a runtime that never holds a set: the executor's
/// driver is armed and claims nothing, makes no pass and creates no file,
/// and the read lists nothing and creates no decision store. The first set
/// held starts the driver. No driver is armed for a runtime that keeps no
/// durable decisions or on a candidate launch.
#[tokio::test]
async fn a_runtime_that_holds_no_set_opens_nothing() {
    let state_dir = tempfile::tempdir().expect("state directory");
    let console = QuestionConsole::start(
        "questions-lazy",
        Setup::enforced(),
        decisions_in(state_dir.path(), "questions-lazy", true),
    )
    .await;
    let told = Arc::new(Told::default());
    let driver = console.drive(&told);
    let listed = console.listed(Some(&bob())).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let passes_before = driver.passes();
    let files_before = decision_store_files(state_dir.path());
    let none_unavailable = spawn_ask_user_driver(
        &RuntimeDecisions::unavailable(),
        Arc::clone(&told) as Arc<dyn AskUserNotices>,
        Duration::from_secs(3600),
        Arc::default(),
    );
    let none_candidate = spawn_ask_user_driver(
        &decisions_in(state_dir.path(), "questions-lazy", false),
        Arc::clone(&told) as Arc<dyn AskUserNotices>,
        Duration::from_secs(3600),
        Arc::default(),
    );
    // The first set held starts the driver.
    let _pending_id = console.asked(ASKER, "call-1").await;
    until("the driver made its first pass", || {
        driver.passes().is_some_and(|passes| passes >= 1)
    })
    .await;
    let files_after = decision_store_files(state_dir.path());
    driver.shutdown().await;
    console.stop().await;

    assert_eq!(listed, Vec::<String>::new());
    assert_eq!(passes_before, None, "no pass before the first set");
    assert_eq!(
        files_before,
        Vec::<String>::new(),
        "no file of the decision store before the first set"
    );
    assert!(
        files_after.iter().any(|name| name == DECISIONS_FILE),
        "{files_after:?}"
    );
    assert!(none_unavailable.is_none());
    assert!(none_candidate.is_none());
    assert!(told.all().is_empty());
}

/// A dispatcher of `names`.
struct Named(Arc<[Arc<meerkat_core::types::ToolDef>]>);

impl Named {
    fn of(names: &[&str]) -> Arc<dyn meerkat_core::AgentToolDispatcher> {
        Arc::new(Self(
            names
                .iter()
                .map(|name| {
                    Arc::new(meerkat_core::types::ToolDef::new(
                        *name,
                        "a host tool",
                        json!({"type": "object"}),
                    ))
                })
                .collect(),
        ))
    }
}

#[async_trait::async_trait]
impl meerkat_core::AgentToolDispatcher for Named {
    fn tools(&self) -> Arc<[Arc<meerkat_core::types::ToolDef>]> {
        Arc::clone(&self.0)
    }

    async fn dispatch(
        &self,
        call: meerkat_core::types::ToolCallView<'_>,
    ) -> Result<meerkat_core::ToolDispatchOutcome, meerkat_core::ToolError> {
        Err(meerkat_core::ToolError::not_found(call.name))
    }
}

fn build_request(
    member: Option<&str>,
    tools: Option<Arc<dyn meerkat_core::AgentToolDispatcher>>,
) -> meerkat_core::service::CreateSessionRequest {
    meerkat_core::service::CreateSessionRequest {
        model: "gpt-5.5".to_string(),
        prompt: meerkat_core::types::ContentInput::Text("hello".to_string()),
        system_prompt: meerkat_core::config::SystemPromptOverride::Inherit,
        max_tokens: None,
        event_tx: None,
        initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
        build: Some(meerkat_core::service::SessionBuildOptions {
            mob_member_binding: member.map(|member| meerkat_core::MobMemberBinding {
                mob_id: "team".to_string(),
                role: "worker".to_string(),
                member: member.to_string(),
            }),
            external_tools: tools,
            ..Default::default()
        }),
        labels: None,
        deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::default(),
        injected_context: Vec::new(),
    }
}

fn tool_names(request: &meerkat_core::service::CreateSessionRequest) -> Vec<String> {
    let mut names: Vec<String> = request
        .build
        .as_ref()
        .and_then(|build| build.external_tools.as_ref())
        .map(|tools| {
            tools
                .tools()
                .iter()
                .map(|tool| tool.name.to_string())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

/// Every member build mounts the tool over the tools it already has; a
/// request that is not a member build gets none, and a build whose tools
/// already name `ask_user` keeps its own.
#[test]
fn every_member_build_mounts_the_tool() {
    let slot = AskUserSlot::default();
    let mut bare = build_request(Some("agent-a"), None);
    let mut over = build_request(Some("agent-a"), Some(Named::of(&["memory", "probe"])));
    let mut plain = build_request(None, Some(Named::of(&["probe"])));
    let mut own = build_request(Some("agent-a"), Some(Named::of(&[ASK_USER_TOOL])));
    let own_before = own
        .build
        .as_ref()
        .and_then(|build| build.external_tools.clone());
    for request in [&mut bare, &mut over, &mut plain, &mut own] {
        slot.mount(request);
    }
    // The mounted tool answers through the slot, read at the call.
    let mounted = bare
        .build
        .as_ref()
        .and_then(|build| build.external_tools.clone())
        .expect("mounted");

    assert_eq!(tool_names(&bare), vec![ASK_USER_TOOL.to_string()]);
    assert_eq!(
        tool_names(&over),
        vec![
            ASK_USER_TOOL.to_string(),
            "memory".to_string(),
            "probe".to_string()
        ]
    );
    assert_eq!(tool_names(&plain), vec!["probe".to_string()]);
    assert_eq!(tool_names(&own), vec![ASK_USER_TOOL.to_string()]);
    assert!(
        Arc::ptr_eq(
            own.build
                .as_ref()
                .and_then(|build| build.external_tools.as_ref())
                .expect("its own tool"),
            own_before.as_ref().expect("its own tool")
        ),
        "a build with its own ask_user keeps it"
    );
    assert!(mounted.tool_catalog_capabilities().exact_catalog);
}

/// The pins this feature leaves as they are, named for the record: the
/// Console serves no `meta/decisions/decide`
/// (`tests/decisions_console.rs`), and every Console method is in the
/// member field table
/// (`console_private_audience_tests::every_console_method_is_in_the_member_field_table`).
/// Here: the read is no change, it needs `gating.view`, names no member,
/// and the Console's capabilities list it beside the approvals read.
#[tokio::test]
async fn the_read_is_a_gating_view_read() {
    let state_dir = tempfile::tempdir().expect("state directory");
    let console = QuestionConsole::start(
        "questions-capabilities",
        Setup::enforced(),
        decisions_in(state_dir.path(), "questions-capabilities", true),
    )
    .await;
    let capabilities =
        console_rpc_as(&console.app, Some(&bob()), "meta/capabilities", json!({})).await;
    let decisions_decide = console_rpc_as(
        &console.app,
        Some(&bob()),
        "meta/decisions/decide",
        json!({ "decision_id": format!("decision:{}", "0".repeat(64)) }),
    )
    .await;
    console.stop().await;

    assert!(!is_console_mutating_rpc_method(QUESTIONS));
    assert_eq!(
        console_rpc_access_requirements(QUESTIONS, &json!({})),
        console_rpc_access_requirements("meta/gating/pending", &json!({})),
    );
    assert_eq!(
        console_method_member_fields(QUESTIONS).map(<[ConsoleMemberField]>::len),
        Some(0)
    );
    let methods = capabilities["result"]["methods"]
        .as_array()
        .unwrap_or_else(|| panic!("capabilities: {capabilities}"));
    assert!(methods.contains(&json!(QUESTIONS)), "{capabilities}");
    assert!(methods.contains(&json!("meta/gating/pending")));
    assert_eq!(
        decisions_decide["error"]["code"],
        json!(-32601),
        "{decisions_decide}"
    );
}
