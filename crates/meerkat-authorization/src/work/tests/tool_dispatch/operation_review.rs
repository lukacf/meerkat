//! Retained R2/R3 review: admission at the common prepared tool entry, a
//! non-consuming check point, and exactly one spend at the effect leaf
//! (ADR-001 model review). These pin the settlement contract, not reviewer
//! judgment quality:
//! - every reviewed call starts natively ALLOWED, so review is really reached;
//! - the required tier is the policy owner's truth inside the exact prepared
//!   authorization, enforced at the public fenced entry with no wrapper;
//! - the allow is spent once, by the leaf, immediately before its body
//!   counter; a failure before that point leaves it unspent;
//! - a review whose authorization was re-prepared (owner publication change)
//!   or refused while retained, or during entry staging, settles only its own
//!   operation with typed local feedback and zero physical entry; a permitted
//!   sibling still enters;
//! - a model review superseded by one completed owner change before any
//!   dispatcher code ran, with its tier and review material unchanged, is
//!   reviewed once more from the start (never by reusing its verdict); a
//!   second change, a negative verdict, a changed tier or material, expiry
//!   or too little tool time settles it as before;
//! - an absolute deadline is checked at the verdict and at the spend;
//! - the `Used` projection is delivered only after the effect;
//! - a dispatcher that cannot carry review refuses required review locally;
//! - a dropped dispatch (caller interrupt or enclosing tool timeout) is
//!   `Abandoned` for the review owner, while the turn owner's `Cancelled` and
//!   the tool owner's `Timeout` keep their own typed outcomes.
//!
//! Reviewer disposal is proven by an explicit drop witness on the reviewer
//! future, never by yielding.

use std::collections::VecDeque;
use std::sync::atomic::AtomicBool;

use meerkat_authorization_contracts::audit::{
    AuditObservation, AuthorizationAuditObservation, AuthorizationAuditSink,
};
use meerkat_core::approval::review::{
    BoundOperationReview, OperationReviewObserver, OperationReviewTier, OperationReviewer,
    ReviewAttemptRef, ReviewAttemptStatus, ReviewCandidate, ReviewEntrySupport, ReviewObservation,
    ReviewRetirementReason, ReviewVerdict, ReviewerFailure,
};
use meerkat_core::authorization::{
    OperationObservation, OperationObservationError, PolicyPublicationObservation,
    PreparedAuthorizationBinding, PreparedOperationAuthorization, WorkAuthorization,
    WorkAuthorizationContext,
};
use meerkat_core::event::AgentErrorClass;
use meerkat_core::{ApprovalService, ReviewUnavailableKind, ReviewUnsatisfiedKind};

use super::*;

const REVIEWED_TOOL: &str = "write_record";
const SIBLING_TOOL: &str = "read_record";
const BOUND: Duration = Duration::from_secs(5);
/// Owner review deadline. Deadline tests run under paused virtual time.
const REVIEW_DEADLINE: Duration = Duration::from_secs(30);

type Observed = (ReviewAttemptStatus, Option<ReviewRetirementReason>);

fn pending() -> Observed {
    (ReviewAttemptStatus::Pending, None)
}

fn allowed() -> Observed {
    (ReviewAttemptStatus::Allowed, None)
}

fn used() -> Observed {
    (ReviewAttemptStatus::Used, None)
}

fn retired(reason: ReviewRetirementReason) -> Observed {
    (ReviewAttemptStatus::Retired, Some(reason))
}

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// The reviewer future's fate: `dropped` is set only when the future is
/// dropped before it returned a verdict.
#[derive(Default)]
struct ReviewerFate {
    dropped: AtomicBool,
    completed: AtomicBool,
}

struct DropWitness {
    fate: Arc<ReviewerFate>,
    completed: bool,
}

impl Drop for DropWitness {
    fn drop(&mut self) {
        if self.completed {
            self.fate.completed.store(true, Ordering::SeqCst);
        } else {
            self.fate.dropped.store(true, Ordering::SeqCst);
        }
    }
}

type OwnerEdit = Box<dyn FnOnce() + Send>;

/// One scripted review answer: an owner edit run just before it returns,
/// and the result it returns.
struct ReviewStep {
    edit: Option<OwnerEdit>,
    result: Result<ReviewVerdict, ReviewerFailure>,
}

impl ReviewStep {
    fn answer(result: Result<ReviewVerdict, ReviewerFailure>) -> Self {
        Self { edit: None, result }
    }

    fn after(edit: OwnerEdit, result: Result<ReviewVerdict, ReviewerFailure>) -> Self {
        Self {
            edit: Some(edit),
            result,
        }
    }
}

/// Controlled reviewer: announces entry, waits for release (or a virtual-time
/// delay), then returns its verdict. It can run an owner edit just before
/// returning, or never return.
struct PausedReviewer {
    entered: Notify,
    release: Notify,
    calls: AtomicUsize,
    verdict: ReviewVerdict,
    fate: Arc<ReviewerFate>,
    on_return: Mutex<Option<OwnerEdit>>,
    /// The owner material each review actually received.
    seen_contexts: Mutex<Vec<Option<(String, String)>>>,
    never_returns: bool,
    /// Return after exactly this virtual delay instead of waiting for release.
    returns_after: Option<Duration>,
    /// Per-call answers, consumed in call order; once empty, a call returns
    /// `verdict` after the one-shot `on_return`.
    script: Mutex<VecDeque<ReviewStep>>,
}

impl PausedReviewer {
    fn new(verdict: ReviewVerdict) -> Self {
        Self {
            entered: Notify::new(),
            release: Notify::new(),
            calls: AtomicUsize::new(0),
            verdict,
            fate: Arc::new(ReviewerFate::default()),
            on_return: Mutex::new(None),
            seen_contexts: Mutex::new(Vec::new()),
            never_returns: false,
            returns_after: None,
            script: Mutex::new(VecDeque::new()),
        }
    }

    /// A reviewer answering call by call from `steps`.
    fn scripted(steps: impl IntoIterator<Item = ReviewStep>) -> Self {
        let reviewer = Self::new(ReviewVerdict::Allow);
        *reviewer.script.lock().expect("review script") = steps.into_iter().collect();
        reviewer
    }

    async fn wait_entered(&self) {
        tokio::time::timeout(Duration::from_secs(2), self.entered.notified())
            .await
            .expect("review really suspended dispatch");
    }

    fn assert_future_dropped_unfinished(&self) {
        assert!(
            self.fate.dropped.load(Ordering::SeqCst),
            "the reviewer future was disposed"
        );
        assert!(
            !self.fate.completed.load(Ordering::SeqCst),
            "the reviewer never delivered a verdict"
        );
    }
}

#[async_trait]
impl OperationReviewer for PausedReviewer {
    async fn review(
        &self,
        candidate: &ReviewCandidate<'_>,
    ) -> Result<ReviewVerdict, ReviewerFailure> {
        let mut witness = DropWitness {
            fate: Arc::clone(&self.fate),
            completed: false,
        };
        assert_eq!(
            candidate.tool_name(),
            REVIEWED_TOOL,
            "R1 must not be reviewed"
        );
        assert!(matches!(
            candidate.facts().operation,
            AuthorizationOperation::Tool(_)
        ));
        self.seen_contexts
            .lock()
            .expect("seen contexts")
            .push(candidate.operation_context().map(|context| {
                (
                    context.action_account().unwrap_or_default().to_owned(),
                    context.mandate().unwrap_or_default().to_owned(),
                )
            }));
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.entered.notify_one();
        if self.never_returns {
            std::future::pending::<()>().await;
        }
        match self.returns_after {
            Some(delay) => tokio::time::sleep(delay).await,
            None => self.release.notified().await,
        }
        let step = self.script.lock().expect("review script").pop_front();
        let (edit, result) = match step {
            Some(ReviewStep { edit, result }) => (edit, result),
            None => (
                self.on_return.lock().expect("on return").take(),
                Ok(self.verdict),
            ),
        };
        if let Some(edit) = edit {
            edit();
        }
        witness.completed = true;
        result
    }
}

/// Projection of generated review transitions. It can arm the entry hook
/// once `Allowed` is recorded, and record how many bodies had entered when
/// `Used` was delivered.
#[derive(Default)]
struct RecordingObserver {
    observations: Mutex<Vec<ReviewObservation>>,
    arm_on_allowed: Mutex<Option<Arc<AtomicBool>>>,
    /// Armed by the first `Allowed` only.
    arm_on_first_allowed: Mutex<Option<Arc<AtomicBool>>>,
    body: Mutex<Option<Arc<RecordingDispatcher>>>,
    bodies_at_used: Mutex<Option<usize>>,
    edit_on_used: Mutex<Option<OwnerEdit>>,
}

impl OperationReviewObserver for RecordingObserver {
    fn observe(&self, observation: ReviewObservation) {
        if observation.status == ReviewAttemptStatus::Allowed
            && let Some(arm) = self.arm_on_allowed.lock().expect("arm").as_ref()
        {
            arm.store(true, Ordering::SeqCst);
        }
        if observation.status == ReviewAttemptStatus::Allowed
            && let Some(arm) = self.arm_on_first_allowed.lock().expect("arm once").take()
        {
            arm.store(true, Ordering::SeqCst);
        }
        if observation.status == ReviewAttemptStatus::Used {
            if let Some(body) = self.body.lock().expect("body").as_ref() {
                *self.bodies_at_used.lock().expect("bodies at used") = Some(body_ids(body).len());
            }
            let edit = self.edit_on_used.lock().expect("edit on used").take();
            if let Some(edit) = edit {
                edit();
            }
        }
        self.observations
            .lock()
            .expect("review observations")
            .push(observation);
    }
}

impl RecordingObserver {
    fn statuses(&self) -> Vec<Observed> {
        self.observations
            .lock()
            .expect("review observations")
            .iter()
            .map(|observation| (observation.status, observation.retirement))
            .collect()
    }

    /// Exactly one owner-issued attempt for one candidate.
    fn single_attempt(&self) -> ReviewAttemptRef {
        let observations = self.observations.lock().expect("review observations");
        let first = observations.first().expect("one review attempt began");
        assert!(
            observations
                .iter()
                .all(|observation| observation.attempt == first.attempt),
            "exactly one retained attempt for one candidate"
        );
        first.attempt.clone()
    }

    /// The distinct owner-issued attempts in the order they began, every one
    /// for the tool call `tool_call_id`.
    fn attempts(&self, tool_call_id: &str) -> Vec<ReviewAttemptRef> {
        let observations = self.observations.lock().expect("review observations");
        assert!(
            observations
                .iter()
                .all(|observation| observation.tool_call_id.as_ref() == tool_call_id),
            "every attempt reviews the same tool call"
        );
        let mut attempts: Vec<ReviewAttemptRef> = Vec::new();
        for observation in observations.iter() {
            if !attempts.contains(&observation.attempt) {
                attempts.push(observation.attempt.clone());
            }
        }
        attempts
    }
}

struct ReviewWorld {
    reviewer: Arc<PausedReviewer>,
    observer: Arc<RecordingObserver>,
    review: Arc<BoundOperationReview>,
}

impl ReviewWorld {
    fn new() -> Self {
        Self::with_reviewer(PausedReviewer::new(ReviewVerdict::Allow))
    }

    fn with_reviewer(reviewer: PausedReviewer) -> Self {
        let reviewer = Arc::new(reviewer);
        let observer = Arc::new(RecordingObserver::default());
        let review = Arc::new(
            BoundOperationReview::new(reviewer.clone(), ApprovalService::new(), REVIEW_DEADLINE)
                .with_observer(observer.clone()),
        );
        Self {
            reviewer,
            observer,
            review,
        }
    }

    /// Governed dispatch context carrying this review composition.
    fn context(&self, work: &ToolWork) -> ToolDispatchContext {
        governed(work).with_operation_review(Some(self.review.clone()))
    }
}

/// Governed context with NO review composition.
fn governed(work: &ToolWork) -> ToolDispatchContext {
    ToolDispatchContext::default().with_work_authorization(Some(work.context.clone()))
}

/// Native work where `write private` is ALLOWED with the owner tier `tier`.
fn reviewed_work(tier: OperationReviewTier) -> ToolWork {
    let work = ToolWork::for_session(&SessionId::new());
    work.set_write_tier(tier);
    work
}

/// How the test leaf behaves around its single entry step.
#[derive(Clone, Copy, Default)]
struct LeafBehavior {
    /// Fail during preparation, before the entry step.
    fail_before_entry: bool,
    /// Wait (virtual time) during preparation, before the entry step.
    wait_before_entry: Option<Duration>,
    /// Call the entry step a second time after the body (a defective leaf).
    enter_twice: bool,
}

/// The test effect leaf: declares `ConsumesAtEntry` and spends exactly once,
/// immediately before the shared body counter.
struct ReviewedLeaf {
    body: Arc<RecordingDispatcher>,
    behavior: LeafBehavior,
    /// Runs once immediately before the entry step: an owner edit landing
    /// after every pre-entry point the fenced dispatch owns.
    before_entry: Mutex<Option<OwnerEdit>>,
}

impl ReviewedLeaf {
    fn new(body: Arc<RecordingDispatcher>) -> Arc<Self> {
        Self::with(body, LeafBehavior::default())
    }

    fn with(body: Arc<RecordingDispatcher>, behavior: LeafBehavior) -> Arc<Self> {
        Arc::new(Self {
            body,
            behavior,
            before_entry: Mutex::new(None),
        })
    }

    fn editing_before_entry(body: Arc<RecordingDispatcher>, edit: OwnerEdit) -> Arc<Self> {
        Arc::new(Self {
            body,
            behavior: LeafBehavior::default(),
            before_entry: Mutex::new(Some(edit)),
        })
    }
}

#[async_trait]
impl AgentToolDispatcher for ReviewedLeaf {
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
        // Argument and preparation work, and any waits, precede entry.
        if let Some(wait) = self.behavior.wait_before_entry {
            tokio::time::sleep(wait).await;
        }
        if self.behavior.fail_before_entry {
            return Err(ToolError::execution_failed("leaf preparation failed"));
        }
        let edit = self.before_entry.lock().expect("before entry").take();
        if let Some(edit) = edit {
            edit();
        }
        let entered = context.enter_reviewed_effect(call, None)?;
        let outcome = self
            .body
            .dispatch_with_context(call, entered.as_ref().unwrap_or(context))
            .await?;
        if self.behavior.enter_twice {
            context.enter_reviewed_effect(call, None)?;
        }
        Ok(outcome)
    }
}

