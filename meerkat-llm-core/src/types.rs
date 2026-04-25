//! LLM client types
//!
//! Defines the core trait and normalized event types.

use crate::error::LlmError;
use async_trait::async_trait;
use futures::Stream;
use meerkat_core::lifecycle::run_primitive::ProviderTag;
use meerkat_core::schema::{CompiledSchema, SchemaError};
use meerkat_core::{Message, OutputSchema, StopReason, ToolDef, Usage};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::pin::Pin;
use std::sync::Arc;

/// Stream type alias — Send on native, not required on wasm32 (single-threaded).
#[cfg(not(target_arch = "wasm32"))]
pub type LlmStream<'a> = Pin<Box<dyn Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>>;
#[cfg(target_arch = "wasm32")]
pub type LlmStream<'a> = Pin<Box<dyn Stream<Item = Result<LlmEvent, LlmError>> + 'a>>;

/// Abstraction over LLM providers
///
/// Each provider implementation normalizes its streaming response
/// to the common `LlmEvent` type, hiding provider-specific quirks.
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait LlmClient: Send + Sync {
    /// Stream a completion request
    ///
    /// Returns a stream of normalized events. The stream completes
    /// when the model finishes (either with EndTurn or ToolUse).
    fn stream<'a>(&'a self, request: &'a LlmRequest) -> LlmStream<'a>;

    /// Get the provider name (for logging/debugging)
    fn provider(&self) -> &'static str;

    /// Check if the client is healthy/connected
    async fn health_check(&self) -> Result<(), LlmError>;

    /// Compile an output schema for this provider.
    ///
    /// Default implementation normalizes the schema without provider-specific lowering.
    /// Provider implementations override this to apply provider-specific transformations.
    fn compile_schema(&self, output_schema: &OutputSchema) -> Result<CompiledSchema, SchemaError> {
        Ok(CompiledSchema {
            schema: output_schema.schema.as_value().clone(),
            warnings: Vec::new(),
        })
    }
}

/// Request to the LLM
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmRequest {
    pub model: String,
    pub messages: Vec<Message>,
    #[serde(default)]
    pub tools: Vec<Arc<ToolDef>>,
    pub max_tokens: u32,
    pub temperature: Option<f32>,
    #[serde(default)]
    pub stop_sequences: Option<Vec<String>>,
    /// Typed provider-specific knobs. `ProviderTag` is a provider-tagged
    /// enum with typed fields for every known knob; legacy untyped JSON
    /// is projected through
    /// [`meerkat_core::lifecycle::run_primitive::AnthropicProviderTag::from_legacy_value`]
    /// (and the OpenAi/Gemini siblings) at the adapter boundary so the
    /// request surface never carries `serde_json::Value`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_params: Option<ProviderTag>,
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
    pub fn with_tools(mut self, tools: Vec<Arc<ToolDef>>) -> Self {
        self.tools = tools;
        self
    }

    /// Set provider-specific parameters (replaces any existing params).
    pub fn with_provider_params(mut self, params: ProviderTag) -> Self {
        self.provider_params = Some(params);
        self
    }

    /// Merge a typed knob into the existing `ProviderTag::Anthropic`
    /// slot, constructing a default `AnthropicProviderTag` if none is
    /// present. Chainable test/builder convenience.
    pub fn with_anthropic_tag_merge<F>(mut self, f: F) -> Self
    where
        F: FnOnce(&mut meerkat_core::lifecycle::run_primitive::AnthropicProviderTag),
    {
        use meerkat_core::lifecycle::run_primitive::AnthropicProviderTag;
        let mut tag = match self.provider_params.take() {
            Some(ProviderTag::Anthropic(t)) => t,
            _ => AnthropicProviderTag::default(),
        };
        f(&mut tag);
        self.provider_params = Some(ProviderTag::Anthropic(tag));
        self
    }

    /// Merge a typed knob into the existing `ProviderTag::OpenAi` slot.
    pub fn with_openai_tag_merge<F>(mut self, f: F) -> Self
    where
        F: FnOnce(&mut meerkat_core::lifecycle::run_primitive::OpenAiProviderTag),
    {
        use meerkat_core::lifecycle::run_primitive::OpenAiProviderTag;
        let mut tag = match self.provider_params.take() {
            Some(ProviderTag::OpenAi(t)) => t,
            _ => OpenAiProviderTag::default(),
        };
        f(&mut tag);
        self.provider_params = Some(ProviderTag::OpenAi(tag));
        self
    }

    /// Merge a typed knob into the existing `ProviderTag::Gemini` slot.
    pub fn with_gemini_tag_merge<F>(mut self, f: F) -> Self
    where
        F: FnOnce(&mut meerkat_core::lifecycle::run_primitive::GeminiProviderTag),
    {
        use meerkat_core::lifecycle::run_primitive::GeminiProviderTag;
        let mut tag = match self.provider_params.take() {
            Some(ProviderTag::Gemini(t)) => t,
            _ => GeminiProviderTag::default(),
        };
        f(&mut tag);
        self.provider_params = Some(ProviderTag::Gemini(tag));
        self
    }

    /// Set the structured-output schema on an existing provider tag.
    /// Requires the request already has a `ProviderTag` variant selected
    /// (the schema lives in the per-provider slot; there is no
    /// provider-agnostic home for it).
    pub fn with_output_schema(mut self, schema: OutputSchema) -> Self {
        self.provider_params = match self.provider_params.take() {
            Some(ProviderTag::Anthropic(mut a)) => {
                a.structured_output = Some(schema);
                Some(ProviderTag::Anthropic(a))
            }
            Some(ProviderTag::OpenAi(mut o)) => {
                o.structured_output = Some(schema);
                Some(ProviderTag::OpenAi(o))
            }
            Some(ProviderTag::Gemini(mut g)) => {
                g.structured_output = Some(schema);
                Some(ProviderTag::Gemini(g))
            }
            Some(other) => Some(other),
            None => None,
        };
        self
    }
}

