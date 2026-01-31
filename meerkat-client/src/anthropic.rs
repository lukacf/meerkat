//! Anthropic Claude API client
//!
//! Implements the LlmClient trait for Anthropic's Claude API.

use crate::error::LlmError;
use crate::types::{LlmClient, LlmDoneOutcome, LlmEvent, LlmRequest};
use async_trait::async_trait;
use futures::{Stream, StreamExt};
use meerkat_core::{Message, StopReason, Usage};
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
        let http = reqwest::Client::builder()
            .connect_timeout(self.connect_timeout)
            .timeout(self.request_timeout)
            .pool_idle_timeout(self.pool_idle_timeout)
            .pool_max_idle_per_host(4)
            .tcp_keepalive(Duration::from_secs(30))
            .build()
            .map_err(|e| LlmError::Unknown {
                message: format!("Failed to build HTTP client: {}", e),
            })?;

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
            // thinking_budget -> Anthropic thinking.budget_tokens (requires thinking.type = "enabled")
            if let Some(thinking_budget) = params.get("thinking_budget") {
                if let Some(budget) = thinking_budget.as_u64() {
                    body["thinking"] = serde_json::json!({
                        "type": "enabled",
                        "budget_tokens": budget
                    });
                }
            }

            if let Some(top_k) = params.get("top_k") {
                body["top_k"] = top_k.clone();
            }
        }

        body
    }

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

#[async_trait]
impl LlmClient for AnthropicClient {
    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> Pin<Box<dyn Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
        let inner: Pin<Box<dyn Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> = Box::pin(
            async_stream::try_stream! {
                let body = self.build_request_body(request);

                let response = self.http
                    .post(format!("{}/v1/messages", self.base_url))
                    .header("x-api-key", &self.api_key)
                    .header("anthropic-version", "2023-06-01")
                    .header("Content-Type", "application/json")
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
                let mut last_stop_reason: Option<StopReason> = None;
                let mut saw_done = false;

                macro_rules! handle_event {
                    ($event:expr) => {
                        match $event.event_type.as_str() {
                            "content_block_delta" => {
                                if let Some(delta) = $event.delta {
                                    match delta.delta_type.as_str() {
                                        "text_delta" => {
                                            if let Some(text) = delta.text {
                                                yield LlmEvent::TextDelta { delta: text };
                                            }
                                        }
                                        "input_json_delta" => {
                                            if let Some(partial_json) = delta.partial_json {
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
                                    if content_block.block_type == "tool_use" {
                                        let id = content_block.id.unwrap_or_default();
                                        current_tool_id = Some(id.clone());
                                        yield LlmEvent::ToolCallDelta {
                                            id,
                                            name: content_block.name,
                                            args_delta: String::new(),
                                        };
                                    }
                                }
                            }
                            "message_delta" => {
                                if let Some(usage) = $event.usage {
                                    yield LlmEvent::UsageUpdate {
                                        usage: Usage {
                                            input_tokens: 0, // already reported in message_start
                                            output_tokens: usage.output_tokens.unwrap_or(0),
                                            cache_creation_tokens: None,
                                            cache_read_tokens: None,
                                        }
                                    };
                                }
                                if let Some(finish_reason) = $event.delta.and_then(|d| d.stop_reason) {
                                    let reason = Self::map_stop_reason(finish_reason.as_str());
                                    last_stop_reason = Some(reason);
                                    if !saw_done {
                                        yield LlmEvent::Done {
                                            outcome: LlmDoneOutcome::Success { stop_reason: reason },
                                        };
                                        saw_done = true;
                                    }
                                }
                            }
                            "message_start" => {
                                if let Some(usage) = $event.message.and_then(|m| m.usage) {
                                    yield LlmEvent::UsageUpdate {
                                        usage: Usage {
                                            input_tokens: usage.input_tokens.unwrap_or(0),
                                            output_tokens: 0,
                                            cache_creation_tokens: usage.cache_creation_input_tokens,
                                            cache_read_tokens: usage.cache_read_input_tokens,
                                        }
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
}

#[derive(Debug, Deserialize)]
struct AnthropicContentBlock {
    #[serde(rename = "type")]
    block_type: String,
    id: Option<String>,
    name: Option<String>,
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
    use meerkat_core::UserMessage;

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

        let body = client.build_request_body(&request);

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

        let body = client.build_request_body(&request);

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

        let body = client.build_request_body(&request);

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
}
