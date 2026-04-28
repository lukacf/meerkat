//! Re-exports of supervisor bridge protocol types from `meerkat-contracts`.
//!
//! The canonical definitions live in `meerkat_contracts::wire::supervisor_bridge`.
//! This module provides convenience re-exports so mob-internal code can import
//! from a single location.

pub use meerkat_contracts::wire::supervisor_bridge::{
    BridgeAck, BridgeBindPayload, BridgeBindResponse, BridgeBootstrapToken, BridgeCapabilities,
    BridgeCommand, BridgeDeliveryOutcome, BridgeDeliveryPayload, BridgeDeliveryRejectionCause,
    BridgeDeliveryResponse, BridgeDestroyResponse, BridgeMemberRuntimeState,
    BridgeObservationResponse, BridgePeerConnectivity, BridgePeerSpec, BridgePeerWiringPayload,
    BridgeRejectionCause, BridgeRejectionClass, BridgeRejectionReply, BridgeReply,
    BridgeRetireResponse, BridgeSupervisorPayload, SUPERVISOR_BRIDGE_BOOTSTRAP_TOKEN_PARAM,
    SUPERVISOR_BRIDGE_INTENT, SUPERVISOR_BRIDGE_PROTOCOL_VERSION, canonicalize_bridge_address,
    decode_bridge_rejection_reply,
};
