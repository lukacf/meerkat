//! Entered governed operations: the typed witness of one dispatch's entered
//! effect. Its steps never spend and never review; a reviewed entry's steps
//! are admitted by the generated review owner only while the admitting
//! dispatch is live, and the review stays in use until the last step
//! admitted before the dispatch end has finished. A step's refusal never
//! claims that the call did not enter.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use crate::approval::ApprovalService;
use crate::approval::review::{
    BoundOperationReview, OperationReviewObserver, OperationReviewer, ReviewCandidate,
    ReviewDispatchGuard, ReviewEntry, ReviewEntrySupport, ReviewObservation, ReviewUnavailableKind,
    ReviewUnsatisfiedKind, ReviewVerdict, ReviewerFailure, admit_operation_review,
};
use crate::approval::{ApprovalId, ApprovalRecord, ApprovalStore, ApprovalStoreError};
use crate::authorization::{
    AuthorizationOperation, OperationAuthorizationError, OperationAuthorizationFacts,
    OperationObservation, OperationObservationError, OperationRefusalKind, OperationRefused,
    OperationReviewTier, OperationValidity, PreparedAuthorizationBinding,
    PreparedOperationAuthorization, PreparedOperationCheck, ReviewOperationContextResolution,
    ToolAuthorizationFacts, ToolAuthorizationTarget, WorkAuthorization,
};
use crate::generated::approval_lifecycle::ReviewAttemptStatus;
use crate::{ToolDispatchContext, ToolError};

/// A work owner for entered steps. Its decisions carry its current tier and
/// stay current until that tier or its publication sequence changes (either
/// change re-prepares). Its entered
/// steps record the entered tier they were given, answer a scripted refusal
/// once, refuse a decision stricter than the entered tier, and otherwise run
/// the operation or issue the `u32` intent; its plain path runs only at R1.
/// While another actor holds its exclusion (`held`) every step, plain or
/// entered, refuses to rebuild without waiting and the hold is named as an
/// owner change in progress, as the native work owner does.
struct Owner {
    tier: parking_lot::Mutex<OperationReviewTier>,
    sequence: AtomicU64,
    refuse_next: parking_lot::Mutex<Option<OperationAuthorizationError>>,
    entered_tiers: parking_lot::Mutex<Vec<OperationReviewTier>>,
    entered_runs: AtomicUsize,
    plain_runs: AtomicUsize,
    issued: parking_lot::Mutex<Vec<u32>>,
    held: AtomicBool,
    /// Steps refused because `held` was set.
    contended: AtomicUsize,
    /// Steps still to refuse to rebuild with no change in progress named
    /// (a holder that finished before the hint was read).
    unnamed_rebuilds: AtomicUsize,
    /// The next entered run's answer after its operation ran.
    refuse_after_run: parking_lot::Mutex<Option<OperationAuthorizationError>>,
    /// Runs at every current check of one of this owner's decisions.
    on_check: parking_lot::Mutex<Option<Box<dyn FnMut() + Send>>>,
}

impl Owner {
    fn new(tier: OperationReviewTier) -> Arc<Self> {
        Arc::new(Self {
            tier: parking_lot::Mutex::new(tier),
            sequence: AtomicU64::new(0),
            refuse_next: parking_lot::Mutex::new(None),
            entered_tiers: parking_lot::Mutex::new(Vec::new()),
            entered_runs: AtomicUsize::new(0),
            plain_runs: AtomicUsize::new(0),
            issued: parking_lot::Mutex::new(Vec::new()),
            held: AtomicBool::new(false),
            contended: AtomicUsize::new(0),
            unnamed_rebuilds: AtomicUsize::new(0),
            refuse_after_run: parking_lot::Mutex::new(None),
            on_check: parking_lot::Mutex::new(None),
        })
    }

    fn tier(&self) -> OperationReviewTier {
        *self.tier.lock()
    }

    fn set_tier(&self, tier: OperationReviewTier) {
        *self.tier.lock() = tier;
    }

    /// An owner change that keeps the tier: every decision re-prepares.
    fn refresh(&self) {
        self.sequence.fetch_add(1, Ordering::SeqCst);
    }

    /// This owner's exclusion, taken without waiting: refused to rebuild
    /// while another actor holds it, or while an unnamed rebuild is due.
    fn exclusion(&self) -> Result<(), OperationAuthorizationError> {
        if self.held.load(Ordering::SeqCst) {
            self.contended.fetch_add(1, Ordering::SeqCst);
            return Err(OperationRefused::new(OperationRefusalKind::ReprepareRequired).into());
        }
        if self
            .unnamed_rebuilds
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                left.checked_sub(1)
            })
            .is_ok()
        {
            return Err(OperationRefused::new(OperationRefusalKind::ReprepareRequired).into());
        }
        Ok(())
    }

    /// One entered step under this owner's own exclusion.
    fn step(&self, entered_tier: OperationReviewTier) -> Result<(), OperationAuthorizationError> {
        self.entered_tiers.lock().push(entered_tier);
        if let Some(error) = self.refuse_next.lock().take() {
            return Err(error);
        }
        self.exclusion()?;
        if self.tier() > entered_tier {
            return Err(OperationRefused::new(OperationRefusalKind::Denied).into());
        }
        Ok(())
    }
}

struct Decision {
    owner: Arc<Owner>,
    tier: OperationReviewTier,
    sequence: u64,
}

impl PreparedOperationAuthorization for Decision {
    fn review_tier(&self) -> OperationReviewTier {
        self.tier
    }

    fn check_current(
        &self,
        _binding: &PreparedAuthorizationBinding,
    ) -> Result<(), OperationAuthorizationError> {
        if let Some(hook) = self.owner.on_check.lock().as_mut() {
            hook();
        }
        if self.owner.tier() == self.tier
            && self.owner.sequence.load(Ordering::SeqCst) == self.sequence
        {
            Ok(())
        } else {
            Err(OperationRefused::new(OperationRefusalKind::ReprepareRequired).into())
        }
    }

    fn observe(
        &self,
        _binding: &PreparedAuthorizationBinding,
        _event: OperationObservation,
    ) -> Result<(), OperationObservationError> {
        Ok(())
    }
}

struct OwnerWork(Arc<Owner>);

impl WorkAuthorization for OwnerWork {
    /// The hold of this owner's exclusion, named while it lasts.
    fn owner_change_in_progress(&self) -> Option<crate::authorization::OwnerChangeInProgress> {
        self.0
            .held
            .load(Ordering::SeqCst)
            .then(|| crate::authorization::OwnerChangeInProgress::new(uuid::Uuid::nil(), 1))
    }

    fn prepare(
        &self,
        _binding: &PreparedAuthorizationBinding,
    ) -> Result<Arc<dyn PreparedOperationAuthorization>, OperationAuthorizationError> {
        Ok(Arc::new(Decision {
            owner: Arc::clone(&self.0),
            tier: self.0.tier(),
            sequence: self.0.sequence.load(Ordering::SeqCst),
        }))
    }

    /// The fixture work names no action account and no mandate.
    fn review_operation_context(
        &self,
        _binding: &PreparedAuthorizationBinding,
    ) -> Result<ReviewOperationContextResolution, OperationAuthorizationError> {
        Ok(ReviewOperationContextResolution::NotApplicable)
    }

