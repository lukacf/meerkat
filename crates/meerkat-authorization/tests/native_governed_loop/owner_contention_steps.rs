//! Two actors on one authorization publication, on the real native owners
//! (the generated grant owner, the grant-backed work owner with its audit
//! wrapper, the fenced dispatch and the review owner). Actor A holds the
//! publication's writer on its own thread, as an issuance with its
//! acknowledged append, another dispatch's final entry or a host's owner
//! change does: variant 1 a held exclusion that never publishes
//! (`try_with_owner_changes_excluded`), variant 2 a published owner change
//! (`begin_owner_change`). Actor B is a governed tool dispatch whose further
//! steps meet that hold: an R1 child issuance of the dispatch's own operation,
//! then an R2 run step of an entered operation.
//!
//! B's waiting steps wait for A's hold outside every lock, within the call's
//! remaining tool time, and decide afresh; the R2 review stays spent once.
//! A poisoned publication is unavailable at once and never waited for. On
//! the base, without the waiting steps, B failed at once (variant 1:
//! `OperationAuthorizationUnavailable` from the work owner's reservation;
//! variant 2: a refusal to rebuild from the currentness check), with no
//! wait.
//!
//! The wait witness is the step's first poll: pending means the step met
//! A's hold and is waiting for it (A releases only after that poll), ready
//! means it answered at once. Only the 50 ms hold of the waited rows is a
//! sleep; every other order is a channel.

use super::*;
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};

use meerkat_authorization::grant_policy::GrantBackedWorkPolicy;
use meerkat_authorization::grants::{ChildGrantIntent, ChildGrantOutcome, ChildGrantRefusal};
use meerkat_authorization::work::{FurtherOriginals, OriginalBasisSource};
use meerkat_authorization_contracts::audit::{
    AuthorizationAuditObservation, AuthorizationAuditSink,
};
use meerkat_core::agent::{
    dispatch_tool_execution_plan_fenced, resolve_tool_execution_plan_fenced,
};
use meerkat_core::approval::review::{
    BoundOperationReview, OperationReviewObserver, OperationReviewer, ReviewAttemptStatus,
    ReviewCandidate, ReviewEntrySupport, ReviewObservation, ReviewUnsatisfiedKind, ReviewVerdict,
    ReviewerFailure,
};
use meerkat_core::authorization::{
    OWNER_CHANGE_WAIT_BOUND, OperationObservationError, OperationReviewTier,
};

/// B's call: `read_record` on `record-7`.
const STEP_CALL: &str = "contended-read";
/// The intended child of B's R1 issuance.
const CHILD: &str = "contended-helper-child";
/// B's tool time when nothing should cut its wait short.
const TOOL_TIME: Duration = Duration::from_secs(600);
/// B's tool time when A holds past it: well inside the 2 s owner-change
/// bound, so the bound that ends B's wait is the tool time.
const SHORT_TOOL_TIME: Duration = Duration::from_secs(1);
/// The documented hold of the waited rows, after B started waiting.
const HOLD: Duration = Duration::from_millis(50);

