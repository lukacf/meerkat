//! Re-exports of supervisor bridge protocol types from `meerkat-contracts`.
//!
//! The canonical definitions live in `meerkat_contracts::wire::supervisor_bridge`.
//! This module provides convenience re-exports so mob-internal code can import
//! from a single location.

use crate::MobError;

pub use meerkat_contracts::wire::supervisor_bridge::{
    BridgeAck, BridgeBindPayload, BridgeBindResponse, BridgeBootstrapToken, BridgeCapabilities,
    BridgeCommand, BridgeCommandDecodeError, BridgeDeliveryOutcome, BridgeDeliveryPayload,
    BridgeDeliveryRejectionCause, BridgeDeliveryResponse, BridgeDestroyResponse,
    BridgeHardCancelPayload, BridgeMemberRuntimeState, BridgeMobPeerOverlayHandoff,
    BridgeObservationResponse, BridgePeerConnectivity, BridgePeerSpec, BridgePeerWiringPayload,
    BridgeProtocolVersion, BridgeRejectionCause, BridgeRejectionReply, BridgeReply,
    BridgeRetireResponse, BridgeSupervisorPayload, SUPERVISOR_BRIDGE_BOOTSTRAP_TOKEN_PARAM,
    SUPERVISOR_BRIDGE_CURRENT_PROTOCOL_VERSION, SUPERVISOR_BRIDGE_DEFAULT_PROTOCOL_VERSION,
    SUPERVISOR_BRIDGE_INTENT, SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
    SUPERVISOR_BRIDGE_SUPPORTED_PROTOCOL_VERSIONS, UnsupportedBridgeProtocolVersion,
    canonicalize_bridge_address, decode_bridge_command, decode_bridge_rejection_reply,
    decode_legacy_v1_raw_string_rejection, supervisor_bridge_current_protocol_version,
    supervisor_bridge_default_protocol_version, supervisor_bridge_protocol_version_supported,
    supervisor_bridge_supported_protocol_versions,
};

/// Decode a wire `Value` into a typed [`BridgeReply`], validating that the
/// reply kind matches the kind expected for `command`.
///
/// A `BridgeReply::Rejected` is mapped to the typed rejection error, and a
/// reply whose kind does not match the command is reported as an internal
/// protocol mismatch. This is the single typed decode/compat seam; callers
/// that want a specific typed payload (e.g. [`decode_bridge_ack`]) match the
/// returned enum directly instead of re-serializing through a
/// `serde_json::Value`.
pub(crate) fn decode_bridge_reply(
    command: &BridgeCommand,
    value: serde_json::Value,
    context: &str,
) -> Result<BridgeReply, MobError> {
    let reply: BridgeReply = serde_json::from_value(value).map_err(|error| {
        MobError::Internal(format!("failed to decode {context} bridge reply: {error}"))
    })?;
    if let BridgeReply::Rejected { cause, reason } = reply {
        return Err(MobError::from(BridgeRejectionReply::Typed {
            cause,
            reason,
        }));
    }
    let expected = expected_reply_kind(command);
    if reply_kind(&reply) != expected {
        return Err(MobError::Internal(format!(
            "unexpected {context} bridge reply: expected {}, got {}",
            expected.as_str(),
            reply_kind(&reply).as_str()
        )));
    }
    Ok(reply)
}

pub(crate) fn decode_bridge_success_payload(
    command: &BridgeCommand,
    value: serde_json::Value,
    context: &str,
) -> Result<serde_json::Value, MobError> {
    match decode_bridge_reply(command, value, context)? {
        BridgeReply::BindMember(payload) => serialize_success_payload(payload, context),
        BridgeReply::Ack(payload) => serialize_success_payload(payload, context),
        BridgeReply::Observation(payload) => serialize_success_payload(payload, context),
        BridgeReply::Delivery(payload) => serialize_success_payload(payload, context),
        BridgeReply::Retire(payload) => serialize_success_payload(payload, context),
        BridgeReply::Destroy(payload) => serialize_success_payload(payload, context),
        // `decode_bridge_reply` already rejected `Rejected` and mismatched
        // kinds; any remaining (non-exhaustive future) variant is a protocol
        // mismatch surfaced as a typed fault rather than silently normalized.
        other => Err(MobError::Internal(format!(
            "unexpected {context} bridge reply: got {}",
            reply_kind(&other).as_str()
        ))),
    }
}

pub(crate) fn decode_bridge_ack(
    command: &BridgeCommand,
    value: serde_json::Value,
    context: &str,
) -> Result<BridgeAck, MobError> {
    // Match the typed reply directly: no serialize-to-`Value`-and-back
    // roundtrip. `decode_bridge_reply` has already enforced that the reply
    // kind matches the command (so a non-`Ack` reply here is a protocol bug).
    match decode_bridge_reply(command, value, context)? {
        BridgeReply::Ack(ack) => Ok(ack),
        other => Err(MobError::Internal(format!(
            "unexpected {context} bridge reply: expected ack, got {}",
            reply_kind(&other).as_str()
        ))),
    }
}

