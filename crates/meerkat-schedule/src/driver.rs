use crate::error::{ScheduleDomainError, ScheduleStoreError};
use crate::lifecycle::{
    AdmissionEvidence, ClaimedDispatchDisposition, CompletionSupersessionDisposition,
    OccurrenceLifecycleEffect, OccurrenceLifecycleInput, StaleCompletionArrivalTrigger,
};
use crate::service::ScheduleService;
use crate::store::{
    AcquireScheduleExecutorLeaseOutcome, AcquireScheduleExecutorLeaseRequest, ClaimDueRequest,
    RenewOccurrenceLeaseOutcome, RenewOccurrenceLeaseRequest, RenewScheduleExecutorLeaseOutcome,
    RenewScheduleExecutorLeaseRequest, ScheduleExecutorLease, ScheduleStore,
};
use crate::types::{
    DeliveryAdmissionOutcome, DeliveryCompletionFailureReason, DeliveryFailureReason,
    DeliveryReceipt, IdentityTargetBinding, Occurrence, OccurrenceId, OccurrencePhase,
    OccurrenceTargetProbeOutcome, RuntimeCompletionOutcome, RuntimeDeliveryOutcome,
};
use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use futures::Future;
use futures::future::{self, Either};
use meerkat_core::SessionId;
use std::fmt;
use std::pin::Pin;
use std::sync::Arc;

const REFILL_CANDIDATE_LIMIT: usize = 32;

pub type DeliveryCompletion =
    Pin<Box<dyn Future<Output = Result<DeliveryTerminal, ScheduleDomainError>> + Send + 'static>>;

pub struct DeliveryDispatch {
    pub receipt: DeliveryReceipt,
    pub correlation_id: Option<String>,
    pub materialized_session_id: Option<SessionId>,
    pub completion: DeliveryCompletion,
}

/// Stable identity of one occurrence's external delivery effect. The driver
/// commits this identity in `DispatchStarted` before calling the adapter.
/// Every retry of the occurrence, including a lease-expiry reclaim in another
/// process, receives these same values and must use `idempotency_key` at the
/// target admission boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduleDeliveryIdentity {
    pub idempotency_key: String,
    pub correlation_id: String,
}

impl ScheduleDeliveryIdentity {
    pub fn for_occurrence(occurrence: &Occurrence) -> Self {
        Self {
            idempotency_key: format!(
                "schedule:{}:occurrence:{}",
                occurrence.schedule_id, occurrence.occurrence_id
            ),
            correlation_id: occurrence.occurrence_id.to_string(),
        }
    }
}

impl fmt::Debug for DeliveryDispatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DeliveryDispatch")
            .field("receipt", &self.receipt)
            .field("correlation_id", &self.correlation_id)
            .field("materialized_session_id", &self.materialized_session_id)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone)]
pub struct DeliveryTerminal {
    pub phase: OccurrencePhase,
    pub receipt: Option<DeliveryReceipt>,
    pub detail: Option<String>,
    pub delivery_failure_reason: Option<DeliveryFailureReason>,
    pub runtime_completion_outcome: Option<RuntimeCompletionOutcome>,
    pub runtime_outcome: Option<RuntimeDeliveryOutcome>,
}

impl DeliveryTerminal {
    pub fn completed(receipt: Option<DeliveryReceipt>) -> Self {
        Self {
            phase: OccurrencePhase::Completed,
            receipt,
            detail: None,
            delivery_failure_reason: None,
            runtime_completion_outcome: None,
            runtime_outcome: None,
        }
    }

    pub fn delivery_failed(detail: impl Into<String>, reason: DeliveryFailureReason) -> Self {
        Self {
            phase: OccurrencePhase::DeliveryFailed,
            receipt: None,
            detail: Some(detail.into()),
            delivery_failure_reason: Some(reason),
            runtime_completion_outcome: None,
            runtime_outcome: None,
        }
    }

    pub fn runtime_completion(
        outcome: RuntimeCompletionOutcome,
        detail: Option<String>,
        runtime_outcome: Option<RuntimeDeliveryOutcome>,
    ) -> Self {
        Self {
            phase: OccurrencePhase::AwaitingCompletion,
            receipt: None,
            detail,
            delivery_failure_reason: None,
            runtime_completion_outcome: Some(outcome),
            runtime_outcome,
        }
    }
}

#[derive(Debug, Clone)]
pub enum TargetProbeOutcome {
    Ready,
    Busy { detail: Option<String> },
    Missing { detail: Option<String> },
}

#[async_trait]
pub trait ScheduleTargetProbe: Send + Sync {
    async fn probe_target(
        &self,
        occurrence: &Occurrence,
    ) -> Result<TargetProbeOutcome, ScheduleDomainError>;
}

/// Create-time deliverability classification of an identity target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdentityTargetDeliverability {
    /// Some host on this surface recognizes the identity form. Current
    /// absence (for example a not-yet-spawned mob member) stays a fire-time
    /// condition governed by the schedule's `missing_target_policy`.
    Deliverable,
    /// No host on this surface recognizes the identity; occurrences could
    /// only ever misfire as `target_missing`.
    NeverDeliverable { detail: Option<String> },
}

/// Surface-composed create-time probe for identity targets.
///
/// Unlike [`ScheduleTargetProbe`] this is binding-level (no occurrence
/// exists yet): it judges whether the identity form is deliverable at all on
/// this surface, not whether the target is ready right now. Attached to a
/// [`crate::ScheduleService`] via `attach_identity_target_probe`, which holds
/// it weakly while the attaching surface owns it, it lets create/update
/// refuse schedules whose every occurrence would misfire.
#[async_trait]
pub trait ScheduleIdentityTargetProbe: Send + Sync {
    async fn probe_identity_target(
        &self,
        binding: &IdentityTargetBinding,
    ) -> Result<IdentityTargetDeliverability, ScheduleDomainError>;
}

#[async_trait]
pub trait ScheduleTargetDelivery: Send + Sync {
    /// Realize the already-committed `DispatchStarted` intent.
    ///
    /// Implementations must durably deduplicate `identity.idempotency_key`
    /// before acknowledging admission. `Ok` means the returned dispatch
    /// describes either that first admission or an exact replay. The
    /// completion of an exact replay resolves from the original input's own
    /// terminal outcome: the driver waits for it exactly as it waits for a
    /// first admission, and never records a terminal at admission. An ordinary
    /// `Err` is permitted only when no target-side effect was admitted. If an
    /// implementation cannot prove whether admission happened, it must return
    /// [`ScheduleDomainError::DeliveryRepairDeferred`] so the occurrence stays
    /// reclaimable and the same stable identity is retried.
    async fn deliver_occurrence(
        &self,
        occurrence: &Occurrence,
        identity: &ScheduleDeliveryIdentity,
    ) -> Result<DeliveryDispatch, ScheduleDomainError>;

    /// Redeliver a reclaimed occurrence whose earlier attempt recorded its
    /// dispatch, without a fresh admission (see [`ExistingDelivery`]). The
    /// driver has committed the attempt's `DispatchStarted` intent, as for
    /// [`Self::deliver_occurrence`].
    ///
    /// The default delivers an [`ExistingDelivery::Admitted`] occurrence
    /// through [`Self::deliver_occurrence`], whose contract already requires
    /// the identity's durable deduplication, and answers
    /// [`ExistingDeliveryAnswer::NotAdmittedPastDeadline`] for
    /// [`ExistingDelivery::UnprovenLate`]: an implementation that cannot
    /// tell whether it admitted the identity never delivers a late
    /// occurrence fresh, and the occurrence ends misfired as before.
    /// Implementations that decide it inside their admission override this.
    async fn deliver_existing_occurrence(
        &self,
        occurrence: &Occurrence,
        identity: &ScheduleDeliveryIdentity,
        existing: ExistingDelivery,
    ) -> Result<ExistingDeliveryAnswer, ScheduleDomainError> {
        match existing {
            ExistingDelivery::Admitted => self
                .deliver_occurrence(occurrence, identity)
                .await
                .map(ExistingDeliveryAnswer::Dispatch),
            ExistingDelivery::UnprovenLate => Ok(ExistingDeliveryAnswer::NotAdmittedPastDeadline),
        }
    }
}

/// How the driver redelivers a reclaimed occurrence whose earlier attempt
/// recorded its dispatch ([`ScheduleTargetDelivery::deliver_existing_occurrence`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ExistingDelivery {
    /// An earlier attempt's target admission is recorded (a DispatchAccepted
    /// receipt). The redelivery must find that admission and never admit
    /// the delivery fresh; the occurrence then waits for that input's own
    /// outcome. A target that no longer knows the admission ends the
    /// occurrence delivery_failed, with nothing admitted.
    ///
    /// A host runnable keeps no admission record and runs again, so for host
    /// runnables the guarantee is at-least-once and relies on the runnable
    /// deduplicating on its `delivery_idempotency_key`. The trait's default
    /// and the surface hosts' defaults (a mob host, a session host that does
    /// not decide existing-only admission) redeliver an admitted occurrence
    /// through the ordinary delivery and rely on its durable deduplication:
    /// a target that lost the admission admits the delivery again.
    Admitted,
    /// Only an earlier attempt's dispatch start is recorded, and the
    /// occurrence is past its misfire deadline. The redelivery may only find
    /// an admission of the same identity; a target that holds none answers
    /// [`ExistingDeliveryAnswer::NotAdmittedPastDeadline`], and the
    /// occurrence ends misfired.
    UnprovenLate,
}

/// The answer of [`ScheduleTargetDelivery::deliver_existing_occurrence`].
#[derive(Debug)]
#[non_exhaustive]
pub enum ExistingDeliveryAnswer {
    /// The target found the earlier admission: the dispatch of that replay.
    Dispatch(DeliveryDispatch),
    /// For [`ExistingDelivery::UnprovenLate`]: the target never admitted the
    /// delivery, and admitted nothing now.
    NotAdmittedPastDeadline,
}

#[derive(Debug, Clone)]
pub struct ScheduleDriverConfig {
    pub claim_limit: usize,
    pub lease_duration: Duration,
}

impl Default for ScheduleDriverConfig {
    fn default() -> Self {
        Self {
            claim_limit: 32,
            lease_duration: Duration::seconds(60),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ScheduleTickReport {
    /// True only when this tick held the store-issued singular firing lease.
    /// A false report is a healthy standby observation, not progress.
    pub executor_authorized: bool,
    pub planned_occurrences: usize,
    pub claimed_occurrences: usize,
    pub terminalized_occurrences: usize,
    /// Schedule or Pending-occurrence rows the bounded refill scan skipped as
    /// typed per-row faults instead of failing the tick wholesale.
    pub schedule_row_faults: Vec<crate::ScheduleStoreRowFault>,
    /// Occurrence rows the claim scan skipped as typed per-row faults.
    pub occurrence_row_faults: Vec<crate::ScheduleStoreRowFault>,
    /// Per-schedule horizon-refill failures. A schedule whose planning
    /// refill errors is reported and skipped so its neighbors still plan
    /// and claim.
    pub refill_faults: Vec<ScheduleRefillFault>,
}

impl ScheduleTickReport {
    /// Whether this tick moved any work: planned, claimed, or terminalized
    /// at least one occurrence. Hosts use this as the pacing signal — a run
    /// of ticks with no progress (idle store, or a stuck one) is safe to
    /// poll less aggressively, and on a remote store every needless tick is
    /// billed (2026-07-29 incident: a no-progress tick retried at 4Hz
    /// forever; a BigQuery-store consumer flagged the query spam as a
    /// parity blocker).
    pub fn made_progress(&self) -> bool {
        self.planned_occurrences > 0
            || self.claimed_occurrences > 0
            || self.terminalized_occurrences > 0
    }

    /// Total typed faults this tick surfaced (rows skipped or schedules
    /// whose refill failed). Non-zero means an operator-visible incident
    /// even though the tick itself succeeded for healthy rows.
    pub fn fault_count(&self) -> usize {
        self.schedule_row_faults.len() + self.occurrence_row_faults.len() + self.refill_faults.len()
    }

    /// Operator-facing bounded sample of the current fault page.
    ///
    /// Durable scans can encounter a different poisoned page on every tick.
    /// Health logs need enough attribution to investigate without copying an
    /// unbounded page (or an unbounded corrupt payload) into every heartbeat.
    pub fn bounded_fault_summary(&self, max_samples: usize, max_chars_per_sample: usize) -> String {
        use std::fmt::Write as _;

        fn truncate_sample(sample: String, max_chars: usize) -> String {
            let mut chars = sample.chars();
            let truncated = chars.by_ref().take(max_chars).collect::<String>();
            if chars.next().is_some() {
                format!("{truncated}...")
            } else {
                truncated
            }
        }

        let mut samples = self
            .schedule_row_faults
            .iter()
            .map(|fault| {
                format!(
                    "schedule-row {}: {:?} fault: {}",
                    fault.schedule_id.as_deref().unwrap_or("?"),
                    fault.kind,
                    fault.detail
                )
            })
            .chain(self.occurrence_row_faults.iter().map(|fault| {
                format!(
                    "occurrence-row {} (schedule={}): {:?} fault: {}",
                    fault.occurrence_id.as_deref().unwrap_or("?"),
                    fault.schedule_id.as_deref().unwrap_or("?"),
                    fault.kind,
                    fault.detail
                )
            }))
            .chain(
                self.refill_faults
                    .iter()
                    .map(|fault| format!("refill {}: {}", fault.schedule_id, fault.detail)),
            );
        let mut out = String::new();
        let mut emitted = 0usize;
        for sample in samples.by_ref().take(max_samples) {
            let _ = writeln!(out, "{}", truncate_sample(sample, max_chars_per_sample));
            emitted += 1;
        }
        let omitted = self.fault_count().saturating_sub(emitted);
        if omitted > 0 {
            let _ = writeln!(out, "... {omitted} additional fault(s) omitted");
        }
        out
    }

    /// Full diagnostic fingerprint of the fault set.
    ///
    /// Hosts must not use this volatile, unbounded detail as their incident
    /// identity; rate limiting is keyed by a stable failure class and logs use
    /// [`Self::bounded_fault_summary`].
    pub fn fault_fingerprint(&self) -> String {
        use std::fmt::Write as _;
        let mut out = String::new();
        for fault in &self.schedule_row_faults {
            let _ = writeln!(out, "schedule-row: {fault}");
        }
        for fault in &self.occurrence_row_faults {
            let _ = writeln!(out, "occurrence-row: {fault}");
        }
        for fault in &self.refill_faults {
            let _ = writeln!(out, "refill {}: {}", fault.schedule_id, fault.detail);
        }
        out
    }
}

/// A schedule whose horizon refill failed during a driver tick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduleRefillFault {
    pub schedule_id: crate::ScheduleId,
    pub detail: String,
}

enum ClaimedOccurrenceDispatchState {
    Ready(Occurrence),
    Frozen,
    Supersede {
        occurrence: Occurrence,
        superseded_by_revision: crate::ScheduleRevision,
    },
}

enum TargetProbeResolution {
    Continue(Box<Occurrence>),
    Terminalized,
    StaleClaim,
}

/// Typed result of `terminalize_occurrence_inner`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TerminalizeOutcome {
    /// The transition was applied and a terminal receipt was minted.
    Applied,
    /// The transition was applied but the machine emitted
    /// `LateCompletionResolutionRecorded` — no fresh receipt is minted
    /// (the commit-time supersession sweep already minted the canonical one).
    LateRecorded,
    /// The transition was a no-op (zero effects, e.g. `SupersedeAlreadySuperseded`).
    /// No receipt is minted.
    IdempotentNoop,
    /// The store screen rejected the claim evidence (attempt/token mismatch).
    /// The caller is responsible for feeding `ClassifyStaleCompletionArrival`.
    StaleClaim,
}

pub struct ScheduleDriver {
    service: ScheduleService,
    store: Arc<dyn ScheduleStore>,
    probe: Arc<dyn ScheduleTargetProbe>,
    delivery: Arc<dyn ScheduleTargetDelivery>,
    owner_id: String,
    executor_owner_id: String,
    executor_lease: crate::tokio::sync::Mutex<Option<ScheduleExecutorLease>>,
    tick_gate: crate::tokio::sync::Mutex<()>,
    config: ScheduleDriverConfig,
}

impl ScheduleDriver {
    pub fn new(
        service: ScheduleService,
        store: Arc<dyn ScheduleStore>,
        probe: Arc<dyn ScheduleTargetProbe>,
        delivery: Arc<dyn ScheduleTargetDelivery>,
        owner_id: impl Into<String>,
        config: ScheduleDriverConfig,
    ) -> Self {
        let owner_id = owner_id.into();
        Self {
            service,
            store,
            probe,
            delivery,
            executor_owner_id: format!("{owner_id}:{}", uuid::Uuid::now_v7()),
            owner_id,
            executor_lease: crate::tokio::sync::Mutex::new(None),
            tick_gate: crate::tokio::sync::Mutex::new(()),
            config,
        }
    }

    async fn acquire_or_renew_executor_lease(
        &self,
    ) -> Result<Option<ScheduleExecutorLease>, ScheduleDomainError> {
        let lease_duration = self.config.lease_duration.max(Duration::seconds(15));
        let mut slot = self.executor_lease.lock().await;
        if let Some(current) = slot.clone() {
            match self
                .store
                .renew_executor_lease(RenewScheduleExecutorLeaseRequest {
                    lease: current,
                    lease_duration,
                })
                .await?
            {
                RenewScheduleExecutorLeaseOutcome::Renewed(renewed) => {
                    *slot = Some(renewed.clone());
                    return Ok(Some(renewed));
                }
                RenewScheduleExecutorLeaseOutcome::Stale => *slot = None,
            }
        }
        match self
            .store
            .acquire_executor_lease(AcquireScheduleExecutorLeaseRequest {
                owner_id: self.executor_owner_id.clone(),
                lease_duration,
            })
            .await?
        {
            AcquireScheduleExecutorLeaseOutcome::Acquired(lease) => {
                *slot = Some(lease.clone());
                Ok(Some(lease))
            }
            AcquireScheduleExecutorLeaseOutcome::Busy { .. } => Ok(None),
        }
    }

    pub async fn release_executor_lease(&self) -> Result<(), ScheduleDomainError> {
        let _tick = self.tick_gate.lock().await;
        let lease = self.executor_lease.lock().await.take();
        if let Some(lease) = lease {
            self.store.release_executor_lease(lease).await?;
        }
        Ok(())
    }

    /// Maximum idle wait that keeps the store-owned executor lease live.
    /// Hosts clamp every sleep/wake wait to this interval, including stores
    /// whose ordinary work wake is push-only or process-local.
    pub fn executor_heartbeat_interval(&self) -> std::time::Duration {
        let lease_duration = self.config.lease_duration.max(Duration::seconds(15));
        let half_ms = (lease_duration.num_milliseconds() / 2).max(1);
        std::time::Duration::from_millis(u64::try_from(half_ms).unwrap_or(u64::MAX))
    }

    pub async fn tick_once(&self) -> Result<ScheduleTickReport, ScheduleDomainError> {
        // Renewal replaces the exact executor witness. Serialize ticks from
        // this driver incarnation so one concurrent tick cannot stale the
        // witness another tick is about to present to the claim transaction.
        let _tick = self.tick_gate.lock().await;
        let mut report = ScheduleTickReport::default();
        let Some(executor_lease) = self.acquire_or_renew_executor_lease().await? else {
            return Ok(report);
        };
        report.executor_authorized = true;
        self.tick_once_authorized(report, executor_lease).await
    }

    /// Run one tick while allowing a host to cancel only after executor lease
    /// acquisition or renewal has committed and its local witness is installed.
    ///
    /// Cancelling the ordinary [`Self::tick_once`] future can lose that witness
    /// if a remote store commits acquisition just before the future is dropped.
    /// Hosts that need bounded shutdown use this seam so later tick work remains
    /// cancellable without orphaning the executor lease.
    pub async fn tick_once_until_cancelled<F>(
        &self,
        cancellation: F,
    ) -> Result<Option<ScheduleTickReport>, ScheduleDomainError>
    where
        F: std::future::Future,
    {
        let _tick = self.tick_gate.lock().await;
        let mut report = ScheduleTickReport::default();
        let Some(executor_lease) = self.acquire_or_renew_executor_lease().await? else {
            return Ok(Some(report));
        };
        report.executor_authorized = true;
        crate::tokio::pin!(cancellation);
        crate::tokio::select! {
            biased;
            _ = &mut cancellation => Ok(None),
            result = self.tick_once_authorized(report, executor_lease) => result.map(Some),
        }
    }

    async fn tick_once_authorized(
        &self,
        mut report: ScheduleTickReport,
        executor_lease: ScheduleExecutorLease,
    ) -> Result<ScheduleTickReport, ScheduleDomainError> {
        // Per-row tolerance end to end: a poisoned schedule row, a poisoned
        // occurrence row, or one schedule whose refill fails must not starve
        // every other schedule. Every skip is a typed fault in the report —
        // never a silent drop.
        let refill_batch = self
            .store
            .read_due_refill_candidates(REFILL_CANDIDATE_LIMIT)
            .await?;
        report.schedule_row_faults = refill_batch.row_faults;
        for candidate in refill_batch.candidates {
            let schedule_id = candidate.schedule.schedule_id.clone();
            match self
                .service
                .refill_candidate(candidate, refill_batch.store_now_utc)
                .await
            {
                Ok(planned) => report.planned_occurrences += planned.len(),
                Err(error) => report.refill_faults.push(ScheduleRefillFault {
                    schedule_id,
                    detail: error.to_string(),
                }),
            }
        }

        let claimed = self
            .store
            .claim_due_occurrences(
                &executor_lease,
                ClaimDueRequest {
                    owner_id: self.owner_id.clone(),
                    limit: self.config.claim_limit,
                    lease_duration: self.config.lease_duration,
                },
            )
            .await?;
        report.occurrence_row_faults = claimed.row_faults;

        // One claimed occurrence's fault must not strand the rest of the
        // batch, claimed but undelivered until their leases expire: every
        // claimed occurrence is handled, then the tick fails with the first
        // fault. Only a claim scan that breaks the store contract still fails
        // the tick at once.
        let mut first_fault = None;
        for commit in claimed.transitions {
            let successor = commit.successor();
            match successor.phase {
                OccurrencePhase::Claimed => {
                    if !commit
                        .effects()
                        .iter()
                        .any(|effect| matches!(effect, OccurrenceLifecycleEffect::Claimed))
                    {
                        return Err(claim_scan_contract_violation(
                            "claim scan committed a Claimed successor without a Claimed effect"
                                .to_string(),
                            first_fault.as_ref(),
                        ));
                    }
                    report.claimed_occurrences += 1;
                    // A reclaim of an occurrence whose earlier attempt
                    // recorded its dispatch redelivers without a fresh
                    // admission.
                    let existing = commit.effects().iter().find_map(|effect| match effect {
                        OccurrenceLifecycleEffect::ReclaimedAfterAdmission => {
                            Some(ExistingDelivery::Admitted)
                        }
                        OccurrenceLifecycleEffect::ReclaimedAfterUnprovenDispatch => {
                            Some(ExistingDelivery::UnprovenLate)
                        }
                        _ => None,
                    });
                    match self
                        .handle_claimed_occurrence(
                            successor.clone(),
                            claimed.store_now_utc,
                            existing,
                        )
                        .await
                    {
                        Ok(true) => report.terminalized_occurrences += 1,
                        Ok(false) => {}
                        Err(error) if first_fault.is_none() => first_fault = Some(error),
                        Err(error) => tracing::error!(
                            occurrence_id = %successor.occurrence_id,
                            %error,
                            "claimed occurrence failed after an earlier fault in the same tick"
                        ),
                    }
                }
                OccurrencePhase::Misfired => {
                    if !commit
                        .effects()
                        .iter()
                        .any(|effect| matches!(effect, OccurrenceLifecycleEffect::Misfired))
                    {
                        return Err(claim_scan_contract_violation(
                            "claim scan committed a Misfired successor without a Misfired effect"
                                .to_string(),
                            first_fault.as_ref(),
                        ));
                    }
                }
                // An admitted delivery whose reclaims are used up ends
                // delivery_failed (lease_lost) in the scan itself.
                OccurrencePhase::DeliveryFailed => {
                    if !commit
                        .effects()
                        .iter()
                        .any(|effect| matches!(effect, OccurrenceLifecycleEffect::DeliveryFailed))
                    {
                        return Err(claim_scan_contract_violation(
                            "claim scan committed a DeliveryFailed successor without a DeliveryFailed effect"
                                .to_string(),
                            first_fault.as_ref(),
                        ));
                    }
                }
                OccurrencePhase::Pending => {
                    if !commit
                        .effects()
                        .iter()
                        .any(|effect| matches!(effect, OccurrenceLifecycleEffect::LeaseExpired))
                    {
                        return Err(claim_scan_contract_violation(
                            "claim scan committed a Pending lease-expiry successor without a LeaseExpired effect"
                                .to_string(),
                            first_fault.as_ref(),
                        ));
                    }
                }
                phase => {
                    return Err(claim_scan_contract_violation(
                        format!(
                            "claim scan returned unsupported committed occurrence phase {phase:?}"
                        ),
                        first_fault.as_ref(),
                    ));
                }
            }
        }

        match first_fault {
            Some(error) => Err(error),
            None => Ok(report),
        }
    }

    async fn handle_claimed_occurrence(
        &self,
        occurrence: Occurrence,
        store_now_utc: chrono::DateTime<Utc>,
        existing: Option<ExistingDelivery>,
    ) -> Result<bool, ScheduleDomainError> {
        let frozen_occurrence = occurrence.clone();
        let occurrence = match self
            .reconcile_claimed_occurrence_before_dispatch(occurrence)
            .await?
        {
            ClaimedOccurrenceDispatchState::Ready(occurrence) => occurrence,
            ClaimedOccurrenceDispatchState::Frozen => {
                let Some(commit) = self
                    .store
                    .transition_occurrence_with_receipt_if_current(
                        &frozen_occurrence.occurrence_id,
                        frozen_occurrence.attempt_count,
                        frozen_occurrence.claim_token(),
                        OccurrenceLifecycleInput::ReleaseLeaseForPausedSchedule {
                            at_utc: store_now_utc,
                        },
                        None,
                    )
                    .await?
                else {
                    return Ok(false);
                };
                if commit.successor().phase != OccurrencePhase::Pending
                    || !commit
                        .effects()
                        .iter()
                        .any(|effect| matches!(effect, OccurrenceLifecycleEffect::LeaseExpired))
                {
                    return Err(ScheduleDomainError::Internal(
                        "paused-schedule lease release committed without a Pending successor and LeaseExpired effect"
                            .to_string(),
                    ));
                }
                return Ok(false);
            }
            ClaimedOccurrenceDispatchState::Supersede {
                occurrence,
                superseded_by_revision,
            } => {
                self.terminalize_occurrence(
                    occurrence,
                    OccurrenceLifecycleInput::Supersede {
                        superseded_by_revision,
                        at_utc: store_now_utc,
                    },
                    None,
                )
                .await?;
                return Ok(true);
            }
        };

        let occurrence = match self
            .resolve_target_probe(occurrence, store_now_utc, existing)
            .await?
        {
            TargetProbeResolution::Continue(occurrence) => *occurrence,
            TargetProbeResolution::Terminalized => return Ok(true),
            TargetProbeResolution::StaleClaim => return Ok(false),
        };

        let delivery_identity = ScheduleDeliveryIdentity::for_occurrence(&occurrence);
        // Persist durable dispatch intent and its stable external identity
        // before the adapter can perform any target-side effect. A crash:
        // - after this commit but before delivery leaves the intent-bearing
        //   occurrence row reclaimable;
        // - after delivery but before observation replays the same idempotency
        //   key, never a newly minted effect identity.
        let Some(dispatch_commit) = self
            .store
            .transition_occurrence_with_receipt_if_current(
                &occurrence.occurrence_id,
                occurrence.attempt_count,
                occurrence.claim_token(),
                OccurrenceLifecycleInput::DispatchStarted {
                    correlation_id: Some(delivery_identity.correlation_id.clone()),
                    at_utc: store_now_utc,
                },
                None,
            )
            .await?
        else {
            return Ok(false);
        };
        if !dispatch_commit
            .effects()
            .iter()
            .any(|effect| matches!(effect, OccurrenceLifecycleEffect::DispatchStarted))
        {
            return Err(ScheduleDomainError::Internal(
                "committed dispatch-start transition lost its generated lifecycle effect"
                    .to_string(),
            ));
        }
        let mut dispatching = dispatch_commit.successor().clone();

        // The lease is renewed while the target admits the delivery, as it is
        // while the delivery runs: an admission slower than the lease must not
        // let another executor reclaim the occurrence mid-call. A renewal
        // rewrites the durable occurrence, so the writes below start from the
        // renewed one.
        let lease_duration = self.config.lease_duration;
        let delivered = match existing {
            None => {
                let (delivered, renewed) = run_with_lease_renewal(
                    &self.store,
                    &dispatching,
                    lease_duration,
                    self.delivery
                        .deliver_occurrence(&dispatching, &delivery_identity),
                )
                .await;
                if let Some(renewed) = renewed {
                    dispatching = renewed;
                }
                delivered
            }
            Some(existing) => {
                let (answer, renewed) = run_with_lease_renewal(
                    &self.store,
                    &dispatching,
                    lease_duration,
                    self.delivery.deliver_existing_occurrence(
                        &dispatching,
                        &delivery_identity,
                        existing,
                    ),
                )
                .await;
                if let Some(renewed) = renewed {
                    dispatching = renewed;
                }
                match answer {
                    Ok(ExistingDeliveryAnswer::Dispatch(dispatch)) => Ok(dispatch),
                    Ok(ExistingDeliveryAnswer::NotAdmittedPastDeadline) => {
                        return self
                            .resolve_existing_delivery_not_admitted(
                                dispatching,
                                existing,
                                store_now_utc,
                            )
                            .await;
                    }
                    Err(error) => Err(error),
                }
            }
        };
        let dispatch = match delivered {
            Ok(dispatch) => dispatch,
            Err(ScheduleDomainError::DeliveryRepairDeferred { detail }) => {
                tracing::warn!(
                    occurrence_id = %dispatching.occurrence_id,
                    attempt = dispatching.attempt_count,
                    detail = %detail,
                    "delivery repair deferred before adapter dispatch; leaving durable occurrence reclaimable"
                );
                return Ok(false);
            }
            Err(error) => {
                let detail = error.to_string();
                self.terminalize_occurrence(
                    dispatching,
                    OccurrenceLifecycleInput::ResolveDeliveryFailure {
                        reason: DeliveryFailureReason::TransportError,
                        detail: Some(detail),
                        at_utc: store_now_utc,
                    },
                    None,
                )
                .await?;
                return Ok(true);
            }
        };

        if let Some(materialized_session_id) = dispatch.materialized_session_id.clone() {
            self.service
                .bind_materialized_session_for_occurrence(&dispatching, &materialized_session_id)
                .await?;
            dispatching = self
                .service
                .record_dispatch_materialization(dispatching, &materialized_session_id)
                .await?;
        }

        let admission_outcome =
            validate_dispatch_receipt(&dispatching, &delivery_identity, &dispatch)?;
        if dispatch.receipt.stage == crate::DeliveryReceiptStage::DispatchAccepted {
            let admission_outcome = admission_outcome.ok_or_else(|| {
                ScheduleDomainError::Internal(
                    "dispatch-accepted receipt omitted a successful admission outcome".to_string(),
                )
            })?;
            let Some(accepted_commit) = self
                .store
                .transition_occurrence_with_receipt_if_current(
                    &dispatching.occurrence_id,
                    dispatching.attempt_count,
                    dispatching.claim_token(),
                    OccurrenceLifecycleInput::DispatchAccepted {
                        admission_outcome,
                        at_utc: store_now_utc,
                    },
                    dispatch.receipt.runtime_outcome.clone(),
                )
                .await?
            else {
                return Ok(false);
            };
            let has_dispatch_accepted = accepted_commit
                .effects()
                .iter()
                .any(|effect| matches!(effect, OccurrenceLifecycleEffect::DispatchAccepted));
            // Both admission outcomes leave the occurrence Dispatching. A
            // deduplicated admission proves only that the target admitted
            // this stable identity once: the occurrence waits for that
            // input's own terminal outcome exactly as it waits for an
            // accepted one, through the completion the adapter returned.
            let accepted_shape_is_valid = accepted_commit.successor().phase
                == OccurrencePhase::Dispatching
                && has_dispatch_accepted;
            if !accepted_shape_is_valid || !accepted_commit.supersession_acks().is_empty() {
                return Err(ScheduleDomainError::Internal(
                    "committed dispatch-accepted transition carried an incomplete or invalid lifecycle result"
                        .to_string(),
                ));
            }
            dispatching = accepted_commit.successor().clone();
        }

        let refetched_id = dispatching.occurrence_id.clone();
        let await_mutator = dispatching
            .apply(OccurrenceLifecycleInput::AwaitCompletion {
                at_utc: store_now_utc,
            })
            .map_err(|error| ScheduleDomainError::Internal(error.to_string()))?;
        // 0.7.2 D1/D2a item 5: a schedule-commit sweep can supersede the
        // occurrence between the dispatch commit and the refetch/await-commit.
        // Two sub-cases:
        //
        // a) Refetch returned Superseded → AwaitCompletion is a machine no-op
        //    (AwaitCompletionAfterSupersession). Commit the no-op and proceed to
        //    spawn the waiter; the waiter's delivery resolution will land as a
        //    typed late-arrival record via AlreadySuperseded → fall-through.
        //
        // b) Refetch returned Dispatching (sweep hadn't run yet) but the sweep
        //    ran between refetch and commit → commit_occurrence_write returns
        //    Concurrency. Refetch again; if now Superseded, spawn the waiter
        //    (same late-arrival path); otherwise propagate the real error.
        let dispatching = match self
            .store
            .commit_occurrence_write(await_mutator.into_authorized_write())
            .await
        {
            Ok(commit) => {
                let await_shape_is_valid = match commit.successor().phase {
                    OccurrencePhase::AwaitingCompletion => commit.effects().iter().any(|effect| {
                        matches!(effect, OccurrenceLifecycleEffect::AwaitingCompletion)
                    }),
                    OccurrencePhase::Superseded => {
                        commit.effects().is_empty() && commit.supersession_acks().is_empty()
                    }
                    _ => false,
                };
                if !await_shape_is_valid {
                    return Err(ScheduleDomainError::Internal(
                        "committed await-completion transition carried an incomplete or invalid lifecycle result"
                            .to_string(),
                    ));
                }
                commit.successor().clone()
            }
            Err(ScheduleStoreError::Concurrency(_)) => {
                // The sweep raced the await commit. Re-read the current state.
                let current = self
                    .store
                    .get_occurrence(&refetched_id)
                    .await?
                    .ok_or_else(|| ScheduleStoreError::OccurrenceNotFound {
                        occurrence_id: refetched_id.clone(),
                    })?;
                if current.phase != OccurrencePhase::Superseded {
                    return Err(ScheduleDomainError::Store(ScheduleStoreError::Concurrency(
                        format!(
                            "await-completion commit failed with non-superseded current phase {:?}",
                            current.phase
                        ),
                    )));
                }
                // Benign stop: sweep already revoked the claim. Spawn the
                // waiter so the dispatched delivery's resolution is recorded
                // as a typed late-arrival fact.
                current
            }
            Err(other) => return Err(ScheduleDomainError::Store(other)),
        };

        self.spawn_completion_waiter(dispatching, dispatch.completion);
        Ok(false)
    }

