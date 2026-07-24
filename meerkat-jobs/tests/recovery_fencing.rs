#![allow(clippy::expect_used)]

use std::sync::Arc;

use meerkat_core::SessionId;
use meerkat_jobs::{
    AttemptClaim, AttemptWriteAuthority, CanonicalArgumentsHash, CheckpointRef, DetachedJobService,
    ExecutionIntentId, InteractionLineageId, JobProgress, JobSpec, JobSubmissionKey,
    MemoryDetachedJobStore, RestartClass, RunnerHandleRef, RunnerIdentity, ToolIdentity, WorkerId,
};

fn spec(key: &str, restart_class: RestartClass) -> JobSpec {
    JobSpec::new(
        "realm-a",
        SessionId::new(),
        ExecutionIntentId::new(),
        InteractionLineageId::new(),
        ToolIdentity::new("security_scan", "v1").expect("valid tool identity"),
        RunnerIdentity::new("homecore.security_scan", "v1").expect("valid runner identity"),
        restart_class,
        CanonicalArgumentsHash::new("sha256:scan-a").expect("valid arguments hash"),
        JobSubmissionKey::new(key).expect("valid submission key"),
    )
}

#[tokio::test]
async fn reopen_rehydrates_attempt_fence_lease_checkpoint_and_handle_without_advancing() {
    let store = Arc::new(MemoryDetachedJobStore::new());
    let service = DetachedJobService::new(store.clone());
    let receipt = service
        .submit(spec("reopen-same-authority", RestartClass::Adoptable))
        .await
        .expect("submit");
    let claim = service
        .claim_attempt(
            &receipt.job_id,
            AttemptClaim::new(
                WorkerId::new("worker-a").expect("worker id"),
                100,
                10_000,
                RunnerHandleRef::new("external:scan-42").expect("runner handle"),
            ),
        )
        .await
        .expect("claim");
    service
        .record_checkpoint(
            &receipt.job_id,
            AttemptWriteAuthority::from(&claim),
            CheckpointRef::new("checkpoint:7").expect("checkpoint ref"),
            9_000,
        )
        .await
        .expect("checkpoint");

    let persisted = store.snapshot().await;
    let reopened_store =
        Arc::new(MemoryDetachedJobStore::from_snapshot(persisted).expect("reopen store"));
    let reopened = DetachedJobService::new(reopened_store.clone());
    let recovered = reopened
        .get(&receipt.job_id)
        .await
        .expect("get")
        .expect("job exists");

    assert_eq!(recovered.attempt_count, claim.attempt_count);
    assert_eq!(
        recovered.current_attempt_id.as_ref(),
        Some(&claim.attempt_id)
    );
    assert_eq!(recovered.current_fence, claim.fence);
    assert_eq!(recovered.lease_expires_at_ms, Some(10_000));
    assert_eq!(
        recovered.checkpoint_ref.as_ref().map(CheckpointRef::as_str),
        Some("checkpoint:7")
    );
    assert_eq!(
        recovered
            .runner_handle
            .as_ref()
            .map(RunnerHandleRef::as_str),
        Some("external:scan-42")
    );

    let stored_after_reopen = reopened_store
        .get(&receipt.job_id)
        .await
        .expect("store read")
        .expect("job exists");
    assert_eq!(
        stored_after_reopen.revision, recovered.revision,
        "read/reconstruction alone must not commit a new revision"
    );

    reopened
        .report_progress(
            &receipt.job_id,
            AttemptWriteAuthority::from(&claim),
            JobProgress::new(1, "reconnected").expect("progress"),
            9_000,
        )
        .await
        .expect("the latest committed writer remains valid after reopen");
}

#[tokio::test]
async fn older_writer_is_fenced_only_after_a_later_claim_commits() {
    let store = Arc::new(MemoryDetachedJobStore::new());
    let service = DetachedJobService::new(store.clone());
    let receipt = service
        .submit(spec("later-claim-fences", RestartClass::Replayable))
        .await
        .expect("submit");
    let first = service
        .claim_attempt(
            &receipt.job_id,
            AttemptClaim::new(
                WorkerId::new("worker-a").expect("worker"),
                100,
                1_000,
                RunnerHandleRef::new("checkpoint-runner:first").expect("handle"),
            ),
        )
        .await
        .expect("first claim");

    let reopened = DetachedJobService::new(Arc::new(
        MemoryDetachedJobStore::from_snapshot(store.snapshot().await).expect("reopen"),
    ));
    reopened
        .report_progress(
            &receipt.job_id,
            AttemptWriteAuthority::from(&first),
            JobProgress::new(1, "still current").expect("progress"),
            900,
        )
        .await
        .expect("reopen alone must not fence the committed writer");

    reopened
        .observe_lease_expired(&receipt.job_id, AttemptWriteAuthority::from(&first), 1_001)
        .await
        .expect("lease expiry");
    reopened
        .schedule_retry(&receipt.job_id, 1_100)
        .await
        .expect("machine-authorized retry schedule");
    let second = reopened
        .claim_attempt(
            &receipt.job_id,
            AttemptClaim::new(
                WorkerId::new("worker-b").expect("worker"),
                1_100,
                2_000,
                RunnerHandleRef::new("checkpoint-runner:second").expect("handle"),
            ),
        )
        .await
        .expect("later claim");

    assert_eq!(second.attempt_count, first.attempt_count + 1);
    assert!(second.fence > first.fence);
    let error = reopened
        .report_progress(
            &receipt.job_id,
            AttemptWriteAuthority::from(&first),
            JobProgress::new(2, "stale").expect("progress"),
            1_200,
        )
        .await
        .expect_err("the genuinely older writer must now be fenced");
    assert!(error.is_stale_attempt());

    reopened
        .report_progress(
            &receipt.job_id,
            AttemptWriteAuthority::from(&second),
            JobProgress::new(2, "current").expect("progress"),
            1_200,
        )
        .await
        .expect("new claim owns writes");
}

