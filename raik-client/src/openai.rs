//! OpenAI API client
//!
//! Implements the LlmClient trait for OpenAI's Chat Completions API.

use crate::error::LlmError;
use crate::types::{LlmClient, LlmEvent, LlmRequest, ToolCallBuffer};
use async_trait::async_trait;
use futures::{Stream, StreamExt};
use raik_core::{Message, StopReason, Usage};
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
use std::pin::Pin;

/// OpenAI API client
pub struct OpenAiClient {
    api_key: String,
    base_url: String,
    http: reqwest::Client,
}

impl OpenAiClient {
    /// Create a new OpenAI client with the given API key
    pub fn new(api_key: String) -> Self {
        Self {
            api_key,
            base_url: "https://api.openai.com".to_string(),
            http: reqwest::Client::new(),
        }
    }

    /// Create from environment variable OPENAI_API_KEY
    pub fn from_env() -> Result<Self, LlmError> {
        let api_key = std::env::var("OPENAI_API_KEY").map_err(|_| LlmError::InvalidApiKey)?;
        Ok(Self::new(api_key))
    }

    /// Set custom base URL (for Azure OpenAI or other compatible APIs)
    pub fn with_base_url(mut self, url: String) -> Self {
        self.base_url = url;
        self
    }

    /// Build request body for OpenAI API
    fn build_request_body(&self, request: &LlmRequest) -> Value {
        let mut messages = Vec::new();

        for msg in &request.messages {
            match msg {
                Message::System(s) => {
                    messages.push(serde_json::json!({
                        "role": "system",
                        "content": s.content
                    }));
                }
                Message::User(u) => {
                    messages.push(serde_json::json!({
                        "role": "user",
                        "content": u.content
                    }));
                }
                Message::Assistant(a) => {
                    let mut msg = serde_json::json!({
                        "role": "assistant",
                        "content": if a.content.is_empty() { Value::Null } else { Value::String(a.content.clone()) }
                    });

                    if !a.tool_calls.is_empty() {
                        let tool_calls: Vec<Value> = a
                            .tool_calls
                            .iter()
                            .map(|tc| {
                                serde_json::json!({
                                    "id": tc.id,
                                    "type": "function",
                                    "function": {
                                        "name": tc.name,
                                        "arguments": tc.args.to_string()
                                    }
                                })
                            })
                            .collect();
                        msg["tool_calls"] = Value::Array(tool_calls);
                    }

                    messages.push(msg);
                }
                Message::ToolResults { results } => {
                    for r in results {
                        messages.push(serde_json::json!({
                            "role": "tool",
                            "tool_call_id": r.tool_use_id,
                            "content": r.content
                        }));
                    }
                }
            }
        }

        let mut body = serde_json::json!({
            "model": request.model,
            "max_completion_tokens": request.max_tokens,
            "messages": messages,
            "stream": true,
            "stream_options": {"include_usage": true}
        });

        if let Some(temp) = request.temperature {
            body["temperature"] = Value::Number(serde_json::Number::from_f64(temp as f64).unwrap());
        }

        if !request.tools.is_empty() {
            let tools: Vec<Value> = request
                .tools
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "type": "function",
                        "function": {
                            "name": t.name,
                            "description": t.description,
                            "parameters": t.input_schema
                        }
                    })
                })
                .collect();
            body["tools"] = Value::Array(tools);
        }

        body
    }

    /// Parse an SSE event from the response
    fn parse_sse_line(line: &str) -> Option<ChatCompletionChunk> {
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
impl LlmClient for OpenAiClient {
    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> Pin<Box<dyn Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
        Box::pin(async_stream::try_stream! {
            let body = self.build_request_body(request);

            let response = self.http
                .post(format!("{}/v1/chat/completions", self.base_url))
                .header("Authorization", format!("Bearer {}", self.api_key))
                .header("Content-Type", "application/json")
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
            let mut buffer = String::new();
            let mut tool_buffers: HashMap<usize, ToolCallBuffer> = HashMap::new();

            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|_| LlmError::ConnectionReset)?;
                buffer.push_str(&String::from_utf8_lossy(&chunk));

                // Process complete lines
                while let Some(newline_pos) = buffer.find('\n') {
                    let line = buffer[..newline_pos].trim().to_string();
                    buffer = buffer[newline_pos + 1..].to_string();

                    if line.is_empty() || line.starts_with(':') {
                        continue;
                    }

                    if let Some(chunk) = Self::parse_sse_line(&line) {
                        // Handle usage info (comes in final chunk)
                        if let Some(usage) = chunk.usage {
                            yield LlmEvent::UsageUpdate {
                                usage: Usage {
                                    input_tokens: usage.prompt_tokens.unwrap_or(0),
                                    output_tokens: usage.completion_tokens.unwrap_or(0),
                                    cache_creation_tokens: None,
                                    cache_read_tokens: None,
                                }
                            };
                        }

                        // Handle choices
                        for choice in chunk.choices {
                            let delta = choice.delta;

                            // Text content
                            if let Some(content) = delta.content {
                                if !content.is_empty() {
                                    yield LlmEvent::TextDelta { delta: content };
                                }
                            }

                            // Tool calls
                            if let Some(tool_calls) = delta.tool_calls {
                                for tc in tool_calls {
                                    let index = tc.index.unwrap_or(0);

                                    // Initialize buffer if new
                                    if let std::collections::hash_map::Entry::Vacant(e) = tool_buffers.entry(index) {
                                        let id = tc.id.unwrap_or_default();
                                        let mut buf = ToolCallBuffer::new(id);
                                        if let Some(func) = &tc.function {
                                            buf.name = func.name.clone();
                                        }
                                        e.insert(buf);
                                    }

                                    // Append arguments
                                    if let Some(func) = &tc.function {
                                        if let Some(args) = &func.arguments {
                                            if let Some(buf) = tool_buffers.get_mut(&index) {
                                                buf.args_json.push_str(args);
                                                yield LlmEvent::ToolCallDelta {
                                                    id: buf.id.clone(),
                                                    name: buf.name.clone(),
                                                    args_delta: args.clone(),
                                                };
                                            }
                                        }
                                    }
                                }
                            }

                            // Finish reason
                            if let Some(finish_reason) = choice.finish_reason {
                                // Complete any pending tool calls
                                for (_, buf) in tool_buffers.drain() {
                                    if let Some(tc) = buf.try_complete() {
                                        yield LlmEvent::ToolCallComplete {
                                            id: tc.id,
                                            name: tc.name,
                                            args: tc.args,
                                        };
                                    }
                                }

                                let reason = match finish_reason.as_str() {
                                    "stop" => StopReason::EndTurn,
                                    "tool_calls" => StopReason::ToolUse,
                                    "length" => StopReason::MaxTokens,
                                    "content_filter" => StopReason::ContentFilter,
                                    _ => StopReason::EndTurn,
                                };
                                yield LlmEvent::Done { stop_reason: reason };
                            }
                        }
                    }
                }
            }
        })
    }

    fn provider(&self) -> &'static str {
        "openai"
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        let response = self
            .http
            .get(format!("{}/v1/models", self.base_url))
            .header("Authorization", format!("Bearer {}", self.api_key))
            .send()
            .await
            .map_err(|_| LlmError::NetworkTimeout { duration_ms: 5000 })?;

        if response.status().is_success() {
            Ok(())
        } else {
            Err(LlmError::from_http_status(
                response.status().as_u16(),
                "Health check failed".to_string(),
            ))
        }
    }
}

