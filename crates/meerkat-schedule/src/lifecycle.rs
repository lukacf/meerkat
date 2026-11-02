//! Domain-facing lifecycle inputs, effects, errors, and mutators for schedules
//! and occurrences. The `apply()` methods live on `Schedule` / `Occurrence` in
//! `types.rs` and delegate to the DSL kernels in `machines::*`. This module
//! houses the pure data types consumed by callers and by the `ScheduleStore`
//! trait wire contract.

use crate::machines::occurrence_lifecycle as occ_dsl;
use crate::machines::schedule_lifecycle as sched_dsl;
use crate::store::PendingSupersession;
use crate::types::{
    CreateScheduleRequest, DeliveryAdmissionOutcome, DeliveryCompletionFailureReason,
    DeliveryFailureReason, DeliveryReceipt, DeliveryReceiptStage, Occurrence,
    OccurrenceFailureClass, OccurrenceId, OccurrenceOrdinal, OccurrencePhase,
    OccurrenceTargetProbeOutcome, RuntimeCompletionOutcome, RuntimeDeliveryOutcome, Schedule,
    ScheduleId, SchedulePhase, ScheduleRevision, TargetBinding, TriggerSpec, UpdateScheduleRequest,
    delivery_receipt_id_from_authority, target_materialized_session_id,
    validate_occurrence_machine_projection, validate_schedule_machine_projection,
};
use chrono::{DateTime, Utc};
use meerkat_core::SessionId;
use serde::Serialize;
use std::collections::BTreeSet;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Schedule lifecycle — public types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum ScheduleLifecycleInput {
    Create(CreateScheduleRequest),
    Update {
        request: UpdateScheduleRequest,
        at_utc: DateTime<Utc>,
    },
    RecordPlanningWindow {
        planning_cursor_utc: DateTime<Utc>,
        next_occurrence_ordinal: OccurrenceOrdinal,
    },
    SyncTargetSnapshot {
        target: TargetBinding,
    },
    Pause {
        at_utc: DateTime<Utc>,
    },
    Resume {
        at_utc: DateTime<Utc>,
    },
    Delete {
        at_utc: DateTime<Utc>,
    },
    ConfirmOccurrencesSuperseded {
        ack: OccurrenceSupersessionAck,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScheduleLifecycleEffect {
    EmitScheduleNotice {
        new_state: SchedulePhase,
        revision: ScheduleRevision,
    },
    SupersedePendingOccurrences {
        superseding_revision: ScheduleRevision,
        at_utc: DateTime<Utc>,
    },
    PlanningWindowRecorded {
        planning_cursor_utc: DateTime<Utc>,
        next_occurrence_ordinal: OccurrenceOrdinal,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OccurrenceSupersessionAck {
    schedule_id: ScheduleId,
    occurrence_id: OccurrenceId,
    superseding_revision: ScheduleRevision,
}

impl OccurrenceSupersessionAck {
    pub fn schedule_id(&self) -> &ScheduleId {
        &self.schedule_id
    }

    pub fn occurrence_id(&self) -> &OccurrenceId {
        &self.occurrence_id
    }

    pub fn superseding_revision(&self) -> ScheduleRevision {
        self.superseding_revision
    }

    fn from_occurrence_authority(
        occurrence: &Occurrence,
        effect: &OccurrenceLifecycleEffect,
    ) -> Option<Self> {
        let OccurrenceLifecycleEffect::OccurrencesSuperseded {
            occurrence_id,
            superseding_revision,
        } = effect
        else {
            return None;
        };
        if occurrence.phase != OccurrencePhase::Superseded
            || occurrence.superseded_by_revision != Some(*superseding_revision)
            || occurrence.occurrence_id != *occurrence_id
        {
            return None;
        }
        Some(Self {
            schedule_id: occurrence.schedule_id.clone(),
            occurrence_id: occurrence_id.clone(),
            superseding_revision: *superseding_revision,
        })
    }
}

#[derive(Debug, Clone)]
pub struct ScheduleLifecycleMutator {
    pub(crate) schedule: Schedule,
    pub(crate) effects: Vec<ScheduleLifecycleEffect>,
    pub(crate) revision_bumped: bool,
    write_precondition: ScheduleWritePrecondition,
    pending_supersession: Option<PendingSupersession>,
}

impl ScheduleLifecycleMutator {
    #[cfg(test)]
    fn into_schedule(self) -> Schedule {
        self.schedule
    }

    pub fn schedule(&self) -> &Schedule {
        &self.schedule
    }

    /// Record the agent whose mandate commissions the schedule this create
    /// or update writes. The agent is not lifecycle state: like the name and
    /// labels it is a config fact of the write, so it is set on the mutated
    /// schedule before planning reads it.
    pub(crate) fn record_commissioning_agent(
        &mut self,
        commissioning_agent: Option<meerkat_core::PrincipalRef>,
    ) {
        self.schedule.config.commissioning_agent = commissioning_agent;
    }

    pub fn effects(&self) -> &[ScheduleLifecycleEffect] {
        &self.effects
    }

    pub fn revision_bumped(&self) -> bool {
        self.revision_bumped
    }

    pub fn into_effectless_schedule(self) -> Result<Schedule, ScheduleLifecycleError> {
        if !self.effects.is_empty() || self.revision_bumped || self.pending_supersession.is_some() {
            return Err(ScheduleLifecycleError::ProjectionMismatch {
                reason: format!(
                    "effectless schedule projection produced {} lifecycle effects, revision_bumped={}, pending_supersession={}",
                    self.effects.len(),
                    self.revision_bumped,
                    self.pending_supersession.is_some()
                ),
            });
        }
        Ok(self.schedule)
    }

    pub fn absorb_followup(
        &mut self,
        followup: ScheduleLifecycleMutator,
    ) -> Result<(), ScheduleLifecycleError> {
        let ScheduleLifecycleMutator {
            schedule,
            effects,
            revision_bumped,
            write_precondition,
            pending_supersession,
        } = followup;
        write_precondition
            .check_current(Some(&self.schedule))
            .map_err(|reason| ScheduleLifecycleError::ProjectionMismatch { reason })?;
        if pending_supersession.is_some() && self.pending_supersession.is_some() {
            return Err(ScheduleLifecycleError::ProjectionMismatch {
                reason: "multiple generated supersession handoffs for one schedule write".into(),
            });
        }
        self.schedule = schedule;
        self.effects.extend(effects);
        self.revision_bumped |= revision_bumped;
        if pending_supersession.is_some() {
            self.pending_supersession = pending_supersession;
        }
        Ok(())
    }

    pub fn into_authorized_write(self) -> AuthorizedScheduleWrite {
        AuthorizedScheduleWrite {
            schedule: self.schedule,
            effects: self.effects,
            revision_bumped: self.revision_bumped,
            precondition: self.write_precondition,
            pending_supersession: self.pending_supersession,
        }
    }
}

#[derive(Debug, Clone)]
pub struct AuthorizedScheduleWrite {
    schedule: Schedule,
    effects: Vec<ScheduleLifecycleEffect>,
    revision_bumped: bool,
    precondition: ScheduleWritePrecondition,
    pending_supersession: Option<PendingSupersession>,
}

impl AuthorizedScheduleWrite {
    pub(crate) fn into_uncommitted_schedule_for_construction(self) -> Schedule {
        self.schedule
    }

    pub fn schedule(&self) -> &Schedule {
        &self.schedule
    }

    pub fn schedule_id(&self) -> &ScheduleId {
        &self.schedule.schedule_id
    }

    pub fn precondition(&self) -> &ScheduleWritePrecondition {
        &self.precondition
    }

    pub fn has_pending_supersession(&self) -> bool {
        self.pending_supersession.is_some()
    }

    #[doc(hidden)]
    pub fn pending_supersession(&self) -> Option<&PendingSupersession> {
        self.pending_supersession.as_ref()
    }

    pub fn effects(&self) -> &[ScheduleLifecycleEffect] {
        &self.effects
    }

    pub fn revision_bumped(&self) -> bool {
        self.revision_bumped
    }

    #[doc(hidden)]
    /// Trusted-backend assertion used by open `ScheduleStore`
    /// implementations after their transaction commits. Rust visibility
    /// cannot prove that provenance across third-party store crates; invoking
    /// this outside the successful store method violates the backend contract.
    pub fn into_transition_commit_after_store_commit(
        self,
        successor: Schedule,
    ) -> ScheduleTransitionCommit {
        ScheduleTransitionCommit {
            prior: self.precondition,
            successor,
            effects: self.effects,
            revision_bumped: self.revision_bumped,
        }
    }
}

/// Trusted-store assertion for one committed schedule lifecycle transition.
///
/// This is not cryptographic or capability-token proof. Its provenance is the
/// `ScheduleStore` implementation contract, and consumers must accept it only
/// as the direct result of the store method they invoked.
#[must_use = "a schedule transition commit carries lifecycle effects and revision authority that must be observed"]
#[derive(Debug)]
pub struct ScheduleTransitionCommit {
    prior: ScheduleWritePrecondition,
    successor: Schedule,
    effects: Vec<ScheduleLifecycleEffect>,
    revision_bumped: bool,
}

impl ScheduleTransitionCommit {
    pub fn prior(&self) -> &ScheduleWritePrecondition {
        &self.prior
    }

    pub fn successor(&self) -> &Schedule {
        &self.successor
    }

    pub fn effects(&self) -> &[ScheduleLifecycleEffect] {
        &self.effects
    }

    pub fn revision_bumped(&self) -> bool {
        self.revision_bumped
    }

    pub fn into_parts(
        self,
    ) -> (
        ScheduleWritePrecondition,
        Schedule,
        Vec<ScheduleLifecycleEffect>,
        bool,
    ) {
        (
            self.prior,
            self.successor,
            self.effects,
            self.revision_bumped,
        )
    }
}

#[derive(Debug, Clone)]
pub struct ScheduleWritePrecondition {
    kind: ScheduleWritePreconditionKind,
}

#[derive(Debug, Clone)]
enum ScheduleWritePreconditionKind {
    Absent {
        schedule_id: ScheduleId,
    },
    Matches {
        schedule_id: ScheduleId,
        fingerprint: Vec<u8>,
    },
}

impl ScheduleWritePrecondition {
    fn absent(schedule_id: ScheduleId) -> Self {
        Self {
            kind: ScheduleWritePreconditionKind::Absent { schedule_id },
        }
    }

    fn matches(schedule: &Schedule) -> Result<Self, ScheduleLifecycleError> {
        Ok(Self {
            kind: ScheduleWritePreconditionKind::Matches {
                schedule_id: schedule.schedule_id.clone(),
                fingerprint: schedule_write_fingerprint(schedule)
                    .map_err(|reason| ScheduleLifecycleError::ProjectionMismatch { reason })?,
            },
        })
    }

    pub fn schedule_id(&self) -> &ScheduleId {
        match &self.kind {
            ScheduleWritePreconditionKind::Absent { schedule_id }
            | ScheduleWritePreconditionKind::Matches { schedule_id, .. } => schedule_id,
        }
    }

    pub fn check_current(&self, current: Option<&Schedule>) -> Result<(), String> {
        match &self.kind {
            ScheduleWritePreconditionKind::Absent { schedule_id } => {
                if current.is_some() {
                    return Err(format!(
                        "generated schedule write expected absent durable schedule {schedule_id}"
                    ));
                }
                Ok(())
            }
            ScheduleWritePreconditionKind::Matches {
                schedule_id,
                fingerprint,
            } => {
                let current = current.ok_or_else(|| {
                    format!("generated schedule write expected durable schedule {schedule_id}")
                })?;
                if &current.schedule_id != schedule_id {
                    return Err(format!(
                        "generated schedule write precondition id `{schedule_id}` did not match durable schedule `{}`",
                        current.schedule_id
                    ));
                }
                let current_fingerprint = schedule_write_fingerprint(current)?;
                if current_fingerprint != *fingerprint {
                    return Err(format!(
                        "generated schedule write precondition for {schedule_id} did not match durable machine authority state"
                    ));
                }
                Ok(())
            }
        }
    }
}

fn schedule_lifecycle_mutator(
    schedule: Schedule,
    effects: Vec<ScheduleLifecycleEffect>,
    revision_bumped: bool,
    write_precondition: ScheduleWritePrecondition,
) -> ScheduleLifecycleMutator {
    let pending_supersession = effects
        .iter()
        .find_map(PendingSupersession::from_schedule_effect);
    ScheduleLifecycleMutator {
        schedule,
        effects,
        revision_bumped,
        write_precondition,
        pending_supersession,
    }
}

#[derive(Debug, thiserror::Error)]
#[error("{fact} semantic key serialization failed: {detail}")]
pub struct SemanticKeySerializationError {
    fact: &'static str,
    detail: String,
}

#[derive(Debug, thiserror::Error)]
pub enum ScheduleLifecycleError {
    #[error("schedule is required for this lifecycle transition")]
    MissingSchedule,
    #[error("schedule is deleted")]
    Deleted,
    #[error("schedule revision mismatch: expected {expected}, found {actual}")]
    RevisionMismatch { expected: u64, actual: u64 },
    #[error(
        "ScheduleLifecycleMachine target key `{machine_key}` did not match target snapshot key `{snapshot_key}`"
    )]
    TargetBindingKeyMismatch {
        machine_key: String,
        snapshot_key: String,
    },
    #[error("ScheduleLifecycleMachine emitted invalid schedule id `{id}`: {source}")]
    InvalidScheduleId { id: String, source: uuid::Error },
    #[error("ScheduleLifecycleMachine emitted invalid planning horizon days `{value}`")]
    InvalidPlanningHorizonDays { value: u64 },
    #[error("ScheduleLifecycleMachine emitted invalid planning horizon occurrences `{value}`")]
    InvalidPlanningHorizonOccurrences { value: u64 },
    #[error("ScheduleLifecycleMachine emitted invalid planning cursor timestamp millis `{millis}`")]
    InvalidPlanningCursorUtcMillis { millis: u64 },
    #[error("ScheduleLifecycleMachine emitted invalid supersession timestamp millis `{millis}`")]
    InvalidSupersessionAtUtcMillis { millis: u64 },
    #[error("ScheduleLifecycleMachine emitted invalid superseded occurrence id `{id}`: {source}")]
    InvalidSupersededAckId { id: String, source: uuid::Error },
    #[error("schedule timestamp `{field}` cannot be represented as unsigned millis: {millis}")]
    InvalidTimestampMillis { field: &'static str, millis: i64 },
    #[error(transparent)]
    SemanticKeySerialization(#[from] SemanticKeySerializationError),
    #[error("ScheduleLifecycleMachine rejected transition: {source}")]
    TransitionRejected {
        source: sched_dsl::ScheduleLifecycleMachineTransitionError,
    },
    #[error("schedule projection does not match generated machine_state: {reason}")]
    ProjectionMismatch { reason: String },
}

// ---------------------------------------------------------------------------
// Occurrence lifecycle — public types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum OccurrenceLifecycleInput {
    PlanOccurrence {
        occurrence_id: crate::types::OccurrenceId,
        schedule_id: ScheduleId,
        schedule_revision: ScheduleRevision,
        occurrence_ordinal: OccurrenceOrdinal,
        trigger_snapshot: TriggerSpec,
        target_snapshot: TargetBinding,
        misfire_policy: crate::types::MisfirePolicy,
        overlap_policy: crate::types::OverlapPolicy,
        missing_target_policy: crate::types::MissingTargetPolicy,
        due_at_utc: DateTime<Utc>,
    },
    SyncTargetSnapshot {
        target_snapshot: TargetBinding,
    },
    RecordReceipt {
        receipt: DeliveryReceipt,
        runtime_outcome: Option<RuntimeDeliveryOutcome>,
    },
    /// `admission_evidence` is what the occurrence's receipt history proves
    /// about its earlier attempts' target admission
    /// ([`AdmissionEvidence::from_receipt_stages`]).
    ClassifyDue {
        now_utc: DateTime<Utc>,
        admission_evidence: AdmissionEvidence,
    },
    ClassifyOccurrenceTerminality,
    ClassifyClaimedDispatchDisposition {
        schedule_phase: SchedulePhase,
        current_schedule_revision: ScheduleRevision,
    },
    ClassifyCompletionSupersession {
        schedule_phase: SchedulePhase,
        current_schedule_revision: ScheduleRevision,
    },
    Claim {
        owner_id: String,
        at_utc: DateTime<Utc>,
        lease_expires_at_utc: DateTime<Utc>,
        claim_token: Uuid,
        admission_evidence: AdmissionEvidence,
    },
    DispatchStarted {
        correlation_id: Option<String>,
        at_utc: DateTime<Utc>,
    },
    /// Target admission succeeded after the stable dispatch identity was
    /// durably committed. The correlation id is intentionally absent: the
    /// occurrence authority retains the pre-effect identity and refuses any
    /// shell-side rewrite.
    DispatchAccepted {
        admission_outcome: DeliveryAdmissionOutcome,
        at_utc: DateTime<Utc>,
    },
    AwaitCompletion {
        at_utc: DateTime<Utc>,
    },
    Complete {
        at_utc: DateTime<Utc>,
    },
    ResolveRuntimeCompletion {
        outcome: RuntimeCompletionOutcome,
        detail: Option<String>,
        at_utc: DateTime<Utc>,
    },
    ResolveDeliveryCompletionFailure {
        reason: DeliveryCompletionFailureReason,
        detail: Option<String>,
        at_utc: DateTime<Utc>,
    },
    ResolveDeliveryFailure {
        reason: DeliveryFailureReason,
        detail: Option<String>,
        at_utc: DateTime<Utc>,
    },
    /// `redelivery` is the redelivery the claim named in its effects
    /// ([`OccurrenceLifecycleEffect::ReclaimedAfterAdmission`]: `Admitted`,
    /// [`OccurrenceLifecycleEffect::ReclaimedAfterUnprovenDispatch`]:
    /// `UnprovenLate`), `None` for an ordinary claim. A busy target ends an
    /// ordinary claim under `SkipIfRunning`, never a redelivery: it starts
    /// no new run.
    ResolveTargetProbe {
        outcome: OccurrenceTargetProbeOutcome,
        redelivery: Option<crate::driver::ExistingDelivery>,
        detail: Option<String>,
        at_utc: DateTime<Utc>,
    },
    ResolveDueMisfire {
        detail: Option<String>,
        at_utc: DateTime<Utc>,
    },
    /// A Pending occurrence past its misfire deadline whose earlier attempts
    /// recorded their dispatch used up its
    /// [`ADMITTED_RECLAIM_ATTEMPT_CAP`] attempts: a recorded admission ends
    /// delivery_failed with lease_lost, a recorded dispatch start alone ends
    /// misfired. A row without such evidence refuses it.
    ResolveReclaimExhausted {
        admission_evidence: AdmissionEvidence,
        detail: Option<String>,
        at_utc: DateTime<Utc>,
    },
    /// A reclaim past the misfire deadline of an occurrence whose earlier
    /// attempt recorded only its dispatch start found that the target never
    /// admitted the delivery: the occurrence ends misfired. An admitted
    /// occurrence, or one inside its window, refuses it.
    ResolveUnprovenLateNotAdmitted {
        admission_evidence: AdmissionEvidence,
        detail: Option<String>,
        at_utc: DateTime<Utc>,
    },
    Supersede {
        superseded_by_revision: ScheduleRevision,
        at_utc: DateTime<Utc>,
    },
    /// Lease renewal presented by the live claim-token holder while its
    /// delivery is in flight (2026-07 P0: a fixed lease with no renewal
    /// reclaimed every delivery longer than the lease mid-flight). The
    /// occurrence authority accepts it only from Dispatching /
    /// AwaitingCompletion, only when `claim_token` matches the machine-owned
    /// token, and only as a monotonic extension.
    RenewLease {
        claim_token: Uuid,
        lease_expires_at_utc: DateTime<Utc>,
        at_utc: DateTime<Utc>,
    },
    LeaseExpired {
        at_utc: DateTime<Utc>,
    },
    ReleaseLeaseForPausedSchedule {
        at_utc: DateTime<Utc>,
    },
    /// 0.7.2 D2a: typed classification of a completion-shaped arrival whose
    /// claim evidence (attempt count / claim token) is no longer current. The
    /// store shell screens such arrivals before they can mutate lifecycle
    /// state; instead of dropping the screened arrival silently, the shell
    /// feeds the screen verdict back through this input so the occurrence
    /// authority records the abandonment as a machine fact.
    ClassifyStaleCompletionArrival {
        trigger: StaleCompletionArrivalTrigger,
    },
}

/// Completion-shaped input variants a stale (claim-evidence-mismatched)
/// arrival can carry (0.7.2 D2a). Domain mirror of the occurrence authority's
/// completion-shaped `OccurrenceLifecycleInputVariant` subset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StaleCompletionArrivalTrigger {
    Complete,
    ResolveRuntimeCompletion,
    ResolveDeliveryCompletionFailure,
    ResolveDeliveryFailure,
    Supersede,
}

/// Typed class of a delivery resolution that arrived after the occurrence was
/// superseded (0.7.2 D2a). Domain mirror of the occurrence authority's
/// `LateCompletionResolutionClass`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LateCompletionResolutionClass {
    DeliveryCompleted,
    RuntimeCompleted,
    RuntimeRejected,
    RuntimeTransportError,
    RuntimeInternalError,
    DeliveryCompletionFailed,
    DeliveryFailed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OccurrenceLifecycleEffect {
    Claimed,
    DispatchStarted,
    DispatchAccepted,
    AwaitingCompletion,
    Completed,
    Skipped,
    Misfired,
    Superseded,
    OccurrencesSuperseded {
        occurrence_id: crate::types::OccurrenceId,
        superseding_revision: ScheduleRevision,
    },
    DueNoAction,
    DueClaimEligible,
    DueMisfireRequired,
    DueLeaseExpired,
    /// A Pending occurrence past its misfire deadline whose recorded
    /// dispatch used up its reclaims.
    DueReclaimExhausted,
    /// The claim reclaimed an occurrence whose earlier attempt recorded its
    /// admission: redeliver it without a fresh admission.
    ReclaimedAfterAdmission,
    /// The claim reclaimed, past its misfire deadline, an occurrence whose
    /// earlier attempt recorded only its dispatch start: ask the target
    /// whether it admitted the delivery, and never deliver it fresh.
    ReclaimedAfterUnprovenDispatch,
    OccurrenceTerminalityClassified {
        terminal: bool,
    },
    ClaimedDispatchDispositionClassified {
        disposition: ClaimedDispatchDisposition,
        superseded_by_revision: Option<ScheduleRevision>,
    },
    CompletionSupersessionClassified {
        disposition: CompletionSupersessionDisposition,
        superseded_by_revision: Option<ScheduleRevision>,
    },
    DeliveryFailed,
    LeaseExpired,
    /// The current claim-token holder extended its own lease while its
    /// delivery is still in flight. Acknowledgement only; no receipt.
    LeaseRenewed,
    /// 0.7.2 D2a: a delivery resolution arrived after this occurrence was
    /// superseded and was recorded as a typed late-arrival fact. The driver
    /// mirrors this to know the arrival landed as a late record (no fresh
    /// terminal receipt is minted for it).
    LateCompletionResolutionRecorded {
        resolution: LateCompletionResolutionClass,
    },
    /// 0.7.2 D2a: a completion-shaped arrival bearing stale claim evidence
    /// was classified. Pure observability; lifecycle state is untouched.
    StaleCompletionArrivalClassified {
        phase: OccurrencePhase,
        trigger: StaleCompletionArrivalTrigger,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum OccurrenceDueAction {
    ClaimEligible,
    MisfireRequired,
    LeaseExpired,
    /// Past the misfire deadline with recorded dispatch evidence and no
    /// reclaim left: resolve it with
    /// [`OccurrenceLifecycleInput::ResolveReclaimExhausted`].
    ReclaimExhausted,
}

/// What an occurrence's receipt history proves about its earlier attempts'
/// target admission. Stores derive it per row from the recorded receipts
/// ([`AdmissionEvidence::from_receipt_stages`]); no occurrence field carries
/// it, because every lease expiry overwrites the receipt fields and a
/// reclaimed attempt can die again before its own admission.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum AdmissionEvidence {
    /// No DispatchStarted and no DispatchAccepted receipt: the misfire
    /// policy decides a late reclaim, as before.
    #[default]
    None,
    /// A DispatchStarted receipt and no DispatchAccepted one: an earlier
    /// attempt started its dispatch, and its admission was never proven.
    Started,
    /// A DispatchAccepted receipt: an earlier attempt's target admitted the
    /// delivery.
    Accepted,
}

impl AdmissionEvidence {
    /// The evidence of a receipt history, given its receipts' stages in any
    /// order: Accepted when any receipt is DispatchAccepted, Started when
    /// any is DispatchStarted and none DispatchAccepted, None otherwise.
    pub fn from_receipt_stages(stages: impl IntoIterator<Item = DeliveryReceiptStage>) -> Self {
        let mut evidence = Self::None;
        for stage in stages {
            match stage {
                DeliveryReceiptStage::DispatchAccepted => return Self::Accepted,
                DeliveryReceiptStage::DispatchStarted => evidence = Self::Started,
                _ => {}
            }
        }
        evidence
    }
}

/// The attempts an occurrence whose earlier attempt recorded its dispatch
/// may use past its misfire deadline. The occurrence machine's guards spell
/// the same cap as a literal, which no constant feeds: rows pin each guard
/// at its boundary, and a test pins this constant to the machine's.
pub const ADMITTED_RECLAIM_ATTEMPT_CAP: u32 = 5;

/// Domain mirror of the occurrence authority's claimed-occurrence pre-dispatch
/// disposition verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaimedDispatchDisposition {
    Frozen,
    Supersede,
    Ready,
    FutureRevision,
}

/// Machine-decided claimed-occurrence pre-dispatch verdict the driver mirrors.
/// `superseded_by_revision` is `Some` only for the `Supersede` disposition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClaimedDispatchVerdict {
    pub disposition: ClaimedDispatchDisposition,
    pub superseded_by_revision: Option<ScheduleRevision>,
}

/// Domain mirror of the occurrence authority's post-completion supersession
/// disposition verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionSupersessionDisposition {
    Supersede,
    Proceed,
    /// 0.7.2 D2a: the occurrence is already Superseded (the schedule-commit
    /// supersession sweep landed between dispatch and completion). The driver
    /// delivers the completion outcome as a typed late-arrival record; no
    /// fresh terminal transition or receipt is minted.
    AlreadySuperseded,
}

/// Machine-decided post-completion supersession verdict the driver mirrors.
/// `superseded_by_revision` is `Some` only for the `Supersede` disposition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompletionSupersessionVerdict {
    pub disposition: CompletionSupersessionDisposition,
    pub superseded_by_revision: Option<ScheduleRevision>,
}

#[derive(Debug, Clone)]
pub struct OccurrenceLifecycleMutator {
    pub(crate) occurrence: Occurrence,
    pub(crate) effects: Vec<OccurrenceLifecycleEffect>,
    write_precondition: OccurrenceWritePrecondition,
    supersession_acks: Vec<OccurrenceSupersessionAck>,
}

impl OccurrenceLifecycleMutator {
    pub fn occurrence(&self) -> &Occurrence {
        &self.occurrence
    }

    pub fn effects(&self) -> &[OccurrenceLifecycleEffect] {
        &self.effects
    }

    #[cfg(test)]
    fn into_occurrence(self) -> Occurrence {
        self.occurrence
    }

    #[cfg(test)]
    fn into_parts_with_supersession_feedback(
        self,
    ) -> (
        Occurrence,
        Vec<OccurrenceLifecycleEffect>,
        Vec<OccurrenceSupersessionAck>,
    ) {
        (self.occurrence, self.effects, self.supersession_acks)
    }

    /// Fold a generated follow-up transition into this still-uncommitted
    /// transition while retaining the original durable CAS parent.
    pub fn absorb_followup(
        &mut self,
        followup: OccurrenceLifecycleMutator,
    ) -> Result<(), OccurrenceLifecycleError> {
        let OccurrenceLifecycleMutator {
            occurrence,
            effects,
            write_precondition,
            supersession_acks,
        } = followup;
        write_precondition
            .check_current(Some(&self.occurrence))
            .map_err(|reason| OccurrenceLifecycleError::ProjectionMismatch { reason })?;
        self.occurrence = occurrence;
        self.effects.extend(effects);
        self.supersession_acks.extend(supersession_acks);
        Ok(())
    }

    pub fn into_authorized_write(self) -> AuthorizedOccurrenceWrite {
        AuthorizedOccurrenceWrite {
            occurrence: self.occurrence,
            precondition: self.write_precondition,
            effects: self.effects,
            supersession_acks: self.supersession_acks,
        }
    }

