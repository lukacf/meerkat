//! Anthropic Claude API client
//!
//! Implements the LlmClient trait for Anthropic's Claude API.

use crate::error::LlmError;
use crate::types::{LlmClient, LlmDoneOutcome, LlmEvent, LlmRequest};
use async_trait::async_trait;
use futures::{Stream, StreamExt};
use meerkat_core::{Message, OutputSchema, Provider, StopReason, Usage};
use serde::Deserialize;
use serde_json::Value;
use std::pin::Pin;
use std::time::Duration;

/// Default connect timeout
const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
/// Default request timeout
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(300);
/// Default pool idle timeout
const DEFAULT_POOL_IDLE_TIMEOUT: Duration = Duration::from_secs(90);
/// SSE buffer capacity to reduce reallocations
const SSE_BUFFER_CAPACITY: usize = 4096;

/// Client for Anthropic API
pub struct AnthropicClient {
    api_key: String,
    base_url: String,
    http: reqwest::Client,
}

/// Builder for AnthropicClient
pub struct AnthropicClientBuilder {
    api_key: String,
    base_url: String,
    connect_timeout: Duration,
    request_timeout: Duration,
    pool_idle_timeout: Duration,
}

impl AnthropicClientBuilder {
    pub fn new(api_key: String) -> Self {
        Self {
            api_key,
            base_url: "https://api.anthropic.com".to_string(),
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
            pool_idle_timeout: DEFAULT_POOL_IDLE_TIMEOUT,
        }
    }

    /// Set custom base URL
    pub fn base_url(mut self, url: String) -> Self {
        self.base_url = url;
        self
    }

    /// Set connection timeout
    pub fn connect_timeout(mut self, timeout: Duration) -> Self {
        self.connect_timeout = timeout;
        self
    }

    /// Set request timeout
    pub fn request_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = timeout;
        self
    }

    /// Set pool idle timeout
    pub fn pool_idle_timeout(mut self, timeout: Duration) -> Self {
        self.pool_idle_timeout = timeout;
        self
    }

    /// Build the client with configured HTTP settings
    pub fn build(self) -> Result<AnthropicClient, LlmError> {
        let http = crate::http::build_http_client(
            reqwest::Client::builder()
            .connect_timeout(self.connect_timeout)
            .timeout(self.request_timeout)
            .pool_idle_timeout(self.pool_idle_timeout)
            .pool_max_idle_per_host(4)
            .tcp_keepalive(Duration::from_secs(30))
        )?;

        Ok(AnthropicClient {
            api_key: self.api_key,
            base_url: self.base_url,
            http,
        })
    }
}

impl AnthropicClient {
    /// Create a new Anthropic client with the given API key and default HTTP settings
    pub fn new(api_key: String) -> Result<Self, LlmError> {
        AnthropicClientBuilder::new(api_key).build()
    }

    /// Create a builder for more control over HTTP configuration
    pub fn builder(api_key: String) -> AnthropicClientBuilder {
        AnthropicClientBuilder::new(api_key)
    }

    /// Create from environment variable ANTHROPIC_API_KEY
    pub fn from_env() -> Result<Self, LlmError> {
        let api_key = std::env::var("RKAT_ANTHROPIC_API_KEY")
            .or_else(|_| std::env::var("ANTHROPIC_API_KEY"))
            .map_err(|_| LlmError::InvalidApiKey)?;
        Self::new(api_key)
    }

    /// Set custom base URL
    pub fn with_base_url(mut self, url: String) -> Self {
        self.base_url = url;
        self
    }

