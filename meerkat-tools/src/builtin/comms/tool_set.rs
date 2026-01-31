//! CommsToolSet - groups all comms tools together

use super::tools::{
    CommsToolState, ListPeersTool, SendMessageTool, SendRequestTool, SendResponseTool,
};
use meerkat_comms::{Router, TrustedPeers};
use std::sync::Arc;
use tokio::sync::RwLock;

/// Collection of all comms tools
pub struct CommsToolSet {
    pub send_message: SendMessageTool,
    pub send_request: SendRequestTool,
    pub send_response: SendResponseTool,
    pub list_peers: ListPeersTool,
}

impl CommsToolSet {
    /// Create a new comms tool set
    pub fn new(router: Arc<Router>, trusted_peers: Arc<RwLock<TrustedPeers>>) -> Self {
        let state = CommsToolState::new(router, trusted_peers);
        Self {
            send_message: SendMessageTool::new(state.clone()),
            send_request: SendRequestTool::new(state.clone()),
            send_response: SendResponseTool::new(state.clone()),
            list_peers: ListPeersTool::new(state),
        }
    }

    /// Get tool names for collision detection
    pub fn tool_names(&self) -> Vec<&str> {
        vec![
            "send_message",
            "send_request",
            "send_response",
            "list_peers",
        ]
    }

    /// Usage instructions for comms tools
    pub fn usage_instructions() -> &'static str {
        r#"## Inter-Agent Communication Tools

You have access to comms tools for communicating with other agents:

- `send_message`: Send a message to a peer. The peer receives it in their inbox.
- `send_request`: Send a request to a peer with an intent and parameters.
- `send_response`: Reply to a request with a status and result.
- `list_peers`: List all trusted peers.

When communicating with other agents, identify them by their peer name (not pubkey)."#
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use meerkat_comms::{CommsConfig, Keypair, TrustedPeer, TrustedPeers};

    #[test]
    fn test_comms_tool_set_creation() {
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
        let (_, inbox_sender) = meerkat_comms::Inbox::new();
        let router = Arc::new(Router::with_shared_peers(
            keypair,
            trusted_peers.clone(),
            CommsConfig::default(),
            inbox_sender,
        ));

        let tool_set = CommsToolSet::new(router, trusted_peers);
        assert_eq!(tool_set.tool_names().len(), 4);
    }
}
