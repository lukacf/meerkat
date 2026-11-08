//! Rows of what an access offer binds and who decides it: offers whose
//! parent can never run them are refused at hold; the executor resolves the
//! parent and compares the branch's subject before it opens anything; the
//! branch's controller grant cannot be delegated; a person-only kind holds no
//! review stage; the use-time currentness adapter answers for the person who
//! decided; and the plan-time refusals of `access_offer`.

use std::time::Duration;

use meerkat_authorization::grants::{ApprovalUseAnswer, ConsumeUseRefusal, GrantUse};
use meerkat_core::approval::decision::{
    DecisionContractError, DecisionHoldRequest, DecisionKind, DecisionOperationId,
    DecisionPrincipalKey, DecisionSubjectKey,
};
use meerkat_core::authorization::{
    OperationAuthorizationError, OperationRefusalKind, OperationRefused,
    PreparedAuthorizationBinding, PreparedOperationAuthorization,
};
use meerkat_core::error::ToolError;
use meerkat_core::types::ToolCallView;
use meerkat_core::{
    AgentToolDispatcher, ToolDeadlineChain, ToolDeadlineContributor, ToolDeadlineOwner,
    ToolDispatchContext, ToolExecutionResolutionContext, ToolExecutionResolutionError,
    ToolUnavailableReason,
};

use super::*;

/// The JSON form of an access offer's refusal at hold.
fn refusal_json(result: Result<StoredDecision, AccessOfferError>) -> serde_json::Value {
    serde_json::to_value(refusal(result)).expect("a refusal encodes")
}

#[test]
fn access_offer_refuses_at_hold_an_offer_its_parent_cannot_run() {
    let start = real_now_ms();
    // The offering grant's window ends before the requested window would:
    // the decision could only ever end `terms_changed`.
    let fixture = Fixture::with(Terms {
        offering_lifetime: Some(LifetimeRestriction::window(start, start + 10 * MINUTE_MS)),
        ..Terms::default()
    });
    assert_eq!(
        refusal_json(fixture.offer(true)),
        serde_json::json!({"refusal": "window_over_parent"})
    );
    assert_eq!(fixture.decision_count(), 0, "nothing was held");
    // Its processors exclude the approval branch: every use would be denied.
    let fixture = Fixture::with(Terms {
        offering_processors: Some(ExactRestriction::exact([ProcessorRef::Principal {
            principal: agent("another-processor"),
        }])),
        ..Terms::default()
    });
    assert_eq!(
        refusal_json(fixture.offer(true)),
        serde_json::json!({"refusal": "outside_parent_bounds", "reason": "processor_excluded"})
    );
    assert_eq!(fixture.decision_count(), 0, "nothing was held");
    // Its audiences exclude the requester the answer goes to.
    let fixture = Fixture::with(Terms {
        offering_audiences: Some(ExactRestriction::exact([AudienceRef::Principal {
            principal: principal(PrincipalKind::Human, "another-audience"),
        }])),
        ..Terms::default()
    });
    assert_eq!(
        refusal_json(fixture.offer(true)),
        serde_json::json!({"refusal": "outside_parent_bounds", "reason": "audience_excluded"})
    );
    assert_eq!(fixture.decision_count(), 0, "nothing was held");
    // The control: a parent whose window covers the request is held.
    let fixture = Fixture::with(Terms {
        offering_lifetime: Some(LifetimeRestriction::window(start, start + 20 * MINUTE_MS)),
        ..Terms::default()
    });
    assert!(fixture.offer(true).is_ok());
}

