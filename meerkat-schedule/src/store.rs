use crate::error::ScheduleStoreError;
use crate::lifecycle::{
    AuthorizedOccurrenceWrite, AuthorizedScheduleWrite, OccurrenceDueAction,
    OccurrenceLifecycleEffect, OccurrenceLifecycleError, OccurrenceLifecycleInput,
    OccurrenceLifecycleMutator, OccurrenceSupersessionAck, ScheduleLifecycleInput,
};
use crate::types::{
    DeliveryReceipt, Occurrence, OccurrenceId, OccurrencePhase, RuntimeDeliveryOutcome, Schedule,
    ScheduleId, SchedulePhase, ScheduleRevision,
};
use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use std::collections::{BTreeMap, BTreeSet};
use std::ops::Bound::{Excluded, Unbounded};
use std::sync::Arc;
use uuid::Uuid;

#[cfg(target_arch = "wasm32")]
use crate::tokio::sync::RwLock;
#[cfg(not(target_arch = "wasm32"))]
use tokio::sync::RwLock;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimDueRequest {
    pub owner_id: String,
    pub limit: usize,
    pub lease_duration: Duration,
}

#[derive(Debug, Clone)]
pub struct ClaimDueResult {
    pub store_now_utc: DateTime<Utc>,
    pub claimed: Vec<Occurrence>,
    /// Durable rows the claim scan skipped instead of failing wholesale:
    /// each skip is a typed, attributable fault (per-row tolerance), never a
    /// silent drop. The rows stay in the store for inspection and repair.
    pub row_faults: Vec<ScheduleStoreRowFault>,
}

/// Store-clock witness for host pacing. `next_action_at_utc` is the earliest
/// instant at which durable schedule work can be required: a pending due
/// time, an in-flight lease expiry, or an active schedule's refill deadline.
/// It is a mechanical index projection only; the generated schedule and
/// occurrence machines remain the eligibility authorities when rows are
/// actually processed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScheduleStoreActionTime {
    pub store_now_utc: DateTime<Utc>,
    pub next_action_at_utc: Option<DateTime<Utc>>,
}

/// How a schedule host learns that durable work created outside this process
/// may be available.
///
/// This is a required store declaration rather than host-side backend
/// guessing. A remote/custom store must deliberately choose its cost and
/// consistency contract: either provide a cancellation-safe push wait or
/// accept a bounded polling cadence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScheduleStoreWakeMode {
    /// The store is process-local. [`crate::ScheduleService`]'s mutation
    /// signal and exact next-action timer are sufficient; no durability poll
    /// is needed.
    ProcessLocal,
    /// The store can be written by another process and has no push primitive.
    /// While otherwise idle, the host waits up to `max_interval` between
    /// polls; a known earlier action deadline shortens that wait.
    BoundedPoll { max_interval: std::time::Duration },
    /// The store provides [`ScheduleStore::wait_for_durable_wake`].
    Push,
}

/// One schedule whose durable refill projection says planning work is due.
///
/// `refill_at_utc` is a CAS token, not semantic authority. The schedule
/// machine and trigger engine still decide what, if anything, to plan.
#[derive(Debug, Clone)]
pub struct ScheduleRefillCandidate {
    pub schedule: Schedule,
    pub pending_occurrences: Vec<Occurrence>,
    pub refill_at_utc: DateTime<Utc>,
}

/// Bounded store-clock refill page. Durable stores advance a mechanical
/// keyset cursor across poisoned rows so one bad payload cannot become a
/// permanent head-of-queue wall.
#[derive(Debug, Clone)]
pub struct ScheduleRefillBatch {
    pub store_now_utc: DateTime<Utc>,
    pub candidates: Vec<ScheduleRefillCandidate>,
    pub row_faults: Vec<ScheduleStoreRowFault>,
}

