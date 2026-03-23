//! Runtime and input RPC wire contracts.

use serde::{Deserialize, Serialize};

/// Request payload for `runtime/state`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RuntimeStateParams {
    pub session_id: String,
}

/// Request payload for `runtime/accept`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RuntimeAcceptParams {
    pub session_id: String,
    pub input: serde_json::Value,
}

/// Request payload for `runtime/retire`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RuntimeRetireParams {
    pub session_id: String,
}

/// Request payload for `runtime/reset`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RuntimeResetParams {
    pub session_id: String,
}

/// Request payload for `input/state`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct InputStateParams {
    pub session_id: String,
    pub input_id: String,
}

/// Request payload for `input/list`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct InputListParams {
    pub session_id: String,
}

/// Public runtime state projection used by RPC surfaces.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum WireRuntimeState {
    Initializing,
    Idle,
    Attached,
    Running,
    Recovering,
    Retired,
    Stopped,
    Destroyed,
}

/// Response payload for `runtime/state`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RuntimeStateResult {
    pub state: WireRuntimeState,
}

/// Discriminator for `runtime/accept` responses.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum RuntimeAcceptOutcomeType {
    Accepted,
    Deduplicated,
    Rejected,
}

/// Public input lifecycle state projection used by RPC surfaces.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum WireInputLifecycleState {
    Accepted,
    Queued,
    Staged,
    Applied,
    AppliedPendingConsumption,
    Consumed,
    Superseded,
    Coalesced,
    Abandoned,
}

/// Input transition history entry for RPC-facing snapshots.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireInputStateHistoryEntry {
    pub timestamp: String,
    pub from: WireInputLifecycleState,
    pub to: WireInputLifecycleState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// RPC-facing input state snapshot.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireInputState {
    pub input_id: String,
    pub current_state: WireInputLifecycleState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policy: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terminal_outcome: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub durability: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
    #[serde(default)]
    pub attempt_count: u32,
    #[serde(default)]
    pub recovery_count: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub history: Vec<WireInputStateHistoryEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reconstruction_source: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub persisted_input: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_boundary_sequence: Option<u64>,
    pub created_at: String,
    pub updated_at: String,
}

/// Response payload for `runtime/accept`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RuntimeAcceptResult {
    pub outcome_type: RuntimeAcceptOutcomeType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub existing_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policy: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<WireInputState>,
}

/// Response payload for `runtime/retire`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RuntimeRetireResult {
    pub inputs_abandoned: usize,
    #[serde(default)]
    pub inputs_pending_drain: usize,
}

/// Response payload for `runtime/reset`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RuntimeResetResult {
    pub inputs_abandoned: usize,
}

/// Response payload for `input/list`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct InputListResult {
    pub input_ids: Vec<String>,
}
