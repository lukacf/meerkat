//! OpenAI API client - Responses API
//!
//! Implements the LlmClient trait for OpenAI's Responses API.
//! This client uses the /v1/responses endpoint which supports reasoning items.

use crate::BlockAssembler;
use crate::error::LlmError;
use crate::types::{LlmClient, LlmDoneOutcome, LlmEvent, LlmRequest, LlmStream};
use async_trait::async_trait;
#[cfg(target_os = "espidf")]
use embedded_svc::http::Method;
#[cfg(target_os = "espidf")]
use embedded_svc::http::client::{Client as EmbeddedHttpClient, Response as EmbeddedResponse};
#[cfg(target_os = "espidf")]
use embedded_svc::io::Write as _;
#[cfg(target_os = "espidf")]
use esp_idf_svc::http::client::{Configuration as EspHttpConfiguration, EspHttpConnection};
#[cfg(target_os = "espidf")]
use esp_idf_svc::sys;
#[cfg(target_os = "espidf")]
use esp_idf_svc::tls::X509;
use futures::{Stream, StreamExt};
use meerkat_core::schema::{CompiledSchema, SchemaError};
use meerkat_core::{
    AssistantBlock, ContentBlock, ImageData, Message, OutputSchema, ProviderMeta, StopReason, Usage,
};
use serde::Deserialize;
use serde_json::Value;
use serde_json::value::RawValue;
use std::collections::HashSet;
#[cfg(target_os = "espidf")]
use std::sync::atomic::{AtomicUsize, Ordering};
#[cfg(target_os = "espidf")]
use std::time::Duration;

#[cfg(target_os = "espidf")]
static OPENAI_ESP_REQUEST_SEQ: AtomicUsize = AtomicUsize::new(1);
#[cfg(target_os = "espidf")]
const OPENAI_WE1_PEM: &[u8] = concat!(include_str!("openai_we1.pem"), "\0").as_bytes();
#[cfg(target_os = "espidf")]
const OPENAI_ESP_MAX_ATTEMPTS: usize = 3;

/// Client for OpenAI Responses API
pub struct OpenAiClient {
    api_key: String,
    base_url: String,
    #[cfg(not(target_os = "espidf"))]
    http: reqwest::Client,
}

impl OpenAiClient {
    fn model_supports_temperature(model: &str) -> bool {
        meerkat_models::profile::openai::supports_temperature(model)
    }

    fn model_supports_reasoning_payload(model: &str) -> bool {
        meerkat_models::profile::openai::supports_reasoning(model)
    }

    /// Create a new OpenAI client with the given API key
    pub fn new(api_key: String) -> Self {
        Self::new_with_base_url(api_key, "https://api.openai.com".to_string())
    }

    /// Create a new OpenAI client with an explicit base URL
    pub fn new_with_base_url(api_key: String, base_url: String) -> Self {
        #[cfg(not(target_os = "espidf"))]
        let http =
            crate::http::build_http_client_for_base_url(reqwest::Client::builder(), &base_url)
                .unwrap_or_else(|_| reqwest::Client::new());
        Self {
            api_key,
            base_url,
            #[cfg(not(target_os = "espidf"))]
            http,
        }
    }

    /// Set custom base URL
    pub fn with_base_url(mut self, url: String) -> Self {
        #[cfg(not(target_os = "espidf"))]
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
        let reasoning_enabled = Self::model_supports_reasoning_payload(&request.model);
        let stream_enabled = Self::request_stream_enabled(request);

        let mut body = serde_json::json!({
            "model": request.model,
            "input": input,
            "max_output_tokens": request.max_tokens,
            "stream": stream_enabled,
        });

        if reasoning_enabled {
            // Request encrypted_content for stateless replay.
            body["include"] = serde_json::json!(["reasoning.encrypted_content"]);
            // Enable reasoning with default effort.
            body["reasoning"] = serde_json::json!({
                "effort": "medium",
                "summary": "auto"
            });
        }

        if Self::model_supports_temperature(&request.model)
            && let Some(temp) = request.temperature
            && let Some(num) = serde_json::Number::from_f64(temp as f64)
        {
            body["temperature"] = Value::Number(num);
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
            if reasoning_enabled && let Some(reasoning_effort) = params.get("reasoning_effort") {
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

    fn request_stream_enabled(request: &LlmRequest) -> bool {
        request
            .provider_params
            .as_ref()
            .and_then(|params| params.get("stream"))
            .and_then(Value::as_bool)
            .unwrap_or(true)
    }

    /// Convert messages to Responses API input format.
    ///
    /// Note: we intentionally do not replay prior `reasoning` items.
    /// OpenAI enforces strict adjacency invariants for reasoning replay, and
    /// violating them causes hard request failures. Replaying only user/assistant
    /// messages and tool items is robust across retries and tool-call turns.
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
                Message::SystemNotice(notice) => {
                    items.push(serde_json::json!({
                        "type": "message",
                        "role": "user",
                        "content": notice.rendered_text()
                    }));
                }
                Message::User(u) => {
                    if meerkat_core::has_images(&u.content) {
                        let content_array: Vec<Value> = u
                            .content
                            .iter()
                            .map(|block| match block {
                                ContentBlock::Text { text } => serde_json::json!({
                                    "type": "input_text",
                                    "text": text
                                }),
                                ContentBlock::Image { media_type, data } => match data {
                                    ImageData::Inline { data } => serde_json::json!({
                                        "type": "input_image",
                                        "image_url": format!("data:{media_type};base64,{data}")
                                    }),
                                    ImageData::Blob { .. } => serde_json::json!({
                                        "type": "input_text",
                                        "text": block.text_projection()
                                    }),
                                },
                                _ => serde_json::json!({
                                    "type": "input_text",
                                    "text": block.text_projection()
                                }),
                            })
                            .collect();
                        items.push(serde_json::json!({
                            "type": "message",
                            "role": "user",
                            "content": content_array
                        }));
                    } else {
                        items.push(serde_json::json!({
                            "type": "message",
                            "role": "user",
                            "content": u.text_content()
                        }));
                    }
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
                            AssistantBlock::ToolUse { id, name, args, .. } => {
                                items.push(serde_json::json!({
                                    "type": "function_call",
                                    "call_id": id,
                                    "name": name,
                                    "arguments": args.get()  // Already JSON string
                                }));
                            }
                            // Reasoning replay can violate Responses API adjacency
                            // constraints and hard-fail requests; skip it and any
                            // unknown future variants.
                            _ => {}
                        }
                    }
                }
                Message::ToolResults { results } => {
                    // OpenAI function_call_output only accepts strings; images
                    // degrade to text projection via text_content().
                    for r in results {
                        items.push(serde_json::json!({
                            "type": "function_call_output",
                            "call_id": r.tool_use_id,
                            "output": r.text_content()
                        }));
                    }
                }
            }
        }

        items
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

#[cfg(all(not(target_arch = "wasm32"), not(target_os = "espidf")))]
fn stream_openai_events_from_bytes<'a, S>(byte_stream: S) -> LlmStream<'a>
where
    S: Stream<Item = Result<Vec<u8>, LlmError>> + Send + 'a,
{
    let inner: LlmStream<'a> = Box::pin(async_stream::try_stream! {
        futures::pin_mut!(byte_stream);
        let mut buffer = String::with_capacity(512);
        let mut state = OpenAiStreamState::default();

        while let Some(chunk) = byte_stream.next().await {
            let chunk = chunk?;
            buffer.push_str(&String::from_utf8_lossy(&chunk));

            while let Some(newline_pos) = buffer.find('\n') {
                let line = buffer[..newline_pos].trim().to_string();
                buffer.drain(..=newline_pos);

                for event in process_openai_stream_line(&line, &mut state)? {
                    yield event;
                }
            }
        }
    });

    crate::streaming::ensure_terminal_done(inner)
}

