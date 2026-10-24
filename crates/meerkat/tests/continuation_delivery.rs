#![allow(clippy::expect_used, clippy::panic)]
//! Continuation rows through the library delivery owner: admission exactly
//! once, the reserve-first crash-cut matrix on the memory and SQLite stores
//! (admitted before the acknowledgement, reserved before admission, a repoint
//! in between), per-producer admission keys, unserved addresses, and hosts
//! that do not admit continuations.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use meerkat::{
    AddressResolution, ContinuationDelivery, ContinuationDeliverySink, ContinuationHandling,
    ContinuationKey, ContinuationOwner, ContinuationOwnerService, ContinuationProducer,
    ContinuationResultRef, ContinuationStatus, JobDeliveryApplication, JobDeliverySink,
    MemoryDetachedJobStore, RuntimeDeliveryHost, RuntimeDeliveryOwner, RuntimeDeliveryPass,
    SessionAddressResolver, SessionId,
};
use meerkat::{
    ContinuationAdmitError, RetainedJobFacts, RetainedJobLookup, RetainedJobRecord,
    RetainedJobSource,
};
use meerkat_core::lifecycle::InputId;
use meerkat_core::retained_work::{RetainedContributorRef, RetainedWorkIdentity};
use meerkat_runtime::input_state::StoredInputState;
use meerkat_runtime::{
    ContinuationAdmission, ContinuationAdmissionOutcome, ContinuationAdmissionTransition,
    InMemoryRuntimeStore, LogicalRuntimeId, RuntimeDeliveryId, RuntimeDeliveryInbox, RuntimeStore,
};
use tokio::sync::watch;

const EVENT_GUARD: Duration = Duration::from_secs(10);

struct NoJobs;

#[async_trait::async_trait]
impl JobDeliverySink for NoJobs {
    async fn apply(
        &self,
        _application: JobDeliveryApplication,
    ) -> Result<(), meerkat::JobDeliveryApplyError> {
        Ok(())
    }
}

/// Records admissions per session; `known` stands in for each session's
/// input ledger, rows keyed by admission key.
#[derive(Default)]
struct RecordingSink {
    admitted: Mutex<Vec<(SessionId, String, InputId)>>,
    /// The inputs admitted, exactly as the owner built them.
    inputs: Mutex<Vec<meerkat_runtime::Input>>,
    known: Mutex<HashMap<(SessionId, String), StoredInputState>>,
    /// Stands in for a runtime with a native work authorization host.
    governed: bool,
    /// Governed resumes admitted, with the retained work each resumed.
    resumed: Mutex<Vec<(SessionId, InputId, RetainedWorkIdentity)>>,
    /// Forces the governed admission's verdict.
    resume_verdict: Mutex<Option<ContinuationAdmitError>>,
    /// Governed host originals admitted, in order: (session, input).
    governed_admitted: Mutex<Vec<(SessionId, InputId)>>,
    /// The bound inputs handed to the governed admission, in order.
    governed_inputs: Mutex<Vec<meerkat_runtime::Input>>,
    /// Forces the governed host original admission's native answer.
    governed_verdict: Mutex<Option<GovernedVerdict>>,
    /// Sessions prepared for a governed admission, in order.
    prepared: Mutex<Vec<SessionId>>,
    /// The rows each session holds, by input id.
    held_by_id: Mutex<HashMap<(SessionId, InputId), StoredInputState>>,
}

/// A scripted native answer to a governed host original admission, with the
/// custody settlement native admission would record for it.
#[derive(Debug, Clone)]
enum GovernedVerdict {
    /// An actual denial: settled `NotAdmitted`.
    Denied,
    /// Malformed facts: settled `NotAdmitted`.
    Malformed,
    /// Not ready: settled `NotAdmitted`.
    NotReady,
    /// The store write may have committed: settled `Uncertain`.
    Uncertain,
    /// An exact replay onto this existing input: settled `Admitted`.
    DeduplicatedOnto(InputId),
}

impl RecordingSink {
    fn admissions(&self) -> Vec<(SessionId, String, InputId)> {
        self.admitted.lock().expect("lock").clone()
    }

    fn resumes(&self) -> Vec<(SessionId, InputId, RetainedWorkIdentity)> {
        self.resumed.lock().expect("lock").clone()
    }

    fn governed() -> Self {
        Self {
            governed: true,
            ..Self::default()
        }
    }

    /// `session` already holds `input`, as its native admission records it.
    fn already_holds(&self, session: &SessionId, input: &meerkat_runtime::Input) {
        let key = input
            .header()
            .idempotency_key
            .as_ref()
            .expect("keyed")
            .to_string();
        let row = StoredInputState::accepted_for_test(input).expect("row");
        self.held_by_id
            .lock()
            .expect("lock")
            .insert((session.clone(), input.id().clone()), row.clone());
        self.known
            .lock()
            .expect("lock")
            .insert((session.clone(), key), row);
    }

    /// `session` holds `input` under its id only, as a governed admission is
    /// stored under its qualified key: a raw-key lookup misses it.
    fn holds_by_id_only(&self, session: &SessionId, input: &meerkat_runtime::Input) {
        self.held_by_id.lock().expect("lock").insert(
            (session.clone(), input.id().clone()),
            StoredInputState::accepted_for_test(input).expect("row"),
        );
    }

    fn governed_admissions(&self) -> Vec<(SessionId, InputId)> {
        self.governed_admitted.lock().expect("lock").clone()
    }

    fn governed_inputs(&self) -> Vec<meerkat_runtime::Input> {
        self.governed_inputs.lock().expect("lock").clone()
    }

    /// `session` holds a row under `key` recorded without any replay
    /// identity (an older admission).
    fn holds_legacy_row(&self, session: &SessionId, key: &str) {
        let mut row = StoredInputState::new_accepted(InputId::new());
        row.state.idempotency_key = Some(meerkat_runtime::IdempotencyKey::new(key));
        self.known
            .lock()
            .expect("lock")
            .insert((session.clone(), key.to_string()), row);
    }
}

/// The exact input the owner admits for a delivery with `body`.
fn continuation_input(
    input_id: InputId,
    admission_key: &str,
    body: &str,
) -> meerkat_runtime::Input {
    meerkat_runtime::Input::Prompt(meerkat_runtime::PromptInput::continuation(
        input_id,
        admission_key,
        body.into(),
        meerkat_core::types::HandlingMode::Queue,
    ))
}

#[async_trait::async_trait]
impl ContinuationDeliverySink for RecordingSink {
    async fn admit(
        &self,
        session: &SessionId,
        input: meerkat_runtime::Input,
    ) -> Result<InputId, String> {
        assert!(
            !self.governed,
            "a governed runtime never receives an unbound continuation"
        );
        let header = input.header();
        assert!(
            header.authority_association.is_none() && header.ingress_context.is_none(),
            "a continuation carries no authority of its own"
        );
        let admission_key = header
            .idempotency_key
            .as_ref()
            .expect("continuations are keyed")
            .to_string();
        let input_id = input.id().clone();
        self.admitted.lock().expect("lock").push((
            session.clone(),
            admission_key,
            input_id.clone(),
        ));
        self.inputs.lock().expect("lock").push(input.clone());
        self.already_holds(session, &input);
        Ok(input_id)
    }

    async fn admitted_input(
        &self,
        session: &SessionId,
        admission_key: &str,
    ) -> Result<Option<StoredInputState>, String> {
        Ok(self
            .known
            .lock()
            .expect("lock")
            .get(&(session.clone(), admission_key.to_string()))
            .cloned())
    }

    fn governs_work_authority(&self) -> bool {
        self.governed
    }

    async fn admit_retained(
        &self,
        session: &SessionId,
        input: meerkat_runtime::Input,
        request: meerkat_runtime::retained_work::RetainedResumeRequest,
    ) -> Result<InputId, ContinuationAdmitError> {
        assert!(
            self.governed,
            "only a governed runtime resumes retained work"
        );
        if let Some(verdict) = self.resume_verdict.lock().expect("lock").clone() {
            return Err(verdict);
        }
        let header = input.header();
        assert!(
            header.authority_association.is_none() && header.ingress_context.is_none(),
            "a resume carries no authority of its own"
        );
        let key = header.idempotency_key.as_ref().expect("keyed").to_string();
        let mut row = StoredInputState::accepted_for_test(&input).expect("row");
        row.state.retained_resume = Some(meerkat_runtime::retained_work::RetainedResumeRecord {
            identity: request.identity().clone(),
            delivery: request.delivery().clone(),
        });
        self.known
            .lock()
            .expect("lock")
            .insert((session.clone(), key), row);
        self.resumed.lock().expect("lock").push((
            session.clone(),
            input.id().clone(),
            request.identity().clone(),
        ));
        Ok(input.id().clone())
    }

    fn admits_governed(&self) -> bool {
        self.governed
    }

    async fn prepare_governed(&self, session: &SessionId) -> Result<(), ContinuationAdmitError> {
        self.prepared.lock().expect("lock").push(session.clone());
        Ok(())
    }

    async fn admit_governed(
        &self,
        session: &SessionId,
        input: meerkat_runtime::Input,
        custody: Box<dyn meerkat_runtime::input_admission_custody::NativeInputAdmissionCustody>,
    ) -> Result<meerkat::GovernedAdmissionOutcome, meerkat_runtime::RuntimeDriverError> {
        use meerkat_runtime::input_admission_custody::NativeInputAdmissionSettlement as Settled;
        assert!(
            self.governed,
            "only a governed runtime admits a governed host original"
        );
        self.governed_inputs
            .lock()
            .expect("lock")
            .push(input.clone());
        let refused = |kind| meerkat_runtime::RuntimeDriverError::InputRefused {
            refusal: meerkat_core::OperationRefused::new(kind),
        };
        let verdict = self.governed_verdict.lock().expect("lock").clone();
        match verdict {
            Some(GovernedVerdict::Denied) => {
                let _ = custody.settle(Settled::NotAdmitted);
                Err(refused(meerkat_core::OperationRefusalKind::Denied))
            }
            Some(GovernedVerdict::Malformed) => {
                let _ = custody.settle(Settled::NotAdmitted);
                Err(refused(meerkat_core::OperationRefusalKind::MalformedFacts))
            }
            Some(GovernedVerdict::NotReady) => {
                let _ = custody.settle(Settled::NotAdmitted);
                Err(
                    meerkat_runtime::RuntimeDriverError::ControllerReadinessUnavailable {
                        reason:
                            meerkat_core::authorization::ControllerReadinessFailure::PolicyUnavailable,
                    },
                )
            }
            Some(GovernedVerdict::Uncertain) => {
                let _ = custody.settle(Settled::Uncertain);
                Err(meerkat_runtime::RuntimeDriverError::Internal(
                    "the admission's store write outcome is unknown".into(),
                ))
            }
            Some(GovernedVerdict::DeduplicatedOnto(existing)) => {
                let _ = custody.settle(Settled::Admitted);
                Ok(meerkat::GovernedAdmissionOutcome::Deduplicated(existing))
            }
            None => {
                let header = input.header();
                assert!(
                    header.authority_association.is_some() && header.ingress_context.is_some(),
                    "a governed host original carries its binding"
                );
                let input_id = input.id().clone();
                self.governed_admitted
                    .lock()
                    .expect("lock")
                    .push((session.clone(), input_id.clone()));
                self.already_holds(session, &input);
                let _ = custody.settle(Settled::Admitted);
                Ok(meerkat::GovernedAdmissionOutcome::Accepted(input_id))
            }
        }
    }

    async fn admitted_input_by_id(
        &self,
        session: &SessionId,
        input_id: &InputId,
    ) -> Result<Option<StoredInputState>, String> {
        Ok(self
            .held_by_id
            .lock()
            .expect("lock")
            .get(&(session.clone(), input_id.clone()))
            .cloned())
    }
}

/// A host whose address resolution the test moves (a repoint, an unserved
/// member), with an optional continuation sink.
struct Host {
    sink: Option<Arc<RecordingSink>>,
    resolutions: Mutex<HashMap<LogicalRuntimeId, AddressResolution>>,
    job_source: Option<Arc<FakeJobs>>,
    /// A sink other than the recording one, when set.
    other_sink: Option<Arc<dyn ContinuationDeliverySink>>,
}

impl Host {
    fn new(sink: Option<Arc<RecordingSink>>) -> Arc<Self> {
        Arc::new(Self {
            sink,
            resolutions: Mutex::new(HashMap::new()),
            job_source: None,
            other_sink: None,
        })
    }

    fn with_jobs(sink: Arc<RecordingSink>, jobs: Arc<FakeJobs>) -> Arc<Self> {
        Arc::new(Self {
            sink: Some(sink),
            resolutions: Mutex::new(HashMap::new()),
            job_source: Some(jobs),
            other_sink: None,
        })
    }

    fn with_other_sink(sink: Arc<dyn ContinuationDeliverySink>, jobs: Arc<FakeJobs>) -> Arc<Self> {
        Arc::new(Self {
            sink: None,
            resolutions: Mutex::new(HashMap::new()),
            job_source: Some(jobs),
            other_sink: Some(sink),
        })
    }

    fn resolve(&self, address: &LogicalRuntimeId, resolution: AddressResolution) {
        self.resolutions
            .lock()
            .expect("lock")
            .insert(address.clone(), resolution);
    }
}

#[async_trait::async_trait]
impl RuntimeDeliveryHost for Host {
    async fn delivery_route(&self, _session_id: &SessionId) -> Option<meerkat::DeliveryRoute> {
        Some(meerkat::DeliveryRoute::ServedHere(Arc::new(NoJobs)))
    }

    async fn claim_cold_delivery(
        &self,
        session_id: &SessionId,
    ) -> Option<Result<meerkat_runtime::HostingClaim, meerkat_runtime::HostingRefused>> {
        // A single-process host: every session is served here.
        Some(meerkat_runtime::grant_session_hosting(
            &meerkat_runtime::HostingCapability::ProcessLocal,
            &meerkat_runtime::HostingOwner::mint(),
            session_id,
        ))
    }

    async fn resolve_address(&self, address: &LogicalRuntimeId) -> AddressResolution {
        self.resolutions
            .lock()
            .expect("lock")
            .get(address)
            .cloned()
            .unwrap_or_else(|| meerkat::default_address_resolution(address))
    }

    async fn continuation_sink(
        &self,
        _session_id: &SessionId,
    ) -> Option<Arc<dyn ContinuationDeliverySink>> {
        if let Some(other) = &self.other_sink {
            return Some(Arc::clone(other));
        }
        self.sink
            .clone()
            .map(|sink| sink as Arc<dyn ContinuationDeliverySink>)
    }

    fn retained_job_source(&self) -> Option<Arc<dyn RetainedJobSource>> {
        self.job_source
            .clone()
            .map(|jobs| jobs as Arc<dyn RetainedJobSource>)
    }
}

/// The committed owner of one producer's jobs (fork_off unless a test
/// says otherwise), as a table the test edits. It holds no other
/// producer's jobs.
#[derive(Default)]
struct FakeJobs {
    jobs: Mutex<HashMap<String, RetainedJobFacts>>,
    unavailable: Mutex<Option<String>>,
    /// The producer this owner holds jobs for; `None` is fork_off.
    producer: Mutex<Option<ContinuationProducer>>,
    /// Every lookup asked of this owner.
    lookups: std::sync::atomic::AtomicUsize,
    /// Parks the next lookup after it decided its answer, until released.
    held: Mutex<
        Option<(
            tokio::sync::oneshot::Sender<()>,
            tokio::sync::oneshot::Receiver<()>,
        )>,
    >,
    /// The host originals this owner holds in its `Host` namespace, by
    /// producer id.
    originals: Mutex<HashMap<String, meerkat::HostOriginalFacts>>,
    /// The binder this owner offers for its `Host` namespace.
    binder: Mutex<Option<Arc<governed_fixture::FakeBinder>>>,
}

impl FakeJobs {
    fn holding(job_id: &str, facts: RetainedJobFacts) -> Arc<Self> {
        let jobs = Arc::new(Self::default());
        jobs.jobs
            .lock()
            .expect("lock")
            .insert(job_id.to_string(), facts);
        jobs
    }

    fn change(&self, job_id: &str, change: impl FnOnce(&mut RetainedJobFacts)) {
        change(
            self.jobs
                .lock()
                .expect("lock")
                .get_mut(job_id)
                .expect("job"),
        );
    }

    fn lookups(&self) -> usize {
        self.lookups.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn owns_namespace(&self, namespace: &str) -> bool {
        matches!(
            self.producer.lock().expect("lock").as_ref(),
            Some(ContinuationProducer::Host { namespace: owned }) if owned == namespace
        )
    }

    fn change_original(
        &self,
        producer_id: &str,
        change: impl FnOnce(&mut meerkat::HostOriginalFacts),
    ) {
        change(
            self.originals
                .lock()
                .expect("lock")
                .get_mut(producer_id)
                .expect("host original"),
        );
    }

    /// Park the next lookup once it decided its answer: the owner asking is
    /// then inside a pass, past the marks of its signals. The first receiver
    /// fires when it parks; the lookup answers once the sender fires.
    fn hold_next_lookup(
        &self,
    ) -> (
        tokio::sync::oneshot::Receiver<()>,
        tokio::sync::oneshot::Sender<()>,
    ) {
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        *self.held.lock().expect("lock") = Some((entered_tx, release_rx));
        (entered_rx, release_tx)
    }

    fn answer(&self, producer: &ContinuationProducer, job_id: &str) -> RetainedJobLookup {
        let owned = self
            .producer
            .lock()
            .expect("lock")
            .clone()
            .unwrap_or(ContinuationProducer::ForkOff);
        if let Some(error) = self.unavailable.lock().expect("lock").clone() {
            return RetainedJobLookup::Unavailable(error);
        }
        if producer != &owned {
            return RetainedJobLookup::Absent;
        }
        self.jobs
            .lock()
            .expect("lock")
            .get(job_id)
            .cloned()
            .map_or(RetainedJobLookup::Absent, RetainedJobLookup::Found)
    }
}

#[async_trait::async_trait]
impl RetainedJobSource for FakeJobs {
    async fn retained_job(
        &self,
        producer: &ContinuationProducer,
        job_id: &str,
    ) -> RetainedJobLookup {
        self.lookups
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let answer = self.answer(producer, job_id);
        let held = self.held.lock().expect("lock").take();
        if let Some((entered, release)) = held {
            let _ = entered.send(());
            let _ = release.await;
        }
        answer
    }

    async fn host_original(
        &self,
        namespace: &str,
        producer_id: &str,
    ) -> meerkat::HostOriginalLookup {
        self.lookups
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if let Some(error) = self.unavailable.lock().expect("lock").clone() {
            return meerkat::HostOriginalLookup::Unavailable(error);
        }
        if !self.owns_namespace(namespace) {
            return meerkat::HostOriginalLookup::Absent;
        }
        self.originals
            .lock()
            .expect("lock")
            .get(producer_id)
            .cloned()
            .map_or(
                meerkat::HostOriginalLookup::Absent,
                meerkat::HostOriginalLookup::Found,
            )
    }

    fn host_original_binder(
        &self,
        namespace: &str,
    ) -> Option<Arc<dyn meerkat::HostOriginalBinder>> {
        if !self.owns_namespace(namespace) {
            return None;
        }
        self.binder
            .lock()
            .expect("lock")
            .clone()
            .map(|binder| binder as Arc<dyn meerkat::HostOriginalBinder>)
    }
}

struct Fixture {
    inbox: RuntimeDeliveryInbox,
    continuations: ContinuationOwnerService,
    _dir: Option<tempfile::TempDir>,
}

impl Fixture {
    fn new() -> Self {
        Self::on(Arc::new(InMemoryRuntimeStore::new()), None)
    }

    #[cfg(feature = "sqlite-store")]
    fn sqlite() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = meerkat_runtime::SqliteRuntimeStore::new(dir.path().join("runtime.sqlite3"))
            .expect("open sqlite runtime store");
        Self::on(Arc::new(store), Some(dir))
    }

    fn on(store: Arc<dyn RuntimeStore>, dir: Option<tempfile::TempDir>) -> Self {
        let inbox = RuntimeDeliveryInbox::new(store);
        Self {
            continuations: ContinuationOwnerService::new(
                inbox.clone(),
                Arc::new(SessionAddressResolver),
                Arc::new(meerkat_runtime::MeerkatMachine::ephemeral()),
            ),
            inbox,
            _dir: dir,
        }
    }

    /// Drive the admission index to `transition`, as the applier does before
    /// a modeled crash.
    async fn admission(
        &self,
        address: &LogicalRuntimeId,
        delivery_id: &str,
        transition: ContinuationAdmissionTransition,
    ) {
        let outcome = self
            .inbox
            .transition_continuation_admission(
                address,
                &RuntimeDeliveryId::new(delivery_id.to_string()).expect("id"),
                transition,
            )
            .await
            .expect("admission index write");
        assert!(
            matches!(outcome, ContinuationAdmissionOutcome::Transitioned(_)),
            "{outcome:?}"
        );
    }

    fn owner(&self) -> RuntimeDeliveryOwner {
        RuntimeDeliveryOwner::new(Arc::new(MemoryDetachedJobStore::new()), self.inbox.clone())
    }
}

fn delivery(key: &str, producer: ContinuationProducer) -> ContinuationDelivery {
    ContinuationDelivery {
        key: ContinuationKey::new(key).expect("key"),
        result: ContinuationResultRef {
            producer,
            producer_id: "op-1".into(),
            result_digest: "sha256:result".into(),
            summary: None,
        },
        body: "result".into(),
        handling: ContinuationHandling::Queue,
    }
}

