//! A cancellation request converges however often the live attempt's
//! writes (lease heartbeats, progress) win the revision race: a lost
//! compare-and-swap is re-read and re-applied, never surfaced as an error.

#![allow(clippy::expect_used)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use meerkat_core::SessionId;
use meerkat_jobs::{
    AttemptClaim, AttemptWriteAuthority, CanonicalArgumentsHash, DetachedJobError,
    DetachedJobService, DetachedJobStore, ExecutionIntentId, InsertJobOutcome,
    InteractionLineageId, JobId, JobOutboxEntry, JobSpec, JobSubmissionKey, MemoryDetachedJobStore,
    RestartClass, RunnerHandleRef, RunnerIdentity, StoredJob, ToolIdentity, WorkerId,
};

/// Commits a competing lease renewal right before each of the first
/// `renewals_to_win` cancellation-request swaps, so each of them loses.
struct ContendedStore {
    inner: Arc<MemoryDetachedJobStore>,
    writer: Mutex<Option<AttemptWriteAuthority>>,
    renewals_to_win: AtomicUsize,
    renewals_won: AtomicUsize,
}

#[async_trait]
impl DetachedJobStore for ContendedStore {
    async fn insert_deduplicated(
        &self,
        job: StoredJob,
    ) -> Result<InsertJobOutcome, DetachedJobError> {
        self.inner.insert_deduplicated(job).await
    }

    async fn get(&self, job_id: &JobId) -> Result<Option<StoredJob>, DetachedJobError> {
        self.inner.get(job_id).await
    }

    async fn compare_and_swap(
        &self,
        expected_revision: u64,
        replacement: StoredJob,
    ) -> Result<StoredJob, DetachedJobError> {
        let is_cancel_request = self
            .inner
            .get(&replacement.job_id)
            .await?
            .is_some_and(|current| {
                !current.machine_state.cancel_requested
                    && replacement.machine_state.cancel_requested
            });
        let writer = self
            .writer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if is_cancel_request
            && let Some(writer) = writer
            && self
                .renewals_to_win
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                    left.checked_sub(1)
                })
                .is_ok()
        {
            let won = self.renewals_won.fetch_add(1, Ordering::SeqCst) as u64;
            DetachedJobService::new(self.inner.clone())
                .renew_lease(&replacement.job_id, writer, 200 + won, 10_000 + won)
                .await?;
        }
        self.inner
            .compare_and_swap(expected_revision, replacement)
            .await
    }

    async fn list_pending_outbox(
        &self,
        limit: usize,
    ) -> Result<Vec<JobOutboxEntry>, DetachedJobError> {
        self.inner.list_pending_outbox(limit).await
    }

    async fn list_for_origin(
        &self,
        realm_id: &str,
        origin_session_id: &SessionId,
        limit: usize,
    ) -> Result<Vec<StoredJob>, DetachedJobError> {
        self.inner
            .list_for_origin(realm_id, origin_session_id, limit)
            .await
    }

    async fn list_all(&self, limit: usize) -> Result<Vec<StoredJob>, DetachedJobError> {
        self.inner.list_all(limit).await
    }

    async fn count_pending_outbox_jobs(
        &self,
        realm_id: Option<&str>,
    ) -> Result<u64, DetachedJobError> {
        self.inner.count_pending_outbox_jobs(realm_id).await
    }

    async fn list_census_candidates(
        &self,
        realm_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<StoredJob>, DetachedJobError> {
        self.inner.list_census_candidates(realm_id, limit).await
    }

    fn is_persistent(&self) -> bool {
        false
    }

    fn outbox_commit_signal(&self) -> meerkat_jobs::JobOutboxCommitSignal {
        self.inner.outbox_commit_signal()
    }
}

#[tokio::test]
async fn a_cancellation_request_converges_past_any_number_of_lost_swaps() {
    const LOST_SWAPS: usize = 20;
    let store = Arc::new(ContendedStore {
        inner: Arc::new(MemoryDetachedJobStore::new()),
        writer: Mutex::new(None),
        renewals_to_win: AtomicUsize::new(LOST_SWAPS),
        renewals_won: AtomicUsize::new(0),
    });
    let service = DetachedJobService::new(store.clone());
    let job = service
        .submit(JobSpec::new(
            "realm-a",
            SessionId::new(),
            ExecutionIntentId::new(),
            InteractionLineageId::new(),
            ToolIdentity::new("scan", "v1").expect("tool"),
            RunnerIdentity::new("runner.scan", "v1").expect("runner"),
            RestartClass::NonResumable,
            CanonicalArgumentsHash::new("sha256:args").expect("hash"),
            JobSubmissionKey::new("contended-cancel").expect("key"),
        ))
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
    *store
        .writer
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) =
        Some(AttemptWriteAuthority::from(&claim));

    let requested = service
        .request_cancel(&job)
        .await
        .expect("a lost swap is re-read and re-applied, never an error");

    assert!(requested.cancel_requested);
    assert!(requested.terminal_result.is_none());
    assert_eq!(store.renewals_won.load(Ordering::SeqCst), LOST_SWAPS);
}
