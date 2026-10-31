//! Rows of the durable decision owner: attested decisions bound to the
//! subject digest, execution once under a permit across a restart, the
//! deadline and the permit, conflicts, candidate launches, lazy reads, torn
//! rows, and the stores' work queries checked against the machine.

use std::collections::BTreeSet;
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
    PresentationRecord, Sha256Digest, StoredDecision, decision_is_due, decision_owes_work,
};
use meerkat_core::approval::{ApprovalAssurance, ApprovalConsentDeliveryState, ApprovalStoreError};
use meerkat_core::{PrincipalKind, PrincipalRef, SessionId, TrustDomainId};
use meerkat_store::SqliteDecisionStore;

use super::dsl::{
    DecisionDigest, DurableDecisionEffect, DurableDecisionInput, DurableDecisionMachineAuthority,
    DurableDecisionMachineMutator, DurableDecisionMachineState, DurableDecisionPhase,
};
use super::service::{issued_permit, machine_state};
use super::{
    DecisionClock, DecisionClockError, DecisionError, DecisionRefusal, DecisionWithdrawer,
    DecisionWriteMode, DurableDecisionService,
};

const HELD_AT: u64 = 1_000;
const DEADLINE: u64 = 61_000;
/// When the ingress observed the rows' default decision event: just after
/// the default presentation, shown at `HELD_AT`.
const OBSERVED_AT: u64 = HELD_AT + 1;
/// When the presentation-order rows show the decision to the approver:
/// after the hold, so a decision can be observed before, at or after it.
const PRESENTED_AT: u64 = HELD_AT + 100;

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

/// `decider`'s attested decision on the rendering whose digest is `digest`,
/// observed just after the default presentation ([`OBSERVED_AT`]).
fn attested(decider: &str, digest: Sha256Digest, approve: bool) -> AttestedDecision {
    attested_at(decider, digest, approve, OBSERVED_AT)
}

/// The complete one-part presentation of `rendering` to the approver id,
/// shown at `at_ms`.
fn presentation_at(rendering: &DecisionRendering, at_ms: u64) -> PresentationRecord {
    PresentationRecord::complete(
        PresentationAudience::Approver {
            approver_id: approver_id(),
        },
        rendering,
        None,
        reference("chat"),
        reference("message-1"),
        at_ms,
    )
}

/// `decider`'s attested decision on the rendering whose digest is `digest`,
/// whose ingress observed the decision event at `observed_at_ms`.
fn attested_at(
    decider: &str,
    digest: Sha256Digest,
    approve: bool,
    observed_at_ms: u64,
) -> AttestedDecision {
    let approver = AuthenticatedApprover::new(
        principal(PrincipalKind::Human, decider),
        ApprovalAssurance::PersonAuthenticated,
    )
    .expect("authenticated human");
    AttestedDecision::new(
        approver,
        DecisionIngress::new("console").expect("ingress"),
        reference("event-1"),
        observed_at_ms,
        approver_id(),
        digest,
        approve,
    )
    .expect("attested decision")
}

/// When the evidence of the decided `row` says its ingress observed the
/// decision event, read from the evidence's stored encoding.
fn recorded_observed_at(row: &StoredDecision) -> Option<u64> {
    let outcome = row.outcome.as_ref()?;
    serde_json::to_value(&outcome.evidence)
        .expect("encode the evidence")
        .get("observed_at_ms")
        .and_then(serde_json::Value::as_u64)
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
    approved_subject(service, rendering, "artifact:build-7")
}

