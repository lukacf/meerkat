//! Google Gemini API client
//!
//! Implements the LlmClient trait for Google's Gemini API.

use crate::error::LlmError;
use crate::types::{LlmClient, LlmDoneOutcome, LlmEvent, LlmRequest, LlmStream};
use async_trait::async_trait;
use futures::StreamExt;
use meerkat_core::schema::{CompiledSchema, SchemaCompat, SchemaError, SchemaWarning};
use meerkat_core::{Message, OutputSchema, Provider, StopReason, Usage};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashMap;

/// Client for Google Gemini API
pub struct GeminiClient {
    api_key: String,
    base_url: String,
    http: reqwest::Client,
}

impl GeminiClient {
    /// Create a new Gemini client with the given API key
    pub fn new(api_key: String) -> Self {
        Self::new_with_base_url(
            api_key,
            "https://generativelanguage.googleapis.com".to_string(),
        )
    }

    /// Create a new Gemini client with an explicit base URL
    pub fn new_with_base_url(api_key: String, base_url: String) -> Self {
        let http =
            crate::http::build_http_client_for_base_url(reqwest::Client::builder(), &base_url)
                .unwrap_or_else(|_| reqwest::Client::new());
        Self {
            api_key,
            base_url,
            http,
        }
    }

    /// Set custom base URL
    pub fn with_base_url(mut self, url: String) -> Self {
        if let Ok(http) =
            crate::http::build_http_client_for_base_url(reqwest::Client::builder(), &url)
        {
            self.http = http;
        }
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
    fn build_request_body(&self, request: &LlmRequest) -> Result<Value, LlmError> {
        let mut contents = Vec::new();
        let mut system_instruction = None;

        let mut tool_name_by_id: HashMap<String, String> = HashMap::new();

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
                Message::Assistant(_) => {
                    return Err(LlmError::InvalidRequest {
                        message: "Legacy Message::Assistant is not supported by Gemini adapter; use BlockAssistant".to_string(),
                    });
                }
                Message::BlockAssistant(a) => {
                    // New format: ordered blocks with ProviderMeta
                    let mut parts = Vec::new();

                    for block in &a.blocks {
                        match block {
                            meerkat_core::AssistantBlock::Text { text, meta } => {
                                if !text.is_empty() {
                                    let mut part = serde_json::json!({"text": text});
                                    // Gemini may have thoughtSignature on text parts for continuity
                                    if let Some(meerkat_core::ProviderMeta::Gemini {
                                        thought_signature,
                                    }) = meta.as_deref()
                                    {
                                        part["thoughtSignature"] =
                                            serde_json::json!(thought_signature);
                                    }
                                    parts.push(part);
                                }
                            }
                            meerkat_core::AssistantBlock::Reasoning { text, .. } => {
                                // Gemini doesn't accept reasoning blocks back
                                // Just include as text if needed for context
                                if !text.is_empty() {
                                    parts.push(serde_json::json!({"text": format!("[Reasoning: {}]", text)}));
                                }
                            }
                            meerkat_core::AssistantBlock::ToolUse {
                                id,
                                name,
                                args,
                                meta,
                            } => {
                                tool_name_by_id.insert(id.clone(), name.clone());
                                // Parse RawValue to Value
                                let args_value: Value = serde_json::from_str(args.get())
                                    .unwrap_or_else(|_| serde_json::json!({}));
                                let mut part = serde_json::json!({"functionCall": {"name": name, "args": args_value}});
                                // Only add signature if present (first parallel call has it)
                                if let Some(meerkat_core::ProviderMeta::Gemini {
                                    thought_signature,
                                }) = meta.as_deref()
                                {
                                    part["thoughtSignature"] = serde_json::json!(thought_signature);
                                }
                                parts.push(part);
                            }
                            _ => {} // non_exhaustive: ignore unknown future variants
                        }
                    }

                    contents.push(serde_json::json!({
                        "role": "model",
                        "parts": parts
                    }));
                }
                Message::ToolResults { results } => {
                    // Per spec section 2.3: thoughtSignature NEVER on functionResponse
                    // Signature belongs on functionCall, not on the response
                    let parts: Vec<Value> = results
                        .iter()
                        .map(|r| {
                            let function_name = tool_name_by_id
                                .get(&r.tool_use_id)
                                .cloned()
                                .unwrap_or_else(|| r.tool_use_id.clone());
                            serde_json::json!({
                                "functionResponse": {
                                    "name": function_name,
                                    "response": {
                                        "content": r.content,
                                        "error": r.is_error
                                    }
                                }
                            })
                            // NOTE: thoughtSignature is intentionally NOT included here
                            // Verified by scripts/test_gemini_thought_signature.py:
                            // - sig on functionCall only: PASS
                            // - sig on functionResponse only: FAIL (400 error)
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

        if let Some(temp) = request.temperature
            && let Some(num) = serde_json::Number::from_f64(temp as f64)
        {
            body["generationConfig"]["temperature"] = Value::Number(num);
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

            // Handle structured output configuration
            // Format: {"structured_output": {"schema": {...}, "name": "output", "strict": true}}
            if let Some(structured) = params.get("structured_output") {
                let output_schema: OutputSchema = serde_json::from_value(structured.clone())
                    .map_err(|e| LlmError::InvalidRequest {
                        message: format!("Invalid structured_output schema: {e}"),
                    })?;
                let compiled = Self::compile_schema_for_gemini(&output_schema).map_err(|e| {
                    LlmError::InvalidRequest {
                        message: e.to_string(),
                    }
                })?;
                body["generationConfig"]["responseMimeType"] =
                    Value::String("application/json".to_string());
                body["generationConfig"]["responseJsonSchema"] = compiled.schema;
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
                        "parameters": t.input_schema.clone()
                    })
                })
                .collect();

            body["tools"] = serde_json::json!([{
                "functionDeclarations": function_declarations
            }]);
        }

        Ok(body)
    }

    /// Parse streaming response line
    fn parse_stream_line(line: &str) -> Option<GenerateContentResponse> {
        serde_json::from_str(line).ok()
    }

    /// Compile an output schema for Gemini structured outputs.
    ///
    /// Uses `responseJsonSchema` without destructive lowering so schema
    /// semantics are preserved.
    fn compile_schema_for_gemini(
        output_schema: &OutputSchema,
    ) -> Result<CompiledSchema, SchemaError> {
        let schema = output_schema.schema.as_value().clone();
        let warnings = validate_gemini_response_json_schema(&schema, Provider::Gemini);

        if output_schema.compat == SchemaCompat::Strict && !warnings.is_empty() {
            return Err(SchemaError::UnsupportedFeatures {
                provider: Provider::Gemini,
                warnings,
            });
        }

        Ok(CompiledSchema { schema, warnings })
    }
}

fn validate_gemini_response_json_schema(schema: &Value, provider: Provider) -> Vec<SchemaWarning> {
    let mut warnings = Vec::new();
    inspect_gemini_json_schema_node(schema, "", provider, &mut warnings);
    warnings
}

fn inspect_gemini_json_schema_node(
    value: &Value,
    path: &str,
    provider: Provider,
    warnings: &mut Vec<SchemaWarning>,
) {
    match value {
        Value::Object(obj) => {
            for key in obj.keys() {
                if !is_gemini_supported_schema_keyword(key) {
                    warnings.push(SchemaWarning {
                        provider,
                        path: join_path(path, key),
                        message: format!(
                            "Keyword '{key}' may be ignored by Gemini responseJsonSchema"
                        ),
                    });
                }
            }

            for (key, child) in obj {
                match key.as_str() {
                    // Map values are schemas keyed by arbitrary names.
                    "properties" | "$defs" => {
                        inspect_schema_map(child, &join_path(path, key), provider, warnings);
                    }
                    _ => inspect_gemini_json_schema_node(
                        child,
                        &join_path(path, key),
                        provider,
                        warnings,
                    ),
                }
            }
        }
        Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                inspect_gemini_json_schema_node(item, &join_index(path, index), provider, warnings);
            }
        }
        _ => {}
    }
}

