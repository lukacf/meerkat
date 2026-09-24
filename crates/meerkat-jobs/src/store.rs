use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

#[cfg(target_arch = "wasm32")]
use crate::tokio::sync::RwLock;
#[cfg(not(target_arch = "wasm32"))]
use ::tokio::sync::RwLock;
use async_trait::async_trait;
use meerkat_core::SessionId;

use crate::machines::detached_job::{
    DetachedJobMachineAuthority, DetachedJobMachineState, DetachedJobPhase,
};
use crate::{
    DetachedJobError, JobId, JobOutboxEntry, JobOutboxPayload, JobProgress, JobSpec,
    JobSubmissionKey, JobTerminalResult, PredicateDeliveryCommit, PredicateDeliveryIdempotencyKey,
    PredicateDeliveryIdentity, PredicateDeliveryReceipt,
};

/// Whether an operational census must still look at a row in this phase.
///
/// **This is not machine terminality and must never be replaced by it.** The
/// generated machine's terminal set is
/// `[Succeeded, Failed, Cancelled, WorkerLost, NeedsAttention]`, and
/// `NeedsAttention` is one of the three conditions that degrade the census.
/// Classifying rows by machine terminality would silently drop every job
/// parked for a human out of the health window - the same silence this whole
/// lane exists to remove, wearing a plausible name.
///
/// The exhaustive `match` is the point: a new phase cannot be added without
/// someone answering this question for it. The classification is the single
/// owner of the fact; the durable `census_live` column and the SQL window are
/// both projections of it.
pub(crate) const fn phase_is_census_live(phase: DetachedJobPhase) -> bool {
    match phase {
        // Live work, or work parked for a human. All of it can be wedged, and
        // some of it is how a wedge is reported.
        DetachedJobPhase::Unsubmitted
        | DetachedJobPhase::Queued
        | DetachedJobPhase::Claimed
        | DetachedJobPhase::Running
        | DetachedJobPhase::WaitingExternal
        | DetachedJobPhase::LossObserved
        | DetachedJobPhase::RetryScheduled
        | DetachedJobPhase::NeedsAttention => true,
        // Settled: the job is over and no lifecycle term can change again.
        // Its delivery obligations are counted separately and remain visible
        // through `count_pending_outbox_jobs`, which is phase-blind.
        DetachedJobPhase::Succeeded
        | DetachedJobPhase::Failed
        | DetachedJobPhase::Cancelled
        | DetachedJobPhase::WorkerLost => false,
    }
}

