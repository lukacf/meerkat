//! OpenAI-compatible client for self-hosted endpoints.

use async_trait::async_trait;
use futures::StreamExt;
use meerkat_core::schema::{CompiledSchema, SchemaError};
use meerkat_core::{
    AssistantBlock, ContentBlock, ImageData, Message, OutputSchema, StopReason, Usage,
};
use meerkat_llm_core::LlmError;
use meerkat_llm_core::{
    LlmClient, LlmDoneOutcome, LlmEvent, LlmRequest, LlmStream, ToolCallBuffer,
};
use meerkat_llm_core::{http, streaming};
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenAiCompatibleMode {
    Responses,
    ChatCompletions,
}

/// OpenAI-compatible client for self-hosted servers.
pub struct OpenAiCompatibleClient {
    mode: OpenAiCompatibleMode,
    remote_model: String,
    bearer_token: Option<String>,
    base_url: String,
    http: reqwest::Client,
    responses_delegate: Option<crate::OpenAiClient>,
    supports_temperature: bool,
    supports_thinking: bool,
    supports_reasoning: bool,
}

impl OpenAiCompatibleClient {
    pub fn new(
        mode: OpenAiCompatibleMode,
        remote_model: String,
        base_url: String,
        bearer_token: Option<String>,
        supports_temperature: bool,
        supports_thinking: bool,
        supports_reasoning: bool,
    ) -> Self {
        let http = http::build_http_client_for_base_url(reqwest::Client::builder(), &base_url)
            .unwrap_or_else(|_| reqwest::Client::new());
        let responses_delegate = matches!(mode, OpenAiCompatibleMode::Responses).then(|| {
            crate::OpenAiClient::new_with_optional_api_key_and_base_url(
                bearer_token.clone(),
                trim_v1_suffix(&base_url),
            )
        });
        Self {
            mode,
            remote_model,
            bearer_token,
            base_url,
            http,
            responses_delegate,
            supports_temperature,
            supports_thinking,
            supports_reasoning,
        }
    }

    fn request_with_remote_model(&self, request: &LlmRequest) -> LlmRequest {
        use meerkat_core::lifecycle::run_primitive::{OpenAiProviderTag, ProviderTag};
        let mut request = request.clone();
        request.model = self.remote_model.clone();
        let mut tag = match request.provider_params.take() {
            Some(ProviderTag::OpenAi(t)) => t,
            Some(_) => OpenAiProviderTag::default(),
            None => OpenAiProviderTag::default(),
        };
        tag.supports_temperature_override = Some(self.supports_temperature);
        tag.supports_reasoning_override = Some(self.supports_reasoning);
        request.provider_params = Some(ProviderTag::OpenAi(tag));
        request
    }

