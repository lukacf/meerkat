//! OpenAI-compatible client for self-hosted endpoints.

// The bounded post-finish read below is compiled for wasm32 too, and the wasm
// glue behind `crate::tokio` (tokio_with_wasm) is what provides `time::timeout`
// there. Native builds use the real tokio crate.
#[cfg(target_arch = "wasm32")]
use crate::tokio;
use async_trait::async_trait;
use futures::StreamExt;
use meerkat_core::schema::{CompiledSchema, SchemaError};
use meerkat_core::{
    AssistantBlock, ContentBlock, HttpAuthorizationRequest, HttpAuthorizer, ImageData, Message,
    OutputSchema, Provider, StopReason, SystemNoticeBlock, SystemNoticeMessage, Usage,
};
use meerkat_llm_core::LlmError;
use meerkat_llm_core::{
    LlmClient, LlmDoneOutcome, LlmEvent, LlmRequest, LlmStream, ToolCallBuffer,
};
use meerkat_llm_core::{http, streaming};
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use crate::client::{OpenAiReplayProjectionMode, project_openai_replay_messages_for_capabilities};

/// How long the chat-completions stream keeps reading after a finish reason has
/// been latched, before it calls the end of the trailer.
///
/// Latching the stop reason (rather than spending it at `finish_reason`) is what
/// lets a usage-only SSE event that follows the finish event still be read. It
/// also extended the read window past the point where the turn is semantically
/// complete, and nothing else bounds that window: `meerkat-llm-core`'s HTTP
/// client carries no timeout, and the agent loop's stream-inactivity watchdog
/// re-arms on every yielded item - including the `WireLiveness` this adapter
/// emits for an SSE keepalive comment - so a server that comments forever after
/// the finish event is, to that watchdog, a healthy stream.
///
/// The window is measured from the latch instant and nothing re-arms it. That is
/// deliberate: post-latch there is exactly one thing left to wait for (the
/// accounting trailer and/or `[DONE]`), and keepalive bytes are not progress
/// toward it.
///
/// The value is bounded from BOTH sides, which is why it is 30s and not
/// something rounder:
///
/// - It must be long enough that no legitimate trailer is ever cut off. A
///   compliant server computes usage as bookkeeping and flushes it immediately
///   behind the finish event, so the only real cost is transport latency. If the
///   window did cut a trailer off, the turn would reach the commit with no
///   normalized accounting and fail there - the mirror of the P0 the latch
///   fixed. 30s is one to two orders of magnitude above any plausible trailer
///   latency.
/// - It must be strictly SHORTER than
///   [`meerkat_core::DEFAULT_STREAM_INACTIVITY_TIMEOUT`], and by a wide margin.
///   That watchdog re-arms on the finish-event chunk, so its window and this one
///   start at ~the same instant; if the two were equal, finish-then-silence
///   would be a coin flip between this adapter's non-destructive end-of-stream
///   and the agent loop's RETRYABLE `StreamStalled` - a retryable failure after
///   the answer already reached the caller, which is the exact user-visible
///   shape of the original P0. `post_finish_trailer_window_is_well_inside_the_stall_window`
///   guards the ordering.
///
/// Per-client override: [`OpenAiCompatibleClient::with_post_finish_trailer_window`].
pub const DEFAULT_POST_FINISH_TRAILER_WINDOW: Duration = Duration::from_secs(30);

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
    authorizer: Option<Arc<dyn HttpAuthorizer>>,
    provider: Provider,
    base_url: String,
    http: reqwest::Client,
    responses_delegate: Option<crate::OpenAiClient>,
    supports_temperature: bool,
    supports_thinking: bool,
    supports_reasoning: bool,
    supports_image_input: bool,
    supports_image_tool_results: bool,
    post_finish_trailer_window: Duration,
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
            authorizer: None,
            provider: Provider::SelfHosted,
            base_url,
            http,
            responses_delegate,
            supports_temperature: options.supports_temperature,
            supports_thinking: options.supports_thinking,
            supports_reasoning: options.supports_reasoning,
            supports_image_input: true,
            supports_image_tool_results: options.supports_image_tool_results,
            post_finish_trailer_window: DEFAULT_POST_FINISH_TRAILER_WINDOW,
        }
    }

    pub fn with_authorizer(mut self, authorizer: Arc<dyn HttpAuthorizer>) -> Self {
        self.responses_delegate = self
            .responses_delegate
            .take()
            .map(|delegate| delegate.with_authorizer(authorizer.clone()));
        self.authorizer = Some(authorizer);
        self
    }

    pub fn with_provider(mut self, provider: Provider) -> Self {
        self.provider = provider;
        self
    }

    #[must_use]
    pub fn with_image_input_support(mut self, supports_image_input: bool) -> Self {
        self.supports_image_input = supports_image_input;
        self
    }

    /// Override how long the chat-completions stream is read after a finish
    /// reason has been latched.
    ///
    /// See [`DEFAULT_POST_FINISH_TRAILER_WINDOW`] for what the window bounds and
    /// why its default sits where it does. Shortening it trades trailer patience
    /// for a tighter bound on a completed-but-unclosed stream; a value that is
    /// too short surfaces as turns that complete with no accounting evidence,
    /// which the commit path rejects.
    #[must_use]
    pub fn with_post_finish_trailer_window(mut self, window: Duration) -> Self {
        self.post_finish_trailer_window = window;
        self
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

    /// Build the Chat Completions request body.
    ///
    /// Public but hidden: the integration matrix in `meerkat-integration-tests`
    /// runs every built-in tool definition through this exact builder (the
    /// `openai_compatible` transport path). It is not a supported host API.
    #[doc(hidden)]
    pub fn build_chat_completions_body(&self, request: &LlmRequest) -> Result<Value, LlmError> {
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
            let tools = request
                .tools
                .iter()
                .map(|tool| -> Result<Value, LlmError> {
                    let parameters = crate::tool_schema::normalize_openai_tool_parameters_schema(
                        &tool.name,
                        &tool.input_schema,
                    )?;
                    Ok(serde_json::json!({
                        "type": "function",
                        "function": {
                            "name": tool.name,
                            "description": tool.description,
                            "parameters": parameters
                        }
                    }))
                })
                .collect::<Result<Vec<Value>, LlmError>>()?;
            body["tools"] = Value::Array(tools);
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

    async fn apply_dynamic_auth_with_receipt(
        &self,
        request: reqwest::RequestBuilder,
        method: &'static str,
        url: &str,
        content_type: &'static str,
        content: meerkat_core::HttpAuthorizationContent,
    ) -> Result<
        (
            reqwest::RequestBuilder,
            meerkat_core::HttpAuthorizationReceipt,
        ),
        LlmError,
    > {
        let mut request = self.apply_auth(request, content_type);
        let receipt = if let Some(authorizer) = &self.authorizer {
            let mut headers = Vec::new();
            authorizer
                .append_content_headers(content, &mut headers)
                .map_err(|error| LlmError::AuthenticationFailed {
                    message: error.to_string(),
                })?;
            let receipt = authorizer
                .authorize_with_receipt(&mut HttpAuthorizationRequest {
                    method,
                    url,
                    headers: &mut headers,
                })
                .await
                .map_err(LlmError::from_authorizer)?;
            for (name, value) in headers {
                request = request.header(name, value);
            }
            receipt
        } else {
            meerkat_core::HttpAuthorizationReceipt::untracked()
        };
        Ok((request, receipt))
    }

    async fn apply_dynamic_auth(
        &self,
        request: reqwest::RequestBuilder,
        method: &'static str,
        url: &str,
        content_type: &'static str,
    ) -> Result<reqwest::RequestBuilder, LlmError> {
        self.apply_dynamic_auth_with_receipt(
            request,
            method,
            url,
            content_type,
            meerkat_core::HttpAuthorizationContent::default(),
        )
        .await
        .map(|(request, _)| request)
    }

    fn parse_chat_completions_line(line: &str) -> Result<ChatCompletionsLine, LlmError> {
        if let Some(data) = line
            .strip_prefix("data: ")
            .or_else(|| line.strip_prefix("data:"))
        {
            if data == "[DONE]" {
                return Ok(ChatCompletionsLine::Done);
            }
            // Check for an error envelope BEFORE decoding as a chunk. With
            // `choices` defaulted, an error envelope decodes as a perfectly
            // valid empty chunk and would be ignored - turning a dead server
            // into a successful turn with truncated output.
            if let Ok(envelope) = serde_json::from_str::<ChatCompletionsErrorEnvelope>(data)
                && let Some(message) = envelope.into_message()
            {
                return Ok(ChatCompletionsLine::ServerError { message });
            }
            serde_json::from_str(data)
                .map(ChatCompletionsLine::Chunk)
                .map_err(|err| LlmError::StreamParseError {
                    message: format!("failed to parse chat completions chunk: {err}; line={data}"),
                })
        } else {
            Ok(ChatCompletionsLine::Ignored)
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
    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        let mode = match self.mode {
            OpenAiCompatibleMode::Responses => OpenAiReplayProjectionMode::Responses,
            OpenAiCompatibleMode::ChatCompletions => OpenAiReplayProjectionMode::ChatCompletions,
        };
        project_openai_replay_messages_for_capabilities(
            messages,
            mode,
            self.supports_image_input,
            self.supports_image_tool_results,
        )
    }

    fn request_pressure(
        &self,
        request: &LlmRequest,
    ) -> Result<Option<meerkat_core::ProviderRequestPressure>, LlmError> {
        let mut projected_request = request.clone();
        projected_request.messages = self.project_replay_messages(&request.messages)?;
        match self.mode {
            OpenAiCompatibleMode::Responses => {
                let Some(delegate) = self.responses_delegate.as_ref() else {
                    return Ok(None);
                };
                let translated = self.request_with_remote_model(&projected_request);
                let Some(mut pressure) = delegate.request_pressure(&translated)? else {
                    return Ok(None);
                };
                pressure.max_bytes = meerkat_models::approximate_request_byte_cap(self.provider);
                if let Some(provenance) = pressure.lowered_request_provenance.as_mut() {
                    provenance.provider = self.provider;
                }
                Ok(Some(pressure))
            }
            OpenAiCompatibleMode::ChatCompletions => {
                let body = self.build_chat_completions_body(&projected_request)?;
                let encoded_body =
                    serde_json::to_vec(&body).map_err(|error| LlmError::InvalidRequest {
                        message: format!(
                            "failed to serialize OpenAI-compatible request body: {error}"
                        ),
                    })?;
                Ok(Some(
                    meerkat_core::ProviderRequestPressure::new(
                        encoded_body.len() as u64,
                        meerkat_models::approximate_request_byte_cap(self.provider),
                    )
                    .with_lowered_request_provenance(
                        meerkat_core::LoweredRequestProvenance::from_body(
                            self.provider,
                            meerkat_core::LoweredRequestEncoding::OpenAiChatCompletionsJson,
                            &encoded_body,
                        ),
                    ),
                ))
            }
        }
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
                        match event? {
                            LlmEvent::UsageUpdate { usage } => {
                                let raw = usage.as_usage().clone();
                                let accounting =
                                    meerkat_core::ProviderTokenAccounting::openai_compatible_for(
                                        self.provider,
                                        &request.model,
                                        raw.input_tokens,
                                    );
                                yield LlmEvent::UsageUpdate {
                                    usage: meerkat_core::TurnUsage::new(raw, accounting),
                                };
                            }
                            other => yield other,
                        }
                    }
                });
                streaming::ensure_terminal_done(inner)
            }
            OpenAiCompatibleMode::ChatCompletions => {
                let inner: LlmStream<'a> = Box::pin(async_stream::try_stream! {
                    let mut projected_request = request.clone();
                    projected_request.messages = self.project_replay_messages(&request.messages)?;
                    let body = self.build_chat_completions_body(&projected_request)?;
                    let content = meerkat_core::HttpAuthorizationContent {
                        has_images: projected_request.has_images(),
                    };
                    let url = format!("{}/chat/completions", self.base_url);
                    let request_builder = self.http.post(&url);
                    let (request_builder, receipt) = self
                        .apply_dynamic_auth_with_receipt(
                            request_builder,
                            "POST",
                            &url,
                            "application/json",
                            content,
                        )
                        .await?;
                    let mut response = request_builder
                        .json(&body)
                        .send()
                        .await
                        .map_err(Self::map_send_error)?;

                    let mut status_code = response.status().as_u16();
                    if !(200..=299).contains(&status_code)
                        && let Some(authorizer) = &self.authorizer
                        && authorizer
                            .observe_response_with_receipt(
                                receipt,
                                &meerkat_core::HttpAuthorizationResponse {
                                    method: "POST",
                                    url: &url,
                                    status: status_code,
                                },
                            )
                            .await
                            .map_err(|error| LlmError::AuthenticationFailed {
                                message: error.to_string(),
                            })?
                            == meerkat_core::HttpAuthorizationResponseAction::RetryWithFreshAuthorization
                    {
                        let request_builder = self.http.post(&url);
                        let (request_builder, retry_receipt) = self
                            .apply_dynamic_auth_with_receipt(
                                request_builder,
                                "POST",
                                &url,
                                "application/json",
                                content,
                            )
                            .await?;
                        response = request_builder
                            .json(&body)
                            .send()
                            .await
                            .map_err(Self::map_send_error)?;
                        status_code = response.status().as_u16();
                        authorizer
                            .observe_response_with_receipt(
                                retry_receipt,
                                &meerkat_core::HttpAuthorizationResponse {
                                    method: "POST",
                                    url: &url,
                                    status: status_code,
                                },
                            )
                            .await
                            .map_err(|error| LlmError::AuthenticationFailed {
                                message: error.to_string(),
                            })?;
                    }
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
                    // `finish_reason` and the `stream_options.include_usage`
                    // accounting are not required to share one SSE event. vLLM
                    // emits the finish event, then a usage-only event with an
                    // empty `choices`, then `[DONE]`; other OpenAI-compatible
                    // servers co-locate usage with the finish event. Both
                    // chunkings are legal, so the derived stop reason is latched
                    // here rather than spent immediately: the terminal `Done`
                    // stops the consuming wrapper, and emitting it at
                    // `finish_reason` would discard the very accounting this
                    // request asked for. The latch is spent exactly once, by
                    // the terminal emission below.
                    let mut latched_stop: Option<StopReason> = None;
                    // The instant the latch was taken, which is where the
                    // post-finish trailer window starts. `time_compat::Instant`
                    // is the browser-safe monotonic clock; this path is compiled
                    // for wasm32, where `tokio::time::Instant` does not exist.
                    let mut latched_at: Option<meerkat_core::time_compat::Instant> = None;
                    let mut saw_done_sentinel = false;
                    let trailer_window = self.post_finish_trailer_window;

                    'consume: loop {
                        // POST-LATCH, THE WAIT FOR THE NEXT CHUNK IS BOUNDED.
                        //
                        // Pre-latch it is not, and must not be: the model may
                        // legitimately think for a long time before emitting
                        // anything, and a bound there is a different policy with
                        // a different owner (the agent loop's call timeout and
                        // stream-inactivity watchdog). Post-latch the turn is
                        // already semantically complete and exactly one thing is
                        // outstanding - the accounting trailer and/or `[DONE]` -
                        // so an unbounded wait here is an unbounded wait for a
                        // finished turn. Nothing else closes it: the shared HTTP
                        // client carries no timeout, and a server emitting SSE
                        // keepalive comments forever re-arms the agent loop's
                        // watchdog through the `WireLiveness` below, so the
                        // liveness signal meant to prove the stream is healthy is
                        // what would keep it hanging.
                        //
                        // ONE budget measured from the latch, not a per-chunk
                        // idle window: keepalive bytes are not progress toward
                        // the trailer, so they must not buy more time.
                        let next = match latched_at {
                            None => stream.next().await,
                            Some(latched_at) => {
                                let remaining =
                                    trailer_window.saturating_sub(latched_at.elapsed());
                                match tokio::time::timeout(remaining, stream.next()).await {
                                    Ok(next) => next,
                                    // EXPIRY IS END OF STREAM, NOT TURN FAILURE
                                    // - the same rule the post-latch read and
                                    // parse faults follow. The model finished and
                                    // its answer already reached the caller, so
                                    // there is nothing here to invalidate;
                                    // failing would turn a delivered turn into a
                                    // retryable one. Nothing is laundered
                                    // either: no accounting is minted or
                                    // substituted, so a trailer that never
                                    // arrived is still absent when the commit
                                    // path looks for it.
                                    Err(_) => break 'consume,
                                }
                            }
                        };
                        let Some(chunk) = next else {
                            break 'consume;
                        };
                        // Latching the stop reason extended the read window past
                        // the finish event, so bytes this adapter never used to
                        // read are now load-bearing. Post-latch, a transport
                        // fault is END OF STREAM, not turn failure: the model
                        // already finished and its answer already reached the
                        // caller, so failing here would convert a complete turn
                        // into a RETRYABLE `ConnectionReset` - answer streams,
                        // turn fails, retry answers again. That is the exact
                        // shape of the P0 this latch was added to fix, on a new
                        // trigger (truncated body, proxy drop, ingress idle
                        // close). Nothing is laundered: if usage never arrived,
                        // the absent-accounting path downstream still owns that
                        // verdict.
                        let chunk = match chunk {
                            Ok(chunk) => chunk,
                            Err(_) if latched_stop.is_some() => break 'consume,
                            Err(_) => Err(LlmError::ConnectionReset)?,
                        };
                        buffer.push_str(&String::from_utf8_lossy(&chunk));

                        let chunk_yielded = std::cell::Cell::new(false);
                        let mut chunk_consumed_line = false;
                        while let Some(newline_pos) = buffer.find('\n') {
                            chunk_consumed_line = true;
                            let line = buffer[..newline_pos].trim();
                            let should_process = !line.is_empty() && !line.starts_with(':');
                            let parsed = if should_process {
                                Self::parse_chat_completions_line(line)
                            } else {
                                Ok(ChatCompletionsLine::Ignored)
                            };
                            buffer.drain(..=newline_pos);

                            // Post-latch, an undecodable line is also end of
                            // stream rather than turn failure. The original
                            // rationale here was that the undecodable line is
                            // probably the usage event, so failing preserves a
                            // diagnosis - but a server is free to emit keepalive
                            // text or `{"choices":null,...}` after the finish
                            // event, and both are ordinary. Failing a completed,
                            // already-delivered turn to preserve a diagnosis
                            // trades a real outage for a log line. If usage
                            // genuinely never arrived, that fact is owned
                            // downstream where it can be reported without
                            // destroying the turn.
                            let parsed = match parsed {
                                Ok(parsed) => parsed,
                                Err(_) if latched_stop.is_some() => break 'consume,
                                Err(err) => Err(err)?,
                            };
                            let event = match parsed {
                                ChatCompletionsLine::Ignored => continue,
                                // A provider error BEFORE the stop reason is
                                // latched invalidates the turn: the model had
                                // not finished, so whatever text reached the
                                // caller is truncated and must not be presented
                                // as a complete answer. AFTER the latch the
                                // model already finished and its answer already
                                // landed, so a late server error - an engine
                                // tearing down after completing the request -
                                // invalidates nothing and ends the stream.
                                ChatCompletionsLine::ServerError { message } => {
                                    if latched_stop.is_some() {
                                        break 'consume;
                                    }
                                    Err(LlmError::StreamParseError {
                                        message: format!("provider error event: {message}"),
                                    })?
                                }
                                ChatCompletionsLine::Done => {
                                    // The protocol's terminal sentinel closes
                                    // the turn without waiting for the server
                                    // to drop the connection.
                                    saw_done_sentinel = true;
                                    break 'consume;
                                }
                                ChatCompletionsLine::Chunk(event) => event,
                            };

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
                                    provider_accounting: Some(
                                        meerkat_core::ProviderTokenAccounting::openai_compatible_for(
                                            self.provider,
                                            &request.model,
                                            event_usage.prompt_tokens.unwrap_or(0),
                                        ),
                                    ),
                                };
                                chunk_yielded.set(true);
                                yield LlmEvent::UsageUpdate {
                                    usage: meerkat_core::TurnUsage::try_from_usage(usage)
                                        .map_err(|error| LlmError::Unknown {
                                            message: error.to_string(),
                                        })?,
                                };
                            }

                            for choice in event.choices {
                                // The turn is terminal once a finish
                                // reason has been latched. A compliant
                                // server sends only usage and `[DONE]`
                                // after it, and choice content past the
                                // finish event was never part of the
                                // observable turn: the terminal-done
                                // wrapper truncated it. Deferring `Done`
                                // must not start admitting it.
                                if latched_stop.is_some() {
                                    continue;
                                }
                                if let Some(delta) = choice.delta {
                                    // REASONING BEFORE CONTENT, and the order is load-bearing.
                                    //
                                    // A single delta may carry BOTH fields - what an
                                    // OpenAI-compatible server emits on the reasoning-to-content
                                    // transition, and vLLM does. The reasoning in such a chunk was
                                    // produced BEFORE the content beside it, so emitting content
                                    // first reverses them and the operator sees the two channels
                                    // shuffled together. Nothing downstream can repair it: by then
                                    // the interleaving IS the stream.
                                    let reasoning_delta = delta
                                        .reasoning_content
                                        .as_ref()
                                        .or(delta.reasoning.as_ref())
                                        .or(delta.thinking.as_ref());
                                    if let Some(reasoning) = reasoning_delta
                                        && !reasoning.is_empty()
                                    {
                                        reasoning_text.push_str(reasoning);
                                        chunk_yielded.set(true);
                                        yield LlmEvent::ReasoningDelta {
                                            delta: reasoning.clone(),
                                        };
                                    }
                                    if let Some(content) = delta.content
                                        && !content.is_empty()
                                    {
                                        chunk_yielded.set(true);
                                        yield LlmEvent::TextDelta {
                                            delta: content,
                                            meta: None,
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
                                                    chunk_yielded.set(true);
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
                                                chunk_yielded.set(true);
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
                                        chunk_yielded.set(true);
                                        yield LlmEvent::ReasoningComplete {
                                            text: std::mem::take(&mut reasoning_text),
                                            meta: None,
                                        };
                                    }
                                    // Latched, not emitted: the terminal
                                    // `Done` is owed to `[DONE]` or to the
                                    // end of the stream, whichever comes
                                    // first, so a usage-only event that
                                    // follows this finish event is still
                                    // read. The trailer window starts here.
                                    latched_stop = Some(stop_reason);
                                    latched_at =
                                        Some(meerkat_core::time_compat::Instant::now());
                                }
                            }
                        }
                        // A chunk whose lines produced no semantic event is
                        // still wire liveness (keepalive comments, unhandled
                        // bookkeeping); surface it so the stream-inactivity
                        // watchdog re-arms. Bytes that never complete a line
                        // intentionally do not count.
                        if chunk_consumed_line && !chunk_yielded.get() {
                            yield LlmEvent::WireLiveness;
                        }
                    }

                    // A trailing partial line only falsifies the turn when the
                    // turn never reached a terminal event of its own. Once a
                    // finish reason is latched or `[DONE]` has been seen, the
                    // turn is complete and trailing bytes are not a fault -
                    // exactly the reachability this branch had when `Done` was
                    // emitted at `finish_reason`.
                    if latched_stop.is_none() && !saw_done_sentinel && !buffer.trim().is_empty() {
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
                    // The single terminal emission, after every usage event the
                    // server chose to send.
                    yield LlmEvent::Done {
                        outcome: LlmDoneOutcome::Success {
                            stop_reason: latched_stop.unwrap_or(StopReason::EndTurn),
                        },
                    };
                });

                streaming::ensure_terminal_done(inner)
            }
        }
    }

    fn provider(&self) -> meerkat_core::Provider {
        self.provider
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        let url = format!("{}/models", self.base_url);
        let response = self
            .apply_dynamic_auth(self.http.get(&url), "GET", &url, "application/json")
            .await?
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

/// One decoded line of an OpenAI-compatible Chat Completions SSE body.
///
/// The `[DONE]` sentinel and a line this adapter has no reason to interpret are
/// different facts: the first closes the turn, the second is only wire
/// liveness. Collapsing both into `Option::None` is what made the terminal
/// event unrepresentable at its true position and forced the adapter to emit
/// `Done` at `finish_reason`, discarding any usage event that followed.
#[derive(Debug)]
enum ChatCompletionsLine {
    Chunk(ChatCompletionsChunk),
    Done,
    Ignored,
    /// A provider error envelope, e.g. `{"object":"error","message":"engine
    /// core proc died"}` or `{"error":{"message":...}}`.
    ///
    /// This variant exists because `choices` is `#[serde(default)]` - which a
    /// usage-only event genuinely needs - and without an error arm an error
    /// envelope decodes as an EMPTY CHUNK and is silently ignored. A server
    /// that dies mid-stream then presents as a SUCCESSFUL turn carrying
    /// truncated text, which is worse than the failure it replaced. The
    /// sibling adapter already models this (`text_adapter.rs`,
    /// `ServerEvent::Error => Err(map_server_error(error))?`).
    ServerError {
        message: String,
    },
}

/// Provider error envelope. Both shapes are seen in the wild: a top-level
/// `{"object":"error","message":...}` and a nested `{"error":{"message":...}}`.
#[derive(Debug, Deserialize)]
struct ChatCompletionsErrorEnvelope {
    #[serde(default)]
    object: Option<String>,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    error: Option<ChatCompletionsNestedError>,
}

#[derive(Debug, Deserialize)]
struct ChatCompletionsNestedError {
    #[serde(default)]
    message: Option<String>,
}

impl ChatCompletionsErrorEnvelope {
    /// `Some(message)` only when this really is an error envelope. A normal
    /// chunk deserializes into this struct too (every field is optional), so
    /// presence of an error marker - not successful decoding - is the test.
    fn into_message(self) -> Option<String> {
        if let Some(nested) = self.error.and_then(|nested| nested.message) {
            return Some(nested);
        }
        if self.object.as_deref() == Some("error") {
            return Some(self.message.unwrap_or_else(|| "provider error".to_string()));
        }
        None
    }
}

#[derive(Debug, Deserialize)]
struct ChatCompletionsChunk {
    // A usage-only or metadata-only event may omit `choices` entirely rather
    // than send it empty. That is a chunking difference, not a fault, and must
    // not fail the turn.
    #[serde(default)]
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
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
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

    /// Regression (A2): the Chat Completions path (the self-hosted
    /// `openai_compatible` transport) is where a root-level `not` produced
    /// `invalid_function_parameters`. The tools array must carry the
    /// normalized `workgraph_claim` schema.
    #[test]
    fn chat_completions_drops_root_level_not_from_pre_fix_workgraph_claim_shape() {
        use meerkat_core::ToolDef;

        let client = OpenAiCompatibleClient::new_with_options(
            OpenAiCompatibleMode::ChatCompletions,
            "remote-model".to_string(),
            "https://example.test".to_string(),
            None,
            options(true, true, true, true),
        );
        let request = LlmRequest::new(
            "catalog-model",
            vec![meerkat_core::Message::User(UserMessage::text(
                "test".to_string(),
            ))],
        )
        .with_tools(vec![Arc::new(ToolDef {
            name: "workgraph_claim".into(),
            description: "Claim a ready WorkGraph item with CAS revision checking.".to_string(),
            input_schema: crate::tool_schema::test_fixtures::pre_fix_workgraph_claim_schema(),
            provenance: None,
        })]);

        let body = client
            .build_chat_completions_body(&request)
            .expect("Chat Completions body");
        let function = &body["tools"][0]["function"];
        let parameters = &function["parameters"];

        assert_eq!(body["tools"][0]["type"], "function");
        assert_eq!(function["name"], "workgraph_claim");
        assert!(
            parameters.get("not").is_none(),
            "root-level not must be dropped: {parameters}"
        );
        assert_eq!(parameters["type"], "object");
        assert_eq!(
            parameters["required"],
            serde_json::json!(["id", "expected_revision", "owner"])
        );
        assert_eq!(
            parameters["properties"]["lease_expires_at"]["format"],
            "date-time"
        );
    }

    #[test]
    fn chat_completions_serializes_typed_structured_output_as_response_format() {
        use meerkat_core::lifecycle::run_primitive::{OpenAiProviderTag, ProviderTag};

        let client = OpenAiCompatibleClient::new_with_options(
            OpenAiCompatibleMode::ChatCompletions,
            "remote-model".to_string(),
            "https://example.test".to_string(),
            None,
            options(true, true, true, true),
        );
        let output_schema = OutputSchema::new(serde_json::json!({
            "type": "object",
            "properties": {"answer": {"type": "string"}},
            "required": ["answer"]
        }))
        .expect("valid output schema");
        let mut request = LlmRequest::new("catalog-model", Vec::new());
        request.provider_params = Some(ProviderTag::OpenAi(OpenAiProviderTag {
            structured_output: Some(output_schema),
            ..Default::default()
        }));

        let body = client
            .build_chat_completions_body(&request)
            .expect("Chat Completions body");

        assert_eq!(body["response_format"]["type"], "json_schema");
        assert_eq!(
            body["response_format"]["json_schema"]["schema"]["properties"]["answer"]["type"],
            "string"
        );
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

    async fn spawn_compatible_replay_capture_server(
        mode: OpenAiCompatibleMode,
        payload: String,
    ) -> (String, Arc<Mutex<Vec<Value>>>, tokio::task::JoinHandle<()>) {
        let request_bodies = Arc::new(Mutex::new(Vec::new()));
        let route = match mode {
            OpenAiCompatibleMode::Responses => "/v1/responses",
            OpenAiCompatibleMode::ChatCompletions => "/v1/chat/completions",
        };
        let app = Router::new()
            .route(route, post(responses_sse))
            .with_state(ResponsesStubState {
                payload,
                auth_headers: Arc::new(Mutex::new(Vec::new())),
                request_bodies: Arc::clone(&request_bodies),
            });
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind replay capture server");
        let addr = listener.local_addr().expect("local addr");
        let handle = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve replay capture server");
        });
        (format!("http://{addr}/v1"), request_bodies, handle)
    }

    #[derive(Clone)]
    struct ChatAuthRetryState {
        calls: Arc<std::sync::atomic::AtomicUsize>,
    }

    async fn chat_auth_retry(State(state): State<ChatAuthRetryState>) -> impl IntoResponse {
        if state
            .calls
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            == 0
        {
            return axum::http::StatusCode::UNAUTHORIZED.into_response();
        }
        ([("content-type", "text/event-stream")], "data: [DONE]\n\n").into_response()
    }

    struct ChatRetryAuthorizer {
        authorizations: std::sync::atomic::AtomicUsize,
    }

    #[async_trait::async_trait]
    impl HttpAuthorizer for ChatRetryAuthorizer {
        async fn authorize(
            &self,
            request: &mut HttpAuthorizationRequest<'_>,
        ) -> Result<(), meerkat_core::AuthError> {
            let generation = self
                .authorizations
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                + 1;
            request.headers.push((
                "Authorization".to_string(),
                format!("Bearer token-{generation}"),
            ));
            Ok(())
        }

        async fn observe_response(
            &self,
            response: &meerkat_core::HttpAuthorizationResponse<'_>,
        ) -> Result<meerkat_core::HttpAuthorizationResponseAction, meerkat_core::AuthError>
        {
            Ok(if response.status == 401 {
                meerkat_core::HttpAuthorizationResponseAction::RetryWithFreshAuthorization
            } else {
                meerkat_core::HttpAuthorizationResponseAction::Propagate
            })
        }

        fn label(&self) -> &'static str {
            "chat-retry-test"
        }
    }

    #[tokio::test]
    async fn chat_completions_reauthorizes_before_reading_stream() {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let app = Router::new()
            .route("/chat/completions", post(chat_auth_retry))
            .with_state(ChatAuthRetryState {
                calls: Arc::clone(&calls),
            });
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test server");
        let addr = listener.local_addr().expect("local addr");
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve test server");
        });
        let authorizer = Arc::new(ChatRetryAuthorizer {
            authorizations: std::sync::atomic::AtomicUsize::new(0),
        });
        let client = OpenAiCompatibleClient::new_with_options(
            OpenAiCompatibleMode::ChatCompletions,
            "remote-model".to_string(),
            format!("http://{addr}"),
            None,
            options(true, true, true, true),
        )
        .with_authorizer(authorizer.clone());
        let request = LlmRequest::new(
            "catalog-model",
            vec![Message::User(UserMessage::text("hello"))],
        );
        let mut stream = client.stream(&request);
        while let Some(event) = stream.next().await {
            event.expect("reauthorized stream should succeed");
        }
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 2);
        assert_eq!(
            authorizer
                .authorizations
                .load(std::sync::atomic::Ordering::SeqCst),
            2
        );
        server.abort();
    }

    /// What the raw stub does after the scripted head bytes are on the wire.
    #[derive(Debug, Clone, Copy)]
    enum RawStubTail {
        /// Hold the connection open and write nothing more, ever.
        Silence,
        /// Write SSE keepalive comment lines forever.
        KeepaliveComments,
    }

    /// A raw HTTP/1.1 stub that can leave a response body OPEN after the finish
    /// event.
    ///
    /// The axum stub above cannot express this shape at all: `axum::serve`
    /// always completes the body it is handed, so every stream it serves ends by
    /// itself and the post-finish read window is closed for it by the server. A
    /// test written against that stub therefore cannot construct the hang and
    /// cannot witness the bound.
    ///
    /// The response deliberately carries NEITHER `content-length` NOR
    /// `transfer-encoding`, which per RFC 7230 3.3.3 makes the body end at
    /// connection close. "Hold open" is then just "stop writing", with no chunk
    /// framing to get wrong - and a framing bug here would show up as an early
    /// transport error, which the post-latch read-fault path already turns into
    /// the SAME observable outcome as the bound firing. The elapsed-time
    /// assertions in the tests below exist to tell those two apart.
    async fn spawn_raw_chat_stub(
        head: String,
        tail: RawStubTail,
    ) -> (String, tokio::task::JoinHandle<()>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind raw chat stub");
        let addr = listener.local_addr().expect("local addr");
        let handle = tokio::spawn(async move {
            let Ok((mut socket, _peer)) = listener.accept().await else {
                return;
            };
            // Read to the end of the request head. The POST body is small
            // enough to sit in the socket buffer, so it never needs draining.
            let mut request = Vec::new();
            let mut read_buffer = [0u8; 1024];
            loop {
                match socket.read(&mut read_buffer).await {
                    Ok(0) | Err(_) => return,
                    Ok(read) => {
                        request.extend_from_slice(&read_buffer[..read]);
                        if request.windows(4).any(|window| window == b"\r\n\r\n") {
                            break;
                        }
                    }
                }
            }
            let response =
                format!("HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\r\n{head}");
            if socket.write_all(response.as_bytes()).await.is_err() {
                return;
            }
            if socket.flush().await.is_err() {
                return;
            }
            match tail {
                // Park forever holding the connection open, so only the
                // client's own bound can end this stream.
                RawStubTail::Silence => std::future::pending::<()>().await,
                RawStubTail::KeepaliveComments => loop {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    if socket.write_all(b": keep-alive\n\n").await.is_err() {
                        return;
                    }
                    if socket.flush().await.is_err() {
                        return;
                    }
                },
            }
        });
        (format!("http://{addr}/v1"), handle)
    }

    struct RawStreamObservation {
        events: Vec<LlmEvent>,
        elapsed: Duration,
    }

    /// Streams one raw-stub body and reports both the events and how long the
    /// stream took to terminate.
    ///
    /// The elapsed time is part of every assertion below. Without it a stub that
    /// failed to hold the connection open would produce a transport error, the
    /// post-latch read-fault path would break out with the same
    /// `Done{Success}` + preserved usage, and the test would pass while proving
    /// nothing at all.
    async fn collect_raw_chat_stream(
        head: &str,
        tail: RawStubTail,
        trailer_window: Duration,
    ) -> RawStreamObservation {
        let (base_url, handle) = spawn_raw_chat_stub(head.to_string(), tail).await;
        let client = OpenAiCompatibleClient::new_with_options(
            OpenAiCompatibleMode::ChatCompletions,
            "remote-model".to_string(),
            base_url,
            None,
            options(true, false, false, false),
        )
        .with_post_finish_trailer_window(trailer_window);
        let request = LlmRequest::new(
            "qwen3.8-27b",
            vec![Message::User(UserMessage::text("Hello".to_string()))],
        );
        let started = std::time::Instant::now();
        let events: Vec<LlmEvent> = tokio::time::timeout(
            Duration::from_secs(10),
            client.stream(&request).collect::<Vec<_>>(),
        )
        .await
        .expect(
            "the post-finish trailer window must end a stream the server never closes; \
             without it this read is unbounded",
        )
        .into_iter()
        .map(|event| event.expect("stream event"))
        .collect();
        let elapsed = started.elapsed();
        handle.abort();
        RawStreamObservation { events, elapsed }
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

        let client = OpenAiCompatibleClient::new_with_options(
            OpenAiCompatibleMode::ChatCompletions,
            "remote-model".to_string(),
            "https://example.test".to_string(),
            None,
            options(true, true, true, true),
        )
        .with_image_input_support(true);
        let projected = client
            .project_replay_messages(&[Message::SystemNotice(SystemNoticeMessage::with_block(
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
            ))])
            .expect("project chat messages");
        let messages =
            OpenAiCompatibleClient::convert_to_chat_messages(&projected).expect("convert chat");

        assert_eq!(messages[0]["role"], "user");
        let content = messages[0]["content"].as_array().expect("content array");
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[1]["type"], "image_url");
        assert_eq!(
            content[1]["image_url"]["url"],
            "data:image/png;base64,iVBOR..."
        );
    }

    /// Streams one stub chat-completions body and returns the observed events.
    ///
    /// The timeout is part of the contract under test: the adapter defers its
    /// terminal `Done` until `[DONE]` or the end of the stream, so a stream
    /// carrying no usage event at all must still terminate rather than wait for
    /// accounting that never arrives.
    async fn collect_chat_stream_events(payload: &str) -> Vec<LlmEvent> {
        let (base_url, handle) = spawn_chat_stub_server(payload.to_string()).await;
        // The remote model name deliberately differs from the request model so
        // the accounting assertion below shows which one the adapter attributes
        // to: `validate_provider_turn_usage_identity` compares the accounting
        // model against the active (request) model, not the remote name.
        let client = OpenAiCompatibleClient::new_with_options(
            OpenAiCompatibleMode::ChatCompletions,
            "remote-model".to_string(),
            base_url,
            None,
            options(true, false, false, false),
        );
        let request = LlmRequest::new(
            "qwen3.8-27b",
            vec![Message::User(UserMessage::text("Hello".to_string()))],
        );
        let events: Vec<LlmEvent> = tokio::time::timeout(
            std::time::Duration::from_secs(15),
            client.stream(&request).collect::<Vec<_>>(),
        )
        .await
        .expect("chat completions stream must terminate")
        .into_iter()
        .map(|event| event.expect("stream event"))
        .collect();
        handle.abort();
        events
    }

    fn usage_update_index(events: &[LlmEvent]) -> Option<usize> {
        events
            .iter()
            .position(|event| matches!(event, LlmEvent::UsageUpdate { .. }))
    }

    fn done_index(events: &[LlmEvent]) -> Option<usize> {
        events
            .iter()
            .position(|event| matches!(event, LlmEvent::Done { .. }))
    }

    fn done_count(events: &[LlmEvent]) -> usize {
        events
            .iter()
            .filter(|event| matches!(event, LlmEvent::Done { .. }))
            .count()
    }

    fn observed_usage(events: &[LlmEvent]) -> Option<&meerkat_core::TurnUsage> {
        events.iter().find_map(|event| match event {
            LlmEvent::UsageUpdate { usage } => Some(usage),
            _ => None,
        })
    }

    fn observed_done_outcome(events: &[LlmEvent]) -> Option<&LlmDoneOutcome> {
        events.iter().find_map(|event| match event {
            LlmEvent::Done { outcome } => Some(outcome),
            _ => None,
        })
    }

    /// A provider error envelope BEFORE the finish event must fail the turn.
    ///
    /// Regression guard for the second-order defect introduced by making
    /// `choices` `#[serde(default)]`: with that default and no error arm, this
    /// envelope decodes as a valid EMPTY CHUNK and is silently ignored, so a
    /// server that dies mid-stream presents as a SUCCESSFUL turn carrying
    /// truncated text. Silent truncation reported as success is strictly worse
    /// than the parse failure it replaced.
    #[tokio::test]
    async fn chat_completions_provider_error_before_finish_fails_the_turn() {
        let events = collect_chat_stream_events(concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\n\n",
            "data: {\"object\":\"error\",\"message\":\"engine core proc died\"}\n\n",
            "data: [DONE]\n\n"
        ))
        .await;

        let outcome = observed_done_outcome(&events).expect("terminal done");
        // Rendered rather than matched, so a wrongly-successful turn fails this
        // assertion with the outcome in the message instead of needing a
        // `panic!` arm (which `-D clippy::panic` rejects).
        let rendered = match outcome {
            LlmDoneOutcome::Error { error } => error.to_string(),
            LlmDoneOutcome::Success { stop_reason } => {
                format!("SUCCESS({stop_reason:?}) - the turn was not failed at all")
            }
        };
        assert!(
            rendered.contains("engine core proc died"),
            "a provider error before the finish event must fail the turn carrying the \
             provider's own message, not present as a success with truncated text; \
             got {rendered}; events {events:?}"
        );
    }

    /// The nested `{"error":{...}}` envelope shape, same rule.
    #[tokio::test]
    async fn chat_completions_nested_provider_error_before_finish_fails_the_turn() {
        let events = collect_chat_stream_events(concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\n\n",
            "data: {\"error\":{\"message\":\"upstream exploded\"}}\n\n"
        ))
        .await;

        let outcome = observed_done_outcome(&events).expect("terminal done");
        assert!(
            matches!(outcome, LlmDoneOutcome::Error { .. }),
            "nested provider error envelope must fail the turn, got {events:?}"
        );
    }

    /// An undecodable line AFTER the stop reason is latched must NOT fail a
    /// turn whose answer already reached the caller.
    ///
    /// Regression guard for the read-window defect: latching the stop reason
    /// extended the read past the finish event, so bytes the adapter never used
    /// to read became load-bearing. A keepalive or any other undecodable line
    /// there previously turned a complete, delivered turn into a RETRYABLE
    /// failure - answer streams, turn fails, retry answers again, which is the
    /// exact shape of the P0 the latch was added to fix.
    #[tokio::test]
    async fn chat_completions_undecodable_line_after_finish_does_not_fail_the_turn() {
        let events = collect_chat_stream_events(concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"},\"finish_reason\":\"stop\"}]}\n\n",
            "data: {\"usage\":{\"prompt_tokens\":56,\"completion_tokens\":16,\"total_tokens\":72}}\n\n",
            "data: keep-alive\n\n"
        ))
        .await;

        let outcome = observed_done_outcome(&events).expect("terminal done");
        assert!(
            matches!(outcome, LlmDoneOutcome::Success { .. }),
            "an undecodable line after the latch must end the stream, not fail a \
             delivered turn; got {events:?}"
        );
        let usage = observed_usage(&events).expect("usage captured before the undecodable line");
        assert_eq!(usage.as_usage().input_tokens, 56);
        assert_eq!(done_count(&events), 1, "exactly one terminal Done");
    }

    #[tokio::test]
    async fn chat_completions_usage_only_event_after_finish_is_not_discarded() {
        // The finish event and the `stream_options.include_usage` accounting are
        // separate SSE events here, which is how vLLM streams. Every fixture in
        // this file used to co-locate them in one chunk, so the corpus encoded
        // one provider's chunking as if it were the protocol and could not
        // express this stream at all.
        let events = collect_chat_stream_events(concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"},\"finish_reason\":\"stop\"}]}\n\n",
            "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":56,\"completion_tokens\":16,\"total_tokens\":72}}\n\n",
            "data: [DONE]\n\n"
        ))
        .await;

        let usage_index = usage_update_index(&events).expect(
            "a usage-only event after the finish event must still reach the consumer; \
             discarding it is what left the turn with no normalized accounting evidence",
        );
        let terminal_index = done_index(&events).expect("terminal done");
        assert!(
            usage_index < terminal_index,
            "UsageUpdate must be emitted before the terminal Done, got {events:?}"
        );
        assert_eq!(done_count(&events), 1, "exactly one terminal Done");

        let usage = observed_usage(&events).expect("usage update");
        assert_eq!(usage.input_tokens, 56);
        assert_eq!(usage.output_tokens, 16);
        // A `TurnUsage` exists at all only because the adapter minted normalized
        // accounting for it, which is precisely the evidence the turn commit
        // requires.
        assert_eq!(usage.accounting().model, "qwen3.8-27b");
        assert!(matches!(
            observed_done_outcome(&events),
            Some(LlmDoneOutcome::Success {
                stop_reason: StopReason::EndTurn
            })
        ));
    }

    #[tokio::test]
    async fn chat_completions_separate_usage_event_preserves_length_finish_reason() {
        // The reported production stream: metadata, deltas, `finish:length`,
        // then a usage-only event, then `[DONE]`. `length` (not `stop`)
        // distinguishes a remembered finish reason from a defaulted one:
        // `stop` and "nothing latched" both project to `EndTurn`.
        let events = collect_chat_stream_events(concat!(
            "data: {\"choices\":[{\"delta\":{\"role\":\"assistant\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"Hello\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"!\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"length\"}]}\n\n",
            "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":56,\"completion_tokens\":16,\"total_tokens\":72}}\n\n",
            "data: [DONE]\n\n"
        ))
        .await;

        let usage_index = usage_update_index(&events).expect("usage-only event after finish");
        let terminal_index = done_index(&events).expect("terminal done");
        assert!(
            usage_index < terminal_index,
            "UsageUpdate must be emitted before the terminal Done, got {events:?}"
        );
        assert_eq!(done_count(&events), 1, "exactly one terminal Done");
        assert!(
            matches!(
                observed_done_outcome(&events),
                Some(LlmDoneOutcome::Success {
                    stop_reason: StopReason::MaxTokens
                })
            ),
            "the deferred Done must carry the remembered finish reason, got {events:?}"
        );
    }

    #[tokio::test]
    async fn chat_completions_separate_usage_event_terminates_without_done_sentinel() {
        // Same split chunking, but the server closes the body instead of
        // sending `[DONE]`. End of stream must close the turn too.
        let events = collect_chat_stream_events(concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"},\"finish_reason\":\"stop\"}]}\n\n",
            "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":56,\"completion_tokens\":16,\"total_tokens\":72}}\n\n"
        ))
        .await;

        let usage_index = usage_update_index(&events).expect("usage-only event after finish");
        let terminal_index = done_index(&events).expect("terminal done at end of stream");
        assert!(usage_index < terminal_index, "got {events:?}");
        assert_eq!(done_count(&events), 1, "exactly one terminal Done");
    }

    #[tokio::test]
    async fn chat_completions_stream_without_any_usage_event_still_terminates() {
        // Deferring the terminal Done must not introduce a wait for accounting
        // a server never sends. This stream terminates with the same successful
        // Done it produced before the fix, carrying no usage event.
        let events = collect_chat_stream_events(concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"},\"finish_reason\":\"stop\"}]}\n\n",
            "data: [DONE]\n\n"
        ))
        .await;

        assert!(
            usage_update_index(&events).is_none(),
            "this stream carries no usage event, got {events:?}"
        );
        assert_eq!(done_count(&events), 1, "exactly one terminal Done");
        assert!(matches!(
            observed_done_outcome(&events),
            Some(LlmDoneOutcome::Success {
                stop_reason: StopReason::EndTurn
            })
        ));
    }

    #[tokio::test]
    async fn chat_completions_stream_without_usage_or_sentinel_still_terminates() {
        // Neither usage nor `[DONE]`: only the end of the body closes this turn.
        let events = collect_chat_stream_events(
            "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"},\"finish_reason\":\"stop\"}]}\n\n",
        )
        .await;

        assert!(usage_update_index(&events).is_none());
        assert_eq!(done_count(&events), 1, "exactly one terminal Done");
        assert!(matches!(
            observed_done_outcome(&events),
            Some(LlmDoneOutcome::Success {
                stop_reason: StopReason::EndTurn
            })
        ));
    }

    /// The post-finish window must sit well inside the agent loop's
    /// stream-inactivity watchdog.
    ///
    /// Both windows start at ~the finish-event chunk (the watchdog re-arms on
    /// it), so if this one were equal or longer, finish-then-silence would race
    /// the adapter's non-destructive end-of-stream against the loop's RETRYABLE
    /// `StreamStalled` - a retryable failure after the answer already reached
    /// the caller, which is the original P0's user-visible shape.
    #[test]
    fn post_finish_trailer_window_is_well_inside_the_stall_window() {
        let stall = meerkat_core::DEFAULT_STREAM_INACTIVITY_TIMEOUT;
        assert!(
            DEFAULT_POST_FINISH_TRAILER_WINDOW * 2 < stall,
            "the post-finish trailer window ({DEFAULT_POST_FINISH_TRAILER_WINDOW:?}) must be \
             well inside the stream-inactivity window ({stall:?}), or the adapter's \
             end-of-stream races the agent loop's retryable stall verdict"
        );
    }

    #[tokio::test]
    async fn chat_completions_finish_then_held_open_connection_terminates_with_usage() {
        // The shape the latch introduced and nothing bounded: the server sends
        // the finish event and the accounting trailer, then holds the connection
        // open without closing the body. Before the bound, this read never
        // returned - the HTTP client has no timeout, so the only remaining
        // authority was the agent loop's stall watchdog, whose expiry is a
        // RETRYABLE failure of an already-delivered turn.
        let window = Duration::from_millis(300);
        let observation = collect_raw_chat_stream(
            concat!(
                "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"},\"finish_reason\":\"stop\"}]}\n\n",
                "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":56,\"completion_tokens\":16,\"total_tokens\":72}}\n\n",
            ),
            RawStubTail::Silence,
            window,
        )
        .await;

        let events = &observation.events;
        assert!(
            matches!(
                observed_done_outcome(events),
                Some(LlmDoneOutcome::Success {
                    stop_reason: StopReason::EndTurn
                })
            ),
            "an unclosed body after the latch is END OF STREAM carrying the latched stop \
             reason, not a turn failure and not a retryable one; got {events:?}"
        );
        assert_eq!(
            done_count(events),
            1,
            "exactly one terminal Done: {events:?}"
        );
        let usage = observed_usage(events)
            .expect("the accounting that arrived before the silence must be preserved");
        assert_eq!(usage.as_usage().input_tokens, 56);
        assert_eq!(usage.as_usage().output_tokens, 16);
        assert_eq!(usage.accounting().model, "qwen3.8-27b");
        assert!(
            observation.elapsed >= window,
            "the stream must have ended because the trailer window expired, not because the \
             stub closed early and tripped the post-latch read-fault path: elapsed \
             {:?} < window {window:?}",
            observation.elapsed
        );
    }

    #[tokio::test]
    async fn chat_completions_keepalive_comments_after_finish_do_not_extend_the_window() {
        // Keepalive comments are the case the agent loop's watchdog cannot see:
        // each comment-only chunk yields `WireLiveness`, every yielded item
        // re-arms that watchdog, so a server commenting forever after the finish
        // event is a permanently healthy stream to it. The trailer window is
        // therefore measured from the latch and re-armed by nothing - including
        // these comments.
        let window = Duration::from_millis(300);
        let observation = collect_raw_chat_stream(
            concat!(
                "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"},\"finish_reason\":\"stop\"}]}\n\n",
                "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":56,\"completion_tokens\":16,\"total_tokens\":72}}\n\n",
            ),
            RawStubTail::KeepaliveComments,
            window,
        )
        .await;

        let events = &observation.events;
        assert!(
            matches!(
                observed_done_outcome(events),
                Some(LlmDoneOutcome::Success {
                    stop_reason: StopReason::EndTurn
                })
            ),
            "endless keepalive comments after the latch must end the stream successfully; \
             got {events:?}"
        );
        assert_eq!(
            done_count(events),
            1,
            "exactly one terminal Done: {events:?}"
        );
        let usage = observed_usage(events)
            .expect("the accounting that arrived before the keepalives must be preserved");
        assert_eq!(usage.as_usage().input_tokens, 56);
        assert!(
            observation.elapsed >= window,
            "elapsed {:?} < window {window:?}: the stub closed early instead of commenting",
            observation.elapsed
        );
        // A window re-armed by wire liveness never expires against a stub that
        // comments every 20ms, so a finite elapsed at all is the assertion. The
        // ceiling is deliberately many multiples of the window so that a loaded
        // build host cannot fail it.
        assert!(
            observation.elapsed < window * 16,
            "keepalive comments must not buy more trailer time: elapsed {:?} against a \
             {window:?} window",
            observation.elapsed
        );
    }

    #[tokio::test]
    async fn chat_completions_finish_then_silence_without_usage_mints_no_accounting() {
        // Finish, then silence, and the server never sent accounting at all. The
        // bound must still end the stream - and must NOT invent usage to make
        // the turn look accounted for. Absent accounting is owned downstream, by
        // the commit path, which is the only place that can report it honestly.
        let window = Duration::from_millis(300);
        let observation = collect_raw_chat_stream(
            "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"},\"finish_reason\":\"stop\"}]}\n\n",
            RawStubTail::Silence,
            window,
        )
        .await;

        let events = &observation.events;
        assert!(
            matches!(
                observed_done_outcome(events),
                Some(LlmDoneOutcome::Success {
                    stop_reason: StopReason::EndTurn
                })
            ),
            "an unclosed body with no trailer at all is still END OF STREAM; got {events:?}"
        );
        assert_eq!(
            done_count(events),
            1,
            "exactly one terminal Done: {events:?}"
        );
        assert!(
            usage_update_index(events).is_none(),
            "no accounting arrived, so none may be minted or substituted here: {events:?}"
        );
        assert!(
            observation.elapsed >= window,
            "elapsed {:?} < window {window:?}: the stub closed early instead of holding open",
            observation.elapsed
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

    /// A single delta may carry BOTH `reasoning` and `content`. An
    /// OpenAI-compatible server emits exactly that on the reasoning-to-content
    /// transition, and vLLM does. The reasoning was produced BEFORE the content
    /// beside it, so the events must leave in that order.
    ///
    /// The reasoning test above cannot catch a reversal: every one of its
    /// chunks carries reasoning OR content, never both, so no ordering
    /// question ever arises in the fixture.
    #[tokio::test]
    async fn combined_reasoning_and_content_delta_emits_reasoning_first() {
        let payload = concat!(
            "data: {\"choices\":[{\"delta\":{\"reasoning\":\"No tool\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"reasoning\":\" needed.\",\"content\":\"I am an\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\" AI assistant.\"},\"finish_reason\":\"stop\"}]}\n\n",
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
        // Record the ORDER the two channels arrive in, not just their contents.
        let mut order = Vec::new();
        for event in events {
            match event.expect("event") {
                LlmEvent::ReasoningDelta { delta } => order.push(("reasoning", delta)),
                LlmEvent::TextDelta { delta, .. } => order.push(("text", delta)),
                _ => {}
            }
        }

        assert_eq!(
            order,
            vec![
                ("reasoning", "No tool".to_string()),
                ("reasoning", " needed.".to_string()),
                ("text", "I am an".to_string()),
                ("text", " AI assistant.".to_string()),
            ],
            "a combined delta must emit its reasoning before its content"
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
        assert!(matches!(chunk, ChatCompletionsLine::Chunk(_)));
    }

    #[test]
    fn parse_chat_completions_line_separates_terminal_sentinel_from_ignored_lines() {
        // The terminal sentinel closes the turn; a line this adapter does not
        // interpret is only wire liveness. One decoded value cannot stand for
        // both.
        assert!(matches!(
            OpenAiCompatibleClient::parse_chat_completions_line("data: [DONE]")
                .expect("sentinel should parse"),
            ChatCompletionsLine::Done
        ));
        assert!(matches!(
            OpenAiCompatibleClient::parse_chat_completions_line("event: ping")
                .expect("unhandled field should parse"),
            ChatCompletionsLine::Ignored
        ));
    }

    #[test]
    fn parse_chat_completions_line_accepts_a_chunk_without_choices() {
        // A usage-only event may omit `choices` instead of sending it empty.
        let line = r#"data: {"usage":{"prompt_tokens":56,"completion_tokens":16}}"#;
        let parsed =
            OpenAiCompatibleClient::parse_chat_completions_line(line).expect("line should parse");
        let ChatCompletionsLine::Chunk(chunk) = parsed else {
            unreachable!("a data line carrying usage decodes to a chunk")
        };
        assert!(chunk.choices.is_empty());
        assert_eq!(
            chunk.usage.and_then(|usage| usage.prompt_tokens),
            Some(56),
            "the usage on a choices-less chunk must survive decoding"
        );
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

    #[test]
    fn self_hosted_replay_uses_openai_wire_family_for_metadata() {
        use meerkat_core::ProviderMeta;

        let client = OpenAiCompatibleClient::new_with_options(
            OpenAiCompatibleMode::Responses,
            "remote-model".to_string(),
            "https://example.test".to_string(),
            None,
            options(true, true, true, true),
        );
        assert_eq!(client.provider(), Provider::SelfHosted);
        let messages = vec![Message::BlockAssistant(BlockAssistantMessage::new(
            vec![
                AssistantBlock::Text {
                    text: "visible".to_string(),
                    meta: None,
                },
                AssistantBlock::Reasoning {
                    text: "openai".to_string(),
                    meta: Some(Box::new(ProviderMeta::OpenAi {
                        id: "rs_1".to_string(),
                        encrypted_content: Some("ciphertext".to_string()),
                        phase: Some("reasoning".to_string()),
                        response_id: None,
                    })),
                },
                AssistantBlock::Reasoning {
                    text: "anthropic".to_string(),
                    meta: Some(Box::new(ProviderMeta::Anthropic {
                        signature: "foreign".to_string(),
                    })),
                },
                AssistantBlock::Reasoning {
                    text: "gemini".to_string(),
                    meta: Some(Box::new(ProviderMeta::Gemini {
                        thought_signature: "foreign".to_string(),
                    })),
                },
            ],
            StopReason::EndTurn,
        ))];

        let projected = client
            .project_replay_messages(&messages)
            .expect("SelfHosted OpenAI-family projection");
        let Message::BlockAssistant(assistant) = &projected[0] else {
            panic!("expected projected assistant message");
        };
        assert_eq!(assistant.blocks.len(), 2);
        assert!(
            matches!(assistant.blocks[1], AssistantBlock::Reasoning { meta: Some(ref meta), .. } if matches!(meta.as_ref(), ProviderMeta::OpenAi { .. }))
        );
        assert!(assistant.blocks.iter().all(|block| !matches!(
            block,
            AssistantBlock::Reasoning { text, .. } if text == "anthropic" || text == "gemini"
        )));
    }

    #[test]
    fn non_vision_self_hosted_replay_lowers_inline_images_to_text() {
        let client = OpenAiCompatibleClient::new_with_options(
            OpenAiCompatibleMode::Responses,
            "remote-model".to_string(),
            "https://example.test".to_string(),
            None,
            options(true, true, true, true),
        )
        .with_image_input_support(false);
        let messages = vec![Message::User(UserMessage::with_blocks(vec![
            ContentBlock::Image {
                media_type: "image/png".to_string(),
                data: ImageData::Inline {
                    data: "AAAA".to_string(),
                },
            },
        ]))];

        let projected = client
            .project_replay_messages(&messages)
            .expect("non-vision SelfHosted replay should lower image history");
        let Message::User(user) = &projected[0] else {
            panic!("expected projected user message");
        };
        assert!(matches!(
            user.content.as_slice(),
            [ContentBlock::Text { .. }]
        ));
    }

    async fn assert_compatible_dispatch_strips_nested_image(mode: OpenAiCompatibleMode) {
        use meerkat_core::{SystemNoticeKind, SystemNoticeMessage};

        let payload = match mode {
            OpenAiCompatibleMode::Responses => concat!(
                "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"output\":[],\"usage\":{\"input_tokens\":1,\"output_tokens\":1}}}\n\n",
                "data: {\"type\":\"response.done\",\"response\":{\"status\":\"completed\",\"output\":[],\"usage\":{\"input_tokens\":1,\"output_tokens\":1}}}\n\n"
            ),
            OpenAiCompatibleMode::ChatCompletions => concat!(
                "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":1}}\n\n",
                "data: [DONE]\n\n"
            ),
        }
        .to_string();
        let (base_url, request_bodies, server) =
            spawn_compatible_replay_capture_server(mode, payload).await;
        let client = OpenAiCompatibleClient::new_with_options(
            mode,
            "remote-model".to_string(),
            base_url,
            None,
            options(true, true, true, true),
        )
        .with_image_input_support(false);
        let request = LlmRequest::new(
            "remote-model",
            vec![Message::SystemNotice(SystemNoticeMessage::with_block(
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
                            data: "NESTED_IMAGE_BYTES".to_string(),
                        },
                    }],
                },
            ))],
        );

        let events = client.stream(&request).collect::<Vec<_>>().await;
        assert!(events.iter().all(Result::is_ok));
        let bodies = request_bodies.lock().expect("request body capture lock");
        assert_eq!(bodies.len(), 1);
        let encoded = serde_json::to_string(&bodies[0]).expect("encode request body");
        assert!(!encoded.contains("NESTED_IMAGE_BYTES"));
        assert!(!encoded.contains("input_image"));
        assert!(!encoded.contains("image_url"));
        assert!(encoded.contains("[image: image/png]"));
        drop(bodies);
        server.abort();
    }

    #[tokio::test]
    async fn compatible_responses_dispatch_strips_nested_images_from_captured_request() {
        assert_compatible_dispatch_strips_nested_image(OpenAiCompatibleMode::Responses).await;
    }

    #[tokio::test]
    async fn compatible_chat_dispatch_strips_nested_images_from_captured_request() {
        assert_compatible_dispatch_strips_nested_image(OpenAiCompatibleMode::ChatCompletions).await;
    }

    #[tokio::test]
    async fn compatible_stream_dispatch_serializes_exact_collapsed_tool_result() {
        for mode in [
            OpenAiCompatibleMode::Responses,
            OpenAiCompatibleMode::ChatCompletions,
        ] {
            let payload = match mode {
                OpenAiCompatibleMode::Responses => concat!(
                    "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"output\":[],\"usage\":{\"input_tokens\":1,\"output_tokens\":1}}}\n\n",
                    "data: {\"type\":\"response.done\",\"response\":{\"status\":\"completed\",\"output\":[],\"usage\":{\"input_tokens\":1,\"output_tokens\":1}}}\n\n"
                ),
                OpenAiCompatibleMode::ChatCompletions => concat!(
                    "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":1}}\n\n",
                    "data: [DONE]\n\n"
                ),
            }
            .to_string();
            let (base_url, request_bodies, server) =
                spawn_compatible_replay_capture_server(mode, payload).await;
            let client = OpenAiCompatibleClient::new_with_options(
                mode,
                "remote-model".to_string(),
                base_url,
                None,
                options(true, true, true, false),
            );
            let request = LlmRequest::new(
                "remote-model",
                vec![
                    Message::BlockAssistant(BlockAssistantMessage::new(
                        vec![AssistantBlock::ToolUse {
                            id: "call-1".to_string(),
                            name: "lookup".to_string(),
                            args: serde_json::value::RawValue::from_string("{}".to_string())
                                .expect("valid tool arguments"),
                            meta: None,
                        }],
                        StopReason::ToolUse,
                    )),
                    Message::tool_results(vec![ToolResult::with_blocks(
                        "call-1".to_string(),
                        vec![
                            ContentBlock::Text {
                                text: "first".to_string(),
                            },
                            ContentBlock::Text {
                                text: "second".to_string(),
                            },
                        ],
                        false,
                    )]),
                ],
            );

            let events = client.stream(&request).collect::<Vec<_>>().await;
            assert!(events.iter().all(Result::is_ok));
            let bodies = request_bodies.lock().expect("request body capture lock");
            assert_eq!(bodies.len(), 1);
            let encoded = serde_json::to_string(&bodies[0]).expect("encode request body");
            assert!(encoded.contains("first\\nsecond"));
            drop(bodies);
            server.abort();
        }
    }
}
