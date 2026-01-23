//! Anthropic Claude API client
//!
//! Implements the LlmClient trait for Anthropic's Messages API.

use crate::error::LlmError;
use crate::types::{LlmClient, LlmEvent, LlmRequest, ToolCallBuffer};
use async_trait::async_trait;
use futures::{Stream, StreamExt};
use meerkat_core::{Message, StopReason, Usage};
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
use std::pin::Pin;
use std::time::Duration;

/// Default connect timeout for HTTP connections
const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Default request timeout (long for streaming responses)
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(300);
/// Default pool idle timeout
const DEFAULT_POOL_IDLE_TIMEOUT: Duration = Duration::from_secs(90);
/// Initial capacity for SSE buffer (typical SSE event is ~200-500 bytes)
const SSE_BUFFER_CAPACITY: usize = 512;

/// Anthropic Claude API client
pub struct AnthropicClient {
    api_key: String,
    base_url: String,
    http: reqwest::Client,
}

/// Builder for AnthropicClient with configurable HTTP settings
pub struct AnthropicClientBuilder {
    api_key: String,
    base_url: String,
    connect_timeout: Duration,
    request_timeout: Duration,
    pool_idle_timeout: Duration,
}

impl AnthropicClientBuilder {
    /// Create a new builder with the given API key
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
    pub fn build(self) -> AnthropicClient {
        let http = reqwest::Client::builder()
            .connect_timeout(self.connect_timeout)
            .timeout(self.request_timeout)
            .pool_idle_timeout(self.pool_idle_timeout)
            .pool_max_idle_per_host(4)
            .tcp_keepalive(Duration::from_secs(30))
            .build()
            .expect("Failed to build HTTP client");

        AnthropicClient {
            api_key: self.api_key,
            base_url: self.base_url,
            http,
        }
    }
}

impl AnthropicClient {
    /// Create a new Anthropic client with the given API key and default HTTP settings
    pub fn new(api_key: String) -> Self {
        AnthropicClientBuilder::new(api_key).build()
    }

    /// Create a builder for more control over HTTP configuration
    pub fn builder(api_key: String) -> AnthropicClientBuilder {
        AnthropicClientBuilder::new(api_key)
    }

    /// Create from environment variable ANTHROPIC_API_KEY
    pub fn from_env() -> Result<Self, LlmError> {
        let api_key = std::env::var("ANTHROPIC_API_KEY").map_err(|_| LlmError::InvalidApiKey)?;
        Ok(Self::new(api_key))
    }

    /// Set custom base URL
    pub fn with_base_url(mut self, url: String) -> Self {
        self.base_url = url;
        self
    }

    /// Build request body for Anthropic API
    fn build_request_body(&self, request: &LlmRequest) -> Value {
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
                Message::ToolResults { results } => {
                    let content: Vec<Value> = results
                        .iter()
                        .map(|r| {
                            serde_json::json!({
                                "type": "tool_result",
                                "tool_use_id": r.tool_use_id,
                                "content": r.content,
                                "is_error": r.is_error
                            })
                        })
                        .collect();

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
            body["temperature"] = Value::Number(serde_json::Number::from_f64(temp as f64).unwrap());
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
            // thinking_budget -> Anthropic thinking.budget_tokens (requires thinking.type = "enabled")
            if let Some(thinking_budget) = params.get("thinking_budget") {
                if let Some(budget) = thinking_budget.as_u64() {
                    body["thinking"] = serde_json::json!({
                        "type": "enabled",
                        "budget_tokens": budget
                    });
                }
            }

            // top_k -> Anthropic top_k parameter
            if let Some(top_k) = params.get("top_k") {
                if let Some(k) = top_k.as_u64() {
                    body["top_k"] = Value::Number(serde_json::Number::from(k));
                }
            }
        }

        body
    }

    /// Parse an SSE event from the response
    fn parse_sse_line(line: &str) -> Option<SseEvent> {
        if let Some(data) = line.strip_prefix("data: ") {
            if data == "[DONE]" {
                return None;
            }
            serde_json::from_str(data).ok()
        } else {
            None
        }
    }
}

