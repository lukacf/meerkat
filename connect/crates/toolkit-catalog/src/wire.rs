//! Lossless persisted-state encoding. This performs no lifecycle decisions.

use crate::{AcquisitionPhase, ToolkitAcquisitionMachineState};

impl serde::Serialize for AcquisitionPhase {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(match self {
            Self::Absent => "absent",
            Self::Investigating => "investigating",
            Self::Preparing => "preparing",
            Self::Ready => "ready",
            Self::Unavailable => "unavailable",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Expired => "expired",
        })
    }
}

impl<'de> serde::Deserialize<'de> for AcquisitionPhase {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = <String as serde::Deserialize>::deserialize(deserializer)?;
        match value.as_str() {
            "absent" => Ok(Self::Absent),
            "investigating" => Ok(Self::Investigating),
            "preparing" => Ok(Self::Preparing),
            "ready" => Ok(Self::Ready),
            "unavailable" => Ok(Self::Unavailable),
            "failed" => Ok(Self::Failed),
            "cancelled" => Ok(Self::Cancelled),
            "expired" => Ok(Self::Expired),
            other => Err(serde::de::Error::custom(format!(
                "invalid AcquisitionPhase `{other}`"
            ))),
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct AcquisitionStateWire {
    lifecycle_phase: AcquisitionPhase,
    operation_id: String,
    request_digest: String,
    authority_digest: String,
    continuation_id: String,
    deadline: u64,
    admitted_at: u64,
    last_observed_at: u64,
    revision: u64,
    selection_digest: String,
    decision_ref: String,
    selection_generation: u64,
    readiness_ref: String,
    readiness_digest: String,
    readiness_observed_at: u64,
    detail_ref: String,
    terminal_revision: u64,
    terminal_at: u64,
    continuation_receipt_ref: String,
}

impl From<&ToolkitAcquisitionMachineState> for AcquisitionStateWire {
    fn from(state: &ToolkitAcquisitionMachineState) -> Self {
        Self {
            lifecycle_phase: state.lifecycle_phase,
            operation_id: state.operation_id.clone(),
            request_digest: state.request_digest.clone(),
            authority_digest: state.authority_digest.clone(),
            continuation_id: state.continuation_id.clone(),
            deadline: state.deadline,
            admitted_at: state.admitted_at,
            last_observed_at: state.last_observed_at,
            revision: state.revision,
            selection_digest: state.selection_digest.clone(),
            decision_ref: state.decision_ref.clone(),
            selection_generation: state.selection_generation,
            readiness_ref: state.readiness_ref.clone(),
            readiness_digest: state.readiness_digest.clone(),
            readiness_observed_at: state.readiness_observed_at,
            detail_ref: state.detail_ref.clone(),
            terminal_revision: state.terminal_revision,
            terminal_at: state.terminal_at,
            continuation_receipt_ref: state.continuation_receipt_ref.clone(),
        }
    }
}

impl From<AcquisitionStateWire> for ToolkitAcquisitionMachineState {
    fn from(wire: AcquisitionStateWire) -> Self {
        Self {
            lifecycle_phase: wire.lifecycle_phase,
            operation_id: wire.operation_id,
            request_digest: wire.request_digest,
            authority_digest: wire.authority_digest,
            continuation_id: wire.continuation_id,
            deadline: wire.deadline,
            admitted_at: wire.admitted_at,
            last_observed_at: wire.last_observed_at,
            revision: wire.revision,
            selection_digest: wire.selection_digest,
            decision_ref: wire.decision_ref,
            selection_generation: wire.selection_generation,
            readiness_ref: wire.readiness_ref,
            readiness_digest: wire.readiness_digest,
            readiness_observed_at: wire.readiness_observed_at,
            detail_ref: wire.detail_ref,
            terminal_revision: wire.terminal_revision,
            terminal_at: wire.terminal_at,
            continuation_receipt_ref: wire.continuation_receipt_ref,
        }
    }
}

impl serde::Serialize for ToolkitAcquisitionMachineState {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        AcquisitionStateWire::from(self).serialize(serializer)
    }
}

impl<'de> serde::Deserialize<'de> for ToolkitAcquisitionMachineState {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        AcquisitionStateWire::deserialize(deserializer).map(Self::from)
    }
}
