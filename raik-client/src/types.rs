//! LLM client types
//!
//! Defines the core trait and normalized event types.

use crate::error::LlmError;
use async_trait::async_trait;
use futures::Stream;
use raik_core::{Message, StopReason, ToolDef, Usage};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::pin::Pin;

/// Abstraction over LLM providers
///
/// Each provider implementation normalizes its streaming response
/// to the common `LlmEvent` type, hiding provider-specific quirks.
#[async_trait]
pub trait LlmClient: Send + Sync {
    /// Stream a completion request
    ///
    /// Returns a stream of normalized events. The stream completes
    /// when the model finishes (either with EndTurn or ToolUse).
    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> Pin<Box<dyn Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>>;

    /// Get the provider name (for logging/debugging)
    fn provider(&self) -> &'static str;

    /// Check if the client is healthy/connected
    async fn health_check(&self) -> Result<(), LlmError>;
}

/// Request to the LLM
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmRequest {
    pub model: String,
    pub messages: Vec<Message>,
    #[serde(default)]
    pub tools: Vec<ToolDef>,
    pub max_tokens: u32,
    pub temperature: Option<f32>,
    #[serde(default)]
    pub stop_sequences: Option<Vec<String>>,
}

impl LlmRequest {
    /// Create a new request
    pub fn new(model: &str, messages: Vec<Message>) -> Self {
        Self {
            model: model.to_string(),
            messages,
            tools: Vec::new(),
            max_tokens: 4096,
            temperature: None,
            stop_sequences: None,
        }
    }

    /// Set max tokens
    pub fn with_max_tokens(mut self, max: u32) -> Self {
        self.max_tokens = max;
        self
    }

    /// Set temperature
    pub fn with_temperature(mut self, temp: f32) -> Self {
        self.temperature = Some(temp);
        self
    }

    /// Add tools
    pub fn with_tools(mut self, tools: Vec<ToolDef>) -> Self {
        self.tools = tools;
        self
    }
}

/// Normalized streaming events from LLM
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LlmEvent {
    /// Incremental text output
    TextDelta { delta: String },

    /// Incremental tool call (may arrive in pieces)
    ToolCallDelta {
        id: String,
        name: Option<String>,
        args_delta: String,
    },

    /// Complete tool call (after buffering deltas)
    ToolCallComplete {
        id: String,
        name: String,
        args: Value,
    },

    /// Token usage update
    UsageUpdate { usage: Usage },

    /// Stream completed with stop reason
    Done { stop_reason: StopReason },
}

/// Accumulated response from LLM stream
#[derive(Debug, Clone, Default)]
pub struct LlmResponse {
    pub content: String,
    pub tool_calls: Vec<raik_core::ToolCall>,
    pub stop_reason: StopReason,
    pub usage: Usage,
}

/// Buffer for accumulating tool call arguments during streaming
#[derive(Debug, Clone, Default)]
pub struct ToolCallBuffer {
    pub id: String,
    pub name: Option<String>,
    pub args_json: String,
}

impl ToolCallBuffer {
    /// Create a new buffer for a tool call
    pub fn new(id: String) -> Self {
        Self {
            id,
            name: None,
            args_json: String::new(),
        }
    }

    /// Try to complete the tool call (parse JSON args)
    pub fn try_complete(&self) -> Option<raik_core::ToolCall> {
        let name = self.name.as_ref()?;

        // Try to parse the accumulated JSON
        // Handle empty args (tools with no parameters) by treating as empty object
        let args: Value = if self.args_json.is_empty() {
            serde_json::json!({})
        } else {
            serde_json::from_str(&self.args_json).ok()?
        };

        Some(raik_core::ToolCall {
            id: self.id.clone(),
            name: name.clone(),
            args,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_llm_event_serialization() {
        let events = vec![
            LlmEvent::TextDelta {
                delta: "Hello".to_string(),
            },
            LlmEvent::ToolCallDelta {
                id: "tc_1".to_string(),
                name: Some("read_file".to_string()),
                args_delta: "{\"path\":".to_string(),
            },
            LlmEvent::ToolCallComplete {
                id: "tc_1".to_string(),
                name: "read_file".to_string(),
                args: serde_json::json!({"path": "/tmp/test"}),
            },
            LlmEvent::UsageUpdate {
                usage: Usage {
                    input_tokens: 100,
                    output_tokens: 50,
                    cache_creation_tokens: None,
                    cache_read_tokens: None,
                },
            },
            LlmEvent::Done {
                stop_reason: StopReason::EndTurn,
            },
        ];

        for event in events {
            let json = serde_json::to_value(&event).unwrap();
            assert!(json.get("type").is_some());

            let parsed: LlmEvent = serde_json::from_value(json).unwrap();
            let _ = serde_json::to_value(&parsed).unwrap();
        }
    }

    #[test]
    fn test_tool_call_buffer() {
        let mut buffer = ToolCallBuffer::new("tc_1".to_string());
        buffer.name = Some("test_tool".to_string());
        buffer.args_json = r#"{"key": "value"}"#.to_string();

        let completed = buffer.try_complete().unwrap();
        assert_eq!(completed.id, "tc_1");
        assert_eq!(completed.name, "test_tool");
        assert_eq!(completed.args["key"], "value");
    }

    #[test]
    fn test_tool_call_buffer_incomplete() {
        let buffer = ToolCallBuffer::new("tc_1".to_string());
        assert!(buffer.try_complete().is_none()); // No name

        let mut buffer = ToolCallBuffer::new("tc_1".to_string());
        buffer.name = Some("test".to_string());
        buffer.args_json = r#"{"incomplete"#.to_string();
        assert!(buffer.try_complete().is_none()); // Invalid JSON
    }

    /// Regression test: tools with no parameters should complete successfully
    /// (args_json is empty string, should be treated as empty object)
    #[test]
    fn test_tool_call_buffer_empty_args() {
        let mut buffer = ToolCallBuffer::new("tc_1".to_string());
        buffer.name = Some("get_todays_activities".to_string());
        buffer.args_json = String::new(); // Empty args (no parameters)

        let completed = buffer.try_complete().unwrap();
        assert_eq!(completed.id, "tc_1");
        assert_eq!(completed.name, "get_todays_activities");
        assert_eq!(completed.args, serde_json::json!({})); // Should be empty object
    }

    #[test]
    fn test_llm_request_builder() {
        let request = LlmRequest::new("claude-3", vec![])
            .with_max_tokens(8192)
            .with_temperature(0.7);

        assert_eq!(request.model, "claude-3");
        assert_eq!(request.max_tokens, 8192);
        assert_eq!(request.temperature, Some(0.7));
    }
}