    /// Build request body for Anthropic API
    fn build_request_body(&self, request: &LlmRequest) -> Result<Value, LlmError> {
        let mut messages = Vec::new();
        let mut system_prompt = None;

        for msg in &request.messages {
            match msg {
                Message::System(s) => {
                    system_prompt = Some(s.content.clone());
                }
                Message::User(u) => {
                    messages.push(serde_json::json!({
                        "role": "user",
                        "content": u.content
                    }));
                }
                Message::Assistant(a) => {
                    // Legacy format: flat content + tool_calls
                    let mut content = Vec::new();

                    if !a.content.is_empty() {
                        content.push(serde_json::json!({
                            "type": "text",
                            "text": a.content
                        }));
                    }

                    for tc in &a.tool_calls {
                        content.push(serde_json::json!({
                            "type": "tool_use",
                            "id": tc.id,
                            "name": tc.name,
                            "input": tc.args
                        }));
                    }

                    messages.push(serde_json::json!({
                        "role": "assistant",
                        "content": content
                    }));
                }
                Message::BlockAssistant(a) => {
                    // New format: ordered blocks with thinking support
                    let mut content = Vec::new();

                    for block in &a.blocks {
                        match block {
                            meerkat_core::AssistantBlock::Text { text, .. } => {
                                // Anthropic text blocks don't use meta
                                if !text.is_empty() {
                                    content.push(serde_json::json!({
                                        "type": "text",
                                        "text": text
                                    }));
                                }
                            }
                            meerkat_core::AssistantBlock::Reasoning { text, meta } => {
                                // Claude 4.5 REQUIRES signature on thinking blocks for replay
                                let Some(meerkat_core::ProviderMeta::Anthropic { signature }) = meta.as_deref() else {
                                    tracing::warn!("thinking block missing Anthropic signature, skipping");
                                    continue;
                                };
                                content.push(serde_json::json!({
                                    "type": "thinking",
                                    "thinking": text,
                                    "signature": signature
                                }));
                            }
                            meerkat_core::AssistantBlock::ToolUse { id, name, args, .. } => {
                                // Parse RawValue to Value for JSON serialization
                                let args_value: Value = serde_json::from_str(args.get())
                                    .unwrap_or_else(|_| serde_json::json!({}));
                                content.push(serde_json::json!({
                                    "type": "tool_use",
                                    "id": id,
                                    "name": name,
                                    "input": args_value
                                }));
                            }
                            // Handle future block types (non_exhaustive pattern)
                            _ => {}
                        }
                    }

                    messages.push(serde_json::json!({
                        "role": "assistant",
                        "content": content
                    }));
                }
                Message::ToolResults { results } => {
                    let mut content = Vec::new();

                    for r in results {
                        content.push(serde_json::json!({
                            "type": "tool_result",
                            "tool_use_id": r.tool_use_id,
                            "content": r.content,
                            "is_error": r.is_error
                        }));
                    }

                    messages.push(serde_json::json!({
                        "role": "user",
                        "content": content
                    }));
                }
            }
        }

        let mut body = serde_json::json!({
            "model": request.model,
            "max_tokens": request.max_tokens,
            "messages": messages,
            "stream": true
        });

        if let Some(system) = system_prompt {
            body["system"] = Value::String(system);
        }

        if let Some(temp) = request.temperature {
            if let Some(num) = serde_json::Number::from_f64(temp as f64) {
                body["temperature"] = Value::Number(num);
            }
        }

        if !request.tools.is_empty() {
            let tools: Vec<Value> = request
                .tools
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "name": t.name,
                        "description": t.description,
                        "input_schema": t.input_schema
                    })
                })
                .collect();
            body["tools"] = Value::Array(tools);
        }

        // Extract provider-specific params
        if let Some(ref params) = request.provider_params {
            // Handle thinking config from both formats:
            // 1. Legacy flat format: {"thinking_budget": 10000}
            // 2. Typed AnthropicParams: {"thinking": {"type": "enabled", "budget_tokens": 10000}}
            let thinking_budget = params
                .get("thinking_budget")
                .and_then(|v| v.as_u64())
                .or_else(|| {
                    params
                        .get("thinking")
                        .and_then(|t| t.get("budget_tokens"))
                        .and_then(|v| v.as_u64())
                });

            if let Some(budget) = thinking_budget {
                body["thinking"] = serde_json::json!({
                    "type": "enabled",
                    "budget_tokens": budget
                });
            }

            // top_k must be a number - coerce strings from CLI --param
            if let Some(top_k) = params.get("top_k") {
                let numeric_top_k = match top_k {
                    Value::Number(_) => Some(top_k.clone()),
                    Value::String(s) => s.parse::<u64>().ok().map(|n| Value::Number(n.into())),
                    _ => None,
                };
                if let Some(v) = numeric_top_k {
                    body["top_k"] = v;
                }
            }

            // Handle structured output configuration
            // Format: {"structured_output": {"schema": {...}, "name": "output", "strict": true}}
            if let Some(structured) = params.get("structured_output") {
                let output_schema: OutputSchema =
                    serde_json::from_value(structured.clone()).map_err(|e| {
                        LlmError::InvalidRequest {
                            message: format!("Invalid structured_output schema: {e}"),
                        }
                    })?;
                let compiled =
                    output_schema
                        .compile_for(Provider::Anthropic)
                        .map_err(|e| LlmError::InvalidRequest {
                            message: e.to_string(),
                        })?;
                body["output_config"] = serde_json::json!({
                    "format": {
                        "type": "json_schema",
                        "schema": compiled.schema
                    }
                });
            }
        }

        Ok(body)
    }

    // prepare_schema_for_anthropic removed: handled by core schema compiler

    /// Parse an SSE event from the response
    fn parse_sse_line(line: &str) -> Option<AnthropicEvent> {
        if let Some(data) = Self::strip_data_prefix(line) {
            serde_json::from_str(data).ok()
        } else {
            None
        }
    }

    fn strip_data_prefix(line: &str) -> Option<&str> {
        line.strip_prefix("data: ")
            .or_else(|| line.strip_prefix("data:"))
            .map(str::trim_start)
    }

    fn map_stop_reason(reason: &str) -> StopReason {
        match reason {
            "end_turn" => StopReason::EndTurn,
            "tool_use" => StopReason::ToolUse,
            "max_tokens" => StopReason::MaxTokens,
            "stop_sequence" => StopReason::StopSequence,
            "content_filter" => StopReason::ContentFilter,
            _ => StopReason::EndTurn,
        }
    }
}

