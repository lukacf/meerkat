use crate::machines::occurrence_lifecycle as occ_dsl;
use crate::machines::schedule_lifecycle as sched_dsl;
use crate::types::{
    CreateScheduleRequest, DeliveryReceipt, Occurrence, OccurrenceFailureClass, OccurrenceOrdinal,
    OccurrencePhase, Schedule, SchedulePhase, ScheduleRevision, TriggerSpec, UpdateScheduleRequest,
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
    Pause {
        at_utc: DateTime<Utc>,
    },
    Resume {
        at_utc: DateTime<Utc>,
    },
    Delete {
        at_utc: DateTime<Utc>,
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
}

// ---------------------------------------------------------------------------
// Occurrence lifecycle — public types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum OccurrenceLifecycleInput {
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
}

// ===========================================================================
// OccurrenceLifecycleAuthority — DSL-backed dispatch
// ===========================================================================

#[derive(Debug, Default)]
pub struct OccurrenceLifecycleAuthority;

impl OccurrenceLifecycleAuthority {
    pub fn apply(
        &self,
        mut occurrence: Occurrence,
        input: OccurrenceLifecycleInput,
    ) -> Result<OccurrenceLifecycleMutator, OccurrenceLifecycleError> {
        // 1. Project domain → DSL state
        let dsl_state = project_occurrence(&occurrence);

        // 2. Convert authority input → DSL input
        let dsl_input = convert_occurrence_input(&input);

        // 3. Run DSL dispatch
        let mut dsl_auth = occ_dsl::OccurrenceLifecycleMachineAuthority::from_state(dsl_state);
        let transition =
            occ_dsl::OccurrenceLifecycleMachineMutator::apply(&mut dsl_auth, dsl_input)
                .map_err(|e| map_occurrence_error(e, &input))?;

        // 4. Write DSL state → occurrence
        write_back_occurrence(&dsl_auth.state, &mut occurrence, &input);

        // 5. Map effects
        let effects = transition
            .effects
            .iter()
            .map(map_occurrence_effect)
            .collect();

        Ok(OccurrenceLifecycleMutator {
            occurrence,
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
        target_binding_key: occ.target_snapshot.stable_key(),
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
        failure_class: occ.failure_class.map(to_dsl_failure_class),
        failure_detail: occ.failure_detail.clone(),
        dispatched_at_utc_ms: occ.dispatched_at_utc.map(datetime_to_millis),
        completed_at_utc_ms: occ.completed_at_utc.map(datetime_to_millis),
        attempt_count: u64::from(occ.attempt_count),
        superseded_by_revision: occ.superseded_by_revision.map(|r| r.0),
    }
}

/// Convert authority input to DSL input.
fn convert_occurrence_input(input: &OccurrenceLifecycleInput) -> occ_dsl::OccurrenceLifecycleInput {
    match input {
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
/// The DSL state is the authority for machine-owned fields (phase, claimed_by,
/// timestamps, etc). Non-machine fields (trigger_snapshot, target_snapshot,
/// policies, created_at_utc) are left untouched since the DSL does not track them.
fn write_back_occurrence(
    dsl: &occ_dsl::OccurrenceLifecycleMachineState,
    occ: &mut Occurrence,
    input: &OccurrenceLifecycleInput,
) {
    // Phase
    occ.phase = match dsl.lifecycle_phase {
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
    };

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
            None
        }
        _ => {
            // Other transitions don't touch last_receipt in the DSL,
            // so preserve the original domain value.
            occ.last_receipt.take()
        }
    };
}

/// Map DSL error to the appropriate authority error variant.
fn map_occurrence_error(
    _error: occ_dsl::OccurrenceLifecycleMachineTransitionError,
    input: &OccurrenceLifecycleInput,
) -> OccurrenceLifecycleError {
    // The DSL returns NoMatchingTransition { phase, trigger }. We reconstruct
    // the specific error variant based on which input was attempted.
    match input {
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

/// Map DSL effect → authority effect (1:1 by name).
fn map_occurrence_effect(effect: &occ_dsl::OccurrenceLifecycleEffect) -> OccurrenceLifecycleEffect {
    match effect {
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
        occ_dsl::OccurrenceLifecycleEffect::DeliveryFailed => {
            OccurrenceLifecycleEffect::DeliveryFailed
        }
        occ_dsl::OccurrenceLifecycleEffect::LeaseExpired => OccurrenceLifecycleEffect::LeaseExpired,
    }
}

// ===========================================================================
// ScheduleLifecycleAuthority — DSL-backed dispatch
// ===========================================================================

#[derive(Debug, Default)]
pub struct ScheduleLifecycleAuthority;

impl ScheduleLifecycleAuthority {
    pub fn apply(
        &self,
        schedule: Option<Schedule>,
        input: ScheduleLifecycleInput,
    ) -> Result<ScheduleLifecycleMutator, ScheduleLifecycleError> {
        match input {
            // Create is special: constructs a fresh schedule, no prior state to project
            ScheduleLifecycleInput::Create(request) => {
                let schedule = Schedule::new(request);
                // The DSL would emit EmitScheduleNotice, but Create constructs
                // the schedule outside the DSL so we emit manually.
                Ok(ScheduleLifecycleMutator {
                    effects: vec![ScheduleLifecycleEffect::EmitScheduleNotice {
                        new_state: schedule.phase,
                        revision: schedule.revision,
                    }],
                    schedule,
                    revision_bumped: false,
                })
            }

            // Update mixes config changes (name, labels, horizons) with
            // revision-affecting changes (trigger, target, policies). The DSL
            // handles only the machine-owned Revise transition.
            ScheduleLifecycleInput::Update(request) => {
                let mut schedule = schedule.ok_or(ScheduleLifecycleError::MissingSchedule)?;
                let revision_bumped = self.apply_update(&mut schedule, request)?;
                let mut effects = vec![ScheduleLifecycleEffect::EmitScheduleNotice {
                    new_state: schedule.phase,
                    revision: schedule.revision,
                }];
                if revision_bumped {
                    effects.push(ScheduleLifecycleEffect::SupersedePendingOccurrences {
                        superseding_revision: schedule.revision,
                    });
                }
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
                let (transition, dsl_state) = self.run_schedule_dsl(&schedule, dsl_input)?;
                write_back_schedule(&dsl_state, &mut schedule);
                let effects = transition
                    .effects
                    .iter()
                    .map(|e| map_schedule_effect(e, planning_cursor_utc, next_occurrence_ordinal))
                    .collect();
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
                let (transition, dsl_state) = self.run_schedule_dsl(&schedule, dsl_input)?;
                write_back_schedule(&dsl_state, &mut schedule);
                schedule.config.updated_at_utc = at_utc;
                let effects = transition
                    .effects
                    .iter()
                    .map(|e| {
                        map_schedule_effect(e, DateTime::default(), OccurrenceOrdinal::default())
                    })
                    .collect();
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
                let (transition, dsl_state) = self.run_schedule_dsl(&schedule, dsl_input)?;
                write_back_schedule(&dsl_state, &mut schedule);
                schedule.config.updated_at_utc = at_utc;
                let effects = transition
                    .effects
                    .iter()
                    .map(|e| {
                        map_schedule_effect(e, DateTime::default(), OccurrenceOrdinal::default())
                    })
                    .collect();
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
                let (transition, dsl_state) = self.run_schedule_dsl(&schedule, dsl_input)?;
                write_back_schedule(&dsl_state, &mut schedule);
                schedule.config.deleted_at_utc = Some(at_utc);
                schedule.config.updated_at_utc = at_utc;
                let revision_bumped = schedule.revision != old_revision;
                let effects = transition
                    .effects
                    .iter()
                    .map(|e| {
                        map_schedule_effect(e, DateTime::default(), OccurrenceOrdinal::default())
                    })
                    .collect();
                Ok(ScheduleLifecycleMutator {
                    schedule,
                    effects,
                    revision_bumped,
                })
            }
        }
    }

    /// Apply config-level update changes. If any revision-affecting field changes,
    /// delegate the revision bump to the DSL via a Revise input.
    pub fn apply_update(
        &self,
        schedule: &mut Schedule,
        request: UpdateScheduleRequest,
    ) -> Result<bool, ScheduleLifecycleError> {
        if schedule.phase == SchedulePhase::Deleted {
            return Err(ScheduleLifecycleError::Deleted);
        }

        if let Some(expected_revision) = request.expected_revision
            && expected_revision != schedule.revision
        {
            return Err(ScheduleLifecycleError::RevisionMismatch {
                expected: expected_revision.0,
                actual: schedule.revision.0,
            });
        }

        let mut revision_affecting_change = false;

        // Config-only changes (not tracked by the DSL)
        if let Some(name) = request.name {
            schedule.config.name = Some(name);
        }
        if let Some(description) = request.description {
            schedule.config.description = Some(description);
        }
        if let Some(days) = request.planning_horizon_days {
            schedule.config.planning_horizon_days = days;
        }
        if let Some(count) = request.planning_horizon_occurrences {
            schedule.config.planning_horizon_occurrences = count;
        }
        if let Some(labels) = request.labels {
            schedule.config.labels = labels;
        }

        // Revision-affecting changes — detect diffs
        if let Some(trigger) = request.trigger {
            revision_affecting_change |= trigger != schedule.trigger;
            schedule.trigger = trigger;
        }
        if let Some(target) = request.target {
            revision_affecting_change |= target != schedule.target;
            schedule.target = target;
        }
        if let Some(policy) = request.misfire_policy {
            revision_affecting_change |= policy != schedule.misfire_policy;
            schedule.misfire_policy = policy;
        }
        if let Some(policy) = request.overlap_policy {
            revision_affecting_change |= policy != schedule.overlap_policy;
            schedule.overlap_policy = policy;
        }
        if let Some(policy) = request.missing_target_policy {
            revision_affecting_change |= policy != schedule.missing_target_policy;
            schedule.missing_target_policy = policy;
        }

        if revision_affecting_change {
            // Use the DSL Revise transition for the revision bump + planning cursor clear
            let dsl_input = sched_dsl::ScheduleLifecycleInput::Revise {
                trigger_key: trigger_stable_key(&schedule.trigger),
                target_binding_key: schedule.target.stable_key(),
                misfire_policy: to_dsl_misfire_policy(&schedule.misfire_policy),
                overlap_policy: to_dsl_overlap_policy(&schedule.overlap_policy),
                missing_target_policy: to_dsl_missing_target_policy(
                    &schedule.missing_target_policy,
                ),
            };
            let (_transition, dsl_state) = self.run_schedule_dsl(schedule, dsl_input)?;
            write_back_schedule(&dsl_state, schedule);
        }
        schedule.touch();
        Ok(revision_affecting_change)
    }

    /// Project schedule → DSL state, apply DSL input, return transition + new state.
    fn run_schedule_dsl(
        &self,
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
        let mut dsl_auth = sched_dsl::ScheduleLifecycleMachineAuthority::from_state(dsl_state);
        let transition =
            sched_dsl::ScheduleLifecycleMachineMutator::apply(&mut dsl_auth, dsl_input)
                .map_err(|_| ScheduleLifecycleError::Deleted)?;
        Ok((transition, dsl_auth.state))
    }
}

/// Project a domain `Schedule` into the DSL flat state struct.
fn project_schedule(sched: &Schedule) -> sched_dsl::ScheduleLifecycleMachineState {
    sched_dsl::ScheduleLifecycleMachineState {
        lifecycle_phase: match sched.phase {
            SchedulePhase::Active => sched_dsl::ScheduleLifecycleState::Active,
            SchedulePhase::Paused => sched_dsl::ScheduleLifecycleState::Paused,
            SchedulePhase::Deleted => sched_dsl::ScheduleLifecycleState::Deleted,
        },
        revision: sched.revision.0,
        trigger_key: trigger_stable_key(&sched.trigger),
        target_binding_key: sched.target.stable_key(),
        misfire_policy: to_dsl_misfire_policy(&sched.misfire_policy),
        overlap_policy: to_dsl_overlap_policy(&sched.overlap_policy),
        missing_target_policy: to_dsl_missing_target_policy(&sched.missing_target_policy),
        planning_cursor_utc_ms: sched.planning_cursor_utc.map(datetime_to_millis),
        next_occurrence_ordinal: sched.next_occurrence_ordinal.0,
    }
}

/// Write DSL state back into the domain schedule (machine-owned fields only).
fn write_back_schedule(dsl: &sched_dsl::ScheduleLifecycleMachineState, sched: &mut Schedule) {
    sched.phase = match dsl.lifecycle_phase {
        sched_dsl::ScheduleLifecycleState::Active => SchedulePhase::Active,
        sched_dsl::ScheduleLifecycleState::Paused => SchedulePhase::Paused,
        sched_dsl::ScheduleLifecycleState::Deleted => SchedulePhase::Deleted,
    };
    sched.revision = ScheduleRevision(dsl.revision);
    sched.planning_cursor_utc = dsl.planning_cursor_utc_ms.and_then(millis_to_datetime);
    sched.next_occurrence_ordinal = OccurrenceOrdinal(dsl.next_occurrence_ordinal);
}

/// Map DSL schedule effect → authority effect.
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

// ===========================================================================
// Conversion helpers
// ===========================================================================

/// Produce a stable string key for a trigger spec. The DSL tracks this as an
/// opaque string — equality is used only for revision-affecting change detection.
fn trigger_stable_key(trigger: &TriggerSpec) -> String {
    match trigger {
        TriggerSpec::Once { due_at_utc } => format!("once:{due_at_utc}"),
        TriggerSpec::Interval(spec) => {
            format!(
                "interval:{}:{}:{:?}",
                spec.start_at_utc, spec.every_seconds, spec.end_at_utc
            )
        }
        TriggerSpec::Calendar(spec) => format!("calendar:{}", spec.timezone),
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

fn to_dsl_misfire_policy(policy: &crate::types::MisfirePolicy) -> sched_dsl::MisfirePolicy {
    match policy {
        crate::types::MisfirePolicy::Skip => sched_dsl::MisfirePolicy::Skip,
        crate::types::MisfirePolicy::CatchUpWithin { .. } => {
            sched_dsl::MisfirePolicy::CatchUpWithin
        }
    }
}

fn to_dsl_overlap_policy(policy: &crate::types::OverlapPolicy) -> sched_dsl::OverlapPolicy {
    match policy {
        crate::types::OverlapPolicy::AllowConcurrent => sched_dsl::OverlapPolicy::AllowConcurrent,
        crate::types::OverlapPolicy::SkipIfRunning => sched_dsl::OverlapPolicy::SkipIfRunning,
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
        let schedule = Schedule::new(CreateScheduleRequest {
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
                    skill_references: Vec::new(),
                    additional_instructions: Vec::new(),
                },
            }),
            misfire_policy: MisfirePolicy::Skip,
            overlap_policy: OverlapPolicy::SkipIfRunning,
            missing_target_policy: MissingTargetPolicy::MarkMisfired,
            labels: BTreeMap::new(),
            planning_horizon_days: Some(1),
            planning_horizon_occurrences: Some(1),
        });
        Occurrence::planned_from_schedule(&schedule, crate::OccurrenceOrdinal(0), Utc::now())
    }

    #[test]
    fn claim_rejects_non_pending_occurrences() {
        let authority = OccurrenceLifecycleAuthority;
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
                authority.apply(occurrence, input.clone()).is_err(),
                "claim should reject occurrences already in phase {phase:?}"
            );
        }
    }

    #[test]
    fn await_completion_requires_dispatching_phase() {
        let authority = OccurrenceLifecycleAuthority;
        let occurrence = sample_occurrence();
        let result = authority.apply(
            occurrence,
            OccurrenceLifecycleInput::AwaitCompletion { at_utc: Utc::now() },
        );
        assert!(matches!(
            result,
            Err(OccurrenceLifecycleError::NotDispatching)
        ));
    }
}