/// An offer's decision is due by the offering grant's window end less the
/// requested window, the last moment its grant can still be issued with the
/// presented terms: an approval after that expires the decision, which
/// opens and issues nothing, instead of executing it into `terms_changed`.
#[tokio::test]
async fn an_offer_approved_after_its_parent_can_run_it_expires_instead() {
    let start = real_now_ms();
    let fixture = Fixture::with(Terms {
        offering_lifetime: Some(LifetimeRestriction::window(start, start + 20 * MINUTE_MS)),
        ..Terms::default()
    });
    let held = fixture.offer(true).expect("held");
    assert_eq!(
        held.descriptor.deadline_at_ms,
        start + 5 * MINUTE_MS,
        "the offering grant's window end less the requested 15 minutes"
    );
    // The approver answers after that.
    fixture.clock.advance(10 * MINUTE_MS);
    assert!(
        fixture.try_decide(&held, true).is_err(),
        "an approval past the deadline approves nothing"
    );
    let pass = fixture.executor_loop().pass().await;
    let stored = fixture.stored(&held.decision_id);
    assert_eq!(stored.state.phase, DecisionPhase::Expired, "{pass:?}");
    assert_eq!(stored.execution_refusal, None, "never terms_changed");
    assert!(fixture.branches.opened().is_empty(), "no branch was opened");
    assert_eq!(fixture.issued(approval_grant_id(&held.decision_id)), None);
    assert_eq!(
        fixture.issued(branch_controller_grant_id(&held.decision_id)),
        None
    );
}

#[tokio::test]
async fn the_executor_resolves_the_parent_lineage_before_it_opens_a_branch() {
    let fixture = Fixture::new();
    let held = fixture.offer(true).expect("held");
    fixture.decide(&held, true);
    fixture
        .owner
        .revoke(&agent("grant-root"), &fixture.offering[0], &mut AnyCustody)
        .expect("the offering grant is revoked after the approval");
    let pass = fixture.executor_loop().pass().await;
    assert!(
        pass.outcomes.contains(&(
            held.decision_id.clone(),
            DecisionPassOutcome::ExecutionRefused
        )),
        "{pass:?}"
    );
    assert!(fixture.branches.opened().is_empty(), "no branch was opened");
    assert_eq!(
        fixture.issued(branch_controller_grant_id(&held.decision_id)),
        None,
        "no controller grant was issued"
    );
    assert_eq!(
        fixture
            .stored(&held.decision_id)
            .execution_refusal
            .as_ref()
            .map(DecisionReference::as_str),
        Some("parent_refused")
    );
}

#[tokio::test]
async fn the_executor_refuses_a_branch_that_acts_for_another_subject_since_the_hold() {
    let fixture = Fixture::new();
    let held = fixture.offer(true).expect("held");
    fixture.decide(&held, true);
    // The asking agent now runs under a controller grant for another subject.
    let other = principal(PrincipalKind::Human, "another-subject");
    let mut delegable = ExecutionRestrictions::unrestricted();
    delegable.delegation_depth = DelegationDepth::remaining(1);
    let controller = fixture
        .owner
        .issue_root(
            &agent("grant-root"),
            id("data-agent-other-controller"),
            data_agent(),
            Some(other.clone()),
            delegable,
        )
        .expect("a controller grant for another subject");
    fixture.branches.can_branch(
        &data_agent(),
        ApprovalGranteeBasis::new(vec![controller], Some(other)),
    );
    let pass = fixture.executor_loop().pass().await;
    assert!(
        pass.outcomes.contains(&(
            held.decision_id.clone(),
            DecisionPassOutcome::ExecutionRefused
        )),
        "{pass:?}"
    );
    assert!(fixture.branches.opened().is_empty(), "no branch was opened");
    assert_eq!(
        fixture.issued(branch_controller_grant_id(&held.decision_id)),
        None
    );
    assert_eq!(fixture.issued(approval_grant_id(&held.decision_id)), None);
    assert_eq!(
        fixture
            .stored(&held.decision_id)
            .execution_refusal
            .as_ref()
            .map(DecisionReference::as_str),
        Some("approval_branch_unavailable")
    );
}

