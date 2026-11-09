//! Real Core consent custody attached to actual enrolled recovery owners.
//! The local probe's effect is retaining that custody, not authorizing Find or
//! retrospectively authorizing the already admitted Original/Helper input.
//! Native admission, canonical rows and recovery are the existing fixtures.
//! The separate rebind control selects handoff transitions directly; it does
//! not claim a native refusal or test the binder's authority to retry.
use super::*;
use crate::private_originals::custody_recovery::{CustodyHandoff, HandoffPhase};
use crate::review_custody::ReviewCustody;
use meerkat_core::{
    AgentToolDispatcher, ApprovalDecision, ApprovalId, ApprovalPrincipalId, ApprovalRisk,
    ApprovalService, PrincipalKind, ToolCallView, ToolDispatchContext, ToolError, TrustDomainId,
    approval::{
        consent::{
            AuthenticatedApprover, AuthenticatedConsentDecision, ConsentEligibility,
            ConsentIneligible, ConsentNotice, ConsentPresentation, ConsentPresentationCandidate,
            ConsentRouteUnavailable, HumanConsentRoute,
        },
        review::{
            BoundOperationReview, OperationReviewObserver, OperationReviewer, ReviewAttemptStatus,
            ReviewCandidate, ReviewEntrySupport, ReviewObservation, ReviewUnsatisfiedKind,
            ReviewVerdict, ReviewerFailure,
        },
    },
    authorization::{
        AuthorizationOperation, OperationAuthorizationError, OperationReviewTier,
        PreparedAuthorizationBinding, PreparedOperationAuthorization,
        ReviewOperationContextResolution, WorkAuthorization, WorkAuthorizationContext,
    },
    session_hosting::{HostingCapability, HostingOwner, HostingPaths, grant_session_hosting},
};
use std::sync::atomic::AtomicBool;
use toolkit_store::*;

const PROBE: &str = "recovery_review_hold_probe";
type OriginalFixture = super::super::Fixture;
type HelperFixture = FirstFixture<meerkat::PersistentSessionService<FactoryAgentBuilder>>;