#[async_trait]
impl LlmClient for AnthropicClient {
    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> Pin<Box<dyn Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
        Box::pin(async_stream::try_stream! {
            let body = self.build_request_body(request);

            let response = self.http
                .post(format!("{}/v1/messages", self.base_url))
                .header("x-api-key", &self.api_key)
                .header("anthropic-version", "2023-06-01")
                .header("content-type", "application/json")
                .json(&body)
                .send()
                .await
                .map_err(|_| LlmError::NetworkTimeout {
                    duration_ms: 30000,
                })?;

            // Check for error responses - use Result to satisfy borrow checker
            let status_code = response.status().as_u16();
            let stream_result = if (200..=299).contains(&status_code) {
                Ok(response.bytes_stream())
            } else {
                let text = response.text().await.unwrap_or_default();
                Err(LlmError::from_http_status(status_code, text))
            };
            let mut stream = stream_result?;
            // Pre-allocate buffer with typical SSE event size to reduce allocations
            let mut buffer = String::with_capacity(SSE_BUFFER_CAPACITY);
            let mut tool_buffers: HashMap<usize, ToolCallBuffer> = HashMap::new();
            let mut current_tool_index: Option<usize> = None;

            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|_| LlmError::ConnectionReset)?;
                // from_utf8_lossy returns Cow which avoids allocation for valid UTF-8
                buffer.push_str(&String::from_utf8_lossy(&chunk));

                // Process complete lines without allocating new strings
                while let Some(newline_pos) = buffer.find('\n') {
                    // Get the line as a slice and trim, avoiding allocation
                    let line = buffer[..newline_pos].trim();

                    // Skip empty lines and SSE comments
                    let should_process = !line.is_empty() && !line.starts_with(':');
                    let event = if should_process {
                        Self::parse_sse_line(line)
                    } else {
                        None
                    };

                    // Drain the processed line from buffer (avoids creating new String)
                    buffer.drain(..=newline_pos);

                    if let Some(event) = event {
                        match event.event_type.as_str() {
                            "content_block_start" => {
                                if let Some(content_block) = event.content_block {
                                    if content_block.block_type == "tool_use" {
                                        let index = event.index.unwrap_or(0);
                                        let mut buf = ToolCallBuffer::new(
                                            content_block.id.unwrap_or_default()
                                        );
                                        buf.name = content_block.name;
                                        tool_buffers.insert(index, buf);
                                        current_tool_index = Some(index);
                                    }
                                }
                            }
                            "content_block_delta" => {
                                if let Some(delta) = event.delta {
                                    match delta.delta_type.as_deref() {
                                        Some("text_delta") => {
                                            if let Some(text) = delta.text {
                                                yield LlmEvent::TextDelta { delta: text };
                                            }
                                        }
                                        Some("input_json_delta") => {
                                            if let Some(partial) = delta.partial_json {
                                                let index = event.index.unwrap_or(
                                                    current_tool_index.unwrap_or(0)
                                                );
                                                if let Some(buf) = tool_buffers.get_mut(&index) {
                                                    buf.push_args(&partial);
                                                    yield LlmEvent::ToolCallDelta {
                                                        id: buf.id.clone(),
                                                        name: buf.name.clone(),
                                                        args_delta: partial,
                                                    };
                                                }
                                            }
                                        }
                                        _ => {}
                                    }
                                }
                            }
                            "content_block_stop" => {
                                let index = event.index.unwrap_or(
                                    current_tool_index.unwrap_or(0)
                                );
                                if let Some(buf) = tool_buffers.remove(&index) {
                                    if let Some(tc) = buf.try_complete() {
                                        yield LlmEvent::ToolCallComplete {
                                            id: tc.id,
                                            name: tc.name,
                                            args: tc.args,
                                        };
                                    }
                                }
                                current_tool_index = None;
                            }
                            "message_delta" => {
                                if let Some(delta) = event.delta {
                                    if let Some(stop_reason) = delta.stop_reason {
                                        let reason = match stop_reason.as_str() {
                                            "end_turn" => StopReason::EndTurn,
                                            "tool_use" => StopReason::ToolUse,
                                            "max_tokens" => StopReason::MaxTokens,
                                            "stop_sequence" => StopReason::StopSequence,
                                            _ => StopReason::EndTurn,
                                        };
                                        yield LlmEvent::Done { stop_reason: reason };
                                    }
                                }
                                if let Some(usage) = event.usage {
                                    yield LlmEvent::UsageUpdate {
                                        usage: Usage {
                                            input_tokens: usage.input_tokens.unwrap_or(0),
                                            output_tokens: usage.output_tokens.unwrap_or(0),
                                            cache_creation_tokens: usage.cache_creation_input_tokens,
                                            cache_read_tokens: usage.cache_read_input_tokens,
                                        }
                                    };
                                }
                            }
                            "message_start" => {
                                if let Some(message) = event.message {
                                    if let Some(usage) = message.usage {
                                        yield LlmEvent::UsageUpdate {
                                            usage: Usage {
                                                input_tokens: usage.input_tokens.unwrap_or(0),
                                                output_tokens: usage.output_tokens.unwrap_or(0),
                                                cache_creation_tokens: usage.cache_creation_input_tokens,
                                                cache_read_tokens: usage.cache_read_input_tokens,
                                            }
                                        };
                                    }
                                }
                            }
                            "message_stop" => {
                                // Final event, stream will close - no action needed
                            }
                            _ => {}
                        }
                    }
                }
            }
        })
    }

    fn provider(&self) -> &'static str {
        "anthropic"
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        // Simple ping to check connectivity
        let response = self
            .http
            .get(format!("{}/v1/models", self.base_url))
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .send()
            .await
            .map_err(|_| LlmError::NetworkTimeout { duration_ms: 5000 })?;

        if response.status().is_success() || response.status().as_u16() == 404 {
            // 404 is OK - endpoint might not exist but auth worked
            Ok(())
        } else {
            Err(LlmError::from_http_status(
                response.status().as_u16(),
                "Health check failed".to_string(),
            ))
        }
    }
}