    async fn reconcile_claimed_occurrence_before_dispatch(
        &self,
        occurrence: Occurrence,
    ) -> Result<ClaimedOccurrenceDispatchState, ScheduleDomainError> {
        let current = match self.service.get(&occurrence.schedule_id).await {
            Ok(schedule) => schedule,
            Err(ScheduleDomainError::Store(crate::ScheduleStoreError::ScheduleNotFound {
                ..
            })) => {
                return Err(ScheduleDomainError::Internal(format!(
                    "claimed occurrence references missing schedule {}",
                    occurrence.schedule_id
                )));
            }
            Err(error) => return Err(error),
        };

        // The schedule's current phase and revision are pure observations. The
        // OccurrenceLifecycleMachine — not this driver — classifies the
        // pre-dispatch disposition; we mirror the emitted verdict and fail
        // closed if no disposition is emitted.
        let verdict = occurrence
            .classify_claimed_dispatch_disposition(current.phase, current.revision)
            .map_err(|error| ScheduleDomainError::Internal(error.to_string()))?;

        match verdict.disposition {
            ClaimedDispatchDisposition::FutureRevision => {
                Err(ScheduleDomainError::Internal(format!(
                    "claimed occurrence {} has future revision {} ahead of schedule {}",
                    occurrence.occurrence_id, occurrence.schedule_revision.0, current.revision.0
                )))
            }
            ClaimedDispatchDisposition::Frozen => Ok(ClaimedOccurrenceDispatchState::Frozen),
            ClaimedDispatchDisposition::Supersede => {
                let superseded_by_revision = verdict.superseded_by_revision.ok_or_else(|| {
                    ScheduleDomainError::Internal(
                        "occurrence authority classified Supersede without a superseding revision"
                            .to_string(),
                    )
                })?;
                Ok(ClaimedOccurrenceDispatchState::Supersede {
                    occurrence,
                    superseded_by_revision,
                })
            }
            ClaimedDispatchDisposition::Ready => {
                let occurrence = self
                    .service
                    .sync_occurrence_target_with_schedule(occurrence)
                    .await?;
                Ok(ClaimedOccurrenceDispatchState::Ready(occurrence))
            }
        }
    }

    async fn terminalize_occurrence(
        &self,
        occurrence: Occurrence,
        lifecycle: OccurrenceLifecycleInput,
        receipt: Option<DeliveryReceipt>,
    ) -> Result<(), ScheduleDomainError> {
        let _ =
            terminalize_occurrence_inner(self.store.clone(), occurrence, lifecycle, receipt, None)
                .await?;
        Ok(())
    }

    /// The target holds no admission of a redelivery that had to find one.
    /// An unproven late one ends misfired, as the misfire policy ends a late
    /// occurrence the target never started; nothing was delivered. An
    /// admitted one answering so breaks the trait's contract (it must find
    /// the admission or fail the delivery) and ends delivery_failed.
    async fn resolve_existing_delivery_not_admitted(
        &self,
        dispatching: Occurrence,
        existing: ExistingDelivery,
        store_now_utc: DateTime<Utc>,
    ) -> Result<bool, ScheduleDomainError> {
        let lifecycle = match existing {
            ExistingDelivery::UnprovenLate => {
                OccurrenceLifecycleInput::ResolveUnprovenLateNotAdmitted {
                    admission_evidence: AdmissionEvidence::Started,
                    detail: Some(
                        "dispatch started past the misfire deadline and the target never admitted it"
                            .to_string(),
                    ),
                    at_utc: store_now_utc,
                }
            }
            ExistingDelivery::Admitted => OccurrenceLifecycleInput::ResolveDeliveryFailure {
                reason: DeliveryFailureReason::InternalError,
                detail: Some(
                    "the target answered not admitted for a delivery whose admission is recorded"
                        .to_string(),
                ),
                at_utc: store_now_utc,
            },
        };
        self.terminalize_occurrence(dispatching, lifecycle, None)
            .await?;
        Ok(true)
    }

    /// `existing` is the redelivery the claim named: the occurrence machine
    /// never ends one on a busy target, since it starts no new run.
    async fn resolve_target_probe(
        &self,
        occurrence: Occurrence,
        store_now_utc: DateTime<Utc>,
        existing: Option<ExistingDelivery>,
    ) -> Result<TargetProbeResolution, ScheduleDomainError> {
        let (outcome, detail) = match self.probe.probe_target(&occurrence).await? {
            TargetProbeOutcome::Ready => (OccurrenceTargetProbeOutcome::Ready, None),
            TargetProbeOutcome::Busy { detail } => (
                OccurrenceTargetProbeOutcome::Busy,
                detail.or_else(|| Some("target busy".to_string())),
            ),
            TargetProbeOutcome::Missing { detail } => (
                OccurrenceTargetProbeOutcome::Missing,
                detail.or_else(|| Some("target missing".to_string())),
            ),
        };

        let lifecycle = OccurrenceLifecycleInput::ResolveTargetProbe {
            outcome,
            redelivery: existing,
            detail,
            at_utc: store_now_utc,
        };
        // Predict the terminal phase the generated authority will resolve the
        // probe to so we can route Claimed (no receipt) and Skipped/Misfired
        // (typed delivery receipt) through the correct store method.
        let predicted_phase = occurrence
            .clone()
            .apply(lifecycle.clone())
            .map_err(|error| ScheduleDomainError::Internal(error.to_string()))?
            .occurrence()
            .phase;
        let commit = match predicted_phase {
            OccurrencePhase::Claimed => {
                self.store
                    .transition_occurrence_if_current(
                        &occurrence.occurrence_id,
                        occurrence.attempt_count,
                        occurrence.claim_token(),
                        lifecycle,
                    )
                    .await?
            }
            OccurrencePhase::Skipped | OccurrencePhase::Misfired => {
                self.store
                    .transition_occurrence_with_receipt_if_current(
                        &occurrence.occurrence_id,
                        occurrence.attempt_count,
                        occurrence.claim_token(),
                        lifecycle,
                        None,
                    )
                    .await?
            }
            other => {
                return Err(ScheduleDomainError::Internal(format!(
                    "generated occurrence authority resolved target probe to unsupported phase: {other:?}"
                )));
            }
        };
        let Some(commit) = commit else {
            return Ok(TargetProbeResolution::StaleClaim);
        };
        let effect_is_valid = match commit.successor().phase {
            OccurrencePhase::Claimed => {
                commit.effects().is_empty() && commit.supersession_acks().is_empty()
            }
            OccurrencePhase::Skipped => commit
                .effects()
                .iter()
                .any(|effect| matches!(effect, OccurrenceLifecycleEffect::Skipped)),
            OccurrencePhase::Misfired => commit
                .effects()
                .iter()
                .any(|effect| matches!(effect, OccurrenceLifecycleEffect::Misfired)),
            other => {
                return Err(ScheduleDomainError::Internal(format!(
                    "committed target probe resolved to unsupported phase: {other:?}"
                )));
            }
        };
        if !effect_is_valid {
            return Err(ScheduleDomainError::Internal(format!(
                "committed target probe {:?} successor carried inconsistent lifecycle effects",
                commit.successor().phase
            )));
        }
        let updated = commit.successor().clone();

        match updated.phase {
            OccurrencePhase::Claimed => Ok(TargetProbeResolution::Continue(Box::new(updated))),
            OccurrencePhase::Skipped | OccurrencePhase::Misfired => {
                Ok(TargetProbeResolution::Terminalized)
            }
            other => Err(ScheduleDomainError::Internal(format!(
                "generated occurrence authority resolved target probe to unsupported phase: {other:?}"
            ))),
        }
    }