fn host_producer() -> ContinuationProducer {
    ContinuationProducer::Host {
        namespace: "tasks".into(),
    }
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

async fn wait_for_status(
    fixture: &Fixture,
    owner: &ContinuationOwner,
    key: &str,
    passes: &mut watch::Receiver<RuntimeDeliveryPass>,
    predicate: impl Fn(&ContinuationStatus) -> bool,
) -> ContinuationStatus {
    let key = ContinuationKey::new(key).expect("key");
    tokio::time::timeout(EVENT_GUARD, async {
        loop {
            let status = fixture
                .continuations
                .continuation_status(owner, &key)
                .await
                .expect("status");
            if predicate(&status) {
                return status;
            }
            passes.changed().await.expect("owner pass channel open");
        }
    })
    .await
    .expect("continuation status reached")
}

#[tokio::test]
async fn a_continuation_is_admitted_once_and_reads_applied() {
    let fixture = Fixture::new();
    let sink = Arc::new(RecordingSink::default());
    let session = SessionId::new();
    let owner = ContinuationOwner::Session {
        session_id: session.clone(),
    };
    let handle = fixture
        .owner()
        .arm(Host::new(Some(sink.clone())))
        .expect("arm");
    let mut passes = handle.subscribe_passes();

    let receipt = fixture
        .continuations
        .submit(&owner, delivery("task-1", host_producer()), 1)
        .await
        .expect("commit");
    let status = wait_for_status(&fixture, &owner, "task-1", &mut passes, |status| {
        matches!(status, ContinuationStatus::Applied { .. })
    })
    .await;
    let admissions = sink.admissions();
    assert_eq!(admissions.len(), 1, "admitted exactly once");
    let (admitted_session, admission_key, input) = admissions[0].clone();
    assert_eq!(admitted_session, session);
    assert_eq!(
        admission_key, receipt.delivery_id,
        "host results admit under the hash-namespaced delivery id"
    );
    assert_eq!(
        status,
        ContinuationStatus::Applied {
            receipt,
            session,
            input
        }
    );
}

/// The E11 cut: the input reached session s1, the process died before the
/// inbox acknowledgement, and the owner was repointed to s2. Recovery finds
/// the s1 input by its admission key and admits nothing to s2.
async fn admitted_before_the_acknowledgement_then_repointed(fixture: Fixture) {
    let sink = Arc::new(RecordingSink::default());
    let s1 = SessionId::new();
    let s2 = SessionId::new();
    let owner = ContinuationOwner::Session {
        session_id: s1.clone(),
    };
    let receipt = fixture
        .continuations
        .submit(&owner, delivery("task-1", host_producer()), 1)
        .await
        .expect("commit");
    let address = LogicalRuntimeId::new(receipt.address.clone());
    let admitted = InputId::new();
    fixture
        .admission(
            &address,
            &receipt.delivery_id,
            ContinuationAdmissionTransition::Reserve {
                session_id: s1.clone(),
                input_id: admitted.clone(),
            },
        )
        .await;
    sink.already_holds(
        &s1,
        &continuation_input(admitted.clone(), &receipt.delivery_id, "result"),
    );

    let host = Host::new(Some(sink.clone()));
    host.resolve(&address, AddressResolution::Session(s2));
    let handle = fixture.owner().arm(host).expect("arm");
    let mut passes = handle.subscribe_passes();
    let status = wait_for_status(&fixture, &owner, "task-1", &mut passes, |status| {
        matches!(status, ContinuationStatus::Applied { .. })
    })
    .await;
    assert!(sink.admissions().is_empty(), "nothing is admitted again");
    assert_eq!(
        status,
        ContinuationStatus::Applied {
            receipt,
            session: s1,
            input: admitted
        }
    );
}

/// A reservation that never reached its session moves to the session that
/// serves the owner now, and is admitted there once.
async fn reserved_before_admission_then_repointed(fixture: Fixture) {
    let sink = Arc::new(RecordingSink::default());
    let s1 = SessionId::new();
    let s2 = SessionId::new();
    let owner = ContinuationOwner::Session {
        session_id: s1.clone(),
    };
    let receipt = fixture
        .continuations
        .submit(&owner, delivery("task-1", host_producer()), 1)
        .await
        .expect("commit");
    let address = LogicalRuntimeId::new(receipt.address.clone());
    fixture
        .admission(
            &address,
            &receipt.delivery_id,
            ContinuationAdmissionTransition::Reserve {
                session_id: s1.clone(),
                input_id: InputId::new(),
            },
        )
        .await;

    let host = Host::new(Some(sink.clone()));
    host.resolve(&address, AddressResolution::Session(s2.clone()));
    let handle = fixture.owner().arm(host).expect("arm");
    let mut passes = handle.subscribe_passes();
    let status = wait_for_status(&fixture, &owner, "task-1", &mut passes, |status| {
        matches!(status, ContinuationStatus::Applied { .. })
    })
    .await;
    let admissions = sink.admissions();
    assert_eq!(admissions.len(), 1);
    assert_eq!(admissions[0].0, s2);
    let ContinuationStatus::Applied { session, .. } = status else {
        unreachable!("waited for Applied");
    };
    assert_eq!(session, s2);

    // The superseded session can no longer be recorded as the admission.
    let late = fixture
        .inbox
        .transition_continuation_admission(
            &address,
            &RuntimeDeliveryId::new(receipt.delivery_id.clone()).expect("id"),
            ContinuationAdmissionTransition::Apply {
                session_id: s1,
                input_id: InputId::new(),
            },
        )
        .await
        .expect("admission index read");
    assert!(
        matches!(late, ContinuationAdmissionOutcome::Rejected { current: Some(ContinuationAdmission::Applied { ref session_id, .. }) } if *session_id == s2),
        "{late:?}"
    );
}

/// Reserved and never admitted, with the owner still in the same session:
/// admitted there once, under the reserved input id.
async fn reserved_before_admission_in_the_same_session(fixture: Fixture) {
    let sink = Arc::new(RecordingSink::default());
    let session = SessionId::new();
    let owner = ContinuationOwner::Session {
        session_id: session.clone(),
    };
    let receipt = fixture
        .continuations
        .submit(&owner, delivery("task-1", host_producer()), 1)
        .await
        .expect("commit");
    let address = LogicalRuntimeId::new(receipt.address.clone());
    let reserved = InputId::new();
    fixture
        .admission(
            &address,
            &receipt.delivery_id,
            ContinuationAdmissionTransition::Reserve {
                session_id: session.clone(),
                input_id: reserved.clone(),
            },
        )
        .await;

    let handle = fixture
        .owner()
        .arm(Host::new(Some(sink.clone())))
        .expect("arm");
    let mut passes = handle.subscribe_passes();
    let status = wait_for_status(&fixture, &owner, "task-1", &mut passes, |status| {
        matches!(status, ContinuationStatus::Applied { .. })
    })
    .await;
    assert_eq!(
        sink.admissions(),
        vec![(
            session.clone(),
            receipt.delivery_id.clone(),
            reserved.clone()
        )]
    );
    assert_eq!(
        status,
        ContinuationStatus::Applied {
            receipt,
            session,
            input: reserved
        }
    );
}

/// The index reached `Applied` in s1 and the process died before the inbox
/// acknowledgement; the owner moved to s2. Recovery acknowledges the row and
/// admits nothing.
async fn applied_in_the_index_before_the_acknowledgement_then_repointed(fixture: Fixture) {
    let sink = Arc::new(RecordingSink::default());
    let s1 = SessionId::new();
    let s2 = SessionId::new();
    let owner = ContinuationOwner::Session {
        session_id: s1.clone(),
    };
    let receipt = fixture
        .continuations
        .submit(&owner, delivery("task-1", host_producer()), 1)
        .await
        .expect("commit");
    let address = LogicalRuntimeId::new(receipt.address.clone());
    let admitted = InputId::new();
    for transition in [
        ContinuationAdmissionTransition::Reserve {
            session_id: s1.clone(),
            input_id: admitted.clone(),
        },
        ContinuationAdmissionTransition::Apply {
            session_id: s1.clone(),
            input_id: admitted.clone(),
        },
    ] {
        fixture
            .admission(&address, &receipt.delivery_id, transition)
            .await;
    }
    assert_eq!(
        fixture
            .continuations
            .continuation_status(&owner, &ContinuationKey::new("task-1").expect("key"))
            .await
            .expect("status"),
        ContinuationStatus::Pending {
            receipt: receipt.clone(),
            sequence: 1,
            admitted: Some((s1.clone(), admitted.clone()))
        }
    );

    let host = Host::new(Some(sink.clone()));
    host.resolve(&address, AddressResolution::Session(s2));
    let handle = fixture.owner().arm(host).expect("arm");
    let mut passes = handle.subscribe_passes();
    let status = wait_for_status(&fixture, &owner, "task-1", &mut passes, |status| {
        matches!(status, ContinuationStatus::Applied { .. })
    })
    .await;
    assert!(sink.admissions().is_empty(), "nothing is admitted again");
    assert_eq!(
        status,
        ContinuationStatus::Applied {
            receipt,
            session: s1,
            input: admitted
        }
    );
}

macro_rules! crash_cut {
    ($case:ident) => {
        mod $case {
            #[tokio::test]
            async fn memory() {
                super::$case(super::Fixture::new()).await;
            }

            #[cfg(feature = "sqlite-store")]
            #[tokio::test]
            async fn sqlite() {
                super::$case(super::Fixture::sqlite()).await;
            }
        }
    };
}

crash_cut!(admitted_before_the_acknowledgement_then_repointed);
crash_cut!(reserved_before_admission_then_repointed);
crash_cut!(reserved_before_admission_in_the_same_session);
crash_cut!(applied_in_the_index_before_the_acknowledgement_then_repointed);

/// A fork_off completion delivered live before it became a continuation
/// stands for it only as an exact replay; a host key that happens to equal
/// an unrelated input's key does not dedupe at all.
#[tokio::test]
async fn an_exact_live_replay_under_a_fork_off_key_is_applied_and_host_keys_never_dedupe() {
    let fixture = Fixture::new();
    let sink = Arc::new(RecordingSink::default());
    let session = SessionId::new();
    let owner = ContinuationOwner::Session {
        session_id: session.clone(),
    };
    let live = InputId::new();
    sink.already_holds(
        &session,
        &continuation_input(live.clone(), "fork_off:job-7", "result"),
    );
    sink.already_holds(
        &session,
        &continuation_input(InputId::new(), "host-collision", "result"),
    );

    let fork = fixture
        .continuations
        .submit(
            &owner,
            delivery("fork_off:job-7", ContinuationProducer::ForkOff),
            1,
        )
        .await
        .expect("fork continuation");
    let host = fixture
        .continuations
        .submit(&owner, delivery("host-collision", host_producer()), 2)
        .await
        .expect("host continuation");
    let handle = fixture
        .owner()
        .arm(Host::new(Some(sink.clone())))
        .expect("arm");
    let mut passes = handle.subscribe_passes();
    let fork_status = wait_for_status(&fixture, &owner, "fork_off:job-7", &mut passes, |status| {
        matches!(status, ContinuationStatus::Applied { .. })
    })
    .await;
    wait_for_status(&fixture, &owner, "host-collision", &mut passes, |status| {
        matches!(status, ContinuationStatus::Applied { .. })
    })
    .await;
    assert_eq!(
        fork_status,
        ContinuationStatus::Applied {
            receipt: fork,
            session: session.clone(),
            input: live
        },
        "the exact live input stands for the fork completion"
    );
    let admissions = sink.admissions();
    assert_eq!(
        admissions.len(),
        1,
        "only the host continuation is admitted"
    );
    assert_eq!(admissions[0].1, host.delivery_id);
}

/// P3-B B5: a notice-bodied continuation (a fork_off or council completion)
/// admits exactly the completion input a live delivery admitted
/// (`PromptInput::detached_job_completed` under the job's key), so the
/// owner's transcript is byte for byte what it always was. Only the input id
/// (the reserved one) and the admission time differ.
#[tokio::test]
async fn a_notice_continuation_admits_exactly_the_live_completion_input() {
    let fixture = Fixture::new();
    let sink = Arc::new(RecordingSink::default());
    let session = SessionId::new();
    let owner = ContinuationOwner::Session {
        session_id: session.clone(),
    };
    let notice = meerkat_core::types::SystemNoticeMessage::persisted_background_job(
        "fork_off",
        "job-9",
        meerkat_core::event::BackgroundJobTerminalStatus::Completed,
        r#"{"status":"completed","text":"done"}"#.to_string(),
    );
    let mut fork = delivery("fork_off:job-9", ContinuationProducer::ForkOff);
    fork.body = meerkat::ContinuationBody::notice(notice.clone());
    fixture
        .continuations
        .submit(&owner, fork, 1)
        .await
        .expect("fork continuation");
    let handle = fixture
        .owner()
        .arm(Host::new(Some(sink.clone())))
        .expect("arm");
    let mut passes = handle.subscribe_passes();
    wait_for_status(&fixture, &owner, "fork_off:job-9", &mut passes, |status| {
        matches!(status, ContinuationStatus::Applied { .. })
    })
    .await;

    let inputs = sink.inputs.lock().expect("lock").clone();
    let [meerkat_runtime::Input::Prompt(admitted)] = inputs.as_slice() else {
        panic!("one prompt input is admitted: {inputs:?}");
    };
    let admitted = admitted.clone();
    let mut live = meerkat_runtime::PromptInput::detached_job_completed("fork_off:job-9", notice);
    live.header.id = admitted.header.id.clone();
    live.header.timestamp = admitted.header.timestamp;
    assert_eq!(
        serde_json::to_value(meerkat_runtime::Input::Prompt(admitted)).expect("encode"),
        serde_json::to_value(meerkat_runtime::Input::Prompt(live)).expect("encode"),
    );
}

/// A row under the same live key that is not this delivery (another result,
/// or an older admission with no recorded identity) is a conflict: the
/// delivery stays pending and visible, is never applied by key, and nothing
/// is admitted over it.
async fn another_row_under_the_live_key_is_a_conflict(
    seed: impl FnOnce(&RecordingSink, &SessionId),
) {
    let fixture = Fixture::new();
    let sink = Arc::new(RecordingSink::default());
    let session = SessionId::new();
    let owner = ContinuationOwner::Session {
        session_id: session.clone(),
    };
    seed(&sink, &session);
    fixture
        .continuations
        .submit(
            &owner,
            delivery("fork_off:job-7", ContinuationProducer::ForkOff),
            1,
        )
        .await
        .expect("fork continuation");
    let handle = fixture
        .owner()
        .arm(Host::new(Some(sink.clone())))
        .expect("arm");
    let mut passes = handle.subscribe_passes();
    let pass = wait_for_pass(&mut passes, "the conflicting row blocked", |pass| {
        !pass.blocked_runtimes.is_empty()
    })
    .await;
    assert!(
        pass.failures
            .iter()
            .any(|failure| failure.contains("is not this delivery")),
        "visible as a conflict: {:?}",
        pass.failures
    );
    assert!(sink.admissions().is_empty(), "nothing is admitted over it");
    assert!(matches!(
        fixture
            .continuations
            .continuation_status(
                &owner,
                &ContinuationKey::new("fork_off:job-7").expect("key")
            )
            .await
            .expect("status"),
        ContinuationStatus::Pending { admitted: None, .. }
    ));
}

#[tokio::test]
async fn a_changed_result_under_the_live_key_is_a_conflict() {
    another_row_under_the_live_key_is_a_conflict(|sink, session| {
        sink.already_holds(
            session,
            &continuation_input(InputId::new(), "fork_off:job-7", "an earlier result"),
        );
    })
    .await;
}

#[tokio::test]
async fn an_older_row_without_an_identity_under_the_live_key_is_a_conflict() {
    another_row_under_the_live_key_is_a_conflict(|sink, session| {
        sink.holds_legacy_row(session, "fork_off:job-7");
    })
    .await;
}

/// A member address no session serves stays blocked, and is applied once an
/// attachment commit retries it with a session serving it.
#[tokio::test]
async fn an_unserved_member_address_is_applied_when_an_attachment_serves_it() {
    let fixture = Fixture::new();
    let sink = Arc::new(RecordingSink::default());
    let address =
        meerkat::member_delivery_address("team", team_incarnation(), "lead", 1).expect("address");
    let session = SessionId::new();
    let submission = meerkat_runtime::RuntimeDeliverySubmission::new(
        RuntimeDeliveryId::new("continuation:member").expect("id"),
        meerkat_runtime::RuntimeDeliveryKind::Continuation,
        "host:tasks",
        1,
        "task-1",
        serde_json::to_vec(&delivery("task-1", host_producer())).expect("encode"),
    )
    .expect("submission");
    fixture
        .inbox
        .submit(&address, submission)
        .await
        .expect("commit");
    let host = Host::new(Some(sink.clone()));
    host.resolve(&address, AddressResolution::NotServed);
    let (attachments, attachment_commits) = watch::channel(0_u64);
    let handle = fixture
        .owner()
        .with_attachment_commits(attachment_commits)
        .arm(host.clone())
        .expect("arm");
    let mut passes = handle.subscribe_passes();
    wait_for_pass(&mut passes, "the unserved address blocked", |pass| {
        pass.blocked_runtimes.contains(&address)
    })
    .await;
    assert!(sink.admissions().is_empty());

    host.resolve(&address, AddressResolution::Session(session.clone()));
    attachments.send_modify(|generation| *generation += 1);
    wait_for_pass(&mut passes, "the address applied", |pass| {
        !pass.blocked_runtimes.contains(&address) && pass.applied > 0
    })
    .await;
    assert_eq!(sink.admissions().len(), 1);
    assert_eq!(sink.admissions()[0].0, session);
}

/// A host without a continuation sink leaves continuation rows visibly
/// blocked as an unsupported kind.
#[tokio::test]
async fn a_host_without_a_continuation_sink_blocks_rows_as_unsupported() {
    let fixture = Fixture::new();
    let owner = ContinuationOwner::Session {
        session_id: SessionId::new(),
    };
    fixture
        .continuations
        .submit(&owner, delivery("task-1", host_producer()), 1)
        .await
        .expect("commit");
    let handle = fixture.owner().arm(Host::new(None)).expect("arm");
    let mut passes = handle.subscribe_passes();
    let pass = wait_for_pass(&mut passes, "the unsupported row blocked", |pass| {
        !pass.blocked_runtimes.is_empty()
    })
    .await;
    assert!(
        pass.failures
            .iter()
            .any(|failure| failure.contains("UnsupportedKind")),
        "visible as unsupported: {:?}",
        pass.failures
    );
}

/// A historical continuation row (committed while the runtime was not
/// governed) reaching a runtime with a native work authorization host is
/// settled as refused, typed, before anything is reserved or admitted. It is
/// never applied, its status reads `Refused`, and the delivery committed
/// after it on the same runtime is delivered.
#[tokio::test]
async fn a_refused_continuation_never_holds_the_delivery_behind_it() {
    let fixture = Fixture::new();
    let sink = Arc::new(RecordingSink {
        governed: true,
        ..RecordingSink::default()
    });
    let session = SessionId::new();
    let owner = ContinuationOwner::Session {
        session_id: session.clone(),
    };
    let receipt = fixture
        .continuations
        .submit(&owner, delivery("task-1", host_producer()), 1)
        .await
        .expect("commit while the runtime is not governed");
    let address = LogicalRuntimeId::new(receipt.address.clone());
    let job_id = meerkat::JobId::generated();
    let permitted = meerkat_runtime::RuntimeDeliverySubmission::new(
        RuntimeDeliveryId::new("job:permitted:1").expect("id"),
        meerkat_runtime::RuntimeDeliveryKind::JobNotification,
        job_id.as_str(),
        1,
        "lineage",
        serde_json::to_vec(&meerkat::JobNotificationDeliveryPayload {
            job_id: job_id.clone(),
            delivery_sequence: 1,
            origin_session_id: session.clone(),
            interaction_lineage_id: meerkat::InteractionLineageId::new(),
            targets: Vec::new(),
            notification: meerkat::JobNotification::new("n-1", "n-1", "progress", "halfway")
                .expect("notification"),
        })
        .expect("encode"),
    )
    .expect("submission");
    fixture
        .inbox
        .submit(&address, permitted)
        .await
        .expect("a later delivery on the same runtime");

    let handle = fixture
        .owner()
        .arm(Host::new(Some(sink.clone())))
        .expect("arm");
    let mut passes = handle.subscribe_passes();
    let pass = wait_for_pass(&mut passes, "the refused row settled", |pass| {
        pass.refused > 0
    })
    .await;
    assert_eq!(
        pass.applied, 1,
        "the permitted delivery behind it is delivered"
    );
    assert!(
        pass.blocked_runtimes.is_empty(),
        "nothing holds the runtime"
    );
    assert!(
        pass.failures
            .iter()
            .any(|failure| failure.contains("refused (NoAdmissibleWorkBinding)")),
        "visible as a refusal: {:?}",
        pass.failures
    );
    assert!(sink.admissions().is_empty(), "nothing was admitted");
    assert_eq!(
        fixture
            .inbox
            .continuation_admission(
                &address,
                &RuntimeDeliveryId::new(receipt.delivery_id.clone()).expect("id"),
            )
            .await
            .expect("admission index"),
        None,
        "nothing was reserved"
    );
    assert_eq!(
        fixture
            .continuations
            .continuation_status(&owner, &ContinuationKey::new("task-1").expect("key"))
            .await
            .expect("status"),
        ContinuationStatus::Refused {
            receipt,
            sequence: 1,
            reason: meerkat_runtime::RuntimeDeliveryRefusalReason::NoAdmissibleWorkBinding
        }
    );
    assert_eq!(
        fixture
            .inbox
            .pending_delivery_total()
            .await
            .expect("backlog"),
        0,
        "the refusal is settled, not pending"
    );
}

/// A host whose native work authorization owner refuses everything; only its
/// installation matters here.
struct RefusingWorkAuthority;

impl meerkat_runtime::input_authority::NativeWorkAuthorizationHost for RefusingWorkAuthority {
    fn authenticate_association(
        &self,
        _runtime_id: &LogicalRuntimeId,
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

/// The machine sink reports a native work authorization host, which is what
/// makes the owner refuse continuations there.
#[tokio::test]
async fn the_machine_sink_reports_a_native_work_authorization_host() {
    let governed = meerkat_runtime::MeerkatMachine::ephemeral()
        .with_native_work_authorization_host(Arc::new(RefusingWorkAuthority))
        .expect("install the native work authorization host");
    assert!(meerkat::MachineContinuationSink::new(Arc::new(governed)).governs_work_authority());
    assert!(
        !meerkat::MachineContinuationSink::new(Arc::new(
            meerkat_runtime::MeerkatMachine::ephemeral()
        ))
        .governs_work_authority()
    );
}

fn work_identity(tag: &str) -> RetainedWorkIdentity {
    RetainedWorkIdentity::new(
        "rt:session:fork-owner",
        meerkat_core::lifecycle::RunId::new(),
        vec![RetainedContributorRef {
            input_id: InputId::new(),
            association_digest: format!("association-{tag}"),
            submission_digest: format!("submission-{tag}"),
        }],
        std::collections::BTreeMap::new(),
        None,
    )
}

fn fork_completion(job_id: &str, result_digest: &str) -> ContinuationDelivery {
    ContinuationDelivery {
        key: ContinuationKey::new(format!("fork_off:{job_id}")).expect("key"),
        result: ContinuationResultRef {
            producer: ContinuationProducer::ForkOff,
            producer_id: job_id.into(),
            result_digest: result_digest.into(),
            summary: None,
        },
        body: "the fork finished".into(),
        handling: ContinuationHandling::Queue,
    }
}

/// A fork_off completion committed from its job's record, owed to `session`.
async fn retained_fork_completion(
    fixture: &Fixture,
    session: &SessionId,
    jobs: &FakeJobs,
) -> meerkat::ContinuationReceipt {
    let record = RetainedJobRecord::from_owner(jobs, ContinuationProducer::ForkOff, "job-1")
        .await
        .expect("read the job owner")
        .expect("the job is committed");
    fixture
        .continuations
        .submit_retained_completion(
            &ContinuationOwner::Session {
                session_id: session.clone(),
            },
            fork_completion("job-1", "sha256:result"),
            &record,
            1,
        )
        .await
        .expect("commit the completion")
}

fn committed_job(session: &SessionId, identity: &RetainedWorkIdentity) -> RetainedJobFacts {
    RetainedJobFacts {
        owner_session_id: session.clone(),
        retained_work: Some(identity.clone()),
        result_digest: "sha256:result".into(),
    }
}

/// On a governed runtime a retained completion is admitted as a resume of
/// its retained work once the job's committed owner confirms it.
#[tokio::test]
async fn a_confirmed_retained_completion_is_resumed_on_a_governed_runtime() {
    let fixture = Fixture::new();
    let session = SessionId::new();
    let identity = work_identity("a");
    let jobs = FakeJobs::holding("job-1", committed_job(&session, &identity));
    let receipt = retained_fork_completion(&fixture, &session, &jobs).await;
    let sink = Arc::new(RecordingSink::governed());
    let handle = fixture
        .owner()
        .arm(Host::with_jobs(sink.clone(), jobs))
        .expect("arm");
    let mut passes = handle.subscribe_passes();
    let owner = ContinuationOwner::Session {
        session_id: session.clone(),
    };
    let status = wait_for_status(&fixture, &owner, "fork_off:job-1", &mut passes, |status| {
        matches!(status, ContinuationStatus::Applied { .. })
    })
    .await;
    let resumes = sink.resumes();
    assert_eq!(resumes.len(), 1);
    assert_eq!(resumes[0].0, session);
    assert_eq!(
        resumes[0].2, identity,
        "the resume names the job's retained work"
    );
    assert!(sink.admissions().is_empty(), "never admitted unbound");
    assert!(
        matches!(status, ContinuationStatus::Applied { receipt: applied, .. } if applied == receipt)
    );
}

/// A completion the job's owner does not confirm exactly (another retained
/// work, another owner session, another result) is an immutably invalid
/// binding: settled, never admitted, and the delivery behind it proceeds.
async fn an_unconfirmed_retained_completion_is_refused(change: impl FnOnce(&mut RetainedJobFacts)) {
    let fixture = Fixture::new();
    let session = SessionId::new();
    let identity = work_identity("a");
    let jobs = FakeJobs::holding("job-1", committed_job(&session, &identity));
    let receipt = retained_fork_completion(&fixture, &session, &jobs).await;
    jobs.change("job-1", change);
    let job_id = meerkat::JobId::generated();
    fixture
        .inbox
        .submit(
            &LogicalRuntimeId::new(receipt.address.clone()),
            meerkat_runtime::RuntimeDeliverySubmission::new(
                RuntimeDeliveryId::new("job:permitted:1").expect("id"),
                meerkat_runtime::RuntimeDeliveryKind::JobNotification,
                job_id.as_str(),
                1,
                "lineage",
                serde_json::to_vec(&meerkat::JobNotificationDeliveryPayload {
                    job_id: job_id.clone(),
                    delivery_sequence: 1,
                    origin_session_id: session.clone(),
                    interaction_lineage_id: meerkat::InteractionLineageId::new(),
                    targets: Vec::new(),
                    notification: meerkat::JobNotification::new(
                        "n-1", "n-1", "progress", "halfway",
                    )
                    .expect("notification"),
                })
                .expect("encode"),
            )
            .expect("submission"),
        )
        .await
        .expect("a permitted delivery behind it");
    let sink = Arc::new(RecordingSink::governed());
    let handle = fixture
        .owner()
        .arm(Host::with_jobs(sink.clone(), jobs))
        .expect("arm");
    let mut passes = handle.subscribe_passes();
    let pass = wait_for_pass(&mut passes, "the unconfirmed row settled", |pass| {
        pass.refused > 0
    })
    .await;
    assert_eq!(
        pass.applied, 1,
        "the permitted delivery behind it is delivered"
    );
    assert!(sink.resumes().is_empty() && sink.admissions().is_empty());
    assert_eq!(
        fixture
            .continuations
            .continuation_status(
                &ContinuationOwner::Session {
                    session_id: session.clone()
                },
                &ContinuationKey::new("fork_off:job-1").expect("key")
            )
            .await
            .expect("status"),
        ContinuationStatus::Refused {
            receipt,
            sequence: 1,
            reason: meerkat_runtime::RuntimeDeliveryRefusalReason::NoAdmissibleWorkBinding
        }
    );
}

#[tokio::test]
async fn a_completion_naming_other_retained_work_is_refused() {
    an_unconfirmed_retained_completion_is_refused(|facts| {
        facts.retained_work = Some(work_identity("other"));
    })
    .await;
}

#[tokio::test]
async fn a_completion_retargeted_at_another_session_is_refused() {
    an_unconfirmed_retained_completion_is_refused(|facts| {
        facts.owner_session_id = SessionId::new();
    })
    .await;
}

#[tokio::test]
async fn a_completion_with_a_changed_result_is_refused() {
    an_unconfirmed_retained_completion_is_refused(|facts| {
        facts.result_digest = "sha256:another-result".into();
    })
    .await;
}

#[tokio::test]
async fn a_legacy_job_without_retained_work_is_refused() {
    an_unconfirmed_retained_completion_is_refused(|facts| {
        facts.retained_work = None;
    })
    .await;
}

/// Without a configured job owner a retained completion cannot be confirmed
/// now: it stays pending (retryable), never settled.
#[tokio::test]
async fn without_a_job_owner_a_retained_completion_stays_pending() {
    let fixture = Fixture::new();
    let session = SessionId::new();
    let identity = work_identity("a");
    let jobs = FakeJobs::holding("job-1", committed_job(&session, &identity));
    retained_fork_completion(&fixture, &session, &jobs).await;
    let sink = Arc::new(RecordingSink::governed());
    let handle = fixture
        .owner()
        .arm(Host::new(Some(sink.clone())))
        .expect("arm");
    let mut passes = handle.subscribe_passes();
    let pass = wait_for_pass(&mut passes, "the row blocked", |pass| {
        !pass.blocked_runtimes.is_empty()
    })
    .await;
    assert_eq!(pass.refused, 0, "never settled");
    assert!(sink.resumes().is_empty());
}

/// The admission's verdict decides: an actual denial settles as
/// AuthorityDenied; a retryable failure keeps the row pending.
async fn a_governed_admission_verdict(
    verdict: ContinuationAdmitError,
) -> (RuntimeDeliveryPass, ContinuationStatus) {
    let fixture = Fixture::new();
    let session = SessionId::new();
    let identity = work_identity("a");
    let jobs = FakeJobs::holding("job-1", committed_job(&session, &identity));
    retained_fork_completion(&fixture, &session, &jobs).await;
    let sink = Arc::new(RecordingSink::governed());
    *sink.resume_verdict.lock().expect("lock") = Some(verdict);
    let handle = fixture
        .owner()
        .arm(Host::with_jobs(sink, jobs))
        .expect("arm");
    let mut passes = handle.subscribe_passes();
    let pass = wait_for_pass(&mut passes, "the verdict applied", |pass| {
        pass.refused > 0 || !pass.blocked_runtimes.is_empty()
    })
    .await;
    let status = fixture
        .continuations
        .continuation_status(
            &ContinuationOwner::Session {
                session_id: session,
            },
            &ContinuationKey::new("fork_off:job-1").expect("key"),
        )
        .await
        .expect("status");
    (pass, status)
}

#[tokio::test]
async fn an_actual_denial_settles_the_retained_completion() {
    let (pass, status) = a_governed_admission_verdict(ContinuationAdmitError::Refused(
        meerkat_runtime::RuntimeDeliveryRefusalReason::AuthorityDenied,
    ))
    .await;
    assert_eq!(pass.refused, 1);
    assert!(matches!(
        status,
        ContinuationStatus::Refused {
            reason: meerkat_runtime::RuntimeDeliveryRefusalReason::AuthorityDenied,
            ..
        }
    ));
}

#[tokio::test]
async fn an_unavailable_owner_keeps_the_retained_completion_pending() {
    let (pass, status) =
        a_governed_admission_verdict(ContinuationAdmitError::Failed("owner unavailable".into()))
            .await;
    assert_eq!(pass.refused, 0);
    assert!(matches!(status, ContinuationStatus::Pending { .. }));
}

/// A job owner that definitively holds no such job is an immutably missing
/// binding; one that cannot be read now keeps the row pending.
#[tokio::test]
async fn an_absent_job_is_refused_and_an_unreadable_owner_keeps_the_row_pending() {
    for unreadable in [false, true] {
        let fixture = Fixture::new();
        let session = SessionId::new();
        let identity = work_identity("a");
        let jobs = FakeJobs::holding("job-1", committed_job(&session, &identity));
        retained_fork_completion(&fixture, &session, &jobs).await;
        if unreadable {
            *jobs.unavailable.lock().expect("lock") = Some("owner store offline".into());
        } else {
            jobs.jobs.lock().expect("lock").clear();
        }
        let sink = Arc::new(RecordingSink::governed());
        let handle = fixture
            .owner()
            .arm(Host::with_jobs(sink.clone(), jobs))
            .expect("arm");
        let mut passes = handle.subscribe_passes();
        let pass = wait_for_pass(&mut passes, "the lookup verdict applied", |pass| {
            pass.refused > 0 || !pass.blocked_runtimes.is_empty()
        })
        .await;
        assert_eq!(
            pass.refused,
            usize::from(!unreadable),
            "unreadable={unreadable}"
        );
        assert!(sink.resumes().is_empty());
    }
}

const SETUP_NAMESPACE: &str = "app.setup";

fn setup_producer(namespace: &str) -> ContinuationProducer {
    ContinuationProducer::Host {
        namespace: namespace.into(),
    }
}

/// A host's committed setup result, owed to `session`, as its owner
/// confirms it, committed as a retained continuation.
async fn retained_setup_completion(
    fixture: &Fixture,
    session: &SessionId,
    identity: &RetainedWorkIdentity,
) -> (Arc<FakeJobs>, meerkat::ContinuationReceipt) {
    let jobs = FakeJobs::holding("setup-1", committed_job(session, identity));
    *jobs.producer.lock().expect("lock") = Some(setup_producer(SETUP_NAMESPACE));
    let record = RetainedJobRecord::from_owner(&*jobs, setup_producer(SETUP_NAMESPACE), "setup-1")
        .await
        .expect("read the setup owner")
        .expect("the setup result is committed");
    let receipt = fixture
        .continuations
        .submit_retained_completion(
            &ContinuationOwner::Session {
                session_id: session.clone(),
            },
            ContinuationDelivery {
                key: ContinuationKey::new("setup:setup-1").expect("key"),
                result: ContinuationResultRef {
                    producer: setup_producer(SETUP_NAMESPACE),
                    producer_id: "setup-1".into(),
                    result_digest: "sha256:result".into(),
                    summary: None,
                },
                body: "setup finished".into(),
                handling: ContinuationHandling::Queue,
            },
            &record,
            1,
        )
        .await
        .expect("commit the setup completion");
    (jobs, receipt)
}

/// A host's committed setup result is confirmed through the same configured
/// job source as a fork_off or council completion, and resumes its retained
/// work on a governed runtime.
#[tokio::test]
async fn a_confirmed_host_setup_completion_is_resumed_on_a_governed_runtime() {
    let fixture = Fixture::new();
    let session = SessionId::new();
    let identity = work_identity("setup");
    let (jobs, receipt) = retained_setup_completion(&fixture, &session, &identity).await;
    let sink = Arc::new(RecordingSink::governed());
    let handle = fixture
        .owner()
        .arm(Host::with_jobs(sink.clone(), jobs))
        .expect("arm");
    let mut passes = handle.subscribe_passes();
    let owner = ContinuationOwner::Session {
        session_id: session.clone(),
    };
    let status = wait_for_status(&fixture, &owner, "setup:setup-1", &mut passes, |status| {
        matches!(status, ContinuationStatus::Applied { .. })
    })
    .await;
    let resumes = sink.resumes();
    assert_eq!(resumes.len(), 1);
    assert_eq!(resumes[0].0, session);
    assert_eq!(
        resumes[0].2, identity,
        "the resume names the setup's retained work"
    );
    assert!(sink.admissions().is_empty(), "never admitted unbound");
    assert!(
        matches!(status, ContinuationStatus::Applied { receipt: applied, .. } if applied == receipt)
    );
}

/// A host completion is confirmed only by a source that owns its exact
/// namespace and job: once the configured source no longer owns that
/// namespace, or its committed facts differ, the row is an immutably invalid
/// binding; an unreadable owner keeps it pending.
#[tokio::test]
async fn a_host_completion_needs_its_namespaces_owner_to_confirm_it_exactly() {
    for case in [
        "other_namespace",
        "other_result",
        "other_session",
        "unreadable",
    ] {
        let fixture = Fixture::new();
        let session = SessionId::new();
        let identity = work_identity("setup");
        let (jobs, receipt) = retained_setup_completion(&fixture, &session, &identity).await;
        match case {
            "other_namespace" => {
                *jobs.producer.lock().expect("lock") = Some(setup_producer("app.other"));
            }
            "other_result" => jobs.change("setup-1", |facts| {
                facts.result_digest = "sha256:another-result".into();
            }),
            "other_session" => jobs.change("setup-1", |facts| {
                facts.owner_session_id = SessionId::new();
            }),
            _ => *jobs.unavailable.lock().expect("lock") = Some("setup owner offline".into()),
        }
        let sink = Arc::new(RecordingSink::governed());
        let handle = fixture
            .owner()
            .arm(Host::with_jobs(sink.clone(), jobs))
            .expect("arm");
        let mut passes = handle.subscribe_passes();
        let pass = wait_for_pass(&mut passes, "the confirmation verdict applied", |pass| {
            pass.refused > 0 || !pass.blocked_runtimes.is_empty()
        })
        .await;
        assert!(
            sink.resumes().is_empty() && sink.admissions().is_empty(),
            "{case}"
        );
        let status = fixture
            .continuations
            .continuation_status(
                &ContinuationOwner::Session {
                    session_id: session.clone(),
                },
                &ContinuationKey::new("setup:setup-1").expect("key"),
            )
            .await
            .expect("status");
        if case == "unreadable" {
            assert_eq!(pass.refused, 0, "{case}: never settled");
            assert!(
                matches!(status, ContinuationStatus::Pending { .. }),
                "{case}"
            );
        } else {
            assert_eq!(
                status,
                ContinuationStatus::Refused {
                    receipt,
                    sequence: 1,
                    reason: meerkat_runtime::RuntimeDeliveryRefusalReason::NoAdmissibleWorkBinding
                },
                "{case}"
            );
        }
    }
}

fn setup_delivery(body: &str) -> ContinuationDelivery {
    ContinuationDelivery {
        key: ContinuationKey::new("setup:setup-1").expect("key"),
        result: ContinuationResultRef {
            producer: setup_producer(SETUP_NAMESPACE),
            producer_id: "setup-1".into(),
            result_digest: "sha256:result".into(),
            summary: None,
        },
        body: body.into(),
        handling: ContinuationHandling::Queue,
    }
}

/// The exact-content observation never commits: before a submission it
/// reports nothing submitted (and the key stays unbound); after it, the
/// exact content matches with its status; other content under the same key
/// is a typed mismatch, never that continuation's status.
#[tokio::test]
async fn an_exact_content_observation_never_commits_and_tells_other_content_apart() {
    let fixture = Fixture::new();
    let session = SessionId::new();
    let identity = work_identity("setup");
    let jobs = FakeJobs::holding("setup-1", committed_job(&session, &identity));
    *jobs.producer.lock().expect("lock") = Some(setup_producer(SETUP_NAMESPACE));
    let record = RetainedJobRecord::from_owner(&*jobs, setup_producer(SETUP_NAMESPACE), "setup-1")
        .await
        .expect("read the setup owner")
        .expect("the setup result is committed");
    let owner = ContinuationOwner::Session {
        session_id: session.clone(),
    };
    let key = ContinuationKey::new("setup:setup-1").expect("key");

    assert_eq!(
        fixture
            .continuations
            .observe_retained_completion(&owner, &setup_delivery("setup finished"), &record)
            .await
            .expect("observe"),
        meerkat::ContinuationObservation::NotSubmitted
    );
    assert_eq!(
        fixture
            .continuations
            .continuation_status(&owner, &key)
            .await
            .expect("status"),
        ContinuationStatus::NotCommitted,
        "observing committed nothing"
    );

    let receipt = fixture
        .continuations
        .submit_retained_completion(&owner, setup_delivery("setup finished"), &record, 1)
        .await
        .expect("commit the setup completion");
    match fixture
        .continuations
        .observe_retained_completion(&owner, &setup_delivery("setup finished"), &record)
        .await
        .expect("observe")
    {
        meerkat::ContinuationObservation::Matched {
            receipt: observed,
            status,
        } => {
            assert_eq!(observed, receipt);
            assert_eq!(
                status,
                fixture
                    .continuations
                    .continuation_status(&owner, &key)
                    .await
                    .expect("status")
            );
        }
        other => panic!("the exact content matches: {other:?}"),
    }
    assert!(matches!(
        fixture
            .continuations
            .observe_retained_completion(&owner, &setup_delivery("another body"), &record)
            .await,
        Err(meerkat::ContinuationObserveError::ContentMismatch { existing }) if *existing == receipt
    ));
    let mut other_result = setup_delivery("setup finished");
    other_result.result.result_digest = "sha256:another-result".into();
    assert!(matches!(
        fixture
            .continuations
            .observe_retained_completion(&owner, &other_result, &record)
            .await,
        Err(meerkat::ContinuationObserveError::Invalid(_))
    ));
}

/// A delivery host whose member resolution and job owner come from late-bound
/// [`meerkat::ContinuationHostBindings`], as the product hosts' do.
struct BindingsHost {
    bindings: Arc<meerkat::ContinuationHostBindings>,
    sink: Arc<RecordingSink>,
}

#[async_trait::async_trait]
impl RuntimeDeliveryHost for BindingsHost {
    async fn delivery_route(&self, _session_id: &SessionId) -> Option<meerkat::DeliveryRoute> {
        Some(meerkat::DeliveryRoute::ServedHere(Arc::new(NoJobs)))
    }

    async fn claim_cold_delivery(
        &self,
        session_id: &SessionId,
    ) -> Option<Result<meerkat_runtime::HostingClaim, meerkat_runtime::HostingRefused>> {
        // A single-process host: every session is served here.
        Some(meerkat_runtime::grant_session_hosting(
            &meerkat_runtime::HostingCapability::ProcessLocal,
            &meerkat_runtime::HostingOwner::mint(),
            session_id,
        ))
    }

    async fn resolve_address(&self, address: &LogicalRuntimeId) -> AddressResolution {
        self.bindings.resolve_address(address).await
    }

    async fn continuation_sink(
        &self,
        _session_id: &SessionId,
    ) -> Option<Arc<dyn ContinuationDeliverySink>> {
        Some(self.sink.clone())
    }

    fn retained_job_source(&self) -> Option<Arc<dyn RetainedJobSource>> {
        self.bindings.job_source()
    }
}

/// Serves every member address by one session.
struct ServedBy(SessionId);

#[async_trait::async_trait]
impl meerkat::ContinuationAddressResolver for ServedBy {
    async fn current_address(
        &self,
        owner: &ContinuationOwner,
    ) -> Result<Option<LogicalRuntimeId>, String> {
        SessionAddressResolver.current_address(owner).await
    }

    async fn resolve_address(
        &self,
        address: &LogicalRuntimeId,
    ) -> Result<AddressResolution, String> {
        Ok(
            match address
                .member_address()
                .map_err(|error| error.to_string())?
            {
                Some(_) => AddressResolution::Session(self.0.clone()),
                None => meerkat::default_address_resolution(address),
            },
        )
    }
}

/// Before the host binds its continuation services (the window between
/// arming the delivery owner and building the mob state), a member row is
/// not served and stays blocked, never refused or settled. Once they are
/// bound, the owner's next wake applies it.
#[tokio::test]
async fn a_member_row_before_the_host_binds_stays_blocked_until_a_wake_after_binding() {
    let fixture = Fixture::new();
    let sink = Arc::new(RecordingSink::default());
    let address =
        meerkat::member_delivery_address("team", team_incarnation(), "lead", 1).expect("address");
    let submission = meerkat_runtime::RuntimeDeliverySubmission::new(
        RuntimeDeliveryId::new("continuation:member-window").expect("id"),
        meerkat_runtime::RuntimeDeliveryKind::Continuation,
        "host:tasks",
        1,
        "task-1",
        serde_json::to_vec(&delivery("task-1", host_producer())).expect("encode"),
    )
    .expect("submission");
    fixture
        .inbox
        .submit(&address, submission)
        .await
        .expect("commit");
    let bindings = Arc::new(meerkat::ContinuationHostBindings::default());
    let (attachments, attachment_commits) = watch::channel(0_u64);
    let handle = fixture
        .owner()
        .with_attachment_commits(attachment_commits)
        .arm(Arc::new(BindingsHost {
            bindings: Arc::clone(&bindings),
            sink: sink.clone(),
        }))
        .expect("arm");
    let mut passes = handle.subscribe_passes();
    let pass = wait_for_pass(&mut passes, "the unbound member row blocked", |pass| {
        pass.blocked_runtimes.contains(&address)
    })
    .await;
    assert_eq!(pass.refused, 0, "never settled");
    assert!(sink.admissions().is_empty());

    let session = SessionId::new();
    let _binding = bindings
        .bind(
            Arc::new(ServedBy(session.clone())),
            Arc::new(FakeJobs::default()),
        )
        .expect("bind");
    attachments.send_modify(|generation| *generation += 1);
    wait_for_pass(&mut passes, "the member row applied", |pass| {
        !pass.blocked_runtimes.contains(&address) && pass.applied > 0
    })
    .await;
    assert_eq!(sink.admissions().len(), 1);
    assert_eq!(sink.admissions()[0].0, session);
}

/// On a governed runtime, a retained completion that reaches the owner
/// before the host binds its job owner stays pending, never refused; once
/// bound, the owner's next wake confirms and resumes it.
#[tokio::test]
async fn a_retained_completion_before_the_host_binds_stays_pending_until_a_wake_after_binding() {
    let fixture = Fixture::new();
    let session = SessionId::new();
    let identity = work_identity("a");
    let jobs = FakeJobs::holding("job-1", committed_job(&session, &identity));
    retained_fork_completion(&fixture, &session, &jobs).await;
    let sink = Arc::new(RecordingSink::governed());
    let bindings = Arc::new(meerkat::ContinuationHostBindings::default());
    let (attachments, attachment_commits) = watch::channel(0_u64);
    let handle = fixture
        .owner()
        .with_attachment_commits(attachment_commits)
        .arm(Arc::new(BindingsHost {
            bindings: Arc::clone(&bindings),
            sink: sink.clone(),
        }))
        .expect("arm");
    let mut passes = handle.subscribe_passes();
    let pass = wait_for_pass(&mut passes, "the unconfirmed row blocked", |pass| {
        !pass.blocked_runtimes.is_empty()
    })
    .await;
    assert_eq!(pass.refused, 0, "never settled");
    assert!(sink.resumes().is_empty());

    let _binding = bindings
        .bind(Arc::new(SessionAddressResolver), jobs)
        .expect("bind");
    attachments.send_modify(|generation| *generation += 1);
    wait_for_pass(&mut passes, "the completion resumed", |pass| {
        pass.applied > 0
    })
    .await;
    assert_eq!(sink.resumes().len(), 1);
    assert_eq!(sink.resumes()[0].2, identity);
}

/// An acknowledged cutpoint of the owner loop: commit an unrelated
/// continuation and wait for the pass that settles it. The loop captures
/// every signal before the pass it runs, so each signal published before
/// this call has been consumed, and acted on if it had work, when it
/// returns.
async fn loop_cutpoint(
    fixture: &Fixture,
    passes: &mut watch::Receiver<RuntimeDeliveryPass>,
) -> RuntimeDeliveryPass {
    let before = passes.borrow().generation;
    fixture
        .continuations
        .submit(
            &ContinuationOwner::Session {
                session_id: SessionId::new(),
            },
            delivery(&format!("cutpoint-{before}"), host_producer()),
            1,
        )
        .await
        .expect("commit the cutpoint row");
    wait_for_pass(passes, "the cutpoint pass", |pass| {
        pass.generation > before && pass.applied + pass.refused > 0
    })
    .await
}

/// A retained completion whose job owner is bound after the owner armed,
/// while its custody is absent (the host's pin is not recorded yet), stays
/// blocked. Restoring the custody alone wakes nothing: up to an acknowledged
/// cutpoint no one asked the source again. The job owner's announcement
/// wakes the same owner, which asks the source once and resumes the
/// completion exactly once with its exact retained work. A repeated
/// announcement with nothing blocked runs no pass and replays nothing.
#[tokio::test]
async fn a_source_hint_retries_a_blocked_retained_completion_exactly_once() {
    let fixture = Fixture::new();
    let session = SessionId::new();
    let identity = work_identity("custody");
    let jobs = FakeJobs::holding("job-1", committed_job(&session, &identity));
    retained_fork_completion(&fixture, &session, &jobs).await;
    *jobs.unavailable.lock().expect("lock") = Some("custody is not restored".into());
    let sink = Arc::new(RecordingSink::governed());
    let bindings = Arc::new(meerkat::ContinuationHostBindings::default());
    let handle = fixture
        .owner()
        .with_source_changes(bindings.subscribe_source_changes())
        .arm(Arc::new(BindingsHost {
            bindings: Arc::clone(&bindings),
            sink: sink.clone(),
        }))
        .expect("arm");
    let mut passes = handle.subscribe_passes();
    let unbound = wait_for_pass(&mut passes, "the unbound completion blocked", |pass| {
        !pass.blocked_runtimes.is_empty()
    })
    .await;
    let blocked = unbound.blocked_runtimes.clone();

    // Bound after arming while its custody is absent: the binding wakes the
    // owner, the source cannot answer, and the row stays blocked.
    let asked = jobs.lookups();
    let _binding = bindings
        .bind(Arc::new(SessionAddressResolver), jobs.clone())
        .expect("bind");
    let bound = wait_for_pass(&mut passes, "the bound source was asked", |pass| {
        pass.generation > unbound.generation
    })
    .await;
    assert_eq!(bound.blocked_runtimes, blocked, "still blocked");
    assert_eq!((bound.applied, bound.refused), (0, 0), "never settled");
    assert_eq!(
        jobs.lookups(),
        asked + 1,
        "the binding asked the source once"
    );

    // Custody restored but not announced: nothing retried it by the cutpoint.
    *jobs.unavailable.lock().expect("lock") = None;
    let asked = jobs.lookups();
    let quiet = loop_cutpoint(&fixture, &mut passes).await;
    assert_eq!(quiet.blocked_runtimes, blocked, "still blocked");
    assert_eq!(jobs.lookups(), asked, "nobody asked the source");
    assert!(sink.resumes().is_empty());

    // Announced: the same owner asks again and resumes it exactly once.
    bindings.source_custody_changed();
    let resumed = wait_for_pass(&mut passes, "the announced completion resumed", |pass| {
        pass.applied > 0
    })
    .await;
    assert!(resumed.blocked_runtimes.is_empty());
    assert_eq!(jobs.lookups(), asked + 1, "one fresh read of the source");
    assert_eq!(sink.resumes().len(), 1);
    assert_eq!(sink.resumes()[0].0, session);
    assert_eq!(sink.resumes()[0].2, identity);

    // Announced again: nothing is blocked, so no pass runs for it (the next
    // pass is the cutpoint's own) and nothing is replayed.
    bindings.source_custody_changed();
    let after = loop_cutpoint(&fixture, &mut passes).await;
    assert_eq!(
        after.generation,
        resumed.generation + 1,
        "no pass for the hint"
    );
    assert_eq!(jobs.lookups(), asked + 1, "the source was not asked again");
    assert_eq!(sink.resumes().len(), 1, "no replay");
    assert!(!handle.is_stopped());
}

/// A job owner bound before the owner armed is asked by the arming
/// reconcile. A custody change announced while a pass is inside its source
/// read (after it captured and marked the source revision) is not
/// swallowed: that read still answers what it saw, and the owner retries
/// once more, which resumes the completion exactly once.
#[tokio::test]
async fn a_source_change_published_during_a_blocked_pass_is_retried() {
    let fixture = Fixture::new();
    let session = SessionId::new();
    let identity = work_identity("during");
    let jobs = FakeJobs::holding("job-1", committed_job(&session, &identity));
    retained_fork_completion(&fixture, &session, &jobs).await;
    *jobs.unavailable.lock().expect("lock") = Some("custody is not restored".into());
    let sink = Arc::new(RecordingSink::governed());
    let bindings = Arc::new(meerkat::ContinuationHostBindings::default());
    let _binding = bindings
        .bind(Arc::new(SessionAddressResolver), jobs.clone())
        .expect("bind before arming");
    let asked = jobs.lookups();
    let handle = fixture
        .owner()
        .with_source_changes(bindings.subscribe_source_changes())
        .arm(Arc::new(BindingsHost {
            bindings: Arc::clone(&bindings),
            sink: sink.clone(),
        }))
        .expect("arm");
    let mut passes = handle.subscribe_passes();
    let armed = wait_for_pass(
        &mut passes,
        "the arming reconcile asked the source",
        |pass| !pass.blocked_runtimes.is_empty(),
    )
    .await;
    assert_eq!(
        jobs.lookups(),
        asked + 1,
        "the reconcile asked the bound source"
    );

    // A retry whose source read parks after deciding "unavailable".
    let (entered, release) = jobs.hold_next_lookup();
    bindings.source_custody_changed();
    entered.await.expect("the retry is inside its source read");
    // Published after that pass captured and marked the revision.
    *jobs.unavailable.lock().expect("lock") = None;
    bindings.source_custody_changed();
    release.send(()).expect("release the parked read");

    let resumed = wait_for_pass(
        &mut passes,
        "the change during the pass was retried",
        |pass| pass.applied > 0,
    )
    .await;
    assert!(
        resumed.generation >= armed.generation + 2,
        "a pass after the parked one"
    );
    assert_eq!(jobs.lookups(), asked + 3, "the parked read and one retry");
    assert_eq!(sink.resumes().len(), 1);
    assert_eq!(sink.resumes()[0].2, identity);
}

/// Through the bundle's own delivery owner and bindings, the composition
/// every facade host arms: a retained completion that blocked before its job
/// owner was bound is resumed by the binding alone, with no attachment, run
/// or custody signal ever raised.
#[cfg(all(feature = "session-store", feature = "memory-store"))]
#[tokio::test]
async fn binding_the_job_owner_alone_resumes_a_blocked_retained_completion() {
    let session_store: Arc<dyn meerkat::SessionStore> = Arc::new(meerkat::MemoryStore::new());
    let bundle = meerkat::PersistenceBundle::new(
        session_store,
        Arc::new(InMemoryRuntimeStore::new()),
        Arc::new(meerkat::MemoryBlobStore::new()),
    )
    .expect("bundle");
    let continuations = ContinuationOwnerService::new(
        bundle.runtime_delivery_inbox(),
        Arc::new(SessionAddressResolver),
        Arc::new(meerkat_runtime::MeerkatMachine::ephemeral()),
    );
    let session = SessionId::new();
    let identity = work_identity("bind-alone");
    let jobs = FakeJobs::holding("job-1", committed_job(&session, &identity));
    let record = RetainedJobRecord::from_owner(&*jobs, ContinuationProducer::ForkOff, "job-1")
        .await
        .expect("read the job owner")
        .expect("the job is committed");
    continuations
        .submit_retained_completion(
            &ContinuationOwner::Session {
                session_id: session.clone(),
            },
            fork_completion("job-1", "sha256:result"),
            &record,
            1,
        )
        .await
        .expect("commit the completion");
    let sink = Arc::new(RecordingSink::governed());
    let bindings = bundle.continuation_bindings();
    let handle = bundle
        .runtime_delivery_owner()
        .arm(Arc::new(BindingsHost {
            bindings: Arc::clone(&bindings),
            sink: sink.clone(),
        }))
        .expect("arm");
    let mut passes = handle.subscribe_passes();
    wait_for_pass(&mut passes, "the unbound completion blocked", |pass| {
        !pass.blocked_runtimes.is_empty()
    })
    .await;
    assert!(sink.resumes().is_empty());

    let _binding = bindings
        .bind(Arc::new(SessionAddressResolver), jobs)
        .expect("bind");
    wait_for_pass(&mut passes, "the binding resumed the completion", |pass| {
        pass.applied > 0
    })
    .await;
    assert_eq!(sink.resumes().len(), 1);
    assert_eq!(sink.resumes()[0].2, identity);
}

/// A source hint grants nothing: the retry it wakes re-reads the job
/// owner's committed facts, and a completion they no longer confirm is
/// refused, never resumed.
#[tokio::test]
async fn a_source_hint_never_bypasses_the_current_refusal_checks() {
    let fixture = Fixture::new();
    let session = SessionId::new();
    let identity = work_identity("refused");
    let jobs = FakeJobs::holding("job-1", committed_job(&session, &identity));
    retained_fork_completion(&fixture, &session, &jobs).await;
    *jobs.unavailable.lock().expect("lock") = Some("custody is not restored".into());
    let sink = Arc::new(RecordingSink::governed());
    let bindings = Arc::new(meerkat::ContinuationHostBindings::default());
    let _binding = bindings
        .bind(Arc::new(SessionAddressResolver), jobs.clone())
        .expect("bind");
    let handle = fixture
        .owner()
        .with_source_changes(bindings.subscribe_source_changes())
        .arm(Arc::new(BindingsHost {
            bindings: Arc::clone(&bindings),
            sink: sink.clone(),
        }))
        .expect("arm");
    let mut passes = handle.subscribe_passes();
    wait_for_pass(&mut passes, "the completion blocked", |pass| {
        !pass.blocked_runtimes.is_empty()
    })
    .await;

    jobs.change("job-1", |facts| {
        facts.retained_work = Some(work_identity("another"));
    });
    *jobs.unavailable.lock().expect("lock") = None;
    bindings.source_custody_changed();
    let refused = wait_for_pass(&mut passes, "the retried completion refused", |pass| {
        pass.refused > 0
    })
    .await;
    assert!(refused.blocked_runtimes.is_empty(), "settled as refused");
    assert_eq!(refused.applied, 0);
    assert!(sink.resumes().is_empty(), "never resumed");
}

/// Dropping the bindings ends their source hint: the owner neither stops
/// nor runs a pass for the closed signal (the next pass is the cutpoint's
/// own), and it keeps serving its other signals.
#[tokio::test]
async fn dropped_bindings_end_their_source_hint_and_the_owner_keeps_serving() {
    let fixture = Fixture::new();
    let sink = Arc::new(RecordingSink::default());
    let hinting = Arc::new(meerkat::ContinuationHostBindings::default());
    let handle = fixture
        .owner()
        .with_source_changes(hinting.subscribe_source_changes())
        .arm(Arc::new(BindingsHost {
            bindings: Arc::new(meerkat::ContinuationHostBindings::default()),
            sink: sink.clone(),
        }))
        .expect("arm");
    let mut passes = handle.subscribe_passes();
    let armed = wait_for_pass(&mut passes, "the reconcile pass", |pass| {
        pass.generation >= 1
    })
    .await;

    drop(hinting);
    let first = loop_cutpoint(&fixture, &mut passes).await;
    assert_eq!(
        first.generation,
        armed.generation + 1,
        "no pass for the closure"
    );
    let second = loop_cutpoint(&fixture, &mut passes).await;
    assert_eq!(second.generation, first.generation + 1);
    assert!(!handle.is_stopped(), "a closed hint never stops the owner");
    assert_eq!(sink.admissions().len(), 2, "both cutpoint rows admitted");
}

/// The binding slot is set once per binder. A second binder is refused while
/// the first lives; a takeover must name the current binding's generation,
/// and a stale one is refused, typed. A dropped binding frees the slot.
#[tokio::test]
async fn a_stale_or_blind_continuation_rebind_is_refused() {
    let bindings = meerkat::ContinuationHostBindings::default();
    let member =
        meerkat::member_delivery_address("team", team_incarnation(), "lead", 1).expect("address");
    let (a, b) = (SessionId::new(), SessionId::new());
    let first = bindings
        .bind(Arc::new(ServedBy(a.clone())), Arc::new(FakeJobs::default()))
        .expect("bind a free slot");
    assert_eq!(
        bindings
            .bind(Arc::new(ServedBy(b.clone())), Arc::new(FakeJobs::default()))
            .expect_err("a live binding holds the slot"),
        meerkat::ContinuationBindError::AlreadyBound(first.generation())
    );
    assert_eq!(
        bindings.resolve_address(&member).await,
        AddressResolution::Session(a.clone())
    );

    let second = bindings
        .rebind(
            first.generation(),
            Arc::new(ServedBy(b.clone())),
            Arc::new(FakeJobs::default()),
        )
        .expect("a takeover naming the current binding");
    assert_eq!(
        bindings
            .rebind(
                first.generation(),
                Arc::new(ServedBy(a.clone())),
                Arc::new(FakeJobs::default()),
            )
            .expect_err("a stale takeover"),
        meerkat::ContinuationBindError::StaleGeneration {
            replaced: first.generation(),
            current: second.generation(),
        }
    );
    assert_eq!(
        bindings.resolve_address(&member).await,
        AddressResolution::Session(b),
        "the stale takeover changed nothing"
    );

    drop(second);
    assert_eq!(
        bindings.resolve_address(&member).await,
        AddressResolution::NotServed,
        "a dropped binding serves nothing"
    );
    drop(first);
    bindings
        .bind(Arc::new(ServedBy(a)), Arc::new(FakeJobs::default()))
        .expect("a freed slot binds again");
}

/// #1813 on continuation rows: a continuation's one recipient is the session
/// serving its address, routed by that session's hosting claim BEFORE any
/// reservation or admission. A peer process is simulated by holding its OS
/// locks through separate open files (flock conflicts per open file, exactly
/// as across processes) on a SQLite store with OS-locked claims; its death is
/// closing them.
#[cfg(feature = "session-store")]
mod hosting {
    use super::*;

    use meerkat_runtime::{
        HostingCapability, HostingClaim, HostingOwner, HostingPaths, HostingRefused,
        SessionHostingAuthority, SessionServing, SqliteRuntimeStore,
    };

    /// The owner's store-watch sweep here: how soon a released claim or lock
    /// is noticed.
    const TEST_SWEEP: Duration = Duration::from_millis(50);

    struct Hosted {
        fixture: Fixture,
        paths: HostingPaths,
        capability: HostingCapability,
    }

    fn hosted() -> Hosted {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = HostingPaths {
            hosting_lock_dir: dir.path().join("hosting"),
            cold_delivery_lock: dir.path().join("delivery").join("cold-delivery.lock"),
            database: None,
        };
        let store = Arc::new(
            SqliteRuntimeStore::new(dir.path().join("runtime.sqlite3"))
                .expect("open store")
                .with_hosting_paths(paths.clone()),
        );
        let capability = RuntimeStore::hosting_capability(store.as_ref());
        assert!(capability.is_cross_process());
        Hosted {
            fixture: Fixture::on(store, Some(dir)),
            paths,
            capability,
        }
    }

    /// A peer process's hold on one lock file. Dropping it is its death.
    struct PeerLock {
        _file: std::fs::File,
    }

    fn peer_lock(path: &std::path::Path) -> PeerLock {
        std::fs::create_dir_all(path.parent().expect("lock parent")).expect("lock dir");
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .expect("open lock file");
        file.try_lock().expect("the peer takes the lock");
        PeerLock { _file: file }
    }

    /// Records, for each admission, where the admitted session was served
    /// for the host's runtime owner at that moment.
    struct ObservingSink {
        inner: RecordingSink,
        authority: SessionHostingAuthority,
        served_at_admission: Mutex<Vec<SessionServing>>,
    }

    impl ObservingSink {
        fn served_at_admission(&self) -> Vec<SessionServing> {
            self.served_at_admission.lock().expect("lock").clone()
        }
    }

    #[async_trait::async_trait]
    impl ContinuationDeliverySink for ObservingSink {
        async fn admit(
            &self,
            session: &SessionId,
            input: meerkat_runtime::Input,
        ) -> Result<InputId, String> {
            self.served_at_admission
                .lock()
                .expect("lock")
                .push(self.authority.serving(session));
            self.inner.admit(session, input).await
        }

        async fn admitted_input(
            &self,
            session: &SessionId,
            admission_key: &str,
        ) -> Result<Option<StoredInputState>, String> {
            self.inner.admitted_input(session, admission_key).await
        }
    }

    /// Routes by its runtime owner's hosting claims, as a product host does:
    /// the claim registry for routing, a real grant for a cold claim.
    struct ClaimHost {
        authority: SessionHostingAuthority,
        sink: Arc<ObservingSink>,
        /// Member address resolutions the test moves (a repoint).
        resolutions: Mutex<HashMap<LogicalRuntimeId, AddressResolution>>,
    }

    impl ClaimHost {
        fn new(capability: &HostingCapability) -> Arc<Self> {
            let authority = SessionHostingAuthority::new(capability.clone(), HostingOwner::mint());
            Arc::new(Self {
                sink: Arc::new(ObservingSink {
                    inner: RecordingSink::default(),
                    authority: authority.clone(),
                    served_at_admission: Mutex::new(Vec::new()),
                }),
                authority,
                resolutions: Mutex::new(HashMap::new()),
            })
        }

        fn resolve(&self, address: &LogicalRuntimeId, resolution: AddressResolution) {
            self.resolutions
                .lock()
                .expect("lock")
                .insert(address.clone(), resolution);
        }
    }

    #[async_trait::async_trait]
    impl RuntimeDeliveryHost for ClaimHost {
        async fn delivery_route(&self, session_id: &SessionId) -> Option<meerkat::DeliveryRoute> {
            Some(match self.authority.serving(session_id) {
                SessionServing::HeldHere => meerkat::DeliveryRoute::ServedHere(Arc::new(NoJobs)),
                SessionServing::HeldByAnotherLocalOwner => meerkat::DeliveryRoute::ServedElsewhere,
                SessionServing::NotHeldInThisProcess => {
                    meerkat::DeliveryRoute::Unserved(Arc::new(NoJobs))
                }
            })
        }

        async fn claim_cold_delivery(
            &self,
            session_id: &SessionId,
        ) -> Option<Result<HostingClaim, HostingRefused>> {
            Some(self.authority.grant(session_id))
        }

        async fn continuation_sink(
            &self,
            _session_id: &SessionId,
        ) -> Option<Arc<dyn ContinuationDeliverySink>> {
            Some(self.sink.clone())
        }

        async fn resolve_address(&self, address: &LogicalRuntimeId) -> AddressResolution {
            self.resolutions
                .lock()
                .expect("lock")
                .get(address)
                .cloned()
                .unwrap_or_else(|| meerkat::default_address_resolution(address))
        }
    }

    impl Hosted {
        fn arm(&self, host: Arc<ClaimHost>) -> meerkat::RuntimeDeliveryOwnerHandle {
            self.fixture
                .owner()
                .with_store_sweep(TEST_SWEEP)
                .arm(host)
                .expect("arm")
        }

        async fn submit(&self, session: &SessionId, key: &str) -> meerkat::ContinuationReceipt {
            self.fixture
                .continuations
                .submit(
                    &ContinuationOwner::Session {
                        session_id: session.clone(),
                    },
                    delivery(key, host_producer()),
                    1,
                )
                .await
                .expect("commit")
        }

        /// The row is untouched: no reservation or admission recorded, and
        /// its status still pending with nothing admitted.
        async fn assert_untouched(
            &self,
            session: &SessionId,
            receipt: &meerkat::ContinuationReceipt,
        ) {
            assert_eq!(
                self.fixture
                    .inbox
                    .continuation_admission(
                        &LogicalRuntimeId::new(receipt.address.clone()),
                        &RuntimeDeliveryId::new(receipt.delivery_id.clone()).expect("id"),
                    )
                    .await
                    .expect("admission index"),
                None,
                "no reservation was written for {:?}",
                receipt.key
            );
            let status = self
                .fixture
                .continuations
                .continuation_status(
                    &ContinuationOwner::Session {
                        session_id: session.clone(),
                    },
                    &receipt.key,
                )
                .await
                .expect("status");
            assert!(
                matches!(status, ContinuationStatus::Pending { admitted: None, .. }),
                "{status:?}"
            );
        }

        async fn wait_applied(
            &self,
            session: &SessionId,
            key: &str,
            passes: &mut watch::Receiver<RuntimeDeliveryPass>,
        ) {
            wait_for_status(
                &self.fixture,
                &ContinuationOwner::Session {
                    session_id: session.clone(),
                },
                key,
                passes,
                |status| matches!(status, ContinuationStatus::Applied { .. }),
            )
            .await;
        }
    }

    async fn first_pass(passes: &mut watch::Receiver<RuntimeDeliveryPass>) -> RuntimeDeliveryPass {
        wait_for_pass(passes, "the reconcile pass", |pass| pass.generation >= 1).await
    }

    /// Another runtime owner of this process hosts the session (the route is
    /// ServedElsewhere): both rows stay pending and untouched, the second in
    /// order behind the first. Once that owner releases the session, the cold
    /// owner admits each exactly once, in order, under its own claim.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_continuation_for_a_session_another_local_owner_hosts_waits_untouched_in_order() {
        let hosted = hosted();
        let session = SessionId::new();
        let other_owner =
            SessionHostingAuthority::new(hosted.capability.clone(), HostingOwner::mint());
        let held = other_owner
            .grant(&session)
            .expect("the other owner hosts it");
        let first = hosted.submit(&session, "task-1").await;
        let second = hosted.submit(&session, "task-2").await;

        let host = ClaimHost::new(&hosted.capability);
        let handle = hosted.arm(host.clone());
        let mut passes = handle.subscribe_passes();
        let pass = first_pass(&mut passes).await;
        assert_eq!(pass.awaiting_other_hosts, 1, "{pass:?}");
        assert_eq!(pass.applied, 0, "{pass:?}");
        assert!(host.sink.inner.admissions().is_empty());
        hosted.assert_untouched(&session, &first).await;
        hosted.assert_untouched(&session, &second).await;
        assert_eq!(
            hosted
                .fixture
                .inbox
                .pending_delivery_total()
                .await
                .expect("backlog"),
            2,
            "the later row stays pending behind the first"
        );

        drop(held);
        hosted.wait_applied(&session, "task-2", &mut passes).await;
        let keys: Vec<String> = host
            .sink
            .inner
            .admissions()
            .into_iter()
            .map(|(admitted, key, _)| {
                assert_eq!(admitted, session);
                key
            })
            .collect();
        assert_eq!(keys.len(), 2, "each admitted exactly once: {keys:?}");
        assert!(
            keys[0].contains(&first.delivery_id) && keys[1].contains(&second.delivery_id),
            "in inbox order: {keys:?}"
        );
        assert_eq!(
            host.sink.served_at_admission(),
            vec![SessionServing::HeldHere, SessionServing::HeldHere],
            "admitted only under this owner's claim"
        );
    }

    /// Two processes on one SQLite realm: a peer process hosts the session.
    /// The route is Unserved (nothing in this process holds the claim), this
    /// process is the cold owner, and its claim attempt is refused: no
    /// reservation is written. After the peer dies the cold owner's claim
    /// succeeds and it admits the row exactly once, under that claim.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_refused_cold_claim_writes_no_reservation_until_the_peer_process_dies() {
        let hosted = hosted();
        let session = SessionId::new();
        let peer = peer_lock(&meerkat_core::session_hosting::session_hosting_lock_path(
            &hosted.paths.hosting_lock_dir,
            &session,
        ));
        let receipt = hosted.submit(&session, "task-1").await;

        let host = ClaimHost::new(&hosted.capability);
        let handle = hosted.arm(host.clone());
        let mut passes = handle.subscribe_passes();
        let pass = first_pass(&mut passes).await;
        assert!(pass.applies_cold_deliveries, "{pass:?}");
        assert_eq!(pass.awaiting_other_hosts, 1, "{pass:?}");
        assert!(host.sink.inner.admissions().is_empty());
        hosted.assert_untouched(&session, &receipt).await;

        drop(peer);
        hosted.wait_applied(&session, "task-1", &mut passes).await;
        assert_eq!(
            host.sink.inner.admissions().len(),
            1,
            "admitted exactly once"
        );
        assert_eq!(
            host.sink.served_at_admission(),
            vec![SessionServing::HeldHere],
            "admitted under the cold claim taken first"
        );
    }

    /// While another process is the store's cold-delivery owner, a
    /// continuation for a session no process hosts is not this process's to
    /// apply: untouched. When the cold lock is released this process takes it
    /// and admits the row exactly once.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_cold_continuation_is_left_to_the_cold_delivery_owner() {
        let hosted = hosted();
        let other_cold_owner = peer_lock(&hosted.paths.cold_delivery_lock);
        let session = SessionId::new();
        let receipt = hosted.submit(&session, "task-1").await;

        let host = ClaimHost::new(&hosted.capability);
        let handle = hosted.arm(host.clone());
        let mut passes = handle.subscribe_passes();
        let pass = first_pass(&mut passes).await;
        assert!(!pass.applies_cold_deliveries, "{pass:?}");
        assert_eq!(pass.awaiting_other_hosts, 1, "{pass:?}");
        assert!(host.sink.inner.admissions().is_empty());
        hosted.assert_untouched(&session, &receipt).await;

        drop(other_cold_owner);
        hosted.wait_applied(&session, "task-1", &mut passes).await;
        assert_eq!(
            host.sink.inner.admissions().len(),
            1,
            "admitted exactly once"
        );
        assert_eq!(
            host.sink.served_at_admission(),
            vec![SessionServing::HeldHere]
        );
    }

    /// The store selected OS-locked claims, but the session's claim is
    /// unavailable (a regular file where the lock directory belongs): the
    /// cold claim is refused typed and nothing is reserved. Once the lock
    /// directory is usable the row is admitted exactly once.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_unavailable_session_claim_leaves_the_continuation_untouched() {
        let hosted = hosted();
        let session = SessionId::new();
        std::fs::write(&hosted.paths.hosting_lock_dir, b"not a directory")
            .expect("block the lock dir");
        let receipt = hosted.submit(&session, "task-1").await;

        let host = ClaimHost::new(&hosted.capability);
        assert!(matches!(
            host.authority.grant(&session),
            Err(HostingRefused::Unavailable(_))
        ));
        let handle = hosted.arm(host.clone());
        let mut passes = handle.subscribe_passes();
        let pass = first_pass(&mut passes).await;
        assert!(pass.applies_cold_deliveries, "{pass:?}");
        assert_eq!(pass.awaiting_other_hosts, 1, "{pass:?}");
        assert!(host.sink.inner.admissions().is_empty());
        hosted.assert_untouched(&session, &receipt).await;

        std::fs::remove_file(&hosted.paths.hosting_lock_dir).expect("unblock the lock dir");
        hosted.wait_applied(&session, "task-1", &mut passes).await;
        assert_eq!(
            host.sink.inner.admissions().len(),
            1,
            "admitted exactly once"
        );
        assert_eq!(
            host.sink.served_at_admission(),
            vec![SessionServing::HeldHere]
        );
    }

    /// A member continuation committed while its address resolves to A,
    /// which another runtime owner hosts, then the member repoints to B
    /// without changing its address. Nothing new is committed (the delivery
    /// generation does not move) and A stays hosted elsewhere.
    async fn repointed_member(
        fixture: &Fixture,
        capability: &HostingCapability,
        host: &ClaimHost,
    ) -> (
        LogicalRuntimeId,
        SessionId,
        SessionId,
        SessionHostingAuthority,
        HostingClaim,
    ) {
        let address = meerkat::member_delivery_address("team", team_incarnation(), "lead", 1)
            .expect("address");
        let (a, b) = (SessionId::new(), SessionId::new());
        let elsewhere = SessionHostingAuthority::new(capability.clone(), HostingOwner::mint());
        let a_claim = elsewhere.grant(&a).expect("another owner hosts A");
        host.resolve(&address, AddressResolution::Session(a.clone()));
        let submission = meerkat_runtime::RuntimeDeliverySubmission::new(
            RuntimeDeliveryId::new("continuation:member").expect("id"),
            meerkat_runtime::RuntimeDeliveryKind::Continuation,
            "host:tasks",
            1,
            "task-1",
            serde_json::to_vec(&delivery("task-1", host_producer())).expect("encode"),
        )
        .expect("submission");
        fixture
            .inbox
            .submit(&address, submission)
            .await
            .expect("commit");
        (address, a, b, elsewhere, a_claim)
    }

    /// ADR correction: the store-watch retry of a waiting continuation
    /// re-resolves its address instead of routing the cached session, so the
    /// row progresses through B once B is served here, although A is still
    /// hosted elsewhere and nothing new was committed.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_waiting_member_continuation_follows_a_repoint_on_the_store_sweep() {
        let hosted = hosted();
        let host = ClaimHost::new(&hosted.capability);
        let (address, a, b, _elsewhere, _a_claim) =
            repointed_member(&hosted.fixture, &hosted.capability, &host).await;
        let generation = hosted
            .fixture
            .inbox
            .delivery_generation()
            .await
            .expect("generation");
        let handle = hosted.arm(host.clone());
        let mut passes = handle.subscribe_passes();
        let pass = first_pass(&mut passes).await;
        assert_eq!(
            pass.awaiting_other_hosts, 1,
            "A is hosted elsewhere: {pass:?}"
        );
        assert!(host.sink.inner.admissions().is_empty());
        // While A is still resolved, the waiting pass committed nothing.
        // Only then is B made eligible: from there on the owner may admit
        // the row, which legitimately advances the generation.
        assert_eq!(
            hosted
                .fixture
                .inbox
                .delivery_generation()
                .await
                .expect("generation"),
            generation,
            "nothing new was committed while the row waited on A"
        );
        host.resolve(&address, AddressResolution::Session(b.clone()));
        let _b_claim = host.authority.grant(&b).expect("B is served here");
        wait_for_pass(&mut passes, "the row applied through B", |pass| {
            pass.applied > 0 || pass.awaiting_other_hosts == 0
        })
        .await;
        assert_eq!(
            host.sink
                .inner
                .admissions()
                .iter()
                .map(|(session, ..)| session.clone())
                .collect::<Vec<_>>(),
            vec![b.clone()],
            "admitted once, into B, never into A ({a})"
        );
        assert_eq!(
            host.sink.served_at_admission(),
            vec![SessionServing::HeldHere]
        );
        assert_eq!(
            hosted
                .fixture
                .inbox
                .pending_delivery_total()
                .await
                .expect("backlog"),
            0
        );
    }

    /// ADR correction, attachment path: the delivery store is process-local,
    /// so the owner runs no store watcher and no tick can ever retry the row
    /// (the claims the host routes by live in their own OS-locked registry).
    /// Only the attachment wake that serves B (the host's mapping change) can
    /// retry the waiting continuation, with a fresh resolution.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_waiting_member_continuation_follows_a_repoint_on_an_attachment_wake() {
        let fixture = Fixture::new();
        let claims_dir = tempfile::tempdir().expect("tempdir");
        let capability = HostingCapability::OsLock(Arc::new(HostingPaths {
            hosting_lock_dir: claims_dir.path().join("hosting"),
            cold_delivery_lock: claims_dir
                .path()
                .join("delivery")
                .join("cold-delivery.lock"),
            database: None,
        }));
        let host = ClaimHost::new(&capability);
        let (address, a, b, _elsewhere, _a_claim) =
            repointed_member(&fixture, &capability, &host).await;
        let (attachments, attachment_commits) = watch::channel(0_u64);
        let handle = fixture
            .owner()
            .with_attachment_commits(attachment_commits)
            .arm(host.clone())
            .expect("arm");
        let mut passes = handle.subscribe_passes();
        let pass = first_pass(&mut passes).await;
        assert_eq!(
            pass.awaiting_other_hosts, 1,
            "A is hosted elsewhere: {pass:?}"
        );
        assert!(
            pass.cross_process_wake_unavailable.is_none()
                && !pass.blocked_runtimes.contains(&address),
            "a process-local store is unwatched and the row only awaits: {pass:?}"
        );

        host.resolve(&address, AddressResolution::Session(b.clone()));
        let _b_claim = host.authority.grant(&b).expect("B is served here");
        attachments.send_modify(|generation| *generation += 1);
        wait_for_pass(&mut passes, "the row applied through B", |pass| {
            pass.applied > 0 || pass.awaiting_other_hosts == 0
        })
        .await;
        assert_eq!(
            host.sink
                .inner
                .admissions()
                .iter()
                .map(|(session, ..)| session.clone())
                .collect::<Vec<_>>(),
            vec![b.clone()],
            "admitted once, into B, never into A ({a})"
        );
        assert_eq!(
            fixture
                .inbox
                .pending_delivery_total()
                .await
                .expect("backlog"),
            0
        );
    }

    /// The store's kept delivery-generation reader under a delivery
    /// owner. Unix only: the store keeps the reader on unix.
    #[cfg(unix)]
    mod kept_reader {
        use super::*;

        use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

        use meerkat_runtime::store::sqlite::delivery_generation_test_hooks as hooks;

        /// A store watched for other processes' commits, with its database
        /// path (the key of the delivery-generation test hooks).
        fn watched_store() -> (Fixture, std::path::PathBuf, HostingCapability) {
            let dir = tempfile::tempdir().expect("tempdir");
            let paths = HostingPaths {
                hosting_lock_dir: dir.path().join("hosting"),
                cold_delivery_lock: dir.path().join("delivery").join("cold-delivery.lock"),
                database: None,
            };
            let store = Arc::new(
                SqliteRuntimeStore::new(dir.path().join("runtime.sqlite3"))
                    .expect("open store")
                    .with_hosting_paths(paths),
            );
            let path = store.path().to_path_buf();
            let capability = RuntimeStore::hosting_capability(store.as_ref());
            assert!(capability.is_cross_process());
            (Fixture::on(store, Some(dir)), path, capability)
        }

        /// Counts the kept reads of the database at `path`: reads on a kept
        /// connection, not its verified opens.
        fn count_kept_reads(path: &std::path::Path) -> Arc<AtomicU64> {
            let reads = Arc::new(AtomicU64::new(0));
            hooks::before_kept_read_guard(path, {
                let reads = Arc::clone(&reads);
                move || {
                    reads.fetch_add(1, Ordering::SeqCst);
                }
            });
            reads
        }

        /// Whether maintenance gets the database at `path` now: no kept
        /// connection holds its presence lock. The fence is released at once.
        fn maintenance_gets_the_database(path: &std::path::Path) -> bool {
            meerkat_sqlite::ExclusiveFence::try_acquire(path)
                .expect("io")
                .is_some()
        }

        /// A quiet watched store ticks its owner every sweep, and
        /// every tick reads the delivery generation. Through the store's kept
        /// reader the owner opens the database once and reads on the kept
        /// connection at its ticks; through the fresh-open path (the one
        /// every tick took before) it opens it on every tick.
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn a_quiet_store_ticks_on_one_kept_open_instead_of_one_open_per_tick() {
            const SWEEP: Duration = Duration::from_millis(5);

            let (fresh, fresh_path, capability) = watched_store();
            hooks::force_fresh_reads(&fresh_path);
            let handle = fresh
                .owner()
                .with_store_sweep(SWEEP)
                .arm(ClaimHost::new(&capability))
                .expect("arm the fresh owner");
            let started = std::time::Instant::now();
            while hooks::verified_opens(&fresh_path) < 100 {
                assert!(
                    started.elapsed() < Duration::from_secs(60),
                    "100 store ticks never came"
                );
                tokio::time::sleep(SWEEP).await;
            }
            let hundred_ticks = started.elapsed();
            drop(handle);

            let (kept, kept_path, capability) = watched_store();
            let kept_reads = count_kept_reads(&kept_path);
            let handle = kept
                .owner()
                .with_store_sweep(SWEEP)
                .arm(ClaimHost::new(&capability))
                .expect("arm the kept owner");
            tokio::time::sleep(hundred_ticks).await;
            assert_eq!(
                hooks::verified_opens(&kept_path),
                1,
                "as many ticks as the fresh owner took for 100 opens cost one open"
            );
            let reads = kept_reads.load(Ordering::SeqCst);
            assert!(
                reads >= 50,
                "the kept owner reads at its ticks: {reads} kept reads in the time the fresh \
                 owner opened 100 times"
            );
            drop(handle);
        }

        /// The kept reader's connection closes once the store has been
        /// quiet for `KEPT_DELIVERY_READER_IDLE`, and with it the presence
        /// lock the maintenance fence waits for (paused time).
        #[tokio::test(start_paused = true)]
        async fn a_quiet_kept_reader_closes_at_its_idle_bound() {
            let (fixture, path, capability) = watched_store();
            // No sweep inside the test: the arming pass is the only read.
            let handle = fixture
                .owner()
                .with_store_sweep(Duration::from_secs(3600))
                .arm(ClaimHost::new(&capability))
                .expect("arm");
            let mut passes = handle.subscribe_passes();
            first_pass(&mut passes).await;
            assert!(
                !maintenance_gets_the_database(&path),
                "the arming read keeps its connection, so maintenance waits"
            );

            tokio::time::advance(
                meerkat_runtime::KEPT_DELIVERY_READER_IDLE
                    .saturating_sub(Duration::from_millis(50)),
            )
            .await;
            tokio::task::yield_now().await;
            assert!(
                !maintenance_gets_the_database(&path),
                "kept until the idle bound"
            );

            tokio::time::advance(Duration::from_millis(100)).await;
            let mut released = false;
            for _ in 0..1000 {
                tokio::task::yield_now().await;
                released = maintenance_gets_the_database(&path);
                if released {
                    break;
                }
            }
            assert!(
                released,
                "the quiet reader closed at its idle bound, and maintenance gets the database"
            );
            drop(handle);
        }

        /// Longer than any tick of a reader's own open or close would take:
        /// the store watch coalesces for 10 ms.
        const SELF_TICK_WINDOW: Duration = Duration::from_millis(500);

        /// Let the owner answer whatever the store watch delivers within
        /// `window` of real time, while the paused clock stands still.
        async fn answer_the_watch_for(window: Duration) {
            let started = std::time::Instant::now();
            while started.elapsed() < window {
                tokio::task::yield_now().await;
                std::thread::sleep(Duration::from_millis(1));
            }
        }

        /// A sweep no test here waits for.
        const NO_SWEEP: Duration = Duration::from_secs(3600);

        /// Arm the owner of a watched store with `sweep`, its store watch
        /// registered before the owner's first read: the owner task runs only
        /// once the test awaits (current-thread runtime).
        /// `wal_lifecycle_as_modified` makes the watch report the WAL created
        /// or removed also as modified, as ReadDirectoryChangesW can.
        fn arm_registered(
            fixture: &Fixture,
            path: &std::path::Path,
            capability: &HostingCapability,
            sweep: Duration,
            wal_lifecycle_as_modified: bool,
        ) -> meerkat::RuntimeDeliveryOwnerHandle {
            let registrations = meerkat_sqlite::watch::test_support::observe_registrations_in(
                path.parent().expect("the database's directory"),
            )
            .expect("observe the store watch's registration");
            if wal_lifecycle_as_modified {
                registrations.report_wal_lifecycle_as_modified();
            }
            let handle = fixture
                .owner()
                .with_store_sweep(sweep)
                .arm(ClaimHost::new(capability))
                .expect("arm");
            assert!(
                registrations.wait_registered(1, EVENT_GUARD),
                "the store watch registers"
            );
            handle
        }

        /// Let the owner run until `done` holds, for at most `bound` of real
        /// time: the store watch is a thread of its own, and the paused clock
        /// stands still.
        async fn wait_real(what: &str, bound: Duration, mut done: impl FnMut() -> bool) {
            let started = std::time::Instant::now();
            while !done() {
                assert!(started.elapsed() < bound, "never happened: {what}");
                tokio::task::yield_now().await;
                std::thread::sleep(Duration::from_millis(1));
            }
        }

        /// The database's WAL file.
        fn wal_of(path: &std::path::Path) -> std::path::PathBuf {
            let mut wal = path.as_os_str().to_owned();
            wal.push("-wal");
            wal.into()
        }

        /// Move the store's delivery generation on through a connection of
        /// the test's own, as another process's delivery would (the delivery
        /// domain is provisioned already).
        fn move_the_delivery_generation(conn: &meerkat_sqlite::Connection) {
            conn.execute(
                "UPDATE runtime_delivery_generation SET generation = generation + 1 \
                 WHERE singleton = 1",
                [],
            )
            .expect("move the delivery generation");
        }

        /// Provision the store's delivery domain, as a store's first delivery
        /// write does, through a store of the test's own on the same file: a
        /// compare-and-swap that expects a revision no row has conflicts and
        /// commits nothing, so no runtime is left for the owner to route.
        async fn provision_the_delivery_domain(path: &std::path::Path) {
            let store = SqliteRuntimeStore::new(path.to_path_buf()).expect("another store");
            let outcome = store
                .compare_and_swap_runtime_delivery_authority(
                    &LogicalRuntimeId::new("provisioning"),
                    Some(1),
                    meerkat_runtime::RuntimeDeliveryAuthorityRecord::from_parts(2, b"{}".to_vec()),
                    None,
                )
                .await
                .expect("the delivery write connection");
            assert!(
                matches!(
                    outcome,
                    meerkat_runtime::RuntimeDeliveryAuthorityCasOutcome::Conflict(_)
                ),
                "{outcome:?}"
            );
        }

        /// The owner's own reads are no store change. Its arming read opens
        /// the database as the store's first connection, creating the WAL and
        /// its index, and its idle release closes it as the last, removing
        /// them; neither wakes the owner, so a quiet store's reader reads
        /// once, closes at its idle bound and stays closed (paused time).
        #[tokio::test(start_paused = true)]
        async fn a_quiet_kept_reader_wakes_on_nothing_it_does_itself() {
            let (fixture, path, capability) = watched_store();
            let kept_reads = count_kept_reads(&path);
            let handle = arm_registered(&fixture, &path, &capability, NO_SWEEP, false);
            let mut passes = handle.subscribe_passes();
            first_pass(&mut passes).await;
            answer_the_watch_for(SELF_TICK_WINDOW).await;
            assert_eq!(
                kept_reads.load(Ordering::SeqCst),
                0,
                "the arming read's own open woke the owner"
            );
            assert_eq!(hooks::verified_opens(&path), 1);
            assert!(
                !maintenance_gets_the_database(&path),
                "the arming read keeps its connection"
            );

            tokio::time::advance(
                meerkat_runtime::KEPT_DELIVERY_READER_IDLE + Duration::from_millis(1),
            )
            .await;
            let mut released = false;
            for _ in 0..1000 {
                tokio::task::yield_now().await;
                released = maintenance_gets_the_database(&path);
                if released {
                    break;
                }
            }
            assert!(released, "the quiet reader closed at its idle bound");
            answer_the_watch_for(SELF_TICK_WINDOW).await;
            assert!(
                maintenance_gets_the_database(&path),
                "the closed reader stays closed: its own close woke nothing"
            );
            assert_eq!(
                hooks::verified_opens(&path),
                1,
                "nothing opened the database again"
            );
            assert_eq!(kept_reads.load(Ordering::SeqCst), 0);
            drop(handle);
        }

        /// Through the fresh-open path (forced here, as a platform without the
        /// kept reader takes it) a quiet store's owner opens the database for
        /// its arming read and never again: each fresh read opens and closes
        /// it as the store's first and last connection, which wakes nothing.
        #[tokio::test(start_paused = true)]
        async fn a_quiet_fresh_read_owner_opens_the_store_once() {
            let (fixture, path, capability) = watched_store();
            hooks::force_fresh_reads(&path);
            let handle = arm_registered(&fixture, &path, &capability, NO_SWEEP, false);
            let mut passes = handle.subscribe_passes();
            first_pass(&mut passes).await;
            answer_the_watch_for(SELF_TICK_WINDOW).await;
            assert_eq!(
                hooks::verified_opens(&path),
                1,
                "the arming read opened the database once; its own open and close woke nothing"
            );
            drop(handle);
        }

        /// A platform that reports the WAL created or removed as a change of
        /// it (ReadDirectoryChangesW, emulated here) notifies every fresh
        /// read's own open and close. The fresh-open path answers such a
        /// notification from its last read when the database and its WAL stat
        /// as they did before that read, so a quiet store's owner opens the
        /// database once, not once per notification (paused time).
        #[tokio::test(start_paused = true)]
        async fn a_fresh_read_owner_opens_once_where_its_own_open_notifies_a_wal_change() {
            let (fixture, path, capability) = watched_store();
            hooks::force_fresh_reads(&path);
            let handle = arm_registered(&fixture, &path, &capability, NO_SWEEP, true);
            let mut passes = handle.subscribe_passes();
            first_pass(&mut passes).await;
            answer_the_watch_for(SELF_TICK_WINDOW).await;
            assert_eq!(
                hooks::verified_opens(&path),
                1,
                "the arming read opened the database once; the notifications of its own open \
                 and close were answered from it"
            );
            drop(handle);
        }

        /// The control on the same platform shape: another connection's
        /// commit changes the WAL, so its notification opens the database and
        /// the owner reconciles the moved generation.
        #[tokio::test(start_paused = true)]
        async fn a_fresh_read_owner_reads_a_commit_where_its_own_open_notifies_a_wal_change() {
            let (fixture, path, capability) = watched_store();
            hooks::force_fresh_reads(&path);
            provision_the_delivery_domain(&path).await;
            let handle = arm_registered(&fixture, &path, &capability, NO_SWEEP, true);
            let mut passes = handle.subscribe_passes();
            first_pass(&mut passes).await;
            answer_the_watch_for(SELF_TICK_WINDOW).await;
            let opens = hooks::verified_opens(&path);
            let reconciled = passes.borrow().generation;

            let conn = meerkat_sqlite::open(&path, meerkat_sqlite::ConnectionProfile::PRIMARY)
                .expect("another connection");
            move_the_delivery_generation(&conn);
            drop(conn);
            wait_real("the commit's reconcile", EVENT_GUARD, || {
                passes.borrow().generation > reconciled
            })
            .await;
            assert!(
                hooks::verified_opens(&path) > opens,
                "the commit's notification opened the database"
            );
            drop(handle);
        }

        /// A commit the stat cannot see (the WAL rewritten from its start at
        /// the same length within the file system's time resolution; the test
        /// restores the WAL's mtime to make it so) is answered from the last
        /// read at its notification, and read at the watch's next sweep,
        /// which always opens: the sweep bounds how long such a commit waits.
        #[tokio::test(start_paused = true)]
        async fn a_commit_the_stat_cannot_see_is_read_at_the_next_sweep() {
            const SWEEP: Duration = Duration::from_secs(3);
            let (fixture, path, capability) = watched_store();
            hooks::force_fresh_reads(&path);
            let wal = wal_of(&path);
            provision_the_delivery_domain(&path).await;
            // Held through the test: no read is the store's first or last
            // connection, so the WAL stays and no read changes it.
            let held = meerkat_sqlite::open(&path, meerkat_sqlite::ConnectionProfile::PRIMARY)
                .expect("a held connection");
            // Commits before the owner arms grow the WAL (an update that
            // rewrites a row unchanged writes nothing).
            for _ in 0..8 {
                move_the_delivery_generation(&held);
            }
            let busy: i64 = held
                .query_row("PRAGMA wal_checkpoint(RESTART)", [], |row| row.get(0))
                .expect("checkpoint");
            assert_eq!(
                busy, 0,
                "the WAL is checkpointed: the next commit rewrites it from its start"
            );
            let handle = arm_registered(&fixture, &path, &capability, SWEEP, false);
            let mut passes = handle.subscribe_passes();
            first_pass(&mut passes).await;
            answer_the_watch_for(SELF_TICK_WINDOW).await;
            let opens = hooks::verified_opens(&path);
            let reconciled = passes.borrow().generation;

            // The commit and the restored mtime, before the owner runs again.
            let before = std::fs::metadata(&wal).expect("the WAL");
            move_the_delivery_generation(&held);
            assert_eq!(
                std::fs::metadata(&wal).expect("the WAL").len(),
                before.len(),
                "the commit rewrote the WAL from its start at the same length"
            );
            std::fs::File::options()
                .write(true)
                .open(&wal)
                .expect("the WAL")
                .set_modified(before.modified().expect("the WAL's mtime"))
                .expect("restore the WAL's mtime");

            answer_the_watch_for(SELF_TICK_WINDOW).await;
            assert_eq!(
                hooks::verified_opens(&path),
                opens,
                "the commit's notification was answered from the last read"
            );
            assert_eq!(
                passes.borrow().generation,
                reconciled,
                "so the owner has not reconciled it yet"
            );
            wait_real("the next sweep's reconcile", EVENT_GUARD, || {
                passes.borrow().generation > reconciled
            })
            .await;
            assert!(
                hooks::verified_opens(&path) > opens,
                "the sweep opened the database"
            );
            drop(held);
            drop(handle);
        }

        /// A stat that fails never answers a notification from the last read:
        /// on the platform shape whose own open and close notify a WAL change,
        /// a notification after the arming read opens the database again
        /// (paused time).
        #[tokio::test(start_paused = true)]
        async fn a_fresh_read_owner_whose_stat_fails_reads_its_notifications() {
            let (fixture, path, capability) = watched_store();
            hooks::force_fresh_reads(&path);
            hooks::fail_file_stamps(&path);
            let handle = arm_registered(&fixture, &path, &capability, NO_SWEEP, true);
            let mut passes = handle.subscribe_passes();
            first_pass(&mut passes).await;
            wait_real("a notification's read", EVENT_GUARD, || {
                hooks::verified_opens(&path) >= 2
            })
            .await;
            drop(handle);
        }

        /// Holds the host calls made through it, once armed, until the test
        /// opens it, and says when it holds one.
        struct HostGate {
            armed: AtomicBool,
            held: watch::Sender<bool>,
            open: watch::Sender<bool>,
        }

        impl HostGate {
            fn new() -> Arc<Self> {
                Arc::new(Self {
                    armed: AtomicBool::new(false),
                    held: watch::Sender::new(false),
                    open: watch::Sender::new(false),
                })
            }

            /// Hold the calls from now on.
            fn arm(&self) {
                self.armed.store(true, Ordering::SeqCst);
            }

            /// A host call through the gate: held while it is armed and
            /// closed.
            async fn pass(&self) {
                if !self.armed.load(Ordering::SeqCst) {
                    return;
                }
                self.held.send_replace(true);
                // The gate owns the sender, so this wait ends when it opens.
                let mut open = self.open.subscribe();
                let _ = open.wait_for(|open| *open).await;
            }

            /// Wait until the gate holds a call.
            async fn wait_held(&self, what: &str) {
                let mut held = self.held.subscribe();
                tokio::time::timeout(EVENT_GUARD, held.wait_for(|held| *held))
                    .await
                    .unwrap_or_else(|_| panic!("never held: {what}"))
                    .expect("the gate owns the sender");
            }

            fn open(&self) {
                self.open.send_replace(true);
            }
        }

        /// Admits through [`ObservingSink`] once its gate lets the admission
        /// on.
        struct GatedSink {
            inner: Arc<ObservingSink>,
            gate: Arc<HostGate>,
        }

        #[async_trait::async_trait]
        impl ContinuationDeliverySink for GatedSink {
            async fn admit(
                &self,
                session: &SessionId,
                input: meerkat_runtime::Input,
            ) -> Result<InputId, String> {
                self.gate.pass().await;
                self.inner.admit(session, input).await
            }

            async fn admitted_input(
                &self,
                session: &SessionId,
                admission_key: &str,
            ) -> Result<Option<StoredInputState>, String> {
                self.inner.admitted_input(session, admission_key).await
            }
        }

        /// Routes and claims as [`ClaimHost`] does, and puts a gate in front
        /// of each host call that can wait: an admission, a page of owed
        /// continuations and an address resolution.
        struct GatedHost {
            claims: Arc<ClaimHost>,
            sink: Arc<GatedSink>,
            owed: Arc<HostGate>,
            resolution: Arc<HostGate>,
            /// Changes of the owing source; `None`: the host owes nothing.
            owed_changes: Option<watch::Receiver<u64>>,
        }

        impl GatedHost {
            fn new(
                capability: &HostingCapability,
                owed_changes: Option<watch::Receiver<u64>>,
            ) -> Arc<Self> {
                let claims = ClaimHost::new(capability);
                Arc::new(Self {
                    sink: Arc::new(GatedSink {
                        inner: claims.sink.clone(),
                        gate: HostGate::new(),
                    }),
                    claims,
                    owed: HostGate::new(),
                    resolution: HostGate::new(),
                    owed_changes,
                })
            }
        }

        #[async_trait::async_trait]
        impl RuntimeDeliveryHost for GatedHost {
            async fn delivery_route(
                &self,
                session_id: &SessionId,
            ) -> Option<meerkat::DeliveryRoute> {
                self.claims.delivery_route(session_id).await
            }

            async fn claim_cold_delivery(
                &self,
                session_id: &SessionId,
            ) -> Option<Result<HostingClaim, HostingRefused>> {
                self.claims.claim_cold_delivery(session_id).await
            }

            async fn continuation_sink(
                &self,
                _session_id: &SessionId,
            ) -> Option<Arc<dyn ContinuationDeliverySink>> {
                Some(self.sink.clone())
            }

            async fn resolve_address(&self, address: &LogicalRuntimeId) -> AddressResolution {
                self.resolution.pass().await;
                self.claims.resolve_address(address).await
            }

            fn projects_owed_continuations(&self) -> bool {
                self.owed_changes.is_some()
            }

            fn subscribe_owed_continuation_changes(&self) -> Option<watch::Receiver<u64>> {
                self.owed_changes.clone()
            }

            async fn project_owed_continuations(
                &self,
                _after: Option<&meerkat::OwedContinuationCursor>,
                _limit: usize,
            ) -> meerkat::OwedContinuationProjection {
                self.owed.pass().await;
                meerkat::OwedContinuationProjection::default()
            }
        }

        /// Delivering calls into the host, which can wait, so an owner
        /// closes its kept reader's connection before it delivers, and
        /// maintenance never waits on a delivery. The arming pass reads the
        /// delivery generation through the kept reader, then delivers a
        /// continuation whose admission the test holds: while it is held,
        /// the fence gets the database.
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn a_delivering_owner_holds_no_kept_connection_while_the_host_admits() {
            let (fixture, path, capability) = watched_store();
            let session = SessionId::new();
            let owner = ContinuationOwner::Session {
                session_id: session.clone(),
            };
            fixture
                .continuations
                .submit(&owner, delivery("task-1", host_producer()), 1)
                .await
                .expect("commit");

            let host = GatedHost::new(&capability, None);
            host.sink.gate.arm();
            let handle = fixture
                .owner()
                .with_store_sweep(Duration::from_secs(3600))
                .arm(host.clone())
                .expect("arm");
            let mut passes = handle.subscribe_passes();

            host.sink
                .gate
                .wait_held("the arming pass reaches the admission")
                .await;
            assert!(
                maintenance_gets_the_database(&path),
                "a delivering owner keeps no connection, so maintenance gets the database"
            );

            host.sink.gate.open();
            wait_for_status(&fixture, &owner, "task-1", &mut passes, |status| {
                matches!(status, ContinuationStatus::Applied { .. })
            })
            .await;
            drop(handle);
        }

        /// Projecting a page of owed continuations calls into the
        /// host, which can wait, so an owner closes its kept reader's
        /// connection before it projects. The store sweep reads on the kept
        /// connection until a change of the owing source starts a round,
        /// whose page the test holds: while it is held, the fence gets the
        /// database.
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn an_owner_projecting_owed_continuations_holds_no_kept_connection() {
            let (fixture, path, capability) = watched_store();
            let (owed_source, owed_changes) = watch::channel(0_u64);
            let host = GatedHost::new(&capability, Some(owed_changes));
            let kept_reads = count_kept_reads(&path);
            let handle = fixture
                .owner()
                .with_store_sweep(TEST_SWEEP)
                .arm(host.clone())
                .expect("arm");
            let mut passes = handle.subscribe_passes();
            let armed = first_pass(&mut passes).await;
            let started = std::time::Instant::now();
            while kept_reads.load(Ordering::SeqCst) == 0 {
                assert!(
                    started.elapsed() < EVENT_GUARD,
                    "the sweep never read on the kept connection"
                );
                tokio::time::sleep(TEST_SWEEP).await;
            }

            host.owed.arm();
            owed_source.send_modify(|revision| *revision += 1);
            host.owed
                .wait_held("a source change starts a round of owed continuations")
                .await;
            assert!(
                maintenance_gets_the_database(&path),
                "an owner projecting owed continuations keeps no connection, so maintenance \
                 gets the database"
            );

            host.owed.open();
            wait_for_pass(&mut passes, "the pass of the held round", |pass| {
                pass.generation > armed.generation
            })
            .await;
            drop(handle);
        }

        /// Resolving a continuation address calls into the host, which
        /// can wait (it can revive a member), so an owner closes its kept
        /// reader's connection before it resolves the address of a member
        /// continuation waiting on another host. The test holds the
        /// resolution a store tick makes: while it is held, the fence gets
        /// the database. Then the member repoints to a session served here,
        /// and the row applies through it.
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn an_owner_resolving_a_waiting_address_holds_no_kept_connection() {
            let (fixture, path, capability) = watched_store();
            let host = GatedHost::new(&capability, None);
            let (address, a, b, _elsewhere, _a_claim) =
                repointed_member(&fixture, &capability, &host.claims).await;
            let handle = fixture
                .owner()
                .with_store_sweep(TEST_SWEEP)
                .arm(host.clone())
                .expect("arm");
            let mut passes = handle.subscribe_passes();
            let pass = first_pass(&mut passes).await;
            assert_eq!(
                pass.awaiting_other_hosts, 1,
                "A is hosted elsewhere: {pass:?}"
            );

            host.resolution.arm();
            host.resolution
                .wait_held("a store tick resolves the waiting address again")
                .await;
            assert!(
                maintenance_gets_the_database(&path),
                "an owner resolving an address keeps no connection, so maintenance gets the \
                 database"
            );

            // B is served here and the member repoints to it while the
            // resolution is held, so the held call resolves B.
            let _b_claim = host.claims.authority.grant(&b).expect("B is served here");
            host.claims
                .resolve(&address, AddressResolution::Session(b.clone()));
            host.resolution.open();
            wait_for_pass(&mut passes, "the row applied through B", |pass| {
                pass.applied > 0 || pass.awaiting_other_hosts == 0
            })
            .await;
            assert_eq!(
                host.claims
                    .sink
                    .inner
                    .admissions()
                    .iter()
                    .map(|(session, ..)| session.clone())
                    .collect::<Vec<_>>(),
                vec![b.clone()],
                "admitted once, into B, never into A ({a})"
            );
            drop(handle);
        }
    }
}

