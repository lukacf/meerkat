#![allow(clippy::expect_used, clippy::panic)]
//! Consent notices through the library delivery owner: the issuing approval
//! owner's owed notices for one hosting domain are projected on the owner's
//! rounds and admitted, as retained resumes the owner confirms, to the exact
//! committed owner scope. Approval rows are written to the store as the
//! approval owner commits them. Parked consent comes from the approval
//! owner's own paths, and an ended notice input is a real input of the
//! host's runtime that the runtime cancels.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use meerkat::{
    ConsentNoticeProjector, ContinuationAdmitError, ContinuationDeliverySink,
    ContinuationHostBindings, ContinuationKey, ContinuationOwner, ContinuationOwnerService,
    ContinuationStatus, JobDeliveryApplication, JobDeliveryApplyError, JobDeliverySink,
    MemoryDetachedJobStore, OwedContinuationCursor, OwedContinuationProjection, RetainedJobLookup,
    RetainedJobSource, RuntimeDeliveryHost, RuntimeDeliveryOwner, RuntimeDeliveryPass,
    SessionAddressResolver, SessionId, consent_notice_producer,
};
use meerkat_core::approval::ApprovalConsentWithdrawalReason;
use meerkat_core::approval::consent::{
    ConsentDeliveryReceipt, ConsentNoticeKind, consent_delivery_job_id,
};
use meerkat_core::approval::consent_test_support::ParkedRoutedConsent;
use meerkat_core::lifecycle::{InputId, RunId};
use meerkat_core::retained_work::RetainedWorkIdentity;
use meerkat_core::session_hosting::{HostingDomainRef, HostingPaths};
use meerkat_core::{
    ApprovalActionKind, ApprovalAssurance, ApprovalConsentApprover, ApprovalConsentDelivery,
    ApprovalConsentDeliveryState, ApprovalConsentOrigin, ApprovalConsentRecord,
    ApprovalConsentScope, ApprovalConsentTransition, ApprovalDecision, ApprovalError, ApprovalId,
    ApprovalOwnerRef, ApprovalPrincipalId, ApprovalProposedAction, ApprovalRecord, ApprovalRequest,
    ApprovalResourceId, ApprovalResourceKind, ApprovalResourceRef, ApprovalRisk, ApprovalService,
    ApprovalStore, ApprovalStoreError, InMemoryApprovalStore, PrincipalKind, PrincipalRef,
    SurfaceMetadata, TrustDomainId,
};
use meerkat_runtime::input_state::{InputAbandonReason, InputTerminalOutcome};
use meerkat_runtime::terminal_status::InteractionSelector;
use meerkat_runtime::{
    AcceptOutcome, InMemoryRuntimeStore, MeerkatMachine, RuntimeDeliveryInbox,
    SessionServiceRuntimeExt as _,
};
use tokio::sync::watch;

/// Hang guard for awaited typed events; never a pacing mechanism.
const EVENT_GUARD: Duration = Duration::from_secs(10);

fn domain(lock_dir: &str) -> HostingDomainRef {
    HostingPaths {
        hosting_lock_dir: lock_dir.into(),
        cold_delivery_lock: format!("{lock_dir}/cold-delivery.lock").into(),
        database: None,
    }
    .domain_ref()
}

fn origin_work(runtime_id: &str) -> RetainedWorkIdentity {
    RetainedWorkIdentity::new(runtime_id, RunId::new(), Vec::new(), BTreeMap::new(), None)
}

fn request() -> ApprovalRequest {
    ApprovalRequest {
        requester: ApprovalPrincipalId::new("agent:fork-child").expect("principal"),
        owner: ApprovalOwnerRef::Session {
            session_id: SessionId::new(),
        },
        resource: ApprovalResourceRef {
            kind: ApprovalResourceKind::ShellCommand,
            id: ApprovalResourceId::new("shell:deploy"),
        },
        proposed_action: ApprovalProposedAction {
            kind: ApprovalActionKind::ShellCommand,
            summary: "deploy the build".to_string(),
            body: None,
        },
        risk: ApprovalRisk::High,
        request_body: None,
        allowed_decisions: BTreeSet::from([ApprovalDecision::Approve, ApprovalDecision::Deny]),
        expires_at: None,
        metadata: SurfaceMetadata::default(),
        request_provenance: None,
    }
}

/// An approved consent row as the issuing owner commits it: decided by an
/// authenticated human, scoped to `session` in `domain`, with the admitting
/// run's retained work, owing the Approved notice. Stored under `id` when
/// given.
fn approved_owing_row(
    id: Option<ApprovalId>,
    domain: &HostingDomainRef,
    session: &SessionId,
    origin: &RetainedWorkIdentity,
) -> ApprovalRecord {
    let scratch = ApprovalService::new();
    let pending = scratch.request(request()).expect("request");
    let mut row = scratch
        .decide(
            &pending.approval_id,
            ApprovalDecision::Approve,
            ApprovalPrincipalId::new("human:lead-1").expect("principal"),
            None,
            None,
        )
        .expect("approved");
    if let Some(id) = id {
        row.approval_id = id;
    }
    row.consent = Some(ApprovalConsentRecord {
        scope: Some(ApprovalConsentScope {
            domain: domain.clone(),
            session_id: session.clone(),
        }),
        approver: Some(ApprovalConsentApprover {
            principal: PrincipalRef::in_domain(
                PrincipalKind::Human,
                "human:lead-1",
                TrustDomainId::new("team").expect("trust domain"),
            )
            .expect("approver"),
            assurance: ApprovalAssurance::PersonAuthenticated,
        }),
        origin: Some(ApprovalConsentOrigin {
            retained_work: origin.clone(),
        }),
        deliveries: vec![ApprovalConsentDelivery {
            transition: ApprovalConsentTransition::Approved,
            committed_at: row.updated_at,
            state: ApprovalConsentDeliveryState::Owed,
        }],
        ..ApprovalConsentRecord::default()
    });
    row
}

/// The approval store, which can lose the acknowledgement of a write that
/// committed and fail its reads.
#[derive(Default)]
struct LossyStore {
    inner: InMemoryApprovalStore,
    lose_next_ack: AtomicBool,
    unreadable: AtomicBool,
}

impl ApprovalStore for LossyStore {
    fn load_all(&self) -> Result<Vec<ApprovalRecord>, ApprovalStoreError> {
        self.inner.load_all()
    }

