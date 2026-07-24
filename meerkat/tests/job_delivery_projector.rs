#![allow(clippy::expect_used)]

use std::sync::Arc;

use meerkat::{
    AttemptClaim, CanonicalArgumentsHash, DetachedJobService, DetachedJobStore,
    InteractionLineageId, JobId, JobOutboxProjector, JobResultRef, JobSpec, JobSubmissionKey,
    JobTerminalDeliveryPayload, JobTerminalResult, MemoryDetachedJobStore, RestartClass,
    RunnerHandleRef, RunnerIdentity, SessionId, ToolIdentity, WorkerId,
};
use meerkat_runtime::{
    InMemoryRuntimeStore, LogicalRuntimeId, RuntimeDeliveryId, RuntimeDeliveryInbox,
};

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
