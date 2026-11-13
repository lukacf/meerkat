//! Rows of the `ask_user.questions` decision kind: what a hold seals, a
//! replay and a conflict, every end and the result an agent reads of it,
//! the one notice each end owes, a restart, and answers that are not valid
//! for their set.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use meerkat_core::approval::consent::{
    AuthenticatedApprover, ConsentEligibility, ConsentEligibilityAnswer, ConsentIneligible,
};
use meerkat_core::approval::decision::{
    ApproverId, AttestedDecision, AttributedReview, DecisionHoldRequest, DecisionId,
    DecisionIngress, DecisionKind, DecisionListFilter, DecisionNotice, DecisionOperationId,
    DecisionPhase, DecisionPrincipalKey, DecisionReference, DecisionRenderer, DecisionRendering,
    DecisionRequestBody, DecisionReviewVerdict, DecisionStore, DecisionSubjectKey,
    InMemoryDecisionStore, PresentationAudience, PresentationRecord, Sha256Digest, StoredDecision,
};
use meerkat_core::approval::{ApprovalAssurance, ApprovalConsentDeliveryState};
use meerkat_core::ask_user::{
    AskUserAnswer, AskUserAnswers, AskUserChoice, AskUserOutcome, AskUserQuestion,
    AskUserQuestionAnswer, AskUserQuestionSet, AskUserResult,
};
use meerkat_core::{PrincipalKind, PrincipalRef, RunId, SessionId, TrustDomainId};
use meerkat_store::SqliteDecisionStore;

use super::*;
use crate::decision::{
    DecisionClock, DecisionClockError, DecisionExecutorLoop, DecisionExecutorPass, DecisionRefusal,
    DecisionWithdrawer,
};

/// When the rows' calls are made.
const ASKED_AT: u64 = 1_800_000_000_000;
/// How long the rows' calls wait: ten minutes.
const TIMEOUT: Duration = Duration::from_mins(10);
/// The deadline a call made at [`ASKED_AT`] seals.
const DEADLINE: u64 = ASKED_AT + 600_000;
/// The asking agent's key, as its host names it.
const ASKER: &str = "member:planner";

struct ManualClock(AtomicU64);

impl ManualClock {
    fn at(now_ms: u64) -> Arc<Self> {
        Arc::new(Self(AtomicU64::new(now_ms)))
    }

    fn set(&self, now_ms: u64) {
        self.0.store(now_ms, Ordering::SeqCst);
    }
}

impl DecisionClock for ManualClock {
    fn now_ms(&self) -> Result<u64, DecisionClockError> {
        Ok(self.0.load(Ordering::SeqCst))
    }
}

/// An eligibility owner's evidence that the person may answer now.
struct MayAnswer;

impl ConsentEligibility for MayAnswer {
    fn check_current(&self) -> Result<(), ConsentIneligible> {
        Ok(())
    }
}

/// The notices a host was asked to deliver, each admitted.
#[derive(Default)]
struct RecordingNotices {
    delivered: Mutex<Vec<AskUserNotice>>,
}

impl RecordingNotices {
    fn delivered(&self) -> Vec<AskUserNotice> {
        self.delivered.lock().unwrap().clone()
    }
}

#[async_trait]
impl AskUserNotices for RecordingNotices {
    async fn deliver(&self, notice: &AskUserNotice) -> Option<ApprovalConsentDeliveryState> {
        self.delivered.lock().unwrap().push(notice.clone());
        Some(ApprovalConsentDeliveryState::Admitted {
            session_id: SessionId::new(),
            input_id: format!("ask-user:{}", notice.decision_id),
        })
    }
}

fn principal(kind: PrincipalKind, id: &str) -> PrincipalRef {
    PrincipalRef::in_domain(
        kind,
        id,
        TrustDomainId::new("example.org").expect("trust domain"),
    )
    .expect("qualified principal")
}

/// The principal the rows' holds name as their requester.
fn requester() -> DecisionPrincipalKey {
    DecisionPrincipalKey::of(&principal(PrincipalKind::SharedAgent, "planner")).expect("key")
}

fn reference(value: &str) -> DecisionReference {
    DecisionReference::new(value).expect("reference")
}

fn approver_id() -> ApproverId {
    ApproverId::new(ASK_USER_APPROVER_ID).expect("approver id")
}

fn question(id: &str, prompt: &str, choices: &[(&str, &str)]) -> AskUserQuestion {
    AskUserQuestion {
        id: id.to_owned(),
        prompt: prompt.to_owned(),
        choices: choices
            .iter()
            .map(|(id, label)| AskUserChoice {
                id: (*id).to_owned(),
                label: (*label).to_owned(),
            })
            .collect(),
        allow_free_text: false,
        recommended: None,
    }
}

/// The rows' question set: a channel question that accepts a written
/// answer and recommends a choice, and a yes or no question.
fn question_set() -> AskUserQuestionSet {
    let mut channel = question(
        "channel",
        "Which channel does this go to?",
        &[("stable", "Stable"), ("beta", "Beta")],
    );
    channel.allow_free_text = true;
    channel.recommended = Some("beta".to_owned());
    AskUserQuestionSet::new(
        Some("Before the release".to_owned()),
        vec![
            channel,
            question("notify", "Tell the team?", &[("yes", "Yes"), ("no", "No")]),
        ],
    )
    .expect("a valid set")
}

/// Another set, of another question.
fn other_set() -> AskUserQuestionSet {
    AskUserQuestionSet::new(
        None,
        vec![question(
            "budget",
            "Raise the budget?",
            &[("yes", "Yes"), ("no", "No")],
        )],
    )
    .expect("a valid set")
}

