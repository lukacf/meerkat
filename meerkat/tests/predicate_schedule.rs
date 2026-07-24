#![cfg(feature = "schedule")]
#![allow(clippy::expect_used)]

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use chrono::{TimeZone, Utc};
use meerkat::{
    PredicateObservationProvider, PredicateSourceObservationError, ScheduledPredicateParams,
    ScheduledPredicateRunnable, scheduled_predicate_runner_handle,
};
use meerkat_core::SessionId;
use meerkat_jobs::{
    AttemptClaim, CanonicalArgumentsHash, DetachedJobService, ExecutionIntentId,
    InteractionLineageId, JobSpec, JobSubmissionKey, MemoryDetachedJobStore, PredicateComparison,
    PredicateObservation, PredicatePollingPolicy, PredicateSource, PredicateWatch,
    PredicateWatchId, RestartClass, RunnerIdentity, ScheduleIdRef, ToolIdentity, WorkerId,
};
use meerkat_schedule::{
    HostRunnable, HostRunnableInvocation, HostRunnableName, OccurrenceId, ScheduleId,
};

struct SequenceProvider {
    observations: Mutex<VecDeque<PredicateObservation>>,
}

#[async_trait::async_trait]
impl PredicateObservationProvider for SequenceProvider {
    async fn observe(
        &self,
        _watch: &PredicateWatch,
    ) -> Result<PredicateObservation, PredicateSourceObservationError> {
        self.observations
            .lock()
            .expect("observations")
            .pop_front()
            .ok_or_else(|| PredicateSourceObservationError::new("no observation"))
    }
}

fn watch(schedule_id: &ScheduleId) -> PredicateWatch {
    PredicateWatch::scheduled(
        PredicateWatchId::new("release-watch").expect("watch id"),
        ScheduleIdRef::new(schedule_id.to_string()).expect("schedule id"),
        PredicateSource::StableHttp {
            url: "https://example.invalid/releases/latest".into(),
            conditional_requests: true,
        },
        PredicateComparison::Changed,
        PredicatePollingPolicy::new(60, 1, 0, 1, 300).expect("policy"),
    )
    .expect("watch")
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .expect("clock")
        .as_millis()
        .try_into()
        .expect("millis")
}

#[tokio::test]
async fn schedule_occurrences_drive_predicate_checkpoint_and_notification_without_timer_registry() {
    let store = Arc::new(MemoryDetachedJobStore::new());
    let jobs = DetachedJobService::new(store);
    let schedule_id = ScheduleId::new();
    let watch = watch(&schedule_id);
    let job_id = jobs
        .submit(JobSpec::new(
            "realm-a",
            SessionId::new(),
            ExecutionIntentId::new(),
            InteractionLineageId::new(),
            ToolIdentity::new("predicate_watch", "v1").expect("tool"),
            RunnerIdentity::new("meerkat.schedule_predicate", "v1").expect("runner"),
            RestartClass::Replayable,
            CanonicalArgumentsHash::new("sha256:predicate").expect("hash"),
            JobSubmissionKey::new("predicate-schedule").expect("key"),
        ))
        .await
        .expect("submit")
        .job_id;
    let claim = jobs
        .claim_attempt(
            &job_id,
            AttemptClaim::new(
                WorkerId::new("schedule-driver").expect("worker"),
                now_ms(),
                now_ms() + 10_000,
                scheduled_predicate_runner_handle(&schedule_id).expect("runner handle"),
            ),
        )
        .await
        .expect("claim");
    let provider = Arc::new(SequenceProvider {
        observations: Mutex::new(VecDeque::from([
            PredicateObservation::available("v1", "Version v1").expect("baseline"),
            PredicateObservation::available("v2", "Version v2").expect("crossing"),
        ])),
    });
    let runnable = ScheduledPredicateRunnable::new(jobs.clone(), provider);
    let params = ScheduledPredicateParams::new(job_id.clone(), watch)
        .target_binding()
        .expect("target")
        .params
        .expect("params")
        .into_raw();

    for timestamp in [100, 200] {
        runnable
            .run(HostRunnableInvocation {
                occurrence_id: OccurrenceId::new(),
                schedule_id: schedule_id.clone(),
                runnable: HostRunnableName::parse("meerkat.predicate.evaluate.v1")
                    .expect("runnable"),
                trigger_time: Utc
                    .timestamp_millis_opt(timestamp)
                    .single()
                    .expect("timestamp"),
                params: Some(params.clone()),
            })
            .await
            .expect("occurrence");
    }

    let snapshot = jobs.get(&job_id).await.expect("read").expect("job");
    assert_eq!(snapshot.attempt_count, 1);
    assert_eq!(snapshot.current_fence, claim.fence);
    assert!(
        snapshot
            .checkpoint_ref
            .as_ref()
            .is_some_and(|checkpoint| checkpoint.as_str().contains("\"stable_key\":\"v2\""))
    );
    assert_eq!(snapshot.outbox.len(), 1);
}

