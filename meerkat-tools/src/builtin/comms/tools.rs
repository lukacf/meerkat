//! Individual comms tool implementations

use crate::builtin::{BuiltinTool, BuiltinToolError};
use crate::schema::empty_object_schema;
use async_trait::async_trait;
use meerkat_comms::{Router, TrustedPeers};
use meerkat_comms_mcp::tools::{ToolContext, handle_tools_call, tools_list};
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

// Helper to get tool def from tools_list by name
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

/// Send a message to a peer
pub struct SendMessageTool {
    state: CommsToolState,
}

impl SendMessageTool {
    pub fn new(state: CommsToolState) -> Self {
        Self { state }
    }
}

#[async_trait]
impl BuiltinTool for SendMessageTool {
    fn name(&self) -> &'static str {
        "send_message"
    }

    fn def(&self) -> ToolDef {
        get_tool_def("send_message")
    }

    fn default_enabled(&self) -> bool {
        // Comms tools are enabled when comms is configured
        true
    }

    async fn call(&self, args: Value) -> Result<Value, BuiltinToolError> {
        handle_tools_call(&self.state.tool_context, "send_message", &args)
            .await
            .map_err(BuiltinToolError::ExecutionFailed)
    }
}

/// Send a request to a peer
pub struct SendRequestTool {
    state: CommsToolState,
}

impl SendRequestTool {
    pub fn new(state: CommsToolState) -> Self {
        Self { state }
    }
}

#[async_trait]
impl BuiltinTool for SendRequestTool {
    fn name(&self) -> &'static str {
        "send_request"
    }

    fn def(&self) -> ToolDef {
        get_tool_def("send_request")
    }

    fn default_enabled(&self) -> bool {
        true
    }

    async fn call(&self, args: Value) -> Result<Value, BuiltinToolError> {
        handle_tools_call(&self.state.tool_context, "send_request", &args)
            .await
            .map_err(BuiltinToolError::ExecutionFailed)
    }
}

/// Send a response to a peer
pub struct SendResponseTool {
    state: CommsToolState,
}

impl SendResponseTool {
    pub fn new(state: CommsToolState) -> Self {
        Self { state }
    }
}

#[async_trait]
impl BuiltinTool for SendResponseTool {
    fn name(&self) -> &'static str {
        "send_response"
    }

    fn def(&self) -> ToolDef {
        get_tool_def("send_response")
    }

    fn default_enabled(&self) -> bool {
        true
    }

    async fn call(&self, args: Value) -> Result<Value, BuiltinToolError> {
        handle_tools_call(&self.state.tool_context, "send_response", &args)
            .await
            .map_err(BuiltinToolError::ExecutionFailed)
    }
}

/// List trusted peers
pub struct ListPeersTool {
    state: CommsToolState,
}

impl ListPeersTool {
    pub fn new(state: CommsToolState) -> Self {
        Self { state }
    }
}

#[async_trait]
impl BuiltinTool for ListPeersTool {
    fn name(&self) -> &'static str {
        "list_peers"
    }

    fn def(&self) -> ToolDef {
        get_tool_def("list_peers")
    }

    fn default_enabled(&self) -> bool {
        true
    }

    async fn call(&self, args: Value) -> Result<Value, BuiltinToolError> {
        handle_tools_call(&self.state.tool_context, "list_peers", &args)
            .await
            .map_err(BuiltinToolError::ExecutionFailed)
    }
}

#[cfg(test)]
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
            }],
        };
        let trusted_peers = Arc::new(RwLock::new(trusted_peers));
        let router = Arc::new(Router::with_shared_peers(
            keypair,
            trusted_peers.clone(),
            CommsConfig::default(),
        ));
        CommsToolState::new(router, trusted_peers)
    }

    #[test]
    fn test_send_message_tool_name() {
        let state = make_test_state();
        let tool = SendMessageTool::new(state);
        assert_eq!(tool.name(), "send_message");
    }

    #[test]
    fn test_send_request_tool_name() {
        let state = make_test_state();
        let tool = SendRequestTool::new(state);
        assert_eq!(tool.name(), "send_request");
    }

    #[test]
    fn test_send_response_tool_name() {
        let state = make_test_state();
        let tool = SendResponseTool::new(state);
        assert_eq!(tool.name(), "send_response");
    }

    #[test]
    fn test_list_peers_tool_name() {
        let state = make_test_state();
        let tool = ListPeersTool::new(state);
        assert_eq!(tool.name(), "list_peers");
    }

    #[tokio::test]
    async fn test_list_peers_tool_works() {
        let state = make_test_state();
        let tool = ListPeersTool::new(state);
        let result = tool.call(serde_json::json!({})).await;
        assert!(result.is_ok());
        let output = result.unwrap();
        assert!(output.get("peers").is_some());
    }
}