/// Real plan resolution and the PUBLIC fenced entry over any dispatcher.
async fn fenced<T: AgentToolDispatcher + ?Sized + 'static>(
    dispatcher: &Arc<T>,
    call: &Call,
    context: &ToolDispatchContext,
) -> Result<ToolDispatchOutcome, ToolError> {
    fenced_within(dispatcher, call, context, Duration::from_mins(10)).await
}

/// [`fenced`] under the direct caller deadline `caller_deadline`.
async fn fenced_within<T: AgentToolDispatcher + ?Sized + 'static>(
    dispatcher: &Arc<T>,
    call: &Call,
    context: &ToolDispatchContext,
    caller_deadline: Duration,
) -> Result<ToolDispatchOutcome, ToolError> {
    let plan = resolve_tool_execution_plan_fenced(
        dispatcher,
        call.view(),
        context,
        &ToolExecutionResolutionContext::new(
            meerkat_core::ToolDeadlineChain::new(vec![
                meerkat_core::ToolDeadlineContributor::finite(
                    meerkat_core::ToolDeadlineOwner::DirectCaller,
                    caller_deadline,
                ),
            ])
            .expect("finite caller deadline"),
        ),
    )
    .expect("real plan resolution");
    dispatch_tool_execution_plan_fenced(dispatcher, call.view(), context, &plan).await
}

/// Wraps the real work authorization; on the first Entry observation after
/// the review owner recorded `Allowed`, it publishes an owner change that
/// keeps permission, modelling a context edit landing during entry staging.
struct EntryHookWork {
    inner: Arc<dyn WorkAuthorization>,
    publication: LocalAuthorizationPublication,
    armed: Arc<AtomicBool>,
    edit: bool,
}

struct EntryHookDecision {
    inner: Arc<dyn PreparedOperationAuthorization>,
    publication: LocalAuthorizationPublication,
    armed: Arc<AtomicBool>,
    edit: bool,
}

impl WorkAuthorization for EntryHookWork {
    fn review_operation_context(
        &self,
        binding: &meerkat_core::authorization::PreparedAuthorizationBinding,
    ) -> Result<
        meerkat_core::authorization::ReviewOperationContextResolution,
        meerkat_core::OperationAuthorizationError,
    > {
        self.inner.review_operation_context(binding)
    }

    fn controller_model_selection(&self) -> Option<meerkat_core::ControllerModelSelection> {
        self.inner.controller_model_selection()
    }

    fn prepare(
        &self,
        binding: &PreparedAuthorizationBinding,
    ) -> Result<Arc<dyn PreparedOperationAuthorization>, meerkat_core::OperationAuthorizationError>
    {
        Ok(Arc::new(EntryHookDecision {
            inner: self.inner.prepare(binding)?,
            publication: self.publication.clone(),
            armed: Arc::clone(&self.armed),
            edit: self.edit,
        }))
    }
}

impl PreparedOperationAuthorization for EntryHookDecision {
    fn policy_observation(&self) -> Option<PolicyPublicationObservation> {
        self.inner.policy_observation()
    }

    fn review_tier(&self) -> OperationReviewTier {
        self.inner.review_tier()
    }

    fn check_current(
        &self,
        binding: &PreparedAuthorizationBinding,
    ) -> Result<(), meerkat_core::OperationAuthorizationError> {
        self.inner.check_current(binding)
    }

    fn observe(
        &self,
        binding: &PreparedAuthorizationBinding,
        observation: OperationObservation,
    ) -> Result<(), OperationObservationError> {
        if matches!(observation, OperationObservation::Entry(_))
            && self.edit
            && self.armed.swap(false, Ordering::SeqCst)
        {
            drop(
                self.publication
                    .begin_owner_change()
                    .expect("owner publication"),
            );
        }
        self.inner.observe(binding, observation)
    }
}

/// Entry-hooked context; the edit is armed only once `Allowed` is recorded.
fn entry_hook_context(world: &ReviewWorld, work: &ToolWork, edit: bool) -> ToolDispatchContext {
    let armed = Arc::new(AtomicBool::new(false));
    *world.observer.arm_on_allowed.lock().expect("arm") = Some(Arc::clone(&armed));
    entry_hooked(world, work, armed, edit)
}

/// [`entry_hook_context`] armed by the first `Allowed` only: one owner edit
/// lands during the first allowed attempt's entry staging.
fn entry_hook_context_once(world: &ReviewWorld, work: &ToolWork) -> ToolDispatchContext {
    let armed = Arc::new(AtomicBool::new(false));
    *world
        .observer
        .arm_on_first_allowed
        .lock()
        .expect("arm once") = Some(Arc::clone(&armed));
    entry_hooked(world, work, armed, true)
}

fn entry_hooked(
    world: &ReviewWorld,
    work: &ToolWork,
    armed: Arc<AtomicBool>,
    edit: bool,
) -> ToolDispatchContext {
    let hooked = WorkAuthorizationContext::new(
        Arc::new(EntryHookWork {
            inner: work.context.authorization().clone(),
            publication: work.publication.clone(),
            armed,
            edit,
        }),
        work.context.execution_scope().clone(),
    );
    ToolDispatchContext::default()
        .with_work_authorization(Some(hooked))
        .with_operation_review(Some(world.review.clone()))
}

/// An owner edit that publishes a change keeping every permission and tier.
fn republishing(work: &Arc<ToolWork>) -> OwnerEdit {
    let edited = Arc::clone(work);
    Box::new(move || edited.republish())
}

/// An owner edit that begins one change on `work`'s own publication on
/// another thread and returns once it is in progress; the change completes
/// after `hold` of real time.
fn hold_owner_change_for(work: &ToolWork, hold: Duration) -> OwnerEdit {
    let publication = work.publication.clone();
    Box::new(move || {
        let (in_progress, began) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let change = publication.begin_owner_change().expect("owner publication");
            in_progress.send(()).expect("change began");
            std::thread::sleep(hold);
            drop(change);
        });
        began
            .recv_timeout(BOUND)
            .expect("the change is in progress before the verdict returns");
    })
}

/// As [`hold_owner_change_for`], completing only once `release` is signalled
/// or dropped (at most `BOUND` of real time).
fn hold_owner_change_until(work: &ToolWork, release: std::sync::mpsc::Receiver<()>) -> OwnerEdit {
    let publication = work.publication.clone();
    Box::new(move || {
        let (in_progress, began) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let change = publication.begin_owner_change().expect("owner publication");
            in_progress.send(()).expect("change began");
            let _ = release.recv_timeout(BOUND);
            drop(change);
        });
        began
            .recv_timeout(BOUND)
            .expect("the change is in progress before the verdict returns");
    })
}

fn reviewed_call(id: &str) -> Call {
    // `write private` is natively permitted by the fixture authority.
    Call::new(id, REVIEWED_TOOL, "private")
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

async fn wait_for_body(body: &RecordingDispatcher, id: &str) {
    // Arm the entry signal before each check so an entry between the check
    // and the await cannot be missed. The timeout is only a hang guard.
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let entered = body.body_entered.notified();
            tokio::pin!(entered);
            entered.as_mut().enable();
            if body_ids(body).iter().any(|recorded| recorded == id) {
                return;
            }
            entered.await;
        }
    })
    .await
    .expect("permitted sibling entered while the review was retained");
}

fn assert_unsatisfied(
    result: Result<ToolDispatchOutcome, ToolError>,
    expected: ReviewUnsatisfiedKind,
) {
    assert!(
        matches!(&result, Err(ToolError::ReviewUnsatisfied { kind, .. }) if *kind == expected),
        "expected local review_unsatisfied {expected:?}, got {result:?}"
    );
}

fn assert_unavailable(
    result: Result<ToolDispatchOutcome, ToolError>,
    expected: ReviewUnavailableKind,
) {
    assert!(
        matches!(&result, Err(ToolError::ReviewUnavailable { kind }) if *kind == expected),
        "expected local review_unavailable {expected:?}, got {result:?}"
    );
}

// ---------------------------------------------------------------------------
// Common prepared entry and unsupported dispatchers.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn bare_fenced_dispatch_without_reviewer_refuses_required_tiers_and_runs_r1() {
    for (tier, expect_unsatisfied) in [
        (OperationReviewTier::R2, false),
        (OperationReviewTier::R3, true),
    ] {
        let work = reviewed_work(tier);
        let body = Arc::new(RecordingDispatcher::default());
        let context = governed(&work);
        let result = fenced(&body, &reviewed_call("reviewed"), &context).await;
        if expect_unsatisfied {
            assert_unsatisfied(result, ReviewUnsatisfiedKind::HumanConsentRequired);
        } else {
            assert_unavailable(result, ReviewUnavailableKind::ReviewerMissing);
        }
        assert!(
            body_ids(&body).is_empty(),
            "zero physical entry at {tier:?}"
        );
        fenced(&body, &sibling_call("sibling"), &context)
            .await
            .expect("explicit R1 sibling succeeds");
        assert_eq!(body_ids(&body), vec!["sibling".to_string()]);
    }
}

#[tokio::test]
async fn context_dropping_dispatcher_cannot_honor_required_review() {
    let work = reviewed_work(OperationReviewTier::R2);
    let world = ReviewWorld::new();
    // The shared RecordingDispatcher never declares review support.
    let body = Arc::new(RecordingDispatcher::default());
    let context = world.context(&work);

    let result = fenced(&body, &reviewed_call("reviewed"), &context).await;

    assert_unavailable(result, ReviewUnavailableKind::UnsupportedEntry);
    assert!(body_ids(&body).is_empty(), "never executes unchecked");
    assert_eq!(world.reviewer.calls.load(Ordering::SeqCst), 0);
    assert!(world.observer.statuses().is_empty(), "no attempt began");
    fenced(&body, &sibling_call("sibling"), &context)
        .await
        .expect("R1 sibling is unaffected by the missing support");
}

// ---------------------------------------------------------------------------
// Retained allow, check point and the single leaf spend.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn unchanged_review_is_spent_once_by_the_leaf_before_its_body() {
    let work = reviewed_work(OperationReviewTier::R2);
    let world = ReviewWorld::new();
    let body = Arc::new(RecordingDispatcher::default());
    *world.observer.body.lock().expect("body") = Some(Arc::clone(&body));
    let leaf = ReviewedLeaf::new(Arc::clone(&body));
    let context = world.context(&work);
    let reviewed = reviewed_call("reviewed");

    let controller = async {
        world.reviewer.wait_entered().await;
        assert!(body_ids(&body).is_empty(), "no entry before the verdict");
        world.reviewer.release.notify_one();
    };
    let (result, ()) = tokio::join!(fenced(&leaf, &reviewed, &context), controller);

    assert!(!result.expect("reviewed operation entered").result.is_error);
    assert_eq!(body_ids(&body), vec!["reviewed".to_string()]);
    assert_eq!(
        world.observer.statuses(),
        vec![pending(), allowed(), used()]
    );
    assert_eq!(
        *world
            .observer
            .bodies_at_used
            .lock()
            .expect("bodies at used"),
        Some(1),
        "the Used projection is delivered after the effect"
    );
}

#[tokio::test]
async fn gated_dispatcher_checks_without_spending_and_the_leaf_spends_once() {
    let work = reviewed_work(OperationReviewTier::R2);
    let world = ReviewWorld::new();
    let body = Arc::new(RecordingDispatcher::default());
    let gate = Arc::new(ExecutionPolicyGatedDispatcher::new(
        ReviewedLeaf::new(Arc::clone(&body)),
        ToolExecutionPolicy::unrestricted(),
    ));
    let context = world.context(&work);
    let reviewed = reviewed_call("reviewed");

    let controller = async {
        world.reviewer.wait_entered().await;
        world.reviewer.release.notify_one();
    };
    let (result, ()) = tokio::join!(fenced(&gate, &reviewed, &context), controller);

    assert!(!result.expect("reviewed operation entered").result.is_error);
    assert_eq!(body_ids(&body), vec!["reviewed".to_string()]);
    assert_eq!(
        world.observer.statuses(),
        vec![pending(), allowed(), used()]
    );
}

#[tokio::test]
async fn failure_before_leaf_entry_leaves_the_allow_unspent() {
    let work = reviewed_work(OperationReviewTier::R2);
    let world = ReviewWorld::new();
    let body = Arc::new(RecordingDispatcher::default());
    let leaf = ReviewedLeaf::with(
        Arc::clone(&body),
        LeafBehavior {
            fail_before_entry: true,
            ..LeafBehavior::default()
        },
    );
    let context = world.context(&work);
    let reviewed = reviewed_call("reviewed");

    let controller = async {
        world.reviewer.wait_entered().await;
        world.reviewer.release.notify_one();
    };
    let (result, ()) = tokio::join!(fenced(&leaf, &reviewed, &context), controller);

    assert!(matches!(result, Err(ToolError::ExecutionFailed { .. })));
    assert!(body_ids(&body).is_empty());
    let statuses = world.observer.statuses();
    assert_eq!(statuses, vec![pending(), allowed()]);
    assert!(!statuses.contains(&used()), "no spend before entry");
}

#[tokio::test]
async fn a_second_leaf_entry_refuses_and_the_body_runs_once() {
    let work = reviewed_work(OperationReviewTier::R2);
    let world = ReviewWorld::new();
    let body = Arc::new(RecordingDispatcher::default());
    let leaf = ReviewedLeaf::with(
        Arc::clone(&body),
        LeafBehavior {
            enter_twice: true,
            ..LeafBehavior::default()
        },
    );
    let context = world.context(&work);
    let reviewed = reviewed_call("reviewed");

    let controller = async {
        world.reviewer.wait_entered().await;
        world.reviewer.release.notify_one();
    };
    let (result, ()) = tokio::join!(fenced(&leaf, &reviewed, &context), controller);

    assert_unsatisfied(result, ReviewUnsatisfiedKind::AlreadyConsumed);
    assert_eq!(body_ids(&body), vec!["reviewed".to_string()], "spent once");
    assert_eq!(
        world.observer.statuses(),
        vec![pending(), allowed(), used()]
    );
}

