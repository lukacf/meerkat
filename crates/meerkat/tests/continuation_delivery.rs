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
        self.known.lock().expect("lock").insert(
            (session.clone(), key),
            StoredInputState::accepted_for_test(input).expect("row"),
        );
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
}

/// A host whose address resolution the test moves (a repoint, an unserved
/// member), with an optional continuation sink.
struct Host {
    sink: Option<Arc<RecordingSink>>,
    resolutions: Mutex<HashMap<LogicalRuntimeId, AddressResolution>>,
    job_source: Option<Arc<FakeJobs>>,
}

impl Host {
    fn new(sink: Option<Arc<RecordingSink>>) -> Arc<Self> {
        Arc::new(Self {
            sink,
            resolutions: Mutex::new(HashMap::new()),
            job_source: None,
        })
    }

    fn with_jobs(sink: Arc<RecordingSink>, jobs: Arc<FakeJobs>) -> Arc<Self> {
        Arc::new(Self {
            sink: Some(sink),
            resolutions: Mutex::new(HashMap::new()),
            job_source: Some(jobs),
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

/// The committed owner of fork_off jobs, as a table the test edits.
#[derive(Default)]
struct FakeJobs {
    jobs: Mutex<HashMap<String, RetainedJobFacts>>,
    unavailable: Mutex<Option<String>>,
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
}

#[async_trait::async_trait]
impl RetainedJobSource for FakeJobs {
    async fn retained_job(
        &self,
        producer: &ContinuationProducer,
        job_id: &str,
    ) -> RetainedJobLookup {
        assert_eq!(producer, &ContinuationProducer::ForkOff);
        if let Some(error) = self.unavailable.lock().expect("lock").clone() {
            return RetainedJobLookup::Unavailable(error);
        }
        self.jobs
            .lock()
            .expect("lock")
            .get(job_id)
            .cloned()
            .map_or(RetainedJobLookup::Absent, RetainedJobLookup::Found)
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
    let address = meerkat::member_delivery_address("team", "lead", 1).expect("address");
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
    let address = meerkat::member_delivery_address("team", "lead", 1).expect("address");
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

/// The binding slot is set once per binder. A second binder is refused while
/// the first lives; a takeover must name the current binding's generation,
/// and a stale one is refused, typed. A dropped binding frees the slot.
#[tokio::test]
async fn a_stale_or_blind_continuation_rebind_is_refused() {
    let bindings = meerkat::ContinuationHostBindings::default();
    let member = meerkat::member_delivery_address("team", "lead", 1).expect("address");
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
        let address = meerkat::member_delivery_address("team", "lead", 1).expect("address");
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
