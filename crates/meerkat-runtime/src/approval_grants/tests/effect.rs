//! Rows of an executed decision's effect on the decision wire: an executed
//! `grant.child` decision names the approval-carried grant its execution
//! issued, a decision that is not executed names none, and a decision of
//! another kind names none.

use meerkat_core::approval::decision::{
    DecisionHoldRequest, DecisionKind, DecisionListFilter, DecisionOperationId,
    DecisionPrincipalKey, DecisionRenderer, DecisionRendering, DecisionRequestBody,
    DecisionSubjectKey, Sha256Digest,
};

use super::*;
use crate::decision::wire::{wire_decision, wire_decision_listing};
use crate::decision::{
    DecisionExecution, DecisionExecutionPermit, DecisionExecutorRecordUnavailable,
    DecisionWithdrawer,
};

/// The decision the wire answers for `row`: the `decision` of a
/// `decision/get` result.
fn wire(row: &StoredDecision) -> serde_json::Value {
    serde_json::to_value(wire_decision(row, false)).expect("the decision encodes")
}

/// `row` stands in `phase`, and the decision the wire answers for it names
/// no effect.
fn assert_no_effect(row: &StoredDecision, phase: DecisionPhase) {
    assert_eq!(row.state.phase, phase);
    let read = wire(row);
    assert!(
        read.get("effect").is_none(),
        "a {phase:?} decision names no effect: {read}"
    );
}

/// The host's approvers once the person who decided was revoked.
struct Revoked;

impl DecisionApproverCurrentness for Revoked {
    fn approver_answer(&self, _decision: &StoredDecision) -> ConsentEligibilityAnswer {
        ConsentEligibilityAnswer::Refused
    }
}

/// The owner of another decision kind's held requests: it commits every
/// execution and keeps no record of its own.
struct OtherKindOwner(DecisionKind);

#[async_trait]
impl DecisionExecutor for OtherKindOwner {
    fn kind(&self) -> &DecisionKind {
        &self.0
    }

    async fn execute(
        &self,
        _decision: &StoredDecision,
        _permit: &DecisionExecutionPermit,
    ) -> DecisionExecution {
        DecisionExecution::Committed
    }

    async fn committed(
        &self,
        _decision: &StoredDecision,
    ) -> Result<bool, DecisionExecutorRecordUnavailable> {
        Ok(false)
    }

    async fn deliver_notice(
        &self,
        _decision: &StoredDecision,
    ) -> Option<ApprovalConsentDeliveryState> {
        None
    }

    async fn sweep(&self) -> Vec<(DecisionId, DecisionPassOutcome)> {
        Vec::new()
    }
}