#[cfg(any(target_arch = "wasm32", target_os = "espidf"))]
fn stream_openai_events_from_bytes<'a, S>(byte_stream: S) -> LlmStream<'a>
where
    S: Stream<Item = Result<Vec<u8>, LlmError>> + 'a,
{
    let inner: LlmStream<'a> = Box::pin(async_stream::try_stream! {
        futures::pin_mut!(byte_stream);
        let mut buffer = String::with_capacity(512);
        let mut state = OpenAiStreamState::default();

        while let Some(chunk) = byte_stream.next().await {
            let chunk = chunk?;
            buffer.push_str(&String::from_utf8_lossy(&chunk));

            while let Some(newline_pos) = buffer.find('\n') {
                let line = buffer[..newline_pos].trim().to_string();
                buffer.drain(..=newline_pos);

                for event in process_openai_stream_line(&line, &mut state)? {
                    yield event;
                }
            }
        }
    });

    crate::streaming::ensure_terminal_done(inner)
}

#[derive(Default)]
struct OpenAiStreamState {
    assembler: BlockAssembler,
    usage: Usage,
    saw_stream_text_delta: bool,
    streamed_tool_ids: HashSet<String>,
    streamed_reasoning_ids: HashSet<String>,
    done_emitted: bool,
}

fn process_openai_stream_line(
    line: &str,
    state: &mut OpenAiStreamState,
) -> Result<Vec<LlmEvent>, LlmError> {
    if line.is_empty() || line.starts_with(':') {
        return Ok(Vec::new());
    }

    let Some(event) = OpenAiClient::parse_responses_sse_line(line) else {
        return Ok(Vec::new());
    };

    let mut emitted = Vec::new();
    match event.event_type.as_str() {
        "response.completed" => {
            if state.done_emitted {
                return Ok(emitted);
            }
            if let Some(response_obj) = &event.response {
                collect_response_completed_events(response_obj, state, &mut emitted);
            }
        }
        "response.output_text.delta" => {
            if let Some(delta) = event.delta {
                state.saw_stream_text_delta = true;
                state.assembler.on_text_delta(&delta, None);
                emitted.push(LlmEvent::TextDelta { delta, meta: None });
            }
        }
        "response.reasoning_summary_text.delta" => {
            if let Some(delta) = event.delta {
                emitted.push(LlmEvent::ReasoningDelta { delta });
            }
        }
        "response.function_call_arguments.delta" => {
            if let (Some(call_id), Some(delta)) = (event.call_id, event.delta) {
                emitted.push(LlmEvent::ToolCallDelta {
                    id: call_id,
                    name: event.name,
                    args_delta: delta,
                });
            }
        }
        "response.function_call_arguments.done" => {
            if let (Some(call_id), Some(arguments)) = (event.call_id, event.arguments) {
                let name = event.name.unwrap_or_default();
                let args: Box<RawValue> =
                    RawValue::from_string(arguments).unwrap_or_else(|_| fallback_raw_value());

                let _ = state.assembler.on_tool_call_start(call_id.clone());
                let _ = state.assembler.on_tool_call_complete(
                    call_id.clone(),
                    name.clone(),
                    args.clone(),
                    None,
                );

                let args_value: Value = serde_json::from_str(args.get()).unwrap_or_default();
                state.streamed_tool_ids.insert(call_id.clone());
                emitted.push(LlmEvent::ToolCallComplete {
                    id: call_id,
                    name,
                    args: args_value,
                    meta: None,
                });
            }
        }
        "response.reasoning_summary.done" | "response.reasoning.done" => {
            if let Some(item) = &event.item {
                collect_reasoning_complete(item, state, &mut emitted);
            }
        }
        "response.done" => {
            if let Some(response_obj) = &event.response {
                if let Some(usage_event) =
                    update_usage_and_make_event(response_obj, &mut state.usage)
                {
                    emitted.push(usage_event);
                }

                if !state.done_emitted {
                    state.done_emitted = true;
                    emitted.push(LlmEvent::Done {
                        outcome: LlmDoneOutcome::Success {
                            stop_reason: stop_reason_from_response(response_obj),
                        },
                    });
                }
            }
        }
        "error" => {
            let (error_code, error_msg) = stream_error_details(event.error.as_ref());
            tracing::error!(
                code = error_code,
                message = error_msg,
                "OpenAI streaming error"
            );

            state.done_emitted = true;
            emitted.push(LlmEvent::Done {
                outcome: LlmDoneOutcome::Error {
                    error: error_from_stream_error(error_code, error_msg),
                },
            });
        }
        _ => {}
    }

    Ok(emitted)
}

fn collect_response_completed_events(
    response_obj: &Value,
    state: &mut OpenAiStreamState,
    emitted: &mut Vec<LlmEvent>,
) {
    if let Some(output) = response_obj.get("output").and_then(Value::as_array) {
        for item in output {
            match item.get("type").and_then(Value::as_str) {
                Some("message") => collect_message_output(item, state, emitted),
                Some("reasoning") => collect_reasoning_complete(item, state, emitted),
                Some("function_call") => collect_function_call_complete(item, state, emitted),
                _ => {}
            }
        }
    }

    if let Some(usage_event) = update_usage_and_make_event(response_obj, &mut state.usage) {
        emitted.push(usage_event);
    }

    state.done_emitted = true;
    emitted.push(LlmEvent::Done {
        outcome: LlmDoneOutcome::Success {
            stop_reason: stop_reason_from_response(response_obj),
        },
    });
}

fn collect_message_output(
    item: &Value,
    state: &mut OpenAiStreamState,
    emitted: &mut Vec<LlmEvent>,
) {
    let Some(content_parts) = item.get("content").and_then(Value::as_array) else {
        return;
    };

    for part in content_parts {
        match part.get("type").and_then(Value::as_str) {
            Some("output_text") => {
                if let Some(text) = part.get("text").and_then(Value::as_str)
                    && !state.saw_stream_text_delta
                {
                    state.assembler.on_text_delta(text, None);
                    emitted.push(LlmEvent::TextDelta {
                        delta: text.to_string(),
                        meta: None,
                    });
                }
            }
            Some("refusal") => {
                if let Some(refusal) = part.get("refusal").and_then(Value::as_str)
                    && !state.saw_stream_text_delta
                {
                    state.assembler.on_text_delta(refusal, None);
                    emitted.push(LlmEvent::TextDelta {
                        delta: refusal.to_string(),
                        meta: None,
                    });
                }
            }
            _ => {}
        }
    }
}

fn collect_reasoning_complete(
    item: &Value,
    state: &mut OpenAiStreamState,
    emitted: &mut Vec<LlmEvent>,
) {
    let Some(reasoning_id) = item.get("id").and_then(Value::as_str) else {
        tracing::warn!("reasoning item missing id, skipping");
        return;
    };

    if state.streamed_reasoning_ids.contains(reasoning_id) {
        return;
    }

    let mut summary_text = String::new();
    if let Some(summaries) = item.get("summary").and_then(Value::as_array) {
        for summary in summaries {
            if let Some(text) = summary.get("text").and_then(Value::as_str) {
                if !summary_text.is_empty() {
                    summary_text.push('\n');
                }
                summary_text.push_str(text);
            }
        }
    }

    let encrypted = item
        .get("encrypted_content")
        .and_then(Value::as_str)
        .map(str::to_string);

    let meta = Some(Box::new(ProviderMeta::OpenAi {
        id: reasoning_id.to_string(),
        encrypted_content: encrypted,
    }));

    state.assembler.on_reasoning_start();
    if !summary_text.is_empty() {
        let _ = state.assembler.on_reasoning_delta(&summary_text);
    }
    state.assembler.on_reasoning_complete(meta.clone());

    state
        .streamed_reasoning_ids
        .insert(reasoning_id.to_string());
    emitted.push(LlmEvent::ReasoningComplete {
        text: summary_text,
        meta,
    });
}

