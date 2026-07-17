//! Re-exports of supervisor bridge protocol types from `meerkat-contracts`.
//!
//! The canonical definitions live in `meerkat_contracts::wire::supervisor_bridge`.
//! This module provides convenience re-exports so mob-internal code can import
//! from a single location.

use crate::MobError;
use hmac::{Hmac, Mac};
use sha2::Sha256;

pub use meerkat_contracts::wire::supervisor_bridge::{
    BridgeAck, BridgeBindPayload, BridgeBindResponse, BridgeBootstrapToken, BridgeCapabilities,
    BridgeCommand, BridgeCommandDecodeError, BridgeDeliveryOutcome, BridgeDeliveryPayload,
    BridgeDeliveryRejectionCause, BridgeDeliveryResponse, BridgeDestroyResponse, BridgeEventCursor,
    BridgeHardCancelPayload, BridgeHostBindPayload, BridgeHostBindResponse,
    BridgeHostBootstrapProof, BridgeHostCapabilityRequirements, BridgeHostMemberRecord,
    BridgeHostRebindPayload, BridgeHostReboundResponse, BridgeHostRevokePayload,
    BridgeHostRevokedResponse, BridgeHostRuntimeIncarnation, BridgeHostStatusPayload,
    BridgeHostStatusResponse, BridgeInterruptPayload, BridgeLiveChannelPayload,
    BridgeLiveControlOutcome, BridgeLiveControlPayload, BridgeLiveControlVerb,
    BridgeLiveControlledResponse, BridgeLiveOpenPayload, BridgeLiveOpenedResponse,
    BridgeLiveStatusPayload, BridgeMaterializePayload, BridgeMaterializedResponse,
    BridgeMemberEventsPage, BridgeMemberHistoryPage, BridgeMemberIncarnation,
    BridgeMemberOperatorPayload, BridgeMemberReleasedResponse, BridgeMemberRuntimeState,
    BridgeMobPeerOverlayHandoff, BridgeObservationResponse, BridgeOutboundTaintPayload,
    BridgeOutboundTaintTarget, BridgeOutcomeTracking, BridgePeerConnectivity, BridgePeerSpec,
    BridgePeerTrustPayload, BridgePeerWiringPayload, BridgePollEventsPayload,
    BridgeProtocolVersion, BridgeReadHistoryPayload, BridgeRejectionCause, BridgeRejectionReply,
    BridgeReleasePayload, BridgeReply, BridgeRetireResponse, BridgeSupervisorDelivery,
    BridgeSupervisorPayload, BridgeSupervisorRotationObservation, BridgeSupervisorRotationObserve,
    BridgeSupervisorRotationOperationReceipt, BridgeSupervisorRotationPendingPhase,
    BridgeSupervisorRotationRejectionCause, BridgeSupervisorRotationRejectionReceipt,
    BridgeSupervisorRotationState, BridgeSupervisorRotationSubmit,
    BridgeSupervisorRotationTargetReceipt, BridgeTrackedInputCancelOutcome,
    BridgeTrackedInputCancelPayload, BridgeTrackedInputCancelResponse, BridgeTurnCorrelation,
    BridgeTurnDirective, BridgeTurnOutcomeAck, BridgeTurnOutcomeRecord, MaterializeLaunchMode,
    MaterializeLaunchOutcome, MemberEventCursor, MemberOperatorOp, MemberOperatorOutcome,
    MemberOperatorReply, MemberOperatorSpawnSpec, MemberSessionDisposal, RuntimeReleaseCause,
    SUPERVISOR_BRIDGE_BOOTSTRAP_TOKEN_PARAM, SUPERVISOR_BRIDGE_CURRENT_PROTOCOL_VERSION,
    SUPERVISOR_BRIDGE_DEFAULT_PROTOCOL_VERSION, SUPERVISOR_BRIDGE_INTENT,
    SUPERVISOR_BRIDGE_PROTOCOL_VERSION, SUPERVISOR_BRIDGE_SUPPORTED_PROTOCOL_VERSIONS,
    SupervisorRotationOperationId, UnsupportedBridgeProtocolVersion, WireEventRow,
    WireFlowTurnOutcome, WireHostBindingDescriptor, WireHostBindingDescriptorKind, WireOpaqueJson,
    canonicalize_bridge_address, decode_bridge_command, decode_bridge_rejection_reply,
    decode_legacy_v1_raw_string_rejection, supervisor_bridge_current_protocol_version,
    supervisor_bridge_default_protocol_version, supervisor_bridge_protocol_version_supported,
    supervisor_bridge_supported_protocol_versions,
};

// Live-family wire vocabulary (phase 6b): the mob's identity-addressed live
// verbs speak CONTRACTS types end-to-end (one wire shape for local and
// remote by construction, DEC-P6B-C1) — re-exported through this seam so
// mob-internal and fixture consumers never import meerkat-contracts
// directly.
pub use meerkat_contracts::wire::{
    LiveCloseStatus, LiveOpenResult, LiveOpenTransport, RealtimeTurningMode, WireLiveAdapterStatus,
    WireLiveTransportBootstrap,
};