/// A hold of `kind` for the fixture's approver, asked by the requester.
fn hold_of(fixture: &Fixture, kind: &DecisionKind) -> DecisionHoldRequest {
    DecisionHoldRequest::new(
        kind.clone(),
        DecisionSubjectKey::new("tool:query").expect("subject key"),
        DecisionOperationId::new("issue:tool:query").expect("operation id"),
        Sha256Digest::of(br#"{"tool":"query"}"#),
        DecisionRequestBody::from_value(&serde_json::json!({"tool": "query"}))
            .expect("request body"),
        DecisionPrincipalKey::of(&requester()).expect("requester key"),
        approver(),
        fixture.clock.now() + 60 * MINUTE_MS,
        DecisionRendering::new("Grant the tool query to the requester.").expect("rendering"),
        DecisionRenderer::new("render.v1").expect("renderer"),
    )
}

/// An executed `grant.child` decision names the grant its execution issued:
/// the decision the wire answers carries `effect.grant_id`, the id of the
/// approval-carried grant, which the grant owner resolves to the grant that
/// carries this decision's approval. A listing names the same effect.
#[tokio::test]
async fn an_executed_grant_child_names_the_grant_it_issued() {
    let fixture = Fixture::new();
    let decision = executed(&fixture).await;
    let read = wire(&fixture.stored(&decision));
    assert_eq!(read["phase"], "executed");
    let issued_id = approval_grant_id(&decision).expect("the approval-carried grant's id");
    assert_eq!(
        read["effect"]["grant_id"].as_str(),
        Some(issued_id.as_str()),
        "the effect names the approval-carried grant: {read}"
    );
    let named = EvidenceId::new(
        read["effect"]["grant_id"]
            .as_str()
            .expect("a grant id")
            .to_owned(),
    )
    .expect("an evidence id");
    let reference = fixture
        .owner
        .issued_reference(&named)
        .expect("the grant owner answers")
        .expect("the grant the effect names was issued");
    let retained = fixture
        .owner
        .retained_grant(&reference)
        .expect("the grant owner retains the grant");
    assert_eq!(
        retained.approval.map(|approval| approval.decision_id),
        Some(decision.clone()),
        "the named grant carries this decision's approval"
    );
    let listing = fixture
        .decisions
        .list(&DecisionListFilter::default(), None, 10)
        .expect("a page of decisions");
    let page = serde_json::to_value(wire_decision_listing(&listing)).expect("the page encodes");
    assert_eq!(page["decisions"][0]["decision_id"], read["decision_id"]);
    assert_eq!(page["decisions"][0]["effect"], read["effect"]);
}

/// A `grant.child` decision names no effect until it is recorded executed:
/// not while it is open or approved, not once its grant was issued under a
/// permit the decision has not settled yet, and never when it ends
/// rejected, expired, withdrawn or refused. The record that it executed
/// brings its effect.
#[tokio::test]
async fn a_grant_child_names_its_effect_only_once_executed() {
    let fixture = Fixture::new();
    let held = fixture.offer(true).expect("held");
    let decision = held.decision_id.clone();
    assert_no_effect(&fixture.stored(&decision), DecisionPhase::Open);
    let approved = fixture.decide(&held, true);
    assert_no_effect(&fixture.stored(&decision), DecisionPhase::Approved);
    // An executor issues the grant, then stops before the decision records
    // that it executed.
    let permit = fixture
        .decisions
        .authorize_execution(
            &decision,
            &approved.descriptor.operation_id,
            &approved.descriptor.request_digest,
            &ConsentEligibilityAnswer::Eligible(Arc::new(Eligible)),
        )
        .expect("a permit");
    let execution = GrantDecisionExecutor::new(fixture.grants.clone())
        .execute(&fixture.stored(&decision), &permit)
        .await;
    assert_eq!(execution, DecisionExecution::Committed);
    drop(permit);
    assert!(
        fixture.issued(approval_grant_id(&decision)).is_some(),
        "the grant was issued"
    );
    assert_no_effect(&fixture.stored(&decision), DecisionPhase::Approved);
    // The next pass settles the landed commit: the decision is executed.
    let pass = fixture.executor_loop().pass().await;
    assert!(
        pass.outcomes
            .contains(&(decision.clone(), DecisionPassOutcome::SettledExecuted)),
        "{pass:?}"
    );
    let read = wire(&fixture.stored(&decision));
    assert_eq!(read["phase"], "executed");
    assert_eq!(
        read["effect"]["grant_id"].as_str(),
        approval_grant_id(&decision)
            .as_ref()
            .map(EvidenceId::as_str),
        "the executed decision names its grant: {read}"
    );

    // Rejected.
    let fixture = Fixture::new();
    let held = fixture.offer(true).expect("held");
    fixture.decide(&held, false);
    assert_no_effect(&fixture.stored(&held.decision_id), DecisionPhase::Rejected);

    // Expired: its deadline passed undecided.
    let fixture = Fixture::new();
    let held = fixture.offer(true).expect("held");
    fixture.clock.advance(
        held.descriptor
            .deadline_at_ms
            .saturating_sub(fixture.clock.now())
            + 1,
    );
    assert_no_effect(&fixture.stored(&held.decision_id), DecisionPhase::Expired);

    // Withdrawn by the host.
    let fixture = Fixture::new();
    let held = fixture.offer(true).expect("held");
    let withdrawn = fixture
        .decisions
        .withdraw(&held.decision_id, &DecisionWithdrawer::Host)
        .expect("withdrawn");
    assert_no_effect(&withdrawn, DecisionPhase::Withdrawn);

    // Refused: the person who approved it was revoked before it executed.
    let fixture = Fixture::new();
    let held = fixture.offer(true).expect("held");
    fixture.decide(&held, true);
    let mut executor = DecisionExecutorLoop::start(
        fixture.decisions.clone(),
        [fixture.grants.executor()],
        Arc::new(Revoked),
    )
    .expect("the executor loop");
    let pass = executor.pass().await;
    assert!(
        pass.outcomes.contains(&(
            held.decision_id.clone(),
            DecisionPassOutcome::ApproverNotCurrent
        )),
        "{pass:?}"
    );
    assert_no_effect(
        &fixture.stored(&held.decision_id),
        DecisionPhase::ExecutionRefused,
    );
    assert_eq!(fixture.issued(approval_grant_id(&held.decision_id)), None);
}

/// The effect is the `grant.child` kind's: a decision of another kind,
/// executed in the same pass over the same store, names none.
#[tokio::test]
async fn an_executed_decision_of_another_kind_names_no_effect() {
    let fixture = Fixture::new();
    let offer = fixture.offer(true).expect("held");
    fixture.decide(&offer, true);
    let kind = DecisionKind::new("tool_grant.issue").expect("kind");
    let other = fixture
        .decisions
        .hold(hold_of(&fixture, &kind), false)
        .expect("held");
    fixture.decide(&other, true);
    let owner: Arc<dyn DecisionExecutor> = Arc::new(OtherKindOwner(kind));
    let mut executor = DecisionExecutorLoop::start(
        fixture.decisions.clone(),
        [fixture.grants.executor(), owner],
        Arc::new(Approvers),
    )
    .expect("the executor loop");
    let pass = executor.pass().await;
    for decision in [&offer.decision_id, &other.decision_id] {
        assert!(
            pass.outcomes
                .contains(&(decision.clone(), DecisionPassOutcome::Executed)),
            "{pass:?}"
        );
    }
    let other_read = wire(&fixture.stored(&other.decision_id));
    assert_eq!(other_read["phase"], "executed");
    assert!(
        other_read.get("effect").is_none(),
        "another kind names no effect: {other_read}"
    );
    let offer_read = wire(&fixture.stored(&offer.decision_id));
    assert_eq!(
        offer_read["effect"]["grant_id"].as_str(),
        approval_grant_id(&offer.decision_id)
            .as_ref()
            .map(EvidenceId::as_str),
        "the executed grant.child in the same pass names its grant: {offer_read}"
    );
}
