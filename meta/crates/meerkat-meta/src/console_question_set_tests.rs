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
/// serves a Console, and whether a person can decide on it.
struct NoSends {
    a_person_decides: bool,
}

#[async_trait::async_trait]
impl ConsoleScheduleDoor for NoSends {
    fn takes_a_persons_decision(&self) -> bool {
        self.a_person_decides
    }

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
        // The Console's own fact, as its send path states it.
        doors.install(Arc::new(NoSends {
            a_person_decides: crate::runtime::console_takes_a_persons_decision(
                &setup.decisions,
                setup.access.as_ref(),
            ),
        }));
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
    /// question, the second question skipped. An id that names no decision
    /// is sent with a digest of nothing.
    async fn answer(&self, token: Option<&str>, pending_id: &str) -> Value {
        let digest = DecisionId::new(pending_id.to_string())
            .ok()
            .and_then(|id| self.decisions.owner().get(&id).ok())
            .map_or_else(
                || Sha256Digest::of(b"no set").to_string(),
                |row| row.descriptor.subject_digest.to_string(),
            );
        self.decide(
            token,
            json!({
                "pending_id": pending_id,
                "decision": "approve",
                "presented_digest": digest,
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
            Duration::from_hours(1),
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
        timeout: Duration::from_hours(1),
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
    // The set of a member the caller may not view is decided exactly as an
    // id that names nothing, whatever the request says.
    let unknown = format!("decision:{}", "0".repeat(64));
    let carol_unknown = console.answer(Some(&carol()), &unknown).await;
    let carol_unread = console
        .decide(
            Some(&carol()),
            json!({ "pending_id": pending_id, "decision": "maybe" }),
        )
        .await;
    let carol_unread_unknown = console
        .decide(
            Some(&carol()),
            json!({ "pending_id": unknown, "decision": "maybe" }),
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

    // A Console whose access controller enforces nothing (it knows the
    // asking member): a service token, and, once it is open, its anonymous caller.
    let service_dir = tempfile::tempdir().expect("state directory");
    let service_console = QuestionConsole::start(
        "questions-who-service",
        Setup::enforced(),
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
    let mut open = Setup::enforced();
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
    // This open Console has an access controller and a trusted issuer: a
    // person who volunteers a token is identified, and answers.
    let volunteered = open_console.answer(Some(&bob()), &open_pending).await;
    open_console.stop().await;

    assert_eq!(
        volunteered["result"]["outcome"],
        json!("answered"),
        "a person who signs in on an open Console answers: {volunteered}"
    );
    assert_eq!(
        refusal_shape(&carol_unread, &pending_id),
        refusal_shape(&carol_unread_unknown, &unknown),
        "a hidden set's decide reads as an unknown id's, whatever it asks: {carol_unread}"
    );
    assert_eq!(carol_listed, vec![visible_to_carol]);
    assert_eq!(
        refusal_shape(&carol_decides, &pending_id),
        refusal_shape(&carol_unknown, &unknown),
        "a hidden set is decided as an id that names nothing: {carol_decides}"
    );
    assert!(!carol_unknown["error"].is_null(), "{carol_unknown}");
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
/// commits: without `gating.decide` it is `access_denied`, and once the
/// person may no longer view the asking member the set is decided as an id
/// that names nothing.
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
        // What the same person is answered for an id that names nothing.
        let unknown = format!("decision:{}", "0".repeat(64));
        let unknown_answer = console.answer(Some(&carol()), &unknown).await;
        console.stop().await;
        reached.expect("the decide reached its hand-off");
        replaced.expect("the configuration is replaced");
        refusals.push((
            removed,
            answer.expect("answered").expect("the decide task"),
            phase,
            refusal_shape(&unknown_answer, &unknown),
            pending_id,
        ));
    }

    // Every case is judged, and each one that is not refused as it should
    // be is named: without `gating.decide` the refusal names it; a set whose
    // asking member the person may no longer view is decided as an id that
    // names nothing.
    let mut honored = Vec::new();
    for (removed, answer, phase, unknown, pending_id) in refusals {
        let refused = match removed {
            "carol-gating" => {
                refusal_kind(&answer) == &json!("access_denied")
                    && answer["error"]["message"] == json!("access denied to gating decisions")
            }
            _ => !answer["error"].is_null() && refusal_shape(&answer, &pending_id) == unknown,
        };
        if !refused || phase != DecisionPhase::Open {
            honored.push(format!("{removed} withdrawn: {phase:?} {answer}"));
        }
    }
    assert!(
        honored.is_empty(),
        "a grant withdrawn while the answer waits is honored: {honored:#?}"
    );
}

/// The asking member is judged again at the moment the answer commits, by
/// the rule an approval's decide asks of its origin when it is applied. A
/// person starts an answer to a set of a member the Console knows; while
/// the answer waits, the Console stops knowing that member. The answer is
/// given as for an id that names nothing, and the set stays open.
#[tokio::test]
async fn a_set_whose_asker_the_console_stops_knowing_while_its_answer_waits_is_not_pending() {
    let state_dir = tempfile::tempdir().expect("state directory");
    let setup = Setup::enforced();
    let access = setup.access.clone().expect("access controller");
    let console = QuestionConsole::start(
        "questions-asker-unknown-while-waiting",
        setup,
        decisions_in(
            state_dir.path(),
            "questions-asker-unknown-while-waiting",
            true,
        ),
    )
    .await;
    let pending_id = console.asked(ASKER, "call-1").await;
    let listed_before = console.listed(Some(&bob())).await;
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
    // The roster the Console reads next no longer holds the asking member.
    access.replace_agent_attributes([crate::access::AgentResourceAttributes {
        identity: OTHER.to_string(),
        agent_id: None,
        role: Some("worker".to_string()),
        labels: BTreeMap::new(),
    }]);
    hold.release();
    let answer = tokio::time::timeout(Duration::from_secs(30), decide).await;
    let row = console.row(&pending_id);
    // What the same person is answered for an id that names nothing.
    let unknown = format!("decision:{}", "0".repeat(64));
    let unknown_answer = console.answer(Some(&bob()), &unknown).await;
    console.stop().await;

    assert_eq!(
        listed_before,
        vec![pending_id.clone()],
        "the person was listed the set when the answer started"
    );
    reached.expect("the decide reached its hand-off");
    let answer = answer
        .expect("the decide answered")
        .expect("the decide task");
    assert!(!unknown_answer["error"].is_null(), "{unknown_answer}");
    assert_eq!(
        refusal_shape(&answer, &pending_id),
        refusal_shape(&unknown_answer, &unknown),
        "the answer is given as for an id that names nothing: {answer}"
    );
    assert_eq!(row.state.phase, DecisionPhase::Open);
    assert!(row.outcome.is_none());
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
    let unknown = format!("decision:{}", "0".repeat(64));
    let others_unknown = console.answer(Some(&carol()), &unknown).await;
    let phase_after_other = console.row(&pending_id).state.phase;
    let owners_list = console.listed(Some(&bob())).await;
    let owners_answer = console.answer(Some(&bob()), &pending_id).await;
    console.stop().await;

    assert_eq!(others_list, Vec::<String>::new());
    assert_eq!(
        refusal_shape(&others_answer, &pending_id),
        refusal_shape(&others_unknown, &unknown),
        "to others the set is decided as an id that names nothing: {others_answer}"
    );
    assert!(!others_unknown["error"].is_null(), "{others_unknown}");
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
    let requests = [
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
        // A request at fault is plain invalid params; the decision's own
        // refusal (another digest than the set's) is typed.
        let data = match label {
            "wrong digest" => refused("subject_changed"),
            _ => Value::Null,
        };
        assert_eq!(answer["error"]["data"], data, "{label}: {answer}");
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
    assert_eq!(again.data, Some(refused("not_open")), "{again:?}");
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
    served.install(Arc::new(NoSends {
        a_person_decides: true,
    }));
    let undecided = ConsoleScheduleDoorSlot::default();
    undecided.install(Arc::new(NoSends {
        a_person_decides: false,
    }));
    let available = decisions_in(state_dir.path(), name, true);
    let surfaces: [(&str, Option<AskUserSurface>, &str, &str); 7] = [
        ("unfilled", None, name, ASKER),
        (
            "a Console nobody can decide on",
            Some(AskUserSurface::new(
                available.clone(),
                undecided.served(),
                name,
            )),
            name,
            ASKER,
        ),
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
    served.install(Arc::new(NoSends {
        a_person_decides: true,
    }));
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
        Duration::from_hours(1),
        Arc::default(),
    );
    let none_candidate = spawn_ask_user_driver(
        &decisions_in(state_dir.path(), "questions-lazy", false),
        Arc::clone(&told) as Arc<dyn AskUserNotices>,
        Duration::from_hours(1),
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
    // Alone on a build without a builtin surface, the tool takes the place
    // of the factory's empty base and reads as that base reads
    // (`the_mount_leaves_the_builds_catalog_as_it_was`).
    assert_eq!(
        mounted.tool_catalog_capabilities().exact_catalog,
        meerkat_core::AgentToolDispatcher::tool_catalog_capabilities(
            &meerkat_tools::EmptyToolDispatcher
        )
        .exact_catalog
    );
}

/// A dispatcher with a declared catalog. `deferred` makes its tools
/// deferred-eligible session entries, as a surface the factory composes
/// beside a build's own tools declares them (the WorkGraph surface, for
/// one); `pending` names sources it is still connecting.
struct Declared {
    tools: Arc<[Arc<meerkat_core::types::ToolDef>]>,
    exact: bool,
    deferred: bool,
    pending: Vec<String>,
}

impl Declared {
    fn of(
        names: &[&str],
        exact: bool,
        deferred: bool,
        pending: &[&str],
    ) -> Arc<dyn meerkat_core::AgentToolDispatcher> {
        Arc::new(Self {
            tools: names
                .iter()
                .map(|name| {
                    Arc::new(meerkat_core::types::ToolDef::new(
                        *name,
                        "a declared tool with a description long enough to count",
                        json!({"type": "object", "properties": {"value": {"type": "string"}}}),
                    ))
                })
                .collect(),
            exact,
            deferred,
            pending: pending.iter().map(ToString::to_string).collect(),
        })
    }
}

#[async_trait::async_trait]
impl meerkat_core::AgentToolDispatcher for Declared {
    fn tools(&self) -> Arc<[Arc<meerkat_core::types::ToolDef>]> {
        Arc::clone(&self.tools)
    }

    async fn dispatch(
        &self,
        call: meerkat_core::types::ToolCallView<'_>,
    ) -> Result<meerkat_core::ToolDispatchOutcome, meerkat_core::ToolError> {
        Err(meerkat_core::ToolError::not_found(call.name))
    }

    fn tool_catalog_capabilities(&self) -> meerkat_core::ToolCatalogCapabilities {
        meerkat_core::ToolCatalogCapabilities {
            exact_catalog: self.exact,
            may_require_catalog_control_plane: self.deferred,
        }
    }

    fn tool_catalog(&self) -> Arc<[meerkat_core::ToolCatalogEntry]> {
        self.tools
            .iter()
            .map(|tool| {
                if self.deferred {
                    meerkat_core::ToolCatalogEntry::session_deferred(
                        Arc::clone(tool),
                        true,
                        meerkat_core::ToolProvenance {
                            kind: meerkat_core::ToolSourceKind::Callback,
                            source_id: "declared".into(),
                            artifact: None,
                        },
                    )
                } else {
                    meerkat_core::ToolCatalogEntry::session_inline(Arc::clone(tool), true)
                }
            })
            .collect::<Vec<_>>()
            .into()
    }

    fn pending_catalog_sources(&self) -> Arc<[String]> {
        self.pending.clone().into()
    }
}

/// What Core judges of a composed tool surface: whether its catalog is
/// exact and may need the catalog's control plane, the catalog mode, whether
/// the control plane is composed, the sources still connecting, and the
/// tool names.
fn judged(tools: &dyn meerkat_core::AgentToolDispatcher) -> (String, Vec<String>) {
    let capabilities = tools.tool_catalog_capabilities();
    let mut names: Vec<String> = tools
        .tools()
        .iter()
        .map(|tool| tool.name.to_string())
        .collect();
    names.sort();
    (
        format!(
            "exact={} may_require_control_plane={} mode={:?} control_plane_composed={} pending={:?}",
            capabilities.exact_catalog,
            capabilities.may_require_catalog_control_plane,
            meerkat_core::select_tool_catalog_mode(tools),
            meerkat_core::should_compose_tool_catalog_control_plane(tools),
            tools.pending_catalog_sources(),
        ),
        names,
    )
}

/// Mounting the tool changes nothing about a member's tool catalog but the
/// one tool: the catalog reads exact or not, inline or deferred, with or
/// without the catalog's control tools and with the same sources still
/// connecting, as the same build without the tool, and the tool list is the
/// same list plus `ask_user`. Judged as Core judges a build: the build's
/// base tools composed with a surface beside them that declares two
/// deferred-eligible tools. Without external tools the factory's base is
/// its builtin composite when the build composes a builtin surface (exact
/// when its external tools are, or when it has none) and its empty
/// dispatcher otherwise (not exact).
#[test]
fn the_mount_leaves_the_builds_catalog_as_it_was() {
    let slot = AskUserSlot::default();
    let beside = || Declared::of(&["beside_one", "beside_two"], true, true, &[]);
    let mut differing = Vec::new();
    for (case, external, builtins) in [
        ("no external tools, no builtin surface", None, false),
        ("no external tools, builtins on", None, true),
        (
            "exact external tools",
            Some(Declared::of(&["probe"], true, false, &[])),
            false,
        ),
        (
            "external tools that are not exact",
            Some(Named::of(&["probe"])),
            false,
        ),
        (
            "exact external tools with deferred entries, builtins on",
            Some(Declared::of(&["later_one", "later_two"], true, true, &[])),
            true,
        ),
        (
            "exact external tools still connecting",
            Some(Declared::of(&["probe"], true, false, &["server-a"])),
            false,
        ),
    ] {
        let mut request = build_request(Some("agent-a"), external.clone());
        if builtins {
            request
                .build
                .as_mut()
                .expect("build options")
                .override_builtins = meerkat_core::ToolCategoryOverride::Enable;
        }
        slot.mount(&mut request);
        let mounted = request
            .build
            .as_ref()
            .and_then(|build| build.external_tools.clone())
            .expect("mounted");
        // The base the factory builds without the tool. A builtin composite
        // reads as its external tools do, and as exact without any: an
        // exact dispatcher of no tools stands in for it here.
        let base_without: Arc<dyn meerkat_core::AgentToolDispatcher> = match (&external, builtins) {
            (Some(external), _) => Arc::clone(external),
            (None, true) => Declared::of(&[], true, false, &[]),
            (None, false) => Arc::new(meerkat_tools::EmptyToolDispatcher),
        };
        let (judgement_without, mut names_without) =
            judged(&meerkat_core::DynamicToolComposite::new(vec![
                base_without,
                beside(),
            ]));
        let (judgement_with, names_with) = judged(&meerkat_core::DynamicToolComposite::new(vec![
            mounted,
            beside(),
        ]));
        names_without.push(ASK_USER_TOOL.to_string());
        names_without.sort();
        if judgement_with != judgement_without || names_with != names_without {
            differing.push(format!(
                "{case}: with the tool {judgement_with} {names_with:?}; without it \
                 {judgement_without} {names_without:?} (the tool added)"
            ));
        }
    }
    assert!(
        differing.is_empty(),
        "mounting the tool leaves the build's catalog as it was:\n{}",
        differing.join("\n")
    );
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

/// The typed `data` of a decision's own refusal, as the decision methods
/// give it: the kind `decision_refused` and the refusal, itself tagged by
/// its kind.
fn refused(refusal: &str) -> Value {
    json!({ "kind": "decision_refused", "decision_refusal": { "kind": refusal } })
}

/// A decision's own refusal of an answer is typed, in the form the decision
/// methods give it (`decision_refused` with the refusal), on the Console
/// and on stdio alike: a set that ended and a set past its deadline (both
/// `not_open`: the read that finds a set past its deadline records its
/// expiry) and another digest than the set's (`subject_changed`).
#[tokio::test]
async fn a_refusal_of_the_decision_is_typed() {
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
        decisions_in(state_dir.path(), "questions-typed", true).with_owner_for_test(owner);
    let console =
        QuestionConsole::start("questions-typed", Setup::enforced(), decisions.clone()).await;
    console.runtime.ask_user_slot().fill(
        AskUserSurface::new(decisions, console.doors.served(), "questions-typed")
            .with_clock(clock.clone()),
    );
    let ended = console.asked(ASKER, "ended").await;
    let open = console.asked(ASKER, "open").await;
    let late = ask(&console.host(ASKER), "late", 60)
        .await
        .request_id
        .expect("asked");
    let dismissal = console
        .decide(
            Some(&bob()),
            json!({
                "pending_id": ended,
                "decision": "reject",
                "presented_digest": console.digest(&ended),
            }),
        )
        .await;
    clock.0.store(now_ms + 61_000, Ordering::SeqCst);
    let another = Sha256Digest::of(b"another set").to_string();
    // A decide the set refuses records nothing: no decide event is bound
    // for a set that ended, one past its deadline, or another digest.
    let recorded_before = other_files(state_dir.path());
    let cases = [
        (
            "a set that ended",
            ended.clone(),
            console.digest(&ended),
            "not_open",
        ),
        (
            "a set past its deadline",
            late.clone(),
            console.digest(&late),
            "not_open",
        ),
        ("another digest", open.clone(), another, "subject_changed"),
    ];
    let mut answers = Vec::new();
    for (label, pending_id, digest, refusal) in cases {
        let params = json!({
            "pending_id": pending_id,
            "decision": "approve",
            "presented_digest": digest,
            "answers": [choice("q1", "a")],
        });
        let on_console = console.decide(Some(&bob()), params.clone()).await;
        let mut stdio_params = params;
        stdio_params["approver_id"] = json!("person:dana");
        let on_stdio = crate::rpc::question_set_methods::gating_decide_question_set(
            &console.runtime,
            &stdio_params,
            json!(1),
        );
        answers.push((label, refusal, on_console, on_stdio.error));
    }
    let recorded_after = other_files(state_dir.path());
    let open_phase = console.row(&open).state.phase;
    console.stop().await;

    assert!(
        recorded_before == recorded_after,
        "a decide the set refuses records nothing outside the decision store"
    );
    assert_eq!(
        dismissal["result"]["outcome"],
        json!("dismissed"),
        "{dismissal}"
    );
    // Every case is judged, and each one that is not typed is named.
    let mut untyped = Vec::new();
    for (label, refusal, on_console, on_stdio) in answers {
        if on_console["error"]["code"] != json!(-32602)
            || on_console["error"]["data"] != refused(refusal)
        {
            untyped.push(format!("{label} on the Console: {on_console}"));
        }
        let stdio = on_stdio
            .as_ref()
            .map(|error| (error.code, error.data.clone()));
        if stdio != Some((-32602, Some(refused(refusal)))) {
            untyped.push(format!("{label} on stdio: {on_stdio:?}"));
        }
    }
    assert!(
        untyped.is_empty(),
        "a decision's refusal is typed: {untyped:#?}"
    );
    assert_eq!(open_phase, DecisionPhase::Open);
}

/// A person who administers decisions and is no access admin reads nothing
/// of the question set of a member private to someone else: it is not in
/// their list, and its id reads as a decision that is not there. The set
/// of a member they may see they read, and an access admin reads both.
#[tokio::test]
async fn a_decisions_administrator_reads_no_set_of_a_member_hidden_from_them() {
    let state_dir = tempfile::tempdir().expect("state directory");
    let setup = Setup::enforced().access(crate::access::AccessControlConfig {
        enabled: true,
        admins: vec![DAVE.to_string()],
        rules: vec![
            rule(
                "carol-administers-decisions",
                CAROL,
                &[
                    crate::access::ACTION_DECISIONS_VIEW,
                    crate::access::ACTION_DECISIONS_ADMIN,
                    ACTION_AGENT_VIEW,
                ],
                &[],
            ),
            rule(
                "bob-answers",
                BOB,
                &[ACTION_GATING_VIEW, ACTION_GATING_DECIDE, ACTION_AGENT_VIEW],
                &[],
            ),
        ],
        ..crate::access::AccessControlConfig::default()
    });
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
    let console = QuestionConsole::start(
        "questions-decisions-admin",
        setup,
        decisions_in(state_dir.path(), "questions-decisions-admin", true),
    )
    .await;
    let private_set = console.asked("agent-p", "call-1").await;
    let shared_set = console.asked(ASKER, "call-2").await;
    let unknown = format!("decision:{}", "0".repeat(64));
    let dave = token(
        json!({ "sub": "oidc-sub-dave", "email": DAVE, "provider": "google_oauth" }),
        Some(now_seconds() + 3600),
    );
    let read = |bearer: String, method: &'static str, decision_id: String| {
        let app = console.app.clone();
        async move {
            let params = if method == "meta/decisions/list" {
                json!({})
            } else {
                json!({ "decision_id": decision_id })
            };
            console_rpc_as(&app, Some(&bearer), method, params).await
        }
    };
    let carol_list = read(carol(), "meta/decisions/list", String::new()).await;
    let carol_get = read(carol(), "meta/decisions/get", private_set.clone()).await;
    let carol_rendering = read(carol(), "meta/decisions/rendering", private_set.clone()).await;
    let carol_unknown = read(carol(), "meta/decisions/get", unknown.clone()).await;
    let carol_shared = read(carol(), "meta/decisions/get", shared_set.clone()).await;
    let dave_list = read(dave.clone(), "meta/decisions/list", String::new()).await;
    let dave_get = read(dave, "meta/decisions/get", private_set.clone()).await;
    // The owner still reads and answers its member's set on the gating route.
    let owner_listed = console.listed(Some(&bob())).await;
    console.stop().await;

    let carol_list = carol_list.to_string();
    assert!(
        carol_list.contains(&shared_set),
        "a set of a member they may see is listed: {carol_list}"
    );
    // Every read is judged, and each one that shows the set is named.
    let mut shown = Vec::new();
    if carol_list.contains(&private_set) {
        shown.push(format!(
            "the set of a member private to someone else is not listed: {carol_list}"
        ));
    }
    if refusal_shape(&carol_get, &private_set) != refusal_shape(&carol_unknown, &unknown) {
        shown.push(format!(
            "it reads as a decision that is not there: {carol_get}"
        ));
    }
    if refusal_shape(&carol_rendering, &private_set) != refusal_shape(&carol_unknown, &unknown) {
        shown.push(format!(
            "its rendering reads as one that is not there: {carol_rendering}"
        ));
    }
    assert!(shown.is_empty(), "nothing of the set is read: {shown:#?}");
    assert!(carol_shared["error"].is_null(), "{carol_shared}");
    assert!(carol_shared.to_string().contains(&shared_set));
    let dave_list = dave_list.to_string();
    assert!(dave_list.contains(&private_set), "{dave_list}");
    assert!(dave_get["error"].is_null(), "{dave_get}");
    assert!(dave_get.to_string().contains(&private_set));
    assert!(owner_listed.contains(&private_set), "{owner_listed:?}");
}

/// A caller the read does not list a set to cannot decide it either: the
/// decide checks the asking member by the read's own rule. Here the Console
/// does not know the asking member (it may have been private to someone),
/// so a person who is no access admin is neither listed the set nor may
/// answer it by its id; an access admin is listed it and answers it.
#[tokio::test]
async fn a_caller_the_read_hides_a_set_from_cannot_decide_it() {
    let state_dir = tempfile::tempdir().expect("state directory");
    let setup = Setup::enforced().access(crate::access::AccessControlConfig {
        enabled: true,
        admins: vec![DAVE.to_string()],
        rules: vec![rule(
            "bob-answers",
            BOB,
            &[ACTION_GATING_VIEW, ACTION_GATING_DECIDE, ACTION_AGENT_VIEW],
            &[],
        )],
        ..crate::access::AccessControlConfig::default()
    });
    let console = QuestionConsole::start(
        "questions-unknown-member",
        setup,
        decisions_in(state_dir.path(), "questions-unknown-member", true),
    )
    .await;
    // A member the Console's access cache does not know.
    let pending_id = console.asked("agent-u", "call-1").await;
    let listed = console.listed(Some(&bob())).await;
    let decided = console.answer(Some(&bob()), &pending_id).await;
    let unknown = format!("decision:{}", "0".repeat(64));
    let decided_unknown = console.answer(Some(&bob()), &unknown).await;
    let phase = console.row(&pending_id).state.phase;
    let dave = token(
        json!({ "sub": "oidc-sub-dave", "email": DAVE, "provider": "google_oauth" }),
        Some(now_seconds() + 3600),
    );
    let admin_listed = console.listed(Some(&dave)).await;
    let admin_decided = console.answer(Some(&dave), &pending_id).await;
    console.stop().await;

    assert_eq!(
        listed,
        Vec::<String>::new(),
        "the read lists no set of a member the Console does not know"
    );
    assert_eq!(
        refusal_shape(&decided, &pending_id),
        refusal_shape(&decided_unknown, &unknown),
        "a caller the read hides the set from cannot decide it: {decided}"
    );
    assert!(!decided_unknown["error"].is_null(), "{decided_unknown}");
    assert_eq!(phase, DecisionPhase::Open);
    assert_eq!(admin_listed, vec![pending_id]);
    assert_eq!(
        admin_decided["result"]["outcome"],
        json!("answered"),
        "{admin_decided}"
    );
}

/// A person who administers decisions and is no access admin cannot
/// withdraw the question set of a member private to someone else: its id is
/// answered as an id that names no decision, and the set stays open. They
/// withdraw the set of a member they may see; the member's owner and an
/// access admin, who administer decisions too, withdraw the member's sets.
#[tokio::test]
async fn a_decisions_administrator_cannot_withdraw_a_set_of_a_member_hidden_from_them() {
    let state_dir = tempfile::tempdir().expect("state directory");
    let administers = [
        crate::access::ACTION_DECISIONS_VIEW,
        crate::access::ACTION_DECISIONS_ADMIN,
        ACTION_AGENT_VIEW,
    ];
    let setup = Setup::enforced().access(crate::access::AccessControlConfig {
        enabled: true,
        admins: vec![DAVE.to_string()],
        rules: vec![
            rule("carol-administers-decisions", CAROL, &administers, &[]),
            rule("bob-administers-decisions", BOB, &administers, &[]),
        ],
        ..crate::access::AccessControlConfig::default()
    });
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
    let console = QuestionConsole::start(
        "questions-decisions-withdraw",
        setup,
        decisions_in(state_dir.path(), "questions-decisions-withdraw", true),
    )
    .await;
    let private_set = console.asked("agent-p", "call-1").await;
    let second_private_set = console.asked("agent-p", "call-2").await;
    let shared_set = console.asked(ASKER, "call-3").await;
    let unknown = format!("decision:{}", "0".repeat(64));
    let dave = token(
        json!({ "sub": "oidc-sub-dave", "email": DAVE, "provider": "google_oauth" }),
        Some(now_seconds() + 3600),
    );
    let withdraw = |bearer: String, decision_id: String| {
        let app = console.app.clone();
        async move {
            console_rpc_as(
                &app,
                Some(&bearer),
                "meta/decisions/withdraw",
                json!({ "decision_id": decision_id }),
            )
            .await
        }
    };
    let carol_hidden = withdraw(carol(), private_set.clone()).await;
    let carol_unknown = withdraw(carol(), unknown.clone()).await;
    let phase_after_carol = console.row(&private_set).state.phase;
    let carol_shared = withdraw(carol(), shared_set.clone()).await;
    let owner_withdraws = withdraw(bob(), private_set.clone()).await;
    let admin_withdraws = withdraw(dave, second_private_set.clone()).await;
    let phases = [
        console.row(&shared_set).state.phase,
        console.row(&private_set).state.phase,
        console.row(&second_private_set).state.phase,
    ];
    console.stop().await;

    assert_eq!(
        refusal_shape(&carol_hidden, &private_set),
        refusal_shape(&carol_unknown, &unknown),
        "a hidden set is not there to withdraw: {carol_hidden}"
    );
    assert!(!carol_unknown["error"].is_null(), "{carol_unknown}");
    assert_eq!(
        phase_after_carol,
        DecisionPhase::Open,
        "the hidden set stays open"
    );
    assert!(carol_shared["error"].is_null(), "{carol_shared}");
    assert!(owner_withdraws["error"].is_null(), "{owner_withdraws}");
    assert!(admin_withdraws["error"].is_null(), "{admin_withdraws}");
    assert_eq!(phases, [DecisionPhase::Withdrawn; 3]);
}

/// The withdraw's look at what its caller may see records nothing: an
/// administrator's withdraw of a set past its deadline is still answered by
/// the touch that records the expiry (`expired`), as a withdraw was before
/// it looked.
#[tokio::test]
async fn a_withdraw_past_the_deadline_still_answers_the_expiry_it_records() {
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
        decisions_in(state_dir.path(), "questions-late-withdraw", true).with_owner_for_test(owner);
    let console = QuestionConsole::start(
        "questions-late-withdraw",
        Setup::enforced().access(crate::access::AccessControlConfig {
            enabled: true,
            admins: vec![BOB.to_string()],
            ..crate::access::AccessControlConfig::default()
        }),
        decisions.clone(),
    )
    .await;
    console.runtime.ask_user_slot().fill(
        AskUserSurface::new(decisions, console.doors.served(), "questions-late-withdraw")
            .with_clock(clock.clone()),
    );
    let late = ask(&console.host(ASKER), "late", 60)
        .await
        .request_id
        .expect("asked");
    clock.0.store(now_ms + 61_000, Ordering::SeqCst);
    let withdrawal = console_rpc_as(
        &console.app,
        Some(&bob()),
        "meta/decisions/withdraw",
        json!({ "decision_id": late }),
    )
    .await;
    let phase = console.row(&late).state.phase;
    console.stop().await;

    assert_eq!(
        withdrawal["error"]["data"],
        refused("expired"),
        "a withdraw past the deadline answers the expiry it records: {withdrawal}"
    );
    assert_eq!(phase, DecisionPhase::Expired);
}

/// One reader's walk over `meta/decisions/list` under `filter`, a row a
/// page: every page read.
async fn decision_pages(console: &QuestionConsole, bearer: &str, filter: &Value) -> Vec<Value> {
    let mut pages = Vec::new();
    let mut after: Option<String> = None;
    for _ in 0..8 {
        let mut params = filter.clone();
        params["limit"] = json!(1);
        if let Some(after) = &after {
            params["after"] = json!(after);
        }
        let page = console_rpc_as(&console.app, Some(bearer), "meta/decisions/list", params).await;
        after = page["result"]["next_after"].as_str().map(str::to_string);
        pages.push(page);
        if after.is_none() {
            break;
        }
    }
    pages
}

/// What a walk tells of the sets `hidden`: those its pages name anywhere,
/// and how many of its pages carry a cursor and no row.
fn told_of(pages: &[Value], hidden: &[String]) -> (Vec<String>, usize) {
    let read = serde_json::to_string(pages).expect("pages");
    let named = hidden
        .iter()
        .filter(|set| read.contains(set.as_str()))
        .cloned()
        .collect();
    let rows = |page: &Value, field: &str| page["result"][field].as_array().map_or(0, Vec::len);
    let empty_with_cursor = pages
        .iter()
        .filter(|page| {
            rows(page, "decisions") + rows(page, "statuses") == 0
                && page["result"]["next_after"].is_string()
        })
        .count();
    (named, empty_with_cursor)
}

/// The decision list pages over what its reader may see: read a row a page,
/// by kind or by the asking member, it names no question set of a member
/// hidden from the reader, in a row or in a cursor, and gives no page that
/// carries a cursor and no row. So for a person who administers decisions,
/// and for one who holds the approver id of question sets; an access admin
/// reads every set.
#[tokio::test]
async fn the_decision_list_pages_tell_nothing_of_a_set_hidden_from_their_reader() {
    let state_dir = tempfile::tempdir().expect("state directory");
    let config: crate::access::AccessControlConfig = serde_json::from_value(json!({
        "enabled": true,
        "admins": [DAVE],
        "rules": [
            {
                "id": "carol-administers-decisions",
                "subjects": [CAROL],
                "actions": [
                    crate::access::ACTION_DECISIONS_VIEW,
                    crate::access::ACTION_DECISIONS_ADMIN,
                    ACTION_AGENT_VIEW,
                ],
            },
            {
                "id": "bob-reads-decisions",
                "subjects": [BOB],
                "actions": [crate::access::ACTION_DECISIONS_VIEW, ACTION_AGENT_VIEW],
            },
        ],
        "approvers": { "ask_user": { "subjects": [BOB] } },
    }))
    .expect("access configuration");
    let setup = Setup::enforced().access(config);
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
                person_private_owner_key(ISSUER, "oidc-sub-someone-else"),
            )]),
        });
    let console = QuestionConsole::start(
        "questions-decision-pages",
        setup,
        decisions_in(state_dir.path(), "questions-decision-pages", true),
    )
    .await;
    let hidden = [
        console.asked("agent-p", "call-1").await,
        console.asked("agent-p", "call-2").await,
    ];
    let shared_set = console.asked(ASKER, "call-3").await;
    let by_kind = json!({ "kind": ASK_USER_QUESTIONS_KIND });
    let by_hidden_asker =
        json!({ "requester": console.row(&hidden[0]).descriptor.requester.as_str() });
    let mut told = Vec::new();
    for (reader, bearer) in [
        ("a person who administers decisions", carol()),
        ("a holder of the approver id", bob()),
    ] {
        for (walk, filter) in [
            ("by kind", &by_kind),
            ("by the hidden asker", &by_hidden_asker),
        ] {
            let pages = decision_pages(&console, &bearer, filter).await;
            let (named, empty_with_cursor) = told_of(&pages, &hidden);
            if !named.is_empty() || empty_with_cursor != 0 {
                told.push(format!(
                    "{reader}, {walk}: names {named:?}; {empty_with_cursor} pages with a \
                     cursor and no row"
                ));
            }
            let read = serde_json::to_string(&pages).expect("pages");
            if walk == "by kind" && !read.contains(&shared_set) {
                told.push(format!(
                    "{reader}, {walk}: the set they may see is not read: {read}"
                ));
            }
        }
    }
    let dave = token(
        json!({ "sub": "oidc-sub-dave", "email": DAVE, "provider": "google_oauth" }),
        Some(now_seconds() + 3600),
    );
    let admin_pages = decision_pages(&console, &dave, &by_kind).await;
    console.stop().await;

    assert!(
        told.is_empty(),
        "the pages tell nothing of a hidden set: {told:#?}"
    );
    let (admin_named, admin_empty) = told_of(&admin_pages, &hidden);
    assert_eq!(admin_named, hidden, "an access admin reads every set");
    assert_eq!(admin_empty, 0, "{admin_pages:?}");
    assert_eq!(admin_pages.len(), 3, "{admin_pages:?}");
}

/// A question set whose body this version cannot read has no asker to
/// judge. It is shown where nobody is kept apart from other people (an
/// access admin reads it) and to nobody who is: a person who administers
/// decisions reads it as a decision that is not there. No read lists it as
/// a set to answer.
#[tokio::test]
async fn a_set_with_a_body_that_cannot_be_read_is_shown_only_where_nobody_is_kept_apart() {
    let state_dir = tempfile::tempdir().expect("state directory");
    let setup = Setup::enforced().access(crate::access::AccessControlConfig {
        enabled: true,
        admins: vec![DAVE.to_string()],
        rules: vec![rule(
            "carol-administers-decisions",
            CAROL,
            &[
                crate::access::ACTION_DECISIONS_VIEW,
                crate::access::ACTION_DECISIONS_ADMIN,
                ACTION_AGENT_VIEW,
                ACTION_GATING_VIEW,
            ],
            &[],
        )],
        ..crate::access::AccessControlConfig::default()
    });
    let console = QuestionConsole::start(
        "questions-unread-body",
        setup,
        decisions_in(state_dir.path(), "questions-unread-body", true),
    )
    .await;
    let odd = held_of_kind(&console.decisions, ASK_USER_QUESTIONS_KIND)
        .decision_id
        .to_string();
    let unknown = format!("decision:{}", "0".repeat(64));
    let dave = token(
        json!({ "sub": "oidc-sub-dave", "email": DAVE, "provider": "google_oauth" }),
        Some(now_seconds() + 3600),
    );
    let read = |bearer: String, method: &'static str, decision_id: String| {
        let app = console.app.clone();
        async move {
            let params = if method == "meta/decisions/list" {
                json!({})
            } else {
                json!({ "decision_id": decision_id })
            };
            console_rpc_as(&app, Some(&bearer), method, params).await
        }
    };
    let carol_list = read(carol(), "meta/decisions/list", String::new()).await;
    let carol_get = read(carol(), "meta/decisions/get", odd.clone()).await;
    let carol_unknown = read(carol(), "meta/decisions/get", unknown.clone()).await;
    let dave_list = read(dave.clone(), "meta/decisions/list", String::new()).await;
    let dave_get = read(dave.clone(), "meta/decisions/get", odd.clone()).await;
    let to_answer = console.listed(Some(&dave)).await;
    console.stop().await;

    assert!(
        !carol_list.to_string().contains(&odd),
        "a person kept apart is not listed it: {carol_list}"
    );
    assert_eq!(
        refusal_shape(&carol_get, &odd),
        refusal_shape(&carol_unknown, &unknown),
        "to a person kept apart it is a decision that is not there: {carol_get}"
    );
    assert!(
        dave_list.to_string().contains(&odd),
        "an access admin is listed it: {dave_list}"
    );
    assert!(
        dave_get["error"].is_null(),
        "an access admin reads it: {dave_get}"
    );
    assert_eq!(to_answer, Vec::<String>::new(), "it is no set to answer");
}

/// The notices of ended sets, as the runtime sends them: one that cannot
/// be sent now (no identity runtime is installed) is asked again at a later
/// pass, and one whose asker is no agent name ends failed for good.
#[tokio::test]
async fn a_notice_that_cannot_be_sent_now_is_asked_again_and_one_for_no_agent_ends() {
    let state_dir = tempfile::tempdir().expect("state directory");
    let console = QuestionConsole::start(
        "questions-notices",
        Setup::enforced(),
        decisions_in(state_dir.path(), "questions-notices", true),
    )
    .await;
    let pending_id = console.asked(ASKER, "call-1").await;
    let dismissal = console
        .decide(
            Some(&bob()),
            json!({
                "pending_id": pending_id,
                "decision": "reject",
                "presented_digest": console.digest(&pending_id),
            }),
        )
        .await;
    let told = Arc::new(Told::default());
    let driver = console.drive(&told);
    until("the asker is told", || told.all().len() == 1).await;
    let notice = told.all().remove(0);
    driver.shutdown().await;
    let notices = crate::ask_user::MemberKeyedNotices::new(console.runtime.clone());
    let asked_again = notices.deliver(&notice).await;
    let mut for_no_agent = notice.clone();
    for_no_agent.asker = "not an agent name".to_string();
    let ended = notices.deliver(&for_no_agent).await;
    console.stop().await;

    assert_eq!(
        dismissal["result"]["outcome"],
        json!("dismissed"),
        "{dismissal}"
    );
    assert!(
        asked_again.is_none(),
        "a notice that cannot be sent now is asked again: {asked_again:?}"
    );
    assert!(
        matches!(
            ended,
            Some(meerkat_core::approval::ApprovalConsentDeliveryState::Failed { .. })
        ),
        "a notice for no agent ends failed: {ended:?}"
    );
}

/// A set its reader may not see, lying between two it may see, hides
/// neither: the list, read a row a page, gives both, names the hidden one
/// nowhere, and gives no page with a cursor and no row. Three members ask;
/// the member whose set has the middle id is then made private to someone
/// else, so the order is certain.
#[tokio::test]
async fn a_hidden_set_between_two_its_reader_may_see_hides_neither() {
    let state_dir = tempfile::tempdir().expect("state directory");
    let setup = Setup::enforced().access(crate::access::AccessControlConfig {
        enabled: true,
        admins: vec![DAVE.to_string()],
        rules: vec![rule(
            "carol-administers-decisions",
            CAROL,
            &[
                crate::access::ACTION_DECISIONS_VIEW,
                crate::access::ACTION_DECISIONS_ADMIN,
                ACTION_AGENT_VIEW,
            ],
            &[],
        )],
        ..crate::access::AccessControlConfig::default()
    });
    let access = setup.access.clone().expect("access controller");
    let members = [ASKER, OTHER, "agent-c"];
    let attributes = |member: &str, private: bool| crate::access::AgentResourceAttributes {
        identity: member.to_string(),
        agent_id: None,
        role: Some("worker".to_string()),
        labels: if private {
            BTreeMap::from([(
                crate::access::PRIVATE_TO_LABEL.to_string(),
                person_private_owner_key(ISSUER, "oidc-sub-someone-else"),
            )])
        } else {
            BTreeMap::new()
        },
    };
    access.record_agent_attributes(attributes("agent-c", false));
    let console = QuestionConsole::start(
        "questions-hidden-between",
        setup,
        decisions_in(state_dir.path(), "questions-hidden-between", true),
    )
    .await;
    let mut sets = Vec::new();
    for member in members {
        sets.push((console.asked(member, "call-1").await, member));
    }
    sets.sort();
    let (hidden, hidden_member) = sets[1].clone();
    access.record_agent_attributes(attributes(hidden_member, true));
    let pages = decision_pages(
        &console,
        &carol(),
        &json!({ "kind": ASK_USER_QUESTIONS_KIND }),
    )
    .await;
    console.stop().await;

    let read = serde_json::to_string(&pages).expect("pages");
    assert!(
        read.contains(&sets[0].0) && read.contains(&sets[2].0),
        "both sets the reader may see are read: {read}"
    );
    let (named, empty_with_cursor) = told_of(&pages, &[hidden]);
    assert_eq!(
        named,
        Vec::<String>::new(),
        "the set between them is named nowhere"
    );
    assert_eq!(empty_with_cursor, 0, "{read}");
}
