//! Domain-facing lifecycle inputs, effects, errors, and mutators for schedules
//! and occurrences. The `apply()` methods live on `Schedule` / `Occurrence` in
//! `types.rs` and delegate to the DSL kernels in `machines::*`. This module
//! houses the pure data types consumed by callers and by the `ScheduleStore`
//! trait wire contract.

use crate::machines::occurrence_lifecycle as occ_dsl;
use crate::machines::schedule_lifecycle as sched_dsl;
use crate::types::{
    CreateScheduleRequest, DeliveryReceipt, Occurrence, OccurrenceFailureClass, OccurrenceOrdinal,
    OccurrencePhase, RuntimeDeliveryOutcome, Schedule, ScheduleId, SchedulePhase, ScheduleRevision,
    TargetBinding, TriggerSpec, UpdateScheduleRequest, default_planning_horizon_days,
    default_planning_horizon_occurrences,
};
use chrono::{DateTime, Utc};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Schedule lifecycle — public types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum ScheduleLifecycleInput {
    Create(CreateScheduleRequest),
    Update(UpdateScheduleRequest),
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
        occurrence_id: crate::types::OccurrenceId,
        superseding_revision: ScheduleRevision,
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
    },
    PlanningWindowRecorded {
        planning_cursor_utc: DateTime<Utc>,
        next_occurrence_ordinal: OccurrenceOrdinal,
    },
}

#[derive(Debug, Clone)]
pub struct ScheduleLifecycleMutator {
    pub schedule: Schedule,
    pub effects: Vec<ScheduleLifecycleEffect>,
    pub revision_bumped: bool,
}

impl ScheduleLifecycleMutator {
    pub fn into_schedule(self) -> Schedule {
        self.schedule
    }
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
        receipt: DeliveryReceipt,
        at_utc: DateTime<Utc>,
    },
    Skip {
        detail: Option<String>,
        failure_class: Option<OccurrenceFailureClass>,
        at_utc: DateTime<Utc>,
    },
    Misfire {
        detail: Option<String>,
        failure_class: Option<OccurrenceFailureClass>,
        at_utc: DateTime<Utc>,
    },
    Supersede {
        superseded_by_revision: ScheduleRevision,
        at_utc: DateTime<Utc>,
    },
    DeliveryFailed {
        receipt: Option<DeliveryReceipt>,
        failure_class: OccurrenceFailureClass,
        detail: Option<String>,
        at_utc: DateTime<Utc>,
    },
    LeaseExpired {
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
    DeliveryFailed,
    LeaseExpired,
}

#[derive(Debug, Clone)]
pub struct OccurrenceLifecycleMutator {
    pub occurrence: Occurrence,
    pub effects: Vec<OccurrenceLifecycleEffect>,
}

impl OccurrenceLifecycleMutator {
    pub fn into_occurrence(self) -> Occurrence {
        self.occurrence
    }
}

#[derive(Debug, thiserror::Error)]
pub enum OccurrenceLifecycleError {
    #[error("occurrence is already terminal")]
    AlreadyTerminal,
    #[error("occurrence must be pending before it can be claimed")]
    NotPendingForClaim,
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
    #[error("OccurrenceLifecycleMachine emitted invalid occurrence id `{id}`: {source}")]
    InvalidOccurrenceId { id: String, source: uuid::Error },
    #[error("OccurrenceLifecycleMachine emitted invalid schedule id `{id}`: {source}")]
    InvalidScheduleId { id: String, source: uuid::Error },
    #[error("OccurrenceLifecycleMachine emitted invalid due timestamp millis `{millis}`")]
    InvalidDueAtUtcMillis { millis: u64 },
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
        let dsl_input = convert_occurrence_input(&input);
        let mut dsl_auth = occ_dsl::OccurrenceLifecycleMachineAuthority::new();
        occ_dsl::OccurrenceLifecycleMachineMutator::apply(&mut dsl_auth, dsl_input)
            .map_err(|e| map_occurrence_error(e, &input))?;
        occurrence_from_planned_state(dsl_auth.state(), schedule, target_snapshot, Utc::now())
    }

    /// Apply a lifecycle input to this occurrence, projecting through the DSL
    /// kernel and writing the resulting machine-owned state back into `self`.
    ///
    /// Non-machine fields (trigger_snapshot, target_snapshot, policies,
    /// created_at_utc) are preserved verbatim; the DSL is sole authority over
    /// phase, claim fields, timestamps, attempt_count, and failure data.
    pub fn apply(
        mut self,
        input: OccurrenceLifecycleInput,
    ) -> Result<OccurrenceLifecycleMutator, OccurrenceLifecycleError> {
        // 1. Project domain → DSL state
        let dsl_state = project_occurrence(&self);

        // 2. Convert domain input → DSL input
        let dsl_input = convert_occurrence_input(&input);

        // 3. Run DSL dispatch
        let mut dsl_auth =
            occ_dsl::OccurrenceLifecycleMachineAuthority::recover_from_state(dsl_state)
                .map_err(|e| map_occurrence_error(e, &input))?;
        let transition =
            occ_dsl::OccurrenceLifecycleMachineMutator::apply(&mut dsl_auth, dsl_input)
                .map_err(|e| map_occurrence_error(e, &input))?;

        // 4. Write DSL state → occurrence
        write_back_occurrence(dsl_auth.state(), &mut self, &input)?;

        // 5. Map effects
        let effects = transition
            .effects
            .iter()
            .map(map_occurrence_effect)
            .collect::<Result<Vec<_>, _>>()?;

        Ok(OccurrenceLifecycleMutator {
            occurrence: self,
            effects,
        })
    }
}