pub(crate) fn job_is_census_live(job: &StoredJob) -> bool {
    phase_is_census_live(job.machine_state.lifecycle_phase)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredJob {
    pub job_id: JobId,
    pub spec: JobSpec,
    pub revision: u64,
    pub machine_state: DetachedJobMachineState,
    pub progress: Option<JobProgress>,
    pub terminal_result: Option<JobTerminalResult>,
    pub subscriptions: Vec<crate::JobSubscription>,
    pub outbox: Vec<JobOutboxEntry>,
}

#[doc(hidden)]
#[derive(Debug, Clone)]
pub enum InsertJobOutcome {
    Inserted(StoredJob),
    Existing(StoredJob),
}

#[doc(hidden)]
#[derive(Debug, Clone, PartialEq)]
pub enum PredicateDeliveryCommitOutcome {
    Committed {
        job: StoredJob,
        receipt: PredicateDeliveryReceipt,
    },
    Deduplicated {
        job: StoredJob,
        receipt: PredicateDeliveryReceipt,
    },
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

    /// Read the immutable result for one exact predicate delivery identity.
    ///
    /// The default fails closed. Stores that host scheduled predicates must
    /// implement this together with [`Self::commit_predicate_delivery`].
    async fn predicate_delivery_receipt(
        &self,
        _job_id: &JobId,
        _identity: &PredicateDeliveryIdentity,
    ) -> Result<Option<PredicateDeliveryReceipt>, DetachedJobError> {
        Err(DetachedJobError::Store(
            "detached-job store does not support predicate delivery idempotency".into(),
        ))
    }

    /// Atomically replace a job and insert its stable predicate result.
    ///
    /// Implementations must commit both halves or neither. Exact retries
    /// return the original receipt. Reusing the key with another occurrence
    /// or runnable must fail closed.
    async fn commit_predicate_delivery(
        &self,
        _expected_revision: u64,
        _replacement: StoredJob,
        _commit: PredicateDeliveryCommit,
    ) -> Result<PredicateDeliveryCommitOutcome, DetachedJobError> {
        Err(DetachedJobError::Store(
            "detached-job store does not support atomic predicate delivery commits".into(),
        ))
    }

    async fn list_pending_outbox(
        &self,
        limit: usize,
    ) -> Result<Vec<JobOutboxEntry>, DetachedJobError>;

    async fn list_for_origin(
        &self,
        realm_id: &str,
        origin_session_id: &SessionId,
        limit: usize,
    ) -> Result<Vec<StoredJob>, DetachedJobError>;

    /// Mechanical recovery census. Callers must use generated lifecycle state
    /// to decide whether a row is runnable, reconcilable, or terminal.
    async fn list_all(&self, limit: usize) -> Result<Vec<StoredJob>, DetachedJobError>;

    /// Exact, uncapped count of jobs carrying at least one unapplied outbox
    /// entry, optionally narrowed to one realm.
    ///
    /// This is deliberately NOT expressible as a bounded [`Self::list_all`]
    /// scan: an outbox backlog is exactly the condition that persists while
    /// rows accumulate, so a capped window answers a different question as the
    /// store ages. It is also deliberately phase-blind - a terminal job whose
    /// terminal delivery never applied is the canonical case this counts.
    ///
    /// The unit is JOBS, not entries: entry counts live inside the job
    /// document, and decoding every document is the cost this method exists
    /// to avoid. There is no default implementation, because a default that
    /// fell back to a capped scan would silently reintroduce the blindness in
    /// every store that forgot to override it.
    async fn count_pending_outbox_jobs(
        &self,
        realm_id: Option<&str>,
    ) -> Result<u64, DetachedJobError>;

    /// Rows an operational census must look at: realm-filtered IN THE STORE,
    /// restricted to phases that can still change, capped last.
    ///
    /// Every clause is load-bearing against a specific way the previous
    /// `list_all` window lied. Filtering the realm after the cap let another
    /// realm's rows consume this realm's window. Including settled rows let
    /// history consume it - and because job ids are time-ordered, history is
    /// exactly what sorts first, so the window degraded with age rather than
    /// with load.
    ///
    /// The cap remains, because an unbounded decode of live jobs is a real
    /// cost. It is now a bound on LIVE work, which is an operational fact
    /// worth reporting rather than a function of retention - and when it is
    /// hit the caller must publish [`crate::JobHealthCoverage::Truncated`]
    /// rather than counts.
    async fn list_census_candidates(
        &self,
        realm_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<StoredJob>, DetachedJobError>;

    fn is_persistent(&self) -> bool;
}

#[derive(Debug, Clone, Default)]
pub struct MemoryDetachedJobStoreSnapshot {
    jobs: BTreeMap<JobId, StoredJob>,
    submission_index: BTreeMap<(String, JobSubmissionKey), JobId>,
    predicate_deliveries:
        BTreeMap<(JobId, PredicateDeliveryIdempotencyKey), PredicateDeliveryReceipt>,
}

#[derive(Debug, Default)]
struct MemoryDetachedJobStoreState {
    jobs: BTreeMap<JobId, StoredJob>,
    submission_index: BTreeMap<(String, JobSubmissionKey), JobId>,
    predicate_deliveries:
        BTreeMap<(JobId, PredicateDeliveryIdempotencyKey), PredicateDeliveryReceipt>,
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
        for ((job_id, idempotency_key), receipt) in &snapshot.predicate_deliveries {
            let job = snapshot.jobs.get(job_id).ok_or_else(|| {
                DetachedJobError::Store(format!(
                    "predicate delivery index points to missing job {job_id}"
                ))
            })?;
            validate_predicate_delivery_receipt(job, idempotency_key, receipt)?;
        }
        Ok(Self {
            inner: Arc::new(RwLock::new(MemoryDetachedJobStoreState {
                jobs: snapshot.jobs,
                submission_index: snapshot.submission_index,
                predicate_deliveries: snapshot.predicate_deliveries,
            })),
        })
    }

    pub async fn snapshot(&self) -> MemoryDetachedJobStoreSnapshot {
        let guard = self.inner.read().await;
        MemoryDetachedJobStoreSnapshot {
            jobs: guard.jobs.clone(),
            submission_index: guard.submission_index.clone(),
            predicate_deliveries: guard.predicate_deliveries.clone(),
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

    pub async fn list_for_origin(
        &self,
        realm_id: &str,
        origin_session_id: &SessionId,
        limit: usize,
    ) -> Result<Vec<StoredJob>, DetachedJobError> {
        DetachedJobStore::list_for_origin(self, realm_id, origin_session_id, limit).await
    }

    pub const fn is_persistent(&self) -> bool {
        false
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
        validate_job_replacement(current, expected_revision, &replacement)?;
        replacement.revision = next_revision(expected_revision)?;
        guard
            .jobs
            .insert(replacement.job_id.clone(), replacement.clone());
        Ok(replacement)
    }

    async fn predicate_delivery_receipt(
        &self,
        job_id: &JobId,
        identity: &PredicateDeliveryIdentity,
    ) -> Result<Option<PredicateDeliveryReceipt>, DetachedJobError> {
        let guard = self.inner.read().await;
        if !guard.jobs.contains_key(job_id) {
            return Err(DetachedJobError::NotFound(job_id.clone()));
        }
        let Some(receipt) = guard
            .predicate_deliveries
            .get(&(job_id.clone(), identity.idempotency_key().clone()))
        else {
            return Ok(None);
        };
        ensure_same_predicate_delivery_identity(job_id, &receipt.identity, identity)?;
        Ok(Some(receipt.clone()))
    }

    async fn commit_predicate_delivery(
        &self,
        expected_revision: u64,
        mut replacement: StoredJob,
        commit: PredicateDeliveryCommit,
    ) -> Result<PredicateDeliveryCommitOutcome, DetachedJobError> {
        let mut guard = self.inner.write().await;
        let ledger_key = (
            replacement.job_id.clone(),
            commit.identity.idempotency_key().clone(),
        );
        if let Some(receipt) = guard.predicate_deliveries.get(&ledger_key) {
            ensure_same_predicate_delivery_identity(
                &replacement.job_id,
                &receipt.identity,
                &commit.identity,
            )?;
            let job = guard
                .jobs
                .get(&replacement.job_id)
                .ok_or_else(|| DetachedJobError::NotFound(replacement.job_id.clone()))?
                .clone();
            return Ok(PredicateDeliveryCommitOutcome::Deduplicated {
                job,
                receipt: receipt.clone(),
            });
        }
        let current = guard
            .jobs
            .get(&replacement.job_id)
            .ok_or_else(|| DetachedJobError::NotFound(replacement.job_id.clone()))?;
        validate_job_replacement(current, expected_revision, &replacement)?;
        replacement.revision = next_revision(expected_revision)?;
        let receipt = PredicateDeliveryReceipt {
            job_id: replacement.job_id.clone(),
            identity: commit.identity,
            committed_revision: replacement.revision,
            evaluation: commit.evaluation,
            notification: commit.notification,
        };
        validate_predicate_delivery_receipt(
            &replacement,
            receipt.identity.idempotency_key(),
            &receipt,
        )?;
        guard
            .jobs
            .insert(replacement.job_id.clone(), replacement.clone());
        guard
            .predicate_deliveries
            .insert(ledger_key, receipt.clone());
        Ok(PredicateDeliveryCommitOutcome::Committed {
            job: replacement,
            receipt,
        })
    }

    async fn list_pending_outbox(
        &self,
        limit: usize,
    ) -> Result<Vec<JobOutboxEntry>, DetachedJobError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let guard = self.inner.read().await;
        Ok(guard
            .jobs
            .values()
            .flat_map(|job| job.outbox.iter())
            .filter(|entry| !entry.applied)
            .take(limit)
            .cloned()
            .collect())
    }

    async fn list_for_origin(
        &self,
        realm_id: &str,
        origin_session_id: &SessionId,
        limit: usize,
    ) -> Result<Vec<StoredJob>, DetachedJobError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        Ok(self
            .inner
            .read()
            .await
            .jobs
            .values()
            .filter(|job| {
                job.spec.realm_id == realm_id && &job.spec.origin_session_id == origin_session_id
            })
            .take(limit)
            .cloned()
            .collect())
    }

    async fn list_all(&self, limit: usize) -> Result<Vec<StoredJob>, DetachedJobError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        Ok(self
            .inner
            .read()
            .await
            .jobs
            .values()
            .take(limit)
            .cloned()
            .collect())
    }

    async fn count_pending_outbox_jobs(
        &self,
        realm_id: Option<&str>,
    ) -> Result<u64, DetachedJobError> {
        Ok(self
            .inner
            .read()
            .await
            .jobs
            .values()
            .filter(|job| realm_id.is_none_or(|realm_id| job.spec.realm_id == realm_id))
            .filter(|job| job.outbox.iter().any(|entry| !entry.applied))
            .count()
            .try_into()
            .unwrap_or(u64::MAX))
    }

    async fn list_census_candidates(
        &self,
        realm_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<StoredJob>, DetachedJobError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        Ok(self
            .inner
            .read()
            .await
            .jobs
            .values()
            .filter(|job| realm_id.is_none_or(|realm_id| job.spec.realm_id == realm_id))
            .filter(|job| job_is_census_live(job))
            .take(limit)
            .cloned()
            .collect())
    }

    fn is_persistent(&self) -> bool {
        false
    }
}

