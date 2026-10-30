//! Rows of the durable decision owner: attested decisions bound to the
//! subject digest, execution once under a permit across a restart, the
//! deadline and the permit, conflicts, candidate launches, lazy reads, and
//! torn rows.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use meerkat_core::approval::consent::{
    AuthenticatedApprover, ConsentEligibility, ConsentIneligible,
};
use meerkat_core::approval::decision::{
    ApproverId, AttestedDecision, AttributedReview, ConsentProvenance, DecisionContractError,
    DecisionEvidence, DecisionHoldRequest, DecisionId, DecisionIngress, DecisionKind,
    DecisionListFilter, DecisionNotice, DecisionOperationId, DecisionPage, DecisionPhase,
    DecisionPrincipalKey, DecisionReference, DecisionRenderer, DecisionRendering,
    DecisionRequestBody, DecisionReviewVerdict, DecisionRow, DecisionStage, DecisionStore,
    DecisionSubjectKey, DecisionTier, InMemoryDecisionStore, PresentationAudience,
    PresentationRecord, Sha256Digest, StoredDecision,
};
use meerkat_core::approval::{ApprovalAssurance, ApprovalConsentDeliveryState, ApprovalStoreError};
use meerkat_core::{PrincipalKind, PrincipalRef, SessionId, TrustDomainId};

use super::{
    DecisionClock, DecisionClockError, DecisionError, DecisionRefusal, DecisionWithdrawer,
    DecisionWriteMode, DurableDecisionService,
};

const HELD_AT: u64 = 1_000;
const DEADLINE: u64 = 61_000;

/// A store that records the calls it serves and can refuse the next
/// conditional writes as conflicts, as another owner's writes would.
#[derive(Default)]
struct CountingStore {
    inner: InMemoryDecisionStore,
    calls: Mutex<Vec<&'static str>>,
    conflicts: AtomicUsize,
}

impl CountingStore {
    fn record(&self, call: &'static str) {
        self.calls.lock().expect("calls").push(call);
    }

    fn calls(&self) -> Vec<&'static str> {
        self.calls.lock().expect("calls").clone()
    }

    fn clear(&self) {
        self.calls.lock().expect("calls").clear();
    }

    fn inject_conflicts(&self, count: usize) {
        self.conflicts.store(count, Ordering::SeqCst);
    }

    fn stored(&self, decision_id: &DecisionId) -> StoredDecision {
        match self.inner.get(decision_id).expect("read the stored row") {
            Some(DecisionRow::Readable(row)) => *row,
            other => panic!("expected a readable row, got {other:?}"),
        }
    }
}

impl DecisionStore for CountingStore {
    fn get(&self, decision_id: &DecisionId) -> Result<Option<DecisionRow>, ApprovalStoreError> {
        self.record("get");
        self.inner.get(decision_id)
    }

    fn insert_if_absent(&self, row: &StoredDecision) -> Result<(), ApprovalStoreError> {
        self.record("insert_if_absent");
        self.inner.insert_if_absent(row)
    }

    fn replace_if_revision(
        &self,
        expected_revision: u64,
        row: &StoredDecision,
    ) -> Result<(), ApprovalStoreError> {
        self.record("replace_if_revision");
        if self
            .conflicts
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                left.checked_sub(1)
            })
            .is_ok()
        {
            return Err(ApprovalStoreError::Conflict);
        }
        self.inner.replace_if_revision(expected_revision, row)
    }

    fn list(
        &self,
        filter: &DecisionListFilter,
        after: Option<&str>,
        limit: usize,
    ) -> Result<DecisionPage, ApprovalStoreError> {
        self.record("list");
        self.inner.list(filter, after, limit)
    }

    fn owed(&self, after: Option<&str>, limit: usize) -> Result<DecisionPage, ApprovalStoreError> {
        self.record("owed");
        self.inner.owed(after, limit)
    }

    fn due(
        &self,
        now_ms: u64,
        after: Option<&str>,
        limit: usize,
    ) -> Result<DecisionPage, ApprovalStoreError> {
        self.record("due");
        self.inner.due(now_ms, after, limit)
    }

    fn count(&self, filter: &DecisionListFilter) -> Result<u64, ApprovalStoreError> {
        self.record("count");
        self.inner.count(filter)
    }

    fn is_persistent(&self) -> bool {
        false
    }
}

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

/// An eligibility owner's evidence, current until revoked.
struct Eligibility(AtomicBool);

impl Eligibility {
    fn current() -> Self {
        Self(AtomicBool::new(true))
    }

