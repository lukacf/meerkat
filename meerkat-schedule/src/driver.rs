use crate::error::{ScheduleDomainError, ScheduleStoreError};
use crate::lifecycle::{
    ClaimedDispatchDisposition, CompletionSupersessionDisposition, OccurrenceLifecycleEffect,
    OccurrenceLifecycleInput, StaleCompletionArrivalTrigger,
};
use crate::service::ScheduleService;
use crate::store::{
    ClaimDueRequest, RenewOccurrenceLeaseOutcome, RenewOccurrenceLeaseRequest, ScheduleStore,
};
use crate::types::{
    DeliveryCompletionFailureReason, DeliveryFailureReason, DeliveryReceipt, Occurrence,
    OccurrenceId, OccurrencePhase, OccurrenceTargetProbeOutcome, RuntimeCompletionOutcome,
    RuntimeDeliveryOutcome,
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

#[async_trait]
pub trait ScheduleTargetDelivery: Send + Sync {
    async fn deliver_occurrence(
        &self,
        occurrence: &Occurrence,
        identity: &ScheduleDeliveryIdentity,
    ) -> Result<DeliveryDispatch, ScheduleDomainError>;
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
            .map(|fault| format!("schedule-row: {fault}"))
            .chain(
                self.occurrence_row_faults
                    .iter()
                    .map(|fault| format!("occurrence-row: {fault}")),
            )
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
        Self {
            service,
            store,
            probe,
            delivery,
            owner_id: owner_id.into(),
            config,
        }
    }

    pub async fn tick_once(&self) -> Result<ScheduleTickReport, ScheduleDomainError> {
        let mut report = ScheduleTickReport::default();

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
            .claim_due_occurrences(ClaimDueRequest {
                owner_id: self.owner_id.clone(),
                limit: self.config.claim_limit,
                lease_duration: self.config.lease_duration,
            })
            .await?;
        report.claimed_occurrences = claimed.claimed.len();
        report.occurrence_row_faults = claimed.row_faults;

        for occurrence in claimed.claimed {
            if self
                .handle_claimed_occurrence(occurrence, claimed.store_now_utc)
                .await?
            {
                report.terminalized_occurrences += 1;
            }
        }

        Ok(report)
    }

    async fn handle_claimed_occurrence(
        &self,
        occurrence: Occurrence,
        store_now_utc: chrono::DateTime<Utc>,
    ) -> Result<bool, ScheduleDomainError> {
        let frozen_occurrence = occurrence.clone();
        let occurrence = match self
            .reconcile_claimed_occurrence_before_dispatch(occurrence)
            .await?
        {
            ClaimedOccurrenceDispatchState::Ready(occurrence) => occurrence,
            ClaimedOccurrenceDispatchState::Frozen => {
                let _ = self
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
                    .await?;
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

        let occurrence = match self.resolve_target_probe(occurrence, store_now_utc).await? {
            TargetProbeResolution::Continue(occurrence) => *occurrence,
            TargetProbeResolution::Terminalized => return Ok(true),
            TargetProbeResolution::StaleClaim => return Ok(false),
        };

        let delivery_identity = ScheduleDeliveryIdentity::for_occurrence(&occurrence);
        // Persist durable dispatch intent and its stable external identity
        // before the adapter can perform any target-side effect. A crash:
        // - after this commit but before delivery leaves a retryable outbox row;
        // - after delivery but before observation replays the same idempotency
        //   key, never a newly minted effect identity.
        let Some(mut dispatching) = self
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

        let dispatch = match self
            .delivery
            .deliver_occurrence(&dispatching, &delivery_identity)
            .await
        {
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
                .sync_occurrence_target_with_schedule(dispatching)
                .await?;
        }

        validate_dispatch_receipt(&dispatching, &delivery_identity, &dispatch)?;
        if dispatch.receipt.stage == crate::DeliveryReceiptStage::DispatchAccepted {
            let Some(accepted) = self
                .store
                .transition_occurrence_with_receipt_if_current(
                    &dispatching.occurrence_id,
                    dispatching.attempt_count,
                    dispatching.claim_token(),
                    OccurrenceLifecycleInput::DispatchAccepted {
                        at_utc: store_now_utc,
                    },
                    dispatch.receipt.runtime_outcome.clone(),
                )
                .await?
            else {
                return Ok(false);
            };
            dispatching = accepted;
        }

        let refetched_id = dispatching.occurrence_id.clone();
        let await_mutator = dispatching
            .apply(OccurrenceLifecycleInput::AwaitCompletion {
                at_utc: store_now_utc,
            })
            .map_err(|error| ScheduleDomainError::Internal(error.to_string()))?;
        // Capture the post-await occurrence before consuming the mutator.
        let awaiting_occurrence = await_mutator.occurrence.clone();
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
            Ok(()) => awaiting_occurrence,
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

    async fn resolve_target_probe(
        &self,
        occurrence: Occurrence,
        store_now_utc: DateTime<Utc>,
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
            detail,
            at_utc: store_now_utc,
        };
        // Predict the terminal phase the generated authority will resolve the
        // probe to so we can route Claimed (no receipt) and Skipped/Misfired
        // (typed delivery receipt) through the correct store method.
        let predicted = occurrence
            .clone()
            .apply(lifecycle.clone())
            .map_err(|error| ScheduleDomainError::Internal(error.to_string()))?
            .into_occurrence();
        let updated = match predicted.phase {
            OccurrencePhase::Claimed => {
                // `transition_occurrence_if_current` now returns the emitted
                // effects alongside the occurrence; the target-probe Continue
                // path does not consume them.
                self.store
                    .transition_occurrence_if_current(
                        &occurrence.occurrence_id,
                        occurrence.attempt_count,
                        occurrence.claim_token(),
                        lifecycle,
                    )
                    .await?
                    .map(|(updated, _effects)| updated)
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
        let Some(updated) = updated else {
            return Ok(TargetProbeResolution::StaleClaim);
        };

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

fn validate_dispatch_receipt(
    occurrence: &Occurrence,
    identity: &ScheduleDeliveryIdentity,
    dispatch: &DeliveryDispatch,
) -> Result<(), ScheduleDomainError> {
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
            Some(
                RuntimeDeliveryOutcome::AdmissionAccepted
                | RuntimeDeliveryOutcome::AdmissionDeduplicated,
            ),
        )
        | (crate::DeliveryReceiptStage::DispatchStarted, None) => Ok(()),
        (stage, outcome) => Err(ScheduleDomainError::Internal(format!(
            "delivery adapter returned invalid admission receipt stage {stage:?} with outcome {outcome:?}"
        ))),
    }
}

/// Drive the delivery's completion future while renewing the occurrence's
/// lease at ~half the lease duration (2026-07 P0: the fixed 60s lease was
/// never renewed, so every delivery longer than the lease — a routine
/// session turn — was reclaimed mid-flight as a presumed-dead deliverer).
///
/// Renewal goes through one store-atomic seam: the store samples its clock,
/// screens the exact attempt/token witness, applies the machine's
/// `RenewLease`, and commits in one critical section/transaction. Transient
/// mechanism failures retry with bounded backoff inside the last proven lease
/// budget; stale evidence or a terminal store failure stops renewal and lets
/// eventual completion take the stale-screening path.
async fn run_completion_with_lease_renewal(
    store: &Arc<dyn ScheduleStore>,
    occurrence: &Occurrence,
    lease_duration: Duration,
    mut completion: DeliveryCompletion,
) -> Result<DeliveryTerminal, ScheduleDomainError> {
    let Some(interval) = lease_renewal_interval(lease_duration) else {
        return completion.await;
    };
    let Some(claim_token) = occurrence.claim_token() else {
        // No claim token on a dispatched occurrence is unreachable through
        // the claim path; without one the machine cannot authorize renewal.
        return completion.await;
    };
    let Some(expected_owner_id) = occurrence.claimed_by.as_deref() else {
        tracing::error!(
            occurrence_id = %occurrence.occurrence_id,
            attempt = occurrence.attempt_count,
            "dispatched occurrence omitted durable claim owner; lease cannot be renewed"
        );
        return completion.await;
    };
    let mut lease_deadline = monotonic_lease_deadline(Utc::now(), occurrence.lease_expires_at_utc)
        .unwrap_or_else(std::time::Instant::now);
    let mut renewal_delay = interval;
    let mut transient_backoff = std::time::Duration::from_millis(50);
    loop {
        let tick = Box::pin(crate::tokio::time::sleep(renewal_delay));
        match future::select(completion, tick).await {
            Either::Left((result, _tick)) => return result,
            Either::Right(((), waiting)) => {
                completion = waiting;
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
                        occurrence,
                    } => {
                        let Some(next_deadline) = monotonic_lease_deadline(
                            store_now_utc,
                            occurrence.lease_expires_at_utc,
                        ) else {
                            tracing::error!(
                                occurrence_id = %occurrence.occurrence_id,
                                attempt = occurrence.attempt_count,
                                "lease renewal committed without a durable expiry"
                            );
                            return completion.await;
                        };
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
                        return completion.await;
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
                            return completion.await;
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
                        return completion.await;
                    }
                }
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
            RenewOccurrenceLeaseOutcome::Renewed(occurrence) => LeaseRenewalAttempt::Renewed {
                store_now_utc: result.store_now_utc,
                occurrence,
            },
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
        if let Some(effect) = mutator.effects.iter().find(|e| {
            matches!(
                e,
                OccurrenceLifecycleEffect::StaleCompletionArrivalClassified { .. }
            )
        }) {
            tracing::debug!(
                occurrence_id = %occurrence_id,
                ?trigger,
                ?effect,
                "stale completion arrival classified as typed machine fact"
            );
        }
        store
            .commit_occurrence_write(mutator.into_authorized_write())
            .await?;
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
            let Some((_updated, effects)) = store
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
            let late_recorded = effects.iter().any(|e| {
                matches!(
                    e,
                    OccurrenceLifecycleEffect::LateCompletionResolutionRecorded { .. }
                )
            });
            if late_recorded {
                Ok(TerminalizeOutcome::LateRecorded)
            } else if effects.is_empty() {
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
            // the claim-screened store seam, never a separate `append_receipt`,
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
                Some(_) => Ok(TerminalizeOutcome::Applied),
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
    let late_recorded = mutator.effects.iter().any(|e| {
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
    store
        .commit_occurrence_write(mutator.into_authorized_write())
        .await?;
    Ok(TerminalizeOutcome::LateRecorded)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::large_futures, clippy::panic)]

    use super::*;
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
            receipt.runtime_outcome = Some(RuntimeDeliveryOutcome::AdmissionDeduplicated);
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
                completion: Box::pin(async { Ok(DeliveryTerminal::completed(None)) }),
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
        ) -> Result<(), ScheduleStoreError> {
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
        ) -> Result<(), ScheduleStoreError> {
            self.inner.commit_occurrence_write(write).await
        }

        async fn commit_schedule_mutation(
            &self,
            schedule: crate::AuthorizedScheduleWrite,
            occurrences: Vec<crate::AuthorizedOccurrenceWrite>,
        ) -> Result<crate::Schedule, ScheduleStoreError> {
            self.inner
                .commit_schedule_mutation(schedule, occurrences)
                .await
        }

        async fn commit_schedule_refill(
            &self,
            schedule: crate::AuthorizedScheduleWrite,
            occurrences: Vec<crate::AuthorizedOccurrenceWrite>,
            next_refill_at_utc: Option<DateTime<Utc>>,
        ) -> Result<crate::Schedule, ScheduleStoreError> {
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

        async fn append_receipt(&self, receipt: DeliveryReceipt) -> Result<(), ScheduleStoreError> {
            self.inner.append_receipt(receipt).await
        }

        async fn list_receipts(
            &self,
            occurrence_id: &crate::OccurrenceId,
        ) -> Result<Vec<DeliveryReceipt>, ScheduleStoreError> {
            self.inner.list_receipts(occurrence_id).await
        }

        async fn claim_due_occurrences(
            &self,
            request: ClaimDueRequest,
        ) -> Result<crate::ClaimDueResult, ScheduleStoreError> {
            let mut result = self.inner.claim_due_occurrences(request).await?;
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
        ) -> Result<Option<(Occurrence, Vec<OccurrenceLifecycleEffect>)>, ScheduleStoreError>
        {
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
        ) -> Result<Option<Occurrence>, ScheduleStoreError> {
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
        ) -> Result<(), ScheduleStoreError> {
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
        ) -> Result<(), ScheduleStoreError> {
            self.inner.commit_occurrence_write(write).await
        }

        async fn commit_occurrence_writes(
            &self,
            writes: Vec<crate::AuthorizedOccurrenceWrite>,
        ) -> Result<(), ScheduleStoreError> {
            self.inner.commit_occurrence_writes(writes).await
        }

        async fn commit_schedule_mutation(
            &self,
            schedule: crate::AuthorizedScheduleWrite,
            occurrences: Vec<crate::AuthorizedOccurrenceWrite>,
        ) -> Result<crate::Schedule, ScheduleStoreError> {
            self.inner
                .commit_schedule_mutation(schedule, occurrences)
                .await
        }

        async fn commit_schedule_refill(
            &self,
            schedule: crate::AuthorizedScheduleWrite,
            occurrences: Vec<crate::AuthorizedOccurrenceWrite>,
            next_refill_at_utc: Option<DateTime<Utc>>,
        ) -> Result<crate::Schedule, ScheduleStoreError> {
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

        async fn append_receipt(
            &self,
            _receipt: DeliveryReceipt,
        ) -> Result<(), ScheduleStoreError> {
            Err(ScheduleStoreError::Internal(
                "standalone receipt append disabled for regression".into(),
            ))
        }

        async fn list_receipts(
            &self,
            occurrence_id: &crate::OccurrenceId,
        ) -> Result<Vec<DeliveryReceipt>, ScheduleStoreError> {
            self.inner.list_receipts(occurrence_id).await
        }

        async fn claim_due_occurrences(
            &self,
            request: ClaimDueRequest,
        ) -> Result<crate::ClaimDueResult, ScheduleStoreError> {
            self.inner.claim_due_occurrences(request).await
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
        ) -> Result<Option<(Occurrence, Vec<OccurrenceLifecycleEffect>)>, ScheduleStoreError>
        {
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
        ) -> Result<Option<Occurrence>, ScheduleStoreError> {
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

    /// Ask 22 regression (HomeCore runaway): a one-shot whose occurrence
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
             claim-screened store seam even when standalone append_receipt is unavailable"
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
        let receipts = store.list_receipts(&intent.occurrence_id).await?;
        assert!(receipts.iter().any(|receipt| {
            receipt.stage == DeliveryReceiptStage::DispatchAccepted
                && receipt.runtime_outcome == Some(RuntimeDeliveryOutcome::AdmissionDeduplicated)
        }));
        let recovered = wait_for_occurrence_attempt(&service, &schedule.schedule_id, 2).await?;
        assert_eq!(recovered.occurrence_id, intent.occurrence_id);
        Ok(())
    }

    /// Commit only the pre-effect outbox intent, with no adapter call or live
    /// completion waiter. This is the exact durable crash boundary between
    /// intent and target effect.
    async fn claim_and_start_dispatch_without_waiter(
        store: &Arc<dyn ScheduleStore>,
        lease_duration: Duration,
    ) -> Result<Occurrence, ScheduleDomainError> {
        let claimed = store
            .claim_due_occurrences(ClaimDueRequest {
                owner_id: "other-process".into(),
                limit: 1,
                lease_duration,
            })
            .await?;
        let occurrence = claimed
            .claimed
            .into_iter()
            .next()
            .ok_or_else(|| ScheduleDomainError::Internal("expected a claim".to_string()))?;
        let identity = ScheduleDeliveryIdentity::for_occurrence(&occurrence);
        store
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
            })
    }

    /// Claim an occurrence through the store and drive it to
    /// AwaitingCompletion through the occurrence authority without spawning
    /// a waiter task — the durable footprint of a deliverer that crashed (or
    /// lives in another process).
    async fn claim_and_dispatch_without_waiter(
        store: &Arc<dyn ScheduleStore>,
        lease_duration: Duration,
    ) -> Result<Occurrence, ScheduleDomainError> {
        let claimed = store
            .claim_due_occurrences(ClaimDueRequest {
                owner_id: "other-process".into(),
                limit: 1,
                lease_duration,
            })
            .await?;
        let occurrence = claimed
            .claimed
            .into_iter()
            .next()
            .ok_or_else(|| ScheduleDomainError::Internal("expected a claim".to_string()))?;
        let dispatch_mutator = occurrence
            .apply(OccurrenceLifecycleInput::DispatchStarted {
                correlation_id: Some("other-process-dispatch".into()),
                at_utc: claimed.store_now_utc,
            })
            .map_err(|error| ScheduleDomainError::Internal(error.to_string()))?;
        let dispatching = dispatch_mutator.occurrence.clone();
        store
            .commit_occurrence_write(dispatch_mutator.into_authorized_write())
            .await?;
        let await_mutator = dispatching
            .apply(OccurrenceLifecycleInput::AwaitCompletion {
                at_utc: claimed.store_now_utc,
            })
            .map_err(|error| ScheduleDomainError::Internal(error.to_string()))?;
        let awaiting = await_mutator.occurrence.clone();
        store
            .commit_occurrence_write(await_mutator.into_authorized_write())
            .await?;
        Ok(awaiting)
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
        let claimed = store
            .claim_due_occurrences(ClaimDueRequest {
                owner_id: "driver-owner".into(),
                limit: 1,
                lease_duration: Duration::seconds(30),
            })
            .await?;
        let occurrence = claimed
            .claimed
            .into_iter()
            .next()
            .expect("claimed occurrence");
        service.pause(&schedule.schedule_id).await?;

        let terminalized = driver
            .handle_claimed_occurrence(occurrence.clone(), claimed.store_now_utc)
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
        let claimed = store
            .claim_due_occurrences(ClaimDueRequest {
                owner_id: "driver-owner".into(),
                limit: 1,
                lease_duration: Duration::seconds(30),
            })
            .await?;
        let occurrence = claimed
            .claimed
            .into_iter()
            .next()
            .expect("claimed occurrence");
        service.delete(&schedule.schedule_id).await?;

        let terminalized = driver
            .handle_claimed_occurrence(occurrence.clone(), claimed.store_now_utc)
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
        let claimed = store
            .claim_due_occurrences(ClaimDueRequest {
                owner_id: "driver-owner".into(),
                limit: 1,
                lease_duration: Duration::seconds(30),
            })
            .await?;
        let occurrence = claimed
            .claimed
            .into_iter()
            .next()
            .expect("claimed occurrence");
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
            .handle_claimed_occurrence(occurrence.clone(), claimed.store_now_utc)
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
        let claimed = store
            .claim_due_occurrences(ClaimDueRequest {
                owner_id: "driver-owner".into(),
                limit: 1,
                lease_duration: Duration::seconds(30),
            })
            .await?;
        let occurrence = claimed
            .claimed
            .clone()
            .into_iter()
            .next()
            .expect("claimed occurrence");

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
            .handle_claimed_occurrence(occurrence.clone(), claimed.store_now_utc)
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
