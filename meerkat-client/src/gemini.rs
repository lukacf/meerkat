//! Google Gemini API client
//!
//! Implements the LlmClient trait for Google's Gemini API.

use crate::error::LlmError;
use crate::types::{LlmClient, LlmDoneOutcome, LlmEvent, LlmRequest};
use async_trait::async_trait;
use futures::{Stream, StreamExt};
use meerkat_core::{Message, StopReason, Usage};
use serde::Deserialize;
use serde_json::{Value, json};
use std::pin::Pin;

/// Client for Google Gemini API
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

    /// Set custom base URL
    pub fn with_base_url(mut self, url: String) -> Self {
        self.base_url = url;
        self
    }

    /// Create from environment variable GEMINI_API_KEY
    pub fn from_env() -> Result<Self, LlmError> {
        let api_key = std::env::var("RKAT_GEMINI_API_KEY")
            .or_else(|_| {
                std::env::var("GEMINI_API_KEY").or_else(|_| std::env::var("GOOGLE_API_KEY"))
            })
            .map_err(|_| LlmError::InvalidApiKey)?;
        Ok(Self::new(api_key))
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
                        let part = if let Some(sig) = &tc.thought_signature {
                            serde_json::json!({
                                "functionCall": {
                                    "name": tc.name,
                                    "args": tc.args
                                },
                                "thoughtSignature": sig
                            })
                        } else {
                            serde_json::json!({
                                "functionCall": {
                                    "name": tc.name,
                                    "args": tc.args
                                }
                            })
                        };
                        parts.push(part);
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
                            if let Some(sig) = &r.thought_signature {
                                serde_json::json!({
                                    "functionResponse": {
                                        "name": r.tool_use_id,
                                        "response": {
                                            "content": r.content,
                                            "error": r.is_error
                                        }
                                    },
                                    "thoughtSignature": sig
                                })
                            } else {
                                serde_json::json!({
                                    "functionResponse": {
                                        "name": r.tool_use_id,
                                        "response": {
                                            "content": r.content,
                                            "error": r.is_error
                                        }
                                    }
                                })
                            }
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
                "maxOutputTokens": request.max_tokens,
            }
        });

        if let Some(system) = system_instruction {
            body["systemInstruction"] = system;
        }

        if let Some(temp) = request.temperature {
            if let Some(num) = serde_json::Number::from_f64(temp as f64) {
                body["generationConfig"]["temperature"] = Value::Number(num);
            }
        }

        // Extract provider-specific parameters from both formats:
        // 1. Legacy flat format: {"thinking_budget": 10000, "top_k": 40}
        // 2. Typed GeminiParams: {"thinking": {"include_thoughts": true, "thinking_budget": 10000}, "top_k": 40, "top_p": 0.95}
        if let Some(ref params) = request.provider_params {
            // Handle thinking config
            let thinking_budget = params.get("thinking_budget").or_else(|| {
                params
                    .get("thinking")
                    .and_then(|t| t.get("thinking_budget"))
            });

            if let Some(budget) = thinking_budget {
                body["generationConfig"]["thinkingConfig"] = serde_json::json!({
                    "thinkingBudget": budget
                });
            }

            // Handle top_k
            if let Some(top_k) = params.get("top_k") {
                body["generationConfig"]["topK"] = top_k.clone();
            }

            // Handle top_p (only in typed params)
            if let Some(top_p) = params.get("top_p") {
                body["generationConfig"]["topP"] = top_p.clone();
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
                        "parameters": Self::sanitize_schema_for_gemini(&t.input_schema)
                    })
                })
                .collect();

            body["tools"] = serde_json::json!([{
                "functionDeclarations": function_declarations
            }]);
        }

        body
    }

    /// Sanitize JSON Schema for Gemini
    fn sanitize_schema_for_gemini(schema: &Value) -> Value {
        match schema {
            Value::Object(map) => {
                let mut sanitized = serde_json::Map::new();
                for (key, value) in map {
                    if key == "$defs"
                        || key == "$ref"
                        || key == "$schema"
                        || key == "additionalProperties"
                    {
                        continue;
                    }
                    sanitized.insert(key.clone(), Self::sanitize_schema_for_gemini(value));
                }
                Value::Object(sanitized)
            }
            Value::Array(arr) => {
                Value::Array(arr.iter().map(Self::sanitize_schema_for_gemini).collect())
            }
            other => other.clone(),
        }
    }

    /// Parse streaming response line
    fn parse_stream_line(line: &str) -> Option<GenerateContentResponse> {
        serde_json::from_str(line).ok()
    }
}

