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
use std::sync::Arc;

// ============================================================================
// Provider-specific Parameters
// ============================================================================

/// Type-safe provider-specific parameters.
///
/// Use this enum to pass provider-specific options in a type-safe manner.
/// Each provider variant contains only the parameters that provider supports.
///
/// For extensibility, the `Other` variant allows passing arbitrary JSON
/// for providers not yet covered or experimental features.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "provider", rename_all = "snake_case")]
pub enum ProviderParams {
    /// Anthropic-specific parameters
    Anthropic(AnthropicParams),
    /// OpenAI-specific parameters
    OpenAi(OpenAiParams),
    /// Google Gemini-specific parameters
    Gemini(GeminiParams),
    /// Raw JSON for extensibility
    Other(Value),
}

impl ProviderParams {
    /// Convert to raw JSON Value for backward compatibility
    pub fn to_value(&self) -> Value {
        match self {
            ProviderParams::Anthropic(p) => serde_json::to_value(p).unwrap_or_default(),
            ProviderParams::OpenAi(p) => serde_json::to_value(p).unwrap_or_default(),
            ProviderParams::Gemini(p) => serde_json::to_value(p).unwrap_or_default(),
            ProviderParams::Other(v) => v.clone(),
        }
    }
}

/// Anthropic-specific parameters
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AnthropicParams {
    /// Extended thinking configuration
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<AnthropicThinking>,
}

/// Anthropic extended thinking configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnthropicThinking {
    /// Budget type for extended thinking
    #[serde(rename = "type")]
    pub budget_type: String,
    /// Token budget for thinking
    pub budget_tokens: u32,
}

impl AnthropicParams {
    /// Create params with extended thinking enabled
    pub fn with_thinking(budget_tokens: u32) -> Self {
        Self {
            thinking: Some(AnthropicThinking {
                budget_type: "enabled".to_string(),
                budget_tokens,
            }),
        }
    }
}

/// OpenAI-specific parameters
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OpenAiParams {
    /// Reasoning effort level for o-series models
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<ReasoningEffort>,
    /// Random seed for reproducibility
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seed: Option<i64>,
    /// Frequency penalty (-2.0 to 2.0)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frequency_penalty: Option<f32>,
    /// Presence penalty (-2.0 to 2.0)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub presence_penalty: Option<f32>,
}

/// Reasoning effort levels for OpenAI o-series models
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningEffort {
    Low,
    Medium,
    High,
}

impl OpenAiParams {
    /// Create params with reasoning effort
    pub fn with_reasoning_effort(effort: ReasoningEffort) -> Self {
        Self {
            reasoning_effort: Some(effort),
            ..Default::default()
        }
    }

    /// Create params with seed for reproducibility
    pub fn with_seed(seed: i64) -> Self {
        Self {
            seed: Some(seed),
            ..Default::default()
        }
    }
}

/// Google Gemini-specific parameters
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GeminiParams {
    /// Thinking configuration for Gemini 3+ models
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<GeminiThinking>,
    /// Top-K sampling parameter
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_k: Option<u32>,
    /// Top-P (nucleus) sampling parameter
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
}

/// Gemini thinking configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeminiThinking {
    /// Whether thinking is enabled
    pub include_thoughts: bool,
    /// Budget in tokens for thinking
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_budget: Option<u32>,
}

impl GeminiParams {
    /// Create params with thinking enabled
    pub fn with_thinking(budget: u32) -> Self {
        Self {
            thinking: Some(GeminiThinking {
                include_thoughts: true,
                thinking_budget: Some(budget),
            }),
            ..Default::default()
        }
    }

