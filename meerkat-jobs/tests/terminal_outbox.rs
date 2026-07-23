#![allow(clippy::expect_used)]

use std::sync::Arc;

use meerkat_core::SessionId;
use meerkat_jobs::{
    AttemptClaim, AttemptWriteAuthority, CanonicalArgumentsHash, DetachedJobService,
    ExecutionIntentId, InteractionLineageId, JobFailureCode, JobResultRef, JobSpec,
    JobSubmissionKey, JobTerminalKind, JobTerminalResult, MemoryDetachedJobStore, RestartClass,
    RunnerHandleRef, RunnerIdentity, ToolIdentity, WorkerId,
};

fn spec(key: &str, restart_class: RestartClass) -> JobSpec {
    JobSpec::new(
        "realm-a",
        SessionId::new(),
        ExecutionIntentId::new(),
        InteractionLineageId::new(),
        ToolIdentity::new("scan", "v1").expect("tool"),
        RunnerIdentity::new("runner.scan", "v1").expect("runner"),
        restart_class,
        CanonicalArgumentsHash::new("sha256:args").expect("hash"),
        JobSubmissionKey::new(key).expect("key"),
    )
}

async fn running_job(
    key: &str,
    restart_class: RestartClass,
) -> (
    Arc<MemoryDetachedJobStore>,
    DetachedJobService,
    meerkat_jobs::JobId,
    meerkat_jobs::AttemptClaimReceipt,
) {
    let store = Arc::new(MemoryDetachedJobStore::new());
    let service = DetachedJobService::new(store.clone());
    let job = service
        .submit(spec(key, restart_class))
        .await
        .expect("submit")
        .job_id;
    let claim = service
        .claim_attempt(
            &job,
            AttemptClaim::new(
                WorkerId::new("worker-a").expect("worker"),
                100,
                1_000,
                RunnerHandleRef::new("runner:live").expect("handle"),
            ),
        )
        .await
        .expect("claim");
    (store, service, job, claim)
}

#[tokio::test]
async fn success_and_its_typed_outbox_entry_commit_atomically_and_survive_reopen() {
    let (store, service, job, claim) = running_job("success-outbox", RestartClass::Adoptable).await;
    let result_ref = JobResultRef::new("artifact:scan-42").expect("result ref");

    let completed = service
        .complete_attempt(
            &job,
            AttemptWriteAuthority::from(&claim),
            900,
            Some(result_ref.clone()),
        )
        .await
        .expect("complete");

    assert_eq!(completed.terminal_kind, Some(JobTerminalKind::Succeeded));
    assert_eq!(
        completed.terminal_result,
        Some(JobTerminalResult::Succeeded {
            result_ref: Some(result_ref),
        })
    );
    assert_eq!(completed.outbox.len(), 1);
    assert!(!completed.outbox[0].applied);
    assert_eq!(
        completed.outbox[0].terminal_result,
        completed.terminal_result.clone().expect("terminal result")
    );

    let reopened = DetachedJobService::new(Arc::new(
        MemoryDetachedJobStore::from_snapshot(store.snapshot().await).expect("reopen"),
    ));
    let recovered = reopened.get(&job).await.expect("read").expect("job");
    assert_eq!(recovered.terminal_result, completed.terminal_result);
    assert_eq!(recovered.outbox, completed.outbox);

    let applied = reopened
        .mark_delivery_applied(&job, completed.outbox[0].delivery_sequence)
        .await
        .expect("ack delivery");
    assert!(applied.outbox[0].applied);
    let duplicate = reopened
        .mark_delivery_applied(&job, completed.outbox[0].delivery_sequence)
        .await
        .expect("duplicate delivery acknowledgement is idempotent");
    assert_eq!(duplicate.revision, applied.revision);
    assert_eq!(duplicate.outbox, applied.outbox);
}

#[tokio::test]
async fn failure_cancel_loss_and_attention_all_use_the_same_typed_outbox_protocol() {
    let (_, failed_service, failed_job, failed_claim) =
        running_job("failed-outbox", RestartClass::Adoptable).await;
    let failed = failed_service
        .fail_attempt(
            &failed_job,
            AttemptWriteAuthority::from(&failed_claim),
            900,
            JobFailureCode::new("runner_failed").expect("failure code"),
            None,
        )
        .await
        .expect("fail");

    let cancelled_store = Arc::new(MemoryDetachedJobStore::new());
    let cancelled_service = DetachedJobService::new(cancelled_store);
    let cancelled_job = cancelled_service
        .submit(spec("cancelled-outbox", RestartClass::NonResumable))
        .await
        .expect("submit")
        .job_id;
    let cancelled = cancelled_service
        .request_cancel(&cancelled_job)
        .await
        .expect("cancel queued");

    let (_, lost_service, lost_job, lost_claim) =
        running_job("lost-outbox", RestartClass::NonResumable).await;
    lost_service
        .observe_lease_expired(&lost_job, AttemptWriteAuthority::from(&lost_claim), 1_001)
        .await
        .expect("observe loss");
    let lost = lost_service
        .classify_worker_loss(&lost_job, 1_001)
        .await
        .expect("classify loss");

    let attention_store = Arc::new(MemoryDetachedJobStore::new());
    let attention_service = DetachedJobService::new(attention_store);
    let attention_job = attention_service
        .submit(spec("attention-outbox", RestartClass::Replayable))
        .await
        .expect("submit")
        .job_id;
    let attention = attention_service
        .mark_needs_attention(
            &attention_job,
            1,
            JobFailureCode::new("operator_action_required").expect("reason"),
        )
        .await
        .expect("needs attention");

    for (snapshot, expected) in [
        (failed, JobTerminalKind::Failed),
        (cancelled, JobTerminalKind::Cancelled),
        (lost, JobTerminalKind::WorkerLost),
        (attention, JobTerminalKind::NeedsAttention),
    ] {
        assert_eq!(snapshot.terminal_kind, Some(expected));
        assert_eq!(snapshot.outbox.len(), 1);
        assert_eq!(snapshot.outbox[0].terminal_kind, expected);
        assert_eq!(
            snapshot.outbox[0].terminal_result,
            snapshot.terminal_result.expect("typed terminal result")
        );
        assert!(!snapshot.outbox[0].applied);
    }
}

#[tokio::test]
async fn live_cancellation_is_a_request_until_the_current_attempt_acknowledges() {
    let (_, service, job, claim) = running_job("live-cancel", RestartClass::NonResumable).await;

    let requested = service.request_cancel(&job).await.expect("request cancel");
    assert!(requested.cancel_requested);
    assert!(requested.terminal_result.is_none());
    assert!(requested.outbox.is_empty());
    let duplicate_request = service
        .request_cancel(&job)
        .await
        .expect("duplicate cancellation request is idempotent");
    assert_eq!(duplicate_request.revision, requested.revision);

    let cancelled = service
        .acknowledge_cancel(&job, AttemptWriteAuthority::from(&claim), 900)
        .await
        .expect("ack cancel");
    assert_eq!(
        cancelled.terminal_result,
        Some(JobTerminalResult::Cancelled)
    );
    assert_eq!(cancelled.outbox.len(), 1);
}
