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
/// Address query parameter carrying the one-time bind bootstrap token.
pub const SUPERVISOR_BRIDGE_BOOTSTRAP_TOKEN_PARAM: &str = "mob_supervisor_bootstrap_token";
/// Current supervisor bridge wire protocol version.
pub const SUPERVISOR_BRIDGE_PROTOCOL_VERSION: u32 = 1;

/// Remove the one-time bind bootstrap token from an advertised bridge address.
pub fn canonicalize_bridge_address(address: &str) -> String {
    let Some((base, query)) = address.split_once('?') else {
        return address.to_string();
    };
    let filtered: Vec<&str> = query
        .split('&')
        .filter(|pair| {
            pair.split_once('=')
                .map(|(key, _)| key != SUPERVISOR_BRIDGE_BOOTSTRAP_TOKEN_PARAM)
                .unwrap_or(true)
        })
        .filter(|pair| !pair.is_empty())
        .collect();
    if filtered.is_empty() {
        base.to_string()
    } else {
        format!("{base}?{}", filtered.join("&"))
    }
}

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

impl TryFrom<BridgePeerSpec> for meerkat_core::comms::TrustedPeerSpec {
    type Error = String;

    fn try_from(spec: BridgePeerSpec) -> Result<Self, Self::Error> {
        meerkat_core::comms::TrustedPeerSpec::new(spec.name, spec.peer_id, spec.address)
    }
}

impl TryFrom<&BridgePeerSpec> for meerkat_core::comms::TrustedPeerSpec {
    type Error = String;