    fn spawn_completion_waiter(&self, occurrence: Occurrence, completion: DeliveryCompletion) {
        let store = self.store.clone();
        let schedule_id = occurrence.schedule_id.clone();
        let occurrence_id = occurrence.occurrence_id.clone();
        let lease_duration = self.config.lease_duration;
        crate::tokio::spawn(async move {
            // While the delivery runs, keep the machine-owned lease alive at
            // ~lease/2 cadence through the RenewLease authority input. A
            // lease that genuinely expires from here on means this waiter is
            // gone (process death) — reclaim stays legal.
            let completion =
                run_completion_with_lease_renewal(&store, &occurrence, lease_duration, completion)
                    .await;
            if let Err(ScheduleDomainError::DeliveryRepairDeferred { detail }) = &completion {
                tracing::warn!(
                    schedule_id = ?schedule_id,
                    occurrence_id = ?occurrence_id,
                    attempt = occurrence.attempt_count,
                    detail = %detail,
                    "bounded live delivery repair exhausted; stopping lease renewal without terminalizing occurrence"
                );
                return;
            }
            let result = complete_dispatched_occurrence(store, occurrence, completion).await;
            // All legitimate interleaving paths (late arrivals, stale claims,
            // idempotent no-ops) are classified as Ok by the time they reach
            // here. A residual Err is a real internal fault.
            if let Err(error) = result {
                tracing::error!(
                    schedule_id = ?schedule_id,
                    occurrence_id = ?occurrence_id,
                    %error,
                    "completion waiter encountered unexpected fault after totality guard"
                );
            }
        });
    }
}

/// A claim scan that breaks the store contract fails the tick at once. The
/// tick returns the violation, so a fault an earlier claimed occurrence of the
/// same tick hit is logged rather than dropped.
fn claim_scan_contract_violation(
    detail: String,
    earlier_fault: Option<&ScheduleDomainError>,
) -> ScheduleDomainError {
    if let Some(earlier) = earlier_fault {
        tracing::error!(
            error = %earlier,
            "claimed occurrence fault superseded by a claim scan contract violation in the same tick"
        );
    }
    ScheduleDomainError::Internal(detail)
}

fn validate_dispatch_receipt(
    occurrence: &Occurrence,
    identity: &ScheduleDeliveryIdentity,
    dispatch: &DeliveryDispatch,
) -> Result<Option<DeliveryAdmissionOutcome>, ScheduleDomainError> {
    let receipt = &dispatch.receipt;
    if receipt.occurrence_id != occurrence.occurrence_id
        || receipt.attempt != occurrence.attempt_count
    {
        return Err(ScheduleDomainError::Internal(format!(
            "delivery adapter returned receipt for occurrence {} attempt {}, expected {} attempt {}",
            receipt.occurrence_id,
            receipt.attempt,
            occurrence.occurrence_id,
            occurrence.attempt_count
        )));
    }
    let expected_correlation = Some(identity.correlation_id.as_str());
    if receipt.correlation_id.as_deref() != expected_correlation
        || dispatch.correlation_id.as_deref() != expected_correlation
    {
        return Err(ScheduleDomainError::Internal(format!(
            "delivery adapter rewrote stable correlation identity for occurrence {}",
            occurrence.occurrence_id
        )));
    }
    if receipt.materialized_session_id != dispatch.materialized_session_id
        || receipt.materialized_session_id
            != crate::types::target_materialized_session_id(&occurrence.target_snapshot)
    {
        return Err(ScheduleDomainError::Internal(format!(
            "delivery adapter returned materialized target inconsistent with occurrence {}",
            occurrence.occurrence_id
        )));
    }
    match (&receipt.stage, &receipt.runtime_outcome) {
        (
            crate::DeliveryReceiptStage::DispatchAccepted,
            Some(RuntimeDeliveryOutcome::AdmissionAccepted),
        ) => Ok(Some(DeliveryAdmissionOutcome::Accepted)),
        (
            crate::DeliveryReceiptStage::DispatchAccepted,
            Some(RuntimeDeliveryOutcome::AdmissionDeduplicated),
        ) => Ok(Some(DeliveryAdmissionOutcome::Deduplicated)),
        (crate::DeliveryReceiptStage::DispatchStarted, None) => Ok(None),
        (stage, outcome) => Err(ScheduleDomainError::Internal(format!(
            "delivery adapter returned invalid admission receipt stage {stage:?} with outcome {outcome:?}"
        ))),
    }
}

/// Drive the delivery's completion future while renewing the occurrence's
/// lease at ~half the lease duration (2026-07 P0: the fixed 60s lease was
/// never renewed, so every delivery longer than the lease — a routine
/// session turn — was reclaimed mid-flight as a presumed-dead deliverer).
/// The renewal itself is [`run_with_lease_renewal`].
async fn run_completion_with_lease_renewal(
    store: &Arc<dyn ScheduleStore>,
    occurrence: &Occurrence,
    lease_duration: Duration,
    completion: DeliveryCompletion,
) -> Result<DeliveryTerminal, ScheduleDomainError> {
    run_with_lease_renewal(store, occurrence, lease_duration, completion)
        .await
        .0
}

/// Drive `work` while renewing the claimed occurrence's lease at ~half the
/// lease duration. The driver runs both the target's admission call and the
/// delivery's completion inside it, so neither lets the lease lapse while it
/// runs: an admission can outlast the lease too (a cold executor rebuild, a
/// mob Begin, an outcome read inside the call).
///
/// Renewal goes through one store-atomic seam: the store samples its clock,
/// screens the exact attempt/token witness, applies the machine's
/// `RenewLease`, and commits in one critical section/transaction. Transient
/// mechanism failures retry with bounded backoff inside the last proven lease
/// budget; stale evidence or a terminal store failure stops renewal and lets
/// the caller's later commit take the stale-screening path.
///
/// Returns the work's output and, when a renewal committed, the last renewed
/// occurrence. That is the durable row the renewal wrote, so a caller that
/// later writes against the occurrence's exact prior state starts from it.
async fn run_with_lease_renewal<T>(
    store: &Arc<dyn ScheduleStore>,
    occurrence: &Occurrence,
    lease_duration: Duration,
    work: impl Future<Output = T>,
) -> (T, Option<Occurrence>) {
    let mut work = std::pin::pin!(work);
    let mut renewed = None;
    let Some(interval) = lease_renewal_interval(lease_duration) else {
        return (work.await, renewed);
    };
    let Some(claim_token) = occurrence.claim_token() else {
        // No claim token on a dispatching occurrence is unreachable through
        // the claim path; without one the machine cannot authorize renewal.
        return (work.await, renewed);
    };
    let Some(expected_owner_id) = occurrence.claimed_by.as_deref() else {
        tracing::error!(
            occurrence_id = %occurrence.occurrence_id,
            attempt = occurrence.attempt_count,
            "dispatched occurrence omitted durable claim owner; lease cannot be renewed"
        );
        return (work.await, renewed);
    };
    let mut lease_deadline = monotonic_lease_deadline(Utc::now(), occurrence.lease_expires_at_utc)
        .unwrap_or_else(std::time::Instant::now);
    let mut renewal_delay = interval;
    let mut transient_backoff = std::time::Duration::from_millis(50);
    loop {
        let tick = Box::pin(crate::tokio::time::sleep(renewal_delay));
        if let Either::Left((output, _tick)) = future::select(work.as_mut(), tick).await {
            return (output, renewed);
        }
        match renew_lease_once(
            store,
            occurrence,
            claim_token,
            expected_owner_id,
            lease_duration,
        )
        .await
        {
            LeaseRenewalAttempt::Renewed {
                store_now_utc,
                occurrence: successor,
            } => {
                let Some(next_deadline) =
                    monotonic_lease_deadline(store_now_utc, successor.lease_expires_at_utc)
                else {
                    tracing::error!(
                        occurrence_id = %successor.occurrence_id,
                        attempt = successor.attempt_count,
                        "lease renewal committed without a durable expiry"
                    );
                    return (work.await, Some(successor));
                };
                renewed = Some(successor);
                lease_deadline = next_deadline;
                renewal_delay = interval;
                transient_backoff = std::time::Duration::from_millis(50);
            }
            LeaseRenewalAttempt::StaleClaim => {
                tracing::debug!(
                    occurrence_id = %occurrence.occurrence_id,
                    attempt = occurrence.attempt_count,
                    "lease renewal stopped: exact claim evidence is no longer current"
                );
                return (work.await, renewed);
            }
            LeaseRenewalAttempt::Transient(error) => {
                let now = std::time::Instant::now();
                let remaining = lease_deadline.saturating_duration_since(now);
                if remaining.is_zero() {
                    tracing::warn!(
                        occurrence_id = %occurrence.occurrence_id,
                        attempt = occurrence.attempt_count,
                        %error,
                        "lease renewal exhausted the last proven lease budget"
                    );
                    return (work.await, renewed);
                }
                renewal_delay = transient_backoff.min(remaining);
                transient_backoff = transient_backoff
                    .saturating_mul(2)
                    .min(interval)
                    .min(std::time::Duration::from_secs(2));
                tracing::warn!(
                    occurrence_id = %occurrence.occurrence_id,
                    attempt = occurrence.attempt_count,
                    retry_in_ms = renewal_delay.as_millis(),
                    remaining_lease_ms = remaining.as_millis(),
                    %error,
                    "transient lease renewal fault; retrying inside durable lease budget"
                );
            }
            LeaseRenewalAttempt::Fatal(error) => {
                tracing::error!(
                    occurrence_id = %occurrence.occurrence_id,
                    attempt = occurrence.attempt_count,
                    %error,
                    "lease renewal stopped on terminal store or authority fault"
                );
                return (work.await, renewed);
            }
        }
    }
}

/// Renewal cadence: half the lease, so one missed tick still leaves a full
/// half-lease of slack before expiry. `None` disables renewal for
/// non-positive leases.
fn lease_renewal_interval(lease_duration: Duration) -> Option<std::time::Duration> {
    let half_ms = lease_duration.num_milliseconds() / 2;
    if half_ms <= 0 {
        return None;
    }
    u64::try_from(half_ms)
        .ok()
        .map(std::time::Duration::from_millis)
}

fn monotonic_lease_deadline(
    store_now_utc: DateTime<Utc>,
    lease_expires_at_utc: Option<DateTime<Utc>>,
) -> Option<std::time::Instant> {
    let remaining = lease_expires_at_utc? - store_now_utc;
    let remaining = remaining.to_std().unwrap_or_default();
    Some(std::time::Instant::now() + remaining)
}

enum LeaseRenewalAttempt {
    Renewed {
        store_now_utc: DateTime<Utc>,
        occurrence: Occurrence,
    },
    StaleClaim,
    Transient(ScheduleStoreError),
    Fatal(ScheduleStoreError),
}

/// One store-atomic renewal attempt. The driver classifies only the store's
/// typed mechanism outcome; it does not reconstruct time or lease policy.
async fn renew_lease_once(
    store: &Arc<dyn ScheduleStore>,
    occurrence: &Occurrence,
    claim_token: uuid::Uuid,
    expected_owner_id: &str,
    lease_duration: Duration,
) -> LeaseRenewalAttempt {
    let result = store
        .renew_occurrence_lease_if_current(RenewOccurrenceLeaseRequest {
            occurrence_id: occurrence.occurrence_id.clone(),
            expected_attempt: occurrence.attempt_count,
            claim_token,
            expected_owner_id: expected_owner_id.to_string(),
            lease_duration,
        })
        .await;
    match result {
        Ok(result) => match result.outcome {
            RenewOccurrenceLeaseOutcome::Renewed(commit) => {
                if commit
                    .effects()
                    .iter()
                    .any(|effect| matches!(effect, OccurrenceLifecycleEffect::LeaseRenewed))
                {
                    LeaseRenewalAttempt::Renewed {
                        store_now_utc: result.store_now_utc,
                        occurrence: commit.successor().clone(),
                    }
                } else {
                    LeaseRenewalAttempt::Fatal(ScheduleStoreError::Internal(
                        "committed lease renewal lost its LeaseRenewed lifecycle effect"
                            .to_string(),
                    ))
                }
            }
            RenewOccurrenceLeaseOutcome::StaleClaim => LeaseRenewalAttempt::StaleClaim,
        },
        Err(error) if error.is_transient() => LeaseRenewalAttempt::Transient(error),
        Err(error) => LeaseRenewalAttempt::Fatal(error),
    }
}

async fn complete_dispatched_occurrence(
    store: Arc<dyn ScheduleStore>,
    occurrence: Occurrence,
    completion: Result<DeliveryTerminal, ScheduleDomainError>,
) -> Result<(), ScheduleDomainError> {
    let store_now_utc = store.get_store_time_utc().await?;
    let current_schedule = store.get_schedule(&occurrence.schedule_id).await?;
    if let Some(schedule) = current_schedule {
        // The schedule's current phase and revision are pure observations. The
        // OccurrenceLifecycleMachine — not this driver — classifies whether the
        // completed delivery is superseded; we mirror the emitted verdict and
        // fail closed if no disposition is emitted.
        let verdict = occurrence
            .classify_completion_supersession(schedule.phase, schedule.revision)
            .map_err(|error| ScheduleDomainError::Internal(error.to_string()))?;
        match verdict.disposition {
            CompletionSupersessionDisposition::Supersede => {
                let superseded_by_revision = verdict.superseded_by_revision.ok_or_else(|| {
                    ScheduleDomainError::Internal(
                        "occurrence authority classified completion Supersede without a superseding revision"
                            .to_string(),
                    )
                })?;
                let outcome = terminalize_occurrence_inner(
                    store.clone(),
                    occurrence.clone(),
                    OccurrenceLifecycleInput::Supersede {
                        superseded_by_revision,
                        at_utc: store_now_utc,
                    },
                    None,
                    None,
                )
                .await?;
                // The supersession verdict is computed against this waiter's
                // (stale) occurrence snapshot, which still reads
                // `AwaitingCompletion` even though a commit-time supersession
                // sweep (schedule delete/update D1) has already moved the
                // durable row to terminal `Superseded`. The `Supersede`
                // terminalize therefore lands as either `IdempotentNoop`
                // (`SupersedeAlreadySuperseded`, claim evidence still matched)
                // or `StaleClaim` (claim evidence revoked) — both meaning the
                // occurrence is already terminally superseded and this delivery
                // is a late arrival. In that case the delivery's *actual*
                // terminal outcome (e.g. a completed delivery) must still be
                // recorded as a typed late-arrival fact on the Superseded row,
                // so fall through to the delivery-terminal resolution below.
                // Only an `Applied`/`LateRecorded` Supersede committed the
                // terminal itself, fully accounting the completion → return.
                if !matches!(
                    outcome,
                    TerminalizeOutcome::StaleClaim | TerminalizeOutcome::IdempotentNoop
                ) {
                    return Ok(());
                }
            }
            CompletionSupersessionDisposition::Proceed => {}
            // 0.7.2 D2a: the occurrence snapshot is already Superseded (the
            // schedule-commit supersession sweep landed between dispatch and
            // completion). Fall through: the delivery resolution below lands
            // on the occurrence authority's late-arrival transitions as a
            // typed record, never a guard rejection.
            CompletionSupersessionDisposition::AlreadySuperseded => {}
        }
    }

    let terminal = match completion {
        Ok(terminal) => terminal,
        Err(error) => {
            let (reason, detail) = delivery_completion_failure_evidence(error);
            let outcome = terminalize_occurrence_inner(
                store.clone(),
                occurrence.clone(),
                OccurrenceLifecycleInput::ResolveDeliveryCompletionFailure {
                    reason,
                    detail,
                    at_utc: store_now_utc,
                },
                None,
                None,
            )
            .await?;
            if outcome == TerminalizeOutcome::StaleClaim {
                classify_stale_arrival(
                    store,
                    &occurrence.occurrence_id,
                    StaleCompletionArrivalTrigger::ResolveDeliveryCompletionFailure,
                )
                .await;
            }
            return Ok(());
        }
    };

    let (lifecycle, stale_trigger) = if let Some(outcome) = terminal.runtime_completion_outcome {
        (
            OccurrenceLifecycleInput::ResolveRuntimeCompletion {
                outcome,
                detail: terminal.detail.clone(),
                at_utc: store_now_utc,
            },
            StaleCompletionArrivalTrigger::ResolveRuntimeCompletion,
        )
    } else {
        match terminal.phase {
            OccurrencePhase::Completed => (
                OccurrenceLifecycleInput::Complete {
                    at_utc: store_now_utc,
                },
                StaleCompletionArrivalTrigger::Complete,
            ),
            OccurrencePhase::Skipped | OccurrencePhase::Misfired => {
                return Err(ScheduleDomainError::Internal(format!(
                    "delivery terminal returned unsupported adapter-selected occurrence phase: {:?}",
                    terminal.phase
                )));
            }
            OccurrencePhase::DeliveryFailed => (
                OccurrenceLifecycleInput::ResolveDeliveryFailure {
                    reason: terminal.delivery_failure_reason.ok_or_else(|| {
                        ScheduleDomainError::Internal(
                            "delivery failed terminal omitted generated failure reason".to_string(),
                        )
                    })?,
                    detail: terminal.detail.clone(),
                    at_utc: store_now_utc,
                },
                StaleCompletionArrivalTrigger::ResolveDeliveryFailure,
            ),
            other => {
                return Err(ScheduleDomainError::Internal(format!(
                    "delivery terminal returned non-terminal occurrence phase: {other:?}"
                )));
            }
        }
    };

    let outcome = terminalize_occurrence_inner(
        store.clone(),
        occurrence.clone(),
        lifecycle,
        terminal.receipt,
        terminal.runtime_outcome,
    )
    .await?;
    if outcome == TerminalizeOutcome::StaleClaim {
        // 0.7.2 D2a: the claim evidence (attempt count / token) no longer
        // matches the durable row (the occurrence was reclaimed for a new
        // attempt while this waiter's completion was in flight). Record the
        // screened arrival as a typed machine fact on the current row.
        classify_stale_arrival(store, &occurrence.occurrence_id, stale_trigger).await;
    }
    Ok(())
}

/// Feed a `ClassifyStaleCompletionArrival` input to the occurrence authority
/// for an arrival whose claim evidence was stale (0.7.2 D2a). Fetches the
/// current row without a claim precondition, applies the classification, and
/// commits. Never returns an error — the classification is observability-only
/// and must not disrupt the caller's completion path.
async fn classify_stale_arrival(
    store: Arc<dyn ScheduleStore>,
    occurrence_id: &OccurrenceId,
    trigger: StaleCompletionArrivalTrigger,
) {
    let result: Result<(), ScheduleDomainError> = async {
        let Some(current) = store.get_occurrence(occurrence_id).await? else {
            return Ok(());
        };
        let mutator = current
            .apply(OccurrenceLifecycleInput::ClassifyStaleCompletionArrival { trigger })
            .map_err(|error| ScheduleDomainError::Internal(error.to_string()))?;
        // Log the classification effect before consuming the mutator.
        let commit = store
            .commit_occurrence_write(mutator.into_authorized_write())
            .await?;
        let effect = commit
            .effects()
            .iter()
            .find(|e| {
                matches!(
                    e,
                    OccurrenceLifecycleEffect::StaleCompletionArrivalClassified { .. }
                )
            })
            .ok_or_else(|| {
                ScheduleDomainError::Internal(
                    "committed stale-arrival classification lost its generated lifecycle effect"
                        .to_string(),
                )
            })?;
        tracing::debug!(
            occurrence_id = %occurrence_id,
            ?trigger,
            ?effect,
            "stale completion arrival classified as typed machine fact"
        );
        Ok(())
    }
    .await;
    if let Err(error) = result {
        tracing::debug!(
            occurrence_id = %occurrence_id,
            ?trigger,
            %error,
            "stale completion arrival classification could not be committed (concurrent modification)"
        );
    }
}

fn delivery_completion_failure_evidence(
    error: ScheduleDomainError,
) -> (DeliveryCompletionFailureReason, Option<String>) {
    match error {
        ScheduleDomainError::DeliveryCompletionFailed { reason, detail } => (reason, Some(detail)),
        ScheduleDomainError::DeliveryRepairDeferred { detail } => (
            DeliveryCompletionFailureReason::CompletionFutureFailed,
            Some(format!(
                "delivery repair deferral reached terminal completion classification unexpectedly: {detail}"
            )),
        ),
        other => (
            DeliveryCompletionFailureReason::CompletionFutureFailed,
            Some(other.to_string()),
        ),
    }
}

/// Apply a terminal transition to an occurrence through the claim-screened
/// store seam and decide receipt policy from the emitted machine effects.
///
/// Receipt policy (0.7.2 D1/D2a):
/// - `Applied` — normal terminal transition; mint and append the receipt.
/// - `LateRecorded` — transition emitted `LateCompletionResolutionRecorded`
///   meaning the occurrence was already Superseded by the commit-time sweep;
///   that sweep already minted the canonical superseded receipt, so no
///   second receipt is appended here.
/// - `IdempotentNoop` — transition emitted zero effects (e.g.
///   `SupersedeAlreadySuperseded`); the first supersession wins, no receipt.
/// - `StaleClaim` — store screen rejected claim evidence; caller must feed
///   `ClassifyStaleCompletionArrival` to record the stale arrival as a
///   typed machine fact.
async fn terminalize_occurrence_inner(
    store: Arc<dyn ScheduleStore>,
    occurrence: Occurrence,
    lifecycle: OccurrenceLifecycleInput,
    _receipt: Option<DeliveryReceipt>,
    runtime_outcome: Option<RuntimeDeliveryOutcome>,
) -> Result<TerminalizeOutcome, ScheduleDomainError> {
    let _ = _receipt;
    // Route on the *current* durable phase rather than the (possibly stale)
    // waiter snapshot. A commit-time supersession sweep (schedule delete/update,
    // 0.7.2 D1) moves the durable row to `Superseded` *without* changing the
    // claim evidence (`SupersedePendingOrLive` leaves attempt/token intact), so
    // the fresh phase read is authoritative for the genuine-terminal vs.
    // late-after-supersession split, while the claim screen inside the chosen
    // store method still guards the actual commit against a concurrent reclaim.
    let current_phase = store
        .get_occurrence(&occurrence.occurrence_id)
        .await?
        .map(|current| current.phase);

    match current_phase {
        None => {
            // Row gone entirely — a genuine stale arrival.
            Ok(TerminalizeOutcome::StaleClaim)
        }
        Some(OccurrencePhase::Superseded) => {
            // The completion resolved after the supersession sweep already
            // moved the row to Superseded and minted the canonical superseded
            // receipt. Apply the terminal input through the effects-returning
            // claim screen so the occurrence authority records it through its
            // `Late*AfterSupersession` transitions
            // (`LateCompletionResolutionRecorded`) without minting a second
            // receipt. A `Supersede` landing on an already-superseded row is the
            // idempotent no-op (`SupersedeAlreadySuperseded`, zero effects).
            let Some(commit) = store
                .transition_occurrence_if_current(
                    &occurrence.occurrence_id,
                    occurrence.attempt_count,
                    occurrence.claim_token(),
                    lifecycle.clone(),
                )
                .await?
            else {
                // Claim evidence was revoked between the read and the apply;
                // fall back to the current-row late handling (no claim
                // precondition) so a genuine late arrival is still recorded.
                return terminalize_late_completion_on_superseded(store, &occurrence, lifecycle)
                    .await;
            };
            let late_recorded = commit.effects().iter().any(|e| {
                matches!(
                    e,
                    OccurrenceLifecycleEffect::LateCompletionResolutionRecorded { .. }
                )
            });
            if late_recorded {
                Ok(TerminalizeOutcome::LateRecorded)
            } else if commit.effects().is_empty() {
                Ok(TerminalizeOutcome::IdempotentNoop)
            } else {
                // A terminal transition unexpectedly succeeded on a Superseded
                // row (no late-arrival record, non-empty effects). This is not a
                // reachable occurrence-authority transition; surface it as a
                // typed internal fault rather than minting an out-of-band
                // receipt.
                Err(ScheduleDomainError::Internal(
                    "terminal transition resolved on a superseded occurrence without a \
                     late-completion record"
                        .to_string(),
                ))
            }
        }
        Some(_) => {
            // Genuine terminal: the row is still live for the terminal
            // transition. Apply it and mint the canonical receipt atomically
            // inside the store transaction (D1). The receipt is written through
            // the claim-screened store seam, never a separate raw receipt write,
            // so a partial receipt-append failure cannot leave a terminalized
            // occurrence without its receipt.
            let updated = store
                .transition_occurrence_with_receipt_if_current(
                    &occurrence.occurrence_id,
                    occurrence.attempt_count,
                    occurrence.claim_token(),
                    lifecycle,
                    runtime_outcome,
                )
                .await?;
            match updated {
                Some(commit) => {
                    let has_required_effect = match commit.successor().phase {
                        OccurrencePhase::Completed => commit
                            .effects()
                            .iter()
                            .any(|effect| matches!(effect, OccurrenceLifecycleEffect::Completed)),
                        OccurrencePhase::Skipped => commit
                            .effects()
                            .iter()
                            .any(|effect| matches!(effect, OccurrenceLifecycleEffect::Skipped)),
                        OccurrencePhase::Misfired => commit
                            .effects()
                            .iter()
                            .any(|effect| matches!(effect, OccurrenceLifecycleEffect::Misfired)),
                        OccurrencePhase::Superseded => commit
                            .effects()
                            .iter()
                            .any(|effect| matches!(effect, OccurrenceLifecycleEffect::Superseded)),
                        OccurrencePhase::DeliveryFailed => commit.effects().iter().any(|effect| {
                            matches!(effect, OccurrenceLifecycleEffect::DeliveryFailed)
                        }),
                        other => {
                            return Err(ScheduleDomainError::Internal(format!(
                                "terminal transition committed non-terminal successor phase {other:?}"
                            )));
                        }
                    };
                    if !has_required_effect {
                        return Err(ScheduleDomainError::Internal(format!(
                            "terminal transition committed {:?} without its required lifecycle effect",
                            commit.successor().phase
                        )));
                    }
                    Ok(TerminalizeOutcome::Applied)
                }
                None => Ok(TerminalizeOutcome::StaleClaim),
            }
        }
    }
}

/// A completion whose claim evidence was screened out by
/// [`terminalize_occurrence_inner`]'s claim precondition. Refetch the current
/// row and, only when it is terminally `Superseded`, apply the same terminal
/// `lifecycle` input directly on the current row (no claim precondition) so the
/// occurrence authority records it through its `Late*AfterSupersession`
/// transitions (`LateCompletionResolutionRecorded`). The commit-time
/// supersession sweep already minted the canonical superseded receipt, so this
/// records the typed late-arrival fact without minting a second receipt
/// (`LateRecorded`). Any non-`Superseded` phase is a genuine stale arrival and
/// is returned as `StaleClaim` for the caller's stale-arrival classification.
async fn terminalize_late_completion_on_superseded(
    store: Arc<dyn ScheduleStore>,
    occurrence: &Occurrence,
    lifecycle: OccurrenceLifecycleInput,
) -> Result<TerminalizeOutcome, ScheduleDomainError> {
    let Some(current) = store.get_occurrence(&occurrence.occurrence_id).await? else {
        return Ok(TerminalizeOutcome::StaleClaim);
    };
    if current.phase != OccurrencePhase::Superseded {
        return Ok(TerminalizeOutcome::StaleClaim);
    }

    let mutator = current
        .apply(lifecycle)
        .map_err(|error| ScheduleDomainError::Internal(error.to_string()))?;
    let late_recorded = mutator.effects().iter().any(|e| {
        matches!(
            e,
            OccurrenceLifecycleEffect::LateCompletionResolutionRecorded { .. }
        )
    });
    if !late_recorded {
        // The current-row apply produced no late-completion record (e.g. a
        // no-op self-loop): the supersession already accounts for the
        // delivery and there is nothing new to commit. Surface it as the
        // benign stale arrival the claim screen first detected.
        return Ok(TerminalizeOutcome::StaleClaim);
    }
    let commit = store
        .commit_occurrence_write(mutator.into_authorized_write())
        .await?;
    if commit.effects().iter().any(|effect| {
        matches!(
            effect,
            OccurrenceLifecycleEffect::LateCompletionResolutionRecorded { .. }
        )
    }) {
        Ok(TerminalizeOutcome::LateRecorded)
    } else {
        Err(ScheduleDomainError::Internal(
            "committed late completion transition lost its generated lifecycle effect".to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::large_futures, clippy::panic)]

    use super::*;

    async fn test_executor_lease<S: ScheduleStore + ?Sized>(
        store: &Arc<S>,
    ) -> ScheduleExecutorLease {
        match store
            .acquire_executor_lease(AcquireScheduleExecutorLeaseRequest {
                owner_id: format!("test-executor:{}", uuid::Uuid::now_v7()),
                lease_duration: Duration::minutes(5),
            })
            .await
            .expect("acquire test executor lease")
        {
            AcquireScheduleExecutorLeaseOutcome::Acquired(lease) => lease,
            AcquireScheduleExecutorLeaseOutcome::Busy {
                current_owner_id, ..
            } => {
                panic!("test executor lease busy under {current_owner_id}")
            }
        }
    }
    use crate::types::{
        CreateScheduleRequest, DeliveryReceiptStage, IntervalTriggerSpec, OccurrenceFailureClass,
        ScheduledSessionAction, SessionMaterializationSpec, SessionTargetBinding, TargetBinding,
    };
    use crate::{
        MemoryScheduleStore, MisfirePolicy, MissingTargetPolicy, OverlapPolicy, TriggerSpec,
        UpdateScheduleRequest,
    };
    use chrono::Duration;
    use meerkat_core::ContentInput;
    use std::collections::{BTreeMap, BTreeSet};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::{Mutex, oneshot};
    use tokio::time::sleep;
    use uuid::Uuid;

    struct ReadyProbe;

    #[async_trait]
    impl ScheduleTargetProbe for ReadyProbe {
        async fn probe_target(
            &self,
            _occurrence: &Occurrence,
        ) -> Result<TargetProbeOutcome, ScheduleDomainError> {
            Ok(TargetProbeOutcome::Ready)
        }
    }

    struct StaticProbe(TargetProbeOutcome);

    #[async_trait]
    impl ScheduleTargetProbe for StaticProbe {
        async fn probe_target(
            &self,
            _occurrence: &Occurrence,
        ) -> Result<TargetProbeOutcome, ScheduleDomainError> {
            Ok(self.0.clone())
        }
    }

    struct MaterializationFailureDelivery;

    #[async_trait]
    impl ScheduleTargetDelivery for MaterializationFailureDelivery {
        async fn deliver_occurrence(
            &self,
            occurrence: &Occurrence,
            identity: &ScheduleDeliveryIdentity,
        ) -> Result<DeliveryDispatch, ScheduleDomainError> {
            let mut receipt = DeliveryReceipt::new(
                occurrence.occurrence_id.clone(),
                occurrence.attempt_count,
                DeliveryReceiptStage::DispatchStarted,
            );
            receipt.correlation_id = Some(identity.correlation_id.clone());
            Ok(DeliveryDispatch {
                receipt,
                correlation_id: Some(identity.correlation_id.clone()),
                materialized_session_id: None,
                completion: Box::pin(async {
                    Ok(DeliveryTerminal {
                        phase: OccurrencePhase::DeliveryFailed,
                        receipt: None,
                        detail: Some("session creation failed".into()),
                        delivery_failure_reason: Some(
                            DeliveryFailureReason::TargetMaterializationFailed,
                        ),
                        runtime_completion_outcome: None,
                        runtime_outcome: None,
                    })
                }),
            })
        }
    }

    #[derive(Default)]
    struct CompletingDelivery {
        dispatched_occurrences: Arc<Mutex<Vec<crate::OccurrenceId>>>,
    }

    #[async_trait]
    impl ScheduleTargetDelivery for CompletingDelivery {
        async fn deliver_occurrence(
            &self,
            occurrence: &Occurrence,
            identity: &ScheduleDeliveryIdentity,
        ) -> Result<DeliveryDispatch, ScheduleDomainError> {
            self.dispatched_occurrences
                .lock()
                .await
                .push(occurrence.occurrence_id.clone());
            let mut receipt = DeliveryReceipt::new(
                occurrence.occurrence_id.clone(),
                occurrence.attempt_count,
                DeliveryReceiptStage::DispatchAccepted,
            );
            receipt.correlation_id = Some(identity.correlation_id.clone());
            receipt.runtime_outcome = Some(RuntimeDeliveryOutcome::AdmissionAccepted);
            Ok(DeliveryDispatch {
                receipt,
                correlation_id: Some(identity.correlation_id.clone()),
                materialized_session_id: None,
                completion: Box::pin(async { Ok(DeliveryTerminal::completed(None)) }),
            })
        }
    }

    /// A delivery that materializes the occurrence's deterministic session, as
    /// the surface adapter does for a materialize-on-demand target, and
    /// completes at once.
    #[derive(Default)]
    struct MaterializingDelivery;

    #[async_trait]
    impl ScheduleTargetDelivery for MaterializingDelivery {
        async fn deliver_occurrence(
            &self,
            occurrence: &Occurrence,
            identity: &ScheduleDeliveryIdentity,
        ) -> Result<DeliveryDispatch, ScheduleDomainError> {
            let session_id = occurrence.materialized_session_id();
            let mut receipt = DeliveryReceipt::new(
                occurrence.occurrence_id.clone(),
                occurrence.attempt_count,
                DeliveryReceiptStage::DispatchAccepted,
            );
            receipt.correlation_id = Some(identity.correlation_id.clone());
            receipt.materialized_session_id = Some(session_id.clone());
            receipt.runtime_outcome = Some(RuntimeDeliveryOutcome::AdmissionAccepted);
            Ok(DeliveryDispatch {
                receipt,
                correlation_id: Some(identity.correlation_id.clone()),
                materialized_session_id: Some(session_id),
                completion: Box::pin(async { Ok(DeliveryTerminal::completed(None)) }),
            })
        }
    }

    /// The first fire of a materialize-on-demand schedule binds the session it
    /// materialized and completes.
    #[tokio::test]
    async fn a_materializing_first_fire_binds_its_session_and_completes() {
        let store = Arc::new(MemoryScheduleStore::new());
        let shared_store: Arc<dyn ScheduleStore> = store.clone();
        let service = ScheduleService::new(Arc::clone(&shared_store));
        let schedule = service
            .create(CreateScheduleRequest {
                name: Some("materializing".into()),
                description: None,
                trigger: TriggerSpec::Once {
                    due_at_utc: Utc::now() - Duration::seconds(1),
                },
                target: materialize_on_demand_target("scheduled prompt"),
                misfire_policy: MisfirePolicy::Skip,
                overlap_policy: OverlapPolicy::AllowConcurrent,
                missing_target_policy: MissingTargetPolicy::Skip,
                labels: BTreeMap::new(),
                planning_horizon_days: Some(1),
                planning_horizon_occurrences: Some(1),
            })
            .await
            .expect("create schedule");
        let driver = ScheduleDriver::new(
            service.clone(),
            shared_store,
            Arc::new(ReadyProbe),
            Arc::new(MaterializingDelivery),
            "driver-owner",
            ScheduleDriverConfig {
                claim_limit: 8,
                lease_duration: Duration::seconds(30),
            },
        );
        let tick = driver.tick_once().await;
        assert!(tick.is_ok(), "the tick must succeed: {tick:?}");
        let occurrence =
            wait_for_occurrence_phase(&service, &schedule.schedule_id, OccurrencePhase::Completed)
                .await
                .expect("the first fire completes");
        let materialized = occurrence.materialized_session_id();
        assert_eq!(
            crate::types::target_materialized_session_id(&occurrence.target_snapshot),
            Some(materialized.clone()),
            "the occurrence records the session its dispatch materialized"
        );
        let schedule = service.get(&schedule.schedule_id).await.expect("schedule");
        assert_eq!(
            crate::types::target_materialized_session_id(&schedule.target),
            Some(materialized),
            "later fires reuse the materialized session"
        );
    }

    /// Completes like [`CompletingDelivery`], except that the dispatch for an
    /// occurrence of `faulty_schedule` rewrites the stable correlation
    /// identity: an adapter fault the driver refuses.
    struct CorrelationRewritingDelivery {
        faulty_schedule: crate::ScheduleId,
        completing: CompletingDelivery,
    }

    #[async_trait]
    impl ScheduleTargetDelivery for CorrelationRewritingDelivery {
        async fn deliver_occurrence(
            &self,
            occurrence: &Occurrence,
            identity: &ScheduleDeliveryIdentity,
        ) -> Result<DeliveryDispatch, ScheduleDomainError> {
            let mut dispatch = self
                .completing
                .deliver_occurrence(occurrence, identity)
                .await?;
            if occurrence.schedule_id == self.faulty_schedule {
                dispatch.receipt.correlation_id = Some("rewritten".into());
                dispatch.correlation_id = Some("rewritten".into());
            }
            Ok(dispatch)
        }
    }

    #[tokio::test]
    async fn cancellation_waits_for_executor_lease_witness_before_stopping_tick() {
        let store = Arc::new(MemoryScheduleStore::new());
        let shared_store: Arc<dyn ScheduleStore> = store.clone();
        let service = ScheduleService::new(Arc::clone(&shared_store));
        let driver = ScheduleDriver::new(
            service,
            shared_store,
            Arc::new(ReadyProbe),
            Arc::new(CompletingDelivery::default()),
            "cancellation-safe-driver",
            ScheduleDriverConfig {
                claim_limit: 1,
                lease_duration: Duration::seconds(30),
            },
        );

        let report = driver
            .tick_once_until_cancelled(std::future::ready(()))
            .await
            .expect("lease acquisition should complete before cancellation");
        assert!(
            report.is_none(),
            "ready cancellation must stop before schedule work"
        );
        assert!(
            store
                .observe_executor_lease()
                .await
                .expect("executor lease should remain observable")
                .active
                .is_some(),
            "cancellation must not strand a committed lease outside the driver witness"
        );

        driver
            .release_executor_lease()
            .await
            .expect("witnessed executor lease should remain releasable");
        assert!(
            store
                .observe_executor_lease()
                .await
                .expect("released executor lease should remain observable")
                .active
                .is_none()
        );
    }

    #[derive(Default)]
    struct RepairDeferredDelivery {
        attempts: Arc<Mutex<Vec<u32>>>,
    }

    #[async_trait]
    impl ScheduleTargetDelivery for RepairDeferredDelivery {
        async fn deliver_occurrence(
            &self,
            occurrence: &Occurrence,
            identity: &ScheduleDeliveryIdentity,
        ) -> Result<DeliveryDispatch, ScheduleDomainError> {
            self.attempts.lock().await.push(occurrence.attempt_count);
            let mut receipt = DeliveryReceipt::new(
                occurrence.occurrence_id.clone(),
                occurrence.attempt_count,
                DeliveryReceiptStage::DispatchAccepted,
            );
            receipt.correlation_id = Some(identity.correlation_id.clone());
            // This fixture exercises the case where a newly accepted
            // delivery loses its completion authority and must remain
            // reclaimable. A deduplicated admission would wait for its
            // completion the same way: the occurrence machine records no
            // terminal at admission for either outcome.
            receipt.runtime_outcome = Some(RuntimeDeliveryOutcome::AdmissionAccepted);
            Ok(DeliveryDispatch {
                receipt,
                correlation_id: Some(identity.correlation_id.clone()),
                materialized_session_id: None,
                completion: Box::pin(async {
                    Err(ScheduleDomainError::DeliveryRepairDeferred {
                        detail: "durable target terminal unavailable".to_string(),
                    })
                }),
            })
        }
    }

    #[derive(Default)]
    struct ControlledCompletionDelivery {
        senders: Arc<Mutex<Vec<oneshot::Sender<DeliveryTerminal>>>>,
        observed_dispatches: Arc<Mutex<Vec<(Occurrence, ScheduleDeliveryIdentity)>>>,
    }

    #[async_trait]
    impl ScheduleTargetDelivery for ControlledCompletionDelivery {
        async fn deliver_occurrence(
            &self,
            occurrence: &Occurrence,
            identity: &ScheduleDeliveryIdentity,
        ) -> Result<DeliveryDispatch, ScheduleDomainError> {
            self.observed_dispatches
                .lock()
                .await
                .push((occurrence.clone(), identity.clone()));
            let mut receipt = DeliveryReceipt::new(
                occurrence.occurrence_id.clone(),
                occurrence.attempt_count,
                DeliveryReceiptStage::DispatchAccepted,
            );
            receipt.correlation_id = Some(identity.correlation_id.clone());
            receipt.runtime_outcome = Some(RuntimeDeliveryOutcome::AdmissionAccepted);
            let (tx, rx) = oneshot::channel();
            self.senders.lock().await.push(tx);
            Ok(DeliveryDispatch {
                receipt,
                correlation_id: Some(identity.correlation_id.clone()),
                materialized_session_id: None,
                completion: Box::pin(async move {
                    rx.await.map_err(|_| ScheduleDomainError::DriverStopped)
                }),
            })
        }
    }

    #[derive(Default)]
    struct CountingProbe {
        calls: Arc<Mutex<u32>>,
    }

    #[async_trait]
    impl ScheduleTargetProbe for CountingProbe {
        async fn probe_target(
            &self,
            _occurrence: &Occurrence,
        ) -> Result<TargetProbeOutcome, ScheduleDomainError> {
            *self.calls.lock().await += 1;
            Ok(TargetProbeOutcome::Ready)
        }
    }

    #[derive(Default)]
    struct CountingDelivery {
        calls: Arc<Mutex<u32>>,
    }

    #[async_trait]
    impl ScheduleTargetDelivery for CountingDelivery {
        async fn deliver_occurrence(
            &self,
            occurrence: &Occurrence,
            identity: &ScheduleDeliveryIdentity,
        ) -> Result<DeliveryDispatch, ScheduleDomainError> {
            *self.calls.lock().await += 1;
            let mut receipt = DeliveryReceipt::new(
                occurrence.occurrence_id.clone(),
                occurrence.attempt_count,
                DeliveryReceiptStage::DispatchAccepted,
            );
            receipt.correlation_id = Some(identity.correlation_id.clone());
            receipt.runtime_outcome = Some(RuntimeDeliveryOutcome::AdmissionAccepted);
            Ok(DeliveryDispatch {
                receipt,
                correlation_id: Some(identity.correlation_id.clone()),
                materialized_session_id: None,
                completion: Box::pin(async { Ok(DeliveryTerminal::completed(None)) }),
            })
        }
    }

    #[derive(Default)]
    struct IdempotentDelivery {
        effect_keys: Arc<Mutex<BTreeSet<String>>>,
        external_effect_count: Arc<Mutex<usize>>,
        admissions: Arc<Mutex<Vec<RuntimeDeliveryOutcome>>>,
        /// When set, a deduplicated replay answers with the original
        /// effect's own Completed terminal instead of an unavailable
        /// completion authority.
        replay_answers_original_completion: bool,
    }

    #[async_trait]
    impl ScheduleTargetDelivery for IdempotentDelivery {
        async fn deliver_occurrence(
            &self,
            occurrence: &Occurrence,
            identity: &ScheduleDeliveryIdentity,
        ) -> Result<DeliveryDispatch, ScheduleDomainError> {
            let inserted = self
                .effect_keys
                .lock()
                .await
                .insert(identity.idempotency_key.clone());
            if inserted {
                *self.external_effect_count.lock().await += 1;
            }
            let admission = if inserted {
                RuntimeDeliveryOutcome::AdmissionAccepted
            } else {
                RuntimeDeliveryOutcome::AdmissionDeduplicated
            };
            let admission_was_deduplicated =
                admission == RuntimeDeliveryOutcome::AdmissionDeduplicated;
            let replay_answers_original_completion = self.replay_answers_original_completion;
            self.admissions.lock().await.push(admission.clone());
            let mut receipt = DeliveryReceipt::new(
                occurrence.occurrence_id.clone(),
                occurrence.attempt_count,
                DeliveryReceiptStage::DispatchAccepted,
            );
            receipt.correlation_id = Some(identity.correlation_id.clone());
            receipt.runtime_outcome = Some(admission);
            Ok(DeliveryDispatch {
                receipt,
                correlation_id: Some(identity.correlation_id.clone()),
                materialized_session_id: None,
                completion: Box::pin(async move {
                    if admission_was_deduplicated && !replay_answers_original_completion {
                        Err(ScheduleDomainError::DeliveryCompletionFailed {
                            reason: DeliveryCompletionFailureReason::RuntimeCompletionAuthorityUnavailable,
                            detail: "original completion channel did not survive the crash"
                                .to_string(),
                        })
                    } else {
                        Ok(DeliveryTerminal::completed(None))
                    }
                }),
            })
        }
    }

    /// A target that admits one input per stable idempotency key and settles
    /// it only when the test says so. A redelivery under the same key
    /// deduplicates and returns a completion that waits for that same
    /// existing input, the way a runtime-backed host attaches a replay to the
    /// original input's own outcome.
    #[derive(Default)]
    struct KeyedInputDelivery {
        inputs: Arc<Mutex<BTreeMap<String, tokio::sync::watch::Sender<Option<DeliveryTerminal>>>>>,
        admissions: Arc<Mutex<Vec<RuntimeDeliveryOutcome>>>,
    }

    impl KeyedInputDelivery {
        /// Settle the input admitted under `idempotency_key` with `terminal`.
        async fn settle(&self, idempotency_key: &str, terminal: DeliveryTerminal) {
            let inputs = self.inputs.lock().await;
            let input = inputs
                .get(idempotency_key)
                .expect("the settled input was admitted under its stable key");
            input.send_replace(Some(terminal));
        }

        async fn wait_for_admission_count(&self, expected: usize) {
            for _ in 0..50 {
                if self.admissions.lock().await.len() >= expected {
                    return;
                }
                sleep(std::time::Duration::from_millis(10)).await;
            }
            panic!("timed out waiting for {expected} keyed input admissions");
        }
    }

    #[async_trait]
    impl ScheduleTargetDelivery for KeyedInputDelivery {
        async fn deliver_occurrence(
            &self,
            occurrence: &Occurrence,
            identity: &ScheduleDeliveryIdentity,
        ) -> Result<DeliveryDispatch, ScheduleDomainError> {
            let (admission, mut terminal_rx) = {
                let mut inputs = self.inputs.lock().await;
                match inputs.get(&identity.idempotency_key) {
                    Some(existing) => (
                        RuntimeDeliveryOutcome::AdmissionDeduplicated,
                        existing.subscribe(),
                    ),
                    None => {
                        let (terminal_tx, terminal_rx) = tokio::sync::watch::channel(None);
                        inputs.insert(identity.idempotency_key.clone(), terminal_tx);
                        (RuntimeDeliveryOutcome::AdmissionAccepted, terminal_rx)
                    }
                }
            };
            self.admissions.lock().await.push(admission.clone());
            let mut receipt = DeliveryReceipt::new(
                occurrence.occurrence_id.clone(),
                occurrence.attempt_count,
                DeliveryReceiptStage::DispatchAccepted,
            );
            receipt.correlation_id = Some(identity.correlation_id.clone());
            receipt.runtime_outcome = Some(admission);
            Ok(DeliveryDispatch {
                receipt,
                correlation_id: Some(identity.correlation_id.clone()),
                materialized_session_id: None,
                completion: Box::pin(async move {
                    loop {
                        let settled = terminal_rx.borrow_and_update().clone();
                        if let Some(terminal) = settled {
                            return Ok(terminal);
                        }
                        if terminal_rx.changed().await.is_err() {
                            return Err(ScheduleDomainError::DriverStopped);
                        }
                    }
                }),
            })
        }
    }

    /// Wrapper store for the per-row-tolerance driver test: injects typed
    /// row faults into the bounded refill page and claim result, and refuses
    /// one candidate's refill acknowledgement.
    struct RowFaultInjectingStore {
        inner: Arc<dyn ScheduleStore>,
        refill_poisoned: crate::ScheduleId,
        transient_renewals_remaining: Arc<AtomicUsize>,
        renewal_attempts: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl ScheduleStore for RowFaultInjectingStore {
        fn kind(&self) -> crate::ScheduleStoreKind {
            self.inner.kind()
        }

        fn wake_mode(&self) -> crate::ScheduleStoreWakeMode {
            self.inner.wake_mode()
        }

        async fn wait_for_durable_wake(&self) -> Result<(), ScheduleStoreError> {
            self.inner.wait_for_durable_wake().await
        }

        async fn acquire_executor_lease(
            &self,
            request: crate::AcquireScheduleExecutorLeaseRequest,
        ) -> Result<crate::AcquireScheduleExecutorLeaseOutcome, ScheduleStoreError> {
            self.inner.acquire_executor_lease(request).await
        }

        async fn renew_executor_lease(
            &self,
            request: crate::RenewScheduleExecutorLeaseRequest,
        ) -> Result<crate::RenewScheduleExecutorLeaseOutcome, ScheduleStoreError> {
            self.inner.renew_executor_lease(request).await
        }

        async fn release_executor_lease(
            &self,
            lease: crate::ScheduleExecutorLease,
        ) -> Result<(), ScheduleStoreError> {
            self.inner.release_executor_lease(lease).await
        }

        async fn get_store_time_utc(&self) -> Result<DateTime<Utc>, ScheduleStoreError> {
            self.inner.get_store_time_utc().await
        }

        async fn next_action_time_utc(
            &self,
        ) -> Result<crate::ScheduleStoreActionTime, ScheduleStoreError> {
            self.inner.next_action_time_utc().await
        }

        async fn read_due_refill_candidates(
            &self,
            limit: usize,
        ) -> Result<crate::ScheduleRefillBatch, ScheduleStoreError> {
            let mut batch = self.inner.read_due_refill_candidates(limit).await?;
            let schedule = self
                .inner
                .get_schedule(&self.refill_poisoned)
                .await?
                .expect("injected refill schedule");
            let pending_occurrences = self
                .inner
                .list_occurrences(crate::OccurrenceFilter {
                    schedule_id: Some(schedule.schedule_id.clone()),
                    phase: Some(OccurrencePhase::Pending),
                    include_terminal: false,
                    ..crate::OccurrenceFilter::default()
                })
                .await?;
            batch.candidates.push(crate::ScheduleRefillCandidate {
                schedule,
                pending_occurrences,
                refill_at_utc: batch.store_now_utc,
            });
            batch.row_faults.push(crate::ScheduleStoreRowFault {
                schedule_id: Some("poisoned-schedule-row".to_string()),
                occurrence_id: None,
                kind: crate::ScheduleStoreRowFaultKind::Deserialization,
                detail: "injected schedule row fault".to_string(),
            });
            Ok(batch)
        }

        async fn commit_schedule_write(
            &self,
            write: crate::AuthorizedScheduleWrite,
        ) -> Result<crate::ScheduleTransitionCommit, ScheduleStoreError> {
            self.inner.commit_schedule_write(write).await
        }

        async fn get_schedule(
            &self,
            schedule_id: &crate::ScheduleId,
        ) -> Result<Option<crate::Schedule>, ScheduleStoreError> {
            if schedule_id == &self.refill_poisoned {
                return Err(ScheduleStoreError::Internal(
                    "injected refill poison".to_string(),
                ));
            }
            self.inner.get_schedule(schedule_id).await
        }

        async fn list_schedules(
            &self,
            filter: crate::ScheduleFilter,
        ) -> Result<Vec<crate::Schedule>, ScheduleStoreError> {
            self.inner.list_schedules(filter).await
        }

        async fn list_schedules_with_row_faults(
            &self,
            _filter: crate::ScheduleFilter,
        ) -> Result<(Vec<crate::Schedule>, Vec<crate::ScheduleStoreRowFault>), ScheduleStoreError>
        {
            Err(ScheduleStoreError::Internal(
                "ordinary driver tick must not call the operator list API".to_string(),
            ))
        }

        async fn commit_occurrence_write(
            &self,
            write: crate::AuthorizedOccurrenceWrite,
        ) -> Result<crate::OccurrenceTransitionCommit, ScheduleStoreError> {
            self.inner.commit_occurrence_write(write).await
        }

        async fn commit_schedule_mutation(
            &self,
            schedule: crate::AuthorizedScheduleWrite,
            occurrences: Vec<crate::AuthorizedOccurrenceWrite>,
        ) -> Result<crate::ScheduleMutationCommit, ScheduleStoreError> {
            self.inner
                .commit_schedule_mutation(schedule, occurrences)
                .await
        }

        async fn commit_schedule_refill(
            &self,
            schedule: crate::AuthorizedScheduleWrite,
            occurrences: Vec<crate::AuthorizedOccurrenceWrite>,
            next_refill_at_utc: Option<DateTime<Utc>>,
        ) -> Result<crate::ScheduleMutationCommit, ScheduleStoreError> {
            self.inner
                .commit_schedule_refill(schedule, occurrences, next_refill_at_utc)
                .await
        }

        async fn record_refill_deadline_if_current(
            &self,
            schedule_id: &crate::ScheduleId,
            expected_revision: crate::ScheduleRevision,
            expected_refill_at_utc: DateTime<Utc>,
            next_refill_at_utc: Option<DateTime<Utc>>,
        ) -> Result<(), ScheduleStoreError> {
            if schedule_id == &self.refill_poisoned {
                return Err(ScheduleStoreError::Internal(
                    "injected refill poison".to_string(),
                ));
            }
            self.inner
                .record_refill_deadline_if_current(
                    schedule_id,
                    expected_revision,
                    expected_refill_at_utc,
                    next_refill_at_utc,
                )
                .await
        }

        async fn get_occurrence(
            &self,
            occurrence_id: &crate::OccurrenceId,
        ) -> Result<Option<Occurrence>, ScheduleStoreError> {
            self.inner.get_occurrence(occurrence_id).await
        }

        async fn list_occurrences(
            &self,
            filter: crate::OccurrenceFilter,
        ) -> Result<Vec<Occurrence>, ScheduleStoreError> {
            self.inner.list_occurrences(filter).await
        }

        async fn list_receipts(
            &self,
            occurrence_id: &crate::OccurrenceId,
        ) -> Result<Vec<DeliveryReceipt>, ScheduleStoreError> {
            self.inner.list_receipts(occurrence_id).await
        }

        async fn claim_due_occurrences(
            &self,
            lease: &crate::ScheduleExecutorLease,
            request: ClaimDueRequest,
        ) -> Result<crate::ClaimDueResult, ScheduleStoreError> {
            let mut result = self.inner.claim_due_occurrences(lease, request).await?;
            result.row_faults.push(crate::ScheduleStoreRowFault {
                schedule_id: Some("poisoned-schedule-row".to_string()),
                occurrence_id: Some("poisoned-occurrence-row".to_string()),
                kind: crate::ScheduleStoreRowFaultKind::Deserialization,
                detail: "injected occurrence row fault".to_string(),
            });
            Ok(result)
        }

        async fn renew_occurrence_lease_if_current(
            &self,
            request: crate::RenewOccurrenceLeaseRequest,
        ) -> Result<crate::RenewOccurrenceLeaseResult, ScheduleStoreError> {
            self.renewal_attempts.fetch_add(1, Ordering::SeqCst);
            if self
                .transient_renewals_remaining
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                    remaining.checked_sub(1)
                })
                .is_ok()
            {
                return Err(ScheduleStoreError::Transient(
                    "injected renewal contention".to_string(),
                ));
            }
            self.inner.renew_occurrence_lease_if_current(request).await
        }

        async fn transition_occurrence_if_current(
            &self,
            occurrence_id: &crate::OccurrenceId,
            expected_attempt: u32,
            expected_claim_token: Option<Uuid>,
            transition: OccurrenceLifecycleInput,
        ) -> Result<Option<crate::OccurrenceTransitionCommit>, ScheduleStoreError> {
            self.inner
                .transition_occurrence_if_current(
                    occurrence_id,
                    expected_attempt,
                    expected_claim_token,
                    transition,
                )
                .await
        }

        async fn transition_occurrence_with_receipt_if_current(
            &self,
            occurrence_id: &crate::OccurrenceId,
            expected_attempt: u32,
            expected_claim_token: Option<Uuid>,
            transition: OccurrenceLifecycleInput,
            runtime_outcome: Option<crate::RuntimeDeliveryOutcome>,
        ) -> Result<Option<crate::OccurrenceTransitionCommit>, ScheduleStoreError> {
            self.inner
                .transition_occurrence_with_receipt_if_current(
                    occurrence_id,
                    expected_attempt,
                    expected_claim_token,
                    transition,
                    runtime_outcome,
                )
                .await
        }
    }

    /// Asks 16+17: one poisoned schedule row, one poisoned occurrence row,
    /// and one schedule whose refill errors must each surface as typed
    /// faults in the tick report while every healthy neighbor still plans
    /// and claims — the tick itself succeeds.
    #[tokio::test]
    async fn tick_reports_row_faults_and_still_services_healthy_schedules() {
        let memory = Arc::new(MemoryScheduleStore::default());
        let bootstrap_service = ScheduleService::new(memory.clone());
        let row_fault_create_request = |name: &str, start_at_utc| CreateScheduleRequest {
            name: Some(name.into()),
            description: None,
            trigger: TriggerSpec::Interval(IntervalTriggerSpec {
                start_at_utc,
                every_seconds: 60,
                end_at_utc: None,
            }),
            target: materialize_on_demand_target("scheduled prompt"),
            misfire_policy: MisfirePolicy::Skip,
            overlap_policy: OverlapPolicy::AllowConcurrent,
            missing_target_policy: MissingTargetPolicy::MarkMisfired,
            labels: BTreeMap::new(),
            planning_horizon_days: Some(1),
            planning_horizon_occurrences: Some(1),
        };
        let healthy = bootstrap_service
            .create(row_fault_create_request("healthy", Utc::now()))
            .await
            .expect("create healthy schedule");
        let refill_poisoned = bootstrap_service
            // A future trigger: this schedule exists to fail its refill, so it
            // must have nothing claimable that would reach the dispatch path.
            .create(row_fault_create_request(
                "refill-poisoned",
                Utc::now() + Duration::hours(1),
            ))
            .await
            .expect("create refill-poisoned schedule");

        let store: Arc<dyn ScheduleStore> = Arc::new(RowFaultInjectingStore {
            inner: memory,
            refill_poisoned: refill_poisoned.schedule_id.clone(),
            transient_renewals_remaining: Arc::new(AtomicUsize::new(0)),
            renewal_attempts: Arc::new(AtomicUsize::new(0)),
        });
        let service = ScheduleService::new(store.clone());
        let delivery = Arc::new(CompletingDelivery::default());
        let driver = ScheduleDriver::new(
            service,
            store,
            Arc::new(ReadyProbe),
            delivery,
            "driver-owner",
            ScheduleDriverConfig {
                claim_limit: 8,
                lease_duration: Duration::seconds(30),
            },
        );

        let report = driver
            .tick_once()
            .await
            .expect("tick must succeed despite per-row faults");
        assert_eq!(
            report.claimed_occurrences, 1,
            "the healthy schedule's due occurrence must still claim: {report:?}"
        );
        assert_eq!(report.schedule_row_faults.len(), 1);
        assert_eq!(
            report.schedule_row_faults[0].schedule_id.as_deref(),
            Some("poisoned-schedule-row")
        );
        assert_eq!(report.occurrence_row_faults.len(), 1);
        assert_eq!(
            report.occurrence_row_faults[0].occurrence_id.as_deref(),
            Some("poisoned-occurrence-row")
        );
        assert_eq!(report.refill_faults.len(), 1);
        assert_eq!(
            report.refill_faults[0].schedule_id,
            refill_poisoned.schedule_id
        );
        assert!(
            report
                .refill_faults
                .iter()
                .all(|fault| fault.schedule_id != healthy.schedule_id),
            "the healthy schedule must not fault"
        );
        assert!(report.fault_count() == 3);
        assert!(!report.fault_fingerprint().is_empty());
    }

    /// A delivery adapter fault on one claimed occurrence fails the tick only
    /// after the rest of the claimed batch is delivered: the other due
    /// occurrence still fires, and the tick returns the fault.
    #[tokio::test]
    async fn adapter_fault_on_one_claimed_occurrence_still_delivers_the_rest_of_the_batch()
    -> Result<(), ScheduleDomainError> {
        let store = Arc::new(MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let service = ScheduleService::new(store.clone());
        let once = |name: &str, due_at_utc| CreateScheduleRequest {
            name: Some(name.into()),
            description: None,
            trigger: TriggerSpec::Once { due_at_utc },
            target: materialize_on_demand_target("scheduled prompt"),
            misfire_policy: MisfirePolicy::Skip,
            overlap_policy: OverlapPolicy::AllowConcurrent,
            missing_target_policy: MissingTargetPolicy::MarkMisfired,
            labels: BTreeMap::new(),
            planning_horizon_days: Some(1),
            planning_horizon_occurrences: Some(1),
        };
        // Due first, so claimed first.
        let faulty = service
            .create(once("faulty", Utc::now() - Duration::seconds(10)))
            .await?;
        let healthy = service
            .create(once("healthy", Utc::now() - Duration::seconds(1)))
            .await?;
        let faulty_occurrence = service
            .list_occurrences(&faulty.schedule_id)
            .await?
            .into_iter()
            .next()
            .expect("planned faulty occurrence");
        let delivery = Arc::new(CorrelationRewritingDelivery {
            faulty_schedule: faulty.schedule_id.clone(),
            completing: CompletingDelivery::default(),
        });
        let driver = ScheduleDriver::new(
            service.clone(),
            store,
            Arc::new(ReadyProbe),
            delivery.clone(),
            "driver-owner",
            ScheduleDriverConfig {
                claim_limit: 8,
                lease_duration: Duration::seconds(30),
            },
        );

        let tick = driver.tick_once().await;

        let dispatched = delivery
            .completing
            .dispatched_occurrences
            .lock()
            .await
            .clone();
        assert_eq!(
            dispatched.len(),
            2,
            "both claimed occurrences reach delivery: {dispatched:?}"
        );
        assert_eq!(dispatched[0], faulty_occurrence.occurrence_id);
        let fired =
            wait_for_occurrence_phase(&service, &healthy.schedule_id, OccurrencePhase::Completed)
                .await?;
        assert_eq!(dispatched[1], fired.occurrence_id);
        match tick {
            Err(ScheduleDomainError::Internal(detail)) => assert!(
                detail.contains(&faulty_occurrence.occurrence_id.to_string()),
                "the tick fails with the faulty occurrence's error: {detail}"
            ),
            other => panic!("the adapter fault must still fail the tick: {other:?}"),
        }
        Ok(())
    }

    /// Full diagnostic fingerprints remain deterministic for callers that
    /// need exact fault-set comparison. Host rate limiting deliberately uses
    /// a stable class instead.
    #[test]
    fn fault_fingerprint_is_stable_for_identical_fault_sets() {
        let fault = crate::ScheduleStoreRowFault {
            schedule_id: Some("sched-1".to_string()),
            occurrence_id: Some("occ-1".to_string()),
            kind: crate::ScheduleStoreRowFaultKind::Deserialization,
            detail: "poisoned row".to_string(),
        };
        let refill = ScheduleRefillFault {
            schedule_id: crate::ScheduleId::new(),
            detail: "refill failed".to_string(),
        };
        let report_a = ScheduleTickReport {
            schedule_row_faults: vec![fault.clone()],
            occurrence_row_faults: vec![fault.clone()],
            refill_faults: vec![refill.clone()],
            ..ScheduleTickReport::default()
        };
        let report_b = ScheduleTickReport {
            schedule_row_faults: vec![fault.clone()],
            occurrence_row_faults: vec![fault.clone()],
            refill_faults: vec![refill],
            ..ScheduleTickReport::default()
        };
        assert_eq!(
            report_a.fault_fingerprint(),
            report_b.fault_fingerprint(),
            "identical fault sets must fingerprint identically across ticks"
        );

        let mut changed = ScheduleTickReport {
            schedule_row_faults: vec![fault],
            ..ScheduleTickReport::default()
        };
        changed.schedule_row_faults[0].detail = "different failure".to_string();
        assert_ne!(
            report_a.fault_fingerprint(),
            changed.fault_fingerprint(),
            "a changed fault set must change the fingerprint"
        );
    }

    #[test]
    fn bounded_fault_summary_caps_rows_and_payload_size() {
        let fault = crate::ScheduleStoreRowFault {
            schedule_id: Some("sched-1".to_string()),
            occurrence_id: Some("occ-1".to_string()),
            kind: crate::ScheduleStoreRowFaultKind::Deserialization,
            detail: "x".repeat(1_000),
        };
        let report = ScheduleTickReport {
            occurrence_row_faults: vec![fault.clone(), fault.clone(), fault],
            ..ScheduleTickReport::default()
        };

        let summary = report.bounded_fault_summary(2, 32);
        assert!(summary.contains("occ-1"), "{summary}");
        assert!(
            summary.contains("1 additional fault(s) omitted"),
            "{summary}"
        );
        assert!(summary.len() < 160, "summary was not bounded: {summary}");
    }

    struct StandaloneReceiptFailingStore {
        inner: Arc<dyn ScheduleStore>,
    }

    #[async_trait]
    impl ScheduleStore for StandaloneReceiptFailingStore {
        fn kind(&self) -> crate::ScheduleStoreKind {
            self.inner.kind()
        }

        fn wake_mode(&self) -> crate::ScheduleStoreWakeMode {
            self.inner.wake_mode()
        }

        async fn wait_for_durable_wake(&self) -> Result<(), ScheduleStoreError> {
            self.inner.wait_for_durable_wake().await
        }

        async fn acquire_executor_lease(
            &self,
            request: crate::AcquireScheduleExecutorLeaseRequest,
        ) -> Result<crate::AcquireScheduleExecutorLeaseOutcome, ScheduleStoreError> {
            self.inner.acquire_executor_lease(request).await
        }

        async fn renew_executor_lease(
            &self,
            request: crate::RenewScheduleExecutorLeaseRequest,
        ) -> Result<crate::RenewScheduleExecutorLeaseOutcome, ScheduleStoreError> {
            self.inner.renew_executor_lease(request).await
        }

        async fn release_executor_lease(
            &self,
            lease: crate::ScheduleExecutorLease,
        ) -> Result<(), ScheduleStoreError> {
            self.inner.release_executor_lease(lease).await
        }

        async fn get_store_time_utc(&self) -> Result<DateTime<Utc>, ScheduleStoreError> {
            self.inner.get_store_time_utc().await
        }

        async fn next_action_time_utc(
            &self,
        ) -> Result<crate::ScheduleStoreActionTime, ScheduleStoreError> {
            self.inner.next_action_time_utc().await
        }

        async fn read_due_refill_candidates(
            &self,
            limit: usize,
        ) -> Result<crate::ScheduleRefillBatch, ScheduleStoreError> {
            self.inner.read_due_refill_candidates(limit).await
        }

        async fn commit_schedule_write(
            &self,
            write: crate::AuthorizedScheduleWrite,
        ) -> Result<crate::ScheduleTransitionCommit, ScheduleStoreError> {
            self.inner.commit_schedule_write(write).await
        }

        async fn get_schedule(
            &self,
            schedule_id: &crate::ScheduleId,
        ) -> Result<Option<crate::Schedule>, ScheduleStoreError> {
            self.inner.get_schedule(schedule_id).await
        }

        async fn list_schedules(
            &self,
            filter: crate::ScheduleFilter,
        ) -> Result<Vec<crate::Schedule>, ScheduleStoreError> {
            self.inner.list_schedules(filter).await
        }

        async fn commit_occurrence_write(
            &self,
            write: crate::AuthorizedOccurrenceWrite,
        ) -> Result<crate::OccurrenceTransitionCommit, ScheduleStoreError> {
            self.inner.commit_occurrence_write(write).await
        }

        async fn commit_occurrence_writes(
            &self,
            writes: Vec<crate::AuthorizedOccurrenceWrite>,
        ) -> Result<Vec<crate::OccurrenceTransitionCommit>, ScheduleStoreError> {
            self.inner.commit_occurrence_writes(writes).await
        }

        async fn commit_schedule_mutation(
            &self,
            schedule: crate::AuthorizedScheduleWrite,
            occurrences: Vec<crate::AuthorizedOccurrenceWrite>,
        ) -> Result<crate::ScheduleMutationCommit, ScheduleStoreError> {
            self.inner
                .commit_schedule_mutation(schedule, occurrences)
                .await
        }

        async fn commit_schedule_refill(
            &self,
            schedule: crate::AuthorizedScheduleWrite,
            occurrences: Vec<crate::AuthorizedOccurrenceWrite>,
            next_refill_at_utc: Option<DateTime<Utc>>,
        ) -> Result<crate::ScheduleMutationCommit, ScheduleStoreError> {
            self.inner
                .commit_schedule_refill(schedule, occurrences, next_refill_at_utc)
                .await
        }

        async fn record_refill_deadline_if_current(
            &self,
            schedule_id: &crate::ScheduleId,
            expected_revision: crate::ScheduleRevision,
            expected_refill_at_utc: DateTime<Utc>,
            next_refill_at_utc: Option<DateTime<Utc>>,
        ) -> Result<(), ScheduleStoreError> {
            self.inner
                .record_refill_deadline_if_current(
                    schedule_id,
                    expected_revision,
                    expected_refill_at_utc,
                    next_refill_at_utc,
                )
                .await
        }

        async fn get_occurrence(
            &self,
            occurrence_id: &crate::OccurrenceId,
        ) -> Result<Option<Occurrence>, ScheduleStoreError> {
            self.inner.get_occurrence(occurrence_id).await
        }

        async fn list_occurrences(
            &self,
            filter: crate::OccurrenceFilter,
        ) -> Result<Vec<Occurrence>, ScheduleStoreError> {
            self.inner.list_occurrences(filter).await
        }

        async fn list_receipts(
            &self,
            occurrence_id: &crate::OccurrenceId,
        ) -> Result<Vec<DeliveryReceipt>, ScheduleStoreError> {
            self.inner.list_receipts(occurrence_id).await
        }

        async fn claim_due_occurrences(
            &self,
            lease: &crate::ScheduleExecutorLease,
            request: ClaimDueRequest,
        ) -> Result<crate::ClaimDueResult, ScheduleStoreError> {
            self.inner.claim_due_occurrences(lease, request).await
        }

        async fn renew_occurrence_lease_if_current(
            &self,
            request: crate::RenewOccurrenceLeaseRequest,
        ) -> Result<crate::RenewOccurrenceLeaseResult, ScheduleStoreError> {
            self.inner.renew_occurrence_lease_if_current(request).await
        }

        async fn transition_occurrence_if_current(
            &self,
            occurrence_id: &crate::OccurrenceId,
            expected_attempt: u32,
            expected_claim_token: Option<Uuid>,
            transition: OccurrenceLifecycleInput,
        ) -> Result<Option<crate::OccurrenceTransitionCommit>, ScheduleStoreError> {
            self.inner
                .transition_occurrence_if_current(
                    occurrence_id,
                    expected_attempt,
                    expected_claim_token,
                    transition,
                )
                .await
        }

        async fn transition_occurrence_with_receipt_if_current(
            &self,
            occurrence_id: &crate::OccurrenceId,
            expected_attempt: u32,
            expected_claim_token: Option<Uuid>,
            transition: OccurrenceLifecycleInput,
            runtime_outcome: Option<RuntimeDeliveryOutcome>,
        ) -> Result<Option<crate::OccurrenceTransitionCommit>, ScheduleStoreError> {
            self.inner
                .transition_occurrence_with_receipt_if_current(
                    occurrence_id,
                    expected_attempt,
                    expected_claim_token,
                    transition,
                    runtime_outcome,
                )
                .await
        }
    }

    #[tokio::test]
    async fn memory_store_issues_one_complete_occurrence_transition_commit()
    -> Result<(), ScheduleDomainError> {
        let store = Arc::new(MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let service = ScheduleService::new(store.clone());
        let schedule = service
            .create(CreateScheduleRequest {
                name: Some("transition-commit".to_string()),
                description: None,
                trigger: TriggerSpec::Once {
                    due_at_utc: Utc::now() - Duration::seconds(1),
                },
                target: materialize_on_demand_target("scheduled prompt"),
                misfire_policy: MisfirePolicy::Skip,
                overlap_policy: OverlapPolicy::AllowConcurrent,
                missing_target_policy: MissingTargetPolicy::MarkMisfired,
                labels: BTreeMap::new(),
                planning_horizon_days: Some(1),
                planning_horizon_occurrences: Some(1),
            })
            .await?;
        let executor_lease = test_executor_lease(&store).await;
        let claim = store
            .claim_due_occurrences(
                &executor_lease,
                ClaimDueRequest {
                    owner_id: "transition-commit-owner".to_string(),
                    limit: 1,
                    lease_duration: Duration::seconds(30),
                },
            )
            .await?;
        store.release_executor_lease(executor_lease).await?;
        let claimed = claim
            .transitions
            .into_iter()
            .next()
            .ok_or_else(|| ScheduleDomainError::Internal("expected claimed occurrence".into()))?
            .successor()
            .clone();

        let dispatch_commit = store
            .transition_occurrence_if_current(
                &claimed.occurrence_id,
                claimed.attempt_count,
                claimed.claim_token(),
                OccurrenceLifecycleInput::DispatchStarted {
                    correlation_id: Some("transition-commit-correlation".to_string()),
                    at_utc: claim.store_now_utc,
                },
            )
            .await?
            .ok_or_else(|| ScheduleDomainError::Internal("dispatch CAS was stale".into()))?;
        dispatch_commit
            .prior()
            .check_current(Some(&claimed))
            .map_err(ScheduleDomainError::Internal)?;
        assert_eq!(
            dispatch_commit.successor().phase,
            OccurrencePhase::Dispatching
        );
        assert_eq!(
            dispatch_commit.effects(),
            &[OccurrenceLifecycleEffect::DispatchStarted]
        );
        assert!(dispatch_commit.supersession_acks().is_empty());

        let dispatching = dispatch_commit.successor().clone();
        let superseding_revision = crate::ScheduleRevision(schedule.revision.0 + 1);
        let supersede_commit = store
            .transition_occurrence_if_current(
                &dispatching.occurrence_id,
                dispatching.attempt_count,
                dispatching.claim_token(),
                OccurrenceLifecycleInput::Supersede {
                    superseded_by_revision: superseding_revision,
                    at_utc: Utc::now(),
                },
            )
            .await?
            .ok_or_else(|| ScheduleDomainError::Internal("supersession CAS was stale".into()))?;
        supersede_commit
            .prior()
            .check_current(Some(&dispatching))
            .map_err(ScheduleDomainError::Internal)?;
        assert_eq!(
            supersede_commit.successor().phase,
            OccurrencePhase::Superseded
        );
        assert!(supersede_commit.effects().iter().any(|effect| matches!(
            effect,
            OccurrenceLifecycleEffect::OccurrencesSuperseded {
                occurrence_id,
                superseding_revision: revision,
            } if occurrence_id == &dispatching.occurrence_id && *revision == superseding_revision
        )));
        assert_eq!(supersede_commit.supersession_acks().len(), 1);
        assert_eq!(
            supersede_commit.supersession_acks()[0].occurrence_id(),
            &dispatching.occurrence_id
        );

        let stale = store
            .transition_occurrence_if_current(
                &dispatching.occurrence_id,
                dispatching.attempt_count,
                Some(Uuid::now_v7()),
                OccurrenceLifecycleInput::AwaitCompletion { at_utc: Utc::now() },
            )
            .await?;
        assert!(stale.is_none(), "stale CAS must not mint a commit carrier");
        Ok(())
    }

    #[tokio::test]
    async fn target_probe_terminality_comes_from_occurrence_authority()
    -> Result<(), ScheduleDomainError> {
        let cases = [
            (
                TargetProbeOutcome::Busy {
                    detail: Some("target already running".to_string()),
                },
                OverlapPolicy::SkipIfRunning,
                MissingTargetPolicy::MarkMisfired,
                OccurrencePhase::Skipped,
                DeliveryReceiptStage::Skipped,
                OccurrenceFailureClass::TargetBusy,
            ),
            (
                TargetProbeOutcome::Missing {
                    detail: Some("target disappeared".to_string()),
                },
                OverlapPolicy::AllowConcurrent,
                MissingTargetPolicy::Skip,
                OccurrencePhase::Skipped,
                DeliveryReceiptStage::Skipped,
                OccurrenceFailureClass::TargetMissing,
            ),
            (
                TargetProbeOutcome::Missing {
                    detail: Some("target disappeared".to_string()),
                },
                OverlapPolicy::AllowConcurrent,
                MissingTargetPolicy::MarkMisfired,
                OccurrencePhase::Misfired,
                DeliveryReceiptStage::Misfired,
                OccurrenceFailureClass::TargetMissing,
            ),
        ];

        for (
            probe_outcome,
            overlap_policy,
            missing_target_policy,
            expected_phase,
            expected_stage,
            expected_failure_class,
        ) in cases
        {
            let store = Arc::new(MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
            let service = ScheduleService::new(store.clone());
            let schedule = service
                .create(CreateScheduleRequest {
                    name: Some(format!("target-probe-{expected_phase:?}")),
                    description: None,
                    trigger: TriggerSpec::Once {
                        due_at_utc: Utc::now() - Duration::seconds(1),
                    },
                    target: materialize_on_demand_target("scheduled prompt"),
                    misfire_policy: MisfirePolicy::Skip,
                    overlap_policy,
                    missing_target_policy,
                    labels: BTreeMap::new(),
                    planning_horizon_days: Some(1),
                    planning_horizon_occurrences: Some(1),
                })
                .await?;
            let delivery = Arc::new(CompletingDelivery::default());
            let driver = ScheduleDriver::new(
                service.clone(),
                store.clone(),
                Arc::new(StaticProbe(probe_outcome)),
                delivery.clone(),
                "driver-owner",
                ScheduleDriverConfig {
                    claim_limit: 8,
                    lease_duration: Duration::seconds(30),
                },
            );

            let report = driver.tick_once().await?;
            assert_eq!(report.claimed_occurrences, 1);
            assert_eq!(report.terminalized_occurrences, 1);
            assert!(delivery.dispatched_occurrences.lock().await.is_empty());

            let occurrence =
                wait_for_occurrence_phase(&service, &schedule.schedule_id, expected_phase).await?;
            assert_eq!(occurrence.failure_class, Some(expected_failure_class));

            let receipts = store.list_receipts(&occurrence.occurrence_id).await?;
            let last_receipt = receipts.last().ok_or_else(|| {
                ScheduleDomainError::Internal(
                    "target probe terminality should emit a receipt".to_string(),
                )
            })?;
            assert_eq!(last_receipt.stage, expected_stage);
            assert_eq!(last_receipt.failure_class, Some(expected_failure_class));
        }

        Ok(())
    }

    #[tokio::test]
    async fn target_probe_busy_allow_concurrent_continues_to_delivery()
    -> Result<(), ScheduleDomainError> {
        let store = Arc::new(MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let service = ScheduleService::new(store.clone());
        let schedule = service
            .create(CreateScheduleRequest {
                name: Some("target-busy-allowed".into()),
                description: None,
                trigger: TriggerSpec::Once {
                    due_at_utc: Utc::now() - Duration::seconds(1),
                },
                target: materialize_on_demand_target("scheduled prompt"),
                misfire_policy: MisfirePolicy::Skip,
                overlap_policy: OverlapPolicy::AllowConcurrent,
                missing_target_policy: MissingTargetPolicy::MarkMisfired,
                labels: BTreeMap::new(),
                planning_horizon_days: Some(1),
                planning_horizon_occurrences: Some(1),
            })
            .await?;
        let delivery = Arc::new(CompletingDelivery::default());
        let driver = ScheduleDriver::new(
            service.clone(),
            store,
            Arc::new(StaticProbe(TargetProbeOutcome::Busy {
                detail: Some("target already running".to_string()),
            })),
            delivery.clone(),
            "driver-owner",
            ScheduleDriverConfig {
                claim_limit: 8,
                lease_duration: Duration::seconds(30),
            },
        );

        driver.tick_once().await?;

        let occurrence =
            wait_for_occurrence_phase(&service, &schedule.schedule_id, OccurrencePhase::Completed)
                .await?;
        assert_eq!(occurrence.failure_class, None);
        assert_eq!(delivery.dispatched_occurrences.lock().await.len(), 1);
        Ok(())
    }

    /// Downstream-ask regression (schedule runaway): a one-shot whose occurrence
    /// went terminal (misfired here) must never regenerate. Pre-fix, the
    /// ns-precision due compared against the ms-precision machine cursor
    /// re-yielded the same due every tick (~1/sec, unbounded).
    #[tokio::test]
    async fn one_shot_misfire_must_not_regenerate() -> Result<(), ScheduleDomainError> {
        let store = Arc::new(MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let service = ScheduleService::new(store.clone());
        let schedule = service
            .create(CreateScheduleRequest {
                name: Some("one-shot-misfire-regen".into()),
                description: None,
                trigger: TriggerSpec::Once {
                    due_at_utc: Utc::now() - Duration::seconds(30),
                },
                target: materialize_on_demand_target("scheduled prompt"),
                misfire_policy: MisfirePolicy::CatchUpWithin { window_seconds: 5 },
                overlap_policy: OverlapPolicy::SkipIfRunning,
                missing_target_policy: MissingTargetPolicy::MarkMisfired,
                labels: BTreeMap::new(),
                planning_horizon_days: Some(1),
                planning_horizon_occurrences: Some(1),
            })
            .await?;
        let delivery = Arc::new(CompletingDelivery::default());
        let driver = ScheduleDriver::new(
            service.clone(),
            store.clone(),
            Arc::new(ReadyProbe),
            delivery.clone(),
            "driver-owner",
            ScheduleDriverConfig {
                claim_limit: 8,
                lease_duration: Duration::seconds(30),
            },
        );
        let created = service.get(&schedule.schedule_id).await?;
        eprintln!(
            "post-create: cursor={:?} ordinal={:?} trigger_due 30s ago",
            created.planning_cursor_utc, created.next_occurrence_ordinal
        );
        for tick in 0..6 {
            let _ = driver.tick_once().await?;
            let all = store
                .list_occurrences(crate::OccurrenceFilter {
                    schedule_id: Some(schedule.schedule_id.clone()),
                    include_terminal: true,
                    ..crate::OccurrenceFilter::default()
                })
                .await?;
            let after = service.get(&schedule.schedule_id).await?;
            eprintln!(
                "tick {tick}: total={} cursor={:?} ordinal={:?}",
                all.len(),
                after.planning_cursor_utc,
                after.next_occurrence_ordinal
            );
        }
        let all = store
            .list_occurrences(crate::OccurrenceFilter {
                schedule_id: Some(schedule.schedule_id.clone()),
                include_terminal: true,
                ..crate::OccurrenceFilter::default()
            })
            .await?;
        assert_eq!(
            all.len(),
            1,
            "a one-shot must never regenerate after misfire"
        );
        Ok(())
    }

    #[tokio::test]
    async fn driver_misfires_long_overdue_skip_occurrence_without_dispatch()
    -> Result<(), ScheduleDomainError> {
        let store = Arc::new(MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let service = ScheduleService::new(store.clone());
        let schedule = service
            .create(CreateScheduleRequest {
                name: Some("skip-misfire".into()),
                description: None,
                trigger: TriggerSpec::Interval(IntervalTriggerSpec {
                    start_at_utc: Utc::now() - Duration::minutes(2),
                    every_seconds: 61,
                    end_at_utc: None,
                }),
                target: materialize_on_demand_target("scheduled prompt"),
                misfire_policy: MisfirePolicy::Skip,
                overlap_policy: OverlapPolicy::SkipIfRunning,
                missing_target_policy: MissingTargetPolicy::MarkMisfired,
                labels: BTreeMap::new(),
                planning_horizon_days: Some(1),
                planning_horizon_occurrences: Some(1),
            })
            .await?;
        let delivery = Arc::new(CompletingDelivery::default());
        let driver = ScheduleDriver::new(
            service.clone(),
            store.clone(),
            Arc::new(ReadyProbe),
            delivery.clone(),
            "driver-owner",
            ScheduleDriverConfig {
                claim_limit: 8,
                lease_duration: Duration::seconds(30),
            },
        );

        let report = driver.tick_once().await?;
        assert_eq!(report.claimed_occurrences, 0);
        assert_eq!(report.terminalized_occurrences, 0);
        assert!(
            delivery.dispatched_occurrences.lock().await.is_empty(),
            "skip policy should not dispatch materially late pending occurrences"
        );

        let occurrence =
            wait_for_occurrence_phase(&service, &schedule.schedule_id, OccurrencePhase::Misfired)
                .await?;
        assert_eq!(occurrence.attempt_count, 0);

        let receipts = store.list_receipts(&occurrence.occurrence_id).await?;
        let last_receipt = receipts.last().ok_or_else(|| {
            ScheduleDomainError::Internal("misfired occurrence should emit a receipt".to_string())
        })?;
        assert_eq!(last_receipt.stage, DeliveryReceiptStage::Misfired);
        assert!(
            last_receipt
                .detail
                .as_deref()
                .is_some_and(|detail| detail.contains("skip policy")),
            "misfire receipt should explain why overdue work was skipped"
        );
        Ok(())
    }

    #[tokio::test]
    async fn driver_catches_up_overdue_occurrence_within_window() -> Result<(), ScheduleDomainError>
    {
        let store = Arc::new(MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let service = ScheduleService::new(store.clone());
        let schedule = service
            .create(CreateScheduleRequest {
                name: Some("catch-up-window".into()),
                description: None,
                trigger: TriggerSpec::Interval(IntervalTriggerSpec {
                    start_at_utc: Utc::now() - Duration::minutes(2),
                    every_seconds: 61,
                    end_at_utc: None,
                }),
                target: materialize_on_demand_target("scheduled prompt"),
                misfire_policy: MisfirePolicy::CatchUpWithin {
                    window_seconds: 120,
                },
                overlap_policy: OverlapPolicy::SkipIfRunning,
                missing_target_policy: MissingTargetPolicy::MarkMisfired,
                labels: BTreeMap::new(),
                planning_horizon_days: Some(1),
                planning_horizon_occurrences: Some(1),
            })
            .await?;
        let delivery = Arc::new(CompletingDelivery::default());
        let driver = ScheduleDriver::new(
            service.clone(),
            store.clone(),
            Arc::new(ReadyProbe),
            delivery.clone(),
            "driver-owner",
            ScheduleDriverConfig {
                claim_limit: 8,
                lease_duration: Duration::seconds(30),
            },
        );

        let report = driver.tick_once().await?;
        assert_eq!(report.claimed_occurrences, 1);
        assert_eq!(delivery.dispatched_occurrences.lock().await.len(), 1);

        let occurrence =
            wait_for_occurrence_phase(&service, &schedule.schedule_id, OccurrencePhase::Completed)
                .await?;
        let receipts = store.list_receipts(&occurrence.occurrence_id).await?;
        assert_eq!(
            receipts.last().map(|receipt| receipt.stage),
            Some(DeliveryReceiptStage::Completed),
            "catch-up policy should still allow overdue work within its window"
        );
        Ok(())
    }

    #[tokio::test]
    async fn driver_misfires_overdue_occurrence_past_catch_up_window()
    -> Result<(), ScheduleDomainError> {
        let store = Arc::new(MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let service = ScheduleService::new(store.clone());
        let schedule = service
            .create(CreateScheduleRequest {
                name: Some("catch-up-expired".into()),
                description: None,
                trigger: TriggerSpec::Interval(IntervalTriggerSpec {
                    start_at_utc: Utc::now() - Duration::minutes(2),
                    every_seconds: 61,
                    end_at_utc: None,
                }),
                target: materialize_on_demand_target("scheduled prompt"),
                misfire_policy: MisfirePolicy::CatchUpWithin { window_seconds: 30 },
                overlap_policy: OverlapPolicy::SkipIfRunning,
                missing_target_policy: MissingTargetPolicy::MarkMisfired,
                labels: BTreeMap::new(),
                planning_horizon_days: Some(1),
                planning_horizon_occurrences: Some(1),
            })
            .await?;
        let delivery = Arc::new(CompletingDelivery::default());
        let driver = ScheduleDriver::new(
            service.clone(),
            store.clone(),
            Arc::new(ReadyProbe),
            delivery.clone(),
            "driver-owner",
            ScheduleDriverConfig {
                claim_limit: 8,
                lease_duration: Duration::seconds(30),
            },
        );

        let report = driver.tick_once().await?;
        assert_eq!(report.claimed_occurrences, 0);
        assert!(
            delivery.dispatched_occurrences.lock().await.is_empty(),
            "expired catch-up window should prevent stale dispatch"
        );

        let occurrence =
            wait_for_occurrence_phase(&service, &schedule.schedule_id, OccurrencePhase::Misfired)
                .await?;
        let receipts = store.list_receipts(&occurrence.occurrence_id).await?;
        let last_receipt = receipts.last().ok_or_else(|| {
            ScheduleDomainError::Internal("misfired occurrence should emit a receipt".to_string())
        })?;
        assert_eq!(last_receipt.stage, DeliveryReceiptStage::Misfired);
        assert!(
            last_receipt
                .detail
                .as_deref()
                .is_some_and(|detail| detail.contains("catch-up window")),
            "misfire receipt should explain the expired catch-up window"
        );
        Ok(())
    }

    #[tokio::test]
    async fn driver_preserves_target_materialization_failure_classification()
    -> Result<(), ScheduleDomainError> {
        let store = Arc::new(MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let service = ScheduleService::new(store.clone());
        let schedule = service
            .create(CreateScheduleRequest {
                name: Some("materialize-now".into()),
                description: None,
                trigger: TriggerSpec::Once {
                    due_at_utc: Utc::now() - Duration::seconds(1),
                },
                target: materialize_on_demand_target("scheduled prompt"),
                misfire_policy: MisfirePolicy::Skip,
                overlap_policy: OverlapPolicy::SkipIfRunning,
                missing_target_policy: MissingTargetPolicy::MarkMisfired,
                labels: BTreeMap::new(),
                planning_horizon_days: Some(1),
                planning_horizon_occurrences: Some(1),
            })
            .await?;

        let driver = ScheduleDriver::new(
            service.clone(),
            store.clone(),
            Arc::new(ReadyProbe),
            Arc::new(MaterializationFailureDelivery),
            "driver-owner",
            ScheduleDriverConfig {
                claim_limit: 8,
                lease_duration: Duration::seconds(30),
            },
        );

        let report = driver.tick_once().await?;
        assert_eq!(report.claimed_occurrences, 1);
        assert_eq!(report.terminalized_occurrences, 0);

        let occurrence = loop {
            let occurrences = service.list_occurrences(&schedule.schedule_id).await?;
            if let Some(occurrence) = occurrences
                .into_iter()
                .find(|occurrence| occurrence.phase == OccurrencePhase::DeliveryFailed)
            {
                break occurrence;
            }
            sleep(std::time::Duration::from_millis(10)).await;
        };

        assert_eq!(
            occurrence.failure_class,
            Some(OccurrenceFailureClass::TargetMaterializationFailed)
        );
        assert_eq!(
            occurrence.failure_detail.as_deref(),
            Some("session creation failed")
        );

        let last_receipt = loop {
            let receipts = store.list_receipts(&occurrence.occurrence_id).await?;
            if let Some(receipt) = receipts
                .last()
                .filter(|receipt| receipt.stage == DeliveryReceiptStage::DeliveryFailed)
            {
                break receipt.clone();
            }
            sleep(std::time::Duration::from_millis(10)).await;
        };
        assert_eq!(last_receipt.stage, DeliveryReceiptStage::DeliveryFailed);
        assert_eq!(
            last_receipt.failure_class,
            Some(OccurrenceFailureClass::TargetMaterializationFailed)
        );
        Ok(())
    }

    #[tokio::test]
    async fn driver_preserves_dispatch_receipt_on_in_flight_occurrence_projection()
    -> Result<(), ScheduleDomainError> {
        let store = Arc::new(MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let service = ScheduleService::new(store.clone());
        let schedule = service
            .create(CreateScheduleRequest {
                name: Some("dispatch-receipt-projection".into()),
                description: None,
                trigger: TriggerSpec::Once {
                    due_at_utc: Utc::now() - Duration::seconds(1),
                },
                target: materialize_on_demand_target("scheduled prompt"),
                misfire_policy: MisfirePolicy::Skip,
                overlap_policy: OverlapPolicy::SkipIfRunning,
                missing_target_policy: MissingTargetPolicy::MarkMisfired,
                labels: BTreeMap::new(),
                planning_horizon_days: Some(1),
                planning_horizon_occurrences: Some(1),
            })
            .await?;
        let delivery = Arc::new(ControlledCompletionDelivery::default());
        let driver = ScheduleDriver::new(
            service.clone(),
            store.clone(),
            Arc::new(ReadyProbe),
            delivery.clone(),
            "driver-owner",
            ScheduleDriverConfig {
                claim_limit: 8,
                lease_duration: Duration::seconds(30),
            },
        );

        let report = driver.tick_once().await?;
        assert_eq!(report.claimed_occurrences, 1);
        wait_for_sender_count(&delivery, 1).await;
        let observed = delivery.observed_dispatches.lock().await;
        let (adapter_occurrence, adapter_identity) =
            observed.first().expect("adapter must observe one dispatch");
        assert_eq!(adapter_occurrence.phase, OccurrencePhase::Dispatching);
        assert_eq!(
            adapter_occurrence
                .last_receipt
                .as_ref()
                .map(|receipt| receipt.stage),
            Some(DeliveryReceiptStage::DispatchStarted),
            "durable intent must be visible before the adapter can act"
        );
        assert_eq!(
            adapter_occurrence
                .last_receipt
                .as_ref()
                .and_then(|receipt| receipt.correlation_id.as_deref()),
            Some(adapter_identity.correlation_id.as_str())
        );
        drop(observed);

        let occurrence = wait_for_occurrence_phase(
            &service,
            &schedule.schedule_id,
            OccurrencePhase::AwaitingCompletion,
        )
        .await?;
        let last_receipt = occurrence.last_receipt.as_ref().ok_or_else(|| {
            ScheduleDomainError::Internal(
                "dispatch receipt should remain projected on in-flight occurrences".to_string(),
            )
        })?;
        assert_eq!(last_receipt.stage, DeliveryReceiptStage::DispatchAccepted);
        assert_eq!(
            last_receipt.correlation_id.as_deref(),
            Some(occurrence.occurrence_id.to_string().as_str())
        );
        assert_eq!(
            last_receipt.runtime_outcome,
            Some(RuntimeDeliveryOutcome::AdmissionAccepted)
        );
        let receipts = store.list_receipts(&occurrence.occurrence_id).await?;
        assert!(
            receipts.windows(2).any(
                |pair| pair[0].stage == DeliveryReceiptStage::DispatchStarted
                    && pair[1].stage == DeliveryReceiptStage::DispatchAccepted
            ),
            "pre-effect intent and post-effect acceptance must both survive"
        );
        Ok(())
    }

    #[tokio::test]
    async fn delivery_failed_without_generated_failure_reason_fails_closed()
    -> Result<(), ScheduleDomainError> {
        let store = Arc::new(MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let service = ScheduleService::new(store.clone());
        let schedule = service
            .create(CreateScheduleRequest {
                name: Some("missing-failure-class".into()),
                description: None,
                trigger: TriggerSpec::Once {
                    due_at_utc: Utc::now() - Duration::seconds(1),
                },
                target: materialize_on_demand_target("scheduled prompt"),
                misfire_policy: MisfirePolicy::Skip,
                overlap_policy: OverlapPolicy::SkipIfRunning,
                missing_target_policy: MissingTargetPolicy::MarkMisfired,
                labels: BTreeMap::new(),
                planning_horizon_days: Some(1),
                planning_horizon_occurrences: Some(1),
            })
            .await?;
        let delivery = Arc::new(ControlledCompletionDelivery::default());
        let driver = ScheduleDriver::new(
            service.clone(),
            store,
            Arc::new(ReadyProbe),
            delivery.clone(),
            "driver-owner",
            ScheduleDriverConfig {
                claim_limit: 8,
                lease_duration: Duration::seconds(30),
            },
        );

        driver.tick_once().await?;
        wait_for_sender_count(&delivery, 1).await;

        let occurrence = wait_for_occurrence_phase(
            &service,
            &schedule.schedule_id,
            OccurrencePhase::AwaitingCompletion,
        )
        .await?;

        let sender = delivery.senders.lock().await.remove(0);
        sender
            .send(DeliveryTerminal {
                phase: OccurrencePhase::DeliveryFailed,
                receipt: None,
                detail: Some("missing generated failure reason".into()),
                delivery_failure_reason: None,
                runtime_completion_outcome: None,
                runtime_outcome: None,
            })
            .expect("completion receiver should be open");
        sleep(std::time::Duration::from_millis(30)).await;

        let after = service
            .list_occurrences(&schedule.schedule_id)
            .await?
            .into_iter()
            .find(|candidate| candidate.occurrence_id == occurrence.occurrence_id)
            .ok_or_else(|| ScheduleDomainError::Internal("occurrence should exist".to_string()))?;
        assert_eq!(after.phase, OccurrencePhase::AwaitingCompletion);
        assert_eq!(after.failure_class, None);
        Ok(())
    }

    #[tokio::test]
    async fn completion_terminalizes_and_records_receipt_without_standalone_append()
    -> Result<(), ScheduleDomainError> {
        let store = Arc::new(MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let service = ScheduleService::new(store.clone());
        let schedule = service
            .create(CreateScheduleRequest {
                name: Some("atomic-terminal-receipt".into()),
                description: None,
                trigger: TriggerSpec::Once {
                    due_at_utc: Utc::now() - Duration::seconds(1),
                },
                target: materialize_on_demand_target("scheduled prompt"),
                misfire_policy: MisfirePolicy::Skip,
                overlap_policy: OverlapPolicy::SkipIfRunning,
                missing_target_policy: MissingTargetPolicy::MarkMisfired,
                labels: BTreeMap::new(),
                planning_horizon_days: Some(1),
                planning_horizon_occurrences: Some(1),
            })
            .await?;
        let delivery = Arc::new(ControlledCompletionDelivery::default());
        let driver = ScheduleDriver::new(
            service.clone(),
            store.clone(),
            Arc::new(ReadyProbe),
            delivery.clone(),
            "driver-owner",
            ScheduleDriverConfig {
                claim_limit: 8,
                lease_duration: Duration::seconds(30),
            },
        );

        driver.tick_once().await?;
        wait_for_sender_count(&delivery, 1).await;
        let awaiting = wait_for_occurrence_phase(
            &service,
            &schedule.schedule_id,
            OccurrencePhase::AwaitingCompletion,
        )
        .await?;

        let terminal_store = Arc::new(StandaloneReceiptFailingStore {
            inner: store.clone(),
        }) as Arc<dyn ScheduleStore>;
        let terminalized = terminalize_occurrence_inner(
            terminal_store,
            awaiting.clone(),
            OccurrenceLifecycleInput::Complete { at_utc: Utc::now() },
            None,
            None,
        )
        .await?;

        assert_eq!(
            terminalized,
            TerminalizeOutcome::Applied,
            "a genuine terminal completion must record its receipt atomically through the \
             claim-screened store seam; no standalone receipt-authority API exists"
        );
        let completed =
            wait_for_occurrence_phase(&service, &schedule.schedule_id, OccurrencePhase::Completed)
                .await?;
        let receipts = store.list_receipts(&completed.occurrence_id).await?;
        let last_receipt = receipts.last().ok_or_else(|| {
            ScheduleDomainError::Internal(
                "terminal completion should append generated receipt".to_string(),
            )
        })?;
        assert_eq!(last_receipt.stage, DeliveryReceiptStage::Completed);
        assert_eq!(
            completed
                .last_receipt
                .as_ref()
                .map(|receipt| receipt.receipt_id),
            Some(last_receipt.receipt_id)
        );
        Ok(())
    }

    #[tokio::test]
    async fn adapter_selected_terminal_skip_or_misfire_fails_closed()
    -> Result<(), ScheduleDomainError> {
        for phase in [OccurrencePhase::Skipped, OccurrencePhase::Misfired] {
            let store = Arc::new(MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
            let service = ScheduleService::new(store.clone());
            let schedule = service
                .create(CreateScheduleRequest {
                    name: Some(format!("adapter-selected-{phase:?}")),
                    description: None,
                    trigger: TriggerSpec::Once {
                        due_at_utc: Utc::now() - Duration::seconds(1),
                    },
                    target: materialize_on_demand_target("scheduled prompt"),
                    misfire_policy: MisfirePolicy::Skip,
                    overlap_policy: OverlapPolicy::SkipIfRunning,
                    missing_target_policy: MissingTargetPolicy::MarkMisfired,
                    labels: BTreeMap::new(),
                    planning_horizon_days: Some(1),
                    planning_horizon_occurrences: Some(1),
                })
                .await?;
            let delivery = Arc::new(ControlledCompletionDelivery::default());
            let driver = ScheduleDriver::new(
                service.clone(),
                store,
                Arc::new(ReadyProbe),
                delivery.clone(),
                "driver-owner",
                ScheduleDriverConfig {
                    claim_limit: 8,
                    lease_duration: Duration::seconds(30),
                },
            );

            driver.tick_once().await?;
            wait_for_sender_count(&delivery, 1).await;

            let occurrence = wait_for_occurrence_phase(
                &service,
                &schedule.schedule_id,
                OccurrencePhase::AwaitingCompletion,
            )
            .await?;

            let sender = delivery.senders.lock().await.remove(0);
            sender
                .send(DeliveryTerminal {
                    phase,
                    receipt: None,
                    detail: Some("adapter-selected terminality".into()),
                    delivery_failure_reason: None,
                    runtime_completion_outcome: None,
                    runtime_outcome: None,
                })
                .expect("completion receiver should be open");
            sleep(std::time::Duration::from_millis(30)).await;

            let after = service
                .list_occurrences(&schedule.schedule_id)
                .await?
                .into_iter()
                .find(|candidate| candidate.occurrence_id == occurrence.occurrence_id)
                .ok_or_else(|| {
                    ScheduleDomainError::Internal("occurrence should exist".to_string())
                })?;
            assert_eq!(after.phase, OccurrencePhase::AwaitingCompletion);
            assert_eq!(after.failure_class, None);
        }
        Ok(())
    }

    #[tokio::test]
    async fn completion_future_failure_classification_comes_from_occurrence_authority()
    -> Result<(), ScheduleDomainError> {
        let store = Arc::new(MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let service = ScheduleService::new(store.clone());
        let schedule = service
            .create(CreateScheduleRequest {
                name: Some("completion-future-failure".into()),
                description: None,
                trigger: TriggerSpec::Once {
                    due_at_utc: Utc::now() - Duration::seconds(1),
                },
                target: materialize_on_demand_target("scheduled prompt"),
                misfire_policy: MisfirePolicy::Skip,
                overlap_policy: OverlapPolicy::SkipIfRunning,
                missing_target_policy: MissingTargetPolicy::MarkMisfired,
                labels: BTreeMap::new(),
                planning_horizon_days: Some(1),
                planning_horizon_occurrences: Some(1),
            })
            .await?;
        let delivery = Arc::new(ControlledCompletionDelivery::default());
        let driver = ScheduleDriver::new(
            service.clone(),
            store,
            Arc::new(ReadyProbe),
            delivery.clone(),
            "driver-owner",
            ScheduleDriverConfig {
                claim_limit: 8,
                lease_duration: Duration::seconds(30),
            },
        );

        driver.tick_once().await?;
        wait_for_sender_count(&delivery, 1).await;
        drop(delivery.senders.lock().await.remove(0));

        let after = wait_for_occurrence_phase(
            &service,
            &schedule.schedule_id,
            OccurrencePhase::DeliveryFailed,
        )
        .await?;
        assert_eq!(
            after.failure_class,
            Some(OccurrenceFailureClass::TransportError)
        );
        assert!(
            after
                .failure_detail
                .as_deref()
                .is_some_and(|detail| detail.contains("schedule driver stopped"))
        );
        Ok(())
    }

    #[tokio::test]
    async fn lease_renewal_retries_transient_store_faults_inside_lease_budget()
    -> Result<(), ScheduleDomainError> {
        let memory = Arc::new(MemoryScheduleStore::new());
        let durable_store = memory.clone() as Arc<dyn ScheduleStore>;
        let service = ScheduleService::new(durable_store.clone());
        let schedule = service
            .create(CreateScheduleRequest {
                name: Some("renew-transient-retry".into()),
                description: None,
                trigger: TriggerSpec::Once {
                    due_at_utc: Utc::now() - Duration::seconds(1),
                },
                target: materialize_on_demand_target("scheduled prompt"),
                misfire_policy: MisfirePolicy::Skip,
                overlap_policy: OverlapPolicy::SkipIfRunning,
                missing_target_policy: MissingTargetPolicy::MarkMisfired,
                labels: BTreeMap::new(),
                planning_horizon_days: Some(1),
                planning_horizon_occurrences: Some(1),
            })
            .await?;
        let occurrence =
            claim_and_start_dispatch_without_waiter(&durable_store, Duration::milliseconds(400))
                .await?;
        let initial_expiry = occurrence
            .lease_expires_at_utc
            .expect("dispatch intent must retain its lease");
        let transient_remaining = Arc::new(AtomicUsize::new(2));
        let renewal_attempts = Arc::new(AtomicUsize::new(0));
        let retrying_store = Arc::new(RowFaultInjectingStore {
            inner: durable_store,
            refill_poisoned: schedule.schedule_id.clone(),
            transient_renewals_remaining: transient_remaining.clone(),
            renewal_attempts: renewal_attempts.clone(),
        }) as Arc<dyn ScheduleStore>;
        let (tx, rx) = oneshot::channel();
        let completion: DeliveryCompletion =
            Box::pin(async move { rx.await.map_err(|_| ScheduleDomainError::DriverStopped) });
        let renewal_occurrence = occurrence.clone();
        let renewal_task = crate::tokio::spawn(async move {
            run_completion_with_lease_renewal(
                &retrying_store,
                &renewal_occurrence,
                Duration::milliseconds(400),
                completion,
            )
            .await
        });

        for _ in 0..100 {
            if renewal_attempts.load(Ordering::SeqCst) >= 3 {
                break;
            }
            sleep(std::time::Duration::from_millis(10)).await;
        }
        assert_eq!(transient_remaining.load(Ordering::SeqCst), 0);
        assert!(
            renewal_attempts.load(Ordering::SeqCst) >= 3,
            "two transient faults must be retried before the successful renewal"
        );
        let renewed = memory
            .get_occurrence(&occurrence.occurrence_id)
            .await?
            .expect("renewed occurrence");
        assert!(
            renewed
                .lease_expires_at_utc
                .is_some_and(|at| at > initial_expiry),
            "successful retry must durably extend the lease"
        );
        tx.send(DeliveryTerminal::completed(None))
            .expect("renewal completion receiver");
        let terminal = renewal_task
            .await
            .map_err(|error| ScheduleDomainError::Internal(error.to_string()))??;
        assert_eq!(terminal.phase, OccurrencePhase::Completed);
        Ok(())
    }

    /// P0 2026-07 contract: a long-running delivery keeps its lease alive
    /// through machine-authorized renewal, so later ticks neither reclaim it
    /// (no attempt 2, no second dispatch) nor misfire it while the delivery
    /// remains unresolved. Pre-fix, this exact setup dispatched attempt 2
    /// while attempt 1 was still running.
    #[tokio::test]
    async fn lease_renewal_keeps_long_running_delivery_claimed_without_reclaim()
    -> Result<(), ScheduleDomainError> {
        let store = Arc::new(MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let service = ScheduleService::new(store.clone());
        let schedule = service
            .create(CreateScheduleRequest {
                name: Some("renew-long-delivery".into()),
                description: None,
                trigger: TriggerSpec::Once {
                    due_at_utc: Utc::now() - Duration::seconds(1),
                },
                target: materialize_on_demand_target("scheduled prompt"),
                misfire_policy: MisfirePolicy::Skip,
                overlap_policy: OverlapPolicy::SkipIfRunning,
                missing_target_policy: MissingTargetPolicy::MarkMisfired,
                labels: BTreeMap::new(),
                planning_horizon_days: Some(1),
                planning_horizon_occurrences: Some(1),
            })
            .await?;
        let delivery = Arc::new(ControlledCompletionDelivery::default());
        let driver = ScheduleDriver::new(
            service.clone(),
            store.clone(),
            Arc::new(ReadyProbe),
            delivery.clone(),
            "driver-owner",
            ScheduleDriverConfig {
                claim_limit: 8,
                lease_duration: Duration::milliseconds(25),
            },
        );

        driver.tick_once().await?;
        wait_for_sender_count(&delivery, 1).await;
        let dispatched = wait_for_occurrence_attempt(&service, &schedule.schedule_id, 1).await?;
        let initial_expiry = dispatched
            .lease_expires_at_utc
            .expect("dispatched occurrence must hold a lease");
        // The waiter's renewal loop (~lease/2 cadence) must extend the
        // machine-owned lease while the delivery is still running.
        let mut renewed = None;
        for _ in 0..100 {
            let current = wait_for_occurrence_attempt(&service, &schedule.schedule_id, 1).await?;
            if current
                .lease_expires_at_utc
                .is_some_and(|at| at > initial_expiry)
            {
                renewed = current.lease_expires_at_utc;
                break;
            }
            sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(
            renewed.is_some(),
            "lease renewal must extend lease_expires_at while the delivery runs"
        );

        // Run well past several lease periods: no reclaim, no second
        // dispatch, no lease-expired receipt, attempt stays 1.
        sleep(std::time::Duration::from_millis(60)).await;
        driver.tick_once().await?;
        driver.tick_once().await?;
        let occurrence = wait_for_occurrence_attempt(&service, &schedule.schedule_id, 1).await?;
        assert_eq!(occurrence.attempt_count, 1, "no reclaim while renewing");
        assert_eq!(occurrence.phase, OccurrencePhase::AwaitingCompletion);
        assert_eq!(
            delivery.senders.lock().await.len(),
            1,
            "a live delivery must never be dispatched a second time"
        );
        let receipts = store.list_receipts(&occurrence.occurrence_id).await?;
        assert!(
            !receipts
                .iter()
                .any(|receipt| receipt.stage == DeliveryReceiptStage::LeaseExpired),
            "no lease-expired receipt may be minted while the deliverer is alive"
        );

        // Completion lands normally on attempt 1.
        let sender = delivery.senders.lock().await.remove(0);
        sender
            .send(DeliveryTerminal::completed(None))
            .expect("completion receiver should be open");
        let completed =
            wait_for_occurrence_phase(&service, &schedule.schedule_id, OccurrencePhase::Completed)
                .await?;
        assert_eq!(completed.attempt_count, 1);
        Ok(())
    }

    /// Holds its first admission call open until the row releases it, then
    /// admits like [`MaterializingDelivery`]: it materializes the session and
    /// completes at once. A later call admits without waiting, so a reclaim
    /// shows as a second call.
    #[derive(Default)]
    struct HeldAdmissionDelivery {
        calls: AtomicUsize,
        release: tokio::sync::Notify,
    }

    #[async_trait]
    impl ScheduleTargetDelivery for HeldAdmissionDelivery {
        async fn deliver_occurrence(
            &self,
            occurrence: &Occurrence,
            identity: &ScheduleDeliveryIdentity,
        ) -> Result<DeliveryDispatch, ScheduleDomainError> {
            if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
                self.release.notified().await;
            }
            let session_id = occurrence.materialized_session_id();
            let mut receipt = DeliveryReceipt::new(
                occurrence.occurrence_id.clone(),
                occurrence.attempt_count,
                DeliveryReceiptStage::DispatchAccepted,
            );
            receipt.correlation_id = Some(identity.correlation_id.clone());
            receipt.materialized_session_id = Some(session_id.clone());
            receipt.runtime_outcome = Some(RuntimeDeliveryOutcome::AdmissionAccepted);
            Ok(DeliveryDispatch {
                receipt,
                correlation_id: Some(identity.correlation_id.clone()),
                materialized_session_id: Some(session_id),
                completion: Box::pin(async { Ok(DeliveryTerminal::completed(None)) }),
            })
        }
    }

    /// The lease is renewed while the target admits the delivery, as it is
    /// while the delivery runs: an admission call held open for several lease
    /// periods keeps its claim (before, the lease stood still for the whole
    /// call). The dispatch's writes after the call start from the renewed
    /// occurrence, so recording the materialized session, a write against the
    /// occurrence's exact prior state, still commits. The occurrence completes
    /// at attempt 1 with one admission and no lease-expired receipt.
    #[tokio::test]
    async fn the_lease_is_renewed_while_the_target_admits_the_delivery()
    -> Result<(), ScheduleDomainError> {
        let store = Arc::new(MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let service = ScheduleService::new(store.clone());
        let schedule = service
            .create(CreateScheduleRequest {
                name: Some("renew-during-admission".into()),
                description: None,
                trigger: TriggerSpec::Once {
                    due_at_utc: Utc::now() - Duration::seconds(1),
                },
                target: materialize_on_demand_target("scheduled prompt"),
                misfire_policy: MisfirePolicy::Skip,
                overlap_policy: OverlapPolicy::SkipIfRunning,
                missing_target_policy: MissingTargetPolicy::MarkMisfired,
                labels: BTreeMap::new(),
                planning_horizon_days: Some(1),
                planning_horizon_occurrences: Some(1),
            })
            .await?;
        let delivery = Arc::new(HeldAdmissionDelivery::default());
        let driver = ScheduleDriver::new(
            service.clone(),
            store.clone(),
            Arc::new(ReadyProbe),
            delivery.clone(),
            "driver-owner",
            ScheduleDriverConfig {
                claim_limit: 8,
                lease_duration: Duration::milliseconds(25),
            },
        );

        let observe_open_admission = async {
            for _ in 0..100 {
                if delivery.calls.load(Ordering::SeqCst) >= 1 {
                    break;
                }
                sleep(std::time::Duration::from_millis(10)).await;
            }
            assert_eq!(
                delivery.calls.load(Ordering::SeqCst),
                1,
                "the admission call is open"
            );
            let admitting = wait_for_occurrence_attempt(&service, &schedule.schedule_id, 1).await?;
            assert_eq!(admitting.phase, OccurrencePhase::Dispatching);
            let initial_expiry = admitting
                .lease_expires_at_utc
                .expect("a dispatching occurrence holds a lease");
            // Several lease periods pass while the call stays open.
            sleep(std::time::Duration::from_millis(100)).await;
            let current = wait_for_occurrence_attempt(&service, &schedule.schedule_id, 1).await?;
            assert!(
                current
                    .lease_expires_at_utc
                    .is_some_and(|at| at > initial_expiry),
                "the lease must be renewed while the admission call is open"
            );
            delivery.release.notify_one();
            Ok::<_, ScheduleDomainError>(current.occurrence_id)
        };
        let (tick, observed) = tokio::join!(driver.tick_once(), observe_open_admission);
        tick?;
        let occurrence_id = observed?;

        let completed =
            wait_for_occurrence_phase(&service, &schedule.schedule_id, OccurrencePhase::Completed)
                .await?;
        assert_eq!(completed.occurrence_id, occurrence_id);
        assert_eq!(
            completed.attempt_count, 1,
            "no reclaim while the call was open"
        );
        assert_eq!(
            delivery.calls.load(Ordering::SeqCst),
            1,
            "the delivery is admitted once"
        );
        assert_eq!(
            crate::types::target_materialized_session_id(&completed.target_snapshot),
            Some(completed.materialized_session_id()),
            "the materialization recorded after the call commits on the renewed occurrence"
        );
        let receipts = store.list_receipts(&occurrence_id).await?;
        assert!(
            !receipts
                .iter()
                .any(|receipt| receipt.stage == DeliveryReceiptStage::LeaseExpired),
            "no lease-expired receipt while the admission call was open"
        );
        Ok(())
    }

    /// Crash-recovery semantics are unchanged: a genuinely dead deliverer
    /// (claimed + dispatched, but no renewal — as after a process crash) is
    /// reclaimed once its lease expires, in the same transaction as
    /// attempt+1, with the lease-expired receipt minted.
    #[tokio::test]
    async fn expired_lease_without_live_waiter_is_reclaimed() -> Result<(), ScheduleDomainError> {
        let store = Arc::new(MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let service = ScheduleService::new(store.clone());
        let schedule = service
            .create(CreateScheduleRequest {
                name: Some("dead-deliverer-reclaim".into()),
                description: None,
                trigger: TriggerSpec::Once {
                    due_at_utc: Utc::now() - Duration::seconds(1),
                },
                target: materialize_on_demand_target("scheduled prompt"),
                misfire_policy: MisfirePolicy::Skip,
                overlap_policy: OverlapPolicy::SkipIfRunning,
                missing_target_policy: MissingTargetPolicy::MarkMisfired,
                labels: BTreeMap::new(),
                planning_horizon_days: Some(1),
                planning_horizon_occurrences: Some(1),
            })
            .await?;

        // Simulate a deliverer from a crashed process: claim + dispatch +
        // await through the store seams, with NO waiter task and NO renewal
        // in this process.
        let occurrence =
            claim_and_dispatch_without_waiter(&store, Duration::milliseconds(25)).await?;

        sleep(std::time::Duration::from_millis(35)).await;

        let delivery = Arc::new(ControlledCompletionDelivery::default());
        let driver = ScheduleDriver::new(
            service.clone(),
            store.clone(),
            Arc::new(ReadyProbe),
            delivery.clone(),
            "driver-owner",
            ScheduleDriverConfig {
                claim_limit: 8,
                lease_duration: Duration::seconds(30),
            },
        );
        driver.tick_once().await?;
        wait_for_sender_count(&delivery, 1).await;
        let reclaimed = wait_for_occurrence_attempt(&service, &schedule.schedule_id, 2).await?;
        assert_eq!(reclaimed.occurrence_id, occurrence.occurrence_id);
        assert_eq!(reclaimed.phase, OccurrencePhase::AwaitingCompletion);
        let receipts = store.list_receipts(&reclaimed.occurrence_id).await?;
        assert!(
            receipts
                .iter()
                .any(|receipt| receipt.stage == DeliveryReceiptStage::LeaseExpired),
            "reclaiming a dead deliverer must mint the lease-expired receipt"
        );
        Ok(())
    }

    #[tokio::test]
    async fn crash_after_durable_intent_before_effect_retries_same_delivery_identity()
    -> Result<(), ScheduleDomainError> {
        let store = Arc::new(MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let service = ScheduleService::new(store.clone());
        let schedule = service
            .create(CreateScheduleRequest {
                name: Some("crash-before-effect".into()),
                description: None,
                trigger: TriggerSpec::Once {
                    due_at_utc: Utc::now() - Duration::seconds(1),
                },
                target: materialize_on_demand_target("scheduled prompt"),
                misfire_policy: MisfirePolicy::Skip,
                overlap_policy: OverlapPolicy::SkipIfRunning,
                missing_target_policy: MissingTargetPolicy::MarkMisfired,
                labels: BTreeMap::new(),
                planning_horizon_days: Some(1),
                planning_horizon_occurrences: Some(1),
            })
            .await?;
        let intent =
            claim_and_start_dispatch_without_waiter(&store, Duration::milliseconds(25)).await?;
        let expected_identity = ScheduleDeliveryIdentity::for_occurrence(&intent);
        assert_eq!(
            intent.last_receipt.as_ref().map(|receipt| receipt.stage),
            Some(DeliveryReceiptStage::DispatchStarted)
        );

        sleep(std::time::Duration::from_millis(35)).await;
        let delivery = Arc::new(IdempotentDelivery::default());
        let driver = ScheduleDriver::new(
            service.clone(),
            store.clone(),
            Arc::new(ReadyProbe),
            delivery.clone(),
            "recovery-owner",
            ScheduleDriverConfig {
                claim_limit: 8,
                lease_duration: Duration::seconds(30),
            },
        );
        driver.tick_once().await?;
        wait_for_admission_count(&delivery, 1).await;

        assert_eq!(*delivery.external_effect_count.lock().await, 1);
        assert!(
            delivery
                .effect_keys
                .lock()
                .await
                .contains(&expected_identity.idempotency_key),
            "reclaim must reuse the durable occurrence identity"
        );
        let receipts = store.list_receipts(&intent.occurrence_id).await?;
        assert!(receipts.iter().any(|receipt| {
            receipt.stage == DeliveryReceiptStage::DispatchAccepted
                && receipt.runtime_outcome == Some(RuntimeDeliveryOutcome::AdmissionAccepted)
        }));
        let recovered = wait_for_occurrence_attempt(&service, &schedule.schedule_id, 2).await?;
        assert_eq!(recovered.occurrence_id, intent.occurrence_id);
        Ok(())
    }

    #[tokio::test]
    async fn deferred_delivery_repair_leaves_occurrence_reclaimable_and_nonterminal()
    -> Result<(), ScheduleDomainError> {
        let store = Arc::new(MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let service = ScheduleService::new(store.clone());
        let schedule = service
            .create(CreateScheduleRequest {
                name: Some("repair-deferred".into()),
                description: None,
                trigger: TriggerSpec::Once {
                    due_at_utc: Utc::now() - Duration::seconds(1),
                },
                target: materialize_on_demand_target("scheduled prompt"),
                misfire_policy: MisfirePolicy::Skip,
                overlap_policy: OverlapPolicy::SkipIfRunning,
                missing_target_policy: MissingTargetPolicy::MarkMisfired,
                labels: BTreeMap::new(),
                planning_horizon_days: Some(1),
                planning_horizon_occurrences: Some(1),
            })
            .await?;
        let delivery = Arc::new(RepairDeferredDelivery::default());
        let driver = ScheduleDriver::new(
            service.clone(),
            store,
            Arc::new(ReadyProbe),
            delivery.clone(),
            "repair-owner",
            ScheduleDriverConfig {
                claim_limit: 8,
                lease_duration: Duration::milliseconds(25),
            },
        );

        driver.tick_once().await?;
        for _ in 0..50 {
            if delivery.attempts.lock().await.len() == 1 {
                break;
            }
            sleep(std::time::Duration::from_millis(2)).await;
        }
        let first = wait_for_occurrence_attempt(&service, &schedule.schedule_id, 1).await?;
        assert_eq!(first.phase, OccurrencePhase::AwaitingCompletion);

        sleep(std::time::Duration::from_millis(35)).await;
        driver.tick_once().await?;
        for _ in 0..50 {
            if delivery.attempts.lock().await.len() == 2 {
                break;
            }
            sleep(std::time::Duration::from_millis(2)).await;
        }
        let reclaimed = wait_for_occurrence_attempt(&service, &schedule.schedule_id, 2).await?;
        assert_eq!(reclaimed.occurrence_id, first.occurrence_id);
        assert_eq!(reclaimed.phase, OccurrencePhase::AwaitingCompletion);
        assert_eq!(delivery.attempts.lock().await.as_slice(), &[1, 2]);
        Ok(())
    }

    /// The replay after a crash deduplicates at the target, and the
    /// occurrence then waits for the completion the replay returned instead
    /// of recording Completed at admission. This fake answers the replay
    /// with an unavailable completion authority, so the occurrence ends
    /// DeliveryFailed with the class that failure maps to, and the receipt
    /// history keeps the deduplicated admission without a Completed receipt.
    #[tokio::test]
    async fn crash_after_effect_before_observation_deduplicates_on_replay()
    -> Result<(), ScheduleDomainError> {
        let store = Arc::new(MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let service = ScheduleService::new(store.clone());
        let schedule = service
            .create(CreateScheduleRequest {
                name: Some("crash-after-effect".into()),
                description: None,
                trigger: TriggerSpec::Once {
                    due_at_utc: Utc::now() - Duration::seconds(1),
                },
                target: materialize_on_demand_target("scheduled prompt"),
                misfire_policy: MisfirePolicy::Skip,
                overlap_policy: OverlapPolicy::SkipIfRunning,
                missing_target_policy: MissingTargetPolicy::MarkMisfired,
                labels: BTreeMap::new(),
                planning_horizon_days: Some(1),
                planning_horizon_occurrences: Some(1),
            })
            .await?;
        let intent =
            claim_and_start_dispatch_without_waiter(&store, Duration::milliseconds(25)).await?;
        let identity = ScheduleDeliveryIdentity::for_occurrence(&intent);
        let delivery = Arc::new(IdempotentDelivery::default());

        // The external target applies the effect, then this process dies
        // before it can persist DispatchAccepted.
        let _unobserved = delivery.deliver_occurrence(&intent, &identity).await?;
        assert_eq!(*delivery.external_effect_count.lock().await, 1);

        sleep(std::time::Duration::from_millis(35)).await;
        let driver = ScheduleDriver::new(
            service.clone(),
            store.clone(),
            Arc::new(ReadyProbe),
            delivery.clone(),
            "recovery-owner",
            ScheduleDriverConfig {
                claim_limit: 8,
                lease_duration: Duration::seconds(30),
            },
        );
        driver.tick_once().await?;
        wait_for_admission_count(&delivery, 2).await;

        assert_eq!(
            *delivery.external_effect_count.lock().await,
            1,
            "stable target idempotency key must suppress the replayed effect"
        );
        assert_eq!(
            delivery.admissions.lock().await.as_slice(),
            &[
                RuntimeDeliveryOutcome::AdmissionAccepted,
                RuntimeDeliveryOutcome::AdmissionDeduplicated
            ]
        );
        let recovered = wait_for_occurrence_phase(
            &service,
            &schedule.schedule_id,
            OccurrencePhase::DeliveryFailed,
        )
        .await?;
        assert_eq!(recovered.occurrence_id, intent.occurrence_id);
        // RuntimeCompletionAuthorityUnavailable resolves through the generated
        // DeliveryCompletionFailureInternalError arm.
        assert_eq!(
            recovered.failure_class,
            Some(OccurrenceFailureClass::InternalError)
        );
        assert!(
            recovered
                .failure_detail
                .as_deref()
                .is_some_and(|detail| detail.contains("original completion channel")),
            "the failure keeps the completion's own detail: {:?}",
            recovered.failure_detail
        );
        assert_eq!(
            *delivery.external_effect_count.lock().await,
            1,
            "the failed replay must not repeat the external effect"
        );
        let receipts = store.list_receipts(&intent.occurrence_id).await?;
        assert!(
            receipts.iter().any(|receipt| {
                receipt.stage == DeliveryReceiptStage::DispatchAccepted
                    && receipt.runtime_outcome
                        == Some(RuntimeDeliveryOutcome::AdmissionDeduplicated)
            }),
            "the receipt history keeps the deduplicated admission: {receipts:?}"
        );
        assert!(
            receipts
                .iter()
                .all(|receipt| receipt.stage != DeliveryReceiptStage::Completed),
            "a deduplicated admission records no Completed receipt: {receipts:?}"
        );
        Ok(())
    }

    /// The control for the replay above: when the deduplicated replay's
    /// completion answers with the original effect's own Completed terminal,
    /// the occurrence ends Completed from that terminal, after a
    /// DispatchAccepted receipt that keeps the deduplicated admission.
    #[tokio::test]
    async fn crash_after_effect_replay_completes_from_the_original_terminal()
    -> Result<(), ScheduleDomainError> {
        let store = Arc::new(MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let service = ScheduleService::new(store.clone());
        let schedule = service
            .create(CreateScheduleRequest {
                name: Some("crash-after-effect-original-completion".into()),
                description: None,
                trigger: TriggerSpec::Once {
                    due_at_utc: Utc::now() - Duration::seconds(1),
                },
                target: materialize_on_demand_target("scheduled prompt"),
                misfire_policy: MisfirePolicy::Skip,
                overlap_policy: OverlapPolicy::SkipIfRunning,
                missing_target_policy: MissingTargetPolicy::MarkMisfired,
                labels: BTreeMap::new(),
                planning_horizon_days: Some(1),
                planning_horizon_occurrences: Some(1),
            })
            .await?;
        let intent =
            claim_and_start_dispatch_without_waiter(&store, Duration::milliseconds(25)).await?;
        let identity = ScheduleDeliveryIdentity::for_occurrence(&intent);
        let delivery = Arc::new(IdempotentDelivery {
            replay_answers_original_completion: true,
            ..IdempotentDelivery::default()
        });

        // The external target applies the effect, then this process dies
        // before it can persist DispatchAccepted.
        let _unobserved = delivery.deliver_occurrence(&intent, &identity).await?;

        sleep(std::time::Duration::from_millis(35)).await;
        let driver = ScheduleDriver::new(
            service.clone(),
            store.clone(),
            Arc::new(ReadyProbe),
            delivery.clone(),
            "recovery-owner",
            ScheduleDriverConfig {
                claim_limit: 8,
                lease_duration: Duration::seconds(30),
            },
        );
        driver.tick_once().await?;
        wait_for_admission_count(&delivery, 2).await;

        let recovered =
            wait_for_occurrence_phase(&service, &schedule.schedule_id, OccurrencePhase::Completed)
                .await?;
        assert_eq!(recovered.occurrence_id, intent.occurrence_id);
        assert_eq!(recovered.failure_class, None);
        assert_eq!(*delivery.external_effect_count.lock().await, 1);
        assert_eq!(
            delivery.admissions.lock().await.as_slice(),
            &[
                RuntimeDeliveryOutcome::AdmissionAccepted,
                RuntimeDeliveryOutcome::AdmissionDeduplicated
            ]
        );
        let receipts = store.list_receipts(&intent.occurrence_id).await?;
        let accepted_at = receipts
            .iter()
            .position(|receipt| {
                receipt.stage == DeliveryReceiptStage::DispatchAccepted
                    && receipt.runtime_outcome
                        == Some(RuntimeDeliveryOutcome::AdmissionDeduplicated)
            })
            .expect("the receipt history keeps the deduplicated admission");
        let completed_at = receipts
            .iter()
            .position(|receipt| receipt.stage == DeliveryReceiptStage::Completed)
            .expect("the original terminal records a Completed receipt");
        assert!(
            accepted_at < completed_at,
            "Completed follows the deduplicated admission: {receipts:?}"
        );
        Ok(())
    }

    /// Dispatch, process restart, reclaim, deduplicated redelivery: the first
    /// process admitted the input and died before it recorded the admission.
    /// A new driver reclaims the occurrence (attempt 2), the redelivery
    /// deduplicates onto the existing input, and the occurrence waits for
    /// that input's own terminal. Returns the waiting occurrence.
    async fn reclaim_onto_a_pending_existing_input(
        store: &Arc<dyn ScheduleStore>,
        service: &ScheduleService,
        delivery: &Arc<KeyedInputDelivery>,
        name: &str,
    ) -> Result<(crate::ScheduleId, ScheduleDeliveryIdentity, Occurrence), ScheduleDomainError>
    {
        let schedule = service
            .create(CreateScheduleRequest {
                name: Some(name.into()),
                description: None,
                trigger: TriggerSpec::Once {
                    due_at_utc: Utc::now() - Duration::seconds(1),
                },
                target: materialize_on_demand_target("scheduled prompt"),
                misfire_policy: MisfirePolicy::Skip,
                overlap_policy: OverlapPolicy::SkipIfRunning,
                missing_target_policy: MissingTargetPolicy::MarkMisfired,
                labels: BTreeMap::new(),
                planning_horizon_days: Some(1),
                planning_horizon_occurrences: Some(1),
            })
            .await?;
        let intent =
            claim_and_start_dispatch_without_waiter(store, Duration::milliseconds(25)).await?;
        let identity = ScheduleDeliveryIdentity::for_occurrence(&intent);

        // The first process admits the input, then dies with its completion
        // waiter before it records DispatchAccepted. The input stays pending
        // at the target.
        drop(delivery.deliver_occurrence(&intent, &identity).await?);

        // The restarted process reclaims the lapsed lease well inside the
        // Skip grace window and redelivers under the same stable identity.
        sleep(std::time::Duration::from_millis(35)).await;
        let driver = ScheduleDriver::new(
            service.clone(),
            store.clone(),
            Arc::new(ReadyProbe),
            delivery.clone(),
            "restarted-owner",
            ScheduleDriverConfig {
                claim_limit: 8,
                lease_duration: Duration::seconds(30),
            },
        );
        driver.tick_once().await?;
        delivery.wait_for_admission_count(2).await;
        assert_eq!(
            delivery.admissions.lock().await.as_slice(),
            &[
                RuntimeDeliveryOutcome::AdmissionAccepted,
                RuntimeDeliveryOutcome::AdmissionDeduplicated
            ]
        );

        let waiting = wait_for_occurrence_phase(
            service,
            &schedule.schedule_id,
            OccurrencePhase::AwaitingCompletion,
        )
        .await?;
        assert_eq!(waiting.occurrence_id, intent.occurrence_id);
        assert_eq!(waiting.attempt_count, 2);
        // The deduplicated admission is no terminal: while the existing input
        // is still pending, the occurrence keeps waiting for it.
        sleep(std::time::Duration::from_millis(30)).await;
        let still_waiting = service
            .list_occurrences(&schedule.schedule_id)
            .await?
            .into_iter()
            .find(|occurrence| occurrence.occurrence_id == intent.occurrence_id)
            .expect("the reclaimed occurrence is listed");
        assert_eq!(
            still_waiting.phase,
            OccurrencePhase::AwaitingCompletion,
            "a deduplicated admission must wait for the existing input's outcome"
        );
        assert_eq!(still_waiting.attempt_count, 2);
        Ok((schedule.schedule_id, identity, still_waiting))
    }

    /// Failure case: the existing input behind a reclaimed,
    /// deduplicated redelivery fails, and the occurrence records that
    /// input's failure class instead of Completed. Before the dedup arm
    /// waited, the occurrence recorded Completed at the deduplicated
    /// admission; restoring that (`to Completed` in
    /// DispatchDeduplicatedFromDispatching) fails this test.
    #[tokio::test]
    async fn reclaimed_dedup_admission_records_the_existing_inputs_failure()
    -> Result<(), ScheduleDomainError> {
        let store = Arc::new(MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let service = ScheduleService::new(store.clone());
        let delivery = Arc::new(KeyedInputDelivery::default());
        let (schedule_id, identity, waiting) = reclaim_onto_a_pending_existing_input(
            &store,
            &service,
            &delivery,
            "reclaimed-dedup-failure",
        )
        .await?;

        delivery
            .settle(
                &identity.idempotency_key,
                DeliveryTerminal::runtime_completion(
                    RuntimeCompletionOutcome::Abandoned,
                    Some("the existing input was abandoned".to_string()),
                    None,
                ),
            )
            .await;

        let failed =
            wait_for_occurrence_phase(&service, &schedule_id, OccurrencePhase::DeliveryFailed)
                .await?;
        assert_eq!(failed.occurrence_id, waiting.occurrence_id);
        assert_eq!(failed.attempt_count, 2);
        // An abandoned runtime completion resolves through the generated
        // RuntimeCompletionRuntimeRejected arm.
        assert_eq!(
            failed.failure_class,
            Some(OccurrenceFailureClass::RuntimeRejected)
        );
        let receipts = store.list_receipts(&waiting.occurrence_id).await?;
        assert!(
            receipts
                .iter()
                .all(|receipt| receipt.stage != DeliveryReceiptStage::Completed),
            "the failed existing input records no Completed receipt: {receipts:?}"
        );
        Ok(())
    }

    /// Success case: the existing input behind a reclaimed,
    /// deduplicated redelivery completes, and the occurrence ends Completed
    /// from that input's own terminal.
    #[tokio::test]
    async fn reclaimed_dedup_admission_records_the_existing_inputs_completion()
    -> Result<(), ScheduleDomainError> {
        let store = Arc::new(MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let service = ScheduleService::new(store.clone());
        let delivery = Arc::new(KeyedInputDelivery::default());
        let (schedule_id, identity, waiting) = reclaim_onto_a_pending_existing_input(
            &store,
            &service,
            &delivery,
            "reclaimed-dedup-completion",
        )
        .await?;

        delivery
            .settle(
                &identity.idempotency_key,
                DeliveryTerminal::runtime_completion(
                    RuntimeCompletionOutcome::Completed,
                    None,
                    None,
                ),
            )
            .await;

        let completed =
            wait_for_occurrence_phase(&service, &schedule_id, OccurrencePhase::Completed).await?;
        assert_eq!(completed.occurrence_id, waiting.occurrence_id);
        assert_eq!(completed.attempt_count, 2);
        assert_eq!(completed.failure_class, None);
        Ok(())
    }

    /// A keyed target that decides a redelivery inside its admission: an
    /// existing input under the stable key is replayed (deduplicated), and
    /// without one an admitted redelivery fails and an unproven late one
    /// answers that the target never admitted it. Nothing is admitted fresh
    /// on either path.
    #[derive(Default)]
    struct ExistingOnlyKeyedDelivery {
        keyed: KeyedInputDelivery,
        existing_calls: Mutex<Vec<ExistingDelivery>>,
    }

    #[async_trait]
    impl ScheduleTargetDelivery for ExistingOnlyKeyedDelivery {
        async fn deliver_occurrence(
            &self,
            occurrence: &Occurrence,
            identity: &ScheduleDeliveryIdentity,
        ) -> Result<DeliveryDispatch, ScheduleDomainError> {
            self.keyed.deliver_occurrence(occurrence, identity).await
        }

        async fn deliver_existing_occurrence(
            &self,
            occurrence: &Occurrence,
            identity: &ScheduleDeliveryIdentity,
            existing: ExistingDelivery,
        ) -> Result<ExistingDeliveryAnswer, ScheduleDomainError> {
            self.existing_calls.lock().await.push(existing);
            let admitted = self
                .keyed
                .inputs
                .lock()
                .await
                .contains_key(&identity.idempotency_key);
            if admitted {
                return self
                    .keyed
                    .deliver_occurrence(occurrence, identity)
                    .await
                    .map(ExistingDeliveryAnswer::Dispatch);
            }
            match existing {
                ExistingDelivery::Admitted => Err(ScheduleDomainError::DeliveryFailed {
                    occurrence_id: occurrence.occurrence_id.clone(),
                    reason: "the admitted input is no longer known".to_string(),
                }),
                _ => Ok(ExistingDeliveryAnswer::NotAdmittedPastDeadline),
            }
        }
    }

    /// How far the first attempt got before its process died.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum FirstAttemptEnd {
        /// The dispatch start was recorded and the target never saw it.
        DispatchStarted,
        /// The target admitted the input; its admission was not recorded.
        AdmittedUnrecorded,
        /// The target admitted the input and DispatchAccepted was recorded.
        AdmissionRecorded,
    }

    /// One occurrence under a one-second catch-up window whose first attempt
    /// ended as `end` and whose executor then died: its 25 ms lease lapses
    /// and the misfire deadline passes before a restarted driver ticks.
    async fn first_attempt_died_past_the_deadline(
        store: &Arc<dyn ScheduleStore>,
        service: &ScheduleService,
        keyed: &KeyedInputDelivery,
        end: FirstAttemptEnd,
        name: &str,
    ) -> Result<(crate::ScheduleId, ScheduleDeliveryIdentity), ScheduleDomainError> {
        first_attempt_died_past_the_deadline_under(
            store,
            service,
            keyed,
            end,
            name,
            MissingTargetPolicy::MarkMisfired,
        )
        .await
    }

    /// [`first_attempt_died_past_the_deadline`] for a schedule with
    /// `missing_target_policy`.
    async fn first_attempt_died_past_the_deadline_under(
        store: &Arc<dyn ScheduleStore>,
        service: &ScheduleService,
        keyed: &KeyedInputDelivery,
        end: FirstAttemptEnd,
        name: &str,
        missing_target_policy: MissingTargetPolicy,
    ) -> Result<(crate::ScheduleId, ScheduleDeliveryIdentity), ScheduleDomainError> {
        let schedule = service
            .create(CreateScheduleRequest {
                name: Some(name.into()),
                description: None,
                trigger: TriggerSpec::Once {
                    due_at_utc: Utc::now() - Duration::milliseconds(200),
                },
                target: materialize_on_demand_target("scheduled prompt"),
                misfire_policy: MisfirePolicy::CatchUpWithin { window_seconds: 1 },
                overlap_policy: OverlapPolicy::SkipIfRunning,
                missing_target_policy,
                labels: BTreeMap::new(),
                planning_horizon_days: Some(1),
                planning_horizon_occurrences: Some(1),
            })
            .await?;
        let intent =
            claim_and_start_dispatch_without_waiter(store, Duration::milliseconds(25)).await?;
        let identity = ScheduleDeliveryIdentity::for_occurrence(&intent);
        if end != FirstAttemptEnd::DispatchStarted {
            drop(keyed.deliver_occurrence(&intent, &identity).await?);
        }
        if end == FirstAttemptEnd::AdmissionRecorded {
            let _accepted = store
                .transition_occurrence_with_receipt_if_current(
                    &intent.occurrence_id,
                    intent.attempt_count,
                    intent.claim_token(),
                    OccurrenceLifecycleInput::DispatchAccepted {
                        admission_outcome: DeliveryAdmissionOutcome::Accepted,
                        at_utc: Utc::now(),
                    },
                    Some(RuntimeDeliveryOutcome::AdmissionAccepted),
                )
                .await?
                .ok_or_else(|| {
                    ScheduleDomainError::Internal("the first claim went stale".to_string())
                })?;
        }
        // The process dies: the lease lapses and the misfire deadline (one
        // second after due) passes.
        sleep(std::time::Duration::from_millis(1_200)).await;
        Ok((schedule.schedule_id, identity))
    }

    fn restarted_driver(
        service: &ScheduleService,
        store: &Arc<dyn ScheduleStore>,
        delivery: Arc<dyn ScheduleTargetDelivery>,
    ) -> ScheduleDriver {
        ScheduleDriver::new(
            service.clone(),
            store.clone(),
            Arc::new(ReadyProbe),
            delivery,
            "restarted-owner",
            ScheduleDriverConfig {
                claim_limit: 8,
                lease_duration: Duration::seconds(30),
            },
        )
    }

    /// An admitted delivery whose executor died, reclaimed past its misfire
    /// deadline: the reclaim is not refused, the redelivery replays the
    /// admitted input without a fresh admission, and the occurrence ends with
    /// that input's own outcome, with no row fault. Before the recorded
    /// admission exempted the reclaim from the misfire deadline it ended
    /// Misfired.
    #[tokio::test]
    async fn admitted_delivery_reclaimed_past_its_misfire_deadline_ends_with_its_input()
    -> Result<(), ScheduleDomainError> {
        let store = Arc::new(MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let service = ScheduleService::new(store.clone());
        let delivery = Arc::new(ExistingOnlyKeyedDelivery::default());
        let (schedule_id, identity) = first_attempt_died_past_the_deadline(
            &store,
            &service,
            &delivery.keyed,
            FirstAttemptEnd::AdmissionRecorded,
            "admitted-reclaim-past-deadline",
        )
        .await?;

        let driver = restarted_driver(&service, &store, delivery.clone());
        let report = driver.tick_once().await?;
        assert_eq!(report.fault_count(), 0, "the reclaim is not refused");
        let waiting =
            wait_for_occurrence_phase(&service, &schedule_id, OccurrencePhase::AwaitingCompletion)
                .await?;
        assert_eq!(waiting.attempt_count, 2);
        assert_eq!(
            delivery.existing_calls.lock().await.as_slice(),
            &[ExistingDelivery::Admitted]
        );
        assert_eq!(
            delivery.keyed.admissions.lock().await.as_slice(),
            &[
                RuntimeDeliveryOutcome::AdmissionAccepted,
                RuntimeDeliveryOutcome::AdmissionDeduplicated
            ],
            "the redelivery replays the first admission"
        );

        delivery
            .keyed
            .settle(
                &identity.idempotency_key,
                DeliveryTerminal::runtime_completion(
                    RuntimeCompletionOutcome::Completed,
                    None,
                    None,
                ),
            )
            .await;
        let completed =
            wait_for_occurrence_phase(&service, &schedule_id, OccurrencePhase::Completed).await?;
        assert_eq!(completed.attempt_count, 2);
        Ok(())
    }

    /// Only the dispatch start was recorded, and the target never admitted
    /// the delivery: the reclaim past the deadline asks the target, which
    /// answers not admitted, and the occurrence ends Misfired with nothing
    /// admitted.
    #[tokio::test]
    async fn unproven_late_redelivery_the_target_never_admitted_ends_misfired()
    -> Result<(), ScheduleDomainError> {
        let store = Arc::new(MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let service = ScheduleService::new(store.clone());
        let delivery = Arc::new(ExistingOnlyKeyedDelivery::default());
        let (schedule_id, _identity) = first_attempt_died_past_the_deadline(
            &store,
            &service,
            &delivery.keyed,
            FirstAttemptEnd::DispatchStarted,
            "unproven-late-not-admitted",
        )
        .await?;

        let driver = restarted_driver(&service, &store, delivery.clone());
        let report = driver.tick_once().await?;
        assert_eq!(report.fault_count(), 0);
        let misfired =
            wait_for_occurrence_phase(&service, &schedule_id, OccurrencePhase::Misfired).await?;
        assert_eq!(misfired.attempt_count, 2);
        assert_eq!(
            delivery.existing_calls.lock().await.as_slice(),
            &[ExistingDelivery::UnprovenLate]
        );
        assert!(
            delivery.keyed.admissions.lock().await.is_empty(),
            "nothing is admitted"
        );
        Ok(())
    }

    /// Only the dispatch start was recorded, but the target did admit the
    /// input: the late reclaim finds that admission, and the occurrence ends
    /// with the input's own outcome. Before the reclaim asked the target, it
    /// ended Misfired.
    #[tokio::test]
    async fn unproven_late_redelivery_the_target_admitted_ends_with_its_input()
    -> Result<(), ScheduleDomainError> {
        let store = Arc::new(MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let service = ScheduleService::new(store.clone());
        let delivery = Arc::new(ExistingOnlyKeyedDelivery::default());
        let (schedule_id, identity) = first_attempt_died_past_the_deadline(
            &store,
            &service,
            &delivery.keyed,
            FirstAttemptEnd::AdmittedUnrecorded,
            "unproven-late-admitted",
        )
        .await?;

        let driver = restarted_driver(&service, &store, delivery.clone());
        driver.tick_once().await?;
        let waiting =
            wait_for_occurrence_phase(&service, &schedule_id, OccurrencePhase::AwaitingCompletion)
                .await?;
        assert_eq!(waiting.attempt_count, 2);
        assert_eq!(
            delivery.existing_calls.lock().await.as_slice(),
            &[ExistingDelivery::UnprovenLate]
        );
        delivery
            .keyed
            .settle(
                &identity.idempotency_key,
                DeliveryTerminal::runtime_completion(
                    RuntimeCompletionOutcome::Abandoned,
                    Some("the admitted input failed".to_string()),
                    None,
                ),
            )
            .await;
        let failed =
            wait_for_occurrence_phase(&service, &schedule_id, OccurrencePhase::DeliveryFailed)
                .await?;
        assert_eq!(
            failed.failure_class,
            Some(OccurrenceFailureClass::RuntimeRejected)
        );
        Ok(())
    }

    /// A driver restarted after the executor died whose target probe answers
    /// Busy: the earlier attempt's input is still running on the target.
    fn restarted_driver_onto_a_busy_target(
        service: &ScheduleService,
        store: &Arc<dyn ScheduleStore>,
        delivery: Arc<dyn ScheduleTargetDelivery>,
    ) -> ScheduleDriver {
        ScheduleDriver::new(
            service.clone(),
            store.clone(),
            Arc::new(StaticProbe(TargetProbeOutcome::Busy {
                detail: Some("the earlier attempt's input is still running".to_string()),
            })),
            delivery,
            "restarted-owner",
            ScheduleDriverConfig {
                claim_limit: 8,
                lease_duration: Duration::seconds(30),
            },
        )
    }

    /// The one occurrence of `schedule_id`, read right after a tick.
    async fn the_occurrence(
        service: &ScheduleService,
        schedule_id: &crate::ScheduleId,
    ) -> Result<Occurrence, ScheduleDomainError> {
        let mut occurrences = service.list_occurrences(schedule_id).await?;
        assert_eq!(occurrences.len(), 1, "one occurrence: {occurrences:?}");
        Ok(occurrences.remove(0))
    }

    /// An admitted delivery reclaimed while its input still runs, under
    /// skip_if_running: the probe answers Busy, but the redelivery starts no
    /// new run, so the overlap policy does not end it. It replays the
    /// admitted input, and the occurrence ends with that input's outcome.
    /// Before the probe resolution knew the claim's redelivery it ended
    /// Skipped with target_busy.
    #[tokio::test]
    async fn admitted_reclaim_onto_a_busy_target_ends_with_its_input()
    -> Result<(), ScheduleDomainError> {
        let store = Arc::new(MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let service = ScheduleService::new(store.clone());
        let delivery = Arc::new(ExistingOnlyKeyedDelivery::default());
        let (schedule_id, identity) = first_attempt_died_past_the_deadline(
            &store,
            &service,
            &delivery.keyed,
            FirstAttemptEnd::AdmissionRecorded,
            "admitted-reclaim-busy-target",
        )
        .await?;

        let driver = restarted_driver_onto_a_busy_target(&service, &store, delivery.clone());
        let report = driver.tick_once().await?;
        assert_eq!(report.fault_count(), 0);
        let reclaimed = the_occurrence(&service, &schedule_id).await?;
        assert!(
            !matches!(
                reclaimed.phase,
                OccurrencePhase::Skipped | OccurrencePhase::Misfired
            ),
            "a busy target must not end an admitted redelivery: {:?} {:?}",
            reclaimed.phase,
            reclaimed.failure_class
        );
        let waiting =
            wait_for_occurrence_phase(&service, &schedule_id, OccurrencePhase::AwaitingCompletion)
                .await?;
        assert_eq!(waiting.attempt_count, 2);
        assert_eq!(
            delivery.existing_calls.lock().await.as_slice(),
            &[ExistingDelivery::Admitted]
        );
        assert_eq!(
            delivery.keyed.admissions.lock().await.as_slice(),
            &[
                RuntimeDeliveryOutcome::AdmissionAccepted,
                RuntimeDeliveryOutcome::AdmissionDeduplicated
            ],
            "the redelivery replays the first admission"
        );

        delivery
            .keyed
            .settle(
                &identity.idempotency_key,
                DeliveryTerminal::runtime_completion(
                    RuntimeCompletionOutcome::Completed,
                    None,
                    None,
                ),
            )
            .await;
        let completed =
            wait_for_occurrence_phase(&service, &schedule_id, OccurrencePhase::Completed).await?;
        assert_eq!(completed.attempt_count, 2);
        assert_eq!(completed.failure_class, None);
        Ok(())
    }

    /// Only the dispatch start was recorded, the target did admit the input,
    /// and that input still runs: the late reclaim's probe answers Busy, the
    /// redelivery finds the admission, and the occurrence ends with the
    /// input's outcome. Before the probe resolution knew the claim's
    /// redelivery it ended Skipped with target_busy.
    #[tokio::test]
    async fn unproven_late_reclaim_onto_a_busy_target_ends_with_its_input()
    -> Result<(), ScheduleDomainError> {
        let store = Arc::new(MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let service = ScheduleService::new(store.clone());
        let delivery = Arc::new(ExistingOnlyKeyedDelivery::default());
        let (schedule_id, identity) = first_attempt_died_past_the_deadline(
            &store,
            &service,
            &delivery.keyed,
            FirstAttemptEnd::AdmittedUnrecorded,
            "unproven-late-busy-target",
        )
        .await?;

        let driver = restarted_driver_onto_a_busy_target(&service, &store, delivery.clone());
        let report = driver.tick_once().await?;
        assert_eq!(report.fault_count(), 0);
        let reclaimed = the_occurrence(&service, &schedule_id).await?;
        assert!(
            !matches!(
                reclaimed.phase,
                OccurrencePhase::Skipped | OccurrencePhase::Misfired
            ),
            "a busy target must not end an unproven late redelivery the target admitted: {:?} {:?}",
            reclaimed.phase,
            reclaimed.failure_class
        );
        let waiting =
            wait_for_occurrence_phase(&service, &schedule_id, OccurrencePhase::AwaitingCompletion)
                .await?;
        assert_eq!(waiting.attempt_count, 2);
        assert_eq!(
            delivery.existing_calls.lock().await.as_slice(),
            &[ExistingDelivery::UnprovenLate]
        );
        assert_eq!(
            delivery.keyed.admissions.lock().await.as_slice(),
            &[
                RuntimeDeliveryOutcome::AdmissionAccepted,
                RuntimeDeliveryOutcome::AdmissionDeduplicated
            ],
            "the redelivery replays the first admission"
        );

        delivery
            .keyed
            .settle(
                &identity.idempotency_key,
                DeliveryTerminal::runtime_completion(
                    RuntimeCompletionOutcome::Completed,
                    None,
                    None,
                ),
            )
            .await;
        let completed =
            wait_for_occurrence_phase(&service, &schedule_id, OccurrencePhase::Completed).await?;
        assert_eq!(completed.attempt_count, 2);
        Ok(())
    }

    /// Only the dispatch start was recorded, the target never admitted the
    /// delivery, and it is busy with other work: the late reclaim still asks
    /// the target rather than skipping, and the occurrence ends Misfired
    /// with nothing admitted, as with a ready target. Before the probe
    /// resolution knew the claim's redelivery it ended Skipped with
    /// target_busy.
    #[tokio::test]
    async fn unproven_late_reclaim_onto_a_busy_target_that_never_admitted_it_misfires()
    -> Result<(), ScheduleDomainError> {
        let store = Arc::new(MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let service = ScheduleService::new(store.clone());
        let delivery = Arc::new(ExistingOnlyKeyedDelivery::default());
        let (schedule_id, _identity) = first_attempt_died_past_the_deadline(
            &store,
            &service,
            &delivery.keyed,
            FirstAttemptEnd::DispatchStarted,
            "unproven-late-busy-not-admitted",
        )
        .await?;

        let driver = restarted_driver_onto_a_busy_target(&service, &store, delivery.clone());
        let report = driver.tick_once().await?;
        assert_eq!(report.fault_count(), 0);
        let ended = the_occurrence(&service, &schedule_id).await?;
        assert_eq!(
            (ended.phase, ended.failure_class),
            (OccurrencePhase::Misfired, None),
            "the redelivery asks the target and misfires, never skips as busy"
        );
        assert_eq!(ended.attempt_count, 2);
        assert_eq!(
            delivery.existing_calls.lock().await.as_slice(),
            &[ExistingDelivery::UnprovenLate]
        );
        assert!(
            delivery.keyed.admissions.lock().await.is_empty(),
            "nothing is admitted"
        );
        Ok(())
    }

    /// A redelivery claimed after the first attempt ended as `end`, onto a
    /// target whose probe answers Missing, under `missing_target_policy`: one
    /// restarted tick. Returns the ended occurrence, the stages of its
    /// receipts, and the delivery.
    async fn redelivery_onto_a_missing_target(
        end: FirstAttemptEnd,
        missing_target_policy: MissingTargetPolicy,
        name: &str,
    ) -> Result<
        (
            Occurrence,
            Vec<DeliveryReceiptStage>,
            Arc<ExistingOnlyKeyedDelivery>,
        ),
        ScheduleDomainError,
    > {
        let store = Arc::new(MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let service = ScheduleService::new(store.clone());
        let delivery = Arc::new(ExistingOnlyKeyedDelivery::default());
        let (schedule_id, _identity) = first_attempt_died_past_the_deadline_under(
            &store,
            &service,
            &delivery.keyed,
            end,
            name,
            missing_target_policy,
        )
        .await?;

        let driver = ScheduleDriver::new(
            service.clone(),
            store.clone(),
            Arc::new(StaticProbe(TargetProbeOutcome::Missing {
                detail: Some("the target session is gone".to_string()),
            })),
            delivery.clone(),
            "restarted-owner",
            ScheduleDriverConfig {
                claim_limit: 8,
                lease_duration: Duration::seconds(30),
            },
        );
        let report = driver.tick_once().await?;
        assert_eq!(report.fault_count(), 0);
        let ended = the_occurrence(&service, &schedule_id).await?;
        let stages = store
            .list_receipts(&ended.occurrence_id)
            .await?
            .into_iter()
            .map(|receipt| receipt.stage)
            .collect();
        Ok((ended, stages, delivery))
    }

    /// An admitted redelivery whose target is gone follows the missing-target
    /// policy, as any claim does: skip ends it Skipped and mark_misfired ends
    /// it Misfired, both with target_missing, and the target is never asked
    /// for the earlier admission. The receipts keep the first attempt's
    /// admission, so a Skipped ending does not read as a delivery that never
    /// ran.
    #[tokio::test]
    async fn an_admitted_redelivery_onto_a_missing_target_follows_the_missing_target_policy()
    -> Result<(), ScheduleDomainError> {
        for (policy, phase) in [
            (MissingTargetPolicy::Skip, OccurrencePhase::Skipped),
            (MissingTargetPolicy::MarkMisfired, OccurrencePhase::Misfired),
        ] {
            let (ended, stages, delivery) = redelivery_onto_a_missing_target(
                FirstAttemptEnd::AdmissionRecorded,
                policy.clone(),
                &format!("admitted-redelivery-missing-target-{policy:?}"),
            )
            .await?;
            assert_eq!(
                (ended.phase, ended.failure_class),
                (phase, Some(OccurrenceFailureClass::TargetMissing)),
                "{policy:?}"
            );
            assert_eq!(ended.attempt_count, 2, "{policy:?}");
            assert!(
                delivery.existing_calls.lock().await.is_empty(),
                "{policy:?}: the probe ends the redelivery before the target is asked"
            );
            assert_eq!(
                delivery.keyed.admissions.lock().await.as_slice(),
                &[RuntimeDeliveryOutcome::AdmissionAccepted],
                "{policy:?}: only the first attempt was admitted"
            );
            assert!(
                stages.contains(&DeliveryReceiptStage::DispatchAccepted),
                "{policy:?}: the receipts keep the first attempt's admission: {stages:?}"
            );
        }
        Ok(())
    }

    /// An unproven late redelivery whose target is gone follows the
    /// missing-target policy too: under skip it ends Skipped with
    /// target_missing, not Misfired as a target that answers not admitted
    /// would end it. Nothing is admitted.
    #[tokio::test]
    async fn an_unproven_late_redelivery_onto_a_missing_target_follows_the_missing_target_policy()
    -> Result<(), ScheduleDomainError> {
        for (policy, phase) in [
            (MissingTargetPolicy::Skip, OccurrencePhase::Skipped),
            (MissingTargetPolicy::MarkMisfired, OccurrencePhase::Misfired),
        ] {
            let (ended, stages, delivery) = redelivery_onto_a_missing_target(
                FirstAttemptEnd::DispatchStarted,
                policy.clone(),
                &format!("unproven-late-missing-target-{policy:?}"),
            )
            .await?;
            assert_eq!(
                (ended.phase, ended.failure_class),
                (phase, Some(OccurrenceFailureClass::TargetMissing)),
                "{policy:?}"
            );
            assert_eq!(ended.attempt_count, 2, "{policy:?}");
            assert!(
                delivery.existing_calls.lock().await.is_empty(),
                "{policy:?}: the probe ends the redelivery before the target is asked"
            );
            assert!(
                delivery.keyed.admissions.lock().await.is_empty(),
                "{policy:?}: nothing is admitted"
            );
            assert!(
                stages.contains(&DeliveryReceiptStage::DispatchStarted)
                    && !stages.contains(&DeliveryReceiptStage::DispatchAccepted),
                "{policy:?}: {stages:?}"
            );
        }
        Ok(())
    }

    /// An implementation that keeps the trait's default never delivers an
    /// unproven late occurrence fresh: the default answers not admitted, and
    /// the occurrence ends Misfired as before, with no second delivery. A
    /// default that called `deliver_occurrence` here would admit it.
    #[tokio::test]
    async fn default_existing_delivery_never_delivers_an_unproven_late_occurrence()
    -> Result<(), ScheduleDomainError> {
        let store = Arc::new(MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let service = ScheduleService::new(store.clone());
        let delivery = Arc::new(KeyedInputDelivery::default());
        let (schedule_id, _identity) = first_attempt_died_past_the_deadline(
            &store,
            &service,
            &delivery,
            FirstAttemptEnd::DispatchStarted,
            "default-unproven-late",
        )
        .await?;

        let driver = restarted_driver(&service, &store, delivery.clone());
        driver.tick_once().await?;
        let misfired =
            wait_for_occurrence_phase(&service, &schedule_id, OccurrencePhase::Misfired).await?;
        assert_eq!(misfired.attempt_count, 2);
        assert!(
            delivery.admissions.lock().await.is_empty(),
            "the default delivers nothing"
        );
        Ok(())
    }

    /// No dispatch was recorded at all: past the deadline the misfire policy
    /// decides as before, and nothing is reclaimed.
    #[tokio::test]
    async fn a_claim_without_a_recorded_dispatch_still_misfires_past_the_deadline()
    -> Result<(), ScheduleDomainError> {
        let store = Arc::new(MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let service = ScheduleService::new(store.clone());
        let schedule = service
            .create(CreateScheduleRequest {
                name: Some("no-recorded-dispatch".into()),
                description: None,
                trigger: TriggerSpec::Once {
                    due_at_utc: Utc::now() - Duration::milliseconds(200),
                },
                target: materialize_on_demand_target("scheduled prompt"),
                misfire_policy: MisfirePolicy::CatchUpWithin { window_seconds: 1 },
                overlap_policy: OverlapPolicy::SkipIfRunning,
                missing_target_policy: MissingTargetPolicy::MarkMisfired,
                labels: BTreeMap::new(),
                planning_horizon_days: Some(1),
                planning_horizon_occurrences: Some(1),
            })
            .await?;
        let executor_lease = test_executor_lease(&store).await;
        let claimed = store
            .claim_due_occurrences(
                &executor_lease,
                ClaimDueRequest {
                    owner_id: "dying-process".into(),
                    limit: 1,
                    lease_duration: Duration::milliseconds(25),
                },
            )
            .await?;
        store.release_executor_lease(executor_lease).await?;
        assert_eq!(claimed.transitions.len(), 1);
        sleep(std::time::Duration::from_millis(1_200)).await;

        let delivery = Arc::new(ExistingOnlyKeyedDelivery::default());
        let driver = restarted_driver(&service, &store, delivery.clone());
        driver.tick_once().await?;
        driver.tick_once().await?;
        let misfired =
            wait_for_occurrence_phase(&service, &schedule.schedule_id, OccurrencePhase::Misfired)
                .await?;
        assert_eq!(misfired.attempt_count, 1, "no reclaim");
        assert!(delivery.existing_calls.lock().await.is_empty());
        assert!(delivery.keyed.admissions.lock().await.is_empty());
        Ok(())
    }

    /// An admitted delivery whose executors keep dying is reclaimed past its
    /// misfire deadline until its attempts reach the cap. The tick that
    /// expires the last lapsed lease ends it delivery_failed with lease_lost
    /// and reports no fault. Before, that tick reported the refused reclaim
    /// as a row fault, and the next tick failed: its claim scan returned the
    /// exhausted occurrence's DeliveryFailed successor, which the driver took
    /// for a broken store contract, leaving the rest of that tick's claims
    /// undelivered.
    #[tokio::test]
    async fn an_admitted_delivery_that_used_up_its_reclaims_ends_lease_lost_in_one_clean_tick()
    -> Result<(), ScheduleDomainError> {
        let store = Arc::new(MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let service = ScheduleService::new(store.clone());
        let delivery = Arc::new(ExistingOnlyKeyedDelivery::default());
        let (schedule_id, _identity) = first_attempt_died_past_the_deadline(
            &store,
            &service,
            &delivery.keyed,
            FirstAttemptEnd::AdmissionRecorded,
            "admitted-reclaims-used-up",
        )
        .await?;
        // Every later executor dies too: each reclaim up to the cap lapses.
        for attempt in 2..=crate::lifecycle::ADMITTED_RECLAIM_ATTEMPT_CAP {
            let executor_lease = test_executor_lease(&store).await;
            let reclaimed = store
                .claim_due_occurrences(
                    &executor_lease,
                    ClaimDueRequest {
                        owner_id: "dying-process".into(),
                        limit: 1,
                        lease_duration: Duration::milliseconds(25),
                    },
                )
                .await?;
            store.release_executor_lease(executor_lease).await?;
            assert!(
                reclaimed.row_faults.is_empty(),
                "{:?}",
                reclaimed.row_faults
            );
            assert_eq!(
                the_occurrence(&service, &schedule_id).await?.attempt_count,
                attempt
            );
            sleep(std::time::Duration::from_millis(50)).await;
        }

        let driver = restarted_driver(&service, &store, delivery.clone());
        let report = driver.tick_once().await?;
        assert_eq!(
            report.fault_count(),
            0,
            "the refused reclaim at the cap is no incident"
        );
        let ended = the_occurrence(&service, &schedule_id).await?;
        assert_eq!(
            (ended.phase, ended.failure_class),
            (
                OccurrencePhase::DeliveryFailed,
                Some(OccurrenceFailureClass::LeaseLost)
            )
        );
        assert_eq!(
            ended.attempt_count,
            crate::lifecycle::ADMITTED_RECLAIM_ATTEMPT_CAP
        );
        assert!(
            delivery.existing_calls.lock().await.is_empty(),
            "nothing is redelivered past the cap"
        );
        assert_eq!(
            delivery.keyed.admissions.lock().await.as_slice(),
            &[RuntimeDeliveryOutcome::AdmissionAccepted],
            "only the first attempt was admitted"
        );
        Ok(())
    }

    /// Commit only the pre-effect intent-bearing occurrence state, with no
    /// adapter call or live completion waiter. This is the exact durable
    /// crash boundary between intent and target effect.
    async fn claim_and_start_dispatch_without_waiter(
        store: &Arc<dyn ScheduleStore>,
        lease_duration: Duration,
    ) -> Result<Occurrence, ScheduleDomainError> {
        let executor_lease = test_executor_lease(store).await;
        let claimed = store
            .claim_due_occurrences(
                &executor_lease,
                ClaimDueRequest {
                    owner_id: "other-process".into(),
                    limit: 1,
                    lease_duration,
                },
            )
            .await?;
        store.release_executor_lease(executor_lease).await?;
        let occurrence = claimed
            .transitions
            .into_iter()
            .next()
            .ok_or_else(|| ScheduleDomainError::Internal("expected a claim".to_string()))?
            .successor()
            .clone();
        let identity = ScheduleDeliveryIdentity::for_occurrence(&occurrence);
        let commit = store
            .transition_occurrence_with_receipt_if_current(
                &occurrence.occurrence_id,
                occurrence.attempt_count,
                occurrence.claim_token(),
                OccurrenceLifecycleInput::DispatchStarted {
                    correlation_id: Some(identity.correlation_id),
                    at_utc: claimed.store_now_utc,
                },
                None,
            )
            .await?
            .ok_or_else(|| {
                ScheduleDomainError::Internal(
                    "claim evidence went stale before dispatch intent".to_string(),
                )
            })?;
        Ok(commit.successor().clone())
    }

    /// Claim an occurrence through the store and drive it to
    /// AwaitingCompletion through the occurrence authority without spawning
    /// a waiter task — the durable footprint of a deliverer that crashed (or
    /// lives in another process).
    async fn claim_and_dispatch_without_waiter(
        store: &Arc<dyn ScheduleStore>,
        lease_duration: Duration,
    ) -> Result<Occurrence, ScheduleDomainError> {
        let executor_lease = test_executor_lease(store).await;
        let claimed = store
            .claim_due_occurrences(
                &executor_lease,
                ClaimDueRequest {
                    owner_id: "other-process".into(),
                    limit: 1,
                    lease_duration,
                },
            )
            .await?;
        store.release_executor_lease(executor_lease).await?;
        let occurrence = claimed
            .transitions
            .into_iter()
            .next()
            .ok_or_else(|| ScheduleDomainError::Internal("expected a claim".to_string()))?
            .successor()
            .clone();
        let dispatch_mutator = occurrence
            .apply(OccurrenceLifecycleInput::DispatchStarted {
                correlation_id: Some("other-process-dispatch".into()),
                at_utc: claimed.store_now_utc,
            })
            .map_err(|error| ScheduleDomainError::Internal(error.to_string()))?;
        let dispatch_commit = store
            .commit_occurrence_write(dispatch_mutator.into_authorized_write())
            .await?;
        assert_eq!(
            dispatch_commit.effects(),
            &[OccurrenceLifecycleEffect::DispatchStarted]
        );
        let await_mutator = dispatch_commit
            .successor()
            .clone()
            .apply(OccurrenceLifecycleInput::AwaitCompletion {
                at_utc: claimed.store_now_utc,
            })
            .map_err(|error| ScheduleDomainError::Internal(error.to_string()))?;
        let await_commit = store
            .commit_occurrence_write(await_mutator.into_authorized_write())
            .await?;
        assert_eq!(
            await_commit.effects(),
            &[OccurrenceLifecycleEffect::AwaitingCompletion]
        );
        Ok(await_commit.successor().clone())
    }

    /// Stale-completion FENCING still holds under the renewal contract: when
    /// a genuinely dead deliverer's attempt (no live waiter in this process)
    /// is reclaimed, a late completion arriving with the expired attempt's
    /// claim evidence must not overwrite the reclaimed attempt.
    #[tokio::test]
    async fn late_completion_from_expired_attempt_does_not_overwrite_reclaimed_attempt()
    -> Result<(), ScheduleDomainError> {
        let store = Arc::new(MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let service = ScheduleService::new(store.clone());
        let schedule = service
            .create(CreateScheduleRequest {
                name: Some("late-completion".into()),
                description: None,
                trigger: TriggerSpec::Once {
                    due_at_utc: Utc::now() - Duration::seconds(1),
                },
                target: materialize_on_demand_target("scheduled prompt"),
                misfire_policy: MisfirePolicy::Skip,
                overlap_policy: OverlapPolicy::SkipIfRunning,
                missing_target_policy: MissingTargetPolicy::MarkMisfired,
                labels: BTreeMap::new(),
                planning_horizon_days: Some(1),
                planning_horizon_occurrences: Some(1),
            })
            .await?;

        // Attempt 1 belongs to a "crashed" deliverer: dispatched durably,
        // but with no live waiter and no renewal in this process. Keep its
        // stale snapshot to replay the zombie completion later.
        let stale_attempt =
            claim_and_dispatch_without_waiter(&store, Duration::milliseconds(25)).await?;
        sleep(std::time::Duration::from_millis(35)).await;

        // The driver reclaims the expired attempt and dispatches attempt 2.
        let delivery = Arc::new(ControlledCompletionDelivery::default());
        let driver = ScheduleDriver::new(
            service.clone(),
            store.clone(),
            Arc::new(ReadyProbe),
            delivery.clone(),
            "driver-owner",
            ScheduleDriverConfig {
                claim_limit: 8,
                lease_duration: Duration::seconds(30),
            },
        );
        driver.tick_once().await?;
        wait_for_sender_count(&delivery, 1).await;
        wait_for_occurrence_attempt(&service, &schedule.schedule_id, 2).await?;

        // The zombie deliverer's completion arrives bearing attempt 1's
        // claim evidence; the store screen must reject it.
        complete_dispatched_occurrence(
            store.clone(),
            stale_attempt,
            Ok(DeliveryTerminal::completed(None)),
        )
        .await?;

        let after_stale_completion = service
            .list_occurrences(&schedule.schedule_id)
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| ScheduleDomainError::Internal("occurrence should exist".to_string()))?;
        assert_eq!(after_stale_completion.attempt_count, 2);
        assert_eq!(
            after_stale_completion.phase,
            OccurrencePhase::AwaitingCompletion
        );

        let second_attempt = delivery.senders.lock().await.remove(0);
        second_attempt
            .send(DeliveryTerminal::completed(None))
            .expect("second attempt sender should be open");

        let completed =
            wait_for_occurrence_phase(&service, &schedule.schedule_id, OccurrencePhase::Completed)
                .await?;
        assert_eq!(completed.attempt_count, 2);
        let receipts = store.list_receipts(&completed.occurrence_id).await?;
        assert_eq!(
            receipts.last().map(|receipt| receipt.attempt),
            Some(2),
            "late completion from the expired lease must not overwrite the reclaimed attempt"
        );
        Ok(())
    }

    #[tokio::test]
    async fn paused_claimed_occurrence_is_released_before_probe_or_delivery()
    -> Result<(), ScheduleDomainError> {
        let store = Arc::new(MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let service = ScheduleService::new(store.clone());
        let schedule = service
            .create(CreateScheduleRequest {
                name: Some("pause-claimed".into()),
                description: None,
                trigger: TriggerSpec::Once {
                    due_at_utc: Utc::now() - Duration::seconds(1),
                },
                target: materialize_on_demand_target("scheduled prompt"),
                misfire_policy: MisfirePolicy::Skip,
                overlap_policy: OverlapPolicy::SkipIfRunning,
                missing_target_policy: MissingTargetPolicy::MarkMisfired,
                labels: BTreeMap::new(),
                planning_horizon_days: Some(1),
                planning_horizon_occurrences: Some(1),
            })
            .await?;
        let probe = Arc::new(CountingProbe::default());
        let delivery = Arc::new(CountingDelivery::default());
        let driver = ScheduleDriver::new(
            service.clone(),
            store.clone(),
            probe.clone(),
            delivery.clone(),
            "driver-owner",
            ScheduleDriverConfig {
                claim_limit: 8,
                lease_duration: Duration::seconds(30),
            },
        );
        let executor_lease = test_executor_lease(&store).await;
        let claimed = store
            .claim_due_occurrences(
                &executor_lease,
                ClaimDueRequest {
                    owner_id: "driver-owner".into(),
                    limit: 1,
                    lease_duration: Duration::seconds(30),
                },
            )
            .await?;
        store.release_executor_lease(executor_lease).await?;
        let occurrence = claimed
            .transitions
            .into_iter()
            .next()
            .expect("claimed occurrence")
            .successor()
            .clone();
        service.pause(&schedule.schedule_id).await?;

        let terminalized = driver
            .handle_claimed_occurrence(occurrence.clone(), claimed.store_now_utc, None)
            .await?;
        let current = service
            .list_occurrences(&schedule.schedule_id)
            .await?
            .into_iter()
            .find(|item| item.occurrence_id == occurrence.occurrence_id)
            .expect("occurrence should still exist");

        assert!(!terminalized, "paused claimed work should be frozen");
        assert_eq!(current.phase, OccurrencePhase::Pending);
        assert_eq!(*probe.calls.lock().await, 0, "pause should block probes");
        assert_eq!(
            *delivery.calls.lock().await,
            0,
            "pause should block delivery"
        );
        let receipts = store.list_receipts(&current.occurrence_id).await?;
        assert!(
            receipts
                .iter()
                .any(|receipt| receipt.stage == DeliveryReceiptStage::LeaseExpired),
            "pause should release the claim immediately"
        );
        Ok(())
    }

    #[tokio::test]
    async fn deleted_claimed_occurrence_is_superseded_before_delivery()
    -> Result<(), ScheduleDomainError> {
        let store = Arc::new(MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let service = ScheduleService::new(store.clone());
        let schedule = service
            .create(CreateScheduleRequest {
                name: Some("delete-claimed".into()),
                description: None,
                trigger: TriggerSpec::Once {
                    due_at_utc: Utc::now() - Duration::seconds(1),
                },
                target: materialize_on_demand_target("scheduled prompt"),
                misfire_policy: MisfirePolicy::Skip,
                overlap_policy: OverlapPolicy::SkipIfRunning,
                missing_target_policy: MissingTargetPolicy::MarkMisfired,
                labels: BTreeMap::new(),
                planning_horizon_days: Some(1),
                planning_horizon_occurrences: Some(1),
            })
            .await?;
        let probe = Arc::new(CountingProbe::default());
        let delivery = Arc::new(CountingDelivery::default());
        let driver = ScheduleDriver::new(
            service.clone(),
            store.clone(),
            probe.clone(),
            delivery.clone(),
            "driver-owner",
            ScheduleDriverConfig {
                claim_limit: 8,
                lease_duration: Duration::seconds(30),
            },
        );
        let executor_lease = test_executor_lease(&store).await;
        let claimed = store
            .claim_due_occurrences(
                &executor_lease,
                ClaimDueRequest {
                    owner_id: "driver-owner".into(),
                    limit: 1,
                    lease_duration: Duration::seconds(30),
                },
            )
            .await?;
        store.release_executor_lease(executor_lease).await?;
        let occurrence = claimed
            .transitions
            .into_iter()
            .next()
            .expect("claimed occurrence")
            .successor()
            .clone();
        service.delete(&schedule.schedule_id).await?;

        let terminalized = driver
            .handle_claimed_occurrence(occurrence.clone(), claimed.store_now_utc, None)
            .await?;
        let current = service
            .list_occurrences(&schedule.schedule_id)
            .await?
            .into_iter()
            .find(|item| item.occurrence_id == occurrence.occurrence_id)
            .expect("occurrence should still exist");

        assert!(terminalized, "deleted claimed work should supersede");
        assert_eq!(current.phase, OccurrencePhase::Superseded);
        assert_eq!(*probe.calls.lock().await, 0, "delete should block probes");
        assert_eq!(
            *delivery.calls.lock().await,
            0,
            "delete should block delivery"
        );
        Ok(())
    }

    #[tokio::test]
    async fn stale_revision_claimed_occurrence_is_superseded_before_delivery()
    -> Result<(), ScheduleDomainError> {
        let store = Arc::new(MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let service = ScheduleService::new(store.clone());
        let schedule = service
            .create(CreateScheduleRequest {
                name: Some("stale-claimed".into()),
                description: None,
                trigger: TriggerSpec::Once {
                    due_at_utc: Utc::now() - Duration::seconds(1),
                },
                target: materialize_on_demand_target("scheduled prompt"),
                misfire_policy: MisfirePolicy::Skip,
                overlap_policy: OverlapPolicy::SkipIfRunning,
                missing_target_policy: MissingTargetPolicy::MarkMisfired,
                labels: BTreeMap::new(),
                planning_horizon_days: Some(1),
                planning_horizon_occurrences: Some(1),
            })
            .await?;
        let probe = Arc::new(CountingProbe::default());
        let delivery = Arc::new(CountingDelivery::default());
        let driver = ScheduleDriver::new(
            service.clone(),
            store.clone(),
            probe.clone(),
            delivery.clone(),
            "driver-owner",
            ScheduleDriverConfig {
                claim_limit: 8,
                lease_duration: Duration::seconds(30),
            },
        );
        let executor_lease = test_executor_lease(&store).await;
        let claimed = store
            .claim_due_occurrences(
                &executor_lease,
                ClaimDueRequest {
                    owner_id: "driver-owner".into(),
                    limit: 1,
                    lease_duration: Duration::seconds(30),
                },
            )
            .await?;
        store.release_executor_lease(executor_lease).await?;
        let occurrence = claimed
            .transitions
            .into_iter()
            .next()
            .expect("claimed occurrence")
            .successor()
            .clone();
        let updated = service
            .update(
                &schedule.schedule_id,
                UpdateScheduleRequest {
                    expected_revision: Some(schedule.revision),
                    trigger: Some(TriggerSpec::Interval(IntervalTriggerSpec {
                        start_at_utc: Utc::now() + Duration::minutes(5),
                        every_seconds: 300,
                        end_at_utc: None,
                    })),
                    ..UpdateScheduleRequest::default()
                },
            )
            .await?;

        let terminalized = driver
            .handle_claimed_occurrence(occurrence.clone(), claimed.store_now_utc, None)
            .await?;
        let current = service
            .list_occurrences(&schedule.schedule_id)
            .await?
            .into_iter()
            .find(|item| item.occurrence_id == occurrence.occurrence_id)
            .expect("occurrence should still exist");

        assert!(terminalized, "stale claimed work should supersede");
        assert_eq!(current.phase, OccurrencePhase::Superseded);
        assert_eq!(
            current.superseded_by_revision,
            Some(updated.revision),
            "stale claimed work should record the current schedule revision"
        );
        assert_eq!(
            *probe.calls.lock().await,
            0,
            "stale revision should block probes"
        );
        assert_eq!(
            *delivery.calls.lock().await,
            0,
            "stale revision should block delivery"
        );
        Ok(())
    }

    #[tokio::test]
    async fn awaiting_completion_occurrence_is_superseded_when_schedule_is_deleted()
    -> Result<(), ScheduleDomainError> {
        let store = Arc::new(MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let service = ScheduleService::new(store.clone());
        let schedule = service
            .create(CreateScheduleRequest {
                name: Some("delete-awaiting".into()),
                description: None,
                trigger: TriggerSpec::Once {
                    due_at_utc: Utc::now() - Duration::seconds(1),
                },
                target: materialize_on_demand_target("scheduled prompt"),
                misfire_policy: MisfirePolicy::Skip,
                overlap_policy: OverlapPolicy::SkipIfRunning,
                missing_target_policy: MissingTargetPolicy::MarkMisfired,
                labels: BTreeMap::new(),
                planning_horizon_days: Some(1),
                planning_horizon_occurrences: Some(1),
            })
            .await?;
        let delivery = Arc::new(ControlledCompletionDelivery::default());
        let driver = ScheduleDriver::new(
            service.clone(),
            store.clone(),
            Arc::new(ReadyProbe),
            delivery.clone(),
            "driver-owner",
            ScheduleDriverConfig {
                claim_limit: 8,
                lease_duration: Duration::seconds(30),
            },
        );

        driver.tick_once().await?;
        wait_for_sender_count(&delivery, 1).await;
        let awaiting = wait_for_occurrence_phase(
            &service,
            &schedule.schedule_id,
            OccurrencePhase::AwaitingCompletion,
        )
        .await?;

        let deleted = service.delete(&schedule.schedule_id).await?;
        let sender = delivery.senders.lock().await.remove(0);
        sender
            .send(DeliveryTerminal::completed(None))
            .expect("sender should stay open");

        let superseded = loop {
            let occurrence = service
                .list_occurrences(&schedule.schedule_id)
                .await?
                .into_iter()
                .find(|item| item.occurrence_id == awaiting.occurrence_id)
                .expect("occurrence should still exist");
            if occurrence.phase == OccurrencePhase::Superseded {
                break occurrence;
            }
            sleep(std::time::Duration::from_millis(10)).await;
        };

        assert_eq!(superseded.superseded_by_revision, Some(deleted.revision));
        Ok(())
    }

    #[tokio::test]
    async fn awaiting_completion_occurrence_is_superseded_when_schedule_revision_advances()
    -> Result<(), ScheduleDomainError> {
        let store = Arc::new(MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let service = ScheduleService::new(store.clone());
        let schedule = service
            .create(CreateScheduleRequest {
                name: Some("update-awaiting".into()),
                description: None,
                trigger: TriggerSpec::Once {
                    due_at_utc: Utc::now() - Duration::seconds(1),
                },
                target: materialize_on_demand_target("scheduled prompt"),
                misfire_policy: MisfirePolicy::Skip,
                overlap_policy: OverlapPolicy::SkipIfRunning,
                missing_target_policy: MissingTargetPolicy::MarkMisfired,
                labels: BTreeMap::new(),
                planning_horizon_days: Some(1),
                planning_horizon_occurrences: Some(1),
            })
            .await?;
        let delivery = Arc::new(ControlledCompletionDelivery::default());
        let driver = ScheduleDriver::new(
            service.clone(),
            store.clone(),
            Arc::new(ReadyProbe),
            delivery.clone(),
            "driver-owner",
            ScheduleDriverConfig {
                claim_limit: 8,
                lease_duration: Duration::seconds(30),
            },
        );

        driver.tick_once().await?;
        wait_for_sender_count(&delivery, 1).await;
        let awaiting = wait_for_occurrence_phase(
            &service,
            &schedule.schedule_id,
            OccurrencePhase::AwaitingCompletion,
        )
        .await?;

        let updated = service
            .update(
                &schedule.schedule_id,
                UpdateScheduleRequest {
                    expected_revision: Some(schedule.revision),
                    trigger: Some(TriggerSpec::Interval(IntervalTriggerSpec {
                        start_at_utc: Utc::now() + Duration::minutes(5),
                        every_seconds: 300,
                        end_at_utc: None,
                    })),
                    ..UpdateScheduleRequest::default()
                },
            )
            .await?;
        let sender = delivery.senders.lock().await.remove(0);
        sender
            .send(DeliveryTerminal::completed(None))
            .expect("sender should stay open");

        let superseded = loop {
            let occurrence = service
                .list_occurrences(&schedule.schedule_id)
                .await?
                .into_iter()
                .find(|item| item.occurrence_id == awaiting.occurrence_id)
                .expect("occurrence should still exist");
            if occurrence.phase == OccurrencePhase::Superseded {
                break occurrence;
            }
            sleep(std::time::Duration::from_millis(10)).await;
        };

        assert_eq!(superseded.superseded_by_revision, Some(updated.revision));
        Ok(())
    }

    // -----------------------------------------------------------------------
    // 0.7.2 disciplined shell inputs (D1/D2a) — shell interleaving tests.
    // Tests marked "STAGE B" assert the wired shell behavior and are expected
    // RED after Stage A codegen (the DSL totality is in; the shell sequencing
    // is not). The lead records them on the red list.
    // -----------------------------------------------------------------------

    /// STAGE B (RED until wired): `service.delete()` must revoke the
    /// driver-claimed in-flight occurrence AT COMMIT by superseding it through
    /// the occurrence authority's typed Supersede transition — not leave it
    /// AwaitingCompletion for the completion waiter to discover later. The
    /// waiter's late resolution then lands as the typed late-arrival record,
    /// never a guard rejection and never a silent drop.
    #[tokio::test]
    async fn delete_revokes_in_flight_claim_at_commit_and_late_completion_lands_typed()
    -> Result<(), ScheduleDomainError> {
        let store = Arc::new(MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let service = ScheduleService::new(store.clone());
        let schedule = service
            .create(CreateScheduleRequest {
                name: Some("delete-revokes-in-flight".into()),
                description: None,
                trigger: TriggerSpec::Once {
                    due_at_utc: Utc::now() - Duration::seconds(1),
                },
                target: materialize_on_demand_target("scheduled prompt"),
                misfire_policy: MisfirePolicy::Skip,
                overlap_policy: OverlapPolicy::SkipIfRunning,
                missing_target_policy: MissingTargetPolicy::MarkMisfired,
                labels: BTreeMap::new(),
                planning_horizon_days: Some(1),
                planning_horizon_occurrences: Some(1),
            })
            .await?;
        let delivery = Arc::new(ControlledCompletionDelivery::default());
        let driver = ScheduleDriver::new(
            service.clone(),
            store.clone(),
            Arc::new(ReadyProbe),
            delivery.clone(),
            "driver-owner",
            ScheduleDriverConfig {
                claim_limit: 8,
                lease_duration: Duration::seconds(30),
            },
        );

        driver.tick_once().await?;
        wait_for_sender_count(&delivery, 1).await;
        let awaiting = wait_for_occurrence_phase(
            &service,
            &schedule.schedule_id,
            OccurrencePhase::AwaitingCompletion,
        )
        .await?;

        // Teardown commits while the completion waiter is still in flight.
        let deleted = service.delete(&schedule.schedule_id).await?;

        // D1: the delete commit itself revokes the in-flight claim.
        let at_commit = service
            .list_occurrences(&schedule.schedule_id)
            .await?
            .into_iter()
            .find(|item| item.occurrence_id == awaiting.occurrence_id)
            .ok_or_else(|| {
                ScheduleDomainError::Internal("occurrence should still exist".to_string())
            })?;
        assert_eq!(
            at_commit.phase,
            OccurrencePhase::Superseded,
            "delete must supersede the driver-claimed in-flight occurrence at commit time"
        );
        assert_eq!(at_commit.superseded_by_revision, Some(deleted.revision));
        assert!(
            deleted.superseded_ack_ids.contains(&awaiting.occurrence_id),
            "the revoked in-flight claim must be accounted in the schedule authority's ack set"
        );

        // The waiter resolves AFTER the teardown committed: typed late-arrival
        // record, zero corruption of the recorded supersession.
        let sender = delivery.senders.lock().await.remove(0);
        sender
            .send(DeliveryTerminal::completed(None))
            .expect("completion receiver should be open");

        let late = wait_for_late_completion_record(&service, &schedule.schedule_id).await?;
        assert_eq!(late.phase, OccurrencePhase::Superseded);
        assert_eq!(late.superseded_by_revision, Some(deleted.revision));
        assert_eq!(
            late.machine_state.late_completion_resolution,
            Some(crate::machines::occurrence_lifecycle::LateCompletionResolutionClass::DeliveryCompleted)
        );
        Ok(())
    }

    /// STAGE B (RED until wired): delete supersedes a CLAIMED (pre-dispatch)
    /// occurrence at commit; the driver's subsequent reconcile of its held
    /// claim is a benign idempotent no-op — no probe, no delivery, no
    /// duplicate superseded receipt.
    #[tokio::test]
    async fn delete_supersedes_claimed_occurrence_at_commit_without_duplicate_receipt()
    -> Result<(), ScheduleDomainError> {
        let store = Arc::new(MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let service = ScheduleService::new(store.clone());
        let schedule = service
            .create(CreateScheduleRequest {
                name: Some("delete-claimed-at-commit".into()),
                description: None,
                trigger: TriggerSpec::Once {
                    due_at_utc: Utc::now() - Duration::seconds(1),
                },
                target: materialize_on_demand_target("scheduled prompt"),
                misfire_policy: MisfirePolicy::Skip,
                overlap_policy: OverlapPolicy::SkipIfRunning,
                missing_target_policy: MissingTargetPolicy::MarkMisfired,
                labels: BTreeMap::new(),
                planning_horizon_days: Some(1),
                planning_horizon_occurrences: Some(1),
            })
            .await?;
        let probe = Arc::new(CountingProbe::default());
        let delivery = Arc::new(CountingDelivery::default());
        let driver = ScheduleDriver::new(
            service.clone(),
            store.clone(),
            probe.clone(),
            delivery.clone(),
            "driver-owner",
            ScheduleDriverConfig {
                claim_limit: 8,
                lease_duration: Duration::seconds(30),
            },
        );
        let executor_lease = test_executor_lease(&store).await;
        let claimed = store
            .claim_due_occurrences(
                &executor_lease,
                ClaimDueRequest {
                    owner_id: "driver-owner".into(),
                    limit: 1,
                    lease_duration: Duration::seconds(30),
                },
            )
            .await?;
        store.release_executor_lease(executor_lease).await?;
        let occurrence = claimed
            .transitions
            .into_iter()
            .next()
            .expect("claimed occurrence")
            .successor()
            .clone();

        let deleted = service.delete(&schedule.schedule_id).await?;

        // D1: revoked at commit, not on the driver's next decision point.
        let at_commit = service
            .list_occurrences(&schedule.schedule_id)
            .await?
            .into_iter()
            .find(|item| item.occurrence_id == occurrence.occurrence_id)
            .expect("occurrence should still exist");
        assert_eq!(
            at_commit.phase,
            OccurrencePhase::Superseded,
            "delete must supersede the driver-claimed occurrence at commit time"
        );
        assert_eq!(at_commit.superseded_by_revision, Some(deleted.revision));

        // The driver still holds the pre-delete claim snapshot; handling it
        // must stay benign (typed idempotent no-op): no probe, no delivery,
        // no error, no duplicate superseded receipt.
        driver
            .handle_claimed_occurrence(occurrence.clone(), claimed.store_now_utc, None)
            .await?;
        assert_eq!(*probe.calls.lock().await, 0, "delete should block probes");
        assert_eq!(
            *delivery.calls.lock().await,
            0,
            "delete should block delivery"
        );
        let receipts = store.list_receipts(&occurrence.occurrence_id).await?;
        assert_eq!(
            receipts
                .iter()
                .filter(|receipt| receipt.stage == DeliveryReceiptStage::Superseded)
                .count(),
            1,
            "the commit-time sweep mints the canonical superseded receipt; the driver's \
             idempotent reconcile path must not duplicate it"
        );
        Ok(())
    }

    /// STAGE B (RED until wired): a completion arrival whose claim evidence is
    /// stale (lease expired and the occurrence was reclaimed for attempt 2)
    /// must be recorded as the occurrence authority's typed
    /// ClassifyStaleCompletionArrival fact — never silently dropped via
    /// `Ok(false)`.
    #[tokio::test]
    async fn stale_completion_arrival_is_recorded_as_typed_machine_fact()
    -> Result<(), ScheduleDomainError> {
        let store = Arc::new(MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let service = ScheduleService::new(store.clone());
        let schedule = service
            .create(CreateScheduleRequest {
                name: Some("stale-arrival-typed".into()),
                description: None,
                trigger: TriggerSpec::Once {
                    due_at_utc: Utc::now() - Duration::seconds(1),
                },
                target: materialize_on_demand_target("scheduled prompt"),
                misfire_policy: MisfirePolicy::Skip,
                overlap_policy: OverlapPolicy::SkipIfRunning,
                missing_target_policy: MissingTargetPolicy::MarkMisfired,
                labels: BTreeMap::new(),
                planning_horizon_days: Some(1),
                planning_horizon_occurrences: Some(1),
            })
            .await?;
        let delivery = Arc::new(ControlledCompletionDelivery::default());
        let driver = ScheduleDriver::new(
            service.clone(),
            store.clone(),
            Arc::new(ReadyProbe),
            delivery.clone(),
            "driver-owner",
            ScheduleDriverConfig {
                claim_limit: 8,
                lease_duration: Duration::milliseconds(25),
            },
        );

        // Attempt 1 belongs to a "crashed" deliverer (no live waiter, no
        // renewal in this process); the driver reclaims it for attempt 2.
        let stale_attempt =
            claim_and_dispatch_without_waiter(&store, Duration::milliseconds(25)).await?;
        sleep(std::time::Duration::from_millis(35)).await;
        driver.tick_once().await?;
        wait_for_sender_count(&delivery, 1).await;
        wait_for_occurrence_attempt(&service, &schedule.schedule_id, 2).await?;

        // The zombie deliverer's completion arrives bearing attempt 1's
        // claim evidence.
        complete_dispatched_occurrence(
            store.clone(),
            stale_attempt,
            Ok(DeliveryTerminal::completed(None)),
        )
        .await?;

        let recorded = loop_until_stale_arrival_recorded(&service, &schedule.schedule_id).await?;
        assert_eq!(
            recorded.phase,
            OccurrencePhase::AwaitingCompletion,
            "the stale arrival must not disturb the reclaimed attempt"
        );
        assert_eq!(recorded.attempt_count, 2);
        assert_eq!(
            recorded.machine_state.stale_completion_arrivals, 1,
            "the screened stale arrival must be recorded as a typed machine fact"
        );
        Ok(())
    }

    /// GREEN pin (by-design semantics): pause does NOT supersede an in-flight
    /// dispatched delivery; its completion lands normally on the paused
    /// schedule's occurrence.
    #[tokio::test]
    async fn pause_does_not_supersede_in_flight_delivery_completion_lands_normally()
    -> Result<(), ScheduleDomainError> {
        let store = Arc::new(MemoryScheduleStore::new()) as Arc<dyn ScheduleStore>;
        let service = ScheduleService::new(store.clone());
        let schedule = service
            .create(CreateScheduleRequest {
                name: Some("pause-in-flight-completes".into()),
                description: None,
                trigger: TriggerSpec::Once {
                    due_at_utc: Utc::now() - Duration::seconds(1),
                },
                target: materialize_on_demand_target("scheduled prompt"),
                misfire_policy: MisfirePolicy::Skip,
                overlap_policy: OverlapPolicy::SkipIfRunning,
                missing_target_policy: MissingTargetPolicy::MarkMisfired,
                labels: BTreeMap::new(),
                planning_horizon_days: Some(1),
                planning_horizon_occurrences: Some(1),
            })
            .await?;
        let delivery = Arc::new(ControlledCompletionDelivery::default());
        let driver = ScheduleDriver::new(
            service.clone(),
            store.clone(),
            Arc::new(ReadyProbe),
            delivery.clone(),
            "driver-owner",
            ScheduleDriverConfig {
                claim_limit: 8,
                lease_duration: Duration::seconds(30),
            },
        );

        driver.tick_once().await?;
        wait_for_sender_count(&delivery, 1).await;
        let awaiting = wait_for_occurrence_phase(
            &service,
            &schedule.schedule_id,
            OccurrencePhase::AwaitingCompletion,
        )
        .await?;

        service.pause(&schedule.schedule_id).await?;
        let sender = delivery.senders.lock().await.remove(0);
        sender
            .send(DeliveryTerminal::completed(None))
            .expect("completion receiver should be open");

        let completed =
            wait_for_occurrence_phase(&service, &schedule.schedule_id, OccurrencePhase::Completed)
                .await?;
        assert_eq!(completed.occurrence_id, awaiting.occurrence_id);
        let receipts = store.list_receipts(&completed.occurrence_id).await?;
        assert_eq!(
            receipts.last().map(|receipt| receipt.stage),
            Some(DeliveryReceiptStage::Completed),
            "in-flight delivery under a paused schedule completes and records normally"
        );
        Ok(())
    }

    async fn wait_for_late_completion_record(
        service: &ScheduleService,
        schedule_id: &crate::ScheduleId,
    ) -> Result<Occurrence, ScheduleDomainError> {
        for _ in 0..50 {
            let occurrences = service.list_occurrences(schedule_id).await?;
            if let Some(occurrence) = occurrences.into_iter().find(|occurrence| {
                occurrence
                    .machine_state
                    .late_completion_resolution
                    .is_some()
            }) {
                return Ok(occurrence);
            }
            sleep(std::time::Duration::from_millis(10)).await;
        }
        Err(ScheduleDomainError::Internal(
            "timed out waiting for typed late-completion record".to_string(),
        ))
    }

    async fn loop_until_stale_arrival_recorded(
        service: &ScheduleService,
        schedule_id: &crate::ScheduleId,
    ) -> Result<Occurrence, ScheduleDomainError> {
        for _ in 0..50 {
            let occurrences = service.list_occurrences(schedule_id).await?;
            if let Some(occurrence) = occurrences
                .into_iter()
                .find(|occurrence| occurrence.machine_state.stale_completion_arrivals > 0)
            {
                return Ok(occurrence);
            }
            sleep(std::time::Duration::from_millis(10)).await;
        }
        Err(ScheduleDomainError::Internal(
            "timed out waiting for typed stale-completion-arrival record".to_string(),
        ))
    }

    fn materialize_on_demand_target(prompt: &str) -> TargetBinding {
        TargetBinding::session(SessionTargetBinding::materialize_on_demand(
            SessionMaterializationSpec {
                model: "claude-sonnet-4-6".into(),
                system_prompt: None,
                max_tokens: None,
                provider: None,
                output_schema: None,
                structured_output_retries: None,
                provider_params: None,
                comms_name: Some("scheduled-materializer".into()),
                peer_meta: None,
                labels: BTreeMap::new(),
                preload_skills: Vec::new(),
                additional_instructions: Vec::new(),
                realm_id: None,
                instance_id: None,
                backend: None,
                config_generation: None,
                keep_alive: true,
                app_context: None,
            },
            ScheduledSessionAction::Prompt {
                prompt: ContentInput::from(prompt),
                system_prompt: None,
                render_metadata: None,
                skill_refs: Vec::new(),
                additional_instructions: Vec::new(),
            },
        ))
    }

    #[test]
    fn materialize_on_demand_target_uses_current_fixture_model() {
        let target = materialize_on_demand_target("scheduled prompt");
        let spec = if let TargetBinding::Session(binding) = target {
            if let SessionTargetBinding::MaterializeOnDemandSession { create, .. } = *binding {
                create
            } else {
                return;
            }
        } else {
            return;
        };

        assert_eq!(spec.model, "claude-sonnet-4-6");
    }

    async fn wait_for_sender_count(delivery: &ControlledCompletionDelivery, expected: usize) {
        for _ in 0..50 {
            if delivery.senders.lock().await.len() >= expected {
                return;
            }
            sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("timed out waiting for {expected} delivery senders");
    }

    async fn wait_for_admission_count(delivery: &IdempotentDelivery, expected: usize) {
        for _ in 0..50 {
            if delivery.admissions.lock().await.len() >= expected {
                return;
            }
            sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("timed out waiting for {expected} delivery admissions");
    }

    async fn wait_for_occurrence_attempt(
        service: &ScheduleService,
        schedule_id: &crate::ScheduleId,
        attempt_count: u32,
    ) -> Result<Occurrence, ScheduleDomainError> {
        for _ in 0..50 {
            let occurrences = service.list_occurrences(schedule_id).await?;
            if let Some(occurrence) = occurrences
                .into_iter()
                .find(|occurrence| occurrence.attempt_count == attempt_count)
            {
                return Ok(occurrence);
            }
            sleep(std::time::Duration::from_millis(10)).await;
        }
        Err(ScheduleDomainError::Internal(format!(
            "timed out waiting for occurrence attempt {attempt_count}"
        )))
    }

    async fn wait_for_occurrence_phase(
        service: &ScheduleService,
        schedule_id: &crate::ScheduleId,
        expected_phase: OccurrencePhase,
    ) -> Result<Occurrence, ScheduleDomainError> {
        for _ in 0..50 {
            let occurrences = service.list_occurrences(schedule_id).await?;
            if let Some(occurrence) = occurrences
                .into_iter()
                .find(|occurrence| occurrence.phase == expected_phase)
            {
                return Ok(occurrence);
            }
            sleep(std::time::Duration::from_millis(10)).await;
        }
        Err(ScheduleDomainError::Internal(format!(
            "timed out waiting for occurrence phase {expected_phase:?}"
        )))
    }
}