fn hold_of(questions: AskUserQuestionSet, run_id: &RunId, call_id: &str) -> AskUserHold {
    AskUserHold {
        asker: ASKER.to_owned(),
        requester: requester(),
        questions,
        timeout: TIMEOUT,
        run_id: run_id.clone(),
        call_id: call_id.to_owned(),
    }
}

fn choice(question_id: &str, choice_id: &str) -> AskUserQuestionAnswer {
    AskUserQuestionAnswer {
        question_id: question_id.to_owned(),
        answer: AskUserAnswer::Choice {
            choice_id: choice_id.to_owned(),
        },
    }
}

fn free_text(question_id: &str, text: &str) -> AskUserQuestionAnswer {
    AskUserQuestionAnswer {
        question_id: question_id.to_owned(),
        answer: AskUserAnswer::FreeText {
            text: text.to_owned(),
        },
    }
}

fn unanswered(question_id: &str) -> AskUserQuestionAnswer {
    AskUserQuestionAnswer {
        question_id: question_id.to_owned(),
        answer: AskUserAnswer::Unanswered,
    }
}

/// Every question of the rows' set, unanswered.
fn all_unanswered() -> Vec<AskUserQuestionAnswer> {
    vec![unanswered("channel"), unanswered("notify")]
}

/// A decision owner over `store`, reading `clock`.
fn service_over(store: Arc<dyn DecisionStore>, clock: &Arc<ManualClock>) -> DurableDecisionService {
    let clock: Arc<dyn DecisionClock> = clock.clone();
    DurableDecisionService::with_clock(store, clock)
}

/// A decision owner over a store of its own, at [`ASKED_AT`].
fn fixture() -> (Arc<ManualClock>, DurableDecisionService) {
    let clock = ManualClock::at(ASKED_AT);
    let service = service_over(Arc::new(InMemoryDecisionStore::default()), &clock);
    (clock, service)
}

/// Hold the rows' set for a call of a new run, at [`ASKED_AT`].
fn held(service: &DurableDecisionService) -> StoredDecision {
    hold_ask_user(
        service,
        ASKED_AT,
        hold_of(question_set(), &RunId::new(), "call-1"),
    )
    .expect("the hold")
}

/// Show `decision`'s whole rendering to the people who may answer it.
fn present(service: &DurableDecisionService, decision: &StoredDecision) {
    service
        .present(
            &decision.decision_id,
            &PresentationRecord::complete(
                PresentationAudience::Approver {
                    approver_id: approver_id(),
                },
                &decision.descriptor.rendering,
                None,
                reference("console"),
                reference("card-1"),
                ASKED_AT,
            ),
        )
        .expect("present");
}

/// A signed-in person's decision on `decision`'s rendering.
fn person_decides(decision: &StoredDecision, approve: bool) -> AttestedDecision {
    let approver = AuthenticatedApprover::new(
        principal(PrincipalKind::Human, "alice"),
        ApprovalAssurance::PersonAuthenticated,
    )
    .expect("an authenticated person");
    AttestedDecision::new(
        approver,
        DecisionIngress::new("console").expect("ingress"),
        reference("event-1"),
        ASKED_AT + 1,
        approver_id(),
        decision.descriptor.rendering.subject_digest(),
        approve,
    )
    .expect("an attested decision")
}

/// Present `decision` and have a person answer it with `answers`.
fn answered(
    service: &DurableDecisionService,
    decision: &StoredDecision,
    answers: Vec<AskUserQuestionAnswer>,
) -> StoredDecision {
    let body = AskUserBody::of(decision).expect("the held body");
    let answers = AskUserAnswers::for_set(&body.questions, answers).expect("valid answers");
    present(service, decision);
    let attested =
        attest_ask_user_answers(person_decides(decision, true), &answers).expect("attested");
    let row = service
        .decide(&decision.decision_id, &attested, &MayAnswer)
        .expect("the answer is recorded");
    assert_eq!(row.state.phase, DecisionPhase::Approved);
    row
}

/// The executor loop of the kind over `service`, delivering to `notices`.
fn executor_loop(
    service: &DurableDecisionService,
    notices: &Arc<RecordingNotices>,
) -> DecisionExecutorLoop {
    let notices: Arc<dyn AskUserNotices> = notices.clone();
    let executor: Arc<dyn DecisionExecutor> =
        Arc::new(AskUserExecutor::new(notices).expect("the executor"));
    DecisionExecutorLoop::start(
        service.clone(),
        [executor],
        Arc::new(AskUserApproverCurrent),
    )
    .expect("the loop starts")
}

/// What a pass did to each decision, in the order it did it.
fn outcomes_of(pass: &DecisionExecutorPass) -> BTreeMap<DecisionId, Vec<DecisionPassOutcome>> {
    let mut outcomes: BTreeMap<DecisionId, Vec<DecisionPassOutcome>> = BTreeMap::new();
    for (decision_id, outcome) in &pass.outcomes {
        outcomes
            .entry(decision_id.clone())
            .or_default()
            .push(outcome.clone());
    }
    outcomes
}

/// Run passes until one does nothing, and answer everything they did to
/// `decision_id`, in order.
async fn settle(
    executor: &mut DecisionExecutorLoop,
    decision_id: &DecisionId,
) -> Vec<DecisionPassOutcome> {
    let mut did = Vec::new();
    for _ in 0..8 {
        let pass = executor.pass().await;
        assert!(pass.unclaimed.is_empty(), "{:?}", pass.unclaimed);
        assert!(pass.read_failures.is_empty(), "{:?}", pass.read_failures);
        if pass.outcomes.is_empty() {
            return did;
        }
        did.extend(outcomes_of(&pass).remove(decision_id).unwrap_or_default());
    }
    panic!("the loop never came to rest: {did:?}");
}

