//! Capability query response types.

use serde::{Deserialize, Serialize};

use super::{CapabilityId, CapabilityStatus};
use crate::version::ContractVersion;

/// A single capability's status in the current runtime.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CapabilityEntry {
    pub id: CapabilityId,
    pub description: String,
    pub status: CapabilityStatus,
}

/// Response to a `capabilities/get` request.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CapabilitiesResponse {
    pub contract_version: ContractVersion,
    pub capabilities: Vec<CapabilityEntry>,
}