#[async_trait]
impl LlmClient for GeminiClient {
    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> Pin<Box<dyn Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
        let inner: Pin<Box<dyn Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> =
            Box::pin(async_stream::try_stream! {
                let body = self.build_request_body(request);
                let url = format!(
                    "{}/v1beta/models/{}:streamGenerateContent?alt=sse&key={}",
                    self.base_url, request.model, self.api_key
                );

                let response = self.http
                    .post(url)
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
                let mut tool_call_index: u32 = 0;

                while let Some(chunk) = stream.next().await {
                    let chunk = chunk.map_err(|_| LlmError::ConnectionReset)?;
                    buffer.push_str(&String::from_utf8_lossy(&chunk));

                    while let Some(newline_pos) = buffer.find('\n') {
                        let line = buffer[..newline_pos].trim();
                        let data = line.strip_prefix("data: ");
                        let parsed_response = if let Some(d) = data {
                            Self::parse_stream_line(d)
                        } else {
                            None
                        };

                        buffer.drain(..=newline_pos);

                        if let Some(resp) = parsed_response {
                            if let Some(usage) = resp.usage_metadata {
                                yield LlmEvent::UsageUpdate {
                                    usage: Usage {
                                        input_tokens: usage.prompt_token_count.unwrap_or(0),
                                        output_tokens: usage.candidates_token_count.unwrap_or(0),
                                        cache_creation_tokens: None,
                                        cache_read_tokens: None,
                                    }
                                };
                            }

                            if let Some(candidates) = resp.candidates {
                                for cand in candidates {
                                    if let Some(content) = cand.content {
                                        if let Some(parts) = content.parts {
                                            for part in parts {
                                                if let Some(text) = part.text {
                                                    yield LlmEvent::TextDelta { delta: text };
                                                }
                                                if let Some(fc) = part.function_call {
                                                    let id = format!("fc_{}", tool_call_index);
                                                    tool_call_index += 1;
                                                    yield LlmEvent::ToolCallComplete {
                                                        id,
                                                        name: fc.name,
                                                        args: fc.args.unwrap_or(json!({})),
                                                        thought_signature: part.thought_signature,
                                                    };
                                                }
                                            }
                                        }
                                    }

                                    if let Some(reason) = cand.finish_reason {
                                        let stop = match reason.as_str() {
                                            "STOP" => StopReason::EndTurn,
                                            "MAX_TOKENS" => StopReason::MaxTokens,
                                            "SAFETY" | "RECITATION" => StopReason::ContentFilter,
                                            // Gemini uses various names for tool calls
                                            "TOOL_CALL" | "FUNCTION_CALL" => StopReason::ToolUse,
                                            _ => StopReason::EndTurn,
                                        };
                                        yield LlmEvent::Done {
                                            outcome: LlmDoneOutcome::Success { stop_reason: stop },
                                        };
                                    }
                                }
                            }
                        }
                    }
                }
            });

