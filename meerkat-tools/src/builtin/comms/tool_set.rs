//! Legacy comms tool set.

use super::tools::{CommsToolState, PeersTool};
use meerkat_comms::{Router, TrustedPeers};
use parking_lot::RwLock;
use std::sync::Arc;

/// Collection of legacy comms builtins.
///
/// Message/request/response tools are provided by `CommsToolSurface`, which
/// exposes the typed split tools from `meerkat-comms`.
pub struct CommsToolSet {
    pub peers: PeersTool,
}

impl CommsToolSet {
    /// Create a new comms tool set
    pub fn new(router: Arc<Router>, trusted_peers: Arc<RwLock<TrustedPeers>>) -> Self {
        let state = CommsToolState::new(router, trusted_peers);
        Self {
            peers: PeersTool::new(state),
        }
    }

    /// Get tool names for collision detection
    pub fn tool_names(&self) -> Vec<&str> {
        vec!["peers"]
    }

    /// Usage instructions for comms tools
    pub fn usage_instructions() -> &'static str {
        r"## Inter-Agent Communication Tools

You have access to comms tools for communicating with other agents:

- `peers`: List all visible peers.

Use `CommsToolSurface` for typed `send_message`, `send_request`, and `send_response` tools.
When communicating with other agents, identify them by their peer name (not pubkey)."
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

        let tool_set = CommsToolSet::new(router, trusted_peers);
        assert_eq!(tool_set.tool_names(), vec!["peers"]);
    }
}
