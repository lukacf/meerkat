#![allow(clippy::expect_used)]

use std::sync::Arc;

use meerkat_core::SessionId;
use meerkat_jobs::{
    AttemptClaim, AttemptWriteAuthority, CanonicalArgumentsHash, DetachedJobService,
    ExecutionIntentId, InteractionLineageId, JobNotification, JobOutboxPayload, JobSpec,
    JobSubmissionKey, JobTerminalResult, MemoryDetachedJobStore, PredicateComparison,
    PredicateEvaluation, PredicateObservation, PredicatePollingPolicy, PredicateSource,
    PredicateWatch, PredicateWatchId, RestartClass, RunnerHandleRef, RunnerIdentity, ScheduleIdRef,
    ToolIdentity, WorkerId,
};

fn monitor_spec(key: &str) -> JobSpec {
    JobSpec::new(
        "realm-a",
        SessionId::new(),
        ExecutionIntentId::new(),
        InteractionLineageId::new(),
        ToolIdentity::new("monitor", "v1").expect("tool"),
        RunnerIdentity::new("meerkat.monitor", "v1").expect("runner"),
        RestartClass::CheckpointResumable,
        CanonicalArgumentsHash::new(format!("sha256:{key}")).expect("hash"),
        JobSubmissionKey::new(key).expect("key"),
    )
}

#[tokio::test]
async fn notifications_are_nonterminal_deduplicated_and_share_job_delivery_sequence() {
    let store = Arc::new(MemoryDetachedJobStore::new());
    let service = DetachedJobService::new(store.clone());
    let job_id = service
        .submit(monitor_spec("notification-sequence"))
        .await
        .expect("submit")
        .job_id;
    let claim = service
        .claim_attempt(
            &job_id,
            AttemptClaim::new(
                WorkerId::new("monitor-worker").expect("worker"),
                100,
                10_000,
                RunnerHandleRef::new("monitor-handle").expect("handle"),
            ),
        )
        .await
        .expect("claim");
    let write = AttemptWriteAuthority::from(&claim);

    let first = service
        .emit_notification(
            &job_id,
            write.clone(),
            200,
            JobNotification::new(
                "release-v1",
                "release:meerkat:v1",
                "Meerkat v1 released",
                "The release is now public.",
            )
            .expect("notification"),
        )
        .await
        .expect("first notification");
    assert!(!first.deduplicated);
    assert_eq!(first.delivery_sequence, 1);
    assert_eq!(first.snapshot.phase, meerkat_jobs::JobPhase::Running);
    assert_eq!(first.snapshot.attempt_count, 1);
    assert_eq!(first.snapshot.current_fence, claim.fence);
    assert!(matches!(
        &first.snapshot.outbox[0].payload,
        JobOutboxPayload::Notification(notification)
            if notification.idempotency_key() == "release:meerkat:v1"
    ));

    let duplicate = service
        .emit_notification(
            &job_id,
            write.clone(),
            201,
            JobNotification::new(
                "release-v1-replayed",
                "release:meerkat:v1",
                "Meerkat v1 released",
                "The release is now public.",
            )
            .expect("notification"),
        )
        .await
        .expect("duplicate notification");
    assert!(duplicate.deduplicated);
    assert_eq!(duplicate.delivery_sequence, 1);
    assert_eq!(duplicate.snapshot.revision, first.snapshot.revision);
    assert_eq!(duplicate.snapshot.outbox.len(), 1);

    let second = service
        .emit_notification(
            &job_id,
            write.clone(),
            300,
            JobNotification::new(
                "release-v2",
                "release:meerkat:v2",
                "Meerkat v2 released",
                "A later release is now public.",
            )
            .expect("notification"),
        )
        .await
        .expect("second notification");
    assert_eq!(second.delivery_sequence, 2);
    assert_eq!(second.snapshot.phase, meerkat_jobs::JobPhase::Running);
    assert_eq!(second.snapshot.outbox.len(), 2);

    let terminal = service
        .complete_attempt(&job_id, write, 400, None)
        .await
        .expect("terminal");
    assert_eq!(terminal.outbox.len(), 3);
    assert_eq!(terminal.outbox[2].delivery_sequence, 3);
    assert!(matches!(
        terminal.outbox[2].payload,
        JobOutboxPayload::Terminal(JobTerminalResult::Succeeded { .. })
    ));

    let reopened = DetachedJobService::new(Arc::new(
        MemoryDetachedJobStore::from_snapshot(store.snapshot().await).expect("reopen"),
    ));
    let recovered = reopened.get(&job_id).await.expect("read").expect("job");
    assert_eq!(recovered.outbox, terminal.outbox);
    assert_eq!(recovered.attempt_count, 1);
    assert_eq!(recovered.current_fence, claim.fence);

    let applied = reopened
        .mark_delivery_applied(&job_id, first.delivery_sequence)
        .await
        .expect("apply notification delivery");
    let duplicate = reopened
        .mark_delivery_applied(&job_id, first.delivery_sequence)
        .await
        .expect("machine-authorized duplicate acknowledgement");
    assert_eq!(duplicate.revision, applied.revision);
    assert_eq!(duplicate.outbox, applied.outbox);
}