fn collect_function_call_complete(
    item: &Value,
    state: &mut OpenAiStreamState,
    emitted: &mut Vec<LlmEvent>,
) {
    let Some(call_id) = item.get("call_id").and_then(Value::as_str) else {
        tracing::warn!("function_call missing call_id");
        return;
    };

    if state.streamed_tool_ids.contains(call_id) {
        return;
    }

    let Some(name) = item.get("name").and_then(Value::as_str) else {
        tracing::warn!(call_id, "function_call missing name");
        return;
    };

    let args: Box<RawValue> = match item.get("arguments").and_then(Value::as_str) {
        Some(args_str) => match RawValue::from_string(args_str.to_string()) {
            Ok(raw) => raw,
            Err(error) => {
                tracing::error!(call_id, "invalid args JSON, skipping: {error}");
                return;
            }
        },
        None => fallback_raw_value(),
    };

    let _ = state.assembler.on_tool_call_start(call_id.to_string());
    let _ = state.assembler.on_tool_call_complete(
        call_id.to_string(),
        name.to_string(),
        args.clone(),
        None,
    );

    let args_value: Value = serde_json::from_str(args.get()).unwrap_or_default();
    emitted.push(LlmEvent::ToolCallComplete {
        id: call_id.to_string(),
        name: name.to_string(),
        args: args_value,
        meta: None,
    });
}

fn update_usage_and_make_event(response_obj: &Value, usage: &mut Usage) -> Option<LlmEvent> {
    let usage_obj = response_obj.get("usage")?;
    usage.input_tokens = usage_obj
        .get("input_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    usage.output_tokens = usage_obj
        .get("output_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    Some(LlmEvent::UsageUpdate {
        usage: usage.clone(),
    })
}

fn stop_reason_from_response(response_obj: &Value) -> StopReason {
    match response_obj.get("status").and_then(Value::as_str) {
        Some("completed") => {
            let has_tool_calls = response_obj
                .get("output")
                .and_then(Value::as_array)
                .is_some_and(|arr| {
                    arr.iter().any(|item| {
                        item.get("type").and_then(Value::as_str) == Some("function_call")
                    })
                });
            if has_tool_calls {
                StopReason::ToolUse
            } else {
                StopReason::EndTurn
            }
        }
        Some("incomplete") => match response_obj
            .get("incomplete_details")
            .and_then(|d| d.get("reason"))
            .and_then(Value::as_str)
        {
            Some("max_output_tokens") => StopReason::MaxTokens,
            Some("content_filter") => StopReason::ContentFilter,
            _ => StopReason::EndTurn,
        },
        Some("cancelled") => StopReason::Cancelled,
        _ => StopReason::EndTurn,
    }
}

fn stream_error_details(error: Option<&Value>) -> (&str, &str) {
    let error_msg = error
        .and_then(|e| e.get("message"))
        .and_then(Value::as_str)
        .unwrap_or("unknown streaming error");
    let error_code = error
        .and_then(|e| e.get("code"))
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    (error_code, error_msg)
}

fn error_from_stream_error(error_code: &str, error_msg: &str) -> LlmError {
    match error_code {
        "rate_limit_exceeded" => LlmError::RateLimited {
            retry_after_ms: None,
        },
        "server_error" => LlmError::ServerError {
            status: 500,
            message: error_msg.to_string(),
        },
        "invalid_request_error" => LlmError::InvalidRequest {
            message: error_msg.to_string(),
        },
        _ => LlmError::Unknown {
            message: format!("{error_code}: {error_msg}"),
        },
    }
}

