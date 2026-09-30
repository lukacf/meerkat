//! MCP protocol wrapper.
//!
//! Keeps rmcp wire-level types and parsing out of `connection.rs`.

use std::sync::Arc;

use meerkat_core::ToolDef;
use meerkat_core::types::{ContentBlock, ToolProvenance, ToolSourceKind};
use rmcp::{
    model::{CallToolRequestParams, CallToolResult, Content, RawContent},
    service::{Peer, RoleClient, RunningService},
};
use serde_json::Value;

use crate::McpError;

pub struct McpProtocol {
    service: crate::client_service::ConnectedClient,
}

impl McpProtocol {
    pub fn new(service: RunningService<RoleClient, ()>) -> Self {
        Self {
            service: service.into(),
        }
    }

    pub(crate) fn from_client(service: crate::client_service::ConnectedClient) -> Self {
        Self { service }
    }

    pub fn server_info(&self) -> Option<Arc<rmcp::model::ServerInfo>> {
        self.service.peer_info()
    }

    pub async fn list_tools(&self, server_name: &str) -> Result<Vec<ToolDef>, McpError> {
        list_all_tools(&self.service, server_name).await
    }

    /// Call a tool, returning multimodal content blocks.
    ///
    /// Text and image content are captured directly as [`ContentBlock`]
    /// variants; resource, audio, and resource-link content the agent loop does
    /// not model are preserved verbatim as [`ContentBlock::Structured`] rather
    /// than silently dropped. The server's optional `structuredContent` JSON
    /// value is appended as one additional Structured block.
    pub async fn call_tool(&self, name: &str, args: &Value) -> Result<Vec<ContentBlock>, McpError> {
        let request = match args.as_object().cloned() {
            Some(arguments) => {
                CallToolRequestParams::new(name.to_string()).with_arguments(arguments)
            }
            None => CallToolRequestParams::new(name.to_string()),
        };

        let result =
            self.service
                .call_tool(request)
                .await
                .map_err(|e| McpError::ToolCallFailed {
                    tool: name.to_string(),
                    reason: e.to_string(),
                })?;

        convert_tool_result(result, name)
    }

