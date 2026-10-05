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

use crate::{McpError, ToolDiscoveryLimit};

pub struct McpProtocol {
    service: crate::client_service::ConnectedClient,
    /// The stdio server's process when built from a connection that owns one.
    stdio_child: Option<crate::connection::StdioChildCustody>,
}

impl McpProtocol {
    pub fn new(service: RunningService<RoleClient, ()>) -> Self {
        Self {
            service: service.into(),
            stdio_child: None,
        }
    }

    pub(crate) fn from_client(
        service: crate::client_service::ConnectedClient,
        stdio_child: Option<crate::connection::StdioChildCustody>,
    ) -> Self {
        Self {
            service,
            stdio_child,
        }
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
    /// value is appended as one additional Structured block; a text block whose
    /// content parses to the same JSON value is its serialization and is not
    /// repeated.
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
        crate::connection::close_connected(self.service, self.stdio_child).await
    }
}

/// Enumerate every page through the same live service. Later-page failures
/// refuse the whole observation rather than publishing a partial tool list.
/// Cursors are opaque; a repeated cursor is a typed protocol error, not
/// completion. Enumeration is bounded by
/// [`McpConnection::MAX_TOOL_DISCOVERY_PAGES`] and
/// [`McpConnection::MAX_DISCOVERED_TOOLS`]; exceeding either refuses the list.
///
/// [`McpConnection::MAX_TOOL_DISCOVERY_PAGES`]: crate::McpConnection::MAX_TOOL_DISCOVERY_PAGES
/// [`McpConnection::MAX_DISCOVERED_TOOLS`]: crate::McpConnection::MAX_DISCOVERED_TOOLS
pub(crate) async fn list_all_tools(
    service: &Peer<RoleClient>,
    server_name: &str,
) -> Result<Vec<ToolDef>, McpError> {
    const MAX_PAGES: usize = crate::McpConnection::MAX_TOOL_DISCOVERY_PAGES;
    const MAX_TOOLS: usize = crate::McpConnection::MAX_DISCOVERED_TOOLS;
    let mut request = None;
    let mut seen_cursors = std::collections::HashSet::new();
    let mut tools = Vec::new();
    let mut pages = 0usize;
    loop {
        let response =
            service
                .list_tools(request)
                .await
                .map_err(|error| McpError::ProtocolError {
                    message: format!("Failed to list tools: {error}"),
                })?;
        pages += 1;
        if tools.len().saturating_add(response.tools.len()) > MAX_TOOLS {
            return Err(McpError::ToolDiscoveryLimitExceeded {
                server: server_name.to_string(),
                limit: ToolDiscoveryLimit::Tools { max: MAX_TOOLS },
            });
        }
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
            return Err(McpError::ToolDiscoveryCursorRepeated {
                server: server_name.to_string(),
                cursor,
            });
        }
        if pages >= MAX_PAGES {
            return Err(McpError::ToolDiscoveryLimitExceeded {
                server: server_name.to_string(),
                limit: ToolDiscoveryLimit::Pages { max: MAX_PAGES },
            });
        }
        request = Some(rmcp::model::PaginatedRequestParams::default().with_cursor(Some(cursor)));
    }
}

/// Preserve the server's ordered content, followed by its optional structured
/// JSON value, which replaces any text block that serializes the same value.
/// Both public MCP wrappers use this conversion. Error results keep
/// their existing failure contract and project all supplied detail into it.
pub(crate) fn convert_tool_result(
    result: CallToolResult,
    name: &str,
) -> Result<Vec<ContentBlock>, McpError> {
    let mut blocks = extract_content_blocks(result.content);
    if let Some(data) = result.structured_content {
        // MCP asks servers that return structuredContent to also serialize it
        // into a text block for older clients. A text block that parses to
        // the same JSON value is that serialization, not further content, so
        // only the typed Structured block carries it. Text that differs or is
        // not JSON stays.
        blocks.retain(|block| !is_text_serialization_of(block, &data));
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

/// True when `block` is text whose content parses to JSON equal to `data`.
fn is_text_serialization_of(block: &ContentBlock, data: &Value) -> bool {
    matches!(
        block,
        ContentBlock::Text { text }
            if serde_json::from_str::<Value>(text).is_ok_and(|parsed| parsed == *data)
    )
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
