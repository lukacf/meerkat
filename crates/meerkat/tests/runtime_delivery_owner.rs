#![allow(clippy::expect_used)]
//! The library delivery owner applies durable job deliveries on typed wakes
//! only: a reconcile pass at arming, then job outbox commits, runtime delivery
//! commits, and attachment commits. No pass ever runs on a timer.

use std::sync::Arc;
use std::time::Duration;

use meerkat::{
    AttemptClaim, CanonicalArgumentsHash, DetachedJobService, InteractionLineageId,
    JobDeliveryApplication, JobDeliverySink, JobId, JobResultRef, JobSpec, JobSubmissionKey,
    MemoryDetachedJobStore, RestartClass, RunnerHandleRef, RunnerIdentity, RuntimeDeliveryHost,
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
    applied: Mutex<Vec<JobId>>,
    commit_during_first_apply: Mutex<Option<(DetachedJobService, SessionId)>>,
}

impl RecordingSink {
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
    async fn apply(&self, application: JobDeliveryApplication) -> Result<(), String> {
        let job_id = match &application {
            JobDeliveryApplication::Record { job_id, .. }
            | JobDeliveryApplication::Notification { job_id, .. }
            | JobDeliveryApplication::Event { job_id, .. } => job_id.clone(),
        };
        if self.poisoned.lock().expect("poison lock").as_ref() == Some(&job_id) {
            return Err(format!("sink rejects deliveries for job {job_id}"));
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
    async fn delivery_sink(&self, _session_id: &SessionId) -> Option<Arc<dyn JobDeliverySink>> {
        Some(self.sink.clone())
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
    tokio::time::advance(Duration::from_secs(3600)).await;
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