#[tokio::test]
async fn the_branch_controller_grant_has_depth_zero_under_a_deeper_controller_grant() {
    let fixture = Fixture::with(Terms {
        controller_depth: 3,
        ..Terms::default()
    });
    let decision = executed(&fixture).await;
    let (lineage, branch) = branch_controller_lineage(&fixture, &decision);
    let resolved = fixture
        .owner
        .resolve_controller_lineage(&lineage, &branch, None)
        .expect("a current controller lineage of the branch");
    assert_eq!(
        resolved.restrictions().delegation_depth,
        DelegationDepth::remaining(0),
        "the branch delegates nothing, whatever its grantee's controller grant allows"
    );
}

#[test]
fn access_offer_refuses_a_parent_that_is_no_longer_current() {
    let no_longer_current = refused(
        AccessOfferRefusalKind::NoChildApprovalRule,
        Some(AccessOfferRefusalReason::ParentNotCurrent),
    );
    // Revoked.
    let fixture = Fixture::new();
    fixture
        .owner
        .revoke(&agent("grant-root"), &fixture.offering[0], &mut AnyCustody)
        .expect("the offering grant is revoked");
    assert_eq!(refusal(fixture.offer(true)), no_longer_current);
    assert_eq!(fixture.decision_count(), 0, "nothing was held");
    // Expired.
    let start = real_now_ms();
    let fixture = Fixture::with(Terms {
        offering_lifetime: Some(LifetimeRestriction::window(start, start + MINUTE_MS)),
        ..Terms::default()
    });
    fixture.clock.advance(2 * MINUTE_MS);
    assert_eq!(refusal(fixture.offer(true)), no_longer_current);
    assert_eq!(fixture.decision_count(), 0, "nothing was held");
}

#[test]
fn a_person_only_kind_refuses_a_review_stage_at_hold() {
    let fixture = Fixture::new();
    let held = fixture.offer(true).expect("held");
    let descriptor = &held.descriptor;
    let reviewer =
        DecisionPrincipalKey::of(&principal(PrincipalKind::SharedAgent, "reviewer-agent"))
            .expect("the reviewer's key");
    let hold = |kind: &str| {
        DecisionHoldRequest::new(
            DecisionKind::new(kind).expect("kind"),
            DecisionSubjectKey::new("another-subject").expect("subject key"),
            DecisionOperationId::new(format!("{kind}:another-operation")).expect("operation id"),
            descriptor.request_digest.clone(),
            descriptor.request.clone(),
            descriptor.requester.clone(),
            descriptor.approver_id.clone(),
            descriptor.deadline_at_ms,
            descriptor.rendering.clone(),
            descriptor.renderer.clone(),
        )
        .with_reviewer(reviewer.clone())
    };
    let refused = fixture
        .decisions
        .hold(hold(GRANT_CHILD_KIND), true)
        .expect_err("a grant.child decision is decided by people only");
    assert!(
        matches!(
            &refused,
            crate::decision::DecisionError::Invalid(DecisionContractError::ReviewStageNotAllowed {
                kind
            }) if kind.as_str() == GRANT_CHILD_KIND
        ),
        "{refused:?}"
    );
    assert!(
        refused.to_string().contains("review_stage_not_allowed"),
        "{refused}"
    );
    assert_eq!(fixture.decision_count(), 1, "nothing more was held");
    // A kind reviewers may decide keeps its review stage.
    assert!(fixture.decisions.hold(hold("release"), true).is_ok());
}

