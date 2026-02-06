//! Comms tool surface - provides comms tools as a ToolDispatcher

use async_trait::async_trait;
use meerkat_comms::{Router, ToolContext, TrustedPeers, handle_tools_call, tools_list};
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

    /// Helper to create an availability predicate that only shows tools if peers exist
    pub fn peer_availability(trusted_peers: Arc<RwLock<TrustedPeers>>) -> Availability {
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
        "# Inter-agent Communication\n\nYou can communicate with other agents using these tools:\n\n- send_message: Send a simple text message\n- send_request: Send a request and wait for a response\n- send_response: Respond to a previous request\n- list_peers: See which agents are available to talk to\n\nAlways check list_peers first to see who is online."
    }
}

fn comms_tool_defs() -> Vec<Arc<ToolDef>> {
    tools_list()
        .into_iter()
        .map(|t| {
            Arc::new(ToolDef {
                name: t["name"].as_str().unwrap_or_default().to_string(),
                description: t["description"].as_str().unwrap_or_default().to_string(),
                input_schema: t["inputSchema"].clone(),
            })
        })
        .collect()
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
            thought_signature: None,
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
}