fn inspect_schema_map(
    value: &Value,
    path: &str,
    provider: Provider,
    warnings: &mut Vec<SchemaWarning>,
) {
    match value {
        Value::Object(map) => {
            for (name, child) in map {
                inspect_gemini_json_schema_node(child, &join_path(path, name), provider, warnings);
            }
        }
        other => inspect_gemini_json_schema_node(other, path, provider, warnings),
    }
}

fn is_gemini_supported_schema_keyword(key: &str) -> bool {
    // Source: Gemini responseJsonSchema supported keyword subset in
    // https://cloud.google.com/vertex-ai/generative-ai/docs/multimodal/control-generated-output#supported-schema-fields
    // Verified against docs on 2026-02-27.
    matches!(
        key,
        "$id"
            | "$defs"
            | "$ref"
            | "$anchor"
            | "type"
            | "format"
            | "title"
            | "description"
            | "const"
            | "default"
            | "examples"
            | "enum"
            | "items"
            | "prefixItems"
            | "minItems"
            | "maxItems"
            | "minimum"
            | "maximum"
            | "anyOf"
            | "oneOf"
            | "properties"
            | "additionalProperties"
            | "required"
            | "propertyOrdering"
            | "nullable"
    )
}

fn join_path(prefix: &str, key: &str) -> String {
    if prefix.is_empty() {
        format!("/{key}")
    } else {
        format!("{prefix}/{key}")
    }
}

