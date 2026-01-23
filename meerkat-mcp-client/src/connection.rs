//! MCP connection management

use crate::transport::sse::{SseClientConfig, SseClientTransport};
use crate::transport::{
    headers_from_map, sse::ReqwestSseClient, streamable_http::ReqwestStreamableHttpClient,
};
use crate::{McpError, McpServerConfig};
use meerkat_core::ToolDef;
use meerkat_core::mcp_config::{McpHttpTransport, McpTransportConfig};
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::{
    model::{CallToolRequestParam, RawContent},
    service::{RoleClient, RunningService, ServiceExt},
    transport::TokioChildProcess,
};
use serde_json::Value;
use tokio::process::Command;

/// Connection to an MCP server
pub struct McpConnection {
    #[allow(dead_code)]
    config: McpServerConfig,
    service: RunningService<RoleClient, ()>,
}

impl McpConnection {
    /// Connect to an MCP server and perform the initialize handshake
    pub async fn connect(config: &McpServerConfig) -> Result<Self, McpError> {
        let service = match &config.transport {
            McpTransportConfig::Stdio(stdio) => {
                let mut cmd = Command::new(&stdio.command);
                cmd.args(&stdio.args);
                for (key, value) in &stdio.env {
                    cmd.env(key, value);
                }

                let transport =
                    TokioChildProcess::new(cmd).map_err(|e| McpError::ConnectionFailed {
                        reason: format!("Failed to spawn process: {}", e),
                    })?;

                ().serve(transport)
                    .await
                    .map_err(|e| McpError::ConnectionFailed {
                        reason: format!("Failed to establish MCP connection: {}", e),
                    })?
            }
            McpTransportConfig::Http(http) => {
                let headers = headers_from_map(&http.headers)
                    .map_err(|e| McpError::ConnectionFailed { reason: e })?;
                match http.transport.unwrap_or_default() {
                    McpHttpTransport::StreamableHttp => {
                        let client = ReqwestStreamableHttpClient::new(headers);
                        let transport = StreamableHttpClientTransport::with_client(
                            client,
                            StreamableHttpClientTransportConfig::with_uri(http.url.clone()),
                        );
                        ().serve(transport)
                            .await
                            .map_err(|e| McpError::ConnectionFailed {
                                reason: format!("Failed to establish MCP connection: {}", e),
                            })?
                    }
                    McpHttpTransport::Sse => {
                        let client = ReqwestSseClient::new(headers);
                        let transport = SseClientTransport::start_with_client(
                            client,
                            SseClientConfig {
                                sse_endpoint: http.url.clone().into(),
                                use_message_endpoint: None,
                            },
                        )
                        .await
                        .map_err(|e| McpError::ConnectionFailed {
                            reason: format!("Failed to establish SSE connection: {}", e),
                        })?;
                        ().serve(transport)
                            .await
                            .map_err(|e| McpError::ConnectionFailed {
                                reason: format!("Failed to establish MCP connection: {}", e),
                            })?
                    }
                }
            }
        };

        Ok(Self {
            config: config.clone(),
            service,
        })
    }

    /// Get server info
    pub fn server_info(&self) -> Option<&rmcp::model::ServerInfo> {
        self.service.peer_info()
    }

    /// List available tools
    pub async fn list_tools(&self) -> Result<Vec<ToolDef>, McpError> {
        let response =
            self.service
                .list_tools(None)
                .await
                .map_err(|e| McpError::ProtocolError {
                    message: format!("Failed to list tools: {}", e),
                })?;

        // Convert MCP tools to our ToolDef format
        let tools = response
            .tools
            .into_iter()
            .map(|t| {
                // Convert Arc<Map<String, Value>> to Value::Object
                // Use Arc::unwrap_or_clone to avoid clone if we have the only reference
                let inner_map = std::sync::Arc::unwrap_or_clone(t.input_schema);
                let schema = Value::Object(inner_map);
                ToolDef {
                    name: t.name.to_string(),
                    description: t.description.unwrap_or_default().to_string(),
                    input_schema: schema,
                }
            })
            .collect();

        Ok(tools)
    }

    /// Call a tool
    pub async fn call_tool(&self, name: &str, args: &Value) -> Result<String, McpError> {
        let arguments = args.as_object().cloned();

        let result = self
            .service
            .call_tool(CallToolRequestParam {
                name: name.to_string().into(),
                arguments,
                task: None,
            })
            .await
            .map_err(|e| McpError::ToolCallFailed {
                tool: name.to_string(),
                reason: format!("{}", e),
            })?;

        // Check for tool error
        if result.is_error.unwrap_or(false) {
            return Err(McpError::ToolCallFailed {
                tool: name.to_string(),
                reason: "Tool returned error".to_string(),
            });
        }

        // Extract text content from result using fold to avoid intermediate Vec allocation
        let text = result
            .content
            .into_iter()
            .filter_map(|c| {
                if let RawContent::Text(text_content) = c.raw {
                    Some(text_content.text)
                } else {
                    None
                }
            })
            .fold(String::new(), |mut acc, text| {
                if !acc.is_empty() {
                    acc.push('\n');
                }
                acc.push_str(&text);
                acc
            });

        Ok(text)
    }