    /// Consume a projection-only transition that is defined to emit no
    /// lifecycle effects or supersession acknowledgements.
    ///
    /// If the generated machine later adds an output to such a transition,
    /// this fails closed instead of silently erasing the new authority.
    pub fn into_effectless_occurrence(self) -> Result<Occurrence, OccurrenceLifecycleError> {
        if !self.effects.is_empty() || !self.supersession_acks.is_empty() {
            return Err(OccurrenceLifecycleError::ProjectionMismatch {
                reason: format!(
                    "effectless occurrence projection produced {} lifecycle effects and {} supersession acknowledgements",
                    self.effects.len(),
                    self.supersession_acks.len()
                ),
            });
        }
        Ok(self.occurrence)
    }
}

/// Machine-authorized occurrence transition awaiting a durable store commit.
///
/// `precondition` binds the exact previous durable authority, `occurrence`
/// carries the exact successor, and `effects` are the generated lifecycle
/// verdicts emitted by that same transition. The authorized write is not a
/// commit; an open [`crate::ScheduleStore`] backend is trusted to construct the
/// commit assertion only after its durable transaction succeeds.
#[derive(Debug)]
pub struct AuthorizedOccurrenceWrite {
    occurrence: Occurrence,
    precondition: OccurrenceWritePrecondition,
    effects: Vec<OccurrenceLifecycleEffect>,
    supersession_acks: Vec<OccurrenceSupersessionAck>,
}

impl AuthorizedOccurrenceWrite {
    pub fn occurrence(&self) -> &Occurrence {
        &self.occurrence
    }

    pub fn occurrence_id(&self) -> &crate::types::OccurrenceId {
        &self.occurrence.occurrence_id
    }

    pub fn precondition(&self) -> &OccurrenceWritePrecondition {
        &self.precondition
    }

    /// Whether this write has generated outputs that require a postcommit
    /// carrier. Atomic schedule planning batches contain only effectless
    /// absent-row inserts; backends must reject any other use of those batch
    /// methods instead of silently erasing machine outputs.
    pub fn has_generated_outputs(&self) -> bool {
        !self.effects.is_empty() || !self.supersession_acks.is_empty()
    }

    #[doc(hidden)]
    pub fn supersession_acks(&self) -> &[OccurrenceSupersessionAck] {
        &self.supersession_acks
    }

    /// Trusted-backend assertion used after the backing store transaction has
    /// successfully committed. Rust visibility cannot prove that provenance
    /// across third-party store crates; calling it before commit or outside a
    /// `ScheduleStore` implementation violates that trait's contract.
    #[doc(hidden)]
    pub fn into_transition_commit_after_store_commit(self) -> OccurrenceTransitionCommit {
        OccurrenceTransitionCommit {
            prior: self.precondition,
            successor: self.occurrence,
            effects: self.effects,
            supersession_acks: self.supersession_acks,
        }
    }
}

/// Trusted-store assertion that one machine-authorized occurrence successor
/// passed its exact prior-state CAS and was committed by the selected backend.
///
/// The generated effects and supersession acknowledgements are inseparable
/// from that exact prior/successor pair. They are postcommit observations, not
/// a durable outbox: callers must not claim crash-replayability or at-most-once
/// external realization from this in-process carrier alone. This is not a
/// cryptographic or capability-token proof; consumers must accept it only as
/// the direct result of the `ScheduleStore` method they invoked.
#[must_use = "a committed occurrence transition carries generated lifecycle outputs that must be observed"]
#[derive(Debug)]
pub struct OccurrenceTransitionCommit {
    prior: OccurrenceWritePrecondition,
    successor: Occurrence,
    effects: Vec<OccurrenceLifecycleEffect>,
    supersession_acks: Vec<OccurrenceSupersessionAck>,
}

impl OccurrenceTransitionCommit {
    pub fn prior(&self) -> &OccurrenceWritePrecondition {
        &self.prior
    }

    pub fn successor(&self) -> &Occurrence {
        &self.successor
    }

    pub fn effects(&self) -> &[OccurrenceLifecycleEffect] {
        &self.effects
    }

    pub fn supersession_acks(&self) -> &[OccurrenceSupersessionAck] {
        &self.supersession_acks
    }

    pub fn into_parts(
        self,
    ) -> (
        OccurrenceWritePrecondition,
        Occurrence,
        Vec<OccurrenceLifecycleEffect>,
        Vec<OccurrenceSupersessionAck>,
    ) {
        (
            self.prior,
            self.successor,
            self.effects,
            self.supersession_acks,
        )
    }
}

impl std::ops::Deref for OccurrenceTransitionCommit {
    type Target = Occurrence;

    fn deref(&self) -> &Self::Target {
        self.successor()
    }
}

#[derive(Debug, Clone)]
pub struct OccurrenceWritePrecondition {
    kind: OccurrenceWritePreconditionKind,
}

#[derive(Debug, Clone)]
enum OccurrenceWritePreconditionKind {
    Absent {
        occurrence_id: crate::types::OccurrenceId,
    },
    Matches {
        occurrence_id: crate::types::OccurrenceId,
        fingerprint: Vec<u8>,
    },
}

impl OccurrenceWritePrecondition {
    fn absent(occurrence_id: crate::types::OccurrenceId) -> Self {
        Self {
            kind: OccurrenceWritePreconditionKind::Absent { occurrence_id },
        }
    }

    fn matches(occurrence: &Occurrence) -> Result<Self, OccurrenceLifecycleError> {
        Ok(Self {
            kind: OccurrenceWritePreconditionKind::Matches {
                occurrence_id: occurrence.occurrence_id.clone(),
                fingerprint: occurrence_write_fingerprint(occurrence)
                    .map_err(|reason| OccurrenceLifecycleError::ProjectionMismatch { reason })?,
            },
        })
    }

    pub fn occurrence_id(&self) -> &crate::types::OccurrenceId {
        match &self.kind {
            OccurrenceWritePreconditionKind::Absent { occurrence_id }
            | OccurrenceWritePreconditionKind::Matches { occurrence_id, .. } => occurrence_id,
        }
    }

    pub fn check_current(&self, current: Option<&Occurrence>) -> Result<(), String> {
        match &self.kind {
            OccurrenceWritePreconditionKind::Absent { occurrence_id } => {
                if current.is_some() {
                    return Err(format!(
                        "generated occurrence write expected absent durable occurrence {occurrence_id}"
                    ));
                }
                Ok(())
            }
            OccurrenceWritePreconditionKind::Matches {
                occurrence_id,
                fingerprint,
            } => {
                let current = current.ok_or_else(|| {
                    format!(
                        "generated occurrence write expected durable occurrence {occurrence_id}"
                    )
                })?;
                if &current.occurrence_id != occurrence_id {
                    return Err(format!(
                        "generated occurrence write precondition id `{occurrence_id}` did not match durable occurrence `{}`",
                        current.occurrence_id
                    ));
                }
                let current_fingerprint = occurrence_write_fingerprint(current)?;
                if current_fingerprint != *fingerprint {
                    return Err(format!(
                        "generated occurrence write precondition for {occurrence_id} did not match durable machine authority state"
                    ));
                }
                Ok(())
            }
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum OccurrenceLifecycleError {
    #[error("occurrence is already terminal")]
    AlreadyTerminal,
    #[error("occurrence must be pending before it can be claimed")]
    NotPendingForClaim,
    #[error("generated occurrence authority rejected claim")]
    ClaimRejected,
    #[error("occurrence must be claimed before dispatching")]
    NotClaimed,
    #[error("occurrence must be dispatching before awaiting completion")]
    NotDispatching,
    #[error("occurrence must be in a live phase for this terminal transition")]
    NotLiveForTerminal,
    #[error("occurrence must hold an active lease before it can expire")]
    NotLeaseHolding,
    #[error("generated occurrence authority rejected planned occurrence facts")]
    PlanRejected,
    #[error("generated occurrence authority rejected target snapshot sync")]
    TargetSyncRejected,
    #[error("generated occurrence authority rejected receipt/result projection")]
    ReceiptRecordRejected,
    #[error("generated occurrence authority rejected due classification")]
    DueClassificationRejected,
    #[error("generated occurrence authority rejected claimed-dispatch disposition classification")]
    ClaimedDispatchClassificationRejected,
    #[error(
        "generated occurrence authority rejected completion-supersession disposition classification"
    )]
    CompletionSupersessionClassificationRejected,
    #[error(
        "generated occurrence authority rejected stale-completion-arrival classification (trigger was not completion-shaped)"
    )]
    StaleCompletionArrivalClassificationRejected,
    #[error("generated occurrence authority rejected recovered machine state: {source}")]
    AuthorityRecoveryRejected {
        source: occ_dsl::OccurrenceLifecycleMachineTransitionError,
    },
    #[error("generated occurrence authority rejected transition failure classification: {source}")]
    TransitionFailureClassificationRejected {
        source: occ_dsl::OccurrenceLifecycleMachineTransitionError,
    },
    #[error("generated occurrence authority did not emit transition failure classification")]
    TransitionFailureClassificationMissing,
    #[error(
        "generated occurrence authority emitted transition failure classification for `{actual:?}` while classifying `{expected:?}`"
    )]
    TransitionFailureClassificationMismatch {
        phase: occ_dsl::OccurrenceLifecycleState,
        expected_refusal_kind: occ_dsl::OccurrenceTransitionFailureRefusalKind,
        actual_refusal_kind: occ_dsl::OccurrenceTransitionFailureRefusalKind,
        expected: occ_dsl::OccurrenceLifecycleInputVariant,
        actual: occ_dsl::OccurrenceLifecycleInputVariant,
    },
    #[error("generated occurrence authority emitted multiple transition failure classifications")]
    TransitionFailureClassificationAmbiguous,
    #[error(
        "generated occurrence authority emitted transition failure classification during lifecycle mutation"
    )]
    UnexpectedTransitionFailureClassificationEffect,
    #[error("OccurrenceLifecycleMachine emitted invalid occurrence id `{id}`: {source}")]
    InvalidOccurrenceId { id: String, source: uuid::Error },
    #[error("OccurrenceLifecycleMachine emitted invalid schedule id `{id}`: {source}")]
    InvalidScheduleId { id: String, source: uuid::Error },
    #[error("OccurrenceLifecycleMachine emitted invalid due timestamp millis `{millis}`")]
    InvalidDueAtUtcMillis { millis: u64 },
    #[error(
        "OccurrenceLifecycleMachine emitted invalid misfire deadline timestamp millis `{millis}`"
    )]
    InvalidMisfireDeadlineUtcMillis { millis: u64 },
    #[error("OccurrenceLifecycleMachine emitted invalid {field} timestamp millis `{millis}`")]
    InvalidTimestampMillis { field: &'static str, millis: u64 },
    #[error("OccurrenceLifecycleMachine emitted invalid claim token `{token}`: {source}")]
    InvalidClaimToken { token: String, source: uuid::Error },
    #[error("OccurrenceLifecycleMachine emitted invalid session id `{id}`: {source}")]
    InvalidSessionId { id: String, source: uuid::Error },
    #[error("OccurrenceLifecycleMachine emitted invalid attempt count `{value}`")]
    InvalidAttemptCount { value: u64 },
    #[error("occurrence timestamp `{field}` cannot be represented as unsigned millis: {millis}")]
    InvalidInputTimestampMillis { field: &'static str, millis: i64 },
    #[error(transparent)]
    SemanticKeySerialization(#[from] SemanticKeySerializationError),
    #[error(
        "OccurrenceLifecycleMachine target key `{machine_key}` did not match target snapshot key `{snapshot_key}`"
    )]
    TargetBindingKeyMismatch {
        machine_key: String,
        snapshot_key: String,
    },
    #[error(
        "OccurrenceLifecycleMachine runtime outcome key `{machine_key:?}` did not match runtime outcome key `{snapshot_key:?}`"
    )]
    RuntimeOutcomeKeyMismatch {
        machine_key: Option<String>,
        snapshot_key: Option<String>,
    },
    #[error("OccurrenceLifecycleMachine has no generated receipt authority for this projection")]
    MissingReceiptAuthority,
    #[error(
        "OccurrenceLifecycleMachine receipt stage `{expected:?}` did not match supplied receipt stage `{actual:?}`"
    )]
    ReceiptStageMismatch {
        expected: DeliveryReceiptStage,
        actual: DeliveryReceiptStage,
    },
    #[error(
        "OccurrenceLifecycleMachine receipt failure class `{expected:?}` did not match supplied receipt failure class `{actual:?}`"
    )]
    ReceiptFailureClassMismatch {
        expected: Option<OccurrenceFailureClass>,
        actual: Option<OccurrenceFailureClass>,
    },
    #[error(
        "OccurrenceLifecycleMachine receipt detail `{expected:?}` did not match supplied receipt detail `{actual:?}`"
    )]
    ReceiptDetailMismatch {
        expected: Option<String>,
        actual: Option<String>,
    },
    #[error(
        "OccurrenceLifecycleMachine canonical receipt `{expected:?}` did not match supplied receipt `{actual:?}`"
    )]
    ReceiptRecordMismatch {
        expected: Box<DeliveryReceipt>,
        actual: Box<DeliveryReceipt>,
    },
    #[error("OccurrenceLifecycleMachine emitted ambiguous due classification effects: {effects:?}")]
    AmbiguousDueClassification { effects: Vec<&'static str> },
    #[error("OccurrenceLifecycleMachine did not emit a terminality verdict")]
    MissingTerminalityClassification,
    #[error("OccurrenceLifecycleMachine emitted multiple terminality verdicts")]
    AmbiguousTerminalityClassification,
    #[error(
        "OccurrenceLifecycleMachine did not emit a claimed-dispatch disposition for the observed schedule facts"
    )]
    MissingClaimedDispatchDisposition,
    #[error(
        "OccurrenceLifecycleMachine emitted multiple claimed-dispatch disposition classifications"
    )]
    AmbiguousClaimedDispatchDisposition,
    #[error(
        "OccurrenceLifecycleMachine classified the claimed-dispatch disposition as Supersede without a superseding revision"
    )]
    ClaimedDispatchSupersedeMissingRevision,
    #[error(
        "OccurrenceLifecycleMachine did not emit a completion-supersession disposition for the observed schedule facts"
    )]
    MissingCompletionSupersessionDisposition,
    #[error(
        "OccurrenceLifecycleMachine emitted multiple completion-supersession disposition classifications"
    )]
    AmbiguousCompletionSupersessionDisposition,
    #[error(
        "OccurrenceLifecycleMachine classified the completion-supersession disposition as Supersede without a superseding revision"
    )]
    CompletionSupersessionSupersedeMissingRevision,
    #[error("occurrence projection does not match generated machine_state: {reason}")]
    ProjectionMismatch { reason: String },
}

// ===========================================================================
// Occurrence::apply — DSL-backed lifecycle transition on the domain type
// ===========================================================================

impl Occurrence {
    pub fn planned_from_schedule(
        schedule: &Schedule,
        occurrence_ordinal: OccurrenceOrdinal,
        due_at_utc: DateTime<Utc>,
    ) -> Result<Self, OccurrenceLifecycleError> {
        let target_snapshot = schedule.target.clone();
        let input = OccurrenceLifecycleInput::PlanOccurrence {
            occurrence_id: crate::types::OccurrenceId::new(),
            schedule_id: schedule.schedule_id.clone(),
            schedule_revision: schedule.revision,
            occurrence_ordinal,
            trigger_snapshot: schedule.trigger.clone(),
            target_snapshot: target_snapshot.clone(),
            misfire_policy: schedule.misfire_policy.clone(),
            overlap_policy: schedule.overlap_policy.clone(),
            missing_target_policy: schedule.missing_target_policy.clone(),
            due_at_utc,
        };
        let dsl_input = convert_occurrence_input(&input)?;
        let mut dsl_auth = occ_dsl::OccurrenceLifecycleMachineAuthority::new();
        occ_dsl::OccurrenceLifecycleMachineMutator::apply(&mut dsl_auth, dsl_input)
            .map_err(|e| map_occurrence_error(&mut dsl_auth, e))?;
        occurrence_from_planned_state(dsl_auth.state(), schedule, target_snapshot, Utc::now())
    }

    pub fn planned_write_from_schedule(
        schedule: &Schedule,
        occurrence_ordinal: OccurrenceOrdinal,
        due_at_utc: DateTime<Utc>,
    ) -> Result<AuthorizedOccurrenceWrite, OccurrenceLifecycleError> {
        let occurrence = Self::planned_from_schedule(schedule, occurrence_ordinal, due_at_utc)?;
        Ok(AuthorizedOccurrenceWrite {
            precondition: OccurrenceWritePrecondition::absent(occurrence.occurrence_id.clone()),
            occurrence,
            effects: Vec::new(),
            supersession_acks: Vec::new(),
        })
    }

    /// Apply a lifecycle input to this occurrence through its persisted
    /// generated machine state, writing the resulting machine-owned state back
    /// into `self`.
    ///
    /// Non-machine fields (trigger_snapshot, target_snapshot, policies,
    /// created_at_utc) are preserved verbatim; the DSL is sole authority over
    /// phase, claim fields, timestamps, attempt_count, and failure data.
    pub fn apply(
        mut self,
        input: OccurrenceLifecycleInput,
    ) -> Result<OccurrenceLifecycleMutator, OccurrenceLifecycleError> {
        let write_precondition = OccurrenceWritePrecondition::matches(&self)?;
        // 1. Recover generated DSL state after validating compatibility fields.
        let dsl_state = occurrence_machine_state_for_authority(&self)?;

        // 2. Convert domain input → DSL input
        let dsl_input = convert_occurrence_input(&input)?;

        // 3. Run DSL dispatch
        let mut dsl_auth =
            occ_dsl::OccurrenceLifecycleMachineAuthority::recover_from_state(dsl_state)
                .map_err(|source| OccurrenceLifecycleError::AuthorityRecoveryRejected { source })?;
        let transition =
            occ_dsl::OccurrenceLifecycleMachineMutator::apply(&mut dsl_auth, dsl_input)
                .map_err(|e| map_occurrence_error(&mut dsl_auth, e))?;

        // 4. Write DSL state → occurrence
        write_back_occurrence(dsl_auth.state(), &mut self, &input)?;

        // 5. Map effects
        let effects = transition
            .effects()
            .iter()
            .map(map_occurrence_effect)
            .collect::<Result<Vec<_>, _>>()?;
        let supersession_acks = effects
            .iter()
            .filter_map(|effect| {
                OccurrenceSupersessionAck::from_occurrence_authority(&self, effect)
            })
            .collect();

        Ok(OccurrenceLifecycleMutator {
            occurrence: self,
            effects,
            write_precondition,
            supersession_acks,
        })
    }

    /// Classify this occurrence's due action at `now_utc`, given what its
    /// receipt history proves about its earlier attempts' admission.
    pub fn classify_due_action(
        &self,
        now_utc: DateTime<Utc>,
        admission_evidence: AdmissionEvidence,
    ) -> Result<Option<OccurrenceDueAction>, OccurrenceLifecycleError> {
        let mutator = self.clone().apply(OccurrenceLifecycleInput::ClassifyDue {
            now_utc,
            admission_evidence,
        })?;
        due_action_from_effects(&mutator.effects)
    }

    /// The detail of [`OccurrenceLifecycleInput::ResolveReclaimExhausted`]
    /// for this occurrence's evidence.
    pub fn reclaim_exhausted_detail(&self, admission_evidence: AdmissionEvidence) -> String {
        match admission_evidence {
            AdmissionEvidence::Accepted => format!(
                "admitted delivery lost its executor {ADMITTED_RECLAIM_ATTEMPT_CAP} times; the input's outcome was never observed"
            ),
            _ => format!(
                "dispatch started {ADMITTED_RECLAIM_ATTEMPT_CAP} times past the misfire deadline without a proven admission"
            ),
        }
    }

    /// Classify whether this occurrence is terminal.
    ///
    /// The shell extracts no fact: it drives the canonical
    /// `OccurrenceLifecycleMachine`'s `ClassifyOccurrenceTerminality` input over
    /// the occurrence's recovered machine state. The machine owns the
    /// lifecycle_phase and the terminal-phase SET; this function mirrors the
    /// emitted `OccurrenceTerminalityClassified.terminal`, failing closed if the
    /// machine refuses or emits no verdict. It decides nothing.
    pub fn classify_terminality(&self) -> Result<bool, OccurrenceLifecycleError> {
        let mutator = self
            .clone()
            .apply(OccurrenceLifecycleInput::ClassifyOccurrenceTerminality)?;
        terminality_from_effects(&mutator.effects)
    }

    /// Mirror the machine-owned terminality verdict as a `bool`, failing closed.
    ///
    /// The terminality verdict belongs to the canonical
    /// `OccurrenceLifecycleMachine` (see [`Occurrence::classify_terminality`]).
    /// This consumer-facing projection mirrors the machine bool; if the machine
    /// refuses to classify (e.g. the recovered state is rejected), it fails
    /// closed by treating the occurrence as terminal so an unclassifiable
    /// occurrence is never offered as live work.
    pub fn is_terminal(&self) -> bool {
        self.classify_terminality().unwrap_or(true)
    }

    /// Classify the pre-dispatch disposition of this claimed occurrence against
    /// the observed owning-schedule facts.
    ///
    /// The driver shell extracts the schedule's current phase and revision (pure
    /// observations) and feeds them here; the occurrence authority — not the
    /// driver — decides Frozen / Supersede / Ready / FutureRevision. The driver
    /// mirrors the returned verdict. Fails closed if the authority emits no
    /// disposition.
    pub fn classify_claimed_dispatch_disposition(
        &self,
        schedule_phase: SchedulePhase,
        current_schedule_revision: ScheduleRevision,
    ) -> Result<ClaimedDispatchVerdict, OccurrenceLifecycleError> {
        let mutator = self.clone().apply(
            OccurrenceLifecycleInput::ClassifyClaimedDispatchDisposition {
                schedule_phase,
                current_schedule_revision,
            },
        )?;
        claimed_dispatch_verdict_from_effects(&mutator.effects)
    }

    /// Classify the post-completion supersession disposition of this dispatched
    /// occurrence against the observed owning-schedule facts.
    ///
    /// The driver shell extracts the schedule's current phase and revision (pure
    /// observations) and feeds them here; the occurrence authority — not the
    /// driver — decides Supersede / Proceed. The driver mirrors the returned
    /// verdict. Fails closed if the authority emits no disposition.
    pub fn classify_completion_supersession(
        &self,
        schedule_phase: SchedulePhase,
        current_schedule_revision: ScheduleRevision,
    ) -> Result<CompletionSupersessionVerdict, OccurrenceLifecycleError> {
        let mutator =
            self.clone()
                .apply(OccurrenceLifecycleInput::ClassifyCompletionSupersession {
                    schedule_phase,
                    current_schedule_revision,
                })?;
        completion_supersession_verdict_from_effects(&mutator.effects)
    }

    /// Build a public receipt from the generated receipt facts accepted by the
    /// occurrence lifecycle machine.
    pub fn delivery_receipt_from_authority(
        &self,
        runtime_outcome: Option<RuntimeDeliveryOutcome>,
    ) -> Result<DeliveryReceipt, OccurrenceLifecycleError> {
        delivery_receipt_from_authority(self, runtime_outcome)
    }
}

/// Recover generated occurrence state after checking display projections.
fn occurrence_machine_state_for_authority(
    occ: &Occurrence,
) -> Result<occ_dsl::OccurrenceLifecycleMachineState, OccurrenceLifecycleError> {
    validate_occurrence_machine_projection(occ)
        .map_err(|reason| OccurrenceLifecycleError::ProjectionMismatch { reason })?;
    Ok(occ.machine_state.clone())
}

#[cfg(test)]
fn occurrence_machine_state_for_test(
    occ: &Occurrence,
) -> Result<occ_dsl::OccurrenceLifecycleMachineState, OccurrenceLifecycleError> {
    occurrence_machine_state_for_authority(occ)
}

fn occurrence_phase_from_dsl(phase: occ_dsl::OccurrenceLifecycleState) -> OccurrencePhase {
    match phase {
        occ_dsl::OccurrenceLifecycleState::Pending => OccurrencePhase::Pending,
        occ_dsl::OccurrenceLifecycleState::Claimed => OccurrencePhase::Claimed,
        occ_dsl::OccurrenceLifecycleState::Dispatching => OccurrencePhase::Dispatching,
        occ_dsl::OccurrenceLifecycleState::AwaitingCompletion => {
            OccurrencePhase::AwaitingCompletion
        }
        occ_dsl::OccurrenceLifecycleState::Completed => OccurrencePhase::Completed,
        occ_dsl::OccurrenceLifecycleState::Skipped => OccurrencePhase::Skipped,
        occ_dsl::OccurrenceLifecycleState::Misfired => OccurrencePhase::Misfired,
        occ_dsl::OccurrenceLifecycleState::Superseded => OccurrencePhase::Superseded,
        occ_dsl::OccurrenceLifecycleState::DeliveryFailed => OccurrencePhase::DeliveryFailed,
    }
}

fn occurrence_from_planned_state(
    dsl: &occ_dsl::OccurrenceLifecycleMachineState,
    schedule: &Schedule,
    target_snapshot: TargetBinding,
    created_at_utc: DateTime<Utc>,
) -> Result<Occurrence, OccurrenceLifecycleError> {
    let snapshot_key = target_stable_key(&target_snapshot)?;
    if dsl.target_binding_key.0 != snapshot_key {
        return Err(OccurrenceLifecycleError::TargetBindingKeyMismatch {
            machine_key: dsl.target_binding_key.0.clone(),
            snapshot_key,
        });
    }
    let trigger_key = trigger_stable_key(&schedule.trigger)?;
    if dsl.trigger_key.0 != trigger_key {
        return Err(OccurrenceLifecycleError::TargetBindingKeyMismatch {
            machine_key: dsl.trigger_key.0.clone(),
            snapshot_key: trigger_key,
        });
    }
    verify_occurrence_policy_keys(
        dsl,
        &schedule.misfire_policy,
        &schedule.overlap_policy,
        &schedule.missing_target_policy,
    )?;

    Ok(Occurrence {
        occurrence_id: occurrence_id_from_dsl(&dsl.occurrence_id)?,
        schedule_id: occurrence_schedule_id_from_dsl(&dsl.schedule_id)?,
        schedule_revision: ScheduleRevision(dsl.schedule_revision),
        occurrence_ordinal: OccurrenceOrdinal(dsl.occurrence_ordinal),
        phase: occurrence_phase_from_dsl(dsl.lifecycle_phase),
        machine_state: dsl.clone(),
        due_at_utc: millis_to_datetime(dsl.due_at_utc_ms).ok_or(
            OccurrenceLifecycleError::InvalidDueAtUtcMillis {
                millis: dsl.due_at_utc_ms,
            },
        )?,
        trigger_snapshot: schedule.trigger.clone(),
        target_snapshot,
        misfire_policy: schedule.misfire_policy.clone(),
        overlap_policy: schedule.overlap_policy.clone(),
        missing_target_policy: schedule.missing_target_policy.clone(),
        claimed_by: dsl.claimed_by.clone().map(|owner| owner.0),
        lease_expires_at_utc: occurrence_optional_datetime_from_dsl(
            dsl.lease_expires_at_utc_ms,
            "lease_expires_at_utc",
        )?,
        claim_token: claim_token_from_dsl(&dsl.claim_token)?,
        delivery_correlation_id: dsl.delivery_correlation_id.clone().map(|id| id.0),
        last_receipt: None,
        failure_class: dsl.failure_class.map(from_dsl_failure_class),
        runtime_outcome: None,
        failure_detail: dsl.failure_detail.clone(),
        attempt_count: attempt_count_from_dsl(dsl.attempt_count)?,
        created_at_utc,
        claimed_at_utc: occurrence_optional_datetime_from_dsl(
            dsl.claimed_at_utc_ms,
            "claimed_at_utc",
        )?,
        dispatched_at_utc: occurrence_optional_datetime_from_dsl(
            dsl.dispatched_at_utc_ms,
            "dispatched_at_utc",
        )?,
        completed_at_utc: occurrence_optional_datetime_from_dsl(
            dsl.completed_at_utc_ms,
            "completed_at_utc",
        )?,
        superseded_by_revision: dsl.superseded_by_revision.map(ScheduleRevision),
    })
}

