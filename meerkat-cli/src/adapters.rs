//! Adapters to bridge existing crate types to agent traits

#[cfg(feature = "mcp")]
use async_trait::async_trait;
#[cfg(feature = "mcp")]
use meerkat_core::{ToolCallView, ToolDef, ToolResult, agent::AgentToolDispatcher};
#[cfg(feature = "mcp")]
use meerkat_tools::ToolError;
#[cfg(feature = "mcp")]
use serde_json::Value;
#[cfg(feature = "mcp")]
use std::sync::Arc;

#[cfg(feature = "mcp")]
use meerkat_mcp::McpRouter;
#[cfg(feature = "mcp")]
use tokio::sync::RwLock;

/// Adapter that wraps an McpRouter to implement AgentToolDispatcher
#[cfg(feature = "mcp")]
pub struct McpRouterAdapter {
    router: RwLock<Option<McpRouter>>,
    cached_tools: RwLock<Arc<[Arc<ToolDef>]>>,
}

#[cfg(feature = "mcp")]
impl McpRouterAdapter {
    pub fn new(router: McpRouter) -> Self {
        Self {
            router: RwLock::new(Some(router)),
            cached_tools: RwLock::new(Arc::from([])),
        }
    }

    /// Refresh the cached tool list from the router
    ///
    /// Note: list_tools() is now synchronous since tools are cached in the router.
    pub async fn refresh_tools(&self) -> Result<(), String> {
        let router = self.router.read().await;
        if let Some(router) = router.as_ref() {
            let tools: Arc<[Arc<ToolDef>]> = router.list_tools().to_vec().into();
            let mut cached = self.cached_tools.write().await;
            *cached = tools;
        }
        Ok(())
    }

    /// Gracefully shutdown the MCP router
    ///
    /// Takes the router out of the adapter and shuts it down.
    /// After this call, tool calls will fail.
    pub async fn shutdown(&self) {
        let mut router = self.router.write().await;
        if let Some(router) = router.take() {
            router.shutdown().await;
        }
    }
}

#[async_trait]
#[cfg(feature = "mcp")]
impl AgentToolDispatcher for McpRouterAdapter {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        // Return the cached tools (blocking read in sync context)
        // Note: This requires refresh_tools() to be called before first use
        // We use try_read to avoid deadlocks, falling back to empty vec
        match self.cached_tools.try_read() {
            Ok(tools) => Arc::clone(&tools),
            Err(_) => Arc::from([]),
        }
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolResult, ToolError> {
        let guard = self.router.read().await;
        match &*guard {
            Some(router) => {
                let args: Value = serde_json::from_str(call.args.get())
                    .unwrap_or_else(|_| Value::String(call.args.get().to_string()));
                let result = router
                    .call_tool(call.name, &args)
                    .await
                    .map_err(|e| ToolError::execution_failed(e.to_string()))?;
                Ok(ToolResult {
                    tool_use_id: call.id.to_string(),
                    content: result,
                    is_error: false,
                })
            }
            None => Err(ToolError::execution_failed("MCP router has been shut down")),
        }
    }
}
