use crate::types::{
    CreateScheduleRequest, DeliveryReceipt, Occurrence, OccurrenceFailureClass, OccurrencePhase,
    Schedule, SchedulePhase, ScheduleRevision, UpdateScheduleRequest,
};
use chrono::{DateTime, Utc};

#[derive(Debug, Clone, PartialEq)]
pub enum ScheduleLifecycleInput {
    Create(CreateScheduleRequest),
    Update(UpdateScheduleRequest),
    Pause { at_utc: DateTime<Utc> },
    Resume { at_utc: DateTime<Utc> },
    Delete { at_utc: DateTime<Utc> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScheduleLifecycleEffect {
    Created,
    RevisionBumped {
        previous_revision: ScheduleRevision,
        new_revision: ScheduleRevision,
    },
    Paused,
    Resumed,
    Deleted,
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

#[derive(Debug, Default)]
pub struct ScheduleLifecycleAuthority;

impl ScheduleLifecycleAuthority {
    pub fn apply(
        &self,
        schedule: Option<Schedule>,
        input: ScheduleLifecycleInput,
    ) -> Result<ScheduleLifecycleMutator, ScheduleLifecycleError> {
        match input {
            ScheduleLifecycleInput::Create(request) => Ok(ScheduleLifecycleMutator {
                schedule: Schedule::new(request),
                effects: vec![ScheduleLifecycleEffect::Created],
                revision_bumped: false,
            }),
            ScheduleLifecycleInput::Update(request) => {
                let mut schedule = schedule.ok_or(ScheduleLifecycleError::MissingSchedule)?;
                let previous_revision = schedule.revision;
                let revision_bumped = self.apply_update(&mut schedule, request)?;
                let mut effects = Vec::new();
                if revision_bumped {
                    effects.push(ScheduleLifecycleEffect::RevisionBumped {
                        previous_revision,
                        new_revision: schedule.revision,
                    });
                }
                Ok(ScheduleLifecycleMutator {
                    schedule,
                    effects,
                    revision_bumped,
                })
            }
            ScheduleLifecycleInput::Pause { at_utc } => {
                let mut schedule = schedule.ok_or(ScheduleLifecycleError::MissingSchedule)?;
                if schedule.phase == SchedulePhase::Deleted {
                    return Err(ScheduleLifecycleError::Deleted);
                }
                schedule.phase = SchedulePhase::Paused;
                schedule.updated_at_utc = at_utc;
                Ok(ScheduleLifecycleMutator {
                    schedule,
                    effects: vec![ScheduleLifecycleEffect::Paused],
                    revision_bumped: false,
                })
            }
            ScheduleLifecycleInput::Resume { at_utc } => {
                let mut schedule = schedule.ok_or(ScheduleLifecycleError::MissingSchedule)?;
                if schedule.phase == SchedulePhase::Deleted {
                    return Err(ScheduleLifecycleError::Deleted);
                }
                schedule.phase = SchedulePhase::Active;
                schedule.updated_at_utc = at_utc;
                Ok(ScheduleLifecycleMutator {
                    schedule,
                    effects: vec![ScheduleLifecycleEffect::Resumed],
                    revision_bumped: false,
                })
            }
            ScheduleLifecycleInput::Delete { at_utc } => {
                let mut schedule = schedule.ok_or(ScheduleLifecycleError::MissingSchedule)?;
                schedule.phase = SchedulePhase::Deleted;
                schedule.deleted_at_utc = Some(at_utc);
                schedule.updated_at_utc = at_utc;
                Ok(ScheduleLifecycleMutator {
                    schedule,
                    effects: vec![ScheduleLifecycleEffect::Deleted],
                    revision_bumped: false,
                })
            }
        }
    }

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

        if let Some(name) = request.name {
            schedule.name = Some(name);
        }
        if let Some(description) = request.description {
            schedule.description = Some(description);
        }
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
        if let Some(days) = request.planning_horizon_days {
            schedule.planning_horizon_days = days;
        }
        if let Some(count) = request.planning_horizon_occurrences {
            schedule.planning_horizon_occurrences = count;
        }
        if let Some(labels) = request.labels {
            schedule.labels = labels;
        }

        if revision_affecting_change {
            schedule.revision = schedule.revision.next();
            schedule.planning_cursor_utc = None;
        }
        schedule.touch();
        Ok(revision_affecting_change)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum OccurrenceLifecycleInput {
    Claim {
        owner_id: String,
        at_utc: DateTime<Utc>,
        lease_expires_at_utc: DateTime<Utc>,
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
    #[error("occurrence must be claimed before dispatching")]
    NotClaimed,
}

#[derive(Debug, Default)]
pub struct OccurrenceLifecycleAuthority;

impl OccurrenceLifecycleAuthority {
    pub fn apply(
        &self,
        mut occurrence: Occurrence,
        input: OccurrenceLifecycleInput,
    ) -> Result<OccurrenceLifecycleMutator, OccurrenceLifecycleError> {
        if occurrence.phase.is_terminal() {
            return Err(OccurrenceLifecycleError::AlreadyTerminal);
        }

        let mut effects = Vec::new();
        match input {
            OccurrenceLifecycleInput::Claim {
                owner_id,
                at_utc,
                lease_expires_at_utc,
            } => {
                occurrence.phase = OccurrencePhase::Claimed;
                occurrence.claimed_by = Some(owner_id);
                occurrence.lease_expires_at_utc = Some(lease_expires_at_utc);
                occurrence.claimed_at_utc = Some(at_utc);
                occurrence.attempt_count = occurrence.attempt_count.saturating_add(1);
                effects.push(OccurrenceLifecycleEffect::Claimed);
            }
            OccurrenceLifecycleInput::DispatchStarted {
                correlation_id,
                at_utc,
            } => {
                if occurrence.phase != OccurrencePhase::Claimed {
                    return Err(OccurrenceLifecycleError::NotClaimed);
                }
                occurrence.phase = OccurrencePhase::Dispatching;
                occurrence.delivery_correlation_id = correlation_id;
                occurrence.dispatched_at_utc = Some(at_utc);
                effects.push(OccurrenceLifecycleEffect::DispatchStarted);
            }
            OccurrenceLifecycleInput::AwaitCompletion { at_utc } => {
                if occurrence.phase != OccurrencePhase::Dispatching {
                    return Err(OccurrenceLifecycleError::NotClaimed);
                }
                occurrence.phase = OccurrencePhase::AwaitingCompletion;
                occurrence.dispatched_at_utc = Some(at_utc);
                effects.push(OccurrenceLifecycleEffect::AwaitingCompletion);
            }
            OccurrenceLifecycleInput::Complete { receipt, at_utc } => {
                occurrence.phase = OccurrencePhase::Completed;
                occurrence.completed_at_utc = Some(at_utc);
                occurrence.last_receipt = Some(receipt);
                effects.push(OccurrenceLifecycleEffect::Completed);
            }
            OccurrenceLifecycleInput::Skip {
                detail,
                failure_class,
                at_utc,
            } => {
                terminalize(
                    &mut occurrence,
                    OccurrencePhase::Skipped,
                    detail,
                    failure_class,
                    at_utc,
                );
                effects.push(OccurrenceLifecycleEffect::Skipped);
            }
            OccurrenceLifecycleInput::Misfire {
                detail,
                failure_class,
                at_utc,
            } => {
                terminalize(
                    &mut occurrence,
                    OccurrencePhase::Misfired,
                    detail,
                    failure_class,
                    at_utc,
                );
                effects.push(OccurrenceLifecycleEffect::Misfired);
            }
            OccurrenceLifecycleInput::Supersede {
                superseded_by_revision,
                at_utc,
            } => {
                occurrence.phase = OccurrencePhase::Superseded;
                occurrence.completed_at_utc = Some(at_utc);
                occurrence.superseded_by_revision = Some(superseded_by_revision);
                effects.push(OccurrenceLifecycleEffect::Superseded);
            }
            OccurrenceLifecycleInput::DeliveryFailed {
                receipt,
                failure_class,
                detail,
                at_utc,
            } => {
                occurrence.phase = OccurrencePhase::DeliveryFailed;
                occurrence.completed_at_utc = Some(at_utc);
                occurrence.failure_class = Some(failure_class);
                occurrence.failure_detail = detail;
                occurrence.last_receipt = receipt;
                effects.push(OccurrenceLifecycleEffect::DeliveryFailed);
            }
            OccurrenceLifecycleInput::LeaseExpired { at_utc: _ } => {
                occurrence.phase = OccurrencePhase::Pending;
                occurrence.claimed_by = None;
                occurrence.lease_expires_at_utc = None;
                effects.push(OccurrenceLifecycleEffect::LeaseExpired);
            }
        }

        Ok(OccurrenceLifecycleMutator {
            occurrence,
            effects,
        })
    }
}

fn terminalize(
    occurrence: &mut Occurrence,
    phase: OccurrencePhase,
    detail: Option<String>,
    failure_class: Option<OccurrenceFailureClass>,
    at_utc: DateTime<Utc>,
) {
    occurrence.phase = phase;
    occurrence.completed_at_utc = Some(at_utc);
    occurrence.failure_detail = detail;
    occurrence.failure_class = failure_class;
    occurrence.lease_expires_at_utc = None;
}