    fn get(&self, approval_id: &ApprovalId) -> Result<Option<ApprovalRecord>, ApprovalStoreError> {
        if self.unreadable.load(Ordering::SeqCst) {
            return Err(ApprovalStoreError::NotCommitted(
                "the approval store cannot be read".to_string(),
            ));
        }
        self.inner.get(approval_id)
    }

    fn put_if_current(
        &self,
        expected: Option<&ApprovalRecord>,
        record: &ApprovalRecord,
    ) -> Result<(), ApprovalStoreError> {
        self.inner.put_if_current(expected, record)?;
        if self.lose_next_ack.swap(false, Ordering::SeqCst) {
            return Err(ApprovalStoreError::CommitUnknown(
                "the acknowledgement was lost".to_string(),
            ));
        }
        Ok(())
    }

    fn is_persistent(&self) -> bool {
        false
    }
}

impl LossyStore {
    /// The owner commits a row whose acknowledgement it never receives;
    /// the row then holds `committed` (written by the owner, never
    /// announced). Returns the row's id.
    fn commit_unacknowledged(
        &self,
        approvals: &ApprovalService,
        committed: impl FnOnce(ApprovalId) -> ApprovalRecord,
    ) -> ApprovalId {
        self.lose_next_ack.store(true, Ordering::SeqCst);
        let approval_id = match approvals.request(request()) {
            Err(ApprovalError::StoreCommitUnknown { approval_id }) => approval_id,
            other => panic!("the request's acknowledgement was lost: {other:?}"),
        };
        let written = self.inner.get(&approval_id).expect("read").expect("row");
        self.inner
            .put_if_current(Some(&written), &committed(approval_id.clone()))
            .expect("the owner's committed row");
        approval_id
    }
}

/// A governed runtime: it admits continuations only as retained resumes,
/// after the delivery owner confirmed them with their committed job owner.
/// Admitting into a runtime, each resume becomes a real input of that
/// runtime's session through the runtime's own admission, the one a
/// governed retained resume ends in once its native host granted it.
#[derive(Default)]
struct GovernedSink {
    resumed: Mutex<Vec<(SessionId, RetainedWorkIdentity)>>,
    runtime: Option<Arc<MeerkatMachine>>,
}

impl GovernedSink {
    fn admitting_into(runtime: Arc<MeerkatMachine>) -> Self {
        Self {
            runtime: Some(runtime),
            ..Self::default()
        }
    }

    fn resumed(&self) -> Vec<(SessionId, RetainedWorkIdentity)> {
        self.resumed.lock().expect("resume lock").clone()
    }
}

#[async_trait::async_trait]
impl ContinuationDeliverySink for GovernedSink {
    async fn admit(
        &self,
        _session: &SessionId,
        _input: meerkat_runtime::Input,
    ) -> Result<InputId, String> {
        panic!("a governed runtime never receives an unbound continuation")
    }

    async fn admitted_input(
        &self,
        session: &SessionId,
        admission_key: &str,
    ) -> Result<Option<meerkat_runtime::input_state::StoredInputState>, String> {
        match &self.runtime {
            Some(runtime) => runtime
                .input_state_by_idempotency_key(session, admission_key)
                .await
                .map_err(|error| error.to_string()),
            None => Ok(None),
        }
    }

    fn governs_work_authority(&self) -> bool {
        true
    }

    async fn admit_retained(
        &self,
        session: &SessionId,
        input: meerkat_runtime::Input,
        request: meerkat_runtime::retained_work::RetainedResumeRequest,
    ) -> Result<InputId, ContinuationAdmitError> {
        self.resumed
            .lock()
            .expect("resume lock")
            .push((session.clone(), request.identity().clone()));
        let Some(runtime) = &self.runtime else {
            return Ok(input.id().clone());
        };
        match runtime
            .accept_input_with_completion(session, input)
            .await
            .map(|(outcome, _completion)| outcome)
        {
            Ok(AcceptOutcome::Accepted { input_id, .. }) => Ok(input_id),
            Ok(AcceptOutcome::Deduplicated { existing_id, .. }) => Ok(existing_id),
            other => Err(ContinuationAdmitError::Failed(format!(
                "the runtime did not admit the notice: {other:?}"
            ))),
        }
    }
}

struct NoJobs;

#[async_trait::async_trait]
impl JobDeliverySink for NoJobs {
    async fn apply(
        &self,
        _application: JobDeliveryApplication,
    ) -> Result<(), JobDeliveryApplyError> {
        Ok(())
    }
}

/// A single-process host of one hosting domain that projects the consent
/// notices its approval owner owes there and confirms them against it, as
/// `SessionServiceDeliveryHost::with_consent_notices` composes them.
struct ConsentHost {
    projector: Arc<ConsentNoticeProjector>,
    sink: Arc<GovernedSink>,
    /// Records where each finished projection page started, for a control
    /// that must know a round ran after a given signal. It wakes nothing.
    pages: Option<PageLog>,
}

/// Where one finished projection page started: the approval owner's
/// consent delivery generation and the host bindings' source generation.
#[derive(Debug, Clone, Copy)]
struct PageStart {
    owed: u64,
    sources: u64,
}

struct PageLog {
    sources: watch::Receiver<u64>,
    finished: watch::Sender<Vec<PageStart>>,
}

#[async_trait::async_trait]
impl RuntimeDeliveryHost for ConsentHost {
    async fn delivery_route(&self, _session_id: &SessionId) -> Option<meerkat::DeliveryRoute> {
        Some(meerkat::DeliveryRoute::ServedHere(Arc::new(NoJobs)))
    }

    async fn claim_cold_delivery(
        &self,
        session_id: &SessionId,
    ) -> Option<Result<meerkat_runtime::HostingClaim, meerkat_runtime::HostingRefused>> {
        Some(meerkat_runtime::grant_session_hosting(
            &meerkat_runtime::HostingCapability::ProcessLocal,
            &meerkat_runtime::HostingOwner::mint(),
            session_id,
        ))
    }

    async fn continuation_sink(
        &self,
        _session_id: &SessionId,
    ) -> Option<Arc<dyn ContinuationDeliverySink>> {
        Some(self.sink.clone())
    }

    fn retained_job_source(&self) -> Option<Arc<dyn RetainedJobSource>> {
        Some(Arc::new(self.projector.job_source()))
    }

    fn projects_owed_continuations(&self) -> bool {
        true
    }