/// Convert domain input to DSL input.
fn convert_occurrence_input(
    input: &OccurrenceLifecycleInput,
) -> Result<occ_dsl::OccurrenceLifecycleInput, OccurrenceLifecycleError> {
    Ok(match input {
        OccurrenceLifecycleInput::PlanOccurrence {
            occurrence_id,
            schedule_id,
            schedule_revision,
            occurrence_ordinal,
            trigger_snapshot,
            target_snapshot,
            misfire_policy,
            overlap_policy,
            missing_target_policy,
            due_at_utc,
        } => occ_dsl::OccurrenceLifecycleInput::PlanOccurrence {
            occurrence_id: occ_dsl::OccurrenceId(occurrence_id.0.to_string()),
            schedule_id: occ_dsl::ScheduleId(schedule_id.0.to_string()),
            schedule_revision: schedule_revision.0,
            occurrence_ordinal: occurrence_ordinal.0,
            trigger_key: trigger_stable_key(trigger_snapshot)?.into(),
            target_binding_key: target_stable_key(target_snapshot)?.into(),
            misfire_policy: to_occ_dsl_misfire_policy(misfire_policy),
            misfire_policy_key: misfire_policy_authority_key(misfire_policy)?,
            overlap_policy: to_occ_dsl_overlap_policy(overlap_policy),
            overlap_policy_key: overlap_policy_authority_key(overlap_policy)?,
            missing_target_policy: to_occ_dsl_missing_target_policy(missing_target_policy),
            missing_target_policy_key: missing_target_policy_authority_key(missing_target_policy)?,
            target_materialized_session_id: target_materialized_session_id(target_snapshot)
                .map(|session_id| occ_dsl::SessionId(session_id.0.to_string())),
            due_at_utc_ms: occurrence_datetime_to_millis(*due_at_utc, "due_at_utc")?,
            misfire_deadline_utc_ms: occurrence_misfire_deadline_to_millis(
                misfire_policy,
                *due_at_utc,
            )?,
        },
        OccurrenceLifecycleInput::SyncTargetSnapshot { target_snapshot } => {
            occ_dsl::OccurrenceLifecycleInput::SyncTargetSnapshot {
                target_binding_key: target_stable_key(target_snapshot)?.into(),
                target_materialized_session_id: target_materialized_session_id(target_snapshot)
                    .map(|session_id| occ_dsl::SessionId(session_id.0.to_string())),
            }
        }
        OccurrenceLifecycleInput::RecordReceipt {
            receipt,
            runtime_outcome,
            ..
        } => occ_dsl::OccurrenceLifecycleInput::RecordReceipt {
            correlation_id: receipt.correlation_id.clone().map(Into::into),
            detail: receipt.detail.clone(),
            materialized_session_id: receipt
                .materialized_session_id
                .as_ref()
                .map(|session_id| occ_dsl::SessionId(session_id.0.to_string())),
            runtime_outcome_key: runtime_outcome
                .as_ref()
                .map(runtime_outcome_authority_key)
                .transpose()?,
        },
        OccurrenceLifecycleInput::ClassifyDue {
            now_utc,
            admission_evidence,
        } => occ_dsl::OccurrenceLifecycleInput::ClassifyDue {
            now_utc_ms: occurrence_datetime_to_millis(*now_utc, "now_utc")?,
            admission_evidence: to_dsl_admission_evidence(*admission_evidence),
        },
        OccurrenceLifecycleInput::ClassifyOccurrenceTerminality => {
            occ_dsl::OccurrenceLifecycleInput::ClassifyOccurrenceTerminality {}
        }
        OccurrenceLifecycleInput::ClassifyClaimedDispatchDisposition {
            schedule_phase,
            current_schedule_revision,
        } => occ_dsl::OccurrenceLifecycleInput::ClassifyClaimedDispatchDisposition {
            schedule_phase: to_dsl_claimed_dispatch_schedule_phase(*schedule_phase),
            current_schedule_revision: current_schedule_revision.0,
        },
        OccurrenceLifecycleInput::ClassifyCompletionSupersession {
            schedule_phase,
            current_schedule_revision,
        } => occ_dsl::OccurrenceLifecycleInput::ClassifyCompletionSupersession {
            schedule_phase: to_dsl_claimed_dispatch_schedule_phase(*schedule_phase),
            current_schedule_revision: current_schedule_revision.0,
        },
        OccurrenceLifecycleInput::Claim {
            owner_id,
            at_utc,
            lease_expires_at_utc,
            claim_token,
            admission_evidence,
        } => occ_dsl::OccurrenceLifecycleInput::Claim {
            owner_id: owner_id.clone().into(),
            at_utc_ms: occurrence_datetime_to_millis(*at_utc, "at_utc")?,
            lease_expires_at_utc_ms: occurrence_datetime_to_millis(
                *lease_expires_at_utc,
                "lease_expires_at_utc",
            )?,
            claim_token: occ_dsl::ClaimToken(claim_token.to_string()),
            admission_evidence: to_dsl_admission_evidence(*admission_evidence),
        },
        OccurrenceLifecycleInput::DispatchStarted {
            correlation_id,
            at_utc,
        } => occ_dsl::OccurrenceLifecycleInput::DispatchStarted {
            correlation_id: correlation_id.clone().map(Into::into),
            at_utc_ms: occurrence_datetime_to_millis(*at_utc, "at_utc")?,
        },
        OccurrenceLifecycleInput::DispatchAccepted {
            admission_outcome,
            at_utc,
        } => occ_dsl::OccurrenceLifecycleInput::DispatchAccepted {
            admission_outcome: to_dsl_delivery_admission_outcome(*admission_outcome),
            at_utc_ms: occurrence_datetime_to_millis(*at_utc, "at_utc")?,
        },
        OccurrenceLifecycleInput::AwaitCompletion { at_utc } => {
            occ_dsl::OccurrenceLifecycleInput::AwaitCompletion {
                at_utc_ms: occurrence_datetime_to_millis(*at_utc, "at_utc")?,
            }
        }
        OccurrenceLifecycleInput::Complete { at_utc } => {
            occ_dsl::OccurrenceLifecycleInput::Complete {
                at_utc_ms: occurrence_datetime_to_millis(*at_utc, "at_utc")?,
            }
        }
        OccurrenceLifecycleInput::ResolveRuntimeCompletion {
            outcome,
            detail,
            at_utc,
        } => occ_dsl::OccurrenceLifecycleInput::ResolveRuntimeCompletion {
            outcome: to_dsl_runtime_completion_outcome(*outcome),
            detail: detail.clone(),
            at_utc_ms: occurrence_datetime_to_millis(*at_utc, "at_utc")?,
        },
        OccurrenceLifecycleInput::ResolveDeliveryCompletionFailure {
            reason,
            detail,
            at_utc,
        } => occ_dsl::OccurrenceLifecycleInput::ResolveDeliveryCompletionFailure {
            reason: to_dsl_delivery_completion_failure_reason(*reason),
            detail: detail.clone(),
            at_utc_ms: occurrence_datetime_to_millis(*at_utc, "at_utc")?,
        },
        OccurrenceLifecycleInput::ResolveDeliveryFailure {
            reason,
            detail,
            at_utc,
        } => occ_dsl::OccurrenceLifecycleInput::ResolveDeliveryFailure {
            reason: to_dsl_delivery_failure_reason(*reason),
            detail: detail.clone(),
            at_utc_ms: occurrence_datetime_to_millis(*at_utc, "at_utc")?,
        },
        OccurrenceLifecycleInput::ResolveTargetProbe {
            outcome,
            redelivery,
            detail,
            at_utc,
        } => occ_dsl::OccurrenceLifecycleInput::ResolveTargetProbe {
            outcome: to_dsl_occurrence_target_probe_outcome(*outcome),
            redelivery: to_dsl_claim_redelivery(*redelivery),
            detail: detail.clone(),
            at_utc_ms: occurrence_datetime_to_millis(*at_utc, "at_utc")?,
        },
        OccurrenceLifecycleInput::ResolveDueMisfire { detail, at_utc } => {
            occ_dsl::OccurrenceLifecycleInput::ResolveDueMisfire {
                detail: detail.clone(),
                at_utc_ms: occurrence_datetime_to_millis(*at_utc, "at_utc")?,
            }
        }
        OccurrenceLifecycleInput::ResolveReclaimExhausted {
            admission_evidence,
            detail,
            at_utc,
        } => occ_dsl::OccurrenceLifecycleInput::ResolveReclaimExhausted {
            admission_evidence: to_dsl_admission_evidence(*admission_evidence),
            detail: detail.clone(),
            at_utc_ms: occurrence_datetime_to_millis(*at_utc, "at_utc")?,
        },
        OccurrenceLifecycleInput::ResolveUnprovenLateNotAdmitted {
            admission_evidence,
            detail,
            at_utc,
        } => occ_dsl::OccurrenceLifecycleInput::ResolveUnprovenLateNotAdmitted {
            admission_evidence: to_dsl_admission_evidence(*admission_evidence),
            detail: detail.clone(),
            at_utc_ms: occurrence_datetime_to_millis(*at_utc, "at_utc")?,
        },
        OccurrenceLifecycleInput::Supersede {
            superseded_by_revision,
            at_utc,
        } => occ_dsl::OccurrenceLifecycleInput::Supersede {
            superseded_by_revision: superseded_by_revision.0,
            at_utc_ms: occurrence_datetime_to_millis(*at_utc, "at_utc")?,
        },
        OccurrenceLifecycleInput::RenewLease {
            claim_token,
            lease_expires_at_utc,
            at_utc,
        } => occ_dsl::OccurrenceLifecycleInput::RenewLease {
            claim_token: occ_dsl::ClaimToken(claim_token.to_string()),
            lease_expires_at_utc_ms: occurrence_datetime_to_millis(
                *lease_expires_at_utc,
                "lease_expires_at_utc",
            )?,
            at_utc_ms: occurrence_datetime_to_millis(*at_utc, "at_utc")?,
        },
        OccurrenceLifecycleInput::LeaseExpired { at_utc } => {
            occ_dsl::OccurrenceLifecycleInput::LeaseExpired {
                at_utc_ms: occurrence_datetime_to_millis(*at_utc, "at_utc")?,
            }
        }
        OccurrenceLifecycleInput::ReleaseLeaseForPausedSchedule { at_utc } => {
            occ_dsl::OccurrenceLifecycleInput::ReleaseLeaseForPausedSchedule {
                at_utc_ms: occurrence_datetime_to_millis(*at_utc, "at_utc")?,
            }
        }
        OccurrenceLifecycleInput::ClassifyStaleCompletionArrival { trigger } => {
            occ_dsl::OccurrenceLifecycleInput::ClassifyStaleCompletionArrival {
                trigger: stale_completion_arrival_trigger_to_dsl(*trigger),
            }
        }
    })
}

/// Map the domain stale-arrival trigger to the occurrence authority's input
/// variant identity (0.7.2 D2a).
fn stale_completion_arrival_trigger_to_dsl(
    trigger: StaleCompletionArrivalTrigger,
) -> occ_dsl::OccurrenceLifecycleInputVariant {
    match trigger {
        StaleCompletionArrivalTrigger::Complete => {
            occ_dsl::OccurrenceLifecycleInputVariant::Complete
        }
        StaleCompletionArrivalTrigger::ResolveRuntimeCompletion => {
            occ_dsl::OccurrenceLifecycleInputVariant::ResolveRuntimeCompletion
        }
        StaleCompletionArrivalTrigger::ResolveDeliveryCompletionFailure => {
            occ_dsl::OccurrenceLifecycleInputVariant::ResolveDeliveryCompletionFailure
        }
        StaleCompletionArrivalTrigger::ResolveDeliveryFailure => {
            occ_dsl::OccurrenceLifecycleInputVariant::ResolveDeliveryFailure
        }
        StaleCompletionArrivalTrigger::Supersede => {
            occ_dsl::OccurrenceLifecycleInputVariant::Supersede
        }
    }
}

/// Mirror the occurrence authority's emitted stale-arrival trigger back into
/// the domain trigger vocabulary. The machine's guard restricts the emitted
/// variant to the completion-shaped subset; any other variant is a projection
/// fault, not a silent coercion.
fn stale_completion_arrival_trigger_from_dsl(
    trigger: occ_dsl::OccurrenceLifecycleInputVariant,
) -> Result<StaleCompletionArrivalTrigger, OccurrenceLifecycleError> {
    match trigger {
        occ_dsl::OccurrenceLifecycleInputVariant::Complete => {
            Ok(StaleCompletionArrivalTrigger::Complete)
        }
        occ_dsl::OccurrenceLifecycleInputVariant::ResolveRuntimeCompletion => {
            Ok(StaleCompletionArrivalTrigger::ResolveRuntimeCompletion)
        }
        occ_dsl::OccurrenceLifecycleInputVariant::ResolveDeliveryCompletionFailure => {
            Ok(StaleCompletionArrivalTrigger::ResolveDeliveryCompletionFailure)
        }
        occ_dsl::OccurrenceLifecycleInputVariant::ResolveDeliveryFailure => {
            Ok(StaleCompletionArrivalTrigger::ResolveDeliveryFailure)
        }
        occ_dsl::OccurrenceLifecycleInputVariant::Supersede => {
            Ok(StaleCompletionArrivalTrigger::Supersede)
        }
        other => Err(OccurrenceLifecycleError::ProjectionMismatch {
            reason: format!(
                "occurrence authority classified non-completion-shaped trigger `{other:?}` as a stale completion arrival"
            ),
        }),
    }
}

/// Mirror the occurrence authority's late-arrival resolution class into the
/// domain vocabulary (0.7.2 D2a).
fn late_completion_resolution_class_from_dsl(
    resolution: occ_dsl::LateCompletionResolutionClass,
) -> LateCompletionResolutionClass {
    match resolution {
        occ_dsl::LateCompletionResolutionClass::DeliveryCompleted => {
            LateCompletionResolutionClass::DeliveryCompleted
        }
        occ_dsl::LateCompletionResolutionClass::RuntimeCompleted => {
            LateCompletionResolutionClass::RuntimeCompleted
        }
        occ_dsl::LateCompletionResolutionClass::RuntimeRejected => {
            LateCompletionResolutionClass::RuntimeRejected
        }
        occ_dsl::LateCompletionResolutionClass::RuntimeTransportError => {
            LateCompletionResolutionClass::RuntimeTransportError
        }
        occ_dsl::LateCompletionResolutionClass::RuntimeInternalError => {
            LateCompletionResolutionClass::RuntimeInternalError
        }
        occ_dsl::LateCompletionResolutionClass::DeliveryCompletionFailed => {
            LateCompletionResolutionClass::DeliveryCompletionFailed
        }
        occ_dsl::LateCompletionResolutionClass::DeliveryFailed => {
            LateCompletionResolutionClass::DeliveryFailed
        }
    }
}

/// Write DSL state back into the domain occurrence.
///
/// The DSL state is the authority for machine-owned fields (phase, target
/// binding key, claimed_by, timestamps, etc). Non-machine fields
/// (trigger_snapshot, policies, created_at_utc) are left untouched since the
/// DSL does not track them.
fn write_back_occurrence(
    dsl: &occ_dsl::OccurrenceLifecycleMachineState,
    occ: &mut Occurrence,
    input: &OccurrenceLifecycleInput,
) -> Result<(), OccurrenceLifecycleError> {
    occ.occurrence_id = occurrence_id_from_dsl(&dsl.occurrence_id)?;
    occ.schedule_id = occurrence_schedule_id_from_dsl(&dsl.schedule_id)?;
    occ.schedule_revision = ScheduleRevision(dsl.schedule_revision);
    occ.occurrence_ordinal = OccurrenceOrdinal(dsl.occurrence_ordinal);
    occ.due_at_utc = millis_to_datetime(dsl.due_at_utc_ms).ok_or(
        OccurrenceLifecycleError::InvalidDueAtUtcMillis {
            millis: dsl.due_at_utc_ms,
        },
    )?;

    // Phase
    occ.phase = occurrence_phase_from_dsl(dsl.lifecycle_phase);

    // Scalar fields
    occ.claimed_by = dsl.claimed_by.clone().map(|owner| owner.0);
    occ.delivery_correlation_id = dsl.delivery_correlation_id.clone().map(|id| id.0);
    occ.failure_detail = dsl.failure_detail.clone();
    occ.attempt_count = attempt_count_from_dsl(dsl.attempt_count)?;

    // Timestamps (u64 millis → DateTime<Utc>)
    occ.lease_expires_at_utc =
        occurrence_optional_datetime_from_dsl(dsl.lease_expires_at_utc_ms, "lease_expires_at_utc")?;
    occ.claimed_at_utc =
        occurrence_optional_datetime_from_dsl(dsl.claimed_at_utc_ms, "claimed_at_utc")?;
    occ.dispatched_at_utc =
        occurrence_optional_datetime_from_dsl(dsl.dispatched_at_utc_ms, "dispatched_at_utc")?;
    occ.completed_at_utc =
        occurrence_optional_datetime_from_dsl(dsl.completed_at_utc_ms, "completed_at_utc")?;

    // Claim token (DSL String → Uuid)
    occ.claim_token = claim_token_from_dsl(&dsl.claim_token)?;

    // Failure class (DSL enum → real enum)
    occ.failure_class = dsl.failure_class.map(from_dsl_failure_class);

    // Superseded revision (DSL u64 → ScheduleRevision)
    occ.superseded_by_revision = dsl.superseded_by_revision.map(ScheduleRevision);
    occ.machine_state = dsl.clone();

    match input {
        OccurrenceLifecycleInput::PlanOccurrence {
            trigger_snapshot,
            target_snapshot,
            misfire_policy,
            overlap_policy,
            missing_target_policy,
            ..
        } => {
            let snapshot_key = target_stable_key(target_snapshot)?;
            if dsl.target_binding_key.0 != snapshot_key {
                return Err(OccurrenceLifecycleError::TargetBindingKeyMismatch {
                    machine_key: dsl.target_binding_key.0.clone(),
                    snapshot_key,
                });
            }
            let target_materialized_session_id = target_materialized_session_id(target_snapshot)
                .map(|session_id| occ_dsl::SessionId(session_id.0.to_string()));
            if dsl.target_materialized_session_id != target_materialized_session_id {
                return Err(OccurrenceLifecycleError::TargetBindingKeyMismatch {
                    machine_key: format!("{:?}", dsl.target_materialized_session_id),
                    snapshot_key: format!("{target_materialized_session_id:?}"),
                });
            }
            occ.target_snapshot = target_snapshot.clone();
            let trigger_key = trigger_stable_key(trigger_snapshot)?;
            if dsl.trigger_key.0 != trigger_key {
                return Err(OccurrenceLifecycleError::TargetBindingKeyMismatch {
                    machine_key: dsl.trigger_key.0.clone(),
                    snapshot_key: trigger_key,
                });
            }
            occ.trigger_snapshot = trigger_snapshot.clone();
            verify_occurrence_policy_keys(
                dsl,
                misfire_policy,
                overlap_policy,
                missing_target_policy,
            )?;
            occ.misfire_policy = misfire_policy.clone();
            occ.overlap_policy = overlap_policy.clone();
            occ.missing_target_policy = missing_target_policy.clone();
        }
        OccurrenceLifecycleInput::SyncTargetSnapshot { target_snapshot } => {
            let snapshot_key = target_stable_key(target_snapshot)?;
            if dsl.target_binding_key.0 != snapshot_key {
                return Err(OccurrenceLifecycleError::TargetBindingKeyMismatch {
                    machine_key: dsl.target_binding_key.0.clone(),
                    snapshot_key,
                });
            }
            let target_materialized_session_id = target_materialized_session_id(target_snapshot)
                .map(|session_id| occ_dsl::SessionId(session_id.0.to_string()));
            if dsl.target_materialized_session_id != target_materialized_session_id {
                return Err(OccurrenceLifecycleError::TargetBindingKeyMismatch {
                    machine_key: format!("{:?}", dsl.target_materialized_session_id),
                    snapshot_key: format!("{target_materialized_session_id:?}"),
                });
            }
            occ.target_snapshot = target_snapshot.clone();
        }
        _ => {}
    }

    if let OccurrenceLifecycleInput::RecordReceipt {
        receipt,
        runtime_outcome,
    } = input
    {
        let receipt = validate_receipt_against_authority(dsl, receipt, runtime_outcome.clone())?;
        occ.last_receipt = Some(receipt);
        occ.runtime_outcome = runtime_outcome.clone();
        occ.machine_state = dsl.clone();
        return Ok(());
    }

    // Lifecycle transitions mint receipt authority for a later RecordReceipt
    // handoff; they do not overwrite the last recorded receipt projection.
    occ.last_receipt = match input {
        OccurrenceLifecycleInput::Claim { .. } => {
            // Generated claim authority starts a fresh attempt.
            occ.runtime_outcome = None;
            None
        }
        OccurrenceLifecycleInput::PlanOccurrence { .. } => {
            occ.runtime_outcome = None;
            None
        }
        _ => {
            // Other transitions don't project a receipt blob. They may mint
            // receipt class authority for a later RecordReceipt handoff.
            occ.last_receipt.take()
        }
    };
    Ok(())
}

/// Map DSL error to the appropriate domain error variant.
fn map_occurrence_error(
    authority: &mut occ_dsl::OccurrenceLifecycleMachineAuthority,
    error: occ_dsl::OccurrenceLifecycleMachineTransitionError,
) -> OccurrenceLifecycleError {
    match classify_occurrence_transition_failure(authority, &error) {
        Ok(public_class) => occurrence_error_from_transition_failure_class(public_class),
        Err(error) => error,
    }
}

fn classify_occurrence_transition_failure(
    authority: &mut occ_dsl::OccurrenceLifecycleMachineAuthority,
    error: &occ_dsl::OccurrenceLifecycleMachineTransitionError,
) -> Result<occ_dsl::OccurrenceTransitionFailureClassKind, OccurrenceLifecycleError> {
    let (refusal_kind, trigger) = occurrence_transition_refusal_evidence(error)
        .map_err(|source| OccurrenceLifecycleError::AuthorityRecoveryRejected { source })?;
    let phase = authority.state().lifecycle_phase;
    let transition = occ_dsl::OccurrenceLifecycleMachineMutator::apply(
        authority,
        occ_dsl::OccurrenceLifecycleInput::ClassifyTransitionFailure {
            refusal_kind,
            trigger,
        },
    )
    .map_err(
        |source| OccurrenceLifecycleError::TransitionFailureClassificationRejected { source },
    )?;

    let mut classified = None;
    for effect in transition.effects() {
        if let occ_dsl::OccurrenceLifecycleEffect::TransitionFailureClassified {
            phase: emitted_phase,
            refusal_kind: emitted_refusal_kind,
            trigger: emitted_trigger,
            public_class,
        } = effect
        {
            if *emitted_phase != phase
                || *emitted_refusal_kind != refusal_kind
                || *emitted_trigger != trigger
            {
                return Err(
                    OccurrenceLifecycleError::TransitionFailureClassificationMismatch {
                        phase: *emitted_phase,
                        expected_refusal_kind: refusal_kind,
                        actual_refusal_kind: *emitted_refusal_kind,
                        expected: trigger,
                        actual: *emitted_trigger,
                    },
                );
            }
            if classified.replace(*public_class).is_some() {
                return Err(OccurrenceLifecycleError::TransitionFailureClassificationAmbiguous);
            }
        }
    }

    classified.ok_or(OccurrenceLifecycleError::TransitionFailureClassificationMissing)
}

fn occurrence_transition_refusal_evidence(
    error: &occ_dsl::OccurrenceLifecycleMachineTransitionError,
) -> Result<
    (
        occ_dsl::OccurrenceTransitionFailureRefusalKind,
        occ_dsl::OccurrenceLifecycleInputVariant,
    ),
    occ_dsl::OccurrenceLifecycleMachineTransitionError,
> {
    match error {
        occ_dsl::OccurrenceLifecycleMachineTransitionError::NoMatchingTransition {
            trigger: occ_dsl::OccurrenceLifecycleMachineTransitionTrigger::Input(trigger),
            ..
        } => Ok((
            occ_dsl::OccurrenceTransitionFailureRefusalKind::NoMatchingTransition,
            *trigger,
        )),
        occ_dsl::OccurrenceLifecycleMachineTransitionError::GuardRejected {
            trigger: occ_dsl::OccurrenceLifecycleMachineTransitionTrigger::Input(trigger),
            ..
        } => Ok((
            occ_dsl::OccurrenceTransitionFailureRefusalKind::GuardRejected,
            *trigger,
        )),
        occ_dsl::OccurrenceLifecycleMachineTransitionError::RecoveredStateInvariantRejected {
            ..
        }
        | occ_dsl::OccurrenceLifecycleMachineTransitionError::AbsentMapKey { .. } => {
            Err(error.clone())
        }
    }
}

fn occurrence_error_from_transition_failure_class(
    public_class: occ_dsl::OccurrenceTransitionFailureClassKind,
) -> OccurrenceLifecycleError {
    match public_class {
        occ_dsl::OccurrenceTransitionFailureClassKind::PlanRejected => {
            OccurrenceLifecycleError::PlanRejected
        }
        occ_dsl::OccurrenceTransitionFailureClassKind::TargetSyncRejected => {
            OccurrenceLifecycleError::TargetSyncRejected
        }
        occ_dsl::OccurrenceTransitionFailureClassKind::ReceiptRecordRejected => {
            OccurrenceLifecycleError::ReceiptRecordRejected
        }
        occ_dsl::OccurrenceTransitionFailureClassKind::DueClassificationRejected => {
            OccurrenceLifecycleError::DueClassificationRejected
        }
        occ_dsl::OccurrenceTransitionFailureClassKind::ClaimedDispatchClassificationRejected => {
            OccurrenceLifecycleError::ClaimedDispatchClassificationRejected
        }
        occ_dsl::OccurrenceTransitionFailureClassKind::CompletionSupersessionClassificationRejected => {
            OccurrenceLifecycleError::CompletionSupersessionClassificationRejected
        }
        occ_dsl::OccurrenceTransitionFailureClassKind::ClaimRejected => {
            OccurrenceLifecycleError::ClaimRejected
        }
        occ_dsl::OccurrenceTransitionFailureClassKind::NotPendingForClaim => {
            OccurrenceLifecycleError::NotPendingForClaim
        }
        occ_dsl::OccurrenceTransitionFailureClassKind::NotClaimed => {
            OccurrenceLifecycleError::NotClaimed
        }
        occ_dsl::OccurrenceTransitionFailureClassKind::NotDispatching => {
            OccurrenceLifecycleError::NotDispatching
        }
        occ_dsl::OccurrenceTransitionFailureClassKind::NotLeaseHolding => {
            OccurrenceLifecycleError::NotLeaseHolding
        }
        occ_dsl::OccurrenceTransitionFailureClassKind::NotLiveForTerminal => {
            OccurrenceLifecycleError::NotLiveForTerminal
        }
        occ_dsl::OccurrenceTransitionFailureClassKind::StaleCompletionArrivalClassificationRejected => {
            OccurrenceLifecycleError::StaleCompletionArrivalClassificationRejected
        }
    }
}

fn to_dsl_delivery_completion_failure_reason(
    reason: DeliveryCompletionFailureReason,
) -> occ_dsl::DeliveryCompletionFailureReason {
    match reason {
        DeliveryCompletionFailureReason::CompletionFutureFailed => {
            occ_dsl::DeliveryCompletionFailureReason::CompletionFutureFailed
        }
        DeliveryCompletionFailureReason::RuntimeCompletionChannelClosed => {
            occ_dsl::DeliveryCompletionFailureReason::RuntimeCompletionChannelClosed
        }
        DeliveryCompletionFailureReason::RuntimeCompletionAuthorityUnavailable => {
            occ_dsl::DeliveryCompletionFailureReason::RuntimeCompletionAuthorityUnavailable
        }
        DeliveryCompletionFailureReason::RuntimeCompletionHandleMissing => {
            occ_dsl::DeliveryCompletionFailureReason::RuntimeCompletionHandleMissing
        }
    }
}

fn to_dsl_delivery_failure_reason(reason: DeliveryFailureReason) -> occ_dsl::DeliveryFailureReason {
    match reason {
        DeliveryFailureReason::TargetMaterializationFailed => {
            occ_dsl::DeliveryFailureReason::TargetMaterializationFailed
        }
        DeliveryFailureReason::TargetMissing => occ_dsl::DeliveryFailureReason::TargetMissing,
        DeliveryFailureReason::TargetBusy => occ_dsl::DeliveryFailureReason::TargetBusy,
        DeliveryFailureReason::RuntimeRejected => occ_dsl::DeliveryFailureReason::RuntimeRejected,
        DeliveryFailureReason::MobRejected => occ_dsl::DeliveryFailureReason::MobRejected,
        DeliveryFailureReason::TransportError => occ_dsl::DeliveryFailureReason::TransportError,
        DeliveryFailureReason::InternalError => occ_dsl::DeliveryFailureReason::InternalError,
        DeliveryFailureReason::ScheduleMandateUnavailable => {
            occ_dsl::DeliveryFailureReason::ScheduleMandateUnavailable
        }
    }
}

