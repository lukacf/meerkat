#![allow(clippy::expect_used, clippy::panic)]
//! The library delivery owner applies durable job deliveries on typed wakes
//! only: a reconcile pass at arming, then job outbox commits, runtime delivery
//! commits, and attachment commits. No pass ever runs on a timer. A host's
//! owed continuations are projected in rounds, one page per pass, that only a
//! reconcile or a source change starts.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use meerkat::{
    AttemptClaim, CanonicalArgumentsHash, ContinuationDelivery, ContinuationDeliverySink,
    ContinuationHandling, ContinuationKey, ContinuationOwner, ContinuationOwnerService,
    ContinuationProducer, ContinuationResultRef, ContinuationStatus, DetachedJobService,
    InteractionLineageId, JobDeliveryApplication, JobDeliveryApplyError, JobDeliverySink, JobId,
    JobResultRef, JobSpec, JobSubmissionKey, MemoryDetachedJobStore, OwedContinuationCursor,
    OwedContinuationProjection, RestartClass, RunnerHandleRef, RunnerIdentity, RuntimeDeliveryHost,
    RuntimeDeliveryOwner, RuntimeDeliveryPass, SessionId, ToolIdentity, WorkerId,
};
use meerkat_runtime::{
    InMemoryRuntimeStore, RuntimeDeliveryInbox, RuntimeDeliveryOwnerAlreadyArmed,
};
use tokio::sync::{Mutex, watch};

/// Hang guard for awaited typed events; never a pacing mechanism.
const EVENT_GUARD: Duration = Duration::from_secs(10);

/// Records the job id of every applied delivery; rejects the poisoned job
/// until healed. Optionally commits one more job outbox entry from inside its
/// first application, to race a commit against a running pass.
#[derive(Default)]
struct RecordingSink {
    poisoned: std::sync::Mutex<Option<JobId>>,
    locally_refused: std::sync::Mutex<HashSet<JobId>>,
    applied: Mutex<Vec<JobId>>,
    commit_during_first_apply: Mutex<Option<(DetachedJobService, SessionId)>>,
}

impl RecordingSink {
    fn refuse(&self, job_id: JobId) {
        self.locally_refused
            .lock()
            .expect("refusal lock")
            .insert(job_id);
    }

    fn poison(&self, job_id: JobId) {
        *self.poisoned.lock().expect("poison lock") = Some(job_id);
    }

    fn heal(&self) {
        *self.poisoned.lock().expect("poison lock") = None;
    }

    async fn applied(&self) -> Vec<JobId> {
        self.applied.lock().await.clone()
    }
}

#[async_trait::async_trait]
impl JobDeliverySink for RecordingSink {
    async fn apply(
        &self,
        application: JobDeliveryApplication,
    ) -> Result<(), JobDeliveryApplyError> {
        let job_id = match &application {
            JobDeliveryApplication::Record { job_id, .. }
            | JobDeliveryApplication::Notification { job_id, .. }
            | JobDeliveryApplication::Event { job_id, .. } => job_id.clone(),
        };
        if self
            .locally_refused
            .lock()
            .expect("refusal lock")
            .contains(&job_id)
        {
            return Err(meerkat_core::OperationAuthorizationError::Refused(
                meerkat_core::OperationRefused::new(meerkat_core::OperationRefusalKind::Denied),
            )
            .into());
        }
        if self.poisoned.lock().expect("poison lock").as_ref() == Some(&job_id) {
            return Err(JobDeliveryApplyError::Infrastructure(format!(
                "sink rejects deliveries for job {job_id}"
            )));
        }
        if let Some((jobs, session_id)) = self.commit_during_first_apply.lock().await.take() {
            completed_job(&jobs, "default", session_id, "committed-during-pass").await;
        }
        self.applied.lock().await.push(job_id);
        Ok(())
    }
}

struct StaticHost {
    sink: Arc<RecordingSink>,
}

#[async_trait::async_trait]
impl RuntimeDeliveryHost for StaticHost {
    async fn delivery_route(&self, _session_id: &SessionId) -> Option<meerkat::DeliveryRoute> {
        Some(meerkat::DeliveryRoute::ServedHere(self.sink.clone()))
    }

    async fn claim_cold_delivery(
        &self,
        session_id: &SessionId,
    ) -> Option<Result<meerkat_runtime::HostingClaim, meerkat_runtime::HostingRefused>> {
        // Every recipient is served here: a single-process host.
        Some(meerkat_runtime::grant_session_hosting(
            &meerkat_runtime::HostingCapability::ProcessLocal,
            &meerkat_runtime::HostingOwner::mint(),
            session_id,
        ))
    }
}

struct Fixture {
    job_store: Arc<MemoryDetachedJobStore>,
    jobs: DetachedJobService,
    inbox: RuntimeDeliveryInbox,
    sink: Arc<RecordingSink>,
}

impl Fixture {
    fn new() -> Self {
        let job_store = Arc::new(MemoryDetachedJobStore::new());
        Self {
            jobs: DetachedJobService::new(job_store.clone()),
            job_store,
            inbox: RuntimeDeliveryInbox::new(Arc::new(InMemoryRuntimeStore::new())),
            sink: Arc::new(RecordingSink::default()),
        }
    }

    fn owner(&self) -> RuntimeDeliveryOwner {
        RuntimeDeliveryOwner::new(self.job_store.clone(), self.inbox.clone())
    }

    fn host(&self) -> Arc<dyn RuntimeDeliveryHost> {
        Arc::new(StaticHost {
            sink: self.sink.clone(),
        })
    }
}

fn spec(realm_id: &str, key: &str, session_id: SessionId) -> JobSpec {
    JobSpec::new(
        realm_id,
        session_id,
        meerkat::ExecutionIntentId::new(),
        InteractionLineageId::new(),
        ToolIdentity::new("shell", "1").expect("tool"),
        RunnerIdentity::new("durable-shell", "1").expect("runner"),
        RestartClass::Adoptable,
        CanonicalArgumentsHash::new(format!("hash-{key}")).expect("hash"),
        JobSubmissionKey::new(key).expect("submission key"),
    )
}

async fn completed_job(
    jobs: &DetachedJobService,
    realm_id: &str,
    session_id: SessionId,
    key: &str,
) -> JobId {
    completed_job_from_spec(jobs, spec(realm_id, key, session_id)).await
}