/// Invariant behind continuation routing (#1813): the applier routes and
/// admits a continuation into the runtime's page session, the serving session
/// of its FIRST pending row. A job row and a continuation row share a
/// runtime only on a session address `rt:session:{id}`, where the job's
/// provenance session and `resolve_address` are the same session; member
/// addresses carry no job rows. Here a projected job row (origin session S)
/// heads S's address and the continuation behind it is admitted into S.
#[tokio::test]
async fn a_continuation_behind_a_job_row_on_a_session_address_is_admitted_into_that_session() {
    let fixture = Fixture::new();
    let sink = Arc::new(RecordingSink::default());
    let session = SessionId::new();
    let owner = ContinuationOwner::Session {
        session_id: session.clone(),
    };
    let address = LogicalRuntimeId::for_session(&session);
    assert_eq!(
        meerkat::default_address_resolution(&address),
        AddressResolution::Session(session.clone())
    );

    // A completed job whose origin is S and whose notification S subscribed.
    let job_store = Arc::new(MemoryDetachedJobStore::new());
    let jobs = meerkat::DetachedJobService::new(job_store.clone());
    let receipt = jobs
        .submit(meerkat::JobSpec::new(
            "default",
            session.clone(),
            meerkat::ExecutionIntentId::new(),
            meerkat::InteractionLineageId::new(),
            meerkat::ToolIdentity::new("shell", "1").expect("tool"),
            meerkat::RunnerIdentity::new("durable-shell", "1").expect("runner"),
            meerkat::RestartClass::Adoptable,
            meerkat::CanonicalArgumentsHash::new("hash-head").expect("hash"),
            meerkat::JobSubmissionKey::new("head").expect("submission key"),
        ))
        .await
        .expect("submit");
    jobs.subscribe(
        &receipt.job_id,
        meerkat::JobSubscription::new(
            meerkat::JobSubscriptionId::new("to-origin").expect("subscription id"),
            session.clone(),
            meerkat::JobDeliveryKind::Notification,
        ),
    )
    .await
    .expect("subscribe");
    let claim = jobs
        .claim_attempt(
            &receipt.job_id,
            meerkat::AttemptClaim::new(
                meerkat::WorkerId::new("worker").expect("worker"),
                1,
                100,
                meerkat::RunnerHandleRef::new("runner-handle").expect("handle"),
            ),
        )
        .await
        .expect("claim");
    jobs.complete_attempt(
        &receipt.job_id,
        (&claim).into(),
        2,
        Some(meerkat::JobResultRef::new("result").expect("result")),
    )
    .await
    .expect("complete");
    // The job row is projected first, so it heads S's runtime.
    let projected = meerkat::JobOutboxProjector::new(job_store.clone(), fixture.inbox.clone())
        .project_pending(16)
        .await
        .expect("project");
    assert_eq!(projected.projected.len(), 1, "{projected:?}");
    let continuation = fixture
        .continuations
        .submit(&owner, delivery("task-1", host_producer()), 1)
        .await
        .expect("commit");
    assert_eq!(
        continuation.address, address.0,
        "one runtime carries both rows"
    );

    let handle = RuntimeDeliveryOwner::new(job_store, fixture.inbox.clone())
        .arm(Host::new(Some(sink.clone())))
        .expect("arm");
    let mut passes = handle.subscribe_passes();
    wait_for_status(&fixture, &owner, "task-1", &mut passes, |status| {
        matches!(status, ContinuationStatus::Applied { .. })
    })
    .await;
    let admissions = sink.admissions();
    assert_eq!(admissions.len(), 1, "{admissions:?}");
    assert_eq!(
        admissions[0].0, session,
        "the page session (from the job row) is the address's session"
    );
    assert_eq!(
        fixture
            .inbox
            .pending_delivery_total()
            .await
            .expect("backlog"),
        0,
        "the job row ahead of it was applied too"
    );
}