#[tokio::test]
async fn reopened_writer_can_renew_same_fence_until_expiry_is_committed() {
    let store = Arc::new(MemoryDetachedJobStore::new());
    let service = DetachedJobService::new(store.clone());
    let receipt = service
        .submit(spec("reopen-renew-same-fence", RestartClass::Adoptable))
        .await
        .expect("submit");
    let claim = service
        .claim_attempt(
            &receipt.job_id,
            AttemptClaim::new(
                WorkerId::new("worker-a").expect("worker"),
                100,
                1_000,
                RunnerHandleRef::new("external:scan-42").expect("handle"),
            ),
        )
        .await
        .expect("claim");
    let reopened = DetachedJobService::new(Arc::new(
        MemoryDetachedJobStore::from_snapshot(store.snapshot().await).expect("reopen"),
    ));

    let renewed = reopened
        .renew_lease(
            &receipt.job_id,
            AttemptWriteAuthority::from(&claim),
            1_500,
            2_500,
        )
        .await
        .expect("reconstruction must not make the exact committed writer unrenewable");

    assert_eq!(renewed.attempt_count, claim.attempt_count);
    assert_eq!(renewed.current_attempt_id, Some(claim.attempt_id));
    assert_eq!(renewed.current_fence, claim.fence);
    assert_eq!(renewed.lease_expires_at_ms, Some(2_500));
}

#[tokio::test]
async fn checkpoint_survives_retry_scheduling_and_is_available_to_the_new_attempt() {
    let store = Arc::new(MemoryDetachedJobStore::new());
    let service = DetachedJobService::new(store);
    let receipt = service
        .submit(spec("checkpoint-retry", RestartClass::CheckpointResumable))
        .await
        .expect("submit");
    let first = service
        .claim_attempt(
            &receipt.job_id,
            AttemptClaim::new(
                WorkerId::new("worker-a").expect("worker"),
                100,
                500,
                RunnerHandleRef::new("runner:first").expect("handle"),
            ),
        )
        .await
        .expect("claim");
    service
        .record_checkpoint(
            &receipt.job_id,
            AttemptWriteAuthority::from(&first),
            CheckpointRef::new("checkpoint:committed").expect("checkpoint"),
            400,
        )
        .await
        .expect("checkpoint");
    service
        .observe_lease_expired(&receipt.job_id, AttemptWriteAuthority::from(&first), 501)
        .await
        .expect("lease expiry");
    service
        .schedule_retry(&receipt.job_id, 600)
        .await
        .expect("retry schedule");
    let second = service
        .claim_attempt(
            &receipt.job_id,
            AttemptClaim::new(
                WorkerId::new("worker-b").expect("worker"),
                600,
                1_000,
                RunnerHandleRef::new("runner:second").expect("handle"),
            ),
        )
        .await
        .expect("second claim");

    assert_eq!(
        second.resume_checkpoint.as_ref().map(CheckpointRef::as_str),
        Some("checkpoint:committed")
    );
}

#[tokio::test]
async fn lease_progress_and_checkpoint_writes_do_not_collapse_external_wait_state() {
    let store = Arc::new(MemoryDetachedJobStore::new());
    let service = DetachedJobService::new(store);
    let receipt = service
        .submit(spec("external-wait-state", RestartClass::Adoptable))
        .await
        .expect("submit");
    let claim = service
        .claim_attempt(
            &receipt.job_id,
            AttemptClaim::new(
                WorkerId::new("worker-a").expect("worker"),
                100,
                1_000,
                RunnerHandleRef::new("external:scan-1").expect("handle"),
            ),
        )
        .await
        .expect("claim");
    let write = AttemptWriteAuthority::from(&claim);

    service
        .wait_external(&receipt.job_id, write.clone(), 800)
        .await
        .expect("wait external");
    service
        .renew_lease(&receipt.job_id, write.clone(), 900, 2_000)
        .await
        .expect("renew");
    service
        .report_progress(
            &receipt.job_id,
            write.clone(),
            JobProgress::new(1, "waiting").expect("progress"),
            950,
        )
        .await
        .expect("progress");
    let waiting = service
        .record_checkpoint(
            &receipt.job_id,
            write.clone(),
            CheckpointRef::new("checkpoint:waiting").expect("checkpoint"),
            950,
        )
        .await
        .expect("checkpoint");
    assert_eq!(waiting.phase, meerkat_jobs::JobPhase::WaitingExternal);

    let running = service
        .resume_running(&receipt.job_id, write, 960)
        .await
        .expect("resume");
    assert_eq!(running.phase, meerkat_jobs::JobPhase::Running);
}
