//! CommsToolDispatcher - Implements AgentToolDispatcher for comms tools.
//!
//! This module provides a tool dispatcher that exposes comms tools (send_message,
//! send_request, send_response, list_peers) to the agent, optionally delegating
//! unknown tools to an inner dispatcher.

use std::sync::Arc;

use async_trait::async_trait;
use meerkat_comms::{Router, TrustedPeers};
use meerkat_comms_mcp::tools::{handle_tools_call, tools_list, ToolContext};
use meerkat_core::types::ToolDef;
use meerkat_core::AgentToolDispatcher;
use serde_json::Value;

/// Tool dispatcher that provides comms tools.
///
/// Can optionally delegate to an inner dispatcher for non-comms tools.
pub struct CommsToolDispatcher<T: AgentToolDispatcher = NoOpDispatcher> {
    /// Context for handling comms tool calls.
    tool_context: ToolContext,
    /// Optional inner dispatcher for non-comms tools.
    inner: Option<Arc<T>>,
}

impl CommsToolDispatcher<NoOpDispatcher> {
    /// Create a new comms tool dispatcher (comms tools only).
    pub fn new(router: Arc<Router>, trusted_peers: Arc<TrustedPeers>) -> Self {
        let tool_context = ToolContext {
            router,
            trusted_peers,
        };
        Self {
            tool_context,
            inner: None,
        }
    }
}

impl<T: AgentToolDispatcher> CommsToolDispatcher<T> {
    /// Create a new comms tool dispatcher with an inner dispatcher.
    ///
    /// Non-comms tools will be delegated to the inner dispatcher.
    pub fn with_inner(
        router: Arc<Router>,
        trusted_peers: Arc<TrustedPeers>,
        inner: Arc<T>,
    ) -> Self {
        let tool_context = ToolContext {
            router,
            trusted_peers,
        };
        Self {
            tool_context,
            inner: Some(inner),
        }
    }
}

/// A no-op dispatcher for when no inner dispatcher is needed.
pub struct NoOpDispatcher;

#[async_trait]
impl AgentToolDispatcher for NoOpDispatcher {
    fn tools(&self) -> Vec<ToolDef> {
        vec![]
    }

    async fn dispatch(&self, name: &str, _args: &Value) -> Result<String, String> {
        Err(format!("Unknown tool: {}", name))
    }
}

/// Names of comms tools.
const COMMS_TOOL_NAMES: &[&str] = &["send_message", "send_request", "send_response", "list_peers"];