    fn try_from(spec: &BridgePeerSpec) -> Result<Self, Self::Error> {
        meerkat_core::comms::TrustedPeerSpec::new(
            spec.name.clone(),
            spec.peer_id.clone(),
            spec.address.clone(),
        )
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
    pub bootstrap_token: String,
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
    /// Bridged runtime state for the observed member.
    pub state: BridgeMemberRuntimeState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accepting_inputs: Option<bool>,
    /// Current run identifier reported by the member runtime.
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
    /// Build an observation reply.
    pub fn new(
        state: BridgeMemberRuntimeState,
        accepting_inputs: Option<bool>,
        current_run_id: Option<String>,
        peer_connectivity: Option<BridgePeerConnectivity>,
        last_error: Option<String>,
        observed_at: String,
    ) -> Self {
        Self {
            state,
            accepting_inputs,
            current_run_id,
            peer_connectivity,
            last_error,
            observed_at,
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn observation_response_new_sets_observation_fields() {
        let response = BridgeObservationResponse::new(
            BridgeMemberRuntimeState::Running,
            Some(true),
            Some("run-1".to_string()),
            Some(BridgePeerConnectivity::Reachable),
            None,
            "2026-04-16T07:00:00Z".to_string(),
        );

        assert_eq!(response.state, BridgeMemberRuntimeState::Running);
        assert_eq!(response.current_run_id.as_deref(), Some("run-1"));
        assert_eq!(response.accepting_inputs, Some(true));
        assert_eq!(
            response.peer_connectivity,
            Some(BridgePeerConnectivity::Reachable)
        );
    }

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    fn sample_peer_spec() -> BridgePeerSpec {
        BridgePeerSpec {
            name: "member-a".to_string(),
            peer_id: "peer-abc".to_string(),
            address: "tcp://127.0.0.1:7000".to_string(),
        }
    }

    fn sample_supervisor_payload() -> BridgeSupervisorPayload {
        BridgeSupervisorPayload {
            supervisor: sample_peer_spec(),
            epoch: 42,
            protocol_version: SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
        }
    }

    fn sample_wiring_payload() -> BridgePeerWiringPayload {
        BridgePeerWiringPayload {
            supervisor: sample_peer_spec(),
            epoch: 7,
            protocol_version: SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
            peer_spec: BridgePeerSpec {
                name: "member-b".to_string(),
                peer_id: "peer-xyz".to_string(),
                address: "tcp://127.0.0.1:7001".to_string(),
            },
        }
    }

    // -----------------------------------------------------------------------
    // 1. BridgeCommand JSON round-trip — one subtest per variant.
    // -----------------------------------------------------------------------
    //
    // `BridgeCommand` does not derive `PartialEq`, so we assert round-trip
    // correctness by comparing `serde_json::Value` before and after a
    // decode/encode cycle. Equal JSON values imply semantic equality under
    // the wire contract.

    fn assert_command_round_trip(cmd: &BridgeCommand) {
        let value = serde_json::to_value(cmd).expect("serialize command");
        let decoded: BridgeCommand = serde_json::from_value(value.clone()).expect("decode command");
        let reencoded = serde_json::to_value(&decoded).expect("reserialize command");
        assert_eq!(
            value, reencoded,
            "BridgeCommand round-trip must preserve wire shape"
        );
    }

    #[test]
    fn bridge_command_bind_member_round_trip() {
        let cmd = BridgeCommand::BindMember(BridgeBindPayload {
            supervisor: sample_peer_spec(),
            epoch: 1,
            protocol_version: SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
            expected_peer_id: "peer-expected".to_string(),
            expected_address: "tcp://127.0.0.1:9000".to_string(),
            bootstrap_token: "bootstrap-secret".to_string(),
        });
        assert_command_round_trip(&cmd);
    }

    #[test]
    fn bridge_command_authorize_supervisor_round_trip() {
        let cmd = BridgeCommand::AuthorizeSupervisor(sample_supervisor_payload());
        assert_command_round_trip(&cmd);
    }

    #[test]
    fn bridge_command_revoke_supervisor_round_trip() {
        let cmd = BridgeCommand::RevokeSupervisor(sample_supervisor_payload());
        assert_command_round_trip(&cmd);
    }

    #[test]
    fn bridge_command_deliver_member_input_round_trip() {
        let cmd = BridgeCommand::DeliverMemberInput(BridgeDeliveryPayload {
            supervisor: sample_peer_spec(),
            epoch: 2,
            protocol_version: SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
            input_id: "input-1".to_string(),
            content: meerkat_core::types::ContentInput::Text("hello".to_string()),
            handling_mode: meerkat_core::types::HandlingMode::Queue,
        });
        assert_command_round_trip(&cmd);
    }

    #[test]
    fn bridge_command_observe_member_round_trip() {
        let cmd = BridgeCommand::ObserveMember(sample_supervisor_payload());
        assert_command_round_trip(&cmd);
    }

    #[test]
    fn bridge_command_interrupt_member_round_trip() {
        let cmd = BridgeCommand::InterruptMember(sample_supervisor_payload());
        assert_command_round_trip(&cmd);
    }

    #[test]
    fn bridge_command_retire_member_round_trip() {
        let cmd = BridgeCommand::RetireMember(sample_supervisor_payload());
        assert_command_round_trip(&cmd);
    }

    #[test]
    fn bridge_command_destroy_member_round_trip() {
        let cmd = BridgeCommand::DestroyMember(sample_supervisor_payload());
        assert_command_round_trip(&cmd);
    }

    #[test]
    fn bridge_command_wire_member_round_trip() {
        let cmd = BridgeCommand::WireMember(sample_wiring_payload());
        assert_command_round_trip(&cmd);
    }

    #[test]
    fn bridge_command_unwire_member_round_trip() {
        let cmd = BridgeCommand::UnwireMember(sample_wiring_payload());
        assert_command_round_trip(&cmd);
    }

    // -----------------------------------------------------------------------
    // 2. BridgeReply::Rejected round-trip with current shape.
    // -----------------------------------------------------------------------
    //
    // TODO(c4): Worker 1's H1 adds a typed `cause: BridgeRejectionCause` field
    // to this variant. This test pins the current `{ result, reason }` shape
    // so the c4 diff is a clean extension rather than a rewrite.

    #[test]
    fn bridge_reply_rejected_round_trip_current_shape() {
        let reply = BridgeReply::Rejected {
            reason: "epoch too low".to_string(),
        };
        let value = serde_json::to_value(&reply).expect("serialize rejected reply");
        assert_eq!(
            value,
            json!({
                "result": "rejected",
                "reason": "epoch too low",
            }),
            "current wire shape must tag rejection with `result` + `reason`"
        );
        let decoded: BridgeReply = serde_json::from_value(value.clone()).expect("decode reply");
        match decoded {
            BridgeReply::Rejected { ref reason } => {
                assert_eq!(reason, "epoch too low");
            }
            other => panic!("expected BridgeReply::Rejected, got {other:?}"),
        }
        let reencoded = serde_json::to_value(&decoded).expect("reserialize reply");
        assert_eq!(value, reencoded);
    }

    // -----------------------------------------------------------------------
    // 3. BridgeObservationResponse round-trip with all-present / all-absent
    //    optional fields. `skip_serializing_if = "Option::is_none"` must
    //    drop absent fields from the wire form.
    // -----------------------------------------------------------------------

    #[test]
    fn observation_response_round_trip_all_optional_present() {
        let response = BridgeObservationResponse {
            state: BridgeMemberRuntimeState::Running,
            accepting_inputs: Some(true),
            current_run_id: Some("run-42".to_string()),
            peer_connectivity: Some(BridgePeerConnectivity::Reachable),
            last_error: Some("transient network blip".to_string()),
            observed_at: "2026-04-16T07:00:00Z".to_string(),
        };
        let value = serde_json::to_value(&response).expect("serialize observation");
        assert_eq!(
            value,
            json!({
                "state": "running",
                "accepting_inputs": true,
                "current_run_id": "run-42",
                "peer_connectivity": "reachable",
                "last_error": "transient network blip",
                "observed_at": "2026-04-16T07:00:00Z",
            })
        );
        let decoded: BridgeObservationResponse =
            serde_json::from_value(value.clone()).expect("decode observation");
        assert_eq!(decoded, response);
        let reencoded = serde_json::to_value(&decoded).expect("reserialize observation");
        assert_eq!(value, reencoded);
    }

    #[test]
    fn observation_response_round_trip_all_optional_absent() {
        let response = BridgeObservationResponse {
            state: BridgeMemberRuntimeState::Idle,
            accepting_inputs: None,
            current_run_id: None,
            peer_connectivity: None,
            last_error: None,
            observed_at: "2026-04-16T07:01:00Z".to_string(),
        };
        let value = serde_json::to_value(&response).expect("serialize observation");
        assert_eq!(
            value,
            json!({
                "state": "idle",
                "observed_at": "2026-04-16T07:01:00Z",
            }),
            "absent optional fields must be skipped on the wire"
        );
        let decoded: BridgeObservationResponse =
            serde_json::from_value(value.clone()).expect("decode observation");
        assert_eq!(decoded, response);
        let reencoded = serde_json::to_value(&decoded).expect("reserialize observation");
        assert_eq!(value, reencoded);
    }

    // -----------------------------------------------------------------------
    // 4. BridgePeerConnectivity snake_case rename.
    // -----------------------------------------------------------------------

    #[test]
    fn peer_connectivity_serializes_as_snake_case() {
        for (variant, expected) in [
            (BridgePeerConnectivity::Reachable, "reachable"),
            (BridgePeerConnectivity::Unreachable, "unreachable"),
            (BridgePeerConnectivity::Unknown, "unknown"),
        ] {
            let value = serde_json::to_value(variant).expect("serialize connectivity");
            assert_eq!(
                value,
                json!(expected),
                "variant {variant:?} must serialize as {expected:?}"
            );
            let decoded: BridgePeerConnectivity =
                serde_json::from_value(value).expect("decode connectivity");
            assert_eq!(decoded, variant);
        }
    }

    // -----------------------------------------------------------------------
    // 5. BridgeMemberRuntimeState — every variant: Display + serde round-trip.
    // -----------------------------------------------------------------------

    #[test]
    fn member_runtime_state_display_and_round_trip_all_variants() {
        let cases: &[(BridgeMemberRuntimeState, &str)] = &[
            (BridgeMemberRuntimeState::Initializing, "initializing"),
            (BridgeMemberRuntimeState::Idle, "idle"),
            (BridgeMemberRuntimeState::Attached, "attached"),
            (BridgeMemberRuntimeState::Running, "running"),
            (BridgeMemberRuntimeState::Retired, "retired"),
            (BridgeMemberRuntimeState::Stopped, "stopped"),
            (BridgeMemberRuntimeState::Destroyed, "destroyed"),
        ];
        for (variant, expected) in cases {
            assert_eq!(
                variant.to_string(),
                *expected,
                "Display output must match snake_case wire form for {variant:?}"
            );
            let value = serde_json::to_value(variant).expect("serialize runtime state");
            assert_eq!(value, json!(expected));
            let decoded: BridgeMemberRuntimeState =
                serde_json::from_value(value).expect("decode runtime state");
            assert_eq!(decoded, *variant);
        }
    }
}