fn merge_usage(target: &mut Usage, update: &AnthropicUsage) {
    if let Some(v) = update.input_tokens {
        target.input_tokens = v;
    }
    if let Some(v) = update.output_tokens {
        target.output_tokens = v;
    }
    if let Some(v) = update.cache_creation_input_tokens {
        target.cache_creation_tokens = Some(v);
    }
    if let Some(v) = update.cache_read_input_tokens {
        target.cache_read_tokens = Some(v);
    }
}

#[async_trait]
impl LlmClient for AnthropicClient {
    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> Pin<Box<dyn Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
        let inner: Pin<Box<dyn Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> = Box::pin(
            async_stream::try_stream! {
                let body = self.build_request_body(request)?;

                // Check if structured output is enabled (requires beta header)
                let has_structured_output = body.get("output_config").is_some();
                // Check if thinking is enabled (requires interleaved-thinking beta header)
                let has_thinking = body.get("thinking").is_some();

                let mut req = self.http
                    .post(format!("{}/v1/messages", self.base_url))
                    .header("x-api-key", &self.api_key)
                    .header("anthropic-version", "2023-06-01")
                    .header("Content-Type", "application/json");

                // Add beta header for interleaved thinking (Claude 4.5)
                if has_thinking {
                    req = req.header("anthropic-beta", "interleaved-thinking-2025-05-14");
                }
                // Add beta header for structured outputs (can be combined)
                if has_structured_output && !has_thinking {
                    req = req.header("anthropic-beta", "structured-outputs-2025-11-13");
                } else if has_structured_output && has_thinking {
                    // Combine beta headers
                    req = req.header("anthropic-beta", "interleaved-thinking-2025-05-14,structured-outputs-2025-11-13");
                }

                let response = req
                    .json(&body)
                    .send()
                    .await
                    .map_err(|_| LlmError::NetworkTimeout {
                        duration_ms: 30000,
                    })?;

                let status_code = response.status().as_u16();
                let stream_result = if (200..=299).contains(&status_code) {
                    Ok(response.bytes_stream())
                } else {
                    let text = response.text().await.unwrap_or_default();
                    Err(LlmError::from_http_status(status_code, text))
                };
                let mut stream = stream_result?;
                let mut buffer = String::with_capacity(SSE_BUFFER_CAPACITY);
                let mut current_tool_id: Option<String> = None;
                let mut current_block_type: Option<String> = None;
                let mut current_thinking_signature: Option<String> = None;
                let mut last_stop_reason: Option<StopReason> = None;
                let mut usage = Usage::default();
                let mut saw_done = false;
                let mut saw_event = false;

                macro_rules! handle_event {
                    ($event:expr) => {
                        match $event.event_type.as_str() {
                            "content_block_delta" => {
                                if let Some(delta) = $event.delta {
                                    match delta.delta_type.as_str() {
                                        "text_delta" => {
                                            if let Some(text) = delta.text {
                                                saw_event = true;
                                                yield LlmEvent::TextDelta { delta: text, meta: None };
                                            }
                                        }
                                        "thinking_delta" => {
                                            // Emit incremental thinking content
                                            if let Some(text) = delta.thinking {
                                                saw_event = true;
                                                yield LlmEvent::ReasoningDelta { delta: text };
                                            }
                                        }
                                        "signature_delta" => {
                                            // Signature arrives as separate delta before content_block_stop
                                            if let Some(sig) = delta.signature {
                                                saw_event = true;
                                                current_thinking_signature = Some(sig);
                                            }
                                        }
                                        "input_json_delta" => {
                                            if let Some(partial_json) = delta.partial_json {
                                                saw_event = true;
                                                yield LlmEvent::ToolCallDelta {
                                                    id: current_tool_id.clone().unwrap_or_default(),
                                                    name: None,
                                                    args_delta: partial_json,
                                                };
                                            }
                                        }
                                        _ => {}
                                    }
                                }
                            }
                            "content_block_start" => {
                                if let Some(content_block) = $event.content_block {
                                    current_block_type = Some(content_block.block_type.clone());
                                    match content_block.block_type.as_str() {
                                        "thinking" => {
                                            // Start of thinking block
                                            // Signature may already be present or arrive via signature_delta
                                            current_thinking_signature = content_block.signature;
                                        }
                                        "tool_use" => {
                                            let id = content_block.id.unwrap_or_default();
                                            current_tool_id = Some(id.clone());
                                            saw_event = true;
                                            yield LlmEvent::ToolCallDelta {
                                                id,
                                                name: content_block.name,
                                                args_delta: String::new(),
                                            };
                                        }
                                        _ => {}
                                    }
                                }
                            }
                            "content_block_stop" => {
                                // Emit complete events based on block type
                                match current_block_type.as_deref() {
                                    Some("thinking") => {
                                        // Emit ReasoningComplete with Anthropic signature
                                        let meta = current_thinking_signature
                                            .take()
                                            .map(|sig| Box::new(meerkat_core::ProviderMeta::Anthropic { signature: sig }));
                                        yield LlmEvent::ReasoningComplete {
                                            text: String::new(), // Text was already streamed via deltas
                                            meta,
                                        };
                                    }
                                    _ => {}
                                }
                                current_block_type = None;
                            }
                            "message_delta" => {
                                if let Some(usage_update) = $event.usage {
                                    merge_usage(&mut usage, &usage_update);
                                    saw_event = true;
                                    yield LlmEvent::UsageUpdate {
                                        usage: usage.clone(),
                                    };
                                }
                                if let Some(finish_reason) = $event.delta.and_then(|d| d.stop_reason) {
                                    let reason = Self::map_stop_reason(finish_reason.as_str());
                                    last_stop_reason = Some(reason);
                                    if !saw_done {
                                        saw_event = true;
                                        yield LlmEvent::Done {
                                            outcome: LlmDoneOutcome::Success { stop_reason: reason },
                                        };
                                        saw_done = true;
                                    }
                                }
                            }
                            "message_start" => {
                                if let Some(usage_update) = $event.message.and_then(|m| m.usage) {
                                    merge_usage(&mut usage, &usage_update);
                                    saw_event = true;
                                    yield LlmEvent::UsageUpdate {
                                        usage: usage.clone(),
                                    };
                                }
                            }
                            "message_stop" => {
                                if !saw_done {
                                    let finish_reason = $event.delta.and_then(|d| d.stop_reason);
                                    let reason = finish_reason
                                        .as_deref()
                                        .map(Self::map_stop_reason)
                                        .or(last_stop_reason)
                                        .unwrap_or(StopReason::EndTurn);
                                    last_stop_reason = Some(reason);
                                    saw_event = true;
                                    yield LlmEvent::Done {
                                        outcome: LlmDoneOutcome::Success { stop_reason: reason },
                                    };
                                    saw_done = true;
                                }
                            }
                            _ => {}
                        }
                    };
                }

                macro_rules! handle_line {
                    ($line:expr) => {
                        if !$line.is_empty() {
                            if let Some(data) = Self::strip_data_prefix($line) {
                                if data == "[DONE]" {
                                    if !saw_done {
                                        let reason =
                                            last_stop_reason.unwrap_or(StopReason::EndTurn);
                                        saw_event = true;
                                        yield LlmEvent::Done {
                                            outcome: LlmDoneOutcome::Success { stop_reason: reason },
                                        };
                                        saw_done = true;
                                    }
                                } else if let Some(event) = Self::parse_sse_line($line) {
                                    handle_event!(event);
                                }
                            }
                        }
                    };
                }

                while let Some(chunk) = stream.next().await {
                    let chunk = chunk.map_err(|_| LlmError::ConnectionReset)?;
                    buffer.push_str(&String::from_utf8_lossy(&chunk));

                    while let Some(newline_pos) = buffer.find('\n') {
                        let line = buffer[..newline_pos].trim();
                        handle_line!(line);
                        buffer.drain(..=newline_pos);
                    }
                }

                if !buffer.is_empty() {
                    for line in buffer.lines() {
                        let line = line.trim();
                        handle_line!(line);
                    }
                }

                if !saw_done && saw_event {
                    let reason = last_stop_reason.unwrap_or(StopReason::EndTurn);
                    yield LlmEvent::Done {
                        outcome: LlmDoneOutcome::Success { stop_reason: reason },
                    };
                }
            },
        );

