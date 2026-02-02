//! OpenAI API client
//!
//! Implements the LlmClient trait for OpenAI's API.

use crate::error::LlmError;
use crate::types::{LlmClient, LlmDoneOutcome, LlmEvent, LlmRequest, ToolCallBuffer};
use async_trait::async_trait;
use futures::{Stream, StreamExt};
use meerkat_core::{Message, StopReason, Usage};
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
use std::pin::Pin;

/// Client for OpenAI API
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

    /// Set custom base URL
    pub fn with_base_url(mut self, url: String) -> Self {
        self.base_url = url;
        self
    }

    /// Create from environment variable OPENAI_API_KEY
    pub fn from_env() -> Result<Self, LlmError> {
        let api_key = std::env::var("RKAT_OPENAI_API_KEY")
            .or_else(|_| std::env::var("OPENAI_API_KEY"))
            .map_err(|_| LlmError::InvalidApiKey)?;
        Ok(Self::new(api_key))
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

        // Extract OpenAI-specific parameters from provider_params
        if let Some(params) = &request.provider_params {
            if let Some(reasoning_effort) = params.get("reasoning_effort") {
                body["reasoning_effort"] = reasoning_effort.clone();
            }

            if let Some(seed) = params.get("seed") {
                body["seed"] = seed.clone();
            }

            if let Some(frequency_penalty) = params.get("frequency_penalty") {
                body["frequency_penalty"] = frequency_penalty.clone();
            }

            if let Some(presence_penalty) = params.get("presence_penalty") {
                body["presence_penalty"] = presence_penalty.clone();
            }

            // Handle structured output configuration
            // Format: {"structured_output": {"schema": {...}, "name": "output", "strict": true}}
            if let Some(structured) = params.get("structured_output") {
                if let Some(schema) = structured.get("schema") {
                    let name = structured
                        .get("name")
                        .and_then(|n| n.as_str())
                        .unwrap_or("output");
                    let strict = structured
                        .get("strict")
                        .and_then(|s| s.as_bool())
                        .unwrap_or(false);

                    body["response_format"] = serde_json::json!({
                        "type": "json_schema",
                        "json_schema": {
                            "name": name,
                            "schema": schema,
                            "strict": strict
                        }
                    });
                }
            }
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
        let inner: Pin<Box<dyn Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> = Box::pin(
            async_stream::try_stream! {
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

                let status_code = response.status().as_u16();
                let stream_result = if (200..=299).contains(&status_code) {
                    Ok(response.bytes_stream())
                } else {
                    let text = response.text().await.unwrap_or_default();
                    Err(LlmError::from_http_status(status_code, text))
                };
                let mut stream = stream_result?;
                let mut buffer = String::with_capacity(512);
                let mut tool_buffers: HashMap<usize, ToolCallBuffer> = HashMap::new();

                while let Some(chunk) = stream.next().await {
                    let chunk = chunk.map_err(|_| LlmError::ConnectionReset)?;
                    buffer.push_str(&String::from_utf8_lossy(&chunk));

                    while let Some(newline_pos) = buffer.find('\n') {
                        let line = buffer[..newline_pos].trim();
                        let should_process = !line.is_empty() && !line.starts_with(':');
                        let parsed_chunk = if should_process {
                            Self::parse_sse_line(line)
                        } else {
                            None
                        };

                        buffer.drain(..=newline_pos);

                        if let Some(chunk) = parsed_chunk {
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

                            for choice in chunk.choices {
                                let delta = choice.delta;

                                if let Some(content) = delta.content {
                                    if !content.is_empty() {
                                        yield LlmEvent::TextDelta { delta: content };
                                    }
                                }

                                if let Some(tool_calls) = delta.tool_calls {
                                    for tc in tool_calls {
                                        let index = tc.index.unwrap_or(0);

                                        if let std::collections::hash_map::Entry::Vacant(e) = tool_buffers.entry(index) {
                                            let id = tc.id.clone().unwrap_or_default();
                                            let mut buf = ToolCallBuffer::new(id);
                                            if let Some(func) = &tc.function {
                                                buf.name = func.name.clone();
                                            }
                                            e.insert(buf);
                                        }

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

                                if let Some(finish_reason) = choice.finish_reason {
                                    for (_, buf) in tool_buffers.drain() {
                                        if let Some(tc) = buf.try_complete() {
                                            yield LlmEvent::ToolCallComplete {
                                                id: tc.id,
                                                name: tc.name,
                                                args: tc.args,
                                                thought_signature: None,
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
                                    yield LlmEvent::Done {
                                        outcome: LlmDoneOutcome::Success { stop_reason: reason },
                                    };
                                }
                            }
                        }
                    }
                }
            },
        );

        crate::streaming::ensure_terminal_done(inner)
    }

    fn provider(&self) -> &'static str {
        "openai"
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
struct ChatCompletionChunk {
    choices: Vec<ChatCompletionChoice>,
    usage: Option<UsageMetadata>,
}

#[derive(Debug, Deserialize)]
struct ChatCompletionChoice {
    delta: ChatCompletionDelta,
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ChatCompletionDelta {
    content: Option<String>,
    tool_calls: Option<Vec<ChatCompletionToolCall>>,
}

#[derive(Debug, Deserialize)]
struct ChatCompletionToolCall {
    index: Option<usize>,
    id: Option<String>,
    function: Option<ChatCompletionFunction>,
}

#[derive(Debug, Deserialize)]
struct ChatCompletionFunction {
    name: Option<String>,
    arguments: Option<String>,
}

#[derive(Debug, Deserialize)]
struct UsageMetadata {
    prompt_tokens: Option<u64>,
    completion_tokens: Option<u64>,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use meerkat_core::UserMessage;

    #[test]
    fn test_request_includes_reasoning_effort_from_provider_params() {
        let client = OpenAiClient::new("test-key".to_string());
        let request = LlmRequest::new(
            "o1-preview",
            vec![Message::User(UserMessage {
                content: "test".to_string(),
            })],
        )
        .with_provider_param("reasoning_effort", "medium");

        let body = client.build_request_body(&request);

        assert_eq!(body["reasoning_effort"], "medium");
    }

    #[test]
    fn test_request_includes_seed_from_provider_params() {
        let client = OpenAiClient::new("test-key".to_string());
        let request = LlmRequest::new(
            "gpt-4o",
            vec![Message::User(UserMessage {
                content: "test".to_string(),
            })],
        )
        .with_provider_param("seed", 12345);

        let body = client.build_request_body(&request);

        assert_eq!(body["seed"], 12345);
    }

    #[test]
    fn test_request_includes_frequency_penalty_from_provider_params() {
        let client = OpenAiClient::new("test-key".to_string());
        let request = LlmRequest::new(
            "gpt-4o",
            vec![Message::User(UserMessage {
                content: "test".to_string(),
            })],
        )
        .with_provider_param("frequency_penalty", 0.5);

        let body = client.build_request_body(&request);

        assert_eq!(body["frequency_penalty"], 0.5);
    }

    #[test]
    fn test_request_includes_presence_penalty_from_provider_params() {
        let client = OpenAiClient::new("test-key".to_string());
        let request = LlmRequest::new(
            "gpt-4o",
            vec![Message::User(UserMessage {
                content: "test".to_string(),
            })],
        )
        .with_provider_param("presence_penalty", 0.8);

        let body = client.build_request_body(&request);

        assert_eq!(body["presence_penalty"], 0.8);
    }

    #[test]
    fn test_unknown_provider_params_are_ignored() {
        let client = OpenAiClient::new("test-key".to_string());
        let request = LlmRequest::new(
            "gpt-4o",
            vec![Message::User(UserMessage {
                content: "test".to_string(),
            })],
        )
        .with_provider_param("unknown_param", "some_value")
        .with_provider_param("another_unknown", 123)
        .with_provider_param("seed", 42);

        let body = client.build_request_body(&request);

        assert!(body.get("unknown_param").is_none());
        assert!(body.get("another_unknown").is_none());
        assert_eq!(body["seed"], 42);
    }

    #[test]
    fn test_multiple_provider_params_combined() {
        let client = OpenAiClient::new("test-key".to_string());
        let request = LlmRequest::new(
            "o1-preview",
            vec![Message::User(UserMessage {
                content: "test".to_string(),
            })],
        )
        .with_provider_param("reasoning_effort", "medium")
        .with_provider_param("seed", 999)
        .with_provider_param("frequency_penalty", 0.3)
        .with_provider_param("presence_penalty", 0.4);

        let body = client.build_request_body(&request);

        assert_eq!(body["reasoning_effort"], "medium");
        assert_eq!(body["seed"], 999);
        assert_eq!(body["frequency_penalty"], 0.3);
        assert_eq!(body["presence_penalty"], 0.4);
    }

    #[test]
    fn test_tool_args_serialization_no_double_encoding() -> Result<(), Box<dyn std::error::Error>> {
        let client = OpenAiClient::new("test-key".to_string());

        let tool_args = serde_json::json!({"city": "Tokyo", "units": "celsius"});
        let request = LlmRequest::new(
            "gpt-4o-mini",
            vec![
                Message::User(UserMessage {
                    content: "What's the weather?".to_string(),
                }),
                Message::Assistant(meerkat_core::AssistantMessage {
                    content: String::new(),
                    tool_calls: vec![meerkat_core::ToolCall::new(
                        "call_123".to_string(),
                        "get_weather".to_string(),
                        tool_args,
                    )],
                    stop_reason: StopReason::ToolUse,
                    usage: Usage::default(),
                }),
            ],
        );

        let body = client.build_request_body(&request);

        let messages = body["messages"].as_array().ok_or("not array")?;
        let assistant_msg = messages
            .iter()
            .find(|m| m["role"] == "assistant")
            .ok_or("no assistant message")?;

        let tool_calls = assistant_msg["tool_calls"]
            .as_array()
            .ok_or("no tool calls")?;
        let arguments = tool_calls[0]["function"]["arguments"]
            .as_str()
            .ok_or("not string")?;

        let parsed: serde_json::Value = serde_json::from_str(arguments)?;

        assert_eq!(parsed["city"], "Tokyo");
        assert_eq!(parsed["units"], "celsius");

        assert!(
            !arguments.starts_with(r#"{\"#),
            "arguments should not be double-encoded: {}",
            arguments
        );
        Ok(())
    }

    #[test]
    fn test_parse_sse_line_valid_data() -> Result<(), Box<dyn std::error::Error>> {
        let line = r###"data: {"id":"123","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"content":"Hi"},"finish_reason":null}]}"###;
        let chunk = OpenAiClient::parse_sse_line(line);
        assert!(chunk.is_some());
        let chunk = chunk.ok_or("missing chunk")?;
        assert_eq!(chunk.choices.len(), 1);
        assert_eq!(chunk.choices[0].delta.content, Some("Hi".to_string()));
        Ok(())
    }

    #[test]
    fn test_parse_sse_line_done_marker() {
        let line = "data: [DONE]";
        let chunk = OpenAiClient::parse_sse_line(line);
        assert!(chunk.is_none());
    }

    #[test]
    fn test_parse_sse_line_non_data_line() {
        let line = "event: message";
        let chunk = OpenAiClient::parse_sse_line(line);
        assert!(chunk.is_none());
    }

    #[test]
    fn test_build_request_body_with_structured_output() {
        let client = OpenAiClient::new("test-key".to_string());

        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "name": {"type": "string"},
                "age": {"type": "integer"}
            },
            "required": ["name", "age"]
        });

        let request = LlmRequest::new(
            "gpt-5.2",
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

        let body = client.build_request_body(&request);

        // Check response_format is present with correct structure
        assert!(
            body.get("response_format").is_some(),
            "response_format should be present"
        );
        let rf = &body["response_format"];
        assert_eq!(rf["type"], "json_schema");
        assert_eq!(rf["json_schema"]["name"], "person");
        assert_eq!(rf["json_schema"]["strict"], true);
        assert!(rf["json_schema"]["schema"].is_object());
    }

    #[test]
    fn test_build_request_body_with_structured_output_defaults() {
        let client = OpenAiClient::new("test-key".to_string());

        let schema = serde_json::json!({"type": "object"});

        // No name or strict specified - should use defaults
        let request = LlmRequest::new(
            "gpt-5.2",
            vec![Message::User(UserMessage {
                content: "test".to_string(),
            })],
        )
        .with_provider_param(
            "structured_output",
            serde_json::json!({
                "schema": schema
            }),
        );

        let body = client.build_request_body(&request);

        let rf = &body["response_format"];
        assert_eq!(rf["json_schema"]["name"], "output"); // default name
        assert_eq!(rf["json_schema"]["strict"], false); // default strict
    }

    #[test]
    fn test_build_request_body_without_structured_output() {
        let client = OpenAiClient::new("test-key".to_string());

        let request = LlmRequest::new(
            "gpt-5.2",
            vec![Message::User(UserMessage {
                content: "test".to_string(),
            })],
        );

        let body = client.build_request_body(&request);

        // text field should not be present
        assert!(
            body.get("response_format").is_none(),
            "response_format should not be present without structured_output"
        );
    }
}