/// Project a domain `Occurrence` into the DSL flat state struct.
fn project_occurrence(occ: &Occurrence) -> occ_dsl::OccurrenceLifecycleMachineState {
    occ_dsl::OccurrenceLifecycleMachineState {
        lifecycle_phase: match occ.phase {
            OccurrencePhase::Pending => occ_dsl::OccurrenceLifecycleState::Pending,
            OccurrencePhase::Claimed => occ_dsl::OccurrenceLifecycleState::Claimed,
            OccurrencePhase::Dispatching => occ_dsl::OccurrenceLifecycleState::Dispatching,
            OccurrencePhase::AwaitingCompletion => {
                occ_dsl::OccurrenceLifecycleState::AwaitingCompletion
            }
            OccurrencePhase::Completed => occ_dsl::OccurrenceLifecycleState::Completed,
            OccurrencePhase::Skipped => occ_dsl::OccurrenceLifecycleState::Skipped,
            OccurrencePhase::Misfired => occ_dsl::OccurrenceLifecycleState::Misfired,
            OccurrencePhase::Superseded => occ_dsl::OccurrenceLifecycleState::Superseded,
            OccurrencePhase::DeliveryFailed => occ_dsl::OccurrenceLifecycleState::DeliveryFailed,
        },
        occurrence_id: occ_dsl::OccurrenceId(occ.occurrence_id.0.to_string()),
        schedule_id: occ_dsl::ScheduleId(occ.schedule_id.0.to_string()),
        schedule_revision: occ.schedule_revision.0,
        occurrence_ordinal: occ.occurrence_ordinal.0,
        trigger_key: trigger_stable_key(&occ.trigger_snapshot),
        target_binding_key: occ.target_snapshot.stable_key(),
        misfire_policy: to_occ_dsl_misfire_policy(&occ.misfire_policy),
        misfire_policy_key: misfire_policy_authority_key(&occ.misfire_policy),
        overlap_policy: to_occ_dsl_overlap_policy(&occ.overlap_policy),
        overlap_policy_key: overlap_policy_authority_key(&occ.overlap_policy),
        missing_target_policy: to_occ_dsl_missing_target_policy(&occ.missing_target_policy),
        missing_target_policy_key: missing_target_policy_authority_key(&occ.missing_target_policy),
        due_at_utc_ms: datetime_to_millis(occ.due_at_utc),
        claimed_by: occ.claimed_by.clone(),
        lease_expires_at_utc_ms: occ.lease_expires_at_utc.map(datetime_to_millis),
        claimed_at_utc_ms: occ.claimed_at_utc.map(datetime_to_millis),
        claim_token: occ.claim_token.map(|u| occ_dsl::ClaimToken(u.to_string())),
        delivery_correlation_id: occ.delivery_correlation_id.clone(),
        last_receipt: occ
            .last_receipt
            .as_ref()
            .map(|r| occ_dsl::DeliveryReceipt(serde_json::to_string(r).unwrap_or_default())),
        runtime_outcome_key: occ
            .runtime_outcome
            .as_ref()
            .map(runtime_outcome_authority_key),
        failure_class: occ.failure_class.map(to_dsl_failure_class),
        failure_detail: occ.failure_detail.clone(),
        dispatched_at_utc_ms: occ.dispatched_at_utc.map(datetime_to_millis),
        completed_at_utc_ms: occ.completed_at_utc.map(datetime_to_millis),
        attempt_count: u64::from(occ.attempt_count),
        superseded_by_revision: occ.superseded_by_revision.map(|r| r.0),
    }
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
    let snapshot_key = target_snapshot.stable_key();
    if dsl.target_binding_key != snapshot_key {
        return Err(OccurrenceLifecycleError::TargetBindingKeyMismatch {
            machine_key: dsl.target_binding_key.clone(),
            snapshot_key,
        });
    }
    let trigger_key = trigger_stable_key(&schedule.trigger);
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
        lease_expires_at_utc: dsl.lease_expires_at_utc_ms.and_then(millis_to_datetime),
        claim_token: dsl
            .claim_token
            .as_ref()
            .and_then(|t| Uuid::parse_str(&t.0).ok()),
        delivery_correlation_id: dsl.delivery_correlation_id.clone(),
        last_receipt: None,
        failure_class: dsl.failure_class.map(from_dsl_failure_class),
        runtime_outcome: None,
        failure_detail: dsl.failure_detail.clone(),
        attempt_count: u32::try_from(dsl.attempt_count).unwrap_or(u32::MAX),
        created_at_utc,
        claimed_at_utc: dsl.claimed_at_utc_ms.and_then(millis_to_datetime),
        dispatched_at_utc: dsl.dispatched_at_utc_ms.and_then(millis_to_datetime),
        completed_at_utc: dsl.completed_at_utc_ms.and_then(millis_to_datetime),
        superseded_by_revision: dsl.superseded_by_revision.map(ScheduleRevision),
    })
}