#[tokio::test]
async fn used_observer_cannot_retire_an_entered_effect() {
    let work = Arc::new(reviewed_work(OperationReviewTier::R2));
    let world = ReviewWorld::new();
    let body = Arc::new(RecordingDispatcher::default());
    *world.observer.body.lock().expect("body") = Some(Arc::clone(&body));
    let edited = Arc::clone(&work);
    *world.observer.edit_on_used.lock().expect("edit on used") =
        Some(Box::new(move || edited.republish()));
    let leaf = ReviewedLeaf::new(Arc::clone(&body));
    let context = world.context(&work);
    let reviewed = reviewed_call("reviewed");

    let controller = async {
        world.reviewer.wait_entered().await;
        world.reviewer.release.notify_one();
    };
    let (result, ()) = tokio::join!(fenced(&leaf, &reviewed, &context), controller);

    // The callback ran only after the effect, so it cannot relabel it.
    assert!(
        !result
            .expect("entered effect keeps its result")
            .result
            .is_error
    );
    assert_eq!(body_ids(&body), vec!["reviewed".to_string()]);
    assert_eq!(
        *world
            .observer
            .bodies_at_used
            .lock()
            .expect("bodies at used"),
        Some(1)
    );
    assert_eq!(
        world.observer.statuses(),
        vec![pending(), allowed(), used()]
    );
}

// ---------------------------------------------------------------------------
// Staleness, authority, entry staging and deadlines.
// ---------------------------------------------------------------------------

mod approval_owner_lock {
    use super::*;
    use meerkat_core::approval::{
        ApprovalActionKind, ApprovalDecision, ApprovalOwnerRef, ApprovalPrincipalId,
        ApprovalProposedAction, ApprovalRecord, ApprovalRequest, ApprovalResourceId,
        ApprovalResourceKind, ApprovalResourceRef, ApprovalRisk, ApprovalStatus, ApprovalStore,
        ApprovalStoreError, InMemoryApprovalStore,
    };
    use std::sync::mpsc as sync_channel;

    struct BlockingDecisionStore {
        inner: InMemoryApprovalStore,
        entered: sync_channel::Sender<()>,
        release: Mutex<sync_channel::Receiver<()>>,
    }

    impl ApprovalStore for BlockingDecisionStore {
        fn load_all(&self) -> Result<Vec<ApprovalRecord>, ApprovalStoreError> {
            self.inner.load_all()
        }

        fn get(
            &self,
            approval_id: &meerkat_core::ApprovalId,
        ) -> Result<Option<ApprovalRecord>, ApprovalStoreError> {
            self.inner.get(approval_id)
        }

        fn put_if_current(
            &self,
            expected: Option<&ApprovalRecord>,
            record: &ApprovalRecord,
        ) -> Result<(), ApprovalStoreError> {
            if record.status == ApprovalStatus::Approved {
                // ApprovalService::decide holds its actual state write lock
                // throughout this store callback. Review consumption shares it.
                self.entered.send(()).map_err(|error| {
                    ApprovalStoreError::NotCommitted(format!("store entry signal: {error}"))
                })?;
                self.release
                    .lock()
                    .expect("store release")
                    .recv_timeout(BOUND)
                    .map_err(|error| {
                        ApprovalStoreError::NotCommitted(format!("store release: {error}"))
                    })?;
            }
            self.inner.put_if_current(expected, record)
        }

        fn is_persistent(&self) -> bool {
            self.inner.is_persistent()
        }
    }

    enum EntryProgress {
        FinalCheckPassed,
        DispatchFinished,
    }

    struct FinalCheck {
        armed: AtomicBool,
        checks: AtomicUsize,
        checked: sync_channel::Sender<EntryProgress>,
    }

    struct ObservedWork {
        inner: Arc<dyn WorkAuthorization>,
        final_check: Arc<FinalCheck>,
    }

    struct ObservedDecision {
        inner: Arc<dyn PreparedOperationAuthorization>,
        final_check: Arc<FinalCheck>,
    }

    impl WorkAuthorization for ObservedWork {
        fn review_operation_context(
            &self,
            binding: &meerkat_core::authorization::PreparedAuthorizationBinding,
        ) -> Result<
            meerkat_core::authorization::ReviewOperationContextResolution,
            meerkat_core::OperationAuthorizationError,
        > {
            self.inner.review_operation_context(binding)
        }

        fn controller_model_selection(&self) -> Option<meerkat_core::ControllerModelSelection> {
            self.inner.controller_model_selection()
        }

        fn prepare(
            &self,
            binding: &PreparedAuthorizationBinding,
        ) -> Result<
            Arc<dyn PreparedOperationAuthorization>,
            meerkat_core::OperationAuthorizationError,
        > {
            Ok(Arc::new(ObservedDecision {
                inner: self.inner.prepare(binding)?,
                final_check: self.final_check.clone(),
            }))
        }
    }

    impl PreparedOperationAuthorization for ObservedDecision {
        fn review_tier(&self) -> OperationReviewTier {
            self.inner.review_tier()
        }

        fn policy_observation(&self) -> Option<PolicyPublicationObservation> {
            self.inner.policy_observation()
        }

        fn check_current(
            &self,
            binding: &PreparedAuthorizationBinding,
        ) -> Result<(), meerkat_core::OperationAuthorizationError> {
            self.inner.check_current(binding)?;
            if self.final_check.armed.load(Ordering::SeqCst)
                && self.final_check.checks.fetch_add(1, Ordering::SeqCst) == 1
            {
                // The leaf's real current -> Entry -> current sequence has
                // passed. Do not invent a permission or edit the returned result.
                self.final_check
                    .checked
                    .send(EntryProgress::FinalCheckPassed)
                    .map_err(|_| meerkat_core::OperationAuthorizationError::Unavailable)?;
            }
            Ok(())
        }

        fn observe(
            &self,
            binding: &PreparedAuthorizationBinding,
            observation: OperationObservation,
        ) -> Result<(), OperationObservationError> {
            self.inner.observe(binding, observation)
        }
    }

    struct ContendedLeaf {
        inner: Arc<ReviewedLeaf>,
        start_write: sync_channel::Sender<()>,
        store_entered: Mutex<sync_channel::Receiver<()>>,
        final_check: Arc<FinalCheck>,
    }

    #[async_trait]
    impl AgentToolDispatcher for ContendedLeaf {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            self.inner.tools()
        }

        fn review_entry_support(&self, name: &str) -> ReviewEntrySupport {
            self.inner.review_entry_support(name)
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
            self.start_write
                .send(())
                .map_err(|error| ToolError::execution_failed(format!("start writer: {error}")))?;
            self.store_entered
                .lock()
                .expect("store entry")
                .recv_timeout(BOUND)
                .map_err(|error| ToolError::execution_failed(format!("wait writer: {error}")))?;
            self.final_check.armed.store(true, Ordering::SeqCst);
            self.inner.dispatch_with_context(call, context).await
        }
    }

    fn unrelated_approval() -> ApprovalRequest {
        ApprovalRequest {
            requester: ApprovalPrincipalId::new("fixture-human").expect("principal"),
            owner: ApprovalOwnerRef::Runtime,
            resource: ApprovalResourceRef {
                kind: ApprovalResourceKind::Other,
                id: ApprovalResourceId::new("unrelated-store-write"),
            },
            proposed_action: ApprovalProposedAction {
                kind: ApprovalActionKind::Other,
                summary: "unrelated approval store write".into(),
                body: None,
            },
            risk: ApprovalRisk::Low,
            request_body: None,
            allowed_decisions: std::collections::BTreeSet::from([ApprovalDecision::Approve]),
            expires_at: None,
            metadata: meerkat_core::SurfaceMetadata::default(),
            request_provenance: None,
        }
    }

    #[tokio::test]
    async fn approval_store_contention_cannot_spend_authority_revoked_after_the_final_check() {
        for revoke in [false, true] {
            let work = Arc::new(reviewed_work(OperationReviewTier::R2));
            let (start_write, start_receiver) = sync_channel::channel();
            let (store_entered, entered_receiver) = sync_channel::channel();
            let (release_write, release_receiver) = sync_channel::channel();
            let (checked, checked_receiver) = sync_channel::channel();
            let store = Arc::new(BlockingDecisionStore {
                inner: InMemoryApprovalStore::new(),
                entered: store_entered,
                release: Mutex::new(release_receiver),
            });
            let approvals = ApprovalService::with_store(store).expect("actual approval owner");
            let record = approvals
                .request(unrelated_approval())
                .expect("seed unrelated approval");
            let reviewer = Arc::new(PausedReviewer::new(ReviewVerdict::Allow));
            let observer = Arc::new(RecordingObserver::default());
            let review = Arc::new(
                BoundOperationReview::new(reviewer.clone(), approvals.clone(), REVIEW_DEADLINE)
                    .with_observer(observer.clone()),
            );
            let dispatch_finished = checked.clone();
            let final_check = Arc::new(FinalCheck {
                armed: AtomicBool::new(false),
                checks: AtomicUsize::new(0),
                checked,
            });
            let context = ToolDispatchContext::default()
                .with_work_authorization(Some(WorkAuthorizationContext::new(
                    Arc::new(ObservedWork {
                        inner: work.context.authorization().clone(),
                        final_check: final_check.clone(),
                    }),
                    work.context.execution_scope().clone(),
                )))
                .with_operation_review(Some(review));
            let body = Arc::new(RecordingDispatcher::default());
            let leaf = Arc::new(ContendedLeaf {
                inner: ReviewedLeaf::new(body.clone()),
                start_write,
                store_entered: Mutex::new(entered_receiver),
                final_check,
            });
            let writer = std::thread::spawn(move || {
                start_receiver
                    .recv_timeout(BOUND)
                    .map_err(|error| error.to_string())?;
                approvals
                    .decide(
                        &record.approval_id,
                        ApprovalDecision::Approve,
                        ApprovalPrincipalId::new("fixture-human")
                            .map_err(|error| error.to_string())?,
                        None,
                        None,
                    )
                    .map_err(|error| error.to_string())
            });
            let changing_work = work.clone();
            let controller = std::thread::spawn(move || {
                let observed = checked_receiver.recv_timeout(BOUND);
                if observed.is_ok() && revoke {
                    changing_work.revoke_tool(REVIEWED_TOOL);
                }
                // A safe owner may refuse contention before checking entry.
                // Completion also releases the writer; only a real final check
                // demonstrates the stale-spend interleaving on the current base.
                let released = release_write.send(());
                let observed = observed.map_err(|error| error.to_string())?;
                released.map_err(|error| error.to_string())?;
                Ok::<_, String>(observed)
            });
            reviewer.release.notify_one();
            let result = fenced(&leaf, &reviewed_call("reviewed"), &context).await;
            let _ = dispatch_finished.send(EntryProgress::DispatchFinished);
            // Drain both owned threads before any behavioral assertion.
            let written = writer.join();
            let changed = controller.join();
            assert_eq!(
                written
                    .expect("writer joined")
                    .expect("store write completed")
                    .status,
                ApprovalStatus::Approved
            );
            let observed = changed
                .expect("controller joined")
                .expect("entry or completion handshake completed");
            if matches!(observed, EntryProgress::DispatchFinished) {
                assert!(
                    matches!(
                        &result,
                        Err(ToolError::ReviewUnavailable {
                            kind: ReviewUnavailableKind::OwnerUnavailable,
                        })
                    ),
                    "only a typed early contention refusal may skip entry: {result:?}"
                );
            }

            if revoke {
                assert!(
                    matches!(&result,
                        Err(ToolError::AuthorizationRefused { refusal })
                            if refusal.kind() == meerkat_core::OperationRefusalKind::Denied
                    ) || matches!(
                        &result,
                        Err(ToolError::ReviewUnsatisfied {
                            kind: ReviewUnsatisfiedKind::ContextChanged,
                            ..
                        } | ToolError::ReviewUnavailable {
                            kind: ReviewUnavailableKind::OwnerUnavailable,
                        })
                    ),
                    "stale approval-owner spend must refuse locally: {result:?}"
                );
                assert!(
                    body_ids(&body).is_empty(),
                    "no effect after owner revocation"
                );
                assert!(
                    !observer.statuses().contains(&used()),
                    "a stale allow is never consumed"
                );
                fenced(
                    &ReviewedLeaf::new(body.clone()),
                    &sibling_call("sibling"),
                    &context,
                )
                .await
                .expect("unrevoked R1 sibling still enters");
                assert_eq!(body_ids(&body), vec!["sibling".to_string()]);
            } else {
                // A nonblocking owner may refuse contention locally. Once the
                // writer has drained, unchanged authority must still enter on
                // a fresh attempt, rather than turning contention into a denial.
                if matches!(
                    &result,
                    Err(ToolError::ReviewUnavailable {
                        kind: ReviewUnavailableKind::OwnerUnavailable,
                    })
                ) {
                    assert!(body_ids(&body).is_empty());
                    assert!(!observer.statuses().contains(&used()));
                    reviewer.release.notify_one();
                    fenced(
                        &ReviewedLeaf::new(body.clone()),
                        &reviewed_call("reviewed"),
                        &context,
                    )
                    .await
                    .expect("unchanged fresh attempt after contention");
                } else {
                    assert!(!result.expect("unchanged control enters").result.is_error);
                }
                assert_eq!(body_ids(&body), vec!["reviewed".to_string()]);
                assert_eq!(
                    observer
                        .statuses()
                        .iter()
                        .filter(|status| **status == used())
                        .count(),
                    1
                );
            }
        }
    }
}

