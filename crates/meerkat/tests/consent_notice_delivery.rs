#![allow(clippy::expect_used, clippy::panic)]
//! Consent notices through the library delivery owner: the issuing approval
//! owner's owed notices for one hosting domain are projected on the owner's
//! rounds and admitted, as retained resumes the owner confirms, to the exact
//! committed owner scope, and each admission records the notice's receipt.
//! Approval rows are written to the store as the approval owner commits
//! them. Parked consent comes from the approval owner's own paths, and an
//! ended notice input is a real input of the host's runtime that the runtime
//! cancels.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use meerkat::{
    ConsentNoticeJobSource, ConsentNoticeProjector, ContinuationAdmitError,
    ContinuationDeliverySink, ContinuationHostBindings, ContinuationKey, ContinuationOwner,
    ContinuationOwnerService, ContinuationStatus, JobDeliveryApplication, JobDeliveryApplyError,
    JobDeliverySink, MemoryDetachedJobStore, OwedContinuationCursor, OwedContinuationProjection,
    RetainedJobLookup, RetainedJobSource, RuntimeDeliveryHost, RuntimeDeliveryOwner,
    RuntimeDeliveryPass, SessionAddressResolver, SessionId, consent_notice_producer,
};
use meerkat_core::approval::ApprovalConsentWithdrawalReason;
use meerkat_core::approval::consent::{
    ConsentNoticeKind, consent_delivery_job_id, parse_consent_delivery_job_id,
};
use meerkat_core::approval::consent_test_support::ParkedRoutedConsent;
use meerkat_core::lifecycle::{InputId, RunId};
use meerkat_core::retained_work::RetainedWorkIdentity;
use meerkat_core::session_hosting::{HostingDomainRef, HostingPaths};
use meerkat_core::{
    ApprovalActionKind, ApprovalAssurance, ApprovalConsentApprover, ApprovalConsentCustody,
    ApprovalConsentDelivery, ApprovalConsentDeliveryState, ApprovalConsentOrigin,
    ApprovalConsentRecord, ApprovalConsentScope, ApprovalConsentTransition, ApprovalDecision,
    ApprovalError, ApprovalId, ApprovalOwnerRef, ApprovalPrincipalId, ApprovalProposedAction,
    ApprovalRecord, ApprovalRequest, ApprovalResourceId, ApprovalResourceKind, ApprovalResourceRef,
    ApprovalRisk, ApprovalService, ApprovalStore, ApprovalStoreError, InMemoryApprovalStore,
    PrincipalKind, PrincipalRef, SurfaceMetadata, TrustDomainId,
};
use meerkat_runtime::input_state::{InputAbandonReason, InputTerminalOutcome, StoredInputState};
use meerkat_runtime::terminal_status::InteractionSelector;
use meerkat_runtime::{
    AcceptOutcome, InMemoryRuntimeStore, MeerkatMachine, RuntimeDeliveryInbox,
    RuntimeDeliveryRefusalReason, SessionServiceRuntimeExt as _,
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
/// committed, fail its reads and refuse its writes.
#[derive(Default)]
struct LossyStore {
    inner: InMemoryApprovalStore,
    lose_next_ack: AtomicBool,
    unreadable: AtomicBool,
    /// While set, every write fails before it can commit.
    unwritable: AtomicBool,
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
        if self.unwritable.load(Ordering::SeqCst) {
            return Err(ApprovalStoreError::NotCommitted(
                "the approval store cannot be written".to_string(),
            ));
        }
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
/// Each admitted resume is held as a session's input ledger holds a
/// governed resume (its exact replay witnesses and the retained work it
/// resumes), so a later drain recognises it as an exact replay. Admitting
/// into a runtime, each resume also becomes a real input of that runtime's
/// session through the runtime's own admission, the one a governed retained
/// resume ends in once its native host granted it. Scripted answers stand
/// in for the runtime's other outcomes.
#[derive(Default)]
struct GovernedSink {
    resumed: Mutex<Vec<(SessionId, RetainedWorkIdentity)>>,
    runtime: Option<Arc<MeerkatMachine>>,
    /// The resumes each session holds, by admission key.
    held: Mutex<HashMap<(SessionId, String), StoredInputState>>,
    /// Answers to the next retained resumes, in order; every later one is
    /// admitted.
    script: Mutex<VecDeque<Scripted>>,
}

/// A scripted answer of the governed sink to one retained resume.
#[derive(Debug, Clone, Copy)]
enum Scripted {
    /// A settled refusal: nothing is admitted.
    Refuse(RuntimeDeliveryRefusalReason),
    /// Not admitted now, retryable: nothing is admitted.
    Fail,
    /// Admitted, then the answer is lost: the session holds the input, and
    /// the drain sees a failure before it applies the admission.
    AdmitThenFail,
}

impl GovernedSink {
    fn admitting_into(runtime: Arc<MeerkatMachine>) -> Self {
        Self {
            runtime: Some(runtime),
            ..Self::default()
        }
    }

    /// This sink, answering its next retained resumes with `script`.
    fn scripted(self, script: impl IntoIterator<Item = Scripted>) -> Self {
        Self {
            script: Mutex::new(script.into_iter().collect()),
            ..self
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
    ) -> Result<Option<StoredInputState>, String> {
        Ok(self
            .held
            .lock()
            .expect("ledger lock")
            .get(&(session.clone(), admission_key.to_string()))
            .cloned())
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
        let scripted = self.script.lock().expect("script lock").pop_front();
        match scripted {
            Some(Scripted::Refuse(reason)) => return Err(ContinuationAdmitError::Refused(reason)),
            Some(Scripted::Fail) => {
                return Err(ContinuationAdmitError::Failed(
                    "the runtime cannot admit the notice now".to_string(),
                ));
            }
            Some(Scripted::AdmitThenFail) | None => {}
        }
        self.resumed
            .lock()
            .expect("resume lock")
            .push((session.clone(), request.identity().clone()));
        let key = input
            .header()
            .idempotency_key
            .as_ref()
            .expect("a continuation is keyed")
            .to_string();
        let mut row = StoredInputState::accepted_for_test(&input).expect("row");
        row.state.retained_resume = Some(meerkat_runtime::retained_work::RetainedResumeRecord {
            identity: request.identity().clone(),
            delivery: request.delivery().clone(),
        });
        let admitted = match &self.runtime {
            None => input.id().clone(),
            Some(runtime) => match runtime
                .accept_input_with_completion(session, input)
                .await
                .map(|(outcome, _completion)| outcome)
            {
                Ok(AcceptOutcome::Accepted { input_id, .. }) => input_id,
                Ok(AcceptOutcome::Deduplicated { existing_id, .. }) => existing_id,
                other => {
                    return Err(ContinuationAdmitError::Failed(format!(
                        "the runtime did not admit the notice: {other:?}"
                    )));
                }
            },
        };
        self.held
            .lock()
            .expect("ledger lock")
            .insert((session.clone(), key), row);
        if matches!(scripted, Some(Scripted::AdmitThenFail)) {
            return Err(ContinuationAdmitError::Failed(
                "the admission's answer was lost".to_string(),
            ));
        }
        Ok(admitted)
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
/// `SessionServiceDeliveryHost::with_consent_notices` composes them. A host
/// that does not project only drains: its owner runs no round, so only a
/// drain's admission report can record a receipt there.
struct ConsentHost {
    projector: Arc<ConsentNoticeProjector>,
    /// Whether the host projects the owed notices on its owner's rounds.
    projects: bool,
    sink: Arc<GovernedSink>,
    /// The committed job owner retained jobs are confirmed with and their
    /// admissions reported to: the projector's own source, or a test's
    /// stand-in for it.
    jobs: Arc<dyn RetainedJobSource>,
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
        Some(Arc::clone(&self.jobs))
    }

    fn projects_owed_continuations(&self) -> bool {
        self.projects
    }

    fn subscribe_owed_continuation_changes(&self) -> Option<watch::Receiver<u64>> {
        self.projects
            .then(|| self.projector.subscribe_owed_changes())
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

    /// [`Self::admitting`] on a persistent host runtime: a session's
    /// graceful requeue stop keeps its eligible queued inputs for the
    /// session's next registration.
    fn admitting_persistent(approvals: ApprovalService, domain: HostingDomainRef) -> Self {
        Self::on_runtime(
            approvals,
            domain,
            MeerkatMachine::persistent_without_blobs(Arc::new(InMemoryRuntimeStore::new()))
                .expect("persistent runtime"),
            GovernedSink::admitting_into,
        )
    }

    fn with_sink(
        approvals: ApprovalService,
        domain: HostingDomainRef,
        sink: impl FnOnce(Arc<MeerkatMachine>) -> GovernedSink,
    ) -> Self {
        Self::on_runtime(approvals, domain, MeerkatMachine::ephemeral(), sink)
    }

    fn on_runtime(
        approvals: ApprovalService,
        domain: HostingDomainRef,
        runtime: MeerkatMachine,
        sink: impl FnOnce(Arc<MeerkatMachine>) -> GovernedSink,
    ) -> Self {
        let approvals = Arc::new(approvals);
        let inbox = RuntimeDeliveryInbox::new(Arc::new(InMemoryRuntimeStore::new()));
        let runtime = Arc::new(runtime);
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
        self.host_with_jobs(Arc::new(self.projector.job_source()))
    }

    /// [`Self::host`], confirming retained jobs, and reporting their
    /// admissions, through `jobs`.
    fn host_with_jobs(&self, jobs: Arc<dyn RetainedJobSource>) -> Arc<dyn RuntimeDeliveryHost> {
        Arc::new(ConsentHost {
            projector: Arc::clone(&self.projector),
            projects: true,
            sink: Arc::clone(&self.sink),
            jobs,
            pages: None,
        })
    }

    /// A host that only drains: it projects no notices, so its owner runs
    /// no round, and it confirms retained jobs, and reports their
    /// admissions, through `jobs`.
    fn draining_host(&self, jobs: Arc<dyn RetainedJobSource>) -> Arc<dyn RuntimeDeliveryHost> {
        Arc::new(ConsentHost {
            projector: Arc::clone(&self.projector),
            projects: false,
            sink: Arc::clone(&self.sink),
            jobs,
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
            projects: true,
            sink: Arc::clone(&self.sink),
            jobs: Arc::new(self.projector.job_source()),
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

/// Where `approval_id`'s `transition` notice stands, as its owner holds it.
fn delivery_state(
    approvals: &ApprovalService,
    approval_id: &ApprovalId,
    transition: ApprovalConsentTransition,
) -> Option<ApprovalConsentDeliveryState> {
    approvals
        .get(approval_id)
        .expect("row")
        .consent
        .and_then(|consent| {
            consent
                .delivery(transition)
                .map(|delivery| delivery.state.clone())
        })
}

/// The cross-run custody `approval_id`'s owner records for its consent.
fn custody(
    approvals: &ApprovalService,
    approval_id: &ApprovalId,
) -> Option<ApprovalConsentCustody> {
    approvals
        .get(approval_id)
        .expect("row")
        .consent
        .and_then(|consent| consent.custody)
}

/// A job owner from before admissions were reported: it confirms the
/// projector's notices exactly as the projector's own source does, and
/// keeps the default report of a committed admission, which records
/// nothing.
struct ConfirmsOnly(ConsentNoticeJobSource);

#[async_trait::async_trait]
impl RetainedJobSource for ConfirmsOnly {
    async fn retained_job(
        &self,
        producer: &meerkat::ContinuationProducer,
        job_id: &str,
    ) -> RetainedJobLookup {
        self.0.retained_job(producer, job_id).await
    }
}

/// The projector's job source, confirming exactly as it does, that fails to
/// hear the admission of the job `job_id`: before it records the receipt (a
/// crash between the admission's commit and its receipt) or after (a crash
/// between the receipt and the row's acknowledgement). Either way that
/// row stays pending, with its admission applied.
struct FailsToHear {
    jobs: ConsentNoticeJobSource,
    job_id: String,
    after_receipt: bool,
}

#[async_trait::async_trait]
impl RetainedJobSource for FailsToHear {
    async fn retained_job(
        &self,
        producer: &meerkat::ContinuationProducer,
        job_id: &str,
    ) -> RetainedJobLookup {
        self.jobs.retained_job(producer, job_id).await
    }

    async fn admission_committed(
        &self,
        producer: &meerkat::ContinuationProducer,
        job_id: &str,
        session: &SessionId,
        input_id: &InputId,
    ) -> Result<(), String> {
        if job_id != self.job_id {
            return self
                .jobs
                .admission_committed(producer, job_id, session, input_id)
                .await;
        }
        if self.after_receipt {
            self.jobs
                .admission_committed(producer, job_id, session, input_id)
                .await?;
        }
        Err("the process ended before the row was acknowledged".to_string())
    }
}

/// One admission reported to a job owner: the job, the session and input it
/// was admitted as, and where the notice stood just before the report
/// reached the approval owner.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Report {
    job_id: String,
    session: SessionId,
    input: InputId,
    before: Option<ApprovalConsentDeliveryState>,
}

/// The projector's job source, confirming and recording exactly as it does,
/// that keeps every admission reported to it.
struct ReportsToOwner {
    jobs: ConsentNoticeJobSource,
    approvals: Arc<ApprovalService>,
    reports: Mutex<Vec<Report>>,
}

impl ReportsToOwner {
    fn new(fixture: &Fixture) -> Self {
        Self {
            jobs: fixture.projector.job_source(),
            approvals: Arc::clone(&fixture.approvals),
            reports: Mutex::default(),
        }
    }

    fn reports(&self) -> Vec<Report> {
        self.reports.lock().expect("report lock").clone()
    }
}

#[async_trait::async_trait]
impl RetainedJobSource for ReportsToOwner {
    async fn retained_job(
        &self,
        producer: &meerkat::ContinuationProducer,
        job_id: &str,
    ) -> RetainedJobLookup {
        self.jobs.retained_job(producer, job_id).await
    }

    async fn admission_committed(
        &self,
        producer: &meerkat::ContinuationProducer,
        job_id: &str,
        session: &SessionId,
        input_id: &InputId,
    ) -> Result<(), String> {
        let before = parse_consent_delivery_job_id(job_id).and_then(|(approval_id, transition)| {
            self.approvals
                .get(&approval_id)
                .ok()?
                .consent?
                .delivery(transition)
                .map(|delivery| delivery.state.clone())
        });
        self.reports.lock().expect("report lock").push(Report {
            job_id: job_id.to_string(),
            session: session.clone(),
            input: input_id.clone(),
            before,
        });
        self.jobs
            .admission_committed(producer, job_id, session, input_id)
            .await
    }
}

/// A notice owed at arming is projected by the arming round and admitted to
/// the exact session of its committed scope, as a retained resume of the
/// admitting run's work, after the approval owner confirmed it. The
/// admission records the notice's receipt, naming that session and input,
/// so nothing stays owed. The new receipt is announced, which starts
/// exactly one more round; that round finds nothing to do, and the owner
/// then stays idle.
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
    // The pass after the arming round is the receipt's own round; the pass
    // channel keeps only the latest, so what each did is read durably.
    let announced = wait_for_pass(&mut passes, "the receipt's round", |pass| {
        pass.generation >= 2
    })
    .await;
    assert!(announced.failures.is_empty(), "{announced:?}");
    assert_eq!(
        fixture.sink.resumed(),
        vec![(session.clone(), origin.clone())],
        "admitted once, to the scope's session, resuming the admitting run's work"
    );
    let input = match fixture.status(&session, &row.approval_id).await {
        ContinuationStatus::Applied {
            session: admitted,
            input,
            ..
        } => {
            assert_eq!(admitted, session);
            input
        }
        other => panic!("the notice was not admitted: {other:?}"),
    };
    assert_eq!(
        delivery_state(
            &fixture.approvals,
            &row.approval_id,
            ApprovalConsentTransition::Approved
        ),
        Some(ApprovalConsentDeliveryState::Admitted {
            session_id: session.clone(),
            input_id: input.to_string(),
        }),
        "the admission recorded the notice's receipt, naming its input"
    );
    assert!(
        fixture
            .approvals
            .owed_consent_deliveries()
            .expect("discovery")
            .owed
            .is_empty(),
        "nothing stays owed"
    );
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
        "no pass repeats anything after the receipt's round"
    );
    assert_eq!(
        passes.borrow().generation,
        2,
        "exactly one round followed the arming round"
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
    // own starts no round. The carried round's discovery reconciles the row
    // and projects its notice, whose admission records the receipt; the
    // admission's status is read, never the row, so only the round
    // reconciles it.
    settlements.send_modify(|generation| *generation += 1);
    tokio::time::timeout(EVENT_GUARD, async {
        while !matches!(
            fixture.status(&session, &approval_id).await,
            ContinuationStatus::Applied { .. }
        ) {
            passes.changed().await.expect("owner pass channel open");
        }
    })
    .await
    .expect("the carried round projected and admitted the notice");
    let input = match fixture.status(&session, &approval_id).await {
        ContinuationStatus::Applied { input, .. } => input,
        other => panic!("the notice was not admitted: {other:?}"),
    };
    assert_eq!(
        delivery_state(
            &fixture.approvals,
            &approval_id,
            ApprovalConsentTransition::Approved
        ),
        Some(ApprovalConsentDeliveryState::Admitted {
            session_id: session.clone(),
            input_id: input.to_string(),
        })
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
    // The source change's round projects the notice, whose admission
    // records its receipt (announced, which starts one more round).
    tokio::time::timeout(EVENT_GUARD, async {
        while !matches!(
            fixture.status(&session, &approval_id).await,
            ContinuationStatus::Applied { .. }
        ) {
            passes.changed().await.expect("owner pass channel open");
        }
    })
    .await
    .expect("the source change round projected and admitted the notice");
    let input = match fixture.status(&session, &approval_id).await {
        ContinuationStatus::Applied { input, .. } => input,
        other => panic!("the notice was not admitted: {other:?}"),
    };
    assert_eq!(
        delivery_state(
            &fixture.approvals,
            &approval_id,
            ApprovalConsentTransition::Approved
        ),
        Some(ApprovalConsentDeliveryState::Admitted {
            session_id: session.clone(),
            input_id: input.to_string(),
        })
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

/// A host's bound job owner that holds no job and keeps every admission
/// reported to it.
#[cfg(feature = "session-store")]
#[derive(Default)]
struct HearsAdmissions {
    heard: Mutex<Vec<(meerkat::ContinuationProducer, String, SessionId, InputId)>>,
}

#[cfg(feature = "session-store")]
impl HearsAdmissions {
    fn heard(&self) -> Vec<(meerkat::ContinuationProducer, String, SessionId, InputId)> {
        self.heard.lock().expect("report lock").clone()
    }
}

#[cfg(feature = "session-store")]
#[async_trait::async_trait]
impl RetainedJobSource for HearsAdmissions {
    async fn retained_job(
        &self,
        _producer: &meerkat::ContinuationProducer,
        _job_id: &str,
    ) -> RetainedJobLookup {
        RetainedJobLookup::Absent
    }

    async fn admission_committed(
        &self,
        producer: &meerkat::ContinuationProducer,
        job_id: &str,
        session: &SessionId,
        input_id: &InputId,
    ) -> Result<(), String> {
        self.heard.lock().expect("report lock").push((
            producer.clone(),
            job_id.to_string(),
            session.clone(),
            input_id.clone(),
        ));
        Ok(())
    }
}

/// The job source a consent-aware session delivery host composes reports a
/// consent notice's committed admission to the notice's approval owner,
/// which records its receipt, and any other job's admission to the host's
/// bound source, exactly as reported; neither reaches the other. Before the
/// host binds, another job's report has no owner here and records nothing.
#[cfg(feature = "session-store")]
#[tokio::test]
async fn the_session_delivery_host_reports_a_consent_notices_admission_to_its_approval_owner() {
    let here = domain("realm-a/hosting");
    let session = SessionId::new();
    let row = approved_owing_row(None, &here, &session, &origin_work("runtime-a"));
    let fixture = Fixture::new(
        ApprovalService::with_store(store_with(std::slice::from_ref(&row))).expect("service"),
        here,
    );
    let temp = tempfile::tempdir().expect("tempdir");
    let service = Arc::new(meerkat::build_ephemeral_service(
        meerkat::AgentFactory::new(temp.path().join("sessions")),
        meerkat::Config::default(),
        4,
    ));
    let bindings = Arc::new(ContinuationHostBindings::default());
    let host = meerkat::surface::SessionServiceDeliveryHost::new(
        &service,
        &fixture.runtime,
        meerkat::DetachedJobService::new(Arc::new(MemoryDetachedJobStore::new())),
        None,
    )
    .with_continuation_bindings(Arc::clone(&bindings))
    .with_consent_notices(Arc::clone(&fixture.projector));
    let approved = ApprovalConsentTransition::Approved;
    let job_id = consent_delivery_job_id(&row.approval_id, approved);
    let notice = consent_notice_producer();
    let fork_off = meerkat::ContinuationProducer::ForkOff;
    let input = InputId::new();
    let admitted = Some(ApprovalConsentDeliveryState::Admitted {
        session_id: session.clone(),
        input_id: input.to_string(),
    });

    let unbound = host
        .retained_job_source()
        .expect("the consent namespace is answered");
    assert_eq!(
        unbound
            .admission_committed(&fork_off, "job-1", &session, &input)
            .await,
        Ok(()),
        "unbound, another job's report has no owner here"
    );
    assert_eq!(
        delivery_state(&fixture.approvals, &row.approval_id, approved),
        Some(ApprovalConsentDeliveryState::Owed),
        "and it records nothing"
    );
    assert!(
        matches!(
            unbound.retained_job(&notice, &job_id).await,
            RetainedJobLookup::Found(_)
        ),
        "the approval owner confirms its notice"
    );
    assert_eq!(
        unbound
            .admission_committed(&notice, &job_id, &session, &input)
            .await,
        Ok(())
    );
    assert_eq!(
        delivery_state(&fixture.approvals, &row.approval_id, approved),
        admitted,
        "the notice's admission recorded its receipt"
    );

    let hears = Arc::new(HearsAdmissions::default());
    let _binding = bindings
        .bind(Arc::new(SessionAddressResolver), hears.clone())
        .expect("bind");
    let bound = host
        .retained_job_source()
        .expect("the consent namespace is answered");
    assert_eq!(
        bound
            .admission_committed(&fork_off, "job-1", &session, &input)
            .await,
        Ok(())
    );
    assert_eq!(
        bound
            .admission_committed(&notice, &job_id, &session, &input)
            .await,
        Ok(())
    );
    assert_eq!(
        hears.heard(),
        vec![(fork_off, "job-1".to_string(), session, input)],
        "the bound source heard the other job's admission once, exactly as reported, and \
         never the notice's"
    );
    assert_eq!(
        delivery_state(&fixture.approvals, &row.approval_id, approved),
        admitted,
        "the same receipt again changes nothing"
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
/// withdrawn by the delivery owner's own rounds, each round woken only by
/// the real signal of the step that makes the settlement possible.
///
/// Real here: the parked custody (issued, approved and parked through the
/// approval owner's own paths), the queued notice input of the host's
/// runtime, the receipt its admission records, the runtime's exact
/// cancellation and the abandoned terminal read back from it, and the wakes
/// (the approval owner's announcement of the receipt, and the runtime's
/// runless-terminal hint through the host bindings). The test never
/// reconciles, never records a receipt, never ticks a source and never
/// polls.
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
/// The admission records the receipt before its row is acknowledged, so the
/// receipt always precedes the cancellation here: the receipt's own round
/// finishes while the input is still queued and settles nothing, so only
/// the cancellation's hint can start the round that settles. It ends with
/// one withdrawal naming the input, the receipt kept, one `Withdrawn` notice
/// delivered once and receipted in turn, and the input abandoned. The other
/// order, an input cancelled before its receipt, has a test of its own.
#[tokio::test]
async fn a_cancelled_notice_input_withdraws_its_parked_consent_after_its_receipt() {
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
    tokio::time::timeout(EVENT_GUARD, async {
        while !matches!(
            fixture.status(&session, &approval_id).await,
            ContinuationStatus::Applied { .. }
        ) {
            passes.changed().await.expect("owner pass channel open");
        }
    })
    .await
    .expect("the arming round admitted the approved notice");
    let notice_input = match fixture.status(&session, &approval_id).await {
        ContinuationStatus::Applied { input, .. } => input,
        other => panic!("the approved notice was not admitted: {other:?}"),
    };
    assert_eq!(
        delivery_state(
            &fixture.approvals,
            &approval_id,
            ApprovalConsentTransition::Approved
        ),
        Some(ApprovalConsentDeliveryState::Admitted {
            session_id: session.clone(),
            input_id: notice_input.to_string(),
        }),
        "the admission recorded the receipt before its row was acknowledged"
    );
    assert_eq!(
        native_terminal(&fixture.runtime, &session, &notice_input).await,
        None,
        "no executor serves the session: the notice input stays queued"
    );
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

    // The receipt's own round finishes while the input is still queued.
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

    // The owner's own rounds settle the parked consent and then deliver its
    // owed Withdrawn notice.
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
    .expect("the Withdrawn notice was delivered");
    let withdrawn_input = match fixture
        .status_of(&session, &approval_id, ApprovalConsentTransition::Withdrawn)
        .await
    {
        ContinuationStatus::Applied { input, .. } => input,
        other => panic!("the Withdrawn notice was not admitted: {other:?}"),
    };

    assert_eq!(
        withdrawn(),
        Some(ApprovalConsentWithdrawalReason::NoticeInputEnded {
            input_id: notice_input.to_string(),
        })
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
                ApprovalConsentDeliveryState::Admitted {
                    session_id: session.clone(),
                    input_id: withdrawn_input.to_string(),
                },
            ),
        ],
        "the exact receipt naming the input stays, and the withdrawal's own notice is \
         receipted once it is admitted"
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

/// Parked consent whose admitted notice input the runtime cancels before
/// any receipt names it is withdrawn once the late receipt lands. The first
/// owner's admission commits, but the owner ends before it records the
/// receipt (as a crash would), so the row stays unacknowledged. The runtime
/// then cancels the queued input while no receipt names it, so nothing can
/// withdraw the consent yet. The next owner records the receipt (its arming
/// round reads the applied admission from the row, and its drain reports
/// the same admission again), and its own rounds, woken by the receipt's
/// announcement, read the input's abandoned terminal and withdraw the
/// consent, naming the input. It ends as the other order does: the receipt
/// kept, one `Withdrawn` notice delivered once and receipted in turn, and
/// the input abandoned. Nothing records a receipt by hand.
#[tokio::test]
async fn a_notice_input_cancelled_before_its_receipt_withdraws_its_parked_consent_once_receipted() {
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
    let approved = ApprovalConsentTransition::Approved;

    let first = fixture
        .owner()
        .arm(fixture.host_with_jobs(Arc::new(FailsToHear {
            jobs: fixture.projector.job_source(),
            job_id: consent_delivery_job_id(&approval_id, approved),
            after_receipt: false,
        })))
        .expect("arm the first owner");
    let mut first_passes = first.subscribe_passes();
    let notice_input = tokio::time::timeout(EVENT_GUARD, async {
        loop {
            if let ContinuationStatus::Pending {
                admitted: Some((admitted, input)),
                ..
            } = fixture.status(&session, &approval_id).await
            {
                assert_eq!(admitted, session);
                return input;
            }
            first_passes
                .changed()
                .await
                .expect("owner pass channel open");
        }
    })
    .await
    .expect("the first owner committed the admission");
    let _stopped = first
        .retire()
        .join(EVENT_GUARD)
        .await
        .expect("the first owner stops");
    assert_eq!(
        delivery_state(&fixture.approvals, &approval_id, approved),
        Some(ApprovalConsentDeliveryState::Owed),
        "no receipt yet"
    );
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
    assert_eq!(withdrawn(), None, "no receipt names the input yet");
    assert_eq!(
        custody(&fixture.approvals, &approval_id),
        Some(ApprovalConsentCustody::Parked),
        "still parked"
    );

    // The next owner records the late receipt, and its own rounds settle
    // the parked consent and then deliver its owed Withdrawn notice.
    let next = fixture
        .owner()
        .arm(fixture.host())
        .expect("arm the next owner over the same inbox");
    let mut passes = next.subscribe_passes();
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
    .expect("the Withdrawn notice was delivered");
    let withdrawn_input = match fixture
        .status_of(&session, &approval_id, ApprovalConsentTransition::Withdrawn)
        .await
    {
        ContinuationStatus::Applied { input, .. } => input,
        other => panic!("the Withdrawn notice was not admitted: {other:?}"),
    };

    assert_eq!(
        withdrawn(),
        Some(ApprovalConsentWithdrawalReason::NoticeInputEnded {
            input_id: notice_input.to_string(),
        })
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
                ApprovalConsentDeliveryState::Admitted {
                    session_id: session.clone(),
                    input_id: withdrawn_input.to_string(),
                },
            ),
        ],
        "the late receipt names the cancelled input, and the withdrawal's own notice is \
         receipted once it is admitted"
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

/// An approved parked consent moves into the resumed run its own notice
/// starts, exactly once, and is spent there once.
///
/// Real here: two consents of one origin work and destination, for the same
/// operation with identical arguments, each issued, approved and parked
/// through the approval owner's own paths; the delivery owner that projects
/// both approved notices and admits them as real inputs of the host's
/// runtime; and the receipts those admissions record. The first owner loses
/// the acknowledgement of the consent's own admission after its receipt, as
/// a crash would, so that row stays pending.
///
/// A resumed run of the origin work staged with the consent's receipt input
/// takes the consent over (its custody names that input), and the consent
/// is spent there once; a second spend is refused. The twin's receipt names
/// its own input, which that run does not carry, so the twin stays parked.
/// The next owner's arming drain finds the consent's admission already
/// applied and reports it again: the same receipt, which writes nothing, so
/// the consent neither moves nor is spent a second time, and each notice
/// stays admitted once.
#[tokio::test]
async fn an_approved_parked_consent_moves_once_into_the_resumed_run_its_notice_starts() {
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
    let consent = ParkedRoutedConsent::park(&fixture.approvals, scope.clone(), origin.clone())
        .await
        .expect("the consent is parked through the owner's paths");
    let twin = ParkedRoutedConsent::park(&fixture.approvals, scope, origin.clone())
        .await
        .expect("its twin is parked through the owner's paths");
    let approved = ApprovalConsentTransition::Approved;
    let consent_id = consent.approval_id().clone();
    let twin_id = twin.approval_id().clone();

    let first = fixture
        .owner()
        .arm(fixture.host_with_jobs(Arc::new(FailsToHear {
            jobs: fixture.projector.job_source(),
            job_id: consent_delivery_job_id(&consent_id, approved),
            after_receipt: true,
        })))
        .expect("arm the first owner");
    let mut first_passes = first.subscribe_passes();
    tokio::time::timeout(EVENT_GUARD, async {
        while delivery_state(&fixture.approvals, &consent_id, approved)
            == Some(ApprovalConsentDeliveryState::Owed)
        {
            first_passes
                .changed()
                .await
                .expect("owner pass channel open");
        }
    })
    .await
    .expect("the first owner's admission recorded the consent's receipt");
    let notice_input = match fixture.status(&session, &consent_id).await {
        ContinuationStatus::Pending {
            admitted: Some((admitted, input)),
            ..
        } => {
            assert_eq!(admitted, session);
            input
        }
        other => panic!("the admission is applied and its row unacknowledged: {other:?}"),
    };
    assert_eq!(
        delivery_state(&fixture.approvals, &consent_id, approved),
        Some(ApprovalConsentDeliveryState::Admitted {
            session_id: session.clone(),
            input_id: notice_input.to_string(),
        }),
        "the receipt names the admitted input"
    );

    let run = consent.open_resumed_run(std::slice::from_ref(&notice_input));
    assert_eq!(
        custody(&fixture.approvals, &consent_id),
        Some(ApprovalConsentCustody::Transferred {
            input_id: notice_input.to_string(),
        }),
        "the consent moved into the run staged with its receipt's input"
    );
    assert_eq!(
        custody(&fixture.approvals, &twin_id),
        Some(ApprovalConsentCustody::Parked),
        "the twin did not move"
    );
    assert_eq!(consent.spend_in(&run), Ok(()), "spent in the resumed run");
    assert_eq!(
        consent.spend_in(&run),
        Err(meerkat_core::ToolError::ReviewUnsatisfied {
            kind: meerkat_core::ReviewUnsatisfiedKind::AlreadyConsumed,
            approval_id: None,
        }),
        "a second spend is refused"
    );
    assert_eq!(
        consent
            .notices()
            .iter()
            .filter(|kind| **kind == ConsentNoticeKind::Used)
            .count(),
        1,
        "used once"
    );

    let _stopped = first
        .retire()
        .join(EVENT_GUARD)
        .await
        .expect("the first owner stops");
    let settled = fixture.approvals.get(&consent_id).expect("row");
    assert!(
        settled
            .consent
            .as_ref()
            .and_then(|consent| consent.consumed_at)
            .is_some(),
        "the spend is recorded"
    );
    let next = fixture
        .owner()
        .arm(fixture.host())
        .expect("arm the next owner over the same inbox");
    let mut passes = next.subscribe_passes();
    tokio::time::timeout(EVENT_GUARD, async {
        loop {
            let consent_applied = matches!(
                fixture.status(&session, &consent_id).await,
                ContinuationStatus::Applied { .. }
            );
            let twin_applied = matches!(
                fixture.status(&session, &twin_id).await,
                ContinuationStatus::Applied { .. }
            );
            if consent_applied && twin_applied {
                break;
            }
            passes.changed().await.expect("owner pass channel open");
        }
    })
    .await
    .expect("the next owner acknowledged both notices");
    match fixture.status(&session, &consent_id).await {
        ContinuationStatus::Applied { input, .. } => {
            assert_eq!(input, notice_input, "the same admission, reported again");
        }
        other => panic!("the consent's notice was not acknowledged: {other:?}"),
    }
    assert_eq!(
        fixture.approvals.get(&consent_id).expect("row"),
        settled,
        "the repeated receipt wrote nothing: no second move and no second spend"
    );
    let twin_input = match fixture.status(&session, &twin_id).await {
        ContinuationStatus::Applied { input, .. } => input,
        other => panic!("the twin's notice was not admitted: {other:?}"),
    };
    assert_ne!(twin_input, notice_input);
    assert_eq!(
        delivery_state(&fixture.approvals, &twin_id, approved),
        Some(ApprovalConsentDeliveryState::Admitted {
            session_id: session.clone(),
            input_id: twin_input.to_string(),
        })
    );
    assert_eq!(
        custody(&fixture.approvals, &twin_id),
        Some(ApprovalConsentCustody::Parked),
        "the twin, with identical arguments, stays parked: its receipt names another input"
    );
    assert!(
        fixture
            .approvals
            .get(&twin_id)
            .expect("row")
            .consent
            .and_then(|consent| consent.consumed_at)
            .is_none(),
        "the twin is not spent"
    );
    assert_eq!(
        fixture.sink.resumed(),
        vec![(session.clone(), origin.clone()), (session.clone(), origin)],
        "each notice admitted once"
    );
    let held = fixture
        .runtime
        .list_active_inputs(&session)
        .await
        .expect("the runtime's inputs");
    assert!(
        held.len() == 2 && held.contains(&notice_input) && held.contains(&twin_input),
        "the runtime holds each notice's one input: {held:?}"
    );
}

/// An approved parked consent's notice survives a graceful requeue stop of
/// its session. The notice keeps the queue handling its delivery declares,
/// so the session's requeue unregister keeps the queued notice input
/// durably for the next start instead of ending it. The session's next
/// registration in the same process holds the input still queued; the next
/// delivery owner's arming round reads it and withdraws nothing (an ended
/// notice input would withdraw the consent as `NoticeInputEnded`); and the
/// resumed run the notice starts takes the consent over and spends it once.
///
/// Real here: the parked custody, through the approval owner's own paths;
/// the notice admitted into a persistent host runtime as a real input; the
/// runtime's requeue unregister and the session's next registration; the
/// delivery owner's round that reads the input's terminal. As in the other
/// parked-consent rows, no executor serves the session, and the governed
/// sink admits through the runtime's direct admission.
#[tokio::test]
async fn a_requeue_stop_keeps_an_approved_consents_notice_and_its_consent() {
    let here = domain("realm-a/hosting");
    let session = SessionId::new();
    let origin = origin_work("runtime-a");
    let fixture = Fixture::admitting_persistent(
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
    let consent = ParkedRoutedConsent::park(&fixture.approvals, scope, origin)
        .await
        .expect("the consent is parked through the owner's paths");
    let approval_id = consent.approval_id().clone();

    let first = fixture
        .owner()
        .arm(fixture.host())
        .expect("arm the first owner");
    let mut passes = first.subscribe_passes();
    tokio::time::timeout(EVENT_GUARD, async {
        while !matches!(
            fixture.status(&session, &approval_id).await,
            ContinuationStatus::Applied { .. }
        ) {
            passes.changed().await.expect("owner pass channel open");
        }
    })
    .await
    .expect("the arming round admitted the approved notice");
    let notice_input = match fixture.status(&session, &approval_id).await {
        ContinuationStatus::Applied { input, .. } => input,
        other => panic!("the approved notice was not admitted: {other:?}"),
    };
    assert_eq!(
        delivery_state(
            &fixture.approvals,
            &approval_id,
            ApprovalConsentTransition::Approved
        ),
        Some(ApprovalConsentDeliveryState::Admitted {
            session_id: session.clone(),
            input_id: notice_input.to_string(),
        }),
        "the receipt names the admitted input"
    );
    let _stopped = first
        .retire()
        .join(EVENT_GUARD)
        .await
        .expect("the first owner stops");

    // The session's graceful requeue stop, then its next registration in
    // this process.
    let registration = fixture
        .runtime
        .current_session_registration_witness(&session)
        .await
        .expect("the session is registered");
    assert!(
        fixture
            .runtime
            .unregister_session_registration_until_terminal_with_queued_inputs_if_current(
                &registration,
                meerkat_runtime::UnregisterQueuedInputs::Requeue {
                    max_age: Duration::from_secs(600),
                },
            )
            .await
            .expect("the requeue stop completes"),
        "the exact registration is torn down"
    );
    fixture
        .runtime
        .register_session(session.clone())
        .await
        .expect("the session registers again");
    assert_eq!(
        native_terminal(&fixture.runtime, &session, &notice_input).await,
        None,
        "the requeue stop kept the notice input queued for the next start"
    );

    // The next owner's arming round reads the input and withdraws nothing.
    let next = fixture
        .owner()
        .arm(fixture.host())
        .expect("arm the next owner");
    let mut passes = next.subscribe_passes();
    let armed = wait_for_pass(&mut passes, "the arming round", |pass| pass.generation >= 1).await;
    assert!(armed.failures.is_empty(), "{armed:?}");
    assert_eq!(
        fixture
            .approvals
            .get(&approval_id)
            .expect("row")
            .consent
            .and_then(|consent| consent.withdrawal),
        None,
        "nothing withdrew the consent"
    );
    assert_eq!(
        custody(&fixture.approvals, &approval_id),
        Some(ApprovalConsentCustody::Parked)
    );

    // The resumed run its notice starts takes the consent over and spends
    // it once.
    let run = consent.open_resumed_run(std::slice::from_ref(&notice_input));
    assert_eq!(
        custody(&fixture.approvals, &approval_id),
        Some(ApprovalConsentCustody::Transferred {
            input_id: notice_input.to_string(),
        }),
        "the consent moved into the run staged with its notice's input"
    );
    assert_eq!(consent.spend_in(&run), Ok(()), "spent in the resumed run");
    assert_eq!(
        consent.spend_in(&run),
        Err(meerkat_core::ToolError::ReviewUnsatisfied {
            kind: meerkat_core::ReviewUnsatisfiedKind::AlreadyConsumed,
            approval_id: None,
        }),
        "a second spend is refused"
    );
}

/// A crash between a notice's admission and its receipt loses nothing. The
/// admission commits, and the row stays unacknowledged. The next owner only
/// drains (its host projects no notices, so no round can record the receipt
/// from the row first): its arming drain finds the admission already
/// applied and reports it once, naming that same input while the notice is
/// still owed, and that report records the receipt; nothing is admitted
/// again. Nothing records a receipt by hand and no other wake is sent. A
/// projecting owner's round would record the same receipt from the row
/// first, as an admission acknowledged without its receipt shows.
#[tokio::test]
async fn a_crash_between_a_notices_admission_and_its_receipt_is_receipted_by_the_next_drain() {
    let here = domain("realm-a/hosting");
    let session = SessionId::new();
    let origin = origin_work("runtime-a");
    let row = approved_owing_row(None, &here, &session, &origin);
    let fixture = Fixture::admitting(
        ApprovalService::with_store(store_with(std::slice::from_ref(&row))).expect("service"),
        here,
    );
    fixture
        .runtime
        .register_session(session.clone())
        .await
        .expect("register the destination session");
    let approved = ApprovalConsentTransition::Approved;

    let first = fixture
        .owner()
        .arm(fixture.host_with_jobs(Arc::new(FailsToHear {
            jobs: fixture.projector.job_source(),
            job_id: consent_delivery_job_id(&row.approval_id, approved),
            after_receipt: false,
        })))
        .expect("arm the first owner");
    let mut first_passes = first.subscribe_passes();
    let input = tokio::time::timeout(EVENT_GUARD, async {
        loop {
            if let ContinuationStatus::Pending {
                admitted: Some((admitted, input)),
                ..
            } = fixture.status(&session, &row.approval_id).await
            {
                assert_eq!(admitted, session);
                return input;
            }
            first_passes
                .changed()
                .await
                .expect("owner pass channel open");
        }
    })
    .await
    .expect("the first owner committed the admission");
    let _stopped = first
        .retire()
        .join(EVENT_GUARD)
        .await
        .expect("the first owner stops");
    assert_eq!(
        delivery_state(&fixture.approvals, &row.approval_id, approved),
        Some(ApprovalConsentDeliveryState::Owed),
        "no receipt yet"
    );

    let reports = Arc::new(ReportsToOwner::new(&fixture));
    let next = fixture
        .owner()
        .arm(fixture.draining_host(reports.clone()))
        .expect("arm the next owner over the same inbox");
    let mut passes = next.subscribe_passes();
    tokio::time::timeout(EVENT_GUARD, async {
        while !matches!(
            fixture.status(&session, &row.approval_id).await,
            ContinuationStatus::Applied { .. }
        ) {
            passes.changed().await.expect("owner pass channel open");
        }
    })
    .await
    .expect("the next owner acknowledged the row");
    match fixture.status(&session, &row.approval_id).await {
        ContinuationStatus::Applied { input: applied, .. } => {
            assert_eq!(applied, input, "the first owner's admission");
        }
        other => panic!("the row was not acknowledged: {other:?}"),
    }
    assert_eq!(
        reports.reports(),
        vec![Report {
            job_id: consent_delivery_job_id(&row.approval_id, approved),
            session: session.clone(),
            input: input.clone(),
            before: Some(ApprovalConsentDeliveryState::Owed),
        }],
        "the drain reported the first owner's admission once, while the notice was owed"
    );
    assert_eq!(
        delivery_state(&fixture.approvals, &row.approval_id, approved),
        Some(ApprovalConsentDeliveryState::Admitted {
            session_id: session.clone(),
            input_id: input.to_string(),
        }),
        "that report recorded the receipt naming the first owner's input"
    );
    assert!(
        fixture
            .approvals
            .owed_consent_deliveries()
            .expect("discovery")
            .owed
            .is_empty(),
        "nothing stays owed"
    );
    assert_eq!(
        fixture.sink.resumed(),
        vec![(session.clone(), origin)],
        "admitted once"
    );
    assert_eq!(
        fixture
            .runtime
            .list_active_inputs(&session)
            .await
            .expect("the runtime's inputs"),
        vec![input],
        "the runtime holds the one input"
    );
}

/// An admission that reached its session before its answer was lost is an
/// exact replay to the next drain: it finds the reservation and the exact
/// input the session holds, applies that input without admitting anything
/// again, and records the receipt naming it.
#[tokio::test]
async fn an_exact_replay_of_a_notices_admission_is_receipted_naming_the_held_input() {
    let here = domain("realm-a/hosting");
    let session = SessionId::new();
    let origin = origin_work("runtime-a");
    let row = approved_owing_row(None, &here, &session, &origin);
    let fixture = Fixture::with_sink(
        ApprovalService::with_store(store_with(std::slice::from_ref(&row))).expect("service"),
        here,
        |runtime| GovernedSink::admitting_into(runtime).scripted([Scripted::AdmitThenFail]),
    );
    fixture
        .runtime
        .register_session(session.clone())
        .await
        .expect("register the destination session");
    let approved = ApprovalConsentTransition::Approved;
    let (settlements, run_settlements) = watch::channel(0_u64);
    let handle = fixture
        .owner()
        .with_run_settlements(run_settlements)
        .arm(fixture.host())
        .expect("arm owner");
    let mut passes = handle.subscribe_passes();
    let armed = wait_for_pass(&mut passes, "the arming round", |pass| pass.generation >= 1).await;
    assert!(
        !armed.blocked_runtimes.is_empty(),
        "the lost answer leaves the row pending: {armed:?}"
    );
    assert!(
        matches!(
            fixture.status(&session, &row.approval_id).await,
            ContinuationStatus::Pending { admitted: None, .. }
        ),
        "reserved, not applied"
    );
    let held = fixture
        .runtime
        .list_active_inputs(&session)
        .await
        .expect("the runtime's inputs");
    let [input] = held.as_slice() else {
        panic!("the session holds the one admitted input: {held:?}");
    };
    let input = input.clone();
    assert_eq!(
        delivery_state(&fixture.approvals, &row.approval_id, approved),
        Some(ApprovalConsentDeliveryState::Owed)
    );

    // Any wake retries the blocked row: here a run settlement.
    settlements.send_modify(|generation| *generation += 1);
    tokio::time::timeout(EVENT_GUARD, async {
        while !matches!(
            fixture.status(&session, &row.approval_id).await,
            ContinuationStatus::Applied { .. }
        ) {
            passes.changed().await.expect("owner pass channel open");
        }
    })
    .await
    .expect("the next drain applied the exact replay");
    match fixture.status(&session, &row.approval_id).await {
        ContinuationStatus::Applied { input: applied, .. } => {
            assert_eq!(applied, input, "the exact input the session already held");
        }
        other => panic!("the row was not acknowledged: {other:?}"),
    }
    assert_eq!(
        delivery_state(&fixture.approvals, &row.approval_id, approved),
        Some(ApprovalConsentDeliveryState::Admitted {
            session_id: session.clone(),
            input_id: input.to_string(),
        }),
        "the receipt names the held input"
    );
    assert_eq!(
        fixture.sink.resumed(),
        vec![(session.clone(), origin)],
        "admitted once"
    );
    assert_eq!(
        fixture
            .runtime
            .list_active_inputs(&session)
            .await
            .expect("the runtime's inputs"),
        vec![input],
        "nothing admitted again"
    );
}

/// A notice admitted and acknowledged without a receipt, by an owner from
/// before admissions were reported (a job source that keeps the default
/// report), is not lost: the next owner's arming round reads the row's
/// applied admission and records the receipt naming that input, admitting
/// nothing again.
#[tokio::test(start_paused = true)]
async fn an_admission_acknowledged_without_its_receipt_is_receipted_from_its_row() {
    let here = domain("realm-a/hosting");
    let session = SessionId::new();
    let origin = origin_work("runtime-a");
    let row = approved_owing_row(None, &here, &session, &origin);
    let fixture = Fixture::new(
        ApprovalService::with_store(store_with(std::slice::from_ref(&row))).expect("service"),
        here,
    );
    let approved = ApprovalConsentTransition::Approved;

    let earlier = fixture
        .owner()
        .arm(fixture.host_with_jobs(Arc::new(ConfirmsOnly(fixture.projector.job_source()))))
        .expect("arm the earlier owner");
    let mut earlier_passes = earlier.subscribe_passes();
    let armed = wait_for_pass(
        &mut earlier_passes,
        "the earlier owner's arming round",
        |pass| pass.generation >= 1,
    )
    .await;
    assert_eq!(
        (armed.owed_projected, armed.applied, armed.failures.len()),
        (1, 1, 0),
        "{armed:?}"
    );
    let input = match fixture.status(&session, &row.approval_id).await {
        ContinuationStatus::Applied { input, .. } => input,
        other => panic!("the notice was not admitted: {other:?}"),
    };
    assert_eq!(
        delivery_state(&fixture.approvals, &row.approval_id, approved),
        Some(ApprovalConsentDeliveryState::Owed),
        "acknowledged with no receipt"
    );
    let _stopped = earlier
        .retire()
        .join(EVENT_GUARD)
        .await
        .expect("the earlier owner stops");

    let next = fixture
        .owner()
        .arm(fixture.host())
        .expect("arm the next owner over the same inbox");
    let mut passes = next.subscribe_passes();
    tokio::time::timeout(EVENT_GUARD, async {
        while delivery_state(&fixture.approvals, &row.approval_id, approved)
            == Some(ApprovalConsentDeliveryState::Owed)
        {
            passes.changed().await.expect("owner pass channel open");
        }
    })
    .await
    .expect("the next owner's arming round recorded the receipt");
    assert_eq!(
        delivery_state(&fixture.approvals, &row.approval_id, approved),
        Some(ApprovalConsentDeliveryState::Admitted {
            session_id: session.clone(),
            input_id: input.to_string(),
        }),
        "the receipt names the input the row records"
    );
    assert!(
        fixture
            .approvals
            .owed_consent_deliveries()
            .expect("discovery")
            .owed
            .is_empty(),
        "nothing stays owed"
    );
    assert_eq!(
        fixture.sink.resumed(),
        vec![(session, origin)],
        "admitted once"
    );
}

/// A notice whose delivery the drain settled as refused can never be
/// delivered: the round that refusing pass starts records its `Failed`
/// receipt with the refusal, which withdraws the approved parked consent.
/// The withdrawal owes its own notice, which is then delivered and
/// receipted in turn.
#[tokio::test(start_paused = true)]
async fn a_refused_approved_notice_is_receipted_failed_and_its_parked_consent_withdrawn() {
    let here = domain("realm-a/hosting");
    let session = SessionId::new();
    let origin = origin_work("runtime-a");
    let fixture = Fixture::with_sink(
        ApprovalService::with_store(store_with(&[])).expect("service"),
        here.clone(),
        |_| {
            GovernedSink::default().scripted([Scripted::Refuse(
                RuntimeDeliveryRefusalReason::AuthorityDenied,
            )])
        },
    );
    let scope = ApprovalConsentScope {
        domain: here,
        session_id: session.clone(),
    };
    let parked = ParkedRoutedConsent::park(&fixture.approvals, scope, origin.clone())
        .await
        .expect("a routed consent parked through the owner's paths");
    let approval_id = parked.approval_id().clone();

    let handle = fixture.owner().arm(fixture.host()).expect("arm owner");
    let mut passes = handle.subscribe_passes();
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
    .expect("the withdrawal's notice was delivered");
    assert!(
        matches!(
            fixture.status(&session, &approval_id).await,
            ContinuationStatus::Refused {
                reason: RuntimeDeliveryRefusalReason::AuthorityDenied,
                ..
            }
        ),
        "the approved notice was settled as refused"
    );
    match delivery_state(
        &fixture.approvals,
        &approval_id,
        ApprovalConsentTransition::Approved,
    ) {
        Some(ApprovalConsentDeliveryState::Failed { reason }) => {
            assert!(reason.contains("AuthorityDenied"), "{reason}");
        }
        other => panic!("the approved notice was not receipted as failed: {other:?}"),
    }
    assert_eq!(
        fixture
            .approvals
            .get(&approval_id)
            .expect("row")
            .consent
            .and_then(|consent| consent.withdrawal)
            .map(|withdrawal| withdrawal.reason),
        Some(ApprovalConsentWithdrawalReason::RunEnded),
        "the undeliverable notice withdrew the parked consent"
    );
    let withdrawn_input = match fixture
        .status_of(&session, &approval_id, ApprovalConsentTransition::Withdrawn)
        .await
    {
        ContinuationStatus::Applied { input, .. } => input,
        other => panic!("the Withdrawn notice was not admitted: {other:?}"),
    };
    assert_eq!(
        delivery_state(
            &fixture.approvals,
            &approval_id,
            ApprovalConsentTransition::Withdrawn
        ),
        Some(ApprovalConsentDeliveryState::Admitted {
            session_id: session.clone(),
            input_id: withdrawn_input.to_string(),
        }),
        "the withdrawal's notice is receipted once it is admitted"
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
        vec![(session, origin)],
        "only the withdrawal's notice was admitted"
    );
}

/// A row still waiting for its admission proves no receipt: its admission
/// reserved an input the runtime did not take (a retryable failure), so a
/// round reading it records nothing, and never `Failed`.
#[tokio::test(start_paused = true)]
async fn a_notice_not_admitted_yet_records_no_receipt() {
    let here = domain("realm-a/hosting");
    let session = SessionId::new();
    let origin = origin_work("runtime-a");
    let row = approved_owing_row(None, &here, &session, &origin);
    let fixture = Fixture::with_sink(
        ApprovalService::with_store(store_with(std::slice::from_ref(&row))).expect("service"),
        here,
        |_| GovernedSink::default().scripted([Scripted::Fail]),
    );
    let handle = fixture.owner().arm(fixture.host()).expect("arm owner");
    let mut passes = handle.subscribe_passes();
    let armed = wait_for_pass(&mut passes, "the arming round", |pass| pass.generation >= 1).await;
    assert!(!armed.blocked_runtimes.is_empty(), "{armed:?}");
    let _stopped = handle
        .retire()
        .join(EVENT_GUARD)
        .await
        .expect("the owner stops");
    assert!(
        matches!(
            fixture.status(&session, &row.approval_id).await,
            ContinuationStatus::Pending { admitted: None, .. }
        ),
        "reserved, not applied"
    );

    let page = fixture.projector.project(None, 8).await;
    assert_eq!(
        (page.committed, page.failures.len()),
        (0, 0),
        "a committed row counts nothing: {page:?}"
    );
    assert_eq!(
        delivery_state(
            &fixture.approvals,
            &row.approval_id,
            ApprovalConsentTransition::Approved
        ),
        Some(ApprovalConsentDeliveryState::Owed),
        "nothing is recorded, and never Failed"
    );
    assert!(fixture.sink.resumed().is_empty(), "nothing was admitted");
}

/// A notice whose key is bound to other content has no status a round can
/// read for it: the observation fails, the page reports it, and nothing is
/// recorded, never `Failed`.
#[tokio::test(start_paused = true)]
async fn a_notice_whose_key_holds_other_content_records_no_receipt() {
    let here = domain("realm-a/hosting");
    let session = SessionId::new();
    let origin = origin_work("runtime-a");
    let row = approved_owing_row(None, &here, &session, &origin);
    let fixture = Fixture::new(
        ApprovalService::with_store(store_with(std::slice::from_ref(&row))).expect("service"),
        here,
    );
    let approved = ApprovalConsentTransition::Approved;
    let job_id = consent_delivery_job_id(&row.approval_id, approved);
    let facts = fixture
        .approvals
        .consent_delivery(&row.approval_id, approved)
        .expect("read the owner")
        .expect("the notice is owed");
    let record = meerkat::RetainedJobRecord::from_owner(
        &fixture.projector.job_source(),
        consent_notice_producer(),
        job_id.clone(),
    )
    .await
    .expect("read the owner")
    .expect("the owner holds the notice");
    fixture
        .continuations
        .submit_retained_completion(
            &ContinuationOwner::Session {
                session_id: session.clone(),
            },
            meerkat::ContinuationDelivery {
                key: ContinuationKey::new(format!("consent:{job_id}")).expect("key"),
                result: meerkat::ContinuationResultRef {
                    producer: consent_notice_producer(),
                    producer_id: job_id.clone(),
                    result_digest: facts.result_digest,
                    summary: None,
                },
                body: "other content".into(),
                handling: meerkat::ContinuationHandling::Queue,
            },
            &record,
            1,
        )
        .await
        .expect("other content commits under the notice's key");

    let page = fixture.projector.project(None, 8).await;
    assert_eq!(page.committed, 0, "{page:?}");
    assert!(
        page.failures
            .iter()
            .any(|failure| failure.contains(&job_id)),
        "the observation's failure is reported: {page:?}"
    );
    assert_eq!(
        delivery_state(&fixture.approvals, &row.approval_id, approved),
        Some(ApprovalConsentDeliveryState::Owed),
        "nothing is recorded, and never Failed"
    );
}

/// An approval owner that cannot record a receipt now loses nothing: the
/// admission stays applied with its row unacknowledged and nothing is
/// recorded (the notice is still owed, never `Failed`); once the owner can
/// record again, the next drain reports the same admission and records its
/// receipt.
#[tokio::test(start_paused = true)]
async fn an_owner_that_cannot_record_a_receipt_keeps_the_admission_until_it_can() {
    let here = domain("realm-a/hosting");
    let session = SessionId::new();
    let origin = origin_work("runtime-a");
    let row = approved_owing_row(None, &here, &session, &origin);
    let store = Arc::new(LossyStore::default());
    store.inner.put_if_current(None, &row).expect("store row");
    let fixture = Fixture::new(
        ApprovalService::with_store(store.clone()).expect("service"),
        here,
    );
    let approved = ApprovalConsentTransition::Approved;
    store.unwritable.store(true, Ordering::SeqCst);
    let (settlements, run_settlements) = watch::channel(0_u64);
    let handle = fixture
        .owner()
        .with_run_settlements(run_settlements)
        .arm(fixture.host())
        .expect("arm owner");
    let mut passes = handle.subscribe_passes();
    let armed = wait_for_pass(&mut passes, "the arming round", |pass| pass.generation >= 1).await;
    assert!(
        !armed.blocked_runtimes.is_empty(),
        "the row stays pending: {armed:?}"
    );
    let input = match fixture.status(&session, &row.approval_id).await {
        ContinuationStatus::Pending {
            admitted: Some((admitted, input)),
            ..
        } => {
            assert_eq!(admitted, session);
            input
        }
        other => panic!("the admission is applied and its row unacknowledged: {other:?}"),
    };
    assert_eq!(
        delivery_state(&fixture.approvals, &row.approval_id, approved),
        Some(ApprovalConsentDeliveryState::Owed),
        "nothing is recorded, and never Failed"
    );

    store.unwritable.store(false, Ordering::SeqCst);
    // Any wake retries the blocked row: here a run settlement.
    settlements.send_modify(|generation| *generation += 1);
    tokio::time::timeout(EVENT_GUARD, async {
        while !matches!(
            fixture.status(&session, &row.approval_id).await,
            ContinuationStatus::Applied { .. }
        ) {
            passes.changed().await.expect("owner pass channel open");
        }
    })
    .await
    .expect("the next drain acknowledged the row");
    assert_eq!(
        delivery_state(&fixture.approvals, &row.approval_id, approved),
        Some(ApprovalConsentDeliveryState::Admitted {
            session_id: session.clone(),
            input_id: input.to_string(),
        }),
        "the receipt names the admission the row kept"
    );
    assert_eq!(
        fixture.sink.resumed(),
        vec![(session, origin)],
        "admitted once"
    );
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

    // The host binds its services; their owner holds neither now. A pass
    // that settles a refusal starts one more round of the host's owed
    // notices, so the refusing pass is read durably, not as the latest pass.
    let _binding = bindings
        .bind(
            Arc::new(SessionAddressResolver),
            Arc::new(Owner {
                original: None,
                job: None,
            }),
        )
        .expect("bind");
    tokio::time::timeout(EVENT_GUARD, async {
        loop {
            let mut refused = 0;
            for (owner, key) in [
                (&original_owner, "ingest:observation-1"),
                (&fork_owner, "fork_off:job-1"),
            ] {
                let status = continuations
                    .continuation_status(owner, &ContinuationKey::new(key).expect("key"))
                    .await
                    .expect("status");
                if matches!(status, ContinuationStatus::Refused { .. }) {
                    refused += 1;
                }
            }
            if refused == 2 {
                break;
            }
            passes.changed().await.expect("owner pass channel open");
        }
    })
    .await
    .expect("both decided by the bound owner");
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