/// Convert domain input to DSL input.
fn convert_occurrence_input(input: &OccurrenceLifecycleInput) -> occ_dsl::OccurrenceLifecycleInput {
    match input {
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
            trigger_key: trigger_stable_key(trigger_snapshot),
            target_binding_key: target_snapshot.stable_key(),
            misfire_policy: to_occ_dsl_misfire_policy(misfire_policy),
            misfire_policy_key: misfire_policy_authority_key(misfire_policy),
            overlap_policy: to_occ_dsl_overlap_policy(overlap_policy),
            overlap_policy_key: overlap_policy_authority_key(overlap_policy),
            missing_target_policy: to_occ_dsl_missing_target_policy(missing_target_policy),
            missing_target_policy_key: missing_target_policy_authority_key(missing_target_policy),
            due_at_utc_ms: datetime_to_millis(*due_at_utc),
        },
        OccurrenceLifecycleInput::SyncTargetSnapshot { target_snapshot } => {
            occ_dsl::OccurrenceLifecycleInput::SyncTargetSnapshot {
                target_binding_key: target_snapshot.stable_key(),
            }
        }
        OccurrenceLifecycleInput::RecordReceipt {
            receipt,
            runtime_outcome,
        } => occ_dsl::OccurrenceLifecycleInput::RecordReceipt {
            receipt: receipt_to_dsl(receipt),
            runtime_outcome_key: runtime_outcome.as_ref().map(runtime_outcome_authority_key),
        },
        OccurrenceLifecycleInput::Claim {
            owner_id,
            at_utc,
            lease_expires_at_utc,
            claim_token,
        } => occ_dsl::OccurrenceLifecycleInput::Claim {
            owner_id: owner_id.clone(),
            at_utc_ms: datetime_to_millis(*at_utc),
            lease_expires_at_utc_ms: datetime_to_millis(*lease_expires_at_utc),
            claim_token: occ_dsl::ClaimToken(claim_token.to_string()),
        },
        OccurrenceLifecycleInput::DispatchStarted {
            correlation_id,
            at_utc,
        } => occ_dsl::OccurrenceLifecycleInput::DispatchStarted {
            correlation_id: correlation_id.clone(),
            at_utc_ms: datetime_to_millis(*at_utc),
        },
        OccurrenceLifecycleInput::AwaitCompletion { at_utc } => {
            occ_dsl::OccurrenceLifecycleInput::AwaitCompletion {
                at_utc_ms: datetime_to_millis(*at_utc),
            }
        }
        OccurrenceLifecycleInput::Complete { receipt, at_utc } => {
            occ_dsl::OccurrenceLifecycleInput::Complete {
                receipt: occ_dsl::DeliveryReceipt(
                    serde_json::to_string(receipt).unwrap_or_default(),
                ),
                at_utc_ms: datetime_to_millis(*at_utc),
            }
        }
        OccurrenceLifecycleInput::Skip {
            detail,
            failure_class,
            at_utc,
        } => occ_dsl::OccurrenceLifecycleInput::Skip {
            detail: detail.clone(),
            failure_class: failure_class.map(to_dsl_failure_class),
            at_utc_ms: datetime_to_millis(*at_utc),
        },
        OccurrenceLifecycleInput::Misfire {
            detail,
            failure_class,
            at_utc,
        } => occ_dsl::OccurrenceLifecycleInput::Misfire {
            detail: detail.clone(),
            failure_class: failure_class.map(to_dsl_failure_class),
            at_utc_ms: datetime_to_millis(*at_utc),
        },
        OccurrenceLifecycleInput::Supersede {
            superseded_by_revision,
            at_utc,
        } => occ_dsl::OccurrenceLifecycleInput::Supersede {
            superseded_by_revision: superseded_by_revision.0,
            at_utc_ms: datetime_to_millis(*at_utc),
        },
        OccurrenceLifecycleInput::DeliveryFailed {
            receipt,
            failure_class,
            detail,
            at_utc,
        } => occ_dsl::OccurrenceLifecycleInput::DeliveryFailed {
            receipt: receipt
                .as_ref()
                .map(|r| occ_dsl::DeliveryReceipt(serde_json::to_string(r).unwrap_or_default())),
            failure_class: to_dsl_failure_class(*failure_class),
            detail: detail.clone(),
            at_utc_ms: datetime_to_millis(*at_utc),
        },
        OccurrenceLifecycleInput::LeaseExpired { at_utc } => {
            occ_dsl::OccurrenceLifecycleInput::LeaseExpired {
                at_utc_ms: datetime_to_millis(*at_utc),
            }
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
    occ.claimed_by = dsl.claimed_by.clone();
    occ.delivery_correlation_id = dsl.delivery_correlation_id.clone();
    occ.failure_detail = dsl.failure_detail.clone();
    occ.attempt_count = u32::try_from(dsl.attempt_count).unwrap_or(u32::MAX);

    // Timestamps (u64 millis → DateTime<Utc>)
    occ.lease_expires_at_utc = dsl.lease_expires_at_utc_ms.and_then(millis_to_datetime);
    occ.claimed_at_utc = dsl.claimed_at_utc_ms.and_then(millis_to_datetime);
    occ.dispatched_at_utc = dsl.dispatched_at_utc_ms.and_then(millis_to_datetime);
    occ.completed_at_utc = dsl.completed_at_utc_ms.and_then(millis_to_datetime);

    // Claim token (DSL String → Uuid)
    occ.claim_token = dsl
        .claim_token
        .as_ref()
        .and_then(|t| Uuid::parse_str(&t.0).ok());

    // Failure class (DSL enum → real enum)
    occ.failure_class = dsl.failure_class.map(from_dsl_failure_class);

    // Superseded revision (DSL u64 → ScheduleRevision)
    occ.superseded_by_revision = dsl.superseded_by_revision.map(ScheduleRevision);

    match input {
        OccurrenceLifecycleInput::PlanOccurrence {
            trigger_snapshot,
            target_snapshot,
            misfire_policy,
            overlap_policy,
            missing_target_policy,
            ..
        } => {
            let snapshot_key = target_snapshot.stable_key();
            if dsl.target_binding_key != snapshot_key {
                return Err(OccurrenceLifecycleError::TargetBindingKeyMismatch {
                    machine_key: dsl.target_binding_key.clone(),
                    snapshot_key,
                });
            }
            occ.target_snapshot = target_snapshot.clone();
            let trigger_key = trigger_stable_key(trigger_snapshot);
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
            let snapshot_key = target_snapshot.stable_key();
            if dsl.target_binding_key != snapshot_key {
                return Err(OccurrenceLifecycleError::TargetBindingKeyMismatch {
                    machine_key: dsl.target_binding_key.clone(),
                    snapshot_key,
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
        let snapshot_key = runtime_outcome.as_ref().map(runtime_outcome_authority_key);
        if dsl.runtime_outcome_key != snapshot_key {
            return Err(OccurrenceLifecycleError::RuntimeOutcomeKeyMismatch {
                machine_key: dsl.runtime_outcome_key.clone(),
                snapshot_key,
            });
        }
        occ.last_receipt = Some(receipt.clone());
        occ.runtime_outcome = runtime_outcome.clone();
        return Ok(());
    }

    // DeliveryReceipt: the DSL stores it as serialized JSON. We recover the
    // real receipt from the original input when the DSL assigned it (Complete,
    // DeliveryFailed). For other transitions the DSL either kept None or the
    // existing value — we round-trip through serde only when the DSL has a value
    // we didn't supply.
    occ.last_receipt = match input {
        OccurrenceLifecycleInput::Complete { receipt, .. } => {
            // DSL sets last_receipt = Some(receipt), recover from input
            if dsl.last_receipt.is_some() {
                Some(receipt.clone())
            } else {
                None
            }
        }
        OccurrenceLifecycleInput::DeliveryFailed { receipt, .. } => {
            // DSL sets last_receipt = receipt (Option), recover from input
            if dsl.last_receipt.is_some() {
                receipt.clone()
            } else {
                None
            }
        }
        OccurrenceLifecycleInput::Claim { .. } => {
            // DSL clears last_receipt to None
            occ.runtime_outcome = None;
            None
        }
        OccurrenceLifecycleInput::PlanOccurrence { .. } => {
            occ.runtime_outcome = None;
            None
        }
        _ => {
            // Other transitions don't touch last_receipt in the DSL,
            // so preserve the original domain value.
            occ.last_receipt.take()
        }
    };
    Ok(())
}

/// Map DSL error to the appropriate domain error variant.
fn map_occurrence_error(
    _error: occ_dsl::OccurrenceLifecycleMachineTransitionError,
    input: &OccurrenceLifecycleInput,
) -> OccurrenceLifecycleError {
    // The DSL returns NoMatchingTransition { phase, trigger }. We reconstruct
    // the specific error variant based on which input was attempted.
    match input {
        OccurrenceLifecycleInput::PlanOccurrence { .. } => OccurrenceLifecycleError::PlanRejected,
        OccurrenceLifecycleInput::SyncTargetSnapshot { .. } => {
            OccurrenceLifecycleError::TargetSyncRejected
        }
        OccurrenceLifecycleInput::RecordReceipt { .. } => {
            OccurrenceLifecycleError::ReceiptRecordRejected
        }
        OccurrenceLifecycleInput::Claim { .. } => OccurrenceLifecycleError::NotPendingForClaim,
        OccurrenceLifecycleInput::DispatchStarted { .. } => OccurrenceLifecycleError::NotClaimed,
        OccurrenceLifecycleInput::AwaitCompletion { .. } => {
            OccurrenceLifecycleError::NotDispatching
        }
        OccurrenceLifecycleInput::LeaseExpired { .. } => OccurrenceLifecycleError::NotLeaseHolding,
        OccurrenceLifecycleInput::Complete { .. }
        | OccurrenceLifecycleInput::Skip { .. }
        | OccurrenceLifecycleInput::Misfire { .. }
        | OccurrenceLifecycleInput::Supersede { .. }
        | OccurrenceLifecycleInput::DeliveryFailed { .. } => {
            OccurrenceLifecycleError::NotLiveForTerminal
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
        occ_dsl::OccurrenceLifecycleEffect::DeliveryFailed => {
            OccurrenceLifecycleEffect::DeliveryFailed
        }
        occ_dsl::OccurrenceLifecycleEffect::LeaseExpired => OccurrenceLifecycleEffect::LeaseExpired,
    })
}

// ===========================================================================
// Schedule::apply — DSL-backed lifecycle transition on the domain type
// ===========================================================================

impl Schedule {
    /// Apply a lifecycle input. `Create` constructs a fresh schedule (self is
    /// ignored); all other inputs require an existing schedule. The DSL is sole
    /// authority over `phase`, `revision`, `planning_cursor_utc`, and
    /// `next_occurrence_ordinal`; `config.updated_at_utc` / `config.deleted_at_utc`
    /// are touched here since the DSL does not track them.
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
            ScheduleLifecycleInput::Update(request) => {
                let mut schedule = schedule.ok_or(ScheduleLifecycleError::MissingSchedule)?;
                let (revision_bumped, effects) = apply_update(&mut schedule, request)?;
                Ok(ScheduleLifecycleMutator {
                    schedule,
                    effects,
                    revision_bumped,
                })
            }

            // Remaining inputs go through the DSL
            ScheduleLifecycleInput::RecordPlanningWindow {
                planning_cursor_utc,
                next_occurrence_ordinal,
            } => {
                let mut schedule = schedule.ok_or(ScheduleLifecycleError::MissingSchedule)?;
                let dsl_input = sched_dsl::ScheduleLifecycleInput::RecordPlanningWindow {
                    planning_cursor_utc_ms: datetime_to_millis(planning_cursor_utc),
                    next_occurrence_ordinal: next_occurrence_ordinal.0,
                };
                let (transition, dsl_state) = run_schedule_dsl(&schedule, dsl_input)?;
                write_back_schedule(&dsl_state, &mut schedule)?;
                let effects = map_schedule_effects(
                    &transition,
                    Some((planning_cursor_utc, next_occurrence_ordinal)),
                );
                Ok(ScheduleLifecycleMutator {
                    schedule,
                    effects,
                    revision_bumped: false,
                })
            }

            ScheduleLifecycleInput::SyncTargetSnapshot { target } => {
                let mut schedule = schedule.ok_or(ScheduleLifecycleError::MissingSchedule)?;
                let target_binding_key = target.stable_key();
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
                let effects = map_schedule_effects(&transition, None);
                Ok(ScheduleLifecycleMutator {
                    schedule,
                    effects,
                    revision_bumped: false,
                })
            }

            ScheduleLifecycleInput::Pause { at_utc } => {
                let mut schedule = schedule.ok_or(ScheduleLifecycleError::MissingSchedule)?;
                let dsl_input = sched_dsl::ScheduleLifecycleInput::Pause {
                    at_utc_ms: datetime_to_millis(at_utc),
                };
                let (transition, dsl_state) = run_schedule_dsl(&schedule, dsl_input)?;
                write_back_schedule(&dsl_state, &mut schedule)?;
                schedule.config.updated_at_utc = at_utc;
                let effects = map_schedule_effects(&transition, None);
                Ok(ScheduleLifecycleMutator {
                    schedule,
                    effects,
                    revision_bumped: false,
                })
            }

            ScheduleLifecycleInput::Resume { at_utc } => {
                let mut schedule = schedule.ok_or(ScheduleLifecycleError::MissingSchedule)?;
                let dsl_input = sched_dsl::ScheduleLifecycleInput::Resume {
                    at_utc_ms: datetime_to_millis(at_utc),
                };
                let (transition, dsl_state) = run_schedule_dsl(&schedule, dsl_input)?;
                write_back_schedule(&dsl_state, &mut schedule)?;
                schedule.config.updated_at_utc = at_utc;
                let effects = map_schedule_effects(&transition, None);
                Ok(ScheduleLifecycleMutator {
                    schedule,
                    effects,
                    revision_bumped: false,
                })
            }

            ScheduleLifecycleInput::Delete { at_utc } => {
                let mut schedule = schedule.ok_or(ScheduleLifecycleError::MissingSchedule)?;
                let dsl_input = sched_dsl::ScheduleLifecycleInput::Delete {
                    at_utc_ms: datetime_to_millis(at_utc),
                };
                let old_revision = schedule.revision;
                let (transition, dsl_state) = run_schedule_dsl(&schedule, dsl_input)?;
                write_back_schedule(&dsl_state, &mut schedule)?;
                schedule.config.deleted_at_utc = Some(at_utc);
                schedule.config.updated_at_utc = at_utc;
                let revision_bumped = schedule.revision != old_revision;
                let effects = map_schedule_effects(&transition, None);
                Ok(ScheduleLifecycleMutator {
                    schedule,
                    effects,
                    revision_bumped,
                })
            }

            ScheduleLifecycleInput::ConfirmOccurrencesSuperseded {
                occurrence_id,
                superseding_revision,
            } => {
                let mut schedule = schedule.ok_or(ScheduleLifecycleError::MissingSchedule)?;
                let dsl_input = sched_dsl::ScheduleLifecycleInput::ConfirmOccurrencesSuperseded {
                    occurrence_id: sched_dsl::OccurrenceId(occurrence_id.0.to_string()),
                    superseding_revision: superseding_revision.0,
                };
                let (transition, dsl_state) = run_schedule_dsl(&schedule, dsl_input)?;
                write_back_schedule(&dsl_state, &mut schedule)?;
                let effects = map_schedule_effects(&transition, None);
                Ok(ScheduleLifecycleMutator {
                    schedule,
                    effects,
                    revision_bumped: false,
                })
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
    let planning_horizon_days = request
        .planning_horizon_days
        .unwrap_or_else(default_planning_horizon_days);
    let planning_horizon_occurrences = request
        .planning_horizon_occurrences
        .unwrap_or_else(default_planning_horizon_occurrences);

    let dsl_input = sched_dsl::ScheduleLifecycleInput::Create {
        schedule_id: sched_dsl::ScheduleId(schedule_id.0.to_string()),
        trigger_key: trigger_stable_key(&trigger),
        target_binding_key: target.stable_key(),
        misfire_policy: to_dsl_misfire_policy(&misfire_policy),
        misfire_policy_key: misfire_policy_authority_key(&misfire_policy),
        overlap_policy: to_dsl_overlap_policy(&overlap_policy),
        overlap_policy_key: overlap_policy_authority_key(&overlap_policy),
        missing_target_policy: to_dsl_missing_target_policy(&missing_target_policy),
        missing_target_policy_key: missing_target_policy_authority_key(&missing_target_policy),
        planning_horizon_days: u64::from(planning_horizon_days),
        planning_horizon_occurrences: u64::from(planning_horizon_occurrences),
    };
    let mut dsl_auth = sched_dsl::ScheduleLifecycleMachineAuthority::new();
    let transition = sched_dsl::ScheduleLifecycleMachineMutator::apply(&mut dsl_auth, dsl_input)
        .map_err(|_| ScheduleLifecycleError::Deleted)?;
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
        trigger,
        target,
        misfire_policy,
        overlap_policy,
        missing_target_policy,
        next_occurrence_ordinal: OccurrenceOrdinal(dsl_state.next_occurrence_ordinal),
        planning_cursor_utc: dsl_state
            .planning_cursor_utc_ms
            .and_then(millis_to_datetime),
        superseded_ack_ids: dsl_state
            .superseded_ack_ids
            .iter()
            .filter_map(|id| crate::types::OccurrenceId::parse(&id.0).ok())
            .collect(),
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
    let effects = map_schedule_effects(&transition, None);
    Ok(ScheduleLifecycleMutator {
        schedule,
        effects,
        revision_bumped: false,
    })
}

/// Apply config-level update changes. If any revision-affecting field changes,
/// delegate the revision bump to the DSL via a Revise input.
fn apply_update(
    schedule: &mut Schedule,
    request: UpdateScheduleRequest,
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
            trigger_key: trigger_stable_key(&next_trigger),
            target_binding_key: next_target.stable_key(),
            misfire_policy: to_dsl_misfire_policy(&next_misfire_policy),
            misfire_policy_key: misfire_policy_authority_key(&next_misfire_policy),
            overlap_policy: to_dsl_overlap_policy(&next_overlap_policy),
            overlap_policy_key: overlap_policy_authority_key(&next_overlap_policy),
            missing_target_policy: to_dsl_missing_target_policy(&next_missing_target_policy),
            missing_target_policy_key: missing_target_policy_authority_key(
                &next_missing_target_policy,
            ),
            planning_horizon_days: u64::from(next_planning_horizon_days),
            planning_horizon_occurrences: u64::from(next_planning_horizon_occurrences),
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
        return Ok((true, map_schedule_effects(&transition, None)));
    }

    let effects = if planning_config_changed {
        let dsl_input = sched_dsl::ScheduleLifecycleInput::UpdatePlanningConfig {
            planning_horizon_days: u64::from(next_planning_horizon_days),
            planning_horizon_occurrences: u64::from(next_planning_horizon_occurrences),
        };
        let (transition, dsl_state) = run_schedule_dsl(schedule, dsl_input)?;
        write_back_schedule(&dsl_state, schedule)?;
        map_schedule_effects(&transition, None)
    } else {
        vec![ScheduleLifecycleEffect::EmitScheduleNotice {
            new_state: schedule.phase,
            revision: schedule.revision,
        }]
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
    let trigger_key = trigger_stable_key(trigger);
    if dsl.trigger_key != trigger_key {
        return Err(ScheduleLifecycleError::TargetBindingKeyMismatch {
            machine_key: dsl.trigger_key.clone(),
            snapshot_key: trigger_key,
        });
    }
    let target_key = target.stable_key();
    if dsl.target_binding_key != target_key {
        return Err(ScheduleLifecycleError::TargetBindingKeyMismatch {
            machine_key: dsl.target_binding_key.clone(),
            snapshot_key: target_key,
        });
    }
    let misfire_key = misfire_policy_authority_key(misfire_policy);
    if dsl.misfire_policy_key != misfire_key {
        return Err(ScheduleLifecycleError::TargetBindingKeyMismatch {
            machine_key: dsl.misfire_policy_key.clone(),
            snapshot_key: misfire_key,
        });
    }
    let overlap_key = overlap_policy_authority_key(overlap_policy);
    if dsl.overlap_policy_key != overlap_key {
        return Err(ScheduleLifecycleError::TargetBindingKeyMismatch {
            machine_key: dsl.overlap_policy_key.clone(),
            snapshot_key: overlap_key,
        });
    }
    let missing_target_key = missing_target_policy_authority_key(missing_target_policy);
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
    let misfire_key = misfire_policy_authority_key(misfire_policy);
    if dsl.misfire_policy_key != misfire_key {
        return Err(OccurrenceLifecycleError::TargetBindingKeyMismatch {
            machine_key: dsl.misfire_policy_key.clone(),
            snapshot_key: misfire_key,
        });
    }
    let overlap_key = overlap_policy_authority_key(overlap_policy);
    if dsl.overlap_policy_key != overlap_key {
        return Err(OccurrenceLifecycleError::TargetBindingKeyMismatch {
            machine_key: dsl.overlap_policy_key.clone(),
            snapshot_key: overlap_key,
        });
    }
    let missing_target_key = missing_target_policy_authority_key(missing_target_policy);
    if dsl.missing_target_policy_key != missing_target_key {
        return Err(OccurrenceLifecycleError::TargetBindingKeyMismatch {
            machine_key: dsl.missing_target_policy_key.clone(),
            snapshot_key: missing_target_key,
        });
    }
    Ok(())
}

/// Project schedule → DSL state, apply DSL input, return transition + new state.
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
    let dsl_state = project_schedule(schedule);
    let mut dsl_auth = sched_dsl::ScheduleLifecycleMachineAuthority::recover_from_state(dsl_state)
        .map_err(|_| ScheduleLifecycleError::Deleted)?;
    let transition = sched_dsl::ScheduleLifecycleMachineMutator::apply(&mut dsl_auth, dsl_input)
        .map_err(|_| ScheduleLifecycleError::Deleted)?;
    Ok((transition, dsl_auth.state().clone()))
}

/// Project a domain `Schedule` into the DSL flat state struct.
fn project_schedule(sched: &Schedule) -> sched_dsl::ScheduleLifecycleMachineState {
    sched_dsl::ScheduleLifecycleMachineState {
        schedule_id: sched_dsl::ScheduleId(sched.schedule_id.0.to_string()),
        lifecycle_phase: match sched.phase {
            SchedulePhase::Active => sched_dsl::ScheduleLifecycleState::Active,
            SchedulePhase::Paused => sched_dsl::ScheduleLifecycleState::Paused,
            SchedulePhase::Deleted => sched_dsl::ScheduleLifecycleState::Deleted,
        },
        revision: sched.revision.0,
        trigger_key: trigger_stable_key(&sched.trigger),
        target_binding_key: sched.target.stable_key(),
        misfire_policy: to_dsl_misfire_policy(&sched.misfire_policy),
        misfire_policy_key: misfire_policy_authority_key(&sched.misfire_policy),
        overlap_policy: to_dsl_overlap_policy(&sched.overlap_policy),
        overlap_policy_key: overlap_policy_authority_key(&sched.overlap_policy),
        missing_target_policy: to_dsl_missing_target_policy(&sched.missing_target_policy),
        missing_target_policy_key: missing_target_policy_authority_key(
            &sched.missing_target_policy,
        ),
        planning_horizon_days: u64::from(sched.config.planning_horizon_days),
        planning_horizon_occurrences: u64::from(sched.config.planning_horizon_occurrences),
        planning_cursor_utc_ms: sched.planning_cursor_utc.map(datetime_to_millis),
        next_occurrence_ordinal: sched.next_occurrence_ordinal.0,
        superseded_ack_ids: sched
            .superseded_ack_ids
            .iter()
            .map(|id| sched_dsl::OccurrenceId(id.0.to_string()))
            .collect(),
    }
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
    sched.planning_cursor_utc = dsl.planning_cursor_utc_ms.and_then(millis_to_datetime);
    sched.next_occurrence_ordinal = OccurrenceOrdinal(dsl.next_occurrence_ordinal);
    sched.superseded_ack_ids = dsl
        .superseded_ack_ids
        .iter()
        .filter_map(|id| crate::types::OccurrenceId::parse(&id.0).ok())
        .collect();
    Ok(())
}

/// Map DSL schedule effect → domain effect.
///
/// `planning_cursor_utc` and `next_occurrence_ordinal` are passed in from the
/// original input since the DSL stores millis, and the PlanningWindowRecorded
/// effect carries the original DateTime values.
fn map_schedule_effect(
    effect: &sched_dsl::ScheduleLifecycleEffect,
    planning_cursor_utc: DateTime<Utc>,
    next_occurrence_ordinal: OccurrenceOrdinal,
) -> ScheduleLifecycleEffect {
    match effect {
        sched_dsl::ScheduleLifecycleEffect::EmitScheduleNotice {
            new_state,
            revision,
        } => ScheduleLifecycleEffect::EmitScheduleNotice {
            new_state: match new_state {
                sched_dsl::ScheduleLifecycleState::Active => SchedulePhase::Active,
                sched_dsl::ScheduleLifecycleState::Paused => SchedulePhase::Paused,
                sched_dsl::ScheduleLifecycleState::Deleted => SchedulePhase::Deleted,
            },
            revision: ScheduleRevision(*revision),
        },
        sched_dsl::ScheduleLifecycleEffect::SupersedePendingOccurrences {
            superseding_revision,
        } => ScheduleLifecycleEffect::SupersedePendingOccurrences {
            superseding_revision: ScheduleRevision(*superseding_revision),
        },
        sched_dsl::ScheduleLifecycleEffect::PlanningWindowRecorded { .. } => {
            ScheduleLifecycleEffect::PlanningWindowRecorded {
                planning_cursor_utc,
                next_occurrence_ordinal,
            }
        }
    }
}

fn map_schedule_effects(
    transition: &sched_dsl::ScheduleLifecycleMachineTransition,
    planning_window: Option<(DateTime<Utc>, OccurrenceOrdinal)>,
) -> Vec<ScheduleLifecycleEffect> {
    let (planning_cursor_utc, next_occurrence_ordinal) =
        planning_window.unwrap_or_else(|| (DateTime::default(), OccurrenceOrdinal::default()));
    transition
        .effects
        .iter()
        .map(|effect| map_schedule_effect(effect, planning_cursor_utc, next_occurrence_ordinal))
        .collect()
}

// ===========================================================================
// Conversion helpers
// ===========================================================================

/// Produce a complete semantic key for a trigger spec. The DSL stores the
/// opaque key, and shell code may persist the full trigger only when the key
/// accepted by generated authority matches this structured serialization.
fn trigger_stable_key(trigger: &TriggerSpec) -> String {
    match serde_json::to_string(trigger) {
        Ok(json) => format!("trigger:{json}"),
        Err(error) => format!("trigger:serialization-error:{error}"),
    }
}

fn receipt_to_dsl(receipt: &DeliveryReceipt) -> occ_dsl::DeliveryReceipt {
    occ_dsl::DeliveryReceipt(serde_json::to_string(receipt).unwrap_or_default())
}

fn runtime_outcome_authority_key(outcome: &RuntimeDeliveryOutcome) -> String {
    match serde_json::to_string(outcome) {
        Ok(json) => format!("runtime_outcome:{json}"),
        Err(error) => format!("runtime_outcome:serialization-error:{error}"),
    }
}

fn misfire_policy_authority_key(policy: &crate::types::MisfirePolicy) -> String {
    match serde_json::to_string(policy) {
        Ok(json) => format!("misfire_policy:{json}"),
        Err(error) => format!("misfire_policy:serialization-error:{error}"),
    }
}

fn overlap_policy_authority_key(policy: &crate::types::OverlapPolicy) -> String {
    match serde_json::to_string(policy) {
        Ok(json) => format!("overlap_policy:{json}"),
        Err(error) => format!("overlap_policy:serialization-error:{error}"),
    }
}

fn missing_target_policy_authority_key(policy: &crate::types::MissingTargetPolicy) -> String {
    match serde_json::to_string(policy) {
        Ok(json) => format!("missing_target_policy:{json}"),
        Err(error) => format!("missing_target_policy:serialization-error:{error}"),
    }
}

fn datetime_to_millis(dt: DateTime<Utc>) -> u64 {
    u64::try_from(dt.timestamp_millis()).unwrap_or(0)
}

fn millis_to_datetime(ms: u64) -> Option<DateTime<Utc>> {
    let ms_i64 = i64::try_from(ms).ok()?;
    DateTime::from_timestamp_millis(ms_i64)
}

fn to_dsl_failure_class(fc: OccurrenceFailureClass) -> occ_dsl::FailureClass {
    match fc {
        OccurrenceFailureClass::TargetMaterializationFailed => {
            occ_dsl::FailureClass::TargetMaterializationFailed
        }
        OccurrenceFailureClass::TargetMissing => occ_dsl::FailureClass::TargetMissing,
        OccurrenceFailureClass::TargetBusy => occ_dsl::FailureClass::TargetBusy,
        OccurrenceFailureClass::RuntimeRejected => occ_dsl::FailureClass::RuntimeRejected,
        OccurrenceFailureClass::MobRejected => occ_dsl::FailureClass::MobRejected,
        OccurrenceFailureClass::LeaseLost => occ_dsl::FailureClass::LeaseLost,
        OccurrenceFailureClass::TransportError => occ_dsl::FailureClass::TransportError,
        OccurrenceFailureClass::InternalError => occ_dsl::FailureClass::InternalError,
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
    fn supersede_emits_occurrence_ack_effect() -> Result<(), Box<dyn std::error::Error>> {
        let occurrence = sample_occurrence();
        let superseding_revision = ScheduleRevision(2);

        let mutator = occurrence
            .clone()
            .apply(OccurrenceLifecycleInput::Supersede {
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
                    occurrence_id: occurrence.occurrence_id,
                    superseding_revision,
                },
            ]
        );
        Ok(())
    }

    #[test]
    fn schedule_records_supersede_ack_without_effects() -> Result<(), Box<dyn std::error::Error>> {
        let schedule = sample_schedule();
        let occurrence_id = crate::types::OccurrenceId::new();
        let revision = schedule.revision.next();

        let mutator = Schedule::apply(
            Some(schedule.clone()),
            ScheduleLifecycleInput::ConfirmOccurrencesSuperseded {
                occurrence_id: occurrence_id.clone(),
                superseding_revision: revision,
            },
        )?;

        assert_eq!(mutator.schedule.phase, SchedulePhase::Active);
        assert_eq!(mutator.schedule.revision, schedule.revision);
        assert!(mutator.effects.is_empty());
        assert!(mutator.schedule.superseded_ack_ids.contains(&occurrence_id));
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
