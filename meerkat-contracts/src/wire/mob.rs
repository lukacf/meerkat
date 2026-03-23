//! Mob wiring RPC wire contracts.

use serde::{Deserialize, Serialize};

/// Minimal trusted peer spec for public mob wiring surfaces.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireTrustedPeerSpec {
    pub name: String,
    pub peer_id: String,
    pub address: String,
}

/// Target for a mob wire/unwire call.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum MobPeerTarget {
    Local(String),
    External(WireTrustedPeerSpec),
}

/// Request payload for `mob/wire`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct MobWireParams {
    pub mob_id: String,
    pub local: String,
    pub target: MobPeerTarget,
}

/// Response payload for `mob/wire`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct MobWireResult {
    pub wired: bool,
}

/// Request payload for `mob/unwire`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct MobUnwireParams {
    pub mob_id: String,
    pub local: String,
    pub target: MobPeerTarget,
}

/// Response payload for `mob/unwire`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct MobUnwireResult {
    pub unwired: bool,
}