fn refusal_of(error: &AskUserError) -> DecisionRefusal {
    match error {
        AskUserError::Decision(DecisionError::Refused { refusal, .. }) => *refusal,
        other => panic!("expected a refusal of the decision owner, got {other:?}"),
    }
}

#[test]
fn the_hold_seals_the_set_the_asker_and_the_deadline() {
    let (_clock, service) = fixture();
    let run_id = RunId::new();
    let decision = hold_ask_user(
        &service,
        ASKED_AT,
        hold_of(question_set(), &run_id, "call-1"),
    )
    .expect("the hold");

    let kind = ask_user_questions_kind().expect("kind");
    assert_eq!(kind.as_str(), "ask_user.questions");
    let subject_key =
        DecisionSubjectKey::new(format!("{ASKER}/{run_id}/call-1")).expect("subject key");
    assert_eq!(
        decision.decision_id,
        DecisionId::derive(&kind, &subject_key)
    );
    let descriptor = &decision.descriptor;
    assert_eq!(descriptor.kind, kind);
    assert_eq!(descriptor.subject_key, subject_key);
    assert_eq!(
        descriptor.operation_id.as_str(),
        format!("ask_user:{}", decision.decision_id)
    );
    assert_eq!(descriptor.requester, requester());
    assert_eq!(descriptor.approver_id.as_str(), "ask_user");
    assert_eq!(descriptor.reviewer, None, "answered by people only");
    assert_eq!(descriptor.renderer.as_str(), "ask_user.render-v1");
    assert_eq!(descriptor.deadline_at_ms, DEADLINE, "the call's timeout");
    assert_eq!(decision.state.phase, DecisionPhase::Open);
    assert_eq!(
        decision.state.notice,
        DecisionNotice::NotDue,
        "its end owes the asker a notice"
    );

    // The held request is the typed set and its asker, sealed by digest.
    let body = AskUserBody::of(&decision).expect("the held body");
    assert_eq!(
        body,
        AskUserBody {
            body: "ask_user.questions.v1".to_owned(),
            asker: ASKER.to_owned(),
            questions: question_set(),
        }
    );
    assert_eq!(
        descriptor.request_digest,
        Sha256Digest::of(descriptor.request.get().as_bytes())
    );
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(descriptor.request.get()).unwrap(),
        serde_json::json!({
            "body": "ask_user.questions.v1",
            "asker": ASKER,
            "questions": serde_json::to_value(question_set()).unwrap(),
        })
    );

    // What a person reads.
    assert_eq!(
        descriptor.rendering.as_str(),
        "Before the release\n\
         \n\
         1. Which channel does this go to?\n\
         \x20  - Stable\n\
         \x20  - Beta (recommended)\n\
         \x20  A written answer is accepted.\n\
         2. Tell the team?\n\
         \x20  - Yes\n\
         \x20  - No"
    );

    // What the agent reads while nobody answered yet.
    assert_eq!(
        ask_user_result_of(&decision).expect("the result"),
        AskUserResult::asked(decision.decision_id.as_str(), DEADLINE)
    );
    assert_eq!(service.get(&decision.decision_id).expect("read"), decision);
}

#[test]
fn a_set_without_a_title_renders_its_questions_only() {
    let (_clock, service) = fixture();
    let decision = hold_ask_user(
        &service,
        ASKED_AT,
        hold_of(other_set(), &RunId::new(), "call-1"),
    )
    .expect("the hold");
    assert_eq!(
        decision.descriptor.rendering.as_str(),
        "1. Raise the budget?\n\
         \x20  - Yes\n\
         \x20  - No"
    );
}

#[test]
fn a_replayed_call_answers_the_same_decision() {
    let (clock, service) = fixture();
    let run_id = RunId::new();
    let first = hold_ask_user(
        &service,
        ASKED_AT,
        hold_of(question_set(), &run_id, "call-1"),
    )
    .expect("the hold");

    // The call is replayed a minute later: the same decision, with the
    // deadline its first hold sealed.
    let later = ASKED_AT + 60_000;
    clock.set(later);
    let replayed = hold_ask_user(&service, later, hold_of(question_set(), &run_id, "call-1"))
        .expect("a replay answers the decision it held");
    assert_eq!(replayed, first);
    assert_eq!(replayed.descriptor.deadline_at_ms, DEADLINE);
    assert_eq!(replayed.revision, 1, "a replay writes nothing");
    assert_eq!(
        service
            .count(&DecisionListFilter::default())
            .expect("count"),
        1
    );

    // A replay after a person answered answers the decision as it stands.
    let approved = answered(&service, &first, vec![choice("notify", "yes")]);
    let after_the_answer =
        hold_ask_user(&service, later, hold_of(question_set(), &run_id, "call-1"))
            .expect("a replay");
    assert_eq!(after_the_answer, approved);

    // Another call of the run, and the same call of another run, hold
    // decisions of their own.
    let another_call = hold_ask_user(&service, later, hold_of(question_set(), &run_id, "call-2"))
        .expect("another call");
    let another_run = hold_ask_user(
        &service,
        later,
        hold_of(question_set(), &RunId::new(), "call-1"),
    )
    .expect("another run");
    assert_ne!(another_call.decision_id, first.decision_id);
    assert_ne!(another_run.decision_id, first.decision_id);
    assert_ne!(another_run.decision_id, another_call.decision_id);
    assert_eq!(another_call.descriptor.deadline_at_ms, later + 600_000);
}