// SSE event structures for parsing OpenAI responses
#[derive(Debug, Deserialize)]
struct ChatCompletionChunk {
    choices: Vec<ChunkChoice>,
    usage: Option<UsageInfo>,
}

#[derive(Debug, Deserialize)]
struct ChunkChoice {
    delta: ChunkDelta,
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ChunkDelta {
    content: Option<String>,
    tool_calls: Option<Vec<ToolCallChunk>>,
}

#[derive(Debug, Deserialize)]
struct ToolCallChunk {
    index: Option<usize>,
    id: Option<String>,
    function: Option<FunctionChunk>,
}

#[derive(Debug, Deserialize)]
struct FunctionChunk {
    name: Option<String>,
    arguments: Option<String>,
}

#[derive(Debug, Deserialize)]
struct UsageInfo {
    prompt_tokens: Option<u64>,
    completion_tokens: Option<u64>,
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use raik_core::{ToolDef, UserMessage};

    fn skip_if_no_key() -> Option<OpenAiClient> {
        OpenAiClient::from_env().ok()
    }

    #[tokio::test]
    async fn test_streaming_text_delta_normalization() {
        let Some(client) = skip_if_no_key() else {
            eprintln!("Skipping: OPENAI_API_KEY not set");
            return;
        };

        let request = LlmRequest::new(
            "gpt-4o-mini",
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
                Ok(LlmEvent::TextDelta { delta: _ }) => {
                    got_text_delta = true;
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
            eprintln!("Skipping: OPENAI_API_KEY not set");
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
            "gpt-4o-mini",
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
            eprintln!("Skipping: OPENAI_API_KEY not set");
            return;
        };

        let request = LlmRequest::new(
            "gpt-4o-mini",
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
    async fn test_usage_mapping() {
        let Some(client) = skip_if_no_key() else {
            eprintln!("Skipping: OPENAI_API_KEY not set");
            return;
        };

        let request = LlmRequest::new(
            "gpt-4o-mini",
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
                assert!(usage.input_tokens > 0 || usage.output_tokens > 0);
            }
        }

        assert!(got_usage, "Should have received usage info");
    }

    #[tokio::test]
    async fn test_error_response_mapping() {
        let client = OpenAiClient::new("invalid-key".to_string());

        let request = LlmRequest::new(
            "gpt-4o-mini",
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
            matches!(err, LlmError::AuthenticationFailed { .. } | LlmError::InvalidApiKey),
            "Expected auth error, got: {:?}",
            err
        );
    }
}