/// A reviewer's approval of a person-only kind never executes: a row that
/// names a reviewer (written before the hold refused one, or by another
/// writer of the store) is approved by the reviewer, and the executor
/// refuses it `not_a_human_decision` before it opens or issues anything.
#[tokio::test]
async fn a_reviewer_approval_of_a_person_only_kind_is_refused_at_execution() {
    let store = Arc::new(InMemoryDecisionStore::new());
    let fixture = Fixture::with(Terms {
        store: Some(store.clone()),
        ..Terms::default()
    });
    let held = fixture.offer(true).expect("held");
    let reviewer = principal(PrincipalKind::SharedAgent, "reviewer-agent");
    let reviewer_key = DecisionPrincipalKey::of(&reviewer).expect("the reviewer's key");
    let mut row = held.clone();
    row.descriptor.reviewer = Some(reviewer_key.clone());
    row.state.reviewer = Some(reviewer_key.as_str().to_owned());
    row.state.stage = meerkat_core::approval::decision::DecisionStage::Review;
    row.revision = held.revision + 1;
    store
        .replace_if_revision(held.revision, &row)
        .expect("the row names a reviewer");
    let rendering = &held.descriptor.rendering;
    fixture
        .decisions
        .present(
            &held.decision_id,
            &PresentationRecord::complete(
                PresentationAudience::Reviewer {
                    reviewer: reviewer_key,
                },
                rendering,
                None,
                reference("chat"),
                reference("message-1"),
                fixture.clock.now(),
            ),
        )
        .expect("presented to the reviewer");
    let review = meerkat_core::approval::decision::AttributedReview::new(
        reviewer,
        meerkat_core::SessionId::new(),
        reference("verdict-call"),
        meerkat_core::approval::decision::DecisionReviewVerdict::Allow,
        rendering.subject_digest(),
    )
    .expect("the reviewer's verdict");
    let approved = fixture
        .decisions
        .review(&held.decision_id, &review)
        .expect("the reviewer approves");
    assert_eq!(approved.state.phase, DecisionPhase::Approved);

    let pass = fixture.executor_loop().pass().await;
    assert!(
        pass.outcomes.contains(&(
            held.decision_id.clone(),
            DecisionPassOutcome::ExecutionRefused
        )),
        "{pass:?}"
    );
    let refused = fixture.stored(&held.decision_id);
    assert_eq!(refused.state.phase, DecisionPhase::ExecutionRefused);
    assert_eq!(
        refused
            .execution_refusal
            .as_ref()
            .map(DecisionReference::as_str),
        Some("not_a_human_decision")
    );
    assert!(fixture.branches.opened().is_empty(), "no branch was opened");
    assert_eq!(fixture.issued(approval_grant_id(&held.decision_id)), None);
    assert_eq!(
        fixture.issued(branch_controller_grant_id(&held.decision_id)),
        None
    );
    assert_eq!(
        refused.state.cleanup,
        DecisionCleanup::NotOwed,
        "nothing was opened that a cleanup would undo"
    );
}

/// The host's approvers, answering what the row sets for every decider.
struct ScriptedApprovers(Mutex<Scripted>);

#[derive(Debug, Clone, Copy)]
enum Scripted {
    Current,
    Refused,
    Stale,
    Unsettled,
}

struct Stale;

impl ConsentEligibility for Stale {
    fn check_current(&self) -> Result<(), ConsentIneligible> {
        Err(ConsentIneligible)
    }
}

impl ScriptedApprovers {
    fn new() -> Arc<Self> {
        Arc::new(Self(Mutex::new(Scripted::Current)))
    }

    fn answer(&self, answer: Scripted) {
        *self.0.lock().expect("answer") = answer;
    }
}

impl DecisionApproverCurrentness for ScriptedApprovers {
    fn approver_answer(&self, _decision: &StoredDecision) -> ConsentEligibilityAnswer {
        match *self.0.lock().expect("answer") {
            Scripted::Current => ConsentEligibilityAnswer::Eligible(Arc::new(Eligible)),
            Scripted::Refused => ConsentEligibilityAnswer::Refused,
            Scripted::Stale => ConsentEligibilityAnswer::Eligible(Arc::new(Stale)),
            Scripted::Unsettled => ConsentEligibilityAnswer::Unsettled,
        }
    }
}