impl OpenAiClient {
    #[cfg(not(target_os = "espidf"))]
    fn stream_via_reqwest<'a>(&'a self, request: &'a LlmRequest) -> LlmStream<'a> {
        if !Self::request_stream_enabled(request) {
            return self.complete_via_reqwest(request);
        }
        let byte_stream = async_stream::try_stream! {
            let body = self.build_request_body(request)?;

            let response = self.http
                .post(format!("{}/v1/responses", self.base_url))
                .header("Authorization", format!("Bearer {}", self.api_key))
                .header("Content-Type", "application/json")
                .json(&body)
                .send()
                .await
                .map_err(|error| {
                    if error.is_timeout() {
                        LlmError::NetworkTimeout { duration_ms: 30000 }
                    } else {
                        #[cfg(not(target_arch = "wasm32"))]
                        if error.is_connect() {
                            return LlmError::ConnectionReset;
                        }
                        LlmError::Unknown { message: error.to_string() }
                    }
                })?;

            let status_code = response.status().as_u16();
            let stream_result = if (200..=299).contains(&status_code) {
                Ok(response.bytes_stream())
            } else {
                let headers = response.headers().clone();
                let text = response.text().await.unwrap_or_default();
                Err(LlmError::from_http_response(status_code, text, &headers))
            };
            let mut stream = stream_result?;
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|_| LlmError::ConnectionReset)?;
                yield chunk.to_vec();
            }
        };

        stream_openai_events_from_bytes(byte_stream)
    }

    #[cfg(not(target_os = "espidf"))]
    fn complete_via_reqwest<'a>(&'a self, request: &'a LlmRequest) -> LlmStream<'a> {
        Box::pin(async_stream::try_stream! {
            let body = self.build_request_body(request)?;

            let response = self.http
                .post(format!("{}/v1/responses", self.base_url))
                .header("Authorization", format!("Bearer {}", self.api_key))
                .header("Content-Type", "application/json")
                .json(&body)
                .send()
                .await
                .map_err(|error| {
                    if error.is_timeout() {
                        LlmError::NetworkTimeout { duration_ms: 30000 }
                    } else {
                        #[cfg(not(target_arch = "wasm32"))]
                        if error.is_connect() {
                            return LlmError::ConnectionReset;
                        }
                        LlmError::Unknown { message: error.to_string() }
                    }
                })?;

            let status_code = response.status().as_u16();
            let headers = response.headers().clone();
            let text = response.text().await.unwrap_or_default();

            if !(200..=299).contains(&status_code) {
                Err(LlmError::from_http_response(
                    status_code,
                    text.clone(),
                    &headers,
                ))?;
            }

            let response_obj: Value = serde_json::from_str(&text).map_err(|error| LlmError::InvalidRequest {
                message: format!("failed to parse OpenAI completion response: {error}"),
            })?;

            let mut state = OpenAiStreamState::default();
            let mut emitted = Vec::new();
            collect_response_completed_events(&response_obj, &mut state, &mut emitted);
            for event in emitted {
                yield event;
            }
        })
    }

    #[cfg(target_os = "espidf")]
    fn stream_via_espidf<'a>(&'a self, request: &'a LlmRequest) -> LlmStream<'a> {
        if !Self::request_stream_enabled(request) {
            return self.complete_via_espidf(request);
        }
        let byte_stream = async_stream::try_stream! {
            let request_seq = OPENAI_ESP_REQUEST_SEQ.fetch_add(1, Ordering::Relaxed);
            let body = self.build_request_body(request)?;
            let payload = serde_json::to_vec(&body).map_err(|error| LlmError::InvalidRequest {
                message: format!("failed to serialize OpenAI request body: {error}"),
            })?;
            println!(
                "MKT:OPENAI:REQ_START seq={} model=\"{}\" payload_bytes={}",
                request_seq,
                request.model,
                payload.len(),
            );
            let auth_header = format!("Bearer {}", self.api_key);
            let content_length = payload.len().to_string();
            let url = format!("{}/v1/responses", self.base_url);
            let headers = [
                ("content-type", "application/json"),
                ("authorization", auth_header.as_str()),
                ("content-length", content_length.as_str()),
            ];

            let http_config = EspHttpConfiguration {
                timeout: Some(Duration::from_secs(60)),
                buffer_size: Some(2048),
                buffer_size_tx: Some(2048),
                server_certificate: Some(X509::pem_until_nul(OPENAI_WE1_PEM)),
                crt_bundle_attach: None,
                // Repeated keep-alive sessions have been flaky on ESP32-S3 in
                // long-lived probe runs; prefer explicit reconnects.
                keep_alive_enable: false,
                ..Default::default()
            };
            let mut client = EmbeddedHttpClient::wrap(
                EspHttpConnection::new(&http_config).map_err(|error| LlmError::Unknown {
                    message: format!("failed to create ESP-IDF HTTPS client: {error}"),
                })?,
            );

            let mut http_request = client
                .request(Method::Post, &url, &headers)
                .map_err(|error| LlmError::Unknown {
                    message: format!("failed to create OpenAI request: {error}"),
                })?;
            http_request.write_all(&payload).map_err(|error| LlmError::Unknown {
                message: format!("failed to write OpenAI request payload: {error}"),
            })?;
            http_request.flush().map_err(|error| LlmError::Unknown {
                message: format!("failed to flush OpenAI request payload: {error}"),
            })?;

            let mut response = http_request.submit().map_err(|error| LlmError::Unknown {
                message: format!("failed to submit OpenAI request: {error}"),
            })?;
            let status_code = response.status();
            println!(
                "MKT:OPENAI:REQ_STATUS seq={} status={}",
                request_seq,
                status_code,
            );
            if !(200..=299).contains(&status_code) {
                let retry_after_ms = response
                    .header("retry-after")
                    .and_then(LlmError::parse_retry_after);
                let text = read_espidf_response_body(&mut response)?;
                Err(LlmError::from_http_status(status_code, text, retry_after_ms))?;
            }

            let mut buffer = [0_u8; 1024];
            let mut total_read = 0_usize;
            let mut chunk_index = 0_usize;
            loop {
                let read = response.read(&mut buffer).map_err(|error| LlmError::Unknown {
                    message: format!("failed to read OpenAI response body: {error}"),
                })?;
                if read == 0 {
                    println!(
                        "MKT:OPENAI:REQ_EOF seq={} total_bytes={} chunks={}",
                        request_seq,
                        total_read,
                        chunk_index,
                    );
                    break;
                }
                chunk_index += 1;
                total_read += read;
                if chunk_index <= 6 || chunk_index % 10 == 0 {
                    println!(
                        "MKT:OPENAI:REQ_READ seq={} chunk={} bytes={} total_bytes={}",
                        request_seq,
                        chunk_index,
                        read,
                        total_read,
                    );
                }
                yield buffer[..read].to_vec();
            }
        };

        stream_openai_events_from_bytes(byte_stream)
    }

    #[cfg(target_os = "espidf")]
    fn complete_via_espidf<'a>(&'a self, request: &'a LlmRequest) -> LlmStream<'a> {
        Box::pin(async_stream::try_stream! {
            let request_seq = OPENAI_ESP_REQUEST_SEQ.fetch_add(1, Ordering::Relaxed);
            let body = self.build_request_body(request)?;
            let payload = serde_json::to_vec(&body).map_err(|error| LlmError::InvalidRequest {
                message: format!("failed to serialize OpenAI request body: {error}"),
            })?;
            println!(
                "MKT:OPENAI:REQ_START seq={} model=\"{}\" payload_bytes={} mode=\"complete\"",
                request_seq,
                request.model,
                payload.len(),
            );
            let auth_header = format!("Bearer {}", self.api_key);
            let content_length = payload.len().to_string();
            let url = format!("{}/v1/responses", self.base_url);
            let headers = [
                ("content-type", "application/json"),
                ("authorization", auth_header.as_str()),
                ("content-length", content_length.as_str()),
            ];
            let mut completed = false;
            for attempt in 1..=OPENAI_ESP_MAX_ATTEMPTS {
                println!(
                    "MKT:OPENAI:REQ_START seq={} attempt={} model=\"{}\" payload_bytes={} mode=\"complete\"",
                    request_seq,
                    attempt,
                    request.model,
                    payload.len(),
                );
                let http_config = EspHttpConfiguration {
                    timeout: Some(Duration::from_secs(60)),
                    buffer_size: Some(2048),
                    buffer_size_tx: Some(2048),
                    server_certificate: Some(X509::pem_until_nul(OPENAI_WE1_PEM)),
                    crt_bundle_attach: None,
                    // Repeated keep-alive sessions have been flaky on ESP32-S3 in
                    // long-lived probe runs; prefer explicit reconnects.
                    keep_alive_enable: false,
                    ..Default::default()
                };

                let attempt_result: Result<Vec<LlmEvent>, LlmError> = (|| {
                    let mut client = EmbeddedHttpClient::wrap(
                        EspHttpConnection::new(&http_config).map_err(|error| LlmError::Unknown {
                            message: format!("failed to create ESP-IDF HTTPS client: {error}"),
                        })?,
                    );

                    let mut http_request = client
                        .request(Method::Post, &url, &headers)
                        .map_err(|error| LlmError::Unknown {
                            message: format!("failed to create OpenAI request: {error}"),
                        })?;
                    http_request.write_all(&payload).map_err(|error| LlmError::Unknown {
                        message: format!("failed to write OpenAI request payload: {error}"),
                    })?;
                    http_request.flush().map_err(|error| LlmError::Unknown {
                        message: format!("failed to flush OpenAI request payload: {error}"),
                    })?;

                    let mut response =
                        http_request.submit().map_err(|error| LlmError::Unknown {
                            message: format!("failed to submit OpenAI request: {error}"),
                        })?;
                    let status_code = response.status();
                    println!(
                        "MKT:OPENAI:REQ_STATUS seq={} attempt={} status={} mode=\"complete\"",
                        request_seq,
                        attempt,
                        status_code,
                    );
                    let text = read_espidf_response_body(&mut response)?;
                    println!(
                        "MKT:OPENAI:REQ_BODY seq={} attempt={} body_bytes={} mode=\"complete\"",
                        request_seq,
                        attempt,
                        text.len(),
                    );

                    if !(200..=299).contains(&status_code) {
                        let retry_after_ms = response
                            .header("retry-after")
                            .and_then(LlmError::parse_retry_after);
                        return Err(LlmError::from_http_status(
                            status_code,
                            text.clone(),
                            retry_after_ms,
                        ));
                    }

                    let response_obj: Value =
                        serde_json::from_str(&text).map_err(|error| LlmError::InvalidRequest {
                            message: format!("failed to parse OpenAI completion response: {error}"),
                        })?;

                    let mut state = OpenAiStreamState::default();
                    let mut emitted = Vec::new();
                    collect_response_completed_events(&response_obj, &mut state, &mut emitted);
                    Ok(emitted)
                })();

                match attempt_result {
                    Ok(emitted) => {
                        for event in emitted {
                            yield event;
                        }
                        completed = true;
                        break;
                    }
                    Err(error)
                        if attempt < OPENAI_ESP_MAX_ATTEMPTS && openai_esp_should_retry(&error) =>
                    {
                        openai_esp_log_retry(request_seq, attempt, "complete", &error);
                        std::thread::sleep(openai_esp_retry_delay(attempt));
                    }
                    Err(error) => Err(error)?,
                }
            }

            if !completed {
                Err(LlmError::Unknown {
                    message: "OpenAI request exhausted retries without completion".to_string(),
                })?;
            }
        })
    }
}