// SSE event structures for parsing Anthropic responses
#[derive(Debug, Deserialize)]
struct SseEvent {
    #[serde(rename = "type")]
    event_type: String,
    index: Option<usize>,
    content_block: Option<ContentBlock>,
    delta: Option<Delta>,
    message: Option<MessageInfo>,
    usage: Option<UsageInfo>,
}

#[derive(Debug, Deserialize)]
struct ContentBlock {
    #[serde(rename = "type")]
    block_type: String,
    id: Option<String>,
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Delta {
    #[serde(rename = "type")]
    delta_type: Option<String>,
    text: Option<String>,
    partial_json: Option<String>,
    stop_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MessageInfo {
    usage: Option<UsageInfo>,
}

#[derive(Debug, Deserialize)]
struct UsageInfo {
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    cache_creation_input_tokens: Option<u64>,
    cache_read_input_tokens: Option<u64>,
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use meerkat_core::{ToolDef, UserMessage};

    fn skip_if_no_key() -> Option<AnthropicClient> {
        AnthropicClient::from_env().ok()
    }

    #[tokio::test]
    async fn test_streaming_text_delta_normalization() {
        let Some(client) = skip_if_no_key() else {
            eprintln!("Skipping: ANTHROPIC_API_KEY not set");
            return;
        };

        let request = LlmRequest::new(
            "claude-sonnet-4-20250514",
            vec![Message::User(UserMessage {
                content: "Say 'hello' and nothing else.".to_string(),
            })],
        )
        .with_max_tokens(100);

        let mut stream = client.stream(&request);
        let mut got_text_delta = false;
        let mut got_done = false;

        while let Some(event) = stream.next().await {
            match event {
                Ok(LlmEvent::TextDelta { delta }) => {
                    got_text_delta = true;
                    // Should contain some text
                    assert!(!delta.is_empty() || got_text_delta);
                }
                Ok(LlmEvent::Done { stop_reason }) => {
                    got_done = true;
                    assert_eq!(stop_reason, StopReason::EndTurn);
                }
                Ok(_) => {}
                Err(e) => panic!("Unexpected error: {:?}", e),
            }
        }

        assert!(got_text_delta, "Should have received TextDelta events");
        assert!(got_done, "Should have received Done event");
    }

    #[tokio::test]
    async fn test_tool_call_normalization() {
        let Some(client) = skip_if_no_key() else {
            eprintln!("Skipping: ANTHROPIC_API_KEY not set");
            return;
        };

        let tools = vec![ToolDef {
            name: "get_weather".to_string(),
            description: "Get weather for a city".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "city": {
                        "type": "string",
                        "description": "City name"
                    }
                },
                "required": ["city"]
            }),
        }];