/// The retained admission keeps a typed readiness cause: not admitted yet and
/// retried, with the cause the runtime named. A terminal refusal stays a
/// refusal.
#[test]
fn a_retained_admission_keeps_a_typed_readiness_cause() {
    use meerkat_core::authorization::ControllerReadinessFailure;
    assert_eq!(
        meerkat::retained_admission_outcome(Err(
            meerkat_runtime::RuntimeDriverError::ControllerReadinessUnavailable {
                reason: ControllerReadinessFailure::OriginalAdmissionPending,
            }
        )),
        Err(ContinuationAdmitError::NotReady(
            ControllerReadinessFailure::OriginalAdmissionPending
        ))
    );
    assert_eq!(
        meerkat::retained_admission_outcome(Err(
            meerkat_runtime::RuntimeDriverError::RetainedResumeRefused {
                reason: meerkat_runtime::retained_work::RetainedResumeRefusal::AuthorityDenied,
            }
        )),
        Err(ContinuationAdmitError::Refused(
            meerkat_runtime::RuntimeDeliveryRefusalReason::AuthorityDenied
        ))
    );
}

/// A governed host original's native admission keeps its typed outcome: an
/// actual denial is `AuthorityDenied` and malformed facts
/// `NoAdmissibleWorkBinding` (both terminal), a re-prepare or a readiness
/// failure is not ready with its cause, and anything else is retryable.
#[test]
fn a_governed_admission_keeps_its_typed_outcome() {
    use meerkat::GovernedAdmissionOutcome;
    use meerkat_core::OperationRefusalKind;
    use meerkat_core::authorization::ControllerReadinessFailure;
    use meerkat_runtime::RuntimeDeliveryRefusalReason::{AuthorityDenied, NoAdmissibleWorkBinding};
    let refused = |kind| {
        Err(meerkat_runtime::RuntimeDriverError::InputRefused {
            refusal: meerkat_core::OperationRefused::new(kind),
        })
    };
    let input = InputId::new();
    assert_eq!(
        meerkat::governed_admission_outcome(Ok(GovernedAdmissionOutcome::Accepted(input.clone()))),
        Ok(input.clone())
    );
    assert_eq!(
        meerkat::governed_admission_outcome(Ok(GovernedAdmissionOutcome::Deduplicated(
            input.clone()
        ))),
        Ok(input)
    );
    assert_eq!(
        meerkat::governed_admission_outcome(refused(OperationRefusalKind::Denied)),
        Err(ContinuationAdmitError::Refused(AuthorityDenied))
    );
    assert_eq!(
        meerkat::governed_admission_outcome(refused(OperationRefusalKind::MalformedFacts)),
        Err(ContinuationAdmitError::Refused(NoAdmissibleWorkBinding))
    );
    assert_eq!(
        meerkat::governed_admission_outcome(refused(OperationRefusalKind::ReprepareRequired)),
        Err(ContinuationAdmitError::NotReady(
            ControllerReadinessFailure::PolicyChanged
        ))
    );
    assert_eq!(
        meerkat::governed_admission_outcome(Err(
            meerkat_runtime::RuntimeDriverError::ControllerReadinessUnavailable {
                reason: ControllerReadinessFailure::TurnBoundaryHeld,
            }
        )),
        Err(ContinuationAdmitError::NotReady(
            ControllerReadinessFailure::TurnBoundaryHeld
        ))
    );
    assert!(matches!(
        meerkat::governed_admission_outcome(Err(meerkat_runtime::RuntimeDriverError::Internal(
            "the store is offline".into()
        ))),
        Err(ContinuationAdmitError::Failed(_))
    ));
    assert!(matches!(
        meerkat::governed_admission_outcome(Ok(GovernedAdmissionOutcome::NotAccepted(
            "rejected".into()
        ))),
        Err(ContinuationAdmitError::Failed(_))
    ));
}

