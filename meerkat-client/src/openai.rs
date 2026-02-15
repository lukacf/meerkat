//! OpenAI API client - Responses API
//!
//! Implements the LlmClient trait for OpenAI's Responses API.
//! This client uses the /v1/responses endpoint which supports reasoning items.

use crate::BlockAssembler;
use crate::error::LlmError;
use crate::types::{LlmClient, LlmDoneOutcome, LlmEvent, LlmRequest};
use async_trait::async_trait;
use futures::{Stream, StreamExt};
use meerkat_core::{AssistantBlock, Message, OutputSchema, ProviderMeta, StopReason, Usage};
use serde::Deserialize;
use serde_json::Value;
use serde_json::value::RawValue;
use std::pin::Pin;

/// Client for OpenAI Responses API
pub struct OpenAiClient {
    api_key: String,
    base_url: String,
    http: reqwest::Client,
}

impl OpenAiClient {
    /// Create a new OpenAI client with the given API key
    pub fn new(api_key: String) -> Self {
        Self::new_with_base_url(api_key, "https://api.openai.com".to_string())
    }

    /// Create a new OpenAI client with an explicit base URL
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

    /// Create from environment variable OPENAI_API_KEY
    pub fn from_env() -> Result<Self, LlmError> {
        let api_key = std::env::var("RKAT_OPENAI_API_KEY")
            .or_else(|_| std::env::var("OPENAI_API_KEY"))
            .map_err(|_| LlmError::InvalidApiKey)?;
        Ok(Self::new(api_key))
    }