#[tokio::test]
async fn changed_review_context_retires_only_that_operation_and_sibling_enters() {
    let work = reviewed_work(OperationReviewTier::R2);
    let world = ReviewWorld::new();
    let body = Arc::new(RecordingDispatcher::default());
    let leaf = ReviewedLeaf::new(Arc::clone(&body));
    let context = world.context(&work);
    let reviewed = reviewed_call("reviewed");
    let sibling = sibling_call("sibling");

    let controller = async {
        world.reviewer.wait_entered().await;
        // The R1 sibling is not parked behind the retained review.
        wait_for_body(&body, "sibling").await;
        assert_eq!(body_ids(&body), vec!["sibling".to_string()]);
        work.republish();
        world.reviewer.release.notify_one();
        // The one re-review of the superseded operation changes again.
        world.reviewer.wait_entered().await;
        work.republish();
        world.reviewer.release.notify_one();
    };
    let (reviewed_result, sibling_result, ()) = tokio::join!(
        fenced(&leaf, &reviewed, &context),
        fenced(&leaf, &sibling, &context),
        controller
    );

    assert_unsatisfied(reviewed_result, ReviewUnsatisfiedKind::ContextChanged);
    assert!(!sibling_result.expect("permitted sibling").result.is_error);
    assert_eq!(
        body_ids(&body),
        vec!["sibling".to_string()],
        "zero reviewed entry"
    );
    assert_eq!(
        world.reviewer.calls.load(Ordering::SeqCst),
        2,
        "one re-review and never a third; the R1 sibling is unreviewed"
    );
    // Currentness is validated before each verdict is accepted: neither
    // stale allow is ever recorded.
    assert_eq!(
        world.observer.statuses(),
        vec![
            pending(),
            retired(ReviewRetirementReason::ContextChanged),
            pending(),
            retired(ReviewRetirementReason::ContextChanged)
        ]
    );
    assert_eq!(world.observer.attempts("reviewed").len(), 2);
}

fn held(account: &str, mandate: &str) -> ReviewMaterial {
    ReviewMaterial::Held {
        account: account.to_owned(),
        mandate: mandate.to_owned(),
    }
}

fn pair(account: &str, mandate: &str) -> Option<(String, String)> {
    Some((account.to_owned(), mandate.to_owned()))
}

#[tokio::test]
async fn changed_owner_material_rejects_the_stale_allow_and_a_fresh_attempt_reviews_it() {
    let work = reviewed_work(OperationReviewTier::R2);
    work.set_review_material(held("account-a", "mandate-a"));
    let world = ReviewWorld::new();
    let body = Arc::new(RecordingDispatcher::default());
    let leaf = ReviewedLeaf::new(Arc::clone(&body));
    let context = world.context(&work);
    let reviewed = reviewed_call("reviewed");
    let sibling = sibling_call("sibling");

    let controller = async {
        world.reviewer.wait_entered().await;
        wait_for_body(&body, "sibling").await;
        // Same permission and the same R2 tier; only the owner material moves.
        work.set_review_material(held("account-b", "mandate-b"));
        world.reviewer.release.notify_one();
    };
    let (reviewed_result, sibling_result, ()) = tokio::join!(
        fenced(&leaf, &reviewed, &context),
        fenced(&leaf, &sibling, &context),
        controller
    );
    assert_unsatisfied(reviewed_result, ReviewUnsatisfiedKind::ContextChanged);
    assert!(!sibling_result.expect("permitted sibling").result.is_error);
    assert_eq!(
        body_ids(&body),
        vec!["sibling".to_string()],
        "no stale entry"
    );

    let fresh = reviewed_call("fresh");
    let release = async {
        world.reviewer.wait_entered().await;
        world.reviewer.release.notify_one();
    };
    let (fresh_result, ()) = tokio::join!(fenced(&leaf, &fresh, &context), release);
    assert!(!fresh_result.expect("fresh attempt").result.is_error);
    assert_eq!(
        body_ids(&body),
        vec!["sibling".to_string(), "fresh".to_string()],
        "the fresh attempt enters once"
    );
    assert_eq!(
        *world.reviewer.seen_contexts.lock().expect("seen contexts"),
        vec![
            pair("account-a", "mandate-a"),
            pair("account-b", "mandate-b")
        ],
        "each attempt reviews the material current at its own preparation"
    );
    assert_eq!(
        work.review_context_calls(),
        3,
        "read once per reviewed attempt, once by the decision that refused a \
         second review of changed material, and never for the R1 sibling"
    );
}

#[tokio::test]
async fn work_owner_without_the_context_read_fails_closed_before_any_attempt() {
    let work = reviewed_work(OperationReviewTier::R2);
    let world = ReviewWorld::new();
    let body = Arc::new(RecordingDispatcher::default());
    let leaf = ReviewedLeaf::new(Arc::clone(&body));
    let context = ToolDispatchContext::default()
        .with_work_authorization(Some(work.inheriting_context()))
        .with_operation_review(Some(world.review.clone()));
    assert_unavailable(
        fenced(&leaf, &reviewed_call("reviewed"), &context).await,
        ReviewUnavailableKind::ContextUnavailable,
    );
    assert_eq!(
        world.reviewer.calls.load(Ordering::SeqCst),
        0,
        "no reviewer call"
    );
    assert!(world.observer.statuses().is_empty(), "no review attempt");
    assert!(body_ids(&body).is_empty(), "no target entry");
    fenced(&leaf, &sibling_call("sibling"), &context)
        .await
        .expect("permitted R1 sibling enters");
    assert_eq!(body_ids(&body), vec!["sibling".to_string()]);
    assert_eq!(
        work.review_context_calls(),
        0,
        "the inner owner is never asked"
    );
}

#[tokio::test]
async fn review_context_outcomes_are_typed_before_any_attempt() {
    // Required but missing material is unavailable, never not-applicable; a
    // deliberate denial stays a refusal; an explicit not-applicable reviews
    // with no material. A permitted sibling enters in every case.
    for material in [
        ReviewMaterial::RequiredButMissing,
        ReviewMaterial::Denied,
        ReviewMaterial::NotApplicable,
    ] {
        let work = reviewed_work(OperationReviewTier::R2);
        work.set_review_material(material.clone());
        let world = ReviewWorld::new();
        let body = Arc::new(RecordingDispatcher::default());
        let leaf = ReviewedLeaf::new(Arc::clone(&body));
        let context = world.context(&work);
        let reviewed = reviewed_call("reviewed");
        let release = async {
            if matches!(material, ReviewMaterial::NotApplicable) {
                world.reviewer.wait_entered().await;
                world.reviewer.release.notify_one();
            }
        };
        let (result, ()) = tokio::join!(fenced(&leaf, &reviewed, &context), release);
        match material {
            ReviewMaterial::RequiredButMissing => {
                assert_unavailable(result, ReviewUnavailableKind::ContextUnavailable);
            }
            ReviewMaterial::Denied => assert!(
                matches!(&result, Err(ToolError::AuthorizationRefused { refusal })
                    if refusal.kind() == OperationRefusalKind::Denied),
                "{result:?}"
            ),
            ReviewMaterial::NotApplicable => {
                assert!(!result.expect("reviewed allow").result.is_error);
            }
            ReviewMaterial::Held { .. } => unreachable!("not exercised here"),
        }
        let failed = !matches!(material, ReviewMaterial::NotApplicable);
        if failed {
            assert_eq!(world.reviewer.calls.load(Ordering::SeqCst), 0);
            assert!(world.observer.statuses().is_empty(), "no review attempt");
            assert!(body_ids(&body).is_empty(), "no reviewed entry");
        } else {
            assert_eq!(
                *world.reviewer.seen_contexts.lock().expect("seen contexts"),
                vec![None]
            );
            assert_eq!(body_ids(&body), vec!["reviewed".to_string()]);
        }
        fenced(&leaf, &sibling_call("sibling"), &context)
            .await
            .expect("permitted sibling enters");
        assert!(body_ids(&body).contains(&"sibling".to_string()));
        assert_eq!(
            work.review_context_calls(),
            1,
            "once, never for the sibling"
        );
    }
}

#[tokio::test]
async fn owner_edit_racing_the_verdict_still_prevents_entry() {
    let work = Arc::new(reviewed_work(OperationReviewTier::R2));
    // An owner edit races each attempt's verdict, so the one re-review of
    // the superseded operation is superseded too.
    let reviewer = PausedReviewer::scripted([
        ReviewStep::after(republishing(&work), Ok(ReviewVerdict::Allow)),
        ReviewStep::after(republishing(&work), Ok(ReviewVerdict::Allow)),
    ]);
    let world = ReviewWorld::with_reviewer(reviewer);
    let body = Arc::new(RecordingDispatcher::default());
    let leaf = ReviewedLeaf::new(Arc::clone(&body));
    let context = world.context(&work);
    let reviewed = reviewed_call("reviewed");

    let controller = async {
        world.reviewer.wait_entered().await;
        world.reviewer.release.notify_one();
        world.reviewer.wait_entered().await;
        world.reviewer.release.notify_one();
    };
    let (result, ()) = tokio::join!(fenced(&leaf, &reviewed, &context), controller);

    assert_unsatisfied(result, ReviewUnsatisfiedKind::ContextChanged);
    assert!(body_ids(&body).is_empty());
    assert_eq!(world.reviewer.calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        world.observer.statuses(),
        vec![
            pending(),
            retired(ReviewRetirementReason::ContextChanged),
            pending(),
            retired(ReviewRetirementReason::ContextChanged)
        ]
    );
}

#[tokio::test]
async fn narrowly_revoked_authority_during_review_returns_the_authority_refusal() {
    let work = reviewed_work(OperationReviewTier::R2);
    let world = ReviewWorld::new();
    let body = Arc::new(RecordingDispatcher::default());
    let leaf = ReviewedLeaf::new(Arc::clone(&body));
    let context = world.context(&work);
    let reviewed = reviewed_call("reviewed");

    let controller = async {
        world.reviewer.wait_entered().await;
        work.revoke_tool(REVIEWED_TOOL);
        world.reviewer.release.notify_one();
    };
    let (result, ()) = tokio::join!(fenced(&leaf, &reviewed, &context), controller);

    // The authority owner's verdict is not relabelled as a review outcome.
    assert_tool_refused(result, OperationRefusalKind::Denied);
    assert!(body_ids(&body).is_empty());
    assert_eq!(
        world.observer.statuses(),
        vec![pending(), retired(ReviewRetirementReason::AuthorityChanged)]
    );
    // The narrow revocation leaves the R1 sibling permitted.
    fenced(&leaf, &sibling_call("sibling"), &context)
        .await
        .expect("permitted sibling after narrow revocation");
    assert_eq!(body_ids(&body), vec!["sibling".to_string()]);
}

#[tokio::test]
async fn owner_edit_during_entry_staging_after_allow_prevents_entry() {
    let work = reviewed_work(OperationReviewTier::R2);
    let world = ReviewWorld::new();
    let body = Arc::new(RecordingDispatcher::default());
    let leaf = ReviewedLeaf::new(Arc::clone(&body));
    let context = entry_hook_context(&world, &work, true);
    let reviewed = reviewed_call("reviewed");

    let controller = async {
        world.reviewer.wait_entered().await;
        world.reviewer.release.notify_one();
        world.reviewer.wait_entered().await;
        world.reviewer.release.notify_one();
    };
    let (result, ()) = tokio::join!(fenced(&leaf, &reviewed, &context), controller);

    // Each allow was recorded while current; an owner edit is armed by each
    // and lands during that attempt's entry staging, so the check with the
    // re-prepared decision retires it unspent. The dispatch reviews the
    // superseded operation once more, never a third time.
    assert_unsatisfied(result, ReviewUnsatisfiedKind::ContextChanged);
    assert!(body_ids(&body).is_empty(), "zero physical body entry");
    assert_eq!(world.reviewer.calls.load(Ordering::SeqCst), 2);
    let statuses = world.observer.statuses();
    assert_eq!(
        statuses,
        vec![
            pending(),
            allowed(),
            retired(ReviewRetirementReason::ContextChanged),
            pending(),
            allowed(),
            retired(ReviewRetirementReason::ContextChanged)
        ]
    );
    assert!(!statuses.contains(&used()), "no allow was ever spent");
}

#[tokio::test]
async fn an_owner_change_at_the_root_staging_is_reviewed_once_more_and_enters() {
    let work = reviewed_work(OperationReviewTier::R2);
    let world = ReviewWorld::new();
    let body = Arc::new(RecordingDispatcher::default());
    let leaf = ReviewedLeaf::new(Arc::clone(&body));
    // The first allowed attempt's root staging meets one completed change.
    let context = entry_hook_context_once(&world, &work);
    let reviewed = reviewed_call("reviewed");

    let controller = async {
        world.reviewer.wait_entered().await;
        world.reviewer.release.notify_one();
        world.reviewer.wait_entered().await;
        world.reviewer.release.notify_one();
    };
    let (result, ()) = tokio::join!(fenced(&leaf, &reviewed, &context), controller);

    assert!(
        !result
            .expect("reviewed once more, then entered")
            .result
            .is_error
    );
    assert_eq!(body_ids(&body), vec!["reviewed".to_string()]);
    assert_eq!(world.reviewer.calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        world.observer.statuses(),
        vec![
            pending(),
            allowed(),
            retired(ReviewRetirementReason::ContextChanged),
            pending(),
            allowed(),
            used()
        ]
    );
    assert_eq!(world.observer.attempts("reviewed").len(), 2);
}

#[tokio::test]
async fn one_re_review_covers_both_the_verdict_and_the_root_staging() {
    let work = reviewed_work(OperationReviewTier::R2);
    let world = ReviewWorld::new();
    let body = Arc::new(RecordingDispatcher::default());
    let leaf = ReviewedLeaf::new(Arc::clone(&body));
    // The first allow is the second attempt's: its root staging meets a
    // change after the first attempt was superseded during its review.
    let context = entry_hook_context_once(&world, &work);
    let reviewed = reviewed_call("reviewed");

    let controller = async {
        world.reviewer.wait_entered().await;
        work.republish();
        world.reviewer.release.notify_one();
        world.reviewer.wait_entered().await;
        world.reviewer.release.notify_one();
    };
    let (result, ()) = tokio::join!(fenced(&leaf, &reviewed, &context), controller);

    assert_unsatisfied(result, ReviewUnsatisfiedKind::ContextChanged);
    assert!(body_ids(&body).is_empty(), "zero entry");
    assert_eq!(
        world.reviewer.calls.load(Ordering::SeqCst),
        2,
        "one re-review per dispatch, whichever site superseded it"
    );
    assert_eq!(
        world.observer.statuses(),
        vec![
            pending(),
            retired(ReviewRetirementReason::ContextChanged),
            pending(),
            allowed(),
            retired(ReviewRetirementReason::ContextChanged)
        ]
    );
}