#[test]
fn the_same_call_with_another_set_is_a_hold_conflict() {
    let (_clock, service) = fixture();
    let run_id = RunId::new();
    let first = hold_ask_user(
        &service,
        ASKED_AT,
        hold_of(question_set(), &run_id, "call-1"),
    )
    .expect("the hold");

    let error = hold_ask_user(&service, ASKED_AT, hold_of(other_set(), &run_id, "call-1"))
        .expect_err("the call asked another set before");
    assert_eq!(refusal_of(&error), DecisionRefusal::HoldConflict);

    // The same questions asked by another requester are another binding too.
    let mut another_requester = hold_of(question_set(), &run_id, "call-1");
    another_requester.requester =
        DecisionPrincipalKey::of(&principal(PrincipalKind::SharedAgent, "other")).expect("key");
    let error = hold_ask_user(&service, ASKED_AT, another_requester)
        .expect_err("another requester under the same call");
    assert_eq!(refusal_of(&error), DecisionRefusal::HoldConflict);

    assert_eq!(
        service.get(&first.decision_id).expect("read"),
        first,
        "the held set stands"
    );
}

#[test]
fn a_hold_outside_its_bounds_holds_nothing() {
    let (_clock, service) = fixture();
    let run_id = RunId::new();
    for (case, timeout) in [
        ("59 seconds", Duration::from_secs(59)),
        (
            "a millisecond under a minute",
            Duration::from_millis(59_999),
        ),
        ("a second over 7 days", Duration::from_secs(604_801)),
    ] {
        let mut hold = hold_of(question_set(), &run_id, "call-1");
        hold.timeout = timeout;
        let error = hold_ask_user(&service, ASKED_AT, hold).expect_err(case);
        assert!(
            matches!(&error, AskUserError::Invalid(reason) if reason.contains("timeout_seconds")),
            "{case}: {error:?}"
        );
    }
    let mut no_asker = hold_of(question_set(), &run_id, "call-1");
    no_asker.asker = String::new();
    assert!(matches!(
        hold_ask_user(&service, ASKED_AT, no_asker),
        Err(AskUserError::Invalid(_))
    ));
    assert_eq!(
        service
            .count(&DecisionListFilter::default())
            .expect("count"),
        0
    );

    // The bounds themselves are held.
    for (call_id, timeout) in [
        ("call-min", Duration::from_mins(1)),
        ("call-max", Duration::from_secs(604_800)),
    ] {
        let mut hold = hold_of(question_set(), &run_id, call_id);
        hold.timeout = timeout;
        let decision = hold_ask_user(&service, ASKED_AT, hold).expect(call_id);
        assert_eq!(
            decision.descriptor.deadline_at_ms,
            ASKED_AT + u64::try_from(timeout.as_millis()).unwrap()
        );
    }
}

#[tokio::test]
async fn an_answered_decision_reads_answered_and_owes_then_settles_exactly_one_notice() {
    let (_clock, service) = fixture();
    let notices = Arc::new(RecordingNotices::default());
    let mut executor = executor_loop(&service, &notices);
    let decision = held(&service);
    let decision_id = decision.decision_id.clone();

    // Nothing to do while nobody answered.
    assert!(executor.pass().await.outcomes.is_empty());

    // The person writes an answer to the first question and skips none.
    let approved = answered(
        &service,
        &decision,
        vec![
            choice("notify", "no"),
            free_text("channel", "  Nightly, then beta  "),
        ],
    );
    assert_eq!(
        ask_user_result_of(&approved).expect("the result").outcome,
        AskUserOutcome::Asked,
        "not answered before the executor's pass"
    );
    let body = AskUserBody::of(&approved).expect("the held body");
    let typed = ask_user_answers_of(&approved, &body)
        .expect("a valid document")
        .expect("the answers");
    assert_eq!(
        typed.answers(),
        [
            choice("notify", "no"),
            free_text("channel", "Nightly, then beta")
        ]
    );
    assert_eq!(
        approved.state.evidence_digest.as_deref(),
        Some(
            approved
                .outcome
                .as_ref()
                .expect("the outcome")
                .evidence
                .digest()
                .expect("digest")
                .as_str()
        ),
        "the machine binds the evidence that carries the answers"
    );

    let first = executor.pass().await;
    assert_eq!(
        first.outcomes,
        vec![(decision_id.clone(), DecisionPassOutcome::Executed)]
    );
    let executed = service.get(&decision_id).expect("read");
    assert_eq!(executed.state.phase, DecisionPhase::Executed);
    assert_eq!(executed.state.notice, DecisionNotice::Owed);
    let result = ask_user_result_of(&executed).expect("the result");
    assert_eq!(
        result,
        AskUserResult::ended(
            AskUserOutcome::Answered,
            decision_id.as_str(),
            vec![
                free_text("channel", "Nightly, then beta"),
                choice("notify", "no")
            ],
        ),
        "every question in the set's order, typed"
    );
    assert_eq!(notices.delivered(), Vec::new(), "owed, not yet delivered");

    let second = executor.pass().await;
    assert_eq!(
        second.outcomes,
        vec![(decision_id.clone(), DecisionPassOutcome::NoticeSettled)]
    );
    assert_eq!(
        notices.delivered(),
        vec![AskUserNotice {
            decision_id: decision_id.clone(),
            asker: ASKER.to_owned(),
            result,
        }]
    );
    let settled = service.get(&decision_id).expect("read");
    assert_eq!(settled.state.notice, DecisionNotice::Delivered);
    assert!(matches!(
        settled.notice_receipt,
        Some(ApprovalConsentDeliveryState::Admitted { .. })
    ));

    let third = executor.pass().await;
    assert!(third.outcomes.is_empty(), "{:?}", third.outcomes);
    assert_eq!(
        notices.delivered().len(),
        1,
        "a second pass delivers nothing"
    );
}