    /// Call a tool, returning only text content (errors on non-text content,
    /// including a server's `structuredContent`).
    pub async fn call_tool_text(&self, name: &str, args: &Value) -> Result<String, McpError> {
        let blocks = self.call_tool(name, args).await?;
        extract_text_content_strict(blocks).map_err(|message| McpError::ProtocolError {
            message: format!("Tool '{name}' returned unsupported content: {message}"),
        })
    }

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

/// Enumerate every page through the same live service. Later-page failures
/// refuse the whole observation rather than publishing a partial tool list.
/// Cursors are opaque; repeated cursors are a protocol error, not completion.
pub(crate) async fn list_all_tools(
    service: &Peer<RoleClient>,
    server_name: &str,
) -> Result<Vec<ToolDef>, McpError> {
    let mut request = None;
    let mut seen_cursors = std::collections::HashSet::new();
    let mut tools = Vec::new();
    loop {
        let response =
            service
                .list_tools(request)
                .await
                .map_err(|error| McpError::ProtocolError {
                    message: format!("Failed to list tools: {error}"),
                })?;
        tools.extend(response.tools.into_iter().map(|tool| {
            let schema = Value::Object(Arc::unwrap_or_clone(tool.input_schema));
            ToolDef {
                name: tool.name.to_string().into(),
                description: tool.description.unwrap_or_default().to_string(),
                input_schema: schema,
                provenance: Some(ToolProvenance {
                    kind: ToolSourceKind::Mcp,
                    source_id: server_name.into(),
                }),
            }
        }));
        let Some(cursor) = response.next_cursor else {
            return Ok(tools);
        };
        if !seen_cursors.insert(cursor.clone()) {
            return Err(McpError::ProtocolError {
                message: "Failed to list tools: repeated pagination cursor".to_string(),
            });
        }
        request = Some(rmcp::model::PaginatedRequestParams::default().with_cursor(Some(cursor)));
    }
}

/// Preserve the server's ordered content, followed by its optional structured
/// JSON value. Both public MCP wrappers use this conversion. Error results keep
/// their existing failure contract and project all supplied detail into it.
pub(crate) fn convert_tool_result(
    result: CallToolResult,
    name: &str,
) -> Result<Vec<ContentBlock>, McpError> {
    let mut blocks = extract_content_blocks(result.content);
    if let Some(data) = result.structured_content {
        let block = ContentBlock::structured(&data).map_err(|error| McpError::ProtocolError {
            message: format!("Tool '{name}' returned invalid structured content: {error}"),
        })?;
        blocks.push(block);
    }
    if result.is_error.unwrap_or(false) {
        return Err(McpError::ToolCallFailed {
            tool: name.to_string(),
            reason: tool_error_reason(&blocks),
        });
    }
    Ok(blocks)
}

/// The public failure type carries derived text, not a typed MCP envelope.
pub(crate) fn tool_error_reason(blocks: &[ContentBlock]) -> String {
    let text = meerkat_core::types::text_content(blocks);
    if text.is_empty() {
        "tool returned error with no content".to_string()
    } else {
        text
    }
}

/// Convert ordered MCP content entries without dropping unmodeled variants.
pub(crate) fn extract_content_blocks(contents: Vec<Content>) -> Vec<ContentBlock> {
    contents.into_iter().map(content_block_from_raw).collect()
}

/// Faithfully map a single MCP [`RawContent`] to a [`ContentBlock`].
///
/// No variant is silently dropped: unmodeled variants are preserved as
/// [`ContentBlock::Structured`] carrying the original JSON. If a Structured
/// block cannot be serialized, the raw content is surfaced as a debug text
/// projection so the fact that content was present is never laundered away.
fn content_block_from_raw(content: Content) -> ContentBlock {
    match content.raw {
        RawContent::Text(text) => ContentBlock::Text { text: text.text },
        RawContent::Image(image) => ContentBlock::Image {
            media_type: image.mime_type,
            data: meerkat_core::ImageData::Inline { data: image.data },
        },
        other => structured_content_block(&other),
    }
}

/// Preserve an unmodeled [`RawContent`] variant as structured JSON, falling
/// back to a debug text projection if it cannot be serialized.
fn structured_content_block(raw: &RawContent) -> ContentBlock {
    match serde_json::value::to_raw_value(raw) {
        Ok(data) => ContentBlock::Structured { data },
        Err(_) => ContentBlock::Text {
            text: format!("{raw:?}"),
        },
    }
}

fn extract_text_content_strict(contents: Vec<ContentBlock>) -> Result<String, String> {
    let mut out = String::new();

    for content in contents {
        match content {
            ContentBlock::Text { text } => {
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str(&text);
            }
            other => return Err(format!("{other:?}")),
        }
    }

    Ok(out)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_text_content_strict_multiple_items() {
        let contents = vec![
            Content::text("Line 1"),
            Content::text("Line 2"),
            Content::text("Line 3"),
        ];
        let result = extract_text_content_strict(extract_content_blocks(contents)).unwrap();
        assert_eq!(result, "Line 1\nLine 2\nLine 3");
    }

    #[test]
    fn test_extract_text_content_strict_single_item() {
        let contents = vec![Content::text("Only line")];
        let result = extract_text_content_strict(extract_content_blocks(contents)).unwrap();
        assert_eq!(result, "Only line");
    }

    #[test]
    fn test_extract_text_content_strict_empty() {
        let contents: Vec<Content> = Vec::new();
        let result = extract_text_content_strict(extract_content_blocks(contents)).unwrap();
        assert_eq!(result, "");
    }

    #[test]
    fn protocol_extract_content_blocks_captures_image() {
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

    #[test]
    fn protocol_extract_content_blocks_mixed() {
        let contents = vec![
            Content::text("Before image"),
            Content::image("cG5nZGF0YQ==", "image/jpeg"),
        ];
        let blocks = extract_content_blocks(contents);
        assert_eq!(blocks.len(), 2);
        assert!(matches!(&blocks[0], ContentBlock::Text { text } if text == "Before image"));
        assert!(
            matches!(&blocks[1], ContentBlock::Image { media_type, .. } if media_type == "image/jpeg")
        );
    }

    #[test]
    fn protocol_extract_content_blocks_text_only() {
        let contents = vec![Content::text("just text")];
        let blocks = extract_content_blocks(contents);
        assert_eq!(blocks.len(), 1);
        assert_eq!(
            blocks[0],
            ContentBlock::Text {
                text: "just text".to_string()
            }
        );
    }

    /// Regression: an unmodeled content variant (embedded resource) must be
    /// preserved as `Structured` JSON, never silently dropped (no `_ => None`
    /// launder). The data the server returned survives the conversion.
    #[test]
    fn protocol_extract_content_blocks_preserves_unmodeled_variant() {
        let contents = vec![
            Content::text("before"),
            Content::embedded_text("file:///doc.txt", "resource body"),
        ];
        let blocks = extract_content_blocks(contents);
        assert_eq!(blocks.len(), 2, "no content variant may be dropped");
        assert!(matches!(&blocks[0], ContentBlock::Text { text } if text == "before"));
        match &blocks[1] {
            ContentBlock::Structured { data } => {
                let rendered = data.get();
                assert!(
                    rendered.contains("resource body"),
                    "structured passthrough must preserve the resource body verbatim, got: {rendered}"
                );
            }
            other => panic!("expected Structured passthrough, got {other:?}"),
        }
    }

    /// Regression: when a tool errors, the typed reason must carry the
    /// server-authored content detail, not a fixed `"Tool returned error"`
    /// string that launders the cause away.
    #[test]
    fn protocol_tool_error_reason_carries_server_detail() {
        let content = vec![Content::text("disk quota exceeded")];
        let reason = tool_error_reason(&extract_content_blocks(content));
        assert_eq!(reason, "disk quota exceeded");
    }

    /// An errored result with no content still produces a non-empty, honest
    /// reason rather than an empty string.
    #[test]
    fn protocol_tool_error_reason_handles_empty_content() {
        let reason = tool_error_reason(&[]);
        assert_eq!(reason, "tool returned error with no content");
    }
}