/// The records owner at the tier the test sets for the record tool (0 is
/// R1, 1 is R2), with no separately selected action account.
struct TieredRecords(AtomicU8);
impl OperationPolicyOwner for TieredRecords {
    fn authorize_operation(
        &self,
        association: &InputAuthorityAssociation,
        binding: &PreparedAuthorizationBinding,
        purpose: LocalPolicyPurpose,
        now_ms: u64,
    ) -> Result<LocalPolicyAllowance, meerkat_core::OperationAuthorizationError> {
        let mut allowance =
            RecordOwner.authorize_operation(association, binding, purpose, now_ms)?;
        if matches!(binding.facts().operation, AuthorizationOperation::Tool(_))
            && self.0.load(Ordering::SeqCst) == 1
        {
            allowance.review_tier = OperationReviewTier::R2;
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

/// The bound model reviewer of B's R2 call: it allows, and counts.
#[derive(Default)]
struct CountingReviewer(AtomicUsize);
#[async_trait]
impl OperationReviewer for CountingReviewer {
    async fn review(
        &self,
        _candidate: &ReviewCandidate<'_>,
    ) -> Result<ReviewVerdict, ReviewerFailure> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(ReviewVerdict::Allow)
    }
}

/// Counts the review attempts reported `Used`.
#[derive(Default)]
struct UsedReports(AtomicUsize);
impl OperationReviewObserver for UsedReports {
    fn observe(&self, observation: ReviewObservation) {
        if observation.status == ReviewAttemptStatus::Used {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
}

/// The work's own audit sink: every observation is accepted.
struct AcceptedAudit;
impl AuthorizationAuditSink for AcceptedAudit {
    fn append(&self, _: AuthorizationAuditObservation) -> Result<(), OperationObservationError> {
        Ok(())
    }
}

/// Every original is active, and none is further retained.
struct ActiveOriginals;
impl OriginalBasisSource for ActiveOriginals {
    fn require_active(
        &self,
        _: &QualifiedIngressNamespace,
        _: &OriginalWorkRef,
    ) -> Result<(), meerkat_core::OperationAuthorizationError> {
        Ok(())
    }

    fn active_retained_basis(
        &self,
        _: &QualifiedIngressNamespace,
        _: &OriginalWorkRef,
    ) -> Result<Option<WorkAuthorityBasis>, meerkat_core::OperationAuthorizationError> {
        Ok(None)
    }
}

/// How actor A holds the publication writer.
#[derive(Clone, Copy, Debug)]
enum Hold {
    /// Variant 1: a held exclusion, which never publishes.
    Excluded,
    /// Variant 2: a published owner change.
    Change,
}

/// Actor A, holding the publication writer on its own thread until it is
/// released.
struct Holder {
    release: std::sync::mpsc::Sender<()>,
    thread: std::thread::JoinHandle<()>,
}

impl Holder {
    /// Begin `hold` on a new thread; returns once A holds the writer.
    fn begin(
        grants: &Arc<LocalGrantAuthority>,
        publication: &LocalAuthorizationPublication,
        hold: Hold,
    ) -> Self {
        let (held, holding) = std::sync::mpsc::channel();
        let (release, released) = std::sync::mpsc::channel::<()>();
        let grants = Arc::clone(grants);
        let publication = publication.clone();
        let thread = std::thread::spawn(move || {
            let hold_until_released = || {
                held.send(()).expect("A reports its hold");
                // Released, or the sender dropped: `recv` returns either way.
                let _ = released.recv();
            };
            match hold {
                Hold::Excluded => grants
                    .try_with_owner_changes_excluded(hold_until_released)
                    .expect("A takes the free writer"),
                Hold::Change => {
                    let change = publication
                        .begin_owner_change()
                        .expect("A begins an owner change");
                    hold_until_released();
                    drop(change);
                }
            }
        });
        holding.recv().expect("A holds the writer");
        Self { release, thread }
    }

    /// A releases the writer, and its thread ends.
    fn release(self) {
        drop(self.release);
        self.thread.join().expect("A's thread");
    }
}

/// When A releases its hold.
#[derive(Clone, Copy, Debug)]
enum Release {
    /// The documented hold: `HOLD` after B started waiting.
    AfterHold,
    /// Only after B's step answered: past B's remaining tool time.
    AfterTheStep,
}

/// One row of B's step against A's hold.
#[derive(Clone, Copy, Debug)]
struct Row {
    hold: Hold,
    release: Release,
    /// The publication is poisoned while A holds the writer.
    poison: bool,
}

/// Which step B takes.
#[derive(Clone, Copy, Debug)]
enum StepKind {
    /// An R1 dispatch's own child issuance, waiting: its single entry, then
    /// the issuance through the entry's witness.
    R1Issuance,
    /// An R2 dispatch's single consuming entry, then one waiting run step of
    /// the entered operation.
    R2EnteredRun,
}

/// What B's step answered.
#[derive(Debug, Default)]
struct StepRecord {
    /// The step's first poll was pending: it met A's hold and waited.
    waited: bool,
    /// From the step's first poll to its answer.
    took: Duration,
    /// An issuance step's answer, with the grant owner's typed outcome.
    issued: Option<Result<Result<ChildGrantOutcome, ChildGrantRefusal>, ToolError>>,
    /// The same intent issued again on the synchronous path once A released,
    /// after an issuance that answered `Ok`.
    again: Option<Result<Result<ChildGrantOutcome, ChildGrantRefusal>, ToolError>>,
    /// A run step's answer.
    run: Option<Result<(), ToolError>>,
    /// How often the run step's operation ran.
    ran: usize,
    /// A second entry of the same R2 dispatch after its step.
    second_entry: Option<Result<(), ToolError>>,
}

/// The grant owner's typed outcome, which the work owner boxes.
fn outcome(boxed: Box<dyn std::any::Any + Send>) -> Result<ChildGrantOutcome, ChildGrantRefusal> {
    *boxed
        .downcast::<Result<ChildGrantOutcome, ChildGrantRefusal>>()
        .expect("the grant owner's typed outcome")
}

/// Take B's `step` while A holds the writer: poll it once (pending means it
/// met the hold and is waiting for it), release A as `release` says, and
/// finish the step. Returns whether it waited, how long it took, and its
/// answer.
async fn drive<F: std::future::Future>(
    step: F,
    holder: Holder,
    release: Release,
) -> (bool, Duration, F::Output) {
    let started = std::time::Instant::now();
    let mut step = std::pin::pin!(step);
    let answer = match futures::poll!(step.as_mut()) {
        std::task::Poll::Ready(answer) => {
            holder.release();
            return (false, started.elapsed(), answer);
        }
        std::task::Poll::Pending => match release {
            Release::AfterHold => {
                let ((), answer) = tokio::join!(
                    async move {
                        tokio::time::sleep(HOLD).await;
                        holder.release();
                    },
                    step
                );
                answer
            }
            Release::AfterTheStep => {
                let answer = step.await;
                holder.release();
                answer
            }
        },
    };
    (true, started.elapsed(), answer)
}

/// B's tool body: it takes the row's step on its own dispatch while A holds
/// the writer, and records the answer.
struct StepLeaf {
    grants: Arc<LocalGrantAuthority>,
    publication: LocalAuthorizationPublication,
    intent: ChildGrantIntent,
    next: Mutex<Option<(StepKind, Row)>>,
    record: Mutex<Option<StepRecord>>,
}

impl StepLeaf {
    /// A takes the writer now (and the publication is poisoned when the row
    /// says so), after B's dispatch reached its body: B's preparation and
    /// review never meet the hold, only its steps.
    fn hold(&self, row: Row) -> Holder {
        let holder = Holder::begin(&self.grants, &self.publication, row.hold);
        if row.poison {
            self.publication.poison();
        }
        holder
    }
}

#[async_trait]
impl AgentToolDispatcher for StepLeaf {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        RecordingTools::default().tools()
    }

    fn review_entry_support(&self, _tool_name: &str) -> ReviewEntrySupport {
        ReviewEntrySupport::ConsumesAtEntry
    }

    async fn dispatch(&self, _: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        panic!("B's leaf keeps its dispatch context")
    }

    async fn dispatch_with_context(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        let (kind, row) = self
            .next
            .lock()
            .expect("row")
            .take()
            .expect("a row for this dispatch");
        let further = FurtherOriginals {
            originals: Arc::from([]),
            source: Arc::new(ActiveOriginals),
        };
        let record = match kind {
            StepKind::R1Issuance => {
                let operation = context.governed_operation(call)?;
                let holder = self.hold(row);
                let (waited, took, answer) = drive(
                    operation
                        .issue_child_if_current_within_after_owner_change(&further, &self.intent),
                    holder,
                    row.release,
                )
                .await;
                let issued = answer.map(outcome);
                let again = issued.is_ok().then(|| {
                    operation
                        .issue_child_if_current_within(&further, &self.intent)
                        .map(outcome)
                });
                StepRecord {
                    waited,
                    took,
                    issued: Some(issued),
                    again,
                    ..StepRecord::default()
                }
            }
            StepKind::R2EnteredRun => {
                // The call's single consuming entry: the review is spent here,
                // once, before A holds anything.
                let entered = context.enter_governed_operation(call)?;
                let holder = self.hold(row);
                let mut ran = 0;
                let (waited, took, answer) = drive(
                    entered.run_if_current_within_after_owner_change(&further, || ran += 1),
                    holder,
                    row.release,
                )
                .await;
                StepRecord {
                    waited,
                    took,
                    run: Some(answer),
                    ran,
                    second_entry: Some(context.enter_governed_operation(call).map(drop)),
                    ..StepRecord::default()
                }
            }
        };
        *self.record.lock().expect("record") = Some(record);
        Ok(ToolResult::new(call.id.into(), "record-7 value".into(), false).into())
    }
}

/// B's call, as the binding retains it.
struct RecordCall {
    id: String,
    name: String,
    args: Box<serde_json::value::RawValue>,
}

impl RecordCall {
    fn new() -> Self {
        Self {
            id: STEP_CALL.into(),
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

/// One publication shared by the grant owner and B's work, with B's
/// governed dispatch context (the grant-backed work owner in its audit
/// wrapper, and a review composition with an allowing reviewer).
struct StepWorld {
    grants: Arc<LocalGrantAuthority>,
    tier: Arc<TieredRecords>,
    reviewer: Arc<CountingReviewer>,
    used: Arc<UsedReports>,
    leaf: Arc<StepLeaf>,
    context: ToolDispatchContext,
}

impl StepWorld {
    fn new() -> Self {
        let publication = LocalAuthorizationPublication::new();
        let grants = Arc::new(
            LocalGrantAuthority::new(
                LocalGrantConfiguration {
                    root: principal("grant-owner"),
                    namespace: id("contended-grants"),
                    generation: 1,
                },
                publication.clone(),
                Arc::new(HostAuthorizationClock),
            )
            .expect("actual generated grant owner"),
        );
        let issue_root = |name: &str, grantee: &str, restrictions| {
            grants
                .issue_root(
                    &principal("grant-owner"),
                    id(name),
                    principal(grantee),
                    None,
                    restrictions,
                )
                .expect("actual root grant")
        };
        let controller = issue_root("controller", "executor", ceiling("infer"));
        let operation = issue_root("read-only", "executor", ceiling("read"));
        let parent = issue_root(
            "helper-parent",
            "helper-operator",
            ExecutionRestrictions::unrestricted(),
        );
        let runtime = LogicalRuntimeId::for_session(&SessionId::new());
        let admitted = association(&runtime, controller, operation, selection());
        let tier = Arc::new(TieredRecords(AtomicU8::new(0)));
        let policy = Arc::new(GrantBackedWorkPolicy::new(
            Arc::clone(&grants),
            Arc::new(InvocationOwner),
            Arc::clone(&tier) as Arc<dyn OperationPolicyOwner>,
        ));
        let work = policy
            .audited_work_context(
                Arc::from([admitted]),
                OperationExecutionScope::RuntimeInput {
                    owner_session_id: SessionId::new(),
                    runtime_epoch_id: serde_json::from_str(
                        r#""00000000-0000-0000-0000-000000000002""#,
                    )
                    .expect("epoch id"),
                    submitted_input_id: serde_json::from_str(
                        r#""00000000-0000-0000-0000-000000000003""#,
                    )
                    .expect("input id"),
                    canonical_input_id: serde_json::from_str(
                        r#""00000000-0000-0000-0000-000000000003""#,
                    )
                    .expect("input id"),
                },
                Arc::new(AcceptedAudit),
            )
            .expect("the native grant-backed work context");
        let reviewer = Arc::new(CountingReviewer::default());
        let used = Arc::new(UsedReports::default());
        let review = BoundOperationReview::new(
            reviewer.clone(),
            meerkat_core::ApprovalService::new(),
            Duration::from_secs(30),
        )
        .with_observer(used.clone());
        let leaf = Arc::new(StepLeaf {
            grants: Arc::clone(&grants),
            publication,
            intent: ChildGrantIntent {
                issuer: principal("helper-operator"),
                parent_lineage: vec![parent],
                id: id(CHILD),
                grantee: principal("helper"),
                represented_subject: None,
                requested: ExecutionRestrictions::unrestricted(),
            },
            next: Mutex::new(None),
            record: Mutex::new(None),
        });
        let context = ToolDispatchContext::default()
            .with_work_authorization(Some(work))
            .with_operation_review(Some(Arc::new(review)));
        Self {
            grants,
            tier,
            reviewer,
            used,
            leaf,
            context,
        }
    }

    /// B dispatches its call with `tool_time` and takes `kind`'s step in its
    /// body while A holds the writer as `row` says.
    async fn dispatch(&self, kind: StepKind, row: Row, tool_time: Duration) -> StepRecord {
        self.tier.0.store(
            match kind {
                StepKind::R1Issuance => 0,
                StepKind::R2EnteredRun => 1,
            },
            Ordering::SeqCst,
        );
        *self.leaf.next.lock().expect("row") = Some((kind, row));
        let call = RecordCall::new();
        let plan = resolve_tool_execution_plan_fenced(
            &self.leaf,
            call.view(),
            &self.context,
            &meerkat_core::ToolExecutionResolutionContext::new(
                meerkat_core::ToolDeadlineChain::new(vec![
                    meerkat_core::ToolDeadlineContributor::finite(
                        meerkat_core::ToolDeadlineOwner::DirectCaller,
                        tool_time,
                    ),
                ])
                .expect("finite caller deadline"),
            ),
        )
        .expect("real plan resolution");
        let dispatched =
            dispatch_tool_execution_plan_fenced(&self.leaf, call.view(), &self.context, &plan)
                .await;
        self.leaf
            .record
            .lock()
            .expect("record")
            .take()
            .unwrap_or_else(|| panic!("B's body took its step: {dispatched:?}"))
    }

    /// The child the grant owner retains for B's intent, if any.
    fn child(&self) -> Option<GrantLineageRef> {
        self.grants.issued_reference(&id(CHILD)).expect("lookup")
    }
}

fn rebuild_refused<T>(answer: &Option<Result<T, ToolError>>) -> bool {
    matches!(
        answer,
        Some(Err(ToolError::AuthorizationRefused { refusal }))
            if refusal.kind() == OperationRefusalKind::ReprepareRequired
    )
}

/// GREEN for `hold`: A releases 50 ms after B started waiting. B's R1
/// issuance waits and then issues exactly one child; B's R2 entered step
/// waits and then runs its operation once, with the review spent once.
async fn both_steps_wait_out_the_hold(hold: Hold) {
    let world = StepWorld::new();
    let row = Row {
        hold,
        release: Release::AfterHold,
        poison: false,
    };

    let r1 = world.dispatch(StepKind::R1Issuance, row, TOOL_TIME).await;
    assert!(r1.waited, "{hold:?}: the issuance met A's hold and waited");
    assert!(r1.took >= HOLD, "{hold:?}: it waited for the hold: {r1:?}");
    let Some(Ok(Ok(ChildGrantOutcome::Issued { lineage }))) = &r1.issued else {
        panic!("{hold:?}: one child is issued once A released: {r1:?}")
    };
    assert_eq!(
        world.child().as_ref(),
        lineage.last(),
        "{hold:?}: the grant owner retains exactly the issued child"
    );
    assert!(
        matches!(&r1.again, Some(Ok(Ok(ChildGrantOutcome::Recovered { lineage: again })))
            if again == lineage),
        "{hold:?}: the same intent recovers that child, so one was appended: {r1:?}"
    );
    assert_eq!(
        world.reviewer.0.load(Ordering::SeqCst),
        0,
        "R1 is not reviewed"
    );

    let r2 = world.dispatch(StepKind::R2EnteredRun, row, TOOL_TIME).await;
    assert!(
        r2.waited,
        "{hold:?}: the entered step met A's hold and waited"
    );
    assert!(r2.took >= HOLD, "{hold:?}: it waited for the hold: {r2:?}");
    assert_eq!(r2.run, Some(Ok(())), "{hold:?}: the step runs: {r2:?}");
    assert_eq!(r2.ran, 1, "{hold:?}: its operation ran once");
    assert!(
        matches!(
            r2.second_entry,
            Some(Err(ToolError::ReviewUnsatisfied {
                kind: ReviewUnsatisfiedKind::AlreadyConsumed,
                ..
            }))
        ),
        "{hold:?}: the review was spent once, by the entry: {r2:?}"
    );
    assert_eq!(
        world.reviewer.0.load(Ordering::SeqCst),
        1,
        "{hold:?}: one review"
    );
    assert_eq!(
        world.used.0.load(Ordering::SeqCst),
        1,
        "{hold:?}: Used once, after the dispatch ended"
    );
}

/// GREEN, variant 1: A holds an exclusion that never publishes.
#[tokio::test]
async fn a_held_exclusion_is_waited_out_by_an_issuance_and_an_entered_step() {
    both_steps_wait_out_the_hold(Hold::Excluded).await;
}

/// GREEN, variant 2: A holds a published owner change. The R1 issuance
/// meets it at its single entry, whose staging waits, and the entered step
/// at its currentness check.
#[tokio::test]
async fn a_held_owner_change_is_waited_out_by_an_issuance_and_an_entered_step() {
    both_steps_wait_out_the_hold(Hold::Change).await;
}

/// Bound, both variants: A holds past B's remaining tool time. Each step
/// waits until that time ends, well before the 2 s owner-change bound, and
/// answers the typed refusal to rebuild: nothing is issued, and the operation
/// never ran.
#[tokio::test]
async fn past_the_remaining_tool_time_each_step_answers_the_typed_rebuild_refusal() {
    for hold in [Hold::Excluded, Hold::Change] {
        let world = StepWorld::new();
        let row = Row {
            hold,
            release: Release::AfterTheStep,
            poison: false,
        };

        let r1 = world
            .dispatch(StepKind::R1Issuance, row, SHORT_TOOL_TIME)
            .await;
        assert!(r1.waited, "{hold:?}: the issuance waited: {r1:?}");
        assert!(
            rebuild_refused(&r1.issued),
            "{hold:?}: the typed refusal to rebuild: {r1:?}"
        );
        assert!(
            r1.took < OWNER_CHANGE_WAIT_BOUND,
            "{hold:?}: the tool time ended the wait: {r1:?}"
        );
        assert_eq!(world.child(), None, "{hold:?}: nothing issued");

        let r2 = world
            .dispatch(StepKind::R2EnteredRun, row, SHORT_TOOL_TIME)
            .await;
        assert!(r2.waited, "{hold:?}: the entered step waited: {r2:?}");
        assert!(
            rebuild_refused(&r2.run),
            "{hold:?}: the typed refusal to rebuild: {r2:?}"
        );
        assert!(
            r2.took < OWNER_CHANGE_WAIT_BOUND,
            "{hold:?}: the tool time ended the wait: {r2:?}"
        );
        assert_eq!(r2.ran, 0, "{hold:?}: the operation never ran");
    }
}

/// Poison, both variants: the publication is poisoned while A holds the
/// writer. Each step answers `OperationAuthorizationUnavailable` at once,
/// with no wait, and nothing is issued or run.
#[tokio::test]
async fn a_poisoned_publication_is_unavailable_at_once_and_never_waited_for() {
    for hold in [Hold::Excluded, Hold::Change] {
        let row = Row {
            hold,
            release: Release::AfterTheStep,
            poison: true,
        };
        // The poison stands until the owners are reconstructed: each step
        // gets a world of its own.
        let world = StepWorld::new();
        let r1 = world.dispatch(StepKind::R1Issuance, row, TOOL_TIME).await;
        assert!(
            !r1.waited,
            "{hold:?}: the issuance answered at once: {r1:?}"
        );
        assert!(
            matches!(
                r1.issued,
                Some(Err(ToolError::OperationAuthorizationUnavailable))
            ),
            "{hold:?}: unavailable: {r1:?}"
        );
        assert_eq!(world.child(), None, "{hold:?}: nothing issued");

        let world = StepWorld::new();
        let r2 = world.dispatch(StepKind::R2EnteredRun, row, TOOL_TIME).await;
        assert!(
            !r2.waited,
            "{hold:?}: the entered step answered at once: {r2:?}"
        );
        assert_eq!(
            r2.run,
            Some(Err(ToolError::OperationAuthorizationUnavailable)),
            "{hold:?}: unavailable: {r2:?}"
        );
        assert_eq!(r2.ran, 0, "{hold:?}: the operation never ran");
    }
}
