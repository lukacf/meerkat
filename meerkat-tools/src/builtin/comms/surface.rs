//! CommsToolSurface - adapts CommsToolSet to AgentToolDispatcher
//!
//! This module provides [`CommsToolSurface`], an adapter that exposes comms tools
//! as an [`AgentToolDispatcher`]. This allows comms tools to be composed with
//! other dispatchers via [`ToolGateway`].
//!
//! ## Dynamic Availability
//!
//! Comms tools should only be visible when there are peers configured. Use
//! [`CommsToolSurface::peer_availability`] to create an [`Availability`] predicate
//! that automatically shows/hides tools based on peer count.
//!
//! ```ignore
//! use meerkat_core::{ToolGatewayBuilder, Availability};
//! use meerkat_tools::CommsToolSurface;
//!
//! let trusted_peers = router.shared_trusted_peers();
//! let availability = CommsToolSurface::peer_availability(trusted_peers.clone());
//! let comms_surface = CommsToolSurface::new(router, trusted_peers);
//!
//! let gateway = ToolGatewayBuilder::new()
//!     .add_dispatcher(base_tools)
//!     .add_dispatcher_with_availability(Arc::new(comms_surface), availability)
//!     .build()?;
//! ```

use super::tool_set::CommsToolSet;
use crate::builtin::{BuiltinTool, BuiltinToolError};
use async_trait::async_trait;
use meerkat_comms::{Router, TrustedPeers};
use meerkat_core::error::ToolError;
use meerkat_core::gateway::Availability;
use meerkat_core::{AgentToolDispatcher, ToolDef};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Adapter that exposes comms tools as an [`AgentToolDispatcher`].
///
/// Use this with [`ToolGateway`] to compose comms tools with other dispatchers:
///
/// ```ignore
/// use meerkat_core::ToolGateway;
/// use meerkat_tools::builtin::comms::CommsToolSurface;
///
/// let comms_surface = CommsToolSurface::new(router, trusted_peers);
/// let gateway = ToolGateway::new(base_dispatcher, Some(Arc::new(comms_surface)))?;
/// ```
pub struct CommsToolSurface {
    tools: HashMap<String, Arc<dyn BuiltinTool>>,
    tool_defs: Vec<ToolDef>,
}

impl CommsToolSurface {
    /// Create a new comms tool surface from router and trusted peers.
    pub fn new(router: Arc<Router>, trusted_peers: Arc<RwLock<TrustedPeers>>) -> Self {
        let tool_set = CommsToolSet::new(router, trusted_peers);
        Self::from_tool_set(tool_set)
    }

    /// Create from an existing CommsToolSet.
    pub fn from_tool_set(tool_set: CommsToolSet) -> Self {
        let mut tools: HashMap<String, Arc<dyn BuiltinTool>> = HashMap::new();
        let mut tool_defs = Vec::new();

        // Register each tool
        let send_message: Arc<dyn BuiltinTool> = Arc::new(tool_set.send_message);
        tool_defs.push(send_message.def());
        tools.insert(send_message.name().to_string(), send_message);

        let send_request: Arc<dyn BuiltinTool> = Arc::new(tool_set.send_request);
        tool_defs.push(send_request.def());
        tools.insert(send_request.name().to_string(), send_request);

        let send_response: Arc<dyn BuiltinTool> = Arc::new(tool_set.send_response);
        tool_defs.push(send_response.def());
        tools.insert(send_response.name().to_string(), send_response);

        let list_peers: Arc<dyn BuiltinTool> = Arc::new(tool_set.list_peers);
        tool_defs.push(list_peers.def());
        tools.insert(list_peers.name().to_string(), list_peers);

        Self { tools, tool_defs }
    }

    /// Get usage instructions for comms tools.
    ///
    /// This should be appended to the system prompt when comms is enabled.
    pub fn usage_instructions() -> &'static str {
        CommsToolSet::usage_instructions()
    }

    /// Create an [`Availability`] predicate based on peer count.
    ///
    /// Returns an availability that is:
    /// - Available when `trusted_peers.has_peers()` returns true
    /// - Unavailable with reason "no peers configured" when no peers
    ///
    /// This uses `try_read()` to avoid blocking on the RwLock. If the lock
    /// cannot be acquired, it defaults to unavailable (conservative).
    ///
    /// # Example
    ///
    /// ```ignore
    /// let trusted_peers = router.shared_trusted_peers();
    /// let availability = CommsToolSurface::peer_availability(trusted_peers.clone());
    ///
    /// // Use with ToolGatewayBuilder
    /// let gateway = ToolGatewayBuilder::new()
    ///     .add_dispatcher(base_tools)
    ///     .add_dispatcher_with_availability(Arc::new(comms_surface), availability)
    ///     .build()?;
    /// ```
    pub fn peer_availability(trusted_peers: Arc<RwLock<TrustedPeers>>) -> Availability {
        Availability::when(
            "no peers configured",
            Arc::new(move || {
                trusted_peers
                    .try_read()
                    .map(|guard| guard.has_peers())
                    .unwrap_or(false) // Conservative: if can't get lock, assume unavailable
            }),
        )
    }
}