    fn subscribe_owed_continuation_changes(&self) -> Option<watch::Receiver<u64>> {
        Some(self.projector.subscribe_owed_changes())
    }

    async fn project_owed_continuations(
        &self,
        after: Option<&OwedContinuationCursor>,
        limit: usize,
    ) -> OwedContinuationProjection {
        let start = self.pages.as_ref().map(|log| PageStart {
            owed: *self.projector.subscribe_owed_changes().borrow(),
            sources: *log.sources.borrow(),
        });
        let page = self.projector.project(after, limit).await;
        if let (Some(log), Some(start)) = (&self.pages, start) {
            log.finished.send_modify(|pages| pages.push(start));
        }
        page
    }
}

struct Fixture {
    approvals: Arc<ApprovalService>,
    domain: HostingDomainRef,
    inbox: RuntimeDeliveryInbox,
    /// The host's runtime, kept alive here: the projector holds it weakly.
    runtime: Arc<MeerkatMachine>,
    continuations: Arc<ContinuationOwnerService>,
    projector: Arc<ConsentNoticeProjector>,
    sink: Arc<GovernedSink>,
}

impl Fixture {
    fn new(approvals: ApprovalService, domain: HostingDomainRef) -> Self {
        Self::with_sink(approvals, domain, |_| GovernedSink::default())
    }

    /// A fixture whose governed sink admits each notice into the host's
    /// runtime as a real input.
    fn admitting(approvals: ApprovalService, domain: HostingDomainRef) -> Self {
        Self::with_sink(approvals, domain, GovernedSink::admitting_into)
    }

    fn with_sink(
        approvals: ApprovalService,
        domain: HostingDomainRef,
        sink: impl FnOnce(Arc<MeerkatMachine>) -> GovernedSink,
    ) -> Self {
        let approvals = Arc::new(approvals);
        let inbox = RuntimeDeliveryInbox::new(Arc::new(InMemoryRuntimeStore::new()));
        let runtime = Arc::new(MeerkatMachine::ephemeral());
        let continuations = Arc::new(ContinuationOwnerService::new(
            inbox.clone(),
            Arc::new(SessionAddressResolver),
            Arc::clone(&runtime),
        ));
        let projector = Arc::new(ConsentNoticeProjector::new(
            Arc::clone(&approvals),
            domain.clone(),
            inbox.clone(),
            &runtime,
        ));
        Self {
            approvals,
            domain,
            inbox,
            sink: Arc::new(sink(Arc::clone(&runtime))),
            runtime,
            continuations,
            projector,
        }
    }

    fn owner(&self) -> RuntimeDeliveryOwner {
        RuntimeDeliveryOwner::new(Arc::new(MemoryDetachedJobStore::new()), self.inbox.clone())
    }

    fn host(&self) -> Arc<dyn RuntimeDeliveryHost> {
        Arc::new(ConsentHost {
            projector: Arc::clone(&self.projector),
            sink: Arc::clone(&self.sink),
            pages: None,
        })
    }

    /// [`Self::host`], recording where each finished page started, read
    /// against `sources` (the host bindings' source changes).
    fn paged_host(
        &self,
        sources: watch::Receiver<u64>,
    ) -> (
        Arc<dyn RuntimeDeliveryHost>,
        watch::Receiver<Vec<PageStart>>,
    ) {
        let (finished, pages) = watch::channel(Vec::new());
        let host = Arc::new(ConsentHost {
            projector: Arc::clone(&self.projector),
            sink: Arc::clone(&self.sink),
            pages: Some(PageLog { sources, finished }),
        });
        (host, pages)
    }

    async fn status(&self, session: &SessionId, approval_id: &ApprovalId) -> ContinuationStatus {
        self.status_of(session, approval_id, ApprovalConsentTransition::Approved)
            .await
    }

    async fn status_of(
        &self,
        session: &SessionId,
        approval_id: &ApprovalId,
        transition: ApprovalConsentTransition,
    ) -> ContinuationStatus {
        let job_id = consent_delivery_job_id(approval_id, transition);
        self.continuations
            .continuation_status(
                &ContinuationOwner::Session {
                    session_id: session.clone(),
                },
                &ContinuationKey::new(format!("consent:{job_id}")).expect("key"),
            )
            .await
            .expect("status")
    }
}

fn store_with(rows: &[ApprovalRecord]) -> Arc<InMemoryApprovalStore> {
    let store = Arc::new(InMemoryApprovalStore::new());
    for row in rows {
        store.put_if_current(None, row).expect("store row");
    }
    store
}

async fn wait_for_pass(
    passes: &mut watch::Receiver<RuntimeDeliveryPass>,
    what: &str,
    predicate: impl FnMut(&RuntimeDeliveryPass) -> bool,
) -> RuntimeDeliveryPass {
    tokio::time::timeout(EVENT_GUARD, passes.wait_for(predicate))
        .await
        .unwrap_or_else(|_| panic!("delivery owner never reached: {what}"))
        .expect("owner pass channel open")
        .clone()
}

/// Let the clock run far past any plausible poll interval and let every
/// woken task run; a looping owner would complete passes here.
async fn idle_for_an_hour() {
    tokio::time::advance(Duration::from_secs(3600)).await;
    for _ in 0..64 {
        tokio::task::yield_now().await;
    }
}

/// A notice owed at arming is projected by the arming round and admitted to
/// the exact session of its committed scope, as a retained resume of the
/// admitting run's work, after the approval owner confirmed it.
#[tokio::test(start_paused = true)]
async fn an_owed_notice_is_admitted_to_its_exact_scope_session_after_the_owner_confirms_it() {
    let here = domain("realm-a/hosting");
    let session = SessionId::new();
    let origin = origin_work("runtime-a");
    let row = approved_owing_row(None, &here, &session, &origin);
    let store = store_with(std::slice::from_ref(&row));
    let fixture = Fixture::new(
        ApprovalService::with_store(store).expect("service"),
        here.clone(),
    );

    let handle = fixture.owner().arm(fixture.host()).expect("arm owner");
    let mut passes = handle.subscribe_passes();
    let armed = wait_for_pass(&mut passes, "the arming round", |pass| pass.generation >= 1).await;
    assert_eq!(
        (armed.owed_projected, armed.applied, armed.failures.len()),
        (1, 1, 0),
        "the arming round projected the notice and drained its row: {armed:?}"
    );
    assert_eq!(
        fixture.sink.resumed(),
        vec![(session.clone(), origin.clone())],
        "admitted to the scope's session, resuming the admitting run's work"
    );
    match fixture.status(&session, &row.approval_id).await {
        ContinuationStatus::Applied {
            session: admitted, ..
        } => assert_eq!(admitted, session),
        other => panic!("the notice was not admitted: {other:?}"),
    }
    let job_id = consent_delivery_job_id(&row.approval_id, ApprovalConsentTransition::Approved);
    match fixture
        .projector
        .job_source()
        .retained_job(&consent_notice_producer(), &job_id)
        .await
    {
        RetainedJobLookup::Found(facts) => {
            assert_eq!(facts.owner_session_id, session);
            assert_eq!(facts.retained_work, Some(origin));
        }
        other => panic!("the owner confirms its committed notice: {other:?}"),
    }

    passes.borrow_and_update();
    idle_for_an_hour().await;
    assert!(
        !passes.has_changed().expect("owner pass channel open"),
        "the notice stays owed until its receipt settles it, and no pass repeats it"
    );
}

