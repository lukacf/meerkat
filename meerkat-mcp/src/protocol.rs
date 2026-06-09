//! MCP protocol wrapper.
//!
//! Keeps rmcp wire-level types and parsing out of `connection.rs`.

use std::sync::Arc;

use meerkat_core::ToolDef;
use meerkat_core::types::{ContentBlock, ToolProvenance, ToolSourceKind};
use rmcp::{
    model::{CallToolRequestParams, Content, RawContent},
    service::{RoleClient, RunningService},
};
use serde_json::Value;

use crate::McpError;

pub struct McpProtocol {
    service: RunningService<RoleClient, ()>,
}

impl McpProtocol {
    pub fn new(service: RunningService<RoleClient, ()>) -> Self {
        Self { service }
    }

    pub fn server_info(&self) -> Option<&rmcp::model::ServerInfo> {
        self.service.peer_info()
    }

    pub async fn list_tools(&self, server_name: &str) -> Result<Vec<ToolDef>, McpError> {
        let response =
            self.service
                .list_tools(None)
                .await
                .map_err(|e| McpError::ProtocolError {
                    message: format!("Failed to list tools: {e}"),
                })?;

        let tools = response
            .tools
            .into_iter()
            .map(|tool| {
                // Convert Arc<Map<String, Value>> to Value::Object
                // Use Arc::unwrap_or_clone to avoid clone if we have the only reference
                let inner_map = Arc::unwrap_or_clone(tool.input_schema);
                let schema = Value::Object(inner_map);
                ToolDef {
                    name: tool.name.to_string().into(),
                    description: tool.description.unwrap_or_default().to_string(),
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
    /// Text and image content are captured directly as [`ContentBlock`]
    /// variants; resource, audio, and resource-link content the agent loop does
    /// not model are preserved verbatim as [`ContentBlock::Structured`] rather
    /// than silently dropped.
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
                reason: e.to_string(),
            })?;

        if result.is_error.unwrap_or(false) {
            return Err(McpError::ToolCallFailed {
                tool: name.to_string(),
                reason: tool_error_reason(&result.content),
            });
        }

        Ok(extract_content_blocks(result.content))
    }

    /// Call a tool, returning only text content (errors on non-text content).
    pub async fn call_tool_text(&self, name: &str, args: &Value) -> Result<String, McpError> {
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
                reason: e.to_string(),
            })?;

        if result.is_error.unwrap_or(false) {
            return Err(McpError::ToolCallFailed {
                tool: name.to_string(),
                reason: tool_error_reason(&result.content),
            });
        }

        extract_text_content_strict(result.content).map_err(|message| McpError::ProtocolError {
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

/// Derive a typed failure reason from an errored tool result's content.
///
/// MCP carries the server-authored error detail in `CallToolResult.content`
/// when `is_error` is set. Flatten that text (and any unmodeled content) into
/// the reason rather than laundering it away behind a fixed string.
fn tool_error_reason(content: &[Content]) -> String {
    let blocks = extract_content_blocks(content.to_vec());
    let text = meerkat_core::types::text_content(&blocks);
    if text.is_empty() {
        "tool returned error with no content".to_string()
    } else {
        text
    }
}

/// Convert MCP [`Content`] items to [`ContentBlock`] variants.
///
/// Shared extraction logic for the protocol layer (mirrors
/// `connection::extract_content_blocks`). Text and image map to their typed
/// [`ContentBlock`] equivalents; resource, audio, and resource-link content
/// the agent loop does not model are preserved verbatim as
/// [`ContentBlock::Structured`] JSON rather than silently dropped.
fn extract_content_blocks(contents: Vec<Content>) -> Vec<ContentBlock> {
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

fn extract_text_content_strict(contents: Vec<Content>) -> Result<String, String> {
    let mut out = String::new();

    for content in contents {
        match content.raw {
            RawContent::Text(text) => {
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str(&text.text);
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
        let result = extract_text_content_strict(contents).unwrap();
        assert_eq!(result, "Line 1\nLine 2\nLine 3");
    }

    #[test]
    fn test_extract_text_content_strict_single_item() {
        let contents = vec![Content::text("Only line")];
        let result = extract_text_content_strict(contents).unwrap();
        assert_eq!(result, "Only line");
    }

    #[test]
    fn test_extract_text_content_strict_empty() {
        let contents: Vec<Content> = Vec::new();
        let result = extract_text_content_strict(contents).unwrap();
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
        let reason = tool_error_reason(&content);
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