    fn run_if_current_within(
        &self,
        _binding: &PreparedAuthorizationBinding,
        _prepared: Option<&OperationValidity>,
        _further: &(dyn std::any::Any + Send + Sync),
        operation: &mut dyn FnMut(),
    ) -> Result<(), OperationAuthorizationError> {
        if self.0.tier() != OperationReviewTier::R1 {
            return Err(OperationRefused::new(OperationRefusalKind::Denied).into());
        }
        self.0.exclusion()?;
        self.0.plain_runs.fetch_add(1, Ordering::SeqCst);
        operation();
        Ok(())
    }

    fn run_entered_if_current_within(
        &self,
        _binding: &PreparedAuthorizationBinding,
        _prepared: Option<&OperationValidity>,
        _further: &(dyn std::any::Any + Send + Sync),
        operation: &mut dyn FnMut(),
        entered_tier: OperationReviewTier,
    ) -> Result<(), OperationAuthorizationError> {
        self.0.step(entered_tier)?;
        self.0.entered_runs.fetch_add(1, Ordering::SeqCst);
        operation();
        match self.0.refuse_after_run.lock().take() {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    fn validate_child_issuance(
        &self,
        _further: &(dyn std::any::Any + Send + Sync),
        intent: &(dyn std::any::Any + Send + Sync),
    ) -> Result<(), OperationAuthorizationError> {
        intent
            .downcast_ref::<u32>()
            .map(|_| ())
            .ok_or_else(|| OperationRefused::new(OperationRefusalKind::MalformedFacts).into())
    }

    fn issue_child_if_current_within(
        &self,
        _binding: &PreparedAuthorizationBinding,
        _prepared: Option<&OperationValidity>,
        _further: &(dyn std::any::Any + Send + Sync),
        intent: &(dyn std::any::Any + Send + Sync),
        entered_tier: OperationReviewTier,
    ) -> Result<Box<dyn std::any::Any + Send>, OperationAuthorizationError> {
        self.0.step(entered_tier)?;
        let intent = *intent.downcast_ref::<u32>().expect("validated");
        self.0.issued.lock().push(intent);
        Ok(Box::new(intent))
    }
}

/// The same owner without the entered step: the trait's default answers.
struct PlainOnlyWork(Arc<Owner>);

impl WorkAuthorization for PlainOnlyWork {
    fn prepare(
        &self,
        _binding: &PreparedAuthorizationBinding,
    ) -> Result<Arc<dyn PreparedOperationAuthorization>, OperationAuthorizationError> {
        Ok(Arc::new(Decision {
            owner: Arc::clone(&self.0),
            tier: self.0.tier(),
            sequence: self.0.sequence.load(Ordering::SeqCst),
        }))
    }

    fn review_operation_context(
        &self,
        _binding: &PreparedAuthorizationBinding,
    ) -> Result<ReviewOperationContextResolution, OperationAuthorizationError> {
        Ok(ReviewOperationContextResolution::NotApplicable)
    }
}

/// A model reviewer that allows every candidate and counts its reviews.
#[derive(Default)]
struct Reviewer(AtomicUsize);

#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
impl OperationReviewer for Reviewer {
    async fn review(
        &self,
        _candidate: &ReviewCandidate<'_>,
    ) -> Result<ReviewVerdict, ReviewerFailure> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(ReviewVerdict::Allow)
    }
}

/// The review statuses the host observes, in order.
#[derive(Default)]
struct Observed(parking_lot::Mutex<Vec<ReviewAttemptStatus>>);

impl OperationReviewObserver for Observed {
    fn observe(&self, observation: ReviewObservation) {
        self.0.lock().push(observation.status);
    }
}

impl Observed {
    fn used(&self) -> usize {
        self.0
            .lock()
            .iter()
            .filter(|status| **status == ReviewAttemptStatus::Used)
            .count()
    }
}

/// The host's review composition over `approvals`.
fn review(
    approvals: &ApprovalService,
) -> (Arc<BoundOperationReview>, Arc<Reviewer>, Arc<Observed>) {
    let reviewer = Arc::new(Reviewer::default());
    let observed = Arc::new(Observed::default());
    let review = Arc::new(
        BoundOperationReview::new(
            reviewer.clone(),
            approvals.clone(),
            std::time::Duration::from_secs(30),
        )
        .with_observer(observed.clone()),
    );
    (review, reviewer, observed)
}

/// One governed dispatch of a `setup` call: its context with the review its
/// owner's tier requires admitted (an R1 decision admits none), its prepared
/// check and its review entry.
struct Dispatch {
    context: ToolDispatchContext,
    prepared: PreparedOperationCheck,
    entry: ReviewEntry,
}

impl Dispatch {
    /// Owned by the admitting dispatch for exactly its lifetime.
    fn guard(&self) -> ReviewDispatchGuard {
        ReviewDispatchGuard::new(self.entry.clone())
    }
}

async fn dispatch(
    authorization: Arc<dyn WorkAuthorization>,
    review: &Arc<BoundOperationReview>,
) -> Dispatch {
    let work =
        crate::WorkAuthorizationContext::new(authorization, crate::OperationExecutionScope::Domain);
    let plan = crate::ToolExecutionContract::default()
        .resolve_default(
            crate::ToolDeadlineChain::new(vec![crate::ToolDeadlineContributor::finite(
                crate::ToolDeadlineOwner::CoreToolDispatch,
                std::time::Duration::from_secs(1),
            )])
            .unwrap(),
        )
        .unwrap();
    let arguments: Arc<serde_json::value::RawValue> =
        Arc::from(serde_json::value::RawValue::from_string("{}".to_string()).unwrap());
    let binding = PreparedAuthorizationBinding::new(OperationAuthorizationFacts {
        operation_id: crate::OperationId::new(),
        execution_scope: crate::OperationExecutionScope::Domain,
        run_id: None,
        context_revision: None,
        operation: AuthorizationOperation::Tool(ToolAuthorizationFacts {
            call_id: Arc::from("setup-call"),
            name: "setup".into(),
            arguments,
            target: ToolAuthorizationTarget::Dispatcher(Arc::new(plan)),
        }),
    });
    let prepared = PreparedOperationCheck::prepare(work.clone(), binding).unwrap();
    let (call, _) = prepared.tool_dispatch_parts().unwrap();
    let entry = admit_operation_review(
        Some(review),
        ReviewEntrySupport::ConsumesAtEntry,
        call,
        &prepared,
        &work,
    )
    .await
    .expect("the review is admitted");
    let context = ToolDispatchContext::default()
        .with_work_authorization(Some(work))
        .with_prepared_authorization(prepared.clone())
        .with_review_entry(entry.clone());
    Dispatch {
        context,
        prepared,
        entry,
    }
}

fn refused(kind: OperationRefusalKind) -> ToolError {
    ToolError::AuthorizationRefused {
        refusal: OperationRefused::new(kind),
    }
}

/// P1. One spend at the leaf's first effect, then a further step and two
/// child issuances through the witness: one review, no second spend, every
/// step at the entered tier R2, and `Used` delivered once, after the
/// dispatch ended and the witness was dropped. Every other entry still finds
/// the review spent.
#[tokio::test]
async fn an_entered_review_runs_a_step_and_issues_two_children_with_one_spend() {
    let owner = Owner::new(OperationReviewTier::R2);
    let approvals = ApprovalService::new();
    let (review, reviewer, observed) = review(&approvals);
    let d = dispatch(Arc::new(OwnerWork(Arc::clone(&owner))), &review).await;
    let guard = d.guard();
    let (call, plan) = d.prepared.tool_dispatch_parts().unwrap();

    let (_, custody) = d
        .context
        .enter_reviewed_effect_held(call, Some(plan))
        .expect("the one spend");
    let entered = d
        .context
        .governed_operation(call)
        .expect("governed")
        .into_entered(custody)
        .expect("this dispatch's own custody");
    assert_eq!(entered.entered_tier(), OperationReviewTier::R2);

    let mut ran = 0;
    entered
        .run_if_current_within(&(), &mut || ran += 1)
        .expect("the step runs");
    assert_eq!(ran, 1);
    for intent in [7_u32, 8] {
        let issued = entered
            .issue_child_if_current_within(&(), &intent)
            .expect("issued without a second spend");
        assert_eq!(*issued.downcast::<u32>().expect("typed"), intent);
    }
    assert_eq!(*owner.issued.lock(), vec![7, 8]);
    assert_eq!(
        *owner.entered_tiers.lock(),
        vec![OperationReviewTier::R2; 3],
        "every step at the entered tier"
    );
    assert_eq!(
        reviewer.0.load(Ordering::SeqCst),
        1,
        "one review, never again"
    );

    let spent = |result: Result<(), ToolError>| {
        matches!(
            result,
            Err(ToolError::ReviewUnsatisfied {
                kind: ReviewUnsatisfiedKind::AlreadyConsumed,
                ..
            })
        )
    };
    assert!(spent(
        d.context
            .enter_reviewed_effect_held(call, Some(plan))
            .map(drop)
    ));
    assert!(spent(d.context.enter_governed_operation(call).map(drop)));
    assert!(spent(
        d.context
            .governed_operation(call)
            .unwrap()
            .issue_child_if_current_within(&(), &9_u32)
            .map(drop)
    ));
    assert_eq!(
        *owner.issued.lock(),
        vec![7, 8],
        "the consuming path issued nothing"
    );

    assert_eq!(observed.used(), 0, "Used waits for the effect");
    guard.complete();
    assert_eq!(observed.used(), 0, "the witness still holds the effect");
    drop(entered);
    assert_eq!(observed.used(), 1, "Used once, after the effect");
}

/// The admitting dispatch is cancelled while a step
/// admitted under the entered operation is still in flight (its store
/// commit blocked). The step completes; a step started after the
/// cancellation is refused as ended, with no not-dispatched certificate; no
/// later step runs; and `Used` is observed only after the witness drops.
#[tokio::test]
async fn a_dispatch_cancelled_while_its_step_is_in_flight_lets_the_step_finish() {
    let owner = Owner::new(OperationReviewTier::R2);
    let approvals = ApprovalService::new();
    let (review, _, observed) = review(&approvals);
    let d = dispatch(Arc::new(OwnerWork(Arc::clone(&owner))), &review).await;
    let mut guard = Some(d.guard());
    let (call, _) = d.prepared.tool_dispatch_parts().unwrap();
    let entered = d
        .context
        .enter_governed_operation(call)
        .expect("entered once");

    let mut later = None;
    let mut committed = false;
    entered
        .run_if_current_within(&(), &mut || {
            // The step's commit is blocked when the dispatch is cancelled.
            drop(guard.take());
            let mut nested_ran = false;
            let nested = entered.run_if_current_within(&(), &mut || nested_ran = true);
            later = Some((nested, nested_ran));
            committed = true;
        })
        .expect("the step admitted before the cancellation completes");
    assert!(committed, "the blocked step's effect committed");
    assert_eq!(
        later,
        Some((Err(refused(OperationRefusalKind::Denied)), false)),
        "a step started after the cancellation is refused as ended"
    );
    assert_eq!(observed.used(), 0, "Used waits for the work");

    let mut ran = false;
    assert_eq!(
        entered.run_if_current_within(&(), &mut || ran = true),
        Err(refused(OperationRefusalKind::Denied)),
        "no step after the dispatch ended"
    );
    assert!(!ran);
    assert_eq!(owner.entered_runs.load(Ordering::SeqCst), 1);
    drop(entered);
    assert_eq!(observed.used(), 1, "Used once, after the work");
}

/// N5 (i): once the admitting dispatch has completed, a held witness admits
/// no step and mints no new witness; the work owner is never asked.
#[tokio::test]
async fn a_step_after_the_dispatch_ended_is_refused_and_runs_nothing() {
    let owner = Owner::new(OperationReviewTier::R2);
    let approvals = ApprovalService::new();
    let (review, _, _) = review(&approvals);
    let d = dispatch(Arc::new(OwnerWork(Arc::clone(&owner))), &review).await;
    let guard = d.guard();
    let (call, plan) = d.prepared.tool_dispatch_parts().unwrap();
    let (_, custody) = d
        .context
        .enter_reviewed_effect_held(call, Some(plan))
        .expect("the one spend");
    let entered = d
        .context
        .governed_operation(call)
        .unwrap()
        .into_entered(custody)
        .expect("entered");
    assert!(
        d.context
            .enter_reviewed_effect_held(call, Some(plan))
            .is_err(),
        "spent once"
    );
    guard.complete();

    let mut ran = false;
    assert_eq!(
        entered.run_if_current_within(&(), &mut || ran = true),
        Err(refused(OperationRefusalKind::Denied))
    );
    assert!(!ran);
    assert_eq!(
        entered.issue_child_if_current_within(&(), &7_u32).map(drop),
        Err(refused(OperationRefusalKind::Denied))
    );
    assert!(owner.issued.lock().is_empty());
    assert!(
        owner.entered_tiers.lock().is_empty(),
        "the work owner was never asked"
    );
}

/// N5 (i), minting: a held custody of a dispatch that already ended mints no
/// witness (Denied), with no not-dispatched certificate.
#[tokio::test]
async fn an_ended_dispatch_mints_no_witness() {
    let owner = Owner::new(OperationReviewTier::R2);
    let approvals = ApprovalService::new();
    let (review, _, _) = review(&approvals);
    let d = dispatch(Arc::new(OwnerWork(owner)), &review).await;
    let guard = d.guard();
    let (call, plan) = d.prepared.tool_dispatch_parts().unwrap();
    let (_, custody) = d
        .context
        .enter_reviewed_effect_held(call, Some(plan))
        .expect("the one spend");
    guard.complete();
    assert_eq!(
        d.context
            .governed_operation(call)
            .unwrap()
            .into_entered(custody)
            .map(drop),
        Err(refused(OperationRefusalKind::Denied))
    );
}

/// N1 and N3: the witness accepts only this dispatch's own custody. An inert
/// custody names no dispatch, so it stands only for an unreviewed (R1)
/// entry: never for an unspent or an entered review. Another dispatch's
/// held custody never enters this one.
#[tokio::test]
async fn into_entered_accepts_only_this_dispatchs_own_custody() {
    let approvals = ApprovalService::new();
    let (review, _, _) = review(&approvals);
    let owner = Owner::new(OperationReviewTier::R2);
    let a = dispatch(Arc::new(OwnerWork(Arc::clone(&owner))), &review).await;
    let b = dispatch(Arc::new(OwnerWork(Arc::clone(&owner))), &review).await;
    let unreviewed = dispatch(
        Arc::new(OwnerWork(Owner::new(OperationReviewTier::R1))),
        &review,
    )
    .await;
    let (a_call, a_plan) = a.prepared.tool_dispatch_parts().unwrap();
    let (b_call, _) = b.prepared.tool_dispatch_parts().unwrap();
    let (r1_call, r1_plan) = unreviewed.prepared.tool_dispatch_parts().unwrap();
    let inert = || {
        unreviewed
            .context
            .enter_reviewed_effect_held(r1_call, Some(r1_plan))
            .expect("an unreviewed entry spends nothing")
            .1
    };
    let malformed: Result<(), ToolError> = Err(refused(OperationRefusalKind::MalformedFacts));

    assert_eq!(
        b.context
            .governed_operation(b_call)
            .unwrap()
            .into_entered(inert())
            .map(drop),
        malformed,
        "an inert custody does not stand for an unspent review"
    );
    let (_, a_custody) = a
        .context
        .enter_reviewed_effect_held(a_call, Some(a_plan))
        .expect("A spends once");
    assert_eq!(
        a.context
            .governed_operation(a_call)
            .unwrap()
            .into_entered(inert())
            .map(drop),
        malformed,
        "nor for an entered one"
    );
    assert_eq!(
        b.context
            .governed_operation(b_call)
            .unwrap()
            .into_entered(a_custody)
            .map(drop),
        malformed,
        "A's custody never enters B"
    );
    let r1 = unreviewed
        .context
        .governed_operation(r1_call)
        .unwrap()
        .into_entered(inert())
        .expect("the unreviewed dispatch's own inert custody");
    assert_eq!(r1.entered_tier(), OperationReviewTier::R1);
}

/// F9 and N4 (raised tier): a current decision that needs more review than
/// was entered is refused by core before the review owner or the work owner
/// is asked; a decision re-prepared at a lower tier runs.
#[tokio::test]
async fn a_step_needing_more_review_than_entered_is_refused_before_the_owners() {
    let owner = Owner::new(OperationReviewTier::R2);
    let approvals = ApprovalService::new();
    let (review, _, _) = review(&approvals);
    let d = dispatch(Arc::new(OwnerWork(Arc::clone(&owner))), &review).await;
    let _guard = d.guard();
    let (call, _) = d.prepared.tool_dispatch_parts().unwrap();
    let entered = d
        .context
        .enter_governed_operation(call)
        .expect("entered once");

    owner.set_tier(OperationReviewTier::R3);
    let mut ran = false;
    assert_eq!(
        entered.run_if_current_within(&(), &mut || ran = true),
        Err(refused(OperationRefusalKind::Denied))
    );
    assert_eq!(
        entered.issue_child_if_current_within(&(), &7_u32).map(drop),
        Err(refused(OperationRefusalKind::Denied))
    );
    assert!(!ran);
    assert!(
        owner.entered_tiers.lock().is_empty(),
        "core refused before asking the work owner"
    );

    owner.set_tier(OperationReviewTier::R1);
    entered
        .run_if_current_within(&(), &mut || ran = true)
        .expect("a decision re-prepared at a lower tier runs");
    assert!(ran);
    assert_eq!(
        *owner.entered_tiers.lock(),
        vec![OperationReviewTier::R2],
        "still within the review that was entered"
    );
}

/// F7 and N7 (core): a step's refusal speaks for the step only. A work
/// owner's refusal keeps its kind and loses any not-dispatched certificate
/// (`tool_invoked` "unknown"); unavailable and failed observation answer
/// `OperationAuthorizationUnavailable`, never
/// `OperationObservationUnavailable` or a review settlement. The witness
/// keeps serving after each refusal.
#[tokio::test]
async fn a_step_refusal_never_claims_the_call_did_not_enter() {
    let owner = Owner::new(OperationReviewTier::R2);
    let approvals = ApprovalService::new();
    let (review, _, _) = review(&approvals);
    let d = dispatch(Arc::new(OwnerWork(Arc::clone(&owner))), &review).await;
    let _guard = d.guard();
    let (call, _) = d.prepared.tool_dispatch_parts().unwrap();
    let entered = d
        .context
        .enter_governed_operation(call)
        .expect("entered once");
    let certified = OperationRefused::new(OperationRefusalKind::Denied).not_dispatched();
    for (scripted, answer) in [
        (
            OperationAuthorizationError::from(OperationRefused::new(OperationRefusalKind::Denied)),
            refused(OperationRefusalKind::Denied),
        ),
        (
            OperationAuthorizationError::from(certified),
            refused(OperationRefusalKind::Denied),
        ),
        (
            OperationAuthorizationError::from(OperationRefused::new(
                OperationRefusalKind::ReprepareRequired,
            )),
            refused(OperationRefusalKind::ReprepareRequired),
        ),
        (
            OperationAuthorizationError::Unavailable,
            ToolError::OperationAuthorizationUnavailable,
        ),
        (
            OperationAuthorizationError::ObservationUnavailable(OperationObservationError),
            ToolError::OperationAuthorizationUnavailable,
        ),
    ] {
        *owner.refuse_next.lock() = Some(scripted);
        let mut ran = false;
        assert_eq!(
            entered.run_if_current_within(&(), &mut || ran = true),
            Err(answer.clone()),
            "run step answering {scripted:?}"
        );
        assert!(!ran);
        *owner.refuse_next.lock() = Some(scripted);
        assert_eq!(
            entered.issue_child_if_current_within(&(), &7_u32).map(drop),
            Err(answer),
            "issuance step answering {scripted:?}"
        );
    }
    *owner.refuse_next.lock() = Some(certified.into());
    let refusal = entered
        .run_if_current_within(&(), &mut || {})
        .expect_err("a refused step");
    assert_eq!(
        refusal.structured_data().expect("refusal data")["tool_invoked"],
        serde_json::json!("unknown"),
        "a step's refusal never certifies that the tool did not run"
    );
    assert!(owner.issued.lock().is_empty());
    let mut ran = false;
    entered
        .run_if_current_within(&(), &mut || ran = true)
        .expect("the witness keeps serving after refused steps");
    assert!(ran);
}

/// A work owner that does not implement the entered step refuses every
/// entered step as unavailable (the trait's fail-closed default), and the
/// operation never runs.
#[tokio::test]
async fn a_work_owner_without_the_entered_step_refuses_it_as_unavailable() {
    let owner = Owner::new(OperationReviewTier::R2);
    let approvals = ApprovalService::new();
    let (review, _, _) = review(&approvals);
    let d = dispatch(Arc::new(PlainOnlyWork(owner)), &review).await;
    let _guard = d.guard();
    let (call, _) = d.prepared.tool_dispatch_parts().unwrap();
    let entered = d
        .context
        .enter_governed_operation(call)
        .expect("entered once");
    let mut ran = false;
    assert_eq!(
        entered.run_if_current_within(&(), &mut || ran = true),
        Err(ToolError::OperationAuthorizationUnavailable)
    );
    assert!(!ran);
}

/// An unreviewed (R1) entry's witness spends nothing and runs the plain R1
/// path; like that path it is bounded by its run, not by its dispatch.
#[tokio::test]
async fn an_unreviewed_witness_runs_the_plain_path_and_spends_nothing() {
    let owner = Owner::new(OperationReviewTier::R1);
    let approvals = ApprovalService::new();
    let (review, reviewer, _) = review(&approvals);
    let d = dispatch(Arc::new(OwnerWork(Arc::clone(&owner))), &review).await;
    let guard = d.guard();
    let (call, _) = d.prepared.tool_dispatch_parts().unwrap();
    let entered = d.context.enter_governed_operation(call).expect("R1 enters");
    let again = d
        .context
        .enter_governed_operation(call)
        .expect("an R1 entry spends nothing, so it enters again");
    assert_eq!(entered.entered_tier(), OperationReviewTier::R1);
    let mut ran = 0;
    entered
        .run_if_current_within(&(), &mut || ran += 1)
        .expect("the plain R1 step runs");
    guard.complete();
    again
        .run_if_current_within(&(), &mut || ran += 1)
        .expect("no dispatch-end bound for R1");
    assert_eq!(ran, 2);
    assert_eq!(owner.plain_runs.load(Ordering::SeqCst), 2);
    assert!(
        owner.entered_tiers.lock().is_empty(),
        "no entered step at R1"
    );
    assert_eq!(reviewer.0.load(Ordering::SeqCst), 0, "nothing reviewed");
}

/// A leaf's own conversion (R1, with a real refresh): the leaf enters with
/// `enter_reviewed_effect_held(call, None)`, its staging re-prepared the
/// decision and returned the refreshed context, and the operation taken from
/// THAT context converts the entry's custody into the witness, which serves
/// two fence steps and five child issuances.
#[tokio::test]
async fn a_refreshed_entry_context_converts_its_custody_into_the_witness() {
    let owner = Owner::new(OperationReviewTier::R1);
    let approvals = ApprovalService::new();
    let (review, _, _) = review(&approvals);
    let d = dispatch(Arc::new(OwnerWork(Arc::clone(&owner))), &review).await;
    let (call, _) = d.prepared.tool_dispatch_parts().unwrap();
    owner.refresh();
    let (entering, custody) = d
        .context
        .enter_reviewed_effect_held(call, None)
        .expect("an unreviewed entry");
    let entering = entering.expect("staging re-prepared the decision");
    assert!(
        !entering
            .prepared_authorization()
            .expect("governed")
            .same_check(&d.prepared),
        "the refreshed context carries another prepared decision"
    );
    let entered = entering
        .governed_operation(call)
        .expect("the same call, from the refreshed context")
        .into_entered(custody)
        .expect("the entry's own custody");
    assert_eq!(entered.entered_tier(), OperationReviewTier::R1);
    let mut fences = 0;
    for _ in 0..2 {
        entered
            .run_if_current_within(&(), &mut || fences += 1)
            .expect("a fence step");
    }
    for intent in 1_u32..=5 {
        entered
            .issue_child_if_current_within(&(), &intent)
            .expect("a child step");
    }
    assert_eq!(fences, 2);
    assert_eq!(*owner.issued.lock(), vec![1, 2, 3, 4, 5]);
}

/// A leaf's own conversion (R2): the leaf's single entry with
/// `enter_reviewed_effect_held(call, None)`, the operation taken from the
/// context the entry returned (this context: a reviewed entry spends only
/// the decision its review admitted), and one witness for two fence steps
/// and five child issuances: one review, one spend, each step decided
/// against the operation's current decision, re-prepared after an owner
/// change at the same tier, at the tier the entry spent.
#[tokio::test]
async fn one_witness_from_the_entry_context_serves_two_fences_and_five_children() {
    let owner = Owner::new(OperationReviewTier::R2);
    let approvals = ApprovalService::new();
    let (review, reviewer, observed) = review(&approvals);
    let d = dispatch(Arc::new(OwnerWork(Arc::clone(&owner))), &review).await;
    let guard = d.guard();
    let (call, _) = d.prepared.tool_dispatch_parts().unwrap();
    let (entering, custody) = d
        .context
        .enter_reviewed_effect_held(call, None)
        .expect("the one spend");
    let entered = entering
        .as_ref()
        .unwrap_or(&d.context)
        .governed_operation(call)
        .expect("the entry's context")
        .into_entered(custody)
        .expect("the entry's own custody");
    assert_eq!(entered.entered_tier(), OperationReviewTier::R2);
    let holder = entered.clone();
    owner.refresh();
    let mut fences = 0;
    for _ in 0..2 {
        entered
            .run_if_current_within(&(), &mut || fences += 1)
            .expect("a fence step on the re-prepared decision");
    }
    for intent in 1_u32..=5 {
        entered
            .issue_child_if_current_within(&(), &intent)
            .expect("a child step, never a second spend");
    }
    assert_eq!(fences, 2);
    assert_eq!(*owner.issued.lock(), vec![1, 2, 3, 4, 5]);
    assert_eq!(
        *owner.entered_tiers.lock(),
        vec![OperationReviewTier::R2; 7],
        "every step at the tier the entry spent"
    );
    assert_eq!(reviewer.0.load(Ordering::SeqCst), 1);
    guard.complete();
    drop(entered);
    assert_eq!(observed.used(), 0, "a kept clone still holds the effect");
    drop(holder);
    assert_eq!(observed.used(), 1);
}

/// A conversion refused after the entry never says that the call did not
/// enter: it carries no not-dispatched certificate (`tool_invoked`
/// "unknown"), for another dispatch's custody and for an ended dispatch.
#[tokio::test]
async fn a_conversion_refused_after_the_entry_is_tool_invoked_unknown() {
    let owner = Owner::new(OperationReviewTier::R2);
    let approvals = ApprovalService::new();
    let (review, _, _) = review(&approvals);
    let a = dispatch(Arc::new(OwnerWork(Arc::clone(&owner))), &review).await;
    let b = dispatch(Arc::new(OwnerWork(Arc::clone(&owner))), &review).await;
    let b_guard = b.guard();
    let (a_call, _) = a.prepared.tool_dispatch_parts().unwrap();
    let (b_call, _) = b.prepared.tool_dispatch_parts().unwrap();
    let (_, a_custody) = a
        .context
        .enter_reviewed_effect_held(a_call, None)
        .expect("A spends");
    let (_, b_custody) = b
        .context
        .enter_reviewed_effect_held(b_call, None)
        .expect("B spends");
    b_guard.complete();
    for error in [
        b.context
            .governed_operation(b_call)
            .unwrap()
            .into_entered(a_custody)
            .map(drop)
            .expect_err("another dispatch's custody"),
        b.context
            .governed_operation(b_call)
            .unwrap()
            .into_entered(b_custody)
            .map(drop)
            .expect_err("an ended dispatch"),
    ] {
        assert!(matches!(error, ToolError::AuthorizationRefused { .. }));
        assert_eq!(
            error.structured_data().expect("refusal data")["tool_invoked"],
            serde_json::json!("unknown"),
            "{error:?}"
        );
    }
}

/// An approval store that counts its writes.
#[derive(Default)]
struct CountingStore {
    puts: AtomicUsize,
}

impl ApprovalStore for CountingStore {
    fn load_all(&self) -> Result<Vec<ApprovalRecord>, ApprovalStoreError> {
        Ok(Vec::new())
    }

