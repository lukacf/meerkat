//! MCP connection management

use crate::McpError;
use crate::transport::sse::{SseClientConfig, SseClientTransport};
use crate::transport::{
    headers_from_map, sse::ReqwestSseClient, streamable_http::ReqwestStreamableHttpClient,
};
use async_trait::async_trait;
use meerkat_auth_core::{McpAuthMode, McpAuthTarget, McpOAuthError};
use meerkat_core::McpServerConfig;
use meerkat_core::ToolDef;
use meerkat_core::mcp_config::{McpHttpTransport, McpTransportConfig};
use meerkat_core::types::{ContentBlock, ToolProvenance, ToolSourceKind};
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

#[async_trait]
pub trait McpAuthResolver: Send + Sync {
    async fn stored_bearer_token(
        &self,
        target: &McpAuthTarget,
    ) -> Result<Option<String>, McpOAuthError>;

    async fn interactive_login(
        &self,
        target: &McpAuthTarget,
        www_authenticate: Option<&str>,
    ) -> Result<String, McpOAuthError>;
}

#[async_trait]
impl McpAuthResolver for meerkat_auth_core::McpOAuthAuthority {
    async fn stored_bearer_token(
        &self,
        target: &McpAuthTarget,
    ) -> Result<Option<String>, McpOAuthError> {
        self.stored_bearer_token(target).await
    }

    async fn interactive_login(
        &self,
        target: &McpAuthTarget,
        www_authenticate: Option<&str>,
    ) -> Result<String, McpOAuthError> {
        self.interactive_login(target, www_authenticate).await
    }
}

impl McpConnection {
    /// Connect to an MCP server and perform the initialize handshake
    pub async fn connect(config: &McpServerConfig) -> Result<Self, McpError> {
        Self::connect_with_mcp_auth(config, McpAuthMode::Stored, None).await
    }

