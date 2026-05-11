//! Re-exports of supervisor bridge protocol types from `meerkat-contracts`.
//!
//! The canonical definitions live in `meerkat_contracts::wire::supervisor_bridge`.
//! This module provides convenience re-exports so mob-internal code can import
//! from a single location.

pub use meerkat_contracts::wire::supervisor_bridge::{
    BridgeAck, BridgeBindPayload, BridgeBindResponse, BridgeBootstrapToken, BridgeCapabilities,
    BridgeCommand, BridgeCommandDecodeError, BridgeDeliveryOutcome, BridgeDeliveryPayload,
    BridgeDeliveryRejectionCause, BridgeDeliveryResponse, BridgeDestroyResponse,
    BridgeHardCancelPayload, BridgeMemberRuntimeState, BridgeObservationResponse,
    BridgePeerConnectivity, BridgePeerSpec, BridgePeerWiringPayload, BridgeProtocolVersion,
    BridgeRejectionCause, BridgeRejectionClass, BridgeRejectionReply, BridgeReply,
    BridgeRetireResponse, BridgeSupervisorPayload, SUPERVISOR_BRIDGE_BOOTSTRAP_TOKEN_PARAM,
    SUPERVISOR_BRIDGE_CURRENT_PROTOCOL_VERSION, SUPERVISOR_BRIDGE_DEFAULT_PROTOCOL_VERSION,
    SUPERVISOR_BRIDGE_INTENT, SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
    SUPERVISOR_BRIDGE_SUPPORTED_PROTOCOL_VERSIONS, UnsupportedBridgeProtocolVersion,
    canonicalize_bridge_address, decode_bridge_command, decode_bridge_rejection_reply,
    decode_legacy_v1_raw_string_rejection, supervisor_bridge_current_protocol_version,
    supervisor_bridge_default_protocol_version, supervisor_bridge_protocol_version_supported,
    supervisor_bridge_supported_protocol_versions,
};

pub(crate) trait BridgeReplyPayload: Sized {
    fn from_bridge_reply(reply: BridgeReply) -> Result<Self, BridgeReply>;
}

macro_rules! bridge_reply_payload {
    ($ty:ty, $variant:ident) => {
        impl BridgeReplyPayload for $ty {
            fn from_bridge_reply(reply: BridgeReply) -> Result<Self, BridgeReply> {
                match reply {
                    BridgeReply::$variant(value) => Ok(value),
                    other => Err(other),
                }
            }
        }
    };
}

bridge_reply_payload!(BridgeBindResponse, BindMember);
bridge_reply_payload!(BridgeAck, Ack);
bridge_reply_payload!(BridgeObservationResponse, Observation);
bridge_reply_payload!(BridgeDeliveryResponse, Delivery);
bridge_reply_payload!(BridgeRetireResponse, Retire);
bridge_reply_payload!(BridgeDestroyResponse, Destroy);
