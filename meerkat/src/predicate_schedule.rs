//! Schedule-owned execution seam for first-class predicate watches.
//!
//! The schedule target carries the immutable watch definition and job
//! identity. Schedule remains the only timer/occurrence authority; the job
//! machine remains the only lifecycle, attempt, fence, checkpoint, and
//! notification authority.

use std::sync::Arc;

use meerkat_jobs::{
    AttemptWriteAuthority, DetachedJobService, JobId, JobPhase, PredicateObservation,
    PredicateWatch, RunnerHandleRef,
};
use meerkat_schedule::{
    HostRunnable, HostRunnableError, HostRunnableInvocation, HostRunnableName, HostRunnableOutcome,
    HostRunnableParams, HostRunnableTargetBinding, ScheduleId,
};
use serde::{Deserialize, Serialize};

pub const SCHEDULED_PREDICATE_RUNNABLE_NAME: &str = "meerkat.predicate.evaluate.v1";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScheduledPredicateParams {
    pub job_id: JobId,
    pub watch: PredicateWatch,
}

impl ScheduledPredicateParams {
    pub fn new(job_id: JobId, watch: PredicateWatch) -> Self {
        Self { job_id, watch }
    }

    pub fn target_binding(&self) -> Result<HostRunnableTargetBinding, String> {
        let params = serde_json::to_string(self)
            .map_err(|error| format!("cannot encode scheduled predicate parameters: {error}"))?;
        Ok(HostRunnableTargetBinding {
            runnable: scheduled_predicate_runnable_name()?,
            params: Some(
                HostRunnableParams::parse(&params)
                    .map_err(|error| format!("invalid scheduled predicate parameters: {error}"))?,
            ),
        })
    }
}

pub fn scheduled_predicate_runnable_name() -> Result<HostRunnableName, String> {
    HostRunnableName::parse(SCHEDULED_PREDICATE_RUNNABLE_NAME).map_err(|error| error.to_string())
}

pub fn scheduled_predicate_runner_handle(
    schedule_id: &ScheduleId,
) -> Result<RunnerHandleRef, String> {
    RunnerHandleRef::new(format!("schedule-predicate:{schedule_id}"))
        .map_err(|error| error.to_string())
}

#[derive(Debug, thiserror::Error)]
#[error("predicate source observation failed: {detail}")]
pub struct PredicateSourceObservationError {
    pub detail: String,
}

impl PredicateSourceObservationError {
    pub fn new(detail: impl Into<String>) -> Self {
        Self {
            detail: detail.into(),
        }
    }
}

#[async_trait::async_trait]
pub trait PredicateObservationProvider: Send + Sync {
    /// Observe the source once.
    ///
    /// Source unavailability that should preserve the watch and follow its
    /// retry policy is returned as `PredicateObservation::Unavailable`.
    /// `Err` is reserved for a broken adapter/invocation contract and fails
    /// the Schedule occurrence.
    async fn observe(
        &self,
        watch: &PredicateWatch,
    ) -> Result<PredicateObservation, PredicateSourceObservationError>;
}

#[derive(Clone)]
pub struct ScheduledPredicateRunnable {
    jobs: DetachedJobService,
    observations: Arc<dyn PredicateObservationProvider>,
}

impl std::fmt::Debug for ScheduledPredicateRunnable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScheduledPredicateRunnable")
            .finish_non_exhaustive()
    }
}

impl ScheduledPredicateRunnable {
    pub fn new(
        jobs: DetachedJobService,
        observations: Arc<dyn PredicateObservationProvider>,
    ) -> Self {
        Self { jobs, observations }
    }
}

#[async_trait::async_trait]
impl HostRunnable for ScheduledPredicateRunnable {
    async fn run(
        &self,
        invocation: HostRunnableInvocation,
    ) -> Result<HostRunnableOutcome, HostRunnableError> {
        let params = invocation
            .params
            .as_ref()
            .ok_or_else(|| failed("scheduled predicate invocation requires target parameters"))?;
        let params: ScheduledPredicateParams =
            serde_json::from_str(params.get()).map_err(|error| {
                failed(format!(
                    "scheduled predicate target parameters are invalid: {error}"
                ))
            })?;
        if params.watch.schedule_id().as_str() != invocation.schedule_id.to_string() {
            return Err(failed(format!(
                "predicate watch schedule {} does not match occurrence schedule {}",
                params.watch.schedule_id().as_str(),
                invocation.schedule_id
            )));
        }

        let snapshot = self
            .jobs
            .get(&params.job_id)
            .await
            .map_err(|error| failed(error.to_string()))?
            .ok_or_else(|| failed(format!("predicate job {} does not exist", params.job_id)))?;
        if !matches!(
            snapshot.phase,
            JobPhase::Running | JobPhase::WaitingExternal
        ) {
            return Err(failed(format!(
                "predicate job {} is not active: {:?}",
                params.job_id, snapshot.phase
            )));
        }
        let expected_handle =
            scheduled_predicate_runner_handle(&invocation.schedule_id).map_err(failed)?;
        if snapshot.runner_handle.as_ref() != Some(&expected_handle) {
            return Err(failed(format!(
                "predicate job {} is not claimed by schedule {}",
                params.job_id, invocation.schedule_id
            )));
        }
        let attempt_id = snapshot.current_attempt_id.ok_or_else(|| {
            failed(format!(
                "predicate job {} has no committed attempt authority",
                params.job_id
            ))
        })?;
        let write = AttemptWriteAuthority {
            attempt_id,
            fence: snapshot.current_fence,
        };
        let observation = self
            .observations
            .observe(&params.watch)
            .await
            .map_err(|error| failed(error.to_string()))?;
        // The trigger time is occurrence identity, not write-authority time.
        // A delayed occurrence must not use an old scheduled timestamp to
        // write through an already-expired job lease.
        let observed_at_ms = meerkat_core::time_compat::SystemTime::now()
            .duration_since(meerkat_core::time_compat::SystemTime::UNIX_EPOCH)
            .map_err(|error| failed(format!("system clock predates Unix epoch: {error}")))?
            .as_millis()
            .try_into()
            .map_err(|_| failed("current time cannot be represented as milliseconds"))?;
        self.jobs
            .evaluate_predicate(
                &params.job_id,
                write,
                &params.watch,
                observation,
                observed_at_ms,
            )
            .await
            .map_err(|error| failed(error.to_string()))?;
        Ok(HostRunnableOutcome::completed())
    }
}

fn failed(detail: impl Into<String>) -> HostRunnableError {
    HostRunnableError::Failed {
        detail: detail.into(),
    }
}