#[async_trait]
impl AgentToolDispatcher for CommsToolSurface {
    fn tools(&self) -> Vec<ToolDef> {
        self.tool_defs.clone()
    }

    async fn dispatch(&self, name: &str, args: &Value) -> Result<Value, ToolError> {
        let tool = self
            .tools
            .get(name)
            .ok_or_else(|| ToolError::not_found(name))?;

        tool.call(args.clone()).await.map_err(|e| match e {
            BuiltinToolError::InvalidArgs(msg) => ToolError::invalid_arguments(name, msg),
            BuiltinToolError::ExecutionFailed(msg) => ToolError::execution_failed(msg),
            BuiltinToolError::TaskError(te) => ToolError::execution_failed(te),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meerkat_comms::{CommsConfig, Keypair, TrustedPeer, TrustedPeers};

    fn create_test_surface() -> CommsToolSurface {
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

        CommsToolSurface::new(router, trusted_peers)
    }

    #[test]
    fn test_surface_has_all_tools() {
        let surface = create_test_surface();
        let tools = surface.tools();

        assert_eq!(tools.len(), 4);

        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"send_message"));
        assert!(names.contains(&"send_request"));
        assert!(names.contains(&"send_response"));
        assert!(names.contains(&"list_peers"));
    }

    #[tokio::test]
    async fn test_surface_dispatch_list_peers() {
        let surface = create_test_surface();
        let result = surface.dispatch("list_peers", &serde_json::json!({})).await;

        // Should succeed and return peer info
        assert!(result.is_ok());
        let response = result.unwrap();
        // The response contains a "peers" array
        assert!(response.get("peers").is_some());
    }

    #[tokio::test]
    async fn test_surface_dispatch_unknown_tool() {
        let surface = create_test_surface();
        let result = surface
            .dispatch("unknown_tool", &serde_json::json!({}))
            .await;

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ToolError::NotFound { .. }));
    }

    #[test]
    fn test_peer_availability_with_peers() {
        let peer_keypair = Keypair::generate();
        let trusted_peers = TrustedPeers {
            peers: vec![TrustedPeer {
                name: "test-peer".to_string(),
                pubkey: peer_keypair.public_key(),
                addr: "tcp://127.0.0.1:4200".to_string(),
            }],
        };
        let trusted_peers = Arc::new(RwLock::new(trusted_peers));

        let availability = CommsToolSurface::peer_availability(trusted_peers);
        assert!(availability.is_available());
        assert!(availability.unavailable_reason().is_none());
    }

    #[test]
    fn test_peer_availability_without_peers() {
        let trusted_peers = Arc::new(RwLock::new(TrustedPeers::new()));

        let availability = CommsToolSurface::peer_availability(trusted_peers);
        assert!(!availability.is_available());
        assert_eq!(
            availability.unavailable_reason(),
            Some("no peers configured")
        );
    }

    #[tokio::test]
    async fn test_peer_availability_dynamic() {
        let trusted_peers = Arc::new(RwLock::new(TrustedPeers::new()));
        let availability = CommsToolSurface::peer_availability(trusted_peers.clone());

        // Initially no peers
        assert!(!availability.is_available());

        // Add a peer
        let peer_keypair = Keypair::generate();
        {
            let mut peers = trusted_peers.write().await;
            peers.upsert(TrustedPeer {
                name: "new-peer".to_string(),
                pubkey: peer_keypair.public_key(),
                addr: "tcp://127.0.0.1:4200".to_string(),
            });
        }

        // Now available
        assert!(availability.is_available());

        // Remove the peer
        {
            let mut peers = trusted_peers.write().await;
            peers.remove(&peer_keypair.public_key());
        }

        // Unavailable again
        assert!(!availability.is_available());
    }
}