#[tokio::test]
async fn a_question_the_person_skipped_reads_unanswered() {
    let (_clock, service) = fixture();
    let notices = Arc::new(RecordingNotices::default());
    let mut executor = executor_loop(&service, &notices);
    let decision = held(&service);
    answered(&service, &decision, vec![choice("channel", "stable")]);

    settle(&mut executor, &decision.decision_id).await;
    let delivered = notices.delivered();
    assert_eq!(delivered.len(), 1);
    assert_eq!(delivered[0].result.outcome, AskUserOutcome::Answered);
    assert_eq!(
        delivered[0].result.answers,
        vec![choice("channel", "stable"), unanswered("notify")]
    );
}

#[tokio::test]
async fn a_rejected_decision_reads_dismissed() {
    let (_clock, service) = fixture();
    let notices = Arc::new(RecordingNotices::default());
    let mut executor = executor_loop(&service, &notices);
    let decision = held(&service);
    present(&service, &decision);
    let rejected = service
        .decide(
            &decision.decision_id,
            &person_decides(&decision, false),
            &MayAnswer,
        )
        .expect("the dismissal is recorded");
    assert_eq!(rejected.state.phase, DecisionPhase::Rejected);

    let result = ask_user_result_of(&rejected).expect("the result");
    assert_eq!(
        result,
        AskUserResult::ended(
            AskUserOutcome::Dismissed,
            decision.decision_id.as_str(),
            all_unanswered()
        )
    );
    assert_eq!(
        settle(&mut executor, &decision.decision_id).await,
        vec![DecisionPassOutcome::NoticeSettled]
    );
    assert_eq!(
        notices.delivered(),
        vec![AskUserNotice {
            decision_id: decision.decision_id.clone(),
            asker: ASKER.to_owned(),
            result,
        }]
    );
    assert!(executor.pass().await.outcomes.is_empty());
    assert_eq!(notices.delivered().len(), 1);
}

#[tokio::test]
async fn the_deadline_passing_reads_timed_out() {
    let (clock, service) = fixture();
    let notices = Arc::new(RecordingNotices::default());
    let mut executor = executor_loop(&service, &notices);
    let decision = held(&service);
    let decision_id = decision.decision_id.clone();

    clock.set(DEADLINE - 1);
    assert!(executor.pass().await.outcomes.is_empty(), "not yet due");
    assert_eq!(
        ask_user_result_of(&service.get(&decision_id).expect("read"))
            .expect("the result")
            .outcome,
        AskUserOutcome::Asked
    );

    clock.set(DEADLINE);
    let did = settle(&mut executor, &decision_id).await;
    assert_eq!(
        did,
        vec![
            DecisionPassOutcome::Expired,
            DecisionPassOutcome::NoticeSettled
        ]
    );
    let expired = service.get(&decision_id).expect("read");
    assert_eq!(expired.state.phase, DecisionPhase::Expired);
    let result = ask_user_result_of(&expired).expect("the result");
    assert_eq!(
        result,
        AskUserResult::ended(
            AskUserOutcome::TimedOut,
            decision_id.as_str(),
            all_unanswered()
        )
    );
    assert_eq!(
        notices.delivered(),
        vec![AskUserNotice {
            decision_id: decision_id.clone(),
            asker: ASKER.to_owned(),
            result,
        }]
    );

    // An answer after the deadline is refused: the set timed out.
    let body = AskUserBody::of(&decision).expect("the held body");
    let answers = AskUserAnswers::for_set(&body.questions, vec![choice("notify", "yes")])
        .expect("valid answers");
    let late =
        attest_ask_user_answers(person_decides(&decision, true), &answers).expect("attested");
    assert!(service.decide(&decision_id, &late, &MayAnswer).is_err());
    assert!(executor.pass().await.outcomes.is_empty());
    assert_eq!(notices.delivered().len(), 1);
}

#[tokio::test]
async fn a_host_withdrawal_reads_withdrawn() {
    let (_clock, service) = fixture();
    let notices = Arc::new(RecordingNotices::default());
    let mut executor = executor_loop(&service, &notices);
    let decision = held(&service);
    let withdrawn = service
        .withdraw(&decision.decision_id, &DecisionWithdrawer::Host)
        .expect("the host withdraws the set");
    assert_eq!(withdrawn.state.phase, DecisionPhase::Withdrawn);

    let result = ask_user_result_of(&withdrawn).expect("the result");
    assert_eq!(
        result,
        AskUserResult::ended(
            AskUserOutcome::Withdrawn,
            decision.decision_id.as_str(),
            all_unanswered()
        )
    );
    assert_eq!(
        settle(&mut executor, &decision.decision_id).await,
        vec![DecisionPassOutcome::NoticeSettled]
    );
    assert_eq!(
        notices.delivered(),
        vec![AskUserNotice {
            decision_id: decision.decision_id.clone(),
            asker: ASKER.to_owned(),
            result,
        }]
    );
    assert!(executor.pass().await.outcomes.is_empty());
    assert_eq!(notices.delivered().len(), 1);
}

