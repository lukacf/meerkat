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
///
/// The advertised method catalog describes the *surface* this build speaks,
/// not its per-instance runtime state — exactly as `mob/status` is advertised
/// with zero mobs present. `skills/list` is compiled unconditionally into the
/// router, so it is always part of the surface; calling it without a skill
/// runtime returns a runtime-state error ("skills not enabled"), the same shape
/// every other stateful method uses. Advertising it is therefore honest and
/// keeps the catalog stable (sibling capabilities gate on compile features,
/// never on runtime-instance presence).
pub fn handle_initialize(id: Option<RpcId>, runtime_available: bool) -> RpcResponse {
    let options = meerkat_contracts::RpcMethodCatalogOptions {
        runtime_available,
        mob_enabled: cfg!(feature = "mob"),
        mcp_enabled: cfg!(feature = "mcp"),
        comms_enabled: cfg!(feature = "comms"),
        blob_enabled: true,
        session_events_enabled: true,
        session_streams_enabled: true,
        schedule_enabled: cfg!(feature = "schedule"),
        workgraph_enabled: cfg!(feature = "workgraph"),
        skills_enabled: true,
    };
    let caps = ServerCapabilities {
        server_info: ServerInfo {
            name: "meerkat-rpc".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
        },
        contract_version: meerkat_contracts::ContractVersion::CURRENT.to_string(),
        methods: {
            let mut methods = vec!["initialized".to_string()];
            methods.extend(meerkat_contracts::rpc_method_names(options));
            methods
        },
    };
    RpcResponse::success(id, caps)
}