#[async_trait]
impl<T: AgentToolDispatcher + 'static> AgentToolDispatcher for CommsToolDispatcher<T> {
    fn tools(&self) -> Vec<ToolDef> {
        // Convert mcp tool definitions to ToolDef
        let comms_tools: Vec<ToolDef> = tools_list()
            .into_iter()
            .map(|t| ToolDef {
                name: t["name"].as_str().unwrap_or_default().to_string(),
                description: t["description"].as_str().unwrap_or_default().to_string(),
                input_schema: t["inputSchema"].clone(),
            })
            .collect();

        // Combine with inner dispatcher tools if present
        match &self.inner {
            Some(inner) => {
                let mut all_tools = comms_tools;
                all_tools.extend(inner.tools());
                all_tools
            }
            None => comms_tools,
        }
    }

    async fn dispatch(&self, name: &str, args: &Value) -> Result<String, String> {
        // Check if this is a comms tool
        if COMMS_TOOL_NAMES.contains(&name) {
            let result = handle_tools_call(&self.tool_context, name, args).await?;
            Ok(serde_json::to_string(&result).unwrap_or_default())
        } else if let Some(inner) = &self.inner {
            // Delegate to inner dispatcher
            inner.dispatch(name, args).await
        } else {
            Err(format!("Unknown tool: {}", name))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meerkat_comms::{CommsConfig, Keypair, TrustedPeer, TrustedPeers};

    fn make_keypair() -> Keypair {
        Keypair::generate()
    }

    fn make_tool_context() -> (Arc<Router>, Arc<TrustedPeers>) {
        let keypair = make_keypair();
        let peer_keypair = make_keypair();
        let trusted_peers = TrustedPeers {
            peers: vec![TrustedPeer {
                name: "test-peer".to_string(),
                pubkey: peer_keypair.public_key(),
                addr: "tcp://127.0.0.1:4200".to_string(),
            }],
        };
        let trusted_peers = Arc::new(trusted_peers.clone());
        let router = Arc::new(Router::new(keypair, trusted_peers.as_ref().clone(), CommsConfig::default()));
        (router, trusted_peers)
    }

    #[test]
    fn test_comms_tool_dispatcher_trait() {
        let (router, trusted_peers) = make_tool_context();
        let dispatcher = CommsToolDispatcher::new(router, trusted_peers);

        // Verify it implements the trait by calling tools()
        let tools = dispatcher.tools();
        assert!(!tools.is_empty());
    }

    #[test]
    fn test_comms_tools_list() {
        let (router, trusted_peers) = make_tool_context();
        let dispatcher = CommsToolDispatcher::new(router, trusted_peers);

        let tools = dispatcher.tools();

        // Should have all 4 comms tools
        assert_eq!(tools.len(), 4);

        let tool_names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(tool_names.contains(&"send_message"));
        assert!(tool_names.contains(&"send_request"));
        assert!(tool_names.contains(&"send_response"));
        assert!(tool_names.contains(&"list_peers"));
    }

    #[tokio::test]
    async fn test_comms_tool_dispatch() {
        let (router, trusted_peers) = make_tool_context();
        let dispatcher = CommsToolDispatcher::new(router, trusted_peers);

        // list_peers should work without network
        let result = dispatcher
            .dispatch("list_peers", &serde_json::json!({}))
            .await;

        assert!(result.is_ok());
        let output = result.unwrap();
        assert!(output.contains("peers"));
        assert!(output.contains("test-peer"));
    }

    #[tokio::test]
    async fn test_comms_tool_dispatch_unknown() {
        let (router, trusted_peers) = make_tool_context();
        let dispatcher = CommsToolDispatcher::new(router, trusted_peers);

        let result = dispatcher
            .dispatch("unknown_tool", &serde_json::json!({}))
            .await;

        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Unknown tool"));
    }

    // Mock inner dispatcher for testing delegation
    struct MockInnerDispatcher {
        tools: Vec<ToolDef>,
    }

    impl MockInnerDispatcher {
        fn new() -> Self {
            Self {
                tools: vec![ToolDef {
                    name: "inner_tool".to_string(),
                    description: "A tool from inner dispatcher".to_string(),
                    input_schema: serde_json::json!({"type": "object"}),
                }],
            }
        }
    }

    #[async_trait]
    impl AgentToolDispatcher for MockInnerDispatcher {
        fn tools(&self) -> Vec<ToolDef> {
            self.tools.clone()
        }

        async fn dispatch(&self, name: &str, _args: &Value) -> Result<String, String> {
            if name == "inner_tool" {
                Ok("inner tool result".to_string())
            } else {
                Err(format!("Unknown tool: {}", name))
            }
        }
    }

    #[test]
    fn test_comms_tool_delegation_tools() {
        let (router, trusted_peers) = make_tool_context();
        let inner = Arc::new(MockInnerDispatcher::new());
        let dispatcher = CommsToolDispatcher::with_inner(router, trusted_peers, inner);

        let tools = dispatcher.tools();

        // Should have 4 comms tools + 1 inner tool
        assert_eq!(tools.len(), 5);

        let tool_names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(tool_names.contains(&"send_message"));
        assert!(tool_names.contains(&"inner_tool"));
    }

    #[tokio::test]
    async fn test_comms_tool_delegation_dispatch() {
        let (router, trusted_peers) = make_tool_context();
        let inner = Arc::new(MockInnerDispatcher::new());
        let dispatcher = CommsToolDispatcher::with_inner(router, trusted_peers, inner);

        // Comms tool should still work
        let comms_result = dispatcher
            .dispatch("list_peers", &serde_json::json!({}))
            .await;
        assert!(comms_result.is_ok());

        // Inner tool should be delegated
        let inner_result = dispatcher
            .dispatch("inner_tool", &serde_json::json!({}))
            .await;
        assert!(inner_result.is_ok());
        assert_eq!(inner_result.unwrap(), "inner tool result");

        // Unknown tool should fail
        let unknown_result = dispatcher
            .dispatch("totally_unknown", &serde_json::json!({}))
            .await;
        assert!(unknown_result.is_err());
    }
}
