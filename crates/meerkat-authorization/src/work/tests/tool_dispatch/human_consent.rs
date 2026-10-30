//! Native human consent through the real fenced tool entry (ADR-001): an R3
//! call settles locally with one consent approval while a permitted sibling
//! enters; an authenticated, eligible decision only records it; the fresh
//! explicit attempt of the retained candidate enters exactly once, before
//! its body counter. Caller text, ineligible approvers, copied references,
//! new equal-looking calls, expired consent and revoked permission never
//! enter, and a failure before entry keeps the consent spendable. An
//! unrelated owner change keeps the consent; a related one needs fresh
//! consent (ADR-001 D9).

use meerkat_core::approval::consent::{
    AuthenticatedApprover, AuthenticatedConsentDecision, ConsentEligibility,
    ConsentEligibilityAnswer, ConsentIneligible, ConsentNotice, ConsentNoticeKind,
    ConsentPresentation, ConsentRouteUnavailable, HumanConsentRoute,
};

use crate::publication::{LocalAuthorizationPublication, PublishedConsentEligibility};
use meerkat_core::approval::review::{
    BoundOperationReview, OperationReviewTier, OperationReviewer, ReviewCandidate,
    ReviewEntrySupport, ReviewVerdict, ReviewerFailure,
};
use meerkat_core::authorization::{
    PolicyPublicationObservation, PreparedAuthorizationBinding, PreparedOperationAuthorization,
    ReviewOperationContextResolution, WorkAuthorization, WorkAuthorizationContext,
};
use meerkat_core::{
    ApprovalAssurance, ApprovalDecision, ApprovalError, ApprovalId, ApprovalListFilter,
    ApprovalPrincipalId, ApprovalRecord, ApprovalRisk, ApprovalService, ApprovalStatus,
    PrincipalKind, PrincipalRef, ReviewUnsatisfiedKind, SurfaceMetadata, TrustDomainId,
    dispatch_retained_consent_attempt,
};

use super::*;

const CONSENTED_TOOL: &str = "write_record";
const SIBLING_TOOL: &str = "read_record";
/// Host-chosen validity of one consent request.
const VALIDITY: Duration = Duration::from_secs(4 * 60 * 60);

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

/// `work`'s own owner, which publishes one completed owner change keeping
/// every permission, tier and material right after the armed step: its
/// first preparation, or its first review material read (immediately before
/// any human presentation).
struct ChurningWork {
    inner: Arc<dyn WorkAuthorization>,
    publication: LocalAuthorizationPublication,
    after_prepare: AtomicBool,
    after_material_read: AtomicBool,
}

impl ChurningWork {
    fn republish_if(&self, armed: &AtomicBool) {
        if armed.swap(false, Ordering::SeqCst) {
            drop(
                self.publication
                    .begin_owner_change()
                    .expect("owner publication"),
            );
        }
    }
}

impl WorkAuthorization for ChurningWork {
    fn review_operation_context(
        &self,
        binding: &PreparedAuthorizationBinding,
    ) -> Result<ReviewOperationContextResolution, meerkat_core::OperationAuthorizationError> {
        let read = self.inner.review_operation_context(binding);
        self.republish_if(&self.after_material_read);
        read
    }

    fn controller_model_selection(&self) -> Option<meerkat_core::ControllerModelSelection> {
        self.inner.controller_model_selection()
    }

    fn prepare(
        &self,
        binding: &PreparedAuthorizationBinding,
    ) -> Result<Arc<dyn PreparedOperationAuthorization>, meerkat_core::OperationAuthorizationError>
    {
        let prepared = self.inner.prepare(binding);
        self.republish_if(&self.after_prepare);
        prepared
    }
}

/// Passive values actually received at the human presentation seam.
struct PresentedCandidate {
    tool_call_id: String,
    tool_name: String,
    arguments: String,
    facts: OperationAuthorizationFacts,
    policy_observation: Option<PolicyPublicationObservation>,
    operation_context: Option<(String, String)>,
}

/// Host route: its eligibility owner (initially only `lead-1`) publishes
/// every change through one local authorization publication; notices are
/// recorded.
struct Route {
    notices: Mutex<Vec<(ApprovalId, ConsentNoticeKind)>>,
    presented: Mutex<Option<Vec<PresentedCandidate>>>,
    unavailable: AtomicBool,
    publication: LocalAuthorizationPublication,
    eligible: Mutex<Vec<String>>,
    /// Revokes the approver right after its eligibility was resolved, before
    /// the decision commits.
    revoke_after_resolve: AtomicBool,
    /// The host's self-approval policy: the requester may decide their own
    /// approval while set.
    self_approval: AtomicBool,
}

impl Default for Route {
    fn default() -> Self {
        Self {
            notices: Mutex::default(),
            presented: Mutex::default(),
            unavailable: AtomicBool::new(false),
            publication: LocalAuthorizationPublication::new(),
            eligible: Mutex::new(vec!["lead-1".to_string()]),
            revoke_after_resolve: AtomicBool::new(false),
            self_approval: AtomicBool::new(false),
        }
    }
}

impl Route {
    /// One owner mutation, published before it becomes visible.
    fn change_eligibility(&self, change: impl FnOnce(&mut Vec<String>)) {
        let _published = self.publication.begin_owner_change().expect("publication");
        change(&mut self.eligible.lock().expect("eligible"));
    }

    /// Set the self-approval policy, published as an owner change.
    fn permit_self_approval(&self, permitted: bool) {
        let _published = self.publication.begin_owner_change().expect("publication");
        self.self_approval.store(permitted, Ordering::SeqCst);
    }
}

/// An in-memory approval store that runs a hook once inside its next write.
#[derive(Default)]
struct HookedStore {
    inner: meerkat_core::InMemoryApprovalStore,
    hook: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

impl meerkat_core::ApprovalStore for HookedStore {
    fn load_all(&self) -> Result<Vec<ApprovalRecord>, meerkat_core::ApprovalStoreError> {
        self.inner.load_all()
    }

    fn get(
        &self,
        approval_id: &ApprovalId,
    ) -> Result<Option<ApprovalRecord>, meerkat_core::ApprovalStoreError> {
        self.inner.get(approval_id)
    }

    fn put_if_current(
        &self,
        expected: Option<&ApprovalRecord>,
        record: &ApprovalRecord,
    ) -> Result<(), meerkat_core::ApprovalStoreError> {
        let hook = self.hook.lock().expect("hook").take();
        if let Some(hook) = hook {
            hook();
        }
        self.inner.put_if_current(expected, record)
    }