async fn completed_job_from_spec(jobs: &DetachedJobService, spec: JobSpec) -> JobId {
    let receipt = jobs.submit(spec).await.expect("submit");
    let claim = jobs
        .claim_attempt(
            &receipt.job_id,
            AttemptClaim::new(
                WorkerId::new("worker").expect("worker"),
                1,
                100,
                RunnerHandleRef::new("runner-handle").expect("handle"),
            ),
        )
        .await
        .expect("claim");
    jobs.complete_attempt(
        &receipt.job_id,
        (&claim).into(),
        2,
        Some(JobResultRef::new("result").expect("result")),
    )
    .await
    .expect("complete");
    receipt.job_id
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

async fn wait_for_applied(
    sink: &RecordingSink,
    job_id: &JobId,
    passes: &mut watch::Receiver<RuntimeDeliveryPass>,
) {
    let deadline_guard = tokio::time::timeout(EVENT_GUARD, async {
        loop {
            if sink.applied().await.contains(job_id) {
                return;
            }
            passes.changed().await.expect("owner pass channel open");
        }
    });
    deadline_guard
        .await
        .unwrap_or_else(|_| panic!("job {job_id} was never applied"));
}

/// Let the clock run far past any plausible poll interval and let every
/// woken task run; a timer-driven owner would complete a pass here.
async fn idle_for_an_hour() {
    tokio::time::advance(Duration::from_hours(1)).await;
    for _ in 0..64 {
        tokio::task::yield_now().await;
    }
}

#[tokio::test(start_paused = true)]
async fn armed_owner_reconciles_existing_rows_then_delivers_new_commits_without_a_timer() {
    let fixture = Fixture::new();
    let session_id = SessionId::new();
    let before_arm =
        completed_job(&fixture.jobs, "default", session_id.clone(), "before-arm").await;

    let handle = fixture.owner().arm(fixture.host()).expect("arm owner");
    let mut passes = handle.subscribe_passes();
    let reconciled = wait_for_pass(&mut passes, "the reconcile pass", |pass| {
        pass.generation >= 1
    })
    .await;
    assert_eq!(
        (
            reconciled.generation,
            reconciled.projected,
            reconciled.applied
        ),
        (1, 1, 1),
        "one reconcile pass projects and applies the existing outbox, with no follow-up pass"
    );
    assert_eq!(fixture.sink.applied().await, vec![before_arm.clone()]);

    let after_arm = completed_job(&fixture.jobs, "default", session_id, "after-arm").await;
    wait_for_applied(&fixture.sink, &after_arm, &mut passes).await;
    assert_eq!(fixture.sink.applied().await, vec![before_arm, after_arm]);

    let settled = passes.borrow_and_update().generation;
    idle_for_an_hour().await;
    assert!(
        !passes.has_changed().expect("owner pass channel open"),
        "no pass ran without a typed wake (still at generation {settled})"
    );
    assert!(!handle.is_stopped());
}

#[tokio::test(start_paused = true)]
async fn a_full_locally_refused_page_does_not_hide_the_next_permitted_delivery() {
    let fixture = Fixture::new();
    let session_id = SessionId::new();
    let projector =
        meerkat::JobOutboxProjector::new(fixture.job_store.clone(), fixture.inbox.clone());
    let mut permitted = None;
    // The owner's page size is 256. Project each real job before submitting
    // the next so the final permitted job has durable sequence 257, regardless
    // of the job store's enumeration order. No owner is armed during setup.
    for index in 0..257 {
        let job = completed_job(
            &fixture.jobs,
            "default",
            session_id.clone(),
            &format!("page-{index}"),
        )
        .await;
        if index < 256 {
            fixture.sink.refuse(job);
        } else {
            permitted = Some(job);
        }
        let projected = projector.project_pending(1).await.expect("project one job");
        assert!(projected.is_fully_projected());
        assert_eq!(projected.projected.len(), 1);
    }
    drop(projector);
    let handle = fixture
        .owner()
        .arm(fixture.host())
        .expect("arm owner after setup");
    let mut passes = handle.subscribe_passes();
    let reconciled = wait_for_pass(&mut passes, "one complete reconcile pass", |pass| {
        pass.generation >= 1
    })
    .await;
    assert_eq!(reconciled.generation, 1);
    assert_eq!(reconciled.projected, 0);
    assert_eq!(
        reconciled.applied, 1,
        "refused recipients are progress, not applied effects"
    );
    assert!(reconciled.blocked_sessions.is_empty());
    assert!(reconciled.failures.is_empty());
    assert_eq!(
        fixture.sink.applied().await,
        vec![permitted.expect("last job")]
    );
    assert_eq!(
        fixture
            .inbox
            .pending_delivery_total()
            .await
            .expect("pending count"),
        0
    );
    assert_eq!(
        fixture
            .inbox
            .applied_cursor(&meerkat_runtime::LogicalRuntimeId::for_session(&session_id))
            .await
            .expect("settled cursor"),
        257,
    );
}

#[tokio::test]
async fn a_second_owner_on_the_same_inbox_is_refused_until_the_first_is_dropped() {
    let fixture = Fixture::new();
    let first = fixture.owner().arm(fixture.host()).expect("first owner");
    let refused = fixture.owner().arm(fixture.host());
    assert_eq!(refused.err().map(|_| ()), Some(()));
    assert_eq!(
        fixture.inbox.claim_delivery_ownership().err(),
        Some(RuntimeDeliveryOwnerAlreadyArmed)
    );

    drop(first);
    // Dropping aborts the owner task; its ownership is released once the
    // aborted task is dropped by the runtime.
    let second = tokio::time::timeout(EVENT_GUARD, async {
        loop {
            match fixture.owner().arm(fixture.host()) {
                Ok(handle) => return handle,
                Err(RuntimeDeliveryOwnerAlreadyArmed) => tokio::task::yield_now().await,
            }
        }
    })
    .await
    .expect("ownership is released after the first owner is dropped");
    assert!(!second.is_stopped());
}

#[tokio::test(start_paused = true)]
async fn a_blocked_session_is_retried_when_an_attachment_serves_and_not_before() {
    let fixture = Fixture::new();
    let session_id = SessionId::new();
    let job = completed_job(&fixture.jobs, "default", session_id.clone(), "blocked").await;
    fixture.sink.poison(job.clone());
    let (attachments, attachment_commits) = watch::channel(0_u64);

    let handle = fixture
        .owner()
        .with_attachment_commits(attachment_commits)
        .arm(fixture.host())
        .expect("arm owner");
    let mut passes = handle.subscribe_passes();
    let blocked = wait_for_pass(&mut passes, "the blocked reconcile pass", |pass| {
        pass.generation >= 1
    })
    .await;
    assert_eq!(blocked.blocked_sessions, vec![session_id.clone()]);
    assert!(fixture.sink.applied().await.is_empty());

    fixture.sink.heal();
    idle_for_an_hour().await;
    assert!(
        fixture.sink.applied().await.is_empty(),
        "a blocked row is not retried on a timer"
    );

    attachments.send_modify(|generation| *generation += 1);
    wait_for_applied(&fixture.sink, &job, &mut passes).await;
    let retried = passes.borrow().clone();
    assert!(retried.blocked_sessions.is_empty());
}

#[tokio::test]
async fn mob_realm_rows_drain_on_the_same_owner() {
    let fixture = Fixture::new();
    let host_session = SessionId::new();
    let member_session = SessionId::new();
    let handle = fixture.owner().arm(fixture.host()).expect("arm owner");
    let mut passes = handle.subscribe_passes();

    let host_job = completed_job(&fixture.jobs, "default", host_session, "host-job").await;
    let member_job = completed_job(&fixture.jobs, "mob.team", member_session, "member-job").await;
    wait_for_applied(&fixture.sink, &host_job, &mut passes).await;
    wait_for_applied(&fixture.sink, &member_job, &mut passes).await;
}

#[tokio::test]
async fn a_commit_landing_during_a_pass_is_delivered_by_the_next_pass() {
    let fixture = Fixture::new();
    let session_id = SessionId::new();
    *fixture.sink.commit_during_first_apply.lock().await =
        Some((fixture.jobs.clone(), session_id.clone()));
    let first = completed_job(&fixture.jobs, "default", session_id, "first").await;

    let handle = fixture.owner().arm(fixture.host()).expect("arm owner");
    let mut passes = handle.subscribe_passes();
    wait_for_applied(&fixture.sink, &first, &mut passes).await;
    tokio::time::timeout(EVENT_GUARD, async {
        loop {
            if fixture.sink.applied().await.len() == 2 {
                return;
            }
            passes.changed().await.expect("owner pass channel open");
        }
    })
    .await
    .expect("the entry committed during the first pass is delivered by a later pass");
}

/// A terminal whose producer applies it (the shell's completion feed) is
/// committed already acknowledged, by whichever of the producer and the owner
/// projects it first, so no sink ever runs for it and it never holds the
/// cursor.
#[tokio::test]
async fn a_producer_applied_terminal_never_reaches_a_sink() {
    let fixture = Fixture::new();
    let session_id = SessionId::new();
    let handle = fixture.owner().arm(fixture.host()).expect("arm owner");
    let mut passes = handle.subscribe_passes();

    let produced = completed_job_from_spec(
        &fixture.jobs,
        spec("default", "producer-applied", session_id.clone())
            .with_terminal_application(meerkat::JobTerminalApplication::Producer),
    )
    .await;
    let subscribed = completed_job(&fixture.jobs, "default", session_id, "subscribers").await;
    wait_for_applied(&fixture.sink, &subscribed, &mut passes).await;
    assert_eq!(
        fixture.sink.applied().await,
        vec![subscribed],
        "the producer-applied terminal {produced} never reaches the sink"
    );
    assert_eq!(
        fixture
            .inbox
            .pending_delivery_total()
            .await
            .expect("backlog read"),
        0
    );
}

/// The shell projects its own terminal before any owner runs; the row is
/// already acknowledged when the owner's reconcile pass reads it.
#[tokio::test]
async fn a_producer_projected_terminal_is_acknowledged_before_the_owner_reads_it() {
    use meerkat_tools::builtin::shell::ShellJobDeliveryProjector as _;

    let fixture = Fixture::new();
    let session_id = SessionId::new();
    let produced = completed_job_from_spec(
        &fixture.jobs,
        spec("default", "shell-projected", session_id)
            .with_terminal_application(meerkat::JobTerminalApplication::Producer),
    )
    .await;
    meerkat::JobOutboxProjector::new(fixture.job_store.clone(), fixture.inbox.clone())
        .project_job(produced.as_str())
        .await
        .expect("producer projects its terminal");

    let handle = fixture.owner().arm(fixture.host()).expect("arm owner");
    let mut passes = handle.subscribe_passes();
    let reconciled = wait_for_pass(&mut passes, "the reconcile pass", |pass| {
        pass.generation >= 1
    })
    .await;
    assert_eq!((reconciled.projected, reconciled.failures.len()), (0, 0));
    assert!(fixture.sink.applied().await.is_empty());
    assert_eq!(
        fixture
            .inbox
            .pending_delivery_total()
            .await
            .expect("backlog read"),
        0
    );
}

/// A run settlement (a callback batch resolves inside a run) retries a
/// blocked session just like an attachment commit, and nothing retries it on
/// a timer.
#[tokio::test(start_paused = true)]
async fn a_blocked_session_is_retried_when_a_run_settles() {
    let fixture = Fixture::new();
    let session_id = SessionId::new();
    let job = completed_job(&fixture.jobs, "default", session_id.clone(), "settles").await;
    fixture.sink.poison(job.clone());
    let (settlements, run_settlements) = watch::channel(0_u64);

    let handle = fixture
        .owner()
        .with_run_settlements(run_settlements)
        .arm(fixture.host())
        .expect("arm owner");
    let mut passes = handle.subscribe_passes();
    let blocked = wait_for_pass(&mut passes, "the blocked reconcile pass", |pass| {
        pass.generation >= 1
    })
    .await;
    assert_eq!(blocked.blocked_sessions, vec![session_id]);

    fixture.sink.heal();
    idle_for_an_hour().await;
    assert!(fixture.sink.applied().await.is_empty());

    settlements.send_modify(|generation| *generation += 1);
    wait_for_applied(&fixture.sink, &job, &mut passes).await;
    assert!(passes.borrow().blocked_sessions.is_empty());
}

/// Run settlements fire on every runtime loop iteration; with nothing
/// blocked they run no pass at all.
#[tokio::test]
async fn a_run_settlement_with_nothing_blocked_runs_no_pass() {
    let fixture = Fixture::new();
    let (settlements, run_settlements) = watch::channel(0_u64);
    let handle = fixture
        .owner()
        .with_run_settlements(run_settlements)
        .arm(fixture.host())
        .expect("arm owner");
    let mut passes = handle.subscribe_passes();
    wait_for_pass(&mut passes, "the reconcile pass", |pass| {
        pass.generation >= 1
    })
    .await;
    passes.borrow_and_update();

    for _ in 0..8 {
        settlements.send_modify(|generation| *generation += 1);
        for _ in 0..16 {
            tokio::task::yield_now().await;
        }
    }
    let session_id = SessionId::new();
    let job = completed_job(&fixture.jobs, "default", session_id, "after-settlements").await;
    wait_for_applied(&fixture.sink, &job, &mut passes).await;
    assert_eq!(
        passes.borrow().generation,
        2,
        "the settlements ran no pass; the commit ran the second"
    );
}

/// Only a producer-applied TERMINAL takes the acknowledged path: a
/// notification of the same job (a monitor line) is still applied by the
/// owner.
#[tokio::test]
async fn a_producer_jobs_notification_is_applied_while_its_terminal_is_not() {
    let fixture = Fixture::new();
    let session_id = SessionId::new();
    let handle = fixture.owner().arm(fixture.host()).expect("arm owner");
    let mut passes = handle.subscribe_passes();

    let receipt = fixture
        .jobs
        .submit(
            spec("default", "producer-monitor", session_id)
                .with_terminal_application(meerkat::JobTerminalApplication::Producer),
        )
        .await
        .expect("submit");
    let job = receipt.job_id;
    let claim = fixture
        .jobs
        .claim_attempt(
            &job,
            AttemptClaim::new(
                WorkerId::new("worker").expect("worker"),
                1,
                100,
                RunnerHandleRef::new("runner-handle").expect("handle"),
            ),
        )
        .await
        .expect("claim");
    fixture
        .jobs
        .emit_notification(
            &job,
            (&claim).into(),
            2,
            meerkat::JobNotification::new("line", "line:1", "Match", "pattern seen")
                .expect("notification"),
        )
        .await
        .expect("emit notification");
    wait_for_applied(&fixture.sink, &job, &mut passes).await;

    fixture
        .jobs
        .complete_attempt(&job, (&claim).into(), 3, None)
        .await
        .expect("complete");
    tokio::time::timeout(EVENT_GUARD, async {
        loop {
            if fixture
                .inbox
                .pending_delivery_total()
                .await
                .expect("backlog read")
                == 0
                && passes.borrow().generation >= 2
            {
                return;
            }
            passes.changed().await.expect("owner pass channel open");
        }
    })
    .await
    .expect("the terminal is consumed without a sink");
    assert_eq!(
        fixture.sink.applied().await,
        vec![job],
        "the notification reached the sink once; the producer-applied terminal never did"
    );
}

/// Admits every continuation it is handed, recording the session and the
/// admission key.
#[derive(Default)]
struct AdmittingSink {
    admitted: std::sync::Mutex<Vec<(SessionId, String)>>,
}

impl AdmittingSink {
    fn admitted(&self) -> Vec<(SessionId, String)> {
        self.admitted.lock().expect("admission lock").clone()
    }
}

#[async_trait::async_trait]
impl ContinuationDeliverySink for AdmittingSink {
    async fn admit(
        &self,
        session: &SessionId,
        input: meerkat_runtime::Input,
    ) -> Result<meerkat_core::lifecycle::InputId, String> {
        let admission_key = input
            .header()
            .idempotency_key
            .as_ref()
            .expect("continuations are keyed")
            .to_string();
        self.admitted
            .lock()
            .expect("admission lock")
            .push((session.clone(), admission_key));
        Ok(input.id().clone())
    }

    async fn admitted_input(
        &self,
        _session: &SessionId,
        _admission_key: &str,
    ) -> Result<Option<meerkat_runtime::input_state::StoredInputState>, String> {
        Ok(None)
    }
}

type PageHook = Box<dyn FnOnce(&OwingHost) + Send>;

/// Owes one continuation per key to its session and commits them as
/// continuation rows, through a continuation owner service over the owner's
/// inbox, at most `page` obligations a page, in key order. An obligation
/// stays owed after its row is committed (its admission is never recorded
/// here), so every round visits every key again.
struct OwingHost {
    owner: ContinuationOwner,
    owed: std::sync::Mutex<Vec<String>>,
    page: usize,
    /// While set, the enumeration cannot be read.
    unavailable: std::sync::atomic::AtomicBool,
    continuations: ContinuationOwnerService,
    committed: std::sync::Mutex<HashSet<String>>,
    /// Every page asked: where it started, and the keys it visited.
    pages: std::sync::Mutex<Vec<(Option<String>, Vec<String>)>>,
    /// Runs once, at the end of the next page.
    after_page: std::sync::Mutex<Option<PageHook>>,
    /// The source's own change signal, as an approval owner announces its
    /// consent notice obligations.
    announcer: watch::Sender<u64>,
    sink: Arc<AdmittingSink>,
    jobs: Arc<RecordingSink>,
}

impl OwingHost {
    fn new(fixture: &Fixture, session_id: &SessionId, page: usize) -> Arc<Self> {
        Arc::new(Self {
            owner: ContinuationOwner::Session {
                session_id: session_id.clone(),
            },
            owed: std::sync::Mutex::default(),
            page,
            unavailable: std::sync::atomic::AtomicBool::new(false),
            continuations: ContinuationOwnerService::new(
                fixture.inbox.clone(),
                Arc::new(meerkat::SessionAddressResolver),
                Arc::new(meerkat_runtime::MeerkatMachine::ephemeral()),
            ),
            committed: std::sync::Mutex::default(),
            pages: std::sync::Mutex::default(),
            after_page: std::sync::Mutex::default(),
            announcer: watch::Sender::new(0),
            sink: Arc::new(AdmittingSink::default()),
            jobs: fixture.sink.clone(),
        })
    }

    fn owe(&self, key: &str) {
        self.owed.lock().expect("owed lock").push(key.to_string());
    }

    /// The source announces a change of what it owes.
    fn announce(&self) {
        self.announcer
            .send_modify(|revision| *revision = revision.wrapping_add(1));
    }

    fn set_unavailable(&self, unavailable: bool) {
        self.unavailable
            .store(unavailable, std::sync::atomic::Ordering::SeqCst);
    }

    fn after_next_page(&self, hook: impl FnOnce(&Self) + Send + 'static) {
        *self.after_page.lock().expect("hook lock") = Some(Box::new(hook));
    }

    fn pages(&self) -> Vec<(Option<String>, Vec<String>)> {
        self.pages.lock().expect("page lock").clone()
    }

    async fn status(&self, key: &str) -> ContinuationStatus {
        self.continuations
            .continuation_status(&self.owner, &ContinuationKey::new(key).expect("key"))
            .await
            .expect("status")
    }
}

fn owed_delivery(key: &str) -> ContinuationDelivery {
    ContinuationDelivery {
        key: ContinuationKey::new(key).expect("key"),
        result: ContinuationResultRef {
            producer: ContinuationProducer::Host {
                namespace: "owed".to_string(),
            },
            producer_id: key.to_string(),
            result_digest: format!("digest-{key}"),
            summary: None,
        },
        body: format!("owed {key}").into(),
        handling: ContinuationHandling::Queue,
    }
}

/// A page as [`OwingHost::pages`] records it.
fn page(after: Option<&str>, keys: &[&str]) -> (Option<String>, Vec<String>) {
    (
        after.map(str::to_string),
        keys.iter().map(|key| (*key).to_string()).collect(),
    )
}

#[async_trait::async_trait]
impl RuntimeDeliveryHost for OwingHost {
    async fn delivery_route(&self, _session_id: &SessionId) -> Option<meerkat::DeliveryRoute> {
        Some(meerkat::DeliveryRoute::ServedHere(self.jobs.clone()))
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

    fn projects_owed_continuations(&self) -> bool {
        true
    }

    fn subscribe_owed_continuation_changes(&self) -> Option<watch::Receiver<u64>> {
        Some(self.announcer.subscribe())
    }

    async fn project_owed_continuations(
        &self,
        after: Option<&OwedContinuationCursor>,
        limit: usize,
    ) -> OwedContinuationProjection {
        let after = after.map(|after| after.as_str().to_string());
        let mut projection = OwedContinuationProjection::default();
        let mut visited = Vec::new();
        if self.unavailable.load(std::sync::atomic::Ordering::SeqCst) {
            projection
                .failures
                .push("the owed enumeration is unavailable".to_string());
        } else {
            let mut owed = self.owed.lock().expect("owed lock").clone();
            owed.sort();
            visited = owed
                .iter()
                .filter(|key| after.as_ref().is_none_or(|after| *key > after))
                .take(limit.min(self.page))
                .cloned()
                .collect();
            for key in &visited {
                match self
                    .continuations
                    .submit(&self.owner, owed_delivery(key), 1)
                    .await
                {
                    Ok(_) => {
                        if self
                            .committed
                            .lock()
                            .expect("commit lock")
                            .insert(key.clone())
                        {
                            projection.committed += 1;
                        }
                    }
                    Err(error) => projection.failures.push(format!("{key}: {error}")),
                }
            }
            projection.next = visited
                .last()
                .filter(|last| owed.iter().any(|key| key > *last))
                .map(|last| OwedContinuationCursor::new(last.clone()));
        }
        self.pages.lock().expect("page lock").push((after, visited));
        let hook = self.after_page.lock().expect("hook lock").take();
        if let Some(hook) = hook {
            hook(self);
        }
        projection
    }
}

/// With nothing blocked, a source change projects an obligation committed
/// after arming, and the row it commits drains in the same pass. The round
/// ends there: the obligation stays owed, and no pass runs without a wake.
#[tokio::test(start_paused = true)]
async fn a_source_change_projects_an_owed_continuation_that_drains_in_the_same_pass() {
    let fixture = Fixture::new();
    let session_id = SessionId::new();
    let host = OwingHost::new(&fixture, &session_id, 8);
    let (sources, source_changes) = watch::channel(0_u64);
    let handle = fixture
        .owner()
        .with_source_changes(source_changes)
        .arm(host.clone())
        .expect("arm owner");
    let mut passes = handle.subscribe_passes();
    let reconciled = wait_for_pass(&mut passes, "the reconcile pass", |pass| {
        pass.generation >= 1
    })
    .await;
    assert_eq!(
        (reconciled.owed_projected, reconciled.applied),
        (0, 0),
        "nothing was owed at arming"
    );
    assert!(reconciled.blocked_runtimes.is_empty());
    assert_eq!(
        host.pages(),
        vec![page(None, &[])],
        "the arming reconcile asked one page"
    );

    host.owe("notice-1");
    sources.send_modify(|revision| *revision += 1);
    let delivered = wait_for_pass(&mut passes, "the source change pass", |pass| {
        pass.generation >= 2
    })
    .await;
    assert_eq!(
        (
            delivered.generation,
            delivered.owed_projected,
            delivered.applied
        ),
        (2, 1, 1),
        "one pass projected the obligation and drained its row"
    );
    assert!(delivered.failures.is_empty(), "{delivered:?}");
    match host.status("notice-1").await {
        ContinuationStatus::Applied { session, .. } => assert_eq!(session, session_id),
        other => panic!("the notice was not admitted: {other:?}"),
    }
    assert_eq!(host.sink.admitted().len(), 1);

    let settled = passes.borrow_and_update().generation;
    idle_for_an_hour().await;
    assert!(
        !passes.has_changed().expect("owner pass channel open"),
        "no pass ran without a wake (still at generation {settled})"
    );
    assert_eq!(host.pages().len(), 2, "one page per round");
}

/// More obligations than a page: the round goes on, a page per pass, until
/// its enumeration ends, and every row drains in the pass that committed it.
/// The round ends there although every obligation stays owed; a later source
/// change runs one new round, which commits nothing new.
#[tokio::test(start_paused = true)]
async fn owed_continuations_past_one_page_are_all_delivered_by_one_finite_round() {
    let fixture = Fixture::new();
    let session_id = SessionId::new();
    let host = OwingHost::new(&fixture, &session_id, 2);
    let keys = ["owed-1", "owed-2", "owed-3", "owed-4", "owed-5"];
    for key in keys {
        host.owe(key);
    }
    let (sources, source_changes) = watch::channel(0_u64);
    let handle = fixture
        .owner()
        .with_source_changes(source_changes)
        .arm(host.clone())
        .expect("arm owner");
    let mut passes = handle.subscribe_passes();
    let last_page = wait_for_pass(&mut passes, "the arming round's last page", |pass| {
        pass.generation >= 3
    })
    .await;
    assert_eq!(
        host.pages(),
        vec![
            page(None, &["owed-1", "owed-2"]),
            page(Some("owed-2"), &["owed-3", "owed-4"]),
            page(Some("owed-4"), &["owed-5"]),
        ],
        "one page per pass, each after the last"
    );
    assert_eq!(
        (
            last_page.generation,
            last_page.owed_projected,
            last_page.applied
        ),
        (3, 1, 1),
        "the last page's row drained in its own pass"
    );
    for key in keys {
        assert!(
            matches!(host.status(key).await, ContinuationStatus::Applied { .. }),
            "{key} was not admitted"
        );
    }
    assert_eq!(host.sink.admitted().len(), keys.len());

    passes.borrow_and_update();
    idle_for_an_hour().await;
    assert!(
        !passes.has_changed().expect("owner pass channel open"),
        "the round ended with its enumeration, though every obligation stays owed"
    );
    assert_eq!(host.pages().len(), 3);

    sources.send_modify(|revision| *revision += 1);
    let repeated = wait_for_pass(&mut passes, "the second round's last page", |pass| {
        pass.generation >= 6
    })
    .await;
    assert_eq!(
        (
            repeated.generation,
            repeated.owed_projected,
            repeated.applied
        ),
        (6, 0, 0),
        "the new round visited every page again and committed nothing new"
    );
    assert_eq!(host.pages().len(), 6);
    assert_eq!(
        host.sink.admitted().len(),
        keys.len(),
        "nothing was admitted twice"
    );
    passes.borrow_and_update();
    idle_for_an_hour().await;
    assert!(!passes.has_changed().expect("owner pass channel open"));
}

/// An obligation committed while a round runs, before the round's cursor, is
/// announced by a source change: one more round follows the running one,
/// from the first page, and projects it.
#[tokio::test(start_paused = true)]
async fn an_obligation_committed_behind_a_running_round_gets_one_more_round() {
    let fixture = Fixture::new();
    let session_id = SessionId::new();
    let host = OwingHost::new(&fixture, &session_id, 1);
    host.owe("owed-2");
    host.owe("owed-3");
    let (sources, source_changes) = watch::channel(0_u64);
    let sources = Arc::new(sources);
    let announce = Arc::clone(&sources);
    host.after_next_page(move |host| {
        host.owe("owed-1");
        announce.send_modify(|revision| *revision += 1);
    });
    let handle = fixture
        .owner()
        .with_source_changes(source_changes)
        .arm(host.clone())
        .expect("arm owner");
    let mut passes = handle.subscribe_passes();
    wait_for_pass(&mut passes, "the second round's last page", |pass| {
        pass.generation >= 5
    })
    .await;
    assert_eq!(
        host.pages(),
        vec![
            page(None, &["owed-2"]),
            page(Some("owed-2"), &["owed-3"]),
            page(None, &["owed-1"]),
            page(Some("owed-1"), &["owed-2"]),
            page(Some("owed-2"), &["owed-3"]),
        ],
        "the running round finished, then one more round ran from the first page"
    );
    for key in ["owed-1", "owed-2", "owed-3"] {
        assert!(
            matches!(host.status(key).await, ContinuationStatus::Applied { .. }),
            "{key} was not admitted"
        );
    }
    passes.borrow_and_update();
    idle_for_an_hour().await;
    assert!(!passes.has_changed().expect("owner pass channel open"));
    assert_eq!(host.pages().len(), 5);
}

/// Run settlements fire on every runtime loop iteration: they never start a
/// round of owed continuations, which only a reconcile or a source change
/// does.
#[tokio::test]
async fn a_run_settlement_never_starts_a_round_of_owed_continuations() {
    let fixture = Fixture::new();
    let session_id = SessionId::new();
    let host = OwingHost::new(&fixture, &session_id, 8);
    let (settlements, run_settlements) = watch::channel(0_u64);
    let (sources, source_changes) = watch::channel(0_u64);
    let handle = fixture
        .owner()
        .with_run_settlements(run_settlements)
        .with_source_changes(source_changes)
        .arm(host.clone())
        .expect("arm owner");
    let mut passes = handle.subscribe_passes();
    wait_for_pass(&mut passes, "the reconcile pass", |pass| {
        pass.generation >= 1
    })
    .await;
    passes.borrow_and_update();

    host.owe("unannounced");
    for _ in 0..8 {
        settlements.send_modify(|generation| *generation += 1);
        for _ in 0..16 {
            tokio::task::yield_now().await;
        }
    }
    assert_eq!(host.pages().len(), 1, "no settlement asked for a page");
    assert!(
        !passes.has_changed().expect("owner pass channel open"),
        "and none ran a pass"
    );

    sources.send_modify(|revision| *revision += 1);
    let announced = wait_for_pass(&mut passes, "the source change pass", |pass| {
        pass.generation >= 2
    })
    .await;
    assert_eq!(
        (
            announced.generation,
            announced.owed_projected,
            announced.applied
        ),
        (2, 1, 1)
    );
}

/// An enumeration that cannot be read ends its round with the failure
/// reported. It is never retried in a loop: it stays owed to the next wake,
/// whatever signal that is.
#[tokio::test(start_paused = true)]
async fn an_unreadable_owed_enumeration_is_retried_on_the_next_wake_never_in_a_loop() {
    let fixture = Fixture::new();
    let session_id = SessionId::new();
    let host = OwingHost::new(&fixture, &session_id, 8);
    host.owe("owed-1");
    host.set_unavailable(true);
    let (settlements, run_settlements) = watch::channel(0_u64);
    let handle = fixture
        .owner()
        .with_run_settlements(run_settlements)
        .arm(host.clone())
        .expect("arm owner");
    let mut passes = handle.subscribe_passes();
    let failed = wait_for_pass(&mut passes, "the reconcile pass", |pass| {
        pass.generation >= 1
    })
    .await;
    assert_eq!(
        failed.failures,
        vec!["the owed enumeration is unavailable".to_string()]
    );
    assert_eq!(failed.owed_projected, 0);

    passes.borrow_and_update();
    idle_for_an_hour().await;
    assert!(
        !passes.has_changed().expect("owner pass channel open"),
        "the failure is not retried in a loop"
    );
    assert_eq!(host.pages().len(), 1);

    host.set_unavailable(false);
    // Any wake retries it: here a run settlement, which on its own never
    // starts a round.
    settlements.send_modify(|generation| *generation += 1);
    let retried = wait_for_pass(&mut passes, "the retry pass", |pass| pass.generation >= 2).await;
    assert_eq!((retried.owed_projected, retried.applied), (1, 1));
    assert!(retried.failures.is_empty(), "{retried:?}");
    assert!(matches!(
        host.status("owed-1").await,
        ContinuationStatus::Applied { .. }
    ));
}

/// A job delivery whose projection into the runtime inbox failed stays
/// pending and is retried on the next wake of any kind, not only on a job
/// commit or a reconcile, and never in a loop.
#[tokio::test(start_paused = true)]
async fn a_skipped_projection_is_retried_on_the_next_wake_never_in_a_loop() {
    let fixture = Fixture::new();
    let (settlements, run_settlements) = watch::channel(0_u64);
    let handle = fixture
        .owner()
        .with_run_settlements(run_settlements)
        .arm(fixture.host())
        .expect("arm owner");
    let mut passes = handle.subscribe_passes();
    wait_for_pass(&mut passes, "the reconcile pass", |pass| {
        pass.generation >= 1
    })
    .await;

    fixture.inbox.fail_next_submits_for_test(1);
    let job = completed_job(&fixture.jobs, "default", SessionId::new(), "skipped-once").await;
    let failed = wait_for_pass(&mut passes, "the failed projection", |pass| {
        !pass.failures.is_empty()
    })
    .await;
    assert_eq!(failed.projected, 0, "{failed:?}");

    passes.borrow_and_update();
    idle_for_an_hour().await;
    assert!(
        !passes.has_changed().expect("owner pass channel open"),
        "the failed projection is not retried in a loop"
    );
    assert!(fixture.sink.applied().await.is_empty());

    // Any wake retries it: here a run settlement, which on its own never
    // projects.
    settlements.send_modify(|generation| *generation += 1);
    let retried = wait_for_pass(&mut passes, "the retry pass", |pass| {
        pass.generation > failed.generation
    })
    .await;
    assert_eq!(retried.projected, 1, "{retried:?}");
    assert!(retried.failures.is_empty(), "{retried:?}");
    wait_for_applied(&fixture.sink, &job, &mut passes).await;
}

/// A job projection that keeps failing is retried on later wakes with a
/// doubling gap, never on every wake: a busy runtime's run settlements do
/// not each retry it. A projection that skips nothing clears the gap.
#[tokio::test(start_paused = true)]
async fn a_failing_projection_is_retried_with_a_doubling_gap_in_wakes() {
    let fixture = Fixture::new();
    let (settlements, run_settlements) = watch::channel(0_u64);
    let handle = fixture
        .owner()
        .with_run_settlements(run_settlements)
        .arm(fixture.host())
        .expect("arm owner");
    let mut passes = handle.subscribe_passes();
    wait_for_pass(&mut passes, "the reconcile pass", |pass| {
        pass.generation >= 1
    })
    .await;

    fixture.inbox.fail_next_submits_for_test(usize::MAX);
    let job = completed_job(&fixture.jobs, "default", SessionId::new(), "keeps-failing").await;
    wait_for_pass(&mut passes, "the failed projection", |pass| {
        !pass.failures.is_empty()
    })
    .await;
    passes.borrow_and_update();

    // Seven settlements, each its own wake: the retries join the 1st, the
    // 3rd and the 7th.
    let mut retried_on = Vec::new();
    for settlement in 1..=7 {
        settlements.send_modify(|generation| *generation += 1);
        idle_for_an_hour().await;
        if passes.has_changed().expect("owner pass channel open") {
            let pass = passes.borrow_and_update().clone();
            assert!(!pass.failures.is_empty(), "{pass:?}");
            retried_on.push(settlement);
        }
    }
    assert_eq!(retried_on, vec![1, 3, 7]);
    assert!(fixture.sink.applied().await.is_empty());

    // Once it can succeed, the next due retry (8 wakes on) applies it.
    fixture.inbox.fail_next_submits_for_test(0);
    for _ in 0..8 {
        settlements.send_modify(|generation| *generation += 1);
        idle_for_an_hour().await;
    }
    wait_for_applied(&fixture.sink, &job, &mut passes).await;

    // That success cleared the gap: a new failure is retried on the next
    // wake again.
    fixture.inbox.fail_next_submits_for_test(1);
    let next = completed_job(&fixture.jobs, "default", SessionId::new(), "fails-once").await;
    wait_for_pass(&mut passes, "the new failure", |pass| {
        !pass.failures.is_empty()
    })
    .await;
    settlements.send_modify(|generation| *generation += 1);
    wait_for_applied(&fixture.sink, &next, &mut passes).await;
}

/// A host that owes no continuations keeps the old path: a source change
/// with nothing blocked runs no pass.
#[tokio::test]
async fn a_source_change_with_nothing_blocked_runs_no_pass_for_a_host_owing_nothing() {
    let fixture = Fixture::new();
    let (sources, source_changes) = watch::channel(0_u64);
    let handle = fixture
        .owner()
        .with_source_changes(source_changes)
        .arm(fixture.host())
        .expect("arm owner");
    let mut passes = handle.subscribe_passes();
    wait_for_pass(&mut passes, "the reconcile pass", |pass| {
        pass.generation >= 1
    })
    .await;
    passes.borrow_and_update();

    for _ in 0..8 {
        sources.send_modify(|revision| *revision += 1);
        for _ in 0..16 {
            tokio::task::yield_now().await;
        }
    }
    let session_id = SessionId::new();
    let job = completed_job(&fixture.jobs, "default", session_id, "after-source-changes").await;
    wait_for_applied(&fixture.sink, &job, &mut passes).await;
    assert_eq!(
        passes.borrow().generation,
        2,
        "the source changes ran no pass; the commit ran the second"
    );
}

/// A prompt that stays queued on a session no executor serves.
fn queued_prompt() -> meerkat_runtime::input::Input {
    use meerkat_runtime::input::{
        Input, InputDurability, InputHeader, InputOrigin, InputVisibility, PromptInput,
    };
    Input::Prompt(PromptInput {
        injected_context: Vec::new(),
        header: InputHeader {
            ingress_context: None,
            retained_resume: None,
            authority_association: None,
            id: meerkat_core::lifecycle::InputId::new(),
            timestamp: chrono::Utc::now(),
            source: InputOrigin::Operator,
            durability: InputDurability::Durable,
            visibility: InputVisibility::default(),
            idempotency_key: None,
            supersession_key: None,
            correlation_id: None,
        },
        content: "queued until cancelled".into(),
        typed_turn_appends: Vec::new(),
        turn_metadata: None,
    })
}

/// A runtime's runless input terminal reaches the owner's next round through
/// its continuation bindings' source-change hint, with no reconcile and no
/// injected tick: cancelling a queued input projects an obligation the source
/// owes but nothing announced. Run settlements and attachments are not
/// subscribed here, so the hint is the only wake.
#[tokio::test]
async fn a_runless_input_terminal_starts_a_round_through_the_continuation_bindings() {
    use meerkat_runtime::SessionServiceRuntimeExt;

    let fixture = Fixture::new();
    let host = OwingHost::new(&fixture, &SessionId::new(), 8);
    let bindings = Arc::new(meerkat::ContinuationHostBindings::default());
    let runtime = Arc::new(meerkat_runtime::MeerkatMachine::ephemeral());
    bindings.announce_runless_input_terminals_of(&runtime);
    let handle = fixture
        .owner()
        .with_source_changes(bindings.subscribe_source_changes())
        .arm(host.clone())
        .expect("arm owner");
    let mut passes = handle.subscribe_passes();
    wait_for_pass(&mut passes, "the arming round", |pass| pass.generation >= 1).await;
    assert_eq!(host.pages(), vec![page(None, &[])], "the arming round");

    let session_id = SessionId::new();
    runtime
        .register_session(session_id.clone())
        .await
        .expect("register session");
    let input = queued_prompt();
    let input_id = input.id().clone();
    assert!(
        runtime
            .accept_input(&session_id, input)
            .await
            .expect("accept the input")
            .is_accepted()
    );
    host.owe("notice-1");

    assert!(
        runtime
            .cancel_input_if_present(&session_id, &input_id, "the notice input ended")
            .await
            .expect("cancel the queued input"),
        "the queued input is observed"
    );
    let projected = wait_for_pass(&mut passes, "the round the terminal starts", |pass| {
        pass.generation >= 2
    })
    .await;
    assert_eq!(projected.owed_projected, 1, "{projected:?}");
    assert_eq!(
        host.pages(),
        vec![page(None, &[]), page(None, &["notice-1"])],
        "the terminal started one new round"
    );
}

/// A persistence bundle announces its runtime's runless input terminals on
/// its own continuation bindings, so every delivery owner it composes hears
/// them.
#[cfg(feature = "session-store")]
#[tokio::test]
async fn a_persistence_bundle_announces_its_runtime_runless_input_terminals() {
    use meerkat_runtime::SessionServiceRuntimeExt;

    let bundle = meerkat::PersistenceBundle::new(
        Arc::new(meerkat_store::MemoryStore::new()) as Arc<dyn meerkat::SessionStore>,
        Arc::new(InMemoryRuntimeStore::new()) as Arc<dyn meerkat_runtime::RuntimeStore>,
        Arc::new(meerkat::MemoryBlobStore::new()),
    )
    .expect("persistence bundle");
    let source_changes = bundle.continuation_bindings().subscribe_source_changes();
    let before = *source_changes.borrow();
    let runtime = bundle.runtime_adapter();
    let session_id = SessionId::new();
    runtime
        .register_session(session_id.clone())
        .await
        .expect("register session");
    let input = queued_prompt();
    let input_id = input.id().clone();
    assert!(
        runtime
            .accept_input(&session_id, input)
            .await
            .expect("accept the input")
            .is_accepted()
    );
    assert_eq!(
        *source_changes.borrow(),
        before,
        "an admission that displaces nothing announces nothing"
    );

    assert!(
        runtime
            .cancel_input_if_present(&session_id, &input_id, "the notice input ended")
            .await
            .expect("cancel the queued input"),
        "the queued input is observed"
    );
    assert_eq!(
        *source_changes.borrow(),
        before + 1,
        "the bundle's bindings heard the terminal once"
    );
}

/// The owing source's own signal: an obligation it commits after the owner
/// armed and then announces is projected by exactly one round, with no
/// reconcile and no bindings source change, and an unannounced one waits.
#[tokio::test(start_paused = true)]
async fn an_owed_change_the_source_announces_runs_exactly_one_round() {
    let fixture = Fixture::new();
    let session_id = SessionId::new();
    let host = OwingHost::new(&fixture, &session_id, 8);
    let handle = fixture.owner().arm(host.clone()).expect("arm owner");
    let mut passes = handle.subscribe_passes();
    wait_for_pass(&mut passes, "the arming round", |pass| pass.generation >= 1).await;
    assert_eq!(host.pages(), vec![page(None, &[])], "the arming round");

    host.owe("notice-1");
    idle_for_an_hour().await;
    assert_eq!(
        host.pages().len(),
        1,
        "an obligation the source has not announced waits"
    );

    host.announce();
    let projected = wait_for_pass(&mut passes, "the announced round", |pass| {
        pass.generation >= 2
    })
    .await;
    assert_eq!(
        (projected.owed_projected, projected.applied),
        (1, 1),
        "{projected:?}"
    );
    assert_eq!(
        host.pages(),
        vec![page(None, &[]), page(None, &["notice-1"])],
        "the announcement started one round"
    );

    let settled = passes.borrow_and_update().generation;
    idle_for_an_hour().await;
    assert!(
        !passes.has_changed().expect("owner pass channel open"),
        "exactly one round ran (still at generation {settled})"
    );
    assert_eq!(host.pages().len(), 2, "exactly one round");
}

/// Records the session each application reached.
#[derive(Default)]
struct RecipientSink {
    applied: Mutex<Vec<(JobId, SessionId)>>,
}

#[async_trait::async_trait]
impl JobDeliverySink for RecipientSink {
    async fn apply(
        &self,
        application: JobDeliveryApplication,
    ) -> Result<(), JobDeliveryApplyError> {
        let (job_id, session_id) = match &application {
            JobDeliveryApplication::Record {
                job_id,
                subscription,
                ..
            }
            | JobDeliveryApplication::Notification {
                job_id,
                subscription,
                ..
            }
            | JobDeliveryApplication::Event {
                job_id,
                subscription,
                ..
            } => (job_id.clone(), subscription.session_id().clone()),
        };
        self.applied.lock().await.push((job_id, session_id));
        Ok(())
    }
}

/// A host whose mob member, addressed at `member`, was repointed by an
/// explicit resume to the session `successor`: the member's address resolves
/// to the successor. Every session is served here, the superseded one too.
struct RepointedMemberHost {
    sink: Arc<RecipientSink>,
    member: meerkat_runtime::LogicalRuntimeId,
    successor: SessionId,
}

#[async_trait::async_trait]
impl RuntimeDeliveryHost for RepointedMemberHost {
    async fn delivery_route(&self, _session_id: &SessionId) -> Option<meerkat::DeliveryRoute> {
        Some(meerkat::DeliveryRoute::ServedHere(self.sink.clone()))
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

    async fn resolve_address(
        &self,
        address: &meerkat_runtime::LogicalRuntimeId,
    ) -> meerkat::AddressResolution {
        if address == &self.member {
            meerkat::AddressResolution::Session(self.successor.clone())
        } else {
            meerkat::default_address_resolution(address)
        }
    }
}

/// Submit, claim, notify once and complete a job from `spec`.
async fn notified_and_completed_job(jobs: &DetachedJobService, spec: JobSpec) -> JobId {
    let job = jobs.submit(spec).await.expect("submit").job_id;
    let claim = jobs
        .claim_attempt(
            &job,
            AttemptClaim::new(
                WorkerId::new("worker").expect("worker"),
                1,
                100,
                RunnerHandleRef::new("runner-handle").expect("handle"),
            ),
        )
        .await
        .expect("claim");
    jobs.emit_notification(
        &job,
        (&claim).into(),
        2,
        meerkat::JobNotification::new("line", "line:1", "Match", "pattern seen")
            .expect("notification"),
    )
    .await
    .expect("emit notification");
    jobs.complete_attempt(
        &job,
        (&claim).into(),
        3,
        Some(JobResultRef::new("result").expect("result")),
    )
    .await
    .expect("complete");
    job
}

/// Wait until `count` applications reached the sink.
async fn wait_for_applications(
    sink: &RecipientSink,
    count: usize,
    passes: &mut watch::Receiver<RuntimeDeliveryPass>,
) -> Vec<(JobId, SessionId)> {
    tokio::time::timeout(EVENT_GUARD, async {
        loop {
            let applied = sink.applied.lock().await.clone();
            if applied.len() >= count {
                return applied;
            }
            passes.changed().await.expect("owner pass channel open");
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{count} applications never reached the sink"))
}

/// #1790: a job a mob member's session submitted records the member's
/// delivery address as its origin member. Its notification and its terminal
/// are applied in the session serving the member when they are delivered,
/// the successor after an explicit-resume repoint, never in the superseded
/// session it was submitted from.
#[tokio::test]
async fn a_member_jobs_deliveries_reach_the_member_after_a_repoint() {
    let fixture = Fixture::new();
    let superseded = SessionId::new();
    let successor = SessionId::new();
    let member = meerkat::member_delivery_address("team", team_incarnation(), "worker", 1)
        .expect("member address");
    let sink = Arc::new(RecipientSink::default());
    let handle = fixture
        .owner()
        .arm(Arc::new(RepointedMemberHost {
            sink: sink.clone(),
            member: member.clone(),
            successor: successor.clone(),
        }))
        .expect("arm owner");
    let mut passes = handle.subscribe_passes();

    let job = notified_and_completed_job(
        &fixture.jobs,
        spec("default", "member-job", superseded.clone()).with_origin_member_id(
            meerkat::OriginMemberId::new(member.0.clone()).expect("origin member"),
        ),
    )
    .await;
    let applied = wait_for_applications(&sink, 2, &mut passes).await;
    assert_eq!(
        applied,
        vec![(job.clone(), successor.clone()), (job, successor)],
        "the member's job reached the session it was submitted from, not the member"
    );
}

/// A job that records no member (or an origin member that is not a member
/// delivery address) is delivered to its origin session, as before.
#[tokio::test]
async fn a_job_without_a_member_address_reaches_its_origin_session() {
    let fixture = Fixture::new();
    let origin = SessionId::new();
    let member = meerkat::member_delivery_address("team", team_incarnation(), "worker", 1)
        .expect("member address");
    let sink = Arc::new(RecipientSink::default());
    let handle = fixture
        .owner()
        .arm(Arc::new(RepointedMemberHost {
            sink: sink.clone(),
            member,
            successor: SessionId::new(),
        }))
        .expect("arm owner");
    let mut passes = handle.subscribe_passes();

    let plain =
        notified_and_completed_job(&fixture.jobs, spec("default", "plain", origin.clone())).await;
    let opaque = notified_and_completed_job(
        &fixture.jobs,
        spec("default", "opaque", origin.clone())
            .with_origin_member_id(meerkat::OriginMemberId::new("worker-7").expect("member id")),
    )
    .await;
    let mut applied = wait_for_applications(&sink, 4, &mut passes).await;
    applied.sort();
    let mut expected = vec![
        (plain.clone(), origin.clone()),
        (plain, origin.clone()),
        (opaque.clone(), origin.clone()),
        (opaque, origin),
    ];
    expected.sort();
    assert_eq!(applied, expected);
}

/// Reports one session as the current session of the member at `address`.
#[cfg(feature = "session-store")]
struct MemberSessions {
    session: SessionId,
    address: meerkat_runtime::LogicalRuntimeId,
}

#[cfg(feature = "session-store")]
#[async_trait::async_trait]
impl meerkat::ContinuationAddressResolver for MemberSessions {
    async fn current_address(
        &self,
        _owner: &ContinuationOwner,
    ) -> Result<Option<meerkat_runtime::LogicalRuntimeId>, String> {
        Ok(None)
    }

    async fn resolve_address(
        &self,
        _address: &meerkat_runtime::LogicalRuntimeId,
    ) -> Result<meerkat::AddressResolution, String> {
        Ok(meerkat::AddressResolution::NotServed)
    }

    async fn member_address_of_session(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<meerkat_runtime::LogicalRuntimeId>, String> {
        Ok((session_id == &self.session).then(|| self.address.clone()))
    }
}

#[cfg(feature = "session-store")]
struct NoRetainedJobs;

#[cfg(feature = "session-store")]
#[async_trait::async_trait]
impl meerkat::RetainedJobSource for NoRetainedJobs {
    async fn retained_job(
        &self,
        _producer: &ContinuationProducer,
        _job_id: &str,
    ) -> meerkat::RetainedJobLookup {
        meerkat::RetainedJobLookup::Absent
    }
}

/// #1790: the job store a bundle hands producers records, as a submission's
/// origin member, the delivery address of the member whose current session
/// submits the job, as
/// the host's bound continuation resolver reports it. A session no member
/// serves, and a submission that already names an origin member, are stored
/// as given.
#[cfg(feature = "session-store")]
#[tokio::test]
async fn a_bundles_job_store_records_the_submitting_members_address() {
    let bundle = meerkat::PersistenceBundle::new(
        Arc::new(meerkat_store::MemoryStore::new()) as Arc<dyn meerkat::SessionStore>,
        Arc::new(InMemoryRuntimeStore::new()) as Arc<dyn meerkat_runtime::RuntimeStore>,
        Arc::new(meerkat::MemoryBlobStore::new()),
    )
    .expect("persistence bundle");
    let member_session = SessionId::new();
    let member = meerkat::member_delivery_address("team", team_incarnation(), "worker", 1)
        .expect("member address");
    let _binding = bundle
        .continuation_bindings()
        .bind(
            Arc::new(MemberSessions {
                session: member_session.clone(),
                address: member.clone(),
            }),
            Arc::new(NoRetainedJobs),
        )
        .expect("bind the host's resolver");
    let store = bundle.producer_job_store();
    let jobs = DetachedJobService::new(store.clone());

    let stamped = jobs
        .submit(spec("default", "stamped", member_session.clone()))
        .await
        .expect("submit")
        .job_id;
    let plain = jobs
        .submit(spec("default", "plain", SessionId::new()))
        .await
        .expect("submit")
        .job_id;
    let named = jobs
        .submit(
            spec("default", "named", member_session).with_origin_member_id(
                meerkat::OriginMemberId::new("worker-7").expect("member id"),
            ),
        )
        .await
        .expect("submit")
        .job_id;

    let origin_member = |job: Option<meerkat_jobs::StoredJob>| {
        job.expect("job")
            .spec
            .origin_member_id
            .map(|id| id.as_str().to_string())
    };
    assert_eq!(
        origin_member(store.get(&stamped).await.expect("read")),
        Some(member.0)
    );
    assert_eq!(origin_member(store.get(&plain).await.expect("read")), None);
    assert_eq!(
        origin_member(store.get(&named).await.expect("read")),
        Some("worker-7".to_string())
    );
}

/// Reports `current` as the current session of the member at `address`;
/// the test moves the member by replacing `current`.
#[cfg(feature = "session-store")]
struct RepointableMember {
    current: std::sync::Mutex<SessionId>,
    address: meerkat_runtime::LogicalRuntimeId,
}

#[cfg(feature = "session-store")]
#[async_trait::async_trait]
impl meerkat::ContinuationAddressResolver for RepointableMember {
    async fn current_address(
        &self,
        _owner: &ContinuationOwner,
    ) -> Result<Option<meerkat_runtime::LogicalRuntimeId>, String> {
        Ok(None)
    }

    async fn resolve_address(
        &self,
        _address: &meerkat_runtime::LogicalRuntimeId,
    ) -> Result<meerkat::AddressResolution, String> {
        Ok(meerkat::AddressResolution::NotServed)
    }

    async fn member_address_of_session(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<meerkat_runtime::LogicalRuntimeId>, String> {
        let current = self.current.lock().expect("current session lock").clone();
        Ok((session_id == &current).then(|| self.address.clone()))
    }
}

/// #1790: a replay through the producer job store keeps what its admitted
/// job recorded as its origin member, in either direction. A job admitted
/// without one (submitted before 0.9.3, through the host's own store, or
/// while the resolver could not tell) is joined by a replay from the
/// member's current session; a job admitted with one is joined by a replay
/// from a session the member has since left.
#[cfg(feature = "session-store")]
#[tokio::test]
async fn a_replay_through_the_producer_store_keeps_its_admitted_origin_member() {
    let bundle = meerkat::PersistenceBundle::new(
        Arc::new(meerkat_store::MemoryStore::new()) as Arc<dyn meerkat::SessionStore>,
        Arc::new(InMemoryRuntimeStore::new()) as Arc<dyn meerkat_runtime::RuntimeStore>,
        Arc::new(meerkat::MemoryBlobStore::new()),
    )
    .expect("persistence bundle");
    let member_session = SessionId::new();
    let member = meerkat::member_delivery_address("team", team_incarnation(), "worker", 1)
        .expect("member address");
    let resolver = Arc::new(RepointableMember {
        current: std::sync::Mutex::new(member_session.clone()),
        address: member.clone(),
    });
    let _binding = bundle
        .continuation_bindings()
        .bind(resolver.clone(), Arc::new(NoRetainedJobs))
        .expect("bind the host's resolver");
    let host_jobs = DetachedJobService::new(bundle.job_store());
    let producer_jobs = DetachedJobService::new(bundle.producer_job_store());
    let origin_member = |job: Option<meerkat_jobs::StoredJob>| {
        job.expect("job")
            .spec
            .origin_member_id
            .map(|id| id.as_str().to_string())
    };

    // Admitted without an origin member, then replayed from the member's
    // current session.
    let plain = spec("default", "admitted-plain", member_session.clone());
    let admitted = host_jobs.submit(plain.clone()).await.expect("submit");
    let replay = producer_jobs
        .submit(plain)
        .await
        .expect("the replay joins the job admitted without an origin member");
    assert_eq!(replay.job_id, admitted.job_id);
    assert!(replay.deduplicated);
    assert_eq!(
        origin_member(
            bundle
                .job_store()
                .get(&admitted.job_id)
                .await
                .expect("read")
        ),
        None
    );

    // Admitted with the member's address, then replayed after the member
    // moved to a successor session.
    let stamped = spec("default", "admitted-stamped", member_session.clone());
    let admitted = producer_jobs.submit(stamped.clone()).await.expect("submit");
    *resolver.current.lock().expect("current session lock") = SessionId::new();
    let replay = producer_jobs
        .submit(stamped)
        .await
        .expect("the replay joins the job admitted with an origin member");
    assert_eq!(replay.job_id, admitted.job_id);
    assert!(replay.deduplicated);
    assert_eq!(
        origin_member(
            bundle
                .job_store()
                .get(&admitted.job_id)
                .await
                .expect("read")
        ),
        Some(member.0)
    );
}

/// The incarnation of the mob `team` these tests address members of.
fn team_incarnation() -> meerkat_core::service::MobIncarnationId {
    "6a1d3c58-2f0e-4b7a-9c41-8e5d2b0f7a13"
        .parse()
        .expect("canonical mob incarnation id")
}
