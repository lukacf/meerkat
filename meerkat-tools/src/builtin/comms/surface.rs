//! Comms tool surface - provides comms tools as a ToolDispatcher

use async_trait::async_trait;
use meerkat_comms::{
    Router, RuntimeCommsCommandHandle, ToolContext, TrustedPeers, comms_tool_defs,
    comms_tool_unavailable_reason, handle_tools_call_with_context,
};
use meerkat_core::AgentToolDispatcher;
use meerkat_core::ToolDispatchContext;
use meerkat_core::error::ToolError;
use meerkat_core::types::{ToolCallView, ToolDef, ToolResult};
use meerkat_core::{
    ToolCallability, ToolCatalogCapabilities, ToolCatalogEntry, ToolUnavailableReason,
};
use parking_lot::RwLock;
use serde_json::Value;
use std::sync::Arc;

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
            runtime: None,
        };
        let tool_defs: Arc<[Arc<ToolDef>]> = comms_tool_defs().into();
        Self {
            tool_context,
            tool_defs,
        }
    }

    pub fn new_with_runtime(
        router: Arc<Router>,
        trusted_peers: Arc<RwLock<TrustedPeers>>,
        runtime: Arc<dyn meerkat_core::agent::CommsRuntime>,
    ) -> Self {
        let tool_context = ToolContext {
            router,
            trusted_peers,
            runtime: Some(RuntimeCommsCommandHandle::new(runtime)),
        };
        let tool_defs: Arc<[Arc<ToolDef>]> = comms_tool_defs().into();
        Self {
            tool_context,
            tool_defs,
        }
    }

    fn has_peers(&self) -> bool {
        self.tool_context
            .trusted_peers
            .try_read()
            .map(|peers| peers.has_peers())
            .unwrap_or(false)
    }

    fn callability_for_tool(&self, name: &str) -> ToolCallability {
        if self.has_peers() {
            comms_tool_unavailable_reason(&self.tool_context, name)
                .map_or_else(ToolCallability::callable, ToolCallability::unavailable)
        } else {
            ToolCallability::unavailable(ToolUnavailableReason::NoPeersConfigured)
        }
    }

    /// Usage instructions for comms tools to be added to the system prompt
    pub fn usage_instructions() -> &'static str {
        r#"# Inter-agent Communication

You can communicate with other agents using these tools:

- **send_message**: Send a message to a peer. Always include `handling_mode` — use `"steer"` for normal collaboration.
- **send_request**: Send a structured request (intent + params) and expect a correlated response. Always include `handling_mode`.
- **send_response**: Reply to a previous request using its ID.
- **peers**: List all visible peers. Always check peers first to see who is available.

