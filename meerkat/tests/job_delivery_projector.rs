#![allow(clippy::expect_used)]

use std::sync::Arc;

use meerkat::{
    AttemptClaim, CanonicalArgumentsHash, DetachedJobService, DetachedJobStore,
    InteractionLineageId, JobDeliveryApplication, JobDeliveryKind, JobDeliverySink, JobId,
    JobNotification, JobNotificationDeliveryPayload, JobOutboxProjector, JobResultRef,
    JobRuntimeDeliveryApplier, JobSpec, JobSubmissionKey, JobSubscription, JobSubscriptionId,
    JobTerminalDeliveryPayload, JobTerminalResult, MemoryDetachedJobStore, RestartClass,
    RunnerHandleRef, RunnerIdentity, SessionId, ToolIdentity, WorkerId,
};
use meerkat_core::HandlingMode;
use meerkat_runtime::{
    InMemoryRuntimeStore, LogicalRuntimeId, RuntimeDeliveryId, RuntimeDeliveryInbox,
};
use meerkat_tools::builtin::shell::ShellJobDeliveryProjector;
use tokio::sync::Mutex;

#[derive(Default)]
struct RecordingDeliverySink {
    applications: Mutex<Vec<JobDeliveryApplication>>,
}

#[async_trait::async_trait]
impl JobDeliverySink for RecordingDeliverySink {
    async fn apply(&self, application: JobDeliveryApplication) -> Result<(), String> {
        self.applications.lock().await.push(application);
        Ok(())
    }
}

/// Rejects every delivery for one designated job until healed; records the
/// job ids of accepted deliveries in application order.
#[derive(Default)]
struct SelectivePoisonSink {
    poisoned: std::sync::Mutex<Option<JobId>>,
    applications: Mutex<Vec<JobId>>,
}

impl SelectivePoisonSink {
    fn poison(&self, job_id: JobId) {
        *self.poisoned.lock().expect("poison lock") = Some(job_id);
    }

    fn heal(&self) {
        *self.poisoned.lock().expect("poison lock") = None;
    }
}

#[async_trait::async_trait]
impl JobDeliverySink for SelectivePoisonSink {
    async fn apply(&self, application: JobDeliveryApplication) -> Result<(), String> {
        let job_id = match &application {
            JobDeliveryApplication::Record { job_id, .. }
            | JobDeliveryApplication::Notification { job_id, .. }
            | JobDeliveryApplication::Event { job_id, .. } => job_id.clone(),
        };
        if self.poisoned.lock().expect("poison lock").as_ref() == Some(&job_id) {
            return Err(format!("sink rejects deliveries for job {job_id}"));
        }
        self.applications.lock().await.push(job_id);
        Ok(())
    }
}

fn spec(key: &str, session_id: SessionId) -> JobSpec {
    spec_for_realm("default", key, session_id)
}