async fn status_of(fixture: &Fixture, owner: &ContinuationOwner, key: &str) -> ContinuationStatus {
    fixture
        .continuations
        .continuation_status(owner, &ContinuationKey::new(key).expect("key"))
        .await
        .expect("status")
}

/// CONTROL-stuck (row part): a retained completion whose original's admission
/// its source owner has not settled is not ready, never refused. Its row stays
/// pending and is reported blocked with the typed cause, and the row behind it
/// in the same runtime waits too. Once the owner withdraws the original (the
/// sink then refuses the resume for good) and the host announces the
/// settlement, the row settles refused and the row behind it proceeds.
#[tokio::test]
async fn a_pending_admission_blocks_its_row_with_the_typed_cause_until_announced() {
    use meerkat_core::authorization::ControllerReadinessFailure;
    let fixture = Fixture::new();
    let session = SessionId::new();
    let owner = ContinuationOwner::Session {
        session_id: session.clone(),
    };
    let identity = work_identity("pending");
    let jobs = FakeJobs::holding("job-1", committed_job(&session, &identity));
    retained_fork_completion(&fixture, &session, &jobs).await;
    fixture
        .continuations
        .submit(&owner, delivery("after", host_producer()), 1)
        .await
        .expect("commit the row behind it");
    let sink = Arc::new(RecordingSink::governed());
    *sink.resume_verdict.lock().expect("lock") = Some(ContinuationAdmitError::NotReady(
        ControllerReadinessFailure::OriginalAdmissionPending,
    ));
    let bindings = Arc::new(meerkat::ContinuationHostBindings::default());
    let _binding = bindings
        .bind(Arc::new(SessionAddressResolver), jobs.clone())
        .expect("bind");
    let handle = fixture
        .owner()
        .with_source_changes(bindings.subscribe_source_changes())
        .arm(Arc::new(BindingsHost {
            bindings: Arc::clone(&bindings),
            sink: sink.clone(),
        }))
        .expect("arm");
    let mut passes = handle.subscribe_passes();
    let blocked = wait_for_pass(&mut passes, "blocked with its typed cause", |pass| {
        pass.failures
            .iter()
            .any(|line| line.contains("NotReady(OriginalAdmissionPending)"))
    })
    .await;
    assert_eq!(blocked.refused, 0, "never refused while pending");
    assert!(matches!(
        status_of(&fixture, &owner, "fork_off:job-1").await,
        ContinuationStatus::Pending { .. }
    ));
    assert!(
        matches!(
            status_of(&fixture, &owner, "after").await,
            ContinuationStatus::Pending { .. }
        ),
        "the row behind it waits"
    );
    assert!(sink.resumes().is_empty());

    *sink.resume_verdict.lock().expect("lock") = Some(ContinuationAdmitError::Refused(
        meerkat_runtime::RuntimeDeliveryRefusalReason::AuthorityDenied,
    ));
    bindings.source_custody_changed();
    wait_for_pass(&mut passes, "settled after the announcement", |pass| {
        pass.refused >= 2
    })
    .await;
    assert!(matches!(
        status_of(&fixture, &owner, "fork_off:job-1").await,
        ContinuationStatus::Refused {
            reason: meerkat_runtime::RuntimeDeliveryRefusalReason::AuthorityDenied,
            ..
        }
    ));
    assert!(
        matches!(
            status_of(&fixture, &owner, "after").await,
            ContinuationStatus::Refused { .. }
        ),
        "the row behind it proceeds"
    );
}

/// The incarnation of the mob `team` these tests address members of.
fn team_incarnation() -> meerkat_core::service::MobIncarnationId {
    "6a1d3c58-2f0e-4b7a-9c41-8e5d2b0f7a13"
        .parse()
        .expect("canonical mob incarnation id")
}

// ---------------------------------------------------------------------------
// Retired addresses: a reservation left on an address that retired is
// settled from the reserved session's ledger, and a stranded answer is
// final: no admission follows it.
// ---------------------------------------------------------------------------

/// Retirements the test records, read by the delivery host and by the status
/// read's resolver alike, as one host's records are.
#[derive(Default)]
struct Retirements(Mutex<std::collections::HashSet<LogicalRuntimeId>>);

impl Retirements {
    fn retire(&self, address: &LogicalRuntimeId) {
        self.0.lock().expect("lock").insert(address.clone());
    }

    fn retired(&self, address: &LogicalRuntimeId) -> bool {
        self.0.lock().expect("lock").contains(address)
    }
}

/// The status read's resolver: session addresses, retired once recorded.
/// `before_observe` runs inside the next observation, before it answers.
struct RetiringResolver {
    retirements: Arc<Retirements>,
    before_observe: Mutex<Option<futures::future::BoxFuture<'static, ()>>>,
}

#[async_trait::async_trait]
impl meerkat::ContinuationAddressResolver for RetiringResolver {
    async fn current_address(
        &self,
        owner: &ContinuationOwner,
    ) -> Result<Option<LogicalRuntimeId>, String> {
        // A member of the mob `team`, at its first generation.
        if let ContinuationOwner::Member { mob_id, identity } = owner {
            return meerkat::member_delivery_address(mob_id, team_incarnation(), identity, 1)
                .map(Some)
                .map_err(|error| error.to_string());
        }
        meerkat::ContinuationAddressResolver::current_address(&SessionAddressResolver, owner).await
    }

    async fn resolve_address(
        &self,
        address: &LogicalRuntimeId,
    ) -> Result<AddressResolution, String> {
        if self.retirements.retired(address) {
            return Ok(AddressResolution::Retired);
        }
        meerkat::ContinuationAddressResolver::resolve_address(&SessionAddressResolver, address)
            .await
    }

    async fn observe_address(
        &self,
        address: &LogicalRuntimeId,
    ) -> Result<meerkat::AddressObservation, String> {
        let hook = self.before_observe.lock().expect("lock").take();
        if let Some(hook) = hook {
            hook.await;
        }
        Ok(if self.retirements.retired(address) {
            meerkat::AddressObservation::Retired
        } else {
            meerkat::AddressObservation::MayServe
        })
    }
}

/// A delivery host over the same retirements: a retired address resolves
/// Retired. A session in `elsewhere` is another runtime owner's. The next
/// route asked can be parked: the pass has resolved its address then, and
/// has reserved nothing yet.
struct RetiringHost {
    sink: Arc<RecordingSink>,
    retirements: Arc<Retirements>,
    /// The committed owner of host namespaces, when the host composes one.
    job_source: Option<Arc<FakeJobs>>,
    /// Addresses served by another session than their own (a repoint).
    served_by: Mutex<HashMap<LogicalRuntimeId, SessionId>>,
    elsewhere: Mutex<std::collections::HashSet<SessionId>>,
    held_route: Mutex<
        Option<(
            tokio::sync::oneshot::Sender<()>,
            tokio::sync::oneshot::Receiver<()>,
        )>,
    >,
}

impl RetiringHost {
    fn new(sink: Arc<RecordingSink>, retirements: Arc<Retirements>) -> Arc<Self> {
        Self::composed(sink, retirements, None)
    }

    /// As [`Self::new`], with `jobs` as the host's job source.
    fn with_jobs(
        sink: Arc<RecordingSink>,
        retirements: Arc<Retirements>,
        jobs: Arc<FakeJobs>,
    ) -> Arc<Self> {
        Self::composed(sink, retirements, Some(jobs))
    }

    fn composed(
        sink: Arc<RecordingSink>,
        retirements: Arc<Retirements>,
        job_source: Option<Arc<FakeJobs>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            sink,
            retirements,
            job_source,
            served_by: Mutex::new(HashMap::new()),
            elsewhere: Mutex::new(std::collections::HashSet::new()),
            held_route: Mutex::new(None),
        })
    }

    fn serve(&self, address: &LogicalRuntimeId, session: SessionId) {
        self.served_by
            .lock()
            .expect("lock")
            .insert(address.clone(), session);
    }

    /// Park the next route asked. The first receiver fires when it parks;
    /// the route answers once the sender fires.
    fn hold_next_route(
        &self,
    ) -> (
        tokio::sync::oneshot::Receiver<()>,
        tokio::sync::oneshot::Sender<()>,
    ) {
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        *self.held_route.lock().expect("lock") = Some((entered_tx, release_rx));
        (entered_rx, release_tx)
    }
}

#[async_trait::async_trait]
impl RuntimeDeliveryHost for RetiringHost {
    async fn delivery_route(&self, session_id: &SessionId) -> Option<meerkat::DeliveryRoute> {
        let held = self.held_route.lock().expect("lock").take();
        if let Some((entered, release)) = held {
            let _ = entered.send(());
            let _ = release.await;
        }
        if self.elsewhere.lock().expect("lock").contains(session_id) {
            return Some(meerkat::DeliveryRoute::ServedElsewhere);
        }
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

    async fn resolve_address(&self, address: &LogicalRuntimeId) -> AddressResolution {
        if self.retirements.retired(address) {
            return AddressResolution::Retired;
        }
        if let Some(session) = self.served_by.lock().expect("lock").get(address) {
            return AddressResolution::Session(session.clone());
        }
        meerkat::default_address_resolution(address)
    }

    async fn continuation_sink(
        &self,
        _session_id: &SessionId,
    ) -> Option<Arc<dyn ContinuationDeliverySink>> {
        Some(self.sink.clone() as Arc<dyn ContinuationDeliverySink>)
    }

    fn retained_job_source(&self) -> Option<Arc<dyn RetainedJobSource>> {
        self.job_source
            .clone()
            .map(|jobs| jobs as Arc<dyn RetainedJobSource>)
    }
}

/// A fixture whose status read observes `retirements` through `resolver`.
struct RetiringFixture {
    fixture: Fixture,
    retirements: Arc<Retirements>,
    resolver: Arc<RetiringResolver>,
}

impl RetiringFixture {
    fn on(store: Arc<dyn RuntimeStore>, dir: Option<tempfile::TempDir>) -> Self {
        let retirements = Arc::new(Retirements::default());
        let resolver = Arc::new(RetiringResolver {
            retirements: Arc::clone(&retirements),
            before_observe: Mutex::new(None),
        });
        let inbox = RuntimeDeliveryInbox::new(store);
        Self {
            fixture: Fixture {
                continuations: ContinuationOwnerService::new(
                    inbox.clone(),
                    resolver.clone(),
                    Arc::new(meerkat_runtime::MeerkatMachine::ephemeral()),
                ),
                inbox,
                _dir: dir,
            },
            retirements,
            resolver,
        }
    }

    fn memory() -> Self {
        Self::on(Arc::new(InMemoryRuntimeStore::new()), None)
    }

    #[cfg(feature = "sqlite-store")]
    fn sqlite() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = meerkat_runtime::SqliteRuntimeStore::new(dir.path().join("runtime.sqlite3"))
            .expect("open sqlite runtime store");
        Self::on(Arc::new(store), Some(dir))
    }

    async fn status(&self, owner: &ContinuationOwner, key: &str) -> ContinuationStatus {
        self.fixture
            .continuations
            .continuation_status(owner, &ContinuationKey::new(key).expect("key"))
            .await
            .expect("status")
    }

    async fn admission_entry(
        &self,
        address: &LogicalRuntimeId,
        delivery_id: &str,
    ) -> Option<ContinuationAdmission> {
        self.fixture
            .inbox
            .continuation_admission(
                address,
                &RuntimeDeliveryId::new(delivery_id.to_string()).expect("id"),
            )
            .await
            .expect("admission index read")
    }
}

/// Wait for the owner's first pass, its reconcile, which visits every
/// runtime with a pending row.
async fn wait_for_reconcile(passes: &mut watch::Receiver<RuntimeDeliveryPass>) {
    wait_for_pass(passes, "the reconcile pass", |pass| pass.generation >= 1).await;
}

/// A drain resolved the address, then the address retired and a status read
/// answered stranded. The drain, released, must admit nothing, so a resend
/// under a new key is the only delivery.
#[tokio::test]
async fn a_drain_that_resolved_before_the_retire_admits_nothing_after_a_stranded_answer() {
    let cut = RetiringFixture::memory();
    let sink = Arc::new(RecordingSink::default());
    let session = SessionId::new();
    let owner = ContinuationOwner::Session {
        session_id: session.clone(),
    };
    let receipt = cut
        .fixture
        .continuations
        .submit(&owner, delivery("task-1", host_producer()), 1)
        .await
        .expect("commit");
    let address = LogicalRuntimeId::new(receipt.address.clone());

    let host = RetiringHost::new(sink.clone(), Arc::clone(&cut.retirements));
    let (entered, release) = host.hold_next_route();
    let handle = cut.fixture.owner().arm(host).expect("arm");
    let mut passes = handle.subscribe_passes();
    tokio::time::timeout(EVENT_GUARD, entered)
        .await
        .expect("the drain reached the session's route")
        .expect("route hold");

    cut.retirements.retire(&address);
    let stranded = cut.status(&owner, "task-1").await;
    assert!(
        matches!(stranded, ContinuationStatus::Stranded { .. }),
        "never admitted, and the address retired: {stranded:?}"
    );

    release.send(()).expect("release the drain");
    wait_for_reconcile(&mut passes).await;
    assert!(
        sink.admissions().is_empty(),
        "nothing is admitted after a stranded answer: {:?}",
        sink.admissions()
    );
    let still = cut.status(&owner, "task-1").await;
    assert!(
        matches!(still, ContinuationStatus::Stranded { .. }),
        "a stranded continuation stays stranded: {still:?}"
    );
}

/// The same through a repoint: a crash left a reservation in s1, and the
/// pass resolved the address to s2. The address retires while the drain is
/// held. Released, the drain repoints to s2, observes the retire and
/// releases the repointed reservation: nothing is admitted.
#[tokio::test]
async fn a_drain_that_repoints_after_the_retire_releases_the_repointed_reservation() {
    let cut = RetiringFixture::memory();
    let sink = Arc::new(RecordingSink::default());
    let (s1, s2) = (SessionId::new(), SessionId::new());
    let owner = ContinuationOwner::Session {
        session_id: s1.clone(),
    };
    let receipt = cut
        .fixture
        .continuations
        .submit(&owner, delivery("task-1", host_producer()), 1)
        .await
        .expect("commit");
    let address = LogicalRuntimeId::new(receipt.address.clone());
    cut.fixture
        .admission(
            &address,
            &receipt.delivery_id,
            ContinuationAdmissionTransition::Reserve {
                session_id: s1.clone(),
                input_id: InputId::new(),
            },
        )
        .await;

    let host = RetiringHost::new(sink.clone(), Arc::clone(&cut.retirements));
    host.serve(&address, s2.clone());
    let (entered, release) = host.hold_next_route();
    let handle = cut.fixture.owner().arm(host).expect("arm");
    let mut passes = handle.subscribe_passes();
    tokio::time::timeout(EVENT_GUARD, entered)
        .await
        .expect("the drain reached the serving session's route")
        .expect("route hold");

    cut.retirements.retire(&address);
    let reserved = cut.status(&owner, "task-1").await;
    assert!(
        matches!(reserved, ContinuationStatus::Pending { admitted: None, .. }),
        "a reservation exists, so the read is not stranded: {reserved:?}"
    );

    release.send(()).expect("release the drain");
    wait_for_reconcile(&mut passes).await;
    assert!(
        sink.admissions().is_empty(),
        "nothing is admitted after the retire: {:?}",
        sink.admissions()
    );
    assert_eq!(
        cut.admission_entry(&address, &receipt.delivery_id).await,
        None,
        "the repointed reservation is released"
    );
    let stranded = cut.status(&owner, "task-1").await;
    assert!(
        matches!(stranded, ContinuationStatus::Stranded { .. }),
        "never admitted, and the address retired: {stranded:?}"
    );
}

/// A reservation the drain left before a crash, whose input never reached
/// the reserved session, on an address that retired since: the owner settles
/// it from that session's ledger, and the continuation reads stranded.
async fn a_reservation_left_on_a_retired_address_without_its_input_reads_stranded(
    cut: RetiringFixture,
) {
    let sink = Arc::new(RecordingSink::default());
    let session = SessionId::new();
    let owner = ContinuationOwner::Session {
        session_id: session.clone(),
    };
    let receipt = cut
        .fixture
        .continuations
        .submit(&owner, delivery("task-1", host_producer()), 1)
        .await
        .expect("commit");
    let address = LogicalRuntimeId::new(receipt.address.clone());
    cut.fixture
        .admission(
            &address,
            &receipt.delivery_id,
            ContinuationAdmissionTransition::Reserve {
                session_id: session.clone(),
                input_id: InputId::new(),
            },
        )
        .await;
    cut.retirements.retire(&address);

    let handle = cut
        .fixture
        .owner()
        .arm(RetiringHost::new(
            sink.clone(),
            Arc::clone(&cut.retirements),
        ))
        .expect("arm");
    let mut passes = handle.subscribe_passes();
    wait_for_reconcile(&mut passes).await;

    let status = cut.status(&owner, "task-1").await;
    assert!(
        matches!(status, ContinuationStatus::Stranded { .. }),
        "the reserved session never held the input: {status:?}"
    );
    assert!(sink.admissions().is_empty(), "nothing is admitted");
    assert_eq!(
        cut.admission_entry(&address, &receipt.delivery_id).await,
        None,
        "the reservation is released"
    );
}

/// As above, but the input reached the reserved session before the crash:
/// the owner records that admission, and the continuation reads admitted.
async fn a_reservation_left_on_a_retired_address_with_its_input_reads_admitted(
    cut: RetiringFixture,
) {
    let sink = Arc::new(RecordingSink::default());
    let session = SessionId::new();
    let owner = ContinuationOwner::Session {
        session_id: session.clone(),
    };
    let receipt = cut
        .fixture
        .continuations
        .submit(&owner, delivery("task-1", host_producer()), 1)
        .await
        .expect("commit");
    let address = LogicalRuntimeId::new(receipt.address.clone());
    let reserved = InputId::new();
    cut.fixture
        .admission(
            &address,
            &receipt.delivery_id,
            ContinuationAdmissionTransition::Reserve {
                session_id: session.clone(),
                input_id: reserved.clone(),
            },
        )
        .await;
    sink.already_holds(
        &session,
        &continuation_input(reserved.clone(), &receipt.delivery_id, "result"),
    );
    cut.retirements.retire(&address);

    let handle = cut
        .fixture
        .owner()
        .arm(RetiringHost::new(
            sink.clone(),
            Arc::clone(&cut.retirements),
        ))
        .expect("arm");
    let mut passes = handle.subscribe_passes();
    wait_for_reconcile(&mut passes).await;

    let status = cut.status(&owner, "task-1").await;
    assert_eq!(
        status,
        ContinuationStatus::Pending {
            receipt,
            sequence: 1,
            admitted: Some((session, reserved)),
        },
        "the reserved session holds the input"
    );
    assert!(sink.admissions().is_empty(), "nothing is admitted again");
}