#[tokio::test]
async fn unchanged_owner_through_entry_staging_enters_once() {
    let work = reviewed_work(OperationReviewTier::R2);
    let world = ReviewWorld::new();
    let body = Arc::new(RecordingDispatcher::default());
    let leaf = ReviewedLeaf::new(Arc::clone(&body));
    let context = entry_hook_context(&world, &work, false);
    let reviewed = reviewed_call("reviewed");

    let controller = async {
        world.reviewer.wait_entered().await;
        world.reviewer.release.notify_one();
    };
    let (result, ()) = tokio::join!(fenced(&leaf, &reviewed, &context), controller);

    assert!(!result.expect("reviewed operation entered").result.is_error);
    assert_eq!(body_ids(&body), vec!["reviewed".to_string()]);
    assert_eq!(
        world.observer.statuses(),
        vec![pending(), allowed(), used()]
    );
}

#[tokio::test]
async fn r3_never_accepts_a_model_allow() {
    let work = reviewed_work(OperationReviewTier::R3);
    // The installed reviewer would allow; R3 must not consult it.
    let world = ReviewWorld::new();
    let body = Arc::new(RecordingDispatcher::default());
    let leaf = ReviewedLeaf::new(Arc::clone(&body));
    let context = world.context(&work);

    let result = fenced(&leaf, &reviewed_call("reviewed"), &context).await;

    assert_unsatisfied(result, ReviewUnsatisfiedKind::HumanConsentRequired);
    assert!(body_ids(&body).is_empty());
    assert_eq!(
        world.reviewer.calls.load(Ordering::SeqCst),
        0,
        "no model review for R3"
    );
    assert!(world.observer.statuses().is_empty());
    fenced(&leaf, &sibling_call("sibling"), &context)
        .await
        .expect("explicit R1 sibling still enters");
}

#[tokio::test]
async fn deny_and_escalate_settle_locally_without_entry() {
    for (verdict, expected) in [
        (ReviewVerdict::Deny, ReviewUnsatisfiedKind::Denied),
        (ReviewVerdict::Escalate, ReviewUnsatisfiedKind::Escalated),
    ] {
        let work = reviewed_work(OperationReviewTier::R2);
        let world = ReviewWorld::with_reviewer(PausedReviewer::new(verdict));
        let body = Arc::new(RecordingDispatcher::default());
        let leaf = ReviewedLeaf::new(Arc::clone(&body));
        let context = world.context(&work);
        let reviewed = reviewed_call("reviewed");
        let controller = async {
            world.reviewer.wait_entered().await;
            world.reviewer.release.notify_one();
        };
        let (result, ()) = tokio::join!(fenced(&leaf, &reviewed, &context), controller);
        assert_unsatisfied(result, expected);
        assert!(body_ids(&body).is_empty());
    }
}

#[tokio::test(start_paused = true)]
async fn review_deadline_retires_before_unavailable_feedback() {
    let work = reviewed_work(OperationReviewTier::R2);
    let mut reviewer = PausedReviewer::new(ReviewVerdict::Allow);
    reviewer.never_returns = true;
    let world = ReviewWorld::with_reviewer(reviewer);
    let body = Arc::new(RecordingDispatcher::default());
    let leaf = ReviewedLeaf::new(Arc::clone(&body));
    let context = world.context(&work);
    let reviewed = reviewed_call("reviewed");

    let dispatched = fenced(&leaf, &reviewed, &context);
    tokio::pin!(dispatched);
    tokio::select! {
        biased;
        result = &mut dispatched => panic!("review settled before its deadline: {result:?}"),
        () = world.reviewer.wait_entered() => {}
    }
    // Retained and still pending: the deadline has not been reached.
    assert_eq!(world.observer.statuses(), vec![pending()]);
    assert!(body_ids(&body).is_empty());

    tokio::time::advance(REVIEW_DEADLINE).await;
    let result = dispatched.await;

    assert_unavailable(result, ReviewUnavailableKind::DeadlineExpired);
    world.reviewer.assert_future_dropped_unfinished();
    assert!(body_ids(&body).is_empty());
    assert_eq!(
        world.observer.statuses(),
        vec![pending(), retired(ReviewRetirementReason::DeadlineExpired)],
        "retired through the owner before fallback; nothing follows"
    );
}

#[tokio::test(start_paused = true)]
async fn verdict_ready_exactly_at_expiry_never_wins() {
    let work = reviewed_work(OperationReviewTier::R2);
    let mut reviewer = PausedReviewer::new(ReviewVerdict::Allow);
    // The verdict and the absolute deadline become ready at the same instant.
    reviewer.returns_after = Some(REVIEW_DEADLINE);
    let world = ReviewWorld::with_reviewer(reviewer);
    let body = Arc::new(RecordingDispatcher::default());
    let leaf = ReviewedLeaf::new(Arc::clone(&body));
    let context = world.context(&work);

    let result = fenced(&leaf, &reviewed_call("reviewed"), &context).await;

    assert_unavailable(result, ReviewUnavailableKind::DeadlineExpired);
    assert!(body_ids(&body).is_empty());
    let statuses = world.observer.statuses();
    assert_eq!(
        statuses,
        vec![pending(), retired(ReviewRetirementReason::DeadlineExpired)]
    );
    assert!(
        !statuses.contains(&allowed()),
        "the late allow is never recorded"
    );
}

#[tokio::test(start_paused = true)]
async fn allow_expiring_before_the_leaf_spend_prevents_entry() {
    let work = reviewed_work(OperationReviewTier::R2);
    let world = ReviewWorld::new();
    let body = Arc::new(RecordingDispatcher::default());
    // The leaf's preparation wait carries it past the absolute deadline.
    let leaf = ReviewedLeaf::with(
        Arc::clone(&body),
        LeafBehavior {
            wait_before_entry: Some(REVIEW_DEADLINE),
            ..LeafBehavior::default()
        },
    );
    let context = world.context(&work);
    let reviewed = reviewed_call("reviewed");

    let controller = async {
        world.reviewer.wait_entered().await;
        world.reviewer.release.notify_one();
    };
    let (result, ()) = tokio::join!(fenced(&leaf, &reviewed, &context), controller);

    assert_unavailable(result, ReviewUnavailableKind::DeadlineExpired);
    assert!(body_ids(&body).is_empty());
    assert_eq!(
        world.observer.statuses(),
        vec![
            pending(),
            allowed(),
            retired(ReviewRetirementReason::DeadlineExpired)
        ]
    );
}

#[tokio::test]
async fn dropped_dispatch_is_abandoned_and_disposes_the_reviewer() {
    let work = reviewed_work(OperationReviewTier::R2);
    let world = ReviewWorld::new();
    let body = Arc::new(RecordingDispatcher::default());
    let leaf = ReviewedLeaf::new(Arc::clone(&body));
    let context = world.context(&work);
    let reviewed = reviewed_call("reviewed");

    let finished = tokio::select! {
        result = fenced(&leaf, &reviewed, &context) => Some(result),
        () = world.reviewer.wait_entered() => None,
    };
    // The dispatch future was dropped: no tool result exists at all.
    assert!(
        finished.is_none(),
        "a drop must not synthesize a tool result"
    );
    world.reviewer.assert_future_dropped_unfinished();
    assert!(body_ids(&body).is_empty());
    assert_eq!(
        world.observer.statuses(),
        vec![pending(), retired(ReviewRetirementReason::Abandoned)]
    );
}

// ---------------------------------------------------------------------------
// Supersession: one more model review when only the owner publication moved.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn superseded_review_is_reviewed_once_more_and_enters() {
    let work = reviewed_work(OperationReviewTier::R2);
    let world = ReviewWorld::new();
    let body = Arc::new(RecordingDispatcher::default());
    *world.observer.body.lock().expect("body") = Some(Arc::clone(&body));
    let leaf = ReviewedLeaf::new(Arc::clone(&body));
    let context = world.context(&work);
    let reviewed = reviewed_call("reviewed");

    let controller = async {
        world.reviewer.wait_entered().await;
        // A completed owner change that keeps permission, tier and material.
        work.republish();
        world.reviewer.release.notify_one();
        // The same operation is reviewed from the start under its current
        // decision; the superseded allow is never used.
        world.reviewer.wait_entered().await;
        assert!(
            body_ids(&body).is_empty(),
            "no entry before the second verdict"
        );
        world.reviewer.release.notify_one();
    };
    let (result, ()) = tokio::join!(fenced(&leaf, &reviewed, &context), controller);

    assert!(
        !result
            .expect("reviewed once more, then entered")
            .result
            .is_error
    );
    assert_eq!(body_ids(&body), vec!["reviewed".to_string()]);
    assert_eq!(world.reviewer.calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        world.observer.statuses(),
        vec![
            pending(),
            retired(ReviewRetirementReason::ContextChanged),
            pending(),
            allowed(),
            used()
        ]
    );
    assert_eq!(
        world.observer.attempts("reviewed").len(),
        2,
        "a second owner-issued attempt, never the first one reused"
    );
    assert_eq!(
        work.review_context_calls(),
        2,
        "the first attempt's read and the supersession decision's, which the \
         second attempt is shown"
    );
    assert_eq!(
        *world
            .observer
            .bodies_at_used
            .lock()
            .expect("bodies at used"),
        Some(1)
    );
}

/// Every record the real audit adapter appends for one work.
#[derive(Default)]
struct RecordingAudit(Mutex<Vec<AuthorizationAuditObservation>>);

impl AuthorizationAuditSink for RecordingAudit {
    fn append(
        &self,
        observation: AuthorizationAuditObservation,
    ) -> Result<(), OperationObservationError> {
        self.0.lock().expect("audit records").push(observation);
        Ok(())
    }
}

/// `work`'s own owner behind the audit adapter a native row records through.
fn audited(work: &ToolWork, audit: &Arc<RecordingAudit>) -> WorkAuthorizationContext {
    WorkAuthorizationContext::new(
        Arc::new(crate::audit::AuditedWorkAuthorization {
            inner: Arc::clone(work.context.authorization()),
            associations: Vec::new().into(),
            publication: work.publication.clone(),
            clock: Arc::new(FixtureClock::new()),
            sink: audit.clone(),
        }),
        work.context.execution_scope().clone(),
    )
}

#[tokio::test]
async fn the_audit_joins_both_attempts_through_the_successor_preparation() {
    let work = reviewed_work(OperationReviewTier::R2);
    let audit = Arc::new(RecordingAudit::default());
    let world = ReviewWorld::new();
    let body = Arc::new(RecordingDispatcher::default());
    let leaf = ReviewedLeaf::new(Arc::clone(&body));
    let context = ToolDispatchContext::default()
        .with_work_authorization(Some(audited(&work, &audit)))
        .with_operation_review(Some(world.review.clone()));
    let reviewed = reviewed_call("reviewed");

    let controller = async {
        world.reviewer.wait_entered().await;
        work.republish();
        world.reviewer.release.notify_one();
        world.reviewer.wait_entered().await;
        world.reviewer.release.notify_one();
    };
    let (result, ()) = tokio::join!(fenced(&leaf, &reviewed, &context), controller);
    assert!(!result.expect("reviewed once more").result.is_error);
    assert_eq!(body_ids(&body), vec!["reviewed".to_string()]);

    // One operation: the reviewed call (the fixture reviewer reads nothing).
    let records = audit.0.lock().expect("audit records").clone();
    let operation = records
        .first()
        .expect("audited records")
        .operation_id
        .clone();
    assert!(
        records
            .iter()
            .all(|record| record.operation_id == operation),
        "both attempts review the same operation"
    );
    let observations: Vec<&AuditObservation> =
        records.iter().map(|record| &record.observation).collect();
    let starts: Vec<(usize, &str)> = observations
        .iter()
        .enumerate()
        .filter_map(|(at, observation)| match observation {
            AuditObservation::ReviewAttemptStarted { attempt_ref } => {
                Some((at, attempt_ref.as_str()))
            }
            _ => None,
        })
        .collect();
    assert_eq!(starts.len(), 2, "exactly two review attempts started");
    let observed: Vec<String> = world
        .observer
        .attempts("reviewed")
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(
        starts
            .iter()
            .map(|(_, attempt)| *attempt)
            .collect::<Vec<_>>(),
        observed.iter().map(String::as_str).collect::<Vec<_>>(),
        "the audit names the observer's two attempts, in order"
    );
    let prepared: Vec<(usize, u64)> = observations
        .iter()
        .enumerate()
        .filter_map(|(at, observation)| match observation {
            AuditObservation::Prepared { policy, .. } => Some((at, policy.publication_sequence)),
            _ => None,
        })
        .collect();
    let (first_at, first_sequence) = prepared[0];
    assert!(
        first_at < starts[0].0,
        "the dispatch prepared before review"
    );
    let joins: Vec<&(usize, u64)> = prepared
        .iter()
        .filter(|(at, _)| starts[0].0 < *at && *at < starts[1].0)
        .collect();
    assert_eq!(
        joins.len(),
        1,
        "one successor preparation joins the two attempts"
    );
    assert!(
        joins[0].1 > first_sequence,
        "the successor was prepared under the newer publication"
    );
    assert!(
        observations.iter().all(|observation| match observation {
            AuditObservation::Refused { reason, .. } => {
                *reason == OperationRefusalKind::ReprepareRequired
            }
            AuditObservation::AuthorizationUnavailable { .. } => false,
            _ => true,
        }),
        "under one completed change only re-preparation is refused, and authorization \
         is never unavailable"
    );
    assert!(
        observations
            .iter()
            .enumerate()
            .all(
                |(at, observation)| !matches!(observation, AuditObservation::Entry { .. })
                    || at > starts[1].0
            ),
        "nothing entered before the second attempt"
    );
}