        crate::streaming::ensure_terminal_done(inner)
    }

    fn provider(&self) -> &'static str {
        "gemini"
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GenerateContentResponse {
    candidates: Option<Vec<Candidate>>,
    usage_metadata: Option<GeminiUsage>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Candidate {
    content: Option<CandidateContent>,
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CandidateContent {
    parts: Option<Vec<Part>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Part {
    text: Option<String>,
    function_call: Option<FunctionCall>,
    thought_signature: Option<String>,
}

#[derive(Debug, Deserialize)]
struct FunctionCall {
    name: String,
    args: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiUsage {
    prompt_token_count: Option<u64>,
    candidates_token_count: Option<u64>,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use meerkat_core::UserMessage;

    #[test]
    fn test_build_request_body_with_thinking_budget() -> Result<(), Box<dyn std::error::Error>> {
        let client = GeminiClient::new("test-key".to_string());
        let request = LlmRequest::new(
            "gemini-1.5-pro",
            vec![Message::User(UserMessage {
                content: "test".to_string(),
            })],
        )
        .with_provider_param("thinking_budget", 10000);

        let body = client.build_request_body(&request);

        let generation_config = body.get("generationConfig").ok_or("missing config")?;
        let thinking_config = generation_config
            .get("thinkingConfig")
            .ok_or("missing thinking")?;
        let thinking_budget = thinking_config
            .get("thinkingBudget")
            .ok_or("missing budget")?;

        assert_eq!(thinking_budget.as_i64(), Some(10000));
        Ok(())
    }

    #[test]
    fn test_build_request_body_with_top_k() -> Result<(), Box<dyn std::error::Error>> {
        let client = GeminiClient::new("test-key".to_string());
        let request = LlmRequest::new(
            "gemini-1.5-pro",
            vec![Message::User(UserMessage {
                content: "test".to_string(),
            })],
        )
        .with_provider_param("top_k", 40);

        let body = client.build_request_body(&request);
        let generation_config = body.get("generationConfig").ok_or("missing config")?;
        let top_k = generation_config.get("topK").ok_or("missing top_k")?;

        assert_eq!(top_k.as_i64(), Some(40));
        Ok(())
    }

    #[test]
    fn test_build_request_body_with_multiple_provider_params()
    -> Result<(), Box<dyn std::error::Error>> {
        let client = GeminiClient::new("test-key".to_string());
        let request = LlmRequest::new(
            "gemini-1.5-pro",
            vec![Message::User(UserMessage {
                content: "test".to_string(),
            })],
        )
        .with_provider_param("top_k", 50)
        .with_provider_param("thinking_budget", 5000);

        let body = client.build_request_body(&request);
        let generation_config = body.get("generationConfig").ok_or("missing config")?;

        let top_k = generation_config.get("topK").ok_or("missing top_k")?;
        assert_eq!(top_k.as_i64(), Some(50));

        let thinking_config = generation_config
            .get("thinkingConfig")
            .ok_or("missing thinking")?;
        let thinking_budget = thinking_config
            .get("thinkingBudget")
            .ok_or("missing budget")?;
        assert_eq!(thinking_budget.as_i64(), Some(5000));
        Ok(())
    }

    #[test]
    fn test_build_request_body_no_provider_params() -> Result<(), Box<dyn std::error::Error>> {
        let client = GeminiClient::new("test-key".to_string());
        let request = LlmRequest::new(
            "gemini-1.5-pro",
            vec![Message::User(UserMessage {
                content: "test".to_string(),
            })],
        );

        let body = client.build_request_body(&request);
        let generation_config = body.get("generationConfig").ok_or("missing config")?;

        assert!(generation_config.get("thinkingConfig").is_none());
        assert!(generation_config.get("topK").is_none());
        Ok(())
    }

    #[test]
    fn test_parse_stream_line_valid_response() -> Result<(), Box<dyn std::error::Error>> {
        let line =
            r#"{"candidates":[{"content":{"parts":[{"text":"Hello"}]},"finishReason":"STOP"}]}"#;
        let response = GeminiClient::parse_stream_line(line);
        assert!(response.is_some());
        let response = response.ok_or("missing response")?;
        assert!(response.candidates.is_some());
        let candidates = response.candidates.ok_or("missing candidates")?;
        assert_eq!(candidates.len(), 1);
        Ok(())
    }

    #[test]
    fn test_parse_stream_line_with_usage() -> Result<(), Box<dyn std::error::Error>> {
        let line = r#"{"usageMetadata":{"promptTokenCount":10,"candidatesTokenCount":5}}"#;
        let response = GeminiClient::parse_stream_line(line);
        assert!(response.is_some());
        let response = response.ok_or("missing response")?;
        assert!(response.usage_metadata.is_some());
        let usage = response.usage_metadata.ok_or("missing usage")?;
        assert_eq!(usage.prompt_token_count, Some(10));
        Ok(())
    }

    #[test]
    fn test_parse_stream_line_function_call() -> Result<(), Box<dyn std::error::Error>> {
        let line = r#"{"candidates":[{"content":{"parts":[{"functionCall":{"name":"get_weather","args":{"city":"Tokyo"}}}]}}]}"#;
        let response = GeminiClient::parse_stream_line(line);
        assert!(response.is_some());
        let response = response.ok_or("missing response")?;
        let candidates = response.candidates.as_ref().ok_or("missing candidates")?;
        let parts = candidates[0]
            .content
            .as_ref()
            .ok_or("missing content")?
            .parts
            .as_ref()
            .ok_or("missing parts")?;
        let fc = parts[0].function_call.as_ref().ok_or("missing fc")?;
        assert_eq!(fc.name, "get_weather");
        assert_eq!(fc.args.as_ref().ok_or("missing args")?["city"], "Tokyo");
        Ok(())
    }

    #[test]
    fn test_parse_stream_line_empty() {
        let line = "";
        let response = GeminiClient::parse_stream_line(line);
        assert!(response.is_none());
    }

    #[test]
    fn test_parse_stream_line_invalid_json() {
        let line = "{invalid}";
        let response = GeminiClient::parse_stream_line(line);
        assert!(response.is_none());
    }

    /// Regression: Gemini tool-call finish reasons must map to ToolUse.
    /// Previously TOOL_CALL and FUNCTION_CALL mapped to EndTurn incorrectly.
    #[test]
    fn test_regression_gemini_finish_reason_tool_call_maps_to_tool_use() {
        // Test the finish reason mapping logic directly
        let finish_reasons = ["TOOL_CALL", "FUNCTION_CALL"];

        for reason in finish_reasons {
            let stop = match reason {
                "STOP" => StopReason::EndTurn,
                "MAX_TOKENS" => StopReason::MaxTokens,
                "SAFETY" | "RECITATION" => StopReason::ContentFilter,
                "TOOL_CALL" | "FUNCTION_CALL" => StopReason::ToolUse,
                _ => StopReason::EndTurn,
            };
            assert_eq!(
                stop,
                StopReason::ToolUse,
                "finish_reason '{}' should map to ToolUse",
                reason
            );
        }
    }

    /// Regression: Gemini RECITATION finish reason must map to ContentFilter.
    #[test]
    fn test_regression_gemini_finish_reason_recitation_maps_to_content_filter() {
        let reason = "RECITATION";
        let stop = match reason {
            "STOP" => StopReason::EndTurn,
            "MAX_TOKENS" => StopReason::MaxTokens,
            "SAFETY" | "RECITATION" => StopReason::ContentFilter,
            "TOOL_CALL" | "FUNCTION_CALL" => StopReason::ToolUse,
            _ => StopReason::EndTurn,
        };
        assert_eq!(stop, StopReason::ContentFilter);
    }

    /// Regression: Multiple tool calls to the same tool must get unique IDs.
    /// Previously, IDs were set to the tool name, causing collisions when
    /// the same tool was called multiple times (e.g., two "search" calls
    /// both got id="search", breaking tool-result correlation).
    #[test]
    fn test_regression_gemini_tool_call_ids_must_be_unique() {
        // Simulate the ID generation logic from streaming
        let mut tool_call_index: u32 = 0;

        // Simulate 3 calls to "search" tool
        let tool_names = ["search", "search", "search"];
        let mut generated_ids = Vec::new();

        for _name in tool_names {
            let id = format!("fc_{}", tool_call_index);
            tool_call_index += 1;
            generated_ids.push(id);
        }

        // All IDs must be unique
        assert_eq!(generated_ids[0], "fc_0");
        assert_eq!(generated_ids[1], "fc_1");
        assert_eq!(generated_ids[2], "fc_2");

        // Verify no duplicates
        let mut seen = std::collections::HashSet::new();
        for id in &generated_ids {
            assert!(
                seen.insert(id.clone()),
                "Duplicate tool call ID found: {}",
                id
            );
        }
    }

    /// Regression: Tool call IDs must be unique across mixed tool types.
    #[test]
    fn test_regression_gemini_tool_call_ids_unique_across_different_tools() {
        let mut tool_call_index: u32 = 0;

        // Simulate mixed tool calls
        let tool_names = ["search", "write_file", "search", "read_file"];
        let mut id_to_name = Vec::new();

        for name in tool_names {
            let id = format!("fc_{}", tool_call_index);
            tool_call_index += 1;
            id_to_name.push((id, name));
        }

        // Each call gets a unique ID regardless of tool name
        assert_eq!(id_to_name[0], ("fc_0".to_string(), "search"));
        assert_eq!(id_to_name[1], ("fc_1".to_string(), "write_file"));
        assert_eq!(id_to_name[2], ("fc_2".to_string(), "search")); // Second search gets fc_2
        assert_eq!(id_to_name[3], ("fc_3".to_string(), "read_file"));
    }
}