/// As above, but the reserved session holds ANOTHER input under the
/// admission key: never this delivery by key alone. The owner keeps the
/// reservation, reports the conflict in its pass, and the continuation stays
/// pending; nothing is applied.
async fn a_reservation_left_on_a_retired_address_beside_another_input_stays_pending(
    cut: RetiringFixture,
) {
    let sink = Arc::new(RecordingSink::default());
    let session = SessionId::new();
    let owner = ContinuationOwner::Session {
        session_id: session.clone(),
    };
    let receipt = cut
        .fixture
        .continuations
        .submit(&owner, delivery("task-1", host_producer()), 1)
        .await
        .expect("commit");
    let address = LogicalRuntimeId::new(receipt.address.clone());
    let reserved = InputId::new();
    cut.fixture
        .admission(
            &address,
            &receipt.delivery_id,
            ContinuationAdmissionTransition::Reserve {
                session_id: session.clone(),
                input_id: reserved.clone(),
            },
        )
        .await;
    sink.holds_legacy_row(&session, &receipt.delivery_id);
    cut.retirements.retire(&address);

    let handle = cut
        .fixture
        .owner()
        .arm(RetiringHost::new(
            sink.clone(),
            Arc::clone(&cut.retirements),
        ))
        .expect("arm");
    let mut passes = handle.subscribe_passes();
    let pass = wait_for_pass(&mut passes, "the reconcile pass", |pass| {
        pass.generation >= 1
    })
    .await;
    assert!(
        pass.failures
            .iter()
            .any(|failure| failure.contains(&receipt.delivery_id)),
        "the conflict is reported: {:?}",
        pass.failures
    );

    let status = cut.status(&owner, "task-1").await;
    assert!(
        matches!(status, ContinuationStatus::Pending { admitted: None, .. }),
        "another input is never this delivery: {status:?}"
    );
    assert_eq!(
        cut.admission_entry(&address, &receipt.delivery_id).await,
        Some(ContinuationAdmission::Reserved {
            session_id: session,
            input_id: reserved,
        }),
        "the reservation stays"
    );
    assert!(sink.admissions().is_empty(), "nothing is admitted");
}

macro_rules! retired_cut {
    ($case:ident) => {
        mod $case {
            #[tokio::test]
            async fn memory() {
                super::$case(super::RetiringFixture::memory()).await;
            }

            #[cfg(feature = "sqlite-store")]
            #[tokio::test]
            async fn sqlite() {
                super::$case(super::RetiringFixture::sqlite()).await;
            }
        }
    };
}

retired_cut!(a_reservation_left_on_a_retired_address_without_its_input_reads_stranded);
retired_cut!(a_reservation_left_on_a_retired_address_with_its_input_reads_admitted);
retired_cut!(a_reservation_left_on_a_retired_address_beside_another_input_stays_pending);

/// A reservation that lands between a status read's entry read and its
/// observation of the retirement is seen: the read answers pending, never
/// stranded. The owner then settles it from the reserved session's ledger.
#[tokio::test]
async fn a_reservation_made_while_the_status_observes_the_retire_reads_pending() {
    let cut = RetiringFixture::memory();
    let sink = Arc::new(RecordingSink::default());
    let session = SessionId::new();
    let owner = ContinuationOwner::Session {
        session_id: session.clone(),
    };
    let receipt = cut
        .fixture
        .continuations
        .submit(&owner, delivery("task-1", host_producer()), 1)
        .await
        .expect("commit");
    let address = LogicalRuntimeId::new(receipt.address.clone());
    cut.retirements.retire(&address);

    let inbox = cut.fixture.inbox.clone();
    let hook_address = address.clone();
    let delivery_id = RuntimeDeliveryId::new(receipt.delivery_id.clone()).expect("id");
    let hook_session = session.clone();
    *cut.resolver.before_observe.lock().expect("lock") = Some(Box::pin(async move {
        let outcome = inbox
            .transition_continuation_admission(
                &hook_address,
                &delivery_id,
                ContinuationAdmissionTransition::Reserve {
                    session_id: hook_session,
                    input_id: InputId::new(),
                },
            )
            .await
            .expect("admission index write");
        assert!(
            matches!(outcome, ContinuationAdmissionOutcome::Transitioned(_)),
            "{outcome:?}"
        );
    }));
    let status = cut.status(&owner, "task-1").await;
    assert!(
        matches!(status, ContinuationStatus::Pending { admitted: None, .. }),
        "a reservation exists, so the read is not stranded: {status:?}"
    );

    let handle = cut
        .fixture
        .owner()
        .arm(RetiringHost::new(
            sink.clone(),
            Arc::clone(&cut.retirements),
        ))
        .expect("arm");
    let mut passes = handle.subscribe_passes();
    wait_for_reconcile(&mut passes).await;
    let settled = cut.status(&owner, "task-1").await;
    assert!(
        matches!(settled, ContinuationStatus::Stranded { .. }),
        "the reserved session never held the input: {settled:?}"
    );
    assert!(sink.admissions().is_empty(), "nothing is admitted");
}

/// The owner settles a reservation only under the reserved session's
/// custody: a session another runtime owner serves keeps its reservation
/// for that owner, and the continuation stays pending.
#[tokio::test]
async fn a_reservation_on_a_retired_address_is_left_to_the_host_that_serves_its_session() {
    let cut = RetiringFixture::memory();
    let sink = Arc::new(RecordingSink::default());
    let session = SessionId::new();
    let owner = ContinuationOwner::Session {
        session_id: session.clone(),
    };
    let receipt = cut
        .fixture
        .continuations
        .submit(&owner, delivery("task-1", host_producer()), 1)
        .await
        .expect("commit");
    let address = LogicalRuntimeId::new(receipt.address.clone());
    let reserved = InputId::new();
    cut.fixture
        .admission(
            &address,
            &receipt.delivery_id,
            ContinuationAdmissionTransition::Reserve {
                session_id: session.clone(),
                input_id: reserved.clone(),
            },
        )
        .await;
    cut.retirements.retire(&address);

    let host = RetiringHost::new(sink.clone(), Arc::clone(&cut.retirements));
    host.elsewhere.lock().expect("lock").insert(session.clone());
    let handle = cut.fixture.owner().arm(host).expect("arm");
    let mut passes = handle.subscribe_passes();
    wait_for_reconcile(&mut passes).await;

    let status = cut.status(&owner, "task-1").await;
    assert!(
        matches!(status, ContinuationStatus::Pending { admitted: None, .. }),
        "another host serves the reserved session: {status:?}"
    );
    assert_eq!(
        cut.admission_entry(&address, &receipt.delivery_id).await,
        Some(ContinuationAdmission::Reserved {
            session_id: session,
            input_id: reserved,
        }),
        "the reservation is left to that host"
    );
    assert!(sink.admissions().is_empty(), "nothing is admitted");
}

/// A live address still drains, once, through the observation that follows
/// its reservation.
#[tokio::test]
async fn a_live_address_still_drains_once_through_the_post_reserve_observation() {
    let cut = RetiringFixture::memory();
    let sink = Arc::new(RecordingSink::default());
    let session = SessionId::new();
    let owner = ContinuationOwner::Session {
        session_id: session.clone(),
    };
    let handle = cut
        .fixture
        .owner()
        .arm(RetiringHost::new(
            sink.clone(),
            Arc::clone(&cut.retirements),
        ))
        .expect("arm");
    let mut passes = handle.subscribe_passes();
    let receipt = cut
        .fixture
        .continuations
        .submit(&owner, delivery("task-1", host_producer()), 1)
        .await
        .expect("commit");
    let status = wait_for_status(&cut.fixture, &owner, "task-1", &mut passes, |status| {
        matches!(status, ContinuationStatus::Applied { .. })
    })
    .await;
    let admissions = sink.admissions();
    assert_eq!(admissions.len(), 1, "admitted exactly once: {admissions:?}");
    assert_eq!(
        status,
        ContinuationStatus::Applied {
            receipt,
            session,
            input: admissions[0].2.clone(),
        }
    );
}

// ---------------------------------------------------------------------------
// Host-originated continuations on a governed runtime.
//
// The rows below assert behaviour only. Everything that depends on how a
// host continuation is committed with its namespace's committed owner, how
// that owner holds or disowns it, and what a governed sink admitted lives in
// `host_continuation_shape`, the one place that changes when the governed
// admission shape lands.
// ---------------------------------------------------------------------------

const INGEST_NAMESPACE: &str = "app.ingest";
const OBSERVATION: &str = "observation-1";
const OBSERVATION_DIGEST: &str = "sha256:observation-1";
const OBSERVATION_KEY: &str = "ingest:observation-1";

fn ingest_producer() -> ContinuationProducer {
    ContinuationProducer::Host {
        namespace: INGEST_NAMESPACE.into(),
    }
}

/// One external observation, delivered under its host namespace.
fn observation_delivery() -> ContinuationDelivery {
    ContinuationDelivery {
        key: ContinuationKey::new(OBSERVATION_KEY).expect("key"),
        result: ContinuationResultRef {
            producer: ingest_producer(),
            producer_id: OBSERVATION.into(),
            result_digest: OBSERVATION_DIGEST.into(),
            summary: None,
        },
        body: "a new item arrived".into(),
        handling: ContinuationHandling::Queue,
    }
}

/// The namespace's committed owner, holding the observation owed to
/// `session`.
fn ingest_owner(session: &SessionId) -> Arc<FakeJobs> {
    host_continuation_shape::owner_of(session)
}

/// Commit a permitted job notification behind the first row of `address`.
async fn a_permitted_delivery_behind(
    fixture: &Fixture,
    address: &LogicalRuntimeId,
    session: &SessionId,
) {
    let job_id = meerkat::JobId::generated();
    fixture
        .inbox
        .submit(
            address,
            meerkat_runtime::RuntimeDeliverySubmission::new(
                RuntimeDeliveryId::new("job:permitted:1").expect("id"),
                meerkat_runtime::RuntimeDeliveryKind::JobNotification,
                job_id.as_str(),
                1,
                "lineage",
                serde_json::to_vec(&meerkat::JobNotificationDeliveryPayload {
                    job_id: job_id.clone(),
                    delivery_sequence: 1,
                    origin_session_id: session.clone(),
                    interaction_lineage_id: meerkat::InteractionLineageId::new(),
                    targets: Vec::new(),
                    notification: meerkat::JobNotification::new(
                        "n-1", "n-1", "progress", "halfway",
                    )
                    .expect("notification"),
                })
                .expect("encode"),
            )
            .expect("submission"),
        )
        .await
        .expect("a permitted delivery behind it");
}

/// The observation's status once the owner admitted or settled it.
async fn settled_observation(
    fixture: &Fixture,
    owner: &ContinuationOwner,
    passes: &mut watch::Receiver<RuntimeDeliveryPass>,
) -> ContinuationStatus {
    wait_for_status(fixture, owner, OBSERVATION_KEY, passes, |status| {
        matches!(
            status,
            ContinuationStatus::Applied { .. }
                | ContinuationStatus::Refused { .. }
                | ContinuationStatus::Stranded { .. }
        )
    })
    .await
}

/// On a governed runtime a host continuation that its namespace's committed
/// owner confirms is admitted exactly once, into the session it is owed to,
/// and never as an authority-free input. The owner is asked again at
/// admission: a confirmation read at commit time is not enough.
#[tokio::test]
async fn a_confirmed_host_continuation_is_admitted_once_on_a_governed_runtime() {
    let fixture = Fixture::new();
    let session = SessionId::new();
    let owner = ContinuationOwner::Session {
        session_id: session.clone(),
    };
    let jobs = ingest_owner(&session);
    let receipt = host_continuation_shape::commit(
        &fixture.continuations,
        &owner,
        observation_delivery(),
        &jobs,
    )
    .await
    .expect("commit the host continuation from its namespace owner");
    let asked_at_commit = jobs.lookups();
    let sink = Arc::new(RecordingSink::governed());
    let handle = fixture
        .owner()
        .arm(Host::with_jobs(sink.clone(), Arc::clone(&jobs)))
        .expect("arm");
    let mut passes = handle.subscribe_passes();
    let (applied, admitted_session, input) =
        match settled_observation(&fixture, &owner, &mut passes).await {
            ContinuationStatus::Applied {
                receipt,
                session,
                input,
            } => (receipt, session, input),
            other => panic!(
                "a confirmed host continuation is admitted on a governed runtime, got {other:?}"
            ),
        };
    assert_eq!(applied, receipt);
    assert_eq!(admitted_session, session);
    assert_eq!(
        host_continuation_shape::admitted(&sink),
        vec![(session.clone(), input)],
        "admitted exactly once, into the session it is owed to"
    );
    assert!(
        sink.admissions().is_empty(),
        "never admitted as an authority-free input"
    );
    assert!(
        jobs.lookups() > asked_at_commit,
        "the namespace owner confirmed it again at admission"
    );
}

/// A host continuation its namespace's committed owner does not confirm
/// exactly is refused with the existing typed reason before anything is
/// reserved or admitted, and the delivery behind it proceeds.
async fn an_unconfirmed_host_continuation_is_refused(disown: host_continuation_shape::Disown) {
    let fixture = Fixture::new();
    let session = SessionId::new();
    let owner = ContinuationOwner::Session {
        session_id: session.clone(),
    };
    let jobs = ingest_owner(&session);
    let receipt = host_continuation_shape::commit(
        &fixture.continuations,
        &owner,
        observation_delivery(),
        &jobs,
    )
    .await
    .expect("commit while the namespace owner holds it");
    host_continuation_shape::disown(&jobs, disown);
    let address = LogicalRuntimeId::new(receipt.address.clone());
    a_permitted_delivery_behind(&fixture, &address, &session).await;
    let sink = Arc::new(RecordingSink::governed());
    let handle = fixture
        .owner()
        .arm(Host::with_jobs(sink.clone(), Arc::clone(&jobs)))
        .expect("arm");
    let mut passes = handle.subscribe_passes();
    let pass = wait_for_pass(
        &mut passes,
        "the unconfirmed host continuation settled",
        |pass| pass.refused > 0,
    )
    .await;
    assert_eq!(
        pass.applied, 1,
        "{disown:?}: the permitted delivery behind it is delivered"
    );
    assert!(
        host_continuation_shape::admitted(&sink).is_empty()
            && sink.admissions().is_empty()
            && sink.resumes().is_empty(),
        "{disown:?}: nothing was admitted"
    );
    assert_eq!(
        fixture
            .inbox
            .continuation_admission(
                &address,
                &RuntimeDeliveryId::new(receipt.delivery_id.clone()).expect("id"),
            )
            .await
            .expect("admission index"),
        None,
        "{disown:?}: nothing was reserved"
    );
    assert_eq!(
        status_of(&fixture, &owner, OBSERVATION_KEY).await,
        ContinuationStatus::Refused {
            receipt,
            sequence: 1,
            reason: meerkat_runtime::RuntimeDeliveryRefusalReason::NoAdmissibleWorkBinding
        },
        "{disown:?}"
    );
}

#[tokio::test]
async fn a_host_continuation_of_a_namespace_its_owner_does_not_hold_is_refused() {
    an_unconfirmed_host_continuation_is_refused(host_continuation_shape::Disown::OtherNamespace)
        .await;
}

#[tokio::test]
async fn a_host_continuation_its_owner_holds_for_another_owner_is_refused() {
    an_unconfirmed_host_continuation_is_refused(host_continuation_shape::Disown::OtherOwner).await;
}

#[tokio::test]
async fn a_host_continuation_with_other_content_than_its_owner_committed_is_refused() {
    an_unconfirmed_host_continuation_is_refused(host_continuation_shape::Disown::OtherResult).await;
}

#[tokio::test]
async fn a_host_continuation_its_owner_no_longer_holds_is_refused() {
    an_unconfirmed_host_continuation_is_refused(host_continuation_shape::Disown::Forgotten).await;
}

/// A namespace owner that cannot be read now decides nothing: the host
/// continuation stays pending (never settled, never admitted) and is
/// admitted once the owner answers again.
#[tokio::test]
async fn an_unreadable_namespace_owner_keeps_the_host_continuation_pending_until_it_answers() {
    let fixture = Fixture::new();
    let session = SessionId::new();
    let owner = ContinuationOwner::Session {
        session_id: session.clone(),
    };
    let jobs = ingest_owner(&session);
    let receipt = host_continuation_shape::commit(
        &fixture.continuations,
        &owner,
        observation_delivery(),
        &jobs,
    )
    .await
    .expect("commit the host continuation from its namespace owner");
    *jobs.unavailable.lock().expect("lock") = Some("the namespace owner is offline".into());
    let sink = Arc::new(RecordingSink::governed());
    let (attachments, attachment_commits) = watch::channel(0_u64);
    let handle = fixture
        .owner()
        .with_attachment_commits(attachment_commits)
        .arm(Host::with_jobs(sink.clone(), Arc::clone(&jobs)))
        .expect("arm");
    let mut passes = handle.subscribe_passes();
    let pass = wait_for_pass(
        &mut passes,
        "the host continuation blocked or settled",
        |pass| pass.refused > 0 || !pass.blocked_runtimes.is_empty(),
    )
    .await;
    assert_eq!(
        pass.refused, 0,
        "an owner that cannot be read decides nothing: {:?}",
        pass.failures
    );
    assert!(
        host_continuation_shape::admitted(&sink).is_empty() && sink.admissions().is_empty(),
        "nothing is admitted while its owner cannot be read"
    );
    assert!(
        matches!(
            status_of(&fixture, &owner, OBSERVATION_KEY).await,
            ContinuationStatus::Pending { .. }
        ),
        "pending, never settled"
    );

    *jobs.unavailable.lock().expect("lock") = None;
    attachments.send_modify(|generation| *generation += 1);
    let status = settled_observation(&fixture, &owner, &mut passes).await;
    assert!(
        matches!(&status, ContinuationStatus::Applied { receipt: applied, .. } if *applied == receipt),
        "admitted once the owner confirms it, got {status:?}"
    );
    assert_eq!(host_continuation_shape::admitted(&sink).len(), 1);
}

/// Without a native work authorization host nothing changes: a host
/// continuation, committed from its namespace owner or plainly, is admitted
/// as the authority-free input it always was, and the namespace owner is
/// never asked at admission (here it no longer even holds the namespace).
#[tokio::test]
async fn an_ungoverned_runtime_admits_host_continuations_as_before() {
    let fixture = Fixture::new();
    let session = SessionId::new();
    let owner = ContinuationOwner::Session {
        session_id: session.clone(),
    };
    let jobs = ingest_owner(&session);
    let owned = host_continuation_shape::commit(
        &fixture.continuations,
        &owner,
        observation_delivery(),
        &jobs,
    )
    .await
    .expect("commit the host continuation from its namespace owner");
    let plain = fixture
        .continuations
        .submit(&owner, delivery("task-1", host_producer()), 1)
        .await
        .expect("commit a plain host continuation");
    host_continuation_shape::disown(&jobs, host_continuation_shape::Disown::OtherNamespace);
    let asked = jobs.lookups();
    let sink = Arc::new(RecordingSink::default());
    let handle = fixture
        .owner()
        .arm(Host::with_jobs(sink.clone(), Arc::clone(&jobs)))
        .expect("arm");
    let mut passes = handle.subscribe_passes();
    let owned_status = settled_observation(&fixture, &owner, &mut passes).await;
    let plain_status = wait_for_status(&fixture, &owner, "task-1", &mut passes, |status| {
        matches!(status, ContinuationStatus::Applied { .. })
    })
    .await;
    assert!(
        matches!(&owned_status, ContinuationStatus::Applied { receipt, session: admitted, .. } if *receipt == owned && *admitted == session),
        "{owned_status:?}"
    );
    assert!(
        matches!(&plain_status, ContinuationStatus::Applied { receipt, session: admitted, .. } if *receipt == plain && *admitted == session),
        "{plain_status:?}"
    );
    let admissions = sink.admissions();
    assert_eq!(admissions.len(), 2, "each admitted once: {admissions:?}");
    assert!(
        admissions
            .iter()
            .all(|(admitted_session, _, _)| *admitted_session == session),
        "into the owner's session: {admissions:?}"
    );
    assert!(
        host_continuation_shape::admitted(&sink).is_empty(),
        "no governed admission without a native work authorization host"
    );
    assert_eq!(
        jobs.lookups(),
        asked,
        "the namespace owner is never asked at admission"
    );
}

// ---------------------------------------------------------------------------
// Governed host originals: the binder, the host's custody, recovery by the
// reserved input id, and the refusals decided before anything is asked.
// ---------------------------------------------------------------------------

/// The observation committed for its session from its namespace owner, and
/// what the rows below read about it.
struct Observation {
    fixture: Fixture,
    session: SessionId,
    owner: ContinuationOwner,
    jobs: Arc<FakeJobs>,
    receipt: meerkat::ContinuationReceipt,
}

/// A delivery owner armed over one host, woken by hand.
struct Armed {
    _handle: meerkat::RuntimeDeliveryOwnerHandle,
    passes: watch::Receiver<RuntimeDeliveryPass>,
    attachments: watch::Sender<u64>,
}

impl Armed {
    /// Wake the owner as an attachment commit does: blocked rows retry.
    fn wake(&self) {
        self.attachments.send_modify(|generation| *generation += 1);
    }
}

impl Observation {
    async fn committed() -> Self {
        let fixture = Fixture::new();
        let session = SessionId::new();
        let owner = ContinuationOwner::Session {
            session_id: session.clone(),
        };
        let jobs = ingest_owner(&session);
        let receipt = host_continuation_shape::commit(
            &fixture.continuations,
            &owner,
            observation_delivery(),
            &jobs,
        )
        .await
        .expect("commit the observation from its namespace owner");
        Self {
            fixture,
            session,
            owner,
            jobs,
            receipt,
        }
    }

    fn address(&self) -> LogicalRuntimeId {
        LogicalRuntimeId::new(self.receipt.address.clone())
    }

    fn delivery_id(&self) -> RuntimeDeliveryId {
        RuntimeDeliveryId::new(self.receipt.delivery_id.clone()).expect("id")
    }

    fn binder(&self) -> Arc<FakeBinder> {
        binder_of(&self.jobs)
    }

    /// Core's exact continuation input for the observation under `input_id`.
    fn input(&self, input_id: InputId) -> meerkat_runtime::Input {
        continuation_input(input_id, &self.receipt.delivery_id, "a new item arrived")
    }

    /// The observation as its binder binds it now for `session`, under
    /// `input_id`.
    fn bound_for(&self, session: &SessionId, input_id: InputId) -> meerkat_runtime::Input {
        self.binder().bind_now(
            &governed_fixture::original_work(),
            session,
            self.input(input_id),
        )
    }

    async fn reserve(&self, session: &SessionId, input_id: &InputId) {
        self.fixture
            .admission(
                &self.address(),
                &self.receipt.delivery_id,
                ContinuationAdmissionTransition::Reserve {
                    session_id: session.clone(),
                    input_id: input_id.clone(),
                },
            )
            .await;
    }

    async fn index(&self) -> Option<ContinuationAdmission> {
        self.fixture
            .inbox
            .continuation_admission(&self.address(), &self.delivery_id())
            .await
            .expect("admission index")
    }

    async fn status(&self) -> ContinuationStatus {
        status_of(&self.fixture, &self.owner, OBSERVATION_KEY).await
    }

    fn arm(&self, host: Arc<dyn RuntimeDeliveryHost>) -> Armed {
        let (attachments, attachment_commits) = watch::channel(0_u64);
        let handle = self
            .fixture
            .owner()
            .with_attachment_commits(attachment_commits)
            .arm(host)
            .expect("arm");
        Armed {
            passes: handle.subscribe_passes(),
            _handle: handle,
            attachments,
        }
    }

    /// Arm over `sink`, with the namespace owner as the host's job source.
    fn arm_over(&self, sink: &Arc<RecordingSink>) -> Armed {
        self.arm(Host::with_jobs(Arc::clone(sink), Arc::clone(&self.jobs)))
    }

    async fn settled(&self, armed: &mut Armed) -> ContinuationStatus {
        settled_observation(&self.fixture, &self.owner, &mut armed.passes).await
    }

    /// The first pass that blocked (or settled) the observation's runtime.
    async fn blocked(&self, armed: &mut Armed, what: &str) -> RuntimeDeliveryPass {
        let address = self.address();
        wait_for_pass(&mut armed.passes, what, |pass| {
            pass.refused > 0 || pass.blocked_runtimes.contains(&address)
        })
        .await
    }

    fn refused(&self, reason: meerkat_runtime::RuntimeDeliveryRefusalReason) -> ContinuationStatus {
        ContinuationStatus::Refused {
            receipt: self.receipt.clone(),
            sequence: 1,
            reason,
        }
    }

    fn applied(&self, session: &SessionId, input: &InputId) -> ContinuationStatus {
        ContinuationStatus::Applied {
            receipt: self.receipt.clone(),
            session: session.clone(),
            input: input.clone(),
        }
    }
}

fn custody_untouched(binder: &FakeBinder) -> bool {
    binder.log.accepted.lock().expect("lock").is_empty()
        && binder.log.retained.lock().expect("lock").is_empty()
        && binder.log.dropped.load(std::sync::atomic::Ordering::SeqCst) == 0
}

fn custody_dropped_once(binder: &FakeBinder) -> bool {
    binder.log.accepted.lock().expect("lock").is_empty()
        && binder.log.retained.lock().expect("lock").is_empty()
        && binder.log.dropped.load(std::sync::atomic::Ordering::SeqCst) == 1
}

/// The binder is handed Core's exact continuation input (its id, the
/// admission key and the content, with no authority of its own) and the
/// admitting session, made live first; it binds once, and native admission
/// accepts exactly that bound input. The host's custody is promoted once, for
/// that admission, and never dropped or retained.
#[tokio::test]
async fn a_host_original_is_bound_from_cores_exact_input_and_its_custody_promoted_once() {
    let observation = Observation::committed().await;
    let sink = Arc::new(RecordingSink::governed());
    let mut armed = observation.arm_over(&sink);
    let status = observation.settled(&mut armed).await;
    let ContinuationStatus::Applied { session, input, .. } = status else {
        panic!("a confirmed host original is admitted, got {status:?}");
    };
    assert_eq!(session, observation.session);
    let binder = observation.binder();
    let calls = binder.calls.lock().expect("lock").clone();
    assert_eq!(calls.len(), 1, "bound once");
    let (original, bound_for, handed) = &calls[0];
    assert_eq!(original.namespace, INGEST_NAMESPACE);
    assert_eq!(original.producer_id, OBSERVATION);
    assert_eq!(original.owner, observation.owner);
    assert!(
        original.original_work == governed_fixture::original_work(),
        "the original its namespace owner confirmed"
    );
    assert_eq!(bound_for, &observation.session);
    assert_eq!(handed.id(), &input, "Core's own input id");
    assert_eq!(
        handed
            .header()
            .idempotency_key
            .as_ref()
            .map(ToString::to_string),
        Some(observation.receipt.delivery_id.clone()),
        "the admission key"
    );
    assert!(
        handed.header().authority_association.is_none()
            && handed.header().ingress_context.is_none(),
        "handed over with no authority of its own"
    );
    let expected =
        StoredInputState::accepted_for_test(&observation.input(input.clone())).expect("row");
    meerkat_runtime::verify_exact_replay(&expected.state, handed)
        .expect("the handed input is Core's exact continuation input");
    let admitted = sink.governed_inputs();
    assert_eq!(admitted.len(), 1, "admitted once");
    assert_eq!(admitted[0].id(), &input, "the bound input, under Core's id");
    assert!(
        admitted[0].header().authority_association.is_some()
            && admitted[0].header().ingress_context.is_some(),
        "admitted with its binding"
    );
    assert_eq!(
        sink.prepared.lock().expect("lock").clone(),
        vec![observation.session.clone()],
        "the session was made live before the bind"
    );
    let accepted = binder.log.accepted.lock().expect("lock").clone();
    assert_eq!(accepted.len(), 1, "custody promoted once");
    assert_eq!(
        (
            accepted[0].session.clone(),
            accepted[0].input.clone(),
            accepted[0].deduplicated
        ),
        (observation.session.clone(), input, false)
    );
    assert_eq!(
        binder.log.dropped.load(std::sync::atomic::Ordering::SeqCst),
        0
    );
    assert!(binder.log.retained.lock().expect("lock").is_empty());
}

/// An observation reserved with its admitted row in the reserved session is
/// applied by the reserved id, verified with the row's OWN recorded binding,
/// and nobody is asked: not the namespace owner (which here holds it no
/// more) and not the binder (which here would bind another destination
/// generation, or deny). It is never refused later and never admitted again,
/// also when the owner moved to another session since.
async fn a_reserved_observation_with_its_admitted_row(sink: Arc<RecordingSink>, repointed: bool) {
    let observation = Observation::committed().await;
    let admitted = InputId::new();
    let bound = observation.bound_for(&observation.session, admitted.clone());
    sink.holds_by_id_only(&observation.session, &bound);
    observation.reserve(&observation.session, &admitted).await;
    host_continuation_shape::disown(
        &observation.jobs,
        host_continuation_shape::Disown::Forgotten,
    );
    let binder = observation.binder();
    binder
        .generation
        .store(2, std::sync::atomic::Ordering::SeqCst);
    *binder.verdict.lock().expect("lock") = Some(meerkat::HostOriginalRefusal::Denied);
    let asked = observation.jobs.lookups();
    let host = Host::with_jobs(Arc::clone(&sink), Arc::clone(&observation.jobs));
    if repointed {
        host.resolve(
            &observation.address(),
            AddressResolution::Session(SessionId::new()),
        );
    }
    let mut armed = observation.arm(host);
    let status = observation.settled(&mut armed).await;
    assert_eq!(status, observation.applied(&observation.session, &admitted));
    assert!(
        sink.governed_inputs().is_empty() && sink.admissions().is_empty(),
        "never admitted again"
    );
    assert_eq!(binder.calls(), 0, "the binder is not asked");
    assert_eq!(
        observation.jobs.lookups(),
        asked,
        "the namespace owner is not asked"
    );
}

#[tokio::test]
async fn a_reserved_host_original_with_its_admitted_row_is_applied_by_id_and_nobody_is_asked() {
    a_reserved_observation_with_its_admitted_row(Arc::new(RecordingSink::governed()), false).await;
}

#[tokio::test]
async fn a_host_original_admitted_before_a_repoint_is_applied_where_it_was_admitted() {
    a_reserved_observation_with_its_admitted_row(Arc::new(RecordingSink::governed()), true).await;
}

/// A governed admission drained later by a sink that governs nothing is
/// still found by its reserved id (its stored key is the qualified one, so a
/// lookup by the raw admission key finds nothing) and applied, never
/// admitted again.
#[tokio::test]
async fn an_ungoverned_drain_applies_a_governed_admission_by_its_reserved_id() {
    a_reserved_observation_with_its_admitted_row(Arc::new(RecordingSink::default()), false).await;
}

/// An admission an ungoverned sink made (with no binding) and a governed
/// drain later: applied by the reserved id with the row's own (absent)
/// binding, never refused by the governed static checks and nobody is asked.
#[tokio::test]
async fn a_governed_drain_applies_an_ungoverned_admission_by_its_reserved_id() {
    let observation = Observation::committed().await;
    let admitted = InputId::new();
    let sink = Arc::new(RecordingSink::governed());
    sink.already_holds(&observation.session, &observation.input(admitted.clone()));
    observation.reserve(&observation.session, &admitted).await;
    host_continuation_shape::disown(
        &observation.jobs,
        host_continuation_shape::Disown::Forgotten,
    );
    let asked = observation.jobs.lookups();
    let mut armed = observation.arm_over(&sink);
    let status = observation.settled(&mut armed).await;
    assert_eq!(status, observation.applied(&observation.session, &admitted));
    assert!(sink.governed_inputs().is_empty() && sink.admissions().is_empty());
    assert_eq!(observation.binder().calls(), 0);
    assert_eq!(observation.jobs.lookups(), asked);
}