#[tokio::test]
async fn after_a_restart_the_open_set_is_found_answered_and_noticed_once() {
    let dir = tempfile::tempdir().expect("tempdir");
    let clock = ManualClock::at(ASKED_AT);
    let open_store =
        || -> Arc<dyn DecisionStore> { Arc::new(SqliteDecisionStore::in_store_path(dir.path())) };
    let notices = Arc::new(RecordingNotices::default());

    // The first process holds the set, runs its loop, and stops before
    // anybody answered.
    let first_owner = service_over(open_store(), &clock);
    let mut first_loop = executor_loop(&first_owner, &notices);
    let decision = held(&first_owner);
    let decision_id = decision.decision_id.clone();
    assert!(first_loop.pass().await.outcomes.is_empty());
    drop(first_loop);
    drop(first_owner);

    // The next process opens the same store: the set is still open, with
    // its questions, its asker and its deadline.
    let second_owner = service_over(open_store(), &clock);
    let mut second_loop = executor_loop(&second_owner, &notices);
    let found = second_owner.get(&decision_id).expect("the set survived");
    assert_eq!(found, decision);
    assert_eq!(found.state.phase, DecisionPhase::Open);
    assert_eq!(
        AskUserBody::of(&found).expect("the held body").questions,
        question_set()
    );
    assert!(second_loop.pass().await.outcomes.is_empty());

    // A person answers after the restart.
    answered(&second_owner, &found, vec![choice("channel", "beta")]);
    assert_eq!(
        settle(&mut second_loop, &decision_id).await,
        vec![
            DecisionPassOutcome::Executed,
            DecisionPassOutcome::NoticeSettled
        ]
    );
    let expected = AskUserResult::ended(
        AskUserOutcome::Answered,
        decision_id.as_str(),
        vec![choice("channel", "beta"), unanswered("notify")],
    );
    assert_eq!(
        notices.delivered(),
        vec![AskUserNotice {
            decision_id: decision_id.clone(),
            asker: ASKER.to_owned(),
            result: expected.clone(),
        }]
    );

    // A third process finds nothing left to do, and reads the same result.
    drop(second_loop);
    drop(second_owner);
    let third_owner = service_over(open_store(), &clock);
    let mut third_loop = executor_loop(&third_owner, &notices);
    assert!(third_loop.pass().await.outcomes.is_empty());
    assert_eq!(notices.delivered().len(), 1, "delivered once");
    let settled = third_owner.get(&decision_id).expect("read");
    assert_eq!(settled.state.notice, DecisionNotice::Delivered);
    assert_eq!(ask_user_result_of(&settled).expect("the result"), expected);
}

#[tokio::test]
async fn a_notice_owed_at_a_restart_is_delivered_by_the_next_process() {
    let dir = tempfile::tempdir().expect("tempdir");
    let clock = ManualClock::at(ASKED_AT);
    let open_store =
        || -> Arc<dyn DecisionStore> { Arc::new(SqliteDecisionStore::in_store_path(dir.path())) };
    let notices = Arc::new(RecordingNotices::default());

    // The first process records the answer and executes it, and stops
    // before it delivers the notice.
    let first_owner = service_over(open_store(), &clock);
    let mut first_loop = executor_loop(&first_owner, &notices);
    let decision = held(&first_owner);
    let decision_id = decision.decision_id.clone();
    answered(&first_owner, &decision, vec![choice("notify", "yes")]);
    assert_eq!(
        first_loop.pass().await.outcomes,
        vec![(decision_id.clone(), DecisionPassOutcome::Executed)]
    );
    drop(first_loop);
    drop(first_owner);
    assert_eq!(notices.delivered(), Vec::new());

    let second_owner = service_over(open_store(), &clock);
    let mut second_loop = executor_loop(&second_owner, &notices);
    assert_eq!(
        settle(&mut second_loop, &decision_id).await,
        vec![DecisionPassOutcome::NoticeSettled]
    );
    let delivered = notices.delivered();
    assert_eq!(delivered.len(), 1);
    assert_eq!(delivered[0].result.outcome, AskUserOutcome::Answered);
    assert_eq!(
        delivered[0].result.answers,
        vec![unanswered("channel"), choice("notify", "yes")]
    );
}