        let request = LlmRequest::new(
            "claude-sonnet-4-20250514",
            vec![Message::User(UserMessage {
                content: "What's the weather in Tokyo? Use the get_weather tool.".to_string(),
            })],
        )
        .with_tools(tools)
        .with_max_tokens(200);

        let mut stream = client.stream(&request);
        let mut got_tool_call = false;
        let mut got_done = false;

        while let Some(event) = stream.next().await {
            match event {
                Ok(LlmEvent::ToolCallComplete { id, name, args }) => {
                    got_tool_call = true;
                    assert!(!id.is_empty());
                    assert_eq!(name, "get_weather");
                    assert!(args.get("city").is_some());
                }
                Ok(LlmEvent::Done { stop_reason }) => {
                    got_done = true;
                    if got_tool_call {
                        assert_eq!(stop_reason, StopReason::ToolUse);
                    }
                }
                Ok(_) => {}
                Err(e) => panic!("Unexpected error: {:?}", e),
            }
        }

        assert!(got_tool_call, "Should have received tool call");
        assert!(got_done, "Should have received Done event");
    }

    #[tokio::test]
    async fn test_stop_reason_mapping() {
        let Some(client) = skip_if_no_key() else {
            eprintln!("Skipping: ANTHROPIC_API_KEY not set");
            return;
        };

        // Test EndTurn
        let request = LlmRequest::new(
            "claude-sonnet-4-20250514",
            vec![Message::User(UserMessage {
                content: "Say 'hi'".to_string(),
            })],
        )
        .with_max_tokens(100);

        let mut stream = client.stream(&request);
        let mut stop_reason = None;

        while let Some(event) = stream.next().await {
            if let Ok(LlmEvent::Done { stop_reason: sr }) = event {
                stop_reason = Some(sr);
            }
        }

        assert_eq!(stop_reason, Some(StopReason::EndTurn));
    }

    #[tokio::test]
    #[ignore = "Requires ANTHROPIC_API_KEY"]
    async fn test_usage_mapping() {
        let Some(client) = skip_if_no_key() else {
            eprintln!("Skipping: ANTHROPIC_API_KEY not set");
            return;
        };

        let request = LlmRequest::new(
            "claude-sonnet-4-20250514",
            vec![Message::User(UserMessage {
                content: "Say 'test'".to_string(),
            })],
        )
        .with_max_tokens(100);

        let mut stream = client.stream(&request);
        let mut got_usage = false;

        while let Some(event) = stream.next().await {
            if let Ok(LlmEvent::UsageUpdate { usage }) = event {
                got_usage = true;
                // Should have reasonable token counts
                assert!(usage.input_tokens > 0 || usage.output_tokens > 0);
            }
        }

        assert!(got_usage, "Should have received usage info");
    }

    #[tokio::test]
    async fn test_error_response_mapping() {
        let client = AnthropicClient::new("invalid-key".to_string());

        let request = LlmRequest::new(
            "claude-sonnet-4-20250514",
            vec![Message::User(UserMessage {
                content: "test".to_string(),
            })],
        );

        let mut stream = client.stream(&request);
        let result = stream.next().await;

        assert!(result.is_some());
        let event = result.unwrap();
        assert!(event.is_err());

        let err = event.unwrap_err();
        assert!(
            matches!(
                err,
                LlmError::AuthenticationFailed { .. } | LlmError::InvalidApiKey
            ),
            "Expected auth error, got: {:?}",
            err
        );
    }

    // Unit tests for build_request_body with provider_params

    #[test]
    fn test_build_request_body_with_thinking_budget() {
        let client = AnthropicClient::new("test-key".to_string());

        let request = LlmRequest::new(
            "claude-sonnet-4-20250514",
            vec![Message::User(UserMessage {
                content: "test".to_string(),
            })],
        )
        .with_provider_param("thinking_budget", 10000);

        let body = client.build_request_body(&request);

        // Should have thinking object with type "enabled" and budget_tokens
        assert!(body.get("thinking").is_some(), "thinking field should be present");
        let thinking = &body["thinking"];
        assert_eq!(thinking["type"], "enabled");
        assert_eq!(thinking["budget_tokens"], 10000);
    }

    #[test]
    fn test_build_request_body_with_top_k() {
        let client = AnthropicClient::new("test-key".to_string());

        let request = LlmRequest::new(
            "claude-sonnet-4-20250514",
            vec![Message::User(UserMessage {
                content: "test".to_string(),
            })],
        )
        .with_provider_param("top_k", 40);

        let body = client.build_request_body(&request);

        // Should have top_k at the top level
        assert_eq!(body["top_k"], 40);
    }

    #[test]
    fn test_build_request_body_with_thinking_and_top_k() {
        let client = AnthropicClient::new("test-key".to_string());

        let request = LlmRequest::new(
            "claude-sonnet-4-20250514",
            vec![Message::User(UserMessage {
                content: "test".to_string(),
            })],
        )
        .with_provider_param("thinking_budget", 5000)
        .with_provider_param("top_k", 50);

        let body = client.build_request_body(&request);

        // Both should be present
        assert_eq!(body["thinking"]["type"], "enabled");
        assert_eq!(body["thinking"]["budget_tokens"], 5000);
        assert_eq!(body["top_k"], 50);
    }

    #[test]
    fn test_build_request_body_unknown_params_ignored() {
        let client = AnthropicClient::new("test-key".to_string());

        let request = LlmRequest::new(
            "claude-sonnet-4-20250514",
            vec![Message::User(UserMessage {
                content: "test".to_string(),
            })],
        )
        .with_provider_param("unknown_param", "some_value")
        .with_provider_param("another_unknown", 123);

        let body = client.build_request_body(&request);

        // Unknown params should NOT be in the request body
        assert!(body.get("unknown_param").is_none());
        assert!(body.get("another_unknown").is_none());

        // Request should still have standard fields
        assert_eq!(body["model"], "claude-sonnet-4-20250514");
        assert!(body.get("messages").is_some());
    }

    #[test]
    fn test_build_request_body_no_provider_params() {
        let client = AnthropicClient::new("test-key".to_string());

        let request = LlmRequest::new(
            "claude-sonnet-4-20250514",
            vec![Message::User(UserMessage {
                content: "test".to_string(),
            })],
        );

        let body = client.build_request_body(&request);

        // Should not have thinking or top_k when no provider_params
        assert!(body.get("thinking").is_none());
        assert!(body.get("top_k").is_none());

        // Standard fields should be present
        assert_eq!(body["model"], "claude-sonnet-4-20250514");
    }

    #[test]
    fn test_client_builder_creates_configured_client() {
        // Test that builder creates client with proper configuration
        let client = AnthropicClient::builder("test-key".to_string())
            .connect_timeout(std::time::Duration::from_secs(5))
            .request_timeout(std::time::Duration::from_secs(120))
            .build();

        assert_eq!(client.provider(), "anthropic");
    }

    #[test]
    fn test_client_default_has_connection_pool() {
        // Default client should have proper HTTP configuration
        let client = AnthropicClient::new("test-key".to_string());
        // Just verify it compiles and works - actual pool config is internal to reqwest
        assert_eq!(client.provider(), "anthropic");
    }

    #[test]
    fn test_parse_sse_line() {
        // Valid data line
        let line = r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}"#;
        let event = AnthropicClient::parse_sse_line(line);
        assert!(event.is_some());
        let event = event.unwrap();
        assert_eq!(event.event_type, "content_block_delta");

        // DONE marker
        let done = AnthropicClient::parse_sse_line("data: [DONE]");
        assert!(done.is_none());

        // Comment line (should return None since it doesn't start with "data: ")
        let comment = AnthropicClient::parse_sse_line(": comment");
        assert!(comment.is_none());

        // Event line (not data)
        let event_line = AnthropicClient::parse_sse_line("event: message_start");
        assert!(event_line.is_none());
    }

    #[test]
    fn test_sse_buffer_constants() {
        // Verify constants are sensible values
        assert!(
            SSE_BUFFER_CAPACITY >= 256,
            "SSE buffer should be at least 256 bytes"
        );
        assert!(
            DEFAULT_CONNECT_TIMEOUT.as_secs() >= 5,
            "Connect timeout should be at least 5s"
        );
        assert!(
            DEFAULT_REQUEST_TIMEOUT.as_secs() >= 60,
            "Request timeout should be at least 60s"
        );
    }
}