    fn map_send_error(error: reqwest::Error) -> LlmError {
        if error.is_timeout() {
            LlmError::NetworkTimeout { duration_ms: 30000 }
        } else if Self::is_connection_error(&error) {
            LlmError::ConnectionReset
        } else {
            LlmError::Unknown {
                message: error.to_string(),
            }
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn is_connection_error(error: &reqwest::Error) -> bool {
        error.is_connect()
    }

    #[cfg(target_arch = "wasm32")]
    fn is_connection_error(_error: &reqwest::Error) -> bool {
        false
    }

    fn build_chat_completions_body(&self, request: &LlmRequest) -> Result<Value, LlmError> {
        let mut body = serde_json::json!({
            "model": self.remote_model,
            "messages": Self::convert_to_chat_messages(&request.messages)?,
            "stream": true,
            "stream_options": { "include_usage": true },
            "max_completion_tokens": request.max_tokens,
        });

        if self.supports_temperature
            && let Some(temp) = request.temperature
            && let Some(num) = serde_json::Number::from_f64(temp as f64)
        {
            body["temperature"] = Value::Number(num);
        }

        if !request.tools.is_empty() {
            body["tools"] = Value::Array(
                request
                    .tools
                    .iter()
                    .map(|tool| {
                        serde_json::json!({
                            "type": "function",
                            "function": {
                                "name": tool.name,
                                "description": tool.description,
                                "parameters": tool.input_schema
                            }
                        })
                    })
                    .collect(),
            );
        }

        if let Some(tag) = crate::client::openai_tag(request) {
            use meerkat_core::lifecycle::run_primitive::ReasoningEffort as TypedReasoningEffort;
            if self.supports_reasoning {
                if let Some(reasoning) = tag.reasoning.as_ref() {
                    let v = reasoning.as_value();
                    if v.is_object() {
                        body["reasoning"] = v;
                    }
                }
                if let Some(effort) = tag.reasoning_effort {
                    let s = match effort {
                        TypedReasoningEffort::Low => "low",
                        TypedReasoningEffort::Medium => "medium",
                        TypedReasoningEffort::High => "high",
                    };
                    if !body["reasoning"].is_object() {
                        body["reasoning"] = serde_json::json!({});
                    }
                    body["reasoning"]["effort"] = Value::String(s.to_string());
                    body["reasoning_effort"] = Value::String(s.to_string());
                }
                if self.supports_thinking
                    && let Some(chat_template_kwargs) = tag.chat_template_kwargs.as_ref()
                {
                    body["chat_template_kwargs"] = chat_template_kwargs.as_value();
                }
                if self.supports_thinking
                    && let Some(thinking) = tag.thinking.as_ref()
                {
                    body["thinking"] = thinking.as_value();
                }
            }
            if let Some(output_schema) = tag.structured_output.as_ref() {
                let compiled =
                    self.compile_schema(output_schema)
                        .map_err(|e| LlmError::InvalidRequest {
                            message: e.to_string(),
                        })?;
                body["response_format"] = serde_json::json!({
                    "type": "json_schema",
                    "json_schema": {
                        "name": output_schema.name.as_deref().unwrap_or("output"),
                        "schema": compiled.schema,
                        "strict": output_schema.strict
                    }
                });
            }
        }

        Ok(body)
    }

    fn convert_to_chat_messages(messages: &[Message]) -> Result<Vec<Value>, LlmError> {
        let mut out = Vec::new();
        for message in messages {
            match message {
                Message::System(system) => {
                    out.push(serde_json::json!({
                        "role": "system",
                        "content": system.content
                    }));
                }
                Message::SystemNotice(notice) => {
                    out.push(serde_json::json!({
                        "role": "user",
                        "content": notice.rendered_text()
                    }));
                }
                Message::User(user) => {
                    if meerkat_core::has_non_text_content(&user.content) {
                        let content: Vec<Value> = user
                            .content
                            .iter()
                            .map(|block| match block {
                                ContentBlock::Text { text } => serde_json::json!({
                                    "type": "text",
                                    "text": text
                                }),
                                ContentBlock::Image { media_type, data } => match data {
                                    ImageData::Inline { data } => serde_json::json!({
                                        "type": "image_url",
                                        "image_url": {
                                            "url": format!("data:{media_type};base64,{data}")
                                        }
                                    }),
                                    ImageData::Blob { .. } => serde_json::json!({
                                        "type": "text",
                                        "text": block.text_projection()
                                    }),
                                },
                                _ => serde_json::json!({
                                    "type": "text",
                                    "text": block.text_projection()
                                }),
                            })
                            .collect();
                        out.push(serde_json::json!({
                            "role": "user",
                            "content": content
                        }));
                    } else {
                        out.push(serde_json::json!({
                            "role": "user",
                            "content": user.text_content()
                        }));
                    }
                }
                Message::Assistant(assistant) => {
                    let tool_calls: Vec<Value> = assistant
                        .tool_calls
                        .iter()
                        .map(|tool_call| {
                            serde_json::json!({
                                "id": tool_call.id,
                                "type": "function",
                                "function": {
                                    "name": tool_call.name,
                                    "arguments": tool_call.args.to_string(),
                                }
                            })
                        })
                        .collect();
                    out.push(serde_json::json!({
                        "role": "assistant",
                        "content": if assistant.content.is_empty() {
                            Value::Null
                        } else {
                            Value::String(assistant.content.clone())
                        },
                        "tool_calls": tool_calls
                    }));
                }
                Message::BlockAssistant(assistant) => {
                    let mut text_parts = Vec::new();
                    let mut tool_calls = Vec::new();
                    for block in &assistant.blocks {
                        match block {
                            AssistantBlock::Text { text, .. } => {
                                if !text.is_empty() {
                                    text_parts.push(text.clone());
                                }
                            }
                            AssistantBlock::ToolUse { id, name, args, .. } => {
                                tool_calls.push(serde_json::json!({
                                    "id": id,
                                    "type": "function",
                                    "function": {
                                        "name": name,
                                        "arguments": args.get(),
                                    }
                                }));
                            }
                            _ => {}
                        }
                    }
                    out.push(serde_json::json!({
                        "role": "assistant",
                        "content": if text_parts.is_empty() {
                            Value::Null
                        } else {
                            Value::String(text_parts.join("\n"))
                        },
                        "tool_calls": tool_calls
                    }));
                }
                Message::ToolResults { results, .. } => {
                    for result in results {
                        out.push(serde_json::json!({
                            "role": "tool",
                            "tool_call_id": result.tool_use_id,
                            "content": result.text_content()
                        }));
                    }
                }
            }
        }
        Ok(out)
    }

    fn apply_auth(
        &self,
        request: reqwest::RequestBuilder,
        content_type: &'static str,
    ) -> reqwest::RequestBuilder {
        let request = request.header("Content-Type", content_type);
        if let Some(token) = &self.bearer_token {
            request.header("Authorization", format!("Bearer {token}"))
        } else {
            request
        }
    }

    fn parse_chat_completions_line(line: &str) -> Result<Option<ChatCompletionsChunk>, LlmError> {
        if let Some(data) = line
            .strip_prefix("data: ")
            .or_else(|| line.strip_prefix("data:"))
        {
            if data == "[DONE]" {
                return Ok(None);
            }
            serde_json::from_str(data)
                .map(Some)
                .map_err(|err| LlmError::StreamParseError {
                    message: format!("failed to parse chat completions chunk: {err}; line={data}"),
                })
        } else {
            Ok(None)
        }
    }
}

fn trim_v1_suffix(base_url: &str) -> String {
    base_url
        .trim_end_matches('/')
        .trim_end_matches("/v1")
        .to_string()
}

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

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl LlmClient for OpenAiCompatibleClient {
    fn stream<'a>(&'a self, request: &'a LlmRequest) -> LlmStream<'a> {
        match self.mode {
            OpenAiCompatibleMode::Responses => {
                let Some(delegate) = self.responses_delegate.as_ref() else {
                    let inner: LlmStream<'a> = Box::pin(futures::stream::once(async {
                        Err(LlmError::InvalidRequest {
                            message: "responses mode requires a configured delegate client"
                                .to_string(),
                        })
                    }));
                    return inner;
                };
                let inner: LlmStream<'a> = Box::pin(async_stream::try_stream! {
                    let translated = self.request_with_remote_model(request);
                    let mut stream = delegate.stream(&translated);
                    while let Some(event) = stream.next().await {
                        yield event?;
                    }
                });
                streaming::ensure_terminal_done(inner)
            }
            OpenAiCompatibleMode::ChatCompletions => {
                let inner: LlmStream<'a> = Box::pin(async_stream::try_stream! {
                    let body = self.build_chat_completions_body(request)?;
                    let response = self
                        .apply_auth(
                            self.http.post(format!("{}/chat/completions", self.base_url)),
                            "application/json",
                        )
                        .json(&body)
                        .send()
                        .await
                        .map_err(Self::map_send_error)?;

                    let status_code = response.status().as_u16();
                    let stream_result = if (200..=299).contains(&status_code) {
                        Ok(response.bytes_stream())
                    } else {
                        let headers = response.headers().clone();
                        let text = response.text().await.unwrap_or_default();
                        Err(LlmError::from_http_response(status_code, text, &headers))
                    };
                    let mut stream = stream_result?;
                    let mut buffer = String::with_capacity(512);
                    let mut tool_buffers: HashMap<usize, ToolCallBuffer> = HashMap::new();
                    let mut reasoning_text = String::new();
                    let mut done_emitted = false;

                    while let Some(chunk) = stream.next().await {
                        let chunk = chunk.map_err(|_| LlmError::ConnectionReset)?;
                        buffer.push_str(&String::from_utf8_lossy(&chunk));

                        while let Some(newline_pos) = buffer.find('\n') {
                            let line = buffer[..newline_pos].trim();
                            let should_process = !line.is_empty() && !line.starts_with(':');
                            let parsed = if should_process {
                                Self::parse_chat_completions_line(line)
                            } else {
                                Ok(None)
                            };
                            buffer.drain(..=newline_pos);

                            if let Some(event) = parsed? {
                                if let Some(event_usage) = event.usage {
                                    let usage = Usage {
                                        input_tokens: event_usage.prompt_tokens.unwrap_or(0),
                                        output_tokens: event_usage.completion_tokens.unwrap_or(0),
                                        cache_creation_tokens: None,
                                        cache_read_tokens: None,
                                    };
                                    yield LlmEvent::UsageUpdate { usage };
                                }

                                for choice in event.choices {
                                    if let Some(delta) = choice.delta {
                                        if let Some(content) = delta.content
                                            && !content.is_empty()
                                        {
                                            yield LlmEvent::TextDelta {
                                                delta: content,
                                                meta: None,
                                            };
                                        }
                                        let reasoning_delta = delta
                                            .reasoning_content
                                            .as_ref()
                                            .or(delta.reasoning.as_ref())
                                            .or(delta.thinking.as_ref());
                                        if let Some(reasoning) = reasoning_delta
                                            && !reasoning.is_empty()
                                        {
                                            reasoning_text.push_str(reasoning);
                                            yield LlmEvent::ReasoningDelta {
                                                delta: reasoning.clone(),
                                            };
                                        }
                                        if let Some(tool_calls) = delta.tool_calls {
                                            for tool_call in tool_calls {
                                                let index = tool_call.index.unwrap_or(0);
                                                let buffer = tool_buffers.entry(index).or_insert_with(|| {
                                                    ToolCallBuffer::new(
                                                        tool_call
                                                            .id
                                                            .clone()
                                                            .unwrap_or_else(|| format!("tool_call_{index}")),
                                                    )
                                                });
                                                if let Some(id) = tool_call.id
                                                    && buffer.id.starts_with("tool_call_")
                                                {
                                                    buffer.id = id;
                                                }
                                                if let Some(function) = tool_call.function {
                                                    if let Some(name) = function.name {
                                                        buffer.name = Some(name);
                                                    }
                                                    if let Some(arguments) = function.arguments
                                                        && !arguments.is_empty()
                                                    {
                                                        buffer.push_args(&arguments);
                                                        yield LlmEvent::ToolCallDelta {
                                                            id: buffer.id.clone(),
                                                            name: buffer.name.clone(),
                                                            args_delta: arguments,
                                                        };
                                                    }
                                                }
                                            }
                                        }
                                    }

                                    if let Some(finish_reason) = choice.finish_reason {
                                        let stop_reason = match finish_reason.as_str() {
                                            "tool_calls" => StopReason::ToolUse,
                                            "length" => StopReason::MaxTokens,
                                            "content_filter" => StopReason::ContentFilter,
                                            _ => StopReason::EndTurn,
                                        };
                                        if matches!(stop_reason, StopReason::ToolUse) {
                                            for buffer in tool_buffers.values() {
                                                if let Some(tool_call) = buffer.try_complete() {
                                                    yield LlmEvent::ToolCallComplete {
                                                        id: tool_call.id,
                                                        name: tool_call.name,
                                                        args: tool_call.args,
                                                        meta: None,
                                                    };
                                                }
                                            }
                                        }
                                        if !reasoning_text.is_empty() {
                                            yield LlmEvent::ReasoningComplete {
                                                text: std::mem::take(&mut reasoning_text),
                                                meta: None,
                                            };
                                        }
                                        if !done_emitted {
                                            done_emitted = true;
                                            yield LlmEvent::Done {
                                                outcome: LlmDoneOutcome::Success { stop_reason },
                                            };
                                        }
                                    }
                                }
                            }
                        }
                    }

                    if !buffer.trim().is_empty() {
                        Err::<(), _>(LlmError::IncompleteResponse {
                            message: format!(
                                "chat completions stream ended with an incomplete SSE buffer: {}",
                                buffer.trim()
                            ),
                        })?;
                    }
                    if !reasoning_text.is_empty() {
                        yield LlmEvent::ReasoningComplete {
                            text: reasoning_text,
                            meta: None,
                        };
                    }
                    if !done_emitted {
                        yield LlmEvent::Done {
                            outcome: LlmDoneOutcome::Success {
                                stop_reason: StopReason::EndTurn,
                            },
                        };
                    }
                });

                streaming::ensure_terminal_done(inner)
            }
        }
    }

    fn provider(&self) -> &'static str {
        "self_hosted"
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        let response = self
            .apply_auth(
                self.http.get(format!("{}/models", self.base_url)),
                "application/json",
            )
            .send()
            .await
            .map_err(|e| LlmError::Unknown {
                message: e.to_string(),
            })?;
        let status = response.status().as_u16();
        if (200..=299).contains(&status) {
            Ok(())
        } else {
            let headers = response.headers().clone();
            let text = response.text().await.unwrap_or_default();
            Err(LlmError::from_http_response(status, text, &headers))
        }
    }

    fn compile_schema(&self, output_schema: &OutputSchema) -> Result<CompiledSchema, SchemaError> {
        let mut schema = output_schema.schema.as_value().clone();
        if output_schema.strict {
            ensure_additional_properties_false(&mut schema);
        }
        Ok(CompiledSchema {
            schema,
            warnings: Vec::new(),
        })
    }
}