/// A reserved observation whose session holds no row under the reserved id
/// was never admitted: it is bound fresh, for the destination's CURRENT
/// generation, and admitted exactly once under the reserved id; after a
/// repoint, into the session that serves its owner now.
async fn a_reserved_observation_without_its_row(repointed: bool) {
    let observation = Observation::committed().await;
    let reserved = InputId::new();
    let reserved_in = if repointed {
        SessionId::new()
    } else {
        observation.session.clone()
    };
    observation.reserve(&reserved_in, &reserved).await;
    let binder = observation.binder();
    binder
        .generation
        .store(2, std::sync::atomic::Ordering::SeqCst);
    let sink = Arc::new(RecordingSink::governed());
    let mut armed = observation.arm_over(&sink);
    let status = observation.settled(&mut armed).await;
    assert_eq!(status, observation.applied(&observation.session, &reserved));
    assert_eq!(
        sink.governed_admissions(),
        vec![(observation.session.clone(), reserved.clone())],
        "admitted once, under the reserved id, where the owner is served now"
    );
    assert_eq!(binder.calls(), 1, "bound once");
    let admitted = sink.governed_inputs();
    assert_eq!(
        admitted[0]
            .header()
            .authority_association
            .as_ref()
            .expect("bound")
            .candidate()
            .target
            .context_generation,
        2,
        "bound fresh, for the destination's current generation"
    );
}

#[tokio::test]
async fn a_reserved_host_original_without_its_row_is_bound_fresh_and_admitted_once() {
    a_reserved_observation_without_its_row(false).await;
}

#[tokio::test]
async fn a_reserved_host_original_without_its_row_follows_a_repoint_and_is_admitted_once() {
    a_reserved_observation_without_its_row(true).await;
}

/// An observation the admission index already applied is acknowledged
/// whatever its namespace owner says now (here: it holds nothing and offers
/// no binder), and nothing is asked or admitted.
#[tokio::test]
async fn an_applied_host_original_is_acknowledged_whatever_its_owner_says_now() {
    let observation = Observation::committed().await;
    let admitted = InputId::new();
    observation.reserve(&observation.session, &admitted).await;
    observation
        .fixture
        .admission(
            &observation.address(),
            &observation.receipt.delivery_id,
            ContinuationAdmissionTransition::Apply {
                session_id: observation.session.clone(),
                input_id: admitted.clone(),
            },
        )
        .await;
    host_continuation_shape::disown(
        &observation.jobs,
        host_continuation_shape::Disown::Forgotten,
    );
    *observation.jobs.binder.lock().expect("lock") = None;
    let asked = observation.jobs.lookups();
    let sink = Arc::new(RecordingSink::governed());
    let mut armed = observation.arm_over(&sink);
    let status = observation.settled(&mut armed).await;
    assert_eq!(status, observation.applied(&observation.session, &admitted));
    assert!(sink.governed_inputs().is_empty() && sink.admissions().is_empty());
    assert_eq!(observation.jobs.lookups(), asked);
}

/// The namespace owner's facts bind the exact delivery: the same namespace,
/// producer id, result digest and owner with another body, or under another
/// key, is refused before anything is bound or reserved.
async fn an_observation_its_owner_now_describes_otherwise(
    change: impl FnOnce(&mut meerkat::HostOriginalFacts),
) {
    let observation = Observation::committed().await;
    observation.jobs.change_original(OBSERVATION, change);
    let sink = Arc::new(RecordingSink::governed());
    let mut armed = observation.arm_over(&sink);
    let status = observation.settled(&mut armed).await;
    assert_eq!(
        status,
        observation.refused(meerkat_runtime::RuntimeDeliveryRefusalReason::NoAdmissibleWorkBinding)
    );
    assert_eq!(observation.binder().calls(), 0, "never bound");
    assert!(sink.governed_inputs().is_empty());
    assert_eq!(observation.index().await, None, "nothing was reserved");
}

#[tokio::test]
async fn a_host_original_whose_owner_holds_another_body_is_refused_unbound() {
    an_observation_its_owner_now_describes_otherwise(|facts| {
        facts.delivery.body = "another body".into();
    })
    .await;
}

#[tokio::test]
async fn a_host_original_whose_owner_holds_it_under_another_key_is_refused_unbound() {
    an_observation_its_owner_now_describes_otherwise(|facts| {
        facts.delivery.key = ContinuationKey::new("ingest:another").expect("key");
    })
    .await;
}

/// Another external observation of the ingest namespace.
fn another_observation(producer_id: &str) -> ContinuationDelivery {
    ContinuationDelivery {
        key: ContinuationKey::new(format!("ingest:{producer_id}")).expect("key"),
        result: ContinuationResultRef {
            producer: ingest_producer(),
            producer_id: producer_id.into(),
            result_digest: format!("sha256:{producer_id}"),
            summary: None,
        },
        body: "another item arrived".into(),
        handling: ContinuationHandling::Queue,
    }
}

/// One original committed for two owners (the namespace owner first held it
/// for one, then for the other): only the row of the owner its namespace
/// owner confirms now is admitted. The other row has the same content and is
/// refused through the key ledger, which binds the confirmed owner's key to
/// the confirmed owner's row only.
#[tokio::test]
async fn one_original_committed_for_two_owners_admits_only_the_confirmed_owners_row() {
    let fixture = Fixture::new();
    let first = SessionId::new();
    let second = SessionId::new();
    let first_owner = ContinuationOwner::Session {
        session_id: first.clone(),
    };
    let second_owner = ContinuationOwner::Session {
        session_id: second.clone(),
    };
    let jobs = ingest_owner(&first);
    let first_receipt = host_continuation_shape::commit(
        &fixture.continuations,
        &first_owner,
        observation_delivery(),
        &jobs,
    )
    .await
    .expect("commit for the first owner");
    jobs.change_original(OBSERVATION, |facts| facts.owner = second_owner.clone());
    host_continuation_shape::commit(
        &fixture.continuations,
        &second_owner,
        observation_delivery(),
        &jobs,
    )
    .await
    .expect("commit for the second owner");
    let sink = Arc::new(RecordingSink::governed());
    let handle = fixture
        .owner()
        .arm(Host::with_jobs(sink.clone(), Arc::clone(&jobs)))
        .expect("arm");
    let mut passes = handle.subscribe_passes();
    let confirmed = wait_for_status(
        &fixture,
        &second_owner,
        OBSERVATION_KEY,
        &mut passes,
        |status| matches!(status, ContinuationStatus::Applied { .. }),
    )
    .await;
    let unconfirmed = wait_for_status(
        &fixture,
        &first_owner,
        OBSERVATION_KEY,
        &mut passes,
        |status| matches!(status, ContinuationStatus::Refused { .. }),
    )
    .await;
    assert_eq!(
        unconfirmed,
        ContinuationStatus::Refused {
            receipt: first_receipt,
            sequence: 1,
            reason: meerkat_runtime::RuntimeDeliveryRefusalReason::NoAdmissibleWorkBinding
        }
    );
    let ContinuationStatus::Applied { input, .. } = confirmed else {
        unreachable!("waited for Applied");
    };
    assert_eq!(sink.governed_admissions(), vec![(second.clone(), input)]);
    assert!(
        binder_of(&jobs)
            .calls
            .lock()
            .expect("lock")
            .iter()
            .all(|(_, session, _)| *session == second),
        "bound only for the confirmed owner's session"
    );
}

/// A binding that is not exactly Core's input with only its association set
/// and a matching trusted ingress observation attached is refused
/// `NoAdmissibleWorkBinding` before anything is reserved or handed to native
/// admission, and the host's custody is dropped (withdrawn), never promoted
/// or retained. One case per field a binder could get wrong.
#[tokio::test]
async fn a_tampered_binding_is_refused_before_admission_and_its_custody_dropped() {
    let mut mismatches = Vec::new();
    for tamper in [
        Tamper::Content,
        Tamper::Key,
        Tamper::Id,
        Tamper::NoAssociation,
        Tamper::NoIngress,
        Tamper::OtherRuntime,
        Tamper::OtherOriginal,
        Tamper::NoControllerLineage,
        Tamper::IngressRequester,
        Tamper::NoController,
    ] {
        let observation = Observation::committed().await;
        let binder = observation.binder();
        *binder.tamper.lock().expect("lock") = Some(tamper);
        let sink = Arc::new(RecordingSink::governed());
        let mut armed = observation.arm_over(&sink);
        let status = observation.settled(&mut armed).await;
        if status
            != observation
                .refused(meerkat_runtime::RuntimeDeliveryRefusalReason::NoAdmissibleWorkBinding)
        {
            mismatches.push(format!("{tamper:?}: {status:?}"));
        }
        if !sink.governed_inputs().is_empty() {
            mismatches.push(format!("{tamper:?}: handed to native admission"));
        }
        if observation.index().await.is_some() {
            mismatches.push(format!("{tamper:?}: reserved"));
        }
        if !custody_dropped_once(&binder) {
            mismatches.push(format!("{tamper:?}: the custody was not dropped once"));
        }
    }
    assert!(mismatches.is_empty(), "{mismatches:#?}");
}

/// Native admission that may have committed (settled `Uncertain`) leaves the
/// observation pending with the host's custody retained unresolved, never
/// promoted or dropped. The next attempt recovers it by the reserved id once
/// the session holds that admission: applied, and nothing is bound or
/// admitted again.
#[tokio::test]
async fn an_uncertain_native_admission_retains_the_custody_and_recovers_by_the_reserved_id() {
    let observation = Observation::committed().await;
    let sink = Arc::new(RecordingSink::governed());
    *sink.governed_verdict.lock().expect("lock") = Some(GovernedVerdict::Uncertain);
    let mut armed = observation.arm_over(&sink);
    let pass = observation
        .blocked(&mut armed, "the uncertain admission blocked")
        .await;
    assert_eq!(pass.refused, 0, "never refused: {:?}", pass.failures);
    let binder = observation.binder();
    assert_eq!(
        binder.log.retained.lock().expect("lock").len(),
        1,
        "custody retained unresolved"
    );
    assert!(binder.log.accepted.lock().expect("lock").is_empty());
    assert_eq!(
        binder.log.dropped.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "never dropped"
    );
    let handed = sink.governed_inputs();
    assert_eq!(handed.len(), 1);
    let reserved = handed[0].id().clone();
    assert_eq!(
        observation.index().await,
        Some(ContinuationAdmission::Reserved {
            session_id: observation.session.clone(),
            input_id: reserved.clone(),
        })
    );
    assert!(matches!(
        observation.status().await,
        ContinuationStatus::Pending { .. }
    ));

    // The admission had committed: the session holds it under the reserved id.
    sink.holds_by_id_only(&observation.session, &handed[0]);
    *sink.governed_verdict.lock().expect("lock") = None;
    let bound = binder.calls();
    armed.wake();
    let status = observation.settled(&mut armed).await;
    assert_eq!(status, observation.applied(&observation.session, &reserved));
    assert!(
        sink.governed_admissions().is_empty(),
        "never admitted again"
    );
    assert_eq!(binder.calls(), bound, "recovery binds nothing");
}

/// Commit a host original row that no submitter API writes (forced into the
/// inbox): `delivery` with the marker, and `retained_work` when given.
async fn forced_host_original(
    fixture: &Fixture,
    session: &SessionId,
    delivery: &ContinuationDelivery,
    retained_work: Option<RetainedWorkIdentity>,
) -> (LogicalRuntimeId, RuntimeDeliveryId) {
    let address = LogicalRuntimeId::for_session(session);
    let delivery_id = RuntimeDeliveryId::new("continuation:forced-host-original").expect("id");
    let mut payload = serde_json::to_value(delivery).expect("encode");
    payload["host_original"] = serde_json::Value::Bool(true);
    if let Some(identity) = retained_work {
        payload["retained_work"] = serde_json::to_value(identity).expect("encode");
    }
    fixture
        .inbox
        .submit(
            &address,
            meerkat_runtime::RuntimeDeliverySubmission::new(
                delivery_id.clone(),
                meerkat_runtime::RuntimeDeliveryKind::Continuation,
                "host:forced",
                1,
                delivery.key.as_str(),
                serde_json::to_vec(&payload).expect("encode"),
            )
            .expect("submission"),
        )
        .await
        .expect("commit the forced row");
    (address, delivery_id)
}

/// A host original row no submitter API commits is refused
/// `NoAdmissibleWorkBinding` on both postures before its namespace owner or
/// binder is asked: one in a reserved namespace (here on a host whose bound
/// source claims that namespace, holds the row exactly and offers a binder),
/// one with retained work, and one of a non-host producer.
#[tokio::test]
async fn a_forced_host_original_row_is_refused_on_both_postures_without_asking_anyone() {
    let reserved = ContinuationDelivery {
        result: ContinuationResultRef {
            producer: ContinuationProducer::Host {
                namespace: "meerkat.consent".into(),
            },
            ..observation_delivery().result
        },
        ..observation_delivery()
    };
    let cases = [
        ("a reserved namespace", reserved, None),
        (
            "retained work",
            observation_delivery(),
            Some(work_identity("forced")),
        ),
        (
            "a non-host producer",
            fork_completion("job-9", "sha256:result"),
            None,
        ),
    ];
    let mut mismatches = Vec::new();
    for (case, delivery, retained_work) in cases {
        for governed in [true, false] {
            let fixture = Fixture::new();
            let session = SessionId::new();
            let jobs = ingest_owner(&session);
            if matches!(delivery.result.producer, ContinuationProducer::Host { .. }) {
                *jobs.producer.lock().expect("lock") = Some(delivery.result.producer.clone());
                jobs.change_original(OBSERVATION, |facts| facts.delivery = delivery.clone());
            }
            let (address, delivery_id) =
                forced_host_original(&fixture, &session, &delivery, retained_work.clone()).await;
            let sink = Arc::new(if governed {
                RecordingSink::governed()
            } else {
                RecordingSink::default()
            });
            let asked = jobs.lookups();
            let handle = fixture
                .owner()
                .arm(Host::with_jobs(sink.clone(), Arc::clone(&jobs)))
                .expect("arm");
            let mut passes = handle.subscribe_passes();
            wait_for_pass(&mut passes, "the forced row settled", |pass| {
                pass.refused > 0 || pass.applied > 0 || !pass.blocked_runtimes.is_empty()
            })
            .await;
            let status = fixture
                .inbox
                .delivery_status(&address, &delivery_id)
                .await
                .expect("delivery status");
            if !matches!(
                status,
                meerkat_runtime::RuntimeDeliveryStatus::Refused {
                    reason: meerkat_runtime::RuntimeDeliveryRefusalReason::NoAdmissibleWorkBinding,
                    ..
                }
            ) {
                mismatches.push(format!("{case}, governed {governed}: {status:?}"));
            }
            if !sink.admissions().is_empty() || !sink.governed_inputs().is_empty() {
                mismatches.push(format!("{case}, governed {governed}: admitted"));
            }
            if binder_of(&jobs).calls() != 0 || jobs.lookups() != asked {
                mismatches.push(format!("{case}, governed {governed}: asked"));
            }
        }
    }
    assert!(mismatches.is_empty(), "{mismatches:#?}");
}

/// A steering host original committed while the runtime was ungoverned is
/// refused typed on a governed runtime, never bound, and the delivery behind
/// it proceeds: a governed host original joins its owner after the current
/// turn (queue) only.
#[tokio::test]
async fn a_steering_host_original_is_refused_on_a_governed_runtime_and_the_row_behind_proceeds() {
    let fixture = Fixture::new();
    let session = SessionId::new();
    let owner = ContinuationOwner::Session {
        session_id: session.clone(),
    };
    let jobs = ingest_owner(&session);
    let steering = ContinuationDelivery {
        handling: ContinuationHandling::Steer,
        ..observation_delivery()
    };
    jobs.change_original(OBSERVATION, |facts| facts.delivery = steering.clone());
    let receipt = host_continuation_shape::commit(&fixture.continuations, &owner, steering, &jobs)
        .await
        .expect("an ungoverned runtime commits a steering host original");
    let address = LogicalRuntimeId::new(receipt.address.clone());
    a_permitted_delivery_behind(&fixture, &address, &session).await;
    let sink = Arc::new(RecordingSink::governed());
    let handle = fixture
        .owner()
        .arm(Host::with_jobs(sink.clone(), Arc::clone(&jobs)))
        .expect("arm");
    let mut passes = handle.subscribe_passes();
    let pass = wait_for_pass(&mut passes, "the steering row settled", |pass| {
        pass.refused > 0
    })
    .await;
    assert_eq!(pass.applied, 1, "the delivery behind it proceeds");
    assert_eq!(
        status_of(&fixture, &owner, OBSERVATION_KEY).await,
        ContinuationStatus::Refused {
            receipt,
            sequence: 1,
            reason: meerkat_runtime::RuntimeDeliveryRefusalReason::NoAdmissibleWorkBinding
        }
    );
    assert_eq!(binder_of(&jobs).calls(), 0, "never bound");
    assert!(sink.governed_inputs().is_empty());
}

/// A governed sink that admits governed host originals but keeps the default
/// by-id read: it cannot tell whether a reserved id was admitted.
#[derive(Default)]
struct NoByIdSink {
    governed_admissions: std::sync::atomic::AtomicUsize,
}

#[async_trait::async_trait]
impl ContinuationDeliverySink for NoByIdSink {
    async fn admit(
        &self,
        _session: &SessionId,
        _input: meerkat_runtime::Input,
    ) -> Result<InputId, String> {
        panic!("a governed runtime never receives an unbound continuation")
    }

    async fn admitted_input(
        &self,
        _session: &SessionId,
        _admission_key: &str,
    ) -> Result<Option<StoredInputState>, String> {
        Ok(None)
    }

    fn governs_work_authority(&self) -> bool {
        true
    }

    fn admits_governed(&self) -> bool {
        true
    }

    async fn admit_governed(
        &self,
        _session: &SessionId,
        input: meerkat_runtime::Input,
        custody: Box<dyn meerkat_runtime::input_admission_custody::NativeInputAdmissionCustody>,
    ) -> Result<meerkat::GovernedAdmissionOutcome, meerkat_runtime::RuntimeDriverError> {
        self.governed_admissions
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let _ = custody.settle(
            meerkat_runtime::input_admission_custody::NativeInputAdmissionSettlement::Admitted,
        );
        Ok(meerkat::GovernedAdmissionOutcome::Accepted(
            input.id().clone(),
        ))
    }
}

/// A sink that cannot read an input by its id leaves a reserved observation
/// pending: an unknown answer is never read as "nothing was admitted", so it
/// is never bound or admitted again.
#[tokio::test]
async fn a_sink_without_a_by_id_read_keeps_a_reserved_host_original_pending() {
    let observation = Observation::committed().await;
    let reserved = InputId::new();
    observation.reserve(&observation.session, &reserved).await;
    let sink = Arc::new(NoByIdSink::default());
    let mut armed = observation.arm(Host::with_other_sink(
        sink.clone(),
        Arc::clone(&observation.jobs),
    ));
    let pass = observation
        .blocked(&mut armed, "the reserved observation blocked")
        .await;
    assert_eq!(pass.refused, 0, "never refused: {:?}", pass.failures);
    assert_eq!(
        sink.governed_admissions
            .load(std::sync::atomic::Ordering::SeqCst),
        0,
        "never admitted again"
    );
    assert_eq!(observation.binder().calls(), 0, "never bound");
    assert!(matches!(
        observation.status().await,
        ContinuationStatus::Pending { .. }
    ));
    assert_eq!(
        observation.index().await,
        Some(ContinuationAdmission::Reserved {
            session_id: observation.session.clone(),
            input_id: reserved,
        })
    );
}

/// A native replay that deduplicates onto an existing input stands for the
/// observation only as its exact admission: applied as that input, with the
/// retry's custody withdrawn (dropped), never promoted. Onto another input it
/// is a visible conflict that stays pending.
async fn a_deduplicated_observation(exact: bool) {
    let observation = Observation::committed().await;
    let existing = InputId::new();
    let sink = Arc::new(RecordingSink::governed());
    let held = if exact {
        observation.bound_for(&observation.session, existing.clone())
    } else {
        observation.binder().bind_now(
            &governed_fixture::original_work(),
            &observation.session,
            continuation_input(
                existing.clone(),
                &observation.receipt.delivery_id,
                "another body",
            ),
        )
    };
    sink.holds_by_id_only(&observation.session, &held);
    *sink.governed_verdict.lock().expect("lock") =
        Some(GovernedVerdict::DeduplicatedOnto(existing.clone()));
    let mut armed = observation.arm_over(&sink);
    let binder = observation.binder();
    if exact {
        let status = observation.settled(&mut armed).await;
        assert_eq!(status, observation.applied(&observation.session, &existing));
    } else {
        let pass = observation
            .blocked(&mut armed, "the conflicting replay blocked")
            .await;
        assert_eq!(
            pass.refused, 0,
            "a conflict is never settled: {:?}",
            pass.failures
        );
        assert!(matches!(
            observation.status().await,
            ContinuationStatus::Pending { .. }
        ));
    }
    assert!(
        custody_dropped_once(&binder),
        "the retry's custody is dropped"
    );
    assert!(
        sink.governed_admissions().is_empty(),
        "nothing new admitted"
    );
}

#[tokio::test]
async fn a_deduplicated_host_original_is_applied_as_the_existing_input_and_its_custody_dropped() {
    a_deduplicated_observation(true).await;
}

#[tokio::test]
async fn a_host_original_deduplicated_onto_another_input_is_a_visible_conflict() {
    a_deduplicated_observation(false).await;
}

/// A host custody that cannot be promoted after its admission never blocks
/// the observation: it is applied and acknowledged (the failure is reported
/// in tracing).
#[tokio::test]
async fn a_failed_custody_promotion_still_applies_the_admitted_host_original() {
    let observation = Observation::committed().await;
    let binder = observation.binder();
    *binder.accept_error.lock().expect("lock") = Some("the host's custody store is offline".into());
    let sink = Arc::new(RecordingSink::governed());
    let mut armed = observation.arm_over(&sink);
    let status = observation.settled(&mut armed).await;
    let admitted = sink.governed_admissions();
    assert_eq!(admitted.len(), 1);
    assert_eq!(
        status,
        observation.applied(&observation.session, &admitted[0].1)
    );
    assert_eq!(binder.log.accepted.lock().expect("lock").len(), 1);
    assert_eq!(
        binder.log.dropped.load(std::sync::atomic::Ordering::SeqCst),
        0
    );
}

/// A binder that cannot bind for one runtime now (its turn boundary is held)
/// keeps only that runtime's observation pending, typed; another runtime's
/// observation is admitted in the same pass, and the first is admitted once
/// the binder can bind.
#[tokio::test]
async fn a_binder_not_ready_for_one_runtime_never_delays_another() {
    let fixture = Fixture::new();
    let busy = SessionId::new();
    let idle = SessionId::new();
    let busy_owner = ContinuationOwner::Session {
        session_id: busy.clone(),
    };
    let idle_owner = ContinuationOwner::Session {
        session_id: idle.clone(),
    };
    let jobs = ingest_owner(&busy);
    let second = another_observation("observation-2");
    jobs.originals.lock().expect("lock").insert(
        "observation-2".into(),
        meerkat::HostOriginalFacts::new(
            idle_owner.clone(),
            second.clone(),
            governed_fixture::original_work(),
        ),
    );
    host_continuation_shape::commit(
        &fixture.continuations,
        &busy_owner,
        observation_delivery(),
        &jobs,
    )
    .await
    .expect("commit for the busy session");
    host_continuation_shape::commit(&fixture.continuations, &idle_owner, second, &jobs)
        .await
        .expect("commit for the idle session");
    let binder = binder_of(&jobs);
    binder.not_ready.lock().expect("lock").push(busy.clone());
    let sink = Arc::new(RecordingSink::governed());
    let (attachments, attachment_commits) = watch::channel(0_u64);
    let handle = fixture
        .owner()
        .with_attachment_commits(attachment_commits)
        .arm(Host::with_jobs(sink.clone(), Arc::clone(&jobs)))
        .expect("arm");
    let mut passes = handle.subscribe_passes();
    let pass = wait_for_pass(&mut passes, "one admitted while the other waits", |pass| {
        pass.applied >= 1
            && pass
                .failures
                .iter()
                .any(|line| line.contains("NotReady(TurnBoundaryHeld)"))
    })
    .await;
    assert_eq!(pass.refused, 0, "never refused: {:?}", pass.failures);
    assert_eq!(
        sink.governed_admissions()
            .into_iter()
            .map(|(session, _)| session)
            .collect::<Vec<_>>(),
        vec![idle.clone()]
    );
    assert!(matches!(
        status_of(&fixture, &busy_owner, OBSERVATION_KEY).await,
        ContinuationStatus::Pending { .. }
    ));
    binder.not_ready.lock().expect("lock").clear();
    attachments.send_modify(|generation| *generation += 1);
    wait_for_status(
        &fixture,
        &busy_owner,
        OBSERVATION_KEY,
        &mut passes,
        |status| matches!(status, ContinuationStatus::Applied { .. }),
    )
    .await;
    assert_eq!(sink.governed_admissions().len(), 2);
}

/// Native admission that is not ready (a realm whose owners are not
/// installed answers `PolicyUnavailable`) keeps the observation pending with
/// its typed cause, never refused, and drops the custody; once ready it is
/// admitted under the id reserved for it.
#[tokio::test]
async fn a_native_readiness_failure_keeps_the_host_original_pending_until_ready() {
    let observation = Observation::committed().await;
    let sink = Arc::new(RecordingSink::governed());
    *sink.governed_verdict.lock().expect("lock") = Some(GovernedVerdict::NotReady);
    let mut armed = observation.arm_over(&sink);
    let pass = wait_for_pass(&mut armed.passes, "pending with its typed cause", |pass| {
        pass.refused > 0
            || pass
                .failures
                .iter()
                .any(|line| line.contains("NotReady(PolicyUnavailable)"))
    })
    .await;
    assert_eq!(pass.refused, 0, "never refused: {:?}", pass.failures);
    let binder = observation.binder();
    assert!(custody_dropped_once(&binder), "nothing was admitted");
    let reserved = sink.governed_inputs()[0].id().clone();
    *sink.governed_verdict.lock().expect("lock") = None;
    armed.wake();
    let status = observation.settled(&mut armed).await;
    assert_eq!(status, observation.applied(&observation.session, &reserved));
    assert_eq!(
        sink.governed_admissions(),
        vec![(observation.session.clone(), reserved)]
    );
}

/// The binder's verdicts: `Denied` refuses the observation for good
/// (`AuthorityDenied`); `NotOwned`, and a namespace owner that offers no
/// binder, refuse it `NoAdmissibleWorkBinding`; a binder that cannot be read
/// keeps it pending. Nothing is reserved or admitted, and no custody exists.
#[tokio::test]
async fn the_binders_verdicts_settle_or_keep_the_host_original_pending() {
    use meerkat_runtime::RuntimeDeliveryRefusalReason::{AuthorityDenied, NoAdmissibleWorkBinding};
    let cases = [
        (
            "denied",
            Some(meerkat::HostOriginalRefusal::Denied),
            true,
            Some(AuthorityDenied),
        ),
        (
            "not owned",
            Some(meerkat::HostOriginalRefusal::NotOwned),
            true,
            Some(NoAdmissibleWorkBinding),
        ),
        ("no binder", None, false, Some(NoAdmissibleWorkBinding)),
        (
            "unavailable",
            Some(meerkat::HostOriginalRefusal::Unavailable(
                "the host is restarting".into(),
            )),
            true,
            None,
        ),
    ];
    let mut mismatches = Vec::new();
    for (case, verdict, offers_binder, refused) in cases {
        let observation = Observation::committed().await;
        let binder = observation.binder();
        *binder.verdict.lock().expect("lock") = verdict;
        if !offers_binder {
            *observation.jobs.binder.lock().expect("lock") = None;
        }
        let sink = Arc::new(RecordingSink::governed());
        let mut armed = observation.arm_over(&sink);
        let status = match refused {
            Some(_) => observation.settled(&mut armed).await,
            None => {
                observation
                    .blocked(&mut armed, "the observation blocked")
                    .await;
                observation.status().await
            }
        };
        let expected = match refused {
            Some(reason) => {
                matches!(&status, ContinuationStatus::Refused { reason: settled, .. } if *settled == reason)
            }
            None => matches!(status, ContinuationStatus::Pending { .. }),
        };
        if !expected {
            mismatches.push(format!("{case}: {status:?}"));
        }
        if !sink.governed_inputs().is_empty() || observation.index().await.is_some() {
            mismatches.push(format!("{case}: reserved or admitted"));
        }
        if !custody_untouched(&binder) {
            mismatches.push(format!("{case}: a custody exists"));
        }
    }
    assert!(mismatches.is_empty(), "{mismatches:#?}");
}

/// Native admission's actual verdicts settle the observation with their
/// typed reason (a denial `AuthorityDenied`, malformed facts
/// `NoAdmissibleWorkBinding`), and the custody is dropped, never promoted.
#[tokio::test]
async fn native_refusals_settle_the_host_original_and_drop_its_custody() {
    use meerkat_runtime::RuntimeDeliveryRefusalReason::{AuthorityDenied, NoAdmissibleWorkBinding};
    let mut mismatches = Vec::new();
    for (verdict, reason) in [
        (GovernedVerdict::Denied, AuthorityDenied),
        (GovernedVerdict::Malformed, NoAdmissibleWorkBinding),
    ] {
        let observation = Observation::committed().await;
        let sink = Arc::new(RecordingSink::governed());
        *sink.governed_verdict.lock().expect("lock") = Some(verdict.clone());
        let mut armed = observation.arm_over(&sink);
        let status = observation.settled(&mut armed).await;
        if status != observation.refused(reason) {
            mismatches.push(format!("{verdict:?}: {status:?}"));
        }
        if !custody_dropped_once(&observation.binder()) {
            mismatches.push(format!("{verdict:?}: the custody was not dropped once"));
        }
    }
    assert!(mismatches.is_empty(), "{mismatches:#?}");
}