    pub async fn connect_with_mcp_auth(
        config: &McpServerConfig,
        auth_mode: McpAuthMode,
        auth_resolver: Option<Arc<dyn McpAuthResolver>>,
    ) -> Result<Self, McpError> {
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
                        return Self::connect_streamable_http(
                            config,
                            headers,
                            &http.url,
                            auth_mode,
                            auth_resolver,
                        )
                        .await;
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

    async fn connect_streamable_http(
        config: &McpServerConfig,
        headers: reqwest::header::HeaderMap,
        url: &str,
        auth_mode: McpAuthMode,
        auth_resolver: Option<Arc<dyn McpAuthResolver>>,
    ) -> Result<Self, McpError> {
        let has_static_authorization = headers
            .keys()
            .any(|name| name.as_str().eq_ignore_ascii_case("authorization"));
        if has_static_authorization {
            return Self::connect_streamable_http_once(config, headers, url, None, None)
                .await
                .map_err(StreamableConnectError::into_mcp_error);
        }
        let target = McpAuthTarget::new(config.name.clone(), url.to_string());
        let mut stored_token = None;
        let mut force_interactive_reauth = false;
        if let Some(resolver) = auth_resolver.as_deref() {
            match resolver.stored_bearer_token(&target).await {
                Ok(token) => stored_token = token,
                Err(McpOAuthError::ReauthRequired { .. })
                    if matches!(auth_mode, McpAuthMode::Interactive) =>
                {
                    force_interactive_reauth = true;
                }
                Err(error) => return Err(mcp_auth_error_to_connection_failed(error)),
            }
        }
        if force_interactive_reauth {
            let Some(resolver) = auth_resolver else {
                return Err(mcp_auth_error_to_connection_failed(
                    McpOAuthError::ReauthRequired {
                        server_name: config.name.clone(),
                    },
                ));
            };
            let token = resolver
                .interactive_login(&target, None)
                .await
                .map_err(mcp_auth_error_to_connection_failed)?;
            return Self::connect_streamable_http_once(config, headers, url, Some(token), None)
                .await
                .map_err(StreamableConnectError::into_mcp_error);
        }
        let first = Self::connect_streamable_http_once(
            config,
            headers.clone(),
            url,
            stored_token.clone(),
            Some(crate::transport::streamable_http::AuthChallengeRecorder::default()),
        )
        .await;
        match first {
            Ok(connection) => Ok(connection),
            Err(err) if auth_failure_suggests_oauth(&err) => {
                let challenge = err.auth_challenge();
                let Some(resolver) = auth_resolver else {
                    return Err(StreamableConnectError::into_mcp_error(err));
                };
                let token = match (stored_token, auth_mode) {
                    (Some(_), McpAuthMode::Stored) => {
                        return Err(McpError::ConnectionFailed {
                            reason: McpOAuthError::ReauthRequired {
                                server_name: config.name.clone(),
                            }
                            .to_string(),
                        });
                    }
                    (Some(_), McpAuthMode::Interactive) => resolver
                        .interactive_login(&target, challenge.as_deref())
                        .await
                        .map_err(mcp_auth_error_to_connection_failed)?,
                    (None, McpAuthMode::Stored) => {
                        return Err(McpError::ConnectionFailed {
                            reason: McpOAuthError::MissingStoredToken {
                                server_name: config.name.clone(),
                            }
                            .to_string(),
                        });
                    }
                    (None, McpAuthMode::Interactive) => resolver
                        .interactive_login(&target, challenge.as_deref())
                        .await
                        .map_err(mcp_auth_error_to_connection_failed)?,
                };
                Self::connect_streamable_http_once(config, headers, url, Some(token), None)
                    .await
                    .map_err(StreamableConnectError::into_mcp_error)
            }
            Err(err) => Err(err.into_mcp_error()),
        }
    }

    async fn connect_streamable_http_once(
        config: &McpServerConfig,
        headers: reqwest::header::HeaderMap,
        url: &str,
        bearer_token: Option<String>,
        recorder: Option<crate::transport::streamable_http::AuthChallengeRecorder>,
    ) -> Result<Self, StreamableConnectError> {
        let recorder = recorder.unwrap_or_default();
        let client =
            ReqwestStreamableHttpClient::new_with_auth_challenge(headers, recorder.clone());
        let mut transport_config = StreamableHttpClientTransportConfig::with_uri(url.to_string());
        if let Some(token) = bearer_token {
            transport_config = transport_config.auth_header(token);
        }
        let transport = StreamableHttpClientTransport::with_client(client, transport_config);
        let service =
            ().serve(transport)
                .await
                .map_err(|error| StreamableConnectError {
                    reason: format!("Failed to establish MCP connection: {error}"),
                    auth_challenge: recorder.take(),
                })?;
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
        Self::connect_and_enumerate_with_mcp_auth(config, McpAuthMode::Stored, None).await
    }

    pub async fn connect_and_enumerate_with_mcp_auth(
        config: &McpServerConfig,
        auth_mode: McpAuthMode,
        auth_resolver: Option<Arc<dyn McpAuthResolver>>,
    ) -> Result<(Self, Vec<Arc<ToolDef>>), McpError> {
        let timeout_secs = config
            .connect_timeout_secs
            .unwrap_or(Self::DEFAULT_CONNECT_TIMEOUT_SECS);
        let mut timeout = Duration::from_secs(timeout_secs as u64);
        if matches!(auth_mode, McpAuthMode::Interactive) {
            timeout += meerkat_auth_core::MCP_INTERACTIVE_LOGIN_TIMEOUT;
        }

        let server_name = config.name.clone();
        tokio::time::timeout(timeout, async {
            let conn = Self::connect_with_mcp_auth(config, auth_mode, auth_resolver).await?;
            let tools = conn
                .list_tools(&server_name)
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
    pub async fn list_tools(&self, server_name: &str) -> Result<Vec<ToolDef>, McpError> {
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
                    name: t.name.to_string().into(),
                    description: t.description.unwrap_or_default().to_string(),
                    input_schema: schema,
                    provenance: Some(ToolProvenance {
                        kind: ToolSourceKind::Mcp,
                        source_id: server_name.into(),
                    }),
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

struct StreamableConnectError {
    reason: String,
    auth_challenge: Option<String>,
}

impl StreamableConnectError {
    fn auth_challenge(&self) -> Option<String> {
        self.auth_challenge.clone()
    }

    fn into_mcp_error(self) -> McpError {
        McpError::ConnectionFailed {
            reason: self.reason,
        }
    }
}

fn auth_failure_suggests_oauth(error: &StreamableConnectError) -> bool {
    error.auth_challenge.is_some()
        || error.reason.contains("Auth required")
        || error.reason.contains("401")
        || error.reason.contains("403")
        || error.reason.contains("Unauthorized")
        || error.reason.contains("Forbidden")
}

fn mcp_auth_error_to_connection_failed(error: McpOAuthError) -> McpError {
    McpError::ConnectionFailed {
        reason: error.to_string(),
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
    use async_trait::async_trait;
    use axum::extract::State;
    use axum::http::{HeaderMap, StatusCode};
    use axum::response::IntoResponse;
    use axum::routing::post;
    use axum::{Json, Router};
    use rmcp::model::Content;
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::net::TcpListener;

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
        let tools = conn
            .list_tools("test-server")
            .await
            .expect("Failed to list tools");

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

    struct HttpMcpTestState {
        accepted_token: &'static str,
        seen_authorizations: Mutex<Vec<Option<String>>>,
    }

    async fn spawn_http_mcp_server(
        accepted_token: &'static str,
    ) -> (String, Arc<HttpMcpTestState>) {
        let state = Arc::new(HttpMcpTestState {
            accepted_token,
            seen_authorizations: Mutex::new(Vec::new()),
        });
        let app = Router::new()
            .route("/mcp", post(http_mcp_handler))
            .with_state(state.clone());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/mcp", listener.local_addr().unwrap());
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (url, state)
    }

    async fn http_mcp_handler(
        State(state): State<Arc<HttpMcpTestState>>,
        headers: HeaderMap,
        Json(request): Json<Value>,
    ) -> impl IntoResponse {
        let authorization = headers
            .get(http::header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .map(ToOwned::to_owned);
        state
            .seen_authorizations
            .lock()
            .unwrap()
            .push(authorization.clone());

        if authorization.as_deref() != Some(&format!("Bearer {}", state.accepted_token)) {
            return (
                StatusCode::UNAUTHORIZED,
                [(
                    http::header::WWW_AUTHENTICATE,
                    "Bearer resource_metadata=\"/.well-known/oauth-protected-resource/mcp\"",
                )],
            )
                .into_response();
        }

        let Some(id) = request.get("id").cloned() else {
            return StatusCode::ACCEPTED.into_response();
        };
        let response = match request.get("method").and_then(|method| method.as_str()) {
            Some("initialize") => serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "protocolVersion": "2024-11-05",
                    "capabilities": { "tools": {} },
                    "serverInfo": {
                        "name": "http-mcp-test-server",
                        "version": "0.1.0"
                    }
                }
            }),
            Some("tools/list") => serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "tools": [{
                        "name": "echo",
                        "description": "Echo input",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "message": { "type": "string" }
                            }
                        }
                    }]
                }
            }),
            method => serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {
                    "code": -32601,
                    "message": format!("unsupported method {method:?}")
                }
            }),
        };
        (StatusCode::OK, Json(response)).into_response()
    }

    struct FakeMcpAuthResolver {
        stored_token: Option<String>,
        stored_reauth_required: bool,
        interactive_token: String,
        interactive_delay: Option<Duration>,
        interactive_calls: AtomicUsize,
        challenges: Mutex<Vec<Option<String>>>,
    }

    impl FakeMcpAuthResolver {
        fn new(stored_token: Option<&str>, interactive_token: &str) -> Self {
            Self {
                stored_token: stored_token.map(ToOwned::to_owned),
                stored_reauth_required: false,
                interactive_token: interactive_token.to_owned(),
                interactive_delay: None,
                interactive_calls: AtomicUsize::new(0),
                challenges: Mutex::new(Vec::new()),
            }
        }

        fn with_interactive_delay(mut self, delay: Duration) -> Self {
            self.interactive_delay = Some(delay);
            self
        }

        fn with_stored_reauth_required(mut self) -> Self {
            self.stored_reauth_required = true;
            self
        }
    }

    #[async_trait]
    impl McpAuthResolver for FakeMcpAuthResolver {
        async fn stored_bearer_token(
            &self,
            target: &McpAuthTarget,
        ) -> Result<Option<String>, McpOAuthError> {
            if self.stored_reauth_required {
                return Err(McpOAuthError::ReauthRequired {
                    server_name: target.server_name.clone(),
                });
            }
            Ok(self.stored_token.clone())
        }

        async fn interactive_login(
            &self,
            _target: &McpAuthTarget,
            www_authenticate: Option<&str>,
        ) -> Result<String, McpOAuthError> {
            if let Some(delay) = self.interactive_delay {
                tokio::time::sleep(delay).await;
            }
            self.interactive_calls.fetch_add(1, Ordering::SeqCst);
            self.challenges
                .lock()
                .unwrap()
                .push(www_authenticate.map(ToOwned::to_owned));
            Ok(self.interactive_token.clone())
        }
    }

    #[tokio::test]
    async fn mcp_oauth_stored_token_is_injected_for_streamable_http() {
        let (url, state) = spawn_http_mcp_server("stored-token").await;
        let config = McpServerConfig::streamable_http("glean", url, HashMap::new());
        let resolver = Arc::new(FakeMcpAuthResolver::new(Some("stored-token"), "unused"));

        let (conn, tools) = McpConnection::connect_and_enumerate_with_mcp_auth(
            &config,
            McpAuthMode::Stored,
            Some(resolver),
        )
        .await
        .expect("stored token should connect");

        assert!(tools.iter().any(|tool| tool.name == "echo"));
        assert!(
            state
                .seen_authorizations
                .lock()
                .unwrap()
                .iter()
                .any(|header| header.as_deref() == Some("Bearer stored-token")),
            "streamable HTTP requests should include the stored bearer token"
        );
        conn.close().await.expect("Failed to close connection");
    }

    #[tokio::test]
    async fn mcp_oauth_interactive_login_retries_streamable_http_connect() {
        let (url, state) = spawn_http_mcp_server("interactive-token").await;
        let config = McpServerConfig::streamable_http("glean", url, HashMap::new());
        let resolver = Arc::new(FakeMcpAuthResolver::new(None, "interactive-token"));

        let (conn, tools) = McpConnection::connect_and_enumerate_with_mcp_auth(
            &config,
            McpAuthMode::Interactive,
            Some(resolver.clone()),
        )
        .await
        .expect("interactive login should retry and connect");

        assert!(tools.iter().any(|tool| tool.name == "echo"));
        assert_eq!(resolver.interactive_calls.load(Ordering::SeqCst), 1);
        assert!(
            resolver
                .challenges
                .lock()
                .unwrap()
                .iter()
                .any(|challenge| challenge
                    .as_deref()
                    .is_some_and(|value| value.contains("resource_metadata"))),
            "interactive login should receive the WWW-Authenticate challenge"
        );
        let seen = state.seen_authorizations.lock().unwrap().clone();
        assert!(
            seen.iter().any(Option::is_none),
            "first connect attempt should be unauthenticated"
        );
        assert!(
            seen.iter()
                .any(|header| header.as_deref() == Some("Bearer interactive-token")),
            "retry should include the interactive bearer token"
        );
        conn.close().await.expect("Failed to close connection");
    }

    #[tokio::test]
    async fn mcp_oauth_interactive_login_gets_browser_timeout_budget() {
        let (url, _state) = spawn_http_mcp_server("interactive-token").await;
        let mut config = McpServerConfig::streamable_http("glean", url, HashMap::new());
        config.connect_timeout_secs = Some(1);
        let resolver = Arc::new(
            FakeMcpAuthResolver::new(None, "interactive-token")
                .with_interactive_delay(Duration::from_millis(1200)),
        );

        let (conn, tools) = McpConnection::connect_and_enumerate_with_mcp_auth(
            &config,
            McpAuthMode::Interactive,
            Some(resolver),
        )
        .await
        .expect("interactive login should not be cut off by the normal connect timeout");

        assert!(tools.iter().any(|tool| tool.name == "echo"));
        conn.close().await.expect("Failed to close connection");
    }

    #[tokio::test]
    async fn mcp_oauth_interactive_mode_recovers_from_stored_reauth_required() {
        let (url, state) = spawn_http_mcp_server("interactive-token").await;
        let config = McpServerConfig::streamable_http("glean", url, HashMap::new());
        let resolver = Arc::new(
            FakeMcpAuthResolver::new(None, "interactive-token").with_stored_reauth_required(),
        );

        let (conn, tools) = McpConnection::connect_and_enumerate_with_mcp_auth(
            &config,
            McpAuthMode::Interactive,
            Some(resolver.clone()),
        )
        .await
        .expect("interactive mode should reauth when stored credentials require it");

        assert!(tools.iter().any(|tool| tool.name == "echo"));
        assert_eq!(resolver.interactive_calls.load(Ordering::SeqCst), 1);
        assert!(
            state
                .seen_authorizations
                .lock()
                .unwrap()
                .iter()
                .any(|header| header.as_deref() == Some("Bearer interactive-token")),
            "reauth retry should connect with the interactive token"
        );
        conn.close().await.expect("Failed to close connection");
    }

    #[tokio::test]
    async fn mcp_oauth_interactive_mode_recovers_from_server_rejected_stored_token() {
        let (url, state) = spawn_http_mcp_server("interactive-token").await;
        let config = McpServerConfig::streamable_http("glean", url, HashMap::new());
        let resolver = Arc::new(FakeMcpAuthResolver::new(
            Some("stale-stored-token"),
            "interactive-token",
        ));

        let (conn, tools) = McpConnection::connect_and_enumerate_with_mcp_auth(
            &config,
            McpAuthMode::Interactive,
            Some(resolver.clone()),
        )
        .await
        .expect("interactive mode should reauth when the server rejects a stored token");

        assert!(tools.iter().any(|tool| tool.name == "echo"));
        assert_eq!(resolver.interactive_calls.load(Ordering::SeqCst), 1);
        assert!(
            resolver
                .challenges
                .lock()
                .unwrap()
                .iter()
                .any(|challenge| challenge
                    .as_deref()
                    .is_some_and(|value| value.contains("resource_metadata"))),
            "server-rejected stored tokens should pass the auth challenge into reauth"
        );
        let seen = state.seen_authorizations.lock().unwrap().clone();
        assert!(
            seen.iter()
                .any(|header| header.as_deref() == Some("Bearer stale-stored-token")),
            "first connect attempt should use the stored token"
        );
        assert!(
            seen.iter()
                .any(|header| header.as_deref() == Some("Bearer interactive-token")),
            "reauth retry should use the interactive token"
        );
        conn.close().await.expect("Failed to close connection");
    }
}