struct ProbePolicy;
struct ProbeDecision(PreparedAuthorizationBinding);
impl WorkAuthorization for ProbePolicy {
    fn review_operation_context(
        &self,
        _: &PreparedAuthorizationBinding,
    ) -> Result<ReviewOperationContextResolution, OperationAuthorizationError> {
        // This trusted test policy governs only the local hold-transfer probe.
        Ok(ReviewOperationContextResolution::NotApplicable)
    }
    fn prepare(
        &self,
        binding: &PreparedAuthorizationBinding,
    ) -> Result<Arc<dyn PreparedOperationAuthorization>, OperationAuthorizationError> {
        let AuthorizationOperation::Tool(tool) = &binding.facts().operation else {
            return Err(OperationAuthorizationError::Unavailable);
        };
        if tool.name != PROBE || tool.arguments.get() != "{}" {
            return Err(OperationAuthorizationError::Unavailable);
        }
        Ok(Arc::new(ProbeDecision(binding.clone())))
    }
}
impl PreparedOperationAuthorization for ProbeDecision {
    fn spend_at_entry(&self) -> Result<(), OperationAuthorizationError> {
        Ok(())
    }
    fn review_tier(&self) -> OperationReviewTier {
        OperationReviewTier::R3
    }
    fn check_current(
        &self,
        binding: &PreparedAuthorizationBinding,
    ) -> Result<(), OperationAuthorizationError> {
        if !binding.same_operation(&self.0) {
            return Err(OperationAuthorizationError::Unavailable);
        }
        Ok(())
    }
}
struct NoReviewer;
#[async_trait::async_trait]
impl OperationReviewer for NoReviewer {
    async fn review(&self, _: &ReviewCandidate<'_>) -> Result<ReviewVerdict, ReviewerFailure> {
        panic!("the exact R3 probe must use real human consent, not model review")
    }
}
fn human() -> PrincipalRef {
    PrincipalRef::in_domain(
        PrincipalKind::Human,
        "local-review-fixture",
        TrustDomainId::new("toolkit-recovery-review").unwrap(),
    )
    .unwrap()
}
struct Route;
struct Eligible;
impl ConsentEligibility for Eligible {
    fn check_current(&self) -> Result<(), ConsentIneligible> {
        Ok(())
    }
}
impl HumanConsentRoute for Route {
    fn present(
        &self,
        _: &ConsentPresentationCandidate<'_>,
    ) -> Result<ConsentPresentation, ConsentRouteUnavailable> {
        Ok(ConsentPresentation {
            requester: ApprovalPrincipalId::new("recovery-probe").unwrap(),
            risk: ApprovalRisk::High,
            metadata: Default::default(),
        })
    }
    fn eligibility(
        &self,
        approver: &AuthenticatedApprover,
        _: &meerkat_core::ApprovalRecord,
    ) -> Result<Arc<dyn ConsentEligibility>, ConsentIneligible> {
        if approver.principal() != &human() {
            return Err(ConsentIneligible);
        }
        Ok(Arc::new(Eligible))
    }
    fn notice(&self, _: ConsentNotice) {}
}
#[derive(Default)]
struct Audit {
    handoff: Mutex<Weak<CustodyHandoff>>,
    used: Mutex<Vec<bool>>,
    notified: tokio::sync::Notify,
    check_task: AtomicBool,
    check_publication: AtomicBool,
}
impl OperationReviewObserver for Audit {
    fn observe(&self, observation: ReviewObservation) {
        if observation.status != ReviewAttemptStatus::Used {
            return;
        }
        let handoff = self.handoff.lock().unwrap().upgrade();
        let unlocked = handoff.is_some_and(|h| {
            h.review_release_locks_available_for_test(
                self.check_task.load(Ordering::SeqCst),
                self.check_publication.load(Ordering::SeqCst),
            )
        });
        self.used.lock().unwrap().push(unlocked);
        self.notified.notify_one();
    }
}
impl Audit {
    fn watch(&self, handoff: &Arc<CustodyHandoff>) {
        *self.handoff.lock().unwrap() = Arc::downgrade(handoff);
    }
    fn unused(&self) {
        assert!(
            self.used.lock().unwrap().is_empty(),
            "Used before custody release"
        );
    }
    async fn exactly_one_used(&self) {
        tokio::time::timeout(WAIT, self.notified.notified())
            .await
            .unwrap();
        assert_eq!(
            *self.used.lock().unwrap(),
            vec![true],
            "one native Used with every selected release lock probe successful"
        );
    }
}
type ReviewCustodyHook = Box<dyn FnOnce(ReviewCustody) + Send>;
struct Probe(Mutex<Option<ReviewCustodyHook>>);
#[async_trait::async_trait]
impl AgentToolDispatcher for Probe {
    fn tools(&self) -> Arc<[Arc<meerkat_core::ToolDef>]> {
        vec![Arc::new(
            meerkat_core::ToolDef::new(
                PROBE,
                "local recovery hold transfer",
                serde_json::json!({"type":"object"}),
            )
            .with_provenance(meerkat_core::ToolProvenance::new(
                meerkat_core::ToolSourceKind::Builtin,
                "toolkit-recovery-review",
            )),
        )]
        .into()
    }
    fn review_entry_support(&self, name: &str) -> ReviewEntrySupport {
        if name == PROBE {
            ReviewEntrySupport::ConsumesAtEntry
        } else {
            ReviewEntrySupport::Unsupported
        }
    }
    async fn dispatch(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<meerkat_core::ops::ToolDispatchOutcome, ToolError> {
        Err(ToolError::access_denied(call.name))
    }
    async fn dispatch_resolved_with_context(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
        plan: &meerkat_core::ResolvedToolExecutionPlan,
    ) -> Result<meerkat_core::ops::ToolDispatchOutcome, ToolError> {
        let (_, native) = context.enter_reviewed_effect_held(call, Some(plan))?;
        let transfer = self.0.lock().unwrap().take().expect("single actual entry");
        transfer(ReviewCustody::entered(native));
        Ok(meerkat_core::ToolResult::new(call.id.to_owned(), "held".into(), false).into())
    }
}
struct NativeReview {
    _root: tempfile::TempDir,
    _context: ToolDispatchContext,
    approvals: ApprovalService,
    id: ApprovalId,
    audit: Arc<Audit>,
}
impl NativeReview {
    async fn consume(
        audit: Arc<Audit>,
        effect: impl FnOnce(ReviewCustody) + Send + 'static,
    ) -> Self {
        let root = tempfile::tempdir().unwrap();
        let approvals = ApprovalService::new();
        let review = Arc::new(
            BoundOperationReview::new(Arc::new(NoReviewer), approvals.clone(), WAIT)
                .with_human_consent(Arc::new(Route), Duration::from_secs(3600))
                .with_observer(audit.clone()),
        );
        let claim = grant_session_hosting(
            &HostingCapability::OsLock(Arc::new(HostingPaths {
                hosting_lock_dir: root.path().join("hosting"),
                cold_delivery_lock: root.path().join("delivery.lock"),
                database: None,
            })),
            &HostingOwner::mint(),
            &SessionId::new(),
        )
        .unwrap();
        let context = ToolDispatchContext::default()
            .with_work_authorization(Some(
                WorkAuthorizationContext::new(
                    Arc::new(ProbePolicy),
                    meerkat_core::OperationExecutionScope::Domain,
                )
                .with_consent_hosting(claim),
            ))
            .with_operation_review(Some(review));
        let probe = Arc::new(Probe(Mutex::new(Some(Box::new(effect)))));
        let args = serde_json::value::RawValue::from_string("{}".into()).unwrap();
        let call = ToolCallView {
            id: "recovery-held-review",
            name: PROBE,
            args: &args,
        };
        let resolution = meerkat_core::ToolExecutionResolutionContext::new(
            meerkat_core::ToolDeadlineChain::new(vec![
                meerkat_core::ToolDeadlineContributor::finite(
                    meerkat_core::ToolDeadlineOwner::CoreToolDispatch,
                    WAIT,
                ),
            ])
            .unwrap(),
        );
        let plan =
            meerkat_core::resolve_tool_execution_plan_fenced(&probe, call, &context, &resolution)
                .unwrap();
        let id =
            match meerkat_core::dispatch_tool_execution_plan_fenced(&probe, call, &context, &plan)
                .await
            {
                Err(ToolError::ReviewUnsatisfied {
                    kind: ReviewUnsatisfiedKind::HumanConsentRequired,
                    approval_id: Some(id),
                }) => id,
                other => panic!("actual R3 refusal required: {other:?}"),
            };
        assert!(
            probe.0.lock().unwrap().is_some(),
            "no effect before consent"
        );
        approvals
            .decide_consent(
                &id,
                AuthenticatedConsentDecision {
                    approver: AuthenticatedApprover::new(
                        human(),
                        meerkat_core::ApprovalAssurance::PersonAuthenticated,
                    )
                    .unwrap(),
                    decision: ApprovalDecision::Approve,
                    reason: None,
                },
            )
            .unwrap();
        meerkat_core::dispatch_retained_consent_attempt(&probe, &context, &id)
            .await
            .unwrap();
        assert!(probe.0.lock().unwrap().is_none());
        let value = Self {
            _root: root,
            _context: context,
            approvals,
            id,
            audit,
        };
        value.consumed();
        value
    }
    fn consumed(&self) {
        assert!(
            self.approvals
                .get(&self.id)
                .unwrap()
                .consent
                .unwrap()
                .consumed_at
                .is_some()
        );
    }
    async fn attach(handoff: &Arc<CustodyHandoff>) -> Self {
        let audit = Arc::new(Audit::default());
        audit.watch(handoff);
        let handoff = handoff.clone();
        Self::consume(audit, move |review| handoff.retain_review_for_test(review)).await
    }
}

#[derive(Clone, Copy)]
enum RecoveryFault {
    HeldTerminalReply,
    PanicRead,
}
struct RecoveryStore {
    inner: Arc<dyn ToolkitStore>,
    mode: RecoveryFault,
    armed: AtomicBool,
    entered: tokio::sync::Notify,
    release: tokio::sync::Semaphore,
    completed: AtomicBool,
}
impl RecoveryStore {
    fn new(inner: Arc<dyn ToolkitStore>, mode: RecoveryFault) -> Self {
        Self {
            inner,
            mode,
            armed: AtomicBool::new(false),
            entered: tokio::sync::Notify::new(),
            release: tokio::sync::Semaphore::new(0),
            completed: AtomicBool::new(false),
        }
    }
}

#[async_trait::async_trait]
impl ToolkitStore for RecoveryStore {
    fn capabilities(&self) -> StoreCapabilities {
        self.inner.capabilities()
    }
    async fn now_ms(&self) -> StoreResult<i64> {
        self.inner.now_ms().await
    }
    async fn admit(&self, request: AdmissionRequest) -> StoreResult<AdmissionOutcome> {
        self.inner.admit(request).await
    }
    async fn get(&self, key: &StoreKey) -> StoreResult<Option<StoredRecord>> {
        if matches!(self.mode, RecoveryFault::PanicRead) && self.armed.swap(false, Ordering::SeqCst)
        {
            self.entered.notify_one();
            self.release.acquire().await.unwrap().forget();
            self.completed.store(true, Ordering::SeqCst);
            panic!("injected actual recovery read panic before any terminal write");
        }
        self.inner.get(key).await
    }
    async fn list_records(
        &self,
        scope: &StoreScope,
        after: Option<&str>,
        limit: u32,
    ) -> StoreResult<Vec<StoredRecord>> {
        self.inner.list_records(scope, after, limit).await
    }
    async fn compare_and_swap(&self, transition: Transition) -> StoreResult<StoredRecord> {
        let terminal = transition.data.body["custody_attempt"]["decision"]
            .as_str()
            .is_some_and(|value| value == "promoted" || value == "withdrawn");
        if matches!(self.mode, RecoveryFault::HeldTerminalReply)
            && terminal
            && self.armed.swap(false, Ordering::SeqCst)
        {
            let committed = self.inner.compare_and_swap(transition).await?;
            self.entered.notify_one();
            self.release.acquire().await.unwrap().forget();
            self.completed.store(true, Ordering::SeqCst);
            return Ok(committed);
        }
        self.inner.compare_and_swap(transition).await
    }
    async fn claim_record(
        &self,
        key: &StoreKey,
        revision: u64,
        worker: &str,
        ttl: u64,
    ) -> StoreResult<RecordLease> {
        self.inner.claim_record(key, revision, worker, ttl).await
    }
    async fn renew_record_claim(&self, lease: &RecordLease, ttl: u64) -> StoreResult<RecordLease> {
        self.inner.renew_record_claim(lease, ttl).await
    }
    async fn recover_record_claim(&self, prior: &RecordLease) -> StoreResult<RecordLease> {
        self.inner.recover_record_claim(prior).await
    }
    async fn release_record_claim(&self, lease: &RecordLease) -> StoreResult<()> {
        self.inner.release_record_claim(lease).await
    }
    async fn read_events(
        &self,
        key: &StoreKey,
        after: u64,
        limit: u32,
    ) -> StoreResult<Vec<StoredEvent>> {
        self.inner.read_events(key, after, limit).await
    }
    async fn get_effect(&self, key: &EffectKey) -> StoreResult<Option<StoredEffect>> {
        self.inner.get_effect(key).await
    }
    async fn list_effects(
        &self,
        key: &StoreKey,
        after: Option<&str>,
        limit: u32,
    ) -> StoreResult<Vec<StoredEffect>> {
        self.inner.list_effects(key, after, limit).await
    }
    async fn claim_effect(
        &self,
        key: &EffectKey,
        revision: u64,
        worker: &str,
        ttl: u64,
    ) -> StoreResult<EffectClaimOutcome> {
        self.inner.claim_effect(key, revision, worker, ttl).await
    }
    async fn acknowledge_effect(
        &self,
        lease: &EffectLease,
        outcome: EffectOutcome,
    ) -> StoreResult<StoredEffect> {
        self.inner.acknowledge_effect(lease, outcome).await
    }
    async fn claim_reconciliation(
        &self,
        key: &EffectKey,
        worker: &str,
        ttl: u64,
    ) -> StoreResult<StoredEffect> {
        self.inner.claim_reconciliation(key, worker, ttl).await
    }
    async fn reconcile_effect(
        &self,
        lease: &EffectLease,
        resolution: Reconciliation,
    ) -> StoreResult<StoredEffect> {
        self.inner.reconcile_effect(lease, resolution).await
    }
}

async fn original_recovery(mode: RecoveryFault) {
    let selected = Arc::new(OnceLock::<Arc<RecoveryStore>>::new());
    let capture = selected.clone();
    let delivery = match mode {
        RecoveryFault::HeldTerminalReply => DeliveryMode::Normal,
        RecoveryFault::PanicRead => DeliveryMode::LoseAcceptedAnswer,
    };
    let fixture = OriginalFixture::with_source_store(
        delivery,
        false,
        Some(Box::new(move |inner, _| {
            let store = Arc::new(RecoveryStore::new(inner, mode));
            assert!(capture.set(store.clone()).is_ok());
            store
        })),
    )
    .await;
    let store = selected.get().unwrap().clone();
    let mut first_round = Some(fixture.source.hold_first_custody_round_for_test());
    let mut shutdown = Box::pin(fixture.shutdown());
    let mut native_cleanup = None;
    let mut review = None;
    let checked =
        std::panic::AssertUnwindSafe(tokio::time::timeout(WAIT * 4, async {
            fixture
                .source
                .start_custody_recovery(
                    fixture.native.mob_runtime().continuation_source_announcer(),
                )
                .unwrap();
            let result = fixture.submit().await;
            let held = fixture
                .source
                .retained
                .lock()
                .unwrap()
                .iter()
                .find(|held| held.reference == fixture.submission.original)
                .cloned()
                .unwrap();
            let pending = held.first_custody.lock().unwrap().clone().unwrap();
            assert!(pending.handoff.enrolled());
            match mode {
                RecoveryFault::HeldTerminalReply => {
                    assert!(
                        matches!(
                            result,
                            Ok(PrivateOriginalSubmission::Admitted {
                                deduplicated: false,
                                ..
                            })
                        ),
                        "{}",
                        submission_context(&result)
                    );
                    assert!(pending.handoff.phase() == HandoffPhase::Accepted);
                }
                RecoveryFault::PanicRead => {
                    assert!(
                        matches!(result, Ok(PrivateOriginalSubmission::AdmissionUnresolved)),
                        "{}",
                        submission_context(&result)
                    );
                    assert!(pending.handoff.phase() == HandoffPhase::Released);
                    assert!(
                        !held.unresolved.lock().unwrap().is_empty(),
                        "actual unresolved Original retained"
                    );
                    assert!(
                        pending.handoff.rebind(|| Ok(())).is_err(),
                        "loss of the actual native reply does not permit rebind"
                    );
                }
            }
            let key = fixture.source.key(&fixture.submission.original).unwrap();
            let before = store.inner.get(&key).await.unwrap().unwrap();
            let native = fixture
                .machine
                .input_state(&pending.session, pending.input.id())
                .await
                .unwrap()
                .unwrap();
            meerkat_runtime::input_state::verify_exact_replay(&native.state, &pending.input)
                .unwrap();
            assert_eq!(fixture.fault.accepted.load(Ordering::SeqCst), 1);
            review = Some(NativeReview::attach(&pending.handoff).await);
            let reviewed = review.as_ref().unwrap();
            reviewed
                .audit
                .check_task
                .store(matches!(mode, RecoveryFault::PanicRead), Ordering::SeqCst);
            reviewed.audit.check_publication.store(
                matches!(mode, RecoveryFault::HeldTerminalReply),
                Ordering::SeqCst,
            );
            reviewed.audit.unused();
            fixture.source.stop_custody_recovery();
            native_cleanup = Some(shutdown.as_mut().await);
            native_cleanup.as_ref().unwrap().as_ref().unwrap();
            store.armed.store(true, Ordering::SeqCst);
            first_round.take().unwrap().send(()).unwrap();
            store.entered.notified().await;
            assert!(!store.completed.load(Ordering::SeqCst));
            reviewed.audit.unused();
            match mode {
                RecoveryFault::HeldTerminalReply => {
                    let committed = store.inner.get(&key).await.unwrap().unwrap();
                    assert_eq!(
                        committed.data.body["custody_attempt"]["decision"],
                        "promoted"
                    );
                    // Durable terminality alone does not end an entered source call.
                    reviewed.audit.unused();
                    let mut join = Box::pin(fixture.source.join_custody_recovery());
                    assert!(matches!(
                        futures::poll!(join.as_mut()),
                        std::task::Poll::Pending
                    ));
                    drop(join);
                    reviewed.audit.unused();
                }
                RecoveryFault::PanicRead => {
                    assert_eq!(store.inner.get(&key).await.unwrap().unwrap(), before)
                }
            }
            store.release.add_permits(1);
            let joined = fixture.source.join_custody_recovery().await;
            assert!(store.completed.load(Ordering::SeqCst));
            match mode {
                RecoveryFault::HeldTerminalReply => {
                    joined.unwrap();
                    assert!(pending.handoff.phase() == HandoffPhase::Done);
                    assert_eq!(
                        store.inner.get(&key).await.unwrap().unwrap().data.body["custody_attempt"]
                            ["decision"],
                        "promoted"
                    );
                }
                RecoveryFault::PanicRead => {
                    assert!(matches!(joined, Err(PrivateOriginalError::Unavailable)));
                    assert!(
                        pending.handoff.phase() == HandoffPhase::Released,
                        "joining a panic releases review, never fabricates Done"
                    );
                    assert!(
                        !pending.handoff.closed_failure(),
                        "panic keeps its distinct task failure"
                    );
                    assert_eq!(store.inner.get(&key).await.unwrap().unwrap(), before);
                    // TaskExit calls stop during unwind. Its actual native
                    // publication guard poisons the publication when dropped;
                    // unavailable publication is distinct from a held lock.
                    assert!(matches!(
                        fixture.owner.publication().observe(|| ()),
                        Err(meerkat_authorization::publication::PublicationError::Unavailable)
                    ));
                }
            }
            reviewed.audit.exactly_one_used().await;
            reviewed.consumed();
            assert!(
                Arc::ptr_eq(
                    &held.first_custody.lock().unwrap().clone().unwrap(),
                    &pending
                ),
                "retained evidence outlives its removable review hold"
            );
            let again = fixture.source.join_custody_recovery().await;
            assert_eq!(
                again.is_ok(),
                matches!(mode, RecoveryFault::HeldTerminalReply)
            );
            assert_eq!(*reviewed.audit.used.lock().unwrap(), vec![true]);
            assert!(fixture.source.start_custody_recovery(None).is_err());
        }))
        .catch_unwind()
        .await;
    // These owners survive timeout/panic; release every fixture gate before
    // awaiting the original native shutdown and exact source JoinHandle.
    fixture.source.stop_custody_recovery();
    drop(first_round.take());
    store.release.add_permits(1);
    fixture.fault.release.add_permits(1);
    let native_cleanup = match native_cleanup {
        Some(done) => done,
        None => shutdown.as_mut().await,
    };
    let source_cleanup = fixture.source.join_custody_recovery().await;
    match checked {
        Ok(Ok(())) => {
            assert!(native_cleanup.is_ok(), "{native_cleanup:?}");
            assert_eq!(
                source_cleanup.is_ok(),
                matches!(mode, RecoveryFault::HeldTerminalReply)
            );
        }
        Ok(Err(_)) => panic!(
            "review recovery timed out; native={native_cleanup:?}; source={source_cleanup:?}"
        ),
        Err(payload) => {
            eprintln!(
                "review recovery cleanup: native={native_cleanup:?}; source={source_cleanup:?}"
            );
            std::panic::resume_unwind(payload);
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn original_review_used_waits_for_actual_terminal_reply_and_joined_source() {
    original_recovery(RecoveryFault::HeldTerminalReply).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unresolved_original_review_releases_after_actual_recovery_task_panic_is_joined() {
    original_recovery(RecoveryFault::PanicRead).await;
}

async fn helper_recovery(panicked: bool) {
    let script = acquisition_denial::Script::paused_for_helper_entry();
    let fault = Arc::new(DeliveryFault::new(
        DeliveryMode::PoisonPublicationAfterSecondAcceptance,
    ));
    let selected = Arc::new(OnceLock::<Arc<RecoveryStore>>::new());
    let capture = selected.clone();
    let (fixture, history, _retained) = HelperFixture::for_durable_helper_recovery(
        script.clone(),
        Some(fault.clone()),
        None,
        Some(Box::new(move |inner| {
            let store = Arc::new(RecoveryStore::new(inner, RecoveryFault::PanicRead));
            assert!(capture.set(store.clone()).is_ok());
            store
        })),
    )
    .await;
    let store = selected.get().unwrap().clone();
    let mut first_round = Some(fixture.source.hold_first_custody_round_for_test());
    let mut shutdown = Box::pin(fixture.shutdown());
    let mut native_cleanup = None;
    let mut review = None;
    let checked = std::panic::AssertUnwindSafe(tokio::time::timeout(WAIT * 4, async {
        fixture
            .source
            .start_custody_recovery(fixture.native.mob_runtime().continuation_source_announcer())
            .unwrap();
        acquisition_approved::select_reviewed_setup(&fixture).await;
        let first = fixture.submit().await;
        assert!(
            matches!(
                first,
                Ok(PrivateOriginalSubmission::Admitted {
                    deduplicated: false,
                    ..
                })
            ),
            "{}",
            submission_context(&first)
        );
        script.first_response_waiting().await;
        script.release_first_response();
        let (held, pending) = loop {
            let held = fixture
                .source
                .helper_sources
                .lock()
                .unwrap()
                .first()
                .cloned();
            if let Some(held) = held
                && !held.handoffs_for_test().is_empty()
            {
                let pending = held.first_custody.lock().unwrap().clone().unwrap();
                break (held, pending);
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        };
        assert!(pending.handoff.enrolled() && pending.handoff.phase() == HandoffPhase::Accepted);
        assert!(
            pending.handoff.rebind(|| Ok(())).is_err(),
            "accepted helper cannot be rebound"
        );
        let original_publication = {
            let state = pending.publication.lock().unwrap();
            let SourcePublication::Settled {
                source_acknowledged,
                result,
            } = &state.source
            else {
                panic!("actual failed publication");
            };
            assert!(!source_acknowledged && result.is_err());
            (*source_acknowledged, result.clone())
        };
        let key = held.source_record().key.clone();
        let before = fixture.store.get(&key).await.unwrap().unwrap();
        assert_eq!(before.data.body["custody_attempt"]["decision"], "pending");
        let rows = acquisition_approved::native_history(history.as_ref(), &pending.session).await;
        assert_eq!(rows.len(), 1);
        meerkat_runtime::input_state::verify_exact_replay(&rows[0].state, &pending.input).unwrap();
        assert_eq!(fault.accepted.load(Ordering::SeqCst), 2);
        review = Some(NativeReview::attach(&pending.handoff).await);
        let reviewed = review.as_ref().unwrap();
        reviewed.audit.check_task.store(panicked, Ordering::SeqCst);
        reviewed.audit.unused();
        fixture.source.stop_custody_recovery();
        native_cleanup = Some(shutdown.as_mut().await);
        native_cleanup.as_ref().unwrap().as_ref().unwrap();
        reviewed.audit.unused();
        store.armed.store(panicked, Ordering::SeqCst);
        first_round.take().unwrap().send(()).unwrap();
        if panicked {
            store.entered.notified().await;
            reviewed.audit.unused();
            store.release.add_permits(1);
        }
        assert!(matches!(
            fixture.source.join_custody_recovery().await,
            Err(PrivateOriginalError::Unavailable)
        ));
        assert!(pending.handoff.phase() == HandoffPhase::Accepted);
        assert_eq!(
            pending.handoff.closed_failure(),
            !panicked,
            "actual task panic and completed failed entry retain distinct evidence"
        );
        assert_eq!(store.completed.load(Ordering::SeqCst), panicked);
        reviewed.audit.exactly_one_used().await;
        assert_eq!(fixture.store.get(&key).await.unwrap().unwrap(), before);
        {
            let state = pending.publication.lock().unwrap();
            let SourcePublication::Settled {
                source_acknowledged,
                result,
            } = &state.source
            else {
                panic!("retained publication");
            };
            assert_eq!((*source_acknowledged, result.clone()), original_publication);
        }
        let writes = pending.handoff.write_attempts.load(Ordering::SeqCst);
        if panicked {
            assert_eq!(
                writes, 0,
                "panic occurred before the helper recovery writer"
            );
        } else {
            assert!(writes > 0);
        }
        assert!(matches!(
            fixture.source.join_custody_recovery().await,
            Err(PrivateOriginalError::Unavailable)
        ));
        assert_eq!(
            pending.handoff.write_attempts.load(Ordering::SeqCst),
            writes
        );
        assert_eq!(*reviewed.audit.used.lock().unwrap(), vec![true]);
        reviewed.consumed();
    }))
    .catch_unwind()
    .await;
    fixture.source.stop_custody_recovery();
    drop(first_round.take());
    store.release.add_permits(1);
    script.release_first_response();
    fault.release.add_permits(1);
    let native_cleanup = match native_cleanup {
        Some(done) => done,
        None => shutdown.as_mut().await,
    };
    let source_cleanup = fixture.source.join_custody_recovery().await;
    match checked {
        Ok(Ok(())) => {
            assert!(native_cleanup.is_ok(), "{native_cleanup:?}");
            assert!(matches!(
                source_cleanup,
                Err(PrivateOriginalError::Unavailable)
            ));
        }
        Ok(Err(_)) => {
            panic!("helper review timed out; native={native_cleanup:?}; source={source_cleanup:?}")
        }
        Err(payload) => {
            eprintln!(
                "helper review cleanup: native={native_cleanup:?}; source={source_cleanup:?}"
            );
            std::panic::resume_unwind(payload);
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn helper_review_releases_after_stopped_failed_recovery_without_erasing_evidence() {
    helper_recovery(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retained_helper_review_releases_only_after_actual_panicked_source_task_is_joined() {
    helper_recovery(true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn same_handoff_review_moves_across_rebind_without_a_second_native_spend() {
    let fixture = OriginalFixture::new(DeliveryMode::Normal, false).await;
    let mut shutdown = Box::pin(fixture.shutdown());
    let mut review = None;
    let checked = std::panic::AssertUnwindSafe(tokio::time::timeout(WAIT, async {
        let slot = Arc::new(OnceLock::<Arc<CustodyHandoff>>::new());
        let selected = slot.clone();
        let source = fixture.source.clone();
        let audit = Arc::new(Audit::default());
        let observed = audit.clone();
        review = Some(
            NativeReview::consume(audit, move |review| {
                let handoff = CustodyHandoff::new(true, &source, &review);
                observed.watch(&handoff);
                assert!(selected.set(handoff).is_ok());
            })
            .await,
        );
        let mut first = slot.get().unwrap().clone();
        drop(slot);
        let reviewed = review.as_ref().unwrap();
        reviewed.audit.unused();
        assert!(
            first.rebind(|| Ok(())).is_err(),
            "no retry without exact-refusal prerequisite"
        );
        first.release();
        first.native_box_finished();
        // A state-machine control only. Real native refusal classification and
        // source rebind guards remain separately owned by the binder tests.
        first.record_definite_refusal();
        assert!(
            first
                .rebind::<()>(|| Err(PrivateOriginalError::Unavailable))
                .is_err()
        );
        reviewed.audit.unused();
        let (_, next) = first.rebind(|| Ok(())).unwrap();
        assert!(first.phase() == HandoffPhase::Done && next.phase() == HandoffPhase::Foreground);
        reviewed.audit.watch(&next);
        first.foreground_finished();
        // The old Done is transfer, not consent completion. Retain the new
        // handoff while dropping every reference to the old state.
        first = next.clone();
        reviewed.audit.unused();
        drop(next);
        // Keep one independent I/O hold to observe release outside destruction
        // of the final handoff itself, using the real native guard's clone.
        let io = first.review_for_test();
        drop(first);
        reviewed.audit.unused();
        drop(io);
        // The last handoff is now gone, so the observer cannot probe its locks.
        tokio::time::timeout(WAIT, reviewed.audit.notified.notified())
            .await
            .unwrap();
        assert_eq!(*reviewed.audit.used.lock().unwrap(), vec![false]);
        reviewed.consumed();
    }))
    .catch_unwind()
    .await;
    fixture.source.stop_custody_recovery();
    let native = shutdown.as_mut().await;
    let source = fixture.source.join_custody_recovery().await;
    assert!(
        native.is_ok() && source.is_ok(),
        "native={native:?}; source={source:?}"
    );
    match checked {
        Ok(Ok(())) => {}
        Ok(Err(_)) => panic!("handoff rebind control timed out"),
        Err(payload) => std::panic::resume_unwind(payload),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn non_enrolled_handoff_does_not_retain_a_recovery_review_hold() {
    let fixture = OriginalFixture::new(DeliveryMode::Normal, false).await;
    let mut shutdown = Box::pin(fixture.shutdown());
    let checked = std::panic::AssertUnwindSafe(tokio::time::timeout(WAIT, async {
        let slot = Arc::new(OnceLock::<Arc<CustodyHandoff>>::new());
        let selected = slot.clone();
        let source = fixture.source.clone();
        let audit = Arc::new(Audit::default());
        let observed = audit.clone();
        let review = NativeReview::consume(audit, move |review| {
            let handoff = CustodyHandoff::new(false, &source, &review);
            observed.watch(&handoff);
            assert!(selected.set(handoff).is_ok());
        })
        .await;
        assert!(!slot.get().unwrap().enrolled());
        review.audit.exactly_one_used().await;
        review.consumed();
        assert!(
            slot.get().unwrap().phase() == HandoffPhase::Foreground,
            "non-enrolled ownership is not completed by an invented Done transition"
        );
    }))
    .catch_unwind()
    .await;
    fixture.source.stop_custody_recovery();
    let native = shutdown.as_mut().await;
    let source = fixture.source.join_custody_recovery().await;
    assert!(
        native.is_ok() && source.is_ok(),
        "native={native:?}; source={source:?}"
    );
    match checked {
        Ok(Ok(())) => {}
        Ok(Err(_)) => panic!("non-enrolled handoff control timed out"),
        Err(payload) => std::panic::resume_unwind(payload),
    }
}