#[tokio::test]
async fn owner_change_in_progress_at_the_verdict_is_waited_out_then_reviewed_once_more() {
    let work = reviewed_work(OperationReviewTier::R2);
    // The change is still in progress when the first verdict returns.
    let world = ReviewWorld::with_reviewer(PausedReviewer::scripted([ReviewStep::after(
        hold_owner_change_for(&work, Duration::from_millis(50)),
        Ok(ReviewVerdict::Allow),
    )]));
    let body = Arc::new(RecordingDispatcher::default());
    let leaf = ReviewedLeaf::new(Arc::clone(&body));
    let context = world.context(&work);
    let reviewed = reviewed_call("reviewed");

    let controller = async {
        world.reviewer.wait_entered().await;
        world.reviewer.release.notify_one();
        world.reviewer.wait_entered().await;
        world.reviewer.release.notify_one();
    };
    let (result, ()) = tokio::join!(fenced(&leaf, &reviewed, &context), controller);

    // Without the wait the check after the verdict reads the change half
    // published and settles as changed authority.
    assert!(
        !result
            .expect("waited out, reviewed once more, then entered")
            .result
            .is_error
    );
    assert_eq!(body_ids(&body), vec!["reviewed".to_string()]);
    assert_eq!(world.reviewer.calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        world.observer.statuses(),
        vec![
            pending(),
            retired(ReviewRetirementReason::ContextChanged),
            pending(),
            allowed(),
            used()
        ]
    );
}

#[tokio::test]
async fn the_second_review_never_reuses_the_superseded_allow() {
    let work = reviewed_work(OperationReviewTier::R2);
    let world = ReviewWorld::with_reviewer(PausedReviewer::scripted([
        ReviewStep::answer(Ok(ReviewVerdict::Allow)),
        ReviewStep::answer(Ok(ReviewVerdict::Deny)),
    ]));
    let body = Arc::new(RecordingDispatcher::default());
    let leaf = ReviewedLeaf::new(Arc::clone(&body));
    let context = world.context(&work);
    let reviewed = reviewed_call("reviewed");

    let controller = async {
        world.reviewer.wait_entered().await;
        work.republish();
        world.reviewer.release.notify_one();
        world.reviewer.wait_entered().await;
        world.reviewer.release.notify_one();
    };
    let (result, ()) = tokio::join!(fenced(&leaf, &reviewed, &context), controller);

    assert_unsatisfied(result, ReviewUnsatisfiedKind::Denied);
    assert!(body_ids(&body).is_empty(), "zero entry");
    assert_eq!(world.reviewer.calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        world.observer.statuses(),
        vec![
            pending(),
            retired(ReviewRetirementReason::ContextChanged),
            pending(),
            (ReviewAttemptStatus::Denied, None)
        ]
    );
}

#[tokio::test]
async fn a_reviewer_failure_superseded_by_an_owner_change_is_reviewed_once_more() {
    let work = reviewed_work(OperationReviewTier::R2);
    // Under churn the reviewer's own request can fail: no verdict.
    let world = ReviewWorld::with_reviewer(PausedReviewer::scripted([
        ReviewStep::answer(Err(ReviewerFailure::Unavailable)),
        ReviewStep::answer(Ok(ReviewVerdict::Allow)),
    ]));
    let body = Arc::new(RecordingDispatcher::default());
    let leaf = ReviewedLeaf::new(Arc::clone(&body));
    let context = world.context(&work);
    let reviewed = reviewed_call("reviewed");

    let controller = async {
        world.reviewer.wait_entered().await;
        work.republish();
        world.reviewer.release.notify_one();
        world.reviewer.wait_entered().await;
        world.reviewer.release.notify_one();
    };
    let (result, ()) = tokio::join!(fenced(&leaf, &reviewed, &context), controller);

    assert!(!result.expect("reviewed once more").result.is_error);
    assert_eq!(body_ids(&body), vec!["reviewed".to_string()]);
    assert_eq!(world.reviewer.calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        world.observer.statuses(),
        vec![
            pending(),
            retired(ReviewRetirementReason::ContextChanged),
            pending(),
            allowed(),
            used()
        ]
    );
}

#[tokio::test]
async fn a_changed_tier_is_never_reviewed_again() {
    for tier in [OperationReviewTier::R3, OperationReviewTier::R1] {
        let work = reviewed_work(OperationReviewTier::R2);
        let world = ReviewWorld::new();
        let body = Arc::new(RecordingDispatcher::default());
        let leaf = ReviewedLeaf::new(Arc::clone(&body));
        let context = world.context(&work);
        let reviewed = reviewed_call("reviewed");

        let controller = async {
            world.reviewer.wait_entered().await;
            work.set_write_tier(tier);
            world.reviewer.release.notify_one();
        };
        let (result, ()) = tokio::join!(fenced(&leaf, &reviewed, &context), controller);

        assert_unsatisfied(result, ReviewUnsatisfiedKind::ContextChanged);
        assert!(body_ids(&body).is_empty(), "zero entry at {tier:?}");
        assert_eq!(world.reviewer.calls.load(Ordering::SeqCst), 1, "{tier:?}");
        assert_eq!(
            world.observer.statuses(),
            vec![pending(), retired(ReviewRetirementReason::ContextChanged)]
        );
    }
}

#[tokio::test(start_paused = true)]
async fn a_negative_verdict_is_never_asked_again_after_an_owner_change() {
    for verdict in [ReviewVerdict::Deny, ReviewVerdict::Escalate] {
        let work = Arc::new(reviewed_work(OperationReviewTier::R2));
        // The negative verdict races an owner edit; any later call would
        // answer with an allow.
        let mut reviewer =
            PausedReviewer::scripted([ReviewStep::after(republishing(&work), Ok(verdict))]);
        reviewer.returns_after = Some(Duration::from_secs(1));
        let world = ReviewWorld::with_reviewer(reviewer);
        let body = Arc::new(RecordingDispatcher::default());
        let leaf = ReviewedLeaf::new(Arc::clone(&body));
        let context = world.context(&work);

        let result = fenced(&leaf, &reviewed_call("reviewed"), &context).await;

        assert_unsatisfied(result, ReviewUnsatisfiedKind::ContextChanged);
        assert!(body_ids(&body).is_empty());
        assert_eq!(
            world.reviewer.calls.load(Ordering::SeqCst),
            1,
            "{verdict:?}"
        );
        assert_eq!(
            world.observer.statuses(),
            vec![pending(), retired(ReviewRetirementReason::ContextChanged)]
        );
    }
}

#[tokio::test(start_paused = true)]
async fn an_expired_review_is_never_asked_again_after_an_owner_change() {
    let work = reviewed_work(OperationReviewTier::R2);
    let mut reviewer = PausedReviewer::new(ReviewVerdict::Allow);
    reviewer.returns_after = Some(REVIEW_DEADLINE);
    let world = ReviewWorld::with_reviewer(reviewer);
    let body = Arc::new(RecordingDispatcher::default());
    let leaf = ReviewedLeaf::new(Arc::clone(&body));
    let context = world.context(&work);
    let reviewed = reviewed_call("reviewed");

    let controller = async {
        world.reviewer.wait_entered().await;
        work.republish();
    };
    let (result, ()) = tokio::join!(fenced(&leaf, &reviewed, &context), controller);

    assert_unavailable(result, ReviewUnavailableKind::DeadlineExpired);
    assert!(body_ids(&body).is_empty());
    assert_eq!(world.reviewer.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        world.observer.statuses(),
        vec![pending(), retired(ReviewRetirementReason::DeadlineExpired)]
    );
}

#[tokio::test]
async fn an_owner_change_at_the_leaf_entry_is_never_reviewed_again() {
    let work = Arc::new(reviewed_work(OperationReviewTier::R2));
    let world = ReviewWorld::new();
    let body = Arc::new(RecordingDispatcher::default());
    // Dispatcher code already ran: the leaf's own preparation is over.
    let leaf = ReviewedLeaf::editing_before_entry(Arc::clone(&body), republishing(&work));
    let context = world.context(&work);
    let reviewed = reviewed_call("reviewed");

    let controller = async {
        world.reviewer.wait_entered().await;
        world.reviewer.release.notify_one();
    };
    let (result, ()) = tokio::join!(fenced(&leaf, &reviewed, &context), controller);

    assert_unsatisfied(result, ReviewUnsatisfiedKind::ContextChanged);
    assert!(body_ids(&body).is_empty(), "zero physical body entry");
    assert_eq!(world.reviewer.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        world.observer.statuses(),
        vec![
            pending(),
            allowed(),
            retired(ReviewRetirementReason::ContextChanged)
        ]
    );
}

#[tokio::test(start_paused = true)]
async fn no_second_review_without_tool_time_for_a_whole_review() {
    // The first review takes 15 s of the caller's deadline; a second one
    // needs the whole 30 s review deadline.
    for (caller_deadline, reviewed_again) in [
        (Duration::from_secs(40), false),
        (Duration::from_mins(1), true),
    ] {
        let work = reviewed_work(OperationReviewTier::R2);
        let mut reviewer = PausedReviewer::new(ReviewVerdict::Allow);
        reviewer.returns_after = Some(Duration::from_secs(15));
        let world = ReviewWorld::with_reviewer(reviewer);
        let body = Arc::new(RecordingDispatcher::default());
        let leaf = ReviewedLeaf::new(Arc::clone(&body));
        let context = world.context(&work);
        let reviewed = reviewed_call("reviewed");

        let controller = async {
            world.reviewer.wait_entered().await;
            work.republish();
        };
        let (result, ()) = tokio::join!(
            fenced_within(&leaf, &reviewed, &context, caller_deadline),
            controller
        );

        if reviewed_again {
            assert!(
                !result
                    .expect("time for a whole review: reviewed once more")
                    .result
                    .is_error
            );
            assert_eq!(world.reviewer.calls.load(Ordering::SeqCst), 2);
            assert_eq!(body_ids(&body), vec!["reviewed".to_string()]);
        } else {
            assert_unsatisfied(result, ReviewUnsatisfiedKind::ContextChanged);
            assert_eq!(world.reviewer.calls.load(Ordering::SeqCst), 1);
            assert!(body_ids(&body).is_empty());
            assert_eq!(
                world.observer.statuses(),
                vec![pending(), retired(ReviewRetirementReason::ContextChanged)]
            );
        }
    }
}

#[tokio::test(start_paused = true)]
async fn a_stuck_owner_change_at_the_verdict_settles_after_the_wait_bound() {
    let bound = meerkat_core::authorization::OWNER_CHANGE_WAIT_BOUND;
    // (when the verdict returns, how long the dispatch takes in all): the
    // wait is the bound, and never outlasts the attempt's own deadline.
    for (verdict_at, settles_at) in [
        (Duration::ZERO, bound),
        (
            REVIEW_DEADLINE.saturating_sub(Duration::from_secs(1)),
            REVIEW_DEADLINE,
        ),
    ] {
        let work = reviewed_work(OperationReviewTier::R2);
        let (release_change, change_released) = std::sync::mpsc::channel();
        let mut reviewer = PausedReviewer::scripted([ReviewStep::after(
            hold_owner_change_until(&work, change_released),
            Ok(ReviewVerdict::Allow),
        )]);
        reviewer.returns_after = Some(verdict_at);
        let world = ReviewWorld::with_reviewer(reviewer);
        let body = Arc::new(RecordingDispatcher::default());
        let leaf = ReviewedLeaf::new(Arc::clone(&body));
        let context = world.context(&work);
        let reviewed = reviewed_call("reviewed");

        let started = tokio::time::Instant::now();
        let result = fenced(&leaf, &reviewed, &context).await;
        let took = started.elapsed();
        let _ = release_change.send(());

        // The refusal takes its ordinary path once the bounded wait is over.
        assert_tool_refused(result, OperationRefusalKind::ReprepareRequired);
        assert!(
            took >= settles_at && took < settles_at + Duration::from_millis(100),
            "settled after {took:?}, not at {settles_at:?}"
        );
        assert!(body_ids(&body).is_empty());
        assert_eq!(world.reviewer.calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            world.observer.statuses(),
            vec![pending(), retired(ReviewRetirementReason::AuthorityChanged)]
        );
    }
}

#[tokio::test(start_paused = true)]
async fn a_dispatch_never_reviews_a_superseded_call_a_third_time() {
    let work = Arc::new(reviewed_work(OperationReviewTier::R2));
    // Every verdict races an owner edit, for as many attempts as run.
    let mut reviewer = PausedReviewer::scripted([
        ReviewStep::after(republishing(&work), Ok(ReviewVerdict::Allow)),
        ReviewStep::after(republishing(&work), Ok(ReviewVerdict::Allow)),
        ReviewStep::after(republishing(&work), Ok(ReviewVerdict::Allow)),
    ]);
    reviewer.returns_after = Some(Duration::from_secs(1));
    let world = ReviewWorld::with_reviewer(reviewer);
    let body = Arc::new(RecordingDispatcher::default());
    let leaf = ReviewedLeaf::new(Arc::clone(&body));
    let context = world.context(&work);

    let result = fenced(&leaf, &reviewed_call("reviewed"), &context).await;

    assert_unsatisfied(result, ReviewUnsatisfiedKind::ContextChanged);
    assert!(body_ids(&body).is_empty());
    assert_eq!(
        world.reviewer.calls.load(Ordering::SeqCst),
        2,
        "one re-review per dispatch, never a third"
    );
    assert_eq!(
        world.observer.statuses(),
        vec![
            pending(),
            retired(ReviewRetirementReason::ContextChanged),
            pending(),
            retired(ReviewRetirementReason::ContextChanged)
        ]
    );
}

#[tokio::test]
async fn a_dropped_dispatch_abandons_the_second_review() {
    let work = reviewed_work(OperationReviewTier::R2);
    let world = ReviewWorld::new();
    let body = Arc::new(RecordingDispatcher::default());
    let leaf = ReviewedLeaf::new(Arc::clone(&body));
    let context = world.context(&work);
    let reviewed = reviewed_call("reviewed");

    let finished = tokio::select! {
        result = fenced(&leaf, &reviewed, &context) => Some(result),
        () = async {
            world.reviewer.wait_entered().await;
            work.republish();
            world.reviewer.release.notify_one();
            world.reviewer.wait_entered().await;
        } => None,
    };

    assert!(
        finished.is_none(),
        "a drop must not synthesize a tool result"
    );
    assert!(
        world.reviewer.fate.dropped.load(Ordering::SeqCst),
        "the second review future was disposed"
    );
    assert!(body_ids(&body).is_empty());
    assert_eq!(world.reviewer.calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        world.observer.statuses(),
        vec![
            pending(),
            retired(ReviewRetirementReason::ContextChanged),
            pending(),
            retired(ReviewRetirementReason::Abandoned)
        ]
    );
}