#[derive(Debug, Deserialize)]
struct ChatCompletionsChunk {
    choices: Vec<ChatChoice>,
    #[serde(default)]
    usage: Option<ChatUsage>,
}

#[derive(Debug, Deserialize)]
struct ChatChoice {
    #[serde(default)]
    delta: Option<ChatDelta>,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ChatDelta {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    reasoning: Option<String>,
    #[serde(default)]
    reasoning_content: Option<String>,
    #[serde(default)]
    thinking: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<ChatToolCallDelta>>,
}

#[derive(Debug, Deserialize)]
struct ChatToolCallDelta {
    #[serde(default)]
    index: Option<usize>,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    function: Option<ChatFunctionDelta>,
}

#[derive(Debug, Deserialize)]
struct ChatFunctionDelta {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ChatUsage {
    #[serde(default)]
    prompt_tokens: Option<u64>,
    #[serde(default)]
    completion_tokens: Option<u64>,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use axum::{
        Json, Router,
        extract::{Request, State},
        response::IntoResponse,
        routing::post,
    };
    use meerkat_core::UserMessage;
    use std::sync::{Arc, Mutex};
    use tokio::net::TcpListener;

    async fn chat_sse(State(payload): State<String>) -> impl IntoResponse {
        ([("content-type", "text/event-stream")], payload)
    }