fn to_dsl_admission_evidence(evidence: AdmissionEvidence) -> occ_dsl::AdmissionEvidence {
    match evidence {
        AdmissionEvidence::None => occ_dsl::AdmissionEvidence::Unrecorded,
        AdmissionEvidence::Started => occ_dsl::AdmissionEvidence::Started,
        AdmissionEvidence::Accepted => occ_dsl::AdmissionEvidence::Accepted,
    }
}

fn to_dsl_occurrence_target_probe_outcome(
    outcome: OccurrenceTargetProbeOutcome,
) -> occ_dsl::OccurrenceTargetProbeOutcome {
    match outcome {
        OccurrenceTargetProbeOutcome::Ready => occ_dsl::OccurrenceTargetProbeOutcome::Ready,
        OccurrenceTargetProbeOutcome::Busy => occ_dsl::OccurrenceTargetProbeOutcome::Busy,
        OccurrenceTargetProbeOutcome::Missing => occ_dsl::OccurrenceTargetProbeOutcome::Missing,
    }
}

fn to_dsl_claim_redelivery(
    redelivery: Option<crate::driver::ExistingDelivery>,
) -> occ_dsl::ClaimRedelivery {
    match redelivery {
        None => occ_dsl::ClaimRedelivery::Ordinary,
        Some(crate::driver::ExistingDelivery::Admitted) => occ_dsl::ClaimRedelivery::AfterAdmission,
        Some(crate::driver::ExistingDelivery::UnprovenLate) => {
            occ_dsl::ClaimRedelivery::AfterUnprovenDispatch
        }
    }
}

fn to_dsl_claimed_dispatch_schedule_phase(
    phase: SchedulePhase,
) -> occ_dsl::ClaimedDispatchSchedulePhase {
    match phase {
        SchedulePhase::Active => occ_dsl::ClaimedDispatchSchedulePhase::Active,
        SchedulePhase::Paused => occ_dsl::ClaimedDispatchSchedulePhase::Paused,
        SchedulePhase::Deleted => occ_dsl::ClaimedDispatchSchedulePhase::Deleted,
    }
}

fn claimed_dispatch_disposition_from_dsl(
    disposition: occ_dsl::ClaimedDispatchDisposition,
) -> ClaimedDispatchDisposition {
    match disposition {
        occ_dsl::ClaimedDispatchDisposition::Frozen => ClaimedDispatchDisposition::Frozen,
        occ_dsl::ClaimedDispatchDisposition::Supersede => ClaimedDispatchDisposition::Supersede,
        occ_dsl::ClaimedDispatchDisposition::Ready => ClaimedDispatchDisposition::Ready,
        occ_dsl::ClaimedDispatchDisposition::FutureRevision => {
            ClaimedDispatchDisposition::FutureRevision
        }
    }
}

fn completion_supersession_disposition_from_dsl(
    disposition: occ_dsl::CompletionSupersessionDisposition,
) -> CompletionSupersessionDisposition {
    match disposition {
        occ_dsl::CompletionSupersessionDisposition::Supersede => {
            CompletionSupersessionDisposition::Supersede
        }
        occ_dsl::CompletionSupersessionDisposition::Proceed => {
            CompletionSupersessionDisposition::Proceed
        }
        occ_dsl::CompletionSupersessionDisposition::AlreadySuperseded => {
            CompletionSupersessionDisposition::AlreadySuperseded
        }
    }
}

/// Map DSL effect → domain effect (1:1 by name).
fn map_occurrence_effect(
    effect: &occ_dsl::OccurrenceLifecycleEffect,
) -> Result<OccurrenceLifecycleEffect, OccurrenceLifecycleError> {
    Ok(match effect {
        occ_dsl::OccurrenceLifecycleEffect::Claimed => OccurrenceLifecycleEffect::Claimed,
        occ_dsl::OccurrenceLifecycleEffect::DispatchStarted => {
            OccurrenceLifecycleEffect::DispatchStarted
        }
        occ_dsl::OccurrenceLifecycleEffect::DispatchAccepted => {
            OccurrenceLifecycleEffect::DispatchAccepted
        }
        occ_dsl::OccurrenceLifecycleEffect::AwaitingCompletion => {
            OccurrenceLifecycleEffect::AwaitingCompletion
        }
        occ_dsl::OccurrenceLifecycleEffect::Completed => OccurrenceLifecycleEffect::Completed,
        occ_dsl::OccurrenceLifecycleEffect::Skipped => OccurrenceLifecycleEffect::Skipped,
        occ_dsl::OccurrenceLifecycleEffect::Misfired => OccurrenceLifecycleEffect::Misfired,
        occ_dsl::OccurrenceLifecycleEffect::Superseded => OccurrenceLifecycleEffect::Superseded,
        occ_dsl::OccurrenceLifecycleEffect::OccurrencesSuperseded {
            occurrence_id,
            superseding_revision,
        } => OccurrenceLifecycleEffect::OccurrencesSuperseded {
            occurrence_id: occurrence_id_from_dsl(occurrence_id)?,
            superseding_revision: ScheduleRevision(*superseding_revision),
        },
        occ_dsl::OccurrenceLifecycleEffect::DueNoAction => OccurrenceLifecycleEffect::DueNoAction,
        occ_dsl::OccurrenceLifecycleEffect::DueClaimEligible => {
            OccurrenceLifecycleEffect::DueClaimEligible
        }
        occ_dsl::OccurrenceLifecycleEffect::DueMisfireRequired => {
            OccurrenceLifecycleEffect::DueMisfireRequired
        }
        occ_dsl::OccurrenceLifecycleEffect::DueLeaseExpired => {
            OccurrenceLifecycleEffect::DueLeaseExpired
        }
        occ_dsl::OccurrenceLifecycleEffect::DueReclaimExhausted => {
            OccurrenceLifecycleEffect::DueReclaimExhausted
        }
        occ_dsl::OccurrenceLifecycleEffect::ReclaimedAfterAdmission => {
            OccurrenceLifecycleEffect::ReclaimedAfterAdmission
        }
        occ_dsl::OccurrenceLifecycleEffect::ReclaimedAfterUnprovenDispatch => {
            OccurrenceLifecycleEffect::ReclaimedAfterUnprovenDispatch
        }
        occ_dsl::OccurrenceLifecycleEffect::OccurrenceTerminalityClassified { terminal } => {
            OccurrenceLifecycleEffect::OccurrenceTerminalityClassified {
                terminal: *terminal,
            }
        }
        occ_dsl::OccurrenceLifecycleEffect::ClaimedDispatchDispositionClassified {
            disposition,
            superseded_by_revision,
        } => OccurrenceLifecycleEffect::ClaimedDispatchDispositionClassified {
            disposition: claimed_dispatch_disposition_from_dsl(*disposition),
            superseded_by_revision: superseded_by_revision.map(ScheduleRevision),
        },
        occ_dsl::OccurrenceLifecycleEffect::CompletionSupersessionClassified {
            disposition,
            superseded_by_revision,
        } => OccurrenceLifecycleEffect::CompletionSupersessionClassified {
            disposition: completion_supersession_disposition_from_dsl(*disposition),
            superseded_by_revision: superseded_by_revision.map(ScheduleRevision),
        },
        occ_dsl::OccurrenceLifecycleEffect::DeliveryFailed => {
            OccurrenceLifecycleEffect::DeliveryFailed
        }
        occ_dsl::OccurrenceLifecycleEffect::LeaseExpired => OccurrenceLifecycleEffect::LeaseExpired,
        occ_dsl::OccurrenceLifecycleEffect::LeaseRenewed => OccurrenceLifecycleEffect::LeaseRenewed,
        occ_dsl::OccurrenceLifecycleEffect::LateCompletionResolutionRecorded { resolution } => {
            OccurrenceLifecycleEffect::LateCompletionResolutionRecorded {
                resolution: late_completion_resolution_class_from_dsl(*resolution),
            }
        }
        occ_dsl::OccurrenceLifecycleEffect::StaleCompletionArrivalClassified { phase, trigger } => {
            OccurrenceLifecycleEffect::StaleCompletionArrivalClassified {
                phase: occurrence_phase_from_dsl(*phase),
                trigger: stale_completion_arrival_trigger_from_dsl(*trigger)?,
            }
        }
        occ_dsl::OccurrenceLifecycleEffect::TransitionFailureClassified { .. } => {
            return Err(OccurrenceLifecycleError::UnexpectedTransitionFailureClassificationEffect);
        }
    })
}

fn due_action_from_effects(
    effects: &[OccurrenceLifecycleEffect],
) -> Result<Option<OccurrenceDueAction>, OccurrenceLifecycleError> {
    let mut classifications = effects.iter().filter_map(|effect| match effect {
        OccurrenceLifecycleEffect::DueNoAction => Some(("DueNoAction", None)),
        OccurrenceLifecycleEffect::DueClaimEligible => {
            Some(("DueClaimEligible", Some(OccurrenceDueAction::ClaimEligible)))
        }
        OccurrenceLifecycleEffect::DueMisfireRequired => Some((
            "DueMisfireRequired",
            Some(OccurrenceDueAction::MisfireRequired),
        )),
        OccurrenceLifecycleEffect::DueLeaseExpired => {
            Some(("DueLeaseExpired", Some(OccurrenceDueAction::LeaseExpired)))
        }
        OccurrenceLifecycleEffect::DueReclaimExhausted => Some((
            "DueReclaimExhausted",
            Some(OccurrenceDueAction::ReclaimExhausted),
        )),
        _ => None,
    });

    let Some((first_name, first_action)) = classifications.next() else {
        return Err(OccurrenceLifecycleError::AmbiguousDueClassification {
            effects: Vec::new(),
        });
    };
    if let Some((second_name, _)) = classifications.next() {
        return Err(OccurrenceLifecycleError::AmbiguousDueClassification {
            effects: vec![first_name, second_name],
        });
    }
    Ok(first_action)
}

fn terminality_from_effects(
    effects: &[OccurrenceLifecycleEffect],
) -> Result<bool, OccurrenceLifecycleError> {
    let mut verdicts = effects.iter().filter_map(|effect| match effect {
        OccurrenceLifecycleEffect::OccurrenceTerminalityClassified { terminal } => Some(*terminal),
        _ => None,
    });
    let Some(terminal) = verdicts.next() else {
        return Err(OccurrenceLifecycleError::MissingTerminalityClassification);
    };
    if verdicts.next().is_some() {
        return Err(OccurrenceLifecycleError::AmbiguousTerminalityClassification);
    }
    Ok(terminal)
}

fn claimed_dispatch_verdict_from_effects(
    effects: &[OccurrenceLifecycleEffect],
) -> Result<ClaimedDispatchVerdict, OccurrenceLifecycleError> {
    let mut verdicts = effects.iter().filter_map(|effect| match effect {
        OccurrenceLifecycleEffect::ClaimedDispatchDispositionClassified {
            disposition,
            superseded_by_revision,
        } => Some(ClaimedDispatchVerdict {
            disposition: *disposition,
            superseded_by_revision: *superseded_by_revision,
        }),
        _ => None,
    });

    let Some(verdict) = verdicts.next() else {
        return Err(OccurrenceLifecycleError::MissingClaimedDispatchDisposition);
    };
    if verdicts.next().is_some() {
        return Err(OccurrenceLifecycleError::AmbiguousClaimedDispatchDisposition);
    }
    if matches!(verdict.disposition, ClaimedDispatchDisposition::Supersede)
        && verdict.superseded_by_revision.is_none()
    {
        return Err(OccurrenceLifecycleError::ClaimedDispatchSupersedeMissingRevision);
    }
    Ok(verdict)
}

fn completion_supersession_verdict_from_effects(
    effects: &[OccurrenceLifecycleEffect],
) -> Result<CompletionSupersessionVerdict, OccurrenceLifecycleError> {
    let mut verdicts = effects.iter().filter_map(|effect| match effect {
        OccurrenceLifecycleEffect::CompletionSupersessionClassified {
            disposition,
            superseded_by_revision,
        } => Some(CompletionSupersessionVerdict {
            disposition: *disposition,
            superseded_by_revision: *superseded_by_revision,
        }),
        _ => None,
    });

    let Some(verdict) = verdicts.next() else {
        return Err(OccurrenceLifecycleError::MissingCompletionSupersessionDisposition);
    };
    if verdicts.next().is_some() {
        return Err(OccurrenceLifecycleError::AmbiguousCompletionSupersessionDisposition);
    }
    if matches!(
        verdict.disposition,
        CompletionSupersessionDisposition::Supersede
    ) && verdict.superseded_by_revision.is_none()
    {
        return Err(OccurrenceLifecycleError::CompletionSupersessionSupersedeMissingRevision);
    }
    Ok(verdict)
}

// ===========================================================================
// Schedule::apply — DSL-backed lifecycle transition on the domain type
// ===========================================================================

impl Schedule {
    /// Apply a lifecycle input. `Create` constructs a fresh schedule (self is
    /// ignored); all other inputs recover generated machine state from the
    /// schedule's persisted authority snapshot after validating compatibility
    /// projections.
    pub fn apply(
        schedule: Option<Schedule>,
        input: ScheduleLifecycleInput,
    ) -> Result<ScheduleLifecycleMutator, ScheduleLifecycleError> {
        match input {
            // Create is special: constructs a fresh schedule, no prior state to project
            ScheduleLifecycleInput::Create(request) => create_schedule_via_dsl(request),

            // Update mixes config changes (name, labels, horizons) with
            // revision-affecting changes (trigger, target, policies). The DSL
            // handles only the machine-owned Revise transition.
            ScheduleLifecycleInput::Update { request, at_utc } => {
                let mut schedule = schedule.ok_or(ScheduleLifecycleError::MissingSchedule)?;
                let write_precondition = ScheduleWritePrecondition::matches(&schedule)?;
                let (revision_bumped, effects) = apply_update(&mut schedule, request, at_utc)?;
                Ok(schedule_lifecycle_mutator(
                    schedule,
                    effects,
                    revision_bumped,
                    write_precondition,
                ))
            }

            // Remaining inputs go through the DSL
            ScheduleLifecycleInput::RecordPlanningWindow {
                planning_cursor_utc,
                next_occurrence_ordinal,
            } => {
                let mut schedule = schedule.ok_or(ScheduleLifecycleError::MissingSchedule)?;
                let write_precondition = ScheduleWritePrecondition::matches(&schedule)?;
                let dsl_input = sched_dsl::ScheduleLifecycleInput::RecordPlanningWindow {
                    planning_cursor_utc_ms: schedule_datetime_to_millis(
                        planning_cursor_utc,
                        "planning_cursor_utc",
                    )?,
                    next_occurrence_ordinal: next_occurrence_ordinal.0,
                };
                let (transition, dsl_state) = run_schedule_dsl(&schedule, dsl_input)?;
                write_back_schedule(&dsl_state, &mut schedule)?;
                let effects = map_schedule_effects(&transition)?;
                Ok(schedule_lifecycle_mutator(
                    schedule,
                    effects,
                    false,
                    write_precondition,
                ))
            }

            ScheduleLifecycleInput::SyncTargetSnapshot { target } => {
                let mut schedule = schedule.ok_or(ScheduleLifecycleError::MissingSchedule)?;
                let write_precondition = ScheduleWritePrecondition::matches(&schedule)?;
                let target_binding_key = target_stable_key(&target)?;
                let dsl_input = sched_dsl::ScheduleLifecycleInput::SyncTargetSnapshot {
                    target_binding_key: target_binding_key.clone().into(),
                };
                let (transition, dsl_state) = run_schedule_dsl(&schedule, dsl_input)?;
                if dsl_state.target_binding_key.0 != target_binding_key {
                    return Err(ScheduleLifecycleError::TargetBindingKeyMismatch {
                        machine_key: dsl_state.target_binding_key.0.clone(),
                        snapshot_key: target_binding_key,
                    });
                }
                write_back_schedule(&dsl_state, &mut schedule)?;
                schedule.target = target;
                schedule.touch();
                let effects = map_schedule_effects(&transition)?;
                Ok(schedule_lifecycle_mutator(
                    schedule,
                    effects,
                    false,
                    write_precondition,
                ))
            }

            ScheduleLifecycleInput::Pause { at_utc } => {
                let mut schedule = schedule.ok_or(ScheduleLifecycleError::MissingSchedule)?;
                let write_precondition = ScheduleWritePrecondition::matches(&schedule)?;
                let dsl_input = sched_dsl::ScheduleLifecycleInput::Pause {
                    at_utc_ms: schedule_datetime_to_millis(at_utc, "at_utc")?,
                };
                let (transition, dsl_state) = run_schedule_dsl(&schedule, dsl_input)?;
                write_back_schedule(&dsl_state, &mut schedule)?;
                schedule.config.updated_at_utc = at_utc;
                let effects = map_schedule_effects(&transition)?;
                Ok(schedule_lifecycle_mutator(
                    schedule,
                    effects,
                    false,
                    write_precondition,
                ))
            }

            ScheduleLifecycleInput::Resume { at_utc } => {
                let mut schedule = schedule.ok_or(ScheduleLifecycleError::MissingSchedule)?;
                let write_precondition = ScheduleWritePrecondition::matches(&schedule)?;
                let dsl_input = sched_dsl::ScheduleLifecycleInput::Resume {
                    at_utc_ms: schedule_datetime_to_millis(at_utc, "at_utc")?,
                };
                let (transition, dsl_state) = run_schedule_dsl(&schedule, dsl_input)?;
                write_back_schedule(&dsl_state, &mut schedule)?;
                schedule.config.updated_at_utc = at_utc;
                let effects = map_schedule_effects(&transition)?;
                Ok(schedule_lifecycle_mutator(
                    schedule,
                    effects,
                    false,
                    write_precondition,
                ))
            }

            ScheduleLifecycleInput::Delete { at_utc } => {
                let mut schedule = schedule.ok_or(ScheduleLifecycleError::MissingSchedule)?;
                let write_precondition = ScheduleWritePrecondition::matches(&schedule)?;
                let dsl_input = sched_dsl::ScheduleLifecycleInput::Delete {
                    at_utc_ms: schedule_datetime_to_millis(at_utc, "at_utc")?,
                };
                let old_revision = schedule.revision;
                let (transition, dsl_state) = run_schedule_dsl(&schedule, dsl_input)?;
                write_back_schedule(&dsl_state, &mut schedule)?;
                schedule.config.deleted_at_utc = Some(at_utc);
                schedule.config.updated_at_utc = at_utc;
                let revision_bumped = schedule.revision != old_revision;
                let effects = map_schedule_effects(&transition)?;
                Ok(schedule_lifecycle_mutator(
                    schedule,
                    effects,
                    revision_bumped,
                    write_precondition,
                ))
            }

            ScheduleLifecycleInput::ConfirmOccurrencesSuperseded { ack } => {
                let mut schedule = schedule.ok_or(ScheduleLifecycleError::MissingSchedule)?;
                let write_precondition = ScheduleWritePrecondition::matches(&schedule)?;
                if ack.schedule_id != schedule.schedule_id {
                    return Err(ScheduleLifecycleError::ProjectionMismatch {
                        reason: format!(
                            "occurrence supersession feedback for schedule `{}` cannot update schedule `{}`",
                            ack.schedule_id, schedule.schedule_id
                        ),
                    });
                }
                let dsl_input = sched_dsl::ScheduleLifecycleInput::ConfirmOccurrencesSuperseded {
                    occurrence_id: sched_dsl::OccurrenceId(ack.occurrence_id.0.to_string()),
                    superseding_revision: ack.superseding_revision.0,
                };
                let (transition, dsl_state) = run_schedule_dsl(&schedule, dsl_input)?;
                write_back_schedule(&dsl_state, &mut schedule)?;
                let effects = map_schedule_effects(&transition)?;
                Ok(schedule_lifecycle_mutator(
                    schedule,
                    effects,
                    false,
                    write_precondition,
                ))
            }
        }
    }
}

fn create_schedule_via_dsl(
    request: CreateScheduleRequest,
) -> Result<ScheduleLifecycleMutator, ScheduleLifecycleError> {
    let schedule_id = ScheduleId::new();
    let trigger = request.trigger;
    let target = request.target;
    let misfire_policy = request.misfire_policy;
    let overlap_policy = request.overlap_policy;
    let missing_target_policy = request.missing_target_policy;
    let planning_horizon_days = request.planning_horizon_days;
    let planning_horizon_occurrences = request.planning_horizon_occurrences;

    let dsl_input = sched_dsl::ScheduleLifecycleInput::Create {
        schedule_id: sched_dsl::ScheduleId(schedule_id.0.to_string()),
        trigger_key: trigger_stable_key(&trigger)?.into(),
        target_binding_key: target_stable_key(&target)?.into(),
        misfire_policy: to_dsl_misfire_policy(&misfire_policy),
        overlap_policy: to_dsl_overlap_policy(&overlap_policy),
        missing_target_policy: to_dsl_missing_target_policy(&missing_target_policy),
        planning_horizon_days: planning_horizon_days.map(u64::from),
        planning_horizon_occurrences: planning_horizon_occurrences.map(u64::from),
    };
    let mut dsl_auth = sched_dsl::ScheduleLifecycleMachineAuthority::new();
    let transition = sched_dsl::ScheduleLifecycleMachineMutator::apply(&mut dsl_auth, dsl_input)
        .map_err(|source| ScheduleLifecycleError::TransitionRejected { source })?;
    let dsl_state = dsl_auth.state().clone();
    verify_schedule_snapshot_keys(&dsl_state, &trigger, &target)?;

    let now = Utc::now();
    let schedule = Schedule {
        schedule_id: schedule_id_from_dsl(&dsl_state.schedule_id)?,
        phase: schedule_phase_from_dsl(dsl_state.lifecycle_phase),
        revision: ScheduleRevision(dsl_state.revision),
        machine_state: dsl_state.clone(),
        trigger,
        target,
        misfire_policy,
        overlap_policy,
        missing_target_policy,
        next_occurrence_ordinal: OccurrenceOrdinal(dsl_state.next_occurrence_ordinal),
        planning_cursor_utc: schedule_planning_cursor_from_dsl(dsl_state.planning_cursor_utc_ms)?,
        superseded_ack_ids: schedule_superseded_ack_ids_from_dsl(&dsl_state.superseded_ack_ids)?,
        config: crate::types::ScheduleConfig {
            name: request.name,
            description: request.description,
            planning_horizon_days: planning_horizon_days_from_dsl(&dsl_state)?,
            planning_horizon_occurrences: planning_horizon_occurrences_from_dsl(&dsl_state)?,
            labels: request.labels,
            created_at_utc: now,
            updated_at_utc: now,
            deleted_at_utc: None,
            commissioning_agent: None,
        },
    };
    let effects = map_schedule_effects(&transition)?;
    let write_precondition = ScheduleWritePrecondition::absent(schedule.schedule_id.clone());
    Ok(schedule_lifecycle_mutator(
        schedule,
        effects,
        false,
        write_precondition,
    ))
}

/// Apply config-level update changes. If any revision-affecting field changes,
/// delegate the revision bump to the DSL via a Revise input.
fn apply_update(
    schedule: &mut Schedule,
    request: UpdateScheduleRequest,
    at_utc: DateTime<Utc>,
) -> Result<(bool, Vec<ScheduleLifecycleEffect>), ScheduleLifecycleError> {
    if let Some(expected_revision) = &request.expected_revision
        && *expected_revision != schedule.revision
    {
        return Err(ScheduleLifecycleError::RevisionMismatch {
            expected: expected_revision.0,
            actual: schedule.revision.0,
        });
    }

    let next_trigger = request
        .trigger
        .clone()
        .unwrap_or_else(|| schedule.trigger.clone());
    let next_target = request
        .target
        .clone()
        .unwrap_or_else(|| schedule.target.clone());
    let next_misfire_policy = request
        .misfire_policy
        .clone()
        .unwrap_or_else(|| schedule.misfire_policy.clone());
    let next_overlap_policy = request
        .overlap_policy
        .clone()
        .unwrap_or_else(|| schedule.overlap_policy.clone());
    let next_missing_target_policy = request
        .missing_target_policy
        .clone()
        .unwrap_or_else(|| schedule.missing_target_policy.clone());
    let next_planning_horizon_days = request
        .planning_horizon_days
        .unwrap_or(schedule.config.planning_horizon_days);
    let next_planning_horizon_occurrences = request
        .planning_horizon_occurrences
        .unwrap_or(schedule.config.planning_horizon_occurrences);

    let revision_affecting_change = next_trigger != schedule.trigger
        || next_target != schedule.target
        || next_misfire_policy != schedule.misfire_policy
        || next_overlap_policy != schedule.overlap_policy
        || next_missing_target_policy != schedule.missing_target_policy;

    let planning_config_changed = next_planning_horizon_days
        != schedule.config.planning_horizon_days
        || next_planning_horizon_occurrences != schedule.config.planning_horizon_occurrences;

    if revision_affecting_change {
        // Use the DSL Revise transition for the revision bump + planning cursor clear
        let dsl_input = sched_dsl::ScheduleLifecycleInput::Revise {
            trigger_key: trigger_stable_key(&next_trigger)?.into(),
            target_binding_key: target_stable_key(&next_target)?.into(),
            misfire_policy: to_dsl_misfire_policy(&next_misfire_policy),
            overlap_policy: to_dsl_overlap_policy(&next_overlap_policy),
            missing_target_policy: to_dsl_missing_target_policy(&next_missing_target_policy),
            planning_horizon_days: u64::from(next_planning_horizon_days),
            planning_horizon_occurrences: u64::from(next_planning_horizon_occurrences),
            at_utc_ms: schedule_datetime_to_millis(at_utc, "at_utc")?,
        };
        let (transition, dsl_state) = run_schedule_dsl(schedule, dsl_input)?;
        verify_schedule_snapshot_keys(&dsl_state, &next_trigger, &next_target)?;
        write_back_schedule(&dsl_state, schedule)?;
        schedule.trigger = next_trigger;
        schedule.target = next_target;
        schedule.misfire_policy = next_misfire_policy;
        schedule.overlap_policy = next_overlap_policy;
        schedule.missing_target_policy = next_missing_target_policy;
        schedule.touch();
        apply_non_machine_schedule_config(schedule, request);
        return Ok((true, map_schedule_effects(&transition)?));
    }

    let effects = if planning_config_changed {
        let dsl_input = sched_dsl::ScheduleLifecycleInput::UpdatePlanningConfig {
            planning_horizon_days: u64::from(next_planning_horizon_days),
            planning_horizon_occurrences: u64::from(next_planning_horizon_occurrences),
        };
        let (transition, dsl_state) = run_schedule_dsl(schedule, dsl_input)?;
        write_back_schedule(&dsl_state, schedule)?;
        map_schedule_effects(&transition)?
    } else {
        Vec::new()
    };
    schedule.touch();
    apply_non_machine_schedule_config(schedule, request);
    Ok((false, effects))
}

fn apply_non_machine_schedule_config(schedule: &mut Schedule, request: UpdateScheduleRequest) {
    if let Some(name) = request.name {
        schedule.config.name = Some(name);
    }
    if let Some(description) = request.description {
        schedule.config.description = Some(description);
    }
    if let Some(labels) = request.labels {
        schedule.config.labels = labels;
    }
}

fn schedule_phase_from_dsl(phase: sched_dsl::ScheduleLifecycleState) -> SchedulePhase {
    match phase {
        sched_dsl::ScheduleLifecycleState::Active => SchedulePhase::Active,
        sched_dsl::ScheduleLifecycleState::Paused => SchedulePhase::Paused,
        sched_dsl::ScheduleLifecycleState::Deleted => SchedulePhase::Deleted,
    }
}

fn planning_horizon_days_from_dsl(
    dsl: &sched_dsl::ScheduleLifecycleMachineState,
) -> Result<u32, ScheduleLifecycleError> {
    u32::try_from(dsl.planning_horizon_days).map_err(|_| {
        ScheduleLifecycleError::InvalidPlanningHorizonDays {
            value: dsl.planning_horizon_days,
        }
    })
}

fn planning_horizon_occurrences_from_dsl(
    dsl: &sched_dsl::ScheduleLifecycleMachineState,
) -> Result<u32, ScheduleLifecycleError> {
    u32::try_from(dsl.planning_horizon_occurrences).map_err(|_| {
        ScheduleLifecycleError::InvalidPlanningHorizonOccurrences {
            value: dsl.planning_horizon_occurrences,
        }
    })
}

fn verify_schedule_snapshot_keys(
    dsl: &sched_dsl::ScheduleLifecycleMachineState,
    trigger: &TriggerSpec,
    target: &TargetBinding,
) -> Result<(), ScheduleLifecycleError> {
    let trigger_key = trigger_stable_key(trigger)?;
    if dsl.trigger_key.0 != trigger_key {
        return Err(ScheduleLifecycleError::TargetBindingKeyMismatch {
            machine_key: dsl.trigger_key.0.clone(),
            snapshot_key: trigger_key,
        });
    }
    let target_key = target_stable_key(target)?;
    if dsl.target_binding_key.0 != target_key {
        return Err(ScheduleLifecycleError::TargetBindingKeyMismatch {
            machine_key: dsl.target_binding_key.0.clone(),
            snapshot_key: target_key,
        });
    }
    Ok(())
}

