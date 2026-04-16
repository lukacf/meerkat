//! Supervisor bridge protocol wire types.
//!
//! Typed wire envelope for cross-machine bridge commands between a mob
//! supervisor and the runtime instances it manages. Both `meerkat-mob`
//! (sender) and `meerkat-runtime` (receiver) consume these types. Neither
//! crate depends on the other — the contracts crate owns the vocabulary.

use serde::{Deserialize, Serialize};

/// Comms intent used for all supervisor bridge commands.
///
/// The sender sets this as the request `intent`; the receiver checks for it
/// before attempting to deserialize `params` as [`BridgeCommand`].
pub const SUPERVISOR_BRIDGE_INTENT: &str = "supervisor.bridge";

// ---------------------------------------------------------------------------
// Command envelope
// ---------------------------------------------------------------------------

/// A typed command sent from a supervisor to a member runtime.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
#[non_exhaustive]
pub enum BridgeCommand {
    BindMember(BridgeBindPayload),
    AuthorizeSupervisor(BridgeSupervisorPayload),
    RevokeSupervisor(BridgeSupervisorPayload),
    DeliverMemberInput(BridgeDeliveryPayload),
    ObserveMember(BridgeSupervisorPayload),
    InterruptMember(BridgeSupervisorPayload),
    RetireMember(BridgeSupervisorPayload),
    DestroyMember(BridgeSupervisorPayload),
    WireMember(BridgePeerWiringPayload),
    UnwireMember(BridgePeerWiringPayload),
}

// ---------------------------------------------------------------------------
// Reply envelope
// ---------------------------------------------------------------------------

/// A typed reply from a member runtime back to the supervisor.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
#[non_exhaustive]
pub enum BridgeReply {
    BindMember(BridgeBindResponse),
    Ack(BridgeAck),
    Observation(BridgeObservationResponse),
    Delivery(BridgeDeliveryResponse),
    Retire(BridgeRetireResponse),
    Destroy(BridgeDestroyResponse),
    Rejected { reason: String },
}

// ---------------------------------------------------------------------------
// Member runtime state (wire projection)
// ---------------------------------------------------------------------------

/// Wire projection of a member's runtime state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum BridgeMemberRuntimeState {
    Initializing,
    Idle,
    Attached,
    Running,
    Retired,
    Stopped,
    Destroyed,
}

impl std::fmt::Display for BridgeMemberRuntimeState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Initializing => write!(f, "initializing"),
            Self::Idle => write!(f, "idle"),
            Self::Attached => write!(f, "attached"),
            Self::Running => write!(f, "running"),
            Self::Retired => write!(f, "retired"),
            Self::Stopped => write!(f, "stopped"),
            Self::Destroyed => write!(f, "destroyed"),
        }
    }
}

// ---------------------------------------------------------------------------
// Trusted peer spec (bridge-local, no meerkat-core dependency needed)
// ---------------------------------------------------------------------------

/// Minimal trusted peer identity for supervisor bridge wire messages.
///
/// Mirrors `meerkat_core::comms::TrustedPeerSpec` but is self-contained in
/// the contracts crate so neither sender nor receiver needs a cross-crate
/// dependency for deserialization.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BridgePeerSpec {
    pub name: String,
    pub peer_id: String,
    pub address: String,
}

/// Connectivity class observed for the bridged member runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum BridgePeerConnectivity {
    Reachable,
    Unreachable,
    Unknown,
}

impl From<meerkat_core::comms::TrustedPeerSpec> for BridgePeerSpec {
    fn from(spec: meerkat_core::comms::TrustedPeerSpec) -> Self {
        Self {
            name: spec.name,
            peer_id: spec.peer_id,
            address: spec.address,
        }
    }
}

impl From<BridgePeerSpec> for meerkat_core::comms::TrustedPeerSpec {
    fn from(spec: BridgePeerSpec) -> Self {
        // Bypass PeerName validation — the spec was already validated when
        // originally constructed. This conversion is infallible because both
        // types carry the same validated fields.
        Self {
            name: spec.name,
            peer_id: spec.peer_id,
            address: spec.address,
        }
    }
}

// ---------------------------------------------------------------------------
// Payload types
// ---------------------------------------------------------------------------

/// Supervisor authority credentials included in every bridge command.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BridgeSupervisorPayload {
    pub supervisor: BridgePeerSpec,
    pub epoch: u64,
    pub protocol_version: u32,
}

