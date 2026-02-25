//! Comms tool implementations: `send` and `peers`.

use crate::builtin::{BuiltinTool, BuiltinToolError};
use crate::schema::empty_object_schema;
#[cfg(target_arch = "wasm32")]
use crate::tokio;
use async_trait::async_trait;
use meerkat_comms::{Router, ToolContext, TrustedPeers, handle_tools_call, tools_list};
use meerkat_core::ToolDef;
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Shared state for all comms tools
#[derive(Clone)]
pub struct CommsToolState {
    tool_context: Arc<ToolContext>,
}

impl CommsToolState {
    pub fn new(router: Arc<Router>, trusted_peers: Arc<RwLock<TrustedPeers>>) -> Self {
        Self {
            tool_context: Arc::new(ToolContext {
                router,
                trusted_peers,
            }),
        }
    }
}

fn get_tool_def(name: &str) -> ToolDef {
    tools_list()
        .into_iter()
        .find(|t| t["name"].as_str() == Some(name))
        .map(|t| ToolDef {
            name: t["name"].as_str().unwrap_or_default().to_string(),
            description: t["description"].as_str().unwrap_or_default().to_string(),
            input_schema: t["inputSchema"].clone(),
        })
        .unwrap_or_else(|| ToolDef {
            name: name.to_string(),
            description: String::new(),
            input_schema: empty_object_schema(),
        })
}

/// Unified send tool — dispatches peer_message, peer_request, peer_response via `kind`.
pub struct SendTool {
    state: CommsToolState,
}

impl SendTool {
    pub fn new(state: CommsToolState) -> Self {
        Self { state }
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl BuiltinTool for SendTool {
    fn name(&self) -> &'static str {
        "send"
    }

    fn def(&self) -> ToolDef {
        get_tool_def("send")
    }

    fn default_enabled(&self) -> bool {
        true
    }

    async fn call(&self, args: Value) -> Result<Value, BuiltinToolError> {
        handle_tools_call(&self.state.tool_context, "send", &args)
            .await
            .map_err(BuiltinToolError::ExecutionFailed)
    }
}

/// Peers discovery tool.
pub struct PeersTool {
    state: CommsToolState,
}

impl PeersTool {
    pub fn new(state: CommsToolState) -> Self {
        Self { state }
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl BuiltinTool for PeersTool {
    fn name(&self) -> &'static str {
        "peers"
    }

    fn def(&self) -> ToolDef {
        get_tool_def("peers")
    }

    fn default_enabled(&self) -> bool {
        true
    }

    async fn call(&self, args: Value) -> Result<Value, BuiltinToolError> {
        handle_tools_call(&self.state.tool_context, "peers", &args)
            .await
            .map_err(BuiltinToolError::ExecutionFailed)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use meerkat_comms::{CommsConfig, Keypair, TrustedPeer, TrustedPeers};

    fn make_test_state() -> CommsToolState {
        let keypair = Keypair::generate();
        let peer_keypair = Keypair::generate();
        let trusted_peers = TrustedPeers {
            peers: vec![TrustedPeer {
                name: "test-peer".to_string(),
                pubkey: peer_keypair.public_key(),
                addr: "tcp://127.0.0.1:4200".to_string(),
                meta: meerkat_comms::PeerMeta::default(),
            }],
        };
        let trusted_peers = Arc::new(RwLock::new(trusted_peers));
        let (_, inbox_sender) = meerkat_comms::Inbox::new();
        let router = Arc::new(Router::with_shared_peers(
            keypair,
            trusted_peers.clone(),
            CommsConfig::default(),
            inbox_sender,
            true,
        ));
        CommsToolState::new(router, trusted_peers)
    }

    #[test]
    fn test_send_tool_name() {
        let state = make_test_state();
        let tool = SendTool::new(state);
        assert_eq!(tool.name(), "send");
    }

    #[test]
    fn test_peers_tool_name() {
        let state = make_test_state();
        let tool = PeersTool::new(state);
        assert_eq!(tool.name(), "peers");
    }

    #[tokio::test]
    async fn test_peers_tool_works() {
        let state = make_test_state();
        let tool = PeersTool::new(state);
        let result = tool.call(serde_json::json!({})).await;
        assert!(result.is_ok());
        let output = result.unwrap();
        assert!(output.get("peers").is_some());
    }
}