/// A notice committed for another hosting domain is that domain's to
/// deliver, whatever its session id: this host neither projects it nor
/// confirms it.
#[tokio::test(start_paused = true)]
async fn a_notice_owed_to_another_hosting_domain_is_never_projected_or_confirmed_here() {
    let here = domain("realm-a/hosting");
    let elsewhere = domain("realm-b/hosting");
    let session = SessionId::new();
    let row = approved_owing_row(None, &elsewhere, &session, &origin_work("runtime-b"));
    let store = store_with(std::slice::from_ref(&row));
    let fixture = Fixture::new(ApprovalService::with_store(store).expect("service"), here);

    let handle = fixture.owner().arm(fixture.host()).expect("arm owner");
    let mut passes = handle.subscribe_passes();
    let armed = wait_for_pass(&mut passes, "the arming round", |pass| pass.generation >= 1).await;
    assert_eq!(
        (armed.owed_projected, armed.applied, armed.failures.len()),
        (0, 0, 0),
        "{armed:?}"
    );
    assert!(fixture.sink.resumed().is_empty());
    assert_eq!(
        fixture.status(&session, &row.approval_id).await,
        ContinuationStatus::NotCommitted
    );
    let job_id = consent_delivery_job_id(&row.approval_id, ApprovalConsentTransition::Approved);
    assert_eq!(
        fixture
            .projector
            .job_source()
            .retained_job(&consent_notice_producer(), &job_id)
            .await,
        RetainedJobLookup::Absent,
        "another domain's notice is not this domain's job"
    );
    assert_ne!(fixture.domain, elsewhere);
}

/// An unavailable approval owner is never "nothing owed": the page fails,
/// the round ends without looping, and the failure stays owed to the next
/// wake. The host keeps projecting: its capability does not depend on the
/// owner's availability.
#[tokio::test(start_paused = true)]
async fn an_unavailable_owner_fails_its_page_and_is_never_reported_as_nothing_owed() {
    let fixture = Fixture::new(
        ApprovalService::unavailable("the approval store is offline"),
        domain("realm-a/hosting"),
    );
    let page = fixture.projector.project(None, 8).await;
    assert_eq!((page.committed, page.next.is_none()), (0, true));
    assert_eq!(page.failures.len(), 1, "{page:?}");
    assert!(
        page.failures[0].contains("could not be discovered"),
        "{page:?}"
    );

    let host = fixture.host();
    assert!(host.projects_owed_continuations());
    let handle = fixture.owner().arm(host).expect("arm owner");
    let mut passes = handle.subscribe_passes();
    let armed = wait_for_pass(&mut passes, "the arming round", |pass| pass.generation >= 1).await;
    assert_eq!(armed.owed_projected, 0);
    assert!(
        armed
            .failures
            .iter()
            .any(|failure| failure.contains("could not be discovered")),
        "{armed:?}"
    );
    passes.borrow_and_update();
    idle_for_an_hour().await;
    assert!(
        !passes.has_changed().expect("owner pass channel open"),
        "the failure waits for the next wake; it is never retried in a loop"
    );
}

/// A row the owner could not reconcile is carried, never dropped: the page
/// reports it, the owner's next wake starts another round, and that round's
/// discovery reconciles the row once it reads and projects what it owes.
#[tokio::test(start_paused = true)]
async fn a_row_the_owner_cannot_reconcile_is_carried_until_a_later_round_projects_it() {
    let here = domain("realm-a/hosting");
    let session = SessionId::new();
    let origin = origin_work("runtime-a");
    let store = Arc::new(LossyStore::default());
    let fixture = Fixture::new(
        ApprovalService::with_store(store.clone()).expect("service"),
        here.clone(),
    );
    let approval_id = store.commit_unacknowledged(&fixture.approvals, |id| {
        approved_owing_row(Some(id), &here, &session, &origin)
    });
    store.unreadable.store(true, Ordering::SeqCst);

    let (settlements, run_settlements) = watch::channel(0_u64);
    let handle = fixture
        .owner()
        .with_run_settlements(run_settlements)
        .arm(fixture.host())
        .expect("arm owner");
    let mut passes = handle.subscribe_passes();
    let armed = wait_for_pass(&mut passes, "the arming round", |pass| pass.generation >= 1).await;
    assert_eq!(armed.owed_projected, 0);
    assert!(
        armed
            .failures
            .iter()
            .any(|failure| failure.contains(&approval_id.to_string())),
        "the unreconciled row is reported, not dropped: {armed:?}"
    );

    store.unreadable.store(false, Ordering::SeqCst);
    // Any wake carries the failed round: here a run settlement, which on its
    // own starts no round.
    settlements.send_modify(|generation| *generation += 1);
    let carried = wait_for_pass(&mut passes, "the carried round", |pass| {
        pass.generation >= 2
    })
    .await;
    assert_eq!(
        (
            carried.owed_projected,
            carried.applied,
            carried.failures.len()
        ),
        (1, 1, 0),
        "{carried:?}"
    );
    assert_eq!(fixture.sink.resumed(), vec![(session, origin)]);
}