#[tokio::test]
async fn the_use_time_currentness_answers_for_the_person_who_decided() {
    let approvers = ScriptedApprovers::new();
    let fixture = Fixture::with(Terms {
        use_approvers: Some(approvers.clone()),
        ..Terms::default()
    });
    let decision = executed(&fixture).await;
    let approval_ref = fixture
        .issued(approval_grant_id(&decision))
        .expect("the approval-carried grant");
    let approval = fixture
        .owner
        .retained_grant(&approval_ref)
        .expect("retained")
        .approval
        .expect("it carries the approval");
    let adapter = DecisionApprovalUseCurrentness::new(fixture.decisions.clone(), approvers.clone());
    use meerkat_authorization::grants::ApprovalUseCurrentness as _;
    for (scripted, answer) in [
        (Scripted::Current, ApprovalUseAnswer::Current),
        (Scripted::Refused, ApprovalUseAnswer::NotCurrent),
        (Scripted::Stale, ApprovalUseAnswer::Unsettled),
        (Scripted::Unsettled, ApprovalUseAnswer::Unsettled),
    ] {
        approvers.answer(scripted);
        assert_eq!(adapter.approver_answer(&approval), answer, "{scripted:?}");
    }
    approvers.answer(Scripted::Current);
    let mut another_approver = approval.clone();
    another_approver.approver_id = ApproverId::new("another-approver").expect("approver id");
    assert_eq!(
        adapter.approver_answer(&another_approver),
        ApprovalUseAnswer::NotCurrent,
        "a decision held for another approver id is not this approval's"
    );
    let mut unknown = approval.clone();
    unknown.decision_id = DecisionId::derive(
        &DecisionKind::new(GRANT_CHILD_KIND).expect("kind"),
        &DecisionSubjectKey::new("a-decision-never-held").expect("subject key"),
    );
    assert_eq!(
        adapter.approver_answer(&unknown),
        ApprovalUseAnswer::Unsettled,
        "a decision that cannot be read is unsettled"
    );
    // At the spend: a refused decider denies it, an unsettled answer is
    // unavailable, and neither spends the one use.
    let request = GrantChildRequest::of(&fixture.stored(&decision)).expect("request");
    let mut lineage = fixture.offering.clone();
    lineage.push(approval_ref);
    let branch = approval_branch_principal(&data_agent(), &decision).expect("branch");
    let requester = requester();
    let spend = || {
        fixture.owner.spend_use_at_entry(
            &lineage,
            &branch,
            None,
            GrantUse {
                requester: &requester,
                work: &request.terms.work,
                operation: request.terms.operation.as_ref(),
            },
        )
    };
    approvers.answer(Scripted::Refused);
    assert_eq!(spend(), Err(ConsumeUseRefusal::ApproverNotCurrent));
    approvers.answer(Scripted::Unsettled);
    assert_eq!(spend(), Err(ConsumeUseRefusal::Unavailable));
    approvers.answer(Scripted::Current);
    assert_eq!(spend(), Ok(()), "the one use was still there");
    assert!(spend().is_err(), "and is spent now");
}

/// A work owner that prepares nothing: these rows never reach an operation.
struct NoOperations;

impl meerkat_core::WorkAuthorization for NoOperations {
    fn prepare(
        &self,
        _binding: &PreparedAuthorizationBinding,
    ) -> Result<Arc<dyn PreparedOperationAuthorization>, OperationAuthorizationError> {
        Err(OperationRefused::new(OperationRefusalKind::Denied).into())
    }
}

/// A dispatch in the owner's agent's governed peer work, whose basis selects
/// the offering grant for the records server's operations.
fn governed(fixture: &Fixture) -> ToolDispatchContext {
    let basis = WorkAuthorityBasis::TargetedGrantLineages {
        base: None,
        targets: vec![TargetGrantLineage {
            target: GrantTarget::McpServer {
                server: SERVER.into(),
            },
            lineage: fixture.offering.clone(),
        }],
    };
    let association = association(true, basis);
    let origin = GovernedPeerOrigin::from_associations([&association]).expect("origin");
    let work = meerkat_core::WorkAuthorizationContext::new(
        Arc::new(NoOperations),
        meerkat_core::OperationExecutionScope::Domain,
    )
    .with_governed_send_origin(Some(origin.to_send_origin()));
    ToolDispatchContext::default().with_work_authorization(Some(work))
}