/// Bind a remote runtime to this supervisor.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BridgeBindPayload {
    pub supervisor: BridgePeerSpec,
    pub epoch: u64,
    pub protocol_version: u32,
    pub expected_peer_id: String,
    pub expected_address: String,
}

/// Capabilities advertised by a member runtime on bind.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct BridgeCapabilities {
    pub deliver_member_input: bool,
    pub observe_member: bool,
    pub interrupt_member: bool,
    pub retire_member: bool,
    pub destroy_member: bool,
    pub wire_member: bool,
    pub unwire_member: bool,
}

// ---------------------------------------------------------------------------
// Response types
// ---------------------------------------------------------------------------

/// Response to a bind command.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BridgeBindResponse {
    pub peer_id: String,
    pub address: String,
    pub capabilities: BridgeCapabilities,
}

/// Simple acknowledgment.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BridgeAck {
    pub ok: bool,
}

/// Deliver one logical input to a member.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BridgeDeliveryPayload {
    pub supervisor: BridgePeerSpec,
    pub epoch: u64,
    pub protocol_version: u32,
    pub input_id: String,
    pub content: meerkat_core::types::ContentInput,
    pub handling_mode: meerkat_core::types::HandlingMode,
}

/// Outcome of a delivery attempt.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum BridgeDeliveryOutcome {
    Accepted,
    Deduplicated { existing_input_id: String },
    Rejected { reason: String },
}

/// Full response to a delivery command.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BridgeDeliveryResponse {
    pub input_id: String,
    pub canonical_input_id: Option<String>,
    pub outcome: BridgeDeliveryOutcome,
}

/// Peer wiring command payload.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BridgePeerWiringPayload {
    pub supervisor: BridgePeerSpec,
    pub epoch: u64,
    pub protocol_version: u32,
    pub peer_spec: BridgePeerSpec,
}

/// Response to a retire command.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BridgeRetireResponse {
    pub inputs_abandoned: usize,
    pub inputs_pending_drain: usize,
}

/// Response to a destroy command.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BridgeDestroyResponse {
    pub inputs_abandoned: usize,
}

/// Response to an observe command.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BridgeObservationResponse {
    /// Backward-compatible mirror of [`Self::state`] kept so mixed-version
    /// peers can continue decoding observation replies during rollout.
    ///
    /// New consumers should read [`Self::state`].
    pub phase: BridgeMemberRuntimeState,
    /// Canonical bridged runtime state for the observed member.
    pub state: BridgeMemberRuntimeState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accepting_inputs: Option<bool>,
    /// Backward-compatible mirror of [`Self::current_run_id`].
    ///
    /// New consumers should read [`Self::current_run_id`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_run: Option<String>,
    /// Canonical current run identifier reported by the member runtime.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peer_connectivity: Option<BridgePeerConnectivity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    /// ISO 8601 timestamp.
    pub observed_at: String,
}

impl BridgeObservationResponse {
    /// Build an observation reply while keeping the legacy compatibility
    /// mirrors (`phase`, `current_run`) aligned with the canonical fields.
    pub fn new(
        state: BridgeMemberRuntimeState,
        accepting_inputs: Option<bool>,
        current_run_id: Option<String>,
        peer_connectivity: Option<BridgePeerConnectivity>,
        last_error: Option<String>,
        observed_at: String,
    ) -> Self {
        Self {
            phase: state,
            state,
            accepting_inputs,
            current_run: current_run_id.clone(),
            current_run_id,
            peer_connectivity,
            last_error,
            observed_at,
        }
    }

    /// Canonical runtime state to use when interpreting an observation.
    pub fn canonical_state(&self) -> BridgeMemberRuntimeState {
        self.state
    }

    /// Canonical current run identifier to use when interpreting an observation.
    pub fn canonical_current_run_id(&self) -> Option<&str> {
        self.current_run_id.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observation_response_keeps_compatibility_mirrors_aligned() {
        let response = BridgeObservationResponse::new(
            BridgeMemberRuntimeState::Running,
            Some(true),
            Some("run-1".to_string()),
            Some(BridgePeerConnectivity::Reachable),
            None,
            "2026-04-16T07:00:00Z".to_string(),
        );

        assert_eq!(response.phase, response.state);
        assert_eq!(
            response.current_run.as_deref(),
            response.current_run_id.as_deref()
        );
        assert_eq!(
            response.canonical_state(),
            BridgeMemberRuntimeState::Running
        );
        assert_eq!(response.canonical_current_run_id(), Some("run-1"));
    }
}