/// Exact durable claim witness used by lease renewal. The store samples its
/// own clock in the same critical section/transaction that screens this
/// evidence and commits the machine-authorized extension.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenewOccurrenceLeaseRequest {
    pub occurrence_id: OccurrenceId,
    pub expected_attempt: u32,
    pub claim_token: Uuid,
    pub expected_owner_id: String,
    pub lease_duration: Duration,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RenewOccurrenceLeaseOutcome {
    Renewed(Occurrence),
    StaleClaim,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RenewOccurrenceLeaseResult {
    pub store_now_utc: DateTime<Utc>,
    pub outcome: RenewOccurrenceLeaseOutcome,
}

/// A durable schedule/occurrence row a tolerant store scan could not surface
/// as a typed value. The scan skips the row so its neighbors stay
/// serviceable and reports the skip so it is never silent: one poisoned row
/// must not starve every schedule, and no skip may go unobserved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduleStoreRowFault {
    /// Owning schedule id as stored in the row's indexed column (readable
    /// even when the row payload itself cannot be deserialized).
    pub schedule_id: Option<String>,
    /// Occurrence id as stored in the row's indexed column, when the faulted
    /// row is an occurrence row.
    pub occurrence_id: Option<String>,
    pub kind: ScheduleStoreRowFaultKind,
    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScheduleStoreRowFaultKind {
    /// The persisted row failed typed deserialization or generated-machine
    /// recovery.
    Deserialization,
    /// The row deserialized but the machine-owned due classification (or the
    /// lifecycle transition realizing its verdict) refused.
    DueClassification,
}

impl std::fmt::Display for ScheduleStoreRowFault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{kind:?} fault (schedule={schedule}, occurrence={occurrence}): {detail}",
            kind = self.kind,
            schedule = self.schedule_id.as_deref().unwrap_or("?"),
            occurrence = self.occurrence_id.as_deref().unwrap_or("-"),
            detail = self.detail,
        )
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScheduleFilter {
    pub phase: Option<SchedulePhase>,
    pub include_deleted: bool,
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OccurrenceFilter {
    pub schedule_id: Option<ScheduleId>,
    pub phase: Option<OccurrencePhase>,
    pub include_terminal: bool,
    pub due_after_utc: Option<DateTime<Utc>>,
    pub due_before_utc: Option<DateTime<Utc>>,
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingSupersession {
    at_utc: DateTime<Utc>,
    superseded_by_revision: ScheduleRevision,
}

impl PendingSupersession {
    pub(crate) fn from_schedule_effect(effect: &crate::ScheduleLifecycleEffect) -> Option<Self> {
        if let crate::ScheduleLifecycleEffect::SupersedePendingOccurrences {
            superseding_revision,
            at_utc,
        } = effect
        {
            Some(Self {
                at_utc: *at_utc,
                superseded_by_revision: *superseding_revision,
            })
        } else {
            None
        }
    }

    pub fn at_utc(&self) -> DateTime<Utc> {
        self.at_utc
    }

    pub fn superseded_by_revision(&self) -> ScheduleRevision {
        self.superseded_by_revision
    }
}

pub fn apply_supersession_feedback(
    mut schedule: Schedule,
    acks: Vec<OccurrenceSupersessionAck>,
) -> Result<Schedule, ScheduleStoreError> {
    for ack in acks {
        schedule = Schedule::apply(
            Some(schedule),
            ScheduleLifecycleInput::ConfirmOccurrencesSuperseded { ack },
        )
        .map_err(|error| ScheduleStoreError::Internal(error.to_string()))?
        .into_schedule();
    }
    Ok(schedule)
}

#[derive(Debug, Clone)]
pub(crate) struct ExpiredOccurrenceLease {
    pub(crate) occurrence: Occurrence,
    pub(crate) receipt: DeliveryReceipt,
}

pub(crate) fn expire_occurrence_lease(
    occurrence: Occurrence,
    at_utc: DateTime<Utc>,
) -> Result<ExpiredOccurrenceLease, OccurrenceLifecycleError> {
    let expired = occurrence
        .apply(OccurrenceLifecycleInput::LeaseExpired { at_utc })?
        .into_occurrence();
    let receipt = expired.delivery_receipt_from_authority(None)?;
    let expired = expired
        .apply(OccurrenceLifecycleInput::RecordReceipt {
            runtime_outcome: receipt.runtime_outcome.clone(),
            receipt: receipt.clone(),
        })?
        .into_occurrence();
    Ok(ExpiredOccurrenceLease {
        occurrence: expired,
        receipt,
    })
}

pub(crate) fn claim_occurrence(
    occurrence: Occurrence,
    request: &ClaimDueRequest,
    at_utc: DateTime<Utc>,
) -> Result<Occurrence, OccurrenceLifecycleError> {
    occurrence
        .apply(OccurrenceLifecycleInput::Claim {
            owner_id: request.owner_id.clone(),
            at_utc,
            lease_expires_at_utc: at_utc + request.lease_duration,
            claim_token: Uuid::now_v7(),
        })
        .map(OccurrenceLifecycleMutator::into_occurrence)
}

pub(crate) fn renew_occurrence_lease(
    occurrence: Occurrence,
    request: &RenewOccurrenceLeaseRequest,
    at_utc: DateTime<Utc>,
) -> Result<Occurrence, OccurrenceLifecycleError> {
    occurrence
        .apply(OccurrenceLifecycleInput::RenewLease {
            claim_token: request.claim_token,
            lease_expires_at_utc: at_utc + request.lease_duration,
            at_utc,
        })
        .map(OccurrenceLifecycleMutator::into_occurrence)
}

pub(crate) fn occurrence_next_action_at(occurrence: &Occurrence) -> Option<DateTime<Utc>> {
    match occurrence.phase {
        OccurrencePhase::Pending => Some(occurrence.due_at_utc),
        OccurrencePhase::Claimed
        | OccurrencePhase::Dispatching
        | OccurrencePhase::AwaitingCompletion => occurrence.lease_expires_at_utc,
        OccurrencePhase::Completed
        | OccurrencePhase::Skipped
        | OccurrencePhase::Misfired
        | OccurrencePhase::Superseded
        | OccurrencePhase::DeliveryFailed => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScheduleStoreKind {
    Disabled,
    Memory,
    Jsonl,
    Sqlite,
    Custom,
}

impl ScheduleStoreKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::Memory => "memory",
            Self::Jsonl => "jsonl",
            Self::Sqlite => "sqlite",
            Self::Custom => "custom",
        }
    }
}

impl std::fmt::Display for ScheduleStoreKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[async_trait]
pub trait ScheduleStore: Send + Sync {
    fn kind(&self) -> ScheduleStoreKind;

    /// Declare how cross-process durable mutations wake a schedule host.
    ///
    /// There is intentionally no default: custom stores must explicitly
    /// choose push or bounded polling instead of silently inheriting a
    /// query-heavy host loop.
    fn wake_mode(&self) -> ScheduleStoreWakeMode;

    /// Wait until durable schedule work may have changed.
    ///
    /// Hosts call this only when [`Self::wake_mode`] is
    /// [`ScheduleStoreWakeMode::Push`]. The future must be cancellation-safe:
    /// host timers, local mutations, and shutdown can cancel and recreate it.
    /// A successful return consumes one notification; the future must not stay
    /// ready merely because previously known work still exists. Notifications
    /// may be coalesced and are advisory; store reads remain authoritative.
    async fn wait_for_durable_wake(&self) -> Result<(), ScheduleStoreError>;

    async fn get_store_time_utc(&self) -> Result<DateTime<Utc>, ScheduleStoreError>;

    async fn next_action_time_utc(&self) -> Result<ScheduleStoreActionTime, ScheduleStoreError>;

    /// Read one bounded page from the durable refill-work projection.
    ///
    /// Every backend must implement this explicitly. A list-based default
    /// would silently recreate the O(all schedules) production defect for
    /// custom/remote stores. `store_now_utc`, each schedule row, its refill
    /// token, and the complete set of current-revision Pending occurrences
    /// must come from one coherent store snapshot. Poisoned payloads are
    /// reported as row faults and must not pin the page cursor forever.
    async fn read_due_refill_candidates(
        &self,
        limit: usize,
    ) -> Result<ScheduleRefillBatch, ScheduleStoreError>;

    async fn commit_schedule_write(
        &self,
        write: AuthorizedScheduleWrite,
    ) -> Result<(), ScheduleStoreError>;

    async fn get_schedule(
        &self,
        schedule_id: &ScheduleId,
    ) -> Result<Option<Schedule>, ScheduleStoreError>;

    async fn list_schedules(
        &self,
        filter: ScheduleFilter,
    ) -> Result<Vec<Schedule>, ScheduleStoreError>;

    /// List schedules with per-row tolerance: rows that fail typed
    /// deserialization/recovery are skipped and surfaced as typed faults so
    /// one poisoned row cannot fail the listing wholesale. The default keeps
    /// the store's strict behavior (a wholesale error, zero faults); stores
    /// with row-granular durable storage override it.
    async fn list_schedules_with_row_faults(
        &self,
        filter: ScheduleFilter,
    ) -> Result<(Vec<Schedule>, Vec<ScheduleStoreRowFault>), ScheduleStoreError> {
        Ok((self.list_schedules(filter).await?, Vec::new()))
    }

    async fn commit_occurrence_write(
        &self,
        write: AuthorizedOccurrenceWrite,
    ) -> Result<(), ScheduleStoreError>;

    async fn commit_occurrence_writes(
        &self,
        writes: Vec<AuthorizedOccurrenceWrite>,
    ) -> Result<(), ScheduleStoreError> {
        for write in writes {
            self.commit_occurrence_write(write).await?;
        }
        Ok(())
    }

    async fn commit_schedule_mutation(
        &self,
        schedule: AuthorizedScheduleWrite,
        occurrences: Vec<AuthorizedOccurrenceWrite>,
    ) -> Result<Schedule, ScheduleStoreError>;

    /// Atomically commit a machine-authorized planning mutation and its exact
    /// next mechanical refill deadline. Backends must also make active
    /// create/resume/revision changes due for planning and re-enqueue an
    /// active schedule when a current-revision Pending occurrence leaves
    /// Pending; otherwise a cleared deadline can lose work.
    async fn commit_schedule_refill(
        &self,
        schedule: AuthorizedScheduleWrite,
        occurrences: Vec<AuthorizedOccurrenceWrite>,
        next_refill_at_utc: Option<DateTime<Utc>>,
    ) -> Result<Schedule, ScheduleStoreError>;

    /// CAS-acknowledge a refill candidate that produced no canonical schedule
    /// mutation. The expected durable deadline prevents a stale planner from
    /// overwriting another host's newer plan at the same schedule revision.
    async fn record_refill_deadline_if_current(
        &self,
        schedule_id: &ScheduleId,
        expected_revision: ScheduleRevision,
        expected_refill_at_utc: DateTime<Utc>,
        next_refill_at_utc: Option<DateTime<Utc>>,
    ) -> Result<(), ScheduleStoreError>;

    async fn get_occurrence(
        &self,
        occurrence_id: &OccurrenceId,
    ) -> Result<Option<Occurrence>, ScheduleStoreError>;

    async fn list_occurrences(
        &self,
        filter: OccurrenceFilter,
    ) -> Result<Vec<Occurrence>, ScheduleStoreError>;

    async fn append_receipt(&self, receipt: DeliveryReceipt) -> Result<(), ScheduleStoreError>;

    async fn list_receipts(
        &self,
        occurrence_id: &OccurrenceId,
    ) -> Result<Vec<DeliveryReceipt>, ScheduleStoreError>;

    async fn claim_due_occurrences(
        &self,
        request: ClaimDueRequest,
    ) -> Result<ClaimDueResult, ScheduleStoreError>;

    async fn renew_occurrence_lease_if_current(
        &self,
        request: RenewOccurrenceLeaseRequest,
    ) -> Result<RenewOccurrenceLeaseResult, ScheduleStoreError>;

    /// Attempt to apply `transition` to the occurrence identified by
    /// `occurrence_id`, gated on the claim evidence matching the durable row.
    /// Returns `None` when the claim evidence is stale (attempt/token mismatch)
    /// so callers can feed a `ClassifyStaleCompletionArrival` input. Returns
    /// the updated occurrence together with the emitted effects so callers can
    /// inspect `LateCompletionResolutionRecorded` without a second read.
    async fn transition_occurrence_if_current(
        &self,
        occurrence_id: &OccurrenceId,
        expected_attempt: u32,
        expected_claim_token: Option<Uuid>,
        transition: OccurrenceLifecycleInput,
    ) -> Result<Option<(Occurrence, Vec<OccurrenceLifecycleEffect>)>, ScheduleStoreError>;

    async fn transition_occurrence_with_receipt_if_current(
        &self,
        occurrence_id: &OccurrenceId,
        expected_attempt: u32,
        expected_claim_token: Option<Uuid>,
        transition: OccurrenceLifecycleInput,
        runtime_outcome: Option<RuntimeDeliveryOutcome>,
    ) -> Result<Option<Occurrence>, ScheduleStoreError>;
}

#[derive(Default)]
pub struct DisabledScheduleStore;

#[async_trait]
impl ScheduleStore for DisabledScheduleStore {
    fn kind(&self) -> ScheduleStoreKind {
        ScheduleStoreKind::Disabled
    }

    fn wake_mode(&self) -> ScheduleStoreWakeMode {
        ScheduleStoreWakeMode::ProcessLocal
    }

    async fn wait_for_durable_wake(&self) -> Result<(), ScheduleStoreError> {
        Err(ScheduleStoreError::DurableWakeUnsupported {
            backend: self.kind(),
        })
    }

    async fn get_store_time_utc(&self) -> Result<DateTime<Utc>, ScheduleStoreError> {
        Err(unsupported(self.kind()))
    }

    async fn next_action_time_utc(&self) -> Result<ScheduleStoreActionTime, ScheduleStoreError> {
        Err(unsupported(self.kind()))
    }

    async fn read_due_refill_candidates(
        &self,
        _limit: usize,
    ) -> Result<ScheduleRefillBatch, ScheduleStoreError> {
        Err(unsupported(self.kind()))
    }

    async fn commit_schedule_write(
        &self,
        _write: AuthorizedScheduleWrite,
    ) -> Result<(), ScheduleStoreError> {
        Err(unsupported(self.kind()))
    }

    async fn get_schedule(
        &self,
        _schedule_id: &ScheduleId,
    ) -> Result<Option<Schedule>, ScheduleStoreError> {
        Err(unsupported(self.kind()))
    }

    async fn list_schedules(
        &self,
        _filter: ScheduleFilter,
    ) -> Result<Vec<Schedule>, ScheduleStoreError> {
        Err(unsupported(self.kind()))
    }

    async fn commit_occurrence_write(
        &self,
        _write: AuthorizedOccurrenceWrite,
    ) -> Result<(), ScheduleStoreError> {
        Err(unsupported(self.kind()))
    }

    async fn commit_schedule_mutation(
        &self,
        _schedule: AuthorizedScheduleWrite,
        _occurrences: Vec<AuthorizedOccurrenceWrite>,
    ) -> Result<Schedule, ScheduleStoreError> {
        Err(unsupported(self.kind()))
    }

    async fn commit_schedule_refill(
        &self,
        _schedule: AuthorizedScheduleWrite,
        _occurrences: Vec<AuthorizedOccurrenceWrite>,
        _next_refill_at_utc: Option<DateTime<Utc>>,
    ) -> Result<Schedule, ScheduleStoreError> {
        Err(unsupported(self.kind()))
    }

    async fn record_refill_deadline_if_current(
        &self,
        _schedule_id: &ScheduleId,
        _expected_revision: ScheduleRevision,
        _expected_refill_at_utc: DateTime<Utc>,
        _next_refill_at_utc: Option<DateTime<Utc>>,
    ) -> Result<(), ScheduleStoreError> {
        Err(unsupported(self.kind()))
    }

    async fn get_occurrence(
        &self,
        _occurrence_id: &OccurrenceId,
    ) -> Result<Option<Occurrence>, ScheduleStoreError> {
        Err(unsupported(self.kind()))
    }

    async fn list_occurrences(
        &self,
        _filter: OccurrenceFilter,
    ) -> Result<Vec<Occurrence>, ScheduleStoreError> {
        Err(unsupported(self.kind()))
    }

    async fn append_receipt(&self, _receipt: DeliveryReceipt) -> Result<(), ScheduleStoreError> {
        Err(unsupported(self.kind()))
    }

    async fn list_receipts(
        &self,
        _occurrence_id: &OccurrenceId,
    ) -> Result<Vec<DeliveryReceipt>, ScheduleStoreError> {
        Err(unsupported(self.kind()))
    }

    async fn claim_due_occurrences(
        &self,
        _request: ClaimDueRequest,
    ) -> Result<ClaimDueResult, ScheduleStoreError> {
        Err(unsupported(self.kind()))
    }

    async fn renew_occurrence_lease_if_current(
        &self,
        _request: RenewOccurrenceLeaseRequest,
    ) -> Result<RenewOccurrenceLeaseResult, ScheduleStoreError> {
        Err(unsupported(self.kind()))
    }

    async fn transition_occurrence_if_current(
        &self,
        _occurrence_id: &OccurrenceId,
        _expected_attempt: u32,
        _expected_claim_token: Option<Uuid>,
        _transition: OccurrenceLifecycleInput,
    ) -> Result<Option<(Occurrence, Vec<OccurrenceLifecycleEffect>)>, ScheduleStoreError> {
        Err(unsupported(self.kind()))
    }

    async fn transition_occurrence_with_receipt_if_current(
        &self,
        _occurrence_id: &OccurrenceId,
        _expected_attempt: u32,
        _expected_claim_token: Option<Uuid>,
        _transition: OccurrenceLifecycleInput,
        _runtime_outcome: Option<RuntimeDeliveryOutcome>,
    ) -> Result<Option<Occurrence>, ScheduleStoreError> {
        Err(unsupported(self.kind()))
    }
}

#[derive(Default)]
pub struct MemoryScheduleStore {
    inner: Arc<RwLock<MemoryScheduleState>>,
}

#[derive(Default)]
struct MemoryScheduleState {
    schedules: BTreeMap<ScheduleId, Schedule>,
    occurrences: BTreeMap<OccurrenceId, Occurrence>,
    receipts: BTreeMap<OccurrenceId, Vec<DeliveryReceipt>>,
    refill_deadlines: BTreeMap<ScheduleId, DateTime<Utc>>,
    refill_queue: BTreeSet<(DateTime<Utc>, ScheduleId)>,
    refill_scan_cursor: Option<(DateTime<Utc>, ScheduleId)>,
}

fn memory_set_schedule_refill_deadline(
    state: &mut MemoryScheduleState,
    schedule_id: &ScheduleId,
    deadline: Option<DateTime<Utc>>,
) {
    if let Some(previous) = state.refill_deadlines.remove(schedule_id) {
        state.refill_queue.remove(&(previous, schedule_id.clone()));
    }
    if let Some(deadline) = deadline {
        state.refill_deadlines.insert(schedule_id.clone(), deadline);
        state.refill_queue.insert((deadline, schedule_id.clone()));
    }
}

fn memory_update_schedule_refill_projection(
    state: &mut MemoryScheduleState,
    previous: Option<&Schedule>,
    schedule: &Schedule,
    exact_deadline: Option<Option<DateTime<Utc>>>,
) {
    if schedule.phase != SchedulePhase::Active {
        memory_set_schedule_refill_deadline(state, &schedule.schedule_id, None);
        return;
    }
    if let Some(deadline) = exact_deadline {
        memory_set_schedule_refill_deadline(state, &schedule.schedule_id, deadline);
        return;
    }
    let must_enqueue = previous.is_none_or(|previous| {
        previous.phase != SchedulePhase::Active || previous.revision != schedule.revision
    });
    if must_enqueue || !state.refill_deadlines.contains_key(&schedule.schedule_id) {
        memory_set_schedule_refill_deadline(state, &schedule.schedule_id, Some(Utc::now()));
    }
}

fn memory_note_pending_departure(
    state: &mut MemoryScheduleState,
    previous: &Occurrence,
    updated: Option<&Occurrence>,
    store_now_utc: DateTime<Utc>,
) {
    if previous.phase != OccurrencePhase::Pending
        || updated.is_some_and(|updated| {
            updated.phase == OccurrencePhase::Pending
                && updated.schedule_id == previous.schedule_id
                && updated.schedule_revision == previous.schedule_revision
        })
    {
        return;
    }
    let should_enqueue = state
        .schedules
        .get(&previous.schedule_id)
        .is_some_and(|schedule| {
            schedule.phase == SchedulePhase::Active
                && schedule.revision == previous.schedule_revision
        });
    if should_enqueue {
        let deadline = state
            .refill_deadlines
            .get(&previous.schedule_id)
            .copied()
            .unwrap_or(store_now_utc)
            .min(store_now_utc);
        memory_set_schedule_refill_deadline(state, &previous.schedule_id, Some(deadline));
    }
}

impl MemoryScheduleStore {
    pub fn new() -> Self {
        Self::default()
    }

    async fn commit_schedule_mutation_with_refill(
        &self,
        schedule: AuthorizedScheduleWrite,
        occurrences: Vec<AuthorizedOccurrenceWrite>,
        exact_refill_deadline: Option<Option<DateTime<Utc>>>,
    ) -> Result<Schedule, ScheduleStoreError> {
        let mut state = self.inner.write().await;
        let previous_schedule = state.schedules.get(schedule.schedule_id()).cloned();
        schedule
            .precondition()
            .check_current(state.schedules.get(schedule.schedule_id()))
            .map_err(ScheduleStoreError::Concurrency)?;
        for occurrence in &occurrences {
            occurrence
                .precondition()
                .check_current(state.occurrences.get(occurrence.occurrence_id()))
                .map_err(ScheduleStoreError::Concurrency)?;
        }
        let (schedule, supersession) = schedule.into_parts();
        let mut committed_schedule = schedule;
        committed_schedule
            .validate_machine_projection()
            .map_err(ScheduleStoreError::Internal)?;
        state.schedules.insert(
            committed_schedule.schedule_id.clone(),
            committed_schedule.clone(),
        );
        for occurrence in occurrences {
            let previous = state.occurrences.get(occurrence.occurrence_id()).cloned();
            let occurrence = occurrence.into_occurrence();
            occurrence
                .validate_machine_projection()
                .map_err(ScheduleStoreError::Internal)?;
            if let Some(previous) = previous.as_ref() {
                memory_note_pending_departure(&mut state, previous, Some(&occurrence), Utc::now());
            }
            state
                .occurrences
                .insert(occurrence.occurrence_id.clone(), occurrence);
        }
        let mut occurrence_acks = Vec::new();
        if let Some(supersession) = supersession {
            let occurrence_ids: Vec<OccurrenceId> = state
                .occurrences
                .values()
                .filter(|occurrence| {
                    occurrence.schedule_id == committed_schedule.schedule_id
                        && !occurrence.is_terminal()
                        && occurrence.schedule_revision < supersession.superseded_by_revision()
                })
                .map(|occurrence| occurrence.occurrence_id.clone())
                .collect();
            for occurrence_id in occurrence_ids {
                let current = state
                    .occurrences
                    .get(&occurrence_id)
                    .cloned()
                    .ok_or_else(|| {
                        ScheduleStoreError::Internal(format!(
                            "occurrence {occurrence_id} disappeared during supersession sweep"
                        ))
                    })?;
                let mutator = current
                    .clone()
                    .apply(OccurrenceLifecycleInput::Supersede {
                        superseded_by_revision: supersession.superseded_by_revision(),
                        at_utc: supersession.at_utc(),
                    })
                    .map_err(|error| ScheduleStoreError::Internal(error.to_string()))?;
                let (updated, _effects, acks) = mutator.into_parts_with_supersession_feedback();
                // The commit-time sweep is the sole receipt minter for
                // supersession (0.7.2 D1). Mint exactly one superseded receipt
                // per swept row; later driver paths that encounter an already-
                // Superseded occurrence must not mint a second one.
                let receipt = updated
                    .delivery_receipt_from_authority(None)
                    .map_err(|error| ScheduleStoreError::Internal(error.to_string()))?;
                state
                    .receipts
                    .entry(updated.occurrence_id.clone())
                    .or_default()
                    .push(receipt);
                occurrence_acks.extend(acks);
                memory_note_pending_departure(
                    &mut state,
                    &current,
                    Some(&updated),
                    supersession.at_utc(),
                );
                state
                    .occurrences
                    .insert(updated.occurrence_id.clone(), updated);
            }
        }
        committed_schedule = apply_supersession_feedback(committed_schedule, occurrence_acks)?;
        state.schedules.insert(
            committed_schedule.schedule_id.clone(),
            committed_schedule.clone(),
        );
        memory_update_schedule_refill_projection(
            &mut state,
            previous_schedule.as_ref(),
            &committed_schedule,
            exact_refill_deadline,
        );
        Ok(committed_schedule)
    }
}

#[async_trait]
impl ScheduleStore for MemoryScheduleStore {
    fn kind(&self) -> ScheduleStoreKind {
        ScheduleStoreKind::Memory
    }

    fn wake_mode(&self) -> ScheduleStoreWakeMode {
        ScheduleStoreWakeMode::ProcessLocal
    }

    async fn wait_for_durable_wake(&self) -> Result<(), ScheduleStoreError> {
        Err(ScheduleStoreError::DurableWakeUnsupported {
            backend: self.kind(),
        })
    }

    async fn get_store_time_utc(&self) -> Result<DateTime<Utc>, ScheduleStoreError> {
        Ok(Utc::now())
    }

    async fn next_action_time_utc(&self) -> Result<ScheduleStoreActionTime, ScheduleStoreError> {
        let store_now_utc = Utc::now();
        let state = self.inner.read().await;
        let next_occurrence_action_at_utc = state
            .occurrences
            .values()
            .filter(|occurrence| {
                state
                    .schedules
                    .get(&occurrence.schedule_id)
                    .is_some_and(|schedule| schedule.phase == SchedulePhase::Active)
            })
            .filter_map(occurrence_next_action_at)
            .min();
        let next_refill_at_utc = state.refill_queue.first().map(|(deadline, _)| *deadline);
        let next_action_at_utc = match (next_occurrence_action_at_utc, next_refill_at_utc) {
            (Some(occurrence), Some(refill)) => Some(occurrence.min(refill)),
            (Some(occurrence), None) => Some(occurrence),
            (None, Some(refill)) => Some(refill),
            (None, None) => None,
        };
        Ok(ScheduleStoreActionTime {
            store_now_utc,
            next_action_at_utc,
        })
    }

    async fn read_due_refill_candidates(
        &self,
        limit: usize,
    ) -> Result<ScheduleRefillBatch, ScheduleStoreError> {
        let store_now_utc = Utc::now();
        if limit == 0 {
            return Ok(ScheduleRefillBatch {
                store_now_utc,
                candidates: Vec::new(),
                row_faults: Vec::new(),
            });
        }
        let mut state = self.inner.write().await;
        let mut due = if let Some(cursor) = state.refill_scan_cursor.as_ref() {
            state
                .refill_queue
                .range((Excluded(cursor.clone()), Unbounded))
                .take_while(|(deadline, _)| *deadline <= store_now_utc)
                .take(limit)
                .cloned()
                .collect::<Vec<_>>()
        } else {
            state
                .refill_queue
                .iter()
                .take_while(|(deadline, _)| *deadline <= store_now_utc)
                .take(limit)
                .cloned()
                .collect::<Vec<_>>()
        };
        if due.len() < limit
            && let Some(cursor) = state.refill_scan_cursor.as_ref()
        {
            let remaining = limit - due.len();
            due.extend(
                state
                    .refill_queue
                    .range(..=cursor.clone())
                    .take_while(|(deadline, _)| *deadline <= store_now_utc)
                    .take(remaining)
                    .cloned(),
            );
        }
        state.refill_scan_cursor = due.last().cloned();

        let candidates = due
            .into_iter()
            .filter_map(|(refill_at_utc, schedule_id)| {
                let schedule = state.schedules.get(&schedule_id)?.clone();
                if schedule.phase != SchedulePhase::Active {
                    return None;
                }
                let pending_occurrences = state
                    .occurrences
                    .values()
                    .filter(|occurrence| {
                        occurrence.schedule_id == schedule_id
                            && occurrence.schedule_revision == schedule.revision
                            && occurrence.phase == OccurrencePhase::Pending
                    })
                    .cloned()
                    .collect();
                Some(ScheduleRefillCandidate {
                    schedule,
                    pending_occurrences,
                    refill_at_utc,
                })
            })
            .collect();
        Ok(ScheduleRefillBatch {
            store_now_utc,
            candidates,
            row_faults: Vec::new(),
        })
    }

    async fn commit_schedule_write(
        &self,
        write: AuthorizedScheduleWrite,
    ) -> Result<(), ScheduleStoreError> {
        reject_standalone_supersession_write(&write)?;
        let mut state = self.inner.write().await;
        write
            .precondition()
            .check_current(state.schedules.get(write.schedule_id()))
            .map_err(ScheduleStoreError::Concurrency)?;
        let previous = state.schedules.get(write.schedule_id()).cloned();
        let schedule = write.into_schedule();
        schedule
            .validate_machine_projection()
            .map_err(ScheduleStoreError::Internal)?;
        memory_update_schedule_refill_projection(&mut state, previous.as_ref(), &schedule, None);
        state
            .schedules
            .insert(schedule.schedule_id.clone(), schedule);
        Ok(())
    }

    async fn get_schedule(
        &self,
        schedule_id: &ScheduleId,
    ) -> Result<Option<Schedule>, ScheduleStoreError> {
        Ok(self.inner.read().await.schedules.get(schedule_id).cloned())
    }

    async fn list_schedules(
        &self,
        filter: ScheduleFilter,
    ) -> Result<Vec<Schedule>, ScheduleStoreError> {
        let mut schedules: Vec<Schedule> = self
            .inner
            .read()
            .await
            .schedules
            .values()
            .filter(|schedule| {
                (filter.include_deleted || schedule.phase != SchedulePhase::Deleted)
                    && filter.phase.is_none_or(|phase| schedule.phase == phase)
            })
            .cloned()
            .collect();
        schedules
            .sort_by_key(|schedule| (schedule.config.created_at_utc, schedule.schedule_id.clone()));
        if let Some(limit) = filter.limit {
            schedules.truncate(limit);
        }
        Ok(schedules)
    }

    async fn commit_occurrence_write(
        &self,
        write: AuthorizedOccurrenceWrite,
    ) -> Result<(), ScheduleStoreError> {
        let mut state = self.inner.write().await;
        write
            .precondition()
            .check_current(state.occurrences.get(write.occurrence_id()))
            .map_err(ScheduleStoreError::Concurrency)?;
        let previous = state.occurrences.get(write.occurrence_id()).cloned();
        let occurrence = write.into_occurrence();
        occurrence
            .validate_machine_projection()
            .map_err(ScheduleStoreError::Internal)?;
        if let Some(previous) = previous.as_ref() {
            memory_note_pending_departure(&mut state, previous, Some(&occurrence), Utc::now());
        }
        state
            .occurrences
            .insert(occurrence.occurrence_id.clone(), occurrence);
        Ok(())
    }

    async fn commit_schedule_mutation(
        &self,
        schedule: AuthorizedScheduleWrite,
        occurrences: Vec<AuthorizedOccurrenceWrite>,
    ) -> Result<Schedule, ScheduleStoreError> {
        self.commit_schedule_mutation_with_refill(schedule, occurrences, None)
            .await
    }

    async fn commit_schedule_refill(
        &self,
        schedule: AuthorizedScheduleWrite,
        occurrences: Vec<AuthorizedOccurrenceWrite>,
        next_refill_at_utc: Option<DateTime<Utc>>,
    ) -> Result<Schedule, ScheduleStoreError> {
        self.commit_schedule_mutation_with_refill(schedule, occurrences, Some(next_refill_at_utc))
            .await
    }

    async fn record_refill_deadline_if_current(
        &self,
        schedule_id: &ScheduleId,
        expected_revision: ScheduleRevision,
        expected_refill_at_utc: DateTime<Utc>,
        next_refill_at_utc: Option<DateTime<Utc>>,
    ) -> Result<(), ScheduleStoreError> {
        let mut state = self.inner.write().await;
        let schedule = state.schedules.get(schedule_id).cloned().ok_or_else(|| {
            ScheduleStoreError::ScheduleNotFound {
                schedule_id: schedule_id.clone(),
            }
        })?;
        if schedule.phase != SchedulePhase::Active
            || schedule.revision != expected_revision
            || state.refill_deadlines.get(schedule_id) != Some(&expected_refill_at_utc)
        {
            return Err(ScheduleStoreError::Concurrency(format!(
                "schedule {schedule_id} refill token changed"
            )));
        }
        memory_update_schedule_refill_projection(
            &mut state,
            Some(&schedule),
            &schedule,
            Some(next_refill_at_utc),
        );
        Ok(())
    }

    async fn get_occurrence(
        &self,
        occurrence_id: &OccurrenceId,
    ) -> Result<Option<Occurrence>, ScheduleStoreError> {
        Ok(self
            .inner
            .read()
            .await
            .occurrences
            .get(occurrence_id)
            .cloned())
    }

    async fn list_occurrences(
        &self,
        filter: OccurrenceFilter,
    ) -> Result<Vec<Occurrence>, ScheduleStoreError> {
        let mut occurrences: Vec<Occurrence> = self
            .inner
            .read()
            .await
            .occurrences
            .values()
            .filter(|occurrence| {
                (filter.include_terminal || !occurrence.is_terminal())
                    && filter
                        .schedule_id
                        .as_ref()
                        .is_none_or(|schedule_id| &occurrence.schedule_id == schedule_id)
                    && filter.phase.is_none_or(|phase| occurrence.phase == phase)
                    && filter
                        .due_after_utc
                        .is_none_or(|due_after| occurrence.due_at_utc >= due_after)
                    && filter
                        .due_before_utc
                        .is_none_or(|due_before| occurrence.due_at_utc <= due_before)
            })
            .cloned()
            .collect();
        occurrences.sort_by_key(|occurrence| {
            (
                occurrence.due_at_utc,
                occurrence.schedule_revision,
                occurrence.occurrence_ordinal,
            )
        });
        if let Some(limit) = filter.limit {
            occurrences.truncate(limit);
        }
        Ok(occurrences)
    }

    async fn append_receipt(&self, receipt: DeliveryReceipt) -> Result<(), ScheduleStoreError> {
        let mut state = self.inner.write().await;
        let Some(occurrence) = state.occurrences.get(&receipt.occurrence_id).cloned() else {
            return Err(ScheduleStoreError::OccurrenceNotFound {
                occurrence_id: receipt.occurrence_id,
            });
        };
        let updated = occurrence
            .clone()
            .apply(OccurrenceLifecycleInput::RecordReceipt {
                runtime_outcome: receipt.runtime_outcome.clone(),
                receipt: receipt.clone(),
            })
            .map_err(|error| ScheduleStoreError::Internal(error.to_string()))?
            .into_occurrence();
        let canonical_receipt = updated.last_receipt.clone().ok_or_else(|| {
            ScheduleStoreError::Internal(
                "generated occurrence authority did not produce a receipt".to_string(),
            )
        })?;
        state
            .receipts
            .entry(receipt.occurrence_id.clone())
            .or_default()
            .push(canonical_receipt);
        memory_note_pending_departure(&mut state, &occurrence, Some(&updated), Utc::now());
        state
            .occurrences
            .insert(updated.occurrence_id.clone(), updated);
        Ok(())
    }

    async fn list_receipts(
        &self,
        occurrence_id: &OccurrenceId,
    ) -> Result<Vec<DeliveryReceipt>, ScheduleStoreError> {
        Ok(self
            .inner
            .read()
            .await
            .receipts
            .get(occurrence_id)
            .cloned()
            .unwrap_or_default())
    }

    async fn claim_due_occurrences(
        &self,
        request: ClaimDueRequest,
    ) -> Result<ClaimDueResult, ScheduleStoreError> {
        let store_now_utc = Utc::now();
        if request.limit == 0 {
            return Ok(ClaimDueResult {
                store_now_utc,
                claimed: Vec::new(),
                row_faults: Vec::new(),
            });
        }
        let mut state = self.inner.write().await;

        let active_schedules: BTreeMap<ScheduleId, SchedulePhase> = state
            .schedules
            .iter()
            .map(|(schedule_id, schedule)| (schedule_id.clone(), schedule.phase))
            .collect();

        let mut occurrence_order: Vec<_> = state
            .occurrences
            .values()
            .filter(|occurrence| {
                active_schedules
                    .get(&occurrence.schedule_id)
                    .is_some_and(|phase| *phase == SchedulePhase::Active)
                    && occurrence_next_action_at(occurrence)
                        .is_some_and(|action_at| action_at <= store_now_utc)
            })
            .map(|occurrence| {
                (
                    (
                        occurrence_next_action_at(occurrence).unwrap_or(occurrence.due_at_utc),
                        occurrence.due_at_utc,
                        occurrence.schedule_revision,
                        occurrence.occurrence_ordinal,
                    ),
                    occurrence.occurrence_id.clone(),
                )
            })
            .collect();
        occurrence_order.sort_by_key(|(key, _)| *key);

        let mut claimed = Vec::new();
        let mut row_faults = Vec::new();
        // Machine refusals are tolerated per row with the SAME typed-fault
        // semantics as the sqlite backend (Rule 8: one semantic condition,
        // one terminal shape across backends): a refusal skips only that
        // occurrence and its neighbors still claim. In-memory rows are typed
        // values already, so there is no durable-format parse boundary to
        // fault on — only `DueClassification` faults can occur here.
        let due_classification_fault =
            |occurrence: &Occurrence, stage: &str, error: &OccurrenceLifecycleError| {
                ScheduleStoreRowFault {
                    schedule_id: Some(occurrence.schedule_id.to_string()),
                    occurrence_id: Some(occurrence.occurrence_id.to_string()),
                    kind: ScheduleStoreRowFaultKind::DueClassification,
                    detail: format!("{stage}: {error}"),
                }
            };
        for (_, occurrence_id) in occurrence_order {
            let Some(existing) = state.occurrences.get(&occurrence_id).cloned() else {
                continue;
            };
            let action = match existing.classify_due_action(store_now_utc) {
                Ok(action) => action,
                Err(error) => {
                    row_faults.push(due_classification_fault(
                        &existing,
                        "due classification",
                        &error,
                    ));
                    continue;
                }
            };
            match action {
                Some(OccurrenceDueAction::MisfireRequired) => {
                    let detail = Some(existing.due_misfire_detail_at(store_now_utc));
                    let mut updated =
                        match existing
                            .clone()
                            .apply(OccurrenceLifecycleInput::ResolveDueMisfire {
                                detail: detail.clone(),
                                at_utc: store_now_utc,
                            }) {
                            Ok(mutator) => mutator.into_occurrence(),
                            Err(error) => {
                                row_faults.push(due_classification_fault(
                                    &existing,
                                    "misfire resolution",
                                    &error,
                                ));
                                continue;
                            }
                        };
                    let receipt = match updated.delivery_receipt_from_authority(None) {
                        Ok(receipt) => receipt,
                        Err(error) => {
                            row_faults.push(due_classification_fault(
                                &existing,
                                "misfire receipt",
                                &error,
                            ));
                            continue;
                        }
                    };
                    updated = match updated.apply(OccurrenceLifecycleInput::RecordReceipt {
                        runtime_outcome: receipt.runtime_outcome.clone(),
                        receipt,
                    }) {
                        Ok(mutator) => mutator.into_occurrence(),
                        Err(error) => {
                            row_faults.push(due_classification_fault(
                                &existing,
                                "misfire receipt record",
                                &error,
                            ));
                            continue;
                        }
                    };
                    let canonical_receipt = updated.last_receipt.clone().ok_or_else(|| {
                        ScheduleStoreError::Concurrency(
                            "generated occurrence authority did not produce a receipt".to_string(),
                        )
                    })?;
                    state
                        .receipts
                        .entry(updated.occurrence_id.clone())
                        .or_default()
                        .push(canonical_receipt);
                    memory_note_pending_departure(
                        &mut state,
                        &existing,
                        Some(&updated),
                        store_now_utc,
                    );
                    state
                        .occurrences
                        .insert(updated.occurrence_id.clone(), updated);
                }
                Some(OccurrenceDueAction::ClaimEligible) => {
                    if claimed.len() >= request.limit {
                        continue;
                    }
                    let updated = claim_occurrence(existing.clone(), &request, store_now_utc)
                        .map_err(|error| ScheduleStoreError::Concurrency(error.to_string()))?;
                    memory_note_pending_departure(
                        &mut state,
                        &existing,
                        Some(&updated),
                        store_now_utc,
                    );
                    state
                        .occurrences
                        .insert(updated.occurrence_id.clone(), updated.clone());
                    claimed.push(updated);
                }
                Some(OccurrenceDueAction::LeaseExpired) => {
                    if claimed.len() >= request.limit {
                        continue;
                    }
                    let lease_expired =
                        match expire_occurrence_lease(existing.clone(), store_now_utc) {
                            Ok(lease_expired) => lease_expired,
                            Err(error) => {
                                row_faults.push(due_classification_fault(
                                    &existing,
                                    "lease expiry",
                                    &error,
                                ));
                                continue;
                            }
                        };
                    state
                        .receipts
                        .entry(lease_expired.receipt.occurrence_id.clone())
                        .or_default()
                        .push(lease_expired.receipt.clone());
                    state.occurrences.insert(
                        lease_expired.occurrence.occurrence_id.clone(),
                        lease_expired.occurrence.clone(),
                    );
                    // A machine refusal of the follow-up claim is this row's
                    // typed fault, never a silent skip: the expiry above
                    // stays applied and the row re-enters the scan on the
                    // next tick.
                    let updated =
                        match claim_occurrence(lease_expired.occurrence, &request, store_now_utc) {
                            Ok(updated) => updated,
                            Err(error) => {
                                row_faults.push(due_classification_fault(
                                    &existing,
                                    "lease-expiry reclaim",
                                    &error,
                                ));
                                continue;
                            }
                        };
                    state
                        .occurrences
                        .insert(updated.occurrence_id.clone(), updated.clone());
                    claimed.push(updated);
                }
                None => {}
            }
        }

        Ok(ClaimDueResult {
            store_now_utc,
            claimed,
            row_faults,
        })
    }

    async fn renew_occurrence_lease_if_current(
        &self,
        request: RenewOccurrenceLeaseRequest,
    ) -> Result<RenewOccurrenceLeaseResult, ScheduleStoreError> {
        let mut state = self.inner.write().await;
        let store_now_utc = Utc::now();
        let Some(current) = state.occurrences.get(&request.occurrence_id).cloned() else {
            return Ok(RenewOccurrenceLeaseResult {
                store_now_utc,
                outcome: RenewOccurrenceLeaseOutcome::StaleClaim,
            });
        };
        if current.attempt_count != request.expected_attempt
            || current.claim_token() != Some(request.claim_token)
            || current.claimed_by.as_deref() != Some(request.expected_owner_id.as_str())
        {
            return Ok(RenewOccurrenceLeaseResult {
                store_now_utc,
                outcome: RenewOccurrenceLeaseOutcome::StaleClaim,
            });
        }
        let renewed = renew_occurrence_lease(current, &request, store_now_utc)
            .map_err(|error| ScheduleStoreError::Concurrency(error.to_string()))?;
        state
            .occurrences
            .insert(renewed.occurrence_id.clone(), renewed.clone());
        Ok(RenewOccurrenceLeaseResult {
            store_now_utc,
            outcome: RenewOccurrenceLeaseOutcome::Renewed(renewed),
        })
    }

    async fn transition_occurrence_if_current(
        &self,
        occurrence_id: &OccurrenceId,
        expected_attempt: u32,
        expected_claim_token: Option<Uuid>,
        transition: OccurrenceLifecycleInput,
    ) -> Result<Option<(Occurrence, Vec<OccurrenceLifecycleEffect>)>, ScheduleStoreError> {
        let mut state = self.inner.write().await;
        let Some(current) = state.occurrences.get(occurrence_id).cloned() else {
            return Ok(None);
        };
        if current.attempt_count != expected_attempt
            || current.claim_token() != expected_claim_token
        {
            return Ok(None);
        }
        let mutator = current
            .clone()
            .apply(transition)
            .map_err(|error| ScheduleStoreError::Concurrency(error.to_string()))?;
        let (updated, effects) = mutator.into_parts();
        memory_note_pending_departure(&mut state, &current, Some(&updated), Utc::now());
        state
            .occurrences
            .insert(updated.occurrence_id.clone(), updated.clone());
        Ok(Some((updated, effects)))
    }

    async fn transition_occurrence_with_receipt_if_current(
        &self,
        occurrence_id: &OccurrenceId,
        expected_attempt: u32,
        expected_claim_token: Option<Uuid>,
        transition: OccurrenceLifecycleInput,
        runtime_outcome: Option<RuntimeDeliveryOutcome>,
    ) -> Result<Option<Occurrence>, ScheduleStoreError> {
        let mut state = self.inner.write().await;
        let Some(current) = state.occurrences.get(occurrence_id).cloned() else {
            return Ok(None);
        };
        if current.attempt_count != expected_attempt
            || current.claim_token() != expected_claim_token
        {
            return Ok(None);
        }
        let terminalized = current
            .clone()
            .apply(transition)
            .map_err(|error| ScheduleStoreError::Concurrency(error.to_string()))?
            .into_occurrence();
        let receipt = terminalized
            .delivery_receipt_from_authority(runtime_outcome)
            .map_err(|error| ScheduleStoreError::Concurrency(error.to_string()))?;
        let updated = terminalized
            .apply(OccurrenceLifecycleInput::RecordReceipt {
                runtime_outcome: receipt.runtime_outcome.clone(),
                receipt,
            })
            .map_err(|error| ScheduleStoreError::Concurrency(error.to_string()))?
            .into_occurrence();
        let canonical_receipt = updated.last_receipt.clone().ok_or_else(|| {
            ScheduleStoreError::Concurrency(
                "generated occurrence authority did not produce a receipt".to_string(),
            )
        })?;
        state
            .receipts
            .entry(updated.occurrence_id.clone())
            .or_default()
            .push(canonical_receipt);
        memory_note_pending_departure(&mut state, &current, Some(&updated), Utc::now());
        state
            .occurrences
            .insert(updated.occurrence_id.clone(), updated.clone());
        Ok(Some(updated))
    }
}

fn unsupported(kind: ScheduleStoreKind) -> ScheduleStoreError {
    ScheduleStoreError::UnsupportedBackend { backend: kind }
}

fn reject_standalone_supersession_write(
    write: &AuthorizedScheduleWrite,
) -> Result<(), ScheduleStoreError> {
    if write.has_pending_supersession() {
        return Err(ScheduleStoreError::Internal(
            "generated schedule supersession requires atomic schedule mutation".into(),
        ));
    }
    Ok(())
}