fn offer_arguments(server: &str) -> Box<serde_json::value::RawValue> {
    serde_json::value::to_raw_value(&serde_json::json!({
        "operation": {
            "server": server,
            "tool": TOOL,
            "arguments": {"query": "revenue by region, last quarter"},
        },
        "summary": "the requester asks for last quarter's revenue by region",
    }))
    .expect("arguments")
}

pub(super) fn resolution() -> ToolExecutionResolutionContext {
    ToolExecutionResolutionContext::new(
        ToolDeadlineChain::new(vec![ToolDeadlineContributor::finite(
            ToolDeadlineOwner::CoreToolDispatch,
            Duration::from_mins(10),
        )])
        .expect("deadline chain"),
    )
}

#[tokio::test]
async fn access_offer_refuses_at_plan_time_with_the_payload_of_dispatch_time() {
    let fixture = Fixture::new();
    let tools = DecisionsToolDispatcher::new(fixture.grants.clone());
    // Outside governed work.
    let records = offer_arguments(SERVER);
    let call = ToolCallView {
        id: "offer-1",
        name: ACCESS_OFFER_TOOL,
        args: &records,
    };
    let outside = ToolDispatchContext::default();
    let plan_time = ToolError::from(
        tools
            .resolve_execution_plan(call, &outside, &resolution())
            .expect_err("no plan outside governed work"),
    )
    .to_error_payload();
    let dispatch_time = tools
        .dispatch_with_context(call, &outside)
        .await
        .expect_err("nothing runs outside governed work")
        .to_error_payload();
    assert_eq!(plan_time, dispatch_time);
    assert_eq!(plan_time["error"], "execution_failed");
    assert_eq!(
        plan_time["data"],
        serde_json::json!({"refusal": "not_governed_work"})
    );
    // In governed work, for a server no grant of the work covers.
    let context = governed(&fixture);
    let plan = tools
        .resolve_execution_plan(call, &context, &resolution())
        .expect("the records offer resolves its plan");
    let ledger = offer_arguments("ledger");
    let call = ToolCallView {
        id: "offer-2",
        name: ACCESS_OFFER_TOOL,
        args: &ledger,
    };
    let plan_time = ToolError::from(
        tools
            .resolve_execution_plan(call, &context, &resolution())
            .expect_err("no plan without an offering grant"),
    )
    .to_error_payload();
    let dispatch_time = tools
        .dispatch_resolved_with_context(call, &context, &plan)
        .await
        .expect_err("nothing runs without an offering grant")
        .to_error_payload();
    assert_eq!(plan_time, dispatch_time);
    assert_eq!(plan_time["error"], "execution_failed");
    assert_eq!(
        plan_time["data"],
        serde_json::json!({"refusal": "no_child_approval_rule", "reason": "no_parent_grant"})
    );
    assert_eq!(fixture.decision_count(), 0, "nothing was held");
}

#[test]
fn access_offer_answers_an_unavailable_grant_owner_retryably_at_plan_time() {
    let journal = Arc::new(UncertainJournal::default());
    let fixture = Fixture::with(Terms {
        grant_store: Some(journal.clone()),
        ..Terms::default()
    });
    let tools = DecisionsToolDispatcher::new(fixture.grants.clone());
    let context = governed(&fixture);
    journal.uncertain.store(true, Ordering::SeqCst);
    assert!(
        fixture
            .owner
            .issue_root(
                &agent("grant-root"),
                id("unrelated-grant"),
                owner_agent(),
                None,
                ExecutionRestrictions::unrestricted(),
            )
            .is_err(),
        "the grant owner is unavailable until it restarts"
    );
    let records = offer_arguments(SERVER);
    let call = ToolCallView {
        id: "offer-1",
        name: ACCESS_OFFER_TOOL,
        args: &records,
    };
    assert_eq!(
        tools
            .resolve_execution_plan(call, &context, &resolution())
            .expect_err("no plan while the grant owner cannot say"),
        ToolExecutionResolutionError::Unavailable {
            tool_name: ACCESS_OFFER_TOOL.into(),
            reason: ToolUnavailableReason::TemporarilyUnavailable,
        }
    );
}