    fn get(&self, _approval_id: &ApprovalId) -> Result<Option<ApprovalRecord>, ApprovalStoreError> {
        Ok(None)
    }

    fn put_if_current(
        &self,
        _expected: Option<&ApprovalRecord>,
        _record: &ApprovalRecord,
    ) -> Result<(), ApprovalStoreError> {
        self.puts.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn is_persistent(&self) -> bool {
        true
    }
}

/// N9 (core): no review runs under a store's lock. The plain path refuses a
/// reviewed decision in every entry state (retained, then entered) without
/// a reviewer call, and an entered step runs its operation (the store's
/// commit) with no reviewer call and no approval-store write, before, inside
/// or after it.
#[tokio::test]
async fn no_step_runs_a_review_or_writes_the_approval_store() {
    let owner = Owner::new(OperationReviewTier::R2);
    let store = Arc::new(CountingStore::default());
    let approvals = ApprovalService::with_store(store.clone()).expect("service");
    let (review, reviewer, _) = review(&approvals);
    let d = dispatch(Arc::new(OwnerWork(Arc::clone(&owner))), &review).await;
    let _guard = d.guard();
    let (call, _) = d.prepared.tool_dispatch_parts().unwrap();
    let plain = d.context.governed_operation(call).expect("governed");
    let unsupported: Result<(), ToolError> = Err(ToolError::ReviewUnavailable {
        kind: ReviewUnavailableKind::UnsupportedEntry,
    });
    let mut ran = false;
    assert_eq!(
        plain.run_if_current_within(&(), &mut || ran = true),
        unsupported,
        "retained: the plain path runs no review"
    );
    let entered = d
        .context
        .enter_governed_operation(call)
        .expect("entered once");
    assert_eq!(
        plain.run_if_current_within(&(), &mut || ran = true),
        unsupported,
        "entered: the plain path still runs no review"
    );
    assert!(!ran);
    let reviews = reviewer.0.load(Ordering::SeqCst);
    let puts = store.puts.load(Ordering::SeqCst);
    assert_eq!(reviews, 1, "the review ran once, before the effect");
    let mut inside = None;
    entered
        .run_if_current_within(&(), &mut || {
            inside = Some((
                reviewer.0.load(Ordering::SeqCst),
                store.puts.load(Ordering::SeqCst),
            ));
        })
        .expect("the entered step");
    assert_eq!(inside, Some((reviews, puts)), "nothing inside the commit");
    entered
        .issue_child_if_current_within(&(), &7_u32)
        .expect("an issuance step");
    assert_eq!(reviewer.0.load(Ordering::SeqCst), reviews);
    assert_eq!(store.puts.load(Ordering::SeqCst), puts);
}

// ---------------------------------------------------------------------------
// Waiting steps: a step refused to rebuild while another actor holds the
// work owners' exclusion waits for that hold outside every lock, within the
// call's bounds, and is taken again. The review stays spent exactly once.
// ---------------------------------------------------------------------------

/// The host's operation refusal observer: every disposition, in order.
#[derive(Default)]
struct Dispositions(parking_lot::Mutex<Vec<crate::authorization::OperationRefusalDisposition>>);

impl crate::authorization::OperationRefusalObserver for Dispositions {
    fn observe(&self, observation: &crate::authorization::OperationRefusalObservation) {
        self.0.lock().push(observation.disposition());
    }
}

impl Dispositions {
    /// Every wait reported, by whether it settled.
    fn waits(&self) -> Vec<bool> {
        self.0
            .lock()
            .iter()
            .filter_map(|disposition| match disposition {
                crate::authorization::OperationRefusalDisposition::WaitedForOwnerChange {
                    settled,
                    ..
                } => Some(*settled),
                _ => None,
            })
            .collect()
    }
}

/// `context` reporting to `observer`, as the agent attaches it to every
/// dispatch.
fn observed_by(context: &ToolDispatchContext, observer: &Arc<Dispositions>) -> ToolDispatchContext {
    context
        .clone()
        .with_refusal_observer(Some(crate::authorization::RefusalObserverBinding::new(
            Arc::clone(observer) as Arc<dyn crate::authorization::OperationRefusalObserver>,
            crate::types::SessionId::new(),
        )))
}

/// Another actor takes `owner`'s exclusion now and releases it after `held`.
fn hold_for(owner: &Arc<Owner>, held: Duration) -> tokio::task::JoinHandle<()> {
    owner.held.store(true, Ordering::SeqCst);
    let owner = Arc::clone(owner);
    tokio::spawn(async move {
        tokio::time::sleep(held).await;
        owner.held.store(false, Ordering::SeqCst);
    })
}

/// A reviewed (R2) entered step meets another actor's hold of the work
/// owners' exclusion. Its waiting form waits for the hold (one settled wait,
/// reported as the call's), takes the step again and runs the operation
/// once. The review was spent once, by the entry: no attempt spends, a later
/// entry finds it spent, and `Used` is reported once.
#[tokio::test(start_paused = true)]
async fn a_waiting_entered_step_waits_out_a_held_exclusion_and_spends_once() {
    let owner = Owner::new(OperationReviewTier::R2);
    let approvals = ApprovalService::new();
    let (review, reviewer, observed) = review(&approvals);
    let d = dispatch(Arc::new(OwnerWork(Arc::clone(&owner))), &review).await;
    let guard = d.guard();
    let dispositions = Arc::new(Dispositions::default());
    let context = observed_by(&d.context, &dispositions);
    let (call, _) = d.prepared.tool_dispatch_parts().unwrap();
    let entered = context
        .enter_governed_operation(call)
        .expect("the one spend");

    let started = tokio::time::Instant::now();
    let release = hold_for(&owner, Duration::from_millis(50));
    let mut ran = 0;
    entered
        .run_if_current_within_after_owner_change(&(), || ran += 1)
        .await
        .expect("the step runs once the hold ends");
    release.await.expect("released");
    assert_eq!(ran, 1, "the operation ran once");
    assert!(
        started.elapsed() >= Duration::from_millis(50),
        "it waited for the hold"
    );
    assert_eq!(
        owner.contended.load(Ordering::SeqCst),
        1,
        "one attempt met the hold"
    );
    assert_eq!(owner.entered_runs.load(Ordering::SeqCst), 1);
    assert_eq!(dispositions.waits(), [true], "one settled wait");
    assert_eq!(reviewer.0.load(Ordering::SeqCst), 1, "one review");
    assert!(
        matches!(
            context.enter_governed_operation(call).map(drop),
            Err(ToolError::ReviewUnsatisfied {
                kind: ReviewUnsatisfiedKind::AlreadyConsumed,
                ..
            })
        ),
        "spent once, by the entry"
    );
    guard.complete();
    drop(entered);
    assert_eq!(observed.used(), 1, "Used once, after the effect");
}

/// A dispatch's own child issuance at R2 meets the held exclusion at its
/// first issuance, under its single consuming entry. The waiting form spends
/// the review once, takes the issuance again through the entry's witness
/// after one settled wait, and issues the child once; a later entry finds
/// the review spent.
#[tokio::test(start_paused = true)]
async fn a_waiting_dispatch_issuance_spends_once_and_issues_through_its_witness() {
    let owner = Owner::new(OperationReviewTier::R2);
    let approvals = ApprovalService::new();
    let (review, reviewer, observed) = review(&approvals);
    let d = dispatch(Arc::new(OwnerWork(Arc::clone(&owner))), &review).await;
    let guard = d.guard();
    let dispositions = Arc::new(Dispositions::default());
    let context = observed_by(&d.context, &dispositions);
    let (call, _) = d.prepared.tool_dispatch_parts().unwrap();

    let release = hold_for(&owner, Duration::from_millis(50));
    let issued = context
        .governed_operation(call)
        .expect("governed")
        .issue_child_if_current_within_after_owner_change(&(), &7_u32)
        .await
        .expect("issued once the hold ends");
    release.await.expect("released");
    assert_eq!(*issued.downcast::<u32>().expect("typed"), 7);
    assert_eq!(*owner.issued.lock(), vec![7], "issued once");
    assert_eq!(owner.contended.load(Ordering::SeqCst), 1);
    assert_eq!(
        *owner.entered_tiers.lock(),
        vec![OperationReviewTier::R2; 2],
        "two attempts, both at the tier the entry spent"
    );
    assert_eq!(dispositions.waits(), [true], "one settled wait");
    assert_eq!(reviewer.0.load(Ordering::SeqCst), 1, "one review");
    assert!(
        matches!(
            context.enter_governed_operation(call).map(drop),
            Err(ToolError::ReviewUnsatisfied {
                kind: ReviewUnsatisfiedKind::AlreadyConsumed,
                ..
            })
        ),
        "spent once, by the waiting form's entry"
    );
    guard.complete();
    assert_eq!(observed.used(), 1, "Used once, after the effect");
}

/// Another actor's read of the approval owner, as `get` or `list` takes it,
/// on a thread of its own: begun at the spend's final check after its commit
/// (the first current check of `owner`'s decisions that finds the approval
/// owner free after one that found it held for the commit) and held until
/// released.
struct ApprovalReader {
    owner: Arc<Owner>,
    reading: Arc<parking_lot::Mutex<Option<HeldRead>>>,
}

/// A reader thread holding the approval owner, and its release.
type HeldRead = (std::sync::mpsc::Sender<()>, std::thread::JoinHandle<()>);

impl ApprovalReader {
    fn after_the_spend(owner: &Arc<Owner>, approvals: &ApprovalService) -> Self {
        let reading: Arc<parking_lot::Mutex<Option<HeldRead>>> = Arc::default();
        let slot = Arc::clone(&reading);
        let approvals = approvals.clone();
        let mut committing = false;
        *owner.on_check.lock() = Some(Box::new(move || {
            if slot.lock().is_some() {
                return;
            }
            if approvals.state_write_held() {
                committing = true;
            } else if committing {
                let (held, holding) = std::sync::mpsc::channel();
                let (release, released) = std::sync::mpsc::channel();
                let reader = approvals.clone();
                let thread = std::thread::spawn(move || reader.read_state_until(&held, &released));
                holding.recv().expect("the reader holds the approval owner");
                *slot.lock() = Some((release, thread));
            }
        }));
        Self {
            owner: Arc::clone(owner),
            reading,
        }
    }

