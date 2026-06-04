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

pub(crate) fn decode_bridge_success_payload(
    command: &BridgeCommand,
    value: serde_json::Value,
    context: &str,
) -> Result<serde_json::Value, MobError> {
    let reply: BridgeReply = serde_json::from_value(value).map_err(|error| {
        MobError::Internal(format!("failed to decode {context} bridge reply: {error}"))
    })?;
    let expected = expected_reply_kind(command);
    match reply {
        BridgeReply::BindMember(payload) if expected == ExpectedBridgeReply::BindMember => {
            serialize_success_payload(payload, context)
        }
        BridgeReply::Ack(payload) if expected == ExpectedBridgeReply::Ack => {
            serialize_success_payload(payload, context)
        }
        BridgeReply::Observation(payload) if expected == ExpectedBridgeReply::Observation => {
            serialize_success_payload(payload, context)
        }
        BridgeReply::Delivery(payload) if expected == ExpectedBridgeReply::Delivery => {
            serialize_success_payload(payload, context)
        }
        BridgeReply::Retire(payload) if expected == ExpectedBridgeReply::Retire => {
            serialize_success_payload(payload, context)
        }
        BridgeReply::Destroy(payload) if expected == ExpectedBridgeReply::Destroy => {
            serialize_success_payload(payload, context)
        }
        BridgeReply::Rejected { cause, reason } => {
            Err(MobError::from(BridgeRejectionReply::Typed {
                cause,
                reason,
            }))
        }
        other => Err(MobError::Internal(format!(
            "unexpected {context} bridge reply: expected {}, got {}",
            expected.as_str(),
            reply_kind(&other).as_str()
        ))),
    }
}

pub(crate) fn decode_bridge_ack(
    command: &BridgeCommand,
    value: serde_json::Value,
    context: &str,
) -> Result<BridgeAck, MobError> {
    let payload = decode_bridge_success_payload(command, value, context)?;
    serde_json::from_value(payload)
        .map_err(|error| MobError::Internal(format!("failed to decode {context} ack: {error}")))
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