pub(crate) fn next_revision(current: u64) -> Result<u64, DetachedJobError> {
    current.checked_add(1).ok_or_else(|| {
        DetachedJobError::Store("detached job revision exhausted u64 authority".into())
    })
}

pub(crate) fn validate_job_replacement(
    current: &StoredJob,
    expected_revision: u64,
    replacement: &StoredJob,
) -> Result<(), DetachedJobError> {
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
    validate_stored_job(replacement)
}

pub(crate) fn ensure_same_predicate_delivery_identity(
    job_id: &JobId,
    committed: &PredicateDeliveryIdentity,
    requested: &PredicateDeliveryIdentity,
) -> Result<(), DetachedJobError> {
    if committed == requested {
        return Ok(());
    }
    Err(DetachedJobError::Store(format!(
        "predicate delivery key {} for job {job_id} is already bound to occurrence {} and runnable {}, not occurrence {} and runnable {}",
        requested.idempotency_key(),
        committed.occurrence_id(),
        committed.runnable(),
        requested.occurrence_id(),
        requested.runnable(),
    )))
}

pub(crate) fn validate_predicate_delivery_receipt(
    job: &StoredJob,
    idempotency_key: &PredicateDeliveryIdempotencyKey,
    receipt: &PredicateDeliveryReceipt,
) -> Result<(), DetachedJobError> {
    if receipt.job_id != job.job_id {
        return Err(DetachedJobError::Store(format!(
            "predicate delivery receipt job {} disagrees with ledger job {}",
            receipt.job_id, job.job_id
        )));
    }
    if receipt.identity.idempotency_key() != idempotency_key {
        return Err(DetachedJobError::Store(format!(
            "predicate delivery ledger key disagrees with receipt for job {}",
            job.job_id
        )));
    }
    if receipt.committed_revision == 0 || receipt.committed_revision > job.revision {
        return Err(DetachedJobError::Store(format!(
            "predicate delivery receipt revision {} is invalid for job {} at revision {}",
            receipt.committed_revision, job.job_id, job.revision
        )));
    }
    let validated_key =
        PredicateDeliveryIdempotencyKey::new(receipt.identity.idempotency_key().as_str())?;
    if &validated_key != receipt.identity.idempotency_key() {
        return Err(DetachedJobError::Store(format!(
            "predicate delivery receipt key is non-canonical for job {}",
            job.job_id
        )));
    }
    let validated = PredicateDeliveryIdentity::new(
        validated_key,
        receipt.identity.occurrence_id(),
        receipt.identity.runnable(),
    )?;
    if validated != receipt.identity {
        return Err(DetachedJobError::Store(format!(
            "predicate delivery receipt identity is non-canonical for job {}",
            job.job_id
        )));
    }
    Ok(())
}