    /// The reader releases the approval owner; returns whether it held it.
    fn release(self) -> bool {
        *self.owner.on_check.lock() = None;
        let Some((release, thread)) = self.reading.lock().take() else {
            return false;
        };
        drop(release);
        thread.join().expect("the reader's thread");
        true
    }
}

/// A dispatch's own child issuance at R2 whose first issuance meets another
/// reader of the approval owner, begun after the spend's commit released the
/// owner and held through the issuance, still issues on that first attempt:
/// it is the synchronous form's own issuance under the entry, with no step
/// admission for the reader to refuse. The review is spent once, nothing
/// waits, and `Used` is reported once.
#[tokio::test(start_paused = true)]
async fn a_waiting_dispatch_issuance_issues_first_under_its_entry_past_an_approval_read() {
    let owner = Owner::new(OperationReviewTier::R2);
    let approvals = ApprovalService::new();
    let (review, reviewer, observed) = review(&approvals);
    let d = dispatch(Arc::new(OwnerWork(Arc::clone(&owner))), &review).await;
    let guard = d.guard();
    let dispositions = Arc::new(Dispositions::default());
    let context = observed_by(&d.context, &dispositions);
    let (call, _) = d.prepared.tool_dispatch_parts().unwrap();
    let started = tokio::time::Instant::now();

    let reader = ApprovalReader::after_the_spend(&owner, &approvals);
    let issued = context
        .governed_operation(call)
        .expect("governed")
        .issue_child_if_current_within_after_owner_change(&(), &7_u32)
        .await;
    assert!(
        reader.release(),
        "a reader held the approval owner from the spend on"
    );
    let issued = issued.expect("issued on the first attempt, under the entry");
    assert_eq!(*issued.downcast::<u32>().expect("typed"), 7);
    assert_eq!(*owner.issued.lock(), vec![7], "issued once");
    assert_eq!(
        *owner.entered_tiers.lock(),
        vec![OperationReviewTier::R2],
        "one attempt, at the tier the entry spent"
    );
    assert_eq!(owner.contended.load(Ordering::SeqCst), 0);
    assert!(dispositions.waits().is_empty(), "no wait");
    assert_eq!(started.elapsed(), Duration::ZERO);
    assert_eq!(reviewer.0.load(Ordering::SeqCst), 1, "one review");
    assert!(
        matches!(
            context.enter_governed_operation(call).map(drop),
            Err(ToolError::ReviewUnsatisfied {
                kind: ReviewUnsatisfiedKind::AlreadyConsumed,
                ..
            })
        ),
        "spent once, by the waiting form's entry"
    );
    guard.complete();
    assert_eq!(observed.used(), 1, "Used once, after the effect");
}

/// Held past the call's remaining tool time, the waiting form waits only
/// until that time ends, never the full owner-change bound, reports one
/// unsettled wait and answers the typed refusal to rebuild: the operation
/// never ran.
#[tokio::test(start_paused = true)]
async fn past_the_calls_tool_time_a_waiting_step_answers_the_typed_rebuild_refusal() {
    let owner = Owner::new(OperationReviewTier::R1);
    let approvals = ApprovalService::new();
    let (review, _, _) = review(&approvals);
    let d = dispatch(Arc::new(OwnerWork(Arc::clone(&owner))), &review).await;
    let dispositions = Arc::new(Dispositions::default());
    let tool_time = Duration::from_millis(300);
    let context =
        observed_by(&d.context, &dispositions).with_staging_wait(super::EntryStagingWait::new(
            super::DispatchDeadline {
                ends: Some(crate::time_compat::DeadlineInstant::now() + tool_time),
            },
            None,
        ));
    let (call, _) = d.prepared.tool_dispatch_parts().unwrap();

    // Held past the tool time: nothing releases it while the step waits.
    owner.held.store(true, Ordering::SeqCst);
    let started = tokio::time::Instant::now();
    let mut ran = false;
    let answer = context
        .governed_operation(call)
        .expect("governed")
        .run_if_current_within_after_owner_change(&(), || ran = true)
        .await;
    let waited = started.elapsed();
    assert_eq!(
        answer,
        Err(refused(OperationRefusalKind::ReprepareRequired)),
        "the typed refusal to rebuild"
    );
    assert!(!ran, "the operation never ran");
    assert_eq!(owner.plain_runs.load(Ordering::SeqCst), 0);
    assert_eq!(owner.contended.load(Ordering::SeqCst), 1);
    assert!(
        waited >= tool_time && waited < crate::authorization::OWNER_CHANGE_WAIT_BOUND,
        "bounded by the tool time: {waited:?}"
    );
    assert_eq!(dispositions.waits(), [false], "one unsettled wait");
}

/// Owners that cannot answer give no refusal to rebuild: the waiting forms
/// answer `OperationAuthorizationUnavailable` at once, even while a hold is
/// in progress, with no wait and nothing run or issued.
#[tokio::test(start_paused = true)]
async fn an_unavailable_step_is_answered_at_once_and_never_waits() {
    let owner = Owner::new(OperationReviewTier::R2);
    let approvals = ApprovalService::new();
    let (review, _, _) = review(&approvals);
    let d = dispatch(Arc::new(OwnerWork(Arc::clone(&owner))), &review).await;
    let _guard = d.guard();
    let dispositions = Arc::new(Dispositions::default());
    let context = observed_by(&d.context, &dispositions);
    let (call, _) = d.prepared.tool_dispatch_parts().unwrap();
    let entered = context
        .enter_governed_operation(call)
        .expect("entered once");
    owner.held.store(true, Ordering::SeqCst);
    let started = tokio::time::Instant::now();

    *owner.refuse_next.lock() = Some(OperationAuthorizationError::Unavailable);
    let mut ran = false;
    assert_eq!(
        entered
            .run_if_current_within_after_owner_change(&(), || ran = true)
            .await,
        Err(ToolError::OperationAuthorizationUnavailable)
    );
    assert!(!ran);
    *owner.refuse_next.lock() = Some(OperationAuthorizationError::Unavailable);
    assert_eq!(
        entered
            .issue_child_if_current_within_after_owner_change(&(), &7_u32)
            .await
            .map(drop),
        Err(ToolError::OperationAuthorizationUnavailable)
    );
    assert_eq!(started.elapsed(), Duration::ZERO, "no wait");
    assert!(dispositions.waits().is_empty(), "no wait reported");
    assert!(owner.issued.lock().is_empty());
    assert_eq!(
        owner.contended.load(Ordering::SeqCst),
        0,
        "never taken again"
    );
}

/// An attempt whose operation ran is the step's answer, whatever it says:
/// after a work owner refuses to rebuild once the operation ran, there is no
/// second attempt and no wait, and the operation ran once.
#[tokio::test(start_paused = true)]
async fn a_step_whose_operation_ran_is_never_taken_again() {
    let owner = Owner::new(OperationReviewTier::R2);
    let approvals = ApprovalService::new();
    let (review, _, _) = review(&approvals);
    let d = dispatch(Arc::new(OwnerWork(Arc::clone(&owner))), &review).await;
    let _guard = d.guard();
    let dispositions = Arc::new(Dispositions::default());
    let context = observed_by(&d.context, &dispositions);
    let (call, _) = d.prepared.tool_dispatch_parts().unwrap();
    let entered = context
        .enter_governed_operation(call)
        .expect("entered once");

    *owner.refuse_after_run.lock() =
        Some(OperationRefused::new(OperationRefusalKind::ReprepareRequired).into());
    let mut ran = 0;
    assert_eq!(
        entered
            .run_if_current_within_after_owner_change(&(), || ran += 1)
            .await,
        Err(refused(OperationRefusalKind::ReprepareRequired))
    );
    assert_eq!(ran, 1, "the operation ran once");
    assert_eq!(owner.entered_runs.load(Ordering::SeqCst), 1, "one attempt");
    assert!(dispositions.waits().is_empty(), "no wait after an effect");
}

/// A refusal to rebuild with no change in progress at the read (its holder
/// finished before it) is one of the step's free rebuilds: the step pauses
/// one backoff step (1 ms) within its deadline, reports a settled wait and
/// is taken again. A step refused three times that way runs on its fourth
/// attempt; one refused a fourth time answers the typed refusal at once,
/// its free rebuilds spent: one bound for every retry, and never a fifth
/// attempt.
#[tokio::test(start_paused = true)]
async fn with_no_change_in_progress_a_refused_step_spends_its_free_rebuilds() {
    let owner = Owner::new(OperationReviewTier::R2);
    let approvals = ApprovalService::new();
    let (review, _, _) = review(&approvals);
    let d = dispatch(Arc::new(OwnerWork(Arc::clone(&owner))), &review).await;
    let _guard = d.guard();
    let dispositions = Arc::new(Dispositions::default());
    let context = observed_by(&d.context, &dispositions);
    let (call, _) = d.prepared.tool_dispatch_parts().unwrap();
    let entered = context
        .enter_governed_operation(call)
        .expect("entered once");
    let started = tokio::time::Instant::now();

    owner.unnamed_rebuilds.store(3, Ordering::SeqCst);
    let mut ran = 0;
    entered
        .run_if_current_within_after_owner_change(&(), || ran += 1)
        .await
        .expect("it runs on its last free rebuild");
    assert_eq!(ran, 1);
    assert_eq!(owner.entered_tiers.lock().len(), 4, "four attempts");
    assert_eq!(dispositions.waits(), [true; 3], "three settled pauses");
    assert_eq!(started.elapsed(), Duration::from_millis(3), "1 ms each");

    owner.unnamed_rebuilds.store(4, Ordering::SeqCst);
    assert_eq!(
        entered
            .issue_child_if_current_within_after_owner_change(&(), &7_u32)
            .await
            .map(drop),
        Err(refused(OperationRefusalKind::ReprepareRequired))
    );
    assert_eq!(
        owner.entered_tiers.lock().len(),
        8,
        "four more attempts, then the refusal"
    );
    assert_eq!(owner.unnamed_rebuilds.load(Ordering::SeqCst), 0);
    assert!(owner.issued.lock().is_empty());
    assert_eq!(
        dispositions.waits(),
        [true, true, true, true, true, true, false],
        "three more settled pauses, then none: the free rebuilds are spent"
    );
    assert_eq!(started.elapsed(), Duration::from_millis(6));
}