fn verify_occurrence_policy_keys(
    dsl: &occ_dsl::OccurrenceLifecycleMachineState,
    misfire_policy: &crate::types::MisfirePolicy,
    overlap_policy: &crate::types::OverlapPolicy,
    missing_target_policy: &crate::types::MissingTargetPolicy,
) -> Result<(), OccurrenceLifecycleError> {
    let misfire_key = misfire_policy_authority_key(misfire_policy)?;
    if dsl.misfire_policy_key != misfire_key {
        return Err(OccurrenceLifecycleError::TargetBindingKeyMismatch {
            machine_key: dsl.misfire_policy_key.clone(),
            snapshot_key: misfire_key,
        });
    }
    let overlap_key = overlap_policy_authority_key(overlap_policy)?;
    if dsl.overlap_policy_key != overlap_key {
        return Err(OccurrenceLifecycleError::TargetBindingKeyMismatch {
            machine_key: dsl.overlap_policy_key.clone(),
            snapshot_key: overlap_key,
        });
    }
    let missing_target_key = missing_target_policy_authority_key(missing_target_policy)?;
    if dsl.missing_target_policy_key != missing_target_key {
        return Err(OccurrenceLifecycleError::TargetBindingKeyMismatch {
            machine_key: dsl.missing_target_policy_key.clone(),
            snapshot_key: missing_target_key,
        });
    }
    Ok(())
}

/// Recover generated schedule state, apply DSL input, return transition + new state.
fn run_schedule_dsl(
    schedule: &Schedule,
    dsl_input: sched_dsl::ScheduleLifecycleInput,
) -> Result<
    (
        sched_dsl::ScheduleLifecycleMachineTransition,
        sched_dsl::ScheduleLifecycleMachineState,
    ),
    ScheduleLifecycleError,
> {
    let dsl_state = schedule_machine_state_for_authority(schedule)?;
    let mut dsl_auth = sched_dsl::ScheduleLifecycleMachineAuthority::recover_from_state(dsl_state)
        .map_err(|source| ScheduleLifecycleError::TransitionRejected { source })?;
    let transition = sched_dsl::ScheduleLifecycleMachineMutator::apply(&mut dsl_auth, dsl_input)
        .map_err(|source| ScheduleLifecycleError::TransitionRejected { source })?;
    Ok((transition, dsl_auth.state().clone()))
}

/// Recover generated schedule state after checking display projections.
fn schedule_machine_state_for_authority(
    sched: &Schedule,
) -> Result<sched_dsl::ScheduleLifecycleMachineState, ScheduleLifecycleError> {
    validate_schedule_machine_projection(sched)
        .map_err(|reason| ScheduleLifecycleError::ProjectionMismatch { reason })?;
    Ok(sched.machine_state.clone())
}

#[cfg(test)]
fn schedule_machine_state_for_test(
    sched: &Schedule,
) -> Result<sched_dsl::ScheduleLifecycleMachineState, ScheduleLifecycleError> {
    schedule_machine_state_for_authority(sched)
}

/// Write DSL state back into the domain schedule (machine-owned fields only).
fn write_back_schedule(
    dsl: &sched_dsl::ScheduleLifecycleMachineState,
    sched: &mut Schedule,
) -> Result<(), ScheduleLifecycleError> {
    sched.schedule_id = schedule_id_from_dsl(&dsl.schedule_id)?;
    sched.phase = match dsl.lifecycle_phase {
        sched_dsl::ScheduleLifecycleState::Active => SchedulePhase::Active,
        sched_dsl::ScheduleLifecycleState::Paused => SchedulePhase::Paused,
        sched_dsl::ScheduleLifecycleState::Deleted => SchedulePhase::Deleted,
    };
    sched.revision = ScheduleRevision(dsl.revision);
    sched.config.planning_horizon_days = planning_horizon_days_from_dsl(dsl)?;
    sched.config.planning_horizon_occurrences = planning_horizon_occurrences_from_dsl(dsl)?;
    sched.planning_cursor_utc = schedule_planning_cursor_from_dsl(dsl.planning_cursor_utc_ms)?;
    sched.next_occurrence_ordinal = OccurrenceOrdinal(dsl.next_occurrence_ordinal);
    sched.superseded_ack_ids = schedule_superseded_ack_ids_from_dsl(&dsl.superseded_ack_ids)?;
    sched.machine_state = dsl.clone();
    Ok(())
}

/// Map DSL schedule effect → domain effect.
fn map_schedule_effect(
    effect: &sched_dsl::ScheduleLifecycleEffect,
) -> Result<ScheduleLifecycleEffect, ScheduleLifecycleError> {
    match effect {
        sched_dsl::ScheduleLifecycleEffect::EmitScheduleNotice {
            new_state,
            revision,
        } => Ok(ScheduleLifecycleEffect::EmitScheduleNotice {
            new_state: match new_state {
                sched_dsl::ScheduleLifecycleState::Active => SchedulePhase::Active,
                sched_dsl::ScheduleLifecycleState::Paused => SchedulePhase::Paused,
                sched_dsl::ScheduleLifecycleState::Deleted => SchedulePhase::Deleted,
            },
            revision: ScheduleRevision(*revision),
        }),
        sched_dsl::ScheduleLifecycleEffect::SupersedePendingOccurrences {
            superseding_revision,
            at_utc_ms,
        } => Ok(ScheduleLifecycleEffect::SupersedePendingOccurrences {
            superseding_revision: ScheduleRevision(*superseding_revision),
            at_utc: millis_to_datetime(*at_utc_ms).ok_or(
                ScheduleLifecycleError::InvalidSupersessionAtUtcMillis { millis: *at_utc_ms },
            )?,
        }),
        sched_dsl::ScheduleLifecycleEffect::PlanningWindowRecorded {
            planning_cursor_utc_ms,
            next_occurrence_ordinal,
        } => Ok(ScheduleLifecycleEffect::PlanningWindowRecorded {
            planning_cursor_utc: millis_to_datetime(*planning_cursor_utc_ms).ok_or(
                ScheduleLifecycleError::InvalidPlanningCursorUtcMillis {
                    millis: *planning_cursor_utc_ms,
                },
            )?,
            next_occurrence_ordinal: OccurrenceOrdinal(*next_occurrence_ordinal),
        }),
    }
}

fn map_schedule_effects(
    transition: &sched_dsl::ScheduleLifecycleMachineTransition,
) -> Result<Vec<ScheduleLifecycleEffect>, ScheduleLifecycleError> {
    transition
        .effects()
        .iter()
        .map(map_schedule_effect)
        .collect()
}

fn schedule_write_fingerprint(schedule: &Schedule) -> Result<Vec<u8>, String> {
    validate_schedule_machine_projection(schedule)?;
    serde_json::to_vec(schedule).map_err(|source| source.to_string())
}

fn occurrence_write_fingerprint(occurrence: &Occurrence) -> Result<Vec<u8>, String> {
    validate_occurrence_machine_projection(occurrence)?;
    serde_json::to_vec(occurrence).map_err(|source| source.to_string())
}

// ===========================================================================
// Conversion helpers
// ===========================================================================

fn semantic_json_key<T: Serialize>(
    fact: &'static str,
    prefix: &'static str,
    value: &T,
) -> Result<String, SemanticKeySerializationError> {
    serde_json::to_string(value)
        .map(|json| format!("{prefix}:{json}"))
        .map_err(|source| SemanticKeySerializationError {
            fact,
            detail: source.to_string(),
        })
}

/// Produce a complete semantic key for a trigger spec. The DSL stores the
/// opaque key, and shell code may persist the full trigger only when the key
/// accepted by generated authority matches this structured serialization.
fn trigger_stable_key(trigger: &TriggerSpec) -> Result<String, SemanticKeySerializationError> {
    semantic_json_key("trigger", "trigger", trigger)
}

fn target_stable_key(target: &TargetBinding) -> Result<String, SemanticKeySerializationError> {
    target
        .stable_key()
        .map_err(|source| SemanticKeySerializationError {
            fact: "target",
            detail: source,
        })
}

fn delivery_receipt_from_authority(
    occurrence: &Occurrence,
    runtime_outcome: Option<RuntimeDeliveryOutcome>,
) -> Result<DeliveryReceipt, OccurrenceLifecycleError> {
    let dsl = occurrence_machine_state_for_authority(occurrence)?;
    receipt_from_pending_authority(&dsl, runtime_outcome)
}

fn receipt_from_pending_authority(
    dsl: &occ_dsl::OccurrenceLifecycleMachineState,
    runtime_outcome: Option<RuntimeDeliveryOutcome>,
) -> Result<DeliveryReceipt, OccurrenceLifecycleError> {
    let stage = dsl
        .receipt_stage
        .map(from_dsl_receipt_stage)
        .ok_or(OccurrenceLifecycleError::MissingReceiptAuthority)?;
    let recorded_at_utc_ms = dsl
        .receipt_recorded_at_utc_ms
        .ok_or(OccurrenceLifecycleError::MissingReceiptAuthority)?;
    let recorded_at_utc = millis_to_datetime(recorded_at_utc_ms).ok_or(
        OccurrenceLifecycleError::InvalidTimestampMillis {
            field: "receipt_recorded_at_utc",
            millis: recorded_at_utc_ms,
        },
    )?;
    let failure_class = dsl.receipt_failure_class.map(from_dsl_failure_class);
    let detail = dsl.receipt_detail.clone();
    let occurrence_id = occurrence_id_from_dsl(&dsl.occurrence_id)?;
    let attempt = attempt_count_from_dsl(dsl.attempt_count)?;
    let materialized_session_id =
        optional_session_id_from_dsl(&dsl.target_materialized_session_id)?;
    let runtime_outcome_key = runtime_outcome
        .as_ref()
        .map(runtime_outcome_authority_key)
        .transpose()?;
    let receipt_id = delivery_receipt_id_from_authority(
        &occurrence_id,
        attempt,
        stage,
        recorded_at_utc_ms,
        dsl.delivery_correlation_id.as_ref().map(|id| id.0.as_str()),
        detail.as_deref(),
        failure_class,
        runtime_outcome_key.as_ref().map(|key| key.0.as_str()),
        materialized_session_id.as_ref(),
    )
    .map_err(|reason| OccurrenceLifecycleError::ProjectionMismatch { reason })?;
    Ok(DeliveryReceipt {
        receipt_id,
        occurrence_id,
        attempt,
        stage,
        recorded_at_utc,
        correlation_id: dsl.delivery_correlation_id.clone().map(|id| id.0),
        detail,
        failure_class,
        runtime_outcome,
        materialized_session_id,
    })
}

fn receipt_from_recorded_authority(
    dsl: &occ_dsl::OccurrenceLifecycleMachineState,
    runtime_outcome: Option<RuntimeDeliveryOutcome>,
) -> Result<DeliveryReceipt, OccurrenceLifecycleError> {
    let stage = dsl
        .last_receipt_stage
        .map(from_dsl_receipt_stage)
        .ok_or(OccurrenceLifecycleError::MissingReceiptAuthority)?;
    let recorded_at_utc_ms = dsl
        .last_receipt_recorded_at_utc_ms
        .ok_or(OccurrenceLifecycleError::MissingReceiptAuthority)?;
    let recorded_at_utc = millis_to_datetime(recorded_at_utc_ms).ok_or(
        OccurrenceLifecycleError::InvalidTimestampMillis {
            field: "last_receipt_recorded_at_utc",
            millis: recorded_at_utc_ms,
        },
    )?;
    let attempt = dsl
        .last_receipt_attempt
        .ok_or(OccurrenceLifecycleError::MissingReceiptAuthority)
        .and_then(attempt_count_from_dsl)?;
    let occurrence_id = occurrence_id_from_dsl(&dsl.occurrence_id)?;
    let failure_class = dsl.last_receipt_failure_class.map(from_dsl_failure_class);
    let detail = dsl.last_receipt_detail.clone();
    let materialized_session_id =
        optional_session_id_from_dsl(&dsl.last_receipt_materialized_session_id)?;
    let runtime_outcome_key = runtime_outcome
        .as_ref()
        .map(runtime_outcome_authority_key)
        .transpose()?;
    if dsl.runtime_outcome_key != runtime_outcome_key {
        return Err(OccurrenceLifecycleError::RuntimeOutcomeKeyMismatch {
            machine_key: dsl.runtime_outcome_key.clone().map(|key| key.0),
            snapshot_key: runtime_outcome_key.map(|key| key.0),
        });
    }
    let receipt_id = delivery_receipt_id_from_authority(
        &occurrence_id,
        attempt,
        stage,
        recorded_at_utc_ms,
        dsl.last_receipt_correlation_id
            .as_ref()
            .map(|id| id.0.as_str()),
        detail.as_deref(),
        failure_class,
        dsl.runtime_outcome_key.as_ref().map(|key| key.0.as_str()),
        materialized_session_id.as_ref(),
    )
    .map_err(|reason| OccurrenceLifecycleError::ProjectionMismatch { reason })?;
    Ok(DeliveryReceipt {
        receipt_id,
        occurrence_id,
        attempt,
        stage,
        recorded_at_utc,
        correlation_id: dsl.last_receipt_correlation_id.clone().map(|id| id.0),
        detail,
        failure_class,
        runtime_outcome,
        materialized_session_id,
    })
}

fn validate_receipt_against_authority(
    dsl: &occ_dsl::OccurrenceLifecycleMachineState,
    supplied: &DeliveryReceipt,
    runtime_outcome: Option<RuntimeDeliveryOutcome>,
) -> Result<DeliveryReceipt, OccurrenceLifecycleError> {
    let expected = receipt_from_recorded_authority(dsl, runtime_outcome)?;
    if supplied.stage != expected.stage {
        return Err(OccurrenceLifecycleError::ReceiptStageMismatch {
            expected: expected.stage,
            actual: supplied.stage,
        });
    }
    if supplied.failure_class != expected.failure_class {
        return Err(OccurrenceLifecycleError::ReceiptFailureClassMismatch {
            expected: expected.failure_class,
            actual: supplied.failure_class,
        });
    }
    if supplied.detail != expected.detail {
        return Err(OccurrenceLifecycleError::ReceiptDetailMismatch {
            expected: expected.detail,
            actual: supplied.detail.clone(),
        });
    }
    if supplied != &expected {
        return Err(OccurrenceLifecycleError::ReceiptRecordMismatch {
            expected: Box::new(expected),
            actual: Box::new(supplied.clone()),
        });
    }
    Ok(expected)
}

fn from_dsl_receipt_stage(stage: occ_dsl::DeliveryReceiptStage) -> DeliveryReceiptStage {
    match stage {
        occ_dsl::DeliveryReceiptStage::Planned => DeliveryReceiptStage::Planned,
        occ_dsl::DeliveryReceiptStage::Claimed => DeliveryReceiptStage::Claimed,
        occ_dsl::DeliveryReceiptStage::DispatchStarted => DeliveryReceiptStage::DispatchStarted,
        occ_dsl::DeliveryReceiptStage::DispatchAccepted => DeliveryReceiptStage::DispatchAccepted,
        occ_dsl::DeliveryReceiptStage::AwaitingCompletion => {
            DeliveryReceiptStage::AwaitingCompletion
        }
        occ_dsl::DeliveryReceiptStage::Completed => DeliveryReceiptStage::Completed,
        occ_dsl::DeliveryReceiptStage::Skipped => DeliveryReceiptStage::Skipped,
        occ_dsl::DeliveryReceiptStage::Misfired => DeliveryReceiptStage::Misfired,
        occ_dsl::DeliveryReceiptStage::Superseded => DeliveryReceiptStage::Superseded,
        occ_dsl::DeliveryReceiptStage::DeliveryFailed => DeliveryReceiptStage::DeliveryFailed,
        occ_dsl::DeliveryReceiptStage::LeaseExpired => DeliveryReceiptStage::LeaseExpired,
    }
}

fn runtime_outcome_authority_key(
    outcome: &RuntimeDeliveryOutcome,
) -> Result<occ_dsl::RuntimeOutcomeKey, SemanticKeySerializationError> {
    semantic_json_key("runtime_outcome", "runtime_outcome", outcome)
        .map(occ_dsl::RuntimeOutcomeKey::from)
}

fn misfire_policy_authority_key(
    policy: &crate::types::MisfirePolicy,
) -> Result<String, SemanticKeySerializationError> {
    semantic_json_key("misfire_policy", "misfire_policy", policy)
}

fn overlap_policy_authority_key(
    policy: &crate::types::OverlapPolicy,
) -> Result<String, SemanticKeySerializationError> {
    semantic_json_key("overlap_policy", "overlap_policy", policy)
}

fn missing_target_policy_authority_key(
    policy: &crate::types::MissingTargetPolicy,
) -> Result<String, SemanticKeySerializationError> {
    semantic_json_key("missing_target_policy", "missing_target_policy", policy)
}

fn datetime_to_millis(dt: DateTime<Utc>) -> Result<u64, i64> {
    let millis = dt.timestamp_millis();
    u64::try_from(millis).map_err(|_| millis)
}

fn schedule_datetime_to_millis(
    dt: DateTime<Utc>,
    field: &'static str,
) -> Result<u64, ScheduleLifecycleError> {
    datetime_to_millis(dt)
        .map_err(|millis| ScheduleLifecycleError::InvalidTimestampMillis { field, millis })
}

fn occurrence_datetime_to_millis(
    dt: DateTime<Utc>,
    field: &'static str,
) -> Result<u64, OccurrenceLifecycleError> {
    datetime_to_millis(dt)
        .map_err(|millis| OccurrenceLifecycleError::InvalidInputTimestampMillis { field, millis })
}

fn occurrence_misfire_deadline_to_millis(
    policy: &crate::types::MisfirePolicy,
    due_at_utc: DateTime<Utc>,
) -> Result<u64, OccurrenceLifecycleError> {
    let deadline = policy.misfire_deadline_utc(due_at_utc).ok_or(
        OccurrenceLifecycleError::InvalidInputTimestampMillis {
            field: "misfire_deadline_utc",
            millis: due_at_utc.timestamp_millis(),
        },
    )?;
    occurrence_datetime_to_millis(deadline, "misfire_deadline_utc")
}

fn millis_to_datetime(ms: u64) -> Option<DateTime<Utc>> {
    let ms_i64 = i64::try_from(ms).ok()?;
    DateTime::from_timestamp_millis(ms_i64)
}

fn schedule_planning_cursor_from_dsl(
    ms: Option<u64>,
) -> Result<Option<DateTime<Utc>>, ScheduleLifecycleError> {
    ms.map(|millis| {
        millis_to_datetime(millis)
            .ok_or(ScheduleLifecycleError::InvalidPlanningCursorUtcMillis { millis })
    })
    .transpose()
}

fn schedule_superseded_ack_ids_from_dsl(
    ids: &BTreeSet<sched_dsl::OccurrenceId>,
) -> Result<BTreeSet<crate::types::OccurrenceId>, ScheduleLifecycleError> {
    ids.iter()
        .map(|id| {
            crate::types::OccurrenceId::parse(&id.0).map_err(|source| {
                ScheduleLifecycleError::InvalidSupersededAckId {
                    id: id.0.clone(),
                    source,
                }
            })
        })
        .collect()
}

fn occurrence_optional_datetime_from_dsl(
    ms: Option<u64>,
    field: &'static str,
) -> Result<Option<DateTime<Utc>>, OccurrenceLifecycleError> {
    ms.map(|millis| {
        millis_to_datetime(millis)
            .ok_or(OccurrenceLifecycleError::InvalidTimestampMillis { field, millis })
    })
    .transpose()
}

fn claim_token_from_dsl(
    token: &Option<occ_dsl::ClaimToken>,
) -> Result<Option<Uuid>, OccurrenceLifecycleError> {
    token
        .as_ref()
        .map(|t| {
            Uuid::parse_str(&t.0).map_err(|source| OccurrenceLifecycleError::InvalidClaimToken {
                token: t.0.clone(),
                source,
            })
        })
        .transpose()
}

fn optional_session_id_from_dsl(
    session_id: &Option<occ_dsl::SessionId>,
) -> Result<Option<SessionId>, OccurrenceLifecycleError> {
    session_id
        .as_ref()
        .map(|id| {
            SessionId::parse(&id.0).map_err(|source| OccurrenceLifecycleError::InvalidSessionId {
                id: id.0.clone(),
                source,
            })
        })
        .transpose()
}

fn attempt_count_from_dsl(value: u64) -> Result<u32, OccurrenceLifecycleError> {
    u32::try_from(value).map_err(|_| OccurrenceLifecycleError::InvalidAttemptCount { value })
}

fn to_dsl_runtime_completion_outcome(
    outcome: RuntimeCompletionOutcome,
) -> occ_dsl::RuntimeCompletionOutcome {
    match outcome {
        RuntimeCompletionOutcome::Completed => occ_dsl::RuntimeCompletionOutcome::Completed,
        RuntimeCompletionOutcome::CallbackPending => {
            occ_dsl::RuntimeCompletionOutcome::CallbackPending
        }
        RuntimeCompletionOutcome::Cancelled => occ_dsl::RuntimeCompletionOutcome::Cancelled,
        RuntimeCompletionOutcome::Abandoned => occ_dsl::RuntimeCompletionOutcome::Abandoned,
        RuntimeCompletionOutcome::FinalizationFailed => {
            occ_dsl::RuntimeCompletionOutcome::FinalizationFailed
        }
        RuntimeCompletionOutcome::RuntimeTerminated => {
            occ_dsl::RuntimeCompletionOutcome::RuntimeTerminated
        }
    }
}

fn to_dsl_delivery_admission_outcome(
    outcome: DeliveryAdmissionOutcome,
) -> occ_dsl::DeliveryAdmissionOutcome {
    match outcome {
        DeliveryAdmissionOutcome::Accepted => occ_dsl::DeliveryAdmissionOutcome::Accepted,
        DeliveryAdmissionOutcome::Deduplicated => occ_dsl::DeliveryAdmissionOutcome::Deduplicated,
    }
}

fn from_dsl_failure_class(fc: occ_dsl::FailureClass) -> OccurrenceFailureClass {
    match fc {
        occ_dsl::FailureClass::TargetMaterializationFailed => {
            OccurrenceFailureClass::TargetMaterializationFailed
        }
        occ_dsl::FailureClass::TargetMissing => OccurrenceFailureClass::TargetMissing,
        occ_dsl::FailureClass::TargetBusy => OccurrenceFailureClass::TargetBusy,
        occ_dsl::FailureClass::RuntimeRejected => OccurrenceFailureClass::RuntimeRejected,
        occ_dsl::FailureClass::MobRejected => OccurrenceFailureClass::MobRejected,
        occ_dsl::FailureClass::LeaseLost => OccurrenceFailureClass::LeaseLost,
        occ_dsl::FailureClass::TransportError => OccurrenceFailureClass::TransportError,
        occ_dsl::FailureClass::InternalError => OccurrenceFailureClass::InternalError,
        occ_dsl::FailureClass::ScheduleMandateUnavailable => {
            OccurrenceFailureClass::ScheduleMandateUnavailable
        }
    }
}

fn occurrence_id_from_dsl(
    id: &occ_dsl::OccurrenceId,
) -> Result<crate::types::OccurrenceId, OccurrenceLifecycleError> {
    crate::types::OccurrenceId::parse(&id.0).map_err(|source| {
        OccurrenceLifecycleError::InvalidOccurrenceId {
            id: id.0.clone(),
            source,
        }
    })
}

fn occurrence_schedule_id_from_dsl(
    id: &occ_dsl::ScheduleId,
) -> Result<ScheduleId, OccurrenceLifecycleError> {
    ScheduleId::parse(&id.0).map_err(|source| OccurrenceLifecycleError::InvalidScheduleId {
        id: id.0.clone(),
        source,
    })
}

fn schedule_id_from_dsl(id: &sched_dsl::ScheduleId) -> Result<ScheduleId, ScheduleLifecycleError> {
    ScheduleId::parse(&id.0).map_err(|source| ScheduleLifecycleError::InvalidScheduleId {
        id: id.0.clone(),
        source,
    })
}

fn to_dsl_misfire_policy(policy: &crate::types::MisfirePolicy) -> sched_dsl::MisfirePolicy {
    match policy {
        crate::types::MisfirePolicy::Skip => sched_dsl::MisfirePolicy::Skip,
        crate::types::MisfirePolicy::CatchUpWithin { .. } => {
            sched_dsl::MisfirePolicy::CatchUpWithin
        }
    }
}

fn to_occ_dsl_misfire_policy(policy: &crate::types::MisfirePolicy) -> occ_dsl::MisfirePolicy {
    match policy {
        crate::types::MisfirePolicy::Skip => occ_dsl::MisfirePolicy::Skip,
        crate::types::MisfirePolicy::CatchUpWithin { .. } => occ_dsl::MisfirePolicy::CatchUpWithin,
    }
}

fn to_dsl_overlap_policy(policy: &crate::types::OverlapPolicy) -> sched_dsl::OverlapPolicy {
    match policy {
        crate::types::OverlapPolicy::AllowConcurrent => sched_dsl::OverlapPolicy::AllowConcurrent,
        crate::types::OverlapPolicy::SkipIfRunning => sched_dsl::OverlapPolicy::SkipIfRunning,
    }
}

fn to_occ_dsl_overlap_policy(policy: &crate::types::OverlapPolicy) -> occ_dsl::OverlapPolicy {
    match policy {
        crate::types::OverlapPolicy::AllowConcurrent => occ_dsl::OverlapPolicy::AllowConcurrent,
        crate::types::OverlapPolicy::SkipIfRunning => occ_dsl::OverlapPolicy::SkipIfRunning,
    }
}

fn to_dsl_missing_target_policy(
    policy: &crate::types::MissingTargetPolicy,
) -> sched_dsl::MissingTargetPolicy {
    match policy {
        crate::types::MissingTargetPolicy::Skip => sched_dsl::MissingTargetPolicy::Skip,
        crate::types::MissingTargetPolicy::MarkMisfired => {
            sched_dsl::MissingTargetPolicy::MarkMisfired
        }
    }
}

