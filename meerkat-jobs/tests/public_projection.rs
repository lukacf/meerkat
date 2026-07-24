#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::Arc;

use meerkat_core::SessionId;
use meerkat_jobs::{
    AttemptClaim, AttemptWriteAuthority, CanonicalArgumentsHash, DetachedJobService,
    ExecutionIntentId, InteractionLineageId, JobResultRef, JobSpec, JobSubmissionKey,
    MemoryDetachedJobStore, RestartClass, RunnerHandleRef, RunnerIdentity, ToolIdentity, WorkerId,
};

fn spec(session_id: SessionId) -> JobSpec {
    JobSpec::new(
        "realm-a",
        session_id,
        ExecutionIntentId::from_string("intent-a").unwrap(),
        InteractionLineageId::from_string("lineage-a").unwrap(),
        ToolIdentity::new("scan", "1").unwrap(),
        RunnerIdentity::new("mobkit_callback", "1").unwrap(),
        RestartClass::NonResumable,
        CanonicalArgumentsHash::new("args-a").unwrap(),
        JobSubmissionKey::new("submission-a").unwrap(),
    )
}

#[tokio::test]
async fn public_description_exposes_status_without_write_authority() {
    let store = Arc::new(MemoryDetachedJobStore::new());
    let service = DetachedJobService::new(store);
    let session_id = SessionId::new();
    let receipt = service.submit(spec(session_id)).await.unwrap();
    service
        .claim_attempt(
            &receipt.job_id,
            AttemptClaim::new(
                WorkerId::new("worker-secret").unwrap(),
                10,
                100,
                RunnerHandleRef::new("runner-secret").unwrap(),
            ),
        )
        .await
        .unwrap();

    let description = service
        .describe(&receipt.job_id)
        .await
        .unwrap()
        .expect("description");
    let encoded = format!("{description:?}");
    assert_eq!(description.runner.name(), "mobkit_callback");
    assert_eq!(description.attempt_count, 1);
    assert!(!encoded.contains("worker-secret"));
    assert!(!encoded.contains("runner-secret"));
    assert!(!encoded.contains("fence"));
    assert!(!encoded.contains("attempt_id"));
}

#[tokio::test]
async fn public_list_is_scoped_to_realm_and_origin_session() {
    let store = Arc::new(MemoryDetachedJobStore::new());
    let service = DetachedJobService::new(store);
    let session_id = SessionId::new();
    let receipt = service.submit(spec(session_id.clone())).await.unwrap();

    let jobs = service
        .list_descriptions_for_origin("realm-a", &session_id, 10)
        .await
        .unwrap();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].job_id, receipt.job_id);
}

#[tokio::test]
async fn health_keeps_live_work_healthy_and_degrades_only_fault_conditions() {
    let store = Arc::new(MemoryDetachedJobStore::new());
    let service = DetachedJobService::new(store);
    let queued = service.submit(spec(SessionId::new())).await.unwrap();
    let mut running_spec = spec(SessionId::new());
    running_spec.submission_key = JobSubmissionKey::new("submission-running").unwrap();
    let running = service.submit(running_spec).await.unwrap();
    let claim = service
        .claim_attempt(
            &running.job_id,
            AttemptClaim::new(
                WorkerId::new("worker").unwrap(),
                10,
                100,
                RunnerHandleRef::new("runner").unwrap(),
            ),
        )
        .await
        .unwrap();

    let live = service.health_snapshot(99, 100).await.unwrap();
    assert_eq!(live.queued, 1);
    assert_eq!(live.running, 1);
    assert_eq!(live.stale_leases, 0);
    assert_eq!(live.delivery_backlog, 0);
    assert!(!live.is_degraded());

    service
        .complete_attempt(
            &running.job_id,
            AttemptWriteAuthority {
                attempt_id: claim.attempt_id,
                fence: claim.fence,
            },
            100,
            Some(JobResultRef::new("artifact:result").unwrap()),
        )
        .await
        .unwrap();
    let backlog = service.health_snapshot(101, 100).await.unwrap();
    assert_eq!(backlog.queued, 1);
    assert_eq!(backlog.running, 0);
    assert_eq!(backlog.delivery_backlog, 1);
    assert!(backlog.is_degraded());

    assert_eq!(queued.phase, meerkat_jobs::JobPhase::Queued);
}

#[tokio::test]
async fn health_detects_expired_lease_without_mutating_attempt_authority() {
    let store = Arc::new(MemoryDetachedJobStore::new());
    let service = DetachedJobService::new(store);
    let receipt = service.submit(spec(SessionId::new())).await.unwrap();
    let claim = service
        .claim_attempt(
            &receipt.job_id,
            AttemptClaim::new(
                WorkerId::new("worker").unwrap(),
                10,
                100,
                RunnerHandleRef::new("runner").unwrap(),
            ),
        )
        .await
        .unwrap();
    let before = service.get(&receipt.job_id).await.unwrap().unwrap();

    let health = service.health_snapshot(101, 100).await.unwrap();
    let after = service.get(&receipt.job_id).await.unwrap().unwrap();

    assert_eq!(health.stale_leases, 1);
    assert!(health.is_degraded());
    assert_eq!(after.current_attempt_id, Some(claim.attempt_id));
    assert_eq!(after.current_fence, claim.fence);
    assert_eq!(after.lease_expires_at_ms, before.lease_expires_at_ms);
    assert_eq!(after.revision, before.revision);
}
