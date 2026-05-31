//! Domain-facing lifecycle inputs, effects, errors, and mutators for schedules
//! and occurrences. The `apply()` methods live on `Schedule` / `Occurrence` in
//! `types.rs` and delegate to the DSL kernels in `machines::*`. This module
//! houses the pure data types consumed by callers and by the `ScheduleStore`
//! trait wire contract.

use crate::machines::occurrence_lifecycle as occ_dsl;
use crate::machines::schedule_lifecycle as sched_dsl;
use crate::store::PendingSupersession;
use crate::types::{
    CreateScheduleRequest, DeliveryCompletionFailureReason, DeliveryFailureReason, DeliveryReceipt,
    DeliveryReceiptStage, Occurrence, OccurrenceFailureClass, OccurrenceId, OccurrenceOrdinal,
    OccurrencePhase, OccurrenceTargetProbeOutcome, RuntimeCompletionOutcome,
    RuntimeDeliveryOutcome, Schedule, ScheduleId, SchedulePhase, ScheduleRevision, TargetBinding,
    TriggerSpec, UpdateScheduleRequest, delivery_receipt_id_from_authority,
    target_materialized_session_id, validate_occurrence_machine_projection,
    validate_schedule_machine_projection,
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
    pub schedule: Schedule,
    pub effects: Vec<ScheduleLifecycleEffect>,
    pub revision_bumped: bool,
    write_precondition: ScheduleWritePrecondition,
    pending_supersession: Option<PendingSupersession>,
}

impl ScheduleLifecycleMutator {
    pub fn into_schedule(self) -> Schedule {
        self.schedule
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
            precondition: self.write_precondition,
            pending_supersession: self.pending_supersession,
        }
    }
}

#[derive(Debug, Clone)]
pub struct AuthorizedScheduleWrite {
    schedule: Schedule,
    precondition: ScheduleWritePrecondition,
    pending_supersession: Option<PendingSupersession>,
}

impl AuthorizedScheduleWrite {
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

    pub fn into_schedule(self) -> Schedule {
        self.schedule
    }

