//! CommsToolDispatcher - Implements AgentToolDispatcher for comms tools.
//!
//! This module provides a tool dispatcher that exposes comms tools (send_message,
//! send_request, send_response, list_peers) to the agent, optionally delegating
//! unknown tools to an inner dispatcher.

use std::sync::Arc;

use async_trait::async_trait;
use meerkat_comms::{Router, TrustedPeers};
use meerkat_comms_mcp::tools::{ToolContext, handle_tools_call, tools_list};
use meerkat_core::types::ToolDef;
use meerkat_core::{AgentToolDispatcher, CommsRuntime};
use serde_json::Value;
use tokio::sync::RwLock;

use crate::ToolError;

/// Tool dispatcher that provides comms tools.
///
/// Can optionally delegate to an inner dispatcher for non-comms tools.
pub struct CommsToolDispatcher<T: AgentToolDispatcher = NoOpDispatcher> {
    /// Context for handling comms tool calls.
    tool_context: ToolContext,
    /// Optional inner dispatcher for non-comms tools.
    inner: Option<Arc<T>>,
    tool_defs: Arc<[Arc<ToolDef>]>,
}

impl CommsToolDispatcher<NoOpDispatcher> {
    /// Create a new comms tool dispatcher (comms tools only).
    pub fn new(router: Arc<Router>, trusted_peers: Arc<RwLock<TrustedPeers>>) -> Self {
        let tool_context = ToolContext {
            router,
            trusted_peers,
        };
        let tool_defs: Arc<[Arc<ToolDef>]> = comms_tool_defs().into();
        Self {
            tool_context,
            inner: None,
            tool_defs,
        }
    }
}

impl<T: AgentToolDispatcher> CommsToolDispatcher<T> {
    /// Create a new comms tool dispatcher with an inner dispatcher.
    ///
    /// Non-comms tools will be delegated to the inner dispatcher.
    pub fn with_inner(
        router: Arc<Router>,
        trusted_peers: Arc<RwLock<TrustedPeers>>,
        inner: Arc<T>,
    ) -> Self {
        let tool_context = ToolContext {
            router,
            trusted_peers,
        };
        let mut tools = comms_tool_defs();
        tools.extend(inner.tools().iter().map(Arc::clone));
        let tool_defs: Arc<[Arc<ToolDef>]> = tools.into();
        Self {
            tool_context,
            inner: Some(inner),
            tool_defs,
        }
    }
}

/// A no-op dispatcher for when no inner dispatcher is needed.
pub struct NoOpDispatcher;

#[async_trait]
impl AgentToolDispatcher for NoOpDispatcher {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::from([])
    }

    async fn dispatch(&self, name: &str, _args: &Value) -> Result<Value, ToolError> {
        Err(ToolError::not_found(name))
    }
}

/// Names of comms tools.
const COMMS_TOOL_NAMES: &[&str] = &[
    "send_message",
    "send_request",
    "send_response",
    "list_peers",
];

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
impl<T: AgentToolDispatcher + 'static> AgentToolDispatcher for CommsToolDispatcher<T> {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::clone(&self.tool_defs)
    }

    async fn dispatch(&self, name: &str, args: &Value) -> Result<Value, ToolError> {
        // Check if this is a comms tool
        if COMMS_TOOL_NAMES.contains(&name) {
            let result = handle_tools_call(&self.tool_context, name, args)
                .await
                .map_err(ToolError::execution_failed)?;
            Ok(result)
        } else if let Some(inner) = &self.inner {
            // Delegate to inner dispatcher
            inner.dispatch(name, args).await
        } else {
            Err(ToolError::not_found(name))
        }
    }
}

/// Dynamic version of CommsToolDispatcher that works with trait objects.
///
/// This is useful when you need to wrap an `Arc<dyn AgentToolDispatcher>` at runtime,
/// such as when setting up sub-agents with comms tools.
pub struct DynCommsToolDispatcher {
    /// Context for handling comms tool calls.
    tool_context: ToolContext,
    /// Inner dispatcher for non-comms tools.
    inner: Arc<dyn AgentToolDispatcher>,
    tool_defs: Arc<[Arc<ToolDef>]>,
}

impl DynCommsToolDispatcher {
    /// Create a new dynamic comms tool dispatcher.
    pub fn new(
        router: Arc<Router>,
        trusted_peers: Arc<RwLock<TrustedPeers>>,
        inner: Arc<dyn AgentToolDispatcher>,
    ) -> Self {
        let tool_context = ToolContext {
            router,
            trusted_peers,
        };
        let mut tools = comms_tool_defs();
        tools.extend(inner.tools().iter().map(Arc::clone));
        let tool_defs: Arc<[Arc<ToolDef>]> = tools.into();
        Self {
            tool_context,
            inner,
            tool_defs,
        }
    }
}

#[async_trait]
impl AgentToolDispatcher for DynCommsToolDispatcher {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::clone(&self.tool_defs)
    }

    async fn dispatch(&self, name: &str, args: &Value) -> Result<Value, ToolError> {
        // Check if this is a comms tool
        if COMMS_TOOL_NAMES.contains(&name) {
            let result = handle_tools_call(&self.tool_context, name, args)
                .await
                .map_err(ToolError::execution_failed)?;
            Ok(result)
        } else {
            // Delegate to inner dispatcher
            self.inner.dispatch(name, args).await
        }
    }
}

/// Wrap a tool dispatcher with comms tools.
///
/// This is the preferred way to add comms capabilities to any agent. It works
/// uniformly for both main agents and sub-agents, treating comms as core
/// infrastructure rather than an optional feature.
///
/// The resulting dispatcher:
/// - Provides comms tools: send_message, send_request, send_response, list_peers
/// - Delegates all other tools to the inner dispatcher
pub fn wrap_with_comms(
    tools: Arc<dyn AgentToolDispatcher>,
    runtime: &CommsRuntime,
) -> Arc<dyn AgentToolDispatcher> {
    let router = runtime.router_arc();
    let trusted_peers = runtime.trusted_peers_shared();
    Arc::new(DynCommsToolDispatcher::new(router, trusted_peers, tools))
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
        let router = Arc::new(Router::with_shared_peers(
            keypair,
            trusted_peers.clone(),
            CommsConfig::default(),
        ));
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
        let output = serde_json::to_string(&result.unwrap()).unwrap();
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
        assert!(matches!(result.unwrap_err(), ToolError::NotFound { .. }));
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
                    input_schema: crate::empty_object_schema(),
                }],
            }
        }
    }

    #[async_trait]
    impl AgentToolDispatcher for MockInnerDispatcher {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            let tools: Vec<Arc<ToolDef>> = self.tools.iter().cloned().map(Arc::new).collect();
            tools.into()
        }

        async fn dispatch(&self, name: &str, _args: &Value) -> Result<Value, ToolError> {
            if name == "inner_tool" {
                Ok(Value::String("inner tool result".to_string()))
            } else {
                Err(ToolError::not_found(name))
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
        assert_eq!(
            inner_result.unwrap(),
            Value::String("inner tool result".to_string())
        );

        // Unknown tool should fail
        let unknown_result = dispatcher
            .dispatch("totally_unknown", &serde_json::json!({}))
            .await;
        assert!(unknown_result.is_err());
    }
}