    fn revoke(&self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

impl ConsentEligibility for Eligibility {
    fn check_current(&self) -> Result<(), ConsentIneligible> {
        if self.0.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err(ConsentIneligible)
        }
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

fn key_of(principal: &PrincipalRef) -> DecisionPrincipalKey {
    DecisionPrincipalKey::of(principal).expect("principal key")
}

fn requester() -> DecisionPrincipalKey {
    key_of(&principal(PrincipalKind::PersonalAgent, "builder"))
}

fn approver_id() -> ApproverId {
    ApproverId::new("team:owners").expect("approver id")
}

fn reference(value: &str) -> DecisionReference {
    DecisionReference::new(value).expect("reference")
}

fn operation_id() -> DecisionOperationId {
    DecisionOperationId::new("operation-7").expect("operation id")
}

fn request_digest() -> Sha256Digest {
    Sha256Digest::of(br#"{"build":7}"#)
}

fn rendering() -> DecisionRendering {
    DecisionRendering::new("Release build 7 to the stable channel.").expect("rendering")
}

/// The hold of `subject` by `requester`, until `deadline_at_ms`.
fn hold_of(
    subject: &str,
    requester: DecisionPrincipalKey,
    reviewer: Option<DecisionPrincipalKey>,
    rendering: &DecisionRendering,
    deadline_at_ms: u64,
) -> DecisionHoldRequest {
    let request = DecisionHoldRequest::new(
        DecisionKind::new("release").expect("kind"),
        DecisionSubjectKey::new(subject).expect("subject key"),
        operation_id(),
        request_digest(),
        DecisionRequestBody::from_value(&serde_json::json!({"build": 7})).expect("request body"),
        requester,
        approver_id(),
        deadline_at_ms,
        rendering.clone(),
        DecisionRenderer::new("render.v1").expect("renderer"),
    );
    match reviewer {
        Some(reviewer) => request.with_reviewer(reviewer),
        None => request,
    }
}

fn hold_request(rendering: &DecisionRendering) -> DecisionHoldRequest {
    hold_of("artifact:build-7", requester(), None, rendering, DEADLINE)
}

/// The complete one-part presentation of `rendering` to the approver id.
fn presentation(rendering: &DecisionRendering) -> PresentationRecord {
    PresentationRecord::complete(
        PresentationAudience::Approver {
            approver_id: approver_id(),
        },
        rendering,
        None,
        reference("console"),
        reference("message-1"),
        HELD_AT,
    )
}

/// `decider`'s attested decision on the rendering whose digest is `digest`.
fn attested(decider: &str, digest: Sha256Digest, approve: bool) -> AttestedDecision {
    let approver = AuthenticatedApprover::new(
        principal(PrincipalKind::Human, decider),
        ApprovalAssurance::PersonAuthenticated,
    )
    .expect("authenticated human");
    AttestedDecision::new(
        approver,
        DecisionIngress::new("console").expect("ingress"),
        reference("event-1"),
        approver_id(),
        digest,
        approve,
    )
    .expect("attested decision")
}

fn service_over(store: &Arc<CountingStore>, clock: &Arc<ManualClock>) -> DurableDecisionService {
    let store: Arc<dyn DecisionStore> = store.clone();
    let clock: Arc<dyn DecisionClock> = clock.clone();
    DurableDecisionService::with_clock(store, clock)
}

fn fixture() -> (Arc<CountingStore>, Arc<ManualClock>, DurableDecisionService) {
    let store = Arc::new(CountingStore::default());
    let clock = ManualClock::at(HELD_AT);
    let service = service_over(&store, &clock);
    (store, clock, service)
}

fn refusal(error: &DecisionError) -> DecisionRefusal {
    match error {
        DecisionError::Refused { refusal, .. } => *refusal,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

/// Hold, present and approve the routable test decision.
fn approved(service: &DurableDecisionService, rendering: &DecisionRendering) -> DecisionId {
    let decision_id = service
        .hold(hold_request(rendering), true)
        .expect("hold")
        .decision_id;
    service
        .present(&decision_id, &presentation(rendering))
        .expect("present");
    let row = service
        .decide(
            &decision_id,
            &attested("alice", rendering.subject_digest(), true),
            &Eligibility::current(),
        )
        .expect("approve");
    assert_eq!(row.state.phase, DecisionPhase::Approved);
    decision_id
}

#[test]
fn a_decide_with_the_canonical_digest_after_a_complete_presentation_approves() {
    let (_store, _clock, service) = fixture();
    let rendering = rendering();
    let held = service.hold(hold_request(&rendering), true).expect("hold");
    assert_eq!(held.state.phase, DecisionPhase::Open);
    assert_eq!(held.descriptor.subject_digest, rendering.subject_digest());
    assert_eq!(held.revision, 1);
    let decision_id = held.decision_id;

    let early = service
        .decide(
            &decision_id,
            &attested("alice", rendering.subject_digest(), true),
            &Eligibility::current(),
        )
        .expect_err("no complete presentation reached the approver yet");
    assert_eq!(refusal(&early), DecisionRefusal::PresentationIncomplete);

    let presented = service
        .present(&decision_id, &presentation(&rendering))
        .expect("present");
    assert!(presented.state.presented_to_approver);
    assert_eq!(presented.presentations, vec![presentation(&rendering)]);

    let decided = service
        .decide(
            &decision_id,
            &attested("alice", rendering.subject_digest(), true),
            &Eligibility::current(),
        )
        .expect("approve");
    assert_eq!(decided.state.phase, DecisionPhase::Approved);
    assert_eq!(decided.state.decided_at_ms, Some(HELD_AT));
    let outcome = decided
        .outcome
        .as_ref()
        .expect("an approval has its outcome");
    assert_eq!(outcome.tier, DecisionTier::Human);
    assert_eq!(
        decided.state.evidence_digest.as_deref(),
        Some(outcome.evidence.digest().expect("digest").as_str())
    );
    assert_eq!(service.get(&decision_id).expect("read"), decided);
}

#[test]
fn an_old_digest_cannot_approve() {
    let (_store, _clock, service) = fixture();
    let rendering = rendering();
    let decision_id = service
        .hold(hold_request(&rendering), true)
        .expect("hold")
        .decision_id;
    let old = Sha256Digest::of(b"Release build 6 to the stable channel.");

    let mut stale_presentation = presentation(&rendering);
    stale_presentation.subject_digest = old.clone();
    assert_eq!(
        refusal(
            &service
                .present(&decision_id, &stale_presentation)
                .expect_err("a presentation of another rendering")
        ),
        DecisionRefusal::SubjectChanged
    );
    service
        .present(&decision_id, &presentation(&rendering))
        .expect("present");
    assert_eq!(
        refusal(
            &service
                .decide(
                    &decision_id,
                    &attested("alice", old, true),
                    &Eligibility::current()
                )
                .expect_err("a decision on another rendering")
        ),
        DecisionRefusal::SubjectChanged
    );
    assert_eq!(
        service.get(&decision_id).expect("read").state.phase,
        DecisionPhase::Open
    );
    service
        .decide(
            &decision_id,
            &attested("alice", rendering.subject_digest(), true),
            &Eligibility::current(),
        )
        .expect("the canonical digest approves");
}

#[test]
fn the_requester_cannot_decide() {
    let (_store, _clock, service) = fixture();
    let rendering = rendering();
    let alice = key_of(&principal(PrincipalKind::Human, "alice"));
    let decision_id = service
        .hold(
            hold_of("artifact:build-7", alice, None, &rendering, DEADLINE),
            true,
        )
        .expect("hold")
        .decision_id;
    service
        .present(&decision_id, &presentation(&rendering))
        .expect("present");
    assert_eq!(
        refusal(
            &service
                .decide(
                    &decision_id,
                    &attested("alice", rendering.subject_digest(), true),
                    &Eligibility::current(),
                )
                .expect_err("the requester decides its own request")
        ),
        DecisionRefusal::SelfApproval
    );
    let decided = service
        .decide(
            &decision_id,
            &attested("bob", rendering.subject_digest(), true),
            &Eligibility::current(),
        )
        .expect("another approver decides");
    assert_eq!(decided.state.phase, DecisionPhase::Approved);
}

#[test]
fn a_non_human_or_unauthenticated_decider_cannot_be_built() {
    let unqualified = PrincipalRef::new(PrincipalKind::Human, "alice").expect("principal");
    for decider in [
        principal(PrincipalKind::PersonalAgent, "helper"),
        principal(PrincipalKind::ServiceAccount, "robot"),
        unqualified,
    ] {
        assert!(
            AuthenticatedApprover::new(decider.clone(), ApprovalAssurance::PersonAuthenticated)
                .is_err(),
            "{decider:?} cannot decide"
        );
    }
    assert_eq!(
        AttributedReview::new(
            principal(PrincipalKind::Human, "alice"),
            SessionId::new(),
            reference("call-1"),
            DecisionReviewVerdict::Allow,
            rendering().subject_digest(),
        )
        .expect_err("a reviewer is an agent"),
        DecisionContractError::ReviewerNotAgent
    );
}

#[test]
fn stale_eligibility_refuses_at_commit() {
    let (_store, _clock, service) = fixture();
    let rendering = rendering();
    let decision_id = service
        .hold(hold_request(&rendering), true)
        .expect("hold")
        .decision_id;
    service
        .present(&decision_id, &presentation(&rendering))
        .expect("present");
    let decision = attested("alice", rendering.subject_digest(), true);
    let eligibility = Eligibility::current();
    eligibility.revoke();
    assert_eq!(
        refusal(
            &service
                .decide(&decision_id, &decision, &eligibility)
                .expect_err("the evidence went stale before the commit")
        ),
        DecisionRefusal::NotEligible
    );
    let stored = service.get(&decision_id).expect("read");
    assert_eq!(stored.state.phase, DecisionPhase::Open);
    assert!(stored.outcome.is_none());
}

#[test]
fn an_approved_decision_executes_once_across_a_restart() {
    let store = Arc::new(CountingStore::default());
    let clock = ManualClock::at(HELD_AT);
    let rendering = rendering();
    let first = service_over(&store, &clock);
    let decision_id = approved(&first, &rendering);
    let permit = first
        .authorize_execution(&decision_id, &operation_id(), &request_digest())
        .expect("permit");
    // The owner commits under the permit and stops before it records the
    // commit.
    drop(first);

    let second = service_over(&store, &clock);
    let executed = second
        .record_executed(
            permit.decision_id(),
            permit.operation_id(),
            permit.request_digest(),
        )
        .expect("the restarted owner settles the commit");
    assert_eq!(executed.state.phase, DecisionPhase::Executed);
    assert_eq!(
        executed.state.executed_request_digest.as_deref(),
        Some(request_digest().as_str())
    );
    assert!(!executed.state.permit_issued);
    assert_eq!(executed.state.notice, DecisionNotice::Owed);

    let replayed = second
        .record_executed(&decision_id, &operation_id(), &request_digest())
        .expect("the same settlement again");
    assert_eq!(replayed, executed, "a replayed settlement changes nothing");
    assert_eq!(
        refusal(
            &second
                .authorize_execution(&decision_id, &operation_id(), &request_digest())
                .expect_err("an executed decision executes once")
        ),
        DecisionRefusal::NotApproved
    );
    assert_eq!(
        refusal(
            &second
                .close_refused(&decision_id, &reference("too_late"))
                .expect_err("an executed decision is never closed refused")
        ),
        DecisionRefusal::NotApproved
    );
}

#[test]
fn a_retryable_refusal_keeps_the_approval() {
    let (_store, _clock, service) = fixture();
    let rendering = rendering();
    let decision_id = approved(&service, &rendering);
    service
        .authorize_execution(&decision_id, &operation_id(), &request_digest())
        .expect("permit");
    let released = service
        .release_execution(&decision_id)
        .expect("release the permit");
    assert_eq!(released.state.phase, DecisionPhase::Approved);
    assert!(!released.state.permit_issued);
    assert_eq!(
        refusal(
            &service
                .release_execution(&decision_id)
                .expect_err("no permit is outstanding")
        ),
        DecisionRefusal::NotAuthorized
    );

    let permit = service
        .authorize_execution(&decision_id, &operation_id(), &request_digest())
        .expect("a later attempt is authorized again");
    let executed = service
        .record_executed(&decision_id, permit.operation_id(), permit.request_digest())
        .expect("executed");
    assert_eq!(executed.state.phase, DecisionPhase::Executed);
}

#[test]
fn a_permanent_refusal_closes_and_owes_the_notice() {
    let (_store, _clock, service) = fixture();
    let rendering = rendering();
    let decision_id = approved(&service, &rendering);
    service
        .authorize_execution(&decision_id, &operation_id(), &request_digest())
        .expect("permit");
    let closed = service
        .close_refused(&decision_id, &reference("quota_exceeded"))
        .expect("close");
    assert_eq!(closed.state.phase, DecisionPhase::ExecutionRefused);
    assert!(!closed.state.permit_issued);
    assert_eq!(closed.state.notice, DecisionNotice::Owed);
    assert_eq!(closed.execution_refusal, Some(reference("quota_exceeded")));

    let owed = service.owed(None, 10).expect("owed");
    assert_eq!(
        owed.decisions
            .iter()
            .map(|row| row.decision_id.clone())
            .collect::<Vec<_>>(),
        vec![decision_id.clone()]
    );
    let receipt = ApprovalConsentDeliveryState::Failed {
        reason: "the requester is unreachable".to_owned(),
    };
    let settled = service
        .settle_notice(&decision_id, &receipt)
        .expect("settle the notice");
    assert_eq!(settled.state.notice, DecisionNotice::Failed);
    assert_eq!(settled.notice_receipt, Some(receipt.clone()));
    assert_eq!(
        service
            .settle_notice(&decision_id, &receipt)
            .expect("the same receipt again"),
        settled
    );
    assert_eq!(
        refusal(
            &service
                .settle_notice(
                    &decision_id,
                    &ApprovalConsentDeliveryState::Admitted {
                        session_id: SessionId::new(),
                        input_id: "input-1".to_owned(),
                    },
                )
                .expect_err("a receipt that contradicts the settled one")
        ),
        DecisionRefusal::NotOwed
    );
    assert_eq!(
        service
            .settle_notice(&decision_id, &ApprovalConsentDeliveryState::Owed)
            .expect_err("an owed state is no receipt"),
        DecisionError::NotAReceipt
    );
    assert!(service.owed(None, 10).expect("owed").decisions.is_empty());
}

#[test]
fn a_second_owner_conflict_reloads_once_then_reports_conflict() {
    let (store, _clock, service) = fixture();
    let rendering = rendering();
    let decision_id = service
        .hold(hold_request(&rendering), true)
        .expect("hold")
        .decision_id;

    store.clear();
    store.inject_conflicts(1);
    let presented = service
        .present(&decision_id, &presentation(&rendering))
        .expect("one conflict reloads the row and applies the input again");
    assert!(presented.state.presented_to_approver);
    assert_eq!(
        store.calls(),
        vec!["get", "replace_if_revision", "get", "replace_if_revision"]
    );

    store.clear();
    store.inject_conflicts(2);
    assert_eq!(
        service
            .decide(
                &decision_id,
                &attested("alice", rendering.subject_digest(), true),
                &Eligibility::current(),
            )
            .expect_err("a second conflict is reported"),
        DecisionError::Conflict(decision_id.clone())
    );
    assert_eq!(
        store.calls(),
        vec!["get", "replace_if_revision", "get", "replace_if_revision"]
    );
    assert_eq!(store.stored(&decision_id).state.phase, DecisionPhase::Open);
}

#[test]
fn a_candidate_reads_without_writing_and_refuses_every_write() {
    let (store, clock, admitted) = fixture();
    let rendering = rendering();
    let decision_id = admitted
        .hold(hold_request(&rendering), true)
        .expect("hold")
        .decision_id;
    let candidate = admitted.for_candidate_launch();
    assert_eq!(candidate.write_mode(), DecisionWriteMode::CandidateLaunch);
    assert_eq!(admitted.write_mode(), DecisionWriteMode::Admitted);

    store.clear();
    let digest = rendering.subject_digest();
    assert_eq!(
        candidate
            .hold(hold_request(&rendering), true)
            .expect_err("hold"),
        DecisionError::CandidateLaunch
    );
    assert_eq!(
        candidate
            .present(&decision_id, &presentation(&rendering))
            .expect_err("present"),
        DecisionError::CandidateLaunch
    );
    assert_eq!(
        candidate
            .decide(
                &decision_id,
                &attested("alice", digest.clone(), true),
                &Eligibility::current(),
            )
            .expect_err("decide"),
        DecisionError::CandidateLaunch
    );
    let review = AttributedReview::new(
        principal(PrincipalKind::SharedAgent, "reviewer"),
        SessionId::new(),
        reference("call-1"),
        DecisionReviewVerdict::Allow,
        digest,
    )
    .expect("review");
    assert_eq!(
        candidate.review(&decision_id, &review).expect_err("review"),
        DecisionError::CandidateLaunch
    );
    assert_eq!(
        candidate
            .withdraw(&decision_id, &DecisionWithdrawer::Host)
            .expect_err("withdraw"),
        DecisionError::CandidateLaunch
    );
    assert_eq!(
        candidate
            .authorize_execution(&decision_id, &operation_id(), &request_digest())
            .expect_err("authorize"),
        DecisionError::CandidateLaunch
    );
    assert_eq!(
        candidate
            .release_execution(&decision_id)
            .expect_err("release"),
        DecisionError::CandidateLaunch
    );
    assert_eq!(
        candidate
            .record_executed(&decision_id, &operation_id(), &request_digest())
            .expect_err("record"),
        DecisionError::CandidateLaunch
    );
    assert_eq!(
        candidate
            .close_refused(&decision_id, &reference("refused"))
            .expect_err("close"),
        DecisionError::CandidateLaunch
    );
    assert_eq!(
        candidate
            .settle_notice(
                &decision_id,
                &ApprovalConsentDeliveryState::Failed {
                    reason: "unreachable".to_owned(),
                },
            )
            .expect_err("settle"),
        DecisionError::CandidateLaunch
    );
    assert!(store.calls().is_empty(), "a refused write reads nothing");

    // Past the deadline a candidate answers the expiry it observes, and
    // records nothing.
    clock.set(DEADLINE);
    let read = candidate.get(&decision_id).expect("read");
    assert_eq!(read.state.phase, DecisionPhase::Expired);
    assert_eq!(read.revision, 1);
    assert_eq!(store.calls(), vec!["get"]);
    let stored = store.stored(&decision_id);
    assert_eq!(stored.state.phase, DecisionPhase::Open);
    assert_eq!(stored.revision, 1);

    let recorded = admitted
        .get(&decision_id)
        .expect("the admitted owner records it");
    assert_eq!(recorded.state.phase, DecisionPhase::Expired);
    assert_eq!(store.stored(&decision_id), recorded);
}

#[test]
fn nothing_is_read_at_open() {
    let store = Arc::new(CountingStore::default());
    let clock = ManualClock::at(HELD_AT);
    let service = service_over(&store, &clock);
    let candidate = service.for_candidate_launch();
    assert!(!service.is_persistent());
    assert!(!candidate.is_persistent());
    assert!(store.calls().is_empty(), "opening reads nothing");

    let unavailable = DurableDecisionService::unavailable("no durable decision store");
    assert_eq!(
        unavailable
            .get(&DecisionId::derive(
                &DecisionKind::new("release").expect("kind"),
                &DecisionSubjectKey::new("artifact:build-7").expect("subject key"),
            ))
            .expect_err("an unavailable owner refuses"),
        DecisionError::Unavailable("no durable decision store".to_owned())
    );
}

#[test]
fn owed_and_due_read_only_their_pages() {
    let (store, clock, service) = fixture();
    let rendering = rendering();
    let hold = |subject: &str, deadline_at_ms: u64| {
        service
            .hold(
                hold_of(subject, requester(), None, &rendering, deadline_at_ms),
                true,
            )
            .expect("hold")
            .decision_id
    };
    let late = hold("subject:late", 200_000);
    let due = hold("subject:due", DEADLINE);
    let approved = hold("subject:approved", 200_000);
    let rejected = hold("subject:rejected", 200_000);
    for (decision_id, approve) in [(&approved, true), (&rejected, false)] {
        service
            .present(decision_id, &presentation(&rendering))
            .expect("present");
        service
            .decide(
                decision_id,
                &attested("alice", rendering.subject_digest(), approve),
                &Eligibility::current(),
            )
            .expect("decide");
    }

    clock.set(100_000);
    store.clear();
    let owed = service.owed(None, 10).expect("owed");
    assert_eq!(store.calls(), vec!["owed"], "one index page, nothing else");
    let mut owed_ids: Vec<DecisionId> = owed
        .decisions
        .iter()
        .map(|row| row.decision_id.clone())
        .collect();
    owed_ids.sort();
    let mut expected = vec![approved, rejected];
    expected.sort();
    assert_eq!(owed_ids, expected);

    store.clear();
    let first = service.owed(None, 1).expect("first page");
    assert_eq!(first.decisions.len(), 1);
    let after = first.next_after.expect("another page");
    let second = service.owed(Some(&after), 1).expect("second page");
    assert_eq!(second.decisions.len(), 1);
    assert_eq!(second.next_after, None);
    assert_eq!(store.calls(), vec!["owed", "owed"]);

    store.clear();
    let due_page = service.due(None, 10).expect("due");
    assert_eq!(store.calls(), vec!["due"]);
    assert_eq!(
        due_page
            .decisions
            .iter()
            .map(|row| row.decision_id.clone())
            .collect::<Vec<_>>(),
        vec![due.clone()]
    );
    assert_ne!(late, due);
}

#[test]
fn a_torn_row_is_unreadable_and_never_executes() {
    let (store, _clock, service) = fixture();
    let rendering = rendering();
    let decision_id = approved(&service, &rendering);

    // The stored evidence no longer hashes to the evidence digest the
    // decision's machine state binds.
    let mut torn = store.stored(&decision_id);
    let Some(outcome) = torn.outcome.as_mut() else {
        panic!("an approved row has its outcome");
    };
    if let DecisionEvidence::Human { reason, .. } = &mut outcome.evidence {
        *reason = Some("rewritten later".to_owned());
    }
    store
        .inner
        .replace_if_revision(torn.revision, &torn)
        .expect("tamper with the row");
    let error = service
        .authorize_execution(&decision_id, &operation_id(), &request_digest())
        .expect_err("a torn row never executes");
    assert!(
        matches!(&error, DecisionError::Unreadable { decision_id: id, .. } if id == decision_id.as_str()),
        "{error:?}"
    );
    let listing = service
        .list(&DecisionListFilter::default(), None, 10)
        .expect("list");
    assert!(listing.decisions.is_empty());
    assert_eq!(listing.unreadable.len(), 1);
    assert_eq!(listing.unreadable[0].decision_id, decision_id.as_str());

    // A row whose machine state breaks an invariant is torn too: an open
    // decision with a permit outstanding.
    let open = service
        .hold(
            hold_of("subject:open", requester(), None, &rendering, DEADLINE),
            true,
        )
        .expect("hold");
    let mut broken = open.clone();
    broken.state.permit_issued = true;
    store
        .inner
        .replace_if_revision(open.revision, &broken)
        .expect("tamper with the row");
    assert!(matches!(
        service
            .get(&open.decision_id)
            .expect_err("an invariant refuses the row"),
        DecisionError::Unreadable { .. }
    ));
}

#[test]
fn the_permit_carries_decided_consent_and_the_held_binding() {
    let (_store, _clock, service) = fixture();
    let rendering = rendering();
    let decision_id = approved(&service, &rendering);
    assert_eq!(
        refusal(
            &service
                .authorize_execution(&decision_id, &operation_id(), &Sha256Digest::of(b"other"))
                .expect_err("another request")
        ),
        DecisionRefusal::BindingMismatch
    );

    let permit = service
        .authorize_execution(&decision_id, &operation_id(), &request_digest())
        .expect("permit");
    assert_eq!(permit.decision_id(), &decision_id);
    assert!(permit.binds(&operation_id(), &request_digest()));
    assert!(!permit.binds(&operation_id(), &Sha256Digest::of(b"other")));
    assert_eq!(permit.deadline_at_ms(), DEADLINE);
    assert!(permit.admits_commit_at(DEADLINE - 1));
    assert!(!permit.admits_commit_at(DEADLINE));

    let row = service.get(&decision_id).expect("read");
    assert!(row.state.permit_issued, "the permit is outstanding");
    let outcome = row.outcome.as_ref().expect("outcome");
    match permit.consent() {
        ConsentProvenance::Decided {
            decision_id: consent_decision,
            subject_digest,
            renderer,
            tier,
            approver_id: consent_approver,
            decided_at_ms,
            evidence,
        } => {
            assert_eq!(consent_decision, &decision_id);
            assert_eq!(subject_digest, &rendering.subject_digest());
            assert_eq!(renderer.as_str(), "render.v1");
            assert_eq!(*tier, DecisionTier::Human);
            assert_eq!(consent_approver, &approver_id());
            assert_eq!(Some(*decided_at_ms), row.state.decided_at_ms);
            assert_eq!(evidence, &outcome.evidence);
        }
        other => panic!("a permit carries decided consent, not {other:?}"),
    }

    let again = service
        .authorize_execution(&decision_id, &operation_id(), &request_digest())
        .expect("authorizing again before the deadline");
    assert_eq!(again, permit);
    assert_eq!(
        service.get(&decision_id).expect("read"),
        row,
        "nothing changed"
    );
}

#[test]
fn a_permit_outstanding_across_the_deadline_settles_executed() {
    let (_store, clock, service) = fixture();
    let rendering = rendering();
    let decision_id = approved(&service, &rendering);
    let permit = service
        .authorize_execution(&decision_id, &operation_id(), &request_digest())
        .expect("permit");

    clock.set(DEADLINE + 5);
    let waiting = service.get(&decision_id).expect("read past the deadline");
    assert_eq!(waiting.state.phase, DecisionPhase::Approved);
    assert!(
        waiting.state.permit_issued,
        "the decision waits for the settlement"
    );
    assert!(
        service.due(None, 10).expect("due").decisions.is_empty(),
        "a decision waiting for its settlement is not due"
    );
    assert_eq!(
        service
            .owed(None, 10)
            .expect("owed")
            .decisions
            .into_iter()
            .map(|row| row.decision_id)
            .collect::<Vec<_>>(),
        vec![decision_id.clone()],
        "it is owed"
    );
    assert_eq!(
        refusal(
            &service
                .authorize_execution(&decision_id, &operation_id(), &request_digest())
                .expect_err("nothing new runs after the deadline")
        ),
        DecisionRefusal::SettlementOwed
    );
    assert!(!permit.admits_commit_at(DEADLINE + 5));

    let executed = service
        .record_executed(&decision_id, permit.operation_id(), permit.request_digest())
        .expect("the commit made before the deadline settles");
    assert_eq!(executed.state.phase, DecisionPhase::Executed);
    assert_eq!(executed.state.ended_at_ms, Some(DEADLINE + 5));
}

#[test]
fn a_release_after_the_deadline_records_the_expiry() {
    let (_store, clock, service) = fixture();
    let rendering = rendering();
    let decision_id = approved(&service, &rendering);
    service
        .authorize_execution(&decision_id, &operation_id(), &request_digest())
        .expect("permit");
    clock.set(DEADLINE);
    let released = service
        .release_execution(&decision_id)
        .expect("release at the deadline");
    assert_eq!(released.state.phase, DecisionPhase::Expired);
    assert!(!released.state.permit_issued);
    assert_eq!(released.state.ended_at_ms, Some(DEADLINE));
    assert_eq!(released.state.notice, DecisionNotice::Owed);
}

#[test]
fn an_authorization_at_the_deadline_is_refused_and_records_the_expiry() {
    let (_store, clock, service) = fixture();
    let rendering = rendering();
    let decision_id = approved(&service, &rendering);
    clock.set(DEADLINE);
    assert_eq!(
        refusal(
            &service
                .authorize_execution(&decision_id, &operation_id(), &request_digest())
                .expect_err("an authorization at the deadline")
        ),
        DecisionRefusal::Expired
    );
    let expired = service.get(&decision_id).expect("read");
    assert_eq!(expired.state.phase, DecisionPhase::Expired);
    assert!(!expired.state.permit_issued);
}

#[test]
fn a_hold_replays_its_binding_and_refuses_another() {
    let (_store, clock, service) = fixture();
    let rendering = rendering();
    let held = service.hold(hold_request(&rendering), true).expect("hold");
    clock.set(HELD_AT + 10);
    assert_eq!(
        service
            .hold(hold_request(&rendering), true)
            .expect("the same binding again"),
        held
    );
    let changed =
        DecisionRendering::new("Release build 8 to the stable channel.").expect("rendering");
    assert_eq!(
        refusal(
            &service
                .hold(hold_request(&changed), true)
                .expect_err("another subject digest")
        ),
        DecisionRefusal::HoldConflict
    );
    assert_eq!(
        refusal(
            &service
                .hold(hold_request(&rendering), false)
                .expect_err("another notice route")
        ),
        DecisionRefusal::HoldConflict
    );
    let foreign = hold_request(&rendering)
        .with_expected_subject_digest(Sha256Digest::of(b"another rendering"));
    assert_eq!(
        service
            .hold(foreign, true)
            .expect_err("a caller digest other than the rendering's"),
        DecisionError::Invalid(DecisionContractError::SubjectDigestMismatch)
    );
    clock.set(DEADLINE);
    assert_eq!(
        refusal(
            &service
                .hold(
                    hold_of("subject:late", requester(), None, &rendering, DEADLINE),
                    true,
                )
                .expect_err("a hold at its own deadline")
        ),
        DecisionRefusal::InvalidWindow
    );
}

#[test]
fn a_review_escalates_to_the_human_and_refuses_another_reviewer() {
    let (_store, _clock, service) = fixture();
    let rendering = rendering();
    let digest = rendering.subject_digest();
    let reviewer = principal(PrincipalKind::SharedAgent, "reviewer");
    let decision_id = service
        .hold(
            hold_of(
                "artifact:build-7",
                requester(),
                Some(key_of(&reviewer)),
                &rendering,
                DEADLINE,
            ),
            true,
        )
        .expect("hold")
        .decision_id;
    assert_eq!(
        service.get(&decision_id).expect("read").state.stage,
        DecisionStage::Review
    );
    assert_eq!(
        refusal(
            &service
                .decide(
                    &decision_id,
                    &attested("alice", digest.clone(), true),
                    &Eligibility::current(),
                )
                .expect_err("the review stage is open")
        ),
        DecisionRefusal::ReviewStage
    );
    service
        .present(
            &decision_id,
            &PresentationRecord::complete(
                PresentationAudience::Reviewer {
                    reviewer: key_of(&reviewer),
                },
                &rendering,
                None,
                reference("session"),
                reference("message-1"),
                HELD_AT,
            ),
        )
        .expect("present to the reviewer");

    let other = AttributedReview::new(
        principal(PrincipalKind::SharedAgent, "other"),
        SessionId::new(),
        reference("call-0"),
        DecisionReviewVerdict::Allow,
        digest.clone(),
    )
    .expect("review");
    assert_eq!(
        refusal(
            &service
                .review(&decision_id, &other)
                .expect_err("another agent's verdict")
        ),
        DecisionRefusal::WrongReviewer
    );

    let escalate = AttributedReview::new(
        reviewer,
        SessionId::new(),
        reference("call-1"),
        DecisionReviewVerdict::Escalate,
        digest.clone(),
    )
    .expect("review");
    let escalated = service.review(&decision_id, &escalate).expect("escalate");
    assert_eq!(escalated.state.phase, DecisionPhase::Open);
    assert_eq!(escalated.state.stage, DecisionStage::Human);
    assert!(escalated.outcome.is_none());

    let decision = attested("alice", digest, true).with_presentation(presentation(&rendering));
    let decided = service
        .decide(&decision_id, &decision, &Eligibility::current())
        .expect("the human approves with the presentation it carried");
    assert_eq!(decided.state.phase, DecisionPhase::Approved);
    assert!(decided.state.presented_to_approver);
    assert_eq!(decided.presentations.len(), 2);
}

#[test]
fn withdraw_is_open_only_by_requester_or_host() {
    let (_store, _clock, service) = fixture();
    let rendering = rendering();
    let decision_id = service
        .hold(hold_request(&rendering), true)
        .expect("hold")
        .decision_id;
    assert_eq!(
        refusal(
            &service
                .withdraw(
                    &decision_id,
                    &DecisionWithdrawer::Requester(key_of(&principal(
                        PrincipalKind::Human,
                        "mallory"
                    ))),
                )
                .expect_err("not the requester")
        ),
        DecisionRefusal::NotRequester
    );
    let withdrawn = service
        .withdraw(&decision_id, &DecisionWithdrawer::Requester(requester()))
        .expect("the requester withdraws");
    assert_eq!(withdrawn.state.phase, DecisionPhase::Withdrawn);
    assert_eq!(withdrawn.state.notice, DecisionNotice::Owed);
    assert_eq!(
        refusal(
            &service
                .decide(
                    &decision_id,
                    &attested("alice", rendering.subject_digest(), true),
                    &Eligibility::current(),
                )
                .expect_err("a withdrawn decision is decided by no one")
        ),
        DecisionRefusal::NotOpen
    );

    let other = service
        .hold(
            hold_of("subject:other", requester(), None, &rendering, DEADLINE),
            false,
        )
        .expect("hold");
    let withdrawn = service
        .withdraw(&other.decision_id, &DecisionWithdrawer::Host)
        .expect("the host withdraws");
    assert_eq!(withdrawn.state.phase, DecisionPhase::Withdrawn);
    assert_eq!(withdrawn.state.notice, DecisionNotice::NotRequired);
}
