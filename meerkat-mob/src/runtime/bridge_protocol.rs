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
    BridgeObservationResponse, BridgeOutboundTaintPayload, BridgePeerConnectivity, BridgePeerSpec,
    BridgePeerWiringPayload, BridgeProtocolVersion, BridgeRejectionCause, BridgeRejectionReply,
    BridgeReply, BridgeRetireResponse, BridgeSupervisorDelivery, BridgeSupervisorPayload,
    BridgeSupervisorRotationObservation, BridgeSupervisorRotationObserve,
    BridgeSupervisorRotationOperationReceipt, BridgeSupervisorRotationPendingPhase,
    BridgeSupervisorRotationRejectionCause, BridgeSupervisorRotationRejectionReceipt,
    BridgeSupervisorRotationState, BridgeSupervisorRotationSubmit,
    BridgeSupervisorRotationTargetReceipt, SUPERVISOR_BRIDGE_BOOTSTRAP_TOKEN_PARAM,
    SUPERVISOR_BRIDGE_CURRENT_PROTOCOL_VERSION, SUPERVISOR_BRIDGE_DEFAULT_PROTOCOL_VERSION,
    SUPERVISOR_BRIDGE_INTENT, SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
    SUPERVISOR_BRIDGE_SUPPORTED_PROTOCOL_VERSIONS, SupervisorRotationOperationId,
    UnsupportedBridgeProtocolVersion, canonicalize_bridge_address, decode_bridge_command,
    decode_bridge_rejection_reply, decode_legacy_v1_raw_string_rejection,
    supervisor_bridge_current_protocol_version, supervisor_bridge_default_protocol_version,
    supervisor_bridge_protocol_version_supported, supervisor_bridge_supported_protocol_versions,
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

/// A typed bridge-success payload that can be extracted directly from a decoded
/// [`BridgeReply`] without any `serde_json::Value` roundtrip.
///
/// Each success payload owns exactly one [`BridgeReply`] variant. Extraction
/// matches the typed enum once; a reply whose kind does not match the requested
/// payload is a protocol mismatch surfaced as a typed [`MobError`] (never
/// silently coerced). This replaces the former serialize-to-`Value`-then-
/// `from_value` roundtrip: there is one typed decode/compat seam
/// ([`decode_bridge_reply`]) and one typed extraction per payload.
pub(crate) trait FromBridgeReply: Sized {
    /// Extract this typed payload from a decoded [`BridgeReply`], or report the
    /// kind mismatch as a typed protocol fault.
    fn from_bridge_reply(reply: BridgeReply, context: &str) -> Result<Self, MobError>;
}

/// Report a `BridgeReply` whose kind does not match the requested payload.
fn unexpected_reply<T>(expected: &str, reply: &BridgeReply, context: &str) -> Result<T, MobError> {
    Err(MobError::Internal(format!(
        "unexpected {context} bridge reply: expected {expected}, got {}",
        reply_kind(reply).as_str()
    )))
}

macro_rules! impl_from_bridge_reply {
    ($payload:ty, $variant:ident, $expected:literal) => {
        impl FromBridgeReply for $payload {
            fn from_bridge_reply(reply: BridgeReply, context: &str) -> Result<Self, MobError> {
                match reply {
                    BridgeReply::$variant(payload) => Ok(payload),
                    other => unexpected_reply($expected, &other, context),
                }
            }
        }
    };
}

impl_from_bridge_reply!(BridgeBindResponse, BindMember, "bind_member");
impl_from_bridge_reply!(BridgeAck, Ack, "ack");
impl_from_bridge_reply!(BridgeObservationResponse, Observation, "observation");
impl_from_bridge_reply!(BridgeDeliveryResponse, Delivery, "delivery");
impl_from_bridge_reply!(BridgeRetireResponse, Retire, "retire");
impl_from_bridge_reply!(BridgeDestroyResponse, Destroy, "destroy");
impl_from_bridge_reply!(
    BridgeSupervisorRotationObservation,
    SupervisorRotation,
    "supervisor_rotation"
);