#[tokio::test]
async fn predicate_crossing_replay_after_notification_commit_does_not_lose_or_duplicate_delivery() {
    let store = Arc::new(MemoryDetachedJobStore::new());
    let service = DetachedJobService::new(store.clone());
    let job_id = service
        .submit(monitor_spec("predicate-crash-window"))
        .await
        .expect("submit")
        .job_id;
    let claim = service
        .claim_attempt(
            &job_id,
            AttemptClaim::new(
                WorkerId::new("predicate-worker").expect("worker"),
                100,
                10_000,
                RunnerHandleRef::new("predicate-handle").expect("handle"),
            ),
        )
        .await
        .expect("claim");
    let write = AttemptWriteAuthority::from(&claim);
    let watch = PredicateWatch::scheduled(
        PredicateWatchId::new("release-watch").expect("watch id"),
        ScheduleIdRef::new("schedule-release-watch").expect("schedule id"),
        PredicateSource::StableHttp {
            url: "https://example.invalid/releases/latest".into(),
            conditional_requests: true,
        },
        PredicateComparison::Changed,
        PredicatePollingPolicy::new(60, 1, 0, 1, 300).expect("policy"),
    )
    .expect("watch");

    let baseline = service
        .evaluate_predicate(
            &job_id,
            write.clone(),
            &watch,
            PredicateObservation::available("v1", "Version v1").expect("observation"),
            200,
        )
        .await
        .expect("record baseline");
    assert!(matches!(
        baseline.evaluation,
        PredicateEvaluation::Baseline { .. }
    ));
    assert!(baseline.notification.is_none());
    assert!(baseline.snapshot.outbox.is_empty());
    assert!(matches!(
        baseline
            .snapshot
            .progress
            .as_ref()
            .map(|progress| &progress.kind),
        Some(meerkat_jobs::JobProgressKind::Health {
            condition: meerkat_jobs::JobHealthCondition::Healthy
        })
    ));

    let unavailable = service
        .evaluate_predicate(
            &job_id,
            write.clone(),
            &watch,
            PredicateObservation::unavailable("source_timeout").expect("unavailable"),
            250,
        )
        .await
        .expect("persist source health");
    assert!(matches!(
        unavailable
            .snapshot
            .progress
            .as_ref()
            .map(|progress| &progress.kind),
        Some(meerkat_jobs::JobProgressKind::Health {
            condition: meerkat_jobs::JobHealthCondition::PredicateSourceUnavailable {
                retry_after_secs: 60
            }
        })
    ));
    assert!(unavailable.snapshot.outbox.is_empty());

    // Simulate the exact crash seam: the crossing notification commits, but
    // the later checkpoint write does not. Recovery must replay from v1 and
    // suppress the duplicate notification before advancing to v2.
    let crossing = watch
        .evaluate(
            baseline.evaluation.checkpoint(),
            PredicateObservation::available("v2", "Version v2").expect("observation"),
        )
        .expect("crossing");
    let notification = crossing
        .notification()
        .cloned()
        .expect("crossing notification");
    service
        .emit_notification(&job_id, write.clone(), 300, notification)
        .await
        .expect("commit notification before crash");

    let reopened = DetachedJobService::new(Arc::new(
        MemoryDetachedJobStore::from_snapshot(store.snapshot().await).expect("reopen"),
    ));
    let replayed = reopened
        .evaluate_predicate(
            &job_id,
            write.clone(),
            &watch,
            PredicateObservation::available("v2", "Version v2").expect("observation"),
            301,
        )
        .await
        .expect("replay crossing");
    assert!(
        replayed
            .notification
            .as_ref()
            .expect("deduplicated notification receipt")
            .deduplicated
    );
    assert_eq!(replayed.snapshot.outbox.len(), 1);
    assert_eq!(replayed.snapshot.attempt_count, 1);
    assert_eq!(replayed.snapshot.current_fence, claim.fence);

    let later = reopened
        .evaluate_predicate(
            &job_id,
            write,
            &watch,
            PredicateObservation::available("v3", "Version v3").expect("observation"),
            400,
        )
        .await
        .expect("later crossing");
    assert!(
        !later
            .notification
            .as_ref()
            .expect("later notification")
            .deduplicated
    );
    assert_eq!(later.snapshot.outbox.len(), 2);
    assert_eq!(later.snapshot.outbox[0].delivery_sequence, 1);
    assert_eq!(later.snapshot.outbox[1].delivery_sequence, 2);
}