    /// Create params with top-k
    pub fn with_top_k(k: u32) -> Self {
        Self {
            top_k: Some(k),
            ..Default::default()
        }
    }
}

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
    pub tools: Vec<Arc<ToolDef>>,
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
    pub fn with_tools(mut self, tools: Vec<Arc<ToolDef>>) -> Self {
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
        let mut params = self
            .provider_params
            .take()
            .unwrap_or_else(|| serde_json::json!({}));
        if let Some(obj) = params.as_object_mut() {
            obj.insert(key.to_string(), value.into());
        }
        self.provider_params = Some(params);
        self
    }

    /// Set type-safe provider parameters
    ///
    /// This is the preferred way to set provider-specific options as it
    /// prevents passing parameters meant for one provider to another.
    ///
    /// # Example
    /// ```ignore
    /// let request = LlmRequest::new("o1-preview", messages)
    ///     .with_typed_params(ProviderParams::OpenAi(
    ///         OpenAiParams::with_reasoning_effort(ReasoningEffort::High)
    ///     ));
    /// ```
    pub fn with_typed_params(mut self, params: ProviderParams) -> Self {
        self.provider_params = Some(params.to_value());
        self
    }

    /// Set OpenAI-specific parameters
    pub fn with_openai_params(self, params: OpenAiParams) -> Self {
        self.with_typed_params(ProviderParams::OpenAi(params))
    }

    /// Set Anthropic-specific parameters
    pub fn with_anthropic_params(self, params: AnthropicParams) -> Self {
        self.with_typed_params(ProviderParams::Anthropic(params))
    }

    /// Set Gemini-specific parameters
    pub fn with_gemini_params(self, params: GeminiParams) -> Self {
        self.with_typed_params(ProviderParams::Gemini(params))
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
        assert!(buffer.try_complete().is_none()); // No name

        let mut buffer = ToolCallBuffer::new("tc_1".to_string());
        buffer.name = Some("test".to_string());
        buffer.args_json = r#"{"incomplete"#.to_string();
        assert!(buffer.try_complete().is_none()); // Invalid JSON
    }

    /// Regression test: tools with no parameters should complete successfully
    /// (args_json is empty string, should be treated as empty object)
    #[test]
    fn test_tool_call_buffer_empty_args() -> Result<(), Box<dyn std::error::Error>> {
        let mut buffer = ToolCallBuffer::new("tc_1".to_string());
        buffer.name = Some("get_todays_activities".to_string());
        buffer.args_json = String::new(); // Empty args (no parameters)

        let completed = buffer.try_complete().ok_or("incomplete")?;
        assert_eq!(completed.id, "tc_1");
        assert_eq!(completed.name, "get_todays_activities");
        assert_eq!(completed.args, serde_json::json!({})); // Should be empty object
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
    fn test_llm_request_provider_params_serialization() -> Result<(), Box<dyn std::error::Error>> {
        // Test serialization with provider_params set
        let request = LlmRequest::new("claude-3", vec![]).with_provider_params(serde_json::json!({
            "thinking": {
                "type": "enabled",
                "budget_tokens": 10000
            },
            "custom_flag": true
        }));

        let json = serde_json::to_value(&request)?;
        assert!(json.get("provider_params").is_some());
        assert_eq!(json["provider_params"]["thinking"]["budget_tokens"], 10000);

        // Deserialize and verify
        let parsed: LlmRequest = serde_json::from_value(json)?;
        assert!(parsed.provider_params.is_some());
        let params = parsed.provider_params.as_ref().ok_or("missing params")?;
        assert_eq!(params["thinking"]["type"], "enabled");
        Ok(())
    }

    #[test]
    fn test_llm_request_provider_params_none_serialization()
    -> Result<(), Box<dyn std::error::Error>> {
        // Test serialization without provider_params (should serialize as null or be absent)
        let request = LlmRequest::new("claude-3", vec![]);

        let json = serde_json::to_value(&request)?;
        // provider_params should either be null or absent
        let params = json.get("provider_params");
        assert!(params.is_none() || params.ok_or("not found")?.is_null());

        // Deserialize should work
        let parsed: LlmRequest = serde_json::from_value(json)?;
        assert!(parsed.provider_params.is_none());
        Ok(())
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
    fn test_llm_request_with_provider_param_single() -> Result<(), Box<dyn std::error::Error>> {
        // Test with_provider_param sets a single key
        let request = LlmRequest::new("claude-3", vec![]).with_provider_param(
            "thinking",
            serde_json::json!({
                "type": "enabled",
                "budget_tokens": 5000
            }),
        );

        let params = request.provider_params.as_ref().ok_or("missing params")?;
        assert_eq!(params["thinking"]["type"], "enabled");
        assert_eq!(params["thinking"]["budget_tokens"], 5000);
        Ok(())
    }

    #[test]
    fn test_llm_request_with_provider_param_multiple() -> Result<(), Box<dyn std::error::Error>> {
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

        let params = request.provider_params.as_ref().ok_or("missing params")?;
        assert_eq!(params["thinking"]["budget_tokens"], 10000);
        assert_eq!(params["custom_option"], "value");
        assert_eq!(params["numeric_setting"], 42);
        Ok(())
    }

    #[test]
    fn test_llm_request_with_provider_param_overwrites() -> Result<(), Box<dyn std::error::Error>> {
        // Test that setting the same key twice overwrites
        let request = LlmRequest::new("claude-3", vec![])
            .with_provider_param("key", "first")
            .with_provider_param("key", "second");

        let params = request.provider_params.as_ref().ok_or("missing params")?;
        assert_eq!(params["key"], "second");
        Ok(())
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
            buffer.args_json.capacity() >= TOOL_CALL_ARGS_CAPACITY,
            "Buffer should have pre-allocated capacity of at least {} bytes, got {}",
            TOOL_CALL_ARGS_CAPACITY,
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

    /// Regression test: ToolCallBuffer must be usable for buffering ToolCallDelta events
    /// from providers like Anthropic that emit deltas instead of complete tool calls.
    #[test]
    fn test_regression_tool_call_buffer_delta_streaming() -> Result<(), Box<dyn std::error::Error>>
    {
        use std::collections::HashMap;

        // Simulate streaming deltas as Anthropic would emit them
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

        // Convert buffered deltas to tool calls
        let tool_calls: Vec<_> = buffers
            .into_values()
            .filter_map(|b| b.try_complete())
            .collect();

        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].name, "read_file");
        assert_eq!(tool_calls[0].args["path"], "/tmp/test.txt");

        Ok(())
    }

    // =========================================================================
    // ProviderParams type-safety tests
    // =========================================================================

    #[test]
    fn test_provider_params_anthropic_serialization() -> Result<(), Box<dyn std::error::Error>> {
        let params = ProviderParams::Anthropic(AnthropicParams::with_thinking(10000));
        let json = serde_json::to_value(&params)?;

        assert_eq!(json["provider"], "anthropic");
        assert_eq!(json["thinking"]["type"], "enabled");
        assert_eq!(json["thinking"]["budget_tokens"], 10000);

        // Round-trip
        let parsed: ProviderParams = serde_json::from_value(json)?;
        match parsed {
            ProviderParams::Anthropic(p) => {
                let thinking = p.thinking.ok_or("missing thinking")?;
                assert_eq!(thinking.budget_tokens, 10000);
            }
            _ => return Err("wrong variant".into()),
        }
        Ok(())
    }

    #[test]
    fn test_provider_params_openai_serialization() -> Result<(), Box<dyn std::error::Error>> {
        let params =
            ProviderParams::OpenAi(OpenAiParams::with_reasoning_effort(ReasoningEffort::High));
        let json = serde_json::to_value(&params)?;

        assert_eq!(json["provider"], "open_ai");
        assert_eq!(json["reasoning_effort"], "high");

        // Round-trip
        let parsed: ProviderParams = serde_json::from_value(json)?;
        match parsed {
            ProviderParams::OpenAi(p) => {
                assert_eq!(p.reasoning_effort, Some(ReasoningEffort::High));
            }
            _ => return Err("wrong variant".into()),
        }
        Ok(())
    }

    #[test]
    fn test_provider_params_gemini_serialization() -> Result<(), Box<dyn std::error::Error>> {
        let params = ProviderParams::Gemini(GeminiParams::with_thinking(8000));
        let json = serde_json::to_value(&params)?;

        assert_eq!(json["provider"], "gemini");
        let thinking = json.get("thinking").ok_or("missing thinking")?;
        assert_eq!(thinking["include_thoughts"], true);
        assert_eq!(thinking["thinking_budget"], 8000);

        // Round-trip
        let parsed: ProviderParams = serde_json::from_value(json)?;
        match parsed {
            ProviderParams::Gemini(p) => {
                let thinking = p.thinking.ok_or("missing thinking")?;
                assert!(thinking.include_thoughts);
                assert_eq!(thinking.thinking_budget, Some(8000));
            }
            _ => return Err("wrong variant".into()),
        }
        Ok(())
    }

    #[test]
    fn test_reasoning_effort_variants() -> Result<(), Box<dyn std::error::Error>> {
        // Test all ReasoningEffort variants serialize correctly
        for (effort, expected) in [
            (ReasoningEffort::Low, "low"),
            (ReasoningEffort::Medium, "medium"),
            (ReasoningEffort::High, "high"),
        ] {
            let json = serde_json::to_value(effort)?;
            assert_eq!(json.as_str(), Some(expected));

            // Round-trip
            let parsed: ReasoningEffort = serde_json::from_value(json)?;
            assert_eq!(parsed, effort);
        }
        Ok(())
    }

    #[test]
    fn test_provider_params_to_value() {
        // Test the to_value() method extracts inner params without the tag
        let anthropic_params = ProviderParams::Anthropic(AnthropicParams::with_thinking(5000));
        let value = anthropic_params.to_value();

        // Should have thinking but NOT the "provider" tag
        assert!(value.get("provider").is_none());
        assert!(value.get("thinking").is_some());
        assert_eq!(value["thinking"]["budget_tokens"], 5000);
    }

    #[test]
    fn test_llm_request_with_typed_params() -> Result<(), Box<dyn std::error::Error>> {
        let request = LlmRequest::new("o1-preview", vec![]).with_typed_params(
            ProviderParams::OpenAi(OpenAiParams::with_reasoning_effort(ReasoningEffort::High)),
        );

        let params = request.provider_params.ok_or("missing params")?;
        assert_eq!(params["reasoning_effort"], "high");
        Ok(())
    }

    #[test]
    fn test_llm_request_with_anthropic_params() -> Result<(), Box<dyn std::error::Error>> {
        let request = LlmRequest::new("claude-sonnet-4", vec![])
            .with_anthropic_params(AnthropicParams::with_thinking(10000));

        let params = request.provider_params.ok_or("missing params")?;
        assert_eq!(params["thinking"]["type"], "enabled");
        assert_eq!(params["thinking"]["budget_tokens"], 10000);
        Ok(())
    }

    #[test]
    fn test_llm_request_with_openai_params() -> Result<(), Box<dyn std::error::Error>> {
        let request = LlmRequest::new("gpt-5.2", vec![])
            .with_openai_params(OpenAiParams::with_reasoning_effort(ReasoningEffort::Medium));

        let params = request.provider_params.ok_or("missing params")?;
        assert_eq!(params["reasoning_effort"], "medium");
        Ok(())
    }

    #[test]
    fn test_llm_request_with_gemini_params() -> Result<(), Box<dyn std::error::Error>> {
        let request = LlmRequest::new("gemini-3-pro", vec![])
            .with_gemini_params(GeminiParams::with_thinking(16000));

        let params = request.provider_params.ok_or("missing params")?;
        assert_eq!(params["thinking"]["include_thoughts"], true);
        assert_eq!(params["thinking"]["thinking_budget"], 16000);
        Ok(())
    }

    #[test]
    fn test_openai_params_with_seed() {
        let params = OpenAiParams::with_seed(42);
        assert_eq!(params.seed, Some(42));
        assert!(params.reasoning_effort.is_none());
    }

    #[test]
    fn test_gemini_params_with_top_k() {
        let params = GeminiParams::with_top_k(40);
        assert_eq!(params.top_k, Some(40));
        assert!(params.thinking.is_none());
    }

    #[test]
    fn test_anthropic_params_default() {
        let params = AnthropicParams::default();
        assert!(params.thinking.is_none());
    }

    #[test]
    fn test_openai_params_default() {
        let params = OpenAiParams::default();
        assert!(params.reasoning_effort.is_none());
        assert!(params.seed.is_none());
        assert!(params.frequency_penalty.is_none());
        assert!(params.presence_penalty.is_none());
    }

    #[test]
    fn test_gemini_params_default() {
        let params = GeminiParams::default();
        assert!(params.thinking.is_none());
        assert!(params.top_k.is_none());
        assert!(params.top_p.is_none());
    }

    /// Regression: Typed AnthropicParams must be extractable by build_request_body.
    /// The provider code extracts thinking.budget_tokens from the serialized JSON.
    #[test]
    fn test_regression_anthropic_params_extractable() -> Result<(), Box<dyn std::error::Error>> {
        let params = ProviderParams::Anthropic(AnthropicParams::with_thinking(10000));
        let json = serde_json::to_value(&params)?;

        // Provider code checks: params["thinking"]["budget_tokens"]
        let budget = json
            .get("thinking")
            .and_then(|t| t.get("budget_tokens"))
            .and_then(|v| v.as_u64());

        assert_eq!(
            budget,
            Some(10000),
            "thinking.budget_tokens must be extractable"
        );
        Ok(())
    }

    /// Regression: Typed GeminiParams must be extractable by build_request_body.
    /// The provider code extracts thinking.thinking_budget and top_p from serialized JSON.
    #[test]
    fn test_regression_gemini_params_extractable() -> Result<(), Box<dyn std::error::Error>> {
        let mut params = GeminiParams::with_thinking(8000);
        params.top_p = Some(0.95);
        let provider_params = ProviderParams::Gemini(params);
        let json = serde_json::to_value(&provider_params)?;

        // Provider code checks: params["thinking"]["thinking_budget"]
        let budget = json
            .get("thinking")
            .and_then(|t| t.get("thinking_budget"))
            .and_then(|v| v.as_u64());
        assert_eq!(
            budget,
            Some(8000),
            "thinking.thinking_budget must be extractable"
        );

        // Provider code checks: params["top_p"]
        let top_p = json.get("top_p").and_then(|v| v.as_f64());
        assert!(
            (top_p.unwrap_or(0.0) - 0.95).abs() < 0.001,
            "top_p must be extractable"
        );

        Ok(())
    }
}
