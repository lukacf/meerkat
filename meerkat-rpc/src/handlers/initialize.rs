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
pub fn handle_initialize(id: Option<RpcId>) -> RpcResponse {
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
                "turn/start".to_string(),
                "turn/interrupt".to_string(),
                "config/get".to_string(),
                "config/set".to_string(),
                "config/patch".to_string(),
                "capabilities/get".to_string(),
            ];
            #[cfg(feature = "comms")]
            {
                m.push("comms/send".to_string());
                m.push("comms/peers".to_string());
                m.push("comms/stream_open".to_string());
                m.push("comms/stream_close".to_string());
            }
            m
        },
    };
    RpcResponse::success(id, caps)
}
