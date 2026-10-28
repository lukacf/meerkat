//! Reviewed governed calls that continue with governed helper steps after
//! their single consuming entry.
//!
//! The consented (R3) rows run on the native grant-backed work owner (with
//! its audit wrapper), the approval owner with a host human consent route,
//! the real fenced entry and the retained consent attempt. The cancelled run
//! row runs a model-reviewed (R2) call inside a real native run (the
//! MeerkatMachine run custody, the Agent owners and the review owner). Every
//! row uses the real helper WorkGraph fence and helper child issuance; the
//! reviewer, the human route, the records policy, the original source and
//! the scripted controller are fixtures.

use super::*;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use meerkat::governed_realm::helper_admission::{
    GovernedHelperAdmissionFence, HelperAdmissionRefusal,
};
use meerkat::governed_realm::helper_child_issuance::{
    GovernedHelperChildIssuance, HelperChildIssuanceRefusal,
};
use meerkat_authorization::grant_policy::GrantBackedWorkPolicy;
use meerkat_authorization::grants::{ChildGrantIntent, ChildGrantOutcome};
use meerkat_authorization::work::{FurtherOriginals, RetainedOriginal};
use meerkat_authorization_contracts::audit::{
    AuthorizationAuditObservation, AuthorizationAuditSink,
};
use meerkat_core::agent::{
    dispatch_tool_execution_plan_fenced, resolve_tool_execution_plan_fenced,
};
use meerkat_core::approval::consent::{
    AuthenticatedApprover, AuthenticatedConsentDecision, ConsentEligibility, ConsentIneligible,
    ConsentNotice, ConsentNoticeKind, ConsentPresentation, ConsentPresentationCandidate,
    ConsentRouteUnavailable, HumanConsentRoute,
};
use meerkat_core::approval::review::{
    BoundOperationReview, OperationReviewObserver, OperationReviewer, ReviewAttemptStatus,
    ReviewCandidate, ReviewEntrySupport, ReviewObservation, ReviewUnsatisfiedKind, ReviewVerdict,
    ReviewerFailure,
};
use meerkat_core::authorization::{
    ControllerReadinessFailure, OperationObservationError, RefusalDispatch,
};
use meerkat_core::{
    ApprovalId, ApprovalRecord, EnteredDispatchOperation, dispatch_retained_consent_attempt,
};
use meerkat_runtime::meerkat_machine::{
    AuthenticatedOriginal, AuthenticatedOriginalSource, OriginalDisposition, RetainedCustody,
    SourceOriginalBasis,
};

/// The consented call's id, as the binding retains it.
const CONSENTED_CALL: &str = "consented-read";
/// Host-chosen validity of one consent request.
const CONSENT_VALIDITY: Duration = Duration::from_secs(4 * 60 * 60);

/// The records owner, rating every record operation R3: a fresh qualified
/// human decision is required. It selects no separate action account.
struct ConsentedRecords;
impl OperationPolicyOwner for ConsentedRecords {
    fn authorize_operation(
        &self,
        association: &InputAuthorityAssociation,
        binding: &PreparedAuthorizationBinding,
        purpose: LocalPolicyPurpose,
        now_ms: u64,
    ) -> Result<LocalPolicyAllowance, meerkat_core::OperationAuthorizationError> {
        let mut allowance =
            RecordOwner.authorize_operation(association, binding, purpose, now_ms)?;
        allowance.review_tier = meerkat_core::authorization::OperationReviewTier::R3;
        Ok(allowance)
    }

    fn review_action_account(
        &self,
        _: &InputAuthorityAssociation,
        _: &PreparedAuthorizationBinding,
        _: u64,
    ) -> Result<Option<String>, meerkat_core::OperationAuthorizationError> {
        Ok(None)
    }
}

/// R3 is never model-reviewed; counts every review it is asked for.
#[derive(Default)]
struct NoReviewer(AtomicUsize);
#[async_trait]
impl OperationReviewer for NoReviewer {
    async fn review(
        &self,
        _candidate: &ReviewCandidate<'_>,
    ) -> Result<ReviewVerdict, ReviewerFailure> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err(ReviewerFailure::Unavailable)
    }
}

/// The host's human route: `lead-1` is the eligible approver, published
/// through one local authorization publication; notices are recorded.
struct ConsentRoute {
    notices: Mutex<Vec<(ApprovalId, ConsentNoticeKind)>>,
    publication: LocalAuthorizationPublication,
}
impl ConsentRoute {
    fn new() -> Self {
        Self {
            notices: Mutex::new(Vec::new()),
            publication: LocalAuthorizationPublication::new(),
        }
    }

    fn notices(&self, approval_id: &ApprovalId) -> Vec<ConsentNoticeKind> {
        self.notices
            .lock()
            .expect("notices")
            .iter()
            .filter(|(id, _)| id == approval_id)
            .map(|(_, kind)| *kind)
            .collect()
    }
}
impl HumanConsentRoute for ConsentRoute {
    fn present(
        &self,
        _candidate: &ConsentPresentationCandidate<'_>,
    ) -> Result<ConsentPresentation, ConsentRouteUnavailable> {
        Ok(ConsentPresentation {
            requester: meerkat_core::ApprovalPrincipalId::new("human:requester")
                .expect("principal"),
            risk: meerkat_core::ApprovalRisk::High,
            metadata: meerkat_core::SurfaceMetadata::default(),
        })
    }

    fn eligibility(
        &self,
        approver: &AuthenticatedApprover,
        _approval: &ApprovalRecord,
    ) -> Result<Arc<dyn ConsentEligibility>, ConsentIneligible> {
        let eligible = approver.principal().id.as_str() == "lead-1";
        meerkat_authorization::publication::PublishedConsentEligibility::observe(
            &self.publication,
            || eligible,
        )
    }

    fn notice(&self, notice: ConsentNotice) {
        self.notices
            .lock()
            .expect("notices")
            .push((notice.approval_id, notice.kind));
    }
}

/// An authenticated approval by `id`, as the host's ingress adapter makes it.
fn approval_by(id: &str) -> AuthenticatedConsentDecision {
    AuthenticatedConsentDecision {
        approver: AuthenticatedApprover::new(
            PrincipalRef::in_domain(
                PrincipalKind::Human,
                id,
                TrustDomainId::new("team").expect("domain"),
            )
            .expect("principal"),
            meerkat_core::ApprovalAssurance::PersonAuthenticated,
        )
        .expect("qualified approver"),
        decision: meerkat_core::ApprovalDecision::Approve,
        reason: None,
    }
}