#[cfg(target_os = "espidf")]
fn openai_esp_retry_delay(attempt: usize) -> Duration {
    match attempt {
        1 => Duration::from_millis(250),
        2 => Duration::from_millis(750),
        _ => Duration::from_millis(1500),
    }
}

#[cfg(target_os = "espidf")]
fn openai_esp_should_retry(error: &LlmError) -> bool {
    if error.is_retryable() {
        return true;
    }

    matches!(
        error,
        LlmError::Unknown { message }
            if message.starts_with("failed to create ESP-IDF HTTPS client:")
                || message.starts_with("failed to create OpenAI request:")
                || message.starts_with("failed to write OpenAI request payload:")
                || message.starts_with("failed to flush OpenAI request payload:")
                || message.starts_with("failed to submit OpenAI request:")
                || message.starts_with("failed to read OpenAI response body:")
                || message.starts_with("failed to read OpenAI error response:")
    )
}

#[cfg(target_os = "espidf")]
fn openai_esp_log_retry(request_seq: usize, attempt: usize, mode: &str, error: &LlmError) {
    let delay_ms = openai_esp_retry_delay(attempt).as_millis();
    println!(
        "MKT:OPENAI:REQ_RETRY seq={} attempt={} next_attempt={} delay_ms={} mode=\"{}\" error={:?}",
        request_seq,
        attempt,
        attempt + 1,
        delay_ms,
        mode,
        error,
    );
}

#[cfg(target_os = "espidf")]
fn read_espidf_response_body(
    response: &mut EmbeddedResponse<&mut EspHttpConnection>,
) -> Result<String, LlmError> {
    let mut body = Vec::new();
    let mut buffer = [0_u8; 512];
    loop {
        let read = response
            .read(&mut buffer)
            .map_err(|error| LlmError::Unknown {
                message: format!("failed to read OpenAI error response: {error}"),
            })?;
        if read == 0 {
            break;
        }
        body.extend_from_slice(&buffer[..read]);
    }
    Ok(String::from_utf8_lossy(&body).into_owned())
}

/// OpenAI strict JSON schema mode requires `additionalProperties: false` on
/// object schemas. We preserve explicit caller values and only inject when
/// missing.
fn ensure_additional_properties_false(value: &mut Value) {
    match value {
        Value::Object(obj) => {
            let is_object_type = match obj.get("type") {
                Some(Value::String(t)) => t == "object",
                Some(Value::Array(types)) => types.iter().any(|t| t.as_str() == Some("object")),
                _ => obj.contains_key("properties") || obj.contains_key("required"),
            };

            if is_object_type && !obj.contains_key("additionalProperties") {
                obj.insert("additionalProperties".to_string(), Value::Bool(false));
            }

            for child in obj.values_mut() {
                ensure_additional_properties_false(child);
            }
        }
        Value::Array(items) => {
            for item in items.iter_mut() {
                ensure_additional_properties_false(item);
            }
        }
        _ => {}
    }
}

#[cfg_attr(any(target_arch = "wasm32", target_os = "espidf"), async_trait(?Send))]
#[cfg_attr(
    all(not(target_arch = "wasm32"), not(target_os = "espidf")),
    async_trait
)]
impl LlmClient for OpenAiClient {
    fn stream<'a>(&'a self, request: &'a LlmRequest) -> LlmStream<'a> {
        #[cfg(target_os = "espidf")]
        {
            self.stream_via_espidf(request)
        }
        #[cfg(not(target_os = "espidf"))]
        {
            self.stream_via_reqwest(request)
        }
    }

