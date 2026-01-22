//! Tool dispatch with timeouts

use crate::error::DispatchError;
use crate::registry::ToolRegistry;
use meerkat_core::{ToolCall, ToolDef, ToolResult};
use meerkat_mcp_client::McpRouter;
use std::sync::Arc;
use std::time::Duration;

/// Dispatcher for tool calls with timeout support
pub struct ToolDispatcher {
    registry: ToolRegistry,
    router: Arc<McpRouter>,
    default_timeout: Duration,
}

impl ToolDispatcher {
    /// Create a new dispatcher
    pub fn new(router: Arc<McpRouter>, default_timeout: Duration) -> Self {
        Self {
            registry: ToolRegistry::new(),
            router,
            default_timeout,
        }
    }

    /// Discover tools from MCP servers
    pub async fn discover_tools(&mut self) -> Result<(), DispatchError> {
        let tools = self.router.list_tools().await?;
        for tool in tools {
            self.registry.register(tool);
        }
        Ok(())
    }

    /// Get tool definitions for LLM requests
    pub fn tool_defs(&self) -> Vec<ToolDef> {
        self.registry.tool_defs()
    }

    /// Dispatch a single tool call
    pub async fn dispatch_one(&self, call: &ToolCall) -> ToolResult {
        match self.dispatch_one_inner(call).await {
            Ok(result) => result,
            Err(e) => ToolResult {
                tool_use_id: call.id.clone(),
                content: e.to_string(),
                is_error: true,
            },
        }
    }

    async fn dispatch_one_inner(&self, call: &ToolCall) -> Result<ToolResult, DispatchError> {
        // Validate arguments
        self.registry.validate(&call.name, &call.args)?;

        // Dispatch via MCP router with timeout
        let result = tokio::time::timeout(
            self.default_timeout,
            self.router.call_tool(&call.name, &call.args),
        )
        .await
        .map_err(|_| DispatchError::Timeout {
            tool: call.name.clone(),
            timeout_ms: self.default_timeout.as_millis() as u64,
        })??;

        Ok(ToolResult {
            tool_use_id: call.id.clone(),
            content: result,
            is_error: false,
        })
    }

    /// Dispatch multiple tool calls in parallel
    pub async fn dispatch_parallel(&self, calls: &[ToolCall]) -> Vec<ToolResult> {
        let futures = calls.iter().map(|call| self.dispatch_one(call));
        futures::future::join_all(futures).await
    }
}
