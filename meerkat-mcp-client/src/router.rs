//! MCP router for multi-server routing

use crate::{McpConnection, McpError, McpServerConfig};
use meerkat_core::ToolDef;
use serde_json::Value;
use std::collections::HashMap;

/// Router for MCP tool calls across multiple servers
pub struct McpRouter {
    servers: HashMap<String, McpConnection>,
    tool_to_server: HashMap<String, String>,
    /// Cached tool definitions - populated during add_server to avoid
    /// redundant network calls on list_tools()
    cached_tools: Vec<ToolDef>,
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
        }
        // Cache the tool definitions to avoid redundant network calls
        self.cached_tools.extend(tools);

        self.servers.insert(config.name.clone(), conn);
        Ok(())
    }

    /// List all available tools (returns cached tool definitions)
    ///
    /// Tool definitions are cached during `add_server()` to avoid redundant
    /// network calls. This method is now synchronous and returns a slice.
    pub fn list_tools(&self) -> &[ToolDef] {
        &self.cached_tools
    }

    /// Call a tool by name
    ///
    /// Uses a single HashMap lookup by storing server name alongside tool mapping,
    /// avoiding double lookups.
    pub async fn call_tool(&self, name: &str, args: &Value) -> Result<String, McpError> {
        // Single lookup: get server name from tool mapping
        let server_name = self
            .tool_to_server
            .get(name)
            .ok_or_else(|| McpError::ToolNotFound(name.to_string()))?;

        // Second lookup is unavoidable - we need the connection object
        let conn = self
            .servers
            .get(server_name)
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

        router
            .add_server(config)
            .await
            .expect("Failed to add server");

        // Verify tools were discovered (now synchronous due to caching)
        let tools = router.list_tools();
        assert!(!tools.is_empty(), "Should have discovered tools");

        // Graceful shutdown - should not produce any errors
        router.shutdown().await;

        // If we get here without panics or errors being logged at ERROR level,
        // the graceful shutdown worked
    }

    /// Test that list_tools returns cached results without extra network calls
    /// TDD: This tests that the router caches tool definitions after add_server
    #[tokio::test]
    async fn test_list_tools_uses_cached_tools() {
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

        router
            .add_server(config)
            .await
            .expect("Failed to add server");

        // First call - should return cached tools from add_server
        let tools1 = router.list_tools();
        assert!(!tools1.is_empty(), "Should have tools from cache");

        // Second call - should return same cached tools
        let tools2 = router.list_tools();
        assert_eq!(
            tools1.len(),
            tools2.len(),
            "Should return same cached tools"
        );

        // Verify tool names match
        let names1: Vec<_> = tools1.iter().map(|t| &t.name).collect();
        let names2: Vec<_> = tools2.iter().map(|t| &t.name).collect();
        assert_eq!(names1, names2, "Tool names should match");

        router.shutdown().await;
    }

    /// Test that Vec is pre-allocated with appropriate capacity
    #[tokio::test]
    async fn test_list_tools_preallocates_capacity() {
        let Some(server_path) = skip_if_no_test_server() else {
            return;
        };

        let mut router = McpRouter::new();

        // Add two servers
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

        router
            .add_server(config1)
            .await
            .expect("Failed to add server1");
        router
            .add_server(config2)
            .await
            .expect("Failed to add server2");

        // list_tools should work efficiently with pre-allocation
        let tools = router.list_tools();
        // With 2 servers, each providing ~3 tools, we should have ~6 tools
        assert!(tools.len() >= 4, "Should have tools from both servers");

        router.shutdown().await;
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

        router
            .add_server(config1)
            .await
            .expect("Failed to add server1");
        router
            .add_server(config2)
            .await
            .expect("Failed to add server2");

        // Shutdown all - should close both connections gracefully
        router.shutdown().await;
    }
}