/// Decode a wire `Value` into the specific typed success payload `R`.
///
/// The typed compat check is performed once by [`decode_bridge_reply`]
/// (rejections become typed errors; reply-vs-command kind mismatches become
/// typed protocol faults); the matching success payload is then moved out of
/// the typed [`BridgeReply`] by [`FromBridgeReply`] with no `Value` roundtrip.
pub(crate) fn decode_bridge_payload<R: FromBridgeReply>(
    command: &BridgeCommand,
    value: serde_json::Value,
    context: &str,
) -> Result<R, MobError> {
    let reply = decode_bridge_reply(command, value, context)?;
    R::from_bridge_reply(reply, context)
}

pub(crate) fn decode_bridge_ack(
    command: &BridgeCommand,
    value: serde_json::Value,
    context: &str,
) -> Result<BridgeAck, MobError> {
    // Match the typed reply directly: no serialize-to-`Value`-and-back
    // roundtrip. `decode_bridge_reply` has already enforced that the reply
    // kind matches the command (so a non-`Ack` reply here is a protocol bug).
    decode_bridge_payload(command, value, context)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ExpectedBridgeReply {
    BindMember,
    Ack,
    Observation,
    Delivery,
    Retire,
    Destroy,
    SupervisorRotation,
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
            Self::SupervisorRotation => "supervisor_rotation",
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
        | BridgeCommand::UnwireMember(_)
        | BridgeCommand::DeclareMemberOutboundTaint(_) => ExpectedBridgeReply::Ack,
        BridgeCommand::DeliverMemberInput(_) => ExpectedBridgeReply::Delivery,
        BridgeCommand::ObserveMember(_) => ExpectedBridgeReply::Observation,
        BridgeCommand::RetireMember(_) => ExpectedBridgeReply::Retire,
        BridgeCommand::DestroyMember(_) => ExpectedBridgeReply::Destroy,
        BridgeCommand::ObserveSupervisorRotation(_) => ExpectedBridgeReply::SupervisorRotation,
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
        BridgeReply::SupervisorRotation(_) => ExpectedBridgeReply::SupervisorRotation,
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

    fn observe_command() -> BridgeCommand {
        BridgeCommand::ObserveMember(BridgeSupervisorPayload {
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
    fn decode_bridge_payload_moves_typed_payload_without_value_roundtrip() {
        // The success path now extracts the typed payload by moving it out of
        // the decoded `BridgeReply` (no serialize-to-`Value`-then-`from_value`
        // roundtrip).
        let reply = BridgeReply::Observation(BridgeObservationResponse {
            state: BridgeMemberRuntimeState::Running,
            accepting_inputs: Some(true),
            current_run_id: None,
            peer_connectivity: None,
            last_error: None,
            observed_at: "2026-04-17T00:00:00Z".to_string(),
        });
        let value = serde_json::to_value(&reply).expect("serialize observation");
        let observation: BridgeObservationResponse =
            decode_bridge_payload(&observe_command(), value, "observe")
                .expect("observation payload should decode");
        assert_eq!(observation.state, BridgeMemberRuntimeState::Running);
        assert_eq!(observation.accepting_inputs, Some(true));
    }

    #[test]
    fn decode_bridge_payload_surfaces_typed_rejection() {
        // A rejection must surface as a typed error regardless of the requested
        // payload type — never coerced into a synthetic success.
        let reply = BridgeReply::Rejected {
            cause: BridgeRejectionCause::StaleSupervisor,
            reason: "stale epoch".to_string(),
        };
        let value = serde_json::to_value(&reply).expect("serialize rejection");
        let error: MobError = decode_bridge_payload::<BridgeObservationResponse>(
            &observe_command(),
            value,
            "observe",
        )
        .expect_err("rejection must surface as a typed error");
        assert!(
            error.to_string().contains("stale epoch"),
            "rejection reason should be surfaced: {error}"
        );
    }
}