// Portable member-spec family (multi-host mobs V4). Mob-internal and test
// consumers import these through this seam only — the integration-tests
// consumer of the shared fixture file has no direct meerkat-contracts
// dependency.
pub use meerkat_contracts::wire::portable_member_spec_digest;
pub use meerkat_contracts::wire::{
    PortableDefinitionExtract, PortableMcpDecl, PortableMemberSpec, PortableProfile,
    PortableSkillSource, PortableSpawnOverlay, PortableSystemPrompt, PortableToolConfig,
    WireMobToolAuthorityContext, WireNonPortableResourceKind, WireResolvedToolAccessPolicy,
    WireSpawnContinuityIntent,
};
pub use meerkat_contracts::wire::{WireMobRuntimeMode, WireOpaqueJson as WirePortableOpaqueJson};

const HOST_BIND_BOOTSTRAP_PROOF_DOMAIN: &[u8] = b"meerkat-host-bind-bootstrap-proof-v1";

/// Derive the proof carried by a `BindHost` command from the raw descriptor
/// token and every authority-bearing field of that exact ceremony.
///
/// The raw one-time token remains out-of-band. The signed bridge transport is
/// not encrypted, so putting that bearer secret directly in the command
/// would let an active on-path observer race the intended supervisor. HMAC
/// turns the on-wire value into a request-bound proof instead: changing the
/// supervisor identity/route, target host, mob, epoch, binding generation, or
/// protocol version requires the raw token again.
pub(crate) fn derive_host_bind_bootstrap_proof(
    raw_token: &str,
    payload: &BridgeHostBindPayload,
) -> BridgeHostBootstrapProof {
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(raw_token.as_bytes()) else {
        // HMAC-SHA256 accepts arbitrary key lengths. Retain a fail-closed
        // branch rather than assuming that invariant at the authority seam.
        return BridgeHostBootstrapProof::new("");
    };
    mac.update(HOST_BIND_BOOTSTRAP_PROOF_DOMAIN);
    update_host_bind_proof_field(&mut mac, payload.supervisor.peer_id.as_bytes());
    update_host_bind_proof_field(&mut mac, &payload.supervisor.pubkey);
    update_host_bind_proof_field(&mut mac, payload.supervisor.address.as_bytes());
    update_host_bind_proof_field(&mut mac, payload.expected_host_peer_id.as_bytes());
    let canonical_host_address = canonicalize_bridge_address(&payload.expected_address);
    update_host_bind_proof_field(&mut mac, canonical_host_address.as_bytes());
    update_host_bind_proof_field(&mut mac, payload.mob_id.as_bytes());
    update_host_bind_proof_field(&mut mac, &payload.epoch.to_be_bytes());
    update_host_bind_proof_field(&mut mac, &payload.binding_generation.to_be_bytes());
    update_host_bind_proof_field(&mut mac, payload.protocol_version.to_string().as_bytes());
    let proof = mac.finalize().into_bytes();
    let mut encoded = String::with_capacity("hmac-sha256-v1:".len() + proof.len() * 2);
    encoded.push_str("hmac-sha256-v1:");
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for &byte in &proof {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    BridgeHostBootstrapProof::new(encoded)
}

fn update_host_bind_proof_field(mac: &mut Hmac<Sha256>, value: &[u8]) {
    // Length-prefix every field so concatenations are injective without
    // depending on a delimiter that a string-valued field could contain.
    mac.update(&(value.len() as u64).to_be_bytes());
    mac.update(value);
}

/// Replace the raw descriptor token in a prepared payload with its exact
/// request-bound proof immediately before wire serialization.
#[doc(hidden)]
pub fn seal_host_bind_bootstrap_proof(
    mut payload: BridgeHostBindPayload,
    raw_token: &BridgeBootstrapToken,
) -> BridgeHostBindPayload {
    payload.bootstrap_proof = derive_host_bind_bootstrap_proof(raw_token.as_str(), &payload);
    payload
}

/// Fold a wire `MemberSessionDisposal` into the machine disposal vocabulary
/// (single owner, exhaustive — a future wire variant is a compile error
/// here, never a silent wildcard).
///
/// §19.L4: `AlreadyArchived` folds to `Archived` — both mean the durable
/// terminal holds, so the machine never stores (and therefore never replays)
/// the `AlreadyArchived` distinction.
pub fn member_session_disposal_from_wire(
    wire: &MemberSessionDisposal,
) -> crate::machines::mob_machine::MemberSessionDisposal {
    use crate::machines::mob_machine::MemberSessionDisposal as Machine;
    match wire {
        MemberSessionDisposal::Archived | MemberSessionDisposal::AlreadyArchived => {
            Machine::Archived
        }
        MemberSessionDisposal::RuntimeReleasedOnly {
            cause: RuntimeReleaseCause::HostOwnedSession,
        } => Machine::RuntimeReleasedOnlyHostOwned,
        MemberSessionDisposal::RuntimeReleasedOnly {
            cause: RuntimeReleaseCause::NoDurableSessions,
        } => Machine::RuntimeReleasedOnlyNoDurableSessions,
    }
}

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
impl_from_bridge_reply!(
    BridgeTrackedInputCancelResponse,
    TrackedInputCancelled,
    "tracked_input_cancelled"
);
impl_from_bridge_reply!(BridgeRetireResponse, Retire, "retire");
impl_from_bridge_reply!(BridgeDestroyResponse, Destroy, "destroy");
impl_from_bridge_reply!(BridgeHostBindResponse, BindHost, "bind_host");
impl_from_bridge_reply!(BridgeHostReboundResponse, HostRebound, "host_rebound");
impl_from_bridge_reply!(BridgeHostRevokedResponse, HostRevoked, "host_revoked");
impl_from_bridge_reply!(
    BridgeMaterializedResponse,
    MemberMaterialized,
    "member_materialized"
);
impl_from_bridge_reply!(
    BridgeMemberReleasedResponse,
    MemberReleased,
    "member_released"
);
impl_from_bridge_reply!(BridgeHostStatusResponse, HostStatus, "host_status");
impl_from_bridge_reply!(
    MemberOperatorReply,
    MemberOperatorReply,
    "member_operator_reply"
);
impl_from_bridge_reply!(
    BridgeMemberHistoryPage,
    MemberHistoryPage,
    "member_history_page"
);
impl_from_bridge_reply!(
    BridgeMemberEventsPage,
    MemberEventsPage,
    "member_events_page"
);
// V4 member-addressed live family (phase 6b). The payload-struct pair rides
// the macro; the two INLINE-FIELD reply variants get manual extractions
// below (the macro only fits single-payload tuple variants).
impl_from_bridge_reply!(
    BridgeLiveOpenedResponse,
    MemberLiveChannelOpened,
    "member_live_channel_opened"
);
impl_from_bridge_reply!(
    BridgeLiveControlledResponse,
    MemberLiveChannelControlled,
    "member_live_channel_controlled"
);

impl FromBridgeReply for LiveCloseStatus {
    fn from_bridge_reply(reply: BridgeReply, context: &str) -> Result<Self, MobError> {
        match reply {
            BridgeReply::MemberLiveChannelClosed { status } => Ok(status),
            other => unexpected_reply("member_live_channel_closed", &other, context),
        }
    }
}

impl FromBridgeReply for super::member_live_proxy::MemberLiveStatusDomain {
    fn from_bridge_reply(reply: BridgeReply, context: &str) -> Result<Self, MobError> {
        match reply {
            BridgeReply::MemberLiveChannelStatusReport { channel_id, status } => {
                Ok(Self { channel_id, status })
            }
            other => unexpected_reply("member_live_channel_status_report", &other, context),
        }
    }
}
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
    TrackedInputCancelled,
    Retire,
    Destroy,
    BindHost,
    HostRebound,
    HostRevoked,
    MemberMaterialized,
    MemberReleased,
    HostStatus,
    MemberOperatorReply,
    MemberHistoryPage,
    MemberEventsPage,
    MemberLiveChannelOpened,
    MemberLiveChannelClosed,
    MemberLiveChannelStatusReport,
    MemberLiveChannelControlled,
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
            Self::TrackedInputCancelled => "tracked_input_cancelled",
            Self::Retire => "retire",
            Self::Destroy => "destroy",
            Self::BindHost => "bind_host",
            Self::HostRebound => "host_rebound",
            Self::HostRevoked => "host_revoked",
            Self::MemberMaterialized => "member_materialized",
            Self::MemberReleased => "member_released",
            Self::HostStatus => "host_status",
            Self::MemberOperatorReply => "member_operator_reply",
            Self::MemberHistoryPage => "member_history_page",
            Self::MemberEventsPage => "member_events_page",
            Self::MemberLiveChannelOpened => "member_live_channel_opened",
            Self::MemberLiveChannelClosed => "member_live_channel_closed",
            Self::MemberLiveChannelStatusReport => "member_live_channel_status_report",
            Self::MemberLiveChannelControlled => "member_live_channel_controlled",
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
        // The host-addressed V4 trust pair replies `Ack` (the V3
        // `WireMember` → `Ack` precedent; no trust-specific reply variant
        // exists on the wire). Without these rows a well-formed host `Ack`
        // would fail controlling-side decode as "expected unknown, got ack".
        | BridgeCommand::InstallPeerTrust(_)
        | BridgeCommand::RemovePeerTrust(_)
        | BridgeCommand::DeclareMemberOutboundTaint(_) => ExpectedBridgeReply::Ack,
        BridgeCommand::DeliverMemberInput(_) => ExpectedBridgeReply::Delivery,
        BridgeCommand::CancelTrackedMemberInput(_) => ExpectedBridgeReply::TrackedInputCancelled,
        BridgeCommand::ObserveMember(_) => ExpectedBridgeReply::Observation,
        BridgeCommand::RetireMember(_) => ExpectedBridgeReply::Retire,
        BridgeCommand::DestroyMember(_) => ExpectedBridgeReply::Destroy,
        BridgeCommand::BindHost(_) => ExpectedBridgeReply::BindHost,
        BridgeCommand::RebindHost(_) => ExpectedBridgeReply::HostRebound,
        BridgeCommand::RevokeHost(_) => ExpectedBridgeReply::HostRevoked,
        BridgeCommand::MaterializeMember(_) => ExpectedBridgeReply::MemberMaterialized,
        BridgeCommand::ReleaseMember(_) => ExpectedBridgeReply::MemberReleased,
        BridgeCommand::HostStatus(_) => ExpectedBridgeReply::HostStatus,
        BridgeCommand::MemberOperatorRequest(_) => ExpectedBridgeReply::MemberOperatorReply,
        // V4 member-addressed observation pair (phase 6).
        BridgeCommand::ReadMemberHistory(_) => ExpectedBridgeReply::MemberHistoryPage,
        BridgeCommand::PollMemberEvents(_) => ExpectedBridgeReply::MemberEventsPage,
        // V4 member-addressed live family (phase 6b): explicit arms — a
        // well-formed live reply must never fail decode as "expected
        // unknown".
        BridgeCommand::OpenMemberLiveChannel(_) => ExpectedBridgeReply::MemberLiveChannelOpened,
        BridgeCommand::CloseMemberLiveChannel(_) => ExpectedBridgeReply::MemberLiveChannelClosed,
        BridgeCommand::MemberLiveChannelStatus(_) => {
            ExpectedBridgeReply::MemberLiveChannelStatusReport
        }
        BridgeCommand::ControlMemberLiveChannel(_) => {
            ExpectedBridgeReply::MemberLiveChannelControlled
        }
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
        BridgeReply::TrackedInputCancelled(_) => ExpectedBridgeReply::TrackedInputCancelled,
        BridgeReply::Retire(_) => ExpectedBridgeReply::Retire,
        BridgeReply::Destroy(_) => ExpectedBridgeReply::Destroy,
        BridgeReply::BindHost(_) => ExpectedBridgeReply::BindHost,
        BridgeReply::HostRebound(_) => ExpectedBridgeReply::HostRebound,
        BridgeReply::HostRevoked(_) => ExpectedBridgeReply::HostRevoked,
        BridgeReply::MemberMaterialized(_) => ExpectedBridgeReply::MemberMaterialized,
        BridgeReply::MemberReleased(_) => ExpectedBridgeReply::MemberReleased,
        BridgeReply::HostStatus(_) => ExpectedBridgeReply::HostStatus,
        BridgeReply::MemberOperatorReply(_) => ExpectedBridgeReply::MemberOperatorReply,
        BridgeReply::MemberHistoryPage(_) => ExpectedBridgeReply::MemberHistoryPage,
        BridgeReply::MemberEventsPage(_) => ExpectedBridgeReply::MemberEventsPage,
        BridgeReply::MemberLiveChannelOpened(_) => ExpectedBridgeReply::MemberLiveChannelOpened,
        BridgeReply::MemberLiveChannelClosed { .. } => ExpectedBridgeReply::MemberLiveChannelClosed,
        BridgeReply::MemberLiveChannelStatusReport { .. } => {
            ExpectedBridgeReply::MemberLiveChannelStatusReport
        }
        BridgeReply::MemberLiveChannelControlled(_) => {
            ExpectedBridgeReply::MemberLiveChannelControlled
        }
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
        BridgeCommand::InterruptMember(BridgeInterruptPayload {
            supervisor: BridgePeerSpec {
                name: "mob/supervisor/lead".to_string(),
                peer_id: "peer-lead".to_string(),
                address: "inproc://mob/supervisor/lead".to_string(),
                pubkey: [7u8; 32],
            },
            epoch: 1,
            protocol_version: BridgeProtocolVersion::default(),
            expected_member: None,
        })
    }

    fn tracked_cancel_command() -> BridgeCommand {
        BridgeCommand::CancelTrackedMemberInput(BridgeTrackedInputCancelPayload {
            supervisor: BridgePeerSpec {
                name: "mob/supervisor/lead".to_string(),
                peer_id: "peer-lead".to_string(),
                address: "inproc://mob/supervisor/lead".to_string(),
                pubkey: [7u8; 32],
            },
            epoch: 1,
            protocol_version: BridgeProtocolVersion::V4,
            expected_member: BridgeMemberIncarnation {
                mob_id: "mob-1".to_string(),
                agent_identity: "worker".to_string(),
                host_id: "host".to_string(),
                binding_generation: 2,
                member_session_id: "session".to_string(),
                generation: 3,
                fence_token: 4,
            },
            input_id: "019f0000-0000-7000-8000-000000000001".to_string(),
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
    fn tracked_input_cancel_reply_has_its_own_exact_reply_kind() {
        let BridgeCommand::CancelTrackedMemberInput(payload) = tracked_cancel_command() else {
            unreachable!()
        };
        let response = BridgeTrackedInputCancelResponse {
            expected_member: payload.expected_member,
            input_id: payload.input_id,
            outcome: BridgeTrackedInputCancelOutcome::NoEffect,
        };
        let value = serde_json::to_value(BridgeReply::TrackedInputCancelled(response.clone()))
            .expect("serialize tracked cancellation reply");
        let decoded: BridgeTrackedInputCancelResponse =
            decode_bridge_payload(&tracked_cancel_command(), value, "tracked cancellation")
                .expect("typed tracked cancellation reply");
        assert_eq!(decoded, response);
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

    fn bind_host_payload() -> BridgeHostBindPayload {
        BridgeHostBindPayload {
            supervisor: BridgePeerSpec {
                name: "mob/supervisor/lead".to_string(),
                peer_id: "peer-lead".to_string(),
                address: "inproc://mob/supervisor/lead".to_string(),
                pubkey: [7u8; 32],
            },
            epoch: 3,
            binding_generation: 1,
            protocol_version: BridgeProtocolVersion::V4,
            expected_host_peer_id: "peer-host".to_string(),
            expected_address: "tcp://127.0.0.1:9000".to_string(),
            bootstrap_proof: BridgeHostBootstrapProof::new(""),
            mob_id: "mob-a".to_string(),
            required_capabilities: Default::default(),
        }
    }

    fn bind_host_command() -> BridgeCommand {
        let raw_token = BridgeBootstrapToken::new("raw-host-token-kept-out-of-band");
        BridgeCommand::BindHost(seal_host_bind_bootstrap_proof(
            bind_host_payload(),
            &raw_token,
        ))
    }

    #[test]
    fn host_bind_bootstrap_proof_is_deterministic_and_binds_authority_fields() {
        let raw_token = "raw-host-token-kept-out-of-band";
        let payload = bind_host_payload();
        let expected = derive_host_bind_bootstrap_proof(raw_token, &payload);
        assert_eq!(
            expected,
            derive_host_bind_bootstrap_proof(raw_token, &payload),
            "an exact retry must reproduce the identical wire proof"
        );

        let mut mutations: Vec<(&str, BridgeHostBindPayload)> = Vec::new();
        let mut changed = payload.clone();
        changed.supervisor.peer_id.push_str("-other");
        mutations.push(("supervisor peer id", changed));
        let mut changed = payload.clone();
        changed.supervisor.pubkey[0] ^= 1;
        mutations.push(("supervisor signing key", changed));
        let mut changed = payload.clone();
        changed.supervisor.address = "tcp://127.0.0.1:7001".to_string();
        mutations.push(("supervisor address", changed));
        let mut changed = payload.clone();
        changed.expected_host_peer_id.push_str("-other");
        mutations.push(("host peer id", changed));
        let mut changed = payload.clone();
        changed.expected_address = "tcp://127.0.0.1:9001".to_string();
        mutations.push(("host address", changed));
        let mut changed = payload.clone();
        changed.mob_id.push_str("-other");
        mutations.push(("mob id", changed));
        let mut changed = payload.clone();
        changed.epoch += 1;
        mutations.push(("epoch", changed));
        let mut changed = payload.clone();
        changed.binding_generation += 1;
        mutations.push(("binding generation", changed));
        let mut changed = payload;
        changed.protocol_version = BridgeProtocolVersion::V3;
        mutations.push(("protocol version", changed));

        for (field, changed) in mutations {
            assert_ne!(
                expected,
                derive_host_bind_bootstrap_proof(raw_token, &changed),
                "changing {field} must invalidate the request-bound proof"
            );
        }
    }

    #[test]
    fn host_bind_wire_and_debug_never_contain_raw_bootstrap_token() {
        let raw_token = "raw-host-token-kept-out-of-band";
        let command = BridgeCommand::BindHost(seal_host_bind_bootstrap_proof(
            bind_host_payload(),
            &BridgeBootstrapToken::new(raw_token),
        ));
        let encoded = serde_json::to_string(&command).expect("serialize sealed BindHost");
        let debug = format!("{command:?}");
        assert!(
            !encoded.contains(raw_token),
            "wire JSON leaked the bearer token"
        );
        assert!(!debug.contains(raw_token), "Debug leaked the bearer token");
        assert!(encoded.contains("hmac-sha256-v1:"));

        let decoded: BridgeCommand =
            serde_json::from_str(&encoded).expect("sealed BindHost round-trips");
        let BridgeCommand::BindHost(decoded) = decoded else {
            panic!("sealed command changed kind")
        };
        assert_ne!(decoded.bootstrap_proof.as_str(), raw_token);
    }

    fn rebind_host_command() -> BridgeCommand {
        BridgeCommand::RebindHost(BridgeHostRebindPayload {
            supervisor: BridgePeerSpec {
                name: "mob/supervisor/lead".to_string(),
                peer_id: "peer-lead-next".to_string(),
                address: "inproc://mob/supervisor/lead".to_string(),
                pubkey: [9u8; 32],
            },
            epoch: 4,
            binding_generation: 2,
            protocol_version: BridgeProtocolVersion::V4,
            mob_id: "mob-a".to_string(),
            required_capabilities: Default::default(),
        })
    }

    fn revoke_host_command() -> BridgeCommand {
        BridgeCommand::RevokeHost(BridgeHostRevokePayload {
            supervisor: BridgePeerSpec {
                name: "mob/supervisor/lead".to_string(),
                peer_id: "peer-lead".to_string(),
                address: "inproc://mob/supervisor/lead".to_string(),
                pubkey: [7u8; 32],
            },
            epoch: 3,
            binding_generation: 1,
            protocol_version: BridgeProtocolVersion::V4,
            mob_id: "mob-a".to_string(),
        })
    }

    #[test]
    fn decode_bridge_payload_extracts_typed_host_bind_response() {
        let reply = BridgeReply::BindHost(BridgeHostBindResponse {
            host_peer_id: "peer-host".to_string(),
            binding_generation: 1,
            address: "tcp://127.0.0.1:9000".to_string(),
            capabilities: BridgeCapabilities::default(),
            live_endpoint: Some("wss://127.0.0.1:9001/live".to_string()),
        });
        let value = serde_json::to_value(&reply).expect("serialize host bind reply");
        let bind: BridgeHostBindResponse =
            decode_bridge_payload(&bind_host_command(), value, "bind host")
                .expect("host bind payload should decode");
        assert_eq!(bind.host_peer_id, "peer-host");
        assert_eq!(
            bind.live_endpoint.as_deref(),
            Some("wss://127.0.0.1:9001/live")
        );
    }

    #[test]
    fn decode_bridge_payload_rejects_host_bind_kind_mismatch() {
        // An Ack for a BindHost command is a protocol mismatch surfaced as a
        // typed fault, never silently coerced into a synthetic bind success.
        let reply = BridgeReply::Ack(BridgeAck { ok: true });
        let value = serde_json::to_value(&reply).expect("serialize ack");
        let error = decode_bridge_payload::<BridgeHostBindResponse>(
            &bind_host_command(),
            value,
            "bind host",
        )
        .expect_err("kind mismatch must surface as an error");
        assert!(
            error
                .to_string()
                .contains("unexpected bind host bridge reply"),
            "mismatch should be reported as a protocol error: {error}"
        );
    }

    #[test]
    fn decode_bridge_payload_extracts_durable_host_revoke_receipt() {
        let reply = BridgeReply::HostRevoked(BridgeHostRevokedResponse {
            host_peer_id: "peer-host".to_string(),
            mob_id: "mob-a".to_string(),
            epoch: 3,
            binding_generation: 1,
            released_members: vec!["worker-1".to_string()],
        });
        let value = serde_json::to_value(&reply).expect("serialize host revoke reply");
        let revoked: BridgeHostRevokedResponse =
            decode_bridge_payload(&revoke_host_command(), value, "revoke host")
                .expect("host revoke receipt should decode");
        assert_eq!(revoked.host_peer_id, "peer-host");
        assert_eq!(revoked.released_members, vec!["worker-1"]);
    }

    #[test]
    fn decode_bridge_payload_extracts_typed_host_rebound_response() {
        let reply = BridgeReply::HostRebound(BridgeHostReboundResponse {
            host_peer_id: "peer-host".to_string(),
            binding_generation: 2,
            capabilities: BridgeCapabilities::default(),
            live_endpoint: None,
        });
        let value = serde_json::to_value(&reply).expect("serialize host rebound reply");
        let rebound: BridgeHostReboundResponse =
            decode_bridge_payload(&rebind_host_command(), value, "rebind host")
                .expect("host rebound payload should decode");
        assert_eq!(rebound.host_peer_id, "peer-host");
        assert!(rebound.live_endpoint.is_none());
    }

    #[test]
    fn decode_bridge_payload_surfaces_host_rebind_rejection() {
        let reply = BridgeReply::Rejected {
            cause: BridgeRejectionCause::StaleSupervisor,
            reason: "host recorded a newer supervisor epoch".to_string(),
        };
        let value = serde_json::to_value(&reply).expect("serialize rejection");
        let error = decode_bridge_payload::<BridgeHostReboundResponse>(
            &rebind_host_command(),
            value,
            "rebind host",
        )
        .expect_err("rejection must surface as a typed error");
        assert!(
            error
                .to_string()
                .contains("host recorded a newer supervisor epoch"),
            "rejection reason should be surfaced: {error}"
        );
    }

    fn sample_peer_spec() -> BridgePeerSpec {
        BridgePeerSpec {
            name: "mob/supervisor/lead".to_string(),
            peer_id: "peer-lead".to_string(),
            address: "inproc://mob/supervisor/lead".to_string(),
            pubkey: [7u8; 32],
        }
    }

    fn minimal_portable_spec() -> meerkat_contracts::wire::PortableMemberSpec {
        use meerkat_contracts::wire::{
            PortableDefinitionExtract, PortableMemberSpec, PortableProfile, PortableSpawnOverlay,
            PortableSystemPrompt, PortableToolConfig, WireSpawnContinuityIntent,
        };
        PortableMemberSpec {
            mob_id: "mob-a".to_string(),
            profile_name: "worker".to_string(),
            agent_identity: "worker-1".to_string(),
            profile: PortableProfile {
                model: "claude-fable-5".to_string(),
                provider: meerkat_core::Provider::Anthropic,
                self_hosted_server_id: None,
                image_generation_provider: None,
                auto_compact_threshold: None,
                resume_overrides: Vec::new(),
                skills: Vec::new(),
                tools: PortableToolConfig::default(),
                peer_description: String::new(),
                external_addressable: false,
                runtime_mode: meerkat_contracts::wire::WireMobRuntimeMode::default(),
                max_inline_peer_notifications: None,
                output_schema: None,
                provider_params: None,
            },
            definition_extract: PortableDefinitionExtract::default(),
            overlay: PortableSpawnOverlay {
                context: None,
                labels: None,
                additional_instructions: None,
                system_prompt: PortableSystemPrompt::Disable,
                tool_access_policy: None,
                mob_tool_authority_context: None,
                auth_binding: None,
                budget_limits: None,
                runtime_mode: meerkat_contracts::wire::WireMobRuntimeMode::default(),
                continuity_intent: WireSpawnContinuityIntent::default(),
            },
            required_env_keys: Vec::new(),
        }
    }

    fn materialize_command() -> BridgeCommand {
        BridgeCommand::MaterializeMember(Box::new(BridgeMaterializePayload {
            supervisor: sample_peer_spec(),
            epoch: 1,
            binding_generation: 1,
            protocol_version: BridgeProtocolVersion::V4,
            generation: 1,
            fence_token: 3,
            spec: minimal_portable_spec(),
            spec_digest: "d".repeat(64),
            launch: MaterializeLaunchMode::Fresh {},
        }))
    }

    #[test]
    fn decode_bridge_payload_extracts_typed_materialized_response() {
        let reply = BridgeReply::MemberMaterialized(BridgeMaterializedResponse {
            member_pubkey: "ed25519:AAAA".to_string(),
            member_peer_id: "peer-worker-1".to_string(),
            advertised_address: "tcp://10.0.0.2:7100".to_string(),
            session_id: "sess-1".to_string(),
            spec_digest: "d".repeat(64),
            engine_version: "0.7.22".to_string(),
            launch_outcome: MaterializeLaunchOutcome::Fresh,
            resolved_auth_binding: None,
        });
        let value = serde_json::to_value(&reply).expect("serialize materialized reply");
        let ack: BridgeMaterializedResponse =
            decode_bridge_payload(&materialize_command(), value, "materialize")
                .expect("materialized payload should decode");
        assert_eq!(ack.member_peer_id, "peer-worker-1");
        assert_eq!(ack.launch_outcome, MaterializeLaunchOutcome::Fresh);
    }

    #[test]
    fn decode_bridge_payload_rejects_materialize_kind_mismatch() {
        // An Ack for a MaterializeMember command is a protocol mismatch
        // surfaced as a typed fault, never coerced into a synthetic
        // materialization success.
        let reply = BridgeReply::Ack(BridgeAck { ok: true });
        let value = serde_json::to_value(&reply).expect("serialize ack");
        let error = decode_bridge_payload::<BridgeMaterializedResponse>(
            &materialize_command(),
            value,
            "materialize",
        )
        .expect_err("kind mismatch must surface as an error");
        assert!(
            error
                .to_string()
                .contains("unexpected materialize bridge reply"),
            "mismatch should be reported as a protocol error: {error}"
        );
    }

    #[test]
    fn decode_bridge_payload_extracts_typed_member_released_response() {
        let command = BridgeCommand::ReleaseMember(BridgeReleasePayload {
            supervisor: sample_peer_spec(),
            epoch: 1,
            binding_generation: 1,
            protocol_version: BridgeProtocolVersion::V4,
            mob_id: "mob-a".to_string(),
            agent_identity: "worker-1".to_string(),
            generation: 1,
            fence_token: 3,
        });
        let reply = BridgeReply::MemberReleased(BridgeMemberReleasedResponse {
            disposal: MemberSessionDisposal::RuntimeReleasedOnly {
                cause: RuntimeReleaseCause::NoDurableSessions,
            },
        });
        let value = serde_json::to_value(&reply).expect("serialize released reply");
        let released: BridgeMemberReleasedResponse =
            decode_bridge_payload(&command, value, "release").expect("release payload decodes");
        assert_eq!(
            released.disposal,
            MemberSessionDisposal::RuntimeReleasedOnly {
                cause: RuntimeReleaseCause::NoDurableSessions,
            }
        );
    }

    #[test]
    fn decode_bridge_payload_extracts_typed_host_status_response() {
        let command = BridgeCommand::HostStatus(BridgeHostStatusPayload {
            supervisor: sample_peer_spec(),
            epoch: 1,
            binding_generation: 1,
            protocol_version: BridgeProtocolVersion::V4,
            mob_id: "mob-a".to_string(),
        });
        let reply = BridgeReply::HostStatus(BridgeHostStatusResponse {
            runtime_incarnation: BridgeHostRuntimeIncarnation::new(),
            members: vec![BridgeHostMemberRecord {
                agent_identity: "worker-1".to_string(),
                generation: 1,
                fence_token: 3,
                session_id: "sess-1".to_string(),
                spec_digest: "d".repeat(64),
                healthy: true,
            }],
            capabilities: BridgeCapabilities::default(),
        });
        let value = serde_json::to_value(&reply).expect("serialize host status reply");
        let status: BridgeHostStatusResponse =
            decode_bridge_payload(&command, value, "host status")
                .expect("host status payload decodes");
        assert_eq!(status.members.len(), 1);
        assert_eq!(status.members[0].agent_identity, "worker-1");
    }

    #[test]
    fn decode_bridge_payload_extracts_typed_member_operator_reply() {
        let command = BridgeCommand::MemberOperatorRequest(BridgeMemberOperatorPayload {
            agent_identity: "worker-1".to_string(),
            requester_generation: 1,
            requester_fence_token: 3,
            requester_host_id: "host-a".to_string(),
            requester_host_binding_generation: 4,
            requester_member_session_id: "member-session-a".to_string(),
            request_id: "req-1".to_string(),
            op: MemberOperatorOp::ListMembers,
            protocol_version: BridgeProtocolVersion::V4,
        });
        let reply = BridgeReply::MemberOperatorReply(MemberOperatorReply {
            request_id: "req-1".to_string(),
            outcome: MemberOperatorOutcome::Completed {
                result: WireOpaqueJson::from_value(&serde_json::json!({ "members": [] })),
            },
        });
        let value = serde_json::to_value(&reply).expect("serialize operator reply");
        let operator: MemberOperatorReply =
            decode_bridge_payload(&command, value, "member operator")
                .expect("operator reply decodes");
        assert_eq!(operator.request_id, "req-1");
    }

    fn peer_trust_payload() -> BridgePeerTrustPayload {
        BridgePeerTrustPayload {
            supervisor: sample_peer_spec(),
            epoch: 1,
            binding_generation: 1,
            protocol_version: BridgeProtocolVersion::V4,
            mob_id: "mob-a".to_string(),
            agent_identity: "worker-1".to_string(),
            peer: BridgePeerSpec {
                name: "mob-a/worker/w-2".to_string(),
                peer_id: "peer-w-2".to_string(),
                address: "tcp://127.0.0.1:7101".to_string(),
                pubkey: [8u8; 32],
            },
        }
    }

    #[test]
    fn decode_bridge_ack_decodes_for_install_peer_trust() {
        let reply = BridgeReply::Ack(BridgeAck { ok: true });
        let value = serde_json::to_value(&reply).expect("serialize ack");
        let ack = decode_bridge_ack(
            &BridgeCommand::InstallPeerTrust(peer_trust_payload()),
            value,
            "install peer trust",
        )
        .expect("InstallPeerTrust expects an Ack reply");
        assert!(ack.ok);
    }

    #[test]
    fn decode_bridge_ack_decodes_for_remove_peer_trust() {
        let reply = BridgeReply::Ack(BridgeAck { ok: true });
        let value = serde_json::to_value(&reply).expect("serialize ack");
        let ack = decode_bridge_ack(
            &BridgeCommand::RemovePeerTrust(peer_trust_payload()),
            value,
            "remove peer trust",
        )
        .expect("RemovePeerTrust expects an Ack reply");
        assert!(ack.ok);
    }

    #[test]
    fn decode_peer_trust_reply_surfaces_typed_rejection() {
        let reply = BridgeReply::Rejected {
            cause: BridgeRejectionCause::Unavailable,
            reason: "member runtime not live on this host".to_string(),
        };
        let value = serde_json::to_value(&reply).expect("serialize rejection");
        let error = decode_bridge_ack(
            &BridgeCommand::InstallPeerTrust(peer_trust_payload()),
            value,
            "install peer trust",
        )
        .expect_err("rejection must surface as a typed error");
        assert!(
            error
                .to_string()
                .contains("member runtime not live on this host"),
            "rejection reason should be surfaced: {error}"
        );
    }

    #[test]
    fn decode_peer_trust_reply_rejects_kind_mismatch() {
        // An Observation for a trust command is a protocol mismatch surfaced
        // as a typed fault, never coerced into a synthetic ack.
        let reply = BridgeReply::Observation(BridgeObservationResponse {
            state: BridgeMemberRuntimeState::Running,
            accepting_inputs: None,
            current_run_id: None,
            peer_connectivity: None,
            last_error: None,
            observed_at: "2026-04-17T00:00:00Z".to_string(),
        });
        let value = serde_json::to_value(&reply).expect("serialize observation");
        let error = decode_bridge_ack(
            &BridgeCommand::RemovePeerTrust(peer_trust_payload()),
            value,
            "remove peer trust",
        )
        .expect_err("kind mismatch must surface as an error");
        assert!(
            error
                .to_string()
                .contains("unexpected remove peer trust bridge reply"),
            "mismatch should be reported as a protocol error: {error}"
        );
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