    fn is_persistent(&self) -> bool {
        self.inner.is_persistent()
    }
}

impl HumanConsentRoute for Route {
    fn present(
        &self,
        candidate: &meerkat_core::approval::consent::ConsentPresentationCandidate<'_>,
    ) -> Result<ConsentPresentation, ConsentRouteUnavailable> {
        if self.unavailable.load(Ordering::SeqCst) {
            return Err(ConsentRouteUnavailable);
        }
        if let Some(presented) = self.presented.lock().expect("presented").as_mut() {
            presented.push(PresentedCandidate {
                tool_call_id: candidate.tool_call_id().to_owned(),
                tool_name: candidate.tool_name().to_owned(),
                arguments: candidate.arguments().get().to_owned(),
                facts: candidate.facts().clone(),
                policy_observation: candidate.policy_observation(),
                operation_context: candidate.operation_context().map(|context| {
                    (
                        context.action_account().unwrap_or_default().to_owned(),
                        context.mandate().unwrap_or_default().to_owned(),
                    )
                }),
            });
        }
        Ok(ConsentPresentation {
            requester: ApprovalPrincipalId::new("human:child").expect("principal"),
            risk: ApprovalRisk::High,
            metadata: SurfaceMetadata::default(),
        })
    }

    fn eligibility(
        &self,
        approver: &AuthenticatedApprover,
        approval: &ApprovalRecord,
    ) -> Result<Arc<dyn ConsentEligibility>, ConsentIneligible> {
        match self.eligibility_answer(approver, approval) {
            ConsentEligibilityAnswer::Eligible(evidence) => Ok(evidence),
            _ => Err(ConsentIneligible),
        }
    }

    /// The eligibility owner's answer from a coherent observation of its
    /// publication: an owner change in progress, or one published during
    /// the read, is `Unsettled`.
    fn eligibility_answer(
        &self,
        approver: &AuthenticatedApprover,
        approval: &ApprovalRecord,
    ) -> ConsentEligibilityAnswer {
        let id = approver.principal().id.as_str().to_owned();
        let requester = approval.requester.as_str() == id;
        let answer = PublishedConsentEligibility::observe_answer(&self.publication, || {
            if requester {
                self.self_approval.load(Ordering::SeqCst)
            } else {
                self.eligible.lock().expect("eligible").contains(&id)
            }
        });
        if matches!(answer, ConsentEligibilityAnswer::Eligible(_))
            && self.revoke_after_resolve.swap(false, Ordering::SeqCst)
        {
            self.change_eligibility(|eligible| eligible.retain(|eligible| *eligible != id));
        }
        answer
    }

    fn notice(&self, notice: ConsentNotice) {
        self.notices
            .lock()
            .expect("notices")
            .push((notice.approval_id, notice.kind));
    }
}

/// The effect leaf: declares `ConsumesAtEntry` and spends exactly once,
/// immediately before the shared body counter. It can fail during its own
/// preparation, before the entry step.
struct ConsentLeaf {
    body: Arc<RecordingDispatcher>,
    fail_before_entry: AtomicBool,
}

#[async_trait]
impl AgentToolDispatcher for ConsentLeaf {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        self.body.tools()
    }

    fn review_entry_support(&self, _tool_name: &str) -> ReviewEntrySupport {
        ReviewEntrySupport::ConsumesAtEntry
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        self.dispatch_with_context(call, &ToolDispatchContext::default())
            .await
    }

    async fn dispatch_with_context(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        if self.fail_before_entry.swap(false, Ordering::SeqCst) {
            return Err(ToolError::execution_failed("leaf preparation failed"));
        }
        let entered = context.enter_reviewed_effect(call, None)?;
        self.body
            .dispatch_with_context(call, entered.as_ref().unwrap_or(context))
            .await
    }
}

struct ConsentWorld {
    work: ToolWork,
    approvals: ApprovalService,
    route: Arc<Route>,
    reviewer: Arc<NoReviewer>,
    review: Arc<BoundOperationReview>,
    body: Arc<RecordingDispatcher>,
    leaf: Arc<ConsentLeaf>,
}

impl ConsentWorld {
    /// Native work where `write private` is ALLOWED at R3 and `read public`
    /// at R1, with a host human route composed.
    fn new() -> Self {
        Self::build(false, None)
    }

    /// [`Self::new`] whose route's eligibility owner publishes through the
    /// work's own publication, so every owner change of the work, related
    /// or not, makes its eligibility evidence stale; `store` backs the
    /// approvals when given.
    fn with_eligibility_on_work_publication(
        store: Option<Arc<dyn meerkat_core::ApprovalStore>>,
    ) -> Self {
        Self::build(true, store)
    }

    fn build(
        eligibility_on_work: bool,
        store: Option<Arc<dyn meerkat_core::ApprovalStore>>,
    ) -> Self {
        let work = ToolWork::for_session(&SessionId::new());
        work.set_write_tier(OperationReviewTier::R3);
        let approvals = match store {
            Some(store) => ApprovalService::with_store(store).expect("approval store"),
            None => ApprovalService::new(),
        };
        let route = Arc::new(if eligibility_on_work {
            Route {
                publication: work.publication.clone(),
                ..Route::default()
            }
        } else {
            Route::default()
        });
        let reviewer = Arc::new(NoReviewer::default());
        let review = Arc::new(
            BoundOperationReview::new(reviewer.clone(), approvals.clone(), Duration::from_secs(30))
                .with_human_consent(route.clone(), VALIDITY),
        );
        let body = Arc::new(RecordingDispatcher::default());
        let leaf = Arc::new(ConsentLeaf {
            body: body.clone(),
            fail_before_entry: AtomicBool::new(false),
        });
        Self {
            work,
            approvals,
            route,
            reviewer,
            review,
            body,
            leaf,
        }
    }

    fn context(&self) -> ToolDispatchContext {
        self.context_for(&self.work)
    }

    fn context_for(&self, work: &ToolWork) -> ToolDispatchContext {
        ToolDispatchContext::default()
            .with_work_authorization(Some(work.context.clone()))
            .with_operation_review(Some(self.review.clone()))
    }