fn spec_for_realm(realm_id: &str, key: &str, session_id: SessionId) -> JobSpec {
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

#[tokio::test]
async fn realm_scoped_projection_leaves_other_realm_outbox_pending() {
    let job_store = Arc::new(MemoryDetachedJobStore::new());
    let jobs = DetachedJobService::new(job_store.clone());
    let session_id = SessionId::new();
    let local = completed_job(&jobs, session_id.clone(), "local").await;

    let foreign_receipt = jobs
        .submit(spec_for_realm("other", "foreign", session_id.clone()))
        .await
        .expect("submit foreign job");
    let foreign_claim = jobs
        .claim_attempt(
            &foreign_receipt.job_id,
            AttemptClaim::new(
                WorkerId::new("worker-other").expect("worker"),
                1,
                100,
                RunnerHandleRef::new("runner-other").expect("handle"),
            ),
        )
        .await
        .expect("claim foreign job");
    jobs.complete_attempt(
        &foreign_receipt.job_id,
        (&foreign_claim).into(),
        2,
        Some(JobResultRef::new("foreign-result").expect("result")),
    )
    .await
    .expect("complete foreign job");

    let inbox = RuntimeDeliveryInbox::new(Arc::new(InMemoryRuntimeStore::new()));
    let projector = JobOutboxProjector::new_for_realm(job_store.clone(), inbox.clone(), "default");
    let pass = projector
        .project_pending(10)
        .await
        .expect("project local realm");

    assert!(pass.is_fully_projected());
    assert_eq!(pass.projected.len(), 1);
    assert_eq!(pass.projected[0].job_id, local);
    assert!(
        jobs.get(&foreign_receipt.job_id)
            .await
            .expect("load foreign")
            .expect("foreign job")
            .outbox
            .iter()
            .any(|entry| !entry.applied)
    );
}

async fn completed_job(jobs: &DetachedJobService, session_id: SessionId, key: &str) -> JobId {
    let receipt = jobs.submit(spec(key, session_id)).await.expect("submit");
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

async fn notified_job(jobs: &DetachedJobService, session_id: SessionId, key: &str) -> JobId {
    let receipt = jobs.submit(spec(key, session_id)).await.expect("submit");
    let claim = jobs
        .claim_attempt(
            &receipt.job_id,
            AttemptClaim::new(
                WorkerId::new("monitor-worker").expect("worker"),
                1,
                100,
                RunnerHandleRef::new("monitor-handle").expect("handle"),
            ),
        )
        .await
        .expect("claim");
    jobs.emit_notification(
        &receipt.job_id,
        (&claim).into(),
        2,
        JobNotification::new(
            "notification-1",
            "monitor:release:v1",
            "Release observed",
            "Meerkat v1 is available.",
        )
        .expect("notification"),
    )
    .await
    .expect("emit notification");
    receipt.job_id
}

#[tokio::test]
async fn nonterminal_notification_is_durable_and_replays_after_runtime_insert_crash() {
    let job_store = Arc::new(MemoryDetachedJobStore::new());
    let jobs = DetachedJobService::new(job_store.clone());
    let session_id = SessionId::new();
    let job_id = notified_job(&jobs, session_id.clone(), "notification-replay").await;
    let pending = job_store
        .list_pending_outbox(10)
        .await
        .expect("job outbox")
        .pop()
        .expect("notification delivery");

    let inbox = RuntimeDeliveryInbox::new(Arc::new(InMemoryRuntimeStore::new()));
    let projector = JobOutboxProjector::new(job_store.clone(), inbox.clone());
    let prepared = projector
        .prepare(&pending)
        .await
        .expect("prepare notification");
    assert_eq!(
        prepared.submission.kind(),
        meerkat_runtime::RuntimeDeliveryKind::JobNotification
    );
    assert_eq!(
        prepared.submission.delivery_id(),
        &RuntimeDeliveryId::new(format!("{job_id}:notification:notification-1"))
            .expect("delivery id")
    );
    let first = inbox
        .submit(&prepared.runtime_id, prepared.submission.clone())
        .await
        .expect("runtime commit");

    let pass = projector.project_pending(10).await.expect("retry project");
    assert!(pass.is_fully_projected());
    assert_eq!(pass.projected.len(), 1);
    assert_eq!(pass.projected[0].runtime_sequence, first.sequence);
    assert!(pass.projected[0].runtime_deduplicated);

    let job = jobs.get(&job_id).await.expect("load").expect("job");
    assert!(job.terminal_result.is_none());
    assert!(job.outbox[0].applied);
    let runtime_id = LogicalRuntimeId::for_session(&session_id);
    let runtime_entries = inbox
        .list_pending(&runtime_id, 10)
        .await
        .expect("runtime inbox");
    assert_eq!(runtime_entries.len(), 1);
    let payload: JobNotificationDeliveryPayload =
        serde_json::from_slice(runtime_entries[0].submission.payload()).expect("typed payload");
    assert_eq!(payload.job_id, job_id);
    assert_eq!(payload.delivery_sequence, 1);
    assert_eq!(payload.notification.title(), "Release observed");
    assert_eq!(payload.notification.body(), "Meerkat v1 is available.");
}

#[tokio::test]
async fn subscription_application_keeps_notifications_turn_free_and_events_canonical() {
    let job_store = Arc::new(MemoryDetachedJobStore::new());
    let jobs = DetachedJobService::new(job_store.clone());
    let origin = SessionId::new();
    let event_session = SessionId::new();
    let receipt = jobs
        .submit(spec("subscription-application", origin.clone()))
        .await
        .expect("submit");
    jobs.subscribe(
        &receipt.job_id,
        JobSubscription::new(
            JobSubscriptionId::new("notify-origin").expect("id"),
            origin.clone(),
            JobDeliveryKind::Notification,
        ),
    )
    .await
    .expect("subscribe notification");
    jobs.subscribe(
        &receipt.job_id,
        JobSubscription::new(
            JobSubscriptionId::new("event-peer").expect("id"),
            event_session.clone(),
            JobDeliveryKind::Event {
                handling_mode: HandlingMode::Queue,
            },
        ),
    )
    .await
    .expect("subscribe event");
    let claim = jobs
        .claim_attempt(
            &receipt.job_id,
            AttemptClaim::new(
                WorkerId::new("monitor-worker").expect("worker"),
                1,
                100,
                RunnerHandleRef::new("monitor-handle").expect("handle"),
            ),
        )
        .await
        .expect("claim");
    jobs.emit_notification(
        &receipt.job_id,
        (&claim).into(),
        2,
        JobNotification::new("n1", "condition:1", "Condition met", "Review me")
            .expect("notification"),
    )
    .await
    .expect("emit");

    let inbox = RuntimeDeliveryInbox::new(Arc::new(InMemoryRuntimeStore::new()));
    let projector = JobOutboxProjector::new(job_store, inbox.clone());
    projector.project_pending(10).await.expect("project");
    let sink = Arc::new(RecordingDeliverySink::default());
    let applier = JobRuntimeDeliveryApplier::new(inbox.clone(), sink.clone());
    let runtime_id = LogicalRuntimeId::for_session(&origin);
    let drain = applier.apply_pending(&runtime_id, 10).await.expect("apply");
    assert!(drain.is_fully_drained());
    assert_eq!(drain.applied.len(), 1);
    assert_eq!(drain.applied[0].applications, 2);
    let retry = applier
        .apply_pending(&runtime_id, 10)
        .await
        .expect("idempotent retry");
    assert!(retry.is_fully_drained());
    assert!(retry.applied.is_empty());

    let applications = sink.applications.lock().await.clone();
    assert!(matches!(
        &applications[0],
        JobDeliveryApplication::Notification { subscription, .. }
            if subscription.session_id() == &origin
    ));
    assert!(matches!(
        &applications[1],
        JobDeliveryApplication::Event {
            subscription,
            handling_mode: HandlingMode::Queue,
            ..
        } if subscription.session_id() == &event_session
    ));
}

#[tokio::test]
async fn crash_before_runtime_insert_leaves_job_delivery_retryable() {
    let job_store = Arc::new(MemoryDetachedJobStore::new());
    let jobs = DetachedJobService::new(job_store.clone());
    let session_id = SessionId::new();
    let job_id = completed_job(&jobs, session_id.clone(), "before-insert").await;
    let before = jobs.get(&job_id).await.expect("load").expect("job exists");

    assert_eq!(
        job_store
            .list_pending_outbox(10)
            .await
            .expect("job outbox")
            .len(),
        1
    );

    let inbox = RuntimeDeliveryInbox::new(Arc::new(InMemoryRuntimeStore::new()));
    let projector = JobOutboxProjector::new(job_store.clone(), inbox.clone());
    let pass = projector.project_pending(10).await.expect("project");
    assert!(pass.is_fully_projected());
    assert_eq!(pass.projected.len(), 1);

    let job = jobs.get(&job_id).await.expect("load").expect("job exists");
    assert!(job.outbox[0].applied);
    assert_eq!(job.attempt_count, before.attempt_count);
    assert_eq!(job.current_fence, before.current_fence);
    assert_eq!(job.current_attempt_id, before.current_attempt_id);
    let runtime_id = LogicalRuntimeId::for_session(&session_id);
    let deliveries = inbox
        .list_pending(&runtime_id, 10)
        .await
        .expect("runtime inbox");
    assert_eq!(deliveries.len(), 1);
    let payload: JobTerminalDeliveryPayload =
        serde_json::from_slice(deliveries[0].submission.payload()).expect("typed payload");
    assert_eq!(payload.job_id, job_id);
    assert_eq!(payload.delivery_sequence, 1);
    assert_eq!(
        payload.terminal_result,
        JobTerminalResult::Succeeded {
            result_ref: Some(JobResultRef::new("result").expect("result"))
        }
    );
}

#[tokio::test]
async fn crash_after_runtime_insert_before_job_ack_reuses_the_same_delivery_and_feed_sequence() {
    let job_store = Arc::new(MemoryDetachedJobStore::new());
    let jobs = DetachedJobService::new(job_store.clone());
    let session_id = SessionId::new();
    let job_id = completed_job(&jobs, session_id.clone(), "after-insert").await;
    let pending = job_store
        .list_pending_outbox(10)
        .await
        .expect("job outbox")
        .pop()
        .expect("pending delivery");

    let inbox = RuntimeDeliveryInbox::new(Arc::new(InMemoryRuntimeStore::new()));
    let projector = JobOutboxProjector::new(job_store.clone(), inbox.clone());
    let prepared = projector.prepare(&pending).await.expect("prepare delivery");
    let first = inbox
        .submit(&prepared.runtime_id, prepared.submission.clone())
        .await
        .expect("runtime commit");

    // Simulated crash: no job-outbox acknowledgement happened.
    assert_eq!(
        job_store
            .list_pending_outbox(10)
            .await
            .expect("still pending")
            .len(),
        1
    );

    let pass = projector.project_pending(10).await.expect("retry project");
    assert!(pass.is_fully_projected());
    assert_eq!(pass.projected.len(), 1);
    assert_eq!(pass.projected[0].runtime_sequence, first.sequence);
    assert!(pass.projected[0].runtime_deduplicated);
    assert!(
        job_store
            .list_pending_outbox(10)
            .await
            .expect("acknowledged")
            .is_empty()
    );

    let runtime_id = LogicalRuntimeId::for_session(&session_id);
    let runtime_entries = inbox
        .list_pending(&runtime_id, 10)
        .await
        .expect("runtime inbox");
    assert_eq!(runtime_entries.len(), 1);
    assert_eq!(runtime_entries[0].sequence, first.sequence);
    assert_eq!(
        runtime_entries[0].submission.delivery_id(),
        &RuntimeDeliveryId::new(job_id.as_str()).expect("delivery id")
    );
}

#[tokio::test]
async fn runtime_cursor_advances_only_after_completion_feed_projection_acknowledgement() {
    let job_store = Arc::new(MemoryDetachedJobStore::new());
    let jobs = DetachedJobService::new(job_store.clone());
    let session_id = SessionId::new();
    let job_id = completed_job(&jobs, session_id.clone(), "agent-applied").await;
    let inbox = RuntimeDeliveryInbox::new(Arc::new(InMemoryRuntimeStore::new()));
    let projector = JobOutboxProjector::new(job_store, inbox.clone());

    projector.project_pending(10).await.expect("runtime commit");
    let runtime_id = LogicalRuntimeId::for_session(&session_id);
    assert_eq!(
        inbox
            .list_pending(&runtime_id, 10)
            .await
            .expect("pending")
            .len(),
        1
    );

    projector
        .acknowledge_applied(job_id.as_str())
        .await
        .expect("completion-feed projection acknowledgement");
    projector
        .acknowledge_applied(job_id.as_str())
        .await
        .expect("idempotent repeated acknowledgement");
    assert!(
        inbox
            .list_pending(&runtime_id, 10)
            .await
            .expect("pending")
            .is_empty()
    );
}

#[tokio::test]
async fn shell_projection_targets_requested_job_beyond_a_global_batch_boundary() {
    let job_store = Arc::new(MemoryDetachedJobStore::new());
    let jobs = DetachedJobService::new(job_store.clone());
    let session_id = SessionId::new();
    let mut job_ids = Vec::new();
    for index in 0..300 {
        job_ids.push(completed_job(&jobs, session_id.clone(), &format!("batch-{index:03}")).await);
    }
    let target = job_ids.last().expect("target").clone();
    let inbox = RuntimeDeliveryInbox::new(Arc::new(InMemoryRuntimeStore::new()));
    let projector = JobOutboxProjector::new(job_store.clone(), inbox.clone());

    ShellJobDeliveryProjector::project_job(&projector, target.as_str())
        .await
        .expect("project exact job");

    let target_job = jobs.get(&target).await.expect("load").expect("target job");
    assert!(target_job.outbox[0].applied);
    let first_job = jobs
        .get(job_ids.first().expect("first"))
        .await
        .expect("load")
        .expect("first job");
    assert!(
        !first_job.outbox[0].applied,
        "exact projection must not claim success after projecting unrelated jobs"
    );
    let runtime_id = LogicalRuntimeId::for_session(&session_id);
    let runtime_entries = inbox
        .list_pending(&runtime_id, 10)
        .await
        .expect("runtime inbox");
    assert_eq!(runtime_entries.len(), 1);
    assert_eq!(
        runtime_entries[0].submission.delivery_id(),
        &RuntimeDeliveryId::new(target.as_str()).expect("delivery id")
    );
}

/// Regression: a delivery the sink rejects must fail SAFE — rows ahead of it
/// apply and acknowledge, the failure is reported as a blocked row instead
/// of aborting the drain, the row itself is never discarded, and it applies
/// once the cause heals. Before this contract, the first poisoned row turned
/// every drain pass into an error and head-of-line blocked the delivery
/// queue forever.
#[tokio::test]
async fn poisoned_delivery_blocks_only_its_row_and_applies_after_heal() {
    let job_store = Arc::new(MemoryDetachedJobStore::new());
    let jobs = DetachedJobService::new(job_store.clone());
    let session_id = SessionId::new();
    completed_job(&jobs, session_id.clone(), "drain-a").await;
    completed_job(&jobs, session_id.clone(), "drain-b").await;

    let inbox = RuntimeDeliveryInbox::new(Arc::new(InMemoryRuntimeStore::new()));
    let projector = JobOutboxProjector::new(job_store, inbox.clone());
    let pass = projector.project_pending(10).await.expect("project");
    assert_eq!(pass.projected.len(), 2);

    let runtime_id = LogicalRuntimeId::for_session(&session_id);
    let pending = inbox.list_pending(&runtime_id, 10).await.expect("pending");
    assert_eq!(pending.len(), 2);
    let first = JobId::new(pending[0].submission.delivery_id().as_str()).expect("first job id");
    let second = JobId::new(pending[1].submission.delivery_id().as_str()).expect("second job id");

    let sink = Arc::new(SelectivePoisonSink::default());
    sink.poison(second.clone());
    let applier = JobRuntimeDeliveryApplier::new(inbox.clone(), sink.clone());

    let drain = applier
        .apply_pending(&runtime_id, 10)
        .await
        .expect("a poisoned row is a blocked drain, not a drain error");
    assert_eq!(
        drain.applied.len(),
        1,
        "rows ahead of the poison must apply"
    );
    assert_eq!(drain.applied[0].delivery_id.as_str(), first.as_str());
    let blocked = drain.blocked.expect("the poisoned row must be reported");
    assert_eq!(blocked.delivery_id.as_str(), second.as_str());
    assert!(blocked.error.contains("sink rejects deliveries"));

    // The blocked row is retained, not discarded: still pending, still
    // reported on the next pass.
    let retry = applier.apply_pending(&runtime_id, 10).await.expect("retry");
    assert!(retry.applied.is_empty());
    assert!(retry.blocked.is_some());
    assert_eq!(
        inbox
            .list_pending(&runtime_id, 10)
            .await
            .expect("still pending")
            .len(),
        1
    );

    // Once the cause heals, the same delivery applies with nothing lost.
    sink.heal();
    let healed = applier
        .apply_pending(&runtime_id, 10)
        .await
        .expect("healed drain");
    assert!(healed.is_fully_drained());
    assert_eq!(healed.applied.len(), 1);
    assert_eq!(healed.applied[0].delivery_id.as_str(), second.as_str());
    assert_eq!(*sink.applications.lock().await, vec![first, second]);
    assert!(
        inbox
            .list_pending(&runtime_id, 10)
            .await
            .expect("drained")
            .is_empty()
    );
}

/// Delegates to the inner store but hides one job from `get`, simulating an
/// outbox row whose job can no longer be resolved (the unmappable-row class).
#[derive(Debug)]
struct HidingJobStore {
    inner: Arc<MemoryDetachedJobStore>,
    hidden: std::sync::Mutex<Option<JobId>>,
}

#[async_trait::async_trait]
impl DetachedJobStore for HidingJobStore {
    async fn insert_deduplicated(
        &self,
        job: meerkat_jobs::StoredJob,
    ) -> Result<meerkat_jobs::InsertJobOutcome, meerkat::DetachedJobError> {
        self.inner.insert_deduplicated(job).await
    }

    async fn get(
        &self,
        job_id: &JobId,
    ) -> Result<Option<meerkat_jobs::StoredJob>, meerkat::DetachedJobError> {
        if self.hidden.lock().expect("hidden lock").as_ref() == Some(job_id) {
            return Ok(None);
        }
        self.inner.get(job_id).await
    }

    async fn compare_and_swap(
        &self,
        expected_revision: u64,
        replacement: meerkat_jobs::StoredJob,
    ) -> Result<meerkat_jobs::StoredJob, meerkat::DetachedJobError> {
        self.inner
            .compare_and_swap(expected_revision, replacement)
            .await
    }

    async fn list_pending_outbox(
        &self,
        limit: usize,
    ) -> Result<Vec<meerkat::JobOutboxEntry>, meerkat::DetachedJobError> {
        self.inner.list_pending_outbox(limit).await
    }

    async fn list_for_origin(
        &self,
        realm_id: &str,
        origin_session_id: &SessionId,
        limit: usize,
    ) -> Result<Vec<meerkat_jobs::StoredJob>, meerkat::DetachedJobError> {
        self.inner
            .list_for_origin(realm_id, origin_session_id, limit)
            .await
    }

    async fn list_all(
        &self,
        limit: usize,
    ) -> Result<Vec<meerkat_jobs::StoredJob>, meerkat::DetachedJobError> {
        self.inner.list_all(limit).await
    }

    fn is_persistent(&self) -> bool {
        self.inner.is_persistent()
    }
}

/// Regression: an outbox row that cannot be mapped to its job must not abort
/// the projection pass. Other jobs keep projecting, the poisoned row is
/// reported and stays pending (never acknowledged), and it projects once the
/// store heals.
#[tokio::test]
async fn unmappable_outbox_row_is_skipped_and_other_jobs_keep_projecting() {
    let memory = Arc::new(MemoryDetachedJobStore::new());
    let jobs = DetachedJobService::new(memory.clone());
    let session_id = SessionId::new();
    let vanished = completed_job(&jobs, session_id.clone(), "vanished").await;
    let healthy = completed_job(&jobs, session_id.clone(), "healthy").await;

    let store = Arc::new(HidingJobStore {
        inner: memory,
        hidden: std::sync::Mutex::new(Some(vanished.clone())),
    });
    let inbox = RuntimeDeliveryInbox::new(Arc::new(InMemoryRuntimeStore::new()));
    let projector = JobOutboxProjector::new_for_realm(store.clone(), inbox, "default");

    let pass = projector
        .project_pending(10)
        .await
        .expect("a poisoned row must not abort the projection pass");
    assert_eq!(pass.projected.len(), 1, "other jobs must keep projecting");
    assert_eq!(pass.projected[0].job_id, healthy);
    assert_eq!(pass.skipped.len(), 1);
    assert_eq!(pass.skipped[0].job_id, vanished);
    assert!(pass.skipped[0].error.contains("missing job"));

    // The skipped row was never acknowledged; it projects once the store
    // resolves the job again.
    *store.hidden.lock().expect("hidden lock") = None;
    let healed = projector.project_pending(10).await.expect("healed pass");
    assert!(healed.is_fully_projected());
    assert_eq!(healed.projected.len(), 1);
    assert_eq!(healed.projected[0].job_id, vanished);
}
