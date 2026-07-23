use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::RwLock;

use crate::machines::detached_job::{DetachedJobMachineAuthority, DetachedJobMachineState};
use crate::{
    DetachedJobError, JobId, JobOutboxEntry, JobProgress, JobSpec, JobSubmissionKey,
    JobTerminalResult,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredJob {
    pub job_id: JobId,
    pub spec: JobSpec,
    pub revision: u64,
    pub machine_state: DetachedJobMachineState,
    pub progress: Option<JobProgress>,
    pub terminal_result: Option<JobTerminalResult>,
    pub outbox: Vec<JobOutboxEntry>,
}

#[doc(hidden)]
#[derive(Debug, Clone)]
pub enum InsertJobOutcome {
    Inserted(StoredJob),
    Existing(StoredJob),
}

#[async_trait]
pub trait DetachedJobStore: Send + Sync {
    async fn insert_deduplicated(
        &self,
        job: StoredJob,
    ) -> Result<InsertJobOutcome, DetachedJobError>;

    async fn get(&self, job_id: &JobId) -> Result<Option<StoredJob>, DetachedJobError>;

    async fn compare_and_swap(
        &self,
        expected_revision: u64,
        replacement: StoredJob,
    ) -> Result<StoredJob, DetachedJobError>;
}

#[derive(Debug, Clone, Default)]
pub struct MemoryDetachedJobStoreSnapshot {
    jobs: BTreeMap<JobId, StoredJob>,
    submission_index: BTreeMap<(String, JobSubmissionKey), JobId>,
}

#[derive(Debug, Default)]
struct MemoryDetachedJobStoreState {
    jobs: BTreeMap<JobId, StoredJob>,
    submission_index: BTreeMap<(String, JobSubmissionKey), JobId>,
}

#[derive(Debug, Clone, Default)]
pub struct MemoryDetachedJobStore {
    inner: Arc<RwLock<MemoryDetachedJobStoreState>>,
}

impl MemoryDetachedJobStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_snapshot(
        snapshot: MemoryDetachedJobStoreSnapshot,
    ) -> Result<Self, DetachedJobError> {
        for ((realm_id, submission_key), job_id) in &snapshot.submission_index {
            let Some(job) = snapshot.jobs.get(job_id) else {
                return Err(DetachedJobError::Store(format!(
                    "submission index {realm_id}/{submission_key} points to missing job {job_id}"
                )));
            };
            if &job.spec.realm_id != realm_id || &job.spec.submission_key != submission_key {
                return Err(DetachedJobError::Store(format!(
                    "submission index {realm_id}/{submission_key} disagrees with job {}",
                    job.job_id
                )));
            }
            validate_stored_job(job)?;
        }
        for job in snapshot.jobs.values() {
            let index_key = (job.spec.realm_id.clone(), job.spec.submission_key.clone());
            if snapshot.submission_index.get(&index_key) != Some(&job.job_id) {
                return Err(DetachedJobError::Store(format!(
                    "job {} has no matching realm-scoped submission index",
                    job.job_id
                )));
            }
        }
        Ok(Self {
            inner: Arc::new(RwLock::new(MemoryDetachedJobStoreState {
                jobs: snapshot.jobs,
                submission_index: snapshot.submission_index,
            })),
        })
    }

    pub async fn snapshot(&self) -> MemoryDetachedJobStoreSnapshot {
        let guard = self.inner.read().await;
        MemoryDetachedJobStoreSnapshot {
            jobs: guard.jobs.clone(),
            submission_index: guard.submission_index.clone(),
        }
    }

    pub async fn get(&self, job_id: &JobId) -> Result<Option<StoredJob>, DetachedJobError> {
        DetachedJobStore::get(self, job_id).await
    }

    pub async fn len(&self) -> usize {
        self.inner.read().await.jobs.len()
    }

    pub async fn is_empty(&self) -> bool {
        self.inner.read().await.jobs.is_empty()
    }
}