/// On a governed runtime with no namespace owner bound yet, a host original
/// decides nothing: it stays pending (never settled) and is admitted once the
/// host binds its continuation services.
#[tokio::test]
async fn without_a_bound_namespace_owner_a_host_original_stays_pending_until_bound() {
    let observation = Observation::committed().await;
    let sink = Arc::new(RecordingSink::governed());
    let bindings = Arc::new(meerkat::ContinuationHostBindings::default());
    let handle = observation
        .fixture
        .owner()
        .with_source_changes(bindings.subscribe_source_changes())
        .arm(Arc::new(BindingsHost {
            bindings: Arc::clone(&bindings),
            sink: sink.clone(),
        }))
        .expect("arm");
    let mut armed = Armed {
        passes: handle.subscribe_passes(),
        _handle: handle,
        attachments: watch::channel(0_u64).0,
    };
    let pass = observation
        .blocked(&mut armed, "the unbound observation blocked")
        .await;
    assert_eq!(pass.refused, 0, "never settled: {:?}", pass.failures);
    assert!(sink.governed_inputs().is_empty());
    let _binding = bindings
        .bind(Arc::new(SessionAddressResolver), observation.jobs.clone())
        .expect("bind");
    let status = observation.settled(&mut armed).await;
    assert!(
        matches!(&status, ContinuationStatus::Applied { session, .. } if *session == observation.session),
        "{status:?}"
    );
    assert_eq!(sink.governed_admissions().len(), 1);
}

/// The setup signal reads the BOUND composition: an ungoverned runtime admits
/// as before; a governed one without bound services is unbound; a reserved
/// namespace (whatever the source claims) or a namespace the source offers no
/// binder for is not offered; otherwise it is the binder's precheck now. It
/// asks the precheck once per question and never confirms, binds or admits.
#[tokio::test]
async fn host_original_readiness_reads_the_bound_composition_and_never_binds() {
    use meerkat::{HostOriginalBinderReadiness as Binder, HostOriginalReadiness as Readiness};
    use meerkat_core::authorization::ControllerReadinessFailure;
    let session = SessionId::new();
    let owner = ContinuationOwner::Session {
        session_id: session.clone(),
    };
    let ungoverned = meerkat_runtime::MeerkatMachine::ephemeral();
    let governed = meerkat_runtime::MeerkatMachine::ephemeral()
        .with_native_work_authorization_host(Arc::new(RefusingWorkAuthority))
        .expect("install the native work authorization host");
    let bindings = meerkat::ContinuationHostBindings::default();
    assert_eq!(
        bindings
            .host_original_readiness(&governed, INGEST_NAMESPACE, &owner)
            .await,
        Readiness::Unbound
    );
    let jobs = ingest_owner(&session);
    let _binding = bindings
        .bind(Arc::new(SessionAddressResolver), jobs.clone())
        .expect("bind");
    let binder = binder_of(&jobs);
    assert_eq!(
        bindings
            .host_original_readiness(&ungoverned, INGEST_NAMESPACE, &owner)
            .await,
        Readiness::Ungoverned
    );
    assert_eq!(
        bindings
            .host_original_readiness(&governed, INGEST_NAMESPACE, &owner)
            .await,
        Readiness::Offered(Binder::Ready)
    );
    assert_eq!(
        bindings
            .host_original_readiness(&governed, "app.other", &owner)
            .await,
        Readiness::NotOffered,
        "a namespace the source offers no binder for"
    );
    *binder.readiness.lock().expect("lock") = Some(Binder::Denied);
    assert_eq!(
        bindings
            .host_original_readiness(&governed, INGEST_NAMESPACE, &owner)
            .await,
        Readiness::Offered(Binder::Denied)
    );
    *binder.readiness.lock().expect("lock") = Some(Binder::NotReady(
        ControllerReadinessFailure::TurnBoundaryHeld,
    ));
    assert_eq!(
        bindings
            .host_original_readiness(&governed, INGEST_NAMESPACE, &owner)
            .await,
        Readiness::Offered(Binder::NotReady(
            ControllerReadinessFailure::TurnBoundaryHeld
        ))
    );
    *jobs.producer.lock().expect("lock") = Some(ContinuationProducer::Host {
        namespace: "meerkat.consent".into(),
    });
    assert_eq!(
        bindings
            .host_original_readiness(&governed, "meerkat.consent", &owner)
            .await,
        Readiness::NotOffered,
        "a reserved namespace is never offered, whatever the source claims"
    );
    assert_eq!(
        binder
            .readiness_calls
            .load(std::sync::atomic::Ordering::SeqCst),
        3,
        "one precheck per offered question, none for the reserved namespace"
    );
    assert_eq!(binder.calls(), 0, "never binds");
    assert_eq!(jobs.lookups(), 0, "never confirms an original");
}

/// The only code of the host continuation rows that depends on the governed
/// admission shape: how a host continuation is committed from its
/// namespace's committed owner, how that owner holds or disowns it, and what
/// a governed sink admitted. The rows above assert behaviour only and do not
/// change when the shape lands.
///
/// The shape: a host original is committed through
/// `ContinuationOwnerService::submit_host_original` from its namespace
/// owner's record, confirmed again by the bound owner at each admission, and
/// bound by the owner's binder for the admitting session.
mod host_continuation_shape {
    use super::*;

    /// How a row makes the namespace owner stop confirming a committed host
    /// continuation.
    #[derive(Debug, Clone, Copy)]
    pub(super) enum Disown {
        /// The owner holds another namespace.
        OtherNamespace,
        /// The owner holds it for another owner.
        OtherOwner,
        /// The owner committed other content.
        OtherResult,
        /// The owner holds it no more.
        Forgotten,
    }

    /// The namespace's committed owner, holding the observation owed to
    /// `session` and offering a binder for its namespace.
    pub(super) fn owner_of(session: &SessionId) -> Arc<FakeJobs> {
        let jobs = Arc::new(FakeJobs::default());
        *jobs.producer.lock().expect("lock") = Some(ingest_producer());
        jobs.originals.lock().expect("lock").insert(
            OBSERVATION.to_string(),
            meerkat::HostOriginalFacts::new(
                ContinuationOwner::Session {
                    session_id: session.clone(),
                },
                observation_delivery(),
                super::governed_fixture::original_work(),
            ),
        );
        *jobs.binder.lock().expect("lock") = Some(Arc::new(FakeBinder::default()));
        jobs
    }

    pub(super) fn disown(jobs: &FakeJobs, disown: Disown) {
        match disown {
            Disown::OtherNamespace => {
                *jobs.producer.lock().expect("lock") = Some(ContinuationProducer::Host {
                    namespace: "app.other".into(),
                });
            }
            Disown::OtherOwner => jobs.change_original(OBSERVATION, |facts| {
                facts.owner = ContinuationOwner::Session {
                    session_id: SessionId::new(),
                };
            }),
            Disown::OtherResult => jobs.change_original(OBSERVATION, |facts| {
                facts.delivery.result.result_digest = "sha256:another-observation".into();
            }),
            Disown::Forgotten => {
                jobs.originals.lock().expect("lock").remove(OBSERVATION);
            }
        }
    }

    /// Commit `delivery` for `owner` from its namespace owner's record.
    pub(super) async fn commit(
        continuations: &ContinuationOwnerService,
        owner: &ContinuationOwner,
        delivery: ContinuationDelivery,
        jobs: &FakeJobs,
    ) -> Result<meerkat::ContinuationReceipt, meerkat::ContinuationSubmitError> {
        let ContinuationProducer::Host { namespace } = &delivery.result.producer else {
            return Err(meerkat::ContinuationSubmitError::Invalid(
                "not a host".into(),
            ));
        };
        let record = meerkat::HostOriginalRecord::from_owner(
            jobs,
            namespace.clone(),
            delivery.result.producer_id.clone(),
        )
        .await
        .map_err(meerkat::ContinuationSubmitError::Unavailable)?
        .ok_or(meerkat::ContinuationSubmitError::NoAdmissibleWorkBinding)?;
        continuations
            .submit_host_original(owner, delivery, &record, 1)
            .await
    }

    /// What `sink` admitted through a governed admission, in order: the
    /// session and the input.
    pub(super) fn admitted(sink: &RecordingSink) -> Vec<(SessionId, InputId)> {
        sink.governed_admissions()
    }
}

/// A trusted host's governed claims and binder, as test data: an
/// association naming the admitting session's runtime and the confirmed
/// original, a trusted ingress observation and a pinned controller client.
mod governed_fixture {
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

    use super::*;
    use meerkat_authorization_contracts::constraints::{
        AudienceRef, ExecutionRestrictions, ResourceDomain,
    };
    use meerkat_authorization_contracts::evidence::{
        EvidenceDigest, EvidenceId, HistoricalEvidenceRef,
    };
    use meerkat_authorization_contracts::protocol::ContractRequirements;
    use meerkat_authorization_contracts::resource::ResourceRef;
    use meerkat_authorization_contracts::work_association::{
        GrantLineageRef, InputAuthorityAssociation, InputAuthorityAssociationCandidate,
        NativeWorkTarget, QualifiedIngressNamespace, WorkAuthorityBasis,
    };
    use meerkat_core::connection::RealmId;
    use meerkat_core::{PrincipalKind, PrincipalRef, TrustDomainId};

    pub(super) fn id(value: &str) -> EvidenceId {
        EvidenceId::new(value).expect("fixture id")
    }

    pub(super) fn principal(value: &str) -> PrincipalRef {
        PrincipalRef::in_domain(
            PrincipalKind::ServiceAccount,
            value,
            TrustDomainId::new("host-test").expect("domain"),
        )
        .expect("qualified")
    }

    fn evidence(value: &str) -> HistoricalEvidenceRef {
        HistoricalEvidenceRef {
            resource: ResourceRef {
                domain: ResourceDomain {
                    authority: principal("source-owner"),
                    namespace: "observations".into(),
                },
                resource_id: value.into(),
            },
            revision: id("r1"),
            digest: EvidenceDigest::from_array([7; 32]),
        }
    }

    fn realm() -> RealmId {
        RealmId::parse("host-test").expect("realm")
    }

    /// The original whose authority the observation's binding names.
    pub(super) fn original_work() -> meerkat_runtime::OriginalWorkRef {
        meerkat_runtime::OriginalWorkRef {
            authority: principal("source-owner"),
            work: id("observation-1"),
        }
    }

    pub(super) fn selection() -> meerkat_core::ControllerModelSelection {
        meerkat_core::ControllerModelSelection::new(
            meerkat_core::SessionLlmIdentity {
                model: "controller".into(),
                provider: meerkat_core::Provider::OpenAI,
                self_hosted_server_id: None,
                provider_params: None,
                auth_binding: None,
            },
            serde_json::from_value(serde_json::json!({"realm": "host-test", "account": "member"}))
                .expect("credential identity"),
            "profile".into(),
            "fixture".into(),
        )
    }

    /// A pinned controller that never runs a model.
    struct Controller(meerkat_core::ControllerModelSelection);

    #[async_trait::async_trait]
    impl meerkat_core::AgentLlmClient for Controller {
        fn controller_model_selection(&self) -> Option<meerkat_core::ControllerModelSelection> {
            Some(self.0.clone())
        }

        async fn stream_response(
            &self,
            _: &[meerkat_core::Message],
            _: &[Arc<meerkat_core::ToolDef>],
            _: u32,
            _: Option<f32>,
            _: Option<&meerkat_core::ProviderParamsOverride>,
        ) -> Result<meerkat_core::LlmStreamResult, meerkat_core::AgentError> {
            panic!("the host original fixture never runs a model")
        }

        fn provider(&self) -> meerkat_core::Provider {
            self.0.provider()
        }

        fn model(&self) -> &str {
            self.0.model()
        }
    }

    /// What a binder may get wrong, one field at a time.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(super) enum Tamper {
        Content,
        Key,
        Id,
        NoAssociation,
        NoIngress,
        OtherRuntime,
        OtherOriginal,
        NoControllerLineage,
        IngressRequester,
        NoController,
    }

    /// The host's custody record of what happened to the settlements it
    /// handed over.
    #[derive(Default)]
    pub(super) struct SettlementLog {
        pub(super) accepted: Mutex<Vec<meerkat::HostOriginalAdmission>>,
        pub(super) dropped: AtomicUsize,
        pub(super) retained: Mutex<Vec<String>>,
    }

    struct Settlement {
        log: Arc<SettlementLog>,
        settled: bool,
        accept_error: Option<String>,
    }

    impl meerkat::HostOriginalSettlement for Settlement {
        fn accepted(
            mut self: Box<Self>,
            admission: &meerkat::HostOriginalAdmission,
        ) -> Result<(), String> {
            self.settled = true;
            self.log
                .accepted
                .lock()
                .expect("lock")
                .push(admission.clone());
            match self.accept_error.take() {
                Some(error) => Err(error),
                None => Ok(()),
            }
        }
    }

    impl Drop for Settlement {
        fn drop(&mut self) {
            if !self.settled {
                self.log.dropped.fetch_add(1, Ordering::SeqCst);
            }
        }
    }

    /// A trusted host's binder of the observation namespace.
    #[derive(Default)]
    pub(super) struct FakeBinder {
        pub(super) log: Arc<SettlementLog>,
        /// Every bind asked: what Core confirmed, the session and the input
        /// it handed over.
        pub(super) calls: Mutex<Vec<(meerkat::HostOriginalRef, SessionId, meerkat_runtime::Input)>>,
        pub(super) verdict: Mutex<Option<meerkat::HostOriginalRefusal>>,
        /// Sessions the binder cannot bind now (a held turn boundary).
        pub(super) not_ready: Mutex<Vec<SessionId>>,
        pub(super) tamper: Mutex<Option<Tamper>>,
        /// The destination generation the binding names.
        pub(super) generation: AtomicU64,
        pub(super) accept_error: Mutex<Option<String>>,
        pub(super) readiness: Mutex<Option<meerkat::HostOriginalBinderReadiness>>,
        pub(super) readiness_calls: AtomicUsize,
        unresolved: Mutex<Vec<Box<dyn meerkat::HostOriginalSettlement>>>,
    }

    impl FakeBinder {
        pub(super) fn calls(&self) -> usize {
            self.calls.lock().expect("lock").len()
        }

        /// Bind `input` for `session` as a correct host would, naming
        /// `original_work`.
        pub(super) fn bind_now(
            &self,
            original_work: &meerkat_runtime::OriginalWorkRef,
            session: &SessionId,
            input: meerkat_runtime::Input,
        ) -> meerkat_runtime::Input {
            bind_input(
                original_work,
                session,
                input,
                self.generation.load(Ordering::SeqCst),
                None,
            )
        }
    }

    pub(super) fn association(
        session: &SessionId,
        original_work: &meerkat_runtime::OriginalWorkRef,
        generation: u64,
        tamper: Option<Tamper>,
    ) -> InputAuthorityAssociation {
        let runtime = if tamper == Some(Tamper::OtherRuntime) {
            LogicalRuntimeId::for_session(&SessionId::new()).to_string()
        } else {
            LogicalRuntimeId::for_session(session).to_string()
        };
        let original_work = if tamper == Some(Tamper::OtherOriginal) {
            meerkat_runtime::OriginalWorkRef {
                authority: principal("source-owner"),
                work: id("another-observation"),
            }
        } else {
            original_work.clone()
        };
        let controller_grant_lineage = if tamper == Some(Tamper::NoControllerLineage) {
            Vec::new()
        } else {
            vec![GrantLineageRef {
                root_authority: principal("grant-owner"),
                authority_namespace: id("controller"),
                authority_generation: 1,
                authority_incarnation: serde_json::from_value(serde_json::json!(
                    "00000000-0000-4000-8000-000000000001"
                ))
                .expect("fixture incarnation"),
                grant_id: id("controller-leaf"),
                issued_revision: 1,
            }]
        };
        InputAuthorityAssociation::new(InputAuthorityAssociationCandidate {
            requester: principal("commissioner"),
            ingress_actor: principal("collector"),
            represented_subject: None,
            original_authentication: evidence("observation-authentication"),
            logical_executor: principal("member"),
            target: NativeWorkTarget {
                logical_owner: principal("host"),
                logical_runtime: id(&runtime),
                context: id("context"),
                context_generation: generation,
                audience: AudienceRef::Principal {
                    principal: principal("audience"),
                },
            },
            original_work,
            root_event: evidence("observation"),
            contributing_work: Vec::new(),
            authority_basis: WorkAuthorityBasis::HostPolicy {
                policy: evidence("ingest-policy"),
            },
            controller_grant_lineage,
            controller_model: Some(selection()),
            controller_ceiling: ExecutionRestrictions::unrestricted(),
            admitted_ceiling: ExecutionRestrictions::unrestricted(),
            source_observations: Vec::new(),
            ingress_namespace: QualifiedIngressNamespace {
                realm: realm(),
                ingress: ResourceDomain {
                    authority: principal("collector"),
                    namespace: "observations".into(),
                },
                occurrence_scope: id("occurrence"),
            },
            contract: ContractRequirements::local_governed_v1(),
        })
        .expect("well-formed claims")
    }

    /// Bind `input` as a host does: set only the association, then attach a
    /// trusted ingress observation and the pinned controller to the final
    /// input. `tamper` makes one deliberate mistake.
    pub(super) fn bind_input(
        original_work: &meerkat_runtime::OriginalWorkRef,
        session: &SessionId,
        input: meerkat_runtime::Input,
        generation: u64,
        tamper: Option<Tamper>,
    ) -> meerkat_runtime::Input {
        let meerkat_runtime::Input::Prompt(mut prompt) = input else {
            panic!("a continuation is a prompt");
        };
        if tamper != Some(Tamper::NoAssociation) {
            prompt.header.authority_association =
                Some(association(session, original_work, generation, tamper));
        }
        match tamper {
            Some(Tamper::Content) => prompt.content = "another body".into(),
            Some(Tamper::Key) => {
                prompt.header.idempotency_key =
                    Some(meerkat_runtime::IdempotencyKey::new("another-key"));
            }
            Some(Tamper::Id) => prompt.header.id = InputId::new(),
            _ => {}
        }
        let input = meerkat_runtime::Input::Prompt(prompt);
        if tamper == Some(Tamper::NoIngress) {
            return input;
        }
        let requester = if tamper == Some(Tamper::IngressRequester) {
            principal("someone-else")
        } else {
            principal("commissioner")
        };
        let ingress = meerkat_runtime::input_authority::NativeIngressContext::from_trusted_ingress(
            &input,
            requester,
            principal("collector"),
            realm(),
            evidence("observation-authentication"),
        )
        .expect("ingress");
        let ingress =
            if tamper == Some(Tamper::NoController) || tamper == Some(Tamper::NoAssociation) {
                ingress
            } else {
                ingress
                    .with_controller_client(
                        &input,
                        meerkat_core::ControllerModelClient::new(
                            selection(),
                            Arc::new(Controller(selection())),
                        ),
                    )
                    .expect("the pinned controller")
            };
        input
            .with_ingress_context(ingress)
            .expect("bound to the final input")
    }

    #[async_trait::async_trait]
    impl meerkat::HostOriginalBinder for FakeBinder {
        async fn bind(
            &self,
            original: &meerkat::HostOriginalRef,
            session: &SessionId,
            input: meerkat_runtime::Input,
        ) -> Result<meerkat::BoundHostOriginal, meerkat::HostOriginalRefusal> {
            self.calls.lock().expect("lock").push((
                original.clone(),
                session.clone(),
                input.clone(),
            ));
            if self.not_ready.lock().expect("lock").contains(session) {
                return Err(meerkat::HostOriginalRefusal::NotReady(
                    meerkat_core::authorization::ControllerReadinessFailure::TurnBoundaryHeld,
                ));
            }
            if let Some(verdict) = self.verdict.lock().expect("lock").clone() {
                return Err(verdict);
            }
            let tamper = *self.tamper.lock().expect("lock");
            let bound = bind_input(
                &original.original_work,
                session,
                input,
                self.generation.load(Ordering::SeqCst),
                tamper,
            );
            Ok(meerkat::BoundHostOriginal::new(
                bound,
                Box::new(Settlement {
                    log: Arc::clone(&self.log),
                    settled: false,
                    accept_error: self.accept_error.lock().expect("lock").clone(),
                }),
            ))
        }

        async fn readiness(
            &self,
            _namespace: &str,
            _owner: &ContinuationOwner,
        ) -> meerkat::HostOriginalBinderReadiness {
            self.readiness_calls.fetch_add(1, Ordering::SeqCst);
            self.readiness
                .lock()
                .expect("lock")
                .clone()
                .unwrap_or(meerkat::HostOriginalBinderReadiness::Ready)
        }

        fn retain_unresolved(
            &self,
            settlement: Box<dyn meerkat::HostOriginalSettlement>,
            detail: &str,
        ) {
            self.log
                .retained
                .lock()
                .expect("lock")
                .push(detail.to_string());
            // Kept, never dropped: the host settles it later from the
            // runtime's own record.
            self.unresolved.lock().expect("lock").push(settlement);
        }
    }
}

use governed_fixture::{FakeBinder, Tamper};

/// The binder the namespace owner offers.
fn binder_of(jobs: &FakeJobs) -> Arc<FakeBinder> {
    jobs.binder.lock().expect("lock").clone().expect("a binder")
}

// ---------------------------------------------------------------------------
// Retired addresses meet governed host originals: a stranded answer stays
// final on the binder's path too, and a reservation left on a retired address
// is settled by the reserved id, as the drain recovers it.
// ---------------------------------------------------------------------------

/// The member `lead` of the mob `team`, owed the observation.
fn lead_of_team() -> ContinuationOwner {
    ContinuationOwner::Member {
        mob_id: "team".into(),
        identity: "lead".into(),
    }
}

/// The observation committed for the member `lead`, whose namespace owner
/// holds it for that member, on a fixture whose status read sees the
/// retirements: the receipt and the owner.
async fn a_member_observation(
    cut: &RetiringFixture,
    session: &SessionId,
) -> (meerkat::ContinuationReceipt, Arc<FakeJobs>) {
    let jobs = ingest_owner(session);
    jobs.change_original(OBSERVATION, |facts| facts.owner = lead_of_team());
    let receipt = host_continuation_shape::commit(
        &cut.fixture.continuations,
        &lead_of_team(),
        observation_delivery(),
        &jobs,
    )
    .await
    .expect("commit the member's observation from its namespace owner");
    assert!(
        LogicalRuntimeId::new(receipt.address.clone())
            .member_address()
            .expect("address")
            .is_some(),
        "the observation is owed at a member address"
    );
    (receipt, jobs)
}

/// The drain resolved the member's address and parked at its session's
/// route; then the address retired (a respawn, or the mob created again
/// under its id) and a status read answered stranded. Released, the drain
/// binds the observation and reserves it, observes the retire before native
/// admission, releases the reservation and withdraws the binder's custody:
/// nothing is admitted after the stranded answer.
#[tokio::test]
async fn a_member_host_original_whose_address_retires_before_native_admission_admits_nothing() {
    let cut = RetiringFixture::memory();
    let session = SessionId::new();
    let (receipt, jobs) = a_member_observation(&cut, &session).await;
    let address = LogicalRuntimeId::new(receipt.address.clone());
    let sink = Arc::new(RecordingSink::governed());
    let host = RetiringHost::with_jobs(
        sink.clone(),
        Arc::clone(&cut.retirements),
        Arc::clone(&jobs),
    );
    host.serve(&address, session.clone());
    let (entered, release) = host.hold_next_route();
    let handle = cut.fixture.owner().arm(host).expect("arm");
    let mut passes = handle.subscribe_passes();
    tokio::time::timeout(EVENT_GUARD, entered)
        .await
        .expect("the drain reached the member's session route")
        .expect("route hold");

    cut.retirements.retire(&address);
    let stranded = cut.status(&lead_of_team(), OBSERVATION_KEY).await;
    assert!(
        matches!(stranded, ContinuationStatus::Stranded { .. }),
        "never admitted, and the address retired: {stranded:?}"
    );

    release.send(()).expect("release the drain");
    wait_for_reconcile(&mut passes).await;
    assert!(
        sink.governed_inputs().is_empty(),
        "nothing reaches native admission after a stranded answer: {:?}",
        sink.governed_admissions()
    );
    assert_eq!(
        cut.admission_entry(&address, &receipt.delivery_id).await,
        None,
        "the reservation is released"
    );
    let binder = binder_of(&jobs);
    assert_eq!(binder.calls(), 1, "bound once, before the observation");
    assert!(
        custody_dropped_once(&binder),
        "the binder's custody is withdrawn, never promoted or retained"
    );
    let still = cut.status(&lead_of_team(), OBSERVATION_KEY).await;
    assert!(
        matches!(still, ContinuationStatus::Stranded { .. }),
        "a stranded continuation stays stranded: {still:?}"
    );
}

/// The same through a repoint: an earlier attempt left a reservation in s1
/// with nothing admitted, and the member's address now resolves to s2. The
/// address retires while the drain is parked, so a status read sees the
/// reservation and answers pending. Released, the drain repoints the reserved
/// id to s2, binds for s2, observes the retire, releases the repointed
/// reservation and withdraws the custody: nothing is admitted, and the
/// continuation reads stranded.
#[tokio::test]
async fn a_repointed_member_host_original_whose_address_retires_releases_its_reservation() {
    let cut = RetiringFixture::memory();
    let (s1, s2) = (SessionId::new(), SessionId::new());
    let (receipt, jobs) = a_member_observation(&cut, &s2).await;
    let address = LogicalRuntimeId::new(receipt.address.clone());
    cut.fixture
        .admission(
            &address,
            &receipt.delivery_id,
            ContinuationAdmissionTransition::Reserve {
                session_id: s1.clone(),
                input_id: InputId::new(),
            },
        )
        .await;
    let sink = Arc::new(RecordingSink::governed());
    let host = RetiringHost::with_jobs(
        sink.clone(),
        Arc::clone(&cut.retirements),
        Arc::clone(&jobs),
    );
    host.serve(&address, s2.clone());
    let (entered, release) = host.hold_next_route();
    let handle = cut.fixture.owner().arm(host).expect("arm");
    let mut passes = handle.subscribe_passes();
    tokio::time::timeout(EVENT_GUARD, entered)
        .await
        .expect("the drain reached the serving session's route")
        .expect("route hold");

    cut.retirements.retire(&address);
    let reserved = cut.status(&lead_of_team(), OBSERVATION_KEY).await;
    assert!(
        matches!(reserved, ContinuationStatus::Pending { admitted: None, .. }),
        "a reservation exists, so the read is not stranded: {reserved:?}"
    );

    release.send(()).expect("release the drain");
    wait_for_reconcile(&mut passes).await;
    assert!(
        sink.governed_inputs().is_empty(),
        "nothing reaches native admission after the retire: {:?}",
        sink.governed_admissions()
    );
    assert_eq!(
        cut.admission_entry(&address, &receipt.delivery_id).await,
        None,
        "the repointed reservation is released"
    );
    let binder = binder_of(&jobs);
    assert_eq!(binder.calls(), 1, "bound once, for s2");
    assert!(
        custody_dropped_once(&binder),
        "the binder's custody is withdrawn, never promoted or retained"
    );
    let stranded = cut.status(&lead_of_team(), OBSERVATION_KEY).await;
    assert!(
        matches!(stranded, ContinuationStatus::Stranded { .. }),
        "never admitted, and the address retired: {stranded:?}"
    );
}

/// A reservation an attempt left before a crash, on a member address that
/// retired since, whose bound observation reached the reserved session under
/// the reserved id. A governed admission is indexed under its qualified key,
/// so only a read by the reserved id finds it: the owner reads it so,
/// verifies it with the row's own recorded binding and records it, and the
/// continuation reads admitted. Nobody is asked, and nothing is admitted
/// again.
#[tokio::test]
async fn a_governed_reservation_on_a_retired_address_with_its_bound_row_reads_admitted() {
    let cut = RetiringFixture::memory();
    let session = SessionId::new();
    let (receipt, jobs) = a_member_observation(&cut, &session).await;
    let address = LogicalRuntimeId::new(receipt.address.clone());
    let reserved = InputId::new();
    cut.fixture
        .admission(
            &address,
            &receipt.delivery_id,
            ContinuationAdmissionTransition::Reserve {
                session_id: session.clone(),
                input_id: reserved.clone(),
            },
        )
        .await;
    let sink = Arc::new(RecordingSink::governed());
    let binder = binder_of(&jobs);
    sink.holds_by_id_only(
        &session,
        &binder.bind_now(
            &governed_fixture::original_work(),
            &session,
            continuation_input(reserved.clone(), &receipt.delivery_id, "a new item arrived"),
        ),
    );
    cut.retirements.retire(&address);

    let handle = cut
        .fixture
        .owner()
        .arm(RetiringHost::with_jobs(
            sink.clone(),
            Arc::clone(&cut.retirements),
            Arc::clone(&jobs),
        ))
        .expect("arm");
    let mut passes = handle.subscribe_passes();
    wait_for_reconcile(&mut passes).await;

    assert_eq!(
        cut.status(&lead_of_team(), OBSERVATION_KEY).await,
        ContinuationStatus::Pending {
            receipt,
            sequence: 1,
            admitted: Some((session, reserved)),
        },
        "the reserved session holds the bound observation"
    );
    assert!(
        sink.governed_inputs().is_empty(),
        "nothing is admitted again"
    );
    assert_eq!(binder.calls(), 0, "the binder is never asked");
    assert!(custody_untouched(&binder), "no custody is made or moved");
}

/// Pins the documented corner of the settle: the reserved session holds the
/// bound observation under ANOTHER input id (native admission deduplicated
/// the retry onto it, and the attempt crashed before it applied that id).
/// A governed admission is indexed under its qualified key, which only a
/// fresh bind derives, and a retired address is never bound again, so
/// neither the reserved id nor the raw key finds it: the owner releases the
/// reservation, and the continuation reads stranded. Keeping the reservation
/// instead would leave every governed reservation without its row pending
/// forever, so changing this is a reviewed decision.
#[tokio::test]
async fn a_governed_reservation_on_a_retired_address_deduplicated_onto_another_input_reads_stranded()
 {
    let cut = RetiringFixture::memory();
    let session = SessionId::new();
    let (receipt, jobs) = a_member_observation(&cut, &session).await;
    let address = LogicalRuntimeId::new(receipt.address.clone());
    cut.fixture
        .admission(
            &address,
            &receipt.delivery_id,
            ContinuationAdmissionTransition::Reserve {
                session_id: session.clone(),
                input_id: InputId::new(),
            },
        )
        .await;
    let sink = Arc::new(RecordingSink::governed());
    let binder = binder_of(&jobs);
    sink.holds_by_id_only(
        &session,
        &binder.bind_now(
            &governed_fixture::original_work(),
            &session,
            continuation_input(InputId::new(), &receipt.delivery_id, "a new item arrived"),
        ),
    );
    cut.retirements.retire(&address);

    let handle = cut
        .fixture
        .owner()
        .arm(RetiringHost::with_jobs(
            sink.clone(),
            Arc::clone(&cut.retirements),
            Arc::clone(&jobs),
        ))
        .expect("arm");
    let mut passes = handle.subscribe_passes();
    wait_for_reconcile(&mut passes).await;

    let status = cut.status(&lead_of_team(), OBSERVATION_KEY).await;
    assert!(
        matches!(status, ContinuationStatus::Stranded { .. }),
        "neither read finds the deduplicated input: {status:?}"
    );
    assert_eq!(
        cut.admission_entry(&address, &receipt.delivery_id).await,
        None,
        "the reservation is released"
    );
    assert!(sink.governed_inputs().is_empty(), "nothing is admitted");
    assert_eq!(binder.calls(), 0, "the binder is never asked");
}