    fn provider(&self) -> &'static str {
        "openai"
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }

    fn compile_schema(&self, output_schema: &OutputSchema) -> Result<CompiledSchema, SchemaError> {
        let mut schema = output_schema.schema.as_value().clone();
        // OpenAI `strict` controls constrained decoding behavior for structured
        // output. `compat` is only used for provider-lowering policies where
        // warnings/errors may be emitted (e.g. Gemini keyword compatibility).
        if output_schema.strict {
            ensure_additional_properties_false(&mut schema);
        }

        Ok(CompiledSchema {
            schema,
            warnings: Vec::new(),
        })
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
    /// Error object for streaming error events
    error: Option<Value>,
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
fn fallback_raw_value() -> Box<RawValue> {
    RawValue::from_string("{}".to_string()).expect("static JSON is valid")
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use axum::{Router, extract::State, response::IntoResponse, routing::post};
    use meerkat_core::UserMessage;
    use tokio::net::TcpListener;

    async fn responses_sse(State(payload): State<String>) -> impl IntoResponse {
        ([("content-type", "text/event-stream")], payload)
    }

    async fn spawn_openai_stub_server(payload: String) -> (String, tokio::task::JoinHandle<()>) {
        let app = Router::new()
            .route("/v1/responses", post(responses_sse))
            .with_state(payload);
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test server");
        let addr = listener.local_addr().expect("local addr");
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve test server");
        });
        (format!("http://{addr}"), handle)
    }

    // =========================================================================
    // Responses API Request Format Tests
    // =========================================================================

    #[test]
    fn test_request_uses_responses_api_endpoint_format() {
        let client = OpenAiClient::new("test-key".to_string());
        let request = LlmRequest::new(
            "gpt-5.2",
            vec![Message::User(UserMessage::text("Hello".to_string()))],
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
                Message::User(UserMessage::text("Hello".to_string())),
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
                Message::User(UserMessage::text("Weather?".to_string())),
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
                Message::User(UserMessage::text("Weather?".to_string())),
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
            "gpt-4.1-mini",
            vec![Message::User(UserMessage::text("test".to_string()))],
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
            vec![Message::User(UserMessage::text("test".to_string()))],
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
            vec![Message::User(UserMessage::text("test".to_string()))],
        )
        .with_provider_param("reasoning_effort", "high");

        let body = client.build_request_body(&request).expect("build request");

        assert_eq!(body["reasoning"]["effort"], "high");
    }

    #[test]
    fn test_request_omits_reasoning_payload_for_non_gpt5_model() {
        let client = OpenAiClient::new("test-key".to_string());
        let request = LlmRequest::new(
            "gpt-4.1-mini",
            vec![Message::User(UserMessage::text("test".to_string()))],
        );

        let body = client.build_request_body(&request).expect("build request");
        assert!(body.get("reasoning").is_none());
        assert!(body.get("include").is_none());
    }

    #[test]
    fn test_reasoning_effort_ignored_for_non_gpt5_model() {
        let client = OpenAiClient::new("test-key".to_string());
        let request = LlmRequest::new(
            "gpt-4.1-mini",
            vec![Message::User(UserMessage::text("test".to_string()))],
        )
        .with_provider_param("reasoning_effort", "high");

        let body = client.build_request_body(&request).expect("build request");
        assert!(body.get("reasoning").is_none());
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
                Message::User(UserMessage::text("Hello".to_string())),
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
    fn test_request_input_format_block_assistant_reasoning_with_output_skips_reasoning_replay() {
        use meerkat_core::BlockAssistantMessage;

        let client = OpenAiClient::new("test-key".to_string());
        let request = LlmRequest::new(
            "gpt-5.2",
            vec![
                Message::User(UserMessage::text("Hello".to_string())),
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

        // Reasoning is intentionally not replayed; assistant text remains.
        assert_eq!(input.len(), 2);
        assert_eq!(input[1]["type"], "message");
        assert_eq!(input[1]["role"], "assistant");
    }

    #[test]
    fn test_request_input_format_block_assistant_tool_use() {
        use meerkat_core::BlockAssistantMessage;

        let client = OpenAiClient::new("test-key".to_string());
        let args = RawValue::from_string(r#"{"location":"Tokyo"}"#.to_string()).unwrap();
        let request = LlmRequest::new(
            "gpt-5.2",
            vec![
                Message::User(UserMessage::text("Weather?".to_string())),
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
            vec![Message::User(UserMessage::text("test".to_string()))],
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
            vec![Message::User(UserMessage::text("test".to_string()))],
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
            vec![Message::User(UserMessage::text("test".to_string()))],
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
            vec![Message::User(UserMessage::text("test".to_string()))],
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
    fn test_request_omits_temperature_for_gpt5_family() {
        let client = OpenAiClient::new("test-key".to_string());
        let request = LlmRequest::new(
            "gpt-5.2-codex",
            vec![Message::User(UserMessage::text("test".to_string()))],
        )
        .with_temperature(0.2);

        let body = client.build_request_body(&request).expect("build request");
        assert!(
            body.get("temperature").is_none(),
            "gpt-5/codex requests should not include temperature"
        );
    }

    #[test]
    fn test_request_includes_temperature_for_supported_model() {
        let client = OpenAiClient::new("test-key".to_string());
        let request = LlmRequest::new(
            "gpt-4.1-mini",
            vec![Message::User(UserMessage::text("test".to_string()))],
        )
        .with_temperature(0.3);

        let body = client.build_request_body(&request).expect("build request");
        let temp = body["temperature"]
            .as_f64()
            .expect("temperature should be numeric");
        assert!((temp - 0.3).abs() < 1e-6);
    }

    #[test]
    fn test_multiple_provider_params_combined() {
        let client = OpenAiClient::new("test-key".to_string());
        let request = LlmRequest::new(
            "gpt-5.2",
            vec![Message::User(UserMessage::text("test".to_string()))],
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
            "gpt-5.2",
            vec![
                Message::User(UserMessage::text("What's the weather?".to_string())),
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
            !arguments.starts_with(r"{\"),
            "arguments should not be double-encoded: {arguments}"
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
            vec![Message::User(UserMessage::text("test".to_string()))],
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
            vec![Message::User(UserMessage::text("test".to_string()))],
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
            vec![Message::User(UserMessage::text("test".to_string()))],
        );

        let body = client.build_request_body(&request).expect("build request");

        // text field should not be present
        assert!(
            body.get("text").is_none(),
            "text should not be present without structured_output"
        );
    }

    #[test]
    fn test_strict_structured_output_injects_additional_properties_recursively() {
        let client = OpenAiClient::new("test-key".to_string());

        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "name": {"type": "string"},
                "profile": {
                    "type": "object",
                    "properties": {
                        "city": {"type": "string"}
                    }
                },
                "addresses": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "street": {"type": "string"}
                        }
                    }
                },
                "choice": {
                    "anyOf": [
                        {
                            "type": "object",
                            "properties": {
                                "kind": {"type": "string"}
                            }
                        },
                        {"type": "string"}
                    ]
                }
            },
            "required": ["name", "profile", "addresses"]
        });

        let request = LlmRequest::new(
            "gpt-5.2",
            vec![Message::User(UserMessage::text("test".to_string()))],
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
        let compiled = &body["text"]["format"]["schema"];

        assert_eq!(compiled["additionalProperties"], false);
        assert_eq!(
            compiled["properties"]["profile"]["additionalProperties"],
            false
        );
        assert_eq!(
            compiled["properties"]["addresses"]["items"]["additionalProperties"],
            false
        );
        assert_eq!(
            compiled["properties"]["choice"]["anyOf"][0]["additionalProperties"],
            false
        );
    }

    #[test]
    fn test_strict_structured_output_preserves_explicit_additional_properties() {
        let client = OpenAiClient::new("test-key".to_string());

        let schema = serde_json::json!({
            "type": "object",
            "additionalProperties": true,
            "properties": {
                "nested": {
                    "type": "object",
                    "additionalProperties": {"type": "string"},
                    "properties": {
                        "x": {"type": "string"}
                    }
                },
                "auto": {
                    "type": "object",
                    "properties": {
                        "y": {"type": "integer"}
                    }
                }
            }
        });

        let request = LlmRequest::new(
            "gpt-5.2",
            vec![Message::User(UserMessage::text("test".to_string()))],
        )
        .with_provider_param(
            "structured_output",
            serde_json::json!({
                "schema": schema,
                "strict": true
            }),
        );

        let body = client.build_request_body(&request).expect("build request");
        let compiled = &body["text"]["format"]["schema"];

        assert_eq!(compiled["additionalProperties"], true);
        assert_eq!(
            compiled["properties"]["nested"]["additionalProperties"],
            serde_json::json!({"type": "string"})
        );
        assert_eq!(
            compiled["properties"]["auto"]["additionalProperties"],
            false
        );
    }

    #[test]
    fn test_non_strict_structured_output_does_not_inject_additional_properties() {
        let client = OpenAiClient::new("test-key".to_string());

        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "nested": {
                    "type": "object",
                    "properties": {
                        "x": {"type": "string"}
                    }
                }
            }
        });

        let request = LlmRequest::new(
            "gpt-5.2",
            vec![Message::User(UserMessage::text("test".to_string()))],
        )
        .with_provider_param(
            "structured_output",
            serde_json::json!({
                "schema": schema,
                "strict": false
            }),
        );

        let body = client.build_request_body(&request).expect("build request");
        let compiled = &body["text"]["format"]["schema"];

        assert!(
            compiled.get("additionalProperties").is_none(),
            "root should not be modified in non-strict mode"
        );
        assert!(
            compiled["properties"]["nested"]
                .get("additionalProperties")
                .is_none(),
            "nested object should not be modified in non-strict mode"
        );
    }

    #[test]
    fn test_compile_schema_strict_handles_object_union_and_defs() {
        let client = OpenAiClient::new("test-key".to_string());
        let schema = serde_json::json!({
            "type": ["object", "null"],
            "$defs": {
                "Meta": {
                    "type": "object",
                    "properties": {
                        "id": {"type": "string"}
                    }
                }
            },
            "properties": {
                "meta": {"$ref": "#/$defs/Meta"},
                "items": {
                    "type": "array",
                    "items": {
                        "type": ["object", "null"],
                        "properties": {
                            "value": {"type": "number"}
                        }
                    }
                }
            }
        });
        let output_schema = OutputSchema::new(schema).expect("valid schema").strict();
        let compiled = client
            .compile_schema(&output_schema)
            .expect("compile should succeed");

        assert!(compiled.warnings.is_empty());
        assert_eq!(compiled.schema["additionalProperties"], false);
        assert_eq!(
            compiled.schema["$defs"]["Meta"]["additionalProperties"],
            false
        );
        assert_eq!(
            compiled.schema["properties"]["items"]["items"]["additionalProperties"],
            false
        );
    }

    #[test]
    fn test_compile_schema_strict_keeps_explicit_additional_properties_forms() {
        let client = OpenAiClient::new("test-key".to_string());
        let schema = serde_json::json!({
            "type": "object",
            "additionalProperties": {"type": "integer"},
            "properties": {
                "a": {"type": "string"},
                "b": {
                    "type": "object",
                    "additionalProperties": true,
                    "properties": {
                        "x": {"type": "string"}
                    }
                }
            }
        });
        let output_schema = OutputSchema::new(schema).expect("valid schema").strict();
        let compiled = client
            .compile_schema(&output_schema)
            .expect("compile should succeed");

        assert!(compiled.warnings.is_empty());
        assert_eq!(
            compiled.schema["additionalProperties"],
            serde_json::json!({"type": "integer"})
        );
        assert_eq!(
            compiled.schema["properties"]["b"]["additionalProperties"],
            true
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

    #[tokio::test]
    async fn test_stream_does_not_duplicate_text_when_completed_replays_output() {
        let payload = [
            r#"data: {"type":"response.output_text.delta","delta":"Hello"}"#,
            r#"data: {"type":"response.completed","response":{"status":"completed","output":[{"type":"message","content":[{"type":"output_text","text":"Hello"}]}],"usage":{"input_tokens":10,"output_tokens":5}}}"#,
            r#"data: {"type":"response.done","response":{"status":"completed","output":[{"type":"message","content":[{"type":"output_text","text":"Hello"}]}],"usage":{"input_tokens":10,"output_tokens":5}}}"#,
            "data: [DONE]",
            "",
        ]
        .join("\n");
        let (base_url, server) = spawn_openai_stub_server(payload).await;
        let client = OpenAiClient::new_with_base_url("test-key".to_string(), base_url);
        let request = LlmRequest::new(
            "gpt-5-mini",
            vec![Message::User(UserMessage::text("hello".to_string()))],
        );

        let mut stream = client.stream(&request);
        let mut deltas = Vec::new();
        while let Some(event) = stream.next().await {
            match event.expect("stream event") {
                LlmEvent::TextDelta { delta, .. } => deltas.push(delta),
                LlmEvent::Done { .. } => break,
                _ => {}
            }
        }
        server.abort();

        assert_eq!(deltas, vec!["Hello"]);
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
                Message::User(UserMessage::text("Hello".to_string())),
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
                Message::User(UserMessage::text("First question".to_string())),
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
                Message::User(UserMessage::text("Second question".to_string())),
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
                Message::User(UserMessage::text("Hello".to_string())),
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
    fn test_reasoning_followed_by_function_call_skips_reasoning_replay() {
        use meerkat_core::BlockAssistantMessage;

        let client = OpenAiClient::new("test-key".to_string());
        let args = RawValue::from_string(r#"{"q":"test"}"#.to_string()).unwrap();
        let request = LlmRequest::new(
            "gpt-5.2",
            vec![
                Message::User(UserMessage::text("Hello".to_string())),
                Message::BlockAssistant(BlockAssistantMessage {
                    blocks: vec![
                        AssistantBlock::Reasoning {
                            text: "I should search".to_string(),
                            meta: Some(Box::new(ProviderMeta::OpenAi {
                                id: "rs_valid".to_string(),
                                encrypted_content: Some("enc_valid".to_string()),
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

        // Reasoning is intentionally not replayed; function_call remains.
        assert_eq!(input.len(), 2);
        assert_eq!(input[1]["type"], "function_call");
    }

    #[test]
    fn test_non_openai_reasoning_blocks_are_skipped() {
        use meerkat_core::BlockAssistantMessage;

        let client = OpenAiClient::new("test-key".to_string());
        let request = LlmRequest::new(
            "gpt-5.2",
            vec![
                Message::User(UserMessage::text("Hello".to_string())),
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

    #[test]
    fn test_consecutive_orphaned_reasoning_items_all_stripped() {
        use meerkat_core::BlockAssistantMessage;

        let client = OpenAiClient::new("test-key".to_string());
        let request = LlmRequest::new(
            "gpt-5.2",
            vec![
                Message::User(UserMessage::text("Hello".to_string())),
                // Two consecutive reasoning-only messages (e.g., repeated interruptions)
                Message::BlockAssistant(BlockAssistantMessage {
                    blocks: vec![AssistantBlock::Reasoning {
                        text: "First thought".to_string(),
                        meta: Some(Box::new(ProviderMeta::OpenAi {
                            id: "rs_first".to_string(),
                            encrypted_content: Some("enc_1".to_string()),
                        })),
                    }],
                    stop_reason: StopReason::EndTurn,
                }),
                Message::BlockAssistant(BlockAssistantMessage {
                    blocks: vec![AssistantBlock::Reasoning {
                        text: "Second thought".to_string(),
                        meta: Some(Box::new(ProviderMeta::OpenAi {
                            id: "rs_second".to_string(),
                            encrypted_content: None,
                        })),
                    }],
                    stop_reason: StopReason::EndTurn,
                }),
                Message::User(UserMessage::text("Still here".to_string())),
            ],
        );

        let body = client.build_request_body(&request).expect("build request");
        let input = body["input"].as_array().expect("input should be array");

        // Both orphaned reasoning items stripped, only user messages remain
        assert_eq!(input.len(), 2);
        assert_eq!(input[0]["content"], "Hello");
        assert_eq!(input[1]["content"], "Still here");
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
        let has_tools = response_tool["output"].as_array().is_some_and(|arr| {
            arr.iter()
                .any(|item| item.get("type").and_then(|t| t.as_str()) == Some("function_call"))
        });
        assert!(has_tools);

        // completed without tool calls -> EndTurn
        let response_text = serde_json::json!({
            "status": "completed",
            "output": [{"type": "message", "content": [{"type": "output_text", "text": "Hi"}]}]
        });
        let has_tools = response_text["output"].as_array().is_some_and(|arr| {
            arr.iter()
                .any(|item| item.get("type").and_then(|t| t.as_str()) == Some("function_call"))
        });
        assert!(!has_tools);
    }

    // =========================================================================
    // Reasoning encrypted_content stripping tests
    // =========================================================================

    #[test]
    fn test_reasoning_without_encrypted_content_is_stripped() {
        use meerkat_core::BlockAssistantMessage;

        let client = OpenAiClient::new("test-key".to_string());
        let args = RawValue::from_string(r#"{"q":"test"}"#.to_string()).unwrap();
        let request = LlmRequest::new(
            "gpt-5.2",
            vec![
                Message::User(UserMessage::text("Hello".to_string())),
                Message::BlockAssistant(BlockAssistantMessage {
                    blocks: vec![
                        AssistantBlock::Reasoning {
                            text: "I should search".to_string(),
                            meta: Some(Box::new(ProviderMeta::OpenAi {
                                id: "rs_no_enc".to_string(),
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

        // Reasoning without encrypted_content is stripped even with valid follower
        assert_eq!(input.len(), 2);
        assert_eq!(input[0]["type"], "message");
        assert_eq!(input[1]["type"], "function_call");
    }

    #[test]
    fn test_reasoning_with_encrypted_content_is_not_replayed() {
        use meerkat_core::BlockAssistantMessage;

        let client = OpenAiClient::new("test-key".to_string());
        let request = LlmRequest::new(
            "gpt-5.2",
            vec![
                Message::User(UserMessage::text("Hello".to_string())),
                Message::BlockAssistant(BlockAssistantMessage {
                    blocks: vec![
                        AssistantBlock::Reasoning {
                            text: "Let me think".to_string(),
                            meta: Some(Box::new(ProviderMeta::OpenAi {
                                id: "rs_enc".to_string(),
                                encrypted_content: Some("enc_data_here".to_string()),
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

        // Reasoning is intentionally not replayed; assistant text remains.
        assert_eq!(input.len(), 2);
        assert_eq!(input[1]["type"], "message");
    }

    // =========================================================================
    // Stream deduplication tests
    // =========================================================================

    #[tokio::test]
    async fn test_stream_does_not_duplicate_tool_calls_when_completed_replays() {
        let payload = [
            // Streaming tool call events
            r#"data: {"type":"response.function_call_arguments.delta","call_id":"call_1","name":"get_weather","delta":"{\"loc"}"#,
            r#"data: {"type":"response.function_call_arguments.delta","call_id":"call_1","delta":"ation\":\"Tokyo\"}"}"#,
            r#"data: {"type":"response.function_call_arguments.done","call_id":"call_1","name":"get_weather","arguments":"{\"location\":\"Tokyo\"}"}"#,
            // response.completed replays the same tool call
            r#"data: {"type":"response.completed","response":{"status":"completed","output":[{"type":"function_call","call_id":"call_1","name":"get_weather","arguments":"{\"location\":\"Tokyo\"}"}],"usage":{"input_tokens":10,"output_tokens":5}}}"#,
            r#"data: {"type":"response.done","response":{"status":"completed","output":[{"type":"function_call","call_id":"call_1","name":"get_weather","arguments":"{\"location\":\"Tokyo\"}"}],"usage":{"input_tokens":10,"output_tokens":5}}}"#,
            "data: [DONE]",
            "",
        ]
        .join("\n");
        let (base_url, server) = spawn_openai_stub_server(payload).await;
        let client = OpenAiClient::new_with_base_url("test-key".to_string(), base_url);
        let request = LlmRequest::new(
            "gpt-5-mini",
            vec![Message::User(UserMessage::text("weather".to_string()))],
        );

        let mut stream = client.stream(&request);
        let mut tool_completes = Vec::new();
        while let Some(event) = stream.next().await {
            match event.expect("stream event") {
                LlmEvent::ToolCallComplete { id, .. } => tool_completes.push(id),
                LlmEvent::Done { .. } => break,
                _ => {}
            }
        }
        server.abort();

        // Should only get one ToolCallComplete, not duplicated from response.completed
        assert_eq!(tool_completes.len(), 1);
        assert_eq!(tool_completes[0], "call_1");
    }

    #[tokio::test]
    async fn test_stream_does_not_duplicate_reasoning_when_completed_replays() {
        let payload = [
            // Streaming reasoning events
            r#"data: {"type":"response.reasoning_summary_text.delta","delta":"thinking..."}"#,
            r#"data: {"type":"response.reasoning.done","item":{"id":"rs_1","summary":[{"type":"summary_text","text":"thinking..."}],"encrypted_content":"enc_xyz"}}"#,
            // response.completed replays the same reasoning
            r#"data: {"type":"response.completed","response":{"status":"completed","output":[{"type":"reasoning","id":"rs_1","summary":[{"type":"summary_text","text":"thinking..."}],"encrypted_content":"enc_xyz"},{"type":"message","content":[{"type":"output_text","text":"Hello"}]}],"usage":{"input_tokens":10,"output_tokens":5}}}"#,
            r#"data: {"type":"response.done","response":{"status":"completed","output":[],"usage":{"input_tokens":10,"output_tokens":5}}}"#,
            "data: [DONE]",
            "",
        ]
        .join("\n");
        let (base_url, server) = spawn_openai_stub_server(payload).await;
        let client = OpenAiClient::new_with_base_url("test-key".to_string(), base_url);
        let request = LlmRequest::new(
            "gpt-5-mini",
            vec![Message::User(UserMessage::text("hello".to_string()))],
        );

        let mut stream = client.stream(&request);
        let mut reasoning_completes = 0;
        while let Some(event) = stream.next().await {
            match event.expect("stream event") {
                LlmEvent::ReasoningComplete { .. } => reasoning_completes += 1,
                LlmEvent::Done { .. } => break,
                _ => {}
            }
        }
        server.abort();

        // Should only get one ReasoningComplete, not duplicated from response.completed
        assert_eq!(reasoning_completes, 1);
    }

    #[tokio::test]
    async fn test_stream_error_event_yields_done_with_error() {
        let payload = [
            r#"data: {"type":"error","error":{"code":"server_error","message":"Internal server error"}}"#,
            "data: [DONE]",
            "",
        ]
        .join("\n");
        let (base_url, server) = spawn_openai_stub_server(payload).await;
        let client = OpenAiClient::new_with_base_url("test-key".to_string(), base_url);
        let request = LlmRequest::new(
            "gpt-5-mini",
            vec![Message::User(UserMessage::text("hello".to_string()))],
        );

        let mut stream = client.stream(&request);
        let mut saw_error_done = false;
        while let Some(event) = stream.next().await {
            if let LlmEvent::Done {
                outcome: LlmDoneOutcome::Error { error },
            } = event.expect("stream event")
            {
                assert!(
                    matches!(error, LlmError::ServerError { status: 500, .. }),
                    "expected ServerError, got: {error:?}"
                );
                let msg = error.to_string();
                assert!(
                    msg.contains("Internal server error"),
                    "error should contain message: {msg}"
                );
                saw_error_done = true;
                break;
            }
        }
        server.abort();

        assert!(saw_error_done, "Expected Done with error outcome");
    }

    // =========================================================================
    // Multimodal content (ContentBlock::Image) serialization tests
    // =========================================================================

    #[test]
    fn openai_user_message_with_image() {
        use meerkat_core::ContentBlock;

        let client = OpenAiClient::new("test-key".to_string());
        let request = LlmRequest::new(
            "gpt-5.2",
            vec![Message::User(UserMessage::with_blocks(vec![
                ContentBlock::Text {
                    text: "describe this".to_string(),
                },
                ContentBlock::Image {
                    media_type: "image/png".to_string(),
                    data: "iVBOR...".into(),
                },
            ]))],
        );

        let body = client.build_request_body(&request).expect("build request");
        let input = body["input"].as_array().expect("input array");
        let user_item = &input[0];

        assert_eq!(user_item["type"], "message");
        assert_eq!(user_item["role"], "user");

        // Content should be an array with typed content parts
        let content = user_item["content"].as_array().expect("content array");
        assert_eq!(content.len(), 2);

        assert_eq!(content[0]["type"], "input_text");
        assert_eq!(content[0]["text"], "describe this");

        assert_eq!(content[1]["type"], "input_image");
        assert_eq!(
            content[1]["image_url"], "data:image/png;base64,iVBOR...",
            "should be a data URI"
        );

        // source_path must NOT leak
        let body_str = serde_json::to_string(&body).unwrap();
        assert!(
            !body_str.contains("source_path"),
            "source_path must never appear in provider payload"
        );
        assert!(
            !body_str.contains("/tmp/img.png"),
            "source_path value must never appear in provider payload"
        );
    }

    #[test]
    fn openai_text_only_user_message_stays_string() {
        let client = OpenAiClient::new("test-key".to_string());
        let request = LlmRequest::new(
            "gpt-5.2",
            vec![Message::User(UserMessage::text("just text"))],
        );

        let body = client.build_request_body(&request).expect("build request");
        let input = body["input"].as_array().expect("input array");

        // Text-only user message content should remain a plain string
        assert!(
            input[0]["content"].is_string(),
            "text-only user message content should be a string"
        );
        assert_eq!(input[0]["content"], "just text");
    }

    #[test]
    fn openai_tool_result_with_image_degrades_to_text() {
        use meerkat_core::ContentBlock;

        let client = OpenAiClient::new("test-key".to_string());
        let request = LlmRequest::new(
            "gpt-5.2",
            vec![
                Message::User(UserMessage::text("Take a screenshot")),
                Message::ToolResults {
                    results: vec![meerkat_core::ToolResult::with_blocks(
                        "call_1".to_string(),
                        vec![
                            ContentBlock::Text {
                                text: "screenshot taken".to_string(),
                            },
                            ContentBlock::Image {
                                media_type: "image/png".to_string(),
                                data: "iVBOR...".into(),
                            },
                        ],
                        false,
                    )],
                },
            ],
        );

        let body = client.build_request_body(&request).expect("build request");
        let input = body["input"].as_array().expect("input array");

        // Tool result item
        let tool_output = &input[1];
        assert_eq!(tool_output["type"], "function_call_output");
        assert_eq!(tool_output["call_id"], "call_1");

        // OpenAI tool results only accept strings -- images degrade to text projection
        let output = tool_output["output"]
            .as_str()
            .expect("output should be string");
        assert!(
            output.contains("screenshot taken"),
            "text content should be preserved"
        );
        assert!(
            output.contains("[image: image/png]"),
            "image should degrade to text projection: got {output}"
        );
    }
}