/// Hold, present and approve the routable decision of `subject`.
fn approved_subject(
    service: &DurableDecisionService,
    rendering: &DecisionRendering,
    subject: &str,
) -> DecisionId {
    let decision_id = service
        .hold(
            hold_of(subject, requester(), None, rendering, DEADLINE),
            true,
        )
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
    assert_eq!(
        presented.state.first_presented_to_approver_at_ms,
        Some(HELD_AT)
    );
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

/// A decision observed before the decision's first presentation to the
/// approver is refused `decided_before_presentation`, an approval and a
/// rejection alike, whether the presentation was recorded first or carried
/// by the decision event; the refusal changes nothing.
#[test]
fn a_decide_observed_before_the_first_presentation_is_refused() {
    let (store, _clock, service) = fixture();
    let rendering = rendering();
    let digest = rendering.subject_digest();
    let hold = |subject: &str| {
        service
            .hold(
                hold_of(subject, requester(), None, &rendering, DEADLINE),
                true,
            )
            .expect("hold")
            .decision_id
    };

    // Shown to the approver at PRESENTED_AT, decided a millisecond before.
    let recorded = hold("subject:recorded");
    let presented = service
        .present(&recorded, &presentation_at(&rendering, PRESENTED_AT))
        .expect("present");
    for approve in [true, false] {
        let refused = service
            .decide(
                &recorded,
                &attested_at("alice", digest.clone(), approve, PRESENTED_AT - 1),
                &Eligibility::current(),
            )
            .expect_err("a decision observed before the first presentation");
        assert_eq!(
            refusal(&refused).as_str(),
            "decided_before_presentation",
            "approve {approve}"
        );
        assert_eq!(
            store.stored(&recorded),
            presented,
            "the refusal changes nothing"
        );
    }

    // The decision event carries the presentation, shown after the event.
    let carried = hold("subject:carried");
    let held = store.stored(&carried);
    let decision = attested_at("alice", digest, true, PRESENTED_AT - 1)
        .with_presentation(presentation_at(&rendering, PRESENTED_AT));
    let refused = service
        .decide(&carried, &decision, &Eligibility::current())
        .expect_err("a decision observed before the presentation it carries");
    assert_eq!(refusal(&refused).as_str(), "decided_before_presentation");
    assert_eq!(store.stored(&carried), held, "the refusal changes nothing");
}

/// A decision observed after the first presentation commits, an approval
/// and a rejection alike, and its evidence records when its ingress observed
/// it. The first presentation is the earliest one: showing the decision
/// again later does not move it, though the row keeps only the latest
/// presentation record of each audience.
#[test]
fn a_decide_observed_after_the_first_presentation_commits() {
    let (_store, _clock, service) = fixture();
    let rendering = rendering();
    for (subject, approve, phase) in [
        ("subject:approved", true, DecisionPhase::Approved),
        ("subject:rejected", false, DecisionPhase::Rejected),
    ] {
        let decision_id = service
            .hold(
                hold_of(subject, requester(), None, &rendering, DEADLINE),
                true,
            )
            .expect("hold")
            .decision_id;
        service
            .present(&decision_id, &presentation_at(&rendering, PRESENTED_AT))
            .expect("present");
        let shown_again = service
            .present(
                &decision_id,
                &presentation_at(&rendering, PRESENTED_AT + 50),
            )
            .expect("present again");
        assert_eq!(
            shown_again.presentations,
            vec![presentation_at(&rendering, PRESENTED_AT + 50)]
        );
        let decided = service
            .decide(
                &decision_id,
                &attested_at(
                    "alice",
                    rendering.subject_digest(),
                    approve,
                    PRESENTED_AT + 10,
                ),
                &Eligibility::current(),
            )
            .expect("a decision observed after the first presentation commits");
        assert_eq!(decided.state.phase, phase);
        assert_eq!(recorded_observed_at(&decided), Some(PRESENTED_AT + 10));
    }
}

/// The boundary: a decision observed in the same millisecond as the first
/// presentation commits, whether the presentation was recorded first or
/// carried by the decision event.
#[test]
fn a_decide_observed_at_the_first_presentation_commits() {
    let (_store, _clock, service) = fixture();
    let rendering = rendering();
    let digest = rendering.subject_digest();
    let hold = |subject: &str| {
        service
            .hold(
                hold_of(subject, requester(), None, &rendering, DEADLINE),
                true,
            )
            .expect("hold")
            .decision_id
    };

    let recorded = hold("subject:recorded");
    service
        .present(&recorded, &presentation_at(&rendering, PRESENTED_AT))
        .expect("present");
    let decided = service
        .decide(
            &recorded,
            &attested_at("alice", digest.clone(), true, PRESENTED_AT),
            &Eligibility::current(),
        )
        .expect("a decision observed at the first presentation commits");
    assert_eq!(decided.state.phase, DecisionPhase::Approved);
    assert_eq!(recorded_observed_at(&decided), Some(PRESENTED_AT));

    let carried = hold("subject:carried");
    let decision = attested_at("alice", digest, true, PRESENTED_AT)
        .with_presentation(presentation_at(&rendering, PRESENTED_AT));
    let decided = service
        .decide(&carried, &decision, &Eligibility::current())
        .expect("a decision observed when the presentation it carries was shown commits");
    assert_eq!(decided.state.phase, DecisionPhase::Approved);
    assert_eq!(
        decided.presentations,
        vec![presentation_at(&rendering, PRESENTED_AT)]
    );
    assert_eq!(recorded_observed_at(&decided), Some(PRESENTED_AT));
}

/// With no presentation, recorded or carried, a decision keeps its
/// `presentation_incomplete` refusal whenever it was observed: there is no
/// first presentation to order it by. A carried presentation that does not
/// count (an incomplete one, or one shown to another audience) is none.
#[test]
fn a_decide_with_no_presentation_keeps_the_presentation_incomplete_refusal() {
    let (store, _clock, service) = fixture();
    let rendering = rendering();
    let decision_id = service
        .hold(hold_request(&rendering), true)
        .expect("hold")
        .decision_id;
    let held = store.stored(&decision_id);
    let mut incomplete = presentation_at(&rendering, PRESENTED_AT);
    incomplete.parts_shown.clear();
    let to_another_audience = PresentationRecord::complete(
        PresentationAudience::Reviewer {
            reviewer: requester(),
        },
        &rendering,
        None,
        reference("chat"),
        reference("message-1"),
        PRESENTED_AT,
    );
    for observed_at_ms in [0, PRESENTED_AT, DEADLINE - 1] {
        for carried in [None, Some(&incomplete), Some(&to_another_audience)] {
            let decision = attested_at("alice", rendering.subject_digest(), true, observed_at_ms);
            let decision = match carried {
                Some(presentation) => decision.with_presentation(presentation.clone()),
                None => decision,
            };
            let refused = service
                .decide(&decision_id, &decision, &Eligibility::current())
                .expect_err("no presentation reached the approver");
            assert_eq!(
                refusal(&refused),
                DecisionRefusal::PresentationIncomplete,
                "observed at {observed_at_ms}, carried {carried:?}"
            );
            assert_eq!(
                store.stored(&decision_id),
                held,
                "the refusal changes nothing"
            );
        }
    }
}

/// A presentation time of 0 is no reading: a presentation recorded at 0 is
/// refused `presentation_incomplete` and records nothing, and a decision
/// carrying a presentation at 0 is refused the same way, even after a
/// presentation was recorded.
#[test]
fn a_presentation_time_of_zero_is_no_presentation() {
    let (store, _clock, service) = fixture();
    let rendering = rendering();
    let decision_id = service
        .hold(hold_request(&rendering), true)
        .expect("hold")
        .decision_id;
    let held = store.stored(&decision_id);
    let refused = service
        .present(&decision_id, &presentation_at(&rendering, 0))
        .expect_err("a presentation at 0 is no reading");
    assert_eq!(refusal(&refused), DecisionRefusal::PresentationIncomplete);
    assert_eq!(store.stored(&decision_id), held, "nothing is recorded");

    let carrying = attested_at("alice", rendering.subject_digest(), true, PRESENTED_AT)
        .with_presentation(presentation_at(&rendering, 0));
    let refused = service
        .decide(&decision_id, &carrying, &Eligibility::current())
        .expect_err("a decision carrying a presentation at 0");
    assert_eq!(refusal(&refused), DecisionRefusal::PresentationIncomplete);

    let presented = service
        .present(&decision_id, &presentation_at(&rendering, PRESENTED_AT))
        .expect("present");
    let refused = service
        .decide(&decision_id, &carrying, &Eligibility::current())
        .expect_err("a recorded presentation does not excuse the untimed one");
    assert_eq!(refusal(&refused), DecisionRefusal::PresentationIncomplete);
    assert_eq!(
        store.stored(&decision_id),
        presented,
        "the refusal changes nothing"
    );
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
                .close_refused_after_restart(&decision_id, &reference("too_late"))
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
    let permit = service
        .authorize_execution(&decision_id, &operation_id(), &request_digest())
        .expect("permit");
    let released = service
        .release_execution(permit)
        .expect("release the permit");
    assert_eq!(released.state.phase, DecisionPhase::Approved);
    assert!(!released.state.permit_issued);
    assert_eq!(
        refusal(
            &service
                .release_after_restart(&decision_id)
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
    let permit = service
        .authorize_execution(&decision_id, &operation_id(), &request_digest())
        .expect("permit");
    let closed = service
        .close_refused(permit, &reference("quota_exceeded"))
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
    assert_eq!(
        presented.state.first_presented_to_approver_at_ms,
        Some(HELD_AT)
    );
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
    // Permits the admitted owner issued: a candidate settles none of them.
    let approved_id = approved_subject(&admitted, &rendering, "subject:approved");
    let released_permit = admitted
        .authorize_execution(&approved_id, &operation_id(), &request_digest())
        .expect("permit");
    let closed_permit = admitted
        .authorize_execution(&approved_id, &operation_id(), &request_digest())
        .expect("the same permit again");

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
            .release_execution(released_permit)
            .expect_err("release"),
        DecisionError::CandidateLaunch
    );
    assert_eq!(
        candidate
            .release_after_restart(&approved_id)
            .expect_err("release after a restart"),
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
            .close_refused(closed_permit, &reference("refused"))
            .expect_err("close"),
        DecisionError::CandidateLaunch
    );
    assert_eq!(
        candidate
            .close_refused_after_restart(&approved_id, &reference("refused"))
            .expect_err("close after a restart"),
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
fn owed_lists_recorded_obligations_and_agrees_with_settle_notice() {
    let (store, clock, service) = fixture();
    let rendering = rendering();
    let decision_id = approved(&service, &rendering);
    let receipt = ApprovalConsentDeliveryState::Failed {
        reason: "the requester is unreachable".to_owned(),
    };

    // Past its deadline with no permit outstanding, the decision has not
    // ended until its expiry is recorded: it is listed as stored, and no
    // notice is owed yet.
    clock.set(DEADLINE + 1);
    store.clear();
    let owed = service.owed(None, 10).expect("owed");
    assert_eq!(
        store.calls(),
        vec!["owed"],
        "one index page, nothing written"
    );
    assert_eq!(owed.decisions.len(), 1);
    assert_eq!(owed.decisions[0].decision_id, decision_id);
    assert_eq!(owed.decisions[0].state.phase, DecisionPhase::Approved);
    assert_eq!(owed.decisions[0].state.notice, DecisionNotice::NotDue);
    assert_eq!(
        refusal(
            &service
                .settle_notice(&decision_id, &receipt)
                .expect_err("no notice is owed before the end is recorded")
        ),
        DecisionRefusal::NotOwed
    );

    // A read records the expiry: from then on the notice is owed, listed,
    // and settled by its receipt.
    let expired = service
        .get(&decision_id)
        .expect("the read records the expiry");
    assert_eq!(expired.state.phase, DecisionPhase::Expired);
    assert_eq!(expired.state.notice, DecisionNotice::Owed);
    assert_eq!(
        service.owed(None, 10).expect("owed").decisions,
        vec![expired]
    );
    let settled = service
        .settle_notice(&decision_id, &receipt)
        .expect("the owed notice settles");
    assert_eq!(settled.state.notice, DecisionNotice::Failed);
    assert!(service.owed(None, 10).expect("owed").decisions.is_empty());
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
fn a_permit_comes_only_from_the_machine_authorization() {
    let (_store, _clock, service) = fixture();
    let rendering = rendering();
    let decision_id = approved(&service, &rendering);
    let row = service.get(&decision_id).expect("read");
    let authorized = |decision: &DecisionId| DurableDecisionEffect::ExecutionAuthorized {
        decision_id: decision.as_str().to_owned(),
        operation_id: operation_id().as_str().to_owned(),
        request_digest: DecisionDigest(request_digest().as_str().to_owned()),
        deadline_at_ms: DEADLINE,
    };

    // No authorization, or another decision's, mints nothing: an approved
    // row and the absence of a refusal are not a permit.
    assert!(matches!(
        issued_permit(&row, &[]),
        Err(DecisionError::Machine(_))
    ));
    let other = DecisionId::derive(
        &DecisionKind::new("release").expect("kind"),
        &DecisionSubjectKey::new("subject:other").expect("subject key"),
    );
    assert!(matches!(
        issued_permit(&row, &[authorized(&other)]),
        Err(DecisionError::Machine(_))
    ));

    // The machine's authorization mints it, with the binding and the
    // deadline the authorization names and the consent the row decided.
    let permit = issued_permit(&row, &[authorized(&decision_id)])
        .expect("the machine's authorization mints the permit");
    assert_eq!(permit.decision_id(), &decision_id);
    assert!(permit.binds(&operation_id(), &request_digest()));
    assert_eq!(permit.deadline_at_ms(), DEADLINE);
    assert!(matches!(
        permit.consent(),
        ConsentProvenance::Decided { decision_id: consent, .. } if *consent == decision_id
    ));
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
    let permit = service
        .authorize_execution(&decision_id, &operation_id(), &request_digest())
        .expect("permit");
    clock.set(DEADLINE);
    let released = service
        .release_execution(permit)
        .expect("release at the deadline");
    assert_eq!(released.state.phase, DecisionPhase::Expired);
    assert!(!released.state.permit_issued);
    assert_eq!(released.state.ended_at_ms, Some(DEADLINE));
    assert_eq!(released.state.notice, DecisionNotice::Owed);
}

#[test]
fn recovery_settles_an_outstanding_permit_without_its_object() {
    let store = Arc::new(CountingStore::default());
    let clock = ManualClock::at(HELD_AT);
    let rendering = rendering();
    let first = service_over(&store, &clock);
    let released = approved_subject(&first, &rendering, "subject:released");
    let refused = approved_subject(&first, &rendering, "subject:refused");
    // The executor is authorized for both and stops (a restart) before it
    // commits or settles either: the permit objects are gone, the permits
    // stay outstanding.
    for decision_id in [&released, &refused] {
        let _permit = first
            .authorize_execution(decision_id, &operation_id(), &request_digest())
            .expect("permit");
    }
    drop(first);

    clock.set(DEADLINE + 1);
    let second = service_over(&store, &clock);
    assert_eq!(
        refusal(
            &second
                .authorize_execution(&released, &operation_id(), &request_digest())
                .expect_err("nothing new runs after the deadline")
        ),
        DecisionRefusal::SettlementOwed
    );
    // Settled first: its own record shows no commit under either permit, so
    // it releases one and refuses the other, by id.
    let expired = second
        .release_after_restart(&released)
        .expect("release the outstanding permit");
    assert_eq!(expired.state.phase, DecisionPhase::Expired);
    assert!(!expired.state.permit_issued);
    let closed = second
        .close_refused_after_restart(&refused, &reference("quota_exceeded"))
        .expect("refuse the outstanding permit");
    assert_eq!(closed.state.phase, DecisionPhase::ExecutionRefused);
    assert!(!closed.state.permit_issued);
    assert_eq!(closed.execution_refusal, Some(reference("quota_exceeded")));
    for settled in [&expired, &closed] {
        assert_eq!(settled.state.notice, DecisionNotice::Owed);
    }
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
fn a_replayed_hold_past_its_deadline_answers_the_recorded_expiry() {
    let (store, clock, service) = fixture();
    let rendering = rendering();
    let held = service.hold(hold_request(&rendering), true).expect("hold");
    clock.set(DEADLINE);
    let replayed = service
        .hold(hold_request(&rendering), true)
        .expect("the same binding again");
    assert_eq!(replayed.state.phase, DecisionPhase::Expired);
    assert_eq!(replayed.state.ended_at_ms, Some(DEADLINE));
    assert_eq!(replayed.state.notice, DecisionNotice::Owed);
    assert_eq!(replayed.revision, held.revision + 1);
    assert_eq!(
        store.stored(&held.decision_id),
        replayed,
        "the replay recorded the expiry it answers"
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
    assert_eq!(
        decided.state.first_presented_to_approver_at_ms,
        Some(HELD_AT),
        "the carried presentation is recorded with the time it was shown"
    );
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

#[test]
fn an_approved_decision_with_no_permit_past_its_deadline_is_due() {
    let (_store, clock, service) = fixture();
    let rendering = rendering();
    let decision_id = approved(&service, &rendering);
    clock.set(DEADLINE - 1);
    assert!(
        service.due(None, 10).expect("due").decisions.is_empty(),
        "before its deadline it is not due"
    );

    clock.set(DEADLINE);
    let due = service.due(None, 10).expect("due");
    assert_eq!(
        due.decisions
            .iter()
            .map(|row| row.decision_id.clone())
            .collect::<Vec<_>>(),
        vec![decision_id.clone()]
    );
    assert_eq!(
        due.decisions[0].state.phase,
        DecisionPhase::Approved,
        "listed as stored"
    );
    let expired = service
        .get(&decision_id)
        .expect("the deadline driver's read records the expiry");
    assert_eq!(expired.state.phase, DecisionPhase::Expired);
    assert!(
        service.due(None, 10).expect("due").decisions.is_empty(),
        "once recorded it is due no more"
    );
}

/// The machine of `row`, rebuilt as the owner rebuilds it, after `input`:
/// its effects, and the state it left.
fn machine_after(
    row: &StoredDecision,
    input: DurableDecisionInput,
) -> (Vec<DurableDecisionEffect>, DurableDecisionMachineState) {
    let mut authority =
        DurableDecisionMachineAuthority::recover_from_state(machine_state(&row.state))
            .unwrap_or_else(|error| panic!("{} recovers: {error}", row.decision_id));
    let effects = DurableDecisionMachineMutator::apply(&mut authority, input)
        .unwrap_or_else(|error| panic!("{} answers: {error}", row.decision_id))
        .into_effects();
    (effects, authority.state().clone())
}

/// Whether the machine's `Expire` at `now_ms` records an expiry: the
/// decision is due an expiry observation then.
fn machine_expires(row: &StoredDecision, now_ms: u64) -> bool {
    machine_after(row, DurableDecisionInput::Expire { now_ms })
        .0
        .iter()
        .any(|effect| matches!(effect, DurableDecisionEffect::Expired { .. }))
}

/// Whether the machine owes its owner work: an approved decision (to
/// execute, settle or expire), or a notice its receipt would settle.
fn machine_owes_work(row: &StoredDecision) -> bool {
    let (_, settled) = machine_after(row, DurableDecisionInput::SettleNotice { delivered: true });
    settled.lifecycle_phase == DurableDecisionPhase::Approved
        || settled.notice != machine_state(&row.state).notice
}

#[test]
fn store_due_and_owed_predicates_agree_with_the_machine() {
    let (store, clock, service) = fixture();
    let rendering = rendering();
    let hold = |subject: &str, routable: bool| {
        service
            .hold(
                hold_of(subject, requester(), None, &rendering, DEADLINE),
                routable,
            )
            .expect("hold")
            .decision_id
    };
    let decide = |decision_id: &DecisionId, approve: bool| {
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
    };
    let permit = |decision_id: &DecisionId| {
        service
            .authorize_execution(decision_id, &operation_id(), &request_digest())
            .expect("permit")
    };

    // One row in every phase, the approval with and without its permit,
    // and every notice state, each reached through the owner's own calls.
    hold("phase:open", true);
    hold("phase:open-unroutable", false);
    let approved = hold("phase:approved", true);
    decide(&approved, true);
    let permitted = hold("phase:permitted", true);
    decide(&permitted, true);
    let _outstanding = permit(&permitted);
    let rejected = hold("phase:rejected", true);
    decide(&rejected, false);
    let delivered = hold("phase:rejected-delivered", true);
    decide(&delivered, false);
    service
        .settle_notice(
            &delivered,
            &ApprovalConsentDeliveryState::Admitted {
                session_id: SessionId::new(),
                input_id: "input-1".to_owned(),
            },
        )
        .expect("deliver the notice");
    let withdrawn = hold("phase:withdrawn", false);
    service
        .withdraw(&withdrawn, &DecisionWithdrawer::Host)
        .expect("withdraw");
    let executed = hold("phase:executed", true);
    decide(&executed, true);
    let executing = permit(&executed);
    service
        .record_executed(
            &executed,
            executing.operation_id(),
            executing.request_digest(),
        )
        .expect("executed");
    let refused = hold("phase:refused", true);
    decide(&refused, true);
    service
        .close_refused(permit(&refused), &reference("quota_exceeded"))
        .expect("refused");
    service
        .settle_notice(
            &refused,
            &ApprovalConsentDeliveryState::Failed {
                reason: "unreachable".to_owned(),
            },
        )
        .expect("the notice failed");
    let expired = hold("phase:expired", true);
    clock.set(DEADLINE);
    service.get(&expired).expect("the read records the expiry");
    clock.set(HELD_AT);

    let rows: Vec<StoredDecision> = store
        .inner
        .list(&DecisionListFilter::default(), None, 100)
        .expect("list")
        .rows
        .into_iter()
        .map(|row| match row {
            DecisionRow::Readable(row) => *row,
            DecisionRow::Unreadable(row) => panic!("unreadable row {row:?}"),
        })
        .collect();
    let phases: BTreeSet<&str> = rows.iter().map(|row| row.state.phase.as_str()).collect();
    assert_eq!(phases.len(), 7, "every phase: {phases:?}");
    let notices: BTreeSet<&str> = rows.iter().map(|row| row.state.notice.as_str()).collect();
    assert_eq!(notices.len(), 5, "every notice state: {notices:?}");
    for permit_issued in [false, true] {
        assert!(
            rows.iter()
                .any(|row| row.state.phase == DecisionPhase::Approved
                    && row.state.permit_issued == permit_issued),
            "an approval with permit_issued {permit_issued}"
        );
    }

    // The same rows in a fresh in-memory store and a fresh SQLite file.
    let memory = InMemoryDecisionStore::new();
    let dir = tempfile::tempdir().expect("tempdir");
    let sqlite = SqliteDecisionStore::in_store_path(dir.path());
    for row in &rows {
        memory.insert_if_absent(row).expect("insert");
        sqlite.insert_if_absent(row).expect("insert");
    }
    let ids = |page: DecisionPage| -> BTreeSet<String> {
        page.rows
            .iter()
            .map(|row| row.decision_id().to_owned())
            .collect()
    };

    // Owed exactly when the machine owes its owner work.
    let owed_in_memory = ids(memory.owed(None, 100).expect("owed"));
    let owed_in_sqlite = ids(sqlite.owed(None, 100).expect("owed"));
    for row in &rows {
        let id = row.decision_id.as_str();
        let owes = machine_owes_work(row);
        assert_eq!(decision_owes_work(row), owes, "{id}: decision_owes_work");
        assert_eq!(owed_in_memory.contains(id), owes, "{id}: in-memory owed");
        assert_eq!(owed_in_sqlite.contains(id), owes, "{id}: SQLite owed");
    }

    // Due exactly when the machine's Expire records the expiry: before,
    // at and after the deadline.
    for now_ms in [DEADLINE - 1, DEADLINE, DEADLINE + 1] {
        let due_in_memory = ids(memory.due(now_ms, None, 100).expect("due"));
        let due_in_sqlite = ids(sqlite.due(now_ms, None, 100).expect("due"));
        for row in &rows {
            let id = row.decision_id.as_str();
            let due = machine_expires(row, now_ms);
            assert_eq!(
                decision_is_due(row, now_ms),
                due,
                "{id} at {now_ms}: decision_is_due"
            );
            assert_eq!(
                due_in_memory.contains(id),
                due,
                "{id} at {now_ms}: in-memory due"
            );
            assert_eq!(
                due_in_sqlite.contains(id),
                due,
                "{id} at {now_ms}: SQLite due"
            );
        }
    }
}