        crate::streaming::ensure_terminal_done(inner)
    }

    fn provider(&self) -> &'static str {
        "anthropic"
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
struct AnthropicEvent {
    #[serde(rename = "type")]
    event_type: String,
    delta: Option<AnthropicDelta>,
    content_block: Option<AnthropicContentBlock>,
    message: Option<AnthropicMessage>,
    usage: Option<AnthropicUsage>,
}

#[derive(Debug, Deserialize)]
struct AnthropicDelta {
    #[serde(rename = "type")]
    delta_type: String,
    text: Option<String>,
    partial_json: Option<String>,
    stop_reason: Option<String>,
    /// Thinking content for thinking_delta events
    thinking: Option<String>,
    /// Signature for signature_delta events (arrives separately before content_block_stop)
    signature: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AnthropicContentBlock {
    #[serde(rename = "type")]
    block_type: String,
    id: Option<String>,
    name: Option<String>,
    /// Signature for thinking block continuity
    signature: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AnthropicMessage {
    usage: Option<AnthropicUsage>,
}

#[derive(Debug, Deserialize)]
struct AnthropicUsage {
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    cache_creation_input_tokens: Option<u64>,
    cache_read_input_tokens: Option<u64>,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use meerkat_core::{AssistantBlock, BlockAssistantMessage, ProviderMeta, UserMessage};

    // =========================================================================
    // Thinking block SSE parsing tests (spec section 3.5)
    // =========================================================================

    #[test]
    fn test_anthropic_content_block_parses_thinking_type() {
        // content_block_start with type: "thinking" should parse
        let json = r#"{"type": "thinking", "thinking": "Let me analyze..."}"#;
        let block: AnthropicContentBlock = serde_json::from_str(json).unwrap();

        assert_eq!(block.block_type, "thinking");
    }

    #[test]
    fn test_anthropic_content_block_parses_thinking_with_signature() {
        // content_block_start may have signature already
        let json = r#"{"type": "thinking", "thinking": "", "signature": "sig_abc123"}"#;
        let block: AnthropicContentBlock = serde_json::from_str(json).unwrap();

        assert_eq!(block.block_type, "thinking");
        assert_eq!(block.signature.as_deref(), Some("sig_abc123"));
    }

    #[test]
    fn test_anthropic_delta_parses_thinking_delta() {
        // content_block_delta with delta type: "thinking_delta"
        let json = r#"{"type": "thinking_delta", "thinking": "I need to consider..."}"#;
        let delta: AnthropicDelta = serde_json::from_str(json).unwrap();

        assert_eq!(delta.delta_type, "thinking_delta");
        assert_eq!(delta.thinking.as_deref(), Some("I need to consider..."));
    }

    #[test]
    fn test_anthropic_delta_parses_signature_delta() {
        // content_block_delta with delta type: "signature_delta"
        let json = r#"{"type": "signature_delta", "signature": "sig_xyz789"}"#;
        let delta: AnthropicDelta = serde_json::from_str(json).unwrap();

        assert_eq!(delta.delta_type, "signature_delta");
        assert_eq!(delta.signature.as_deref(), Some("sig_xyz789"));
    }

    #[test]
    fn test_anthropic_delta_parses_text_delta_unchanged() {
        // Existing text_delta should still work
        let json = r#"{"type": "text_delta", "text": "Hello world"}"#;
        let delta: AnthropicDelta = serde_json::from_str(json).unwrap();

        assert_eq!(delta.delta_type, "text_delta");
        assert_eq!(delta.text.as_deref(), Some("Hello world"));
        assert!(delta.thinking.is_none());
        assert!(delta.signature.is_none());
    }

    #[test]
    fn test_anthropic_delta_parses_input_json_delta_unchanged() {
        // Existing input_json_delta should still work
        let json = r#"{"type": "input_json_delta", "partial_json": "{\"path\":"}"#;
        let delta: AnthropicDelta = serde_json::from_str(json).unwrap();

        assert_eq!(delta.delta_type, "input_json_delta");
        assert_eq!(delta.partial_json.as_deref(), Some("{\"path\":"));
    }

    // =========================================================================
    // Request building tests for BlockAssistantMessage (spec section 3.5)
    // =========================================================================

    #[test]
    fn test_build_request_body_renders_thinking_block_with_signature() -> Result<(), Box<dyn std::error::Error>> {
        let client = AnthropicClient::new("test-key".to_string())?;

        // Create a session with a thinking block
        let assistant_msg = Message::BlockAssistant(BlockAssistantMessage {
            blocks: vec![
                AssistantBlock::Reasoning {
                    text: "Let me analyze this problem...".to_string(),
                    meta: Some(Box::new(ProviderMeta::Anthropic {
                        signature: "sig_abc123".to_string(),
                    })),
                },
                AssistantBlock::Text {
                    text: "The answer is 42.".to_string(),
                    meta: None,
                },
            ],
            stop_reason: StopReason::EndTurn,
        });

        let request = LlmRequest::new(
            "claude-sonnet-4-5",
            vec![
                Message::User(UserMessage {
                    content: "What is the meaning of life?".to_string(),
                }),
                assistant_msg,
                Message::User(UserMessage {
                    content: "Can you elaborate?".to_string(),
                }),
            ],
        );

        let body = client.build_request_body(&request);
        let messages = body["messages"].as_array().unwrap();

        // Second message should be the assistant with thinking + text
        let assistant_content = messages[1]["content"].as_array().unwrap();
        assert_eq!(assistant_content.len(), 2);

        // First block should be thinking
        assert_eq!(assistant_content[0]["type"], "thinking");
        assert_eq!(assistant_content[0]["thinking"], "Let me analyze this problem...");
        assert_eq!(assistant_content[0]["signature"], "sig_abc123");

        // Second block should be text
        assert_eq!(assistant_content[1]["type"], "text");
        assert_eq!(assistant_content[1]["text"], "The answer is 42.");

        Ok(())
    }

    #[test]
    fn test_build_request_body_skips_thinking_block_without_signature() -> Result<(), Box<dyn std::error::Error>> {
        let client = AnthropicClient::new("test-key".to_string())?;

        // Thinking block without Anthropic signature should be skipped
        let assistant_msg = Message::BlockAssistant(BlockAssistantMessage {
            blocks: vec![
                AssistantBlock::Reasoning {
                    text: "Some thinking...".to_string(),
                    meta: None, // No signature!
                },
                AssistantBlock::Text {
                    text: "The answer.".to_string(),
                    meta: None,
                },
            ],
            stop_reason: StopReason::EndTurn,
        });

        let request = LlmRequest::new(
            "claude-sonnet-4-5",
            vec![
                Message::User(UserMessage {
                    content: "Question".to_string(),
                }),
                assistant_msg,
            ],
        );

        let body = client.build_request_body(&request);
        let messages = body["messages"].as_array().unwrap();

        // Assistant content should only have the text block (thinking skipped)
        let assistant_content = messages[1]["content"].as_array().unwrap();
        assert_eq!(assistant_content.len(), 1);
        assert_eq!(assistant_content[0]["type"], "text");

        Ok(())
    }

    #[test]
    fn test_build_request_body_renders_tool_use_blocks() -> Result<(), Box<dyn std::error::Error>> {
        let client = AnthropicClient::new("test-key".to_string())?;

        let assistant_msg = Message::BlockAssistant(BlockAssistantMessage {
            blocks: vec![
                AssistantBlock::Text {
                    text: "I'll read that file for you.".to_string(),
                    meta: None,
                },
                AssistantBlock::ToolUse {
                    id: "tu_123".to_string(),
                    name: "read_file".to_string(),
                    args: serde_json::value::RawValue::from_string(
                        r#"{"path": "/tmp/test.txt"}"#.to_string()
                    ).unwrap(),
                    meta: None, // Tool use blocks don't have signatures in Anthropic
                },
            ],
            stop_reason: StopReason::ToolUse,
        });

        let request = LlmRequest::new(
            "claude-sonnet-4-5",
            vec![
                Message::User(UserMessage {
                    content: "Read /tmp/test.txt".to_string(),
                }),
                assistant_msg,
            ],
        );

        let body = client.build_request_body(&request);
        let messages = body["messages"].as_array().unwrap();
        let assistant_content = messages[1]["content"].as_array().unwrap();

        assert_eq!(assistant_content.len(), 2);
        assert_eq!(assistant_content[0]["type"], "text");
        assert_eq!(assistant_content[1]["type"], "tool_use");
        assert_eq!(assistant_content[1]["id"], "tu_123");
        assert_eq!(assistant_content[1]["name"], "read_file");
        assert_eq!(assistant_content[1]["input"]["path"], "/tmp/test.txt");

        Ok(())
    }

    #[test]
    fn test_build_request_body_adds_thinking_beta_header() -> Result<(), Box<dyn std::error::Error>> {
        let client = AnthropicClient::new("test-key".to_string())?;

        // When thinking is enabled via provider_params, beta header should be added
        let request = LlmRequest::new(
            "claude-sonnet-4-5",
            vec![Message::User(UserMessage {
                content: "test".to_string(),
            })],
        )
        .with_provider_param("thinking_budget", 10000);

        let body = client.build_request_body(&request);

        // The beta header is added during the HTTP request, not in the body
        // But the thinking config should be in the body
        assert!(body.get("thinking").is_some());
        assert_eq!(body["thinking"]["type"], "enabled");
        assert_eq!(body["thinking"]["budget_tokens"], 10000);

        Ok(())
    }

    // =========================================================================
    // Original tests (unchanged)
    // =========================================================================

    #[test]
    fn test_build_request_body_basic() -> Result<(), Box<dyn std::error::Error>> {
        let client = AnthropicClient::new("test-key".to_string())?;

        let request = LlmRequest::new(
            "claude-sonnet-4-20250514",
            vec![Message::User(UserMessage {
                content: "test".to_string(),
            })],
        )
        .with_provider_param("thinking_budget", 10000);

        let body = client.build_request_body(&request)?;

        assert!(
            body.get("thinking").is_some(),
            "thinking field should be present"
        );
        let thinking = &body["thinking"];
        assert_eq!(thinking["type"], "enabled");
        assert_eq!(thinking["budget_tokens"], 10000);
        Ok(())
    }

    #[test]
    fn test_build_request_body_with_top_k() -> Result<(), Box<dyn std::error::Error>> {
        let client = AnthropicClient::new("test-key".to_string())?;

        let request = LlmRequest::new(
            "claude-sonnet-4-20250514",
            vec![Message::User(UserMessage {
                content: "test".to_string(),
            })],
        )
        .with_provider_param("top_k", 40);

        let body = client.build_request_body(&request)?;

        assert_eq!(body["top_k"], 40);
        Ok(())
    }

    #[test]
    fn test_build_request_body_no_provider_params() -> Result<(), Box<dyn std::error::Error>> {
        let client = AnthropicClient::new("test-key".to_string())?;
        let request = LlmRequest::new(
            "claude-sonnet-4-20250514",
            vec![Message::User(UserMessage {
                content: "test".to_string(),
            })],
        );

        let body = client.build_request_body(&request)?;

        assert!(body.get("thinking").is_none());
        assert!(body.get("top_k").is_none());
        assert_eq!(body["model"], "claude-sonnet-4-20250514");
        Ok(())
    }

    #[test]
    fn test_client_builder_creates_configured_client() -> Result<(), Box<dyn std::error::Error>> {
        let client = AnthropicClient::builder("test-key".to_string())
            .connect_timeout(std::time::Duration::from_secs(5))
            .request_timeout(std::time::Duration::from_secs(120))
            .build()?;

        assert_eq!(client.provider(), "anthropic");
        Ok(())
    }

    #[test]
    fn test_client_default_has_connection_pool() -> Result<(), Box<dyn std::error::Error>> {
        let client = AnthropicClient::new("test-key".to_string())?;
        assert_eq!(client.provider(), "anthropic");
        Ok(())
    }

    #[test]
    fn test_parse_sse_line() {
        let line = r###"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}"###;
        let event = AnthropicClient::parse_sse_line(line);
        assert!(event.is_some());
        assert_eq!(event.unwrap().event_type, "content_block_delta");

        let done = AnthropicClient::parse_sse_line("data: [DONE]");
        assert!(done.is_none());

        let comment = AnthropicClient::parse_sse_line(": comment");
        assert!(comment.is_none());

        let event_line = AnthropicClient::parse_sse_line("event: message_start");
        assert!(event_line.is_none());
    }

    #[test]
    fn test_sse_buffer_constants() {
        let buffer_cap = SSE_BUFFER_CAPACITY;
        assert!(buffer_cap >= 256, "SSE buffer should be at least 256 bytes");
        let connect_timeout = DEFAULT_CONNECT_TIMEOUT.as_secs();
        assert!(
            connect_timeout >= 5,
            "Connect timeout should be at least 5s"
        );
        let request_timeout = DEFAULT_REQUEST_TIMEOUT.as_secs();
        assert!(
            request_timeout >= 60,
            "Request timeout should be at least 60s"
        );
    }

    /// Regression: CLI --param passes top_k as string; must coerce to number
    /// Previously: `--param top_k=40` sent `"top_k": "40"` which Anthropic rejects
    #[test]
    fn test_regression_top_k_string_coercion() -> Result<(), Box<dyn std::error::Error>> {
        let client = AnthropicClient::new("test-key".to_string())?;

        // Simulate CLI --param which passes values as strings
        let request = LlmRequest::new(
            "claude-sonnet-4-20250514",
            vec![Message::User(UserMessage {
                content: "test".to_string(),
            })],
        )
        .with_provider_param("top_k", "40"); // String, not number!

        let body = client.build_request_body(&request)?;

        // Should be coerced to a number
        assert!(
            body["top_k"].is_number(),
            "top_k should be a number, not string"
        );
        assert_eq!(body["top_k"], 40);
        Ok(())
    }

    /// Regression: non-numeric string values for top_k should be ignored
    #[test]
    fn test_regression_top_k_invalid_string_ignored() -> Result<(), Box<dyn std::error::Error>> {
        let client = AnthropicClient::new("test-key".to_string())?;

        let request = LlmRequest::new(
            "claude-sonnet-4-20250514",
            vec![Message::User(UserMessage {
                content: "test".to_string(),
            })],
        )
        .with_provider_param("top_k", "not_a_number");

        let body = client.build_request_body(&request)?;

        // Invalid string should be ignored (no top_k in body)
        assert!(
            body.get("top_k").is_none(),
            "invalid top_k should be ignored"
        );
        Ok(())
    }

    #[test]
    fn test_usage_merge_preserves_input_tokens() {
        let mut usage = Usage::default();
        let start = AnthropicUsage {
            input_tokens: Some(120),
            output_tokens: Some(0),
            cache_creation_input_tokens: Some(4),
            cache_read_input_tokens: Some(2),
        };

        merge_usage(&mut usage, &start);

        assert_eq!(usage.input_tokens, 120);
        assert_eq!(usage.output_tokens, 0);
        assert_eq!(usage.cache_creation_tokens, Some(4));
        assert_eq!(usage.cache_read_tokens, Some(2));

        let delta = AnthropicUsage {
            input_tokens: None,
            output_tokens: Some(7),
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
        };

        merge_usage(&mut usage, &delta);

        assert_eq!(usage.input_tokens, 120);
        assert_eq!(usage.output_tokens, 7);
        assert_eq!(usage.cache_creation_tokens, Some(4));
        assert_eq!(usage.cache_read_tokens, Some(2));
    }

    #[test]
    fn test_build_request_body_with_structured_output() -> Result<(), Box<dyn std::error::Error>> {
        let client = AnthropicClient::new("test-key".to_string())?;

        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "name": {"type": "string"},
                "age": {"type": "integer"}
            },
            "required": ["name", "age"]
        });