/// Every review observation, for counting the `Used` reports.
#[derive(Default)]
struct UsedObservations(Mutex<Vec<ReviewObservation>>);
impl OperationReviewObserver for UsedObservations {
    fn observe(&self, observation: ReviewObservation) {
        self.0
            .lock()
            .expect("review observations")
            .push(observation);
    }
}
impl UsedObservations {
    /// How many times the attempt of `call` was reported used.
    fn used(&self, call: &str) -> usize {
        self.0
            .lock()
            .expect("review observations")
            .iter()
            .filter(|observation| {
                observation.status == ReviewAttemptStatus::Used
                    && observation.tool_call_id.as_ref() == call
            })
            .count()
    }
}

/// The work's own audit sink: every observation is accepted.
#[derive(Default)]
struct AuditRecords(Mutex<Vec<AuthorizationAuditObservation>>);
impl AuthorizationAuditSink for AuditRecords {
    fn append(
        &self,
        observation: AuthorizationAuditObservation,
    ) -> Result<(), OperationObservationError> {
        self.0.lock().expect("audit records").push(observation);
        Ok(())
    }
}

/// The work's own original as its source owner holds it: active, and never
/// retained as a contributor. Armed, its next disposition read, which a
/// helper step makes inside the work owners' exclusion after the review
/// owner admitted that step, reports the step in flight and holds it until
/// released: a store publication blocked under the entered operation.
struct StepOriginals {
    armed: AtomicBool,
    in_flight: Notify,
    gate: Mutex<Option<std::sync::mpsc::Receiver<()>>>,
    release: Mutex<Option<std::sync::mpsc::Sender<()>>>,
}
impl StepOriginals {
    fn new() -> Self {
        let (release, gate) = std::sync::mpsc::channel();
        Self {
            armed: AtomicBool::new(false),
            in_flight: Notify::new(),
            gate: Mutex::new(Some(gate)),
            release: Mutex::new(Some(release)),
        }
    }

    /// Let a held step go on. Idempotent.
    fn release(&self) {
        drop(self.release.lock().expect("step gate").take());
    }

    /// Report the step in flight and hold it here until released.
    fn hold(&self) {
        self.in_flight.notify_one();
        let gate = self.gate.lock().expect("step gate").take();
        if let Some(gate) = gate {
            // Released, or the sender dropped: `recv` returns either way.
            let _ = gate.recv();
        }
    }
}
impl AuthenticatedOriginalSource for StepOriginals {
    fn authenticated_original(
        &self,
        _: &NativeIngressContext,
        _: &QualifiedIngressNamespace,
        _: &OriginalWorkRef,
    ) -> Result<Option<AuthenticatedOriginal>, ControllerReadinessFailure> {
        Ok(None)
    }

    fn retained_custody(
        &self,
        _: &QualifiedIngressNamespace,
        _: &OriginalWorkRef,
    ) -> Result<RetainedCustody, ControllerReadinessFailure> {
        Ok(RetainedCustody::NotRetained)
    }

    fn original_disposition(
        &self,
        _: &QualifiedIngressNamespace,
        _: &OriginalWorkRef,
    ) -> Result<OriginalDisposition, ControllerReadinessFailure> {
        if self.armed.swap(false, Ordering::SeqCst) {
            self.hold();
        }
        Ok(OriginalDisposition::Active)
    }
}

/// Releases a held step when the test ends, whether it passed or not.
struct ReleaseOnDrop(Arc<StepOriginals>);
impl Drop for ReleaseOnDrop {
    fn drop(&mut self) {
        self.0.release();
    }
}

/// The consented call: `read_record` on `record-7`, rated R3.
struct ConsentedCall {
    id: String,
    name: String,
    args: Box<serde_json::value::RawValue>,
}
impl ConsentedCall {
    fn new() -> Self {
        Self {
            id: CONSENTED_CALL.into(),
            name: "read_record".into(),
            args: serde_json::value::RawValue::from_string(r#"{"record":"record-7"}"#.into())
                .expect("arguments"),
        }
    }

    fn view(&self) -> ToolCallView<'_> {
        ToolCallView {
            id: &self.id,
            name: &self.name,
            args: &self.args,
        }
    }
}

/// What one entered call's helper steps answered.
#[derive(Debug, Clone)]
struct HelperSteps {
    created: bool,
    fence_refusal: Option<HelperAdmissionRefusal>,
    children: Vec<Result<ChildGrantOutcome, HelperChildIssuanceRefusal>>,
    retried: Result<ChildGrantOutcome, HelperChildIssuanceRefusal>,
}

/// What a held helper worker answered once it was released.
#[derive(Debug, PartialEq)]
enum HeldStepAnswer {
    /// The helper fence's commit: whether it created the item, and the
    /// fence's own refusal.
    Fence {
        created: bool,
        refusal: Option<HelperAdmissionRefusal>,
    },
    /// A core step whose operation is the item's commit: whether it created
    /// the item, and the step's own answer.
    Step {
        created: bool,
        answer: Result<(), ToolError>,
    },
}

/// What the consented leaf does after its single consuming entry.
enum LeafMode {
    /// Admit the helper item, issue every intent's child, then retry the
    /// first intent: each a further step of the entered operation.
    Steps(Vec<ChildGrantIntent>),
    /// Admit the helper item from a worker thread, whose step the test holds
    /// in flight, and wait for that worker.
    Blocked,
    /// Commit the helper item from a worker thread as the operation of one
    /// core step of the entered operation, which is the helper fence's own
    /// commit path, and wait for that worker. The test holds the step: armed,
    /// on its first original read inside the work owners' exclusion, before
    /// their in-exclusion re-check; otherwise at the start of its operation,
    /// past that re-check.
    CommitStep,
}

/// The consented tool's effect leaf: it consumes review at its entry, makes
/// the call's single consuming entry with `enter_governed_operation`, then
/// takes the call's governed helper steps with the witness.
struct EnteredLeaf {
    mode: LeafMode,
    graph: meerkat::WorkGraphService,
    originals: Arc<StepOriginals>,
    bodies: AtomicUsize,
    steps: Mutex<Vec<HelperSteps>>,
    witness: Mutex<Option<EnteredDispatchOperation>>,
    worker: Mutex<Option<std::thread::JoinHandle<HeldStepAnswer>>>,
}

impl EnteredLeaf {
    fn helper_admission(&self) -> (meerkat::WorkAdmissionKey, meerkat::WorkAdmission) {
        let key = meerkat::WorkAdmissionKey::new("helper-for-consented-read").expect("key");
        let admission = self
            .graph
            .admission_identity(key.clone(), helper_request())
            .expect("identity");
        (key, admission)
    }

