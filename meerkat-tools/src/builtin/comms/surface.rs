//! Comms tool surface - provides comms tools as a ToolDispatcher

use async_trait::async_trait;
use meerkat_comms::{
    PubKey, Router, ToolContext, TrustedPeers, comms_tool_defs, handle_tools_call,
};
use meerkat_core::AgentToolDispatcher;
use meerkat_core::error::ToolError;
use meerkat_core::gateway::Availability;
use meerkat_core::types::{ToolCallView, ToolDef, ToolResult};
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Tool dispatcher that provides comms tools.
pub struct CommsToolSurface {
    tool_context: ToolContext,
    tool_defs: Arc<[Arc<ToolDef>]>,
}

impl CommsToolSurface {
    /// Create a new comms tool surface
    pub fn new(router: Arc<Router>, trusted_peers: Arc<RwLock<TrustedPeers>>) -> Self {
        let tool_context = ToolContext {
            router,
            trusted_peers,
        };
        let tool_defs: Arc<[Arc<ToolDef>]> = comms_tool_defs().into();
        Self {
            tool_context,
            tool_defs,
        }
    }

    /// Helper to create an availability predicate that only shows tools if peers exist.
    ///
    /// Peers are considered present only when trusted peers are configured.
    pub fn peer_availability(
        trusted_peers: Arc<RwLock<TrustedPeers>>,
        _self_pubkey: PubKey,
    ) -> Availability {
        Availability::when(
            "no peers configured",
            Arc::new(move || {
                trusted_peers
                    .try_read()
                    .map(|g| g.has_peers())
                    .unwrap_or(false)
            }),
        )
    }

    /// Usage instructions for comms tools to be added to the system prompt
    pub fn usage_instructions() -> &'static str {
        "# Inter-agent Communication\n\nYou can communicate with other agents using these tools:\n\n- send: Send a message, request, or response to a peer (use `kind` to select type)\n- peers: List all visible peers and their connection info\n\nAlways check peers first to see who is available."
    }
}

#[async_trait]
impl AgentToolDispatcher for CommsToolSurface {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::clone(&self.tool_defs)
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolResult, ToolError> {
        let is_comms = self.tool_defs.iter().any(|t| t.name == call.name);
        if !is_comms {
            return Err(ToolError::NotFound {
                name: call.name.to_string(),
            });
        }

        let args: Value = serde_json::from_str(call.args.get())
            .unwrap_or_else(|_| Value::String(call.args.get().to_string()));
        let result = handle_tools_call(&self.tool_context, call.name, &args)
            .await
            .map_err(|e| ToolError::ExecutionFailed { message: e })?;
        Ok(ToolResult {
            tool_use_id: call.id.to_string(),
            content: result.to_string(),
            is_error: false,
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use meerkat_comms::{CommsConfig, Keypair, TrustedPeer, TrustedPeers};

    fn make_keypair() -> Keypair {
        Keypair::generate()
    }

    fn make_tool_context() -> (Arc<Router>, Arc<RwLock<TrustedPeers>>) {
        let keypair = make_keypair();
        let peer_keypair = make_keypair();
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
        (router, trusted_peers)
    }

    #[test]
    fn test_comms_tool_surface_trait() {
        let (router, trusted_peers) = make_tool_context();
        let surface = CommsToolSurface::new(router, trusted_peers);
        let tools = surface.tools();
        assert!(!tools.is_empty());
    }

    #[tokio::test]
    async fn test_comms_tool_surface_dispatch_unknown() {
        let (router, trusted_peers) = make_tool_context();
        let surface = CommsToolSurface::new(router, trusted_peers);
        let args_raw = serde_json::value::RawValue::from_string(Value::Null.to_string()).unwrap();
        let call = ToolCallView {
            id: "test-1",
            name: "unknown",
            args: &args_raw,
        };
        let result = surface.dispatch(call).await;
        assert!(matches!(result, Err(ToolError::NotFound { .. })));
    }

    #[test]
    fn test_peer_availability_true_with_trusted_peers() {
        let self_key = make_keypair().public_key();
        let trusted_peers = Arc::new(RwLock::new(TrustedPeers {
            peers: vec![TrustedPeer {
                name: "trusted".to_string(),
                pubkey: make_keypair().public_key(),
                addr: "inproc://trusted".to_string(),
                meta: meerkat_comms::PeerMeta::default(),
            }],
        }));
        let availability = CommsToolSurface::peer_availability(trusted_peers, self_key);
        assert!(availability.is_available());
    }

    #[test]
    fn test_peer_availability_false_without_trusted_peers() {
        let self_key = make_keypair().public_key();
        let trusted_peers = Arc::new(RwLock::new(TrustedPeers::new()));
        let availability = CommsToolSurface::peer_availability(trusted_peers, self_key);
        assert!(!availability.is_available());
    }
}