// ---------------------------------------------------------------------------
// Agent level: real factory agent; one batch with a natively allowed reviewed
// call and an R1 sibling; model continuation after local settlement.
// ---------------------------------------------------------------------------

struct ReviewBatchAttempt {
    client: Arc<ReviewBatchClient>,
    messages: Arc<Vec<Message>>,
    authorization: Option<meerkat_core::LlmRequestAuthorization>,
}

#[async_trait]
impl AgentLlmRequestAttempt for ReviewBatchAttempt {
    fn request_pressure(
        &self,
    ) -> Result<Option<meerkat_core::ProviderRequestPressure>, AgentError> {
        Ok(None)
    }

    async fn stream_response(
        &self,
        _assistant_message_id: meerkat_core::AssistantMessageId,
    ) -> Result<LlmStreamResult, AgentError> {
        self.client
            .stream_response_authorized(
                &self.messages,
                &[],
                0,
                None,
                None,
                self.authorization.clone(),
            )
            .await
    }
}

/// First request: one reviewed call and one R1 sibling. Then plain text.
#[derive(Default)]
struct ReviewBatchClient(Mutex<Vec<Vec<Message>>>);

#[async_trait]
impl AgentLlmClient for ReviewBatchClient {
    fn prepare_request_attempt_authorized(
        self: Arc<Self>,
        messages: Arc<Vec<Message>>,
        _tools: Arc<[Arc<ToolDef>]>,
        _max_tokens: u32,
        _temperature: Option<f32>,
        _provider_params: Option<ProviderParamsOverride>,
        authorization: Option<meerkat_core::LlmRequestAuthorization>,
    ) -> Result<Arc<dyn AgentLlmRequestAttempt>, AgentError> {
        Ok(Arc::new(ReviewBatchAttempt {
            client: self,
            messages,
            authorization,
        }))
    }

    fn request_attempt_authority(&self) -> RequestAttemptAuthority {
        RequestAttemptAuthority::Unified
    }

    async fn stream_response_authorized(
        &self,
        messages: &[Message],
        tools: &[Arc<ToolDef>],
        max_tokens: u32,
        temperature: Option<f32>,
        provider_params: Option<&ProviderParamsOverride>,
        authorization: Option<meerkat_core::LlmRequestAuthorization>,
    ) -> Result<LlmStreamResult, AgentError> {
        let prepared = authorization
            .as_ref()
            .map(|authorization| authorization.prepare(fixture_controller_target()))
            .transpose()
            .map_err(AgentError::from)?;
        let _current = prepared
            .as_ref()
            .map(|prepared| prepared.current())
            .transpose()
            .map_err(AgentError::from)?;
        self.stream_response(messages, tools, max_tokens, temperature, provider_params)
            .await
    }

    async fn stream_response(
        &self,
        messages: &[Message],
        _tools: &[Arc<ToolDef>],
        _max_tokens: u32,
        _temperature: Option<f32>,
        _provider_params: Option<&ProviderParamsOverride>,
    ) -> Result<LlmStreamResult, AgentError> {
        let mut requests = self.0.lock().expect("model requests");
        let first = requests.is_empty();
        requests.push(messages.to_vec());
        let blocks = if first {
            [reviewed_call("reviewed"), sibling_call("sibling")]
                .into_iter()
                .map(|call| AssistantBlock::ToolUse {
                    id: call.id,
                    name: call.name,
                    args: call.args,
                    meta: None,
                })
                .collect()
        } else {
            vec![AssistantBlock::Text {
                text: "continued after local review settlement".into(),
                meta: None,
            }]
        };
        Ok(LlmStreamResult::new(
            blocks,
            if first {
                StopReason::ToolUse
            } else {
                StopReason::EndTurn
            },
            meerkat_core::TurnUsage::host_declared(
                self.provider(),
                self.model(),
                Default::default(),
            )
            .into_inner(),
        ))
    }

    fn provider(&self) -> Provider {
        Provider::Anthropic
    }

    fn model(&self) -> &'static str {
        "claude-sonnet-4-5"
    }
}

/// Review composition is trusted host attachment on the factory, never a
/// build option; `None` builds a factory with no reviewer at all. The tool
/// dispatcher is the review-capable test leaf over the shared body counter.
#[cfg(feature = "workspace-tests")]
async fn review_agent(
    review: Option<Arc<BoundOperationReview>>,
    client: Arc<ReviewBatchClient>,
    body: Arc<RecordingDispatcher>,
    config: &Config,
) -> meerkat::DynAgent {
    let mut factory = meerkat::AgentFactory::minimal();
    if let Some(review) = review {
        factory = factory.with_operation_review(review);
    }
    factory
        .build_agent(
            meerkat::AgentBuildConfig {
                agent_llm_client_override: Some(client),
                tool_dispatcher_override: Some(ReviewedLeaf::new(body)),
                session_store_override: Some(Arc::new(RecordingStore::default())),
                ..meerkat::AgentBuildConfig::new("claude-sonnet-4-5")
            },
            config,
        )
        .await
        .expect("real factory with explicit host resources")
}

#[cfg(feature = "workspace-tests")]
async fn run_batch(
    agent: &mut meerkat::DynAgent,
    work: &ToolWork,
    events: mpsc::Sender<AgentEvent>,
) -> Result<meerkat_core::RunResult, AgentError> {
    tokio::time::timeout(
        BOUND,
        agent.run_with_events_and_work_authorization(
            "perform both fixture operations".to_string().into(),
            vec![],
            vec![],
            None,
            events,
            Some(work.context.clone()),
        ),
    )
    .await
    .expect("bounded actual agent run")
}

fn second_request_results(client: &ReviewBatchClient) -> Vec<ToolResult> {
    let requests = client.0.lock().expect("model requests");
    assert_eq!(
        requests.len(),
        2,
        "agent must send a follow-up model request"
    );
    requests[1]
        .iter()
        .flat_map(|message| match message {
            Message::ToolResults { results, .. } => results.clone(),
            _ => Vec::new(),
        })
        .collect()
}

fn assert_single_completion(received: &mut mpsc::Receiver<AgentEvent>) {
    let mut completed = 0;
    while let Ok(event) = received.try_recv() {
        assert!(
            !matches!(event, AgentEvent::RunFailed { .. }),
            "local review settlement must not fail the run"
        );
        if matches!(event, AgentEvent::RunCompleted { .. }) {
            completed += 1;
        }
    }
    assert_eq!(completed, 1);
}

fn result_text<'a>(results: &'a [ToolResult], id: &str) -> (&'a ToolResult, String) {
    let result = results
        .iter()
        .find(|result| result.tool_use_id == id)
        .expect("the tool call has a result");
    (result, meerkat_core::types::text_content(&result.content))
}

#[cfg(feature = "workspace-tests")]
#[tokio::test]
async fn factory_agent_settles_stale_review_locally_and_continues_with_sibling() {
    let world = ReviewWorld::new();
    let client = Arc::new(ReviewBatchClient::default());
    let body = Arc::new(RecordingDispatcher::default());
    let mut agent = review_agent(
        Some(world.review.clone()),
        client.clone(),
        body.clone(),
        &Config::default(),
    )
    .await;
    let work = ToolWork::for_session(agent.session().id());
    work.set_write_tier(OperationReviewTier::R2);
    let (events, mut received) = mpsc::channel(256);

    let controller = async {
        world.reviewer.wait_entered().await;
        wait_for_body(&body, "sibling").await;
        work.republish();
        world.reviewer.release.notify_one();
        // The one re-review of the superseded call changes again.
        world.reviewer.wait_entered().await;
        work.republish();
        world.reviewer.release.notify_one();
    };
    let (result, ()) = tokio::join!(run_batch(&mut agent, &work, events), controller);
    let result = result.expect("stale review is not a run failure");

    assert_eq!(result.turns, 2);
    let results = second_request_results(&client);
    assert_eq!(results.len(), 2);
    let (reviewed, reviewed_text) = result_text(&results, "reviewed");
    assert!(reviewed.is_error);
    assert!(
        reviewed_text.contains("review_unsatisfied"),
        "{reviewed_text}"
    );
    assert!(!result_text(&results, "sibling").0.is_error);
    assert_eq!(
        body_ids(&body),
        vec!["sibling".to_string()],
        "zero reviewed entry"
    );
    assert_eq!(
        world.observer.statuses(),
        vec![
            pending(),
            retired(ReviewRetirementReason::ContextChanged),
            pending(),
            retired(ReviewRetirementReason::ContextChanged)
        ]
    );
    assert_single_completion(&mut received);
}

#[cfg(feature = "workspace-tests")]
#[tokio::test]
async fn factory_agent_reviews_a_superseded_call_once_more_and_enters() {
    let world = ReviewWorld::new();
    let client = Arc::new(ReviewBatchClient::default());
    let body = Arc::new(RecordingDispatcher::default());
    let mut agent = review_agent(
        Some(world.review.clone()),
        client.clone(),
        body.clone(),
        &Config::default(),
    )
    .await;
    let work = ToolWork::for_session(agent.session().id());
    work.set_write_tier(OperationReviewTier::R2);
    let (events, mut received) = mpsc::channel(256);

    let controller = async {
        world.reviewer.wait_entered().await;
        wait_for_body(&body, "sibling").await;
        work.republish();
        world.reviewer.release.notify_one();
        world.reviewer.wait_entered().await;
        world.reviewer.release.notify_one();
    };
    let (result, ()) = tokio::join!(run_batch(&mut agent, &work, events), controller);
    let result = result.expect("a superseded review is not a run failure");

    // The second review settles inside the same tool call: no extra turn.
    assert_eq!(result.turns, 2);
    let results = second_request_results(&client);
    assert!(results.iter().all(|result| !result.is_error));
    let mut entered = body_ids(&body);
    entered.sort();
    assert_eq!(entered, vec!["reviewed".to_string(), "sibling".to_string()]);
    assert_eq!(
        world.observer.statuses(),
        vec![
            pending(),
            retired(ReviewRetirementReason::ContextChanged),
            pending(),
            allowed(),
            used()
        ]
    );
    assert_single_completion(&mut received);
}

#[cfg(feature = "workspace-tests")]
#[tokio::test]
async fn factory_agent_narrow_revocation_keeps_sibling_controller_and_continuation() {
    let world = ReviewWorld::new();
    let client = Arc::new(ReviewBatchClient::default());
    let body = Arc::new(RecordingDispatcher::default());
    let mut agent = review_agent(
        Some(world.review.clone()),
        client.clone(),
        body.clone(),
        &Config::default(),
    )
    .await;
    let work = ToolWork::for_session(agent.session().id());
    work.set_write_tier(OperationReviewTier::R2);
    let (events, mut received) = mpsc::channel(256);

    let controller = async {
        world.reviewer.wait_entered().await;
        wait_for_body(&body, "sibling").await;
        // Narrow: only the reviewed tool loses authority; the controller and
        // the sibling's relation stay permitted.
        work.revoke_tool(REVIEWED_TOOL);
        world.reviewer.release.notify_one();
    };
    let (result, ()) = tokio::join!(run_batch(&mut agent, &work, events), controller);
    let result = result.expect("authority refusal of one call is not a run failure");

    assert_eq!(
        result.turns, 2,
        "controller still serves the second request"
    );
    let results = second_request_results(&client);
    let (reviewed, reviewed_text) = result_text(&results, "reviewed");
    assert!(reviewed.is_error);
    assert!(
        reviewed_text.contains("operation_refused"),
        "{reviewed_text}"
    );
    assert!(!result_text(&results, "sibling").0.is_error);
    assert_eq!(body_ids(&body), vec!["sibling".to_string()]);
    assert_eq!(
        world.observer.statuses(),
        vec![pending(), retired(ReviewRetirementReason::AuthorityChanged)]
    );
    assert_single_completion(&mut received);
}

#[cfg(feature = "workspace-tests")]
#[tokio::test]
async fn factory_review_denial_and_escalation_preserve_goal_sibling_and_model_continuation() {
    for (verdict, expected) in [
        (ReviewVerdict::Deny, ReviewUnsatisfiedKind::Denied),
        (ReviewVerdict::Escalate, ReviewUnsatisfiedKind::Escalated),
    ] {
        let world = ReviewWorld::with_reviewer(PausedReviewer::new(verdict));
        let client = Arc::new(ReviewBatchClient::default());
        let body = Arc::new(RecordingDispatcher::default());
        let mut agent = review_agent(
            Some(world.review.clone()),
            client.clone(),
            body.clone(),
            &Config::default(),
        )
        .await;
        let session_id = agent.session().id().clone();
        let work = ToolWork::for_session(&session_id);
        work.set_write_tier(OperationReviewTier::R2);
        let (events, mut received) = mpsc::channel(256);

        let release_review = async {
            world.reviewer.wait_entered().await;
            wait_for_body(&body, "sibling").await;
            world.reviewer.release.notify_one();
        };
        let (result, ()) = tokio::join!(run_batch(&mut agent, &work, events), release_review);
        let result = result.expect("review denial or escalation must stay local to its operation");

        assert_eq!(result.turns, 2, "the same run reaches its follow-up model");
        assert_eq!(agent.session().id(), &session_id);
        let results = second_request_results(&client);
        assert_eq!(results.len(), 2, "one result per original tool-use ID");
        let (reviewed, reviewed_text) = result_text(&results, "reviewed");
        assert!(reviewed.is_error);
        assert_eq!(
            reviewed_text,
            ToolError::ReviewUnsatisfied {
                kind: expected,
                approval_id: None,
            }
            .to_transcript_content(),
            "review feedback must retain its type instead of claiming native permission denial"
        );
        assert!(!result_text(&results, "sibling").0.is_error);
        assert_eq!(body_ids(&body), vec!["sibling".to_string()]);
        assert_eq!(world.reviewer.calls.load(Ordering::SeqCst), 1);
        assert!(world.reviewer.fate.completed.load(Ordering::SeqCst));
        world.observer.single_attempt();
        assert!(world.observer.statuses().iter().all(|(status, _)| {
            !matches!(
                status,
                ReviewAttemptStatus::Allowed | ReviewAttemptStatus::Used
            )
        }));
        let requests = client.0.lock().expect("model requests");
        let original_goal = requests[0]
            .iter()
            .find(|message| match message {
                Message::User(user) => {
                    meerkat_core::types::text_content(&user.content)
                        == "perform both fixture operations"
                }
                _ => false,
            })
            .expect("the original user goal was sent to the first model");
        assert!(
            requests[1].contains(original_goal),
            "local review feedback must preserve the exact original goal message"
        );
        drop(requests);
        assert_single_completion(&mut received);
    }
}