#[tokio::test]
async fn a_reason_that_is_not_an_answer_document_never_reads_as_an_answer_and_is_refused() {
    let (_clock, service) = fixture();
    let notices = Arc::new(RecordingNotices::default());
    let mut executor = executor_loop(&service, &notices);

    // Three approvals that carry no valid answer document: a reason in a
    // person's words, no reason at all, and answers that are valid for
    // another set.
    let in_words = held(&service);
    present(&service, &in_words);
    let approval = person_decides(&in_words, true)
        .with_reason("Looks good to me, go with beta")
        .expect("a reason");
    service
        .decide(&in_words.decision_id, &approval, &MayAnswer)
        .expect("the approval is recorded");

    let bare = held(&service);
    present(&service, &bare);
    service
        .decide(&bare.decision_id, &person_decides(&bare, true), &MayAnswer)
        .expect("the approval is recorded");

    let for_another_set = held(&service);
    present(&service, &for_another_set);
    let other_answers = AskUserAnswers::for_set(&other_set(), vec![choice("budget", "yes")])
        .expect("valid for the other set");
    let approval = attest_ask_user_answers(person_decides(&for_another_set, true), &other_answers)
        .expect("attested");
    service
        .decide(&for_another_set.decision_id, &approval, &MayAnswer)
        .expect("the approval is recorded");

    for decision in [&in_words, &bare, &for_another_set] {
        let approved = service.get(&decision.decision_id).expect("read");
        assert_eq!(approved.state.phase, DecisionPhase::Approved);
        let body = AskUserBody::of(&approved).expect("the held body");
        assert!(
            !matches!(ask_user_answers_of(&approved, &body), Ok(Some(_))),
            "never an answer"
        );
    }
    let approved = service.get(&for_another_set.decision_id).expect("read");
    let body = AskUserBody::of(&approved).expect("the held body");
    assert!(
        matches!(
            ask_user_answers_of(&approved, &body),
            Err(AskUserError::InvalidAnswer(_))
        ),
        "answers that are not valid for the held set are an error"
    );

    let pass = executor.pass().await;
    for decision in [&in_words, &bare, &for_another_set] {
        let decision_id = &decision.decision_id;
        assert_eq!(
            outcomes_of(&pass).get(decision_id),
            Some(&vec![DecisionPassOutcome::ExecutionRefused]),
        );
        let refused = service.get(decision_id).expect("read");
        assert_eq!(refused.state.phase, DecisionPhase::ExecutionRefused);
        assert_eq!(
            refused
                .execution_refusal
                .as_ref()
                .map(DecisionReference::as_str),
            Some("invalid_answer")
        );
        assert_eq!(
            ask_user_result_of(&refused).expect("the result"),
            AskUserResult::ended(
                AskUserOutcome::Withdrawn,
                decision_id.as_str(),
                all_unanswered()
            ),
            "the asker reads no answer"
        );
    }
    settle(&mut executor, &bare.decision_id).await;
    let delivered = notices.delivered();
    assert_eq!(delivered.len(), 3, "each refused set owes its one notice");
    assert!(
        delivered
            .iter()
            .all(|notice| notice.result.outcome == AskUserOutcome::Withdrawn)
    );
}

#[tokio::test]
async fn a_permit_left_outstanding_settles_executed_only_with_a_valid_answer() {
    let (_clock, service) = fixture();
    let notices = Arc::new(RecordingNotices::default());

    // Two approved sets, one with valid answers and one without, whose
    // executor was authorized and stopped before it settled either.
    let valid = held(&service);
    let valid_row = answered(&service, &valid, vec![choice("notify", "yes")]);
    let bare = held(&service);
    present(&service, &bare);
    let bare_row = service
        .decide(&bare.decision_id, &person_decides(&bare, true), &MayAnswer)
        .expect("the approval is recorded");
    for row in [&valid_row, &bare_row] {
        let _permit = service
            .authorize_execution(
                &row.decision_id,
                &row.descriptor.operation_id,
                &row.descriptor.request_digest,
                &AskUserApproverCurrent.approver_answer(row),
            )
            .expect("a permit");
    }

    let mut executor = executor_loop(&service, &notices);
    let pass = executor.pass().await;
    assert_eq!(
        outcomes_of(&pass),
        BTreeMap::from([
            (
                valid.decision_id.clone(),
                vec![DecisionPassOutcome::SettledExecuted]
            ),
            (
                bare.decision_id.clone(),
                vec![
                    DecisionPassOutcome::SettledReleased,
                    DecisionPassOutcome::ExecutionRefused
                ]
            ),
        ])
    );
    assert_eq!(
        ask_user_result_of(&service.get(&valid.decision_id).expect("read"))
            .expect("the result")
            .outcome,
        AskUserOutcome::Answered
    );
    assert_eq!(
        ask_user_result_of(&service.get(&bare.decision_id).expect("read"))
            .expect("the result")
            .outcome,
        AskUserOutcome::Withdrawn
    );
}