fn serialize_success_payload<T: serde::Serialize>(
    payload: T,
    context: &str,
) -> Result<serde_json::Value, MobError> {
    serde_json::to_value(payload).map_err(|error| {
        MobError::Internal(format!(
            "failed to normalize {context} bridge reply: {error}"
        ))
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ExpectedBridgeReply {
    BindMember,
    Ack,
    Observation,
    Delivery,
    Retire,
    Destroy,
    Rejected,
    Unknown,
}

impl ExpectedBridgeReply {
    const fn as_str(self) -> &'static str {
        match self {
            Self::BindMember => "bind_member",
            Self::Ack => "ack",
            Self::Observation => "observation",
            Self::Delivery => "delivery",
            Self::Retire => "retire",
            Self::Destroy => "destroy",
            Self::Rejected => "rejected",
            Self::Unknown => "unknown",
        }
    }
}

fn expected_reply_kind(command: &BridgeCommand) -> ExpectedBridgeReply {
    match command {
        BridgeCommand::BindMember(_) => ExpectedBridgeReply::BindMember,
        BridgeCommand::AuthorizeSupervisor(_)
        | BridgeCommand::RevokeSupervisor(_)
        | BridgeCommand::InterruptMember(_)
        | BridgeCommand::HardCancelMember(_)
        | BridgeCommand::WireMember(_)
        | BridgeCommand::UnwireMember(_) => ExpectedBridgeReply::Ack,
        BridgeCommand::DeliverMemberInput(_) => ExpectedBridgeReply::Delivery,
        BridgeCommand::ObserveMember(_) => ExpectedBridgeReply::Observation,
        BridgeCommand::RetireMember(_) => ExpectedBridgeReply::Retire,
        BridgeCommand::DestroyMember(_) => ExpectedBridgeReply::Destroy,
        _ => ExpectedBridgeReply::Unknown,
    }
}

fn reply_kind(reply: &BridgeReply) -> ExpectedBridgeReply {
    match reply {
        BridgeReply::BindMember(_) => ExpectedBridgeReply::BindMember,
        BridgeReply::Ack(_) => ExpectedBridgeReply::Ack,
        BridgeReply::Observation(_) => ExpectedBridgeReply::Observation,
        BridgeReply::Delivery(_) => ExpectedBridgeReply::Delivery,
        BridgeReply::Retire(_) => ExpectedBridgeReply::Retire,
        BridgeReply::Destroy(_) => ExpectedBridgeReply::Destroy,
        BridgeReply::Rejected { .. } => ExpectedBridgeReply::Rejected,
        _ => ExpectedBridgeReply::Unknown,
    }
}

#[cfg(test)]
mod tests {
    // All bridge wire types used below are re-exported at module scope via the
    // `pub use meerkat_contracts::wire::supervisor_bridge::{..}` block above.
    use super::*;

    fn interrupt_command() -> BridgeCommand {
        BridgeCommand::InterruptMember(BridgeSupervisorPayload {
            supervisor: BridgePeerSpec {
                name: "mob/supervisor/lead".to_string(),
                peer_id: "peer-lead".to_string(),
                address: "inproc://mob/supervisor/lead".to_string(),
                pubkey: [7u8; 32],
            },
            epoch: 1,
            protocol_version: BridgeProtocolVersion::default(),
        })
    }

    #[test]
    fn decode_bridge_ack_extracts_typed_ack_without_value_roundtrip() {
        let reply = BridgeReply::Ack(BridgeAck { ok: true });
        let value = serde_json::to_value(&reply).expect("serialize reply");
        let ack =
            decode_bridge_ack(&interrupt_command(), value, "interrupt").expect("ack should decode");
        assert!(ack.ok);
    }

    #[test]
    fn decode_bridge_reply_surfaces_typed_rejection() {
        let reply = BridgeReply::Rejected {
            cause: BridgeRejectionCause::StaleSupervisor,
            reason: "stale epoch".to_string(),
        };
        let value = serde_json::to_value(&reply).expect("serialize rejection");
        let error = decode_bridge_ack(&interrupt_command(), value, "interrupt")
            .expect_err("rejection must surface as a typed error");
        // The typed rejection cause must be preserved (not flattened to a
        // generic internal error).
        assert!(
            error.to_string().contains("stale epoch"),
            "rejection reason should be surfaced: {error}"
        );
    }

    #[test]
    fn decode_bridge_ack_rejects_kind_mismatch() {
        // An Observation reply for an Ack-expecting command is a protocol
        // mismatch surfaced as a typed fault, not silently coerced.
        let reply = BridgeReply::Observation(BridgeObservationResponse {
            state: BridgeMemberRuntimeState::Running,
            accepting_inputs: None,
            current_run_id: None,
            peer_connectivity: None,
            last_error: None,
            observed_at: "2026-04-17T00:00:00Z".to_string(),
        });
        let value = serde_json::to_value(&reply).expect("serialize observation");
        let error = decode_bridge_ack(&interrupt_command(), value, "interrupt")
            .expect_err("kind mismatch must surface as an error");
        assert!(
            error
                .to_string()
                .contains("unexpected interrupt bridge reply"),
            "mismatch should be reported as a protocol error: {error}"
        );
    }
}
