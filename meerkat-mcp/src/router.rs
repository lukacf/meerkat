//! MCP router for multi-server routing

use crate::{McpConnection, McpError, McpServerConfig};
use async_trait::async_trait;
use meerkat_core::AgentToolDispatcher;
use meerkat_core::types::{ToolCallView, ToolResult};
use meerkat_core::error::ToolError;
use meerkat_core::types::ToolDef;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;

/// Router for MCP tool calls across multiple servers
pub struct McpRouter {
    servers: HashMap<String, McpConnection>,
    tool_to_server: HashMap<String, String>,
    /// Cached tool definitions - populated during add_server to avoid
    /// redundant network calls on list_tools()
    cached_tools: Vec<Arc<ToolDef>>,
}

impl McpRouter {
    /// Create a new empty router
    pub fn new() -> Self {
        Self {
            servers: HashMap::new(),
            tool_to_server: HashMap::new(),
            cached_tools: Vec::new(),
        }
    }

    /// Add an MCP server
    pub async fn add_server(&mut self, config: McpServerConfig) -> Result<(), McpError> {
        let conn = McpConnection::connect(&config).await?;

        // Discover tools from this server and cache them
        let tools = conn.list_tools().await?;
        for tool in &tools {
            self.tool_to_server
                .insert(tool.name.clone(), config.name.clone());
            self.cached_tools.push(Arc::new(tool.clone()));
        }

        self.servers.insert(config.name.clone(), conn);
        Ok(())
    }

    /// List all available tools (returns cached tool definitions)
    pub fn list_tools(&self) -> &[Arc<ToolDef>] {
        &self.cached_tools
    }

    /// Call a tool by name
    pub async fn call_tool(&self, name: &str, args: &Value) -> Result<String, McpError> {
        let server_name = self
            .tool_to_server
            .get(name)
            .ok_or_else(|| McpError::ToolNotFound(name.to_string()))?;

        let conn = self
            .servers
            .get(server_name)
            .ok_or_else(|| McpError::ServerNotFound(server_name.clone()))?;

        conn.call_tool(name, args).await
    }

    /// Gracefully shutdown all connections
    pub async fn shutdown(self) {
        for (name, conn) in self.servers {
            if let Err(e) = conn.close().await {
                tracing::debug!("Error closing MCP connection '{}': {}", name, e);
            }
        }
    }
}

#[async_trait]
impl AgentToolDispatcher for McpRouter {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        self.cached_tools.clone().into()
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolResult, ToolError> {
        let args: Value = serde_json::from_str(call.args.get())
            .unwrap_or_else(|_| Value::String(call.args.get().to_string()));
        let result_str = self.call_tool(call.name, &args).await.map_err(|e| match e {
            McpError::ToolNotFound(name) => ToolError::NotFound { name },
            other => ToolError::ExecutionFailed {
                message: other.to_string(),
            },
        })?;

        Ok(ToolResult {
            tool_use_id: call.id.to_string(),
            content: result_str,
            is_error: false,
            thought_signature: None,
        })
    }
}

impl Default for McpRouter {
    fn default() -> Self {
        Self::new()
    }
}
