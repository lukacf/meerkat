use crate::types::{
    CreateScheduleRequest, DeliveryReceipt, Occurrence, OccurrenceFailureClass, OccurrenceOrdinal,
    OccurrencePhase, Schedule, SchedulePhase, ScheduleRevision, UpdateScheduleRequest,
};
use chrono::{DateTime, Utc};
use uuid::Uuid;

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
                effects: vec![ScheduleLifecycleEffect::EmitScheduleNotice {
                    new_state: SchedulePhase::Active,
                    revision: ScheduleRevision::initial(),
                }],
                revision_bumped: false,
            }),
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
            ScheduleLifecycleInput::RecordPlanningWindow {
                planning_cursor_utc,
                next_occurrence_ordinal,
            } => {
                let mut schedule = schedule.ok_or(ScheduleLifecycleError::MissingSchedule)?;
                if schedule.phase == SchedulePhase::Deleted {
                    return Err(ScheduleLifecycleError::Deleted);
                }
                schedule.planning_cursor_utc = Some(planning_cursor_utc);
                schedule.next_occurrence_ordinal = next_occurrence_ordinal;
                let phase = schedule.phase;
                let revision = schedule.revision;
                Ok(ScheduleLifecycleMutator {
                    schedule,
                    effects: vec![
                        ScheduleLifecycleEffect::EmitScheduleNotice {
                            new_state: phase,
                            revision,
                        },
                        ScheduleLifecycleEffect::PlanningWindowRecorded {
                            planning_cursor_utc,
                            next_occurrence_ordinal,
                        },
                    ],
                    revision_bumped: false,
                })
            }
            ScheduleLifecycleInput::Pause { at_utc } => {
                let mut schedule = schedule.ok_or(ScheduleLifecycleError::MissingSchedule)?;
                if schedule.phase == SchedulePhase::Deleted {
                    return Err(ScheduleLifecycleError::Deleted);
                }
                schedule.phase = SchedulePhase::Paused;
                schedule.updated_at_utc = at_utc;
                let revision = schedule.revision;
                Ok(ScheduleLifecycleMutator {
                    schedule,
                    effects: vec![ScheduleLifecycleEffect::EmitScheduleNotice {
                        new_state: SchedulePhase::Paused,
                        revision,
                    }],
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
                let revision = schedule.revision;
                Ok(ScheduleLifecycleMutator {
                    schedule,
                    effects: vec![ScheduleLifecycleEffect::EmitScheduleNotice {
                        new_state: SchedulePhase::Active,
                        revision,
                    }],
                    revision_bumped: false,
                })
            }
            ScheduleLifecycleInput::Delete { at_utc } => {
                let mut schedule = schedule.ok_or(ScheduleLifecycleError::MissingSchedule)?;
                schedule.revision = schedule.revision.next();
                schedule.phase = SchedulePhase::Deleted;
                schedule.planning_cursor_utc = None;
                schedule.deleted_at_utc = Some(at_utc);
                schedule.updated_at_utc = at_utc;
                let revision = schedule.revision;
                Ok(ScheduleLifecycleMutator {
                    schedule,
                    effects: vec![
                        ScheduleLifecycleEffect::EmitScheduleNotice {
                            new_state: SchedulePhase::Deleted,
                            revision,
                        },
                        ScheduleLifecycleEffect::SupersedePendingOccurrences {
                            superseding_revision: revision,
                        },
                    ],
                    revision_bumped: true,
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
                claim_token,
            } => {
                if occurrence.phase != OccurrencePhase::Pending {
                    return Err(OccurrenceLifecycleError::NotPendingForClaim);
                }
                occurrence.phase = OccurrencePhase::Claimed;
                occurrence.claimed_by = Some(owner_id);
                occurrence.lease_expires_at_utc = Some(lease_expires_at_utc);
                occurrence.claimed_at_utc = Some(at_utc);
                occurrence.claim_token = Some(claim_token);
                occurrence.delivery_correlation_id = None;
                occurrence.last_receipt = None;
                occurrence.failure_class = None;
                occurrence.failure_detail = None;
                occurrence.dispatched_at_utc = None;
                occurrence.completed_at_utc = None;
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
                    return Err(OccurrenceLifecycleError::NotDispatching);
                }
                occurrence.phase = OccurrencePhase::AwaitingCompletion;
                occurrence.dispatched_at_utc = Some(at_utc);
                effects.push(OccurrenceLifecycleEffect::AwaitingCompletion);
            }
            OccurrenceLifecycleInput::Complete { receipt, at_utc } => {
                if !matches!(
                    occurrence.phase,
                    OccurrencePhase::Dispatching | OccurrencePhase::AwaitingCompletion
                ) {
                    return Err(OccurrenceLifecycleError::NotLiveForTerminal);
                }
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
                if !matches!(
                    occurrence.phase,
                    OccurrencePhase::Pending
                        | OccurrencePhase::Claimed
                        | OccurrencePhase::Dispatching
                        | OccurrencePhase::AwaitingCompletion
                ) {
                    return Err(OccurrenceLifecycleError::NotLiveForTerminal);
                }
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
                if !matches!(
                    occurrence.phase,
                    OccurrencePhase::Pending
                        | OccurrencePhase::Claimed
                        | OccurrencePhase::Dispatching
                        | OccurrencePhase::AwaitingCompletion
                ) {
                    return Err(OccurrenceLifecycleError::NotLiveForTerminal);
                }
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
                if !matches!(
                    occurrence.phase,
                    OccurrencePhase::Pending
                        | OccurrencePhase::Claimed
                        | OccurrencePhase::Dispatching
                        | OccurrencePhase::AwaitingCompletion
                ) {
                    return Err(OccurrenceLifecycleError::NotLiveForTerminal);
                }
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
                if !matches!(
                    occurrence.phase,
                    OccurrencePhase::Claimed
                        | OccurrencePhase::Dispatching
                        | OccurrencePhase::AwaitingCompletion
                ) {
                    return Err(OccurrenceLifecycleError::NotLiveForTerminal);
                }
                occurrence.phase = OccurrencePhase::DeliveryFailed;
                occurrence.completed_at_utc = Some(at_utc);
                occurrence.failure_class = Some(failure_class);
                occurrence.failure_detail = detail;
                occurrence.last_receipt = receipt;
                effects.push(OccurrenceLifecycleEffect::DeliveryFailed);
            }
            OccurrenceLifecycleInput::LeaseExpired { at_utc: _ } => {
                if !matches!(
                    occurrence.phase,
                    OccurrencePhase::Claimed
                        | OccurrencePhase::Dispatching
                        | OccurrencePhase::AwaitingCompletion
                ) {
                    return Err(OccurrenceLifecycleError::NotLeaseHolding);
                }
                occurrence.phase = OccurrencePhase::Pending;
                occurrence.claimed_by = None;
                occurrence.lease_expires_at_utc = None;
                occurrence.claim_token = None;
                occurrence.delivery_correlation_id = None;
                occurrence.claimed_at_utc = None;
                occurrence.dispatched_at_utc = None;
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
    occurrence.claimed_by = None;
    occurrence.lease_expires_at_utc = None;
    occurrence.claim_token = None;
    occurrence.delivery_correlation_id = None;
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