fn to_occ_dsl_missing_target_policy(
    policy: &crate::types::MissingTargetPolicy,
) -> occ_dsl::MissingTargetPolicy {
    match policy {
        crate::types::MissingTargetPolicy::Skip => occ_dsl::MissingTargetPolicy::Skip,
        crate::types::MissingTargetPolicy::MarkMisfired => {
            occ_dsl::MissingTargetPolicy::MarkMisfired
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::*;

    use crate::types::{
        MisfirePolicy, MissingTargetPolicy, OverlapPolicy, ScheduledSessionAction,
        SessionTargetBinding, TargetBinding, TriggerSpec,
    };
    use chrono::Duration;
    use meerkat_core::ContentInput;
    use std::collections::BTreeMap;

    fn sample_occurrence() -> Occurrence {
        Occurrence::planned_from_schedule(
            &sample_schedule(),
            crate::OccurrenceOrdinal(0),
            Utc::now(),
        )
        .expect("sample occurrence planning should pass generated authority")
    }

    fn sample_schedule() -> Schedule {
        Schedule::new(CreateScheduleRequest {
            name: Some("claim-guard".into()),
            description: None,
            trigger: TriggerSpec::Once {
                due_at_utc: Utc::now() + Duration::minutes(1),
            },
            target: TargetBinding::session(SessionTargetBinding::ExactSession {
                session_id: meerkat_core::SessionId::new(),
                action: ScheduledSessionAction::Prompt {
                    prompt: ContentInput::from("scheduled hello"),
                    system_prompt: None,
                    render_metadata: None,
                    skill_refs: Vec::new(),
                    additional_instructions: Vec::new(),
                },
            }),
            misfire_policy: MisfirePolicy::Skip,
            overlap_policy: OverlapPolicy::SkipIfRunning,
            missing_target_policy: MissingTargetPolicy::MarkMisfired,
            labels: BTreeMap::new(),
            planning_horizon_days: Some(1),
            planning_horizon_occurrences: Some(1),
        })
        .expect("sample schedule creation should pass generated authority")
    }

    fn sample_claimed_occurrence() -> Occurrence {
        sample_occurrence()
            .apply(OccurrenceLifecycleInput::Claim {
                owner_id: "owner".into(),
                at_utc: Utc::now(),
                lease_expires_at_utc: Utc::now() + Duration::seconds(30),
                claim_token: Uuid::now_v7(),
                admission_evidence: AdmissionEvidence::None,
            })
            .expect("claim should pass generated authority")
            .into_occurrence()
    }

    fn sample_completed_occurrence() -> Occurrence {
        sample_claimed_occurrence()
            .apply(OccurrenceLifecycleInput::DispatchStarted {
                correlation_id: Some("corr-1".into()),
                at_utc: Utc::now(),
            })
            .expect("dispatch should pass generated authority")
            .into_occurrence()
            .apply(OccurrenceLifecycleInput::Complete { at_utc: Utc::now() })
            .expect("completion should pass generated authority")
            .into_occurrence()
    }

    /// Lease renewal is machine authority (2026-07 P0): only the current
    /// claim-token holder may extend its own in-flight lease, the extension
    /// is monotonic, and everything else is a typed NotLeaseHolding refusal.
    #[test]
    fn lease_renewal_extends_only_for_current_token_holder_in_flight() {
        let dispatching = sample_claimed_occurrence()
            .apply(OccurrenceLifecycleInput::DispatchStarted {
                correlation_id: Some("corr-1".into()),
                at_utc: Utc::now(),
            })
            .expect("dispatch should pass generated authority")
            .into_occurrence();
        let token = dispatching.claim_token().expect("claimed token");
        let initial_expiry = dispatching
            .lease_expires_at_utc
            .expect("claimed occurrence must hold a lease");

        // Current token holder extends from Dispatching.
        let extended_to = initial_expiry + Duration::seconds(60);
        let mutator = dispatching
            .clone()
            .apply(OccurrenceLifecycleInput::RenewLease {
                claim_token: token,
                lease_expires_at_utc: extended_to,
                at_utc: Utc::now(),
            })
            .expect("token-holder renewal must pass generated authority");
        assert!(
            mutator
                .effects
                .contains(&OccurrenceLifecycleEffect::LeaseRenewed),
            "renewal must emit the machine's LeaseRenewed acknowledgement"
        );
        let renewed = mutator.into_occurrence();
        assert_eq!(renewed.phase, OccurrencePhase::Dispatching);
        assert_eq!(renewed.lease_expires_at_utc, Some(extended_to));
        assert_eq!(renewed.attempt_count, dispatching.attempt_count);

        // And again from AwaitingCompletion.
        let awaiting = renewed
            .apply(OccurrenceLifecycleInput::AwaitCompletion { at_utc: Utc::now() })
            .expect("await should pass generated authority")
            .into_occurrence();
        let later = extended_to + Duration::seconds(60);
        let renewed = awaiting
            .apply(OccurrenceLifecycleInput::RenewLease {
                claim_token: token,
                lease_expires_at_utc: later,
                at_utc: Utc::now(),
            })
            .expect("in-flight renewal must pass generated authority")
            .into_occurrence();
        assert_eq!(renewed.phase, OccurrencePhase::AwaitingCompletion);
        assert_eq!(renewed.lease_expires_at_utc, Some(later));

        // A non-holder token is refused (typed NotLeaseHolding).
        assert!(matches!(
            renewed.clone().apply(OccurrenceLifecycleInput::RenewLease {
                claim_token: Uuid::now_v7(),
                lease_expires_at_utc: later + Duration::seconds(60),
                at_utc: Utc::now(),
            }),
            Err(OccurrenceLifecycleError::NotLeaseHolding)
        ));

        // A shrinking "renewal" is refused: extensions are monotonic.
        assert!(matches!(
            renewed.apply(OccurrenceLifecycleInput::RenewLease {
                claim_token: token,
                lease_expires_at_utc: later - Duration::seconds(1),
                at_utc: Utc::now(),
            }),
            Err(OccurrenceLifecycleError::NotLeaseHolding)
        ));

        // Renewal is not legal before dispatch (Claimed: nothing in flight)
        // or after terminality.
        assert!(matches!(
            sample_claimed_occurrence().apply(OccurrenceLifecycleInput::RenewLease {
                claim_token: token,
                lease_expires_at_utc: later,
                at_utc: Utc::now(),
            }),
            Err(OccurrenceLifecycleError::NotLeaseHolding)
        ));
        assert!(matches!(
            sample_completed_occurrence().apply(OccurrenceLifecycleInput::RenewLease {
                claim_token: token,
                lease_expires_at_utc: later,
                at_utc: Utc::now(),
            }),
            Err(OccurrenceLifecycleError::NotLeaseHolding)
        ));
    }

    #[test]
    fn terminality_is_decided_by_occurrence_authority() {
        // Live phases mirror `false`; terminal phases mirror `true`. The
        // verdict is owned by OccurrenceLifecycleMachine's
        // ClassifyOccurrenceTerminality, not a handwritten shell matches!.
        let pending = sample_occurrence();
        assert_eq!(pending.phase, OccurrencePhase::Pending);
        assert!(!pending.classify_terminality().expect("classify pending"));
        assert!(!pending.is_terminal());

        let claimed = sample_claimed_occurrence();
        assert_eq!(claimed.phase, OccurrencePhase::Claimed);
        assert!(!claimed.classify_terminality().expect("classify claimed"));
        assert!(!claimed.is_terminal());

        let completed = sample_completed_occurrence();
        assert_eq!(completed.phase, OccurrencePhase::Completed);
        assert!(
            completed
                .classify_terminality()
                .expect("classify completed")
        );
        assert!(completed.is_terminal());

        // Superseded is terminal and records a revision (invariant-satisfying
        // terminal state recovered through the real machine).
        let superseded = sample_claimed_occurrence()
            .apply(OccurrenceLifecycleInput::Supersede {
                superseded_by_revision: ScheduleRevision(2),
                at_utc: Utc::now(),
            })
            .expect("supersede should pass generated authority")
            .into_occurrence();
        assert_eq!(superseded.phase, OccurrencePhase::Superseded);
        assert!(
            superseded
                .classify_terminality()
                .expect("classify superseded")
        );
        assert!(superseded.is_terminal());
    }

    fn classify_rejected_occurrence_input(
        occurrence: &Occurrence,
        input: OccurrenceLifecycleInput,
    ) -> occ_dsl::OccurrenceTransitionFailureClassKind {
        let dsl_state = occurrence_machine_state_for_authority(occurrence)
            .expect("test occurrence projection should match authority");
        let mut authority =
            occ_dsl::OccurrenceLifecycleMachineAuthority::recover_from_state(dsl_state)
                .expect("test occurrence state should recover");
        let dsl_input = convert_occurrence_input(&input).expect("test input should convert");
        let error = occ_dsl::OccurrenceLifecycleMachineMutator::apply(&mut authority, dsl_input)
            .expect_err("test input should be rejected by generated authority");
        classify_occurrence_transition_failure(&mut authority, &error)
            .expect("generated authority should classify its rejection")
    }

    #[test]
    fn transition_failure_public_class_comes_from_occurrence_authority() {
        let now = Utc::now();
        let schedule = sample_schedule();
        let pending = sample_occurrence();
        let claimed = sample_claimed_occurrence();
        let completed = sample_completed_occurrence();
        let receipt = DeliveryReceipt::new(
            pending.occurrence_id.clone(),
            pending.attempt_count,
            DeliveryReceiptStage::Claimed,
        );
        let future = Occurrence::planned_from_schedule(
            &sample_schedule(),
            crate::OccurrenceOrdinal(1),
            now + Duration::minutes(5),
        )
        .expect("future occurrence planning should pass generated authority");

        let cases = vec![
            (
                &claimed,
                OccurrenceLifecycleInput::PlanOccurrence {
                    occurrence_id: crate::types::OccurrenceId::new(),
                    schedule_id: schedule.schedule_id.clone(),
                    schedule_revision: schedule.revision,
                    occurrence_ordinal: crate::OccurrenceOrdinal(0),
                    trigger_snapshot: schedule.trigger.clone(),
                    target_snapshot: schedule.target.clone(),
                    misfire_policy: schedule.misfire_policy.clone(),
                    overlap_policy: schedule.overlap_policy.clone(),
                    missing_target_policy: schedule.missing_target_policy.clone(),
                    due_at_utc: now,
                },
                occ_dsl::OccurrenceTransitionFailureClassKind::PlanRejected,
            ),
            (
                &completed,
                OccurrenceLifecycleInput::SyncTargetSnapshot {
                    target_snapshot: schedule.target,
                },
                occ_dsl::OccurrenceTransitionFailureClassKind::TargetSyncRejected,
            ),
            (
                &pending,
                OccurrenceLifecycleInput::RecordReceipt {
                    receipt,
                    runtime_outcome: None,
                },
                occ_dsl::OccurrenceTransitionFailureClassKind::ReceiptRecordRejected,
            ),
            (
                &future,
                OccurrenceLifecycleInput::Claim {
                    owner_id: "owner".into(),
                    at_utc: now,
                    lease_expires_at_utc: now + Duration::seconds(30),
                    claim_token: Uuid::now_v7(),
                    admission_evidence: AdmissionEvidence::None,
                },
                occ_dsl::OccurrenceTransitionFailureClassKind::ClaimRejected,
            ),
            (
                &claimed,
                OccurrenceLifecycleInput::Claim {
                    owner_id: "owner".into(),
                    at_utc: now,
                    lease_expires_at_utc: now + Duration::seconds(30),
                    claim_token: Uuid::now_v7(),
                    admission_evidence: AdmissionEvidence::None,
                },
                occ_dsl::OccurrenceTransitionFailureClassKind::NotPendingForClaim,
            ),
            (
                &pending,
                OccurrenceLifecycleInput::DispatchStarted {
                    correlation_id: Some("corr-1".into()),
                    at_utc: now,
                },
                occ_dsl::OccurrenceTransitionFailureClassKind::NotClaimed,
            ),
            (
                &pending,
                OccurrenceLifecycleInput::DispatchAccepted {
                    admission_outcome: DeliveryAdmissionOutcome::Accepted,
                    at_utc: now,
                },
                occ_dsl::OccurrenceTransitionFailureClassKind::NotDispatching,
            ),
            (
                &pending,
                OccurrenceLifecycleInput::AwaitCompletion { at_utc: now },
                occ_dsl::OccurrenceTransitionFailureClassKind::NotDispatching,
            ),
            (
                &pending,
                OccurrenceLifecycleInput::Complete { at_utc: now },
                occ_dsl::OccurrenceTransitionFailureClassKind::NotLiveForTerminal,
            ),
            (
                &pending,
                OccurrenceLifecycleInput::ResolveRuntimeCompletion {
                    outcome: RuntimeCompletionOutcome::Completed,
                    detail: None,
                    at_utc: now,
                },
                occ_dsl::OccurrenceTransitionFailureClassKind::NotLiveForTerminal,
            ),
            (
                &pending,
                OccurrenceLifecycleInput::ResolveDeliveryCompletionFailure {
                    reason: DeliveryCompletionFailureReason::RuntimeCompletionHandleMissing,
                    detail: None,
                    at_utc: now,
                },
                occ_dsl::OccurrenceTransitionFailureClassKind::NotLiveForTerminal,
            ),
            (
                &pending,
                OccurrenceLifecycleInput::ResolveDeliveryFailure {
                    reason: DeliveryFailureReason::TransportError,
                    detail: None,
                    at_utc: now,
                },
                occ_dsl::OccurrenceTransitionFailureClassKind::NotLiveForTerminal,
            ),
            (
                &completed,
                OccurrenceLifecycleInput::ResolveTargetProbe {
                    outcome: OccurrenceTargetProbeOutcome::Busy,
                    redelivery: None,
                    detail: None,
                    at_utc: now,
                },
                occ_dsl::OccurrenceTransitionFailureClassKind::NotLiveForTerminal,
            ),
            (
                &completed,
                OccurrenceLifecycleInput::ResolveDueMisfire {
                    detail: None,
                    at_utc: now,
                },
                occ_dsl::OccurrenceTransitionFailureClassKind::NotLiveForTerminal,
            ),
            (
                &completed,
                OccurrenceLifecycleInput::Supersede {
                    superseded_by_revision: ScheduleRevision(2),
                    at_utc: now,
                },
                occ_dsl::OccurrenceTransitionFailureClassKind::NotLiveForTerminal,
            ),
            (
                &pending,
                OccurrenceLifecycleInput::LeaseExpired { at_utc: now },
                occ_dsl::OccurrenceTransitionFailureClassKind::NotLeaseHolding,
            ),
            (
                &pending,
                OccurrenceLifecycleInput::ReleaseLeaseForPausedSchedule { at_utc: now },
                occ_dsl::OccurrenceTransitionFailureClassKind::NotLeaseHolding,
            ),
        ];

        for (occurrence, input, expected) in cases {
            assert_eq!(
                classify_rejected_occurrence_input(occurrence, input),
                expected
            );
        }
    }

    #[test]
    fn claim_rejects_non_pending_occurrences() {
        let input = OccurrenceLifecycleInput::Claim {
            owner_id: "owner".into(),
            at_utc: Utc::now(),
            lease_expires_at_utc: Utc::now() + Duration::seconds(30),
            claim_token: Uuid::now_v7(),
            admission_evidence: AdmissionEvidence::None,
        };

        for phase in [
            OccurrencePhase::Claimed,
            OccurrencePhase::Dispatching,
            OccurrencePhase::AwaitingCompletion,
        ] {
            let mut occurrence = sample_occurrence();
            occurrence.phase = phase;
            assert!(
                occurrence.apply(input.clone()).is_err(),
                "claim should reject occurrences already in phase {phase:?}"
            );
        }
    }

    #[test]
    fn claimed_dispatch_disposition_comes_from_occurrence_authority() {
        // Occurrence is claimed at schedule revision 1 (the sample schedule's
        // initial revision). The machine — not the driver — classifies each
        // disposition from the observed schedule phase + current revision.
        let claimed = sample_claimed_occurrence();
        assert_eq!(claimed.schedule_revision, ScheduleRevision(1));

        // Active + current revision -> Ready.
        let ready = claimed
            .classify_claimed_dispatch_disposition(SchedulePhase::Active, ScheduleRevision(1))
            .expect("ready disposition should classify");
        assert_eq!(ready.disposition, ClaimedDispatchDisposition::Ready);
        assert_eq!(ready.superseded_by_revision, None);

        // Paused (revision not behind) -> Frozen.
        let frozen = claimed
            .classify_claimed_dispatch_disposition(SchedulePhase::Paused, ScheduleRevision(1))
            .expect("frozen disposition should classify");
        assert_eq!(frozen.disposition, ClaimedDispatchDisposition::Frozen);
        assert_eq!(frozen.superseded_by_revision, None);

        // Deleted -> Supersede by current revision.
        let superseded_deleted = claimed
            .classify_claimed_dispatch_disposition(SchedulePhase::Deleted, ScheduleRevision(3))
            .expect("deleted disposition should classify");
        assert_eq!(
            superseded_deleted.disposition,
            ClaimedDispatchDisposition::Supersede
        );
        assert_eq!(
            superseded_deleted.superseded_by_revision,
            Some(ScheduleRevision(3))
        );

        // Active but stale claimed revision -> Supersede by current revision.
        let superseded_stale = claimed
            .classify_claimed_dispatch_disposition(SchedulePhase::Active, ScheduleRevision(2))
            .expect("stale disposition should classify");
        assert_eq!(
            superseded_stale.disposition,
            ClaimedDispatchDisposition::Supersede
        );
        assert_eq!(
            superseded_stale.superseded_by_revision,
            Some(ScheduleRevision(2))
        );

        // Claimed revision ahead of the schedule -> FutureRevision (the driver
        // surfaces this as an internal error; the class itself is the
        // machine's).
        let future = claimed
            .classify_claimed_dispatch_disposition(SchedulePhase::Active, ScheduleRevision(0))
            .expect("future-revision disposition should classify");
        assert_eq!(
            future.disposition,
            ClaimedDispatchDisposition::FutureRevision
        );
        assert_eq!(future.superseded_by_revision, None);
    }

    #[test]
    fn completion_supersession_comes_from_occurrence_authority() {
        // Occurrence is dispatched and awaiting completion at schedule revision 1.
        // The machine — not the driver — classifies whether the completed
        // delivery is superseded. Unlike the pre-dispatch disposition, a paused
        // schedule does NOT supersede an already-completed delivery.
        let awaiting = sample_claimed_occurrence()
            .apply(OccurrenceLifecycleInput::DispatchStarted {
                correlation_id: Some("corr-supersede".into()),
                at_utc: Utc::now(),
            })
            .expect("dispatch should pass generated authority")
            .into_occurrence()
            .apply(OccurrenceLifecycleInput::AwaitCompletion { at_utc: Utc::now() })
            .expect("await-completion should pass generated authority")
            .into_occurrence();
        assert_eq!(awaiting.schedule_revision, ScheduleRevision(1));

        // Active + current revision -> Proceed.
        let proceed = awaiting
            .classify_completion_supersession(SchedulePhase::Active, ScheduleRevision(1))
            .expect("proceed disposition should classify");
        assert_eq!(
            proceed.disposition,
            CompletionSupersessionDisposition::Proceed
        );
        assert_eq!(proceed.superseded_by_revision, None);

        // Paused (revision not behind) -> Proceed (paused does NOT supersede a
        // completed delivery).
        let paused = awaiting
            .classify_completion_supersession(SchedulePhase::Paused, ScheduleRevision(1))
            .expect("paused completion disposition should classify");
        assert_eq!(
            paused.disposition,
            CompletionSupersessionDisposition::Proceed
        );
        assert_eq!(paused.superseded_by_revision, None);

        // Deleted -> Supersede by current revision.
        let deleted = awaiting
            .classify_completion_supersession(SchedulePhase::Deleted, ScheduleRevision(3))
            .expect("deleted completion disposition should classify");
        assert_eq!(
            deleted.disposition,
            CompletionSupersessionDisposition::Supersede
        );
        assert_eq!(deleted.superseded_by_revision, Some(ScheduleRevision(3)));

        // Active but stale claimed revision -> Supersede by current revision.
        let stale = awaiting
            .classify_completion_supersession(SchedulePhase::Active, ScheduleRevision(2))
            .expect("stale completion disposition should classify");
        assert_eq!(
            stale.disposition,
            CompletionSupersessionDisposition::Supersede
        );
        assert_eq!(stale.superseded_by_revision, Some(ScheduleRevision(2)));
    }

    #[test]
    fn due_classification_comes_from_occurrence_authority() {
        let now = Utc::now();
        let claimable =
            Occurrence::planned_from_schedule(&sample_schedule(), crate::OccurrenceOrdinal(0), now)
                .expect("claimable occurrence planning should pass generated authority");
        assert_eq!(
            claimable
                .classify_due_action(now, AdmissionEvidence::None)
                .expect("due classification should pass"),
            Some(OccurrenceDueAction::ClaimEligible)
        );

        let future = Occurrence::planned_from_schedule(
            &sample_schedule(),
            crate::OccurrenceOrdinal(1),
            now + Duration::minutes(5),
        )
        .expect("future occurrence planning should pass generated authority");
        assert_eq!(
            future
                .classify_due_action(now, AdmissionEvidence::None)
                .expect("future classification should pass"),
            None
        );
        assert!(matches!(
            future.apply(OccurrenceLifecycleInput::Claim {
                owner_id: "owner".into(),
                at_utc: now,
                lease_expires_at_utc: now + Duration::seconds(30),
                claim_token: Uuid::now_v7(),
                admission_evidence: AdmissionEvidence::None,
            }),
            Err(OccurrenceLifecycleError::ClaimRejected)
        ));

        let overdue = Occurrence::planned_from_schedule(
            &sample_schedule(),
            crate::OccurrenceOrdinal(2),
            now - Duration::seconds(60),
        )
        .expect("overdue occurrence planning should pass generated authority");
        assert_eq!(
            overdue
                .classify_due_action(now, AdmissionEvidence::None)
                .expect("overdue classification should pass"),
            Some(OccurrenceDueAction::MisfireRequired)
        );
    }

    #[test]
    fn record_receipt_rejects_stage_without_generated_authority() {
        let dispatching = sample_claimed_occurrence()
            .apply(OccurrenceLifecycleInput::DispatchStarted {
                correlation_id: Some("corr-1".into()),
                at_utc: Utc::now(),
            })
            .expect("dispatch start should pass generated authority")
            .into_occurrence();
        let mut receipt = dispatching
            .delivery_receipt_from_authority(None)
            .expect("generated receipt authority should project dispatch receipt");
        receipt.stage = DeliveryReceiptStage::Completed;

        assert!(matches!(
            dispatching.apply(OccurrenceLifecycleInput::RecordReceipt {
                receipt,
                runtime_outcome: None,
            }),
            Err(OccurrenceLifecycleError::ReceiptStageMismatch { .. })
        ));
    }

    #[test]
    fn record_receipt_rejects_provenance_without_generated_authority() {
        let dispatching = sample_claimed_occurrence()
            .apply(OccurrenceLifecycleInput::DispatchStarted {
                correlation_id: Some("corr-1".into()),
                at_utc: Utc::now(),
            })
            .expect("dispatch start should pass generated authority")
            .into_occurrence();
        let mut receipt = dispatching
            .delivery_receipt_from_authority(None)
            .expect("generated receipt authority should project dispatch receipt");
        receipt.receipt_id = Uuid::now_v7();

        assert!(matches!(
            dispatching.apply(OccurrenceLifecycleInput::RecordReceipt {
                receipt,
                runtime_outcome: None,
            }),
            Err(OccurrenceLifecycleError::ReceiptRecordMismatch { .. })
        ));
    }

    #[test]
    fn dispatch_acceptance_retains_precommitted_identity_and_records_admission_outcome() {
        let now = Utc::now();
        let dispatching = sample_claimed_occurrence()
            .apply(OccurrenceLifecycleInput::DispatchStarted {
                correlation_id: Some("stable-occurrence-correlation".into()),
                at_utc: now,
            })
            .expect("dispatch start should pass generated authority")
            .into_occurrence();
        let started_receipt = dispatching
            .delivery_receipt_from_authority(None)
            .expect("started receipt authority");
        let dispatching = dispatching
            .apply(OccurrenceLifecycleInput::RecordReceipt {
                receipt: started_receipt,
                runtime_outcome: None,
            })
            .expect("record dispatch start")
            .into_occurrence();
        let accepted = dispatching
            .apply(OccurrenceLifecycleInput::DispatchAccepted {
                admission_outcome: DeliveryAdmissionOutcome::Accepted,
                at_utc: now + Duration::milliseconds(1),
            })
            .expect("dispatch acceptance should pass generated authority")
            .into_occurrence();
        let admission = RuntimeDeliveryOutcome::AdmissionAccepted;
        let accepted_receipt = accepted
            .delivery_receipt_from_authority(Some(admission.clone()))
            .expect("accepted receipt authority");
        assert_eq!(
            accepted_receipt.stage,
            DeliveryReceiptStage::DispatchAccepted
        );
        assert_eq!(
            accepted_receipt.correlation_id.as_deref(),
            Some("stable-occurrence-correlation"),
            "post-effect transition cannot rewrite the pre-effect identity"
        );
        let recorded = accepted
            .apply(OccurrenceLifecycleInput::RecordReceipt {
                receipt: accepted_receipt,
                runtime_outcome: Some(admission.clone()),
            })
            .expect("record dispatch acceptance")
            .into_occurrence();
        assert_eq!(recorded.runtime_outcome, Some(admission));
        assert_eq!(
            recorded.last_receipt.as_ref().map(|receipt| receipt.stage),
            Some(DeliveryReceiptStage::DispatchAccepted)
        );
    }

    #[test]
    fn deduplicated_admission_waits_in_dispatching() {
        let now = Utc::now();
        let dispatching = sample_claimed_occurrence()
            .apply(OccurrenceLifecycleInput::DispatchStarted {
                correlation_id: Some("stable-occurrence-correlation".into()),
                at_utc: now,
            })
            .expect("dispatch start should pass generated authority")
            .into_occurrence();

        // A deduplicated admission proves that the target admitted this
        // identity once, not what the existing input did: no terminal yet.
        let mutator = dispatching
            .apply(OccurrenceLifecycleInput::DispatchAccepted {
                admission_outcome: DeliveryAdmissionOutcome::Deduplicated,
                at_utc: now + Duration::milliseconds(1),
            })
            .expect("deduplicated admission should pass generated authority");
        assert_eq!(
            mutator.effects(),
            &[OccurrenceLifecycleEffect::DispatchAccepted],
            "a deduplicated admission emits only DispatchAccepted, never Completed"
        );
        let waiting = mutator.into_occurrence();
        assert_eq!(waiting.phase, OccurrencePhase::Dispatching);
        assert_eq!(waiting.completed_at_utc, None);
        assert_eq!(waiting.failure_class, None);

        let receipt = waiting
            .delivery_receipt_from_authority(Some(RuntimeDeliveryOutcome::AdmissionDeduplicated))
            .expect("deduplicated admission should project a dispatch-accepted receipt");
        assert_eq!(receipt.stage, DeliveryReceiptStage::DispatchAccepted);
        assert_eq!(
            receipt.runtime_outcome,
            Some(RuntimeDeliveryOutcome::AdmissionDeduplicated)
        );
        assert_eq!(
            receipt.correlation_id.as_deref(),
            Some("stable-occurrence-correlation")
        );
    }

    #[test]
    fn lease_expired_receipt_projection_comes_from_generated_authority() {
        let expired = sample_claimed_occurrence()
            .apply(OccurrenceLifecycleInput::LeaseExpired { at_utc: Utc::now() })
            .expect("lease expiry should pass generated authority")
            .into_occurrence();
        let receipt = expired
            .delivery_receipt_from_authority(None)
            .expect("generated receipt authority should project lease receipt");

        assert_eq!(receipt.stage, DeliveryReceiptStage::LeaseExpired);
        assert_eq!(
            receipt.failure_class,
            Some(OccurrenceFailureClass::LeaseLost)
        );
        assert_eq!(
            receipt.detail.as_deref(),
            Some("lease expired before completion")
        );
        expired
            .apply(OccurrenceLifecycleInput::RecordReceipt {
                receipt,
                runtime_outcome: None,
            })
            .expect("generated receipt authority should accept matching receipt");
    }

    #[test]
    fn await_completion_requires_dispatching_phase() {
        let occurrence = sample_occurrence();
        let result =
            occurrence.apply(OccurrenceLifecycleInput::AwaitCompletion { at_utc: Utc::now() });
        assert!(matches!(
            result,
            Err(OccurrenceLifecycleError::NotDispatching)
        ));
    }

    #[test]
    fn runtime_completion_terminality_comes_from_occurrence_authority()
    -> Result<(), Box<dyn std::error::Error>> {
        let at_utc = Utc::now();
        let awaiting = sample_claimed_occurrence()
            .apply(OccurrenceLifecycleInput::DispatchStarted {
                correlation_id: Some("corr-1".into()),
                at_utc,
            })?
            .into_occurrence()
            .apply(OccurrenceLifecycleInput::AwaitCompletion { at_utc })?
            .into_occurrence();

        let completed = awaiting
            .clone()
            .apply(OccurrenceLifecycleInput::ResolveRuntimeCompletion {
                outcome: RuntimeCompletionOutcome::Completed,
                detail: None,
                at_utc,
            })?
            .into_occurrence();

        assert_eq!(completed.phase, OccurrencePhase::Completed);
        assert_eq!(completed.failure_class, None);
        assert_eq!(completed.failure_detail, None);

        let rejected = awaiting
            .apply(OccurrenceLifecycleInput::ResolveRuntimeCompletion {
                outcome: RuntimeCompletionOutcome::CallbackPending,
                detail: Some("callback still pending".into()),
                at_utc,
            })?
            .into_occurrence();

        assert_eq!(rejected.phase, OccurrencePhase::DeliveryFailed);
        assert_eq!(
            rejected.failure_class,
            Some(OccurrenceFailureClass::RuntimeRejected)
        );
        assert_eq!(
            rejected.failure_detail.as_deref(),
            Some("callback still pending")
        );

        let completion_authority_missing = sample_claimed_occurrence()
            .apply(OccurrenceLifecycleInput::DispatchStarted {
                correlation_id: Some("corr-2".into()),
                at_utc,
            })?
            .into_occurrence()
            .apply(OccurrenceLifecycleInput::AwaitCompletion { at_utc })?
            .into_occurrence()
            .apply(OccurrenceLifecycleInput::ResolveDeliveryCompletionFailure {
                reason: DeliveryCompletionFailureReason::RuntimeCompletionHandleMissing,
                detail: Some("missing completion handle".into()),
                at_utc,
            })?
            .into_occurrence();

        assert_eq!(
            completion_authority_missing.phase,
            OccurrencePhase::DeliveryFailed
        );
        assert_eq!(
            completion_authority_missing.failure_class,
            Some(OccurrenceFailureClass::InternalError)
        );
        assert_eq!(
            completion_authority_missing.failure_detail.as_deref(),
            Some("missing completion handle")
        );

        let completion_future_failed = sample_claimed_occurrence()
            .apply(OccurrenceLifecycleInput::DispatchStarted {
                correlation_id: Some("corr-3".into()),
                at_utc,
            })?
            .into_occurrence()
            .apply(OccurrenceLifecycleInput::AwaitCompletion { at_utc })?
            .into_occurrence()
            .apply(OccurrenceLifecycleInput::ResolveDeliveryCompletionFailure {
                reason: DeliveryCompletionFailureReason::CompletionFutureFailed,
                detail: Some("completion future failed".into()),
                at_utc,
            })?
            .into_occurrence();

        assert_eq!(
            completion_future_failed.phase,
            OccurrencePhase::DeliveryFailed
        );
        assert_eq!(
            completion_future_failed.failure_class,
            Some(OccurrenceFailureClass::TransportError)
        );
        assert_eq!(
            completion_future_failed.failure_detail.as_deref(),
            Some("completion future failed")
        );

        let delivery_failure = sample_claimed_occurrence()
            .apply(OccurrenceLifecycleInput::ResolveDeliveryFailure {
                reason: DeliveryFailureReason::TargetMaterializationFailed,
                detail: Some("materialization failed".into()),
                at_utc,
            })?
            .into_occurrence();

        assert_eq!(delivery_failure.phase, OccurrencePhase::DeliveryFailed);
        assert_eq!(
            delivery_failure.failure_class,
            Some(OccurrenceFailureClass::TargetMaterializationFailed)
        );
        assert_eq!(
            delivery_failure.failure_detail.as_deref(),
            Some("materialization failed")
        );
        Ok(())
    }

    #[test]
    fn supersede_emits_occurrence_ack_effect() -> Result<(), Box<dyn std::error::Error>> {
        let occurrence = sample_occurrence();
        let occurrence_id = occurrence.occurrence_id.clone();
        let superseding_revision = ScheduleRevision(2);

        let mutator = occurrence.apply(OccurrenceLifecycleInput::Supersede {
            superseded_by_revision: superseding_revision,
            at_utc: Utc::now(),
        })?;

        assert_eq!(mutator.occurrence.phase, OccurrencePhase::Superseded);
        assert_eq!(
            mutator.occurrence.superseded_by_revision,
            Some(superseding_revision)
        );
        assert_eq!(
            mutator.effects,
            vec![
                OccurrenceLifecycleEffect::Superseded,
                OccurrenceLifecycleEffect::OccurrencesSuperseded {
                    occurrence_id: occurrence_id.clone(),
                    superseding_revision,
                },
            ]
        );
        let (_updated, _effects, acks) = mutator.into_parts_with_supersession_feedback();
        assert_eq!(acks.len(), 1);
        assert_eq!(acks[0].occurrence_id(), &occurrence_id);
        assert_eq!(acks[0].superseding_revision(), superseding_revision);
        Ok(())
    }

    #[test]
    fn schedule_records_supersede_ack_from_occurrence_feedback()
    -> Result<(), Box<dyn std::error::Error>> {
        let original = sample_schedule();
        let old_occurrence =
            Occurrence::planned_from_schedule(&original, OccurrenceOrdinal(0), Utc::now())?;
        let revised = Schedule::apply(
            Some(original.clone()),
            ScheduleLifecycleInput::Update {
                request: UpdateScheduleRequest {
                    expected_revision: Some(original.revision),
                    trigger: Some(TriggerSpec::Once {
                        due_at_utc: Utc::now() + Duration::minutes(2),
                    }),
                    ..UpdateScheduleRequest::default()
                },
                at_utc: Utc::now(),
            },
        )?
        .into_schedule();
        let occurrence_id = old_occurrence.occurrence_id.clone();
        let (_updated_occurrence, _effects, mut acks) = old_occurrence
            .apply(OccurrenceLifecycleInput::Supersede {
                superseded_by_revision: revised.revision,
                at_utc: Utc::now(),
            })?
            .into_parts_with_supersession_feedback();
        assert_eq!(acks.len(), 1);

        let mutator = Schedule::apply(
            Some(revised.clone()),
            ScheduleLifecycleInput::ConfirmOccurrencesSuperseded {
                ack: acks.pop().expect("supersede should mint feedback"),
            },
        )?;

        assert_eq!(mutator.schedule.phase, SchedulePhase::Active);
        assert_eq!(mutator.schedule.revision, revised.revision);
        assert!(mutator.effects.is_empty());
        assert!(mutator.schedule.superseded_ack_ids.contains(&occurrence_id));
        Ok(())
    }

    #[test]
    fn public_occurrence_effect_tampering_cannot_forge_supersession_feedback()
    -> Result<(), Box<dyn std::error::Error>> {
        let schedule = sample_schedule();
        let occurrence =
            Occurrence::planned_from_schedule(&schedule, OccurrenceOrdinal(0), Utc::now())?;
        let forged_id = occurrence.occurrence_id.clone();
        let mut mutator = occurrence.apply(OccurrenceLifecycleInput::Claim {
            owner_id: "owner".into(),
            at_utc: Utc::now(),
            lease_expires_at_utc: Utc::now() + Duration::seconds(30),
            claim_token: Uuid::now_v7(),
            admission_evidence: AdmissionEvidence::None,
        })?;
        mutator
            .effects
            .push(OccurrenceLifecycleEffect::OccurrencesSuperseded {
                occurrence_id: forged_id.clone(),
                superseding_revision: schedule.revision,
            });

        let (_updated_occurrence, _effects, acks) = mutator.into_parts_with_supersession_feedback();
        assert!(acks.is_empty());
        let schedule = crate::store::apply_supersession_feedback(schedule, acks)?;
        assert!(!schedule.superseded_ack_ids.contains(&forged_id));
        Ok(())
    }

    /// Revise `schedule` (a trigger change, so a new revision), supersede
    /// `outstanding` at that revision through the occurrence authority, and
    /// feed the acknowledgements back the way a store commit does.
    fn revise_superseding(
        schedule: Schedule,
        outstanding: Vec<Occurrence>,
    ) -> Result<(Schedule, Vec<OccurrenceSupersessionAck>), Box<dyn std::error::Error>> {
        let at_utc = Utc::now();
        let due_offset_minutes = i64::try_from(schedule.revision.0)? + 1;
        let revised = Schedule::apply(
            Some(schedule.clone()),
            ScheduleLifecycleInput::Update {
                request: UpdateScheduleRequest {
                    expected_revision: Some(schedule.revision),
                    trigger: Some(TriggerSpec::Once {
                        due_at_utc: at_utc + Duration::minutes(due_offset_minutes),
                    }),
                    ..UpdateScheduleRequest::default()
                },
                at_utc,
            },
        )?
        .into_schedule();
        let mut acks = Vec::new();
        for occurrence in outstanding {
            let (_superseded, _effects, occurrence_acks) = occurrence
                .apply(OccurrenceLifecycleInput::Supersede {
                    superseded_by_revision: revised.revision,
                    at_utc,
                })?
                .into_parts_with_supersession_feedback();
            acks.extend(occurrence_acks);
        }
        let schedule = crate::store::apply_supersession_feedback(revised, acks.clone())?;
        Ok((schedule, acks))
    }

    /// A late acknowledgement of an occurrence the latest revise superseded
    /// (that supersession's own acknowledgement, delivered again) keeps
    /// today's answer: accepted, effectless, no revision bump, the id still
    /// recorded. The latest supersession's ids are the only ones recorded.
    #[test]
    fn late_ack_for_the_latest_supersession_keeps_its_answer()
    -> Result<(), Box<dyn std::error::Error>> {
        let schedule = sample_schedule();
        let first = Occurrence::planned_from_schedule(
            &schedule,
            OccurrenceOrdinal(0),
            Utc::now() + Duration::hours(1),
        )?;
        let (schedule, _first_acks) = revise_superseding(schedule, vec![first])?;
        let second = Occurrence::planned_from_schedule(
            &schedule,
            OccurrenceOrdinal(1),
            Utc::now() + Duration::hours(2),
        )?;
        let second_id = second.occurrence_id.clone();
        let (schedule, second_acks) = revise_superseding(schedule, vec![second])?;
        assert_eq!(second_acks.len(), 1);
        assert_eq!(
            schedule.superseded_ack_ids,
            BTreeSet::from([second_id.clone()]),
            "the second revise must drop the first supersession's acknowledgement"
        );

        let late = Schedule::apply(
            Some(schedule.clone()),
            ScheduleLifecycleInput::ConfirmOccurrencesSuperseded {
                ack: second_acks[0].clone(),
            },
        )?;
        assert!(late.effects().is_empty());
        assert!(!late.revision_bumped());
        let after = late.into_effectless_schedule()?;
        assert_eq!(
            after, schedule,
            "a repeated acknowledgement of the latest supersession changes nothing"
        );
        assert!(after.superseded_ack_ids.contains(&second_id));
        validate_schedule_machine_projection(&after)?;
        Ok(())
    }

    /// A late acknowledgement of an occurrence two revisions back, whose id a
    /// later revise dropped, ends in the documented answer: it is accepted as
    /// an idempotent no-op through the schedule authority and through the
    /// store's feedback seam, and the id stays dropped. The schedule copy and
    /// the machine copy stay equal, and the schedule still round-trips.
    #[test]
    fn late_ack_for_a_dropped_occurrence_is_an_idempotent_noop()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut schedule = sample_schedule();
        let mut acks_by_revise = Vec::new();
        let mut latest_id = None;
        for ordinal in 0..3_u64 {
            let occurrence = Occurrence::planned_from_schedule(
                &schedule,
                OccurrenceOrdinal(ordinal),
                Utc::now() + Duration::hours(i64::try_from(ordinal)? + 1),
            )?;
            latest_id = Some(occurrence.occurrence_id.clone());
            let (revised, acks) = revise_superseding(schedule, vec![occurrence])?;
            assert_eq!(acks.len(), 1);
            acks_by_revise.push(acks);
            schedule = revised;
        }
        let latest_id = latest_id.ok_or("three revises supersede three occurrences")?;
        assert_eq!(schedule.revision, ScheduleRevision(4));
        assert_eq!(
            schedule.superseded_ack_ids,
            BTreeSet::from([latest_id]),
            "after three revises the set holds the third supersession's acknowledgement only"
        );

        let dropped = acks_by_revise[0][0].clone();
        assert_eq!(dropped.superseding_revision(), ScheduleRevision(2));
        let late = Schedule::apply(
            Some(schedule.clone()),
            ScheduleLifecycleInput::ConfirmOccurrencesSuperseded {
                ack: dropped.clone(),
            },
        )?;
        assert!(late.effects().is_empty());
        assert!(!late.revision_bumped());
        let after = late.into_effectless_schedule()?;
        assert_eq!(after, schedule, "the late acknowledgement changes nothing");
        assert!(!after.superseded_ack_ids.contains(dropped.occurrence_id()));
        validate_schedule_machine_projection(&after)?;

        let fed = crate::store::apply_supersession_feedback(
            schedule.clone(),
            vec![dropped, acks_by_revise[1][0].clone()],
        )?;
        assert_eq!(
            fed, schedule,
            "late acknowledgements through the store's feedback seam change nothing"
        );

        let round_trip: Schedule = serde_json::from_value(serde_json::to_value(&after)?)?;
        assert_eq!(round_trip.superseded_ack_ids, after.superseded_ack_ids);
        assert_eq!(round_trip.machine_state, after.machine_state);
        Ok(())
    }

    /// Delete supersedes like a revise: it drops the earlier supersessions'
    /// ids and records its own acknowledgements. A late acknowledgement of a
    /// dropped id changes nothing on the deleted schedule, and a repeated
    /// delete keeps the delete's own acknowledgements.
    #[test]
    fn delete_drops_earlier_acks_and_records_its_own() -> Result<(), Box<dyn std::error::Error>> {
        let schedule = sample_schedule();
        let first = Occurrence::planned_from_schedule(
            &schedule,
            OccurrenceOrdinal(0),
            Utc::now() + Duration::hours(1),
        )?;
        let (schedule, first_acks) = revise_superseding(schedule, vec![first])?;
        let second = Occurrence::planned_from_schedule(
            &schedule,
            OccurrenceOrdinal(1),
            Utc::now() + Duration::hours(2),
        )?;
        let second_id = second.occurrence_id.clone();

        let at_utc = Utc::now();
        let deleted = Schedule::apply(Some(schedule), ScheduleLifecycleInput::Delete { at_utc })?
            .into_schedule();
        assert_eq!(deleted.phase, SchedulePhase::Deleted);
        assert!(
            deleted.superseded_ack_ids.is_empty(),
            "delete must drop the revise's acknowledgements, found {:?}",
            deleted.superseded_ack_ids
        );
        validate_schedule_machine_projection(&deleted)?;

        let (_superseded, _effects, delete_acks) = second
            .apply(OccurrenceLifecycleInput::Supersede {
                superseded_by_revision: deleted.revision,
                at_utc,
            })?
            .into_parts_with_supersession_feedback();
        let deleted = crate::store::apply_supersession_feedback(deleted, delete_acks)?;
        assert_eq!(
            deleted.superseded_ack_ids,
            BTreeSet::from([second_id.clone()])
        );

        let late = crate::store::apply_supersession_feedback(deleted.clone(), first_acks)?;
        assert_eq!(late, deleted);

        let repeated = Schedule::apply(
            Some(deleted),
            ScheduleLifecycleInput::Delete { at_utc: Utc::now() },
        )?;
        assert!(repeated.effects().is_empty());
        assert_eq!(
            repeated.schedule().superseded_ack_ids,
            BTreeSet::from([second_id])
        );
        Ok(())
    }

    /// A schedule stored before the bound (0.9.8 and earlier) carries the
    /// acknowledgements of every earlier supersession in both copies. It
    /// loads unchanged, with no migration, and its next revise or its delete
    /// drops them; the schedule copy follows the machine copy through the
    /// drop.
    #[test]
    fn stored_schedule_with_a_large_ack_set_loads_and_its_next_revise_drops_it()
    -> Result<(), Box<dyn std::error::Error>> {
        const STORED_ACKS: usize = 256;
        const STORED_REVISION: u64 = 41;
        let mut row = serde_json::to_value(sample_schedule())?;
        let stored_ids: Vec<String> = (0..STORED_ACKS)
            .map(|_| Uuid::now_v7().to_string())
            .collect();
        row["revision"] = serde_json::json!(STORED_REVISION);
        row["machine_state"]["revision"] = serde_json::json!(STORED_REVISION);
        row["superseded_ack_ids"] = serde_json::json!(stored_ids);
        row["machine_state"]["superseded_ack_ids"] = serde_json::json!(stored_ids);

        let stored: Schedule = serde_json::from_value(row)?;
        assert_eq!(stored.revision, ScheduleRevision(STORED_REVISION));
        assert_eq!(
            stored
                .superseded_ack_ids
                .iter()
                .map(ToString::to_string)
                .collect::<BTreeSet<_>>(),
            stored_ids.iter().cloned().collect::<BTreeSet<_>>(),
            "the stored set loads unchanged"
        );
        assert_eq!(stored.machine_state.superseded_ack_ids.len(), STORED_ACKS);
        validate_schedule_machine_projection(&stored)?;

        let (revised, acks) = revise_superseding(stored.clone(), Vec::new())?;
        assert!(acks.is_empty());
        assert_eq!(revised.revision, ScheduleRevision(STORED_REVISION + 1));
        assert!(
            revised.superseded_ack_ids.is_empty(),
            "the next revise must drop the stored acknowledgements, found {}",
            revised.superseded_ack_ids.len()
        );
        assert!(revised.machine_state.superseded_ack_ids.is_empty());
        validate_schedule_machine_projection(&revised)?;

        let deleted = Schedule::apply(
            Some(stored),
            ScheduleLifecycleInput::Delete { at_utc: Utc::now() },
        )?
        .into_schedule();
        assert!(
            deleted.superseded_ack_ids.is_empty(),
            "the next delete must drop the stored acknowledgements, found {}",
            deleted.superseded_ack_ids.len()
        );
        validate_schedule_machine_projection(&deleted)?;
        Ok(())
    }

    #[test]
    fn config_only_update_emits_no_machine_effects() -> Result<(), Box<dyn std::error::Error>> {
        let schedule = sample_schedule();

        let mutator = Schedule::apply(
            Some(schedule.clone()),
            ScheduleLifecycleInput::Update {
                request: UpdateScheduleRequest {
                    name: Some("renamed".into()),
                    ..UpdateScheduleRequest::default()
                },
                at_utc: Utc::now(),
            },
        )?;

        assert_eq!(mutator.schedule.revision, schedule.revision);
        assert!(!mutator.revision_bumped);
        assert_eq!(mutator.schedule.config.name.as_deref(), Some("renamed"));
        assert!(mutator.effects.is_empty());
        Ok(())
    }

    #[test]
    fn supersede_effect_timestamp_comes_from_revise_authority()
    -> Result<(), Box<dyn std::error::Error>> {
        let schedule = sample_schedule();
        let at_utc = DateTime::from_timestamp(1_700_000_000, 0).expect("test timestamp is valid");

        let mutator = Schedule::apply(
            Some(schedule.clone()),
            ScheduleLifecycleInput::Update {
                request: UpdateScheduleRequest {
                    expected_revision: Some(schedule.revision),
                    trigger: Some(TriggerSpec::Once {
                        due_at_utc: at_utc + Duration::minutes(30),
                    }),
                    ..UpdateScheduleRequest::default()
                },
                at_utc,
            },
        )?;

        assert!(mutator.effects.iter().any(|effect| matches!(
            effect,
            ScheduleLifecycleEffect::SupersedePendingOccurrences {
                superseding_revision,
                at_utc: effect_at_utc,
            } if *superseding_revision == mutator.schedule.revision && *effect_at_utc == at_utc
        )));
        Ok(())
    }

    #[test]
    fn create_uses_generated_planning_defaults() -> Result<(), Box<dyn std::error::Error>> {
        let mut request = CreateScheduleRequest {
            name: None,
            description: None,
            trigger: TriggerSpec::Once {
                due_at_utc: Utc::now() + Duration::minutes(1),
            },
            target: TargetBinding::session(SessionTargetBinding::ExactSession {
                session_id: meerkat_core::SessionId::new(),
                action: ScheduledSessionAction::Prompt {
                    prompt: ContentInput::from("scheduled hello"),
                    system_prompt: None,
                    render_metadata: None,
                    skill_refs: Vec::new(),
                    additional_instructions: Vec::new(),
                },
            }),
            misfire_policy: MisfirePolicy::Skip,
            overlap_policy: OverlapPolicy::SkipIfRunning,
            missing_target_policy: MissingTargetPolicy::MarkMisfired,
            labels: BTreeMap::new(),
            planning_horizon_days: None,
            planning_horizon_occurrences: None,
        };

        let schedule = Schedule::new(request.clone())?;
        assert_eq!(
            schedule.config.planning_horizon_days,
            sched_dsl::ScheduleLifecycleMachineState::default().planning_horizon_days as u32
        );
        assert_eq!(
            schedule.config.planning_horizon_occurrences,
            sched_dsl::ScheduleLifecycleMachineState::default().planning_horizon_occurrences as u32
        );

        request.planning_horizon_days = Some(7);
        request.planning_horizon_occurrences = Some(9);
        let schedule = Schedule::new(request)?;
        assert_eq!(schedule.config.planning_horizon_days, 7);
        assert_eq!(schedule.config.planning_horizon_occurrences, 9);
        Ok(())
    }

    #[test]
    fn schedule_transition_errors_preserve_generated_refusal()
    -> Result<(), Box<dyn std::error::Error>> {
        let schedule = sample_schedule();
        let paused = Schedule::apply(
            Some(schedule),
            ScheduleLifecycleInput::Pause { at_utc: Utc::now() },
        )?
        .into_schedule();

        let error = Schedule::apply(
            Some(paused),
            ScheduleLifecycleInput::RecordPlanningWindow {
                planning_cursor_utc: Utc::now(),
                next_occurrence_ordinal: OccurrenceOrdinal(1),
            },
        )
        .expect_err("paused schedules should reject planning-window advancement");

        assert!(matches!(
            error,
            ScheduleLifecycleError::TransitionRejected { .. }
        ));
        Ok(())
    }

    #[test]
    fn schedule_inputs_reject_negative_timestamp_projection()
    -> Result<(), Box<dyn std::error::Error>> {
        let schedule = sample_schedule();
        let before_epoch = DateTime::from_timestamp(-1, 0).expect("test timestamp is valid");

        let error = Schedule::apply(
            Some(schedule),
            ScheduleLifecycleInput::RecordPlanningWindow {
                planning_cursor_utc: before_epoch,
                next_occurrence_ordinal: OccurrenceOrdinal(1),
            },
        )
        .expect_err("negative timestamp should fail before generated input");

        assert!(matches!(
            error,
            ScheduleLifecycleError::InvalidTimestampMillis {
                field: "planning_cursor_utc",
                millis: -1000
            }
        ));
        Ok(())
    }

    #[test]
    fn schedule_write_back_rejects_invalid_generated_facts()
    -> Result<(), Box<dyn std::error::Error>> {
        let schedule = sample_schedule();
        let mut dsl = schedule_machine_state_for_test(&schedule)?;
        dsl.planning_cursor_utc_ms = Some(u64::MAX);
        let mut projected = schedule.clone();

        assert!(matches!(
            write_back_schedule(&dsl, &mut projected),
            Err(ScheduleLifecycleError::InvalidPlanningCursorUtcMillis { millis })
                if millis == u64::MAX
        ));

        let mut dsl = schedule_machine_state_for_test(&schedule)?;
        dsl.superseded_ack_ids
            .insert(sched_dsl::OccurrenceId("not-a-uuid".into()));
        let mut projected = schedule;

        assert!(matches!(
            write_back_schedule(&dsl, &mut projected),
            Err(ScheduleLifecycleError::InvalidSupersededAckId { id, .. })
                if id == "not-a-uuid"
        ));
        Ok(())
    }

    #[test]
    fn occurrence_write_back_rejects_invalid_generated_facts()
    -> Result<(), Box<dyn std::error::Error>> {
        let occurrence = sample_occurrence();
        let input = OccurrenceLifecycleInput::LeaseExpired { at_utc: Utc::now() };

        let mut dsl = occurrence_machine_state_for_test(&occurrence)?;
        dsl.claim_token = Some(occ_dsl::ClaimToken("not-a-uuid".into()));
        let mut projected = occurrence.clone();
        assert!(matches!(
            write_back_occurrence(&dsl, &mut projected, &input),
            Err(OccurrenceLifecycleError::InvalidClaimToken { token, .. })
                if token == "not-a-uuid"
        ));

        let mut dsl = occurrence_machine_state_for_test(&occurrence)?;
        dsl.attempt_count = u64::from(u32::MAX) + 1;
        let mut projected = occurrence;
        assert!(matches!(
            write_back_occurrence(&dsl, &mut projected, &input),
            Err(OccurrenceLifecycleError::InvalidAttemptCount { value })
                if value == u64::from(u32::MAX) + 1
        ));
        Ok(())
    }

    #[test]
    fn planning_window_effect_maps_generated_payload() -> Result<(), Box<dyn std::error::Error>> {
        let effect = map_schedule_effect(
            &sched_dsl::ScheduleLifecycleEffect::PlanningWindowRecorded {
                planning_cursor_utc_ms: 1234,
                next_occurrence_ordinal: 99,
            },
        )?;

        assert_eq!(
            effect,
            ScheduleLifecycleEffect::PlanningWindowRecorded {
                planning_cursor_utc: DateTime::from_timestamp_millis(1234)
                    .expect("test timestamp should be valid"),
                next_occurrence_ordinal: OccurrenceOrdinal(99),
            }
        );

        assert!(matches!(
            map_schedule_effect(
                &sched_dsl::ScheduleLifecycleEffect::PlanningWindowRecorded {
                    planning_cursor_utc_ms: u64::MAX,
                    next_occurrence_ordinal: 1,
                },
            ),
            Err(ScheduleLifecycleError::InvalidPlanningCursorUtcMillis { millis })
                if millis == u64::MAX
        ));
        Ok(())
    }

    #[test]
    fn delete_from_deleted_is_idempotent_noop() -> Result<(), Box<dyn std::error::Error>> {
        let schedule = sample_schedule();
        let first = Schedule::apply(
            Some(schedule),
            ScheduleLifecycleInput::Delete { at_utc: Utc::now() },
        )?;
        let deleted = first.schedule;
        let revision_after_delete = deleted.revision;

        let second = Schedule::apply(
            Some(deleted),
            ScheduleLifecycleInput::Delete { at_utc: Utc::now() },
        )?;

        assert_eq!(second.schedule.phase, SchedulePhase::Deleted);
        assert_eq!(second.schedule.revision, revision_after_delete);
        assert!(second.effects.is_empty());
        assert!(!second.revision_bumped);
        Ok(())
    }

    // -----------------------------------------------------------------------
    // 0.7.2 D2a — completion inputs are total in Superseded (machine level)
    // -----------------------------------------------------------------------

    /// Claimed → dispatched → awaiting → superseded, with the claim evidence
    /// (claim token, attempt count) still attached — exactly the shape a
    /// schedule-commit supersession sweep leaves behind while the completion
    /// waiter is still in flight.
    fn sample_superseded_in_flight_occurrence() -> Occurrence {
        sample_claimed_occurrence()
            .apply(OccurrenceLifecycleInput::DispatchStarted {
                correlation_id: Some("corr-late".into()),
                at_utc: Utc::now(),
            })
            .expect("dispatch should pass generated authority")
            .into_occurrence()
            .apply(OccurrenceLifecycleInput::AwaitCompletion { at_utc: Utc::now() })
            .expect("await should pass generated authority")
            .into_occurrence()
            .apply(OccurrenceLifecycleInput::Supersede {
                superseded_by_revision: ScheduleRevision(2),
                at_utc: Utc::now(),
            })
            .expect("supersede should pass generated authority")
            .into_occurrence()
    }

    #[test]
    fn late_complete_after_supersession_is_accepted_as_typed_record() {
        let superseded = sample_superseded_in_flight_occurrence();
        let token_before = superseded.claim_token();
        let receipt_stage_before = superseded.machine_state.receipt_stage;

        let mutator = superseded
            .apply(OccurrenceLifecycleInput::Complete { at_utc: Utc::now() })
            .expect("Complete after supersession must be accepted, not guard-rejected");

        assert_eq!(
            mutator.effects,
            vec![
                OccurrenceLifecycleEffect::LateCompletionResolutionRecorded {
                    resolution: LateCompletionResolutionClass::DeliveryCompleted,
                }
            ],
            "late arrival must emit exactly the late-record effect (no terminal effects)"
        );
        let updated = mutator.into_occurrence();
        assert_eq!(updated.phase, OccurrencePhase::Superseded);
        assert_eq!(updated.superseded_by_revision, Some(ScheduleRevision(2)));
        assert_eq!(
            updated.machine_state.late_completion_resolution,
            Some(occ_dsl::LateCompletionResolutionClass::DeliveryCompleted)
        );
        assert!(
            updated
                .machine_state
                .late_completion_recorded_at_utc_ms
                .is_some()
        );
        assert_eq!(
            updated.claim_token(),
            token_before,
            "late arrival must not touch claim evidence"
        );
        assert_eq!(
            updated.machine_state.receipt_stage, receipt_stage_before,
            "late arrival must not mint fresh receipt authority"
        );
    }

    #[test]
    fn late_runtime_completion_after_supersession_records_outcome_class() {
        let cases = [
            (
                RuntimeCompletionOutcome::Completed,
                occ_dsl::LateCompletionResolutionClass::RuntimeCompleted,
            ),
            (
                RuntimeCompletionOutcome::CallbackPending,
                occ_dsl::LateCompletionResolutionClass::RuntimeRejected,
            ),
            (
                RuntimeCompletionOutcome::Cancelled,
                occ_dsl::LateCompletionResolutionClass::RuntimeRejected,
            ),
            (
                RuntimeCompletionOutcome::Abandoned,
                occ_dsl::LateCompletionResolutionClass::RuntimeRejected,
            ),
            (
                RuntimeCompletionOutcome::RuntimeTerminated,
                occ_dsl::LateCompletionResolutionClass::RuntimeTransportError,
            ),
            (
                RuntimeCompletionOutcome::FinalizationFailed,
                occ_dsl::LateCompletionResolutionClass::RuntimeInternalError,
            ),
        ];
        for (outcome, expected_class) in cases {
            let updated = sample_superseded_in_flight_occurrence()
                .apply(OccurrenceLifecycleInput::ResolveRuntimeCompletion {
                    outcome,
                    detail: Some("late runtime resolution".into()),
                    at_utc: Utc::now(),
                })
                .expect("ResolveRuntimeCompletion after supersession must be accepted")
                .into_occurrence();
            assert_eq!(updated.phase, OccurrencePhase::Superseded);
            assert_eq!(
                updated.machine_state.late_completion_resolution,
                Some(expected_class),
                "outcome {outcome:?} must record class {expected_class:?}"
            );
            assert_eq!(
                updated.machine_state.late_completion_detail.as_deref(),
                Some("late runtime resolution")
            );
            assert_eq!(
                updated.failure_class, None,
                "late arrival must not rewrite terminal failure facts"
            );
        }
    }

    #[test]
    fn late_delivery_failures_after_supersession_record_typed_classes() {
        let updated = sample_superseded_in_flight_occurrence()
            .apply(OccurrenceLifecycleInput::ResolveDeliveryCompletionFailure {
                reason: DeliveryCompletionFailureReason::CompletionFutureFailed,
                detail: Some("completion future dropped".into()),
                at_utc: Utc::now(),
            })
            .expect("ResolveDeliveryCompletionFailure after supersession must be accepted")
            .into_occurrence();
        assert_eq!(updated.phase, OccurrencePhase::Superseded);
        assert_eq!(
            updated.machine_state.late_completion_resolution,
            Some(occ_dsl::LateCompletionResolutionClass::DeliveryCompletionFailed)
        );

        let updated = sample_superseded_in_flight_occurrence()
            .apply(OccurrenceLifecycleInput::ResolveDeliveryFailure {
                reason: DeliveryFailureReason::TransportError,
                detail: Some("adapter transport failure".into()),
                at_utc: Utc::now(),
            })
            .expect("ResolveDeliveryFailure after supersession must be accepted")
            .into_occurrence();
        assert_eq!(updated.phase, OccurrencePhase::Superseded);
        assert_eq!(
            updated.machine_state.late_completion_resolution,
            Some(occ_dsl::LateCompletionResolutionClass::DeliveryFailed)
        );
    }

    #[test]
    fn supersede_after_supersession_is_idempotent_noop() {
        let superseded = sample_superseded_in_flight_occurrence();
        let mutator = superseded
            .apply(OccurrenceLifecycleInput::Supersede {
                superseded_by_revision: ScheduleRevision(7),
                at_utc: Utc::now(),
            })
            .expect("duplicate Supersede must be an idempotent no-op, not a guard rejection");
        assert!(
            mutator.effects.is_empty(),
            "duplicate Supersede must emit zero effects so the shell mints no duplicate receipt"
        );
        let updated = mutator.into_occurrence();
        assert_eq!(
            updated.superseded_by_revision,
            Some(ScheduleRevision(2)),
            "the first recorded supersession wins"
        );
    }

    #[test]
    fn await_completion_and_release_lease_after_supersession_are_noops() {
        let superseded = sample_superseded_in_flight_occurrence();
        let token_before = superseded.claim_token();

        let mutator = superseded
            .apply(OccurrenceLifecycleInput::AwaitCompletion { at_utc: Utc::now() })
            .expect("AwaitCompletion after supersession must be a benign no-op");
        assert!(mutator.effects.is_empty());
        let updated = mutator.into_occurrence();
        assert_eq!(updated.phase, OccurrencePhase::Superseded);
        assert_eq!(updated.claim_token(), token_before);

        let mutator = updated
            .apply(OccurrenceLifecycleInput::ReleaseLeaseForPausedSchedule { at_utc: Utc::now() })
            .expect("ReleaseLeaseForPausedSchedule after supersession must be a benign no-op");
        assert!(mutator.effects.is_empty());
        let updated = mutator.into_occurrence();
        assert_eq!(updated.phase, OccurrencePhase::Superseded);
        assert_eq!(
            updated.machine_state.receipt_stage,
            Some(occ_dsl::DeliveryReceiptStage::Superseded),
            "release on a superseded occurrence must not mint lease-expired receipt authority"
        );
    }

    #[test]
    fn classify_completion_supersession_on_superseded_snapshot_is_already_superseded() {
        for schedule_phase in [
            SchedulePhase::Active,
            SchedulePhase::Paused,
            SchedulePhase::Deleted,
        ] {
            let verdict = sample_superseded_in_flight_occurrence()
                .classify_completion_supersession(schedule_phase, ScheduleRevision(3))
                .expect("classification must stay total on a superseded snapshot");
            assert_eq!(
                verdict.disposition,
                CompletionSupersessionDisposition::AlreadySuperseded,
                "schedule phase {schedule_phase:?}"
            );
            assert_eq!(verdict.superseded_by_revision, None);
        }
    }

    #[test]
    fn classify_stale_completion_arrival_is_total_in_every_phase() {
        let fixtures: Vec<Occurrence> = vec![
            sample_occurrence(),
            sample_claimed_occurrence(),
            sample_completed_occurrence(),
            sample_superseded_in_flight_occurrence(),
        ];
        for occurrence in fixtures {
            let phase_before = occurrence.phase;
            let mutator = occurrence
                .apply(OccurrenceLifecycleInput::ClassifyStaleCompletionArrival {
                    trigger: StaleCompletionArrivalTrigger::Complete,
                })
                .expect("stale-arrival classification must be total in every phase");
            assert_eq!(
                mutator.effects,
                vec![
                    OccurrenceLifecycleEffect::StaleCompletionArrivalClassified {
                        phase: phase_before,
                        trigger: StaleCompletionArrivalTrigger::Complete,
                    }
                ]
            );
            let updated = mutator.into_occurrence();
            assert_eq!(
                updated.phase, phase_before,
                "stale-arrival classification must never mutate lifecycle state"
            );
            assert_eq!(updated.machine_state.stale_completion_arrivals, 1);
        }
    }

    #[test]
    fn classify_stale_completion_arrival_rejects_non_completion_trigger() {
        // The domain trigger enum only carries completion-shaped variants, so
        // the refusal is only reachable at the generated-authority seam.
        let occurrence = sample_claimed_occurrence();
        let dsl_state = occurrence_machine_state_for_test(&occurrence)
            .expect("test occurrence projection should match authority");
        let mut authority =
            occ_dsl::OccurrenceLifecycleMachineAuthority::recover_from_state(dsl_state)
                .expect("test occurrence state should recover");
        let error = occ_dsl::OccurrenceLifecycleMachineMutator::apply(
            &mut authority,
            occ_dsl::OccurrenceLifecycleInput::ClassifyStaleCompletionArrival {
                trigger: occ_dsl::OccurrenceLifecycleInputVariant::Claim,
            },
        )
        .expect_err("non-completion-shaped trigger must be refused");
        let class = classify_occurrence_transition_failure(&mut authority, &error)
            .expect("refusal must classify to a typed public class");
        assert_eq!(
            class,
            occ_dsl::OccurrenceTransitionFailureClassKind::StaleCompletionArrivalClassificationRejected
        );
    }

    #[test]
    fn late_terminal_inputs_remain_rejected_in_non_superseded_terminal_phases() {
        // "Not allowed" stays meaningful: totality is scoped to the
        // post-teardown Superseded state, not blanket-accepted everywhere.
        let completed = sample_completed_occurrence();
        let error = completed
            .apply(OccurrenceLifecycleInput::Complete { at_utc: Utc::now() })
            .expect_err("Complete on a Completed occurrence must still be refused");
        assert!(
            matches!(error, OccurrenceLifecycleError::NotLiveForTerminal),
            "got {error:?}"
        );
    }

    fn materializing_target() -> TargetBinding {
        TargetBinding::session(SessionTargetBinding::materialize_on_demand(
            crate::types::SessionMaterializationSpec {
                model: "claude-sonnet-4-6".into(),
                system_prompt: None,
                max_tokens: None,
                provider: None,
                output_schema: None,
                structured_output_retries: None,
                provider_params: None,
                comms_name: None,
                peer_meta: None,
                labels: BTreeMap::new(),
                preload_skills: Vec::new(),
                additional_instructions: Vec::new(),
                realm_id: None,
                instance_id: None,
                backend: None,
                config_generation: None,
                keep_alive: false,
                app_context: None,
            },
            ScheduledSessionAction::Prompt {
                prompt: ContentInput::from("scheduled prompt"),
                system_prompt: None,
                render_metadata: None,
                skill_refs: Vec::new(),
                additional_instructions: Vec::new(),
            },
        ))
    }

    /// A planned occurrence of a schedule with `target`, claimed and
    /// dispatching.
    fn dispatching_occurrence(target: TargetBinding) -> Occurrence {
        let schedule = Schedule::new(CreateScheduleRequest {
            name: Some("dispatching".into()),
            description: None,
            trigger: TriggerSpec::Once {
                due_at_utc: Utc::now() + Duration::minutes(1),
            },
            target,
            misfire_policy: MisfirePolicy::Skip,
            overlap_policy: OverlapPolicy::AllowConcurrent,
            missing_target_policy: MissingTargetPolicy::Skip,
            labels: BTreeMap::new(),
            planning_horizon_days: Some(1),
            planning_horizon_occurrences: Some(1),
        })
        .expect("schedule");
        Occurrence::planned_from_schedule(&schedule, crate::OccurrenceOrdinal(0), Utc::now())
            .expect("plan")
            .apply(OccurrenceLifecycleInput::Claim {
                owner_id: "owner".into(),
                at_utc: Utc::now(),
                lease_expires_at_utc: Utc::now() + Duration::seconds(30),
                claim_token: Uuid::now_v7(),
                admission_evidence: AdmissionEvidence::None,
            })
            .expect("claim")
            .into_occurrence()
            .apply(OccurrenceLifecycleInput::DispatchStarted {
                correlation_id: Some("corr-1".into()),
                at_utc: Utc::now(),
            })
            .expect("dispatch")
            .into_occurrence()
    }

    /// `target` with its materialize-on-demand binding set to `session_id`,
    /// whatever it was bound to before.
    fn bound_to(target: &TargetBinding, session_id: &meerkat_core::SessionId) -> TargetBinding {
        let mut target = target.clone();
        if let TargetBinding::Session(binding) = &mut target
            && let SessionTargetBinding::MaterializeOnDemandSession {
                bound_session_id, ..
            } = binding.as_mut()
        {
            *bound_session_id = Some(session_id.clone());
        }
        target
    }

    /// Generated occurrence authority lets a dispatching occurrence record the
    /// session its dispatch materialized once, accepts an exact replay, and
    /// refuses a rebinding.
    #[test]
    fn a_dispatching_occurrence_records_its_materialized_session_once() {
        let dispatching = dispatching_occurrence(materializing_target());
        assert_eq!(dispatching.phase, crate::OccurrencePhase::Dispatching);
        let materialized = meerkat_core::SessionId::new();
        let recorded = dispatching
            .clone()
            .apply(OccurrenceLifecycleInput::SyncTargetSnapshot {
                target_snapshot: bound_to(&dispatching.target_snapshot, &materialized),
            })
            .expect("a dispatching occurrence records the session it materialized")
            .into_occurrence();
        assert_eq!(recorded.phase, crate::OccurrencePhase::Dispatching);
        assert_eq!(
            crate::types::target_materialized_session_id(&recorded.target_snapshot),
            Some(materialized)
        );

        recorded
            .clone()
            .apply(OccurrenceLifecycleInput::SyncTargetSnapshot {
                target_snapshot: recorded.target_snapshot.clone(),
            })
            .expect("an exact replay of the recorded binding is accepted");

        let rebind = recorded
            .clone()
            .apply(OccurrenceLifecycleInput::SyncTargetSnapshot {
                target_snapshot: bound_to(
                    &recorded.target_snapshot,
                    &meerkat_core::SessionId::new(),
                ),
            })
            .map(|_| ());
        assert!(
            matches!(
                rebind,
                Err(crate::OccurrenceLifecycleError::TargetSyncRejected)
            ),
            "a dispatching occurrence never rebinds: {rebind:?}"
        );
    }

    /// The shell records exactly the delivered session on the occurrence's
    /// own target: an already-bound occurrence never takes another session,
    /// an exact replay is a no-op, and a target that does not materialize a
    /// session records nothing.
    #[tokio::test]
    async fn recording_a_dispatch_materialization_never_rebinds_or_retargets() {
        let service =
            crate::ScheduleService::new(std::sync::Arc::new(crate::MemoryScheduleStore::new()));
        let dispatching = dispatching_occurrence(materializing_target());
        let materialized = meerkat_core::SessionId::new();
        let recorded = dispatching
            .clone()
            .apply(OccurrenceLifecycleInput::SyncTargetSnapshot {
                target_snapshot: bound_to(&dispatching.target_snapshot, &materialized),
            })
            .expect("record")
            .into_occurrence();

        let replayed = service
            .record_dispatch_materialization(recorded.clone(), &materialized)
            .await
            .expect("an exact replay is a no-op");
        assert_eq!(replayed, recorded);

        let rebind = service
            .record_dispatch_materialization(recorded, &meerkat_core::SessionId::new())
            .await;
        assert!(
            matches!(rebind, Err(crate::ScheduleDomainError::Internal(_))),
            "an already-bound occurrence never takes another session: {rebind:?}"
        );

        let exact =
            dispatching_occurrence(TargetBinding::session(SessionTargetBinding::ExactSession {
                session_id: meerkat_core::SessionId::new(),
                action: ScheduledSessionAction::Prompt {
                    prompt: ContentInput::from("scheduled prompt"),
                    system_prompt: None,
                    render_metadata: None,
                    skill_refs: Vec::new(),
                    additional_instructions: Vec::new(),
                },
            }));
        let retarget = service
            .record_dispatch_materialization(exact, &meerkat_core::SessionId::new())
            .await;
        assert!(
            matches!(retarget, Err(crate::ScheduleDomainError::Internal(_))),
            "a target that materializes no session records none: {retarget:?}"
        );
    }

    // --- Recorded dispatch evidence: reclaim, exhaustion, unproven late ---

    /// A Pending occurrence (Skip, 30 s grace) due 60 s ago whose earlier
    /// `attempts` were claimed inside its window and lost their lease.
    /// Returns it with its due time.
    fn pending_after_attempts(attempts: u32) -> (Occurrence, DateTime<Utc>) {
        let due = Utc::now() - Duration::seconds(60);
        let mut occurrence =
            Occurrence::planned_from_schedule(&sample_schedule(), crate::OccurrenceOrdinal(0), due)
                .expect("planning should pass generated authority");
        for _ in 0..attempts {
            occurrence = occurrence
                .apply(OccurrenceLifecycleInput::Claim {
                    owner_id: "owner".into(),
                    at_utc: due,
                    lease_expires_at_utc: due + Duration::seconds(1),
                    claim_token: Uuid::now_v7(),
                    admission_evidence: AdmissionEvidence::None,
                })
                .expect("a claim inside the window should pass")
                .into_occurrence()
                .apply(OccurrenceLifecycleInput::LeaseExpired {
                    at_utc: due + Duration::seconds(1),
                })
                .expect("the lease expiry should pass")
                .into_occurrence();
        }
        assert_eq!(occurrence.attempt_count, attempts);
        assert_eq!(occurrence.phase, OccurrencePhase::Pending);
        (occurrence, due)
    }

    fn reclaim(
        occurrence: &Occurrence,
        at_utc: DateTime<Utc>,
        admission_evidence: AdmissionEvidence,
    ) -> Result<OccurrenceLifecycleMutator, OccurrenceLifecycleError> {
        occurrence.clone().apply(OccurrenceLifecycleInput::Claim {
            owner_id: "restarted".into(),
            at_utc,
            lease_expires_at_utc: at_utc + Duration::seconds(30),
            claim_token: Uuid::now_v7(),
            admission_evidence,
        })
    }

    #[test]
    fn admission_evidence_reads_the_receipt_history() {
        use DeliveryReceiptStage::{
            Claimed, DispatchAccepted, DispatchStarted, LeaseExpired, Planned,
        };
        assert_eq!(
            AdmissionEvidence::from_receipt_stages([Planned, Claimed, LeaseExpired]),
            AdmissionEvidence::None
        );
        assert_eq!(
            AdmissionEvidence::from_receipt_stages([Claimed, DispatchStarted, LeaseExpired]),
            AdmissionEvidence::Started
        );
        assert_eq!(
            AdmissionEvidence::from_receipt_stages([
                DispatchStarted,
                DispatchAccepted,
                LeaseExpired,
                DispatchStarted,
            ]),
            AdmissionEvidence::Accepted,
            "an earlier attempt's admission stays proof after a later start"
        );
    }

    /// The pending arms partition by the deadline, the evidence and the cap:
    /// past the deadline no evidence misfires, evidence reclaims below the
    /// cap and is exhausted at it; inside the window every row claims.
    #[test]
    fn recorded_dispatch_partitions_due_classification_past_the_deadline() {
        use AdmissionEvidence::{Accepted, None as NoEvidence, Started};
        let cap = ADMITTED_RECLAIM_ATTEMPT_CAP;

        let (occurrence, due) = pending_after_attempts(1);
        let inside = due + Duration::seconds(10);
        let past = due + Duration::seconds(45);
        let classify = |occurrence: &Occurrence, at, evidence| {
            occurrence
                .classify_due_action(at, evidence)
                .expect("due classification should pass")
        };
        assert_eq!(
            classify(&occurrence, past, NoEvidence),
            Some(OccurrenceDueAction::MisfireRequired)
        );
        assert_eq!(
            classify(&occurrence, inside, NoEvidence),
            Some(OccurrenceDueAction::ClaimEligible)
        );
        for evidence in [Started, Accepted] {
            assert_eq!(
                classify(&occurrence, past, evidence),
                Some(OccurrenceDueAction::ClaimEligible),
                "{evidence:?} past the deadline reclaims"
            );
            assert_eq!(
                classify(&occurrence, inside, evidence),
                Some(OccurrenceDueAction::ClaimEligible)
            );
        }

        let (below, due) = pending_after_attempts(cap - 1);
        assert_eq!(
            classify(&below, due + Duration::seconds(45), Accepted),
            Some(OccurrenceDueAction::ClaimEligible)
        );

        let (capped, due) = pending_after_attempts(cap);
        for evidence in [Started, Accepted] {
            assert_eq!(
                classify(&capped, due + Duration::seconds(45), evidence),
                Some(OccurrenceDueAction::ReclaimExhausted),
                "{evidence:?} at the cap past the deadline is exhausted"
            );
            assert_eq!(
                classify(&capped, due + Duration::seconds(10), evidence),
                Some(OccurrenceDueAction::ClaimEligible),
                "{evidence:?} at the cap inside the window still claims"
            );
        }
        assert_eq!(
            classify(&capped, due + Duration::seconds(45), NoEvidence),
            Some(OccurrenceDueAction::MisfireRequired)
        );
    }

    /// A reclaim's claim tells the driver which redelivery its evidence
    /// proves: a recorded admission always (inside the window too), a
    /// recorded start only past the deadline.
    #[test]
    fn a_reclaim_claim_names_the_redelivery_its_evidence_proves() {
        let (occurrence, due) = pending_after_attempts(1);
        let inside = due + Duration::seconds(10);
        let past = due + Duration::seconds(45);
        let effects = |at, evidence| {
            reclaim(&occurrence, at, evidence)
                .expect("the reclaim should pass")
                .effects()
                .to_vec()
        };
        for at in [inside, past] {
            assert_eq!(
                effects(at, AdmissionEvidence::Accepted),
                vec![
                    OccurrenceLifecycleEffect::Claimed,
                    OccurrenceLifecycleEffect::ReclaimedAfterAdmission
                ]
            );
        }
        assert_eq!(
            effects(past, AdmissionEvidence::Started),
            vec![
                OccurrenceLifecycleEffect::Claimed,
                OccurrenceLifecycleEffect::ReclaimedAfterUnprovenDispatch
            ]
        );
        assert_eq!(
            effects(inside, AdmissionEvidence::Started),
            vec![OccurrenceLifecycleEffect::Claimed],
            "inside the window an unproven start claims as before"
        );
        assert!(
            reclaim(&occurrence, past, AdmissionEvidence::None).is_err(),
            "no evidence past the deadline is not reclaimed"
        );
        let reclaimed = reclaim(&occurrence, past, AdmissionEvidence::Accepted)
            .expect("the reclaim should pass")
            .into_occurrence();
        assert_eq!(reclaimed.attempt_count, 2);

        let (capped, due) = pending_after_attempts(ADMITTED_RECLAIM_ATTEMPT_CAP);
        assert!(
            reclaim(
                &capped,
                due + Duration::seconds(45),
                AdmissionEvidence::Accepted
            )
            .is_err(),
            "the cap ends the reclaims past the deadline"
        );
    }

    /// Exhaustion ends an admitted occurrence delivery_failed with lease_lost
    /// and an unproven one misfired, and fails closed without evidence,
    /// below the cap or inside the window.
    #[test]
    fn exhausted_reclaims_end_by_their_evidence_and_fail_closed_without_it() {
        let cap = ADMITTED_RECLAIM_ATTEMPT_CAP;
        let (capped, due) = pending_after_attempts(cap);
        let past = due + Duration::seconds(45);
        let resolve = |occurrence: &Occurrence, at, evidence| {
            occurrence
                .clone()
                .apply(OccurrenceLifecycleInput::ResolveReclaimExhausted {
                    admission_evidence: evidence,
                    detail: Some(occurrence.reclaim_exhausted_detail(evidence)),
                    at_utc: at,
                })
                .map(OccurrenceLifecycleMutator::into_occurrence)
        };

        let failed = resolve(&capped, past, AdmissionEvidence::Accepted)
            .expect("an admitted occurrence at the cap fails");
        assert_eq!(failed.phase, OccurrencePhase::DeliveryFailed);
        assert_eq!(
            failed.failure_class,
            Some(OccurrenceFailureClass::LeaseLost)
        );
        let detail = failed.failure_detail.as_deref().unwrap_or_default();
        assert!(
            detail.contains(&format!("{cap} times")) && detail.contains("never observed"),
            "{detail}"
        );

        let misfired = resolve(&capped, past, AdmissionEvidence::Started)
            .expect("an unproven occurrence at the cap misfires");
        assert_eq!(misfired.phase, OccurrencePhase::Misfired);

        assert!(resolve(&capped, past, AdmissionEvidence::None).is_err());
        assert!(
            resolve(
                &capped,
                due + Duration::seconds(10),
                AdmissionEvidence::Accepted
            )
            .is_err()
        );
        let (below, due) = pending_after_attempts(cap - 1);
        assert!(
            resolve(
                &below,
                due + Duration::seconds(45),
                AdmissionEvidence::Accepted
            )
            .is_err()
        );
    }

    /// The not-admitted answer of an unproven late redelivery ends it
    /// misfired, from Claimed or Dispatching, and fails closed for an
    /// admitted occurrence and inside the window.
    #[test]
    fn unproven_late_not_admitted_ends_misfired_and_fails_closed() {
        let (occurrence, due) = pending_after_attempts(1);
        let past = due + Duration::seconds(45);
        let claimed = reclaim(&occurrence, past, AdmissionEvidence::Started)
            .expect("the unproven reclaim should pass")
            .into_occurrence();
        let dispatching = claimed
            .clone()
            .apply(OccurrenceLifecycleInput::DispatchStarted {
                correlation_id: Some("stable-correlation".into()),
                at_utc: past,
            })
            .expect("dispatch start should pass")
            .into_occurrence();
        let resolve = |occurrence: &Occurrence, at, evidence| {
            occurrence
                .clone()
                .apply(OccurrenceLifecycleInput::ResolveUnprovenLateNotAdmitted {
                    admission_evidence: evidence,
                    detail: Some("the target never admitted it".to_string()),
                    at_utc: at,
                })
                .map(OccurrenceLifecycleMutator::into_occurrence)
        };
        for live in [&claimed, &dispatching] {
            let misfired =
                resolve(live, past, AdmissionEvidence::Started).expect("not admitted misfires");
            assert_eq!(misfired.phase, OccurrencePhase::Misfired);
            assert_eq!(misfired.failure_class, None);
            assert!(resolve(live, past, AdmissionEvidence::Accepted).is_err());
            assert!(
                resolve(
                    live,
                    due + Duration::seconds(10),
                    AdmissionEvidence::Started
                )
                .is_err()
            );
        }
    }

    /// A busy target ends an ordinary claim under skip_if_running, never a
    /// redelivery its claim named: the redelivery starts no new run and goes
    /// on to find the earlier admission, inside the window and past it. A
    /// missing target keeps the missing-target policy for a redelivery too.
    #[test]
    fn a_busy_target_skips_an_ordinary_claim_but_not_a_redelivery() {
        use crate::ExistingDelivery::{Admitted, UnprovenLate};
        let (occurrence, due) = pending_after_attempts(1);
        let inside = due + Duration::seconds(10);
        let past = due + Duration::seconds(45);
        let probe = |claimed: &Occurrence, at, outcome, redelivery| {
            claimed
                .clone()
                .apply(OccurrenceLifecycleInput::ResolveTargetProbe {
                    outcome,
                    redelivery,
                    detail: Some("target busy".to_string()),
                    at_utc: at,
                })
                .expect("the probe resolution should pass")
        };
        for (at, evidence, existing) in [
            (inside, AdmissionEvidence::Accepted, Admitted),
            (past, AdmissionEvidence::Accepted, Admitted),
            (past, AdmissionEvidence::Started, UnprovenLate),
        ] {
            let claimed = reclaim(&occurrence, at, evidence)
                .expect("the reclaim should pass")
                .into_occurrence();

            let continued = probe(
                &claimed,
                at,
                OccurrenceTargetProbeOutcome::Busy,
                Some(existing),
            );
            assert!(continued.effects().is_empty(), "{existing:?}");
            assert_eq!(
                continued.into_occurrence().phase,
                OccurrencePhase::Claimed,
                "a busy target keeps the {existing:?} redelivery claimed"
            );

            let skipped =
                probe(&claimed, at, OccurrenceTargetProbeOutcome::Busy, None).into_occurrence();
            assert_eq!(skipped.phase, OccurrencePhase::Skipped);
            assert_eq!(
                skipped.failure_class,
                Some(OccurrenceFailureClass::TargetBusy)
            );

            let missing = probe(
                &claimed,
                at,
                OccurrenceTargetProbeOutcome::Missing,
                Some(existing),
            )
            .into_occurrence();
            assert_eq!(
                (missing.phase, missing.failure_class),
                (
                    OccurrencePhase::Misfired,
                    Some(OccurrenceFailureClass::TargetMissing)
                ),
                "a missing target keeps the missing-target policy for {existing:?}"
            );
        }
    }

    /// The unproven side of the reclaim cap, each guard at its boundary. A
    /// reclaim whose only evidence is a dispatch start claims past the
    /// deadline below the cap and is refused at it
    /// (ClaimPendingAfterUnprovenDispatch), and its exhaustion is refused
    /// below the cap (ReclaimExhaustedAfterUnprovenDispatch). The rows above
    /// pin the admitted side and the due classification.
    #[test]
    fn the_unproven_dispatch_guards_hold_the_reclaim_cap() {
        let cap = ADMITTED_RECLAIM_ATTEMPT_CAP;
        let exhaust = |occurrence: &Occurrence, at| {
            occurrence
                .clone()
                .apply(OccurrenceLifecycleInput::ResolveReclaimExhausted {
                    admission_evidence: AdmissionEvidence::Started,
                    detail: Some(occurrence.reclaim_exhausted_detail(AdmissionEvidence::Started)),
                    at_utc: at,
                })
                .map(OccurrenceLifecycleMutator::into_occurrence)
        };

        let (below, due) = pending_after_attempts(cap - 1);
        let past = due + Duration::seconds(45);
        let reclaimed = reclaim(&below, past, AdmissionEvidence::Started)
            .expect("an unproven reclaim below the cap claims");
        assert_eq!(
            reclaimed.effects().to_vec(),
            vec![
                OccurrenceLifecycleEffect::Claimed,
                OccurrenceLifecycleEffect::ReclaimedAfterUnprovenDispatch
            ]
        );
        assert_eq!(reclaimed.into_occurrence().attempt_count, cap);
        assert!(
            exhaust(&below, past).is_err(),
            "an unproven occurrence below the cap is not exhausted"
        );

        let (capped, due) = pending_after_attempts(cap);
        let past = due + Duration::seconds(45);
        assert!(
            reclaim(&capped, past, AdmissionEvidence::Started).is_err(),
            "the cap ends an unproven reclaim past the deadline"
        );
        assert_eq!(
            exhaust(&capped, past)
                .expect("an unproven occurrence at the cap is exhausted")
                .phase,
            OccurrencePhase::Misfired
        );
    }

    /// The domain constant equals the occurrence machine's constant. No guard
    /// reads either: the guards spell the cap as a literal, and the cap rows
    /// above pin each guard at its boundary.
    #[test]
    fn the_reclaim_cap_is_the_occurrence_machines_cap() {
        assert_eq!(
            u64::from(ADMITTED_RECLAIM_ATTEMPT_CAP),
            crate::machines::occurrence_lifecycle::ADMITTED_RECLAIM_ATTEMPT_CAP
        );
    }
}