pub(crate) fn validate_stored_job(job: &StoredJob) -> Result<(), DetachedJobError> {
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
    let mut subscription_ids = BTreeSet::new();
    for subscription in &job.subscriptions {
        if !subscription_ids.insert(subscription.subscription_id().as_str()) {
            return Err(DetachedJobError::Store(format!(
                "job {} contains duplicate active subscription {}",
                job.job_id,
                subscription.subscription_id()
            )));
        }
    }
    let mut sequences = BTreeSet::new();
    for entry in &job.outbox {
        if entry.job_id != job.job_id
            || entry.delivery_sequence == 0
            || entry.delivery_sequence > state.delivery_sequence
            || !sequences.insert(entry.delivery_sequence)
        {
            return Err(DetachedJobError::Store(format!(
                "job {} outbox identity or delivery sequence disagrees with generated authority",
                job.job_id
            )));
        }
        let mut target_ids = BTreeSet::new();
        for target in &entry.targets {
            if !target_ids.insert(target.subscription_id().as_str()) {
                return Err(DetachedJobError::Store(format!(
                    "job {} delivery {} contains duplicate subscription target {}",
                    job.job_id,
                    entry.delivery_sequence,
                    target.subscription_id()
                )));
            }
        }
    }
    if job.outbox.len() != usize::try_from(state.delivery_sequence).unwrap_or(usize::MAX) {
        return Err(DetachedJobError::Store(format!(
            "job {} outbox cardinality disagrees with generated delivery sequence",
            job.job_id
        )));
    }

    let notifications = job
        .outbox
        .iter()
        .filter_map(|entry| match &entry.payload {
            JobOutboxPayload::Notification(notification) => Some((entry, notification)),
            JobOutboxPayload::Terminal(_) => None,
        })
        .collect::<Vec<_>>();
    if notifications.len() != state.notification_ids.len() {
        return Err(DetachedJobError::Store(format!(
            "job {} notification outbox cardinality disagrees with generated authority",
            job.job_id
        )));
    }
    for (entry, notification) in notifications {
        let notification_id = notification.notification_id().as_str();
        let idempotency_key = notification.idempotency_key();
        if entry.delivery_id != notification_id
            || !state.notification_ids.contains(notification_id)
            || !state
                .notification_idempotency_keys
                .contains(idempotency_key)
            || state
                .notification_id_by_key
                .get(idempotency_key)
                .map(String::as_str)
                != Some(notification_id)
            || state
                .notification_delivery_ids
                .get(notification_id)
                .map(String::as_str)
                != Some(entry.runtime_delivery_id().as_str())
            || state.notification_sequences.get(notification_id).copied()
                != Some(entry.delivery_sequence)
            || state.notification_applied.contains(notification_id) != entry.applied
        {
            return Err(DetachedJobError::Store(format!(
                "job {} notification outbox projection disagrees with generated authority",
                job.job_id
            )));
        }
    }

    let terminal_entries = job
        .outbox
        .iter()
        .filter_map(|entry| match &entry.payload {
            JobOutboxPayload::Terminal(result) => Some((entry, result)),
            JobOutboxPayload::Notification(_) => None,
        })
        .collect::<Vec<_>>();
    match (
        state.terminal_kind,
        &job.terminal_result,
        terminal_entries.as_slice(),
    ) {
        (None, None, []) => {}
        (Some(kind), Some(result), [(entry, payload)])
            if result.kind() == kind
                && *payload == result
                && entry.delivery_id == "terminal"
                && entry.delivery_sequence == state.terminal_delivery_sequence
                && entry.applied == state.terminal_delivery_applied => {}
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
