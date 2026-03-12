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
            #[allow(unused_mut)]
            let mut m = vec![
                "initialize".to_string(),
                "initialized".to_string(),
                "session/create".to_string(),
                "session/list".to_string(),
                "session/read".to_string(),
                "session/archive".to_string(),
                "session/inject_context".to_string(),
                "session/stream_open".to_string(),
                "session/stream_close".to_string(),
                "turn/start".to_string(),
                "turn/interrupt".to_string(),
                "config/get".to_string(),
                "config/set".to_string(),
                "config/patch".to_string(),
                "capabilities/get".to_string(),
                "skills/list".to_string(),
                "skills/inspect".to_string(),
                "tools/register".to_string(),
            ];
            if runtime_available {
                m.push("runtime/state".to_string());
                m.push("runtime/accept".to_string());
                m.push("runtime/retire".to_string());
                m.push("runtime/reset".to_string());
                m.push("input/state".to_string());
                m.push("input/list".to_string());
            }
            #[cfg(feature = "mob")]
            {
                m.push("mob/prefabs".to_string());
                m.push("mob/create".to_string());
                m.push("mob/list".to_string());
                m.push("mob/status".to_string());
                m.push("mob/lifecycle".to_string());
                m.push("mob/spawn".to_string());
                m.push("mob/spawn_many".to_string());
                m.push("mob/retire".to_string());
                m.push("mob/respawn".to_string());
                m.push("mob/wire".to_string());
                m.push("mob/unwire".to_string());
                m.push("mob/members".to_string());
                m.push("mob/send".to_string());
                m.push("mob/events".to_string());
                m.push("mob/append_system_context".to_string());
                m.push("mob/flows".to_string());
                m.push("mob/flow_run".to_string());
                m.push("mob/flow_status".to_string());
                m.push("mob/flow_cancel".to_string());
                m.push("mob/tools".to_string());
                m.push("mob/call".to_string());
                m.push("mob/stream_open".to_string());
                m.push("mob/stream_close".to_string());
            }
            #[cfg(feature = "mcp")]
            {
                m.push("mcp/add".to_string());
                m.push("mcp/remove".to_string());
                m.push("mcp/reload".to_string());
            }
            #[cfg(feature = "comms")]
            {
                m.push("comms/send".to_string());
                m.push("comms/peers".to_string());
            }
            m
        },
    };
    RpcResponse::success(id, caps)
}
