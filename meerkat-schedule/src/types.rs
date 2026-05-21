use crate::machines::{occurrence_lifecycle as occ_dsl, schedule_lifecycle as sched_dsl};
use chrono::{DateTime, Duration, Utc};
use meerkat_core::ops::ToolAccessPolicy;
use meerkat_core::skills::{SkillKey, SkillRef};
use meerkat_core::types::RenderMetadata;
use meerkat_core::{
    ContentInput, OutputSchema, PeerMeta, Provider, SessionId, ToolVisibilityWitness,
};
use serde::{Deserialize, Deserializer, Serialize, de::Error as DeError};
use serde_json::value::RawValue;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::str::FromStr;
use uuid::Uuid;

const DEFAULT_SKIP_MISFIRE_GRACE_SECONDS: i64 = 30;

fn semantic_json_key<T: Serialize>(prefix: &str, value: &T) -> String {
    match serde_json::to_string(value) {
        Ok(json) => format!("{prefix}:{json}"),
        Err(error) => format!("{prefix}:serialization-error:{error}"),
    }
}

fn trigger_stable_key(trigger: &TriggerSpec) -> String {
    semantic_json_key("trigger", trigger)
}

fn policy_key<T: Serialize>(prefix: &str, value: &T) -> String {
    semantic_json_key(prefix, value)
}

pub(crate) fn target_materialized_session_id(target: &TargetBinding) -> Option<SessionId> {
    let TargetBinding::Session(binding) = target else {
        return None;
    };
    match binding.as_ref() {
        SessionTargetBinding::MaterializeOnDemandSession {
            bound_session_id, ..
        } => bound_session_id.clone(),
        SessionTargetBinding::ExactSession { .. }
        | SessionTargetBinding::ResumableSession { .. } => None,
    }
}

#[derive(Serialize)]
struct DeliveryReceiptAuthorityKey<'a> {
    schema: &'static str,
    occurrence_id: &'a str,
    attempt: u32,
    stage: &'static str,
    recorded_at_utc_ms: u64,
    correlation_id: Option<&'a str>,
    detail: Option<&'a str>,
    failure_class: Option<&'static str>,
    runtime_outcome_key: Option<&'a str>,
    materialized_session_id: Option<String>,
}

pub(crate) fn delivery_receipt_id_from_authority(
    occurrence_id: &OccurrenceId,
    attempt: u32,
    stage: DeliveryReceiptStage,
    recorded_at_utc_ms: u64,
    correlation_id: Option<&str>,
    detail: Option<&str>,
    failure_class: Option<OccurrenceFailureClass>,
    runtime_outcome_key: Option<&str>,
    materialized_session_id: Option<&SessionId>,
) -> Uuid {
    let materialized_session_id = materialized_session_id.map(|id| id.0.to_string());
    let occurrence_id = occurrence_id.to_string();
    let key = DeliveryReceiptAuthorityKey {
        schema: "meerkat.schedule.delivery_receipt.v1",
        occurrence_id: &occurrence_id,
        attempt,
        stage: delivery_receipt_stage_to_wire(stage),
        recorded_at_utc_ms,
        correlation_id,
        detail,
        failure_class: failure_class
            .map(failure_class_to_machine)
            .map(occurrence_failure_class_to_wire),
        runtime_outcome_key,
        materialized_session_id,
    };
    let key = serde_json::to_vec(&key).unwrap_or_default();
    Uuid::new_v5(&Uuid::NAMESPACE_URL, &key)
}

fn datetime_to_machine_millis(dt: DateTime<Utc>, field: &'static str) -> Result<u64, String> {
    let millis = dt.timestamp_millis();
    u64::try_from(millis).map_err(|_| {
        format!("{field} timestamp cannot be represented as unsigned millis: {millis}")
    })
}

fn optional_datetime_to_machine_millis(
    dt: Option<DateTime<Utc>>,
    field: &'static str,
) -> Result<Option<u64>, String> {
    dt.map(|value| datetime_to_machine_millis(value, field))
        .transpose()
}

fn schedule_phase_to_machine(phase: SchedulePhase) -> sched_dsl::ScheduleLifecycleState {
    match phase {
        SchedulePhase::Active => sched_dsl::ScheduleLifecycleState::Active,
        SchedulePhase::Paused => sched_dsl::ScheduleLifecycleState::Paused,
        SchedulePhase::Deleted => sched_dsl::ScheduleLifecycleState::Deleted,
    }
}

fn occurrence_phase_to_machine(phase: OccurrencePhase) -> occ_dsl::OccurrenceLifecycleState {
    match phase {
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
    }
}

fn schedule_misfire_policy_to_machine(policy: &MisfirePolicy) -> sched_dsl::MisfirePolicy {
    match policy {
        MisfirePolicy::Skip => sched_dsl::MisfirePolicy::Skip,
        MisfirePolicy::CatchUpWithin { .. } => sched_dsl::MisfirePolicy::CatchUpWithin,
    }
}

fn schedule_lifecycle_state_to_wire(phase: sched_dsl::ScheduleLifecycleState) -> &'static str {
    match phase {
        sched_dsl::ScheduleLifecycleState::Active => "active",
        sched_dsl::ScheduleLifecycleState::Paused => "paused",
        sched_dsl::ScheduleLifecycleState::Deleted => "deleted",
    }
}

fn schedule_lifecycle_state_from_wire(
    phase: &str,
) -> Result<sched_dsl::ScheduleLifecycleState, String> {
    match phase {
        "active" | "Active" => Ok(sched_dsl::ScheduleLifecycleState::Active),
        "paused" | "Paused" => Ok(sched_dsl::ScheduleLifecycleState::Paused),
        "deleted" | "Deleted" => Ok(sched_dsl::ScheduleLifecycleState::Deleted),
        other => Err(format!("invalid ScheduleLifecycleState `{other}`")),
    }
}

fn schedule_misfire_policy_to_wire(policy: sched_dsl::MisfirePolicy) -> &'static str {
    match policy {
        sched_dsl::MisfirePolicy::Skip => "skip",
        sched_dsl::MisfirePolicy::CatchUpWithin => "catch_up_within",
    }
}

fn schedule_misfire_policy_from_wire(policy: &str) -> Result<sched_dsl::MisfirePolicy, String> {
    match policy {
        "skip" | "Skip" => Ok(sched_dsl::MisfirePolicy::Skip),
        "catch_up_within" | "CatchUpWithin" => Ok(sched_dsl::MisfirePolicy::CatchUpWithin),
        other => Err(format!("invalid schedule MisfirePolicy `{other}`")),
    }
}

fn schedule_overlap_policy_to_wire(policy: sched_dsl::OverlapPolicy) -> &'static str {
    match policy {
        sched_dsl::OverlapPolicy::AllowConcurrent => "allow_concurrent",
        sched_dsl::OverlapPolicy::SkipIfRunning => "skip_if_running",
    }
}

fn schedule_overlap_policy_from_wire(policy: &str) -> Result<sched_dsl::OverlapPolicy, String> {
    match policy {
        "allow_concurrent" | "AllowConcurrent" => Ok(sched_dsl::OverlapPolicy::AllowConcurrent),
        "skip_if_running" | "SkipIfRunning" => Ok(sched_dsl::OverlapPolicy::SkipIfRunning),
        other => Err(format!("invalid schedule OverlapPolicy `{other}`")),
    }
}

fn schedule_missing_target_policy_to_wire(policy: sched_dsl::MissingTargetPolicy) -> &'static str {
    match policy {
        sched_dsl::MissingTargetPolicy::Skip => "skip",
        sched_dsl::MissingTargetPolicy::MarkMisfired => "mark_misfired",
    }
}

fn schedule_missing_target_policy_from_wire(
    policy: &str,
) -> Result<sched_dsl::MissingTargetPolicy, String> {
    match policy {
        "skip" | "Skip" => Ok(sched_dsl::MissingTargetPolicy::Skip),
        "mark_misfired" | "MarkMisfired" => Ok(sched_dsl::MissingTargetPolicy::MarkMisfired),
        other => Err(format!("invalid schedule MissingTargetPolicy `{other}`")),
    }
}

fn occurrence_misfire_policy_to_machine(policy: &MisfirePolicy) -> occ_dsl::MisfirePolicy {
    match policy {
        MisfirePolicy::Skip => occ_dsl::MisfirePolicy::Skip,
        MisfirePolicy::CatchUpWithin { .. } => occ_dsl::MisfirePolicy::CatchUpWithin,
    }
}

fn occurrence_lifecycle_state_to_wire(phase: occ_dsl::OccurrenceLifecycleState) -> &'static str {
    match phase {
        occ_dsl::OccurrenceLifecycleState::Pending => "pending",
        occ_dsl::OccurrenceLifecycleState::Claimed => "claimed",
        occ_dsl::OccurrenceLifecycleState::Dispatching => "dispatching",
        occ_dsl::OccurrenceLifecycleState::AwaitingCompletion => "awaiting_completion",
        occ_dsl::OccurrenceLifecycleState::Completed => "completed",
        occ_dsl::OccurrenceLifecycleState::Skipped => "skipped",
        occ_dsl::OccurrenceLifecycleState::Misfired => "misfired",
        occ_dsl::OccurrenceLifecycleState::Superseded => "superseded",
        occ_dsl::OccurrenceLifecycleState::DeliveryFailed => "delivery_failed",
    }
}

fn occurrence_lifecycle_state_from_wire(
    phase: &str,
) -> Result<occ_dsl::OccurrenceLifecycleState, String> {
    match phase {
        "pending" | "Pending" => Ok(occ_dsl::OccurrenceLifecycleState::Pending),
        "claimed" | "Claimed" => Ok(occ_dsl::OccurrenceLifecycleState::Claimed),
        "dispatching" | "Dispatching" => Ok(occ_dsl::OccurrenceLifecycleState::Dispatching),
        "awaiting_completion" | "AwaitingCompletion" => {
            Ok(occ_dsl::OccurrenceLifecycleState::AwaitingCompletion)
        }
        "completed" | "Completed" => Ok(occ_dsl::OccurrenceLifecycleState::Completed),
        "skipped" | "Skipped" => Ok(occ_dsl::OccurrenceLifecycleState::Skipped),
        "misfired" | "Misfired" => Ok(occ_dsl::OccurrenceLifecycleState::Misfired),
        "superseded" | "Superseded" => Ok(occ_dsl::OccurrenceLifecycleState::Superseded),
        "delivery_failed" | "DeliveryFailed" => {
            Ok(occ_dsl::OccurrenceLifecycleState::DeliveryFailed)
        }
        other => Err(format!("invalid OccurrenceLifecycleState `{other}`")),
    }
}

fn occurrence_misfire_policy_to_wire(policy: occ_dsl::MisfirePolicy) -> &'static str {
    match policy {
        occ_dsl::MisfirePolicy::Skip => "skip",
        occ_dsl::MisfirePolicy::CatchUpWithin => "catch_up_within",
    }
}

fn occurrence_misfire_policy_from_wire(policy: &str) -> Result<occ_dsl::MisfirePolicy, String> {
    match policy {
        "skip" | "Skip" => Ok(occ_dsl::MisfirePolicy::Skip),
        "catch_up_within" | "CatchUpWithin" => Ok(occ_dsl::MisfirePolicy::CatchUpWithin),
        other => Err(format!("invalid occurrence MisfirePolicy `{other}`")),
    }
}

fn occurrence_overlap_policy_to_wire(policy: occ_dsl::OverlapPolicy) -> &'static str {
    match policy {
        occ_dsl::OverlapPolicy::AllowConcurrent => "allow_concurrent",
        occ_dsl::OverlapPolicy::SkipIfRunning => "skip_if_running",
    }
}

