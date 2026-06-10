//! `initialize` / `initialized` handshake handlers.

use crate::protocol::{RpcId, RpcResponse};

/// Canonical wire contracts for the `initialize` handshake (K20).
pub use meerkat_contracts::wire::{ServerCapabilities, ServerInfo};

/// Handle the `initialize` method.
///
/// The advertised method catalog describes the *surface* this build speaks.
/// `skills_enabled` is derived from the actual skill-runtime capability seam
/// (`skill_runtime.is_some()`), not hardcoded: the deliver path
/// (`skills/list`) returns a "skills not enabled" error when no skill runtime
/// is bound, so advertising the method without a runtime would make
/// advertise != deliver. Threading the runtime presence keeps the catalog
/// honest — `skills/list` is advertised exactly when it can be served.
pub fn handle_initialize(
    id: Option<RpcId>,
    runtime_available: bool,
    skills_enabled: bool,
) -> RpcResponse {
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
        skills_enabled,
        live_webrtc_enabled: cfg!(feature = "live-webrtc"),
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