#[cfg(feature = "workspace-tests")]
#[tokio::test]
async fn factory_agent_unchanged_review_enters_with_sibling() {
    let world = ReviewWorld::new();
    let client = Arc::new(ReviewBatchClient::default());
    let body = Arc::new(RecordingDispatcher::default());
    let mut agent = review_agent(
        Some(world.review.clone()),
        client.clone(),
        body.clone(),
        &Config::default(),
    )
    .await;
    let work = ToolWork::for_session(agent.session().id());
    work.set_write_tier(OperationReviewTier::R2);
    let (events, mut received) = mpsc::channel(256);

    let controller = async {
        world.reviewer.wait_entered().await;
        world.reviewer.release.notify_one();
    };
    let (result, ()) = tokio::join!(run_batch(&mut agent, &work, events), controller);
    let result = result.expect("unchanged review run");

    assert_eq!(result.turns, 2);
    let results = second_request_results(&client);
    assert!(results.iter().all(|result| !result.is_error));
    let mut entered = body_ids(&body);
    entered.sort();
    assert_eq!(entered, vec!["reviewed".to_string(), "sibling".to_string()]);
    assert_eq!(
        world.observer.statuses(),
        vec![pending(), allowed(), used()]
    );
    assert_single_completion(&mut received);
}

#[cfg(feature = "workspace-tests")]
#[tokio::test]
async fn factory_without_reviewer_settles_required_review_locally() {
    for (tier, expected_code) in [
        (OperationReviewTier::R2, "review_unavailable"),
        (OperationReviewTier::R3, "review_unsatisfied"),
    ] {
        let client = Arc::new(ReviewBatchClient::default());
        let body = Arc::new(RecordingDispatcher::default());
        let mut agent = review_agent(None, client.clone(), body.clone(), &Config::default()).await;
        let work = ToolWork::for_session(agent.session().id());
        work.set_write_tier(tier);
        let (events, mut received) = mpsc::channel(256);

        let result = run_batch(&mut agent, &work, events)
            .await
            .expect("required review without a reviewer is not a run failure");

        assert_eq!(result.turns, 2);
        let results = second_request_results(&client);
        let (reviewed, reviewed_text) = result_text(&results, "reviewed");
        assert!(reviewed.is_error);
        assert!(
            reviewed_text.contains(expected_code),
            "{tier:?}: {reviewed_text}"
        );
        assert!(!result_text(&results, "sibling").0.is_error);
        assert_eq!(
            body_ids(&body),
            vec!["sibling".to_string()],
            "zero entry at {tier:?}"
        );
        assert_single_completion(&mut received);
    }
}

#[cfg(feature = "workspace-tests")]
#[tokio::test(start_paused = true)]
async fn agent_tool_timeout_keeps_timeout_and_abandons_review() {
    // The agent loop's own per-call timeout drops the dispatch. The tool
    // owner's Timeout is the call's result; the review owner records only
    // Abandoned; the turn continues, so nothing is caller cancellation.
    let mut reviewer = PausedReviewer::new(ReviewVerdict::Allow);
    reviewer.never_returns = true;
    let world = ReviewWorld::with_reviewer(reviewer);
    let client = Arc::new(ReviewBatchClient::default());
    let body = Arc::new(RecordingDispatcher::default());
    let tool_timeout = Duration::from_secs(2);
    assert!(tool_timeout < REVIEW_DEADLINE);
    let mut config = Config::default();
    config
        .tools
        .tool_timeouts
        .insert(REVIEWED_TOOL.to_string(), tool_timeout);
    let mut agent = review_agent(
        Some(world.review.clone()),
        client.clone(),
        body.clone(),
        &config,
    )
    .await;
    let work = ToolWork::for_session(agent.session().id());
    work.set_write_tier(OperationReviewTier::R2);
    let (events, mut received) = mpsc::channel(256);

    let result = run_batch(&mut agent, &work, events)
        .await
        .expect("a tool timeout is not a run failure");

    assert_eq!(result.turns, 2);
    let results = second_request_results(&client);
    let (reviewed, reviewed_text) = result_text(&results, "reviewed");
    assert!(reviewed.is_error);
    assert!(reviewed_text.contains("timeout"), "{reviewed_text}");
    assert!(
        !reviewed_text.contains("review_unavailable"),
        "the enclosing timeout is not the review deadline: {reviewed_text}"
    );
    assert!(!result_text(&results, "sibling").0.is_error);
    world.reviewer.assert_future_dropped_unfinished();
    assert_eq!(body_ids(&body), vec!["sibling".to_string()]);
    assert_eq!(
        world.observer.statuses(),
        vec![pending(), retired(ReviewRetirementReason::Abandoned)]
    );
    assert_single_completion(&mut received);
}

/// Agent-level cleanup after a hard interrupt, as the session owner performs
/// it: drop the run future, clear the work context, take the dropped run's
/// canonical terminal. This is Agent cleanup coverage, not a
/// `SessionService::interrupt` test.
#[cfg(feature = "workspace-tests")]
#[tokio::test]
async fn dropped_run_with_pending_review_stays_cancelled_through_agent_cleanup() {
    let world = ReviewWorld::new();
    let client = Arc::new(ReviewBatchClient::default());
    let body = Arc::new(RecordingDispatcher::default());
    let mut agent = review_agent(
        Some(world.review.clone()),
        client.clone(),
        body.clone(),
        &Config::default(),
    )
    .await;
    let work = ToolWork::for_session(agent.session().id());
    work.set_write_tier(OperationReviewTier::R2);
    let (events, mut received) = mpsc::channel(256);

    let finished = {
        let run = agent.run_with_events_and_work_authorization(
            "perform both fixture operations".to_string().into(),
            vec![],
            vec![],
            None,
            events,
            Some(work.context.clone()),
        );
        tokio::select! {
            result = run => Some(result),
            () = world.reviewer.wait_entered() => None,
        }
    };
    assert!(finished.is_none(), "the run must still be awaiting review");
    world.reviewer.assert_future_dropped_unfinished();
    agent.clear_work_authorization();
    let terminal = agent
        .cancel_dropped_run()
        .expect("dropped run owes a terminal");
    assert!(
        matches!(
            &terminal,
            AgentEvent::RunFailed { error_report, .. }
                if error_report.class == AgentErrorClass::Cancelled
        ),
        "expected cancelled RunFailed terminal, got {terminal:?}"
    );

    assert!(
        !body_ids(&body).iter().any(|id| id == "reviewed"),
        "a cancelled review never enters"
    );
    assert_eq!(
        client.0.lock().expect("model requests").len(),
        1,
        "no follow-up model request after caller cancellation"
    );
    for message in agent.session().messages() {
        if let Message::ToolResults { results, .. } = message {
            for result in results {
                let text = meerkat_core::types::text_content(&result.content);
                assert!(
                    !text.contains("review_unsatisfied") && !text.contains("review_unavailable"),
                    "caller cancellation relabelled as review feedback: {text}"
                );
            }
        }
    }
    while let Ok(event) = received.try_recv() {
        assert!(!matches!(event, AgentEvent::RunCompleted { .. }));
    }
    // The review owner records only that the attempt was abandoned; caller
    // cancellation is the turn owner's outcome asserted above.
    assert_eq!(
        world.observer.statuses(),
        vec![pending(), retired(ReviewRetirementReason::Abandoned)]
    );
}

// A blocking effect outlives its cancelled awaiting dispatch. This uses the
// real prepared entry and retained review, not a synthetic refusal hook.
mod entered_worker_cancellation {
    use super::*;

    struct BlockingLeaf {
        catalog: RecordingDispatcher,
        consumed: Arc<Notify>,
        release: Mutex<Option<std::sync::mpsc::Receiver<()>>>,
        spare: Mutex<Option<meerkat_core::ReviewedEntryTicket>>,
        worker: Mutex<Option<tokio::task::JoinHandle<Result<(), ToolError>>>>,
        effects: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl AgentToolDispatcher for BlockingLeaf {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            self.catalog.tools()
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
            let ticket = context.reviewed_entry_ticket(call, None)?;
            *self.spare.lock().expect("spare entry") =
                Some(context.reviewed_entry_ticket(call, None)?);
            let release = self
                .release
                .lock()
                .expect("worker gate")
                .take()
                .expect("one worker dispatch");
            let consumed = Arc::clone(&self.consumed);
            let effects = Arc::clone(&self.effects);
            let (finished_tx, finished_rx) = tokio::sync::oneshot::channel();
            let worker = tokio::task::spawn_blocking(move || {
                // Keep whatever entry custody the public API returns through
                // the whole body. At this base it is unit, which cannot defer
                // Used until this independently running leaf completes.
                let result = ticket.enter().and_then(|_entry_custody| {
                    consumed.notify_one();
                    release.recv_timeout(BOUND).map_err(|error| {
                        ToolError::execution_failed(format!("worker release failed: {error}"))
                    })?;
                    effects.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                });
                let _ = finished_tx.send(result.clone());
                result
            });
            // The test owns the handle even if this waiting future is dropped.
            *self.worker.lock().expect("worker handle") = Some(worker);
            finished_rx.await.map_err(|error| {
                ToolError::execution_failed(format!("worker completion lost: {error}"))
            })??;
            Ok(ToolResult::new(call.id.to_string(), "worker completed".to_string(), false).into())
        }
    }

    #[tokio::test]
    async fn cancelled_outer_dispatch_defers_used_until_entered_worker_completes() {
        let work = reviewed_work(OperationReviewTier::R2);
        let world = ReviewWorld::new();
        let context = world.context(&work);
        let call = reviewed_call("reviewed-worker");
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let leaf = Arc::new(BlockingLeaf {
            catalog: RecordingDispatcher::default(),
            consumed: Arc::new(Notify::new()),
            release: Mutex::new(Some(release_rx)),
            spare: Mutex::new(None),
            worker: Mutex::new(None),
            effects: Arc::new(AtomicUsize::new(0)),
        });
        let effects_at_used = Arc::new(AtomicUsize::new(usize::MAX));
        let observed = Arc::clone(&effects_at_used);
        let effects = Arc::clone(&leaf.effects);
        *world.observer.edit_on_used.lock().expect("Used witness") = Some(Box::new(move || {
            observed.store(effects.load(Ordering::SeqCst), Ordering::SeqCst);
        }));
        world.reviewer.release.notify_one();
        let mut dispatch = Box::pin(fenced(&leaf, &call, &context));
        let reached_entry = tokio::time::timeout(BOUND, async {
            tokio::select! {
                result = &mut dispatch => Err(format!("dispatch ended before held entry: {result:?}")),
                () = leaf.consumed.notified() => Ok(()),
            }
        }).await;
        let before_drop = world.observer.statuses();
        let effects_while_held = leaf.effects.load(Ordering::SeqCst);

        // This drops the real fenced dispatch, including ReviewDispatchGuard;
        // the already-entered blocking worker remains held before its body.
        drop(dispatch);
        let after_drop = world.observer.statuses();
        let spare = leaf.spare.lock().expect("spare entry").take();
        let late_entry = spare.map(|ticket| ticket.enter());
        let after_late_entry = world.observer.statuses();

        // Cleanup precedes every assertion, including the intended RED. The
        // bounded receive also releases on sender drop if setup panics.
        let release_result = release_tx.send(());
        drop(release_tx);
        let worker = leaf.worker.lock().expect("worker handle").take();
        let (joined, join_timed_out) = match worker {
            Some(mut worker) => match tokio::time::timeout(BOUND, &mut worker).await {
                Ok(result) => (Some(result), false),
                Err(_) => {
                    worker.abort();
                    // An already-running blocking task cannot be aborted, but
                    // its only wait was bounded and its gate is now released.
                    (Some(worker.await), true)
                }
            },
            None => (None, false),
        };
        let after_completion = world.observer.statuses();

        assert!(matches!(reached_entry, Ok(Ok(()))), "{reached_entry:?}");
        assert!(
            release_result.is_ok(),
            "the worker remained held until release"
        );
        assert!(!join_timed_out, "worker completion exceeded the hang guard");
        assert!(matches!(joined, Some(Ok(Ok(())))), "{joined:?}");
        assert_eq!(effects_while_held, 0);
        assert_eq!(leaf.effects.load(Ordering::SeqCst), 1);
        assert_eq!(before_drop, vec![pending(), allowed()]);
        assert_eq!(
            after_drop,
            vec![pending(), allowed()],
            "outer cancellation must not deliver Used while the entered worker is held"
        );
        assert!(
            matches!(
                late_entry,
                Some(Err(ToolError::ReviewUnsatisfied {
                    kind: ReviewUnsatisfiedKind::AlreadyConsumed,
                    ..
                } | ToolError::ReviewUnavailable {
                    kind: ReviewUnavailableKind::DispatchEnded,
                }))
            ),
            "an unspent clone cannot enter after cancellation: {late_entry:?}"
        );
        assert_eq!(after_late_entry, vec![pending(), allowed()]);
        assert_eq!(
            after_completion,
            vec![pending(), allowed(), used()],
            "the completed leaf delivers exactly one Used despite outer cancellation"
        );
        assert_eq!(
            effects_at_used.load(Ordering::SeqCst),
            1,
            "the actual Used callback observes the completed worker body"
        );
        assert_eq!(world.reviewer.calls.load(Ordering::SeqCst), 1);
    }
}
