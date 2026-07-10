use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};

#[cfg(feature = "schema")]
use schemars::JsonSchema;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
pub struct ListSchedulesParams {
    pub labels: Option<BTreeMap<String, String>>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
pub struct ScheduleIdParams {
    pub schedule_id: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
pub struct ScheduleOccurrencesParams {
    pub schedule_id: String,
    pub include_terminal: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
pub struct UpdateScheduleParams {
    pub schedule_id: String,
    #[serde(flatten)]
    pub update: meerkat_schedule::UpdateScheduleRequest,
}

/// Canonical public projection returned by every `schedule/*` RPC method.
///
/// The durable schedule domain object has a custom persistence serializer
/// which includes generated machine authority state. That persistence shape
/// must not double as the public contract. This projection deliberately keeps
/// the existing flattened schedule configuration while excluding the private
/// machine state, and it is the single source for both production responses
/// and generated SDK schemas.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
pub struct Schedule {
    pub schedule_id: meerkat_schedule::ScheduleId,
    pub phase: meerkat_schedule::SchedulePhase,
    pub revision: meerkat_schedule::ScheduleRevision,
    pub trigger: meerkat_schedule::TriggerSpec,
    pub target: meerkat_schedule::TargetBinding,
    pub misfire_policy: meerkat_schedule::MisfirePolicy,
    pub overlap_policy: meerkat_schedule::OverlapPolicy,
    pub missing_target_policy: meerkat_schedule::MissingTargetPolicy,
    pub next_occurrence_ordinal: meerkat_schedule::OccurrenceOrdinal,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub planning_cursor_utc: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub superseded_ack_ids: BTreeSet<meerkat_schedule::OccurrenceId>,
    #[serde(flatten)]
    #[cfg_attr(feature = "schema", schemars(flatten))]
    pub config: meerkat_schedule::ScheduleConfig,
}

impl From<&meerkat_schedule::Schedule> for Schedule {
    fn from(schedule: &meerkat_schedule::Schedule) -> Self {
        Self {
            schedule_id: schedule.schedule_id.clone(),
            phase: schedule.phase,
            revision: schedule.revision,
            trigger: schedule.trigger.clone(),
            target: schedule.target.clone(),
            misfire_policy: schedule.misfire_policy.clone(),
            overlap_policy: schedule.overlap_policy.clone(),
            missing_target_policy: schedule.missing_target_policy.clone(),
            next_occurrence_ordinal: schedule.next_occurrence_ordinal,
            planning_cursor_utc: schedule.planning_cursor_utc,
            superseded_ack_ids: schedule.superseded_ack_ids.clone(),
            config: schedule.config.clone(),
        }
    }
}

impl From<meerkat_schedule::Schedule> for Schedule {
    fn from(schedule: meerkat_schedule::Schedule) -> Self {
        Self::from(&schedule)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
pub struct ScheduleListResult {
    pub schedules: Vec<Schedule>,
}

impl ScheduleListResult {
    pub fn from_domain(schedules: Vec<meerkat_schedule::Schedule>) -> Self {
        Self {
            schedules: schedules.into_iter().map(Schedule::from).collect(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
pub struct Occurrence {
    pub occurrence_id: meerkat_schedule::OccurrenceId,
    pub schedule_id: meerkat_schedule::ScheduleId,
    pub schedule_revision: meerkat_schedule::ScheduleRevision,
    pub occurrence_ordinal: meerkat_schedule::OccurrenceOrdinal,
    pub phase: meerkat_schedule::OccurrencePhase,
    pub due_at_utc: DateTime<Utc>,
    pub trigger_snapshot: meerkat_schedule::TriggerSpec,
    pub target_snapshot: meerkat_schedule::TargetBinding,
    pub misfire_policy: meerkat_schedule::MisfirePolicy,
    pub overlap_policy: meerkat_schedule::OverlapPolicy,
    pub missing_target_policy: meerkat_schedule::MissingTargetPolicy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claimed_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lease_expires_at_utc: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery_correlation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_receipt: Option<meerkat_schedule::DeliveryReceipt>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_class: Option<meerkat_schedule::OccurrenceFailureClass>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_outcome: Option<meerkat_schedule::RuntimeDeliveryOutcome>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_detail: Option<String>,
    pub attempt_count: u32,
    pub created_at_utc: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claimed_at_utc: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dispatched_at_utc: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at_utc: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub superseded_by_revision: Option<meerkat_schedule::ScheduleRevision>,
}

impl From<&meerkat_schedule::Occurrence> for Occurrence {
    fn from(occurrence: &meerkat_schedule::Occurrence) -> Self {
        Self {
            occurrence_id: occurrence.occurrence_id.clone(),
            schedule_id: occurrence.schedule_id.clone(),
            schedule_revision: occurrence.schedule_revision,
            occurrence_ordinal: occurrence.occurrence_ordinal,
            phase: occurrence.phase,
            due_at_utc: occurrence.due_at_utc,
            trigger_snapshot: occurrence.trigger_snapshot.clone(),
            target_snapshot: occurrence.target_snapshot.clone(),
            misfire_policy: occurrence.misfire_policy.clone(),
            overlap_policy: occurrence.overlap_policy.clone(),
            missing_target_policy: occurrence.missing_target_policy.clone(),
            claimed_by: occurrence.claimed_by.clone(),
            lease_expires_at_utc: occurrence.lease_expires_at_utc,
            delivery_correlation_id: occurrence.delivery_correlation_id.clone(),
            last_receipt: occurrence.last_receipt.clone(),
            failure_class: occurrence.failure_class,
            runtime_outcome: occurrence.runtime_outcome.clone(),
            failure_detail: occurrence.failure_detail.clone(),
            attempt_count: occurrence.attempt_count,
            created_at_utc: occurrence.created_at_utc,
            claimed_at_utc: occurrence.claimed_at_utc,
            dispatched_at_utc: occurrence.dispatched_at_utc,
            completed_at_utc: occurrence.completed_at_utc,
            superseded_by_revision: occurrence.superseded_by_revision,
        }
    }
}

impl From<meerkat_schedule::Occurrence> for Occurrence {
    fn from(occurrence: meerkat_schedule::Occurrence) -> Self {
        Self::from(&occurrence)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
pub struct ScheduleOccurrencesResult {
    pub occurrences: Vec<Occurrence>,
}

impl ScheduleOccurrencesResult {
    pub fn from_domain(occurrences: Vec<meerkat_schedule::Occurrence>) -> Self {
        Self {
            occurrences: occurrences.into_iter().map(Occurrence::from).collect(),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn domain_schedule_projects_to_flat_public_wire_without_machine_state() {
        let domain = meerkat_schedule::Schedule::new(meerkat_schedule::CreateScheduleRequest {
            name: Some("nightly report".to_string()),
            description: None,
            trigger: meerkat_schedule::TriggerSpec::Once {
                due_at_utc: Utc::now(),
            },
            target: meerkat_schedule::TargetBinding::host_runnable(
                meerkat_schedule::HostRunnableTargetBinding {
                    runnable: meerkat_schedule::HostRunnableName::parse("nightly-report").unwrap(),
                    params: None,
                },
            ),
            misfire_policy: meerkat_schedule::MisfirePolicy::default(),
            overlap_policy: meerkat_schedule::OverlapPolicy::default(),
            missing_target_policy: meerkat_schedule::MissingTargetPolicy::default(),
            labels: BTreeMap::from([("team".to_string(), "platform".to_string())]),
            planning_horizon_days: Some(7),
            planning_horizon_occurrences: Some(12),
        })
        .unwrap();

        let public = serde_json::to_value(Schedule::from(domain)).unwrap();
        assert_eq!(public["name"], "nightly report");
        assert_eq!(public["planning_horizon_days"], 7);
        assert_eq!(public["planning_horizon_occurrences"], 12);
        assert_eq!(public["labels"]["team"], "platform");
        assert!(public.get("config").is_none());
        assert!(public.get("machine_state").is_none());
    }

    #[test]
    fn domain_occurrence_projects_exact_public_keys_without_lease_authority() {
        let schedule = meerkat_schedule::Schedule::new(meerkat_schedule::CreateScheduleRequest {
            name: Some("one shot".to_string()),
            description: None,
            trigger: meerkat_schedule::TriggerSpec::Once {
                due_at_utc: Utc::now(),
            },
            target: meerkat_schedule::TargetBinding::host_runnable(
                meerkat_schedule::HostRunnableTargetBinding {
                    runnable: meerkat_schedule::HostRunnableName::parse("one-shot").unwrap(),
                    params: None,
                },
            ),
            misfire_policy: meerkat_schedule::MisfirePolicy::default(),
            overlap_policy: meerkat_schedule::OverlapPolicy::default(),
            missing_target_policy: meerkat_schedule::MissingTargetPolicy::default(),
            labels: BTreeMap::new(),
            planning_horizon_days: Some(1),
            planning_horizon_occurrences: Some(1),
        })
        .unwrap();
        let domain = meerkat_schedule::Occurrence::planned_from_schedule(
            &schedule,
            meerkat_schedule::OccurrenceOrdinal(0),
            Utc::now(),
        )
        .unwrap();
        let value = serde_json::to_value(Occurrence::from(domain)).unwrap();
        let keys = value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        assert_eq!(
            keys,
            BTreeSet::from([
                "attempt_count",
                "created_at_utc",
                "due_at_utc",
                "misfire_policy",
                "missing_target_policy",
                "occurrence_id",
                "occurrence_ordinal",
                "overlap_policy",
                "phase",
                "schedule_id",
                "schedule_revision",
                "target_snapshot",
                "trigger_snapshot",
            ])
        );
        assert!(value.get("machine_state").is_none());
        assert!(value.get("claim_token").is_none());
    }
}