/// A notice the owner committed after the delivery owner armed, and that no
/// source change announced, is not found by other wakes: it waits for the
/// next source change (or reconcile), whose round discovers it.
#[tokio::test(start_paused = true)]
async fn an_unannounced_owed_notice_is_projected_by_the_next_source_change_only() {
    let here = domain("realm-a/hosting");
    let session = SessionId::new();
    let origin = origin_work("runtime-a");
    let store = Arc::new(LossyStore::default());
    let fixture = Fixture::new(
        ApprovalService::with_store(store.clone()).expect("service"),
        here.clone(),
    );
    let (settlements, run_settlements) = watch::channel(0_u64);
    let (sources, source_changes) = watch::channel(0_u64);
    let handle = fixture
        .owner()
        .with_run_settlements(run_settlements)
        .with_source_changes(source_changes)
        .arm(fixture.host())
        .expect("arm owner");
    let mut passes = handle.subscribe_passes();
    let armed = wait_for_pass(&mut passes, "the arming round", |pass| pass.generation >= 1).await;
    assert_eq!((armed.owed_projected, armed.failures.len()), (0, 0));
    passes.borrow_and_update();

    let approval_id = store.commit_unacknowledged(&fixture.approvals, |id| {
        approved_owing_row(Some(id), &here, &session, &origin)
    });
    settlements.send_modify(|generation| *generation += 1);
    idle_for_an_hour().await;
    assert!(
        !passes.has_changed().expect("owner pass channel open"),
        "a run settlement starts no round, so the unannounced notice waits"
    );
    assert_eq!(
        fixture.status(&session, &approval_id).await,
        ContinuationStatus::NotCommitted
    );

    sources.send_modify(|revision| *revision += 1);
    let announced = wait_for_pass(&mut passes, "the source change round", |pass| {
        pass.generation >= 2
    })
    .await;
    assert_eq!(
        (
            announced.owed_projected,
            announced.applied,
            announced.failures.len()
        ),
        (1, 1, 0),
        "{announced:?}"
    );
    assert_eq!(fixture.sink.resumed(), vec![(session, origin)]);
}

/// The real session delivery host composed with consent notices opts in as
/// a stable capability: with the approval owner unavailable it still asks
/// for its pages, which fail, and still answers the consent namespace as
/// unavailable (retry later), never absent.
#[cfg(feature = "session-store")]
#[tokio::test]
async fn the_session_delivery_host_projects_consent_notices_while_their_owner_is_unavailable() {
    let fixture = Fixture::new(
        ApprovalService::unavailable("the approval store is offline"),
        domain("realm-a/hosting"),
    );
    let temp = tempfile::tempdir().expect("tempdir");
    let service = Arc::new(meerkat::build_ephemeral_service(
        meerkat::AgentFactory::new(temp.path().join("sessions")),
        meerkat::Config::default(),
        4,
    ));
    let runtime = Arc::new(meerkat_runtime::MeerkatMachine::ephemeral());
    let host = meerkat::surface::SessionServiceDeliveryHost::new(
        &service,
        &runtime,
        meerkat::DetachedJobService::new(Arc::new(MemoryDetachedJobStore::new())),
        None,
    )
    .with_consent_notices(Arc::new(ConsentNoticeProjector::new(
        Arc::clone(&fixture.approvals),
        fixture.domain.clone(),
        fixture.inbox.clone(),
        &runtime,
    )));

    assert!(host.projects_owed_continuations());
    let page = host.project_owed_continuations(None, 8).await;
    assert_eq!(page.committed, 0);
    assert!(
        page.failures
            .iter()
            .any(|failure| failure.contains("could not be discovered")),
        "{page:?}"
    );
    let job_id = consent_delivery_job_id(&ApprovalId::new(), ApprovalConsentTransition::Approved);
    let lookup = host
        .retained_job_source()
        .expect("the consent namespace is answered")
        .retained_job(&consent_notice_producer(), &job_id)
        .await;
    assert!(
        matches!(lookup, RetainedJobLookup::Unavailable(_)),
        "{lookup:?}"
    );
}

/// A consent host's own owed-change signal is its approval owner's consent
/// delivery signal, so a change the owner announces wakes the delivery owner
/// composed over the host; a host without consent notices announces nothing.
#[cfg(feature = "session-store")]
#[tokio::test]
async fn a_consent_delivery_host_hears_its_approval_owner_announce() {
    let fixture = Fixture::new(
        ApprovalService::with_store(store_with(&[])).expect("service"),
        domain("realm-a/hosting"),
    );
    let temp = tempfile::tempdir().expect("tempdir");
    let service = Arc::new(meerkat::build_ephemeral_service(
        meerkat::AgentFactory::new(temp.path().join("sessions")),
        meerkat::Config::default(),
        4,
    ));
    let runtime = Arc::new(meerkat_runtime::MeerkatMachine::ephemeral());
    let plain = meerkat::surface::SessionServiceDeliveryHost::new(
        &service,
        &runtime,
        meerkat::DetachedJobService::new(Arc::new(MemoryDetachedJobStore::new())),
        None,
    );
    assert!(
        plain.subscribe_owed_continuation_changes().is_none(),
        "a host without consent notices announces nothing of its own"
    );

    let host = plain.with_consent_notices(Arc::new(ConsentNoticeProjector::new(
        Arc::clone(&fixture.approvals),
        fixture.domain.clone(),
        fixture.inbox.clone(),
        &runtime,
    )));
    let changes = host
        .subscribe_owed_continuation_changes()
        .expect("a consent host announces its approval owner's changes");
    assert!(
        changes.same_channel(&fixture.approvals.subscribe_consent_deliveries()),
        "the host's own signal is its approval owner's consent delivery signal"
    );
}

