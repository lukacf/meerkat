//! OpenAI-compatible client for self-hosted endpoints.

use async_trait::async_trait;
use futures::StreamExt;
use meerkat_core::schema::{CompiledSchema, SchemaError};
use meerkat_core::{
    AssistantBlock, ContentBlock, ImageData, Message, OutputSchema, StopReason, SystemNoticeBlock,
    SystemNoticeMessage, Usage,
};
use meerkat_llm_core::LlmError;
use meerkat_llm_core::{
    LlmClient, LlmDoneOutcome, LlmEvent, LlmRequest, LlmStream, ToolCallBuffer,
};
use meerkat_llm_core::{http, streaming};
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;

use crate::client::{OpenAiReplayProjectionMode, project_openai_replay_messages_for_capabilities};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenAiCompatibleMode {
    Responses,
    ChatCompletions,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct OpenAiCompatibleClientOptions {
    pub supports_temperature: bool,
    pub supports_thinking: bool,
    pub supports_reasoning: bool,
    pub supports_image_tool_results: bool,
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
    supports_image_tool_results: bool,
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
        Self::new_with_options(
            mode,
            remote_model,
            base_url,
            bearer_token,
            OpenAiCompatibleClientOptions {
                supports_temperature,
                supports_thinking,
                supports_reasoning,
                supports_image_tool_results: false,
            },
        )
    }

    pub fn new_with_options(
        mode: OpenAiCompatibleMode,
        remote_model: String,
        base_url: String,
        bearer_token: Option<String>,
        options: OpenAiCompatibleClientOptions,
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
            supports_temperature: options.supports_temperature,
            supports_thinking: options.supports_thinking,
            supports_reasoning: options.supports_reasoning,
            supports_image_tool_results: options.supports_image_tool_results,
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

    fn content_block_to_chat_part(block: &ContentBlock) -> Value {
        match block {
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
        }
    }

    fn system_notice_chat_content(notice: &SystemNoticeMessage) -> Value {
        let rendered = notice.model_projection_text();
        let mut parts = Vec::new();
        if !rendered.trim().is_empty() {
            parts.push(serde_json::json!({
                "type": "text",
                "text": rendered
            }));
        }

        for block in &notice.blocks {
            match block {
                SystemNoticeBlock::Comms { content, .. }
                | SystemNoticeBlock::ExternalEvent { content, .. } => {
                    for content_block in content {
                        if !matches!(content_block, ContentBlock::Text { .. }) {
                            parts.push(Self::content_block_to_chat_part(content_block));
                        }
                    }
                }
                _ => {}
            }
        }

        match parts.as_slice() {
            [] => Value::String(String::new()),
            [only] if only.get("type").and_then(Value::as_str) == Some("text") => only
                .get("text")
                .cloned()
                .unwrap_or_else(|| Value::String(String::new())),
            _ => Value::Array(parts),
        }
    }

    fn build_chat_completions_body(&self, request: &LlmRequest) -> Result<Value, LlmError> {
        let tag = crate::client::openai_tag(request);
        let author_explicit_breakpoints = tag.is_some_and(|tag| {
            tag.prompt_cache_enabled != Some(false)
                && tag.prompt_cache_options.is_some_and(|options| {
                    options.mode
                        == Some(
                            meerkat_core::model_profile::capabilities::OpenAiPromptCacheMode::Explicit,
                        )
                })
        });
        let messages = Self::convert_to_chat_messages_with_cache(
            &request.messages,
            author_explicit_breakpoints,
        )?;
        let mut body = serde_json::json!({
            "model": self.remote_model,
            "messages": messages,
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

        if let Some(tag) = tag {
            if tag.prompt_cache_enabled == Some(false) {
                // Stable Meerkat opt-out. This remains authoritative over
                // compatible Chat Completions breakpoint authoring.
                body["prompt_cache_options"] = serde_json::json!({"mode": "explicit"});
            } else {
                if let Some(key) = tag.prompt_cache_key.as_ref() {
                    body["prompt_cache_key"] = Value::String(key.clone());
                }
                if let Some(retention) = tag.prompt_cache_retention {
                    body["prompt_cache_retention"] = Value::String(
                        match retention {
                            meerkat_core::lifecycle::run_primitive::OpenAiPromptCacheRetention::InMemory => "in_memory",
                            meerkat_core::lifecycle::run_primitive::OpenAiPromptCacheRetention::TwentyFourHours => "24h",
                        }
                        .to_string(),
                    );
                }
                if let Some(options) = tag.prompt_cache_options {
                    body["prompt_cache_options"] =
                        serde_json::to_value(options).map_err(|error| {
                            LlmError::InvalidRequest {
                                message: format!("invalid prompt_cache_options: {error}"),
                            }
                        })?;
                } else if tag.prompt_cache_enabled == Some(true) {
                    body["prompt_cache_options"] = serde_json::json!({"mode": "implicit"});
                }
            }
            if self.supports_reasoning {
                if let Some(reasoning) = tag.reasoning.as_ref() {
                    let v = reasoning.as_value();
                    if v.is_object() {
                        body["reasoning"] = v;
                    }
                }
                if let Some(effort) = tag.reasoning_effort {
                    let s = effort.as_legacy_str();
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

    fn author_chat_content_breakpoint(content: &mut Value) -> bool {
        if let Some(parts) = content.as_array_mut() {
            for part in parts.iter_mut().rev() {
                if matches!(
                    part.get("type").and_then(Value::as_str),
                    Some("text" | "image_url" | "input_audio" | "file" | "refusal")
                ) {
                    part["prompt_cache_breakpoint"] = serde_json::json!({"mode": "explicit"});
                    return true;
                }
            }
            return false;
        }
        if let Some(text) = content
            .as_str()
            .filter(|text| !text.is_empty())
            .map(str::to_owned)
        {
            *content = serde_json::json!([{
                "type": "text",
                "text": text,
                "prompt_cache_breakpoint": {"mode": "explicit"}
            }]);
            return true;
        }
        false
    }

    #[cfg(test)]
    fn convert_to_chat_messages(messages: &[Message]) -> Result<Vec<Value>, LlmError> {
        Self::convert_to_chat_messages_with_cache(messages, false)
    }

    fn convert_to_chat_messages_with_cache(
        messages: &[Message],
        author_explicit_breakpoints: bool,
    ) -> Result<Vec<Value>, LlmError> {
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
                        "content": Self::system_notice_chat_content(notice)
                    }));
                    if author_explicit_breakpoints && let Some(message) = out.last_mut() {
                        Self::author_chat_content_breakpoint(&mut message["content"]);
                    }
                }
                Message::User(user) => {
                    if meerkat_core::has_non_text_content(&user.content) {
                        let content: Vec<Value> = user
                            .content
                            .iter()
                            .map(Self::content_block_to_chat_part)
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
                    if author_explicit_breakpoints && let Some(message) = out.last_mut() {
                        Self::author_chat_content_breakpoint(&mut message["content"]);
                    }
                }
                Message::BlockAssistant(assistant) => {
                    let mut text_parts = Vec::new();
                    let mut tool_calls = Vec::new();
                    for block in &assistant.blocks {
                        match block {
                            AssistantBlock::Text { text, .. }
                            | AssistantBlock::Transcript { text, .. } => {
                                // Display text and spoken transcripts both
                                // replay as plain assistant text on the
                                // Chat Completions–compatible surface.
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
                    let first_result_message = out.len();
                    for result in results {
                        out.push(serde_json::json!({
                            "role": "tool",
                            "tool_call_id": result.tool_use_id,
                            "content": result.text_content()
                        }));
                    }
                    if author_explicit_breakpoints {
                        for message in out[first_result_message..].iter_mut().rev() {
                            if Self::author_chat_content_breakpoint(&mut message["content"]) {
                                break;
                            }
                        }
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
    fn system_message_wire_capability(&self) -> meerkat_core::SystemMessageWireCapability {
        meerkat_core::SystemMessageWireCapability::Interleaved
    }

    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        let mode = match self.mode {
            OpenAiCompatibleMode::Responses => OpenAiReplayProjectionMode::Responses,
            OpenAiCompatibleMode::ChatCompletions => OpenAiReplayProjectionMode::ChatCompletions,
        };
        project_openai_replay_messages_for_capabilities(
            messages,
            mode,
            self.supports_image_tool_results,
        )
    }

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
                    let mut translated = self.request_with_remote_model(request);
                    translated.messages = self.project_replay_messages(&request.messages)?;
                    let mut stream = delegate.stream(&translated);
                    while let Some(event) = stream.next().await {
                        yield event?;
                    }
                });
                streaming::ensure_terminal_done(inner)
            }
            OpenAiCompatibleMode::ChatCompletions => {
                let inner: LlmStream<'a> = Box::pin(async_stream::try_stream! {
                    let mut projected_request = request.clone();
                    projected_request.messages = self.project_replay_messages(&request.messages)?;
                    let body = self.build_chat_completions_body(&projected_request)?;
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
                                        cache_creation_tokens: event_usage
                                            .prompt_tokens_details
                                            .as_ref()
                                            .and_then(|details| details.cache_write_tokens),
                                        cache_read_tokens: event_usage
                                            .prompt_tokens_details
                                            .as_ref()
                                            .and_then(|details| details.cached_tokens),
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
                                                if let Some(tool_call) = buffer.try_complete()? {
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

    fn provider(&self) -> meerkat_core::Provider {
        meerkat_core::Provider::SelfHosted
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
    #[serde(default)]
    prompt_tokens_details: Option<ChatPromptTokensDetails>,
}

#[derive(Debug, Deserialize)]
struct ChatPromptTokensDetails {
    #[serde(default)]
    cached_tokens: Option<u64>,
    #[serde(default)]
    cache_write_tokens: Option<u64>,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use axum::{
        Json, Router,
        extract::{Request, State},
        response::IntoResponse,
        routing::post,
    };
    use meerkat_core::{
        BlockAssistantMessage, ContentBlock, ImageData, StopReason, ToolResult, UserMessage,
    };
    use std::sync::{Arc, Mutex};
    use tokio::net::TcpListener;

    fn options(
        supports_temperature: bool,
        supports_thinking: bool,
        supports_reasoning: bool,
        supports_image_tool_results: bool,
    ) -> OpenAiCompatibleClientOptions {
        OpenAiCompatibleClientOptions {
            supports_temperature,
            supports_thinking,
            supports_reasoning,
            supports_image_tool_results,
        }
    }

    async fn chat_sse(State(payload): State<String>) -> impl IntoResponse {
        ([("content-type", "text/event-stream")], payload)
    }

    #[derive(Clone)]
    struct ResponsesStubState {
        payload: String,
        auth_headers: Arc<Mutex<Vec<Option<String>>>>,
        request_bodies: Arc<Mutex<Vec<Value>>>,
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
        if let Ok(bytes) = to_bytes(request.into_body(), usize::MAX).await
            && let Ok(value) = serde_json::from_slice::<Value>(&bytes)
        {
            state
                .request_bodies
                .lock()
                .expect("request body capture lock")
                .push(value);
        }
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
        Arc<Mutex<Vec<Value>>>,
        tokio::task::JoinHandle<()>,
    ) {
        let auth_headers = Arc::new(Mutex::new(Vec::new()));
        let request_bodies = Arc::new(Mutex::new(Vec::new()));
        let app = Router::new()
            .route("/v1/responses", post(responses_sse))
            .route("/v1/models", axum::routing::get(models))
            .with_state(ResponsesStubState {
                payload,
                auth_headers: Arc::clone(&auth_headers),
                request_bodies: Arc::clone(&request_bodies),
            });
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test server");
        let addr = listener.local_addr().expect("local addr");
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve test server");
        });
        (
            format!("http://{addr}/v1"),
            auth_headers,
            request_bodies,
            handle,
        )
    }

    #[test]
    fn chat_completions_preserves_ordered_system_messages_exactly_in_place() {
        let client = OpenAiCompatibleClient::new(
            OpenAiCompatibleMode::ChatCompletions,
            "test-model".to_string(),
            "http://localhost".to_string(),
            None,
            true,
            false,
            false,
        );
        assert_eq!(
            client.system_message_wire_capability(),
            meerkat_core::SystemMessageWireCapability::Interleaved
        );
        let messages = vec![
            Message::User(UserMessage::text("work")),
            Message::System(meerkat_core::SystemMessage::new("")),
            Message::User(UserMessage::text("continue")),
            Message::System(meerkat_core::SystemMessage::new(" \t ")),
            Message::System(meerkat_core::SystemMessage::new("duplicate")),
            Message::System(meerkat_core::SystemMessage::new("duplicate")),
        ];
        let original = messages.clone();

        let projected = OpenAiCompatibleClient::convert_to_chat_messages(&messages)
            .expect("convert chat messages");

        assert_eq!(messages, original);
        assert_eq!(projected.len(), 6);
        assert_eq!(projected[0]["role"], "user");
        assert_eq!(projected[0]["content"], "work");
        assert_eq!(projected[1]["role"], "system");
        assert_eq!(projected[1]["content"], "");
        assert_eq!(projected[2]["role"], "user");
        assert_eq!(projected[2]["content"], "continue");
        assert_eq!(projected[3]["role"], "system");
        assert_eq!(projected[3]["content"], " \t ");
        assert_eq!(projected[4]["role"], "system");
        assert_eq!(projected[4]["content"], "duplicate");
        assert_eq!(projected[5]["role"], "system");
        assert_eq!(projected[5]["content"], "duplicate");
    }

    #[test]
    fn chat_completions_system_notice_with_image_emits_typed_image_part() {
        use meerkat_core::{ContentBlock, ImageData, SystemNoticeKind, SystemNoticeMessage};

        let messages = OpenAiCompatibleClient::convert_to_chat_messages(&[Message::SystemNotice(
            SystemNoticeMessage::with_block(
                SystemNoticeKind::ExternalEvent,
                None,
                SystemNoticeBlock::ExternalEvent {
                    source: "console".to_string(),
                    event_type: "operator_message".to_string(),
                    summary: None,
                    body: Some("inspect this".to_string()),
                    payload: None,
                    content: vec![ContentBlock::Image {
                        media_type: "image/png".to_string(),
                        data: ImageData::Inline {
                            data: "iVBOR...".to_string(),
                        },
                    }],
                },
            ),
        )])
        .expect("convert chat messages");

        assert_eq!(messages[0]["role"], "user");
        let content = messages[0]["content"].as_array().expect("content array");
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[1]["type"], "image_url");
        assert_eq!(
            content[1]["image_url"]["url"],
            "data:image/png;base64,iVBOR..."
        );
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
        let client = OpenAiCompatibleClient::new_with_options(
            OpenAiCompatibleMode::ChatCompletions,
            "remote-model".to_string(),
            base_url,
            None,
            options(true, false, false, false),
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
    async fn chat_completions_stream_malformed_tool_args_fail_closed() {
        // A tool call whose accumulated argument JSON is truncated must NOT be
        // silently dropped and laundered into Done{Success}. It must surface as
        // a terminal Done{Error{StreamParseError}}.
        let payload = concat!(
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"function\":{\"name\":\"read_file\",\"arguments\":\"{\\\"path\\\":\"}}]}}]}\n\n",
            "data: {\"choices\":[{\"finish_reason\":\"tool_calls\"}],\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":4}}\n\n",
            "data: [DONE]\n\n"
        )
        .to_string();
        let (base_url, handle) = spawn_chat_stub_server(payload).await;
        let client = OpenAiCompatibleClient::new_with_options(
            OpenAiCompatibleMode::ChatCompletions,
            "remote-model".to_string(),
            base_url,
            None,
            options(true, false, false, false),
        );
        let request = LlmRequest::new(
            "gemma-4-31b",
            vec![Message::User(UserMessage::text("hello".to_string()))],
        );

        let events: Vec<_> = client.stream(&request).collect().await;
        let mut saw_complete = false;
        let mut saw_success_done = false;
        let mut terminal_error = None;
        for event in events {
            match event {
                Ok(LlmEvent::ToolCallComplete { .. }) => saw_complete = true,
                Ok(LlmEvent::Done {
                    outcome: LlmDoneOutcome::Error { error },
                }) => terminal_error = Some(error),
                Ok(LlmEvent::Done {
                    outcome: LlmDoneOutcome::Success { .. },
                }) => saw_success_done = true,
                Err(error) => terminal_error = Some(error),
                _ => {}
            }
        }
        assert!(
            !saw_complete,
            "malformed tool args must not produce a ToolCallComplete"
        );
        assert!(
            !saw_success_done,
            "malformed tool args must not be laundered into a successful Done"
        );
        assert!(
            matches!(terminal_error, Some(LlmError::StreamParseError { .. })),
            "expected terminal StreamParseError, got {terminal_error:?}"
        );
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
        let client = OpenAiCompatibleClient::new_with_options(
            OpenAiCompatibleMode::ChatCompletions,
            "remote-model".to_string(),
            base_url,
            None,
            options(true, true, true, false),
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
        let client = OpenAiCompatibleClient::new_with_options(
            OpenAiCompatibleMode::ChatCompletions,
            "remote-model".to_string(),
            "http://localhost:11434/v1".to_string(),
            None,
            options(true, true, true, false),
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

    #[test]
    fn build_chat_completions_body_preserves_xhigh_reasoning_effort() {
        let client = OpenAiCompatibleClient::new_with_options(
            OpenAiCompatibleMode::ChatCompletions,
            "remote-model".to_string(),
            "http://localhost:11434/v1".to_string(),
            None,
            options(true, true, true, false),
        );
        let request = LlmRequest::new(
            "gemma-4-31b",
            vec![Message::User(UserMessage::text("hello".to_string()))],
        )
        .with_openai_tag_merge(|t| {
            t.reasoning_effort =
                Some(meerkat_core::lifecycle::run_primitive::ReasoningEffort::XHigh);
        });

        let body = client
            .build_chat_completions_body(&request)
            .expect("body should build");

        assert_eq!(body["reasoning"]["effort"], "xhigh");
        assert_eq!(body["reasoning_effort"], "xhigh");
    }

    #[test]
    fn chat_completions_lowers_cache_policy_key_and_durable_disable() {
        use meerkat_core::lifecycle::run_primitive::{
            OpenAiPromptCacheOptions, OpenAiPromptCacheRetention,
        };
        use meerkat_core::model_profile::capabilities::{
            OpenAiPromptCacheMode, OpenAiPromptCacheTtl,
        };

        let client = OpenAiCompatibleClient::new_with_options(
            OpenAiCompatibleMode::ChatCompletions,
            "gpt-5.6-sol".to_string(),
            "http://localhost:11434/v1".to_string(),
            None,
            options(true, true, true, false),
        );
        let request = LlmRequest::new(
            "gpt-5.6-sol",
            vec![Message::User(UserMessage::text("hello".to_string()))],
        )
        .with_openai_tag_merge(|tag| {
            tag.prompt_cache_enabled = Some(true);
            tag.prompt_cache_key = Some("identity:parent-1".to_string());
            tag.prompt_cache_options = Some(OpenAiPromptCacheOptions {
                mode: Some(OpenAiPromptCacheMode::Implicit),
                ttl: Some(OpenAiPromptCacheTtl::ThirtyMinutes),
            });
        });
        let body = client
            .build_chat_completions_body(&request)
            .expect("cache-enabled body");
        assert_eq!(body["prompt_cache_key"], "identity:parent-1");
        assert_eq!(
            body["prompt_cache_options"],
            serde_json::json!({"mode":"implicit","ttl":"30m"})
        );
        assert!(
            !body.to_string().contains("prompt_cache_breakpoint"),
            "implicit mode must leave breakpoint placement to OpenAI"
        );

        let disabled = request.with_openai_tag_merge(|tag| {
            tag.prompt_cache_enabled = Some(false);
            tag.prompt_cache_retention = Some(OpenAiPromptCacheRetention::InMemory);
        });
        let disabled_body = client
            .build_chat_completions_body(&disabled)
            .expect("durable cache opt-out body");
        assert!(disabled_body.get("prompt_cache_key").is_none());
        assert!(disabled_body.get("prompt_cache_retention").is_none());
        assert_eq!(
            disabled_body["prompt_cache_options"],
            serde_json::json!({"mode":"explicit"})
        );
    }

    #[test]
    fn chat_completions_explicit_breakpoints_are_append_monotone() {
        use meerkat_core::lifecycle::run_primitive::OpenAiPromptCacheOptions;
        use meerkat_core::model_profile::capabilities::OpenAiPromptCacheMode;

        let client = OpenAiCompatibleClient::new_with_options(
            OpenAiCompatibleMode::ChatCompletions,
            "gpt-5.6-sol".to_string(),
            "http://localhost:11434/v1".to_string(),
            None,
            options(true, true, true, false),
        );
        let first_request = LlmRequest::new(
            "gpt-5.6-sol",
            vec![Message::User(UserMessage::text("hello".to_string()))],
        )
        .with_openai_tag_merge(|tag| {
            tag.prompt_cache_options = Some(OpenAiPromptCacheOptions {
                mode: Some(OpenAiPromptCacheMode::Explicit),
                ttl: None,
            });
        });
        let first_body = client
            .build_chat_completions_body(&first_request)
            .expect("first explicit body");
        let mut second_request = first_request;
        second_request
            .messages
            .push(Message::User(UserMessage::text("again".to_string())));
        let second_body = client
            .build_chat_completions_body(&second_request)
            .expect("growing explicit body");

        assert_eq!(first_body["messages"][0], second_body["messages"][0]);
        assert_eq!(
            first_body["messages"][0]["content"][0]["prompt_cache_breakpoint"],
            serde_json::json!({"mode":"explicit"})
        );
        assert_eq!(
            second_body["messages"][1]["content"][0]["prompt_cache_breakpoint"],
            serde_json::json!({"mode":"explicit"})
        );
    }

    #[test]
    fn chat_explicit_handles_no_boundary_and_marks_final_multimodal_part() {
        use meerkat_core::lifecycle::run_primitive::OpenAiPromptCacheOptions;
        use meerkat_core::model_profile::capabilities::OpenAiPromptCacheMode;

        let client = OpenAiCompatibleClient::new_with_options(
            OpenAiCompatibleMode::ChatCompletions,
            "gpt-5.6-sol".to_string(),
            "http://localhost:11434/v1".to_string(),
            None,
            options(true, true, true, false),
        );
        let explicit = |messages| {
            LlmRequest::new("gpt-5.6-sol", messages).with_openai_tag_merge(|tag| {
                tag.prompt_cache_options = Some(OpenAiPromptCacheOptions {
                    mode: Some(OpenAiPromptCacheMode::Explicit),
                    ttl: None,
                });
            })
        };

        let system_only = client
            .build_chat_completions_body(&explicit(vec![Message::System(
                meerkat_core::SystemMessage::new("system only"),
            )]))
            .expect("explicit mode without a markable boundary is valid");
        assert!(!system_only.to_string().contains("prompt_cache_breakpoint"));

        let multimodal = client
            .build_chat_completions_body(&explicit(vec![Message::User(UserMessage::with_blocks(
                vec![
                    ContentBlock::Text {
                        text: "describe this".to_string(),
                    },
                    ContentBlock::Image {
                        media_type: "image/png".to_string(),
                        data: ImageData::Inline {
                            data: "iVBOR...".to_string(),
                        },
                    },
                ],
            ))]))
            .expect("multimodal explicit body");
        assert!(
            multimodal["messages"][0]["content"][0]
                .get("prompt_cache_breakpoint")
                .is_none()
        );
        assert_eq!(
            multimodal["messages"][0]["content"][1]["prompt_cache_breakpoint"],
            serde_json::json!({"mode": "explicit"}),
            "the marker belongs on the final supported multimodal part"
        );
    }

    #[test]
    fn chat_explicit_tool_results_mark_last_cacheable_result() {
        use meerkat_core::lifecycle::run_primitive::OpenAiPromptCacheOptions;
        use meerkat_core::model_profile::capabilities::OpenAiPromptCacheMode;

        let client = OpenAiCompatibleClient::new_with_options(
            OpenAiCompatibleMode::ChatCompletions,
            "gpt-5.6-sol".to_string(),
            "http://localhost:11434/v1".to_string(),
            None,
            options(true, true, true, false),
        );
        let request = LlmRequest::new(
            "gpt-5.6-sol",
            vec![
                Message::User(UserMessage::text("run tools".to_string())),
                Message::tool_results(vec![
                    ToolResult::new(
                        "call-1".to_string(),
                        "large stable output".to_string(),
                        false,
                    ),
                    ToolResult::new("call-2".to_string(), String::new(), false),
                ]),
            ],
        )
        .with_openai_tag_merge(|tag| {
            tag.prompt_cache_options = Some(OpenAiPromptCacheOptions {
                mode: Some(OpenAiPromptCacheMode::Explicit),
                ttl: None,
            });
        });

        let body = client
            .build_chat_completions_body(&request)
            .expect("parallel tool results");
        assert_eq!(
            body["messages"][1]["content"][0]["prompt_cache_breakpoint"],
            serde_json::json!({"mode":"explicit"})
        );
        assert!(
            body["messages"][2]["content"]
                .to_string()
                .find("prompt_cache_breakpoint")
                .is_none()
        );
    }

    #[tokio::test]
    async fn chat_completions_maps_cache_read_and_write_usage() {
        let payload = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":100,\"completion_tokens\":2,\"prompt_tokens_details\":{\"cached_tokens\":80,\"cache_write_tokens\":20}}}\n\n",
            "data: [DONE]\n\n"
        )
        .to_string();
        let (base_url, handle) = spawn_chat_stub_server(payload).await;
        let client = OpenAiCompatibleClient::new_with_options(
            OpenAiCompatibleMode::ChatCompletions,
            "gpt-5.6-sol".to_string(),
            base_url,
            None,
            options(true, true, true, false),
        );
        let request = LlmRequest::new(
            "gpt-5.6-sol",
            vec![Message::User(UserMessage::text("hello".to_string()))],
        );

        let events: Vec<_> = client.stream(&request).collect().await;
        let usage = events
            .into_iter()
            .filter_map(Result::ok)
            .find_map(|event| match event {
                LlmEvent::UsageUpdate { usage } => Some(usage),
                _ => None,
            })
            .expect("usage update");
        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.output_tokens, 2);
        assert_eq!(usage.cache_read_tokens, Some(80));
        assert_eq!(usage.cache_creation_tokens, Some(20));
        handle.abort();
    }

    #[tokio::test]
    async fn responses_mode_uses_single_v1_prefix_and_omits_auth_when_unset() {
        let payload = concat!(
            "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"output\":[{\"type\":\"message\",\"content\":[{\"type\":\"output_text\",\"text\":\"Hello\"}]}],\"usage\":{\"input_tokens\":10,\"output_tokens\":5}}}\n\n",
            "data: {\"type\":\"response.done\",\"response\":{\"status\":\"completed\",\"output\":[],\"usage\":{\"input_tokens\":10,\"output_tokens\":5}}}\n\n"
        )
        .to_string();
        let (base_url, auth_headers, _request_bodies, handle) =
            spawn_responses_stub_server(payload).await;
        let client = OpenAiCompatibleClient::new_with_options(
            OpenAiCompatibleMode::Responses,
            "gemma4:e2b".to_string(),
            base_url,
            None,
            options(true, true, true, true),
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

    #[tokio::test]
    async fn responses_mode_posts_tool_result_images_as_responses_output_parts() {
        let payload = concat!(
            "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"output\":[{\"type\":\"message\",\"content\":[{\"type\":\"output_text\",\"text\":\"ok\"}]}],\"usage\":{\"input_tokens\":10,\"output_tokens\":1}}}\n\n",
            "data: {\"type\":\"response.done\",\"response\":{\"status\":\"completed\",\"output\":[],\"usage\":{\"input_tokens\":10,\"output_tokens\":1}}}\n\n"
        )
        .to_string();
        let (base_url, _auth_headers, request_bodies, handle) =
            spawn_responses_stub_server(payload).await;
        let client = OpenAiCompatibleClient::new_with_options(
            OpenAiCompatibleMode::Responses,
            "gemma4:e2b".to_string(),
            base_url,
            None,
            options(true, true, true, true),
        );
        let request = LlmRequest::new(
            "gemma-4-e2b",
            vec![
                Message::BlockAssistant(BlockAssistantMessage::new(
                    vec![AssistantBlock::ToolUse {
                        id: "call_1".to_string(),
                        name: "screenshot".to_string(),
                        args: serde_json::value::RawValue::from_string("{}".to_string())
                            .expect("valid args"),
                        meta: None,
                    }],
                    StopReason::ToolUse,
                )),
                Message::tool_results(vec![ToolResult::with_blocks(
                    "call_1".to_string(),
                    vec![
                        ContentBlock::Text {
                            text: "screenshot taken".to_string(),
                        },
                        ContentBlock::Image {
                            media_type: "image/png".to_string(),
                            data: ImageData::Inline {
                                data: "iVBOR...".to_string(),
                            },
                        },
                    ],
                    false,
                )]),
            ],
        );

        let events: Vec<_> = client.stream(&request).collect().await;
        assert!(
            events.iter().all(Result::is_ok),
            "responses mode should complete against the stub server"
        );
        let request_bodies = request_bodies.lock().expect("request body capture lock");
        assert_eq!(request_bodies.len(), 1);
        assert_eq!(request_bodies[0]["model"], "gemma4:e2b");
        let input = request_bodies[0]["input"].as_array().expect("input array");
        let tool_output = input
            .iter()
            .find(|item| item.get("type").and_then(Value::as_str) == Some("function_call_output"))
            .expect("function_call_output");
        let output = tool_output["output"].as_array().expect("output array");
        assert_eq!(output[0]["type"], "input_text");
        assert_eq!(output[0]["text"], "screenshot taken");
        assert_eq!(output[1]["type"], "input_image");
        assert_eq!(output[1]["image_url"], "data:image/png;base64,iVBOR...");
        handle.abort();
    }

    #[test]
    fn request_with_remote_model_preserves_self_hosted_capabilities_for_delegate() {
        let client = OpenAiCompatibleClient::new_with_options(
            OpenAiCompatibleMode::Responses,
            "gemma4:e2b".to_string(),
            "http://localhost:11434/v1".to_string(),
            None,
            options(true, true, true, true),
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
    fn responses_mode_replay_preserves_tool_result_images_for_delegate() {
        let client = OpenAiCompatibleClient::new_with_options(
            OpenAiCompatibleMode::Responses,
            "gemma4:e2b".to_string(),
            "http://localhost:11434/v1".to_string(),
            None,
            options(true, true, true, true),
        );
        let messages = vec![
            Message::BlockAssistant(BlockAssistantMessage::new(
                vec![AssistantBlock::ToolUse {
                    id: "call_1".to_string(),
                    name: "screenshot".to_string(),
                    args: serde_json::value::RawValue::from_string("{}".to_string())
                        .expect("valid args"),
                    meta: None,
                }],
                StopReason::ToolUse,
            )),
            Message::tool_results(vec![ToolResult::with_blocks(
                "call_1".to_string(),
                vec![ContentBlock::Image {
                    media_type: "image/png".to_string(),
                    data: ImageData::Inline {
                        data: "iVBOR...".to_string(),
                    },
                }],
                false,
            )]),
        ];

        let projected = client
            .project_replay_messages(&messages)
            .expect("responses projection");
        assert!(
            matches!(&projected[1], Message::ToolResults { .. }),
            "expected tool results"
        );
        let Message::ToolResults { results, .. } = &projected[1] else {
            return;
        };
        assert!(
            results[0].has_images(),
            "compatible Responses mode must keep images for delegate serialization"
        );
    }

    #[test]
    fn responses_mode_replay_text_projects_tool_result_images_when_model_cannot_accept_them() {
        let client = OpenAiCompatibleClient::new_with_options(
            OpenAiCompatibleMode::Responses,
            "gemma4:e2b".to_string(),
            "http://localhost:11434/v1".to_string(),
            None,
            options(true, true, true, false),
        );
        let messages = vec![
            Message::BlockAssistant(BlockAssistantMessage::new(
                vec![AssistantBlock::ToolUse {
                    id: "call_1".to_string(),
                    name: "screenshot".to_string(),
                    args: serde_json::value::RawValue::from_string("{}".to_string())
                        .expect("valid args"),
                    meta: None,
                }],
                StopReason::ToolUse,
            )),
            Message::tool_results(vec![ToolResult::with_blocks(
                "call_1".to_string(),
                vec![ContentBlock::Image {
                    media_type: "image/png".to_string(),
                    data: ImageData::Inline {
                        data: "iVBOR...".to_string(),
                    },
                }],
                false,
            )]),
        ];

        let projected = client
            .project_replay_messages(&messages)
            .expect("responses projection");
        assert!(
            matches!(&projected[1], Message::ToolResults { .. }),
            "expected tool results"
        );
        let Message::ToolResults { results, .. } = &projected[1] else {
            return;
        };
        assert!(
            !results[0].has_images(),
            "incapable compatible Responses models should receive text-projected images"
        );
        assert_eq!(results[0].text_content(), "[image: image/png]");
    }

    #[test]
    fn legacy_constructor_text_projects_tool_result_images() {
        let client = OpenAiCompatibleClient::new(
            OpenAiCompatibleMode::Responses,
            "gemma4:e2b".to_string(),
            "http://localhost:11434/v1".to_string(),
            None,
            true,
            true,
            true,
        );
        let messages = vec![
            Message::BlockAssistant(BlockAssistantMessage::new(
                vec![AssistantBlock::ToolUse {
                    id: "call_1".to_string(),
                    name: "screenshot".to_string(),
                    args: serde_json::value::RawValue::from_string("{}".to_string())
                        .expect("valid args"),
                    meta: None,
                }],
                StopReason::ToolUse,
            )),
            Message::tool_results(vec![ToolResult::with_blocks(
                "call_1".to_string(),
                vec![ContentBlock::Image {
                    media_type: "image/png".to_string(),
                    data: ImageData::Inline {
                        data: "iVBOR...".to_string(),
                    },
                }],
                false,
            )]),
        ];

        let projected = client
            .project_replay_messages(&messages)
            .expect("responses projection");
        assert!(
            matches!(&projected[1], Message::ToolResults { .. }),
            "expected tool results"
        );
        let Message::ToolResults { results, .. } = &projected[1] else {
            return;
        };
        assert!(
            !results[0].has_images(),
            "legacy constructor should preserve source compatibility and keep image tool-results disabled"
        );
        assert_eq!(results[0].text_content(), "[image: image/png]");
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
