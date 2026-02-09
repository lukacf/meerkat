//! Handler for `capabilities/get`.

use meerkat_contracts::{
    CapabilitiesResponse, CapabilityEntry, CapabilityStatus, ContractVersion, build_capabilities,
};

use crate::protocol::{RpcId, RpcResponse};

/// Handle `capabilities/get` — returns the runtime's capability set.
pub fn handle_get(id: Option<RpcId>) -> RpcResponse {
    let registrations = build_capabilities();
    let capabilities = registrations
        .into_iter()
        .map(|reg| CapabilityEntry {
            id: reg.id,
            description: reg.description.to_string(),
            status: CapabilityStatus::Available,
        })
        .collect();

    let response = CapabilitiesResponse {
        contract_version: ContractVersion::CURRENT,
        capabilities,
    };

    RpcResponse::success(id, &response)
}
