//! LLM client types
//!
//! Defines the core trait and normalized event types.

use crate::error::LlmError;
use async_trait::async_trait;
use futures::Stream;
use meerkat_core::{Message, StopReason, ToolDef, Usage};
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
    /// Provider-specific parameters (e.g., thinking config, reasoning effort)
    ///
    /// This is a generic JSON bag that providers can extract provider-specific
    /// options from. Each provider implementation is responsible for reading
    /// and applying relevant parameters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_params: Option<Value>,
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
            provider_params: None,
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

    /// Set provider-specific parameters (replaces any existing params)
    ///
    /// Use this to pass the entire provider_params object at once.
    pub fn with_provider_params(mut self, params: Value) -> Self {
        self.provider_params = Some(params);
        self
    }

    /// Set a single provider-specific parameter
    ///
    /// If provider_params is None, creates a new object.
    /// If provider_params exists, merges the new key into it.
    pub fn with_provider_param(mut self, key: &str, value: impl Into<Value>) -> Self {
        let params = self
            .provider_params
            .get_or_insert_with(|| serde_json::json!({}));
        if let Some(obj) = params.as_object_mut() {
            obj.insert(key.to_string(), value.into());
        }
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
        /// Thought signature for Gemini 3+ (must be passed back)
        thought_signature: Option<String>,
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
    pub tool_calls: Vec<meerkat_core::ToolCall>,
    pub stop_reason: StopReason,
    pub usage: Usage,
}

/// Buffer for accumulating tool call arguments during streaming
///
/// Uses pre-allocated capacity to minimize allocations during streaming.
/// The default capacity of 256 bytes handles most small tool arguments
/// without reallocation.
#[derive(Debug, Clone, Default)]
pub struct ToolCallBuffer {
    pub id: String,
    pub name: Option<String>,
    pub args_json: String,
}

/// Default capacity for tool call args buffer (256 bytes handles most small args)
const TOOL_CALL_ARGS_CAPACITY: usize = 256;

impl ToolCallBuffer {
    /// Create a new buffer for a tool call with pre-allocated capacity
    pub fn new(id: String) -> Self {
        Self {
            id,
            name: None,
            args_json: String::with_capacity(TOOL_CALL_ARGS_CAPACITY),
        }
    }

    /// Create a new buffer with custom capacity for large arguments
    pub fn with_capacity(id: String, capacity: usize) -> Self {
        Self {
            id,
            name: None,
            args_json: String::with_capacity(capacity),
        }
    }

    /// Append to the args buffer
    #[inline]
    pub fn push_args(&mut self, delta: &str) {
        self.args_json.push_str(delta);
    }

