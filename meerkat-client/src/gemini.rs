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
            body["generationConfig"]["temperature"] =
                Value::Number(serde_json::Number::from_f64(temp as f64).unwrap());
        }

        // Extract provider-specific parameters
        if let Some(ref params) = request.provider_params {
            // thinking_budget -> generationConfig.thinkingConfig.thinkingBudget
            if let Some(thinking_budget) = params.get("thinking_budget") {
                body["generationConfig"]["thinkingConfig"] = serde_json::json!({
                    "thinkingBudget": thinking_budget
                });
            }

            // top_k -> generationConfig.topK
            if let Some(top_k) = params.get("top_k") {
                body["generationConfig"]["topK"] = top_k.clone();
            }
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
            // Pre-allocate buffer with typical SSE event size to reduce allocations
            let mut buffer = String::with_capacity(512);
            let mut tool_index = 0usize;

            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|_| LlmError::ConnectionReset)?;
                // from_utf8_lossy returns Cow which avoids allocation for valid UTF-8
                buffer.push_str(&String::from_utf8_lossy(&chunk));

                // Process complete lines without allocating new strings
                while let Some(newline_pos) = buffer.find('\n') {
                    // Get the line as a slice and trim, avoiding allocation
                    let line = buffer[..newline_pos].trim();

                    // Skip empty lines
                    let parsed_response = if !line.is_empty() {
                        // Handle SSE format - strip prefix if present
                        let data = line.strip_prefix("data: ").unwrap_or(line);
                        Self::parse_stream_line(data)
                    } else {
                        None
                    };

                    // Drain the processed line from buffer (avoids creating new String)
                    buffer.drain(..=newline_pos);

                    if let Some(response) = parsed_response {
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
        let url = format!("{}/v1beta/models?key={}", self.base_url, self.api_key);

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
    use meerkat_core::UserMessage;

    // ==================== Unit tests for provider_params extraction ====================

    /// Test that thinking_budget is extracted and converted to Gemini's thinkingConfig format
    #[test]
    fn test_build_request_body_with_thinking_budget() {
        let client = GeminiClient::new("test-key".to_string());

        let request = LlmRequest::new(
            "gemini-2.5-flash",
            vec![Message::User(UserMessage {
                content: "Test".to_string(),
            })],
        )
        .with_max_tokens(1000)
        .with_provider_param("thinking_budget", 10000);

        let body = client.build_request_body(&request);

        // Verify thinkingConfig is set in generationConfig
        let generation_config = body
            .get("generationConfig")
            .expect("generationConfig should exist");
        let thinking_config = generation_config
            .get("thinkingConfig")
            .expect("thinkingConfig should exist");
        let thinking_budget = thinking_config
            .get("thinkingBudget")
            .expect("thinkingBudget should exist");

        assert_eq!(thinking_budget.as_i64(), Some(10000));
    }

    /// Test that top_k is extracted and added to generationConfig
    #[test]
    fn test_build_request_body_with_top_k() {
        let client = GeminiClient::new("test-key".to_string());

        let request = LlmRequest::new(
            "gemini-2.5-flash",
            vec![Message::User(UserMessage {
                content: "Test".to_string(),
            })],
        )
        .with_max_tokens(1000)
        .with_provider_param("top_k", 40);

        let body = client.build_request_body(&request);

        // Verify topK is set in generationConfig
        let generation_config = body
            .get("generationConfig")
            .expect("generationConfig should exist");
        let top_k = generation_config.get("topK").expect("topK should exist");

        assert_eq!(top_k.as_i64(), Some(40));
    }

    /// Test that both thinking_budget and top_k work together
    #[test]
    fn test_build_request_body_with_multiple_provider_params() {
        let client = GeminiClient::new("test-key".to_string());

        let request = LlmRequest::new(
            "gemini-2.5-flash",
            vec![Message::User(UserMessage {
                content: "Test".to_string(),
            })],
        )
        .with_max_tokens(1000)
        .with_provider_param("thinking_budget", 5000)
        .with_provider_param("top_k", 20);

        let body = client.build_request_body(&request);

        let generation_config = body
            .get("generationConfig")
            .expect("generationConfig should exist");

        // Verify topK
        let top_k = generation_config.get("topK").expect("topK should exist");
        assert_eq!(top_k.as_i64(), Some(20));

        // Verify thinkingConfig
        let thinking_config = generation_config
            .get("thinkingConfig")
            .expect("thinkingConfig should exist");
        let thinking_budget = thinking_config
            .get("thinkingBudget")
            .expect("thinkingBudget should exist");
        assert_eq!(thinking_budget.as_i64(), Some(5000));
    }

    /// Test that unknown provider params are silently ignored (no error, no inclusion)
    #[test]
    fn test_build_request_body_unknown_params_ignored() {
        let client = GeminiClient::new("test-key".to_string());

        let request = LlmRequest::new(
            "gemini-2.5-flash",
            vec![Message::User(UserMessage {
                content: "Test".to_string(),
            })],
        )
        .with_max_tokens(1000)
        .with_provider_param("unknown_param", "should_be_ignored")
        .with_provider_param("another_unknown", 12345);

        // Should not panic - unknown params are silently ignored
        let body = client.build_request_body(&request);

        let generation_config = body
            .get("generationConfig")
            .expect("generationConfig should exist");

        // Verify unknown params are NOT in generationConfig
        assert!(generation_config.get("unknown_param").is_none());
        assert!(generation_config.get("another_unknown").is_none());

        // Verify the body doesn't have unknown params at the top level either
        assert!(body.get("unknown_param").is_none());
        assert!(body.get("another_unknown").is_none());
    }

    /// Test that no provider_params results in standard request body
    #[test]
    fn test_build_request_body_no_provider_params() {
        let client = GeminiClient::new("test-key".to_string());

        let request = LlmRequest::new(
            "gemini-2.5-flash",
            vec![Message::User(UserMessage {
                content: "Test".to_string(),
            })],
        )
        .with_max_tokens(1000);

        let body = client.build_request_body(&request);

        let generation_config = body
            .get("generationConfig")
            .expect("generationConfig should exist");

        // Verify no thinkingConfig when not requested
        assert!(generation_config.get("thinkingConfig").is_none());
        // Verify no topK when not requested
        assert!(generation_config.get("topK").is_none());
    }

    // Unit tests for SSE buffer handling

    /// Test SSE parsing with valid response data
    #[test]
    fn test_parse_stream_line_valid_response() {
        let line =
            r#"{"candidates":[{"content":{"parts":[{"text":"Hello"}]},"finishReason":"STOP"}]}"#;
        let response = GeminiClient::parse_stream_line(line);
        assert!(response.is_some());
        let response = response.unwrap();
        assert!(response.candidates.is_some());
        let candidates = response.candidates.unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].finish_reason, Some("STOP".to_string()));
    }

    /// Test SSE parsing with usage metadata
    #[test]
    fn test_parse_stream_line_with_usage() {
        let line =
            r#"{"candidates":[],"usageMetadata":{"promptTokenCount":10,"candidatesTokenCount":5}}"#;
        let response = GeminiClient::parse_stream_line(line);
        assert!(response.is_some());
        let response = response.unwrap();
        assert!(response.usage_metadata.is_some());
        let usage = response.usage_metadata.unwrap();
        assert_eq!(usage.prompt_token_count, Some(10));
        assert_eq!(usage.candidates_token_count, Some(5));
    }

    /// Test SSE parsing with function call
    #[test]
    fn test_parse_stream_line_function_call() {
        let line = r#"{"candidates":[{"content":{"parts":[{"functionCall":{"name":"get_weather","args":{"city":"Tokyo"}}}]}}]}"#;
        let response = GeminiClient::parse_stream_line(line);
        assert!(response.is_some());
        let response = response.unwrap();
        let candidates = response.candidates.unwrap();
        let parts = candidates[0]
            .content
            .as_ref()
            .unwrap()
            .parts
            .as_ref()
            .unwrap();
        let fc = parts[0].function_call.as_ref().unwrap();
        assert_eq!(fc.name, "get_weather");
        assert_eq!(fc.args.as_ref().unwrap()["city"], "Tokyo");
    }

    /// Test SSE parsing with invalid JSON returns None
    #[test]
    fn test_parse_stream_line_invalid_json() {
        let line = "not valid json";
        let response = GeminiClient::parse_stream_line(line);
        assert!(response.is_none());
    }

    /// Test SSE parsing with empty line returns None
    #[test]
    fn test_parse_stream_line_empty() {
        let line = "";
        let response = GeminiClient::parse_stream_line(line);
        assert!(response.is_none());
    }
}
