//! Google Gemini API client
//!
//! Implements the LlmClient trait for Google's Gemini API.

use crate::error::LlmError;
use crate::types::{LlmClient, LlmEvent, LlmRequest};
use async_trait::async_trait;
use futures::{Stream, StreamExt};
use meerkat_core::{Message, StopReason, Usage};
use serde::Deserialize;
use serde_json::Value;
use std::pin::Pin;

/// Google Gemini API client
pub struct GeminiClient {
    api_key: String,
    base_url: String,
    http: reqwest::Client,
}

impl GeminiClient {
    /// Create a new Gemini client with the given API key
    pub fn new(api_key: String) -> Self {
        Self {
            api_key,
            base_url: "https://generativelanguage.googleapis.com".to_string(),
            http: reqwest::Client::new(),
        }
    }

    /// Create from environment variable GOOGLE_API_KEY
    pub fn from_env() -> Result<Self, LlmError> {
        let api_key = std::env::var("GOOGLE_API_KEY").map_err(|_| LlmError::InvalidApiKey)?;
        Ok(Self::new(api_key))
    }

    /// Set custom base URL
    pub fn with_base_url(mut self, url: String) -> Self {
        self.base_url = url;
        self
    }

    /// Build request body for Gemini API
    fn build_request_body(&self, request: &LlmRequest) -> Value {
        let mut contents = Vec::new();
        let mut system_instruction = None;

        for msg in &request.messages {
            match msg {
                Message::System(s) => {
                    system_instruction = Some(serde_json::json!({
                        "parts": [{"text": s.content}]
                    }));
                }
                Message::User(u) => {
                    contents.push(serde_json::json!({
                        "role": "user",
                        "parts": [{"text": u.content}]
                    }));
                }
                Message::Assistant(a) => {
                    let mut parts = Vec::new();

                    if !a.content.is_empty() {
                        parts.push(serde_json::json!({"text": a.content}));
                    }

                    for tc in &a.tool_calls {
                        parts.push(serde_json::json!({
                            "functionCall": {
                                "name": tc.name,
                                "args": tc.args
                            }
                        }));
                    }

                    contents.push(serde_json::json!({
                        "role": "model",
                        "parts": parts
                    }));
                }
                Message::ToolResults { results } => {
                    let parts: Vec<Value> = results
                        .iter()
                        .map(|r| {
                            serde_json::json!({
                                "functionResponse": {
                                    "name": r.tool_use_id,  // Gemini uses name, not id
                                    "response": {
                                        "content": r.content,
                                        "error": r.is_error
                                    }
                                }
                            })
                        })
                        .collect();

                    contents.push(serde_json::json!({
                        "role": "user",
                        "parts": parts
                    }));
                }
            }
        }

        let mut body = serde_json::json!({
            "contents": contents,
            "generationConfig": {
                "maxOutputTokens": request.max_tokens
            }
        });

        if let Some(system) = system_instruction {
            body["systemInstruction"] = system;
        }

        if let Some(temp) = request.temperature {
            body["generationConfig"]["temperature"] = Value::Number(
                serde_json::Number::from_f64(temp as f64).unwrap()
            );
        }

        if !request.tools.is_empty() {
            let function_declarations: Vec<Value> = request
                .tools
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.input_schema
                    })
                })
                .collect();

            body["tools"] = serde_json::json!([{
                "functionDeclarations": function_declarations
            }]);
        }

        body
    }

    /// Parse streaming response line
    fn parse_stream_line(line: &str) -> Option<GenerateContentResponse> {
        // Gemini streams JSON objects, one per line
        serde_json::from_str(line).ok()
    }
}