fn occurrence_overlap_policy_from_wire(policy: &str) -> Result<occ_dsl::OverlapPolicy, String> {
    match policy {
        "allow_concurrent" | "AllowConcurrent" => Ok(occ_dsl::OverlapPolicy::AllowConcurrent),
        "skip_if_running" | "SkipIfRunning" => Ok(occ_dsl::OverlapPolicy::SkipIfRunning),
        other => Err(format!("invalid occurrence OverlapPolicy `{other}`")),
    }
}

fn occurrence_missing_target_policy_to_wire(policy: occ_dsl::MissingTargetPolicy) -> &'static str {
    match policy {
        occ_dsl::MissingTargetPolicy::Skip => "skip",
        occ_dsl::MissingTargetPolicy::MarkMisfired => "mark_misfired",
    }
}

fn occurrence_missing_target_policy_from_wire(
    policy: &str,
) -> Result<occ_dsl::MissingTargetPolicy, String> {
    match policy {
        "skip" | "Skip" => Ok(occ_dsl::MissingTargetPolicy::Skip),
        "mark_misfired" | "MarkMisfired" => Ok(occ_dsl::MissingTargetPolicy::MarkMisfired),
        other => Err(format!("invalid occurrence MissingTargetPolicy `{other}`")),
    }
}

fn occurrence_failure_class_to_wire(failure_class: occ_dsl::FailureClass) -> &'static str {
    match failure_class {
        occ_dsl::FailureClass::TargetMaterializationFailed => "target_materialization_failed",
        occ_dsl::FailureClass::TargetMissing => "target_missing",
        occ_dsl::FailureClass::TargetBusy => "target_busy",
        occ_dsl::FailureClass::RuntimeRejected => "runtime_rejected",
        occ_dsl::FailureClass::MobRejected => "mob_rejected",
        occ_dsl::FailureClass::LeaseLost => "lease_lost",
        occ_dsl::FailureClass::TransportError => "transport_error",
        occ_dsl::FailureClass::InternalError => "internal_error",
    }
}

fn occurrence_failure_class_from_wire(
    failure_class: &str,
) -> Result<occ_dsl::FailureClass, String> {
    match failure_class {
        "target_materialization_failed" | "TargetMaterializationFailed" => {
            Ok(occ_dsl::FailureClass::TargetMaterializationFailed)
        }
        "target_missing" | "TargetMissing" => Ok(occ_dsl::FailureClass::TargetMissing),
        "target_busy" | "TargetBusy" => Ok(occ_dsl::FailureClass::TargetBusy),
        "runtime_rejected" | "RuntimeRejected" => Ok(occ_dsl::FailureClass::RuntimeRejected),
        "mob_rejected" | "MobRejected" => Ok(occ_dsl::FailureClass::MobRejected),
        "lease_lost" | "LeaseLost" => Ok(occ_dsl::FailureClass::LeaseLost),
        "transport_error" | "TransportError" => Ok(occ_dsl::FailureClass::TransportError),
        "internal_error" | "InternalError" => Ok(occ_dsl::FailureClass::InternalError),
        other => Err(format!("invalid OccurrenceFailureClass `{other}`")),
    }
}

fn occurrence_receipt_stage_to_wire(stage: occ_dsl::DeliveryReceiptStage) -> &'static str {
    match stage {
        occ_dsl::DeliveryReceiptStage::Planned => "planned",
        occ_dsl::DeliveryReceiptStage::Claimed => "claimed",
        occ_dsl::DeliveryReceiptStage::DispatchStarted => "dispatch_started",
        occ_dsl::DeliveryReceiptStage::DispatchAccepted => "dispatch_accepted",
        occ_dsl::DeliveryReceiptStage::AwaitingCompletion => "awaiting_completion",
        occ_dsl::DeliveryReceiptStage::Completed => "completed",
        occ_dsl::DeliveryReceiptStage::Skipped => "skipped",
        occ_dsl::DeliveryReceiptStage::Misfired => "misfired",
        occ_dsl::DeliveryReceiptStage::Superseded => "superseded",
        occ_dsl::DeliveryReceiptStage::DeliveryFailed => "delivery_failed",
        occ_dsl::DeliveryReceiptStage::LeaseExpired => "lease_expired",
    }
}

fn delivery_receipt_stage_to_wire(stage: DeliveryReceiptStage) -> &'static str {
    match stage {
        DeliveryReceiptStage::Planned => "planned",
        DeliveryReceiptStage::Claimed => "claimed",
        DeliveryReceiptStage::DispatchStarted => "dispatch_started",
        DeliveryReceiptStage::DispatchAccepted => "dispatch_accepted",
        DeliveryReceiptStage::AwaitingCompletion => "awaiting_completion",
        DeliveryReceiptStage::Completed => "completed",
        DeliveryReceiptStage::Skipped => "skipped",
        DeliveryReceiptStage::Misfired => "misfired",
        DeliveryReceiptStage::Superseded => "superseded",
        DeliveryReceiptStage::DeliveryFailed => "delivery_failed",
        DeliveryReceiptStage::LeaseExpired => "lease_expired",
    }
}

fn occurrence_receipt_stage_from_wire(
    stage: &str,
) -> Result<occ_dsl::DeliveryReceiptStage, String> {
    match stage {
        "planned" | "Planned" => Ok(occ_dsl::DeliveryReceiptStage::Planned),
        "claimed" | "Claimed" => Ok(occ_dsl::DeliveryReceiptStage::Claimed),
        "dispatch_started" | "DispatchStarted" => {
            Ok(occ_dsl::DeliveryReceiptStage::DispatchStarted)
        }
        "dispatch_accepted" | "DispatchAccepted" => {
            Ok(occ_dsl::DeliveryReceiptStage::DispatchAccepted)
        }
        "awaiting_completion" | "AwaitingCompletion" => {
            Ok(occ_dsl::DeliveryReceiptStage::AwaitingCompletion)
        }
        "completed" | "Completed" => Ok(occ_dsl::DeliveryReceiptStage::Completed),
        "skipped" | "Skipped" => Ok(occ_dsl::DeliveryReceiptStage::Skipped),
        "misfired" | "Misfired" => Ok(occ_dsl::DeliveryReceiptStage::Misfired),
        "superseded" | "Superseded" => Ok(occ_dsl::DeliveryReceiptStage::Superseded),
        "delivery_failed" | "DeliveryFailed" => Ok(occ_dsl::DeliveryReceiptStage::DeliveryFailed),
        "lease_expired" | "LeaseExpired" => Ok(occ_dsl::DeliveryReceiptStage::LeaseExpired),
        other => Err(format!("invalid DeliveryReceiptStage `{other}`")),
    }
}

fn schedule_overlap_policy_to_machine(policy: &OverlapPolicy) -> sched_dsl::OverlapPolicy {
    match policy {
        OverlapPolicy::AllowConcurrent => sched_dsl::OverlapPolicy::AllowConcurrent,
        OverlapPolicy::SkipIfRunning => sched_dsl::OverlapPolicy::SkipIfRunning,
    }
}

fn occurrence_overlap_policy_to_machine(policy: &OverlapPolicy) -> occ_dsl::OverlapPolicy {
    match policy {
        OverlapPolicy::AllowConcurrent => occ_dsl::OverlapPolicy::AllowConcurrent,
        OverlapPolicy::SkipIfRunning => occ_dsl::OverlapPolicy::SkipIfRunning,
    }
}

fn schedule_missing_target_policy_to_machine(
    policy: &MissingTargetPolicy,
) -> sched_dsl::MissingTargetPolicy {
    match policy {
        MissingTargetPolicy::Skip => sched_dsl::MissingTargetPolicy::Skip,
        MissingTargetPolicy::MarkMisfired => sched_dsl::MissingTargetPolicy::MarkMisfired,
    }
}

fn occurrence_missing_target_policy_to_machine(
    policy: &MissingTargetPolicy,
) -> occ_dsl::MissingTargetPolicy {
    match policy {
        MissingTargetPolicy::Skip => occ_dsl::MissingTargetPolicy::Skip,
        MissingTargetPolicy::MarkMisfired => occ_dsl::MissingTargetPolicy::MarkMisfired,
    }
}