    /// The helper's WorkGraph item, then each intent's child, then the first
    /// intent again: every one a further step of the entered operation.
    async fn helper_steps(
        &self,
        entered: &EnteredDispatchOperation,
        intents: &[ChildGrantIntent],
    ) -> HelperSteps {
        let (key, admission) = self.helper_admission();
        let fence = GovernedHelperAdmissionFence::from_entered(
            entered,
            self.originals.clone(),
            no_contributors(),
            "default",
            meerkat::WorkNamespace::default(),
            admission,
        );
        let created = matches!(
            self.graph
                .create_idempotent_with_fence(key, helper_request(), fence.clone())
                .await,
            Ok(meerkat::WorkAdmissionOutcome::Created(_))
        );
        let issue = |intent: &ChildGrantIntent| {
            GovernedHelperChildIssuance::from_entered(
                entered,
                self.originals.clone(),
                no_contributors(),
                intent.clone(),
            )
            .issue_or_recover()
        };
        let children = intents.iter().map(&issue).collect();
        let retried = issue(&intents[0]);
        HelperSteps {
            created,
            fence_refusal: fence.refusal(),
            children,
            retried,
        }
    }
}

#[async_trait]
impl AgentToolDispatcher for EnteredLeaf {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        RecordingTools::default().tools()
    }

    fn review_entry_support(&self, _tool_name: &str) -> ReviewEntrySupport {
        ReviewEntrySupport::ConsumesAtEntry
    }

    async fn dispatch(&self, _: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        panic!("the consented leaf keeps its dispatch context")
    }

    async fn dispatch_with_context(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        // The call's single consuming entry, at its first mutating step: the
        // human consent is spent here, once.
        let entered = context.enter_governed_operation(call)?;
        match &self.mode {
            LeafMode::Steps(intents) => {
                let steps = self.helper_steps(&entered, intents).await;
                self.steps.lock().expect("helper steps").push(steps);
            }
            LeafMode::Blocked => {
                *self.witness.lock().expect("witness") = Some(entered.clone());
                let (key, admission) = self.helper_admission();
                let fence = GovernedHelperAdmissionFence::from_entered(
                    &entered,
                    self.originals.clone(),
                    no_contributors(),
                    "default",
                    meerkat::WorkNamespace::default(),
                    admission,
                );
                let (done, finished) = tokio::sync::oneshot::channel();
                let graph = self.graph.clone();
                let runtime = tokio::runtime::Handle::current();
                let worker = std::thread::spawn(move || {
                    let created = matches!(
                        runtime.block_on(graph.create_idempotent_with_fence(
                            key,
                            helper_request(),
                            fence.clone(),
                        )),
                        Ok(meerkat::WorkAdmissionOutcome::Created(_))
                    );
                    let refusal = fence.refusal();
                    drop(fence);
                    let _ = done.send(());
                    HeldStepAnswer::Fence { created, refusal }
                });
                *self.worker.lock().expect("helper worker") = Some(worker);
                // The dispatch waits for its helper publication; the host
                // cancels the dispatch here, while that step is held.
                let _ = finished.await;
            }
            LeafMode::CommitStep => {
                *self.witness.lock().expect("witness") = Some(entered.clone());
                let (key, _) = self.helper_admission();
                let (done, finished) = tokio::sync::oneshot::channel();
                let graph = self.graph.clone();
                let originals = Arc::clone(&self.originals);
                let runtime = tokio::runtime::Handle::current();
                let worker = std::thread::spawn(move || {
                    let further = FurtherOriginals {
                        originals: no_contributors(),
                        source: Arc::new(SourceOriginalBasis::new(originals.clone())),
                    };
                    let mut created = false;
                    // The step's operation is the item's commit, as in the
                    // fence: it runs past the work owners' in-exclusion
                    // re-check, and the test may hold it there.
                    let answer = entered.run_if_current_within(&further, &mut || {
                        originals.hold();
                        created = matches!(
                            runtime
                                .block_on(graph.create_idempotent(key.clone(), helper_request())),
                            Ok(meerkat::WorkAdmissionOutcome::Created(_))
                        );
                    });
                    drop(entered);
                    let _ = done.send(());
                    HeldStepAnswer::Step { created, answer }
                });
                *self.worker.lock().expect("helper worker") = Some(worker);
                // The dispatch waits for its helper commit; the host cancels
                // the dispatch here, while that commit is held.
                let _ = finished.await;
            }
        }
        self.bodies.fetch_add(1, Ordering::SeqCst);
        Ok(ToolResult::new(call.id.into(), "record-7 value".into(), false).into())
    }
}

fn helper_request() -> meerkat::CreateWorkItemRequest {
    meerkat::CreateWorkItemRequest {
        title: "setup helper".into(),
        ..Default::default()
    }
}

fn no_contributors() -> Arc<[RetainedOriginal]> {
    Arc::from([])
}

fn child_intent(parent: &GrantLineageRef, name: &str) -> ChildGrantIntent {
    ChildGrantIntent {
        issuer: principal("helper-operator"),
        parent_lineage: vec![parent.clone()],
        id: id(name),
        grantee: principal("helper"),
        represented_subject: None,
        requested: ExecutionRestrictions::unrestricted(),
    }
}