    /// Try to complete the tool call (parse JSON args)
    pub fn try_complete(&self) -> Option<meerkat_core::ToolCall> {
        let name = self.name.as_ref()?;

        // Try to parse the accumulated JSON
        // Handle empty args (tools with no parameters) by treating as empty object
        let args: Value = if self.args_json.is_empty() {
            serde_json::json!({})
        } else {
            serde_json::from_str(&self.args_json).ok()?
        };

        Some(meerkat_core::ToolCall::new(
            self.id.clone(),
            name.clone(),
            args,
        ))
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
                thought_signature: None,
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

    #[test]
    fn test_llm_request_provider_params_serialization() {
        // Test serialization with provider_params set
        let request = LlmRequest::new("claude-3", vec![]).with_provider_params(serde_json::json!({
            "thinking": {
                "type": "enabled",
                "budget_tokens": 10000
            },
            "custom_flag": true
        }));

        let json = serde_json::to_value(&request).unwrap();
        assert!(json.get("provider_params").is_some());
        assert_eq!(json["provider_params"]["thinking"]["budget_tokens"], 10000);

        // Deserialize and verify
        let parsed: LlmRequest = serde_json::from_value(json).unwrap();
        assert!(parsed.provider_params.is_some());
        let params = parsed.provider_params.unwrap();
        assert_eq!(params["thinking"]["type"], "enabled");
    }

    #[test]
    fn test_llm_request_provider_params_none_serialization() {
        // Test serialization without provider_params (should serialize as null or be absent)
        let request = LlmRequest::new("claude-3", vec![]);

        let json = serde_json::to_value(&request).unwrap();
        // provider_params should either be null or absent
        let params = json.get("provider_params");
        assert!(params.is_none() || params.unwrap().is_null());

        // Deserialize should work
        let parsed: LlmRequest = serde_json::from_value(json).unwrap();
        assert!(parsed.provider_params.is_none());
    }

    #[test]
    fn test_llm_request_with_provider_params() {
        // Test with_provider_params sets the entire object
        let params = serde_json::json!({
            "reasoning_effort": "high",
            "stream_options": { "include_usage": true }
        });

        let request = LlmRequest::new("gpt-4", vec![]).with_provider_params(params.clone());

        assert_eq!(request.provider_params, Some(params));
    }

    #[test]
    fn test_llm_request_with_provider_param_single() {
        // Test with_provider_param sets a single key
        let request = LlmRequest::new("claude-3", vec![]).with_provider_param(
            "thinking",
            serde_json::json!({
                "type": "enabled",
                "budget_tokens": 5000
            }),
        );

        let params = request.provider_params.unwrap();
        assert_eq!(params["thinking"]["type"], "enabled");
        assert_eq!(params["thinking"]["budget_tokens"], 5000);
    }

    #[test]
    fn test_llm_request_with_provider_param_multiple() {
        // Test chaining multiple with_provider_param calls
        let request = LlmRequest::new("claude-3", vec![])
            .with_provider_param(
                "thinking",
                serde_json::json!({
                    "type": "enabled",
                    "budget_tokens": 10000
                }),
            )
            .with_provider_param("custom_option", "value")
            .with_provider_param("numeric_setting", 42);

        let params = request.provider_params.unwrap();
        assert_eq!(params["thinking"]["budget_tokens"], 10000);
        assert_eq!(params["custom_option"], "value");
        assert_eq!(params["numeric_setting"], 42);
    }

    #[test]
    fn test_llm_request_with_provider_param_overwrites() {
        // Test that setting the same key twice overwrites
        let request = LlmRequest::new("claude-3", vec![])
            .with_provider_param("key", "first")
            .with_provider_param("key", "second");

        let params = request.provider_params.unwrap();
        assert_eq!(params["key"], "second");
    }

    #[test]
    fn test_llm_request_provider_params_empty_object() {
        // Test with empty object
        let request =
            LlmRequest::new("claude-3", vec![]).with_provider_params(serde_json::json!({}));

        assert_eq!(request.provider_params, Some(serde_json::json!({})));
    }

    #[test]
    fn test_tool_call_buffer_preallocated_capacity() {
        let buffer = ToolCallBuffer::new("tc_1".to_string());
        // Should have pre-allocated capacity to reduce allocations
        assert!(
            buffer.args_json.capacity() >= super::TOOL_CALL_ARGS_CAPACITY,
            "Buffer should have pre-allocated capacity of at least {} bytes, got {}",
            super::TOOL_CALL_ARGS_CAPACITY,
            buffer.args_json.capacity()
        );
    }

    #[test]
    fn test_tool_call_buffer_with_capacity() {
        let buffer = ToolCallBuffer::with_capacity("tc_1".to_string(), 1024);
        assert!(
            buffer.args_json.capacity() >= 1024,
            "Buffer should have at least 1024 bytes capacity"
        );
    }

    #[test]
    fn test_tool_call_buffer_push_args() {
        let mut buffer = ToolCallBuffer::new("tc_1".to_string());
        buffer.name = Some("test_tool".to_string());
        buffer.push_args(r#"{"key""#);
        buffer.push_args(r#": "value"}"#);

        let completed = buffer.try_complete().unwrap();
        assert_eq!(completed.args["key"], "value");
    }

    #[test]
    fn test_tool_call_buffer_no_reallocation_small_args() {
        let mut buffer = ToolCallBuffer::new("tc_1".to_string());
        let initial_capacity = buffer.args_json.capacity();

        // Small JSON args that fit within default capacity
        let small_json = r#"{"path": "/tmp/test.txt"}"#;
        buffer.push_args(small_json);

        // Should not have reallocated
        assert_eq!(
            buffer.args_json.capacity(),
            initial_capacity,
            "Buffer should not reallocate for small args"
        );
    }
}