#[test]
fn a_dismissal_carries_no_answers() {
    let (_clock, service) = fixture();
    let decision = held(&service);
    let body = AskUserBody::of(&decision).expect("the held body");
    let answers = AskUserAnswers::for_set(&body.questions, vec![choice("notify", "yes")])
        .expect("valid answers");
    assert!(matches!(
        attest_ask_user_answers(person_decides(&decision, false), &answers),
        Err(AskUserError::Invalid(_))
    ));

    // A dismissal with a reason that happens to be an answer document
    // reads as no answer.
    present(&service, &decision);
    let dismissal = person_decides(&decision, false)
        .with_reason(r#"{"answers":[{"question_id":"notify","answer":{"kind":"choice","choice_id":"yes"}}]}"#)
        .expect("a reason");
    let rejected = service
        .decide(&decision.decision_id, &dismissal, &MayAnswer)
        .expect("the dismissal is recorded");
    assert_eq!(ask_user_answers_of(&rejected, &body), Ok(None));
    assert_eq!(
        ask_user_result_of(&rejected).expect("the result").answers,
        all_unanswered()
    );
}

/// A hold of `kind` whose request is `request`, made directly with the
/// decision owner.
fn raw_hold(kind: &str, subject: &str, request: &serde_json::Value) -> DecisionHoldRequest {
    DecisionHoldRequest::new(
        DecisionKind::new(kind).expect("kind"),
        DecisionSubjectKey::new(subject).expect("subject key"),
        DecisionOperationId::new(format!("operation:{subject}")).expect("operation id"),
        Sha256Digest::of(request.to_string().as_bytes()),
        DecisionRequestBody::from_value(request).expect("request body"),
        requester(),
        approver_id(),
        DEADLINE,
        DecisionRendering::new("Release build 7 to the stable channel.").expect("rendering"),
        DecisionRenderer::new("render.v1").expect("renderer"),
    )
}

#[test]
fn a_decision_of_another_kind_or_body_version_is_not_a_question_set() {
    let (_clock, service) = fixture();
    let question_set_body = serde_json::json!({
        "body": ASK_USER_QUESTIONS_BODY,
        "asker": ASKER,
        "questions": serde_json::to_value(question_set()).unwrap(),
    });

    // The control: this body under the kind reads.
    let control = service
        .hold(
            raw_hold(ASK_USER_QUESTIONS_KIND, "control", &question_set_body),
            true,
        )
        .expect("hold");
    assert_eq!(
        AskUserBody::of(&control).expect("the held body").questions,
        question_set()
    );

    // The same body held under another kind is not a question set.
    let another_kind = service
        .hold(
            raw_hold("release", "another-kind", &question_set_body),
            true,
        )
        .expect("hold");
    let error = AskUserBody::of(&another_kind).expect_err("another kind");
    assert!(
        matches!(&error, AskUserError::UnreadableRequest(why) if why.contains("is of kind release")),
        "{error:?}"
    );
    assert!(matches!(
        ask_user_result_of(&another_kind),
        Err(AskUserError::UnreadableRequest(_))
    ));

    // Nor is a later body version, a body with a field this version does
    // not know, or a set that breaks a bound of the tool.
    let mut later_version = question_set_body.clone();
    later_version["body"] = serde_json::json!("ask_user.questions.v2");
    let mut unknown_field = question_set_body.clone();
    unknown_field["priority"] = serde_json::json!("high");
    let mut out_of_bounds = question_set_body;
    out_of_bounds["questions"]["questions"][0]["choices"] = serde_json::json!([]);
    for (subject, request, names) in [
        (
            "later-version",
            later_version,
            "holds the body ask_user.questions.v2",
        ),
        ("unknown-field", unknown_field, "priority"),
        ("out-of-bounds", out_of_bounds, "0 choices"),
    ] {
        let decision = service
            .hold(raw_hold(ASK_USER_QUESTIONS_KIND, subject, &request), true)
            .expect("hold");
        let error = AskUserBody::of(&decision).expect_err(subject);
        assert!(
            matches!(&error, AskUserError::UnreadableRequest(why) if why.contains(names)),
            "{subject}: {error:?}"
        );
    }
}

#[tokio::test]
async fn an_execution_no_person_decided_or_under_another_binding_commits_nothing() {
    let (_clock, service) = fixture();
    let notices = Arc::new(RecordingNotices::default());
    let executor = AskUserExecutor::new(notices.clone()).expect("the executor");
    assert_eq!(executor.kind().as_str(), "ask_user.questions");

    // A decision of a kind a model may decide, allowed by its model party:
    // its permit's consent is not a person's.
    let model = principal(PrincipalKind::SharedAgent, "checker");
    let model_key = DecisionPrincipalKey::of(&model).expect("key");
    let request = serde_json::json!({"build": 7});
    let by_a_model = service
        .hold(
            raw_hold("release", "by-a-model", &request).with_reviewer(model_key.clone()),
            true,
        )
        .expect("hold");
    service
        .present(
            &by_a_model.decision_id,
            &PresentationRecord::complete(
                PresentationAudience::Reviewer {
                    reviewer: model_key,
                },
                &by_a_model.descriptor.rendering,
                None,
                reference("session"),
                reference("message-1"),
                ASKED_AT,
            ),
        )
        .expect("present");
    let allowed = service
        .review(
            &by_a_model.decision_id,
            &AttributedReview::new(
                model,
                SessionId::new(),
                reference("call-1"),
                DecisionReviewVerdict::Allow,
                by_a_model.descriptor.rendering.subject_digest(),
            )
            .expect("a verdict"),
        )
        .expect("allowed");
    assert_eq!(allowed.state.phase, DecisionPhase::Approved);
    let model_permit = service
        .authorize_execution(
            &allowed.decision_id,
            &allowed.descriptor.operation_id,
            &allowed.descriptor.request_digest,
            &ConsentEligibilityAnswer::Eligible(Arc::new(MayAnswer)),
        )
        .expect("a permit");
    assert_eq!(
        executor.execute(&allowed, &model_permit).await,
        DecisionExecution::Refused(reference("not_a_human_decision"))
    );

    // A person's permit for one answered set does not execute another.
    let first = held(&service);
    let first_row = answered(&service, &first, vec![choice("notify", "yes")]);
    let second = held(&service);
    let second_row = answered(&service, &second, vec![choice("notify", "no")]);
    let permit = service
        .authorize_execution(
            &first_row.decision_id,
            &first_row.descriptor.operation_id,
            &first_row.descriptor.request_digest,
            &AskUserApproverCurrent.approver_answer(&first_row),
        )
        .expect("a permit");
    assert_eq!(
        executor.execute(&second_row, &permit).await,
        DecisionExecution::Released
    );
    assert_eq!(
        executor.execute(&first_row, &permit).await,
        DecisionExecution::Committed,
        "the control: the permit executes its own set"
    );
    assert_eq!(executor.committed(&first_row).await, Ok(true));
    assert!(executor.sweep().await.is_empty());
    assert_eq!(notices.delivered(), Vec::new());
}

#[test]
fn the_person_who_answered_is_always_current() {
    let (_clock, service) = fixture();
    let decision = held(&service);
    match AskUserApproverCurrent.approver_answer(&decision) {
        ConsentEligibilityAnswer::Eligible(evidence) => {
            assert_eq!(evidence.check_current(), Ok(()));
        }
        other => panic!("expected eligible, got {other:?}"),
    }
}