/// Normalized streaming events from LLM
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum LlmDoneOutcome {
    Success { stop_reason: StopReason },
    Error { error: LlmError },
}

/// Normalized streaming events from LLM
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LlmEvent {
    /// Incremental text output.
    /// Gemini may include `meta` for thoughtSignature on text parts.
    TextDelta {
        delta: String,
        /// Provider metadata (Gemini thoughtSignature on text parts)
        #[serde(default, skip_serializing_if = "Option::is_none")]
        meta: Option<Box<meerkat_core::ProviderMeta>>,
    },

    /// Incremental reasoning/thinking content
    ReasoningDelta { delta: String },

    /// Complete reasoning block (emitted at content_block_stop)
    ReasoningComplete {
        text: String,
        /// Typed provider metadata - each adapter knows its provider
        #[serde(default, skip_serializing_if = "Option::is_none")]
        meta: Option<Box<meerkat_core::ProviderMeta>>,
    },

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
        /// Typed provider metadata
        #[serde(default, skip_serializing_if = "Option::is_none")]
        meta: Option<Box<meerkat_core::ProviderMeta>>,
    },

    /// Token usage update
    UsageUpdate { usage: Usage },

    /// Stream completed with stop reason
    Done {
        #[serde(flatten)]
        outcome: LlmDoneOutcome,
    },
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
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn test_llm_event_serialization() -> Result<(), Box<dyn std::error::Error>> {
        let events = vec![
            LlmEvent::TextDelta {
                delta: "Hello".to_string(),
                meta: None,
            },
            LlmEvent::ReasoningDelta {
                delta: "thinking...".to_string(),
            },
            LlmEvent::ReasoningComplete {
                text: "done thinking".to_string(),
                meta: Some(Box::new(meerkat_core::ProviderMeta::Anthropic {
                    signature: "sig123".to_string(),
                })),
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
                meta: None,
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
                outcome: LlmDoneOutcome::Success {
                    stop_reason: StopReason::EndTurn,
                },
            },
        ];

        for event in events {
            let json = serde_json::to_value(&event)?;
            assert!(json.get("type").is_some());

            let parsed: LlmEvent = serde_json::from_value(json)?;
            let _ = serde_json::to_value(&parsed)?;
        }
        Ok(())
    }

    #[test]
    fn test_tool_call_buffer() -> Result<(), Box<dyn std::error::Error>> {
        let mut buffer = ToolCallBuffer::new("tc_1".to_string());
        buffer.name = Some("test_tool".to_string());
        buffer.args_json = r#"{"key": "value"}"#.to_string();

        let completed = buffer.try_complete().ok_or("incomplete")?;
        assert_eq!(completed.id, "tc_1");
        assert_eq!(completed.name, "test_tool");
        assert_eq!(completed.args["key"], "value");
        Ok(())
    }

    #[test]
    fn test_tool_call_buffer_incomplete() {
        let buffer = ToolCallBuffer::new("tc_1".to_string());
        assert!(buffer.try_complete().is_none());

        let mut buffer = ToolCallBuffer::new("tc_1".to_string());
        buffer.name = Some("test".to_string());
        buffer.args_json = r#"{"incomplete"#.to_string();
        assert!(buffer.try_complete().is_none());
    }

    #[test]
    fn test_tool_call_buffer_empty_args() -> Result<(), Box<dyn std::error::Error>> {
        let mut buffer = ToolCallBuffer::new("tc_1".to_string());
        buffer.name = Some("get_todays_activities".to_string());
        buffer.args_json = String::new();

        let completed = buffer.try_complete().ok_or("incomplete")?;
        assert_eq!(completed.id, "tc_1");
        assert_eq!(completed.name, "get_todays_activities");
        assert_eq!(completed.args, serde_json::json!({}));
        Ok(())
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
    fn test_llm_request_typed_provider_params_round_trip() -> Result<(), Box<dyn std::error::Error>>
    {
        use meerkat_core::lifecycle::run_primitive::{
            AnthropicProviderTag, AnthropicThinkingConfig,
        };
        let request = LlmRequest::new("claude-3", vec![]).with_provider_params(
            ProviderTag::Anthropic(AnthropicProviderTag {
                thinking: Some(AnthropicThinkingConfig::Enabled {
                    budget_tokens: 10000,
                }),
                ..Default::default()
            }),
        );

        let json = serde_json::to_value(&request)?;
        assert!(json.get("provider_params").is_some());
        let parsed: LlmRequest = serde_json::from_value(json)?;
        match parsed.provider_params {
            Some(ProviderTag::Anthropic(t)) => {
                assert_eq!(
                    t.thinking,
                    Some(AnthropicThinkingConfig::Enabled {
                        budget_tokens: 10000,
                    }),
                );
            }
            other => return Err(format!("wrong variant: {other:?}").into()),
        }
        Ok(())
    }

    #[test]
    fn test_tool_call_buffer_preallocated_capacity() {
        let buffer = ToolCallBuffer::new("tc_1".to_string());
        assert!(
            buffer.args_json.capacity() >= TOOL_CALL_ARGS_CAPACITY,
            "Buffer should have pre-allocated capacity of at least {} bytes, got {}",
            TOOL_CALL_ARGS_CAPACITY,
            buffer.args_json.capacity()
        );
    }

    #[test]
    fn test_tool_call_buffer_with_capacity() {
        let buffer = ToolCallBuffer::with_capacity("tc_1".to_string(), 1024);
        assert!(buffer.args_json.capacity() >= 1024);
    }

    #[test]
    fn test_tool_call_buffer_push_args() -> Result<(), Box<dyn std::error::Error>> {
        let mut buffer = ToolCallBuffer::new("tc_1".to_string());
        buffer.name = Some("test_tool".to_string());
        buffer.push_args(r#"{"key""#);
        buffer.push_args(r#": "value"}"#);

        let completed = buffer.try_complete().ok_or("incomplete")?;
        assert_eq!(completed.args["key"], "value");
        Ok(())
    }

    #[test]
    fn test_tool_call_buffer_no_reallocation_small_args() {
        let mut buffer = ToolCallBuffer::new("tc_1".to_string());
        let initial_capacity = buffer.args_json.capacity();
        let small_json = r#"{"path": "/tmp/test.txt"}"#;
        buffer.push_args(small_json);
        assert_eq!(buffer.args_json.capacity(), initial_capacity);
    }

    #[test]
    fn test_regression_tool_call_buffer_delta_streaming() -> Result<(), Box<dyn std::error::Error>>
    {
        use std::collections::HashMap;

        let deltas = vec![
            ("tc_1", Some("read_file"), r#"{"pa"#),
            ("tc_1", None, r#"th": ""#),
            ("tc_1", None, r#"/tmp/test.txt"}"#),
        ];

        let mut buffers: HashMap<String, ToolCallBuffer> = HashMap::new();

        for (id, name, args_delta) in deltas {
            let buffer = buffers
                .entry(id.to_string())
                .or_insert_with(|| ToolCallBuffer::new(id.to_string()));

            if let Some(n) = name {
                buffer.name = Some(n.to_string());
            }
            buffer.push_args(args_delta);
        }

        let tool_calls: Vec<_> = buffers
            .into_values()
            .filter_map(|b| b.try_complete())
            .collect();

        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].name, "read_file");
        assert_eq!(tool_calls[0].args["path"], "/tmp/test.txt");

        Ok(())
    }
}