    fn plan(&self, call: &Call) -> meerkat_core::ResolvedToolExecutionPlan {
        resolve_tool_execution_plan_fenced(
            &self.leaf,
            call.view(),
            &self.context(),
            &ToolExecutionResolutionContext::new(
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

    async fn fenced(&self, call: &Call) -> Result<ToolDispatchOutcome, ToolError> {
        let plan = self.plan(call);
        dispatch_tool_execution_plan_fenced(&self.leaf, call.view(), &self.context(), &plan).await
    }

    /// The consented call settles locally; the host learns the approval
    /// from the route's notice.
    async fn request(&self, id: &str) -> ApprovalId {
        assert_human_consent_required(self.fenced(&consented_call(id)).await);
        let notices = self.route.notices.lock().expect("notices");
        let (approval_id, kind) = notices.last().cloned().expect("requested notice");
        assert_eq!(kind, ConsentNoticeKind::Requested);
        approval_id
    }

    fn approve(&self, approval_id: &ApprovalId) -> ApprovalRecord {
        self.approvals
            .decide_consent(approval_id, decision(ApprovalDecision::Approve, "lead-1"))
            .expect("eligible authenticated approval")
    }

    async fn attempt(&self, approval_id: &ApprovalId) -> Result<ToolDispatchOutcome, ToolError> {
        dispatch_retained_consent_attempt(&self.leaf, &self.context(), approval_id).await
    }

    /// A fenced call under `self.work`'s owner wrapped to publish one
    /// completed change right after its first preparation or its first
    /// review material read.
    async fn fenced_churning(
        &self,
        call: &Call,
        after_prepare: bool,
        after_material_read: bool,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        let churning = Arc::new(ChurningWork {
            inner: self.work.context.authorization().clone(),
            publication: self.work.publication.clone(),
            after_prepare: AtomicBool::new(after_prepare),
            after_material_read: AtomicBool::new(after_material_read),
        });
        let context = ToolDispatchContext::default()
            .with_work_authorization(Some(WorkAuthorizationContext::new(
                churning.clone(),
                self.work.context.execution_scope().clone(),
            )))
            .with_operation_review(Some(self.review.clone()));
        let plan = self.plan(call);
        let result =
            dispatch_tool_execution_plan_fenced(&self.leaf, call.view(), &context, &plan).await;
        assert!(
            !churning.after_prepare.load(Ordering::SeqCst)
                && !churning.after_material_read.load(Ordering::SeqCst),
            "every armed owner change was published"
        );
        result
    }

    fn bodies(&self) -> Vec<String> {
        body_ids(&self.body)
    }

    fn notices(&self, approval_id: &ApprovalId) -> Vec<ConsentNoticeKind> {
        self.route
            .notices
            .lock()
            .expect("notices")
            .iter()
            .filter(|(id, _)| id == approval_id)
            .map(|(_, kind)| *kind)
            .collect()
    }
}

fn consented_call(id: &str) -> Call {
    // `write private` is natively permitted by the fixture authority.
    Call::new(id, CONSENTED_TOOL, "private")
}

fn sibling_call(id: &str) -> Call {
    // `read public` is natively permitted by the fixture authority, at R1.
    Call::new(id, SIBLING_TOOL, "public")
}

fn body_ids(body: &RecordingDispatcher) -> Vec<String> {
    body.bodies
        .lock()
        .expect("bodies")
        .iter()
        .map(|(id, _, _)| id.clone())
        .collect()
}

fn decision(decision: ApprovalDecision, id: &str) -> AuthenticatedConsentDecision {
    AuthenticatedConsentDecision {
        approver: AuthenticatedApprover::new(
            PrincipalRef::in_domain(
                PrincipalKind::Human,
                id,
                TrustDomainId::new("team").expect("domain"),
            )
            .expect("principal"),
            ApprovalAssurance::PersonAuthenticated,
        )
        .expect("qualified approver"),
        decision,
        reason: None,
    }
}

fn assert_human_consent_required(result: Result<ToolDispatchOutcome, ToolError>) {
    assert!(
        matches!(
            result,
            Err(ToolError::ReviewUnsatisfied {
                kind: ReviewUnsatisfiedKind::HumanConsentRequired,
                ..
            })
        ),
        "expected local human_consent_required, got {result:?}"
    );
}

#[tokio::test]
async fn an_r3_call_settles_locally_with_one_approval_and_its_sibling_enters() {
    let w = ConsentWorld::new();
    *w.route.presented.lock().expect("presented") = Some(Vec::new());
    // The owners' actual selected account and current mandate.
    const ACCOUNT: &str = "team-utilities-checking";
    const MANDATE: &str = "Pay the monthly electricity bill, at most 150 EUR";
    w.work.set_review_material(ReviewMaterial::Held {
        account: ACCOUNT.to_owned(),
        mandate: MANDATE.to_owned(),
    });
    let mut call = consented_call("consented");
    // Preserve raw arguments at presentation, not a parsed/reserialized copy.
    call.args =
        RawValue::from_string("{ \"namespace\" : \"private\" }".to_owned()).expect("raw arguments");
    let plan = w.plan(&call);
    let ((), publication) = w
        .work
        .publication
        .observe(|| ())
        .expect("native publication");
    let expected_observation = publication.policy_observation();
    let refusal = dispatch_tool_execution_plan_fenced(&w.leaf, call.view(), &w.context(), &plan)
        .await
        .expect_err("pending consent settles this call locally");
    let feedback = refusal.to_error_payload();
    assert_eq!(feedback["error"], "review_unsatisfied");
    assert_eq!(feedback["data"]["kind"], "human_consent_required");
    let notices = w.route.notices.lock().expect("notices").clone();
    assert_eq!(notices.len(), 1, "one actual requested notice");
    let (approval_id, kind) = &notices[0];
    assert_eq!(*kind, ConsentNoticeKind::Requested);
    assert!(w.bodies().is_empty(), "zero entry while consent is pending");

    w.fenced(&sibling_call("sibling"))
        .await
        .expect("permitted R1 sibling enters");
    assert_eq!(w.bodies(), vec!["sibling".to_string()]);

    // Take the recorded facts: their dispatcher target retains the effect leaf.
    let presented = w
        .route
        .presented
        .lock()
        .expect("presented")
        .take()
        .expect("recording enabled");
    assert_eq!(presented.len(), 1, "only the R3 candidate is presented");
    let candidate = &presented[0];
    assert_eq!(candidate.tool_call_id, call.id);
    assert_eq!(candidate.tool_name, call.name);
    assert_eq!(candidate.arguments, call.args.get());
    assert_eq!(candidate.facts.execution_scope, w.work.owner.native_scope);
    assert!(candidate.facts.run_id.is_none());
    assert!(candidate.facts.context_revision.is_none());
    let AuthorizationOperation::Tool(tool) = &candidate.facts.operation else {
        unreachable!("the human route must receive the actual prepared tool operation");
    };
    assert_eq!(tool.call_id.as_ref(), call.id);
    assert_eq!(tool.name.as_str(), call.name);
    assert_eq!(tool.arguments.get(), call.args.get());
    let ToolAuthorizationTarget::Dispatcher(presented_plan) = &tool.target else {
        unreachable!("the human route must retain the actual dispatcher target");
    };
    assert_eq!(presented_plan.as_ref(), &plan);
    assert_eq!(candidate.policy_observation, Some(expected_observation));
    assert_eq!(
        candidate.operation_context,
        Some((ACCOUNT.to_owned(), MANDATE.to_owned())),
        "the presentation carries the owners' literal account and mandate"
    );
    assert!(!candidate.arguments.contains(ACCOUNT) && !candidate.arguments.contains(MANDATE));
    assert_eq!(
        w.work.review_context_calls(),
        1,
        "read once for the R3 candidate and never for the R1 sibling"
    );

    let approvals = w
        .approvals
        .list(ApprovalListFilter::default())
        .expect("approvals");
    assert_eq!(approvals.len(), 1, "one approval for one candidate");
    let record = &approvals[0];
    assert_eq!(&record.approval_id, approval_id);
    assert_eq!(record.status, ApprovalStatus::Pending);
    assert_eq!(
        record.proposed_action.body,
        Some(serde_json::json!({"namespace": "private"}))
    );
    let history = serde_json::to_string(record).expect("approval record JSON");
    assert!(
        !history.contains(ACCOUNT) && !history.contains(MANDATE),
        "the material is presentation input, not stored approval history"
    );
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&refusal.to_transcript_content())
            .expect("canonical tool feedback JSON"),
        feedback,
    );
    assert_eq!(
        feedback["data"]["approval_id"],
        serde_json::json!(approval_id),
        "the local feedback identifies the exact routed and stored approval",
    );
}

#[tokio::test]
async fn r3_work_owner_without_the_context_read_fails_closed_before_any_approval() {
    let w = ConsentWorld::new();
    *w.route.presented.lock().expect("presented") = Some(Vec::new());
    let context = ToolDispatchContext::default()
        .with_work_authorization(Some(w.work.inheriting_context()))
        .with_operation_review(Some(w.review.clone()));
    let call = consented_call("consented");
    let plan = resolve_tool_execution_plan_fenced(
        &w.leaf,
        call.view(),
        &context,
        &ToolExecutionResolutionContext::new(
            meerkat_core::ToolDeadlineChain::new(vec![
                meerkat_core::ToolDeadlineContributor::finite(
                    meerkat_core::ToolDeadlineOwner::DirectCaller,
                    Duration::from_secs(600),
                ),
            ])
            .expect("finite caller deadline"),
        ),
    )
    .expect("real plan resolution");
    let result = dispatch_tool_execution_plan_fenced(&w.leaf, call.view(), &context, &plan).await;
    assert!(
        matches!(
            &result,
            Err(ToolError::ReviewUnavailable {
                kind: meerkat_core::ReviewUnavailableKind::ContextUnavailable
            })
        ),
        "{result:?}"
    );
    assert!(
        w.approvals
            .list(ApprovalListFilter::default())
            .expect("approvals")
            .is_empty(),
        "no approval"
    );
    assert!(
        w.route
            .presented
            .lock()
            .expect("presented")
            .as_ref()
            .is_some_and(Vec::is_empty),
        "nothing presented"
    );
    assert!(w.route.notices.lock().expect("notices").is_empty());
    assert!(w.bodies().is_empty(), "no target entry");
    let sibling = sibling_call("sibling");
    let sibling_plan = resolve_tool_execution_plan_fenced(
        &w.leaf,
        sibling.view(),
        &context,
        &ToolExecutionResolutionContext::new(
            meerkat_core::ToolDeadlineChain::new(vec![
                meerkat_core::ToolDeadlineContributor::finite(
                    meerkat_core::ToolDeadlineOwner::DirectCaller,
                    Duration::from_secs(600),
                ),
            ])
            .expect("finite caller deadline"),
        ),
    )
    .expect("real plan resolution");
    dispatch_tool_execution_plan_fenced(&w.leaf, sibling.view(), &context, &sibling_plan)
        .await
        .expect("permitted R1 sibling enters");
    assert_eq!(w.bodies(), vec!["sibling".to_string()]);
}

/// A model reviewer that never answers, so the owner deadline decides.
struct SilentReviewer {
    entered: tokio::sync::Notify,
}

#[async_trait]
impl OperationReviewer for SilentReviewer {
    async fn review(
        &self,
        _candidate: &ReviewCandidate<'_>,
    ) -> Result<ReviewVerdict, ReviewerFailure> {
        self.entered.notify_one();
        std::future::pending().await
    }
}

#[tokio::test(start_paused = true)]
async fn r2_deadline_fallback_never_presents_material_changed_during_review() {
    let mut w = ConsentWorld::new();
    w.work.set_write_tier(OperationReviewTier::R2);
    w.work.set_review_material(ReviewMaterial::Held {
        account: "account-a".to_owned(),
        mandate: "mandate-a".to_owned(),
    });
    let reviewer = Arc::new(SilentReviewer {
        entered: tokio::sync::Notify::new(),
    });
    w.review = Arc::new(
        BoundOperationReview::new(
            reviewer.clone(),
            w.approvals.clone(),
            Duration::from_secs(30),
        )
        .with_human_consent(w.route.clone(), VALIDITY),
    );
    *w.route.presented.lock().expect("presented") = Some(Vec::new());
    let call = consented_call("reviewed");
    let edit = async {
        reviewer.entered.notified().await;
        // Only the owner material changes; permission and tier stay.
        w.work.set_review_material(ReviewMaterial::Held {
            account: "account-b".to_owned(),
            mandate: "mandate-b".to_owned(),
        });
    };
    let (result, ()) = tokio::join!(w.fenced(&call), edit);
    assert!(
        matches!(
            &result,
            Err(ToolError::ReviewUnsatisfied {
                kind: ReviewUnsatisfiedKind::ContextChanged,
                ..
            })
        ),
        "{result:?}"
    );
    let presented = w
        .route
        .presented
        .lock()
        .expect("presented")
        .take()
        .expect("recording enabled");
    assert!(presented.is_empty(), "stale material is never presented");
    assert!(
        w.approvals
            .list(ApprovalListFilter::default())
            .expect("approvals")
            .is_empty(),
        "no approval for stale material"
    );
    assert!(w.bodies().is_empty(), "no target entry");
    w.fenced(&sibling_call("sibling"))
        .await
        .expect("permitted R1 sibling enters");
    assert_eq!(w.bodies(), vec!["sibling".to_string()]);
    assert_eq!(
        w.work.review_context_calls(),
        1,
        "read once, never for the sibling"
    );
}

#[tokio::test]
async fn r3_context_outcomes_are_typed_before_any_approval() {
    for material in [
        ReviewMaterial::RequiredButMissing,
        ReviewMaterial::Denied,
        ReviewMaterial::NotApplicable,
    ] {
        let w = ConsentWorld::new();
        *w.route.presented.lock().expect("presented") = Some(Vec::new());
        w.work.set_review_material(material.clone());
        let result = w.fenced(&consented_call("consented")).await;
        let approvals = w
            .approvals
            .list(ApprovalListFilter::default())
            .expect("approvals");
        let presented = w
            .route
            .presented
            .lock()
            .expect("presented")
            .take()
            .expect("recording enabled");
        match material {
            ReviewMaterial::RequiredButMissing | ReviewMaterial::Denied => {
                if matches!(material, ReviewMaterial::RequiredButMissing) {
                    assert!(
                        matches!(
                            &result,
                            Err(ToolError::ReviewUnavailable {
                                kind: meerkat_core::ReviewUnavailableKind::ContextUnavailable
                            })
                        ),
                        "{result:?}"
                    );
                } else {
                    assert!(
                        matches!(&result, Err(ToolError::AuthorizationRefused { refusal })
                            if refusal.kind() == OperationRefusalKind::Denied),
                        "{result:?}"
                    );
                }
                let feedback = result.expect_err("local refusal").to_error_payload();
                assert!(
                    feedback["data"].get("approval_id").is_none(),
                    "no reference"
                );
                assert!(approvals.is_empty(), "no approval");
                assert!(presented.is_empty(), "nothing presented");
                assert!(w.route.notices.lock().expect("notices").is_empty());
            }
            ReviewMaterial::NotApplicable => {
                assert_human_consent_required(result);
                assert_eq!(approvals.len(), 1);
                assert_eq!(presented.len(), 1);
                assert_eq!(presented[0].operation_context, None);
            }
            ReviewMaterial::Held { .. } => unreachable!("not exercised here"),
        }
        assert!(w.bodies().is_empty(), "no target entry");
        w.fenced(&sibling_call("sibling"))
            .await
            .expect("permitted R1 sibling enters");
        assert_eq!(w.bodies(), vec!["sibling".to_string()]);
        assert_eq!(
            w.work.review_context_calls(),
            1,
            "once, never for the sibling"
        );
    }
}

#[tokio::test]
async fn missing_or_unavailable_human_routes_emit_no_approval_reference() {
    for missing_route in [true, false] {
        let mut w = ConsentWorld::new();
        let (code, kind) = if missing_route {
            w.review = Arc::new(BoundOperationReview::new(
                w.reviewer.clone(),
                w.approvals.clone(),
                Duration::from_secs(30),
            ));
            ("review_unsatisfied", "human_consent_required")
        } else {
            w.route.unavailable.store(true, Ordering::SeqCst);
            ("review_unavailable", "owner_unavailable")
        };
        let refusal = w
            .fenced(&consented_call("unreferenced"))
            .await
            .expect_err("an absent or unavailable route settles locally");
        let feedback = refusal.to_error_payload();
        assert_eq!(feedback["error"], code);
        assert_eq!(feedback["data"]["kind"], kind);
        assert!(
            feedback["data"].get("approval_id").is_none(),
            "no retained request means no approval reference, not even null",
        );
        assert!(w.route.notices.lock().expect("notices").is_empty());
        assert!(
            w.approvals
                .list(ApprovalListFilter::default())
                .expect("approvals")
                .is_empty(),
        );
        assert!(w.bodies().is_empty(), "the refused call did not enter");
        w.fenced(&sibling_call("sibling"))
            .await
            .expect("permitted R1 sibling still enters");
        assert_eq!(w.bodies(), vec!["sibling".to_string()]);
    }
}

#[tokio::test]
async fn only_an_authenticated_eligible_decision_records_and_it_executes_nothing() {
    let w = ConsentWorld::new();
    let approval_id = w.request("consented").await;

    assert_eq!(
        w.approvals.decide(
            &approval_id,
            ApprovalDecision::Approve,
            ApprovalPrincipalId::new("human:lead-1").expect("principal"),
            Some("relayed by the agent".to_string()),
            None,
        ),
        Err(ApprovalError::AuthenticationRequired {
            approval_id: approval_id.clone()
        }),
        "caller-supplied actor text is not authentication"
    );
    assert_eq!(
        w.approvals.decide_consent(
            &approval_id,
            decision(ApprovalDecision::Approve, "channel-member")
        ),
        Err(ApprovalError::NotEligible {
            approval_id: approval_id.clone()
        }),
        "an authenticated channel member is not an eligible approver"
    );

    let approved = w.approve(&approval_id);
    assert_eq!(approved.status, ApprovalStatus::Approved);
    assert!(
        w.bodies().is_empty(),
        "recording the decision executed nothing"
    );
    assert_eq!(
        w.notices(&approval_id),
        vec![ConsentNoticeKind::Requested, ConsentNoticeKind::Approved]
    );
}

#[tokio::test]
async fn the_fresh_retained_attempt_enters_exactly_once() {
    let w = ConsentWorld::new();
    let approval_id = w.request("consented").await;
    w.approve(&approval_id);

    w.attempt(&approval_id)
        .await
        .expect("the retained candidate enters");
    assert_eq!(
        w.bodies(),
        vec!["consented".to_string()],
        "the retained binding-owned call, once"
    );
    let second = w.attempt(&approval_id).await;
    assert!(
        matches!(
            second,
            Err(ToolError::ReviewUnsatisfied {
                kind: ReviewUnsatisfiedKind::AlreadyConsumed,
                ..
            })
        ),
        "expected already consumed, got {second:?}"
    );
    assert_eq!(w.bodies().len(), 1, "a second attempt never enters");
    assert_eq!(
        w.notices(&approval_id),
        vec![
            ConsentNoticeKind::Requested,
            ConsentNoticeKind::Approved,
            ConsentNoticeKind::Used,
        ]
    );
    assert!(
        w.approvals
            .get(&approval_id)
            .expect("record")
            .consent
            .and_then(|consent| consent.consumed_at)
            .is_some()
    );
}

#[tokio::test]
async fn an_r3_call_without_a_route_under_owner_churn_settles_as_before() {
    let mut w = ConsentWorld::new();
    w.review = Arc::new(BoundOperationReview::new(
        w.reviewer.clone(),
        w.approvals.clone(),
        Duration::from_secs(30),
    ));

    // A completed owner change right after the call's first preparation:
    // the R3 call settles exactly as without it, and nothing reviews it.
    let refused = w
        .fenced_churning(&consented_call("churned"), true, false)
        .await;
    assert_human_consent_required(refused);
    assert_eq!(
        w.reviewer.0.load(Ordering::SeqCst),
        0,
        "never model-reviewed"
    );
    assert!(w.route.notices.lock().expect("notices").is_empty());
    assert!(
        w.approvals
            .list(ApprovalListFilter::default())
            .expect("approvals")
            .is_empty()
    );
    assert!(w.bodies().is_empty(), "zero entry");
}

#[tokio::test]
async fn an_owner_change_before_r3_presentation_is_never_presented_or_reviewed() {
    let w = ConsentWorld::new();
    *w.route.presented.lock().expect("presented") = Some(Vec::new());

    // A completed owner change after the material read, before the human
    // presentation's currentness check: nothing is presented or requested,
    // and nothing reviews or requests the call again.
    let refused = w
        .fenced_churning(&consented_call("churned"), false, true)
        .await;
    assert!(
        matches!(
            refused,
            Err(ToolError::ReviewUnsatisfied {
                kind: ReviewUnsatisfiedKind::ContextChanged,
                ..
            })
        ),
        "{refused:?}"
    );
    assert_eq!(
        w.reviewer.0.load(Ordering::SeqCst),
        0,
        "never model-reviewed"
    );
    let presented = w
        .route
        .presented
        .lock()
        .expect("presented")
        .take()
        .expect("recording enabled");
    assert!(presented.is_empty(), "nothing presented");
    assert!(w.route.notices.lock().expect("notices").is_empty());
    assert!(
        w.approvals
            .list(ApprovalListFilter::default())
            .expect("approvals")
            .is_empty()
    );
    assert!(w.bodies().is_empty(), "zero entry");
    assert_eq!(w.work.review_context_calls(), 1, "read once, never again");
}

/// ADR-001 D9 with the native owners: an owner change that keeps every
/// permission, tier and material (an unrelated one) while the consent is
/// pending leaves it pending, and another after the approval makes the
/// approved decision stale before the attempt's entry staging. The attempt
/// re-adjudicates it and enters exactly once; nothing reviews or requests it
/// again.
#[tokio::test]
async fn an_unrelated_owner_change_keeps_the_consent_and_the_attempt_enters_once() {
    let w = ConsentWorld::new();
    w.work.set_review_material(ReviewMaterial::Held {
        account: "team-utilities-checking".to_owned(),
        mandate: "Pay the monthly electricity bill".to_owned(),
    });
    let approval_id = w.request("consented").await;

    w.work.republish();
    assert_eq!(
        w.approvals.get(&approval_id).expect("record").status,
        ApprovalStatus::Pending,
        "an unrelated change keeps the pending consent"
    );
    w.approve(&approval_id);
    w.work.republish();
    w.attempt(&approval_id)
        .await
        .expect("the re-adjudicated consent enters");
    assert_eq!(w.bodies(), vec!["consented".to_string()], "exactly once");
    assert_eq!(
        w.reviewer.0.load(Ordering::SeqCst),
        0,
        "never model-reviewed"
    );
    let notices = w.notices(&approval_id);
    assert_eq!(
        notices,
        vec![
            ConsentNoticeKind::Requested,
            ConsentNoticeKind::Approved,
            ConsentNoticeKind::Used,
        ],
        "no second consent request, never withdrawn"
    );
    let second = w.attempt(&approval_id).await;
    assert!(
        matches!(
            second,
            Err(ToolError::ReviewUnsatisfied {
                kind: ReviewUnsatisfiedKind::AlreadyConsumed,
                ..
            })
        ),
        "{second:?}"
    );
    assert_eq!(w.bodies().len(), 1, "a second attempt never enters");
}

/// ADR-001 D9: a related owner change refuses the attempt with zero entry
/// and withdraws the consent, as `context_changed`. Pending, a changed
/// review tier withdraws it on the next read; approved, changed review
/// material refuses the attempt itself. Nothing reviews or requests it
/// again.
#[tokio::test]
async fn a_related_owner_change_refuses_the_attempt_with_zero_entry() {
    // Pending: the owners now resolve another review tier for it.
    let w = ConsentWorld::new();
    let approval_id = w.request("consented").await;
    w.work.set_write_tier(OperationReviewTier::R2);
    let record = w.approvals.get(&approval_id).expect("record");
    assert_eq!(record.status, ApprovalStatus::Cancelled);
    assert!(
        matches!(
            record.consent.and_then(|consent| consent.withdrawal),
            Some(withdrawal) if withdrawal.reason
                == meerkat_core::approval::ApprovalConsentWithdrawalReason::ContextChanged
        ),
        "withdrawn as context_changed"
    );
    assert_human_consent_required(w.attempt(&approval_id).await);

    // Approved: the owners now present other review material.
    let w = ConsentWorld::new();
    w.work.set_review_material(ReviewMaterial::Held {
        account: "account-a".to_owned(),
        mandate: "mandate-a".to_owned(),
    });
    let approval_id = w.request("consented").await;
    w.approve(&approval_id);
    w.work.set_review_material(ReviewMaterial::Held {
        account: "account-b".to_owned(),
        mandate: "mandate-a".to_owned(),
    });
    let refused = w.attempt(&approval_id).await;
    assert!(
        matches!(
            refused,
            Err(ToolError::ReviewUnsatisfied {
                kind: ReviewUnsatisfiedKind::ContextChanged,
                ..
            })
        ),
        "{refused:?}"
    );
    assert!(w.bodies().is_empty(), "zero entry");
    assert_eq!(
        w.reviewer.0.load(Ordering::SeqCst),
        0,
        "never model-reviewed"
    );
    let notices = w.notices(&approval_id);
    assert!(!notices.contains(&ConsentNoticeKind::Used), "{notices:?}");
    assert_eq!(
        notices
            .iter()
            .filter(|kind| **kind == ConsentNoticeKind::Requested)
            .count(),
        1,
        "no second consent request: {notices:?}"
    );
    assert_human_consent_required(w.attempt(&approval_id).await);
    assert!(w.bodies().is_empty(), "fresh consent is required");
}

#[tokio::test]
async fn copied_references_and_new_equal_calls_never_enter() {
    let w = ConsentWorld::new();
    let approval_id = w.request("consented").await;
    w.approve(&approval_id);

    // The reference copied into another session's admitted work.
    let other = ToolWork::for_session(&SessionId::new());
    other.set_write_tier(OperationReviewTier::R3);
    assert_human_consent_required(
        dispatch_retained_consent_attempt(&w.leaf, &w.context_for(&other), &approval_id).await,
    );
    // A new call with equal arguments in the same work is a new candidate.
    let fresh = w.request("equal-arguments").await;
    assert_ne!(fresh, approval_id);
    assert!(w.bodies().is_empty(), "zero entry for both");

    w.attempt(&approval_id)
        .await
        .expect("the exact retained attempt still enters");
    assert_eq!(w.bodies(), vec!["consented".to_string()]);
}

#[tokio::test]
async fn a_failure_before_entry_keeps_the_consent_spendable() {
    let w = ConsentWorld::new();
    let approval_id = w.request("consented").await;
    w.approve(&approval_id);

    w.leaf.fail_before_entry.store(true, Ordering::SeqCst);
    assert!(matches!(
        w.attempt(&approval_id).await,
        Err(ToolError::ExecutionFailed { .. })
    ));
    assert!(w.bodies().is_empty());

    w.attempt(&approval_id)
        .await
        .expect("still spendable after a pre-entry failure");
    assert_eq!(w.bodies(), vec!["consented".to_string()]);
}

#[tokio::test]
async fn consent_never_overrides_a_revoked_permission() {
    let w = ConsentWorld::new();
    let approval_id = w.request("consented").await;
    w.approve(&approval_id);

    w.work.revoke_tool(CONSENTED_TOOL);
    let refused = w.attempt(&approval_id).await;
    assert!(
        matches!(refused, Err(ToolError::AuthorizationRefused { .. })),
        "consent is a conjunction with permission, got {refused:?}"
    );
    assert!(w.bodies().is_empty(), "zero entry");
}

#[tokio::test(start_paused = true)]
async fn an_expired_consent_never_enters() {
    let w = ConsentWorld::new();
    let approval_id = w.request("consented").await;
    w.approve(&approval_id);

    tokio::time::advance(VALIDITY + Duration::from_secs(1)).await;
    assert_human_consent_required(w.attempt(&approval_id).await);
    assert!(w.bodies().is_empty(), "zero entry after expiry");
}

#[tokio::test]
async fn an_eligibility_revocation_before_commit_never_records_or_enters() {
    let w = ConsentWorld::new();
    let approval_id = w.request("consented").await;

    // Published between the resolution and the commit of the decision.
    w.route.revoke_after_resolve.store(true, Ordering::SeqCst);
    assert_eq!(
        w.approvals
            .decide_consent(&approval_id, decision(ApprovalDecision::Approve, "lead-1")),
        Err(ApprovalError::NotEligible {
            approval_id: approval_id.clone()
        })
    );
    assert_eq!(
        w.approvals.get(&approval_id).expect("record").status,
        ApprovalStatus::Pending
    );
    assert_eq!(w.notices(&approval_id), vec![ConsentNoticeKind::Requested]);
    assert_human_consent_required(w.attempt(&approval_id).await);
    assert!(w.bodies().is_empty(), "zero entry");

    // Re-granted and unchanged since resolution: approval, one entry.
    w.route
        .change_eligibility(|eligible| eligible.push("lead-1".to_string()));
    w.approve(&approval_id);
    w.attempt(&approval_id)
        .await
        .expect("the retained candidate enters");
    assert_eq!(w.bodies(), vec!["consented".to_string()]);
}

/// The withdrawal reason of `approval_id`'s consent, if any.
fn consent_withdrawal(
    w: &ConsentWorld,
    approval_id: &ApprovalId,
) -> Option<meerkat_core::approval::ApprovalConsentWithdrawalReason> {
    w.approvals
        .get(approval_id)
        .expect("record")
        .consent
        .and_then(|consent| consent.withdrawal)
        .map(|withdrawal| withdrawal.reason)
}

/// ADR-001 D9 with the native compiler and an owner whose tool allowance
/// slides with every preparation (now plus 60 s, as ordinary host work grants
/// it), on a clock that moves on between the request and each
/// re-adjudication. Unrelated owner changes keep the consent on reads, the
/// decision records and publishes it, and the attempt enters exactly once:
/// each fresh decision's own window differs, and never counts as a change.
#[tokio::test]
async fn a_sliding_owner_window_never_counts_as_a_change() {
    let w = ConsentWorld::new();
    w.work.slide_allowance(Duration::from_secs(60));
    let approval_id = w.request("consented").await;

    // Reads.
    w.work.advance_clock(Duration::from_secs(10));
    w.work.republish();
    assert_eq!(
        w.approvals.get(&approval_id).expect("record").status,
        ApprovalStatus::Pending
    );
    assert!(
        w.approvals
            .list(ApprovalListFilter::default())
            .expect("list")
            .iter()
            .any(|record| record.approval_id == approval_id
                && record.status == ApprovalStatus::Pending)
    );

    // The decision.
    w.work.advance_clock(Duration::from_secs(10));
    w.work.republish();
    let approved = w.approve(&approval_id);
    assert_eq!(approved.status, ApprovalStatus::Approved);
    assert_eq!(consent_withdrawal(&w, &approval_id), None);

    // The spend.
    w.work.advance_clock(Duration::from_secs(10));
    w.work.republish();
    w.attempt(&approval_id)
        .await
        .expect("the re-adjudicated consent enters");
    assert_eq!(w.bodies(), vec!["consented".to_string()], "exactly once");
    assert_eq!(
        w.notices(&approval_id),
        vec![
            ConsentNoticeKind::Requested,
            ConsentNoticeKind::Approved,
            ConsentNoticeKind::Used,
        ]
    );
}

/// ADR-001 D9: a re-adjudication asks the owners afresh and takes their
/// answer; it never synthesizes a window or carries the old one forward. An
/// owner whose deadline for the operation is fixed refuses the fresh
/// preparation once that deadline passed while the consent was pending: a
/// read leaves the consent as it is (reads decide afresh only what an owner
/// publication change made stale), and the approval withdraws it as
/// `AuthorityChanged`, so the attempt enters nothing. Beside it, an owner
/// whose allowance slides keeps the consent past the same moment, and its
/// attempt enters once.
#[tokio::test]
async fn a_fixed_owner_deadline_withdraws_where_a_sliding_window_keeps() {
    for sliding in [false, true] {
        let w = ConsentWorld::new();
        if sliding {
            w.work.slide_allowance(Duration::from_secs(60));
        }
        let approval_id = w.request("consented").await;
        // Past the fixed deadline and past the first sliding window, with
        // no owner change.
        w.work.advance_clock(Duration::from_secs(70));
        let record = w.approvals.get(&approval_id).expect("record");
        if sliding {
            assert_eq!(record.status, ApprovalStatus::Pending, "kept");
            assert_eq!(consent_withdrawal(&w, &approval_id), None);
            w.approve(&approval_id);
            w.attempt(&approval_id).await.expect("enters");
            assert_eq!(w.bodies(), vec!["consented".to_string()], "exactly once");
        } else {
            assert_eq!(
                record.status,
                ApprovalStatus::Pending,
                "a read decides nothing"
            );
            assert_eq!(
                w.approvals
                    .decide_consent(&approval_id, decision(ApprovalDecision::Approve, "lead-1")),
                Err(ApprovalError::ConsentUnavailable {
                    approval_id: approval_id.clone()
                })
            );
            assert_eq!(
                w.approvals.get(&approval_id).expect("record").status,
                ApprovalStatus::Cancelled
            );
            assert_eq!(
                consent_withdrawal(&w, &approval_id),
                Some(meerkat_core::approval::ApprovalConsentWithdrawalReason::AuthorityChanged)
            );
            assert_human_consent_required(w.attempt(&approval_id).await);
            assert!(w.bodies().is_empty(), "zero entry");
        }
    }
}

/// ADR-001 D9: a pending consent outlives its decision's window (a human
/// takes longer than the 60 s an owner grants a decision). Approved after the
/// window ended, its decision is decided afresh and published, and the
/// attempt, after that window ended too, enters exactly once. A relevant
/// change after the window (here the review material) is left by reads to
/// the approval, which finds it and withdraws the consent as
/// `context_changed`, with zero entry.
#[tokio::test]
async fn a_consent_outlives_its_decision_window() {
    let w = ConsentWorld::new();
    w.work.slide_allowance(Duration::from_secs(60));
    let approval_id = w.request("consented").await;
    w.work.advance_clock(Duration::from_secs(90));
    let approved = w.approve(&approval_id);
    assert_eq!(approved.status, ApprovalStatus::Approved);
    assert_eq!(consent_withdrawal(&w, &approval_id), None);
    w.work.advance_clock(Duration::from_secs(90));
    w.attempt(&approval_id).await.expect("enters");
    assert_eq!(w.bodies(), vec!["consented".to_string()], "exactly once");
    assert_eq!(
        w.notices(&approval_id),
        vec![
            ConsentNoticeKind::Requested,
            ConsentNoticeKind::Approved,
            ConsentNoticeKind::Used,
        ]
    );

    // A relevant change found after the window.
    let w = ConsentWorld::new();
    w.work.slide_allowance(Duration::from_secs(60));
    w.work.set_review_material(ReviewMaterial::Held {
        account: "account-a".to_owned(),
        mandate: "mandate-a".to_owned(),
    });
    let approval_id = w.request("consented").await;
    w.work.advance_clock(Duration::from_secs(90));
    w.work.set_review_material(ReviewMaterial::Held {
        account: "account-b".to_owned(),
        mandate: "mandate-a".to_owned(),
    });
    assert_eq!(
        w.approvals.get(&approval_id).expect("record").status,
        ApprovalStatus::Pending,
        "a read leaves a decision its own check refuses as it is"
    );
    assert_eq!(
        w.approvals
            .decide_consent(&approval_id, decision(ApprovalDecision::Approve, "lead-1")),
        Err(ApprovalError::ConsentUnavailable {
            approval_id: approval_id.clone()
        })
    );
    assert_eq!(
        w.approvals.get(&approval_id).expect("record").status,
        ApprovalStatus::Cancelled
    );
    assert_eq!(
        consent_withdrawal(&w, &approval_id),
        Some(meerkat_core::approval::ApprovalConsentWithdrawalReason::ContextChanged)
    );
    assert_human_consent_required(w.attempt(&approval_id).await);
    assert!(w.bodies().is_empty(), "zero entry");
}

/// ADR-001 D9 for the approver's eligibility, with the route's eligibility
/// owner publishing through the work's own publication, so every owner
/// change of the work makes its evidence stale. Revoked after the approval,
/// the attempt enters nothing and the consent is withdrawn as
/// `AuthorityChanged`; so is a self-approval the host's policy permitted
/// once that policy forbids it. An unrelated owner change during the
/// decision's own store write keeps the consent, which enters once.
#[tokio::test]
async fn an_approved_consent_follows_its_approvers_eligibility() {
    // Revoked after the approval.
    let w = ConsentWorld::with_eligibility_on_work_publication(None);
    let approval_id = w.request("consented").await;
    w.approve(&approval_id);
    w.route
        .change_eligibility(|eligible| eligible.retain(|id| id != "lead-1"));
    assert_human_consent_required(w.attempt(&approval_id).await);
    assert!(w.bodies().is_empty(), "zero entry");
    assert_eq!(
        consent_withdrawal(&w, &approval_id),
        Some(meerkat_core::approval::ApprovalConsentWithdrawalReason::AuthorityChanged)
    );

    // A self-approval the host's policy permitted, then forbids.
    let w = ConsentWorld::with_eligibility_on_work_publication(None);
    w.route.permit_self_approval(true);
    let approval_id = w.request("consented").await;
    w.approvals
        .decide_consent(
            &approval_id,
            decision(ApprovalDecision::Approve, "human:child"),
        )
        .expect("self-approved under the permitting policy");
    w.route.permit_self_approval(false);
    assert_human_consent_required(w.attempt(&approval_id).await);
    assert!(w.bodies().is_empty(), "zero entry");
    assert_eq!(
        consent_withdrawal(&w, &approval_id),
        Some(meerkat_core::approval::ApprovalConsentWithdrawalReason::AuthorityChanged)
    );

    // An unrelated owner change during the decision's own store write.
    let store = Arc::new(HookedStore::default());
    let w = ConsentWorld::with_eligibility_on_work_publication(Some(
        Arc::clone(&store) as Arc<dyn meerkat_core::ApprovalStore>
    ));
    let approval_id = w.request("consented").await;
    let publication = w.work.publication.clone();
    *store.hook.lock().expect("hook") = Some(Box::new(move || {
        drop(publication.begin_owner_change().expect("owner publication"));
    }));
    let approved = w.approve(&approval_id);
    assert_eq!(approved.status, ApprovalStatus::Approved);
    assert_eq!(consent_withdrawal(&w, &approval_id), None);
    assert_eq!(
        w.notices(&approval_id),
        vec![ConsentNoticeKind::Requested, ConsentNoticeKind::Approved]
    );
    w.attempt(&approval_id).await.expect("enters");
    assert_eq!(w.bodies(), vec!["consented".to_string()], "exactly once");
}

/// ADR-001 D9: an eligibility owner change still in progress when the spend
/// asks the eligibility owner afresh decides nothing (the owner cannot say
/// now): the attempt enters nothing and the consent is kept, and once the
/// change completed the next attempt enters once.
#[tokio::test]
async fn an_eligibility_change_in_progress_at_the_spend_keeps_the_consent() {
    let w = ConsentWorld::new();
    let approval_id = w.request("consented").await;
    w.approve(&approval_id);
    // An unrelated change made the evidence stale; another is in progress.
    w.route
        .change_eligibility(|eligible| eligible.push("lead-2".to_string()));
    let publication = w.route.publication.clone();
    let (began, in_progress) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel::<()>();
    let holder = std::thread::spawn(move || {
        let change = publication.begin_owner_change().expect("owner publication");
        began.send(()).expect("change began");
        let _ = released.recv_timeout(Duration::from_secs(30));
        drop(change);
    });
    in_progress
        .recv_timeout(Duration::from_secs(5))
        .expect("the change is in progress");
    let refused = w.attempt(&approval_id).await;
    let _ = release.send(());
    holder.join().expect("holder thread");
    assert_eq!(
        refused.err(),
        Some(ToolError::ReviewUnavailable {
            kind: meerkat_core::ReviewUnavailableKind::OwnerUnavailable,
        })
    );
    assert!(w.bodies().is_empty(), "nothing entered");
    assert_eq!(
        consent_withdrawal(&w, &approval_id),
        None,
        "the consent is kept"
    );
    w.attempt(&approval_id)
        .await
        .expect("enters once the change completed");
    assert_eq!(w.bodies(), vec!["consented".to_string()], "exactly once");
}