fn execution_scope() -> OperationExecutionScope {
    OperationExecutionScope::RuntimeInput {
        owner_session_id: SessionId::new(),
        runtime_epoch_id: serde_json::from_str(r#""00000000-0000-0000-0000-000000000002""#)
            .expect("epoch id"),
        submitted_input_id: serde_json::from_str(r#""00000000-0000-0000-0000-000000000003""#)
            .expect("input id"),
        canonical_input_id: serde_json::from_str(r#""00000000-0000-0000-0000-000000000003""#)
            .expect("input id"),
    }
}

/// Native work where the consented read is permitted at R3 by the actual
/// grant-backed owner, with a host human route composed; the helper's
/// independent parent grant is issued by the same grant owner.
struct ConsentedWorld {
    grants: Arc<LocalGrantAuthority>,
    approvals: meerkat_core::ApprovalService,
    route: Arc<ConsentRoute>,
    reviewer: Arc<NoReviewer>,
    observer: Arc<UsedObservations>,
    originals: Arc<StepOriginals>,
    leaf: Arc<EnteredLeaf>,
    context: ToolDispatchContext,
    parent: GrantLineageRef,
}

impl ConsentedWorld {
    /// `children` intents for the inline helper steps, or the blocked
    /// helper step when `blocked`.
    fn new(children: usize, blocked: bool) -> Self {
        let grants = Arc::new(
            LocalGrantAuthority::new(
                LocalGrantConfiguration {
                    root: principal("grant-owner"),
                    namespace: id("entered-consent-grants"),
                    generation: 1,
                },
                LocalAuthorizationPublication::new(),
                Arc::new(HostAuthorizationClock),
            )
            .expect("actual generated grant owner"),
        );
        let controller = grants
            .issue_root(
                &principal("grant-owner"),
                id("controller"),
                principal("executor"),
                None,
                ceiling("infer"),
            )
            .expect("actual controller lineage");
        let operation = grants
            .issue_root(
                &principal("grant-owner"),
                id("read-only"),
                principal("executor"),
                None,
                ceiling("read"),
            )
            .expect("actual read-only operation grant");
        let parent = grants
            .issue_root(
                &principal("grant-owner"),
                id("helper-parent"),
                principal("helper-operator"),
                None,
                ExecutionRestrictions::unrestricted(),
            )
            .expect("actual independent parent grant");
        let runtime = LogicalRuntimeId::for_session(&SessionId::new());
        let admitted = association(&runtime, controller, operation, selection());
        let policy = Arc::new(GrantBackedWorkPolicy::new(
            Arc::clone(&grants),
            Arc::new(InvocationOwner),
            Arc::new(ConsentedRecords),
        ));
        let work = policy
            .audited_work_context(
                Arc::from([admitted]),
                execution_scope(),
                Arc::new(AuditRecords::default()),
            )
            .expect("the native grant-backed work context");
        let approvals = meerkat_core::ApprovalService::new();
        let route = Arc::new(ConsentRoute::new());
        let reviewer = Arc::new(NoReviewer::default());
        let observer = Arc::new(UsedObservations::default());
        let review =
            BoundOperationReview::new(reviewer.clone(), approvals.clone(), Duration::from_secs(30))
                .with_observer(observer.clone())
                .with_human_consent(route.clone(), CONSENT_VALIDITY);
        let originals = Arc::new(StepOriginals::new());
        let mode = if blocked {
            LeafMode::Blocked
        } else {
            LeafMode::Steps(
                (1..=children)
                    .map(|n| child_intent(&parent, &format!("helper-child-{n}")))
                    .collect(),
            )
        };
        let leaf = Arc::new(EnteredLeaf {
            mode,
            graph: meerkat::WorkGraphService::new(Arc::new(meerkat::MemoryWorkGraphStore::new())),
            originals: Arc::clone(&originals),
            bodies: AtomicUsize::new(0),
            steps: Mutex::new(Vec::new()),
            witness: Mutex::new(None),
            worker: Mutex::new(None),
        });
        let context = ToolDispatchContext::default()
            .with_work_authorization(Some(work))
            .with_operation_review(Some(Arc::new(review)));
        Self {
            grants,
            approvals,
            route,
            reviewer,
            observer,
            originals,
            leaf,
            context,
            parent,
        }
    }

    fn plan(&self, call: &ConsentedCall) -> meerkat_core::ResolvedToolExecutionPlan {
        resolve_tool_execution_plan_fenced(
            &self.leaf,
            call.view(),
            &self.context,
            &meerkat_core::ToolExecutionResolutionContext::new(
                meerkat_core::ToolDeadlineChain::new(vec![
                    meerkat_core::ToolDeadlineContributor::finite(
                        meerkat_core::ToolDeadlineOwner::DirectCaller,
                        Duration::from_secs(600),
                    ),
                ])
                .expect("finite caller deadline"),
            ),
        )
        .expect("real plan resolution")
    }

    /// The consented call settles locally with its one consent approval;
    /// nothing entered.
    async fn request(&self, call: &ConsentedCall) -> ApprovalId {
        let plan = self.plan(call);
        let settled =
            dispatch_tool_execution_plan_fenced(&self.leaf, call.view(), &self.context, &plan)
                .await;
        let approval_id = match settled {
            Err(ToolError::ReviewUnsatisfied {
                kind: ReviewUnsatisfiedKind::HumanConsentRequired,
                approval_id: Some(approval_id),
            }) => approval_id,
            other => panic!("expected local human_consent_required, got {other:?}"),
        };
        assert_eq!(
            self.route.notices(&approval_id),
            [ConsentNoticeKind::Requested]
        );
        assert_eq!(
            self.leaf.bodies.load(Ordering::SeqCst),
            0,
            "nothing entered before the consent"
        );
        approval_id
    }

    fn approve(&self, approval_id: &ApprovalId) {
        let approved = self
            .approvals
            .decide_consent(approval_id, approval_by("lead-1"))
            .expect("the authenticated eligible approval commits");
        assert_eq!(approved.status, meerkat_core::ApprovalStatus::Approved);
    }

    /// The step's further originals: none beyond the work's own.
    fn further(&self) -> FurtherOriginals {
        FurtherOriginals {
            originals: no_contributors(),
            source: Arc::new(SourceOriginalBasis::new(self.originals.clone())),
        }
    }
}

/// The consented call is entered once by its retained attempt; the witness
/// then admits the helper item and issues `children` children, each a step
/// decided afresh, and a retry of the first intent recovers its child. The
/// consent is spent exactly once, the body runs once, nothing is refused as
/// already consumed, and a second entry finds the consent spent.
async fn consented_call_takes_its_helper_steps(children: usize) {
    let world = ConsentedWorld::new(children, false);
    let call = ConsentedCall::new();
    let approval_id = world.request(&call).await;
    world.approve(&approval_id);

    let entered =
        dispatch_retained_consent_attempt(&world.leaf, &world.context, &approval_id).await;
    assert!(
        entered.is_ok(),
        "the retained candidate enters: {entered:?}"
    );
    let recorded = world.leaf.steps.lock().expect("helper steps").clone();
    assert_eq!(recorded.len(), 1, "one entered call");
    let steps = &recorded[0];
    assert!(steps.created, "the helper item is committed: {steps:?}");
    assert_eq!(steps.fence_refusal, None);
    assert_eq!(steps.children.len(), children);
    for (index, child) in steps.children.iter().enumerate() {
        let Ok(ChildGrantOutcome::Issued { lineage }) = child else {
            panic!("child {index} is issued: {child:?}")
        };
        assert_eq!(
            world
                .grants
                .issued_reference(&id(&format!("helper-child-{}", index + 1)))
                .expect("lookup")
                .as_ref(),
            lineage.last(),
            "the grant owner retains exactly the issued child"
        );
        world
            .grants
            .resolve_controller_lineage(lineage, &principal("helper"), None)
            .expect("the child resolves as the helper's controller lineage");
    }
    let Ok(ChildGrantOutcome::Recovered { lineage: recovered }) = &steps.retried else {
        panic!("retrying an issued intent recovers its child: {steps:?}")
    };
    assert_eq!(
        Some(recovered.as_slice()),
        steps.children[0]
            .as_ref()
            .ok()
            .map(ChildGrantOutcome::lineage),
        "the same child, with no second spend"
    );
    assert_eq!(
        world.leaf.bodies.load(Ordering::SeqCst),
        1,
        "one body entry"
    );
    let consent = world
        .approvals
        .get(&approval_id)
        .expect("record")
        .consent
        .expect("consent history");
    assert!(consent.consumed_at.is_some(), "spent by the entry");
    assert!(consent.withdrawal.is_none(), "never withdrawn");
    assert_eq!(
        world.route.notices(&approval_id),
        [
            ConsentNoticeKind::Requested,
            ConsentNoticeKind::Approved,
            ConsentNoticeKind::Used,
        ],
        "decided, then used exactly once"
    );
    assert_eq!(
        world.observer.used(CONSENTED_CALL),
        1,
        "Used is reported once, after the dispatch ended"
    );
    assert_eq!(
        world.reviewer.0.load(Ordering::SeqCst),
        0,
        "R3 is never model-reviewed"
    );

    // The steps spent nothing more: a second entry finds the consent spent.
    let second = dispatch_retained_consent_attempt(&world.leaf, &world.context, &approval_id).await;
    assert!(
        matches!(
            second,
            Err(ToolError::ReviewUnsatisfied {
                kind: ReviewUnsatisfiedKind::AlreadyConsumed,
                ..
            })
        ),
        "a second entry finds the consent spent: {second:?}"
    );
    assert_eq!(world.leaf.bodies.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_consented_call_admits_its_helper_item_and_issues_two_children_with_one_spend() {
    consented_call_takes_its_helper_steps(2).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_consented_call_admits_its_helper_item_and_issues_five_children_with_one_spend() {
    consented_call_takes_its_helper_steps(5).await;
}

/// The consented dispatch is cancelled while the helper publication it
/// admitted under its entered operation is still held in flight. The consent
/// stays spent and in use: a second spend is refused as already consumed,
/// and no Used is reported. A step started after the cancellation is refused
/// as denied, with no not-dispatched certificate, and runs nothing; the held
/// step then completes and commits its item. Only once the last witness is
/// dropped is Used reported, exactly once.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cancelled_consented_dispatch_keeps_its_review_in_use_until_its_held_step_commits() {
    let world = ConsentedWorld::new(0, true);
    let _release = ReleaseOnDrop(Arc::clone(&world.originals));
    let call = ConsentedCall::new();
    let approval_id = world.request(&call).await;
    world.approve(&approval_id);

    world.originals.armed.store(true, Ordering::SeqCst);
    let attempt = tokio::spawn({
        let leaf = Arc::clone(&world.leaf);
        let context = world.context.clone();
        let approval_id = approval_id.clone();
        async move { dispatch_retained_consent_attempt(&leaf, &context, &approval_id).await }
    });
    tokio::time::timeout(
        Duration::from_secs(20),
        world.originals.in_flight.notified(),
    )
    .await
    .expect("the helper step is admitted and in flight");
    // The host cancels the admitting dispatch while its helper step is held.
    attempt.abort();
    let cancelled = attempt.await;
    assert!(
        cancelled
            .as_ref()
            .is_err_and(tokio::task::JoinError::is_cancelled),
        "the admitting dispatch was cancelled: {cancelled:?}"
    );

    // The consent stays spent and in use while the admitted step is in
    // flight: a second spend is refused, and no Used is reported.
    let second = dispatch_retained_consent_attempt(&world.leaf, &world.context, &approval_id).await;
    assert!(
        matches!(
            second,
            Err(ToolError::ReviewUnsatisfied {
                kind: ReviewUnsatisfiedKind::AlreadyConsumed,
                ..
            })
        ),
        "a second spend is refused as already consumed: {second:?}"
    );
    assert!(
        world
            .approvals
            .get(&approval_id)
            .expect("record")
            .consent
            .and_then(|consent| consent.consumed_at)
            .is_some(),
        "spent once, at the entry"
    );
    assert_eq!(
        world.observer.used(CONSENTED_CALL),
        0,
        "no Used while the admitted step is in flight"
    );
    assert_eq!(
        world.route.notices(&approval_id),
        [ConsentNoticeKind::Requested, ConsentNoticeKind::Approved],
        "no Used notice while the admitted step is in flight"
    );

    // A step that starts after the cancellation is refused as ended.
    let witness = world
        .leaf
        .witness
        .lock()
        .expect("witness")
        .take()
        .expect("the entry minted its witness");
    let mut ran = false;
    let refused = witness.run_if_current_within(&world.further(), &mut || ran = true);
    match &refused {
        Err(ToolError::AuthorizationRefused { refusal }) => {
            assert_eq!(refusal.kind(), OperationRefusalKind::Denied);
            assert_eq!(refusal.dispatch(), RefusalDispatch::Unknown);
        }
        other => panic!("a step after the cancellation is refused as denied: {other:?}"),
    }
    assert!(!ran, "the refused step ran nothing");
    let error = refused.expect_err("refused");
    assert_eq!(error.to_error_payload()["data"]["tool_invoked"], "unknown");
    let after = GovernedHelperChildIssuance::from_entered(
        &witness,
        world.originals.clone(),
        no_contributors(),
        child_intent(&world.parent, "after-the-end"),
    )
    .issue_or_recover();
    assert_eq!(after, Err(HelperChildIssuanceRefusal::Denied));
    assert_eq!(
        world
            .grants
            .issued_reference(&id("after-the-end"))
            .expect("lookup"),
        None,
        "nothing issued after the cancellation"
    );

    // The held publication goes on: its step completes and commits.
    world.originals.release();
    let worker = world
        .leaf
        .worker
        .lock()
        .expect("helper worker")
        .take()
        .expect("the entry started its helper worker");
    let answer = tokio::time::timeout(
        Duration::from_secs(20),
        tokio::task::spawn_blocking(move || worker.join()),
    )
    .await
    .expect("the helper worker finishes")
    .expect("join task")
    .expect("helper worker");
    assert_eq!(
        answer,
        HeldStepAnswer::Fence {
            created: true,
            refusal: None,
        },
        "the in-flight step committed its helper item"
    );

    // Released at its last step's finish, the attempt admits no step either.
    let mut ran_after_release = false;
    let refused_after_release =
        witness.run_if_current_within(&world.further(), &mut || ran_after_release = true);
    assert!(refused_after_release.is_err(), "{refused_after_release:?}");
    assert!(!ran_after_release, "the refused step ran nothing");
    assert_eq!(
        world.observer.used(CONSENTED_CALL),
        0,
        "the witness still holds the entered custody"
    );

    drop(witness);
    assert_eq!(
        world.observer.used(CONSENTED_CALL),
        1,
        "Used is reported once, after the last witness was dropped"
    );
    assert_eq!(
        world.route.notices(&approval_id),
        [
            ConsentNoticeKind::Requested,
            ConsentNoticeKind::Approved,
            ConsentNoticeKind::Used,
        ]
    );
}

/// The records owner rating every record tool R2 (one model review), with
/// the controller admitted as `RecordOwner` admits it and no separately
/// selected action account.
struct ReviewedRecords;
impl OperationPolicyOwner for ReviewedRecords {
    fn authorize_controller_admission(
        &self,
        association: &InputAuthorityAssociation,
        facts: &meerkat_core::ControllerModelFacts,
        now_ms: u64,
    ) -> Result<
        meerkat_authorization::grant_policy::ControllerAdmissionAllowance,
        meerkat_core::OperationAuthorizationError,
    > {
        RecordOwner.authorize_controller_admission(association, facts, now_ms)
    }

    fn authorize_operation(
        &self,
        association: &InputAuthorityAssociation,
        binding: &PreparedAuthorizationBinding,
        purpose: LocalPolicyPurpose,
        now_ms: u64,
    ) -> Result<LocalPolicyAllowance, meerkat_core::OperationAuthorizationError> {
        let mut allowance =
            RecordOwner.authorize_operation(association, binding, purpose, now_ms)?;
        if matches!(binding.facts().operation, AuthorizationOperation::Tool(_)) {
            allowance.review_tier = meerkat_core::authorization::OperationReviewTier::R2;
        }
        Ok(allowance)
    }

    fn review_action_account(
        &self,
        _: &InputAuthorityAssociation,
        _: &PreparedAuthorizationBinding,
        _: u64,
    ) -> Result<Option<String>, meerkat_core::OperationAuthorizationError> {
        Ok(None)
    }
}

/// The bound model reviewer of the reviewed run: it allows, and counts.
#[derive(Default)]
struct AllowingReviewer(AtomicUsize);
#[async_trait]
impl OperationReviewer for AllowingReviewer {
    async fn review(
        &self,
        _candidate: &ReviewCandidate<'_>,
    ) -> Result<ReviewVerdict, ReviewerFailure> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(ReviewVerdict::Allow)
    }
}

/// The reviewed call's id in the controller's response.
const REVIEWED_CALL: &str = "reviewed-read";

/// The controller of the reviewed run: its first request asks for the
/// reviewed read, and the run is cancelled before it needs another.
#[derive(Default)]
struct ReviewedReadProvider(AtomicUsize);
#[async_trait]
impl LlmClient for ReviewedReadProvider {
    fn plain_model_route(
        &self,
        logical_model: &str,
    ) -> Result<meerkat_llm_core::PlainModelRoute, meerkat_core::ControllerFactsUnavailable> {
        if logical_model != MODEL {
            return Err(meerkat_core::ControllerFactsUnavailable);
        }
        meerkat_llm_core::PlainModelRoute::new(ENDPOINT, MODEL)
    }

    fn controller_model_selection(&self) -> Option<ControllerModelSelection> {
        Some(selection())
    }

    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(&'a self, _: &'a LlmRequest) -> LlmStream<'a> {
        panic!("governed request must reach the prepared provider entry")
    }

    fn stream_prepared<'a>(&'a self, request: &'a PreparedLlmRequest) -> LlmStream<'a> {
        let check = request
            .authorization()
            .expect("native work reaches actual provider")
            .prepare(model_facts())
            .expect("actual native controller authorization")
            .current()
            .expect("final controller entry check");
        check
            .observe_entry(meerkat_core::OperationEntryBoundary::ModelRequest)
            .expect("native audit stages actual transport entry");
        check
            .observe_outcome(OperationObservedOutcome::HttpResponse { status: 200 })
            .expect("native audit stages fixture response headers");
        if self.0.fetch_add(1, Ordering::SeqCst) == 0 {
            Box::pin(futures::stream::iter(tool_events(
                REVIEWED_CALL,
                "read_record",
            )))
        } else {
            Box::pin(futures::stream::iter([
                Ok(LlmEvent::TextDelta {
                    delta: "unexpected continuation after the cancellation".into(),
                    meta: None,
                }),
                done(StopReason::EndTurn),
            ]))
        }
    }

    fn provider(&self) -> Provider {
        Provider::Anthropic
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

/// Where the held step of the reviewed run waits. Its operation is the
/// helper item's commit, as in the helper fence.
#[derive(Clone, Copy, Debug, PartialEq)]
enum HeldAt {
    /// On its first original read inside the work owners' exclusion: before
    /// their in-exclusion re-check, which reads the run's native
    /// accepted-work custody again.
    BeforeTheRecheck,
    /// At the start of its operation, the item's commit: past that re-check.
    InsideItsCommit,
}

/// A model-reviewed (R2) call in a real native run enters with
/// `enter_governed_operation` and hands a helper step to a worker, whose step
/// is held in flight before the work owners' in-exclusion re-check. The run
/// is cancelled meanwhile. While the step is still held: the run reaches its
/// cancelled terminal within a bound; a step started after the cancellation
/// is refused and commits nothing (MalformedFacts at core once the runtime
/// applied the cancellation, with no not-dispatched certificate; Denied at
/// the helpers); and no Used is reported. Once the gate opens, the held step
/// re-reads the run's custody inside the exclusion and is refused too
/// (MalformedFacts, with no not-dispatched certificate): it commits nothing.
/// Used is reported exactly once, after the worker's last witness clone
/// drops.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cancelled_reviewed_run_settles_while_its_entered_step_is_held() {
    cancelled_reviewed_run_with_a_held_step(HeldAt::BeforeTheRecheck).await;
}

/// The same run, with the held step past its in-exclusion re-check, at the
/// start of its operation, the item's commit: the cancelled terminal still
/// arrives within the bound, later steps are refused and no Used is reported
/// while it is held; once the gate opens the held commit answers Created,
/// and Used is reported exactly once, after the worker's last witness clone
/// drops.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cancelled_reviewed_run_keeps_the_commit_of_a_step_past_its_recheck() {
    cancelled_reviewed_run_with_a_held_step(HeldAt::InsideItsCommit).await;
}

async fn cancelled_reviewed_run_with_a_held_step(held: HeldAt) {
    let grants = Arc::new(
        LocalGrantAuthority::new(
            LocalGrantConfiguration {
                root: principal("grant-owner"),
                namespace: id("entered-review-grants"),
                generation: 1,
            },
            LocalAuthorizationPublication::new(),
            Arc::new(HostAuthorizationClock),
        )
        .expect("actual generated grant owner"),
    );
    let controller = grants
        .issue_root(
            &principal("grant-owner"),
            id("controller"),
            principal("executor"),
            None,
            ceiling("infer"),
        )
        .expect("actual controller lineage with unrestricted lifetime");
    let operation = grants
        .issue_root(
            &principal("grant-owner"),
            id("read-only"),
            principal("executor"),
            None,
            ceiling("read"),
        )
        .expect("actual read-only operation grant");
    // Issued before the run: a held step keeps the grant publication's
    // writer, so no owner change can land while it is held.
    let helper_parent = grants
        .issue_root(
            &principal("grant-owner"),
            id("helper-parent"),
            principal("helper-operator"),
            None,
            ExecutionRestrictions::unrestricted(),
        )
        .expect("actual independent parent grant");
    let expected_controller = controller.clone();
    let expected_operation = operation.clone();
    let ingress: Arc<NativeIngressCheck> = Arc::new(move |runtime, _, current, claimed| {
        let candidate = claimed.candidate();
        if current.requester() != &principal("requester")
            || current.ingress_actor() != &principal("ingress")
            || current.realm() != &RealmId::parse("native-loop").expect("realm")
            || candidate.logical_executor != principal("executor")
            || candidate.represented_subject.is_some()
            || candidate.target.logical_runtime != id(&runtime.to_string())
            || candidate.controller_grant_lineage != [expected_controller.clone()]
            || candidate.authority_basis
                != (WorkAuthorityBasis::GrantLineage {
                    lineage: vec![expected_operation.clone()],
                })
            || candidate.controller_model.as_ref() != Some(&selection())
            || candidate.controller_ceiling != ceiling("infer")
        {
            return Err(denied().into());
        }
        Ok(())
    });
    let machine = Arc::new(
        MeerkatMachine::ephemeral()
            .with_local_grant_authorization(NativeGrantWorkConfiguration {
                retained_resume: None,
                require_scoped_destination: false,
                grants: Arc::clone(&grants),
                ingress,
                invocation_owner: Arc::new(InvocationOwner),
                operation_owner: Arc::new(ReviewedRecords),
                configured_mcp_attachment: None,
            })
            .expect("exclusive actual native host setup"),
    );
    let originals = Arc::new(StepOriginals::new());
    let _release = ReleaseOnDrop(Arc::clone(&originals));
    let leaf = Arc::new(EnteredLeaf {
        mode: LeafMode::CommitStep,
        graph: meerkat::WorkGraphService::new(Arc::new(meerkat::MemoryWorkGraphStore::new())),
        originals: Arc::clone(&originals),
        bodies: AtomicUsize::new(0),
        steps: Mutex::new(Vec::new()),
        witness: Mutex::new(None),
        worker: Mutex::new(None),
    });
    let reviewer = Arc::new(AllowingReviewer::default());
    let observer = Arc::new(UsedObservations::default());
    let review = Arc::new(
        BoundOperationReview::new(
            reviewer.clone(),
            meerkat_core::ApprovalService::new(),
            Duration::from_secs(30),
        )
        .with_observer(observer.clone()),
    );
    let mut builder = FactoryAgentBuilder::new(
        AgentFactory::minimal().with_operation_review(review),
        Config::default(),
    );
    builder.default_llm_client = Some(Arc::new(ReviewedReadProvider::default()));
    builder.default_tool_dispatcher = Some(leaf.clone());
    builder.default_session_store = Some(Arc::new(RecordingStore::default()));
    let service = Arc::new(EphemeralSessionService::new(builder, 2));
    let created = meerkat::surface::materialize_ephemeral_runtime_session(
        &service,
        &machine,
        CreateSessionRequest {
            model: MODEL.into(),
            prompt: "".into(),
            injected_context: Vec::new(),
            system_prompt: meerkat_core::config::SystemPromptOverride::Inherit,
            max_tokens: None,
            event_tx: None,
            initial_turn: InitialTurnPolicy::Defer,
            deferred_prompt_policy: DeferredPromptPolicy::Discard,
            build: None,
            labels: None,
        },
        false,
    )
    .await
    .expect("real native executor and deferred Agent actor");
    let session_id = created.session_id;
    let actor = service
        .live_session_actor_witness(&session_id)
        .await
        .expect("actual live actor");
    let pin = service
        .pin_controller_client_for_actor(&actor)
        .await
        .expect("actual immutable selected LlmClientAdapter");
    let credential_owner = machine.generated_auth_lease_handle();
    meerkat_core::publish_token_lifecycle_acquired_for_identity_within(
        &credential_owner,
        pin.selection().credential(),
        &meerkat_core::auth::PersistedTokens::api_key("synthetic-native-loop-credential"),
        tokio::time::Instant::now() + CREDENTIAL_PUBLICATION_WAIT,
    )
    .await
    .expect("actual generated fixture credential before native admission");
    let runtime = LogicalRuntimeId::for_session(&session_id);
    let mut prompt = PromptInput::new("Read record-7 through its reviewed tool", None);
    prompt.header.authority_association = Some(association(
        &runtime,
        controller,
        operation,
        pin.selection().clone(),
    ));
    let input = Input::Prompt(prompt);
    let current = NativeIngressContext::from_trusted_ingress(
        &input,
        principal("requester"),
        principal("ingress"),
        RealmId::parse("native-loop").expect("realm"),
        evidence("fresh-transport-authentication"),
    )
    .expect("separate current trusted ingress observation")
    .with_controller_client(&input, pin)
    .expect("bind real pin before native acceptance");
    let input = input
        .with_ingress_context(current)
        .expect("exact finalized input binding");

    // The helper step the leaf hands to its worker is held inside the work
    // owners' exclusion, after the review owner admitted it: on its first
    // original read (before the in-exclusion re-check), or inside its commit
    // (past it).
    if held == HeldAt::BeforeTheRecheck {
        originals.armed.store(true, Ordering::SeqCst);
    }
    let (_, completion) = machine
        .accept_input_with_completion(&session_id, input)
        .await
        .expect("real native admission");
    let completion = completion.expect("actual native completion waiter");
    tokio::time::timeout(Duration::from_secs(20), originals.in_flight.notified())
        .await
        .expect("the reviewed call entered and its helper step is admitted and held");
    assert_eq!(
        reviewer.0.load(Ordering::SeqCst),
        1,
        "one model review, spent at the entry"
    );

    // The host cancels the run while the step is held. The dispatch end hands
    // the attempt back without waiting, so the run reaches its cancelled
    // terminal with the gate still closed.
    meerkat::surface::interrupt_ephemeral_runtime_session(&machine, &session_id)
        .await
        .expect("the run is interrupted");
    let outcome = tokio::time::timeout(Duration::from_secs(10), completion.wait())
        .await
        .expect("the cancelled terminal is reported while the step is held")
        .expect("native completion observation");
    assert!(
        matches!(outcome, CompletionOutcome::Cancelled),
        "the run ends cancelled: {outcome:?}"
    );

    // Still held. A step started now is refused and commits nothing: the
    // runtime applied the cancellation before it reported the terminal, so
    // the run's custody refuses the ended run (MalformedFacts) before the
    // review owner is asked.
    let witness = leaf
        .witness
        .lock()
        .expect("witness")
        .take()
        .expect("the entry minted its witness");
    let further = FurtherOriginals {
        originals: no_contributors(),
        source: Arc::new(SourceOriginalBasis::new(originals.clone())),
    };
    let mut ran = false;
    let refused = witness.run_if_current_within(&further, &mut || ran = true);
    match &refused {
        Err(ToolError::AuthorizationRefused { refusal }) => {
            assert_eq!(
                refusal.kind(),
                OperationRefusalKind::MalformedFacts,
                "the ended run refuses the step"
            );
            assert_eq!(refusal.dispatch(), RefusalDispatch::Unknown);
        }
        other => panic!("a step after the cancellation is refused: {other:?}"),
    }
    assert!(!ran, "the refused step ran nothing");
    let error = refused.expect_err("refused");
    assert_eq!(error.to_error_payload()["data"]["tool_invoked"], "unknown");
    // The late admission goes to a graph of its own: the leaf's graph is the
    // held step's alone.
    let late_graph = meerkat::WorkGraphService::new(Arc::new(meerkat::MemoryWorkGraphStore::new()));
    let late_key = meerkat::WorkAdmissionKey::new("helper-after-the-cancellation").expect("key");
    let late_admission = late_graph
        .admission_identity(late_key.clone(), helper_request())
        .expect("identity");
    let late_fence = GovernedHelperAdmissionFence::from_entered(
        &witness,
        originals.clone(),
        no_contributors(),
        "default",
        meerkat::WorkNamespace::default(),
        late_admission,
    );
    assert!(
        late_graph
            .create_idempotent_with_fence(late_key, helper_request(), late_fence.clone())
            .await
            .is_err(),
        "the late admission commits nothing"
    );
    assert_eq!(late_fence.refusal(), Some(HelperAdmissionRefusal::Denied));
    drop(late_fence);
    assert!(
        late_graph
            .list(meerkat::WorkItemFilter {
                include_terminal: true,
                ..Default::default()
            })
            .await
            .expect("list")
            .is_empty(),
        "nothing committed after the cancellation"
    );
    let late_child = GovernedHelperChildIssuance::from_entered(
        &witness,
        originals.clone(),
        no_contributors(),
        child_intent(&helper_parent, "child-after-the-cancellation"),
    )
    .issue_or_recover();
    assert_eq!(late_child, Err(HelperChildIssuanceRefusal::Denied));
    assert_eq!(
        grants
            .issued_reference(&id("child-after-the-cancellation"))
            .expect("lookup"),
        None,
        "nothing issued after the cancellation"
    );
    assert_eq!(
        observer.used(REVIEWED_CALL),
        0,
        "no Used while the admitted step is held"
    );
    drop(witness);
    assert_eq!(
        observer.used(REVIEWED_CALL),
        0,
        "the held worker keeps the entered custody"
    );

    // The gate opens: the held step goes on, and the worker's last witness
    // clone releases the entered custody.
    originals.release();
    let worker = leaf
        .worker
        .lock()
        .expect("helper worker")
        .take()
        .expect("the entry started its helper worker");
    let answer = tokio::time::timeout(
        Duration::from_secs(20),
        tokio::task::spawn_blocking(move || worker.join()),
    )
    .await
    .expect("the helper worker finishes")
    .expect("join task")
    .expect("helper worker");
    let items = leaf
        .graph
        .list(meerkat::WorkItemFilter {
            include_terminal: true,
            ..Default::default()
        })
        .await
        .expect("list")
        .len();
    match held {
        HeldAt::BeforeTheRecheck => {
            // Held before its in-exclusion re-check: the work owners then
            // read the run's native accepted-work custody again, which the
            // cancellation ended, and refuse (MalformedFacts; the helper
            // fence answers Denied for it, as the late admission above). The
            // step commits nothing.
            let (committed, answer) = match answer {
                HeldStepAnswer::Step { created, answer } => (created, answer),
                other => panic!("the held worker runs a core step: {other:?}"),
            };
            match &answer {
                Err(ToolError::AuthorizationRefused { refusal }) => {
                    assert_eq!(
                        refusal.kind(),
                        OperationRefusalKind::MalformedFacts,
                        "the re-check inside the exclusion refuses the ended run"
                    );
                    assert_eq!(refusal.dispatch(), RefusalDispatch::Unknown);
                }
                other => panic!("the held step is refused at its re-check: {other:?}"),
            }
            let error = answer.expect_err("refused");
            assert_eq!(error.to_error_payload()["data"]["tool_invoked"], "unknown");
            assert!(!committed, "the refused step committed nothing");
            assert_eq!(items, 0, "the leaf's graph is empty");
        }
        HeldAt::InsideItsCommit => {
            // Past its re-check, the held commit keeps its effect.
            assert_eq!(
                answer,
                HeldStepAnswer::Step {
                    created: true,
                    answer: Ok(()),
                },
                "the held commit answers Created"
            );
            assert_eq!(items, 1, "exactly the held step's item");
        }
    }
    assert_eq!(
        observer.used(REVIEWED_CALL),
        1,
        "Used is reported once, after the worker's last witness clone"
    );
    assert_eq!(
        leaf.bodies.load(Ordering::SeqCst),
        0,
        "the cancelled call's body never returned"
    );
}