#[async_trait]
impl DetachedJobStore for MemoryDetachedJobStore {
    async fn insert_deduplicated(
        &self,
        job: StoredJob,
    ) -> Result<InsertJobOutcome, DetachedJobError> {
        validate_stored_job(&job)?;
        let mut guard = self.inner.write().await;
        let index_key = (job.spec.realm_id.clone(), job.spec.submission_key.clone());
        if let Some(existing_id) = guard.submission_index.get(&index_key) {
            let existing = guard.jobs.get(existing_id).ok_or_else(|| {
                DetachedJobError::Store(format!(
                    "submission index points to missing job {existing_id}"
                ))
            })?;
            if !existing.spec.equivalent_submission(&job.spec) {
                return Err(DetachedJobError::SubmissionConflict);
            }
            return Ok(InsertJobOutcome::Existing(existing.clone()));
        }
        if guard.jobs.contains_key(&job.job_id) {
            return Err(DetachedJobError::Store(format!(
                "generated job id {} already exists",
                job.job_id
            )));
        }
        guard.submission_index.insert(index_key, job.job_id.clone());
        guard.jobs.insert(job.job_id.clone(), job.clone());
        Ok(InsertJobOutcome::Inserted(job))
    }

    async fn get(&self, job_id: &JobId) -> Result<Option<StoredJob>, DetachedJobError> {
        Ok(self.inner.read().await.jobs.get(job_id).cloned())
    }

    async fn compare_and_swap(
        &self,
        expected_revision: u64,
        mut replacement: StoredJob,
    ) -> Result<StoredJob, DetachedJobError> {
        let mut guard = self.inner.write().await;
        let current = guard
            .jobs
            .get(&replacement.job_id)
            .ok_or_else(|| DetachedJobError::NotFound(replacement.job_id.clone()))?;
        if current.revision != expected_revision {
            return Err(DetachedJobError::StaleRevision {
                job_id: replacement.job_id.clone(),
                expected: expected_revision,
                actual: current.revision,
            });
        }
        if current.spec.submission_key != replacement.spec.submission_key
            || !current.spec.equivalent_submission(&replacement.spec)
        {
            return Err(DetachedJobError::Store(
                "compare-and-swap cannot change the submitted job specification".into(),
            ));
        }
        validate_stored_job(&replacement)?;
        replacement.revision = next_revision(expected_revision)?;
        guard
            .jobs
            .insert(replacement.job_id.clone(), replacement.clone());
        Ok(replacement)
    }
}

fn next_revision(current: u64) -> Result<u64, DetachedJobError> {
    current.checked_add(1).ok_or_else(|| {
        DetachedJobError::Store("detached job revision exhausted u64 authority".into())
    })
}

fn validate_stored_job(job: &StoredJob) -> Result<(), DetachedJobError> {
    let state = &job.machine_state;
    if state.job_id != job.job_id.as_str() {
        return Err(DetachedJobError::Store(format!(
            "job {} disagrees with generated authority identity {}",
            job.job_id, state.job_id
        )));
    }
    if state.restart_class != job.spec.restart_class {
        return Err(DetachedJobError::Store(format!(
            "job {} restart class disagrees with generated authority",
            job.job_id
        )));
    }
    DetachedJobMachineAuthority::recover_from_state(state.clone()).map_err(|error| {
        DetachedJobError::Store(format!(
            "job {} contains invalid generated authority state: {error:?}",
            job.job_id
        ))
    })?;
    match (&job.progress, state.progress_cursor) {
        (None, 0) => {}
        (Some(progress), cursor) if progress.cursor == cursor => {}
        _ => {
            return Err(DetachedJobError::Store(format!(
                "job {} progress projection disagrees with generated authority",
                job.job_id
            )));
        }
    }
    match (state.terminal_kind, &job.terminal_result) {
        (None, None) if job.outbox.is_empty() => {}
        (Some(kind), Some(result))
            if result.kind() == kind
                && job.outbox.len() == 1
                && job.outbox[0].job_id == job.job_id
                && job.outbox[0].delivery_sequence == state.terminal_delivery_sequence
                && job.outbox[0].terminal_kind == kind
                && job.outbox[0].terminal_result == *result
                && job.outbox[0].applied == state.terminal_delivery_applied => {}
        _ => {
            return Err(DetachedJobError::Store(format!(
                "job {} terminal result/outbox projection disagrees with generated authority",
                job.job_id
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn revision_increment_fails_closed_at_u64_max() {
        assert!(super::next_revision(u64::MAX).is_err());
    }
}