    pub fn into_parts(self) -> (Schedule, Option<PendingSupersession>) {
        (self.schedule, self.pending_supersession)
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
    ClassifyDue {
        now_utc: DateTime<Utc>,
    },
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
    },
    DispatchStarted {
        correlation_id: Option<String>,
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
    ResolveTargetProbe {
        outcome: OccurrenceTargetProbeOutcome,
        detail: Option<String>,
        at_utc: DateTime<Utc>,
    },
    ResolveDueMisfire {
        detail: Option<String>,
        at_utc: DateTime<Utc>,
    },
    Supersede {
        superseded_by_revision: ScheduleRevision,
        at_utc: DateTime<Utc>,
    },
    LeaseExpired {
        at_utc: DateTime<Utc>,
    },
    ReleaseLeaseForPausedSchedule {
        at_utc: DateTime<Utc>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OccurrenceLifecycleEffect {
    Claimed,
    DispatchStarted,
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OccurrenceDueAction {
    ClaimEligible,
    MisfireRequired,
    LeaseExpired,
}

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
    pub occurrence: Occurrence,
    pub effects: Vec<OccurrenceLifecycleEffect>,
    write_precondition: OccurrenceWritePrecondition,
    supersession_acks: Vec<OccurrenceSupersessionAck>,
}

impl OccurrenceLifecycleMutator {
    pub fn into_occurrence(self) -> Occurrence {
        self.occurrence
    }

    pub fn into_parts(self) -> (Occurrence, Vec<OccurrenceLifecycleEffect>) {
        (self.occurrence, self.effects)
    }

    pub fn into_parts_with_supersession_feedback(
        self,
    ) -> (
        Occurrence,
        Vec<OccurrenceLifecycleEffect>,
        Vec<OccurrenceSupersessionAck>,
    ) {
        (self.occurrence, self.effects, self.supersession_acks)
    }

    pub fn into_authorized_write(self) -> AuthorizedOccurrenceWrite {
        AuthorizedOccurrenceWrite {
            occurrence: self.occurrence,
            precondition: self.write_precondition,
        }
    }
}

#[derive(Debug, Clone)]
pub struct AuthorizedOccurrenceWrite {
    occurrence: Occurrence,
    precondition: OccurrenceWritePrecondition,
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

    pub fn into_occurrence(self) -> Occurrence {
        self.occurrence
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

    pub fn classify_due_action(
        &self,
        now_utc: DateTime<Utc>,
    ) -> Result<Option<OccurrenceDueAction>, OccurrenceLifecycleError> {
        let mutator = self
            .clone()
            .apply(OccurrenceLifecycleInput::ClassifyDue { now_utc })?;
        due_action_from_effects(&mutator.effects)
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
    if dsl.target_binding_key != snapshot_key {
        return Err(OccurrenceLifecycleError::TargetBindingKeyMismatch {
            machine_key: dsl.target_binding_key.clone(),
            snapshot_key,
        });
    }
    let trigger_key = trigger_stable_key(&schedule.trigger)?;
    if dsl.trigger_key != trigger_key {
        return Err(OccurrenceLifecycleError::TargetBindingKeyMismatch {
            machine_key: dsl.trigger_key.clone(),
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
        claimed_by: dsl.claimed_by.clone(),
        lease_expires_at_utc: occurrence_optional_datetime_from_dsl(
            dsl.lease_expires_at_utc_ms,
            "lease_expires_at_utc",
        )?,
        claim_token: claim_token_from_dsl(&dsl.claim_token)?,
        delivery_correlation_id: dsl.delivery_correlation_id.clone(),
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
            trigger_key: trigger_stable_key(trigger_snapshot)?,
            target_binding_key: target_stable_key(target_snapshot)?,
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
                target_binding_key: target_stable_key(target_snapshot)?,
                target_materialized_session_id: target_materialized_session_id(target_snapshot)
                    .map(|session_id| occ_dsl::SessionId(session_id.0.to_string())),
            }
        }
        OccurrenceLifecycleInput::RecordReceipt {
            receipt,
            runtime_outcome,
            ..
        } => occ_dsl::OccurrenceLifecycleInput::RecordReceipt {
            correlation_id: receipt.correlation_id.clone(),
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
        OccurrenceLifecycleInput::ClassifyDue { now_utc } => {
            occ_dsl::OccurrenceLifecycleInput::ClassifyDue {
                now_utc_ms: occurrence_datetime_to_millis(*now_utc, "now_utc")?,
            }
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
        } => occ_dsl::OccurrenceLifecycleInput::Claim {
            owner_id: owner_id.clone(),
            at_utc_ms: occurrence_datetime_to_millis(*at_utc, "at_utc")?,
            lease_expires_at_utc_ms: occurrence_datetime_to_millis(
                *lease_expires_at_utc,
                "lease_expires_at_utc",
            )?,
            claim_token: occ_dsl::ClaimToken(claim_token.to_string()),
        },
        OccurrenceLifecycleInput::DispatchStarted {
            correlation_id,
            at_utc,
        } => occ_dsl::OccurrenceLifecycleInput::DispatchStarted {
            correlation_id: correlation_id.clone(),
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
            detail,
            at_utc,
        } => occ_dsl::OccurrenceLifecycleInput::ResolveTargetProbe {
            outcome: to_dsl_occurrence_target_probe_outcome(*outcome),
            detail: detail.clone(),
            at_utc_ms: occurrence_datetime_to_millis(*at_utc, "at_utc")?,
        },
        OccurrenceLifecycleInput::ResolveDueMisfire { detail, at_utc } => {
            occ_dsl::OccurrenceLifecycleInput::ResolveDueMisfire {
                detail: detail.clone(),
                at_utc_ms: occurrence_datetime_to_millis(*at_utc, "at_utc")?,
            }
        }
        OccurrenceLifecycleInput::Supersede {
            superseded_by_revision,
            at_utc,
        } => occ_dsl::OccurrenceLifecycleInput::Supersede {
            superseded_by_revision: superseded_by_revision.0,
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
    })
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
    occ.claimed_by = dsl.claimed_by.clone();
    occ.delivery_correlation_id = dsl.delivery_correlation_id.clone();
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
            if dsl.target_binding_key != snapshot_key {
                return Err(OccurrenceLifecycleError::TargetBindingKeyMismatch {
                    machine_key: dsl.target_binding_key.clone(),
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
            if dsl.trigger_key != trigger_key {
                return Err(OccurrenceLifecycleError::TargetBindingKeyMismatch {
                    machine_key: dsl.trigger_key.clone(),
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
            if dsl.target_binding_key != snapshot_key {
                return Err(OccurrenceLifecycleError::TargetBindingKeyMismatch {
                    machine_key: dsl.target_binding_key.clone(),
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
        } => Err(error.clone()),
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
                    target_binding_key: target_binding_key.clone(),
                };
                let (transition, dsl_state) = run_schedule_dsl(&schedule, dsl_input)?;
                if dsl_state.target_binding_key != target_binding_key {
                    return Err(ScheduleLifecycleError::TargetBindingKeyMismatch {
                        machine_key: dsl_state.target_binding_key.clone(),
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
        trigger_key: trigger_stable_key(&trigger)?,
        target_binding_key: target_stable_key(&target)?,
        misfire_policy: to_dsl_misfire_policy(&misfire_policy),
        misfire_policy_key: misfire_policy_authority_key(&misfire_policy)?,
        overlap_policy: to_dsl_overlap_policy(&overlap_policy),
        overlap_policy_key: overlap_policy_authority_key(&overlap_policy)?,
        missing_target_policy: to_dsl_missing_target_policy(&missing_target_policy),
        missing_target_policy_key: missing_target_policy_authority_key(&missing_target_policy)?,
        planning_horizon_days: planning_horizon_days.map(u64::from),
        planning_horizon_occurrences: planning_horizon_occurrences.map(u64::from),
    };
    let mut dsl_auth = sched_dsl::ScheduleLifecycleMachineAuthority::new();
    let transition = sched_dsl::ScheduleLifecycleMachineMutator::apply(&mut dsl_auth, dsl_input)
        .map_err(|source| ScheduleLifecycleError::TransitionRejected { source })?;
    let dsl_state = dsl_auth.state().clone();
    verify_schedule_snapshot_keys(
        &dsl_state,
        &trigger,
        &target,
        &misfire_policy,
        &overlap_policy,
        &missing_target_policy,
    )?;

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
            trigger_key: trigger_stable_key(&next_trigger)?,
            target_binding_key: target_stable_key(&next_target)?,
            misfire_policy: to_dsl_misfire_policy(&next_misfire_policy),
            misfire_policy_key: misfire_policy_authority_key(&next_misfire_policy)?,
            overlap_policy: to_dsl_overlap_policy(&next_overlap_policy),
            overlap_policy_key: overlap_policy_authority_key(&next_overlap_policy)?,
            missing_target_policy: to_dsl_missing_target_policy(&next_missing_target_policy),
            missing_target_policy_key: missing_target_policy_authority_key(
                &next_missing_target_policy,
            )?,
            planning_horizon_days: u64::from(next_planning_horizon_days),
            planning_horizon_occurrences: u64::from(next_planning_horizon_occurrences),
            at_utc_ms: schedule_datetime_to_millis(at_utc, "at_utc")?,
        };
        let (transition, dsl_state) = run_schedule_dsl(schedule, dsl_input)?;
        verify_schedule_snapshot_keys(
            &dsl_state,
            &next_trigger,
            &next_target,
            &next_misfire_policy,
            &next_overlap_policy,
            &next_missing_target_policy,
        )?;
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
    misfire_policy: &crate::types::MisfirePolicy,
    overlap_policy: &crate::types::OverlapPolicy,
    missing_target_policy: &crate::types::MissingTargetPolicy,
) -> Result<(), ScheduleLifecycleError> {
    let trigger_key = trigger_stable_key(trigger)?;
    if dsl.trigger_key != trigger_key {
        return Err(ScheduleLifecycleError::TargetBindingKeyMismatch {
            machine_key: dsl.trigger_key.clone(),
            snapshot_key: trigger_key,
        });
    }
    let target_key = target_stable_key(target)?;
    if dsl.target_binding_key != target_key {
        return Err(ScheduleLifecycleError::TargetBindingKeyMismatch {
            machine_key: dsl.target_binding_key.clone(),
            snapshot_key: target_key,
        });
    }
    let misfire_key = misfire_policy_authority_key(misfire_policy)?;
    if dsl.misfire_policy_key != misfire_key {
        return Err(ScheduleLifecycleError::TargetBindingKeyMismatch {
            machine_key: dsl.misfire_policy_key.clone(),
            snapshot_key: misfire_key,
        });
    }
    let overlap_key = overlap_policy_authority_key(overlap_policy)?;
    if dsl.overlap_policy_key != overlap_key {
        return Err(ScheduleLifecycleError::TargetBindingKeyMismatch {
            machine_key: dsl.overlap_policy_key.clone(),
            snapshot_key: overlap_key,
        });
    }
    let missing_target_key = missing_target_policy_authority_key(missing_target_policy)?;
    if dsl.missing_target_policy_key != missing_target_key {
        return Err(ScheduleLifecycleError::TargetBindingKeyMismatch {
            machine_key: dsl.missing_target_policy_key.clone(),
            snapshot_key: missing_target_key,
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
        dsl.delivery_correlation_id.as_deref(),
        detail.as_deref(),
        failure_class,
        runtime_outcome_key.as_deref(),
        materialized_session_id.as_ref(),
    )
    .map_err(|reason| OccurrenceLifecycleError::ProjectionMismatch { reason })?;
    Ok(DeliveryReceipt {
        receipt_id,
        occurrence_id,
        attempt,
        stage,
        recorded_at_utc,
        correlation_id: dsl.delivery_correlation_id.clone(),
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
            machine_key: dsl.runtime_outcome_key.clone(),
            snapshot_key: runtime_outcome_key,
        });
    }
    let receipt_id = delivery_receipt_id_from_authority(
        &occurrence_id,
        attempt,
        stage,
        recorded_at_utc_ms,
        dsl.last_receipt_correlation_id.as_deref(),
        detail.as_deref(),
        failure_class,
        dsl.runtime_outcome_key.as_deref(),
        materialized_session_id.as_ref(),
    )
    .map_err(|reason| OccurrenceLifecycleError::ProjectionMismatch { reason })?;
    Ok(DeliveryReceipt {
        receipt_id,
        occurrence_id,
        attempt,
        stage,
        recorded_at_utc,
        correlation_id: dsl.last_receipt_correlation_id.clone(),
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
) -> Result<String, SemanticKeySerializationError> {
    semantic_json_key("runtime_outcome", "runtime_outcome", outcome)
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
                .classify_due_action(now)
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
                .classify_due_action(now)
                .expect("future classification should pass"),
            None
        );
        assert!(matches!(
            future.apply(OccurrenceLifecycleInput::Claim {
                owner_id: "owner".into(),
                at_utc: now,
                lease_expires_at_utc: now + Duration::seconds(30),
                claim_token: Uuid::now_v7(),
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
                .classify_due_action(now)
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
}
