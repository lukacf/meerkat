//! MCP router for multi-server routing

use crate::{McpConnection, McpError, McpServerConfig};
use meerkat_core::ToolDef;
use serde_json::Value;
use std::collections::HashMap;

/// Router for MCP tool calls across multiple servers
pub struct McpRouter {
    servers: HashMap<String, McpConnection>,
    tool_to_server: HashMap<String, String>,
}

impl McpRouter {
    /// Create a new empty router
    pub fn new() -> Self {
        Self {
            servers: HashMap::new(),
            tool_to_server: HashMap::new(),
        }
    }

    /// Add an MCP server
    pub async fn add_server(&mut self, config: McpServerConfig) -> Result<(), McpError> {
        let conn = McpConnection::connect(&config).await?;

        // Discover tools from this server
        let tools = conn.list_tools().await?;
        for tool in &tools {
            self.tool_to_server.insert(tool.name.clone(), config.name.clone());
        }

        self.servers.insert(config.name.clone(), conn);
        Ok(())
    }

    /// List all available tools
    pub async fn list_tools(&self) -> Result<Vec<ToolDef>, McpError> {
        let mut all_tools = Vec::new();
        for conn in self.servers.values() {
            let tools = conn.list_tools().await?;
            all_tools.extend(tools);
        }
        Ok(all_tools)
    }

    /// Call a tool by name
    pub async fn call_tool(&self, name: &str, args: &Value) -> Result<String, McpError> {
        let server_name = self.tool_to_server.get(name)
            .ok_or_else(|| McpError::ToolNotFound(name.to_string()))?;

        let conn = self.servers.get(server_name)
            .ok_or_else(|| McpError::ServerNotFound(server_name.clone()))?;

        conn.call_tool(name, args).await
    }

    /// Gracefully shutdown all connections
    ///
    /// This should be called before the program exits to avoid
    /// "task was cancelled" errors from the MCP service.
    pub async fn shutdown(self) {
        for (name, conn) in self.servers {
            if let Err(e) = conn.close().await {
                tracing::debug!("Error closing MCP connection '{}': {}", name, e);
            }
        }
    }
}

impl Default for McpRouter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn test_server_path() -> PathBuf {
        let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
        let workspace_root = PathBuf::from(manifest_dir).parent().unwrap().to_path_buf();
        workspace_root
            .join("target")
            .join("debug")
            .join("mcp-test-server")
    }

    fn skip_if_no_test_server() -> Option<PathBuf> {
        let path = test_server_path();
        if path.exists() {
            Some(path)
        } else {
            eprintln!(
                "Skipping: mcp-test-server not built. Run `cargo build -p mcp-test-server` first."
            );
            None
        }
    }

    /// Regression test: verify router shutdown closes connections gracefully
    /// without "task was cancelled" errors
    #[tokio::test]
    async fn test_router_shutdown_graceful() {
        let Some(server_path) = skip_if_no_test_server() else {
            return;
        };

        let mut router = McpRouter::new();
        let config = McpServerConfig::stdio(
            "test-server",
            server_path.to_string_lossy().to_string(),
            vec![],
            HashMap::new(),
        );

        router.add_server(config).await.expect("Failed to add server");

        // Verify tools were discovered
        let tools = router.list_tools().await.expect("Failed to list tools");
        assert!(!tools.is_empty(), "Should have discovered tools");

        // Graceful shutdown - should not produce any errors
        router.shutdown().await;

        // If we get here without panics or errors being logged at ERROR level,
        // the graceful shutdown worked
    }

    /// Test that router handles multiple servers shutdown
    #[tokio::test]
    async fn test_router_multiple_servers_shutdown() {
        let Some(server_path) = skip_if_no_test_server() else {
            return;
        };

        let mut router = McpRouter::new();

        // Add the same server twice with different names
        let config1 = McpServerConfig::stdio(
            "server1",
            server_path.to_string_lossy().to_string(),
            vec![],
            HashMap::new(),
        );
        let config2 = McpServerConfig::stdio(
            "server2",
            server_path.to_string_lossy().to_string(),
            vec![],
            HashMap::new(),
        );

        router.add_server(config1).await.expect("Failed to add server1");
        router.add_server(config2).await.expect("Failed to add server2");

        // Shutdown all - should close both connections gracefully
        router.shutdown().await;
    }
}