    /// Build request body for OpenAI Responses API
    fn build_request_body(&self, request: &LlmRequest) -> Result<Value, LlmError> {
        let input = Self::convert_to_responses_input(&request.messages);

        let mut body = serde_json::json!({
            "model": request.model,
            "input": input,
            "max_output_tokens": request.max_tokens,
            "stream": true,
            // Request encrypted_content for stateless replay
            "include": ["reasoning.encrypted_content"],
        });

        // Enable reasoning with default effort
        body["reasoning"] = serde_json::json!({
            "effort": "medium",
            "summary": "auto"
        });

        if let Some(temp) = request.temperature {
            if let Some(num) = serde_json::Number::from_f64(temp as f64) {
                body["temperature"] = Value::Number(num);
            }
        }

        if !request.tools.is_empty() {
            // Responses API tool format: {"type": "function", "name": "...", "parameters": {...}}
            let tools: Vec<Value> = request
                .tools
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "type": "function",
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.input_schema
                    })
                })
                .collect();
            body["tools"] = Value::Array(tools);
        }

        // Extract OpenAI-specific parameters from provider_params
        if let Some(params) = &request.provider_params {
            if let Some(reasoning_effort) = params.get("reasoning_effort") {
                body["reasoning"]["effort"] = reasoning_effort.clone();
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
            if let Some(structured) = params.get("structured_output") {
                let output_schema: OutputSchema = serde_json::from_value(structured.clone())
                    .map_err(|e| LlmError::InvalidRequest {
                        message: format!("Invalid structured_output schema: {e}"),
                    })?;
                let compiled =
                    self.compile_schema(&output_schema)
                        .map_err(|e| LlmError::InvalidRequest {
                            message: e.to_string(),
                        })?;
                let name = output_schema.name.as_deref().unwrap_or("output");
                let strict = output_schema.strict;

                body["text"] = serde_json::json!({
                    "format": {
                        "type": "json_schema",
                        "name": name,
                        "schema": compiled.schema,
                        "strict": strict
                    }
                });
            }
        }

        Ok(body)
    }

    /// Convert messages to Responses API input format.
    ///
    /// OpenAI requires every `reasoning` item to be immediately followed by an
    /// output item (`message` with `role: assistant` or `function_call`). Orphaned
    /// reasoning items (e.g., from stream interruption or cancellation) are stripped
    /// in a post-processing pass to prevent `invalid_request_error`.
    fn convert_to_responses_input(messages: &[Message]) -> Vec<Value> {
        let mut items = Vec::new();

        for msg in messages {
            match msg {
                Message::System(s) => {
                    items.push(serde_json::json!({
                        "type": "message",
                        "role": "system",
                        "content": s.content
                    }));
                }
                Message::User(u) => {
                    items.push(serde_json::json!({
                        "type": "message",
                        "role": "user",
                        "content": u.content
                    }));
                }
                Message::Assistant(a) => {
                    // Legacy AssistantMessage format - convert to items
                    if !a.content.is_empty() {
                        items.push(serde_json::json!({
                            "type": "message",
                            "role": "assistant",
                            "content": a.content
                        }));
                    }
                    for tc in &a.tool_calls {
                        items.push(serde_json::json!({
                            "type": "function_call",
                            "call_id": tc.id,
                            "name": tc.name,
                            "arguments": tc.args.to_string()
                        }));
                    }
                }
                Message::BlockAssistant(a) => {
                    // New BlockAssistantMessage format - render blocks as items
                    for block in &a.blocks {
                        match block {
                            AssistantBlock::Text { text, .. } => {
                                if !text.is_empty() {
                                    items.push(serde_json::json!({
                                        "type": "message",
                                        "role": "assistant",
                                        "content": text
                                    }));
                                }
                            }
                            AssistantBlock::Reasoning {
                                text,
                                meta: Some(meta),
                            } => {
                                if let ProviderMeta::OpenAi {
                                    id,
                                    encrypted_content,
                                } = meta.as_ref()
                                {
                                    // OpenAI requires id and summary for reasoning items
                                    let mut item = serde_json::json!({
                                        "type": "reasoning",
                                        "id": id,
                                        "summary": [{"type": "summary_text", "text": text}]
                                    });
                                    if let Some(enc) = encrypted_content {
                                        item["encrypted_content"] = serde_json::json!(enc);
                                    }
                                    items.push(item);
                                }
                                // Skip reasoning blocks without OpenAI metadata
                            }
                            AssistantBlock::Reasoning { .. } => {}
                            AssistantBlock::ToolUse { id, name, args, .. } => {
                                items.push(serde_json::json!({
                                    "type": "function_call",
                                    "call_id": id,
                                    "name": name,
                                    "arguments": args.get()  // Already JSON string
                                }));
                            }
                            _ => {} // non_exhaustive: ignore unknown future variants
                        }
                    }
                }
                Message::ToolResults { results } => {
                    for r in results {
                        items.push(serde_json::json!({
                            "type": "function_call_output",
                            "call_id": r.tool_use_id,
                            "output": r.content
                        }));
                    }
                }
            }
        }

        Self::strip_orphaned_reasoning(&mut items);
        items
    }

    /// Remove reasoning items not immediately followed by an output item.
    ///
    /// OpenAI's Responses API requires every `reasoning` item to be immediately
    /// followed by either a `message` with `role: assistant` or a `function_call`.
    /// Orphaned reasoning can appear when a stream is interrupted/cancelled after
    /// reasoning completes but before output is produced, or when a response hits
    /// `max_output_tokens` during reasoning.
    fn strip_orphaned_reasoning(items: &mut Vec<Value>) {
        let mut i = 0;
        while i < items.len() {
            let is_reasoning = items[i]
                .get("type")
                .and_then(|t| t.as_str())
                == Some("reasoning");

            if !is_reasoning {
                i += 1;
                continue;
            }

            // Check if the next item is a valid output for this reasoning
            let has_valid_follower = items.get(i + 1).is_some_and(|next| {
                let next_type = next.get("type").and_then(|t| t.as_str());
                match next_type {
                    Some("function_call") => true,
                    Some("message") => {
                        next.get("role").and_then(|r| r.as_str()) == Some("assistant")
                    }
                    _ => false,
                }
            });

            if has_valid_follower {
                i += 1;
            } else {
                tracing::warn!(
                    reasoning_id = items[i].get("id").and_then(|id| id.as_str()).unwrap_or("?"),
                    "stripping orphaned reasoning item (no valid following output)"
                );
                items.remove(i);
                // Don't increment i — the next element shifted into this position
            }
        }
    }

    /// Parse an SSE event from the Responses API stream
    fn parse_responses_sse_line(line: &str) -> Option<ResponsesStreamEvent> {
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
                let body = self.build_request_body(request)?;

                let response = self.http
                    .post(format!("{}/v1/responses", self.base_url))
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
                let mut assembler = BlockAssembler::new();
                let mut usage = Usage::default();

                while let Some(chunk) = stream.next().await {
                    let chunk = chunk.map_err(|_| LlmError::ConnectionReset)?;
                    buffer.push_str(&String::from_utf8_lossy(&chunk));

                    while let Some(newline_pos) = buffer.find('\n') {
                        let line = buffer[..newline_pos].trim();
                        let should_process = !line.is_empty() && !line.starts_with(':');
                        let parsed_event = if should_process {
                            Self::parse_responses_sse_line(line)
                        } else {
                            None
                        };

                        buffer.drain(..=newline_pos);

                        if let Some(event) = parsed_event {
                            // Handle response.completed event (non-streaming final response)
                            if event.event_type == "response.completed" {
                                if let Some(response_obj) = &event.response {
                                    // Process output items
                                    if let Some(output) = response_obj.get("output").and_then(|o| o.as_array()) {
                                        for item in output {
                                            if let Some(item_type) = item.get("type").and_then(|t| t.as_str()) {
                                                match item_type {
                                                    "message" => {
                                                        // content is an array: [{"type": "output_text", "text": "..."}, ...]
                                                        if let Some(content_parts) = item.get("content").and_then(|c| c.as_array()) {
                                                            for part in content_parts {
                                                                if let Some(part_type) = part.get("type").and_then(|t| t.as_str()) {
                                                                    match part_type {
                                                                        "output_text" => {
                                                                            if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                                                                                assembler.on_text_delta(text, None);
                                                                                yield LlmEvent::TextDelta { delta: text.to_string(), meta: None };
                                                                            }
                                                                        }
                                                                        "refusal" => {
                                                                            if let Some(refusal) = part.get("refusal").and_then(|r| r.as_str()) {
                                                                                assembler.on_text_delta(refusal, None);
                                                                                yield LlmEvent::TextDelta { delta: refusal.to_string(), meta: None };
                                                                            }
                                                                        }
                                                                        _ => {}
                                                                    }
                                                                }
                                                            }
                                                        }
                                                    }
                                                    "reasoning" => {
                                                        // Required: reasoning item ID for replay
                                                        let Some(reasoning_id) = item.get("id").and_then(|i| i.as_str()) else {
                                                            tracing::warn!("reasoning item missing id, skipping");
                                                            continue;
                                                        };

                                                        // Extract summary text
                                                        let mut summary_text = String::new();
                                                        if let Some(summaries) = item.get("summary").and_then(|s| s.as_array()) {
                                                            for summary in summaries {
                                                                if let Some(text) = summary.get("text").and_then(|t| t.as_str()) {
                                                                    if !summary_text.is_empty() {
                                                                        summary_text.push('\n');
                                                                    }
                                                                    summary_text.push_str(text);
                                                                }
                                                            }
                                                        }

                                                        // encrypted_content is optional
                                                        let encrypted = item.get("encrypted_content")
                                                            .and_then(|v| v.as_str())
                                                            .map(|s| s.to_string());

                                                        let meta = Some(Box::new(ProviderMeta::OpenAi {
                                                            id: reasoning_id.to_string(),
                                                            encrypted_content: encrypted,
                                                        }));

                                                        assembler.on_reasoning_start();
                                                        if !summary_text.is_empty() {
                                                            let _ = assembler.on_reasoning_delta(&summary_text);
                                                        }
                                                        assembler.on_reasoning_complete(meta.clone());

                                                        yield LlmEvent::ReasoningComplete {
                                                            text: summary_text,
                                                            meta,
                                                        };
                                                    }
                                                    "function_call" => {
                                                        // Extract required fields
                                                        let Some(call_id) = item.get("call_id").and_then(|c| c.as_str()) else {
                                                            tracing::warn!("function_call missing call_id");
                                                            continue;
                                                        };
                                                        let Some(name) = item.get("name").and_then(|n| n.as_str()) else {
                                                            tracing::warn!(call_id, "function_call missing name");
                                                            continue;
                                                        };
                                                        // arguments is a JSON string
                                                        let args: Box<RawValue> = match item.get("arguments").and_then(|a| a.as_str()) {
                                                            Some(args_str) => match RawValue::from_string(args_str.to_string()) {
                                                                Ok(raw) => raw,
                                                                Err(e) => {
                                                                    tracing::error!(call_id, "invalid args JSON, skipping: {e}");
                                                                    continue;
                                                                }
                                                            },
                                                            None => {
                                                                // Empty args - treat as empty object
                                                                fallback_raw_value()
                                                            }
                                                        };

                                                        let _ = assembler.on_tool_call_start(call_id.to_string());
                                                        let _ = assembler.on_tool_call_complete(
                                                            call_id.to_string(),
                                                            name.to_string(),
                                                            args.clone(),
                                                            None,
                                                        );

                                                        // Also emit as legacy Value for compatibility
                                                        let args_value: Value = serde_json::from_str(args.get()).unwrap_or_default();
                                                        yield LlmEvent::ToolCallComplete {
                                                            id: call_id.to_string(),
                                                            name: name.to_string(),
                                                            args: args_value,
                                                            meta: None,
                                                        };
                                                    }
                                                    _ => {}
                                                }
                                            }
                                        }
                                    }

                                    // Extract usage
                                    if let Some(usage_obj) = response_obj.get("usage") {
                                        usage.input_tokens = usage_obj.get("input_tokens")
                                            .and_then(|v| v.as_u64())
                                            .unwrap_or(0);
                                        usage.output_tokens = usage_obj.get("output_tokens")
                                            .and_then(|v| v.as_u64())
                                            .unwrap_or(0);
                                        yield LlmEvent::UsageUpdate { usage: usage.clone() };
                                    }

                                    // Determine stop reason
                                    let stop_reason = match response_obj.get("status").and_then(|s| s.as_str()) {
                                        Some("completed") => {
                                            // Check if there were tool calls
                                            let has_tool_calls = response_obj.get("output")
                                                .and_then(|o| o.as_array())
                                                .map(|arr| arr.iter().any(|item| item.get("type").and_then(|t| t.as_str()) == Some("function_call")))
                                                .unwrap_or(false);
                                            if has_tool_calls {
                                                StopReason::ToolUse
                                            } else {
                                                StopReason::EndTurn
                                            }
                                        }
                                        Some("incomplete") => {
                                            match response_obj.get("incomplete_details").and_then(|d| d.get("reason")).and_then(|r| r.as_str()) {
                                                Some("max_output_tokens") => StopReason::MaxTokens,
                                                Some("content_filter") => StopReason::ContentFilter,
                                                _ => StopReason::EndTurn,
                                            }
                                        }
                                        Some("cancelled") => StopReason::Cancelled,
                                        _ => StopReason::EndTurn,
                                    };

                                    yield LlmEvent::Done {
                                        outcome: LlmDoneOutcome::Success { stop_reason },
                                    };
                                }
                            }
                            // Handle streaming delta events
                            else if event.event_type == "response.output_text.delta" {
                                if let Some(delta) = &event.delta {
                                    assembler.on_text_delta(delta, None);
                                    yield LlmEvent::TextDelta { delta: delta.clone(), meta: None };
                                }
                            }
                            else if event.event_type == "response.reasoning_summary_text.delta" {
                                if let Some(delta) = &event.delta {
                                    yield LlmEvent::ReasoningDelta { delta: delta.clone() };
                                }
                            }
                            else if event.event_type == "response.function_call_arguments.delta" {
                                if let (Some(call_id), Some(delta)) = (&event.call_id, &event.delta) {
                                    let name = event.name.clone();
                                    yield LlmEvent::ToolCallDelta {
                                        id: call_id.clone(),
                                        name,
                                        args_delta: delta.clone(),
                                    };
                                }
                            }
                            else if event.event_type == "response.function_call_arguments.done" {
                                if let (Some(call_id), Some(arguments)) = (&event.call_id, &event.arguments) {
                                    let name = event.name.clone().unwrap_or_default();
                                    let args: Box<RawValue> = RawValue::from_string(arguments.clone())
                                        .unwrap_or_else(|_| fallback_raw_value());

                                    let _ = assembler.on_tool_call_start(call_id.clone());
                                    let _ = assembler.on_tool_call_complete(
                                        call_id.clone(),
                                        name.clone(),
                                        args.clone(),
                                        None,
                                    );

                                    let args_value: Value = serde_json::from_str(args.get()).unwrap_or_default();
                                    yield LlmEvent::ToolCallComplete {
                                        id: call_id.clone(),
                                        name,
                                        args: args_value,
                                        meta: None,
                                    };
                                }
                            }
                            else if event.event_type == "response.reasoning_summary.done" || event.event_type == "response.reasoning.done" {
                                // Extract reasoning item details from the item field
                                if let Some(item) = &event.item {
                                    let reasoning_id = item.get("id")
                                        .and_then(|i| i.as_str())
                                        .unwrap_or_default();

                                    let mut summary_text = String::new();
                                    if let Some(summaries) = item.get("summary").and_then(|s| s.as_array()) {
                                        for summary in summaries {
                                            if let Some(text) = summary.get("text").and_then(|t| t.as_str()) {
                                                if !summary_text.is_empty() {
                                                    summary_text.push('\n');
                                                }
                                                summary_text.push_str(text);
                                            }
                                        }
                                    }

                                    let encrypted = item.get("encrypted_content")
                                        .and_then(|v| v.as_str())
                                        .map(|s| s.to_string());

                                    let meta = Some(Box::new(ProviderMeta::OpenAi {
                                        id: reasoning_id.to_string(),
                                        encrypted_content: encrypted,
                                    }));

                                    assembler.on_reasoning_start();
                                    if !summary_text.is_empty() {
                                        let _ = assembler.on_reasoning_delta(&summary_text);
                                    }
                                    assembler.on_reasoning_complete(meta.clone());

                                    yield LlmEvent::ReasoningComplete {
                                        text: summary_text,
                                        meta,
                                    };
                                }
                            }
                            else if event.event_type == "response.done" {
                                // Final done event with usage
                                if let Some(response_obj) = &event.response {
                                    if let Some(usage_obj) = response_obj.get("usage") {
                                        usage.input_tokens = usage_obj.get("input_tokens")
                                            .and_then(|v| v.as_u64())
                                            .unwrap_or(0);
                                        usage.output_tokens = usage_obj.get("output_tokens")
                                            .and_then(|v| v.as_u64())
                                            .unwrap_or(0);
                                        yield LlmEvent::UsageUpdate { usage: usage.clone() };
                                    }

                                    let stop_reason = match response_obj.get("status").and_then(|s| s.as_str()) {
                                        Some("completed") => {
                                            let has_tool_calls = response_obj.get("output")
                                                .and_then(|o| o.as_array())
                                                .map(|arr| arr.iter().any(|item| item.get("type").and_then(|t| t.as_str()) == Some("function_call")))
                                                .unwrap_or(false);
                                            if has_tool_calls {
                                                StopReason::ToolUse
                                            } else {
                                                StopReason::EndTurn
                                            }
                                        }
                                        Some("incomplete") => {
                                            match response_obj.get("incomplete_details").and_then(|d| d.get("reason")).and_then(|r| r.as_str()) {
                                                Some("max_output_tokens") => StopReason::MaxTokens,
                                                Some("content_filter") => StopReason::ContentFilter,
                                                _ => StopReason::EndTurn,
                                            }
                                        }
                                        Some("cancelled") => StopReason::Cancelled,
                                        _ => StopReason::EndTurn,
                                    };

                                    yield LlmEvent::Done {
                                        outcome: LlmDoneOutcome::Success { stop_reason },
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

/// SSE event from OpenAI Responses API streaming
#[derive(Debug, Deserialize)]
struct ResponsesStreamEvent {
    /// Event type (e.g., "response.output_text.delta", "response.done")
    #[serde(rename = "type")]
    event_type: String,
    /// Text delta for streaming events
    delta: Option<String>,
    /// Call ID for function call events
    call_id: Option<String>,
    /// Function name for function call events
    name: Option<String>,
    /// Complete arguments for function_call_arguments.done
    arguments: Option<String>,
    /// Item object for reasoning/function done events
    item: Option<Value>,
    /// Full response object for response.done and response.completed
    response: Option<Value>,
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
fn fallback_raw_value() -> Box<RawValue> {
    RawValue::from_string("{}".to_string()).expect("static JSON is valid")
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use meerkat_core::UserMessage;

    // =========================================================================
    // Responses API Request Format Tests
    // =========================================================================

    #[test]
    fn test_request_uses_responses_api_endpoint_format() {
        let client = OpenAiClient::new("test-key".to_string());
        let request = LlmRequest::new(
            "gpt-5.2",
            vec![Message::User(UserMessage {
                content: "Hello".to_string(),
            })],
        );

        let body = client.build_request_body(&request).expect("build request");

        // Should have "input" not "messages"
        assert!(body.get("input").is_some(), "should have 'input' field");
        assert!(
            body.get("messages").is_none(),
            "should NOT have 'messages' field"
        );

        // Should include reasoning.encrypted_content
        let include = body.get("include").expect("should have include");
        let include_arr = include.as_array().expect("include should be array");
        assert!(
            include_arr
                .iter()
                .any(|v| v.as_str() == Some("reasoning.encrypted_content")),
            "should include reasoning.encrypted_content"
        );
    }

    #[test]
    fn test_request_input_format_system_message() {
        let client = OpenAiClient::new("test-key".to_string());
        let request = LlmRequest::new(
            "gpt-5.2",
            vec![
                Message::System(meerkat_core::SystemMessage {
                    content: "You are helpful".to_string(),
                }),
                Message::User(UserMessage {
                    content: "Hello".to_string(),
                }),
            ],
        );

        let body = client.build_request_body(&request).expect("build request");
        let input = body["input"].as_array().expect("input should be array");

        // System message should have type: "message"
        assert_eq!(input[0]["type"], "message");
        assert_eq!(input[0]["role"], "system");
        assert_eq!(input[0]["content"], "You are helpful");

        // User message
        assert_eq!(input[1]["type"], "message");
        assert_eq!(input[1]["role"], "user");
        assert_eq!(input[1]["content"], "Hello");
    }

    #[test]
    fn test_request_input_format_tool_call() {
        let client = OpenAiClient::new("test-key".to_string());
        let tool_args = serde_json::json!({"location": "Tokyo"});
        let request = LlmRequest::new(
            "gpt-5.2",
            vec![
                Message::User(UserMessage {
                    content: "Weather?".to_string(),
                }),
                Message::Assistant(meerkat_core::AssistantMessage {
                    content: String::new(),
                    tool_calls: vec![meerkat_core::ToolCall::new(
                        "call_abc123".to_string(),
                        "get_weather".to_string(),
                        tool_args,
                    )],
                    stop_reason: StopReason::ToolUse,
                    usage: Usage::default(),
                }),
            ],
        );

        let body = client.build_request_body(&request).expect("build request");
        let input = body["input"].as_array().expect("input should be array");

        // Tool call should be type: "function_call"
        assert_eq!(input[1]["type"], "function_call");
        assert_eq!(input[1]["call_id"], "call_abc123");
        assert_eq!(input[1]["name"], "get_weather");
        // arguments should be JSON string
        let args_str = input[1]["arguments"]
            .as_str()
            .expect("arguments should be string");
        let parsed_args: Value = serde_json::from_str(args_str).expect("should be valid JSON");
        assert_eq!(parsed_args["location"], "Tokyo");
    }

    #[test]
    fn test_request_input_format_tool_result() {
        let client = OpenAiClient::new("test-key".to_string());
        let request = LlmRequest::new(
            "gpt-5.2",
            vec![
                Message::User(UserMessage {
                    content: "Weather?".to_string(),
                }),
                Message::ToolResults {
                    results: vec![meerkat_core::ToolResult::new(
                        "call_abc123".to_string(),
                        "Sunny, 25C".to_string(),
                        false,
                    )],
                },
            ],
        );

        let body = client.build_request_body(&request).expect("build request");
        let input = body["input"].as_array().expect("input should be array");

        // Tool result should be type: "function_call_output"
        assert_eq!(input[1]["type"], "function_call_output");
        assert_eq!(input[1]["call_id"], "call_abc123");
        assert_eq!(input[1]["output"], "Sunny, 25C");
    }

    #[test]
    fn test_tool_definition_format() {
        use meerkat_core::ToolDef;
        use std::sync::Arc;

        let client = OpenAiClient::new("test-key".to_string());
        let request = LlmRequest::new(
            "gpt-5.2",
            vec![Message::User(UserMessage {
                content: "test".to_string(),
            })],
        )
        .with_tools(vec![Arc::new(ToolDef {
            name: "get_weather".to_string(),
            description: "Get weather info".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "location": {"type": "string"}
                }
            }),
        })]);

        let body = client.build_request_body(&request).expect("build request");
        let tools = body["tools"].as_array().expect("tools should be array");

        // Responses API tool format: name at top level, not nested in "function"
        assert_eq!(tools[0]["type"], "function");
        assert_eq!(tools[0]["name"], "get_weather");
        assert_eq!(tools[0]["description"], "Get weather info");
        assert!(tools[0]["parameters"].is_object());
        // Should NOT have "function" wrapper
        assert!(tools[0].get("function").is_none());
    }

    #[test]
    fn test_request_includes_reasoning_config() {
        let client = OpenAiClient::new("test-key".to_string());
        let request = LlmRequest::new(
            "gpt-5.2",
            vec![Message::User(UserMessage {
                content: "test".to_string(),
            })],
        );

        let body = client.build_request_body(&request).expect("build request");

        // Should have reasoning config
        let reasoning = body.get("reasoning").expect("should have reasoning");
        assert_eq!(reasoning["effort"], "medium");
        assert_eq!(reasoning["summary"], "auto");
    }

    #[test]
    fn test_request_reasoning_effort_override() {
        let client = OpenAiClient::new("test-key".to_string());
        let request = LlmRequest::new(
            "gpt-5.2",
            vec![Message::User(UserMessage {
                content: "test".to_string(),
            })],
        )
        .with_provider_param("reasoning_effort", "high");

        let body = client.build_request_body(&request).expect("build request");

        assert_eq!(body["reasoning"]["effort"], "high");
    }

    // =========================================================================
    // BlockAssistant Message Tests
    // =========================================================================

    #[test]
    fn test_request_input_format_block_assistant_text() {
        use meerkat_core::BlockAssistantMessage;

        let client = OpenAiClient::new("test-key".to_string());
        let request = LlmRequest::new(
            "gpt-5.2",
            vec![
                Message::User(UserMessage {
                    content: "Hello".to_string(),
                }),
                Message::BlockAssistant(BlockAssistantMessage {
                    blocks: vec![AssistantBlock::Text {
                        text: "Hi there!".to_string(),
                        meta: None,
                    }],
                    stop_reason: StopReason::EndTurn,
                }),
            ],
        );

        let body = client.build_request_body(&request).expect("build request");
        let input = body["input"].as_array().expect("input should be array");

        assert_eq!(input[1]["type"], "message");
        assert_eq!(input[1]["role"], "assistant");
        assert_eq!(input[1]["content"], "Hi there!");
    }

    #[test]
    fn test_request_input_format_block_assistant_reasoning_with_output() {
        use meerkat_core::BlockAssistantMessage;

        let client = OpenAiClient::new("test-key".to_string());
        let request = LlmRequest::new(
            "gpt-5.2",
            vec![
                Message::User(UserMessage {
                    content: "Hello".to_string(),
                }),
                Message::BlockAssistant(BlockAssistantMessage {
                    blocks: vec![
                        AssistantBlock::Reasoning {
                            text: "Let me think about this".to_string(),
                            meta: Some(Box::new(ProviderMeta::OpenAi {
                                id: "rs_abc123".to_string(),
                                encrypted_content: Some("encrypted_data".to_string()),
                            })),
                        },
                        AssistantBlock::Text {
                            text: "Here is my answer".to_string(),
                            meta: None,
                        },
                    ],
                    stop_reason: StopReason::EndTurn,
                }),
            ],
        );

        let body = client.build_request_body(&request).expect("build request");
        let input = body["input"].as_array().expect("input should be array");

        // Reasoning followed by assistant message — both preserved
        assert_eq!(input.len(), 3);
        assert_eq!(input[1]["type"], "reasoning");
        assert_eq!(input[1]["id"], "rs_abc123");
        assert_eq!(input[1]["encrypted_content"], "encrypted_data");
        let summary = input[1]["summary"]
            .as_array()
            .expect("summary should be array");
        assert_eq!(summary[0]["text"], "Let me think about this");
        assert_eq!(input[2]["type"], "message");
        assert_eq!(input[2]["role"], "assistant");
    }

    #[test]
    fn test_request_input_format_block_assistant_tool_use() {
        use meerkat_core::BlockAssistantMessage;

        let client = OpenAiClient::new("test-key".to_string());
        let args = RawValue::from_string(r#"{"location":"Tokyo"}"#.to_string()).unwrap();
        let request = LlmRequest::new(
            "gpt-5.2",
            vec![
                Message::User(UserMessage {
                    content: "Weather?".to_string(),
                }),
                Message::BlockAssistant(BlockAssistantMessage {
                    blocks: vec![AssistantBlock::ToolUse {
                        id: "call_xyz".to_string(),
                        name: "get_weather".to_string(),
                        args,
                        meta: None,
                    }],
                    stop_reason: StopReason::ToolUse,
                }),
            ],
        );

        let body = client.build_request_body(&request).expect("build request");
        let input = body["input"].as_array().expect("input should be array");

        assert_eq!(input[1]["type"], "function_call");
        assert_eq!(input[1]["call_id"], "call_xyz");
        assert_eq!(input[1]["name"], "get_weather");
        // arguments should be the raw JSON string
        let args_str = input[1]["arguments"]
            .as_str()
            .expect("arguments should be string");
        assert_eq!(args_str, r#"{"location":"Tokyo"}"#);
    }

    // =========================================================================
    // Provider Parameters Tests
    // =========================================================================

    #[test]
    fn test_request_includes_seed_from_provider_params() {
        let client = OpenAiClient::new("test-key".to_string());
        let request = LlmRequest::new(
            "gpt-5.2",
            vec![Message::User(UserMessage {
                content: "test".to_string(),
            })],
        )
        .with_provider_param("seed", 12345);

        let body = client.build_request_body(&request).expect("build request");

        assert_eq!(body["seed"], 12345);
    }

    #[test]
    fn test_request_includes_frequency_penalty_from_provider_params() {
        let client = OpenAiClient::new("test-key".to_string());
        let request = LlmRequest::new(
            "gpt-5.2",
            vec![Message::User(UserMessage {
                content: "test".to_string(),
            })],
        )
        .with_provider_param("frequency_penalty", 0.5);

        let body = client.build_request_body(&request).expect("build request");

        assert_eq!(body["frequency_penalty"], 0.5);
    }

    #[test]
    fn test_request_includes_presence_penalty_from_provider_params() {
        let client = OpenAiClient::new("test-key".to_string());
        let request = LlmRequest::new(
            "gpt-5.2",
            vec![Message::User(UserMessage {
                content: "test".to_string(),
            })],
        )
        .with_provider_param("presence_penalty", 0.8);

        let body = client.build_request_body(&request).expect("build request");

        assert_eq!(body["presence_penalty"], 0.8);
    }

    #[test]
    fn test_unknown_provider_params_are_ignored() {
        let client = OpenAiClient::new("test-key".to_string());
        let request = LlmRequest::new(
            "gpt-5.2",
            vec![Message::User(UserMessage {
                content: "test".to_string(),
            })],
        )
        .with_provider_param("unknown_param", "some_value")
        .with_provider_param("another_unknown", 123)
        .with_provider_param("seed", 42);

        let body = client.build_request_body(&request).expect("build request");

        assert!(body.get("unknown_param").is_none());
        assert!(body.get("another_unknown").is_none());
        assert_eq!(body["seed"], 42);
    }

    #[test]
    fn test_multiple_provider_params_combined() {
        let client = OpenAiClient::new("test-key".to_string());
        let request = LlmRequest::new(
            "gpt-5.2",
            vec![Message::User(UserMessage {
                content: "test".to_string(),
            })],
        )
        .with_provider_param("reasoning_effort", "high")
        .with_provider_param("seed", 999)
        .with_provider_param("frequency_penalty", 0.3)
        .with_provider_param("presence_penalty", 0.4);

        let body = client.build_request_body(&request).expect("build request");

        assert_eq!(body["reasoning"]["effort"], "high");
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

        let body = client.build_request_body(&request).expect("build request");

        let input = body["input"].as_array().ok_or("not array")?;
        let tool_call = input
            .iter()
            .find(|item| item["type"] == "function_call")
            .ok_or("no tool call")?;
        let arguments = tool_call["arguments"].as_str().ok_or("not string")?;

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

    // =========================================================================
    // Structured Output Tests
    // =========================================================================

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

        let body = client.build_request_body(&request).expect("build request");

        // Responses API uses "text.format" for structured output
        let text = body.get("text").expect("should have text");
        let format = text.get("format").expect("should have format");
        assert_eq!(format["type"], "json_schema");
        assert_eq!(format["name"], "person");
        assert_eq!(format["strict"], true);
        assert!(format["schema"].is_object());
    }

    #[test]
    fn test_build_request_body_with_structured_output_defaults() {
        let client = OpenAiClient::new("test-key".to_string());

        let schema = serde_json::json!({"type": "object"});

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

        let body = client.build_request_body(&request).expect("build request");

        let format = &body["text"]["format"];
        assert_eq!(format["name"], "output"); // default name
        assert_eq!(format["strict"], false); // default strict
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

        let body = client.build_request_body(&request).expect("build request");

        // text field should not be present
        assert!(
            body.get("text").is_none(),
            "text should not be present without structured_output"
        );
    }

    // =========================================================================
    // SSE Parsing Tests
    // =========================================================================

    #[test]
    fn test_parse_responses_sse_line_text_delta() {
        let line = r#"data: {"type":"response.output_text.delta","delta":"Hello"}"#;
        let event = OpenAiClient::parse_responses_sse_line(line);
        assert!(event.is_some());
        let event = event.unwrap();
        assert_eq!(event.event_type, "response.output_text.delta");
        assert_eq!(event.delta, Some("Hello".to_string()));
    }

    #[test]
    fn test_parse_responses_sse_line_reasoning_delta() {
        let line =
            r#"data: {"type":"response.reasoning_summary_text.delta","delta":"thinking..."}"#;
        let event = OpenAiClient::parse_responses_sse_line(line);
        assert!(event.is_some());
        let event = event.unwrap();
        assert_eq!(event.event_type, "response.reasoning_summary_text.delta");
        assert_eq!(event.delta, Some("thinking...".to_string()));
    }

    #[test]
    fn test_parse_responses_sse_line_function_call_done() {
        let line = r#"data: {"type":"response.function_call_arguments.done","call_id":"call_123","name":"get_weather","arguments":"{\"location\":\"Tokyo\"}"}"#;
        let event = OpenAiClient::parse_responses_sse_line(line);
        assert!(event.is_some());
        let event = event.unwrap();
        assert_eq!(event.event_type, "response.function_call_arguments.done");
        assert_eq!(event.call_id, Some("call_123".to_string()));
        assert_eq!(event.name, Some("get_weather".to_string()));
        assert_eq!(event.arguments, Some(r#"{"location":"Tokyo"}"#.to_string()));
    }

    #[test]
    fn test_parse_responses_sse_line_response_done() {
        let line = r#"data: {"type":"response.done","response":{"status":"completed","output":[{"type":"message","content":[{"type":"output_text","text":"Hi"}]}],"usage":{"input_tokens":10,"output_tokens":5}}}"#;
        let event = OpenAiClient::parse_responses_sse_line(line);
        assert!(event.is_some());
        let event = event.unwrap();
        assert_eq!(event.event_type, "response.done");
        assert!(event.response.is_some());
        let response = event.response.unwrap();
        assert_eq!(response["status"], "completed");
    }

    #[test]
    fn test_parse_responses_sse_line_done_marker() {
        let line = "data: [DONE]";
        let event = OpenAiClient::parse_responses_sse_line(line);
        assert!(event.is_none());
    }

    #[test]
    fn test_parse_responses_sse_line_non_data_line() {
        let line = "event: message";
        let event = OpenAiClient::parse_responses_sse_line(line);
        assert!(event.is_none());
    }

    #[test]
    fn test_parse_responses_sse_line_reasoning_item_with_encrypted() {
        let line = r#"data: {"type":"response.reasoning.done","item":{"id":"rs_abc123","summary":[{"type":"summary_text","text":"I need to think"}],"encrypted_content":"enc_xyz"}}"#;
        let event = OpenAiClient::parse_responses_sse_line(line);
        assert!(event.is_some());
        let event = event.unwrap();
        assert_eq!(event.event_type, "response.reasoning.done");
        let item = event.item.expect("should have item");
        assert_eq!(item["id"], "rs_abc123");
        assert_eq!(item["encrypted_content"], "enc_xyz");
        let summary = item["summary"].as_array().expect("summary array");
        assert_eq!(summary[0]["text"], "I need to think");
    }

    // =========================================================================
    // Response Parsing Tests
    // =========================================================================

    #[test]
    fn test_response_completed_parses_message_content() {
        // Test that response.completed event correctly parses message content array
        let response_json = serde_json::json!({
            "status": "completed",
            "output": [
                {
                    "type": "message",
                    "content": [
                        {"type": "output_text", "text": "Hello"},
                        {"type": "output_text", "text": " World"}
                    ]
                }
            ],
            "usage": {"input_tokens": 10, "output_tokens": 5}
        });

        // Verify structure matches what we expect to parse
        let output = response_json["output"].as_array().expect("output array");
        assert_eq!(output[0]["type"], "message");
        let content = output[0]["content"].as_array().expect("content array");
        assert_eq!(content[0]["type"], "output_text");
        assert_eq!(content[0]["text"], "Hello");
    }

    #[test]
    fn test_response_completed_parses_reasoning_item() {
        let response_json = serde_json::json!({
            "status": "completed",
            "output": [
                {
                    "type": "reasoning",
                    "id": "rs_abc123",
                    "summary": [
                        {"type": "summary_text", "text": "Let me think about this"}
                    ],
                    "encrypted_content": "encrypted_stuff_here"
                }
            ],
            "usage": {"input_tokens": 10, "output_tokens": 5}
        });

        let output = response_json["output"].as_array().expect("output array");
        let reasoning = &output[0];
        assert_eq!(reasoning["type"], "reasoning");
        assert_eq!(reasoning["id"], "rs_abc123");
        assert_eq!(reasoning["encrypted_content"], "encrypted_stuff_here");
        let summary = reasoning["summary"].as_array().expect("summary array");
        assert_eq!(summary[0]["text"], "Let me think about this");
    }

    #[test]
    fn test_response_completed_parses_function_call() {
        let response_json = serde_json::json!({
            "status": "completed",
            "output": [
                {
                    "type": "function_call",
                    "call_id": "call_xyz789",
                    "name": "get_weather",
                    "arguments": "{\"location\":\"Tokyo\"}"
                }
            ],
            "usage": {"input_tokens": 10, "output_tokens": 5}
        });

        let output = response_json["output"].as_array().expect("output array");
        let func_call = &output[0];
        assert_eq!(func_call["type"], "function_call");
        assert_eq!(func_call["call_id"], "call_xyz789");
        assert_eq!(func_call["name"], "get_weather");
        // arguments is a JSON STRING
        let args_str = func_call["arguments"].as_str().expect("string");
        let args: Value = serde_json::from_str(args_str).expect("valid json");
        assert_eq!(args["location"], "Tokyo");
    }

    // =========================================================================
    // Orphaned Reasoning Stripping Tests
    // =========================================================================

    #[test]
    fn test_orphaned_reasoning_at_end_is_stripped() {
        use meerkat_core::BlockAssistantMessage;

        let client = OpenAiClient::new("test-key".to_string());
        let request = LlmRequest::new(
            "gpt-5.2",
            vec![
                Message::User(UserMessage {
                    content: "Hello".to_string(),
                }),
                // Reasoning-only response (e.g., stream interrupted after reasoning)
                Message::BlockAssistant(BlockAssistantMessage {
                    blocks: vec![AssistantBlock::Reasoning {
                        text: "Let me think".to_string(),
                        meta: Some(Box::new(ProviderMeta::OpenAi {
                            id: "rs_orphan".to_string(),
                            encrypted_content: None,
                        })),
                    }],
                    stop_reason: StopReason::EndTurn,
                }),
            ],
        );

        let body = client.build_request_body(&request).expect("build request");
        let input = body["input"].as_array().expect("input should be array");

        // Orphaned reasoning should be stripped, leaving only the user message
        assert_eq!(input.len(), 1);
        assert_eq!(input[0]["type"], "message");
        assert_eq!(input[0]["role"], "user");
    }

    #[test]
    fn test_orphaned_reasoning_before_user_message_is_stripped() {
        use meerkat_core::BlockAssistantMessage;

        let client = OpenAiClient::new("test-key".to_string());
        let request = LlmRequest::new(
            "gpt-5.2",
            vec![
                Message::User(UserMessage {
                    content: "First question".to_string(),
                }),
                // Reasoning without output, followed by next user turn
                Message::BlockAssistant(BlockAssistantMessage {
                    blocks: vec![AssistantBlock::Reasoning {
                        text: "Thinking...".to_string(),
                        meta: Some(Box::new(ProviderMeta::OpenAi {
                            id: "rs_mid".to_string(),
                            encrypted_content: None,
                        })),
                    }],
                    stop_reason: StopReason::EndTurn,
                }),
                Message::User(UserMessage {
                    content: "Second question".to_string(),
                }),
            ],
        );

        let body = client.build_request_body(&request).expect("build request");
        let input = body["input"].as_array().expect("input should be array");

        // Reasoning followed by user message (not assistant output) should be stripped
        assert_eq!(input.len(), 2);
        assert_eq!(input[0]["role"], "user");
        assert_eq!(input[0]["content"], "First question");
        assert_eq!(input[1]["role"], "user");
        assert_eq!(input[1]["content"], "Second question");
    }

    #[test]
    fn test_orphaned_reasoning_before_tool_result_is_stripped() {
        use meerkat_core::BlockAssistantMessage;

        let client = OpenAiClient::new("test-key".to_string());
        let request = LlmRequest::new(
            "gpt-5.2",
            vec![
                Message::User(UserMessage {
                    content: "Hello".to_string(),
                }),
                // Reasoning-only at end of one assistant message
                Message::BlockAssistant(BlockAssistantMessage {
                    blocks: vec![AssistantBlock::Reasoning {
                        text: "Thinking...".to_string(),
                        meta: Some(Box::new(ProviderMeta::OpenAi {
                            id: "rs_before_tool".to_string(),
                            encrypted_content: None,
                        })),
                    }],
                    stop_reason: StopReason::EndTurn,
                }),
                // Next message is tool results — not a valid follower for reasoning
                Message::ToolResults {
                    results: vec![meerkat_core::ToolResult::new(
                        "call_123".to_string(),
                        "result".to_string(),
                        false,
                    )],
                },
            ],
        );

        let body = client.build_request_body(&request).expect("build request");
        let input = body["input"].as_array().expect("input should be array");

        // Reasoning should be stripped; user message + tool result remain
        assert_eq!(input.len(), 2);
        assert_eq!(input[0]["type"], "message");
        assert_eq!(input[1]["type"], "function_call_output");
    }

    #[test]
    fn test_reasoning_followed_by_function_call_is_preserved() {
        use meerkat_core::BlockAssistantMessage;

        let client = OpenAiClient::new("test-key".to_string());
        let args = RawValue::from_string(r#"{"q":"test"}"#.to_string()).unwrap();
        let request = LlmRequest::new(
            "gpt-5.2",
            vec![
                Message::User(UserMessage {
                    content: "Hello".to_string(),
                }),
                Message::BlockAssistant(BlockAssistantMessage {
                    blocks: vec![
                        AssistantBlock::Reasoning {
                            text: "I should search".to_string(),
                            meta: Some(Box::new(ProviderMeta::OpenAi {
                                id: "rs_valid".to_string(),
                                encrypted_content: None,
                            })),
                        },
                        AssistantBlock::ToolUse {
                            id: "call_1".to_string(),
                            name: "search".to_string(),
                            args,
                            meta: None,
                        },
                    ],
                    stop_reason: StopReason::ToolUse,
                }),
            ],
        );

        let body = client.build_request_body(&request).expect("build request");
        let input = body["input"].as_array().expect("input should be array");

        // Reasoning + function_call is a valid pair — both preserved
        assert_eq!(input.len(), 3);
        assert_eq!(input[1]["type"], "reasoning");
        assert_eq!(input[2]["type"], "function_call");
    }

    #[test]
    fn test_non_openai_reasoning_blocks_are_skipped() {
        use meerkat_core::BlockAssistantMessage;

        let client = OpenAiClient::new("test-key".to_string());
        let request = LlmRequest::new(
            "gpt-5.2",
            vec![
                Message::User(UserMessage {
                    content: "Hello".to_string(),
                }),
                Message::BlockAssistant(BlockAssistantMessage {
                    blocks: vec![
                        AssistantBlock::Reasoning {
                            text: "Anthropic thinking".to_string(),
                            meta: Some(Box::new(ProviderMeta::Anthropic {
                                signature: "sig_abc".to_string(),
                            })),
                        },
                        AssistantBlock::Text {
                            text: "Answer".to_string(),
                            meta: None,
                        },
                    ],
                    stop_reason: StopReason::EndTurn,
                }),
            ],
        );

        let body = client.build_request_body(&request).expect("build request");
        let input = body["input"].as_array().expect("input should be array");

        // Non-OpenAI reasoning is not serialized at all, only text remains
        assert_eq!(input.len(), 2);
        assert_eq!(input[0]["type"], "message");
        assert_eq!(input[0]["role"], "user");
        assert_eq!(input[1]["type"], "message");
        assert_eq!(input[1]["role"], "assistant");
    }

    // =========================================================================
    // Response Status Tests
    // =========================================================================

    #[test]
    fn test_stop_reason_from_response_status() {
        // completed with tool calls -> ToolUse
        let response_tool = serde_json::json!({
            "status": "completed",
            "output": [{"type": "function_call", "call_id": "1", "name": "x", "arguments": "{}"}]
        });
        let has_tools = response_tool["output"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .any(|item| item.get("type").and_then(|t| t.as_str()) == Some("function_call"))
            })
            .unwrap_or(false);
        assert!(has_tools);

        // completed without tool calls -> EndTurn
        let response_text = serde_json::json!({
            "status": "completed",
            "output": [{"type": "message", "content": [{"type": "output_text", "text": "Hi"}]}]
        });
        let has_tools = response_text["output"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .any(|item| item.get("type").and_then(|t| t.as_str()) == Some("function_call"))
            })
            .unwrap_or(false);
        assert!(!has_tools);
    }
}