Use `send_message` for ordinary collaboration. Use `send_request` only when you need structured intent/params with a correlated reply."#
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl AgentToolDispatcher for CommsToolSurface {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        self.tool_catalog()
            .iter()
            .filter(|entry| entry.currently_callable())
            .map(|entry| Arc::clone(&entry.tool))
            .collect::<Vec<_>>()
            .into()
    }

    fn tool_catalog_capabilities(&self) -> ToolCatalogCapabilities {
        ToolCatalogCapabilities {
            exact_catalog: true,
            may_require_catalog_control_plane: false,
        }
    }

    fn tool_catalog(&self) -> Arc<[ToolCatalogEntry]> {
        self.tool_defs
            .iter()
            .map(|tool| {
                ToolCatalogEntry::session_inline_with_callability(
                    Arc::clone(tool),
                    self.callability_for_tool(&tool.name),
                )
            })
            .collect::<Vec<_>>()
            .into()
    }

    async fn dispatch(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<meerkat_core::ops::ToolDispatchOutcome, ToolError> {
        self.dispatch_with_context(call, &ToolDispatchContext::default())
            .await
    }

    async fn dispatch_with_context(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
    ) -> Result<meerkat_core::ops::ToolDispatchOutcome, ToolError> {
        let is_comms = self.tool_defs.iter().any(|t| t.name == call.name);
        if !is_comms {
            return Err(ToolError::NotFound {
                name: call.name.to_string(),
            });
        }
        if let Some(reason) = self.callability_for_tool(call.name).unavailable_reason() {
            return Err(ToolError::unavailable(call.name, reason));
        }

        let args: Value = serde_json::from_str(call.args.get())
            .unwrap_or_else(|_| Value::String(call.args.get().to_string()));
        let result = handle_tools_call_with_context(&self.tool_context, call.name, &args, context)
            .await
            .map_err(|e| ToolError::ExecutionFailed { message: e })?;
        Ok(ToolResult::new(call.id.to_string(), result.to_string(), false).into())
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
    fn comms_surface_reports_exact_catalog_support() {
        let (router, trusted_peers) = make_tool_context();
        let surface = CommsToolSurface::new(router, trusted_peers);
        assert!(surface.tool_catalog_capabilities().exact_catalog);
        let catalog = surface.tool_catalog();
        assert_eq!(catalog.len(), surface.tool_defs.len());
        assert!(catalog.iter().any(|entry| entry.tool.name == "send_request"
            && entry.callability.unavailable_reason()
                == Some(ToolUnavailableReason::RuntimeCommandAuthorityUnavailable)));
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
    fn test_comms_callability_true_with_trusted_peers() {
        let trusted_peers = Arc::new(RwLock::new(TrustedPeers {
            peers: vec![TrustedPeer {
                name: "trusted".to_string(),
                pubkey: make_keypair().public_key(),
                addr: "inproc://trusted".to_string(),
                meta: meerkat_comms::PeerMeta::default(),
            }],
        }));
        let router = make_tool_context().0;
        let surface = CommsToolSurface::new(router, trusted_peers);
        let tools = surface.tools();
        let tool_names: Vec<&str> = tools.iter().map(|tool| tool.name.as_str()).collect();
        assert!(tool_names.contains(&"send_message"));
        assert!(tool_names.contains(&"peers"));
        assert!(!tool_names.contains(&"send_request"));
        assert!(!tool_names.contains(&"send_response"));
    }

    #[tokio::test]
    async fn test_comms_callability_false_without_trusted_peers() {
        let trusted_peers = Arc::new(RwLock::new(TrustedPeers::new()));
        let router = make_tool_context().0;
        let surface = CommsToolSurface::new(router, trusted_peers);
        assert!(surface.tools().is_empty());
        assert!(
            surface
                .tool_catalog()
                .iter()
                .all(|entry| !entry.currently_callable())
        );

        let args_raw = serde_json::value::RawValue::from_string(Value::Null.to_string()).unwrap();
        let call = ToolCallView {
            id: "test-1",
            name: surface.tool_defs[0].name.as_str(),
            args: &args_raw,
        };
        let result = surface.dispatch(call).await;
        assert!(matches!(result, Err(ToolError::Unavailable { .. })));
    }

    #[tokio::test]
    async fn send_request_without_runtime_returns_typed_unavailable_reason() {
        let (router, trusted_peers) = make_tool_context();
        let surface = CommsToolSurface::new(router, trusted_peers);
        let peer_id = surface.tool_context.trusted_peers.read().peers[0]
            .pubkey
            .to_peer_id();
        let args_raw = serde_json::value::RawValue::from_string(
            serde_json::json!({
                "peer_id": peer_id,
                "intent": "review",
                "params": {"file": "main.rs"},
                "handling_mode": "steer"
            })
            .to_string(),
        )
        .unwrap();
        let call = ToolCallView {
            id: "test-1",
            name: "send_request",
            args: &args_raw,
        };

        let result = surface.dispatch(call).await;
        assert!(matches!(
            result,
            Err(ToolError::Unavailable {
                reason: ToolUnavailableReason::RuntimeCommandAuthorityUnavailable,
                ..
            })
        ));
    }
}