    /// Close the connection
    pub async fn close(self) -> Result<(), McpError> {
        self.service
            .cancel()
            .await
            .map_err(|e| McpError::ConnectionFailed {
                reason: format!("Failed to close connection: {:?}", e),
            })?;
        Ok(())
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use rmcp::model::Content;
    use std::collections::HashMap;
    use std::path::PathBuf;

    /// Helper function that extracts text from Content items using the optimized approach
    /// This mirrors the logic in call_tool() but as a standalone testable function
    fn extract_text_content(contents: Vec<Content>) -> String {
        contents
            .into_iter()
            .filter_map(|c| {
                if let RawContent::Text(text_content) = c.raw {
                    Some(text_content.text)
                } else {
                    None
                }
            })
            .fold(String::new(), |mut acc, text| {
                if !acc.is_empty() {
                    acc.push('\n');
                }
                acc.push_str(&text);
                acc
            })
    }

    /// Test that text content extraction works correctly with multiple items
    #[test]
    fn test_extract_text_content_multiple_items() {
        let contents = vec![
            Content::text("Line 1"),
            Content::text("Line 2"),
            Content::text("Line 3"),
        ];

        let result = extract_text_content(contents);
        assert_eq!(result, "Line 1\nLine 2\nLine 3");
    }

    /// Test that text content extraction works with single item (no separator)
    #[test]
    fn test_extract_text_content_single_item() {
        let contents = vec![Content::text("Only line")];

        let result = extract_text_content(contents);
        assert_eq!(result, "Only line");
    }

    /// Test that text content extraction returns empty string for empty input
    #[test]
    fn test_extract_text_content_empty() {
        let contents: Vec<Content> = vec![];
        let result = extract_text_content(contents);
        assert_eq!(result, "");
    }

    /// Get path to the test server binary
    fn test_server_path() -> PathBuf {
        // Build path relative to workspace root
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

    /// RCT: Verify MCP initialize handshake works
    #[tokio::test]
    async fn test_mcp_initialize_handshake() {
        let Some(server_path) = skip_if_no_test_server() else {
            return;
        };

        let config = McpServerConfig::stdio(
            "test-server",
            server_path.to_string_lossy().to_string(),
            vec![],
            HashMap::new(),
        );

        // Connect (includes initialize handshake)
        let conn = McpConnection::connect(&config)
            .await
            .expect("Failed to connect to MCP server");

        // Verify we got server info back
        let info = conn.server_info();
        assert!(info.is_some(), "Server should return info after initialize");
        let info = info.unwrap();
        assert_eq!(info.server_info.name, "mcp-test-server");

        // Clean up
        conn.close().await.expect("Failed to close connection");
    }

    /// RCT: Verify tools/list schema parsing
    #[tokio::test]
    async fn test_mcp_tools_list_schema_parse() {
        let Some(server_path) = skip_if_no_test_server() else {
            return;
        };

        let config = McpServerConfig::stdio(
            "test-server",
            server_path.to_string_lossy().to_string(),
            vec![],
            HashMap::new(),
        );

        let conn = McpConnection::connect(&config)
            .await
            .expect("Failed to connect");

        // List tools
        let tools = conn.list_tools().await.expect("Failed to list tools");

        // Verify we got the expected tools
        assert!(!tools.is_empty(), "Should have at least one tool");

        // Find the echo tool
        let echo_tool = tools.iter().find(|t| t.name == "echo");
        assert!(echo_tool.is_some(), "Should have echo tool");
        let echo_tool = echo_tool.unwrap();
        assert!(
            !echo_tool.description.is_empty(),
            "Echo tool should have description"
        );

        // Verify schema has expected structure
        let schema = &echo_tool.input_schema;
        assert_eq!(
            schema.get("type").and_then(|v| v.as_str()),
            Some("object"),
            "Schema should be object type"
        );
        assert!(
            schema.get("properties").is_some(),
            "Schema should have properties"
        );

        // Find the add tool and verify its schema
        let add_tool = tools.iter().find(|t| t.name == "add");
        assert!(add_tool.is_some(), "Should have add tool");
        let add_schema = &add_tool.unwrap().input_schema;
        let props = add_schema.get("properties").unwrap();
        assert!(
            props.get("a").is_some(),
            "Add tool should have 'a' property"
        );
        assert!(
            props.get("b").is_some(),
            "Add tool should have 'b' property"
        );

        conn.close().await.expect("Failed to close connection");
    }

    /// RCT: Verify tools/call round-trip
    #[tokio::test]
    async fn test_mcp_tools_call_round_trip() {
        let Some(server_path) = skip_if_no_test_server() else {
            return;
        };

        let config = McpServerConfig::stdio(
            "test-server",
            server_path.to_string_lossy().to_string(),
            vec![],
            HashMap::new(),
        );

        let conn = McpConnection::connect(&config)
            .await
            .expect("Failed to connect");

        // Test echo tool
        let result = conn
            .call_tool("echo", &serde_json::json!({"message": "Hello, MCP!"}))
            .await
            .expect("Echo call failed");
        assert_eq!(result, "Hello, MCP!");

        // Test add tool
        let result = conn
            .call_tool("add", &serde_json::json!({"a": 5, "b": 3}))
            .await
            .expect("Add call failed");
        assert_eq!(result, "8");

        // Test fail tool returns error
        let result = conn
            .call_tool("fail", &serde_json::json!({"message": "Expected error"}))
            .await;
        assert!(result.is_err(), "Fail tool should return error");

        conn.close().await.expect("Failed to close connection");
    }
}