/// The delivery owner's loop holds its host for as long as it runs, so a
/// consent host must never keep its own runtime alive. Composed over the
/// runtime it delivers for, with a notice to commit (which takes the runtime
/// for that page), the loop lets the runtime go once its last outside handle
/// drops: the runtime is freed, its attachment signal closes, and the owner
/// stops.
#[cfg(feature = "session-store")]
#[tokio::test]
async fn a_consent_delivery_host_never_keeps_its_own_runtime_alive() {
    let here = domain("realm-a/hosting");
    let row = approved_owing_row(None, &here, &SessionId::new(), &origin_work("runtime-a"));
    let approvals = Arc::new(
        ApprovalService::with_store(store_with(std::slice::from_ref(&row))).expect("service"),
    );
    let temp = tempfile::tempdir().expect("tempdir");
    let service = Arc::new(meerkat::build_ephemeral_service(
        meerkat::AgentFactory::new(temp.path().join("sessions")),
        meerkat::Config::default(),
        4,
    ));
    let runtime = Arc::new(meerkat_runtime::MeerkatMachine::ephemeral());
    let inbox = RuntimeDeliveryInbox::new(Arc::new(InMemoryRuntimeStore::new()));
    let host = meerkat::surface::SessionServiceDeliveryHost::new(
        &service,
        &runtime,
        meerkat::DetachedJobService::new(Arc::new(MemoryDetachedJobStore::new())),
        None,
    )
    .with_consent_notices(Arc::new(ConsentNoticeProjector::new(
        approvals,
        here,
        inbox.clone(),
        &runtime,
    )));
    let handle = RuntimeDeliveryOwner::new(Arc::new(MemoryDetachedJobStore::new()), inbox)
        .with_attachment_commits(runtime.subscribe_attachment_commits())
        .arm(Arc::new(host))
        .expect("arm owner");
    let mut passes = handle.subscribe_passes();
    let armed = wait_for_pass(&mut passes, "the arming round", |pass| pass.generation >= 1).await;
    assert_eq!(
        armed.owed_projected, 1,
        "the arming page committed the notice with the runtime: {armed:?}"
    );

    let freed = Arc::downgrade(&runtime);
    drop(runtime);
    let released = tokio::time::timeout(EVENT_GUARD, async {
        while freed.upgrade().is_some() || !handle.is_stopped() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    assert!(
        released.is_ok(),
        "the delivery owner kept its own runtime alive (runtime alive: {}, owner stopped: {})",
        freed.upgrade().is_some(),
        handle.is_stopped()
    );
}

/// The runtime's terminal of `input` in `session`, if it has one.
async fn native_terminal(
    runtime: &MeerkatMachine,
    session: &SessionId,
    input: &InputId,
) -> Option<InputTerminalOutcome> {
    runtime
        .interaction_terminal_status(session, InteractionSelector::InputId(input.clone()))
        .await
        .expect("the runtime reads its input")
        .expect("the runtime knows its input")
        .report
        .terminal
}

/// Wait until a projection page that started at or after both generations
/// has finished: a round that saw that signal ran to its end.
async fn page_finished_after(
    pages: &mut watch::Receiver<Vec<PageStart>>,
    what: &str,
    owed: u64,
    sources: u64,
) {
    tokio::time::timeout(
        EVENT_GUARD,
        pages.wait_for(|pages| {
            pages
                .iter()
                .any(|page| page.owed >= owed && page.sources >= sources)
        }),
    )
    .await
    .unwrap_or_else(|_| panic!("no round ran after {what}"))
    .expect("page log open");
}

/// Parked consent whose admitted notice input the runtime cancels is
/// withdrawn by the delivery owner's own rounds in either order of the
/// input's end and its delayed approval receipt, each round woken only by
/// the real signal of the step that makes the settlement possible.
///
/// Real here: the parked custody (issued, approved and parked through the
/// approval owner's own paths), the queued notice input of the host's
/// runtime and its idempotency lookup, the runtime's exact cancellation and
/// the abandoned terminal read back from it, the receipt commit, and the
/// wakes (the runtime's runless-terminal hint through the host bindings,
/// and the approval owner's announcement of the receipt). The test never
/// reconciles, never ticks a source and never polls.
///
/// Scope: the governed sink admits the notice through the runtime's direct
/// admission, bypassing retained-resume grant evaluation and its native
/// retained-authority attachment. This establishes the cancellation,
/// receipt and wake composition, not retained-resume authorization or
/// replay qualification, which stay with their own suites.
///
/// Limits: no executor serves this fixture's session. The fixed R3 and
/// eligible-human helper and the direct governed sink bypass retained-resume
/// grants and their attachment. This does not prove a resumed governed run,
/// permitted sibling execution, a physical one-use spend or model-turn
/// feedback; those are other native and installed acceptance obligations.
/// Pending, missing and error reads and the other terminal classifications
/// are separately scoped controls.
///
/// Receipt first: the receipt's own round finishes while the input is still
/// queued and settles nothing, so only the cancellation's hint can start
/// the round that settles. Cancellation first: the hint's round finishes
/// and the consent stays unwithdrawn until the receipt is committed and
/// announced. Both orders end alike: one withdrawal naming the input, the
/// receipt kept, one `Withdrawn` notice delivered once, the input
/// abandoned.
#[tokio::test]
async fn a_cancelled_notice_input_withdraws_its_parked_consent_in_either_order() {
    for receipt_first in [false, true] {
        let here = domain("realm-a/hosting");
        let session = SessionId::new();
        let origin = origin_work("runtime-a");
        let fixture = Fixture::admitting(
            ApprovalService::with_store(store_with(&[])).expect("service"),
            here.clone(),
        );
        fixture
            .runtime
            .register_session(session.clone())
            .await
            .expect("register the destination session");
        let scope = ApprovalConsentScope {
            domain: here,
            session_id: session.clone(),
        };
        let parked = ParkedRoutedConsent::park(&fixture.approvals, scope, origin.clone())
            .await
            .expect("a routed consent parked through the owner's paths");
        let approval_id = parked.approval_id().clone();
        let bindings = Arc::new(ContinuationHostBindings::default());
        bindings.announce_runless_input_terminals_of(&fixture.runtime);
        let (host, mut pages) = fixture.paged_host(bindings.subscribe_source_changes());
        let handle = fixture
            .owner()
            .with_source_changes(bindings.subscribe_source_changes())
            .arm(host)
            .expect("arm owner");
        let mut passes = handle.subscribe_passes();
        let armed =
            wait_for_pass(&mut passes, "the arming round", |pass| pass.generation >= 1).await;
        assert_eq!(
            (armed.owed_projected, armed.applied, armed.failures.len()),
            (1, 1, 0),
            "the arming round admitted the approved notice: {armed:?}"
        );
        let notice_input = match fixture.status(&session, &approval_id).await {
            ContinuationStatus::Applied { input, .. } => input,
            other => panic!("the approved notice was not admitted: {other:?}"),
        };
        assert_eq!(
            native_terminal(&fixture.runtime, &session, &notice_input).await,
            None,
            "no executor serves the session: the notice input stays queued"
        );
        let receipt = ConsentDeliveryReceipt::Admitted {
            session_id: session.clone(),
            input_id: notice_input.clone(),
        };
        let withdrawn = || {
            fixture
                .approvals
                .get(&approval_id)
                .expect("row")
                .consent
                .and_then(|consent| consent.withdrawal)
                .map(|withdrawal| withdrawal.reason)
        };
        let abandoned = Some(InputTerminalOutcome::Abandoned {
            reason: InputAbandonReason::Cancelled,
        });

        if receipt_first {
            fixture
                .approvals
                .settle_consent_delivery(
                    &approval_id,
                    ApprovalConsentTransition::Approved,
                    receipt.clone(),
                )
                .expect("the receipt commits");
            let announced = *fixture.approvals.subscribe_consent_deliveries().borrow();
            page_finished_after(&mut pages, "the receipt", announced, 0).await;
            assert_eq!(withdrawn(), None, "a queued input proves nothing");
            assert!(
                fixture
                    .runtime
                    .cancel_input_if_present(&session, &notice_input, "the notice was withdrawn")
                    .await
                    .expect("cancel the notice input"),
                "the queued notice input is observed"
            );
            assert_eq!(
                native_terminal(&fixture.runtime, &session, &notice_input).await,
                abandoned,
                "the runtime abandoned the queued notice input"
            );
        } else {
            assert!(
                fixture
                    .runtime
                    .cancel_input_if_present(&session, &notice_input, "the notice was withdrawn")
                    .await
                    .expect("cancel the notice input"),
                "the queued notice input is observed"
            );
            assert_eq!(
                native_terminal(&fixture.runtime, &session, &notice_input).await,
                abandoned,
                "the runtime abandoned the queued notice input"
            );
            let hinted = *bindings.subscribe_source_changes().borrow();
            page_finished_after(&mut pages, "the cancellation", 0, hinted).await;
            assert_eq!(withdrawn(), None, "no receipt names the input yet");
            fixture
                .approvals
                .settle_consent_delivery(
                    &approval_id,
                    ApprovalConsentTransition::Approved,
                    receipt.clone(),
                )
                .expect("the delayed receipt commits");
        }

        // The owner's own rounds settle the parked consent and then deliver
        // its owed Withdrawn notice.
        tokio::time::timeout(EVENT_GUARD, async {
            while !matches!(
                fixture
                    .status_of(&session, &approval_id, ApprovalConsentTransition::Withdrawn)
                    .await,
                ContinuationStatus::Applied { .. }
            ) {
                passes.changed().await.expect("owner pass channel open");
            }
        })
        .await
        .unwrap_or_else(|_| {
            panic!("the Withdrawn notice was never delivered (receipt_first={receipt_first})")
        });

        assert_eq!(
            withdrawn(),
            Some(ApprovalConsentWithdrawalReason::NoticeInputEnded {
                input_id: notice_input.to_string(),
            }),
            "receipt_first={receipt_first}"
        );
        let deliveries: Vec<_> = fixture
            .approvals
            .get(&approval_id)
            .expect("row")
            .consent
            .expect("consent")
            .deliveries
            .into_iter()
            .map(|delivery| (delivery.transition, delivery.state))
            .collect();
        assert_eq!(
            deliveries,
            vec![
                (
                    ApprovalConsentTransition::Approved,
                    ApprovalConsentDeliveryState::Admitted {
                        session_id: session.clone(),
                        input_id: notice_input.to_string(),
                    },
                ),
                (
                    ApprovalConsentTransition::Withdrawn,
                    ApprovalConsentDeliveryState::Owed,
                ),
            ],
            "the exact receipt naming the input stays, and the withdrawal owes its own notice"
        );
        assert_eq!(
            parked
                .notices()
                .iter()
                .filter(|kind| **kind == ConsentNoticeKind::Withdrawn)
                .count(),
            1,
            "one Withdrawn notice"
        );
        assert_eq!(
            fixture.sink.resumed(),
            vec![(session.clone(), origin.clone()), (session.clone(), origin)],
            "the approved notice, then the Withdrawn notice, each delivered once"
        );
        assert_eq!(
            native_terminal(&fixture.runtime, &session, &notice_input).await,
            abandoned
        );
    }
}

/// A governed runtime's native work authorization host, which refuses all;
/// only its installation (the governed posture) matters here.
#[cfg(feature = "session-store")]
struct RefusingWorkAuthority;

#[cfg(feature = "session-store")]
impl meerkat_runtime::input_authority::NativeWorkAuthorizationHost for RefusingWorkAuthority {
    fn authenticate_association(
        &self,
        _runtime_id: &meerkat_runtime::LogicalRuntimeId,
        _input: &meerkat_runtime::Input,
        _ingress: &meerkat_runtime::input_authority::NativeIngressContext,
        _association: &meerkat_authorization_contracts::work_association::InputAuthorityAssociation,
    ) -> Result<(), meerkat_runtime::input_authority::NativeAdmissionError> {
        Err(meerkat_core::OperationRefused::new(meerkat_core::OperationRefusalKind::Denied).into())
    }

    fn work_context(
        &self,
        _batch: &meerkat_runtime::input_authority::NativeWorkBatch,
    ) -> Result<
        meerkat_core::WorkAuthorizationContext,
        meerkat_runtime::input_authority::NativeWorkContextError,
    > {
        Err(meerkat_runtime::input_authority::NativeWorkContextError::MalformedAcceptedWork)
    }
}

/// The committed owner of one host original and one fork_off job, as a test
/// sets it: holding them, or holding nothing.
#[cfg(feature = "session-store")]
struct Owner {
    original: Option<meerkat::HostOriginalFacts>,
    job: Option<meerkat::RetainedJobFacts>,
}

#[cfg(feature = "session-store")]
#[async_trait::async_trait]
impl RetainedJobSource for Owner {
    async fn retained_job(
        &self,
        producer: &meerkat::ContinuationProducer,
        job_id: &str,
    ) -> RetainedJobLookup {
        match &self.job {
            Some(facts)
                if *producer == meerkat::ContinuationProducer::ForkOff && job_id == "job-1" =>
            {
                RetainedJobLookup::Found(facts.clone())
            }
            _ => RetainedJobLookup::Absent,
        }
    }

    async fn host_original(
        &self,
        namespace: &str,
        producer_id: &str,
    ) -> meerkat::HostOriginalLookup {
        match &self.original {
            Some(facts) if namespace == "app.ingest" && producer_id == "observation-1" => {
                meerkat::HostOriginalLookup::Found(facts.clone())
            }
            _ => meerkat::HostOriginalLookup::Absent,
        }
    }
}

/// The restart window on a consent-aware host: deliveries committed before a
/// restart are drained by the arming reconcile before the host binds its
/// continuation services. A host original and a retained completion drained
/// then stay pending (retried), never refused, as the readiness signal
/// (`Unbound`) says. Once the host binds, the bound owner decides them: here
/// it holds neither, so each is refused.
#[cfg(feature = "session-store")]
#[tokio::test]
async fn deliveries_drained_before_the_host_binds_its_services_stay_pending_until_bound() {
    use meerkat::{
        ContinuationDelivery, ContinuationHandling, ContinuationProducer, ContinuationResultRef,
    };
    let runtime = Arc::new(
        MeerkatMachine::ephemeral()
            .with_native_work_authorization_host(Arc::new(RefusingWorkAuthority))
            .expect("a governed runtime"),
    );
    let temp = tempfile::tempdir().expect("tempdir");
    let service = Arc::new(meerkat::build_ephemeral_service(
        meerkat::AgentFactory::new(temp.path().join("sessions")),
        meerkat::Config::default(),
        4,
    ));
    let inbox = RuntimeDeliveryInbox::new(Arc::new(InMemoryRuntimeStore::new()));
    let continuations = ContinuationOwnerService::new(
        inbox.clone(),
        Arc::new(SessionAddressResolver),
        Arc::clone(&runtime),
    );

    // Before the restart: a host original owed to one session and a
    // retained fork_off completion owed to another, each committed from its
    // owner's record.
    let original_owner = ContinuationOwner::Session {
        session_id: SessionId::new(),
    };
    let original = ContinuationDelivery {
        key: ContinuationKey::new("ingest:observation-1").expect("key"),
        result: ContinuationResultRef {
            producer: ContinuationProducer::Host {
                namespace: "app.ingest".into(),
            },
            producer_id: "observation-1".into(),
            result_digest: "sha256:observation-1".into(),
            summary: None,
        },
        body: "a new item arrived".into(),
        handling: ContinuationHandling::Queue,
    };
    let fork_session = SessionId::new();
    let fork_owner = ContinuationOwner::Session {
        session_id: fork_session.clone(),
    };
    let identity = origin_work("rt:session:fork-owner");
    let fork = ContinuationDelivery {
        key: ContinuationKey::new("fork_off:job-1").expect("key"),
        result: ContinuationResultRef {
            producer: ContinuationProducer::ForkOff,
            producer_id: "job-1".into(),
            result_digest: "sha256:result".into(),
            summary: None,
        },
        body: "the fork finished".into(),
        handling: ContinuationHandling::Queue,
    };
    let before = Owner {
        original: Some(meerkat::HostOriginalFacts::new(
            original_owner.clone(),
            original.clone(),
            meerkat_runtime::OriginalWorkRef {
                authority: PrincipalRef::in_domain(
                    PrincipalKind::ServiceAccount,
                    "source-owner",
                    TrustDomainId::new("restart-window").expect("domain"),
                )
                .expect("qualified"),
                work: meerkat_authorization_contracts::evidence::EvidenceId::new("observation-1")
                    .expect("id"),
            },
        )),
        job: Some(meerkat::RetainedJobFacts {
            owner_session_id: fork_session,
            retained_work: Some(identity),
            result_digest: "sha256:result".into(),
        }),
    };
    let record = meerkat::HostOriginalRecord::from_owner(&before, "app.ingest", "observation-1")
        .await
        .expect("read the owner")
        .expect("held");
    continuations
        .submit_host_original(&original_owner, original, &record, 1)
        .await
        .expect("commit the host original");
    let job =
        meerkat::RetainedJobRecord::from_owner(&before, ContinuationProducer::ForkOff, "job-1")
            .await
            .expect("read the owner")
            .expect("held");
    continuations
        .submit_retained_completion(&fork_owner, fork, &job, 1)
        .await
        .expect("commit the retained completion");

    // After the restart: the host composes consent notices and arms its
    // delivery owner before it binds its continuation services.
    let bindings = Arc::new(ContinuationHostBindings::default());
    let host = meerkat::surface::SessionServiceDeliveryHost::new(
        &service,
        &runtime,
        meerkat::DetachedJobService::new(Arc::new(MemoryDetachedJobStore::new())),
        None,
    )
    .with_continuation_bindings(Arc::clone(&bindings))
    .with_consent_notices(Arc::new(ConsentNoticeProjector::new(
        Arc::new(ApprovalService::new()),
        domain("restart-window/hosting"),
        inbox.clone(),
        &runtime,
    )));
    let handle = RuntimeDeliveryOwner::new(Arc::new(MemoryDetachedJobStore::new()), inbox)
        .with_source_changes(bindings.subscribe_source_changes())
        .arm(Arc::new(host))
        .expect("arm");
    let mut passes = handle.subscribe_passes();
    let drained = wait_for_pass(&mut passes, "the arming reconcile drained both", |pass| {
        pass.refused > 0 || pass.blocked_runtimes.len() >= 2
    })
    .await;
    assert_eq!(
        drained.refused, 0,
        "never refused before the host binds: {:?}",
        drained.failures
    );
    for (owner, key) in [
        (&original_owner, "ingest:observation-1"),
        (&fork_owner, "fork_off:job-1"),
    ] {
        let status = continuations
            .continuation_status(owner, &ContinuationKey::new(key).expect("key"))
            .await
            .expect("status");
        assert!(
            matches!(status, ContinuationStatus::Pending { .. }),
            "{key}: {status:?}"
        );
    }

    // The host binds its services; their owner holds neither now.
    let _binding = bindings
        .bind(
            Arc::new(SessionAddressResolver),
            Arc::new(Owner {
                original: None,
                job: None,
            }),
        )
        .expect("bind");
    wait_for_pass(&mut passes, "both decided by the bound owner", |pass| {
        pass.refused >= 2
    })
    .await;
    for (owner, key) in [
        (&original_owner, "ingest:observation-1"),
        (&fork_owner, "fork_off:job-1"),
    ] {
        let status = continuations
            .continuation_status(owner, &ContinuationKey::new(key).expect("key"))
            .await
            .expect("status");
        assert!(
            matches!(
                status,
                ContinuationStatus::Refused {
                    reason: meerkat_runtime::RuntimeDeliveryRefusalReason::NoAdmissibleWorkBinding,
                    ..
                }
            ),
            "{key}: {status:?}"
        );
    }
}
