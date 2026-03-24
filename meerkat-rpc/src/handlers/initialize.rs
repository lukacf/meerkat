//! `initialize` / `initialized` handshake handlers.

use serde::Serialize;

use crate::protocol::{RpcId, RpcResponse};

/// Capabilities returned by the server during the initialize handshake.
#[derive(Debug, Clone, Serialize)]
pub struct ServerCapabilities {
    pub server_info: ServerInfo,
    pub contract_version: String,
    pub methods: Vec<String>,
}

/// Basic server identity.
#[derive(Debug, Clone, Serialize)]
pub struct ServerInfo {
    pub name: String,
    pub version: String,
}

/// Handle the `initialize` method.
pub fn handle_initialize(id: Option<RpcId>, runtime_available: bool) -> RpcResponse {
    let caps = ServerCapabilities {
        server_info: ServerInfo {
            name: "meerkat-rpc".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
        },
        contract_version: meerkat_contracts::ContractVersion::CURRENT.to_string(),
        methods: {
            let mut methods = vec!["initialized".to_string()];
            methods.extend(meerkat_contracts::rpc_method_names(
                meerkat_contracts::RpcMethodCatalogOptions {
                    runtime_available,
                    mob_enabled: cfg!(feature = "mob"),
                    mcp_enabled: cfg!(feature = "mcp"),
                    comms_enabled: cfg!(feature = "comms"),
                },
            ));
            methods
        },
    };
    RpcResponse::success(id, caps)
}