#[tokio::test]
async fn mismatched_schedule_cannot_reuse_a_predicate_jobs_current_fence() {
    let store = Arc::new(MemoryDetachedJobStore::new());
    let jobs = DetachedJobService::new(store);
    let schedule_id = ScheduleId::new();
    let other_schedule = ScheduleId::new();
    let watch = watch(&schedule_id);
    let job_id = jobs
        .submit(JobSpec::new(
            "realm-a",
            SessionId::new(),
            ExecutionIntentId::new(),
            InteractionLineageId::new(),
            ToolIdentity::new("predicate_watch", "v1").expect("tool"),
            RunnerIdentity::new("meerkat.schedule_predicate", "v1").expect("runner"),
            RestartClass::Replayable,
            CanonicalArgumentsHash::new("sha256:predicate-mismatch").expect("hash"),
            JobSubmissionKey::new("predicate-schedule-mismatch").expect("key"),
        ))
        .await
        .expect("submit")
        .job_id;
    jobs.claim_attempt(
        &job_id,
        AttemptClaim::new(
            WorkerId::new("schedule-driver").expect("worker"),
            now_ms(),
            now_ms() + 10_000,
            scheduled_predicate_runner_handle(&schedule_id).expect("runner handle"),
        ),
    )
    .await
    .expect("claim");
    let runnable = ScheduledPredicateRunnable::new(
        jobs.clone(),
        Arc::new(SequenceProvider {
            observations: Mutex::new(VecDeque::from([PredicateObservation::available(
                "v1",
                "Version v1",
            )
            .expect("observation")])),
        }),
    );
    let params = ScheduledPredicateParams::new(job_id.clone(), watch)
        .target_binding()
        .expect("target")
        .params
        .expect("params")
        .into_raw();
    let error = runnable
        .run(HostRunnableInvocation {
            occurrence_id: OccurrenceId::new(),
            schedule_id: other_schedule,
            runnable: HostRunnableName::parse("meerkat.predicate.evaluate.v1").expect("runnable"),
            trigger_time: Utc.timestamp_millis_opt(100).single().expect("timestamp"),
            params: Some(params),
        })
        .await
        .expect_err("mismatched schedule");
    assert!(error.to_string().contains("does not match"));
    let snapshot = jobs.get(&job_id).await.expect("read").expect("job");
    assert!(snapshot.checkpoint_ref.is_none());
    assert!(snapshot.outbox.is_empty());
    assert_eq!(snapshot.attempt_count, 1);
    assert_eq!(snapshot.current_fence.get(), 1);
}

#[tokio::test]
async fn delayed_occurrence_trigger_time_cannot_write_through_an_expired_job_lease() {
    let store = Arc::new(MemoryDetachedJobStore::new());
    let jobs = DetachedJobService::new(store);
    let schedule_id = ScheduleId::new();
    let watch = watch(&schedule_id);
    let job_id = jobs
        .submit(JobSpec::new(
            "realm-a",
            SessionId::new(),
            ExecutionIntentId::new(),
            InteractionLineageId::new(),
            ToolIdentity::new("predicate_watch", "v1").expect("tool"),
            RunnerIdentity::new("meerkat.schedule_predicate", "v1").expect("runner"),
            RestartClass::Replayable,
            CanonicalArgumentsHash::new("sha256:predicate-expired").expect("hash"),
            JobSubmissionKey::new("predicate-schedule-expired").expect("key"),
        ))
        .await
        .expect("submit")
        .job_id;
    let current = now_ms();
    jobs.claim_attempt(
        &job_id,
        AttemptClaim::new(
            WorkerId::new("schedule-driver").expect("worker"),
            current.saturating_sub(2),
            current.saturating_sub(1),
            scheduled_predicate_runner_handle(&schedule_id).expect("runner handle"),
        ),
    )
    .await
    .expect("claim");
    let runnable = ScheduledPredicateRunnable::new(
        jobs.clone(),
        Arc::new(SequenceProvider {
            observations: Mutex::new(VecDeque::from([PredicateObservation::available(
                "v1",
                "Version v1",
            )
            .expect("observation")])),
        }),
    );
    let params = ScheduledPredicateParams::new(job_id.clone(), watch)
        .target_binding()
        .expect("target")
        .params
        .expect("params")
        .into_raw();
    let error = runnable
        .run(HostRunnableInvocation {
            occurrence_id: OccurrenceId::new(),
            schedule_id,
            runnable: HostRunnableName::parse("meerkat.predicate.evaluate.v1").expect("runnable"),
            trigger_time: Utc.timestamp_millis_opt(1).single().expect("timestamp"),
            params: Some(params),
        })
        .await
        .expect_err("expired lease");
    assert!(error.to_string().contains("transition"));
    let snapshot = jobs.get(&job_id).await.expect("read").expect("job");
    assert!(snapshot.checkpoint_ref.is_none());
    assert!(snapshot.outbox.is_empty());
    assert_eq!(snapshot.attempt_count, 1);
    assert_eq!(snapshot.current_fence.get(), 1);
}