fn failure_class_to_machine(failure_class: OccurrenceFailureClass) -> occ_dsl::FailureClass {
    match failure_class {
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

fn failure_class_from_machine(failure_class: occ_dsl::FailureClass) -> OccurrenceFailureClass {
    match failure_class {
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

fn receipt_stage_from_machine(stage: occ_dsl::DeliveryReceiptStage) -> DeliveryReceiptStage {
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

fn schedule_machine_schema_identity() -> (String, u32) {
    let schema = sched_dsl::ScheduleLifecycleMachineState::schema();
    (schema.machine.to_string(), schema.version)
}

fn occurrence_machine_schema_identity() -> (String, u32) {
    let schema = occ_dsl::OccurrenceLifecycleMachineState::schema();
    (schema.machine.to_string(), schema.version)
}

fn validate_schedule_machine_wire_header(machine: &str, schema_version: u32) -> Result<(), String> {
    let (expected_machine, expected_version) = schedule_machine_schema_identity();
    if machine != expected_machine {
        return Err(format!(
            "schedule machine_state machine `{machine}` does not match generated schema `{expected_machine}`"
        ));
    }
    if schema_version != expected_version {
        return Err(format!(
            "schedule machine_state schema_version `{schema_version}` does not match generated schema version `{expected_version}`"
        ));
    }
    Ok(())
}

fn validate_occurrence_machine_wire_header(
    machine: &str,
    schema_version: u32,
) -> Result<(), String> {
    let (expected_machine, expected_version) = occurrence_machine_schema_identity();
    if machine != expected_machine {
        return Err(format!(
            "occurrence machine_state machine `{machine}` does not match generated schema `{expected_machine}`"
        ));
    }
    if schema_version != expected_version {
        return Err(format!(
            "occurrence machine_state schema_version `{schema_version}` does not match generated schema version `{expected_version}`"
        ));
    }
    Ok(())
}

fn validate_schedule_machine_recovery(
    machine: &sched_dsl::ScheduleLifecycleMachineState,
) -> Result<(), String> {
    sched_dsl::ScheduleLifecycleMachineAuthority::recover_from_state(machine.clone())
        .map(|_| ())
        .map_err(|source| {
            format!(
                "generated ScheduleLifecycleMachine rejected recovered machine_state: {source:?}"
            )
        })
}

fn validate_occurrence_machine_recovery(
    machine: &occ_dsl::OccurrenceLifecycleMachineState,
) -> Result<(), String> {
    occ_dsl::OccurrenceLifecycleMachineAuthority::recover_from_state(machine.clone())
        .map(|_| ())
        .map_err(|source| {
            format!(
                "generated OccurrenceLifecycleMachine rejected recovered machine_state: {source:?}"
            )
        })
}

fn session_id_from_machine(value: &occ_dsl::SessionId) -> Result<SessionId, String> {
    SessionId::parse(&value.0)
        .map_err(|source| format!("invalid machine SessionId `{}`: {source}", value.0))
}

fn optional_session_id_to_machine(value: Option<SessionId>) -> Option<occ_dsl::SessionId> {
    value.map(|id| occ_dsl::SessionId(id.0.to_string()))
}

fn last_receipt_from_machine_projection(
    machine: &occ_dsl::OccurrenceLifecycleMachineState,
    runtime_outcome: Option<RuntimeDeliveryOutcome>,
) -> Result<Option<DeliveryReceipt>, String> {
    let has_last_receipt = machine.last_receipt_stage.is_some()
        || machine.last_receipt_recorded_at_utc_ms.is_some()
        || machine.last_receipt_attempt.is_some()
        || machine.last_receipt_failure_class.is_some()
        || machine.last_receipt_detail.is_some()
        || machine.last_receipt_correlation_id.is_some()
        || machine.last_receipt_materialized_session_id.is_some()
        || machine.runtime_outcome_key.is_some();
    if !has_last_receipt {
        if runtime_outcome.is_some() {
            return Err(
                "runtime_outcome projection exists without machine receipt authority".into(),
            );
        }
        return Ok(None);
    }

    let stage = machine
        .last_receipt_stage
        .ok_or_else(|| "machine receipt authority missing last receipt stage".to_string())?;
    let recorded_at_utc_ms = machine.last_receipt_recorded_at_utc_ms.ok_or_else(|| {
        "machine receipt authority missing last receipt recorded timestamp".to_string()
    })?;
    let recorded_at_utc_ms_i64 = i64::try_from(recorded_at_utc_ms).map_err(|_| {
        format!(
            "machine receipt recorded timestamp `{recorded_at_utc_ms}` cannot be represented as i64"
        )
    })?;
    let recorded_at_utc =
        DateTime::from_timestamp_millis(recorded_at_utc_ms_i64).ok_or_else(|| {
            format!("machine receipt recorded timestamp `{recorded_at_utc_ms}` is invalid")
        })?;
    let attempt = machine
        .last_receipt_attempt
        .ok_or_else(|| "machine receipt authority missing last receipt attempt".to_string())
        .and_then(|attempt| {
            u32::try_from(attempt).map_err(|_| {
                format!("machine receipt attempt `{attempt}` cannot be represented as u32")
            })
        })?;
    let occurrence_id = OccurrenceId::parse(&machine.occurrence_id.0).map_err(|source| {
        format!(
            "machine receipt authority emitted invalid occurrence id `{}`: {source}",
            machine.occurrence_id.0
        )
    })?;
    let stage = receipt_stage_from_machine(stage);
    let failure_class = machine
        .last_receipt_failure_class
        .map(failure_class_from_machine);
    let materialized_session_id = machine
        .last_receipt_materialized_session_id
        .as_ref()
        .map(session_id_from_machine)
        .transpose()?;
    let runtime_outcome_key = runtime_outcome
        .as_ref()
        .map(|outcome| semantic_json_key("runtime_outcome", outcome));
    if runtime_outcome_key != machine.runtime_outcome_key {
        return Err(format!(
            "runtime_outcome projection key `{runtime_outcome_key:?}` does not match machine receipt key `{:?}`",
            machine.runtime_outcome_key
        ));
    }
    let receipt_id = delivery_receipt_id_from_authority(
        &occurrence_id,
        attempt,
        stage,
        recorded_at_utc_ms,
        machine.last_receipt_correlation_id.as_deref(),
        machine.last_receipt_detail.as_deref(),
        failure_class,
        machine.runtime_outcome_key.as_deref(),
        materialized_session_id.as_ref(),
    );
    Ok(Some(DeliveryReceipt {
        receipt_id,
        occurrence_id,
        attempt,
        stage,
        recorded_at_utc,
        correlation_id: machine.last_receipt_correlation_id.clone(),
        detail: machine.last_receipt_detail.clone(),
        failure_class,
        runtime_outcome,
        materialized_session_id,
    }))
}

pub(crate) fn validate_schedule_machine_projection(schedule: &Schedule) -> Result<(), String> {
    let machine = &schedule.machine_state;
    validate_schedule_machine_recovery(machine)?;
    if machine.schedule_id.0 != schedule.schedule_id.0.to_string() {
        return Err(format!(
            "schedule {} id projection does not match machine_state",
            schedule.schedule_id
        ));
    }
    if schedule_phase_to_machine(schedule.phase) != machine.lifecycle_phase {
        return Err(format!(
            "schedule {} phase projection does not match machine_state",
            schedule.schedule_id
        ));
    }
    if schedule.revision.0 != machine.revision {
        return Err(format!(
            "schedule {} revision projection does not match machine_state",
            schedule.schedule_id
        ));
    }
    if trigger_stable_key(&schedule.trigger) != machine.trigger_key {
        return Err(format!(
            "schedule {} trigger projection does not match machine_state",
            schedule.schedule_id
        ));
    }
    if schedule.target.stable_key() != machine.target_binding_key {
        return Err(format!(
            "schedule {} target projection does not match machine_state",
            schedule.schedule_id
        ));
    }
    if schedule_misfire_policy_to_machine(&schedule.misfire_policy) != machine.misfire_policy
        || policy_key("misfire_policy", &schedule.misfire_policy) != machine.misfire_policy_key
    {
        return Err(format!(
            "schedule {} misfire policy projection does not match machine_state",
            schedule.schedule_id
        ));
    }
    if schedule_overlap_policy_to_machine(&schedule.overlap_policy) != machine.overlap_policy
        || policy_key("overlap_policy", &schedule.overlap_policy) != machine.overlap_policy_key
    {
        return Err(format!(
            "schedule {} overlap policy projection does not match machine_state",
            schedule.schedule_id
        ));
    }
    if schedule_missing_target_policy_to_machine(&schedule.missing_target_policy)
        != machine.missing_target_policy
        || policy_key("missing_target_policy", &schedule.missing_target_policy)
            != machine.missing_target_policy_key
    {
        return Err(format!(
            "schedule {} missing-target policy projection does not match machine_state",
            schedule.schedule_id
        ));
    }
    if u64::from(schedule.config.planning_horizon_days) != machine.planning_horizon_days
        || u64::from(schedule.config.planning_horizon_occurrences)
            != machine.planning_horizon_occurrences
    {
        return Err(format!(
            "schedule {} planning horizon projection does not match machine_state",
            schedule.schedule_id
        ));
    }
    if optional_datetime_to_machine_millis(schedule.planning_cursor_utc, "planning_cursor_utc")?
        != machine.planning_cursor_utc_ms
    {
        return Err(format!(
            "schedule {} planning cursor projection does not match machine_state",
            schedule.schedule_id
        ));
    }
    if schedule.next_occurrence_ordinal.0 != machine.next_occurrence_ordinal {
        return Err(format!(
            "schedule {} next occurrence ordinal projection does not match machine_state",
            schedule.schedule_id
        ));
    }
    let superseded_ack_ids = schedule
        .superseded_ack_ids
        .iter()
        .map(|id| sched_dsl::OccurrenceId(id.0.to_string()))
        .collect::<BTreeSet<_>>();
    if superseded_ack_ids != machine.superseded_ack_ids {
        return Err(format!(
            "schedule {} superseded ack projection does not match machine_state",
            schedule.schedule_id
        ));
    }
    Ok(())
}

pub(crate) fn validate_occurrence_machine_projection(
    occurrence: &Occurrence,
) -> Result<(), String> {
    let machine = &occurrence.machine_state;
    validate_occurrence_machine_recovery(machine)?;
    if occurrence_phase_to_machine(occurrence.phase) != machine.lifecycle_phase {
        return Err(format!(
            "occurrence {} phase projection does not match machine_state",
            occurrence.occurrence_id
        ));
    }
    if machine.occurrence_id.0 != occurrence.occurrence_id.0.to_string()
        || machine.schedule_id.0 != occurrence.schedule_id.0.to_string()
    {
        return Err(format!(
            "occurrence {} identity projection does not match machine_state",
            occurrence.occurrence_id
        ));
    }
    if occurrence.schedule_revision.0 != machine.schedule_revision
        || occurrence.occurrence_ordinal.0 != machine.occurrence_ordinal
    {
        return Err(format!(
            "occurrence {} revision/ordinal projection does not match machine_state",
            occurrence.occurrence_id
        ));
    }
    if trigger_stable_key(&occurrence.trigger_snapshot) != machine.trigger_key
        || occurrence.target_snapshot.stable_key() != machine.target_binding_key
        || optional_session_id_to_machine(target_materialized_session_id(
            &occurrence.target_snapshot,
        )) != machine.target_materialized_session_id.clone()
    {
        return Err(format!(
            "occurrence {} target snapshot projection does not match machine_state",
            occurrence.occurrence_id
        ));
    }
    if occurrence_misfire_policy_to_machine(&occurrence.misfire_policy) != machine.misfire_policy
        || policy_key("misfire_policy", &occurrence.misfire_policy) != machine.misfire_policy_key
        || occurrence_overlap_policy_to_machine(&occurrence.overlap_policy)
            != machine.overlap_policy
        || policy_key("overlap_policy", &occurrence.overlap_policy) != machine.overlap_policy_key
        || occurrence_missing_target_policy_to_machine(&occurrence.missing_target_policy)
            != machine.missing_target_policy
        || policy_key("missing_target_policy", &occurrence.missing_target_policy)
            != machine.missing_target_policy_key
    {
        return Err(format!(
            "occurrence {} policy projection does not match machine_state",
            occurrence.occurrence_id
        ));
    }
    if datetime_to_machine_millis(occurrence.due_at_utc, "due_at_utc")? != machine.due_at_utc_ms {
        return Err(format!(
            "occurrence {} due time projection does not match machine_state",
            occurrence.occurrence_id
        ));
    }
    let misfire_deadline = occurrence
        .misfire_policy
        .misfire_deadline_utc(occurrence.due_at_utc)
        .map(|deadline| datetime_to_machine_millis(deadline, "misfire_deadline_utc"))
        .transpose()?
        .ok_or_else(|| {
            format!(
                "occurrence {} missing misfire deadline projection",
                occurrence.occurrence_id
            )
        })?;
    if misfire_deadline != machine.misfire_deadline_utc_ms {
        return Err(format!(
            "occurrence {} misfire deadline projection does not match machine_state",
            occurrence.occurrence_id
        ));
    }
    if occurrence.claimed_by != machine.claimed_by
        || optional_datetime_to_machine_millis(
            occurrence.lease_expires_at_utc,
            "lease_expires_at_utc",
        )? != machine.lease_expires_at_utc_ms
        || optional_datetime_to_machine_millis(occurrence.claimed_at_utc, "claimed_at_utc")?
            != machine.claimed_at_utc_ms
        || occurrence
            .claim_token
            .map(|token| occ_dsl::ClaimToken(token.to_string()))
            != machine.claim_token.clone()
    {
        return Err(format!(
            "occurrence {} claim projection does not match machine_state",
            occurrence.occurrence_id
        ));
    }
    if occurrence.delivery_correlation_id != machine.delivery_correlation_id
        || occurrence.last_receipt
            != last_receipt_from_machine_projection(machine, occurrence.runtime_outcome.clone())?
    {
        return Err(format!(
            "occurrence {} delivery projection does not match machine_state",
            occurrence.occurrence_id
        ));
    }
    if occurrence.failure_class.map(failure_class_to_machine) != machine.failure_class
        || occurrence.failure_detail != machine.failure_detail
    {
        return Err(format!(
            "occurrence {} failure projection does not match machine_state",
            occurrence.occurrence_id
        ));
    }
    if optional_datetime_to_machine_millis(occurrence.dispatched_at_utc, "dispatched_at_utc")?
        != machine.dispatched_at_utc_ms
        || optional_datetime_to_machine_millis(occurrence.completed_at_utc, "completed_at_utc")?
            != machine.completed_at_utc_ms
    {
        return Err(format!(
            "occurrence {} terminal timestamp projection does not match machine_state",
            occurrence.occurrence_id
        ));
    }
    if u64::from(occurrence.attempt_count) != machine.attempt_count
        || occurrence.superseded_by_revision.map(|revision| revision.0)
            != machine.superseded_by_revision
    {
        return Err(format!(
            "occurrence {} attempt/supersession projection does not match machine_state",
            occurrence.occurrence_id
        ));
    }
    Ok(())
}

macro_rules! uuid_newtype {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        pub struct $name(
            #[cfg_attr(feature = "schema", schemars(with = "String"))] pub Uuid
        );

        impl $name {
            pub fn new() -> Self {
                Self(Uuid::now_v7())
            }

            pub fn from_uuid(uuid: Uuid) -> Self {
                Self(uuid)
            }

            pub fn parse(value: &str) -> Result<Self, uuid::Error> {
                Ok(Self(Uuid::parse_str(value)?))
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }

        impl FromStr for $name {
            type Err = uuid::Error;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Self::parse(s)
            }
        }
    };
}

uuid_newtype!(
    /// Opaque identifier for a persisted schedule.
    ScheduleId
);

uuid_newtype!(
    /// Opaque identifier for a persisted occurrence.
    OccurrenceId
);

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ScheduleRevision(pub u64);

impl ScheduleRevision {
    pub fn initial() -> Self {
        Self(1)
    }

    pub fn next(self) -> Self {
        Self(self.0.saturating_add(1))
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct OccurrenceOrdinal(pub u64);

impl OccurrenceOrdinal {
    pub fn next(self) -> Self {
        Self(self.0.saturating_add(1))
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SchedulePhase {
    Active,
    Paused,
    Deleted,
}

impl SchedulePhase {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Deleted)
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OccurrencePhase {
    Pending,
    Claimed,
    Dispatching,
    AwaitingCompletion,
    Completed,
    Skipped,
    Misfired,
    Superseded,
    DeliveryFailed,
}

impl OccurrencePhase {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed
                | Self::Skipped
                | Self::Misfired
                | Self::Superseded
                | Self::DeliveryFailed
        )
    }

    pub fn holds_active_lease(self) -> bool {
        matches!(
            self,
            Self::Claimed | Self::Dispatching | Self::AwaitingCompletion
        )
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TriggerSpec {
    Once { due_at_utc: DateTime<Utc> },
    Interval(IntervalTriggerSpec),
    Calendar(CalendarTriggerSpec),
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntervalTriggerSpec {
    pub start_at_utc: DateTime<Utc>,
    pub every_seconds: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_at_utc: Option<DateTime<Utc>>,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "values", rename_all = "snake_case")]
pub enum CalendarFieldSpec {
    #[default]
    Any,
    Values(Vec<u32>),
}

impl CalendarFieldSpec {
    pub fn contains(&self, value: u32) -> bool {
        match self {
            Self::Any => true,
            Self::Values(values) => values.contains(&value),
        }
    }

    pub fn normalized(mut values: Vec<u32>) -> Self {
        values.sort_unstable();
        values.dedup();
        Self::Values(values)
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CalendarTriggerSpec {
    pub timezone: String,
    #[serde(default)]
    pub minute: CalendarFieldSpec,
    #[serde(default)]
    pub hour: CalendarFieldSpec,
    #[serde(default)]
    pub day_of_month: CalendarFieldSpec,
    #[serde(default)]
    pub month: CalendarFieldSpec,
    #[serde(default)]
    pub day_of_week: CalendarFieldSpec,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub year: Option<CalendarFieldSpec>,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MisfirePolicy {
    /// Skip materially late pending occurrences instead of catching them up.
    ///
    /// The scheduler applies a small internal grace window so normal tick
    /// jitter does not immediately misfire on-time work.
    #[default]
    Skip,
    /// Catch up overdue pending occurrences only while they remain within the
    /// configured lateness window.
    CatchUpWithin { window_seconds: u64 },
}

impl MisfirePolicy {
    pub(crate) fn catch_up_window(&self) -> Option<Duration> {
        match self {
            Self::Skip => Some(Duration::seconds(DEFAULT_SKIP_MISFIRE_GRACE_SECONDS)),
            Self::CatchUpWithin { window_seconds } => {
                let seconds = i64::try_from(*window_seconds).unwrap_or(i64::MAX);
                Some(Duration::seconds(seconds))
            }
        }
    }

    pub(crate) fn misfire_deadline_utc(&self, due_at_utc: DateTime<Utc>) -> Option<DateTime<Utc>> {
        self.catch_up_window()
            .and_then(|window| due_at_utc.checked_add_signed(window))
    }

    pub(crate) fn misfire_detail(
        &self,
        due_at_utc: DateTime<Utc>,
        now_utc: DateTime<Utc>,
    ) -> String {
        let lateness_seconds = now_utc
            .signed_duration_since(due_at_utc)
            .num_seconds()
            .max(0);
        match self {
            Self::Skip => format!(
                "missed scheduled time by {lateness_seconds}s; skip policy does not catch up materially late pending occurrences"
            ),
            Self::CatchUpWithin { window_seconds } => format!(
                "missed scheduled time by {lateness_seconds}s, exceeding catch-up window of {window_seconds}s"
            ),
        }
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OverlapPolicy {
    AllowConcurrent,
    #[default]
    SkipIfRunning,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MissingTargetPolicy {
    Skip,
    #[default]
    MarkMisfired,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "target_kind", rename_all = "snake_case")]
pub enum TargetBinding {
    Session(Box<SessionTargetBinding>),
    Mob(Box<MobTargetBinding>),
}

impl TargetBinding {
    pub fn session(binding: SessionTargetBinding) -> Self {
        Self::Session(Box::new(binding))
    }

    pub fn stable_key(&self) -> String {
        semantic_json_key("target", self)
    }

    pub fn bind_materialized_session(&mut self, session_id: &SessionId) -> bool {
        match self {
            Self::Session(binding) => binding.bind_materialized_session(session_id),
            Self::Mob(_) => false,
        }
    }

    pub fn validate_public_api(&self) -> Result<(), String> {
        match self {
            Self::Session(_) => Ok(()),
            Self::Mob(binding) => binding.validate_public_api(),
        }
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionTargetBinding {
    ExactSession {
        session_id: SessionId,
        action: ScheduledSessionAction,
    },
    ResumableSession {
        session_id: SessionId,
        action: ScheduledSessionAction,
    },
    MaterializeOnDemandSession {
        create: Box<SessionMaterializationSpec>,
        action: ScheduledSessionAction,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bound_session_id: Option<SessionId>,
    },
}

impl SessionTargetBinding {
    pub fn materialize_on_demand(
        create: SessionMaterializationSpec,
        action: ScheduledSessionAction,
    ) -> Self {
        Self::MaterializeOnDemandSession {
            create: Box::new(create),
            action,
            bound_session_id: None,
        }
    }

    pub fn stable_key(&self) -> String {
        semantic_json_key("session_target", self)
    }

    pub fn action(&self) -> &ScheduledSessionAction {
        match self {
            Self::ExactSession { action, .. }
            | Self::ResumableSession { action, .. }
            | Self::MaterializeOnDemandSession { action, .. } => action,
        }
    }

    pub fn resolved_session_id(&self) -> Option<&SessionId> {
        match self {
            Self::ExactSession { session_id, .. } | Self::ResumableSession { session_id, .. } => {
                Some(session_id)
            }
            Self::MaterializeOnDemandSession {
                bound_session_id, ..
            } => bound_session_id.as_ref(),
        }
    }

    pub fn bind_materialized_session(&mut self, session_id: &SessionId) -> bool {
        match self {
            Self::MaterializeOnDemandSession {
                bound_session_id, ..
            } => match bound_session_id {
                Some(existing) if existing != session_id => false,
                Some(_) => false,
                None => {
                    *bound_session_id = Some(session_id.clone());
                    true
                }
            },
            Self::ExactSession { .. } | Self::ResumableSession { .. } => false,
        }
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ScheduledSessionAction {
    Prompt {
        prompt: ContentInput,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        system_prompt: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        render_metadata: Option<RenderMetadata>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        skill_refs: Vec<SkillRef>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        additional_instructions: Vec<String>,
    },
    Event {
        event_type: String,
        payload: serde_json::Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        render_metadata: Option<RenderMetadata>,
    },
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMaterializationSpec {
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<Provider>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        alias = "output_schema_json"
    )]
    pub output_schema: Option<OutputSchema>,
    #[serde(default)]
    pub structured_output_retries: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_params: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comms_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peer_meta: Option<PeerMeta>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub labels: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub preload_skills: Vec<SkillKey>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub additional_instructions: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub realm_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_generation: Option<u64>,
    #[serde(default)]
    pub keep_alive: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_context: Option<serde_json::Value>,
}

impl SessionMaterializationSpec {
    pub fn with_output_schema(mut self, output_schema: OutputSchema) -> Self {
        self.output_schema = Some(output_schema);
        self
    }
}

impl PartialEq for SessionMaterializationSpec {
    fn eq(&self, other: &Self) -> bool {
        self.model == other.model
            && self.system_prompt == other.system_prompt
            && self.max_tokens == other.max_tokens
            && self.provider == other.provider
            && self.output_schema == other.output_schema
            && self.structured_output_retries == other.structured_output_retries
            && self.provider_params == other.provider_params
            && self.comms_name == other.comms_name
            && self.peer_meta == other.peer_meta
            && self.labels == other.labels
            && self.preload_skills == other.preload_skills
            && self.additional_instructions == other.additional_instructions
            && self.realm_id == other.realm_id
            && self.instance_id == other.instance_id
            && self.backend == other.backend
            && self.config_generation == other.config_generation
            && self.keep_alive == other.keep_alive
            && self.app_context == other.app_context
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MobTargetBinding {
    Member {
        mob_id: String,
        member_id: String,
        action: ScheduledMobAction,
    },
    Flow {
        mob_id: String,
        flow_id: String,
        #[cfg_attr(feature = "schema", schemars(with = "serde_json::Value"))]
        params: Box<RawValue>,
    },
    SpawnHelper {
        mob_id: String,
        member_id: String,
        prompt: String,
        #[serde(default)]
        options: HelperOptionsSpec,
    },
    ForkHelper {
        mob_id: String,
        source_member_id: String,
        member_id: String,
        prompt: String,
        #[serde(default)]
        fork_context: ForkContextSpec,
        #[serde(default)]
        options: HelperOptionsSpec,
    },
}

impl PartialEq for MobTargetBinding {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (
                Self::Member {
                    mob_id: left_mob_id,
                    member_id: left_member_id,
                    action: left_action,
                },
                Self::Member {
                    mob_id: right_mob_id,
                    member_id: right_member_id,
                    action: right_action,
                },
            ) => {
                left_mob_id == right_mob_id
                    && left_member_id == right_member_id
                    && left_action == right_action
            }
            (
                Self::Flow {
                    mob_id: left_mob_id,
                    flow_id: left_flow_id,
                    params: left_params,
                },
                Self::Flow {
                    mob_id: right_mob_id,
                    flow_id: right_flow_id,
                    params: right_params,
                },
            ) => {
                left_mob_id == right_mob_id
                    && left_flow_id == right_flow_id
                    && left_params.get() == right_params.get()
            }
            (
                Self::SpawnHelper {
                    mob_id: left_mob_id,
                    member_id: left_member_id,
                    prompt: left_prompt,
                    options: left_options,
                },
                Self::SpawnHelper {
                    mob_id: right_mob_id,
                    member_id: right_member_id,
                    prompt: right_prompt,
                    options: right_options,
                },
            ) => {
                left_mob_id == right_mob_id
                    && left_member_id == right_member_id
                    && left_prompt == right_prompt
                    && left_options == right_options
            }
            (
                Self::ForkHelper {
                    mob_id: left_mob_id,
                    source_member_id: left_source_member_id,
                    member_id: left_member_id,
                    prompt: left_prompt,
                    fork_context: left_fork_context,
                    options: left_options,
                },
                Self::ForkHelper {
                    mob_id: right_mob_id,
                    source_member_id: right_source_member_id,
                    member_id: right_member_id,
                    prompt: right_prompt,
                    fork_context: right_fork_context,
                    options: right_options,
                },
            ) => {
                left_mob_id == right_mob_id
                    && left_source_member_id == right_source_member_id
                    && left_member_id == right_member_id
                    && left_prompt == right_prompt
                    && left_fork_context == right_fork_context
                    && left_options == right_options
            }
            _ => false,
        }
    }
}

impl MobTargetBinding {
    pub fn stable_key(&self) -> String {
        semantic_json_key("mob_target", self)
    }

    pub fn validate_public_api(&self) -> Result<(), String> {
        match self {
            Self::SpawnHelper { options, .. } | Self::ForkHelper { options, .. } => {
                options.validate_public_api()
            }
            Self::Member { .. } | Self::Flow { .. } => Ok(()),
        }
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ScheduledMobAction {
    Send {
        content: ContentInput,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        render_metadata: Option<RenderMetadata>,
    },
}

/// How the scheduled helper's tool surface should be determined.
///
/// Schedule-local mirror of `meerkat_mob::SpawnTooling`, wire-compatible.
/// `InheritParent` and `Minimal` require an active parent agent context and
/// are rejected by public schedule APIs — they are only valid when a schedule
/// is created through agent tools (where the parent can snapshot its tools).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum ScheduleSpawnTooling {
    /// Inherit the parent's currently visible tools (ToolScope snapshot).
    InheritParent {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        allow_overlay: Option<Vec<String>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        deny_overlay: Option<Vec<String>>,
    },
    /// Minimal: only comms tools.
    Minimal,
    /// Use a specific named profile for model/tool resolution.
    Profile {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        allow_overlay: Option<Vec<String>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        deny_overlay: Option<Vec<String>>,
    },
}

impl ScheduleSpawnTooling {
    /// Returns true if this tooling mode requires an active parent agent context.
    pub fn requires_parent_context(&self) -> bool {
        matches!(self, Self::InheritParent { .. } | Self::Minimal)
    }
}

/// Pre-resolved spawn snapshot captured at schedule creation time.
///
/// When `ScheduleSpawnTooling::InheritParent` or `Minimal` is specified through
/// agent tools, the parent's visible tool set is snapshotted and stored here.
/// At execution time, the schedule driver uses this snapshot directly — no
/// parent context is needed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolvedSpawnSnapshot {
    /// The tool filter to apply as the child's inherited base filter.
    pub tool_filter: meerkat_core::tool_scope::ToolFilter,
    /// Witnesses for every named tool in `tool_filter`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub tool_filter_witnesses: BTreeMap<String, ToolVisibilityWitness>,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HelperOptionsSpec {
    /// Role name (profile key) for the helper in the mob roster.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_mode: Option<ScheduledMobRuntimeMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend: Option<ScheduledMobBackendKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_access_policy: Option<ToolAccessPolicy>,
    /// Tooling mode request for the helper's tool surface.
    ///
    /// This is an input-side field consumed during schedule creation.
    /// `InheritParent` and `Minimal` are resolved into `resolved_spawn_snapshot`
    /// by the agent tool path; they are rejected by public schedule APIs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "schema", schemars(skip))]
    pub tooling: Option<ScheduleSpawnTooling>,
    /// Pre-resolved tool-visibility snapshot, populated when `tooling` is consumed.
    ///
    /// At execution time, the schedule driver reads this field directly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "schema", schemars(skip))]
    pub resolved_spawn_snapshot: Option<ResolvedSpawnSnapshot>,
}

impl HelperOptionsSpec {
    /// Validate that the tooling mode is acceptable for public schedule APIs.
    ///
    /// `InheritParent` and `Minimal` require an active parent agent context and
    /// are only valid when created through agent tools. Public APIs must reject them.
    pub fn validate_public_api(&self) -> Result<(), String> {
        if self.resolved_spawn_snapshot.is_some() {
            return Err(
                "resolved spawn snapshots are trusted internal schedule state and cannot be \
                 supplied through public schedule APIs"
                    .to_string(),
            );
        }
        if let Some(tooling) = &self.tooling
            && tooling.requires_parent_context()
        {
            return Err("schedule spawn tooling mode requires parent agent context \
                 (inherit_parent/minimal are only valid through agent tools)"
                .to_string());
        }
        if let Some(ScheduleSpawnTooling::Profile {
            allow_overlay,
            deny_overlay,
            ..
        }) = &self.tooling
            && (allow_overlay.is_some() || deny_overlay.is_some())
        {
            return Err(
                "schedule spawn profile overlays require trusted resolved spawn state and are \
                 only valid through agent tools"
                    .to_string(),
            );
        }
        Ok(())
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScheduledMobRuntimeMode {
    AutonomousHost,
    TurnDriven,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScheduledMobBackendKind {
    Session,
    External,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ForkContextSpec {
    #[default]
    FullHistory,
    LastMessages {
        count: u32,
    },
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OccurrenceFailureClass {
    TargetMaterializationFailed,
    TargetMissing,
    TargetBusy,
    RuntimeRejected,
    MobRejected,
    LeaseLost,
    TransportError,
    InternalError,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RuntimeDeliveryOutcome {
    AdmissionRejected {
        detail: String,
    },
    CompletionAbandoned {
        detail: String,
    },
    CompletionCallbackPending {
        tool_name: String,
        payload: serde_json::Value,
    },
    CompletionRuntimeTerminated {
        detail: String,
    },
}

impl RuntimeDeliveryOutcome {
    pub fn detail(&self) -> String {
        match self {
            Self::AdmissionRejected { detail }
            | Self::CompletionAbandoned { detail }
            | Self::CompletionRuntimeTerminated { detail } => detail.clone(),
            Self::CompletionCallbackPending { tool_name, payload } => {
                format!("callback pending for tool '{tool_name}': {payload}")
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum RuntimeCompletionOutcome {
    Completed,
    CallbackPending,
    Cancelled,
    Abandoned,
    FinalizationFailed,
    RuntimeTerminated,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryReceiptStage {
    Planned,
    Claimed,
    DispatchStarted,
    DispatchAccepted,
    AwaitingCompletion,
    Completed,
    Skipped,
    Misfired,
    Superseded,
    DeliveryFailed,
    LeaseExpired,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveryReceipt {
    #[cfg_attr(feature = "schema", schemars(with = "String"))]
    pub receipt_id: Uuid,
    pub occurrence_id: OccurrenceId,
    pub attempt: u32,
    pub stage: DeliveryReceiptStage,
    pub recorded_at_utc: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_class: Option<OccurrenceFailureClass>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_outcome: Option<RuntimeDeliveryOutcome>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub materialized_session_id: Option<SessionId>,
}

impl DeliveryReceipt {
    pub fn new(occurrence_id: OccurrenceId, attempt: u32, stage: DeliveryReceiptStage) -> Self {
        Self {
            receipt_id: Uuid::now_v7(),
            occurrence_id,
            attempt,
            stage,
            recorded_at_utc: Utc::now(),
            correlation_id: None,
            detail: None,
            failure_class: None,
            runtime_outcome: None,
            materialized_session_id: None,
        }
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ScheduleConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub planning_horizon_days: u32,
    pub planning_horizon_occurrences: u32,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub labels: BTreeMap<String, String>,
    pub created_at_utc: DateTime<Utc>,
    pub updated_at_utc: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted_at_utc: Option<DateTime<Utc>>,
}

#[derive(Deserialize)]
struct ScheduleConfigWire {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    planning_horizon_days: Option<u64>,
    #[serde(default)]
    planning_horizon_occurrences: Option<u64>,
    #[serde(default)]
    labels: BTreeMap<String, String>,
    created_at_utc: DateTime<Utc>,
    updated_at_utc: DateTime<Utc>,
    #[serde(default)]
    deleted_at_utc: Option<DateTime<Utc>>,
}

impl<'de> Deserialize<'de> for ScheduleConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = ScheduleConfigWire::deserialize(deserializer)?;
        let defaults =
            crate::machines::schedule_lifecycle::ScheduleLifecycleMachineState::default();
        Ok(Self {
            name: wire.name,
            description: wire.description,
            planning_horizon_days: planning_horizon_u32(
                "planning_horizon_days",
                wire.planning_horizon_days
                    .unwrap_or(defaults.planning_horizon_days),
            )
            .map_err(D::Error::custom)?,
            planning_horizon_occurrences: planning_horizon_u32(
                "planning_horizon_occurrences",
                wire.planning_horizon_occurrences
                    .unwrap_or(defaults.planning_horizon_occurrences),
            )
            .map_err(D::Error::custom)?,
            labels: wire.labels,
            created_at_utc: wire.created_at_utc,
            updated_at_utc: wire.updated_at_utc,
            deleted_at_utc: wire.deleted_at_utc,
        })
    }
}

fn planning_horizon_u32(field: &'static str, value: u64) -> Result<u32, String> {
    u32::try_from(value).map_err(|error| format!("{field} value {value} exceeds u32: {error}"))
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq)]
pub struct Schedule {
    pub schedule_id: ScheduleId,
    pub phase: SchedulePhase,
    pub revision: ScheduleRevision,
    #[cfg_attr(feature = "schema", schemars(skip))]
    pub(crate) machine_state: sched_dsl::ScheduleLifecycleMachineState,
    pub trigger: TriggerSpec,
    pub target: TargetBinding,
    pub misfire_policy: MisfirePolicy,
    pub overlap_policy: OverlapPolicy,
    pub missing_target_policy: MissingTargetPolicy,
    pub next_occurrence_ordinal: OccurrenceOrdinal,
    pub planning_cursor_utc: Option<DateTime<Utc>>,
    pub superseded_ack_ids: BTreeSet<OccurrenceId>,
    pub config: ScheduleConfig,
}

#[derive(Serialize)]
struct ScheduleSerdeWire {
    schedule_id: ScheduleId,
    phase: SchedulePhase,
    revision: ScheduleRevision,
    machine_state: ScheduleMachineStateWire,
    trigger: TriggerSpec,
    target: TargetBinding,
    misfire_policy: MisfirePolicy,
    overlap_policy: OverlapPolicy,
    missing_target_policy: MissingTargetPolicy,
    next_occurrence_ordinal: OccurrenceOrdinal,
    #[serde(skip_serializing_if = "Option::is_none")]
    planning_cursor_utc: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    superseded_ack_ids: BTreeSet<OccurrenceId>,
    #[serde(flatten)]
    config: ScheduleConfig,
}

#[derive(Deserialize)]
struct ScheduleWire {
    schedule_id: ScheduleId,
    phase: SchedulePhase,
    revision: ScheduleRevision,
    #[serde(default)]
    machine_state: Option<ScheduleMachineStateWire>,
    trigger: TriggerSpec,
    target: TargetBinding,
    #[serde(default)]
    misfire_policy: MisfirePolicy,
    #[serde(default)]
    overlap_policy: OverlapPolicy,
    #[serde(default)]
    missing_target_policy: MissingTargetPolicy,
    next_occurrence_ordinal: OccurrenceOrdinal,
    #[serde(default)]
    planning_cursor_utc: Option<DateTime<Utc>>,
    #[serde(default)]
    superseded_ack_ids: BTreeSet<OccurrenceId>,
    #[serde(flatten)]
    config: ScheduleConfig,
}

#[derive(Clone, Serialize, Deserialize)]
struct ScheduleMachineStateWire {
    machine: String,
    schema_version: u32,
    schedule_id: String,
    lifecycle_phase: String,
    revision: u64,
    trigger_key: String,
    target_binding_key: String,
    misfire_policy: String,
    misfire_policy_key: String,
    overlap_policy: String,
    overlap_policy_key: String,
    missing_target_policy: String,
    missing_target_policy_key: String,
    planning_horizon_days: u64,
    planning_horizon_occurrences: u64,
    planning_cursor_utc_ms: Option<u64>,
    next_occurrence_ordinal: u64,
    #[serde(default)]
    superseded_ack_ids: BTreeSet<String>,
}

impl From<&sched_dsl::ScheduleLifecycleMachineState> for ScheduleMachineStateWire {
    fn from(state: &sched_dsl::ScheduleLifecycleMachineState) -> Self {
        let (machine, schema_version) = schedule_machine_schema_identity();
        Self {
            machine,
            schema_version,
            schedule_id: state.schedule_id.0.clone(),
            lifecycle_phase: schedule_lifecycle_state_to_wire(state.lifecycle_phase).to_string(),
            revision: state.revision,
            trigger_key: state.trigger_key.clone(),
            target_binding_key: state.target_binding_key.clone(),
            misfire_policy: schedule_misfire_policy_to_wire(state.misfire_policy).to_string(),
            misfire_policy_key: state.misfire_policy_key.clone(),
            overlap_policy: schedule_overlap_policy_to_wire(state.overlap_policy).to_string(),
            overlap_policy_key: state.overlap_policy_key.clone(),
            missing_target_policy: schedule_missing_target_policy_to_wire(
                state.missing_target_policy,
            )
            .to_string(),
            missing_target_policy_key: state.missing_target_policy_key.clone(),
            planning_horizon_days: state.planning_horizon_days,
            planning_horizon_occurrences: state.planning_horizon_occurrences,
            planning_cursor_utc_ms: state.planning_cursor_utc_ms,
            next_occurrence_ordinal: state.next_occurrence_ordinal,
            superseded_ack_ids: state
                .superseded_ack_ids
                .iter()
                .map(|id| id.0.clone())
                .collect(),
        }
    }
}

impl TryFrom<ScheduleMachineStateWire> for sched_dsl::ScheduleLifecycleMachineState {
    type Error = String;

    fn try_from(wire: ScheduleMachineStateWire) -> Result<Self, Self::Error> {
        validate_schedule_machine_wire_header(&wire.machine, wire.schema_version)?;
        Ok(Self {
            schedule_id: sched_dsl::ScheduleId(wire.schedule_id),
            lifecycle_phase: schedule_lifecycle_state_from_wire(&wire.lifecycle_phase)?,
            revision: wire.revision,
            trigger_key: wire.trigger_key,
            target_binding_key: wire.target_binding_key,
            misfire_policy: schedule_misfire_policy_from_wire(&wire.misfire_policy)?,
            misfire_policy_key: wire.misfire_policy_key,
            overlap_policy: schedule_overlap_policy_from_wire(&wire.overlap_policy)?,
            overlap_policy_key: wire.overlap_policy_key,
            missing_target_policy: schedule_missing_target_policy_from_wire(
                &wire.missing_target_policy,
            )?,
            missing_target_policy_key: wire.missing_target_policy_key,
            planning_horizon_days: wire.planning_horizon_days,
            planning_horizon_occurrences: wire.planning_horizon_occurrences,
            planning_cursor_utc_ms: wire.planning_cursor_utc_ms,
            next_occurrence_ordinal: wire.next_occurrence_ordinal,
            superseded_ack_ids: wire
                .superseded_ack_ids
                .into_iter()
                .map(sched_dsl::OccurrenceId)
                .collect(),
        })
    }
}

impl From<&Schedule> for ScheduleSerdeWire {
    fn from(schedule: &Schedule) -> Self {
        Self {
            schedule_id: schedule.schedule_id.clone(),
            phase: schedule.phase,
            revision: schedule.revision,
            machine_state: ScheduleMachineStateWire::from(&schedule.machine_state),
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

impl Serialize for Schedule {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        ScheduleSerdeWire::from(self).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Schedule {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let mut wire = ScheduleWire::deserialize(deserializer)?;
        let machine_state = wire
            .machine_state
            .take()
            .ok_or_else(|| D::Error::missing_field("machine_state"))?
            .try_into()
            .map_err(D::Error::custom)?;
        let schedule = Self {
            schedule_id: wire.schedule_id,
            phase: wire.phase,
            revision: wire.revision,
            machine_state,
            trigger: wire.trigger,
            target: wire.target,
            misfire_policy: wire.misfire_policy,
            overlap_policy: wire.overlap_policy,
            missing_target_policy: wire.missing_target_policy,
            next_occurrence_ordinal: wire.next_occurrence_ordinal,
            planning_cursor_utc: wire.planning_cursor_utc,
            superseded_ack_ids: wire.superseded_ack_ids,
            config: wire.config,
        };
        validate_schedule_machine_projection(&schedule).map_err(D::Error::custom)?;
        Ok(schedule)
    }
}

impl Schedule {
    pub fn new(
        request: CreateScheduleRequest,
    ) -> Result<Self, crate::lifecycle::ScheduleLifecycleError> {
        Ok(Self::apply(
            None,
            crate::lifecycle::ScheduleLifecycleInput::Create(request),
        )?
        .into_schedule())
    }

    pub fn touch(&mut self) {
        self.config.updated_at_utc = Utc::now();
    }

    pub fn validate_machine_projection(&self) -> Result<(), String> {
        validate_schedule_machine_projection(self)
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq)]
pub struct Occurrence {
    pub occurrence_id: OccurrenceId,
    pub schedule_id: ScheduleId,
    pub schedule_revision: ScheduleRevision,
    pub occurrence_ordinal: OccurrenceOrdinal,
    pub phase: OccurrencePhase,
    #[cfg_attr(feature = "schema", schemars(skip))]
    pub(crate) machine_state: occ_dsl::OccurrenceLifecycleMachineState,
    pub due_at_utc: DateTime<Utc>,
    pub trigger_snapshot: TriggerSpec,
    pub target_snapshot: TargetBinding,
    pub misfire_policy: MisfirePolicy,
    pub overlap_policy: OverlapPolicy,
    pub missing_target_policy: MissingTargetPolicy,
    pub claimed_by: Option<String>,
    pub lease_expires_at_utc: Option<DateTime<Utc>>,
    #[cfg_attr(feature = "schema", schemars(skip))]
    pub(crate) claim_token: Option<Uuid>,
    pub delivery_correlation_id: Option<String>,
    pub last_receipt: Option<DeliveryReceipt>,
    pub failure_class: Option<OccurrenceFailureClass>,
    pub runtime_outcome: Option<RuntimeDeliveryOutcome>,
    pub failure_detail: Option<String>,
    pub attempt_count: u32,
    pub created_at_utc: DateTime<Utc>,
    pub claimed_at_utc: Option<DateTime<Utc>>,
    pub dispatched_at_utc: Option<DateTime<Utc>>,
    pub completed_at_utc: Option<DateTime<Utc>>,
    pub superseded_by_revision: Option<ScheduleRevision>,
}

#[derive(Serialize)]
struct OccurrenceSerdeWire {
    occurrence_id: OccurrenceId,
    schedule_id: ScheduleId,
    schedule_revision: ScheduleRevision,
    occurrence_ordinal: OccurrenceOrdinal,
    phase: OccurrencePhase,
    machine_state: OccurrenceMachineStateWire,
    due_at_utc: DateTime<Utc>,
    trigger_snapshot: TriggerSpec,
    target_snapshot: TargetBinding,
    misfire_policy: MisfirePolicy,
    overlap_policy: OverlapPolicy,
    missing_target_policy: MissingTargetPolicy,
    #[serde(skip_serializing_if = "Option::is_none")]
    claimed_by: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    lease_expires_at_utc: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    claim_token: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    delivery_correlation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_receipt: Option<DeliveryReceipt>,
    #[serde(skip_serializing_if = "Option::is_none")]
    failure_class: Option<OccurrenceFailureClass>,
    #[serde(skip_serializing_if = "Option::is_none")]
    runtime_outcome: Option<RuntimeDeliveryOutcome>,
    #[serde(skip_serializing_if = "Option::is_none")]
    failure_detail: Option<String>,
    attempt_count: u32,
    created_at_utc: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    claimed_at_utc: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    dispatched_at_utc: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    completed_at_utc: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    superseded_by_revision: Option<ScheduleRevision>,
}

#[derive(Deserialize)]
struct OccurrenceWire {
    occurrence_id: OccurrenceId,
    schedule_id: ScheduleId,
    schedule_revision: ScheduleRevision,
    occurrence_ordinal: OccurrenceOrdinal,
    phase: OccurrencePhase,
    #[serde(default)]
    machine_state: Option<OccurrenceMachineStateWire>,
    due_at_utc: DateTime<Utc>,
    trigger_snapshot: TriggerSpec,
    target_snapshot: TargetBinding,
    misfire_policy: MisfirePolicy,
    overlap_policy: OverlapPolicy,
    missing_target_policy: MissingTargetPolicy,
    #[serde(default)]
    claimed_by: Option<String>,
    #[serde(default)]
    lease_expires_at_utc: Option<DateTime<Utc>>,
    #[serde(default)]
    claim_token: Option<Uuid>,
    #[serde(default)]
    delivery_correlation_id: Option<String>,
    #[serde(default)]
    last_receipt: Option<DeliveryReceipt>,
    #[serde(default)]
    failure_class: Option<OccurrenceFailureClass>,
    #[serde(default)]
    runtime_outcome: Option<RuntimeDeliveryOutcome>,
    #[serde(default)]
    failure_detail: Option<String>,
    #[serde(default)]
    attempt_count: u32,
    created_at_utc: DateTime<Utc>,
    #[serde(default)]
    claimed_at_utc: Option<DateTime<Utc>>,
    #[serde(default)]
    dispatched_at_utc: Option<DateTime<Utc>>,
    #[serde(default)]
    completed_at_utc: Option<DateTime<Utc>>,
    #[serde(default)]
    superseded_by_revision: Option<ScheduleRevision>,
}

#[derive(Clone, Serialize, Deserialize)]
struct OccurrenceMachineStateWire {
    machine: String,
    schema_version: u32,
    lifecycle_phase: String,
    occurrence_id: String,
    schedule_id: String,
    schedule_revision: u64,
    occurrence_ordinal: u64,
    trigger_key: String,
    target_binding_key: String,
    misfire_policy: String,
    misfire_policy_key: String,
    overlap_policy: String,
    overlap_policy_key: String,
    missing_target_policy: String,
    missing_target_policy_key: String,
    due_at_utc_ms: u64,
    misfire_deadline_utc_ms: u64,
    claimed_by: Option<String>,
    lease_expires_at_utc_ms: Option<u64>,
    claimed_at_utc_ms: Option<u64>,
    claim_token: Option<String>,
    delivery_correlation_id: Option<String>,
    target_materialized_session_id: Option<String>,
    receipt_recorded_at_utc_ms: Option<u64>,
    last_receipt_recorded_at_utc_ms: Option<u64>,
    last_receipt_attempt: Option<u64>,
    last_receipt_stage: Option<String>,
    last_receipt_failure_class: Option<String>,
    last_receipt_detail: Option<String>,
    last_receipt_correlation_id: Option<String>,
    last_receipt_materialized_session_id: Option<String>,
    runtime_outcome_key: Option<String>,
    receipt_stage: Option<String>,
    receipt_failure_class: Option<String>,
    receipt_detail: Option<String>,
    failure_class: Option<String>,
    failure_detail: Option<String>,
    dispatched_at_utc_ms: Option<u64>,
    completed_at_utc_ms: Option<u64>,
    attempt_count: u64,
    superseded_by_revision: Option<u64>,
}

impl From<&occ_dsl::OccurrenceLifecycleMachineState> for OccurrenceMachineStateWire {
    fn from(state: &occ_dsl::OccurrenceLifecycleMachineState) -> Self {
        let (machine, schema_version) = occurrence_machine_schema_identity();
        Self {
            machine,
            schema_version,
            lifecycle_phase: occurrence_lifecycle_state_to_wire(state.lifecycle_phase).to_string(),
            occurrence_id: state.occurrence_id.0.clone(),
            schedule_id: state.schedule_id.0.clone(),
            schedule_revision: state.schedule_revision,
            occurrence_ordinal: state.occurrence_ordinal,
            trigger_key: state.trigger_key.clone(),
            target_binding_key: state.target_binding_key.clone(),
            misfire_policy: occurrence_misfire_policy_to_wire(state.misfire_policy).to_string(),
            misfire_policy_key: state.misfire_policy_key.clone(),
            overlap_policy: occurrence_overlap_policy_to_wire(state.overlap_policy).to_string(),
            overlap_policy_key: state.overlap_policy_key.clone(),
            missing_target_policy: occurrence_missing_target_policy_to_wire(
                state.missing_target_policy,
            )
            .to_string(),
            missing_target_policy_key: state.missing_target_policy_key.clone(),
            due_at_utc_ms: state.due_at_utc_ms,
            misfire_deadline_utc_ms: state.misfire_deadline_utc_ms,
            claimed_by: state.claimed_by.clone(),
            lease_expires_at_utc_ms: state.lease_expires_at_utc_ms,
            claimed_at_utc_ms: state.claimed_at_utc_ms,
            claim_token: state.claim_token.as_ref().map(|token| token.0.clone()),
            delivery_correlation_id: state.delivery_correlation_id.clone(),
            target_materialized_session_id: state
                .target_materialized_session_id
                .as_ref()
                .map(|session_id| session_id.0.clone()),
            receipt_recorded_at_utc_ms: state.receipt_recorded_at_utc_ms,
            last_receipt_recorded_at_utc_ms: state.last_receipt_recorded_at_utc_ms,
            last_receipt_attempt: state.last_receipt_attempt,
            last_receipt_stage: state
                .last_receipt_stage
                .map(occurrence_receipt_stage_to_wire)
                .map(str::to_string),
            last_receipt_failure_class: state
                .last_receipt_failure_class
                .map(occurrence_failure_class_to_wire)
                .map(str::to_string),
            last_receipt_detail: state.last_receipt_detail.clone(),
            last_receipt_correlation_id: state.last_receipt_correlation_id.clone(),
            last_receipt_materialized_session_id: state
                .last_receipt_materialized_session_id
                .as_ref()
                .map(|session_id| session_id.0.clone()),
            runtime_outcome_key: state.runtime_outcome_key.clone(),
            receipt_stage: state
                .receipt_stage
                .map(occurrence_receipt_stage_to_wire)
                .map(str::to_string),
            receipt_failure_class: state
                .receipt_failure_class
                .map(occurrence_failure_class_to_wire)
                .map(str::to_string),
            receipt_detail: state.receipt_detail.clone(),
            failure_class: state
                .failure_class
                .map(occurrence_failure_class_to_wire)
                .map(str::to_string),
            failure_detail: state.failure_detail.clone(),
            dispatched_at_utc_ms: state.dispatched_at_utc_ms,
            completed_at_utc_ms: state.completed_at_utc_ms,
            attempt_count: state.attempt_count,
            superseded_by_revision: state.superseded_by_revision,
        }
    }
}

impl TryFrom<OccurrenceMachineStateWire> for occ_dsl::OccurrenceLifecycleMachineState {
    type Error = String;

    fn try_from(wire: OccurrenceMachineStateWire) -> Result<Self, Self::Error> {
        validate_occurrence_machine_wire_header(&wire.machine, wire.schema_version)?;
        Ok(Self {
            lifecycle_phase: occurrence_lifecycle_state_from_wire(&wire.lifecycle_phase)?,
            occurrence_id: occ_dsl::OccurrenceId(wire.occurrence_id),
            schedule_id: occ_dsl::ScheduleId(wire.schedule_id),
            schedule_revision: wire.schedule_revision,
            occurrence_ordinal: wire.occurrence_ordinal,
            trigger_key: wire.trigger_key,
            target_binding_key: wire.target_binding_key,
            misfire_policy: occurrence_misfire_policy_from_wire(&wire.misfire_policy)?,
            misfire_policy_key: wire.misfire_policy_key,
            overlap_policy: occurrence_overlap_policy_from_wire(&wire.overlap_policy)?,
            overlap_policy_key: wire.overlap_policy_key,
            missing_target_policy: occurrence_missing_target_policy_from_wire(
                &wire.missing_target_policy,
            )?,
            missing_target_policy_key: wire.missing_target_policy_key,
            due_at_utc_ms: wire.due_at_utc_ms,
            misfire_deadline_utc_ms: wire.misfire_deadline_utc_ms,
            claimed_by: wire.claimed_by,
            lease_expires_at_utc_ms: wire.lease_expires_at_utc_ms,
            claimed_at_utc_ms: wire.claimed_at_utc_ms,
            claim_token: wire.claim_token.map(occ_dsl::ClaimToken),
            delivery_correlation_id: wire.delivery_correlation_id,
            target_materialized_session_id: wire
                .target_materialized_session_id
                .map(occ_dsl::SessionId),
            receipt_recorded_at_utc_ms: wire.receipt_recorded_at_utc_ms,
            last_receipt_recorded_at_utc_ms: wire.last_receipt_recorded_at_utc_ms,
            last_receipt_attempt: wire.last_receipt_attempt,
            last_receipt_stage: wire
                .last_receipt_stage
                .as_deref()
                .map(occurrence_receipt_stage_from_wire)
                .transpose()?,
            last_receipt_failure_class: wire
                .last_receipt_failure_class
                .as_deref()
                .map(occurrence_failure_class_from_wire)
                .transpose()?,
            last_receipt_detail: wire.last_receipt_detail,
            last_receipt_correlation_id: wire.last_receipt_correlation_id,
            last_receipt_materialized_session_id: wire
                .last_receipt_materialized_session_id
                .map(occ_dsl::SessionId),
            runtime_outcome_key: wire.runtime_outcome_key,
            receipt_stage: wire
                .receipt_stage
                .as_deref()
                .map(occurrence_receipt_stage_from_wire)
                .transpose()?,
            receipt_failure_class: wire
                .receipt_failure_class
                .as_deref()
                .map(occurrence_failure_class_from_wire)
                .transpose()?,
            receipt_detail: wire.receipt_detail,
            failure_class: wire
                .failure_class
                .as_deref()
                .map(occurrence_failure_class_from_wire)
                .transpose()?,
            failure_detail: wire.failure_detail,
            dispatched_at_utc_ms: wire.dispatched_at_utc_ms,
            completed_at_utc_ms: wire.completed_at_utc_ms,
            attempt_count: wire.attempt_count,
            superseded_by_revision: wire.superseded_by_revision,
        })
    }
}

impl From<&Occurrence> for OccurrenceSerdeWire {
    fn from(occurrence: &Occurrence) -> Self {
        Self {
            occurrence_id: occurrence.occurrence_id.clone(),
            schedule_id: occurrence.schedule_id.clone(),
            schedule_revision: occurrence.schedule_revision,
            occurrence_ordinal: occurrence.occurrence_ordinal,
            phase: occurrence.phase,
            machine_state: OccurrenceMachineStateWire::from(&occurrence.machine_state),
            due_at_utc: occurrence.due_at_utc,
            trigger_snapshot: occurrence.trigger_snapshot.clone(),
            target_snapshot: occurrence.target_snapshot.clone(),
            misfire_policy: occurrence.misfire_policy.clone(),
            overlap_policy: occurrence.overlap_policy.clone(),
            missing_target_policy: occurrence.missing_target_policy.clone(),
            claimed_by: occurrence.claimed_by.clone(),
            lease_expires_at_utc: occurrence.lease_expires_at_utc,
            claim_token: occurrence.claim_token,
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

impl Serialize for Occurrence {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        OccurrenceSerdeWire::from(self).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Occurrence {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let mut wire = OccurrenceWire::deserialize(deserializer)?;
        let machine_state = wire
            .machine_state
            .take()
            .ok_or_else(|| D::Error::missing_field("machine_state"))?
            .try_into()
            .map_err(D::Error::custom)?;
        let occurrence = Self {
            occurrence_id: wire.occurrence_id,
            schedule_id: wire.schedule_id,
            schedule_revision: wire.schedule_revision,
            occurrence_ordinal: wire.occurrence_ordinal,
            phase: wire.phase,
            machine_state,
            due_at_utc: wire.due_at_utc,
            trigger_snapshot: wire.trigger_snapshot,
            target_snapshot: wire.target_snapshot,
            misfire_policy: wire.misfire_policy,
            overlap_policy: wire.overlap_policy,
            missing_target_policy: wire.missing_target_policy,
            claimed_by: wire.claimed_by,
            lease_expires_at_utc: wire.lease_expires_at_utc,
            claim_token: wire.claim_token,
            delivery_correlation_id: wire.delivery_correlation_id,
            last_receipt: wire.last_receipt,
            failure_class: wire.failure_class,
            runtime_outcome: wire.runtime_outcome,
            failure_detail: wire.failure_detail,
            attempt_count: wire.attempt_count,
            created_at_utc: wire.created_at_utc,
            claimed_at_utc: wire.claimed_at_utc,
            dispatched_at_utc: wire.dispatched_at_utc,
            completed_at_utc: wire.completed_at_utc,
            superseded_by_revision: wire.superseded_by_revision,
        };
        validate_occurrence_machine_projection(&occurrence).map_err(D::Error::custom)?;
        Ok(occurrence)
    }
}

impl Occurrence {
    pub fn validate_machine_projection(&self) -> Result<(), String> {
        validate_occurrence_machine_projection(self)
    }

    pub fn due_misfire_detail_at(&self, now_utc: DateTime<Utc>) -> String {
        self.misfire_policy.misfire_detail(self.due_at_utc, now_utc)
    }

    pub fn claim_token(&self) -> Option<Uuid> {
        self.claim_token
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateScheduleRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub trigger: TriggerSpec,
    pub target: TargetBinding,
    #[serde(default)]
    pub misfire_policy: MisfirePolicy,
    #[serde(default)]
    pub overlap_policy: OverlapPolicy,
    #[serde(default)]
    pub missing_target_policy: MissingTargetPolicy,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub labels: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub planning_horizon_days: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub planning_horizon_occurrences: Option<u32>,
}

impl CreateScheduleRequest {
    pub fn validate_public_api(&self) -> Result<(), String> {
        self.target.validate_public_api()
    }
}

#[cfg(all(test, feature = "schema"))]
#[allow(clippy::unwrap_used)]
mod schema_tests {
    use super::HelperOptionsSpec;

    #[test]
    fn helper_options_schema_exposes_typed_tool_access_policy_variants() {
        let schema_json = serde_json::to_string(&schemars::schema_for!(HelperOptionsSpec)).unwrap();

        assert!(
            schema_json.contains("allow_list"),
            "helper options schema should expose ToolAccessPolicy variants"
        );
        assert!(
            schema_json.contains("deny_list"),
            "helper options schema should expose ToolAccessPolicy variants"
        );
        assert!(
            schema_json.contains("inherit"),
            "helper options schema should expose ToolAccessPolicy variants"
        );
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct UpdateScheduleRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_revision: Option<ScheduleRevision>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger: Option<TriggerSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<TargetBinding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub misfire_policy: Option<MisfirePolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overlap_policy: Option<OverlapPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub missing_target_policy: Option<MissingTargetPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub planning_horizon_days: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub planning_horizon_occurrences: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labels: Option<BTreeMap<String, String>>,
}

impl UpdateScheduleRequest {
    pub fn validate_public_api(&self) -> Result<(), String> {
        if let Some(target) = &self.target {
            target.validate_public_api()?;
        }
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use meerkat_core::ToolNameSet;
    use meerkat_core::skills::{SkillName, SourceUuid};

    fn fixture_source_uuid() -> SourceUuid {
        SourceUuid::parse("dc256086-0d2f-4f61-a307-320d4148107f").unwrap()
    }

    fn fixture_skill_key(name: &str) -> SkillKey {
        SkillKey::new(fixture_source_uuid(), SkillName::parse(name).unwrap())
    }

    fn fixture_session_materialization(
        preload_skills: Vec<SkillKey>,
    ) -> SessionMaterializationSpec {
        SessionMaterializationSpec {
            model: "claude-sonnet-4-6".into(),
            system_prompt: None,
            max_tokens: None,
            provider: None,
            output_schema: None,
            structured_output_retries: 0,
            provider_params: None,
            comms_name: None,
            peer_meta: None,
            labels: BTreeMap::new(),
            preload_skills,
            additional_instructions: Vec::new(),
            realm_id: None,
            instance_id: None,
            backend: None,
            config_generation: None,
            keep_alive: false,
            app_context: None,
        }
    }

    // -----------------------------------------------------------------------
    // ScheduleSpawnTooling serde roundtrip
    // -----------------------------------------------------------------------

    #[test]
    fn schedule_spawn_tooling_inherit_parent_roundtrip() {
        let tooling = ScheduleSpawnTooling::InheritParent {
            allow_overlay: Some(vec!["shell".into()]),
            deny_overlay: None,
        };
        let json = serde_json::to_string(&tooling).unwrap();
        let parsed: ScheduleSpawnTooling = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, tooling);
    }

    #[test]
    fn schedule_spawn_tooling_minimal_roundtrip() {
        let tooling = ScheduleSpawnTooling::Minimal;
        let json = serde_json::to_string(&tooling).unwrap();
        let parsed: ScheduleSpawnTooling = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, tooling);
    }

    #[test]
    fn schedule_spawn_tooling_profile_roundtrip() {
        let tooling = ScheduleSpawnTooling::Profile {
            name: "worker-v2".into(),
            allow_overlay: None,
            deny_overlay: Some(vec!["dangerous".into()]),
        };
        let json = serde_json::to_string(&tooling).unwrap();
        let parsed: ScheduleSpawnTooling = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, tooling);
    }

    #[test]
    fn schedule_spawn_tooling_requires_parent_context() {
        assert!(
            ScheduleSpawnTooling::InheritParent {
                allow_overlay: None,
                deny_overlay: None,
            }
            .requires_parent_context()
        );
        assert!(ScheduleSpawnTooling::Minimal.requires_parent_context());
        assert!(
            !ScheduleSpawnTooling::Profile {
                name: "x".into(),
                allow_overlay: None,
                deny_overlay: None,
            }
            .requires_parent_context()
        );
    }

    // -----------------------------------------------------------------------
    // ResolvedSpawnSnapshot serde roundtrip
    // -----------------------------------------------------------------------

    #[test]
    fn resolved_spawn_snapshot_roundtrip_allow_filter() {
        let snapshot = ResolvedSpawnSnapshot {
            tool_filter: meerkat_core::tool_scope::ToolFilter::Allow(ToolNameSet::from_iter([
                "shell".to_string(),
                "read_file".to_string(),
            ])),
            tool_filter_witnesses: Default::default(),
        };
        let json = serde_json::to_string(&snapshot).unwrap();
        let parsed: ResolvedSpawnSnapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, snapshot);
    }

    #[test]
    fn resolved_spawn_snapshot_roundtrip_deny_filter() {
        let snapshot = ResolvedSpawnSnapshot {
            tool_filter: meerkat_core::tool_scope::ToolFilter::Deny(ToolNameSet::from_iter([
                "dangerous_tool".to_string(),
            ])),
            tool_filter_witnesses: Default::default(),
        };
        let json = serde_json::to_string(&snapshot).unwrap();
        let parsed: ResolvedSpawnSnapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, snapshot);
    }

    #[test]
    fn resolved_spawn_snapshot_roundtrip_all_filter() {
        let snapshot = ResolvedSpawnSnapshot {
            tool_filter: meerkat_core::tool_scope::ToolFilter::All,
            tool_filter_witnesses: Default::default(),
        };
        let json = serde_json::to_string(&snapshot).unwrap();
        let parsed: ResolvedSpawnSnapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, snapshot);
    }

    #[test]
    fn resolved_spawn_snapshot_rejects_legacy_profile_material() {
        let snapshot = ResolvedSpawnSnapshot {
            tool_filter: meerkat_core::tool_scope::ToolFilter::All,
            tool_filter_witnesses: Default::default(),
        };
        let mut legacy = serde_json::to_value(snapshot).unwrap();
        let legacy_object = legacy.as_object_mut().unwrap();
        legacy_object.insert("model".to_string(), serde_json::json!("legacy-model"));
        legacy_object.insert(
            "provider_params".to_string(),
            serde_json::json!({"temperature": 0.2}),
        );

        let parsed = serde_json::from_value::<ResolvedSpawnSnapshot>(legacy);

        assert!(
            parsed.is_err(),
            "legacy profile-material fields must not be silently accepted"
        );
    }

    // -----------------------------------------------------------------------
    // HelperOptionsSpec with tooling/snapshot fields
    // -----------------------------------------------------------------------

    #[test]
    fn helper_options_spec_with_tooling_roundtrip() {
        let spec = HelperOptionsSpec {
            tooling: Some(ScheduleSpawnTooling::Profile {
                name: "worker".into(),
                allow_overlay: None,
                deny_overlay: None,
            }),
            ..Default::default()
        };
        let json = serde_json::to_string(&spec).unwrap();
        let parsed: HelperOptionsSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, spec);
    }

    #[test]
    fn helper_options_spec_with_resolved_snapshot_roundtrip() {
        let spec = HelperOptionsSpec {
            resolved_spawn_snapshot: Some(ResolvedSpawnSnapshot {
                tool_filter: meerkat_core::tool_scope::ToolFilter::Allow(ToolNameSet::from_iter([
                    "shell".to_string(),
                ])),
                tool_filter_witnesses: Default::default(),
            }),
            ..Default::default()
        };
        let json = serde_json::to_string(&spec).unwrap();
        let parsed: HelperOptionsSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, spec);
    }

    #[test]
    fn helper_options_spec_default_omits_tooling_fields() {
        let spec = HelperOptionsSpec::default();
        let json = serde_json::to_string(&spec).unwrap();
        assert!(!json.contains("tooling"));
        assert!(!json.contains("resolved_spawn_snapshot"));
    }

    // -----------------------------------------------------------------------
    // Public API validation
    // -----------------------------------------------------------------------

    #[test]
    fn validate_public_api_rejects_inherit_parent() {
        let spec = HelperOptionsSpec {
            tooling: Some(ScheduleSpawnTooling::InheritParent {
                allow_overlay: None,
                deny_overlay: None,
            }),
            ..Default::default()
        };
        let result = spec.validate_public_api();
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .contains("requires parent agent context")
        );
    }

    #[test]
    fn validate_public_api_rejects_minimal() {
        let spec = HelperOptionsSpec {
            tooling: Some(ScheduleSpawnTooling::Minimal),
            ..Default::default()
        };
        assert!(spec.validate_public_api().is_err());
    }

    #[test]
    fn validate_public_api_rejects_resolved_snapshot() {
        let spec = HelperOptionsSpec {
            resolved_spawn_snapshot: Some(ResolvedSpawnSnapshot {
                tool_filter: meerkat_core::tool_scope::ToolFilter::Allow(ToolNameSet::from_iter([
                    "shell".to_string(),
                ])),
                tool_filter_witnesses: Default::default(),
            }),
            ..Default::default()
        };
        let result = spec.validate_public_api();
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .contains("trusted internal schedule state")
        );
    }

    #[test]
    fn validate_public_api_rejects_profile_overlay() {
        let spec = HelperOptionsSpec {
            tooling: Some(ScheduleSpawnTooling::Profile {
                name: "worker".into(),
                allow_overlay: Some(vec!["shell".into()]),
                deny_overlay: None,
            }),
            ..Default::default()
        };
        let result = spec.validate_public_api();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("profile overlays"));
    }

    #[test]
    fn validate_public_api_accepts_profile() {
        let spec = HelperOptionsSpec {
            tooling: Some(ScheduleSpawnTooling::Profile {
                name: "worker".into(),
                allow_overlay: None,
                deny_overlay: None,
            }),
            ..Default::default()
        };
        assert!(spec.validate_public_api().is_ok());
    }

    #[test]
    fn validate_public_api_accepts_none_tooling() {
        let spec = HelperOptionsSpec::default();
        assert!(spec.validate_public_api().is_ok());
    }

    // -----------------------------------------------------------------------
    // role_name backward compat
    // -----------------------------------------------------------------------

    #[test]
    fn helper_options_spec_role_name_is_canonical_field() {
        let spec = HelperOptionsSpec {
            role_name: Some("worker".into()),
            ..Default::default()
        };
        let json = serde_json::to_string(&spec).unwrap();
        assert!(json.contains("role_name"));
        assert!(!json.contains("profile_name"));
        let parsed: HelperOptionsSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.role_name, Some("worker".into()));
    }

    #[test]
    fn helper_options_spec_rejects_profile_name_alias() {
        let json = r#"{"profile_name":"legacy-worker"}"#;
        let err = serde_json::from_str::<HelperOptionsSpec>(json).unwrap_err();
        assert!(
            err.to_string().contains("profile_name"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn session_materialization_preload_skills_round_trip_typed_skill_keys() {
        let key = fixture_skill_key("email");
        let spec = fixture_session_materialization(vec![key.clone()]);

        let json = serde_json::to_value(&spec).unwrap();
        assert_eq!(
            json["preload_skills"][0]["source_uuid"],
            key.source_uuid.to_string()
        );
        assert_eq!(
            json["preload_skills"][0]["skill_name"],
            key.skill_name.to_string()
        );

        let parsed: SessionMaterializationSpec = serde_json::from_value(json).unwrap();
        assert_eq!(parsed, spec);
    }

    #[test]
    fn session_materialization_rejects_legacy_string_preload_skills() {
        let json = serde_json::json!({
            "model": "claude-sonnet-4-6",
            "preload_skills": ["email"]
        });

        let err = serde_json::from_value::<SessionMaterializationSpec>(json).unwrap_err();
        assert!(
            err.to_string().contains("invalid type: string"),
            "unexpected error: {err}"
        );
    }

    // -----------------------------------------------------------------------
    // Existing tests
    // -----------------------------------------------------------------------

    #[test]
    fn target_binding_round_trips_without_duplicate_type_fields() -> Result<(), String> {
        let binding = TargetBinding::session(SessionTargetBinding::ExactSession {
            session_id: SessionId::new(),
            action: ScheduledSessionAction::Prompt {
                prompt: ContentInput::from("scheduled hello"),
                system_prompt: None,
                render_metadata: None,
                skill_refs: Vec::new(),
                additional_instructions: Vec::new(),
            },
        });

        let value = serde_json::to_value(&binding).map_err(|error| error.to_string())?;
        assert_eq!(value["target_kind"], "session");
        assert_eq!(value["type"], "exact_session");

        let round_trip: TargetBinding =
            serde_json::from_value(value).map_err(|error| error.to_string())?;
        assert_eq!(round_trip, binding);
        Ok(())
    }
}
