use crate::RuntimeState;
use chrono::{DateTime, Utc};
use meerkat_core::comms::TrustedPeerSpec;
use meerkat_core::types::{ContentInput, HandlingMode};
use serde::{Deserialize, Serialize};

pub const INTENT_BIND_MEMBER: &str = "mob.runtime.bind_member";
pub const INTENT_AUTHORIZE_SUPERVISOR: &str = "mob.runtime.authorize_supervisor";
pub const INTENT_REVOKE_SUPERVISOR: &str = "mob.runtime.revoke_supervisor";
pub const INTENT_DELIVER_MEMBER_INPUT: &str = "mob.runtime.deliver_member_input";
pub const INTENT_INTERRUPT_MEMBER: &str = "mob.runtime.interrupt_member";
pub const INTENT_RETIRE_MEMBER: &str = "mob.runtime.retire_member";
pub const INTENT_DESTROY_MEMBER: &str = "mob.runtime.destroy_member";
pub const INTENT_OBSERVE_MEMBER: &str = "mob.runtime.observe_member";
pub const INTENT_WIRE_MEMBER: &str = "mob.runtime.wire_member";
pub const INTENT_UNWIRE_MEMBER: &str = "mob.runtime.unwire_member";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SupervisorAuthorityPayload {
    pub supervisor: TrustedPeerSpec,
    pub epoch: u64,
    pub protocol_version: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeBindPayload {
    pub supervisor: TrustedPeerSpec,
    pub epoch: u64,
    pub protocol_version: u32,
    pub expected_peer_id: String,
    pub expected_address: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct RuntimeBridgeCapabilities {
    pub deliver_member_input: bool,
    pub observe_member: bool,
    pub interrupt_member: bool,
    pub retire_member: bool,
    pub destroy_member: bool,
    pub wire_member: bool,
    pub unwire_member: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeBindResponse {
    pub peer_id: String,
    pub address: String,
    pub capabilities: RuntimeBridgeCapabilities,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeBridgeAck {
    pub ok: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeDeliveryPayload {
    pub supervisor: TrustedPeerSpec,
    pub epoch: u64,
    pub protocol_version: u32,
    pub input_id: String,
    pub content: ContentInput,
    pub handling_mode: HandlingMode,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum RuntimeDeliveryOutcome {
    Accepted,
    Deduplicated { existing_input_id: String },
    Rejected { reason: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeDeliveryResponse {
    pub input_id: String,
    pub canonical_input_id: Option<String>,
    pub outcome: RuntimeDeliveryOutcome,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimePeerWiringPayload {
    pub supervisor: TrustedPeerSpec,
    pub epoch: u64,
    pub protocol_version: u32,
    pub peer_spec: TrustedPeerSpec,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeRetireResponse {
    pub inputs_abandoned: usize,
    pub inputs_pending_drain: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeDestroyResponse {
    pub inputs_abandoned: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeObservationResponse {
    pub state: RuntimeState,
    pub current_run_id: Option<String>,
    pub observed_at: DateTime<Utc>,
}
