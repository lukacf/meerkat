//! MCP connection management

use crate::McpError;
use crate::transport::sse::{SseClientConfig, SseClientTransport};
use crate::transport::{
    headers_from_map, sse::ReqwestSseClient, streamable_http::ReqwestStreamableHttpClient,
};
use meerkat_core::McpServerConfig;
use meerkat_core::ToolDef;
use meerkat_core::mcp_config::{McpHttpTransport, McpTransportConfig};
use meerkat_core::types::ContentBlock;
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::{
    model::{CallToolRequestParams, RawContent},
    service::{RoleClient, RunningService, ServiceExt},
    transport::TokioChildProcess,
};
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;
use tokio::process::Command;

/// Connection to an MCP server
pub struct McpConnection {
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
                        reason: format!("Failed to spawn process: {e}"),
                    })?;

                ().serve(transport)
                    .await
                    .map_err(|e| McpError::ConnectionFailed {
                        reason: format!("Failed to establish MCP connection: {e}"),
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
                                reason: format!("Failed to establish MCP connection: {e}"),
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
                            reason: format!("Failed to establish SSE connection: {e}"),
                        })?;
                        ().serve(transport)
                            .await
                            .map_err(|e| McpError::ConnectionFailed {
                                reason: format!("Failed to establish MCP connection: {e}"),
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

    /// Default connection timeout in seconds.
    pub const DEFAULT_CONNECT_TIMEOUT_SECS: u32 = 10;

    /// Connect to an MCP server, perform handshake, and enumerate tools in a
    /// single timeout-bounded operation.
    ///
    /// This is the preferred entry point for all add/reload paths. The timeout
    /// covers connect + initialize handshake + list_tools as a single budget.
    pub async fn connect_and_enumerate(
        config: &McpServerConfig,
    ) -> Result<(Self, Vec<Arc<ToolDef>>), McpError> {
        let timeout_secs = config
            .connect_timeout_secs
            .unwrap_or(Self::DEFAULT_CONNECT_TIMEOUT_SECS);
        let timeout = Duration::from_secs(timeout_secs as u64);

        tokio::time::timeout(timeout, async {
            let conn = Self::connect(config).await?;
            let tools = conn
                .list_tools()
                .await?
                .into_iter()
                .map(Arc::new)
                .collect::<Vec<_>>();
            Ok((conn, tools))
        })
        .await
        .map_err(|_| McpError::ConnectionFailed {
            reason: format!(
                "Timed out connecting to '{}' ({timeout_secs}s)",
                config.name
            ),
        })?
    }

    /// Get the config used to create this connection.
    pub fn config(&self) -> &McpServerConfig {
        &self.config
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
                    message: format!("Failed to list tools: {e}"),
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

    /// Call a tool, returning multimodal content blocks.
    ///
    /// MCP servers can return text, image, and other content types. Text and
    /// image content are captured as [`ContentBlock`] variants; other content
    /// types (resources, audio, etc.) are silently dropped.
    pub async fn call_tool(&self, name: &str, args: &Value) -> Result<Vec<ContentBlock>, McpError> {
        let arguments = args.as_object().cloned();

        let result = self
            .service
            .call_tool(CallToolRequestParams {
                name: name.to_string().into(),
                arguments,
                meta: None,
                task: None,
            })
            .await
            .map_err(|e| McpError::ToolCallFailed {
                tool: name.to_string(),
                reason: format!("{e}"),
            })?;

        // Check for tool error
        if result.is_error.unwrap_or(false) {
            return Err(McpError::ToolCallFailed {
                tool: name.to_string(),
                reason: "Tool returned error".to_string(),
            });
        }

        Ok(extract_content_blocks(result.content))
    }

    /// Call a tool, returning only the text content as a concatenated string.
    ///
    /// This is a convenience wrapper around [`call_tool`](Self::call_tool) that
    /// discards non-text content. Useful for callers that only need text.
    pub async fn call_tool_text(&self, name: &str, args: &Value) -> Result<String, McpError> {
        let blocks = self.call_tool(name, args).await?;
        Ok(meerkat_core::types::text_content(&blocks))
    }

    /// Close the connection
    pub async fn close(self) -> Result<(), McpError> {
        self.service
            .cancel()
            .await
            .map_err(|e| McpError::ConnectionFailed {
                reason: format!("Failed to close connection: {e:?}"),
            })?;
        Ok(())
    }
}

/// Convert MCP [`Content`] items to [`ContentBlock`] variants.
///
/// Text and image content are captured. Other content types (resources,
/// audio, resource links) are silently dropped since the core agent loop
/// does not model them.
fn extract_content_blocks(contents: Vec<rmcp::model::Content>) -> Vec<ContentBlock> {
    contents
        .into_iter()
        .filter_map(|c| match c.raw {
            RawContent::Text(text) => Some(ContentBlock::Text { text: text.text }),
            RawContent::Image(image) => Some(ContentBlock::Image {
                media_type: image.mime_type,
                data: meerkat_core::ImageData::Inline { data: image.data },
            }),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
pub mod tests {
    use super::*;
    use rmcp::model::Content;
    use std::collections::HashMap;
    use std::path::PathBuf;

    /// Test that content block extraction works correctly with multiple text items
    #[test]
    fn test_extract_content_blocks_multiple_text() {
        let contents = vec![
            Content::text("Line 1"),
            Content::text("Line 2"),
            Content::text("Line 3"),
        ];

        let blocks = extract_content_blocks(contents);
        assert_eq!(blocks.len(), 3);
        assert_eq!(
            blocks[0],
            ContentBlock::Text {
                text: "Line 1".to_string()
            }
        );
        assert_eq!(
            blocks[1],
            ContentBlock::Text {
                text: "Line 2".to_string()
            }
        );
        assert_eq!(
            blocks[2],
            ContentBlock::Text {
                text: "Line 3".to_string()
            }
        );
    }

    /// Test that content block extraction works with single text item
    #[test]
    fn test_extract_content_blocks_single_text() {
        let contents = vec![Content::text("Only line")];

        let blocks = extract_content_blocks(contents);
        assert_eq!(blocks.len(), 1);
        assert_eq!(
            blocks[0],
            ContentBlock::Text {
                text: "Only line".to_string()
            }
        );
    }

    /// Test that content block extraction returns empty vec for empty input
    #[test]
    fn test_extract_content_blocks_empty() {
        let contents: Vec<Content> = Vec::new();
        let blocks = extract_content_blocks(contents);
        assert!(blocks.is_empty());
    }

    /// Test that image content from MCP is captured as ContentBlock::Image
    #[test]
    fn mcp_call_tool_captures_image() {
        let contents = vec![Content::image("aW1hZ2VkYXRh", "image/png")];

        let blocks = extract_content_blocks(contents);
        assert_eq!(blocks.len(), 1);
        assert_eq!(
            blocks[0],
            ContentBlock::Image {
                media_type: "image/png".to_string(),
                data: "aW1hZ2VkYXRh".into(),
            }
        );
    }

    /// Test that mixed text and image content is preserved in order
    #[test]
    fn mcp_call_tool_mixed_text_and_image() {
        let contents = vec![
            Content::text("description of the image"),
            Content::image("cG5nZGF0YQ==", "image/png"),
            Content::text("additional context"),
        ];

        let blocks = extract_content_blocks(contents);
        assert_eq!(blocks.len(), 3);
        assert!(
            matches!(&blocks[0], ContentBlock::Text { text } if text == "description of the image")
        );
        assert!(matches!(
            &blocks[1],
            ContentBlock::Image { media_type, data, .. }
                if media_type == "image/png"
                    && matches!(data, meerkat_core::ImageData::Inline { data } if data == "cG5nZGF0YQ==")
        ));
        assert!(matches!(&blocks[2], ContentBlock::Text { text } if text == "additional context"));
    }

    /// Test that text-only responses still work as before
    #[test]
    fn mcp_call_tool_text_only_compat() {
        let contents = vec![Content::text("Hello, MCP!")];

        let blocks = extract_content_blocks(contents);
        assert_eq!(blocks.len(), 1);
        assert_eq!(
            blocks[0],
            ContentBlock::Text {
                text: "Hello, MCP!".to_string()
            }
        );

        // Verify text_content projection matches legacy behavior
        let text = meerkat_core::types::text_content(&blocks);
        assert_eq!(text, "Hello, MCP!");
    }

    /// Get path to the test server binary
    fn test_server_path() -> PathBuf {
        if let Some(target_dir) = std::env::var_os("CARGO_TARGET_DIR") {
            return PathBuf::from(target_dir).join("debug/mcp-test-server");
        }

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

    /// RCT: Verify tools/call round-trip (returns Vec<ContentBlock>)
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

        // Test echo tool -- returns Vec<ContentBlock>
        let blocks = conn
            .call_tool("echo", &serde_json::json!({"message": "Hello, MCP!"}))
            .await
            .expect("Echo call failed");
        assert_eq!(meerkat_core::types::text_content(&blocks), "Hello, MCP!");

        // Test add tool
        let blocks = conn
            .call_tool("add", &serde_json::json!({"a": 5, "b": 3}))
            .await
            .expect("Add call failed");
        assert_eq!(meerkat_core::types::text_content(&blocks), "8");

        // Test fail tool returns error
        let result = conn
            .call_tool("fail", &serde_json::json!({"message": "Expected error"}))
            .await;
        assert!(result.is_err(), "Fail tool should return error");

        conn.close().await.expect("Failed to close connection");
    }
}
