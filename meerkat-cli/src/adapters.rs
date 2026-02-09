//! Adapters to bridge existing crate types to agent traits

use async_trait::async_trait;
use meerkat_core::{ToolCallView, ToolDef, ToolResult, agent::AgentToolDispatcher};
use meerkat_tools::ToolError;
use serde_json::Value;
use std::sync::Arc;

/// Empty tool dispatcher for when no tools are configured
pub struct EmptyToolDispatcher;

#[async_trait]
impl AgentToolDispatcher for EmptyToolDispatcher {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::from([])
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolResult, ToolError> {
        Err(ToolError::not_found(call.name))
    }
}

#[cfg(feature = "mcp")]
use meerkat_mcp::McpRouter;
use meerkat_tools::builtin::CompositeDispatcher;
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

/// Combined tool dispatcher that can be empty, MCP-backed, or composite (with builtins)
pub enum CliToolDispatcher {
    Empty(EmptyToolDispatcher),
    #[cfg(feature = "mcp")]
    Mcp(Box<McpRouterAdapter>),
    Composite(std::sync::Arc<CompositeDispatcher>),
    /// Dispatcher wrapped with comms tools (uses Arc to match wrap_with_comms output)
    #[cfg(feature = "comms")]
    WithComms(Arc<dyn AgentToolDispatcher>),
}

impl CliToolDispatcher {
    /// Gracefully shutdown MCP connections (no-op for Empty and Composite)
    pub async fn shutdown(&self) {
        match self {
            CliToolDispatcher::Empty(_) => {}
            #[cfg(feature = "mcp")]
            CliToolDispatcher::Mcp(adapter) => adapter.shutdown().await,
            CliToolDispatcher::Composite(_) => {
                // CompositeDispatcher doesn't have external connections to shut down
            }
            #[cfg(feature = "comms")]
            CliToolDispatcher::WithComms(_) => {
                // Comms connections are managed by CommsRuntime
            }
        }
    }
}

#[async_trait]
impl AgentToolDispatcher for CliToolDispatcher {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        match self {
            CliToolDispatcher::Empty(d) => d.tools(),
            #[cfg(feature = "mcp")]
            CliToolDispatcher::Mcp(d) => d.tools(),
            CliToolDispatcher::Composite(d) => d.tools(),
            #[cfg(feature = "comms")]
            CliToolDispatcher::WithComms(d) => d.tools(),
        }
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolResult, ToolError> {
        match self {
            CliToolDispatcher::Empty(d) => d.dispatch(call).await,
            #[cfg(feature = "mcp")]
            CliToolDispatcher::Mcp(d) => d.dispatch(call).await,
            CliToolDispatcher::Composite(d) => d.dispatch(call).await,
            #[cfg(feature = "comms")]
            CliToolDispatcher::WithComms(d) => d.dispatch(call).await,
        }
    }
}