    #[derive(Clone)]
    struct ResponsesStubState {
        payload: String,
        auth_headers: Arc<Mutex<Vec<Option<String>>>>,
    }

    async fn responses_sse(
        State(state): State<ResponsesStubState>,
        request: Request,
    ) -> impl IntoResponse {
        let auth = request
            .headers()
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            .map(std::string::ToString::to_string);
        state
            .auth_headers
            .lock()
            .expect("auth header capture lock")
            .push(auth);
        ([("content-type", "text/event-stream")], state.payload)
    }

    async fn models() -> impl IntoResponse {
        Json(serde_json::json!({"data": []}))
    }

    async fn spawn_chat_stub_server(payload: String) -> (String, tokio::task::JoinHandle<()>) {
        let app = Router::new()
            .route("/v1/chat/completions", post(chat_sse))
            .route("/v1/models", axum::routing::get(models))
            .with_state(payload);
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test server");
        let addr = listener.local_addr().expect("local addr");
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve test server");
        });
        (format!("http://{addr}/v1"), handle)
    }

    async fn spawn_responses_stub_server(
        payload: String,
    ) -> (
        String,
        Arc<Mutex<Vec<Option<String>>>>,
        tokio::task::JoinHandle<()>,
    ) {
        let auth_headers = Arc::new(Mutex::new(Vec::new()));
        let app = Router::new()
            .route("/v1/responses", post(responses_sse))
            .route("/v1/models", axum::routing::get(models))
            .with_state(ResponsesStubState {
                payload,
                auth_headers: Arc::clone(&auth_headers),
            });
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test server");
        let addr = listener.local_addr().expect("local addr");
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve test server");
        });
        (format!("http://{addr}/v1"), auth_headers, handle)
    }

    #[tokio::test]
    async fn chat_completions_stream_accumulates_tool_calls() {
        let payload = concat!(
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"function\":{\"name\":\"read_file\",\"arguments\":\"{\\\"path\\\":\"}}]}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"\\\"/tmp/a\\\"}\"}}]}}]}\n\n",
            "data: {\"choices\":[{\"finish_reason\":\"tool_calls\"}],\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":4}}\n\n",
            "data: [DONE]\n\n"
        )
        .to_string();
        let (base_url, handle) = spawn_chat_stub_server(payload).await;
        let client = OpenAiCompatibleClient::new(
            OpenAiCompatibleMode::ChatCompletions,
            "remote-model".to_string(),
            base_url,
            None,
            true,
            false,
            false,
        );
        let request = LlmRequest::new(
            "gemma-4-31b",
            vec![Message::User(UserMessage::text("hello".to_string()))],
        );

        let events: Vec<_> = client.stream(&request).collect().await;
        let mut saw_complete = false;
        let mut saw_done = false;
        for event in events {
            let event = event.expect("event");
            match event {
                LlmEvent::ToolCallComplete { id, name, args, .. } => {
                    saw_complete = true;
                    assert_eq!(id, "call_1");
                    assert_eq!(name, "read_file");
                    assert_eq!(args["path"], "/tmp/a");
                }
                LlmEvent::Done { outcome } => {
                    saw_done = true;
                    assert!(matches!(
                        outcome,
                        LlmDoneOutcome::Success {
                            stop_reason: StopReason::ToolUse
                        }
                    ));
                }
                _ => {}
            }
        }
        assert!(saw_complete);
        assert!(saw_done);
        handle.abort();
    }

    #[tokio::test]
    async fn chat_completions_stream_emits_reasoning_events() {
        let payload = concat!(
            "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"Let me think. \"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"Need one more step.\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"Final answer\"},\"finish_reason\":\"stop\"}]}\n\n",
            "data: [DONE]\n\n"
        )
        .to_string();
        let (base_url, handle) = spawn_chat_stub_server(payload).await;
        let client = OpenAiCompatibleClient::new(
            OpenAiCompatibleMode::ChatCompletions,
            "remote-model".to_string(),
            base_url,
            None,
            true,
            true,
            true,
        );
        let request = LlmRequest::new(
            "gemma-4-31b",
            vec![Message::User(UserMessage::text("hello".to_string()))],
        );

        let events: Vec<_> = client.stream(&request).collect().await;
        let mut reasoning_deltas = Vec::new();
        let mut reasoning_complete = None;
        for event in events {
            match event.expect("event") {
                LlmEvent::ReasoningDelta { delta } => reasoning_deltas.push(delta),
                LlmEvent::ReasoningComplete { text, .. } => reasoning_complete = Some(text),
                _ => {}
            }
        }

        assert_eq!(
            reasoning_deltas,
            vec![
                "Let me think. ".to_string(),
                "Need one more step.".to_string()
            ]
        );
        assert_eq!(
            reasoning_complete,
            Some("Let me think. Need one more step.".to_string())
        );
        handle.abort();
    }

    #[test]
    fn build_chat_completions_body_preserves_reasoning_overrides() {
        let client = OpenAiCompatibleClient::new(
            OpenAiCompatibleMode::ChatCompletions,
            "remote-model".to_string(),
            "http://localhost:11434/v1".to_string(),
            None,
            true,
            true,
            true,
        );
        let request = LlmRequest::new(
            "gemma-4-31b",
            vec![Message::User(UserMessage::text("hello".to_string()))],
        )
        .with_openai_tag_merge(|t| {
            t.reasoning_effort =
                Some(meerkat_core::lifecycle::run_primitive::ReasoningEffort::Medium);
            t.chat_template_kwargs = Some(
                meerkat_core::lifecycle::run_primitive::OpaqueProviderBody::from_value(
                    &serde_json::json!({"enable_thinking": true}),
                ),
            );
            t.thinking = Some(
                meerkat_core::lifecycle::run_primitive::OpaqueProviderBody::from_value(
                    &serde_json::json!({"type": "enabled"}),
                ),
            );
        });

        let body = client
            .build_chat_completions_body(&request)
            .expect("body should build");

        assert_eq!(body["reasoning"]["effort"], "medium");
        assert_eq!(body["reasoning_effort"], "medium");
        assert_eq!(body["chat_template_kwargs"]["enable_thinking"], true);
        assert_eq!(body["thinking"]["type"], "enabled");
    }

    #[tokio::test]
    async fn responses_mode_uses_single_v1_prefix_and_omits_auth_when_unset() {
        let payload = concat!(
            "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"output\":[{\"type\":\"message\",\"content\":[{\"type\":\"output_text\",\"text\":\"Hello\"}]}],\"usage\":{\"input_tokens\":10,\"output_tokens\":5}}}\n\n",
            "data: {\"type\":\"response.done\",\"response\":{\"status\":\"completed\",\"output\":[],\"usage\":{\"input_tokens\":10,\"output_tokens\":5}}}\n\n"
        )
        .to_string();
        let (base_url, auth_headers, handle) = spawn_responses_stub_server(payload).await;
        let client = OpenAiCompatibleClient::new(
            OpenAiCompatibleMode::Responses,
            "gemma4:e2b".to_string(),
            base_url,
            None,
            true,
            true,
            true,
        );
        let request = LlmRequest::new(
            "gemma-4-e2b",
            vec![Message::User(UserMessage::text("hello".to_string()))],
        );

        let events: Vec<_> = client.stream(&request).collect().await;
        assert!(
            events.iter().all(Result::is_ok),
            "responses mode should succeed against a single /v1/responses endpoint"
        );
        let auth_headers = auth_headers.lock().expect("auth header capture lock");
        assert_eq!(auth_headers.len(), 1);
        assert_eq!(auth_headers[0], None);
        handle.abort();
    }

    #[test]
    fn request_with_remote_model_preserves_self_hosted_capabilities_for_delegate() {
        let client = OpenAiCompatibleClient::new(
            OpenAiCompatibleMode::Responses,
            "gemma4:e2b".to_string(),
            "http://localhost:11434/v1".to_string(),
            None,
            true,
            true,
            true,
        );
        let request = LlmRequest::new(
            "gemma-4-e2b",
            vec![Message::User(UserMessage::text("hello".to_string()))],
        );

        let translated = client.request_with_remote_model(&request);

        assert_eq!(translated.model, "gemma4:e2b");
        let tag = match translated.provider_params.as_ref() {
            Some(meerkat_core::lifecycle::run_primitive::ProviderTag::OpenAi(t)) => t,
            other => unreachable!("expected OpenAi variant, got {other:?}"),
        };
        assert_eq!(tag.supports_temperature_override, Some(true));
        assert_eq!(tag.supports_reasoning_override, Some(true));
    }

    #[test]
    fn parse_chat_completions_line_accepts_sse_data_without_space() {
        let line = r#"data:{"choices":[{"delta":{"content":"Hello"}}]}"#;
        let chunk =
            OpenAiCompatibleClient::parse_chat_completions_line(line).expect("line should parse");
        assert!(chunk.is_some());
    }

    #[test]
    fn ensure_additional_properties_false_recurses_into_nested_objects() {
        let mut value = serde_json::json!({
            "type": "object",
            "properties": {
                "outer": {
                    "type": "object",
                    "properties": {
                        "inner": {
                            "type": "object",
                            "properties": {}
                        }
                    }
                }
            }
        });

        ensure_additional_properties_false(&mut value);

        assert_eq!(value["additionalProperties"], Value::Bool(false));
        assert_eq!(
            value["properties"]["outer"]["additionalProperties"],
            Value::Bool(false)
        );
        assert_eq!(
            value["properties"]["outer"]["properties"]["inner"]["additionalProperties"],
            Value::Bool(false)
        );
    }
}