#[async_trait]
impl LlmClient for GeminiClient {
    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> Pin<Box<dyn Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
        Box::pin(async_stream::try_stream! {
            let body = self.build_request_body(request);

            let url = format!(
                "{}/v1beta/models/{}:streamGenerateContent?key={}&alt=sse",
                self.base_url,
                request.model,
                self.api_key
            );

            let response = self.http
                .post(&url)
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
            let mut tool_index = 0usize;

            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|_| LlmError::ConnectionReset)?;
                buffer.push_str(&String::from_utf8_lossy(&chunk));

                // Process complete lines
                while let Some(newline_pos) = buffer.find('\n') {
                    let line = buffer[..newline_pos].trim().to_string();
                    buffer = buffer[newline_pos + 1..].to_string();

                    if line.is_empty() {
                        continue;
                    }

                    // Handle SSE format
                    let data = line.strip_prefix("data: ").unwrap_or(&line);

                    if let Some(response) = Self::parse_stream_line(data) {
                        // Handle error
                        if let Some(error) = response.error {
                            Err(LlmError::InvalidRequest {
                                message: error.message.unwrap_or_default()
                            })?;
                        }

                        // Handle candidates
                        for candidate in response.candidates.unwrap_or_default() {
                            if let Some(content) = candidate.content {
                                for part in content.parts.unwrap_or_default() {
                                    // Text part
                                    if let Some(text) = part.text {
                                        yield LlmEvent::TextDelta { delta: text };
                                    }

                                    // Function call part
                                    if let Some(fc) = part.function_call {
                                        let id = format!("fc_{}", tool_index);
                                        tool_index += 1;

                                        // Gemini sends complete function calls
                                        yield LlmEvent::ToolCallComplete {
                                            id,
                                            name: fc.name,
                                            args: fc.args.unwrap_or(Value::Object(Default::default())),
                                        };
                                    }
                                }
                            }

                            // Finish reason
                            if let Some(finish_reason) = candidate.finish_reason {
                                let reason = match finish_reason.as_str() {
                                    "STOP" => StopReason::EndTurn,
                                    "MAX_TOKENS" => StopReason::MaxTokens,
                                    "SAFETY" => StopReason::ContentFilter,
                                    "RECITATION" => StopReason::ContentFilter,
                                    "TOOL_CALL" | "FUNCTION_CALL" => StopReason::ToolUse,
                                    _ => StopReason::EndTurn,
                                };
                                yield LlmEvent::Done { stop_reason: reason };
                            }
                        }

                        // Handle usage metadata
                        if let Some(metadata) = response.usage_metadata {
                            yield LlmEvent::UsageUpdate {
                                usage: Usage {
                                    input_tokens: metadata.prompt_token_count.unwrap_or(0),
                                    output_tokens: metadata.candidates_token_count.unwrap_or(0),
                                    cache_creation_tokens: None,
                                    cache_read_tokens: None,
                                }
                            };
                        }
                    }
                }
            }
        })
    }

    fn provider(&self) -> &'static str {
        "gemini"
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        let url = format!(
            "{}/v1beta/models?key={}",
            self.base_url,
            self.api_key
        );

        let response = self
            .http
            .get(&url)
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

// Response structures for parsing Gemini responses
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GenerateContentResponse {
    candidates: Option<Vec<Candidate>>,
    usage_metadata: Option<UsageMetadata>,
    error: Option<ErrorInfo>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Candidate {
    content: Option<Content>,
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Content {
    parts: Option<Vec<Part>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Part {
    text: Option<String>,
    function_call: Option<FunctionCall>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FunctionCall {
    name: String,
    args: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UsageMetadata {
    prompt_token_count: Option<u64>,
    candidates_token_count: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct ErrorInfo {
    message: Option<String>,
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use meerkat_core::{ToolDef, UserMessage};

    fn skip_if_no_key() -> Option<GeminiClient> {
        GeminiClient::from_env().ok()
    }

    #[tokio::test]
    async fn test_streaming_text_delta_normalization() {
        let Some(client) = skip_if_no_key() else {
            eprintln!("Skipping: GOOGLE_API_KEY not set");
            return;
        };

        let request = LlmRequest::new(
            "gemini-2.0-flash",
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
    async fn test_function_call_normalization() {
        let Some(client) = skip_if_no_key() else {
            eprintln!("Skipping: GOOGLE_API_KEY not set");
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
            "gemini-2.0-flash",
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
                Ok(LlmEvent::Done { stop_reason: _ }) => {
                    got_done = true;
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
            eprintln!("Skipping: GOOGLE_API_KEY not set");
            return;
        };

        let request = LlmRequest::new(
            "gemini-2.0-flash",
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
            eprintln!("Skipping: GOOGLE_API_KEY not set");
            return;
        };

        let request = LlmRequest::new(
            "gemini-2.0-flash",
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
        let client = GeminiClient::new("invalid-key".to_string());

        let request = LlmRequest::new(
            "gemini-2.0-flash",
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
        // Gemini returns 400 for invalid API key
        assert!(
            matches!(
                err,
                LlmError::AuthenticationFailed { .. }
                    | LlmError::InvalidApiKey
                    | LlmError::InvalidRequest { .. }
            ),
            "Expected auth/invalid error, got: {:?}",
            err
        );
    }
}