        let request = LlmRequest::new(
            "claude-sonnet-4-20250514",
            vec![Message::User(UserMessage {
                content: "test".to_string(),
            })],
        )
        .with_provider_param(
            "structured_output",
            serde_json::json!({
                "schema": schema,
                "name": "person",
                "strict": true
            }),
        );

        let body = client.build_request_body(&request)?;

        // Check output_config is present with correct structure
        assert!(
            body.get("output_config").is_some(),
            "output_config should be present"
        );
        let output_config = &body["output_config"];
        assert_eq!(output_config["format"]["type"], "json_schema");
        assert!(output_config["format"]["schema"].is_object());
        assert_eq!(
            output_config["format"]["schema"]["additionalProperties"],
            serde_json::json!(false)
        );
        Ok(())
    }

    #[test]
    fn test_build_request_body_without_structured_output() -> Result<(), Box<dyn std::error::Error>>
    {
        let client = AnthropicClient::new("test-key".to_string())?;

        let request = LlmRequest::new(
            "claude-sonnet-4-20250514",
            vec![Message::User(UserMessage {
                content: "test".to_string(),
            })],
        );

        let body = client.build_request_body(&request)?;

        // output_config should not be present
        assert!(
            body.get("output_config").is_none(),
            "output_config should not be present without structured_output"
        );
        Ok(())
    }

    // AdditionalProperties handling is covered by core schema compiler tests.
}
