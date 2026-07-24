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

fn spec(key: &str, session_id: SessionId) -> JobSpec {
    JobSpec::new(
        "default",
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

    let projected = projector.project_pending(10).await.expect("retry project");
    assert_eq!(projected.len(), 1);
    assert_eq!(projected[0].runtime_sequence, first.sequence);
    assert!(projected[0].runtime_deduplicated);

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
    let applied = applier.apply_pending(&runtime_id, 10).await.expect("apply");
    assert_eq!(applied.len(), 1);
    assert_eq!(applied[0].applications, 2);
    assert!(
        applier
            .apply_pending(&runtime_id, 10)
            .await
            .expect("idempotent retry")
            .is_empty()
    );

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
    let projected = projector.project_pending(10).await.expect("project");
    assert_eq!(projected.len(), 1);

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

    let projected = projector.project_pending(10).await.expect("retry project");
    assert_eq!(projected.len(), 1);
    assert_eq!(projected[0].runtime_sequence, first.sequence);
    assert!(projected[0].runtime_deduplicated);
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
