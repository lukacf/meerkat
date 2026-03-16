//! MCP protocol wrapper.
//!
//! Keeps rmcp wire-level types and parsing out of `connection.rs`.

use std::sync::Arc;

use meerkat_core::ToolDef;
use meerkat_core::types::ContentBlock;
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

    pub async fn list_tools(&self) -> Result<Vec<ToolDef>, McpError> {
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
                    name: tool.name.to_string(),
                    description: tool.description.unwrap_or_default().to_string(),
                    input_schema: schema,
                }
            })
            .collect();

        Ok(tools)
    }

    /// Call a tool, returning multimodal content blocks.
    ///
    /// Text and image content are captured as [`ContentBlock`] variants.
    /// Other content types are silently dropped.
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
                reason: "Tool returned error".to_string(),
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
                reason: "Tool returned error".to_string(),
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

/// Convert MCP [`Content`] items to [`ContentBlock`] variants.
///
/// Shared extraction logic for the protocol layer (mirrors
/// `connection::extract_content_blocks`).
fn extract_content_blocks(contents: Vec<Content>) -> Vec<ContentBlock> {
    contents
        .into_iter()
        .filter_map(|c| match c.raw {
            RawContent::Text(text) => Some(ContentBlock::Text { text: text.text }),
            RawContent::Image(image) => Some(ContentBlock::Image {
                media_type: image.mime_type,
                data: image.data,
                source_path: None,
            }),
            _ => None,
        })
        .collect()
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
                data: "aW1hZ2VkYXRh".to_string(),
                source_path: None,
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
}