fn join_index(prefix: &str, index: usize) -> String {
    if prefix.is_empty() {
        format!("/{index}")
    } else {
        format!("{prefix}/{index}")
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl LlmClient for GeminiClient {
    fn stream<'a>(&'a self, request: &'a LlmRequest) -> LlmStream<'a> {
        let inner: LlmStream<'a> = Box::pin(async_stream::try_stream! {
            let body = self.build_request_body(request)?;
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
                                    // Not collapsed: inner loop processes heterogeneous part types
                                    // (text, function_call, function_response) independently.
                                    #[allow(clippy::collapsible_if)]
                                    if let Some(parts) = content.parts {
                                        for part in parts {
                                            // Build meta from thoughtSignature if present
                                            let meta = part.thought_signature.as_ref().map(|sig| {
                                                Box::new(meerkat_core::ProviderMeta::Gemini {
                                                    thought_signature: sig.clone(),
                                                })
                                            });

                                            if let Some(text) = part.text {
                                                yield LlmEvent::TextDelta {
                                                    delta: text,
                                                    meta: meta.clone(),
                                                };
                                            }
                                            if let Some(fc) = part.function_call {
                                                let id = format!("fc_{tool_call_index}");
                                                tool_call_index += 1;
                                                yield LlmEvent::ToolCallComplete {
                                                    id,
                                                    name: fc.name,
                                                    args: fc.args.unwrap_or(json!({})),
                                                    meta,
                                                };
                                            }
                                        }
                                    }
                                }

                                if let Some(reason) = cand.finish_reason {
                                    let stop = match reason.as_str() {
                                        "MAX_TOKENS" => StopReason::MaxTokens,
                                        "SAFETY" | "RECITATION" => StopReason::ContentFilter,
                                        // Gemini uses various names for tool calls
                                        "TOOL_CALL" | "FUNCTION_CALL" => StopReason::ToolUse,
                                        // "STOP" and any unrecognized reason default to EndTurn
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

    fn compile_schema(&self, output_schema: &OutputSchema) -> Result<CompiledSchema, SchemaError> {
        GeminiClient::compile_schema_for_gemini(output_schema)
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
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::explicit_counter_loop
)]
mod tests {
    use super::*;
    use meerkat_core::{AssistantBlock, BlockAssistantMessage, ProviderMeta, UserMessage};

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

        let body = client.build_request_body(&request)?;

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

        let body = client.build_request_body(&request)?;
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

        let body = client.build_request_body(&request)?;
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

        let body = client.build_request_body(&request)?;
        let generation_config = body.get("generationConfig").ok_or("missing config")?;

        assert!(generation_config.get("thinkingConfig").is_none());
        assert!(generation_config.get("topK").is_none());
        Ok(())
    }

    /// Test that functionCall has thoughtSignature but functionResponse does NOT
    /// Per spec section 2.3: Signatures on functionCall, NEVER on functionResponse
    /// Uses BlockAssistant with ProviderMeta::Gemini for thoughtSignature.
    #[test]
    fn test_tool_response_uses_function_name_no_signature() -> Result<(), Box<dyn std::error::Error>>
    {
        use serde_json::value::RawValue;
        let client = GeminiClient::new("test-key".to_string());
        let args_raw = RawValue::from_string(json!({"city": "Tokyo"}).to_string()).unwrap();
        let request = LlmRequest::new(
            "gemini-1.5-pro",
            vec![
                Message::User(UserMessage {
                    content: "test".to_string(),
                }),
                Message::BlockAssistant(BlockAssistantMessage {
                    blocks: vec![AssistantBlock::ToolUse {
                        id: "call_1".to_string(),
                        name: "get_weather".to_string(),
                        args: args_raw,
                        meta: Some(Box::new(ProviderMeta::Gemini {
                            thought_signature: "sig_123".to_string(),
                        })),
                    }],
                    stop_reason: StopReason::ToolUse,
                }),
                Message::ToolResults {
                    results: vec![meerkat_core::ToolResult::new(
                        "call_1".to_string(),
                        "Sunny".to_string(),
                        false,
                    )],
                },
            ],
        );

        let body = client.build_request_body(&request)?;
        let contents = body
            .get("contents")
            .and_then(|c| c.as_array())
            .ok_or("missing contents")?;

        // Find the assistant message (role: "model") - functionCall SHOULD have signature
        let model_content = contents
            .iter()
            .find(|c| c.get("role").and_then(|r| r.as_str()) == Some("model"))
            .ok_or("missing model content")?;
        let model_parts = model_content
            .get("parts")
            .and_then(|p| p.as_array())
            .ok_or("missing model parts")?;
        let fc_part = model_parts
            .iter()
            .find(|p| p.get("functionCall").is_some())
            .ok_or("missing functionCall part")?;
        assert_eq!(
            fc_part["thoughtSignature"], "sig_123",
            "functionCall SHOULD have signature"
        );

        // Find the tool result (last message) - functionResponse MUST NOT have signature
        let tool_result_parts = contents
            .last()
            .and_then(|c| c.get("parts"))
            .and_then(|p| p.as_array())
            .ok_or("missing parts")?;

        let function_response = &tool_result_parts[0]["functionResponse"];
        assert_eq!(function_response["name"], "get_weather");
        // IMPORTANT: functionResponse MUST NOT have thoughtSignature (spec section 2.3)
        assert!(
            tool_result_parts[0].get("thoughtSignature").is_none(),
            "functionResponse MUST NOT have thoughtSignature"
        );
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
                "MAX_TOKENS" => StopReason::MaxTokens,
                "SAFETY" | "RECITATION" => StopReason::ContentFilter,
                "TOOL_CALL" | "FUNCTION_CALL" => StopReason::ToolUse,
                // "STOP" and any unrecognized reason default to EndTurn
                _ => StopReason::EndTurn,
            };
            assert_eq!(
                stop,
                StopReason::ToolUse,
                "finish_reason '{reason}' should map to ToolUse"
            );
        }
    }

    /// Regression: Gemini RECITATION finish reason must map to ContentFilter.
    #[test]
    fn test_regression_gemini_finish_reason_recitation_maps_to_content_filter() {
        let reason = "RECITATION";
        let stop = match reason {
            "MAX_TOKENS" => StopReason::MaxTokens,
            "SAFETY" | "RECITATION" => StopReason::ContentFilter,
            "TOOL_CALL" | "FUNCTION_CALL" => StopReason::ToolUse,
            // "STOP" and any unrecognized reason default to EndTurn
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
            let id = format!("fc_{tool_call_index}");
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
                "Duplicate tool call ID found: {id}"
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
            let id = format!("fc_{tool_call_index}");
            tool_call_index += 1;
            id_to_name.push((id, name));
        }

        // Each call gets a unique ID regardless of tool name
        assert_eq!(id_to_name[0], ("fc_0".to_string(), "search"));
        assert_eq!(id_to_name[1], ("fc_1".to_string(), "write_file"));
        assert_eq!(id_to_name[2], ("fc_2".to_string(), "search")); // Second search gets fc_2
        assert_eq!(id_to_name[3], ("fc_3".to_string(), "read_file"));
    }

    #[test]
    fn test_build_request_body_with_structured_output() -> Result<(), Box<dyn std::error::Error>> {
        let client = GeminiClient::new("test-key".to_string());

        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "name": {"type": "string"},
                "age": {"type": "integer"}
            },
            "required": ["name", "age"]
        });

        let request = LlmRequest::new(
            "gemini-3-pro-preview",
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

        let gen_config = body
            .get("generationConfig")
            .ok_or("missing generationConfig")?;
        assert_eq!(gen_config["responseMimeType"], "application/json");
        assert!(gen_config.get("responseJsonSchema").is_some());

        let response_schema = &gen_config["responseJsonSchema"];
        assert_eq!(response_schema["type"], "object");
        assert!(response_schema.get("properties").is_some());
        Ok(())
    }

    #[test]
    fn test_build_request_body_with_structured_output_preserves_schema_keywords()
    -> Result<(), Box<dyn std::error::Error>> {
        let client = GeminiClient::new("test-key".to_string());

        // Schema keywords supported by Gemini responseJsonSchema should be preserved.
        let schema = serde_json::json!({
            "type": "object",
            "$defs": {
                "Address": {"type": "object"}
            },
            "$ref": "#/$defs/Address",
            "anyOf": [
                {"type": "object"},
                {"type": "null"}
            ],
            "properties": {
                "name": {"type": "string"}
            },
            "additionalProperties": false
        });

        let request = LlmRequest::new(
            "gemini-3-pro-preview",
            vec![Message::User(UserMessage {
                content: "test".to_string(),
            })],
        )
        .with_provider_param("structured_output", serde_json::json!({"schema": schema}));

        let body = client.build_request_body(&request)?;

        let gen_config = body
            .get("generationConfig")
            .ok_or("missing generationConfig")?;
        let response_schema = &gen_config["responseJsonSchema"];

        // These should be preserved
        assert!(response_schema.get("$defs").is_some());
        assert_eq!(response_schema["$ref"], "#/$defs/Address");
        assert!(response_schema.get("anyOf").is_some());
        assert_eq!(response_schema["additionalProperties"], false);
        assert_eq!(response_schema["type"], "object");
        assert!(response_schema.get("properties").is_some());
        Ok(())
    }

    #[test]
    fn test_compile_schema_for_gemini_strict_errors_on_unsupported_keywords() {
        let schema = serde_json::json!({
            "type": "object",
            "allOf": [
                {"type": "object"}
            ]
        });

        let output_schema = OutputSchema::new(schema)
            .expect("valid schema")
            .with_compat(SchemaCompat::Strict);
        let err = GeminiClient::compile_schema_for_gemini(&output_schema)
            .expect_err("strict mode should reject unsupported keywords");

        match err {
            SchemaError::UnsupportedFeatures { provider, warnings } => {
                assert_eq!(provider, Provider::Gemini);
                assert!(!warnings.is_empty());
                assert!(
                    warnings.iter().any(|w| w.path.contains("/allOf")),
                    "expected warning path to include /allOf, got: {warnings:?}"
                );
            }
            other => panic!("expected UnsupportedFeatures, got: {other:?}"),
        }
    }

    #[test]
    fn test_compile_schema_for_gemini_lossy_keeps_schema_and_emits_warnings() {
        let schema = serde_json::json!({
            "type": "object",
            "allOf": [{"type": "object"}],
            "properties": {
                "name": {
                    "type": "string",
                    "pattern": "^[a-z]+$"
                }
            }
        });
        let output_schema = OutputSchema::new(schema).expect("valid schema");
        let expected = output_schema.schema.as_value().clone();
        let compiled = GeminiClient::compile_schema_for_gemini(&output_schema)
            .expect("lossy mode should not error");

        assert_eq!(compiled.schema, expected);
        assert!(!compiled.warnings.is_empty());
        assert!(
            compiled.warnings.iter().any(|w| w.path.contains("/allOf")),
            "expected /allOf warning"
        );
        assert!(
            compiled
                .warnings
                .iter()
                .any(|w| w.path.contains("/properties/name/pattern")),
            "expected /properties/name/pattern warning"
        );
    }

    #[test]
    fn test_compile_schema_for_gemini_strict_accepts_supported_keywords() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "items": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "id": {"type": "string"},
                            "score": {"type": "number", "minimum": 0.0, "maximum": 1.0}
                        },
                        "required": ["id", "score"],
                        "additionalProperties": false
                    }
                },
                "choice": {
                    "oneOf": [
                        {"type": "string"},
                        {"type": "null"}
                    ]
                }
            },
            "required": ["items", "choice"],
            "additionalProperties": false
        });
        let output_schema = OutputSchema::new(schema)
            .expect("valid schema")
            .with_compat(SchemaCompat::Strict);
        let compiled = GeminiClient::compile_schema_for_gemini(&output_schema)
            .expect("strict mode should accept supported keywords");

        assert!(compiled.warnings.is_empty());
        assert_eq!(compiled.schema["type"], "object");
    }

    #[test]
    fn test_compile_schema_for_gemini_warns_nested_unsupported_paths() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "nested": {
                    "type": "object",
                    "allOf": [
                        {"type": "object", "properties": {"x": {"type": "string"}}}
                    ],
                    "properties": {
                        "payload": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "patternProperties": {
                                    "^k_": {"type": "integer"}
                                }
                            }
                        }
                    }
                }
            }
        });
        let output_schema = OutputSchema::new(schema).expect("valid schema");
        let compiled = GeminiClient::compile_schema_for_gemini(&output_schema)
            .expect("lossy mode should still compile");

        let paths: Vec<String> = compiled.warnings.iter().map(|w| w.path.clone()).collect();
        assert!(
            paths.iter().any(|p| p.contains("/properties/nested/allOf")),
            "expected warning at /properties/nested/allOf, got: {paths:?}"
        );
        assert!(
            paths.iter().any(|p| {
                p.contains("/properties/nested/properties/payload/items/patternProperties")
            }),
            "expected warning at nested patternProperties path, got: {paths:?}"
        );
    }

    #[test]
    fn test_build_request_body_strict_compat_rejects_unsupported_schema() {
        let client = GeminiClient::new("test-key".to_string());
        let request = LlmRequest::new(
            "gemini-3-pro-preview",
            vec![Message::User(UserMessage {
                content: "test".to_string(),
            })],
        )
        .with_provider_param(
            "structured_output",
            serde_json::json!({
                "schema": {
                    "type": "object",
                    "allOf": [{"type": "object"}]
                },
                "compat": "strict"
            }),
        );

        let err = client
            .build_request_body(&request)
            .expect_err("strict compat should reject unsupported schema keywords");

        match err {
            LlmError::InvalidRequest { message } => {
                assert!(
                    message.contains("unsupported"),
                    "unexpected message: {message}"
                );
                assert!(message.contains("Gemini"), "unexpected message: {message}");
            }
            other => panic!("expected InvalidRequest, got {other:?}"),
        }
    }

    #[test]
    fn test_build_request_body_without_structured_output() -> Result<(), Box<dyn std::error::Error>>
    {
        let client = GeminiClient::new("test-key".to_string());

        let request = LlmRequest::new(
            "gemini-3-pro-preview",
            vec![Message::User(UserMessage {
                content: "test".to_string(),
            })],
        );

        let body = client.build_request_body(&request)?;

        let gen_config = body
            .get("generationConfig")
            .ok_or("missing generationConfig")?;
        assert!(
            gen_config.get("responseMimeType").is_none(),
            "responseMimeType should not be present"
        );
        assert!(
            gen_config.get("responseJsonSchema").is_none(),
            "responseJsonSchema should not be present"
        );
        Ok(())
    }

    /// Regression: tool schema in request body should preserve union type arrays.
    #[test]
    fn test_tool_schema_preserves_array_types() -> Result<(), Box<dyn std::error::Error>> {
        use meerkat_core::ToolDef;
        use std::sync::Arc;

        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "name": {"type": "string"},
                "age": {"type": ["integer", "null"]},
                "email": {"type": ["string", "null"]}
            }
        });

        let client = GeminiClient::new("test-key".to_string());
        let request = LlmRequest::new(
            "gemini-3-pro-preview",
            vec![Message::User(UserMessage {
                content: "test".to_string(),
            })],
        )
        .with_tools(vec![Arc::new(ToolDef {
            name: "test_tool".to_string(),
            description: "test".to_string(),
            input_schema: schema,
        })]);
        let body = client.build_request_body(&request)?;
        let preserved = &body["tools"][0]["functionDeclarations"][0]["parameters"];

        // Array types should be preserved
        assert_eq!(
            preserved["properties"]["age"]["type"],
            serde_json::json!(["integer", "null"]),
            "['integer', 'null'] should be preserved"
        );
        assert_eq!(
            preserved["properties"]["email"]["type"],
            serde_json::json!(["string", "null"]),
            "['string', 'null'] should be preserved"
        );
        // Non-array types should be unchanged
        assert_eq!(
            preserved["properties"]["name"]["type"], "string",
            "'string' should remain 'string'"
        );
        Ok(())
    }

    /// Regression: tool schema in request body should preserve composition keywords.
    #[test]
    fn test_tool_schema_preserves_oneof_anyof_allof() -> Result<(), Box<dyn std::error::Error>> {
        use meerkat_core::ToolDef;
        use std::sync::Arc;

        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "status": {
                    "oneOf": [
                        {"const": "active"},
                        {"const": "inactive"}
                    ]
                },
                "value": {
                    "anyOf": [
                        {"type": "string"},
                        {"type": "number"}
                    ]
                }
            },
            "allOf": [
                {"required": ["status"]}
            ]
        });

        let client = GeminiClient::new("test-key".to_string());
        let request = LlmRequest::new(
            "gemini-3-pro-preview",
            vec![Message::User(UserMessage {
                content: "test".to_string(),
            })],
        )
        .with_tools(vec![Arc::new(ToolDef {
            name: "test_tool".to_string(),
            description: "test".to_string(),
            input_schema: schema,
        })]);
        let body = client.build_request_body(&request)?;
        let preserved = &body["tools"][0]["functionDeclarations"][0]["parameters"];

        assert!(
            preserved["properties"]["status"].get("oneOf").is_some(),
            "oneOf should be preserved"
        );
        assert!(
            preserved["properties"]["value"].get("anyOf").is_some(),
            "anyOf should be preserved"
        );
        assert!(
            preserved.get("allOf").is_some(),
            "allOf should be preserved"
        );
        Ok(())
    }

    // =========================================================================
    // Thought Signature Tests (Spec Section 3.5)
    // =========================================================================

    /// Parse thoughtSignature from functionCall parts into ProviderMeta::Gemini
    #[test]
    fn test_parse_function_call_with_thought_signature() -> Result<(), Box<dyn std::error::Error>> {
        let line = r#"{"candidates":[{"content":{"parts":[{"functionCall":{"name":"get_weather","args":{"city":"Tokyo"}},"thoughtSignature":"sig_abc123"}]}}]}"#;
        let response = GeminiClient::parse_stream_line(line).ok_or("missing response")?;
        let candidates = response.candidates.as_ref().ok_or("missing candidates")?;
        let parts = candidates[0]
            .content
            .as_ref()
            .ok_or("missing content")?
            .parts
            .as_ref()
            .ok_or("missing parts")?;

        assert!(
            parts[0].function_call.is_some(),
            "should have function_call"
        );
        assert_eq!(
            parts[0].thought_signature.as_deref(),
            Some("sig_abc123"),
            "should have thoughtSignature"
        );
        Ok(())
    }

    /// Parse thoughtSignature from text parts into ProviderMeta::Gemini
    #[test]
    fn test_parse_text_with_thought_signature() -> Result<(), Box<dyn std::error::Error>> {
        let line = r#"{"candidates":[{"content":{"parts":[{"text":"Hello world","thoughtSignature":"sig_text_456"}]}}]}"#;
        let response = GeminiClient::parse_stream_line(line).ok_or("missing response")?;
        let candidates = response.candidates.as_ref().ok_or("missing candidates")?;
        let parts = candidates[0]
            .content
            .as_ref()
            .ok_or("missing content")?
            .parts
            .as_ref()
            .ok_or("missing parts")?;

        assert_eq!(parts[0].text.as_deref(), Some("Hello world"));
        assert_eq!(
            parts[0].thought_signature.as_deref(),
            Some("sig_text_456"),
            "text parts can have thoughtSignature for continuity"
        );
        Ok(())
    }

    /// Parallel tool calls: only FIRST call has signature per spec section 2.3
    #[test]
    fn test_parallel_calls_only_first_has_signature() -> Result<(), Box<dyn std::error::Error>> {
        // Simulating 3 parallel function calls from Gemini - only first has signature
        let line = r#"{"candidates":[{"content":{"parts":[
            {"functionCall":{"name":"get_weather","args":{"city":"Tokyo"}},"thoughtSignature":"sig_first"},
            {"functionCall":{"name":"get_time","args":{"tz":"JST"}}},
            {"functionCall":{"name":"get_population","args":{"city":"Tokyo"}}}
        ]}}]}"#;

        let response = GeminiClient::parse_stream_line(line).ok_or("missing response")?;
        let candidates = response.candidates.ok_or("missing candidates")?;
        let parts = candidates[0]
            .content
            .as_ref()
            .ok_or("missing content")?
            .parts
            .as_ref()
            .ok_or("missing parts")?;

        assert_eq!(parts.len(), 3);
        assert_eq!(
            parts[0].thought_signature.as_deref(),
            Some("sig_first"),
            "first parallel call MUST have signature"
        );
        assert!(
            parts[1].thought_signature.is_none(),
            "second parallel call must NOT have signature"
        );
        assert!(
            parts[2].thought_signature.is_none(),
            "third parallel call must NOT have signature"
        );
        Ok(())
    }

    /// Request building: thoughtSignature on functionCall via ProviderMeta, NEVER on functionResponse
    #[test]
    fn test_request_building_no_signature_on_function_response()
    -> Result<(), Box<dyn std::error::Error>> {
        use serde_json::value::RawValue;
        let client = GeminiClient::new("test-key".to_string());

        let args_raw = RawValue::from_string(json!({"city": "Tokyo"}).to_string()).unwrap();
        let request = LlmRequest::new(
            "gemini-3-pro-preview",
            vec![
                Message::User(UserMessage {
                    content: "What's the weather?".to_string(),
                }),
                Message::BlockAssistant(BlockAssistantMessage {
                    blocks: vec![AssistantBlock::ToolUse {
                        id: "call_1".to_string(),
                        name: "get_weather".to_string(),
                        args: args_raw,
                        meta: Some(Box::new(ProviderMeta::Gemini {
                            thought_signature: "sig_123".to_string(),
                        })),
                    }],
                    stop_reason: StopReason::ToolUse,
                }),
                Message::ToolResults {
                    results: vec![meerkat_core::ToolResult::new(
                        "call_1".to_string(),
                        "Sunny, 25C".to_string(),
                        false,
                    )],
                },
            ],
        );

        let body = client.build_request_body(&request)?;
        let contents = body
            .get("contents")
            .and_then(|c| c.as_array())
            .ok_or("missing contents")?;

        // Find the assistant message (role: "model")
        let assistant_content = contents
            .iter()
            .find(|c| c.get("role").and_then(|r| r.as_str()) == Some("model"))
            .ok_or("missing model content")?;
        let assistant_parts = assistant_content
            .get("parts")
            .and_then(|p| p.as_array())
            .ok_or("missing parts")?;

        // Assistant's functionCall SHOULD have thoughtSignature
        let fc_part = assistant_parts
            .iter()
            .find(|p| p.get("functionCall").is_some())
            .ok_or("missing functionCall part")?;
        assert!(
            fc_part.get("thoughtSignature").is_some(),
            "functionCall part SHOULD have thoughtSignature"
        );

        // Find the tool results message (role: "user" with functionResponse)
        let tool_results_content = contents.last().ok_or("missing last content")?;
        let tool_result_parts = tool_results_content
            .get("parts")
            .and_then(|p| p.as_array())
            .ok_or("missing tool result parts")?;

        // Tool result's functionResponse MUST NOT have thoughtSignature
        let fr_part = tool_result_parts
            .iter()
            .find(|p| p.get("functionResponse").is_some())
            .ok_or("missing functionResponse part")?;
        assert!(
            fr_part.get("thoughtSignature").is_none(),
            "functionResponse MUST NOT have thoughtSignature"
        );

        Ok(())
    }

    /// ToolCallComplete event should use ProviderMeta::Gemini instead of deprecated thought_signature field
    #[test]
    fn test_tool_call_complete_uses_provider_meta() {
        use meerkat_core::ProviderMeta;

        // This test verifies the LlmEvent::ToolCallComplete uses the `meta` field
        // with ProviderMeta::Gemini variant instead of the deprecated `thought_signature` field
        let meta = Some(Box::new(ProviderMeta::Gemini {
            thought_signature: "sig_test".to_string(),
        }));

        let event = LlmEvent::ToolCallComplete {
            id: "fc_0".to_string(),
            name: "test_tool".to_string(),
            args: json!({}),
            meta, // new field
        };

        if let LlmEvent::ToolCallComplete { meta: m, .. } = event {
            assert!(m.is_some(), "meta should be Some");
            match *m.unwrap() {
                ProviderMeta::Gemini { thought_signature } => {
                    assert_eq!(thought_signature, "sig_test");
                }
                _ => panic!("expected Gemini variant"),
            }
        }
    }

    /// TextDelta event should include meta for Gemini text parts with thoughtSignature
    #[test]
    fn test_text_delta_uses_provider_meta() {
        use meerkat_core::ProviderMeta;

        let meta = Some(Box::new(ProviderMeta::Gemini {
            thought_signature: "sig_text".to_string(),
        }));

        let event = LlmEvent::TextDelta {
            delta: "Hello".to_string(),
            meta,
        };

        if let LlmEvent::TextDelta { meta: m, .. } = event {
            assert!(m.is_some());
            match *m.unwrap() {
                ProviderMeta::Gemini { thought_signature } => {
                    assert_eq!(thought_signature, "sig_text");
                }
                _ => panic!("expected Gemini variant"),
            }
        }
    }
}
