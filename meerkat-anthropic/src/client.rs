//! Anthropic Claude API client
//!
//! Implements the LlmClient trait for Anthropic's Claude API.

use async_trait::async_trait;
use futures::StreamExt;
use meerkat_core::lifecycle::run_primitive::{
    AnthropicCompactionConfig, AnthropicContextWindow, AnthropicEffort, AnthropicInferenceGeo,
    AnthropicProviderTag, AnthropicThinkingConfig, ProviderTag,
};
use meerkat_core::schema::{CompiledSchema, SchemaError};
use meerkat_core::{
    AssistantBlock, BlockAssistantMessage, ContentBlock, ImageData, Message, OutputSchema,
    Provider, StopReason, ToolResult, Usage,
};
use meerkat_llm_core::LlmError;
use meerkat_llm_core::{LlmClient, LlmDoneOutcome, LlmEvent, LlmRequest, LlmStream};
use meerkat_llm_core::{http, streaming};
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashSet;
use std::time::Duration;

/// Default connect timeout
const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
/// Default request timeout
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(300);
/// Default pool idle timeout
const DEFAULT_POOL_IDLE_TIMEOUT: Duration = Duration::from_secs(90);
/// SSE buffer capacity to reduce reallocations
const SSE_BUFFER_CAPACITY: usize = 4096;

/// Client for Anthropic API
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub struct AnthropicClient {
    api_key: String,
    base_url: String,
    http: reqwest::Client,
    connect_timeout: Duration,
    request_timeout: Duration,
    pool_idle_timeout: Duration,
    /// Dynamic authorizer — when set, replaces the `x-api-key` header
    /// path with `HttpAuthorizer::authorize` invocation (used for
    /// Vertex and Foundry backends where the Bearer token is acquired
    /// at request time from Google Auth / Azure AD).
    authorizer: Option<std::sync::Arc<dyn meerkat_core::HttpAuthorizer>>,
}

/// Builder for AnthropicClient
pub struct AnthropicClientBuilder {
    api_key: String,
    base_url: String,
    connect_timeout: Duration,
    request_timeout: Duration,
    pool_idle_timeout: Duration,
    authorizer: Option<std::sync::Arc<dyn meerkat_core::HttpAuthorizer>>,
}

impl AnthropicClientBuilder {
    pub fn new(api_key: String) -> Self {
        Self {
            api_key,
            base_url: "https://api.anthropic.com".to_string(),
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
            pool_idle_timeout: DEFAULT_POOL_IDLE_TIMEOUT,
            authorizer: None,
        }
    }

    /// Set custom base URL
    pub fn base_url(mut self, url: String) -> Self {
        self.base_url = url;
        self
    }

    /// Attach a dynamic authorizer (Vertex/Foundry Bearer token path).
    /// When set, `x-api-key` is NOT sent; the authorizer injects its
    /// own headers (typically `Authorization: Bearer <token>`).
    pub fn authorizer(
        mut self,
        authorizer: std::sync::Arc<dyn meerkat_core::HttpAuthorizer>,
    ) -> Self {
        self.authorizer = Some(authorizer);
        self
    }

    /// Set connection timeout
    pub fn connect_timeout(mut self, timeout: Duration) -> Self {
        self.connect_timeout = timeout;
        self
    }

    /// Set request timeout
    pub fn request_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = timeout;
        self
    }

    /// Set pool idle timeout
    pub fn pool_idle_timeout(mut self, timeout: Duration) -> Self {
        self.pool_idle_timeout = timeout;
        self
    }

    /// Build the client with configured HTTP settings
    pub fn build(self) -> Result<AnthropicClient, LlmError> {
        let base_url = self.base_url;
        let connect_timeout = self.connect_timeout;
        let request_timeout = self.request_timeout;
        let pool_idle_timeout = self.pool_idle_timeout;
        #[cfg(not(target_arch = "wasm32"))]
        let builder = reqwest::Client::builder()
            .connect_timeout(connect_timeout)
            .timeout(request_timeout)
            .pool_idle_timeout(pool_idle_timeout)
            .pool_max_idle_per_host(4)
            .tcp_keepalive(Duration::from_secs(30));
        #[cfg(target_arch = "wasm32")]
        let builder = reqwest::Client::builder();
        let http = http::build_http_client_for_base_url(builder, &base_url)?;

        Ok(AnthropicClient {
            api_key: self.api_key,
            base_url,
            http,
            connect_timeout,
            request_timeout,
            pool_idle_timeout,
            authorizer: self.authorizer,
        })
    }
}

/// Extract the typed Anthropic provider tag from a request, or `None` if
/// the request does not carry one (callers pass a different variant or
/// leave `provider_params` empty).
fn anthropic_tag(request: &LlmRequest) -> Option<&AnthropicProviderTag> {
    match request.provider_params.as_ref()? {
        ProviderTag::Anthropic(t) => Some(t),
        _ => None,
    }
}

fn catalog_beta_value(request: &LlmRequest, feature: &str) -> Option<&'static str> {
    meerkat_core::model_profile::capabilities::capabilities_for(
        Provider::Anthropic,
        &request.model,
    )?
    .beta_headers
    .iter()
    .find(|header| header.feature == feature)
    .map(|header| header.header_value)
}

fn catalog_context_beta_value(request: &LlmRequest) -> Option<&'static str> {
    let header = meerkat_core::model_profile::capabilities::capabilities_for(
        Provider::Anthropic,
        &request.model,
    )?
    .context_window_beta?
    .header;
    header.strip_prefix("anthropic-beta: ")
}

fn invalid_replay(message: impl Into<String>) -> LlmError {
    LlmError::InvalidRequest {
        message: message.into(),
    }
}

fn project_anthropic_content_blocks(blocks: &[ContentBlock]) -> Vec<ContentBlock> {
    blocks
        .iter()
        .map(|block| match block {
            ContentBlock::Text { .. } => block.clone(),
            ContentBlock::Image {
                data: ImageData::Inline { .. },
                ..
            } => block.clone(),
            ContentBlock::Image { .. } | ContentBlock::Video { .. } => ContentBlock::Text {
                text: block.text_projection().into_owned(),
            },
            _ => ContentBlock::Text {
                text: block.text_projection().into_owned(),
            },
        })
        .collect()
}

fn project_anthropic_tool_result(result: &ToolResult) -> Result<ToolResult, LlmError> {
    if result.has_video() {
        return Err(invalid_replay(
            "video blocks are not supported in Anthropic tool results",
        ));
    }
    Ok(ToolResult::with_blocks(
        result.tool_use_id.clone(),
        project_anthropic_content_blocks(&result.content),
        result.is_error,
    ))
}

fn anthropic_server_tool_content_replayable(content: &Value) -> bool {
    matches!(
        content.get("type").and_then(Value::as_str),
        Some("server_tool_use" | "web_search_tool_result")
    )
}

fn project_anthropic_assistant_blocks(blocks: &[AssistantBlock]) -> Vec<AssistantBlock> {
    blocks
        .iter()
        .filter_map(|block| match block {
            AssistantBlock::Text { text, .. } if text.is_empty() => None,
            AssistantBlock::Text { .. } | AssistantBlock::ToolUse { .. } => Some(block.clone()),
            AssistantBlock::Reasoning { meta, .. } => match meta.as_deref() {
                Some(meerkat_core::ProviderMeta::Anthropic { .. })
                | Some(meerkat_core::ProviderMeta::AnthropicRedacted { .. })
                | Some(meerkat_core::ProviderMeta::AnthropicCompaction { .. }) => {
                    Some(block.clone())
                }
                _ => None,
            },
            AssistantBlock::ServerToolContent { content, .. }
                if anthropic_server_tool_content_replayable(content) =>
            {
                Some(block.clone())
            }
            AssistantBlock::Image { .. } | AssistantBlock::ServerToolContent { .. } => None,
            _ => None,
        })
        .collect()
}

fn tool_ids_from_assistant(message: &Message) -> HashSet<String> {
    match message {
        Message::Assistant(assistant) => assistant
            .tool_calls
            .iter()
            .map(|tool_call| tool_call.id.clone())
            .collect(),
        Message::BlockAssistant(assistant) => assistant
            .blocks
            .iter()
            .filter_map(|block| match block {
                AssistantBlock::ToolUse { id, .. } => Some(id.clone()),
                _ => None,
            })
            .collect(),
        _ => HashSet::new(),
    }
}

fn validate_tool_results(
    provider: &str,
    pending: HashSet<String>,
    results: &[ToolResult],
) -> Result<(), LlmError> {
    let actual: HashSet<String> = results
        .iter()
        .map(|result| result.tool_use_id.clone())
        .collect();
    if actual == pending {
        Ok(())
    } else {
        Err(invalid_replay(format!(
            "{provider} replay projection found tool results that are not adjacent to matching tool uses"
        )))
    }
}

fn project_anthropic_replay_messages(messages: &[Message]) -> Result<Vec<Message>, LlmError> {
    let mut projected = Vec::with_capacity(messages.len());
    let mut pending_tool_ids: Option<HashSet<String>> = None;

    for message in messages {
        if let Message::ToolResults {
            results,
            created_at,
        } = message
        {
            let Some(pending) = pending_tool_ids.take() else {
                return Err(invalid_replay(
                    "Anthropic replay projection found tool results without preceding tool use",
                ));
            };
            validate_tool_results("Anthropic", pending, results)?;
            let results = results
                .iter()
                .map(project_anthropic_tool_result)
                .collect::<Result<Vec<_>, _>>()?;
            projected.push(Message::ToolResults {
                results,
                created_at: *created_at,
            });
            continue;
        }

        if pending_tool_ids.is_some() {
            return Err(invalid_replay(
                "Anthropic replay projection found a tool use without adjacent tool results",
            ));
        }

        let next_message = match message {
            Message::System(_) | Message::SystemNotice(_) => Some(message.clone()),
            Message::User(user) => Some(Message::User(meerkat_core::UserMessage {
                content: project_anthropic_content_blocks(&user.content),
                render_metadata: user.render_metadata.clone(),
                created_at: user.created_at,
            })),
            Message::Assistant(assistant) => {
                if assistant.content.is_empty() && assistant.tool_calls.is_empty() {
                    None
                } else {
                    Some(Message::Assistant(assistant.clone()))
                }
            }
            Message::BlockAssistant(assistant) => {
                let blocks = project_anthropic_assistant_blocks(&assistant.blocks);
                if blocks.is_empty() {
                    None
                } else {
                    Some(Message::BlockAssistant(BlockAssistantMessage {
                        blocks,
                        stop_reason: assistant.stop_reason,
                        created_at: assistant.created_at,
                    }))
                }
            }
            Message::ToolResults { .. } => unreachable!("handled above"),
        };

        if let Some(message) = next_message {
            let tool_ids = tool_ids_from_assistant(&message);
            if !tool_ids.is_empty() {
                pending_tool_ids = Some(tool_ids);
            }
            projected.push(message);
        }
    }

    if pending_tool_ids.is_some() {
        return Err(invalid_replay(
            "Anthropic replay projection found a trailing tool use without tool results",
        ));
    }

    Ok(projected)
}

impl AnthropicClient {
    fn model_supports_temperature(model: &str) -> bool {
        meerkat_core::model_profile::anthropic::supports_temperature(model)
    }

    fn request_supports_temperature(request: &LlmRequest) -> bool {
        anthropic_tag(request)
            .and_then(|t| t.supports_temperature_override)
            .unwrap_or_else(|| Self::model_supports_temperature(&request.model))
    }

    /// Create a new Anthropic client with the given API key and default HTTP settings
    pub fn new(api_key: String) -> Result<Self, LlmError> {
        AnthropicClientBuilder::new(api_key).build()
    }

    /// Create a builder for more control over HTTP configuration
    pub fn builder(api_key: String) -> AnthropicClientBuilder {
        AnthropicClientBuilder::new(api_key)
    }

    /// Set custom base URL
    pub fn with_base_url(mut self, url: String) -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        let builder = reqwest::Client::builder()
            .connect_timeout(self.connect_timeout)
            .timeout(self.request_timeout)
            .pool_idle_timeout(self.pool_idle_timeout)
            .pool_max_idle_per_host(4)
            .tcp_keepalive(Duration::from_secs(30));
        #[cfg(target_arch = "wasm32")]
        let builder = reqwest::Client::builder();
        if let Ok(http) = http::build_http_client_for_base_url(builder, &url) {
            self.http = http;
        }
        self.base_url = url;
        self
    }

    /// Build request body for Anthropic API
    fn build_request_body(&self, request: &LlmRequest) -> Result<Value, LlmError> {
        let mut messages = Vec::new();
        let mut system_prompt = None;

        for msg in &request.messages {
            match msg {
                Message::System(s) => {
                    system_prompt = Some(s.content.clone());
                }
                Message::SystemNotice(notice) => {
                    messages.push(serde_json::json!({
                        "role": "user",
                        "content": notice.rendered_text()
                    }));
                }
                Message::User(u) => {
                    if meerkat_core::has_non_text_content(&u.content) {
                        let content_array: Vec<Value> = u
                            .content
                            .iter()
                            .map(|block| match block {
                                ContentBlock::Text { text } => serde_json::json!({
                                    "type": "text",
                                    "text": text
                                }),
                                ContentBlock::Image { media_type, data } => match data {
                                    ImageData::Inline { data } => serde_json::json!({
                                        "type": "image",
                                        "source": {
                                            "type": "base64",
                                            "media_type": media_type,
                                            "data": data
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
                        messages.push(serde_json::json!({
                            "role": "user",
                            "content": content_array
                        }));
                    } else {
                        messages.push(serde_json::json!({
                            "role": "user",
                            "content": u.text_content()
                        }));
                    }
                }
                Message::Assistant(a) => {
                    // Legacy format: flat content + tool_calls
                    let mut content = Vec::new();

                    if !a.content.is_empty() {
                        content.push(serde_json::json!({
                            "type": "text",
                            "text": a.content
                        }));
                    }

                    for tc in &a.tool_calls {
                        content.push(serde_json::json!({
                            "type": "tool_use",
                            "id": tc.id,
                            "name": tc.name,
                            "input": tc.args
                        }));
                    }

                    messages.push(serde_json::json!({
                        "role": "assistant",
                        "content": content
                    }));
                }
                Message::BlockAssistant(a) => {
                    // New format: ordered blocks with thinking support
                    let mut content = Vec::new();

                    for block in &a.blocks {
                        match block {
                            meerkat_core::AssistantBlock::Text { text, .. } => {
                                // Anthropic text blocks don't use meta
                                if !text.is_empty() {
                                    content.push(serde_json::json!({
                                        "type": "text",
                                        "text": text
                                    }));
                                }
                            }
                            meerkat_core::AssistantBlock::Reasoning { text, meta } => {
                                match meta.as_deref() {
                                    Some(meerkat_core::ProviderMeta::Anthropic { signature }) => {
                                        // Regular thinking block with signature for replay
                                        content.push(serde_json::json!({
                                            "type": "thinking",
                                            "thinking": text,
                                            "signature": signature
                                        }));
                                    }
                                    Some(meerkat_core::ProviderMeta::AnthropicRedacted {
                                        data,
                                    }) => {
                                        // Redacted thinking — pass encrypted data verbatim
                                        content.push(serde_json::json!({
                                            "type": "redacted_thinking",
                                            "data": data
                                        }));
                                    }
                                    Some(meerkat_core::ProviderMeta::AnthropicCompaction {
                                        content: summary,
                                    }) => {
                                        // Compaction summary — replaces prior context
                                        content.push(serde_json::json!({
                                            "type": "compaction",
                                            "content": summary
                                        }));
                                    }
                                    _ => {
                                        tracing::warn!(
                                            "thinking block missing Anthropic signature, skipping"
                                        );
                                        continue;
                                    }
                                }
                            }
                            meerkat_core::AssistantBlock::ToolUse { id, name, args, .. } => {
                                // Parse RawValue to Value for JSON serialization
                                let args_value: Value = serde_json::from_str(args.get())
                                    .unwrap_or_else(|_| serde_json::json!({}));
                                content.push(serde_json::json!({
                                    "type": "tool_use",
                                    "id": id,
                                    "name": name,
                                    "input": args_value
                                }));
                            }
                            meerkat_core::AssistantBlock::ServerToolContent {
                                id,
                                name,
                                content: server_content,
                                ..
                            } => {
                                if server_content.get("type").and_then(Value::as_str)
                                    == Some("server_tool_use")
                                {
                                    content.push(serde_json::json!({
                                        "type": "server_tool_use",
                                        "id": id,
                                        "name": name,
                                        "input": server_content
                                            .get("input")
                                            .cloned()
                                            .unwrap_or_else(|| serde_json::json!({}))
                                    }));
                                } else {
                                    let mut block = server_content.clone();
                                    if let Some(object) = block.as_object_mut() {
                                        object.insert(
                                            "type".to_string(),
                                            Value::String("web_search_tool_result".to_string()),
                                        );
                                        if let Some(tool_use_id) = id {
                                            object.insert(
                                                "tool_use_id".to_string(),
                                                Value::String(tool_use_id.clone()),
                                            );
                                        }
                                    }
                                    content.push(block);
                                }
                            }
                            // Handle future block types (non_exhaustive pattern)
                            _ => {}
                        }
                    }

                    messages.push(serde_json::json!({
                        "role": "assistant",
                        "content": content
                    }));
                }
                Message::ToolResults { results, .. } => {
                    let mut content = Vec::new();

                    for r in results {
                        if r.has_video() {
                            return Err(LlmError::InvalidRequest {
                                message: "video blocks are not supported in Anthropic tool results"
                                    .to_string(),
                            });
                        }
                        if r.has_images() {
                            let result_content: Vec<Value> = r
                                .content
                                .iter()
                                .map(|block| match block {
                                    ContentBlock::Text { text } => serde_json::json!({
                                        "type": "text",
                                        "text": text
                                    }),
                                    ContentBlock::Image {
                                        media_type,
                                        data: ImageData::Inline { data },
                                    } => serde_json::json!({
                                        "type": "image",
                                        "source": {
                                            "type": "base64",
                                            "media_type": media_type,
                                            "data": data
                                        }
                                    }),
                                    ContentBlock::Image { .. } => serde_json::json!({
                                        "type": "text",
                                        "text": block.text_projection()
                                    }),
                                    _ => serde_json::json!({
                                        "type": "text",
                                        "text": block.text_projection()
                                    }),
                                })
                                .collect();
                            content.push(serde_json::json!({
                                "type": "tool_result",
                                "tool_use_id": r.tool_use_id,
                                "content": result_content,
                                "is_error": r.is_error
                            }));
                        } else {
                            content.push(serde_json::json!({
                                "type": "tool_result",
                                "tool_use_id": r.tool_use_id,
                                "content": r.text_content(),
                                "is_error": r.is_error
                            }));
                        }
                    }

                    messages.push(serde_json::json!({
                        "role": "user",
                        "content": content
                    }));
                }
            }
        }

        let mut body = serde_json::json!({
            "model": request.model,
            "max_tokens": request.max_tokens,
            "messages": messages,
            "stream": true
        });

        if let Some(system) = system_prompt {
            body["system"] = Value::String(system);
        }

        if Self::request_supports_temperature(request)
            && let Some(temp) = request.temperature
            && let Some(num) = serde_json::Number::from_f64(temp as f64)
        {
            body["temperature"] = Value::Number(num);
        }

        if !request.tools.is_empty() {
            let tools: Vec<Value> = request
                .tools
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "name": t.name,
                        "description": t.description,
                        "input_schema": t.input_schema
                    })
                })
                .collect();
            body["tools"] = Value::Array(tools);
        }

        // Inject provider-native web search tool from the typed tag.
        if let Some(web_search) = anthropic_tag(request).and_then(|t| t.web_search.as_ref()) {
            let ws_value = web_search.as_value();
            if ws_value.is_object() {
                match body.get_mut("tools").and_then(|v| v.as_array_mut()) {
                    Some(arr) => arr.push(ws_value),
                    None => body["tools"] = Value::Array(vec![ws_value]),
                }
            }
        }

        if let Some(tag) = anthropic_tag(request) {
            // Thinking: typed enum captures adaptive vs enabled;
            // legacy flat `thinking_budget_tokens` maps to enabled.
            if let Some(cfg) = tag.thinking.as_ref() {
                body["thinking"] = match cfg {
                    AnthropicThinkingConfig::Adaptive => serde_json::json!({"type": "adaptive"}),
                    AnthropicThinkingConfig::Enabled { budget_tokens } => serde_json::json!({
                        "type": "enabled",
                        "budget_tokens": budget_tokens,
                    }),
                };
            } else if let Some(budget) = tag.thinking_budget_tokens {
                body["thinking"] = serde_json::json!({
                    "type": "enabled",
                    "budget_tokens": budget,
                });
            }

            if let Some(top_k) = tag.top_k {
                body["top_k"] = Value::Number(serde_json::Number::from(top_k));
            }

            if let Some(effort) = tag.effort {
                let effort_str = match effort {
                    AnthropicEffort::Low => "low",
                    AnthropicEffort::Medium => "medium",
                    AnthropicEffort::High => "high",
                    AnthropicEffort::Max => "max",
                    AnthropicEffort::XHigh => "xhigh",
                };
                if body.get("output_config").is_none() {
                    body["output_config"] = serde_json::json!({});
                }
                body["output_config"]["effort"] = Value::String(effort_str.to_string());
            }

            if let Some(output_schema) = tag.structured_output.as_ref() {
                let compiled =
                    self.compile_schema(output_schema)
                        .map_err(|e| LlmError::InvalidRequest {
                            message: e.to_string(),
                        })?;
                if body.get("output_config").is_none() {
                    body["output_config"] = serde_json::json!({});
                }
                body["output_config"]["format"] = serde_json::json!({
                    "type": "json_schema",
                    "schema": compiled.schema
                });
            }

            if let Some(geo) = tag.inference_geo.as_ref() {
                let geo_str = match geo {
                    AnthropicInferenceGeo::Us => "us".to_string(),
                    AnthropicInferenceGeo::Global => "global".to_string(),
                    AnthropicInferenceGeo::Other { region } => region.clone(),
                };
                body["inference_geo"] = Value::String(geo_str);
            }

            if let Some(compaction) = tag.compaction.as_ref() {
                match compaction {
                    AnthropicCompactionConfig::Auto => {
                        body["context_management"] = serde_json::json!({
                            "edits": [{"type": "compact_20260112"}]
                        });
                    }
                    AnthropicCompactionConfig::Custom { edit } => {
                        let mut edit_value = serde_json::json!({"type": "compact_20260112"});
                        if let Some(obj) = edit.as_value().as_object() {
                            for (k, v) in obj {
                                edit_value[k] = v.clone();
                            }
                        }
                        body["context_management"] = serde_json::json!({
                            "edits": [edit_value]
                        });
                    }
                }
            }
        }

        Ok(body)
    }

    /// Parse an SSE event from the response
    fn parse_sse_line(line: &str) -> Option<AnthropicEvent> {
        if let Some(data) = Self::strip_data_prefix(line) {
            serde_json::from_str(data).ok()
        } else {
            None
        }
    }

    fn strip_data_prefix(line: &str) -> Option<&str> {
        line.strip_prefix("data: ")
            .or_else(|| line.strip_prefix("data:"))
            .map(str::trim_start)
    }

    fn map_stop_reason(reason: &str) -> StopReason {
        match reason {
            "tool_use" => StopReason::ToolUse,
            "max_tokens" => StopReason::MaxTokens,
            "stop_sequence" => StopReason::StopSequence,
            "content_filter" => StopReason::ContentFilter,
            // "end_turn" and any unrecognized reason default to EndTurn
            _ => StopReason::EndTurn,
        }
    }
}

/// Recursively add `additionalProperties: false` to all object schemas that
/// have `properties` but no explicit `additionalProperties`.
///
/// This is required by Anthropic's structured output API.
fn ensure_additional_properties_false(value: &mut Value) {
    match value {
        Value::Object(obj) => {
            let is_object_type = match obj.get("type") {
                Some(Value::String(t)) => t == "object",
                Some(Value::Array(types)) => types.iter().any(|t| t.as_str() == Some("object")),
                _ => obj.contains_key("properties"),
            };
            if is_object_type
                && obj.contains_key("properties")
                && !obj.contains_key("additionalProperties")
            {
                obj.insert("additionalProperties".to_string(), Value::Bool(false));
            }

            let keys: Vec<String> = obj.keys().cloned().collect();
            for key in keys {
                if let Some(child) = obj.get_mut(&key) {
                    ensure_additional_properties_false(child);
                }
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

fn merge_usage(target: &mut Usage, update: &AnthropicUsage) {
    if let Some(v) = update.input_tokens {
        target.input_tokens = v;
    }
    if let Some(v) = update.output_tokens {
        target.output_tokens = v;
    }
    if let Some(v) = update.cache_creation_input_tokens {
        target.cache_creation_tokens = Some(v);
    }
    if let Some(v) = update.cache_read_input_tokens {
        target.cache_read_tokens = Some(v);
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl LlmClient for AnthropicClient {
    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        project_anthropic_replay_messages(messages)
    }

    fn stream<'a>(&'a self, request: &'a LlmRequest) -> LlmStream<'a> {
        let inner: LlmStream<'a> = Box::pin(async_stream::try_stream! {
            let mut projected_request = request.clone();
            projected_request.messages = self.project_replay_messages(&request.messages)?;
            let request = &projected_request;
            let body = self.build_request_body(request)?;

            // Collect beta headers based on request features
            let mut betas = Vec::new();

            // Legacy thinking (type: "enabled") requires interleaved-thinking header
            // Adaptive thinking (Opus 4.6) does NOT need this header
            let thinking_type = body.get("thinking")
                .and_then(|t| t.get("type"))
                .and_then(|t| t.as_str());
            if thinking_type == Some("enabled")
                && let Some(beta) = catalog_beta_value(request, "interleaved_thinking")
            {
                betas.push(beta);
            }

            // Structured output format requires beta header
            if body.get("output_config").and_then(|c| c.get("format")).is_some()
                && let Some(beta) = catalog_beta_value(request, "structured_output")
            {
                betas.push(beta);
            }

            // 1M context window (opt-in via typed AnthropicProviderTag.context)
            if anthropic_tag(request).and_then(|t| t.context)
                == Some(AnthropicContextWindow::OneMegabyte)
                && let Some(beta) = catalog_context_beta_value(request)
            {
                betas.push(beta);
            }

            // Compaction API (beta)
            if body.get("context_management").is_some()
                && let Some(beta) = catalog_beta_value(request, "compaction")
            {
                betas.push(beta);
            }

            let url = format!("{}/v1/messages", self.base_url);
            let mut req = self.http
                .post(&url)
                .header("anthropic-version", "2023-06-01")
                .header("Content-Type", "application/json");

            // Auth: dynamic authorizer takes precedence over x-api-key.
            #[cfg(not(target_arch = "wasm32"))]
            if let Some(authorizer) = &self.authorizer {
                let mut extra: Vec<(String, String)> = Vec::new();
                let mut auth_req = meerkat_core::HttpAuthorizationRequest {
                    method: "POST",
                    url: &url,
                    headers: &mut extra,
                };
                authorizer.authorize(&mut auth_req).await.map_err(|e| {
                    LlmError::AuthenticationFailed {
                        message: format!("authorizer failed: {e}"),
                    }
                })?;
                for (k, v) in extra {
                    req = req.header(k, v);
                }
            } else {
                req = req.header("x-api-key", &self.api_key);
            }
            #[cfg(target_arch = "wasm32")]
            {
                req = req.header("x-api-key", &self.api_key);
            }

            if !betas.is_empty() {
                req = req.header("anthropic-beta", betas.join(","));
            }

            // On wasm32 (browser), Anthropic requires this header for CORS
            #[cfg(target_arch = "wasm32")]
            {
                req = req.header("anthropic-dangerous-direct-browser-access", "true");
            }

            let response = req
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
                let headers = response.headers().clone();
                let text = response.text().await.unwrap_or_default();
                Err(LlmError::from_http_response(status_code, text, &headers))
            };
            let mut stream = stream_result?;
            let mut buffer = String::with_capacity(SSE_BUFFER_CAPACITY);
            let mut current_tool_id: Option<String> = None;
            let mut current_tool_name: Option<String> = None;
            let mut accumulated_tool_args = String::new();
            let mut current_server_tool_id: Option<String> = None;
            let mut current_server_tool_name: Option<String> = None;
            let mut accumulated_server_tool_input = String::new();
            let mut current_block_type: Option<String> = None;
            let mut current_thinking_signature: Option<String> = None;
            let mut last_stop_reason: Option<StopReason> = None;
            let mut usage = Usage::default();
            let mut saw_done = false;
            let mut saw_event = false;

            macro_rules! handle_event {
                ($event:expr) => {
                    match $event.event_type.as_str() {
                        "content_block_delta" => {
                            if let Some(delta) = $event.delta {
                                match delta.delta_type.as_str() {
                                    "text_delta" => {
                                        if let Some(text) = delta.text {
                                            saw_event = true;
                                            yield LlmEvent::TextDelta { delta: text, meta: None };
                                        }
                                    }
                                    "thinking_delta" => {
                                        // Emit incremental thinking content
                                        if let Some(text) = delta.thinking {
                                            saw_event = true;
                                            yield LlmEvent::ReasoningDelta { delta: text };
                                        }
                                    }
                                    "signature_delta" => {
                                        // Signature arrives as separate delta before content_block_stop
                                        if let Some(sig) = delta.signature {
                                            saw_event = true;
                                            current_thinking_signature = Some(sig);
                                        }
                                    }
                                    "input_json_delta" => {
                                        if let Some(partial_json) = delta.partial_json {
                                            saw_event = true;
                                            if current_block_type.as_deref() == Some("server_tool_use") {
                                                accumulated_server_tool_input.push_str(&partial_json);
                                            } else {
                                                accumulated_tool_args.push_str(&partial_json);
                                                yield LlmEvent::ToolCallDelta {
                                                    id: current_tool_id.clone().unwrap_or_default(),
                                                    name: None,
                                                    args_delta: partial_json,
                                                };
                                            }
                                        }
                                    }
                                    "compaction_delta" => {
                                        // Compaction summary arrives as single delta — emit as Reasoning with compaction meta
                                        if let Some(content) = delta.content {
                                            saw_event = true;
                                            yield LlmEvent::ReasoningComplete {
                                                text: String::new(),
                                                meta: Some(Box::new(meerkat_core::ProviderMeta::AnthropicCompaction { content })),
                                            };
                                        }
                                    }
                                    _ => {}
                                }
                            }
                        }
                        "content_block_start" => {
                            if let Some(content_block) = $event.content_block {
                                current_block_type = Some(content_block.block_type.clone());
                                match content_block.block_type.as_str() {
                                    "text" => {
                                        if let Some(citations) = content_block.extra.get("citations") {
                                            saw_event = true;
                                            yield LlmEvent::ServerToolContent {
                                                id: None,
                                                name: "web_search_citations".to_string(),
                                                content: serde_json::json!({
                                                    "type": "text_citations",
                                                    "citations": citations
                                                }),
                                                meta: None,
                                            };
                                        }
                                    }
                                    "thinking" => {
                                        // Start of thinking block
                                        // Signature may already be present or arrive via signature_delta
                                        current_thinking_signature = content_block.signature;
                                    }
                                    "redacted_thinking" => {
                                        // Redacted by safety systems — emit as Reasoning with redacted meta
                                        if let Some(data) = content_block.data {
                                            saw_event = true;
                                            yield LlmEvent::ReasoningComplete {
                                                text: String::new(),
                                                meta: Some(Box::new(meerkat_core::ProviderMeta::AnthropicRedacted { data })),
                                            };
                                        }
                                    }
                                    "compaction" => {
                                        // Compaction block start — content arrives via compaction_delta
                                    }
                                    "tool_use" => {
                                        let id = content_block.id.unwrap_or_default();
                                        current_tool_id = Some(id.clone());
                                        current_tool_name = content_block.name.clone();
                                        accumulated_tool_args.clear();
                                        saw_event = true;
                                        yield LlmEvent::ToolCallDelta {
                                            id,
                                            name: content_block.name,
                                            args_delta: String::new(),
                                        };
                                    }
                                    "server_tool_use" => {
                                        current_server_tool_id = content_block.id.clone();
                                        current_server_tool_name = content_block.name.clone();
                                        accumulated_server_tool_input.clear();
                                    }
                                    "web_search_tool_result" => {
                                        saw_event = true;
                                        yield LlmEvent::ServerToolContent {
                                            id: content_block.tool_use_id.clone(),
                                            name: "web_search".to_string(),
                                            content: content_block.extra.clone(),
                                            meta: None,
                                        };
                                    }
                                    _ => {}
                                }
                            }
                        }
                        "content_block_stop" => {
                            // Emit complete events based on block type
                            match current_block_type.as_deref() {
                                Some("thinking") => {
                                    // Emit ReasoningComplete with Anthropic signature
                                    let meta = current_thinking_signature
                                        .take()
                                        .map(|sig| Box::new(meerkat_core::ProviderMeta::Anthropic { signature: sig }));
                                    yield LlmEvent::ReasoningComplete {
                                        text: String::new(), // Text was already streamed via deltas
                                        meta,
                                    };
                                }
                                Some("tool_use") => {
                                    // Finalize tool call: assemble accumulated
                                    // JSON args from deltas into a complete event.
                                    if let Some(ref tool_id) = current_tool_id {
                                        // Accumulate args from the buffer by
                                        // reading the accumulated json string.
                                        // The name was set in content_block_start
                                        // and passed via the initial ToolCallDelta.
                                        // We emit ToolCallComplete with the
                                        // accumulated_tool_args (collected during
                                        // input_json_delta events).
                                        match parse_streamed_tool_args(
                                            &accumulated_tool_args,
                                            tool_id,
                                        ) {
                                            Ok(args_val) => {
                                                saw_event = true;
                                                yield LlmEvent::ToolCallComplete {
                                                    id: tool_id.clone(),
                                                    name: current_tool_name.take().unwrap_or_default(),
                                                    args: args_val,
                                                    meta: None,
                                                };
                                            }
                                            Err(error) => {
                                                saw_event = true;
                                                if !saw_done {
                                                    yield LlmEvent::Done {
                                                        outcome: LlmDoneOutcome::Error { error },
                                                    };
                                                    saw_done = true;
                                                }
                                            }
                                        }
                                        accumulated_tool_args.clear();
                                    }
                                    current_tool_id = None;
                                }
                                Some("server_tool_use") => {
                                    let input = if accumulated_server_tool_input.is_empty() {
                                        serde_json::json!({})
                                    } else {
                                        serde_json::from_str(&accumulated_server_tool_input)
                                            .unwrap_or_else(|_| serde_json::json!({
                                                "partial_json": accumulated_server_tool_input
                                            }))
                                    };
                                    let id = current_server_tool_id.take();
                                    let name = current_server_tool_name
                                        .take()
                                        .unwrap_or_else(|| "server_tool".to_string());
                                    accumulated_server_tool_input.clear();
                                    saw_event = true;
                                    yield LlmEvent::ServerToolContent {
                                        id,
                                        name,
                                        content: serde_json::json!({
                                            "type": "server_tool_use",
                                            "input": input
                                        }),
                                        meta: None,
                                    };
                                }
                                _ => {}
                            }
                            current_block_type = None;
                        }
                        "message_delta" => {
                            if let Some(usage_update) = $event.usage {
                                merge_usage(&mut usage, &usage_update);
                                saw_event = true;
                                yield LlmEvent::UsageUpdate {
                                    usage: usage.clone(),
                                };
                            }
                            if let Some(finish_reason) = $event.delta.and_then(|d| d.stop_reason) {
                                let reason = Self::map_stop_reason(finish_reason.as_str());
                                last_stop_reason = Some(reason);
                                if !saw_done {
                                    saw_event = true;
                                    yield LlmEvent::Done {
                                        outcome: LlmDoneOutcome::Success { stop_reason: reason },
                                    };
                                    saw_done = true;
                                }
                            }
                        }
                        "message_start" => {
                            if let Some(usage_update) = $event.message.and_then(|m| m.usage) {
                                merge_usage(&mut usage, &usage_update);
                                saw_event = true;
                                yield LlmEvent::UsageUpdate {
                                    usage: usage.clone(),
                                };
                            }
                        }
                        "message_stop" => {
                            if !saw_done {
                                let finish_reason = $event.delta.and_then(|d| d.stop_reason);
                                let reason = finish_reason
                                    .as_deref()
                                    .map(Self::map_stop_reason)
                                    .or(last_stop_reason)
                                    .unwrap_or(StopReason::EndTurn);
                                last_stop_reason = Some(reason);
                                saw_event = true;
                                yield LlmEvent::Done {
                                    outcome: LlmDoneOutcome::Success { stop_reason: reason },
                                };
                                saw_done = true;
                            }
                        }
                        "error" => {
                            // Anthropic streaming error (e.g. overloaded_error mid-stream)
                            let error_msg = $event.error
                                .as_ref()
                                .and_then(|e| e.get("message"))
                                .and_then(|m| m.as_str())
                                .unwrap_or("unknown streaming error");
                            let error_type = $event.error
                                .as_ref()
                                .and_then(|e| e.get("type"))
                                .and_then(|t| t.as_str())
                                .unwrap_or("unknown");

                            tracing::error!(
                                error_type,
                                error_msg,
                                "Anthropic streaming error"
                            );

                            let error = match error_type {
                                "overloaded_error" => LlmError::ServerOverloaded,
                                "rate_limit_error" => LlmError::RateLimited { retry_after_ms: None },
                                "api_error" => LlmError::ServerError {
                                    status: 500,
                                    message: error_msg.to_string(),
                                },
                                _ => LlmError::Unknown {
                                    message: format!("{error_type}: {error_msg}"),
                                },
                            };

                            if !saw_done {
                                saw_event = true;
                                yield LlmEvent::Done {
                                    outcome: LlmDoneOutcome::Error { error },
                                };
                                saw_done = true;
                            }
                        }
                        _ => {}
                    }
                };
            }

            macro_rules! handle_line {
                ($line:expr) => {
                    if !$line.is_empty() {
                        if let Some(data) = Self::strip_data_prefix($line) {
                            if data == "[DONE]" {
                                if !saw_done {
                                    let reason =
                                        last_stop_reason.unwrap_or(StopReason::EndTurn);
                                    saw_event = true;
                                    yield LlmEvent::Done {
                                        outcome: LlmDoneOutcome::Success { stop_reason: reason },
                                    };
                                    saw_done = true;
                                }
                            } else if let Some(event) = Self::parse_sse_line($line) {
                                handle_event!(event);
                            }
                        }
                    }
                };
            }

            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|_| LlmError::ConnectionReset)?;
                buffer.push_str(&String::from_utf8_lossy(&chunk));

                while let Some(newline_pos) = buffer.find('\n') {
                    let line = buffer[..newline_pos].trim();
                    handle_line!(line);
                    buffer.drain(..=newline_pos);
                }
            }

            if !buffer.is_empty() {
                for line in buffer.lines() {
                    let line = line.trim();
                    handle_line!(line);
                }
            }

            if !saw_done && saw_event {
                tracing::warn!(
                    model = %request.model,
                    "Anthropic stream ended without terminal event; emitting synthetic Done"
                );
                let reason = last_stop_reason.unwrap_or(StopReason::EndTurn);
                yield LlmEvent::Done {
                    outcome: LlmDoneOutcome::Success { stop_reason: reason },
                };
            }
        });

        streaming::ensure_terminal_done(inner)
    }

    fn provider(&self) -> &'static str {
        "anthropic"
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }

    fn compile_schema(&self, output_schema: &OutputSchema) -> Result<CompiledSchema, SchemaError> {
        let mut schema = output_schema.schema.as_value().clone();
        ensure_additional_properties_false(&mut schema);

        Ok(CompiledSchema {
            schema,
            warnings: Vec::new(),
        })
    }
}

#[derive(Debug, Deserialize)]
struct AnthropicEvent {
    #[serde(rename = "type")]
    event_type: String,
    delta: Option<AnthropicDelta>,
    content_block: Option<AnthropicContentBlock>,
    message: Option<AnthropicMessage>,
    usage: Option<AnthropicUsage>,
    /// Error object for streaming error events
    error: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct AnthropicDelta {
    #[serde(rename = "type", default)]
    delta_type: String,
    text: Option<String>,
    partial_json: Option<String>,
    stop_reason: Option<String>,
    /// Thinking content for thinking_delta events
    thinking: Option<String>,
    /// Signature for signature_delta events (arrives separately before content_block_stop)
    signature: Option<String>,
    /// Content for compaction_delta events
    content: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AnthropicContentBlock {
    #[serde(rename = "type")]
    block_type: String,
    id: Option<String>,
    name: Option<String>,
    tool_use_id: Option<String>,
    /// Signature for thinking block continuity
    signature: Option<String>,
    /// Encrypted data for redacted_thinking blocks
    data: Option<String>,
    #[serde(flatten)]
    extra: Value,
}

fn parse_streamed_tool_args(args: &str, tool_id: &str) -> Result<Value, LlmError> {
    let args = if args.is_empty() { "{}" } else { args };
    let value: Value = serde_json::from_str(args).map_err(|error| LlmError::StreamParseError {
        message: format!("invalid Anthropic tool call arguments JSON for {tool_id}: {error}"),
    })?;
    if value.is_object() {
        Ok(value)
    } else {
        Err(LlmError::StreamParseError {
            message: format!("Anthropic tool call arguments for {tool_id} must be a JSON object"),
        })
    }
}

#[derive(Debug, Deserialize)]
struct AnthropicMessage {
    usage: Option<AnthropicUsage>,
}

#[derive(Debug, Deserialize)]
struct AnthropicUsage {
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    cache_creation_input_tokens: Option<u64>,
    cache_read_input_tokens: Option<u64>,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use meerkat_core::{
        AssistantBlock, AssistantImageId, BlobId, BlobRef, BlockAssistantMessage, ContentBlock,
        ImageData, MediaType, ProviderImageMetadata, ProviderMeta, RevisedPromptDisposition,
        ToolResult, UserMessage,
    };

    fn assistant_image_block() -> AssistantBlock {
        AssistantBlock::Image {
            image_id: AssistantImageId::new(meerkat_core::time_compat::new_uuid_v7()),
            blob_ref: BlobRef {
                blob_id: BlobId::from("anthropic-image"),
                media_type: "image/png".to_string(),
            },
            media_type: MediaType::new("image/png"),
            width: 128,
            height: 128,
            revised_prompt: RevisedPromptDisposition::NotRequested,
            meta: ProviderImageMetadata::NotEmitted,
        }
    }

    // =========================================================================
    // Thinking block SSE parsing tests (spec section 3.5)
    // =========================================================================

    #[test]
    fn replay_projection_policy_handles_anthropic_history_blocks()
    -> Result<(), Box<dyn std::error::Error>> {
        let client = AnthropicClient::new("test-key".to_string())?;
        let tool_args = serde_json::value::RawValue::from_string(r#"{"query":"m"}"#.to_string())?;
        let messages = vec![
            Message::User(UserMessage::with_blocks(vec![
                ContentBlock::Text {
                    text: "look at this".to_string(),
                },
                ContentBlock::Image {
                    media_type: "image/png".to_string(),
                    data: ImageData::Inline {
                        data: "AAAA".to_string(),
                    },
                },
                ContentBlock::Video {
                    media_type: "video/mp4".to_string(),
                    duration_ms: 1_000,
                    data: meerkat_core::VideoData::Inline {
                        data: "BBBB".to_string(),
                    },
                },
            ])),
            Message::BlockAssistant(BlockAssistantMessage::new(
                vec![
                    AssistantBlock::Reasoning {
                        text: "signed plan".to_string(),
                        meta: Some(Box::new(ProviderMeta::Anthropic {
                            signature: "sig_1".to_string(),
                        })),
                    },
                    AssistantBlock::Reasoning {
                        text: "unsigned plan".to_string(),
                        meta: None,
                    },
                    AssistantBlock::ServerToolContent {
                        id: Some("srv_1".to_string()),
                        name: "web_search".to_string(),
                        content: serde_json::json!({
                            "type": "server_tool_use",
                            "input": {"query": "m"}
                        }),
                        meta: None,
                    },
                    AssistantBlock::ServerToolContent {
                        id: None,
                        name: "web_search_citations".to_string(),
                        content: serde_json::json!({
                            "type": "text_citations",
                            "citations": []
                        }),
                        meta: None,
                    },
                    assistant_image_block(),
                    AssistantBlock::Text {
                        text: "answer".to_string(),
                        meta: None,
                    },
                    AssistantBlock::ToolUse {
                        id: "tool_1".to_string(),
                        name: "lookup".to_string(),
                        args: tool_args,
                        meta: None,
                    },
                ],
                StopReason::ToolUse,
            )),
            Message::tool_results(vec![ToolResult::with_blocks(
                "tool_1".to_string(),
                vec![
                    ContentBlock::Text {
                        text: "tool text".to_string(),
                    },
                    ContentBlock::Image {
                        media_type: "image/png".to_string(),
                        data: ImageData::Inline {
                            data: "CCCC".to_string(),
                        },
                    },
                ],
                false,
            )]),
        ];

        let projected = client.project_replay_messages(&messages)?;

        let Message::User(user) = &projected[0] else {
            panic!("expected user message");
        };
        assert!(
            user.content
                .iter()
                .any(|block| matches!(block, ContentBlock::Image { .. })),
            "Anthropic should retain inline user images"
        );
        assert!(
            user.content.iter().any(|block| matches!(
                block,
                ContentBlock::Text { text } if text == "[video: video/mp4]"
            )),
            "Anthropic should degrade user video to text"
        );
        assert!(
            !user
                .content
                .iter()
                .any(|block| matches!(block, ContentBlock::Video { .. })),
            "Anthropic projection should not leave video blocks"
        );

        let Message::BlockAssistant(assistant) = &projected[1] else {
            panic!("expected assistant message");
        };
        assert!(assistant.blocks.iter().any(|block| matches!(
            block,
            AssistantBlock::Reasoning {
                text,
                meta: Some(meta),
            } if text == "signed plan"
                && matches!(meta.as_ref(), ProviderMeta::Anthropic { .. })
        )));
        assert!(!assistant.blocks.iter().any(|block| matches!(
            block,
            AssistantBlock::Reasoning { text, .. } if text == "unsigned plan"
        )));
        assert!(assistant.blocks.iter().any(|block| matches!(
            block,
            AssistantBlock::ServerToolContent { content, .. }
                if content.get("type").and_then(Value::as_str) == Some("server_tool_use")
        )));
        assert!(!assistant.blocks.iter().any(|block| matches!(
            block,
            AssistantBlock::ServerToolContent { content, .. }
                if content.get("type").and_then(Value::as_str) == Some("text_citations")
        )));
        assert!(
            !assistant
                .blocks
                .iter()
                .any(|block| matches!(block, AssistantBlock::Image { .. })),
            "assistant images must be removed from Anthropic replay"
        );

        let Message::ToolResults { results, .. } = &projected[2] else {
            panic!("expected tool results");
        };
        assert!(
            results[0].has_images(),
            "Anthropic should retain inline image tool results"
        );
        Ok(())
    }

    #[test]
    fn replay_projection_rejects_anthropic_orphan_tool_results() {
        let client = AnthropicClient::new("test-key".to_string()).expect("client");
        let err = client
            .project_replay_messages(&[Message::tool_results(vec![ToolResult::new(
                "tool_1".to_string(),
                "orphaned".to_string(),
                false,
            )])])
            .expect_err("orphan tool results must be rejected");
        assert!(matches!(err, LlmError::InvalidRequest { .. }));
    }

    #[test]
    fn test_anthropic_content_block_parses_thinking_type() {
        // content_block_start with type: "thinking" should parse
        let json = r#"{"type": "thinking", "thinking": "Let me analyze..."}"#;
        let block: AnthropicContentBlock = serde_json::from_str(json).unwrap();

        assert_eq!(block.block_type, "thinking");
    }

    #[test]
    fn test_anthropic_content_block_parses_thinking_with_signature() {
        // content_block_start may have signature already
        let json = r#"{"type": "thinking", "thinking": "", "signature": "sig_abc123"}"#;
        let block: AnthropicContentBlock = serde_json::from_str(json).unwrap();

        assert_eq!(block.block_type, "thinking");
        assert_eq!(block.signature.as_deref(), Some("sig_abc123"));
    }

    #[test]
    fn test_anthropic_delta_parses_thinking_delta() {
        // content_block_delta with delta type: "thinking_delta"
        let json = r#"{"type": "thinking_delta", "thinking": "I need to consider..."}"#;
        let delta: AnthropicDelta = serde_json::from_str(json).unwrap();

        assert_eq!(delta.delta_type, "thinking_delta");
        assert_eq!(delta.thinking.as_deref(), Some("I need to consider..."));
    }

    #[test]
    fn test_anthropic_delta_parses_signature_delta() {
        // content_block_delta with delta type: "signature_delta"
        let json = r#"{"type": "signature_delta", "signature": "sig_xyz789"}"#;
        let delta: AnthropicDelta = serde_json::from_str(json).unwrap();

        assert_eq!(delta.delta_type, "signature_delta");
        assert_eq!(delta.signature.as_deref(), Some("sig_xyz789"));
    }

    #[test]
    fn test_anthropic_delta_parses_text_delta_unchanged() {
        // Existing text_delta should still work
        let json = r#"{"type": "text_delta", "text": "Hello world"}"#;
        let delta: AnthropicDelta = serde_json::from_str(json).unwrap();

        assert_eq!(delta.delta_type, "text_delta");
        assert_eq!(delta.text.as_deref(), Some("Hello world"));
        assert!(delta.thinking.is_none());
        assert!(delta.signature.is_none());
    }

    #[test]
    fn test_anthropic_delta_parses_input_json_delta_unchanged() {
        // Existing input_json_delta should still work
        let json = r#"{"type": "input_json_delta", "partial_json": "{\"path\":"}"#;
        let delta: AnthropicDelta = serde_json::from_str(json).unwrap();

        assert_eq!(delta.delta_type, "input_json_delta");
        assert_eq!(delta.partial_json.as_deref(), Some("{\"path\":"));
    }

    // =========================================================================
    // Request building tests for BlockAssistantMessage (spec section 3.5)
    // =========================================================================

    #[test]
    fn test_build_request_body_renders_thinking_block_with_signature()
    -> Result<(), Box<dyn std::error::Error>> {
        let client = AnthropicClient::new("test-key".to_string())?;

        // Create a session with a thinking block
        let assistant_msg = Message::BlockAssistant(BlockAssistantMessage {
            blocks: vec![
                AssistantBlock::Reasoning {
                    text: "Let me analyze this problem...".to_string(),
                    meta: Some(Box::new(ProviderMeta::Anthropic {
                        signature: "sig_abc123".to_string(),
                    })),
                },
                AssistantBlock::Text {
                    text: "The answer is 42.".to_string(),
                    meta: None,
                },
            ],
            stop_reason: StopReason::EndTurn,
            created_at: meerkat_core::types::message_timestamp_now(),
        });

        let request = LlmRequest::new(
            "claude-sonnet-4-5",
            vec![
                Message::User(UserMessage::text(
                    "What is the meaning of life?".to_string(),
                )),
                assistant_msg,
                Message::User(UserMessage::text("Can you elaborate?".to_string())),
            ],
        );

        let body = client.build_request_body(&request)?;
        let messages = body["messages"].as_array().unwrap();

        // Second message should be the assistant with thinking + text
        let assistant_content = messages[1]["content"].as_array().unwrap();
        assert_eq!(assistant_content.len(), 2);

        // First block should be thinking
        assert_eq!(assistant_content[0]["type"], "thinking");
        assert_eq!(
            assistant_content[0]["thinking"],
            "Let me analyze this problem..."
        );
        assert_eq!(assistant_content[0]["signature"], "sig_abc123");

        // Second block should be text
        assert_eq!(assistant_content[1]["type"], "text");
        assert_eq!(assistant_content[1]["text"], "The answer is 42.");

        Ok(())
    }

    #[test]
    fn test_build_request_body_degrades_video_user_content_to_text()
    -> Result<(), Box<dyn std::error::Error>> {
        let client = AnthropicClient::new("test-key".to_string())?;

        let request = LlmRequest::new(
            "claude-sonnet-4-5",
            vec![Message::User(UserMessage::with_blocks(vec![
                ContentBlock::Video {
                    media_type: "video/mp4".to_string(),
                    duration_ms: 12_000,
                    data: meerkat_core::VideoData::Inline {
                        data: "AAAA".to_string(),
                    },
                },
            ]))],
        );

        let body = client.build_request_body(&request)?;
        let messages = body["messages"].as_array().ok_or("missing messages")?;
        let content = messages[0]["content"]
            .as_array()
            .ok_or("missing content array")?;
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[0]["text"], "[video: video/mp4]");
        Ok(())
    }

    #[test]
    fn test_build_request_body_rejects_video_tool_results() {
        let client = AnthropicClient::new("test-key".to_string()).expect("client");
        let request = LlmRequest::new(
            "claude-sonnet-4-5",
            vec![Message::ToolResults {
                results: vec![meerkat_core::ToolResult::with_blocks(
                    "tool_1".to_string(),
                    vec![ContentBlock::Video {
                        media_type: "video/mp4".to_string(),
                        duration_ms: 12_000,
                        data: meerkat_core::VideoData::Inline {
                            data: "AAAA".to_string(),
                        },
                    }],
                    false,
                )],
                created_at: meerkat_core::types::message_timestamp_now(),
            }],
        );

        let err = client
            .build_request_body(&request)
            .expect_err("video tool results should be rejected");
        match err {
            LlmError::InvalidRequest { message } => {
                assert!(message.contains("video blocks are not supported"));
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn test_build_request_body_skips_thinking_block_without_signature()
    -> Result<(), Box<dyn std::error::Error>> {
        let client = AnthropicClient::new("test-key".to_string())?;

        // Thinking block without Anthropic signature should be skipped
        let assistant_msg = Message::BlockAssistant(BlockAssistantMessage {
            blocks: vec![
                AssistantBlock::Reasoning {
                    text: "Some thinking...".to_string(),
                    meta: None, // No signature!
                },
                AssistantBlock::Text {
                    text: "The answer.".to_string(),
                    meta: None,
                },
            ],
            stop_reason: StopReason::EndTurn,
            created_at: meerkat_core::types::message_timestamp_now(),
        });

        let request = LlmRequest::new(
            "claude-sonnet-4-5",
            vec![
                Message::User(UserMessage::text("Question".to_string())),
                assistant_msg,
            ],
        );

        let body = client.build_request_body(&request)?;
        let messages = body["messages"].as_array().unwrap();

        // Assistant content should only have the text block (thinking skipped)
        let assistant_content = messages[1]["content"].as_array().unwrap();
        assert_eq!(assistant_content.len(), 1);
        assert_eq!(assistant_content[0]["type"], "text");

        Ok(())
    }

    #[test]
    fn test_build_request_body_renders_tool_use_blocks() -> Result<(), Box<dyn std::error::Error>> {
        let client = AnthropicClient::new("test-key".to_string())?;

        let assistant_msg = Message::BlockAssistant(BlockAssistantMessage {
            blocks: vec![
                AssistantBlock::Text {
                    text: "I'll read that file for you.".to_string(),
                    meta: None,
                },
                AssistantBlock::ToolUse {
                    id: "tu_123".to_string(),
                    name: "read_file".to_string(),
                    args: serde_json::value::RawValue::from_string(
                        r#"{"path": "/tmp/test.txt"}"#.to_string(),
                    )
                    .unwrap(),
                    meta: None, // Tool use blocks don't have signatures in Anthropic
                },
            ],
            stop_reason: StopReason::ToolUse,
            created_at: meerkat_core::types::message_timestamp_now(),
        });

        let request = LlmRequest::new(
            "claude-sonnet-4-5",
            vec![
                Message::User(UserMessage::text("Read /tmp/test.txt".to_string())),
                assistant_msg,
            ],
        );

        let body = client.build_request_body(&request)?;
        let messages = body["messages"].as_array().unwrap();
        let assistant_content = messages[1]["content"].as_array().unwrap();

        assert_eq!(assistant_content.len(), 2);
        assert_eq!(assistant_content[0]["type"], "text");
        assert_eq!(assistant_content[1]["type"], "tool_use");
        assert_eq!(assistant_content[1]["id"], "tu_123");
        assert_eq!(assistant_content[1]["name"], "read_file");
        assert_eq!(assistant_content[1]["input"]["path"], "/tmp/test.txt");

        Ok(())
    }

    #[test]
    fn test_build_request_body_adds_thinking_beta_header() -> Result<(), Box<dyn std::error::Error>>
    {
        let client = AnthropicClient::new("test-key".to_string())?;

        // When thinking is enabled via provider_params, beta header should be added
        let request = LlmRequest::new(
            "claude-sonnet-4-5",
            vec![Message::User(UserMessage::text("test".to_string()))],
        )
        .with_anthropic_tag_merge(|t| t.thinking_budget_tokens = Some(10000));

        let body = client.build_request_body(&request)?;

        // The beta header is added during the HTTP request, not in the body
        // But the thinking config should be in the body
        assert!(body.get("thinking").is_some());
        assert_eq!(body["thinking"]["type"], "enabled");
        assert_eq!(body["thinking"]["budget_tokens"], 10000);

        Ok(())
    }

    // =========================================================================
    // Original tests (unchanged)
    // =========================================================================

    #[test]
    fn test_build_request_body_basic() -> Result<(), Box<dyn std::error::Error>> {
        let client = AnthropicClient::new("test-key".to_string())?;

        let request = LlmRequest::new(
            "claude-sonnet-4-20250514",
            vec![Message::User(UserMessage::text("test".to_string()))],
        )
        .with_anthropic_tag_merge(|t| t.thinking_budget_tokens = Some(10000));

        let body = client.build_request_body(&request)?;

        assert!(
            body.get("thinking").is_some(),
            "thinking field should be present"
        );
        let thinking = &body["thinking"];
        assert_eq!(thinking["type"], "enabled");
        assert_eq!(thinking["budget_tokens"], 10000);
        Ok(())
    }

    #[test]
    fn test_request_omits_temperature_for_opus_47() -> Result<(), Box<dyn std::error::Error>> {
        let client = AnthropicClient::new("test-key".to_string())?;
        let request = LlmRequest::new(
            "claude-opus-4-7",
            vec![Message::User(UserMessage::text("test".to_string()))],
        )
        .with_temperature(0.3);

        let body = client.build_request_body(&request)?;
        assert!(
            body.get("temperature").is_none(),
            "claude-opus-4-7 requests must not include temperature (API rejects non-default)",
        );
        Ok(())
    }

    #[test]
    fn test_request_keeps_temperature_for_opus_46() -> Result<(), Box<dyn std::error::Error>> {
        let client = AnthropicClient::new("test-key".to_string())?;
        let request = LlmRequest::new(
            "claude-opus-4-6",
            vec![Message::User(UserMessage::text("test".to_string()))],
        )
        .with_temperature(0.3);

        let body = client.build_request_body(&request)?;
        let temp = body
            .get("temperature")
            .and_then(Value::as_f64)
            .expect("temperature should be present for opus 4.6");
        assert!((temp - 0.3).abs() < 1e-6);
        Ok(())
    }

    #[test]
    fn test_request_internal_flag_forces_temperature_on_opus_47()
    -> Result<(), Box<dyn std::error::Error>> {
        let client = AnthropicClient::new("test-key".to_string())?;
        let request = LlmRequest::new(
            "claude-opus-4-7",
            vec![Message::User(UserMessage::text("test".to_string()))],
        )
        .with_temperature(0.3)
        .with_anthropic_tag_merge(|t| t.supports_temperature_override = Some(true));

        let body = client.build_request_body(&request)?;
        assert!(
            body.get("temperature").is_some(),
            "internal override must force-include temperature",
        );
        Ok(())
    }

    #[test]
    fn test_build_request_body_with_top_k() -> Result<(), Box<dyn std::error::Error>> {
        let client = AnthropicClient::new("test-key".to_string())?;

        let request = LlmRequest::new(
            "claude-sonnet-4-20250514",
            vec![Message::User(UserMessage::text("test".to_string()))],
        )
        .with_anthropic_tag_merge(|t| t.top_k = Some(40));

        let body = client.build_request_body(&request)?;

        assert_eq!(body["top_k"], 40);
        Ok(())
    }

    #[test]
    fn test_build_request_body_no_provider_params() -> Result<(), Box<dyn std::error::Error>> {
        let client = AnthropicClient::new("test-key".to_string())?;
        let request = LlmRequest::new(
            "claude-sonnet-4-20250514",
            vec![Message::User(UserMessage::text("test".to_string()))],
        );

        let body = client.build_request_body(&request)?;

        assert!(body.get("thinking").is_none());
        assert!(body.get("top_k").is_none());
        assert_eq!(body["model"], "claude-sonnet-4-20250514");
        Ok(())
    }

    #[test]
    fn test_client_builder_creates_configured_client() -> Result<(), Box<dyn std::error::Error>> {
        let client = AnthropicClient::builder("test-key".to_string())
            .connect_timeout(std::time::Duration::from_secs(5))
            .request_timeout(std::time::Duration::from_secs(120))
            .build()?;

        assert_eq!(client.provider(), "anthropic");
        Ok(())
    }

    #[test]
    fn test_client_default_has_connection_pool() -> Result<(), Box<dyn std::error::Error>> {
        let client = AnthropicClient::new("test-key".to_string())?;
        assert_eq!(client.provider(), "anthropic");
        Ok(())
    }

    #[test]
    fn test_parse_sse_line() {
        let line = r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}"#;
        let event = AnthropicClient::parse_sse_line(line);
        assert!(event.is_some());
        assert_eq!(event.unwrap().event_type, "content_block_delta");

        let done = AnthropicClient::parse_sse_line("data: [DONE]");
        assert!(done.is_none());

        let comment = AnthropicClient::parse_sse_line(": comment");
        assert!(comment.is_none());

        let event_line = AnthropicClient::parse_sse_line("event: message_start");
        assert!(event_line.is_none());
    }

    #[test]
    fn test_sse_buffer_constants() {
        let buffer_cap = SSE_BUFFER_CAPACITY;
        assert!(buffer_cap >= 256, "SSE buffer should be at least 256 bytes");
        let connect_timeout = DEFAULT_CONNECT_TIMEOUT.as_secs();
        assert!(
            connect_timeout >= 5,
            "Connect timeout should be at least 5s"
        );
        let request_timeout = DEFAULT_REQUEST_TIMEOUT.as_secs();
        assert!(
            request_timeout >= 60,
            "Request timeout should be at least 60s"
        );
    }

    #[test]
    fn test_usage_merge_preserves_input_tokens() {
        let mut usage = Usage::default();
        let start = AnthropicUsage {
            input_tokens: Some(120),
            output_tokens: Some(0),
            cache_creation_input_tokens: Some(4),
            cache_read_input_tokens: Some(2),
        };

        merge_usage(&mut usage, &start);

        assert_eq!(usage.input_tokens, 120);
        assert_eq!(usage.output_tokens, 0);
        assert_eq!(usage.cache_creation_tokens, Some(4));
        assert_eq!(usage.cache_read_tokens, Some(2));

        let delta = AnthropicUsage {
            input_tokens: None,
            output_tokens: Some(7),
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
        };

        merge_usage(&mut usage, &delta);

        assert_eq!(usage.input_tokens, 120);
        assert_eq!(usage.output_tokens, 7);
        assert_eq!(usage.cache_creation_tokens, Some(4));
        assert_eq!(usage.cache_read_tokens, Some(2));
    }

    #[test]
    fn test_build_request_body_with_structured_output() -> Result<(), Box<dyn std::error::Error>> {
        let client = AnthropicClient::new("test-key".to_string())?;

        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "name": {"type": "string"},
                "age": {"type": "integer"}
            },
            "required": ["name", "age"]
        });

        let output_schema: OutputSchema = serde_json::from_value(serde_json::json!({
            "schema": schema,
            "name": "person",
            "strict": true
        }))?;
        let request = LlmRequest::new(
            "claude-sonnet-4-20250514",
            vec![Message::User(UserMessage::text("test".to_string()))],
        )
        .with_anthropic_tag_merge(|t| t.structured_output = Some(output_schema));

        let body = client.build_request_body(&request)?;

        // Check output_config is present with correct structure
        assert!(
            body.get("output_config").is_some(),
            "output_config should be present"
        );
        let output_config = &body["output_config"];
        assert_eq!(output_config["format"]["type"], "json_schema");
        assert!(output_config["format"]["schema"].is_object());
        assert_eq!(
            output_config["format"]["schema"]["additionalProperties"],
            serde_json::json!(false)
        );
        Ok(())
    }

    #[test]
    fn test_build_request_body_without_structured_output() -> Result<(), Box<dyn std::error::Error>>
    {
        let client = AnthropicClient::new("test-key".to_string())?;

        let request = LlmRequest::new(
            "claude-sonnet-4-20250514",
            vec![Message::User(UserMessage::text("test".to_string()))],
        );

        let body = client.build_request_body(&request)?;

        // output_config should not be present
        assert!(
            body.get("output_config").is_none(),
            "output_config should not be present without structured_output"
        );
        Ok(())
    }

    #[test]
    fn test_compile_schema_recursively_injects_additional_properties()
    -> Result<(), Box<dyn std::error::Error>> {
        let client = AnthropicClient::new("test-key".to_string())?;
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "profile": {
                    "type": "object",
                    "properties": {
                        "name": {"type": "string"}
                    }
                },
                "rows": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "col": {"type": "integer"}
                        }
                    }
                },
                "union": {
                    "anyOf": [
                        {
                            "type": ["object", "null"],
                            "properties": {
                                "ok": {"type": "boolean"}
                            }
                        },
                        {"type": "string"}
                    ]
                }
            }
        });

        let output_schema = OutputSchema::new(schema)?.strict();
        let compiled = client.compile_schema(&output_schema)?;

        assert!(compiled.warnings.is_empty());
        assert_eq!(compiled.schema["additionalProperties"], false);
        assert_eq!(
            compiled.schema["properties"]["profile"]["additionalProperties"],
            false
        );
        assert_eq!(
            compiled.schema["properties"]["rows"]["items"]["additionalProperties"],
            false
        );
        assert_eq!(
            compiled.schema["properties"]["union"]["anyOf"][0]["additionalProperties"],
            false
        );
        Ok(())
    }

    #[test]
    fn test_compile_schema_preserves_explicit_additional_properties()
    -> Result<(), Box<dyn std::error::Error>> {
        let client = AnthropicClient::new("test-key".to_string())?;
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
                        "y": {"type": "number"}
                    }
                }
            }
        });

        let output_schema = OutputSchema::new(schema)?;
        let compiled = client.compile_schema(&output_schema)?;

        assert!(compiled.warnings.is_empty());
        assert_eq!(compiled.schema["additionalProperties"], true);
        assert_eq!(
            compiled.schema["properties"]["nested"]["additionalProperties"],
            serde_json::json!({"type": "string"})
        );
        assert_eq!(
            compiled.schema["properties"]["auto"]["additionalProperties"],
            false
        );
        Ok(())
    }

    // =========================================================================
    // Opus 4.6: Adaptive thinking & effort parameter tests
    // =========================================================================

    #[test]
    fn test_build_request_body_adaptive_thinking() -> Result<(), Box<dyn std::error::Error>> {
        let client = AnthropicClient::new("test-key".to_string())?;

        let request = LlmRequest::new(
            "claude-opus-4-6",
            vec![Message::User(UserMessage::text("test".to_string()))],
        )
        .with_anthropic_tag_merge(|t| {
            t.thinking =
                Some(meerkat_core::lifecycle::run_primitive::AnthropicThinkingConfig::Adaptive);
        });

        let body = client.build_request_body(&request)?;

        assert!(
            body.get("thinking").is_some(),
            "thinking field should be present"
        );
        assert_eq!(body["thinking"]["type"], "adaptive");
        // adaptive mode should NOT have budget_tokens
        assert!(body["thinking"].get("budget_tokens").is_none());
        Ok(())
    }

    #[test]
    fn test_build_request_body_effort_parameter() -> Result<(), Box<dyn std::error::Error>> {
        let client = AnthropicClient::new("test-key".to_string())?;

        let request = LlmRequest::new(
            "claude-opus-4-6",
            vec![Message::User(UserMessage::text("test".to_string()))],
        )
        .with_anthropic_tag_merge(|t| {
            t.effort = Some(meerkat_core::lifecycle::run_primitive::AnthropicEffort::Medium);
        });

        let body = client.build_request_body(&request)?;

        assert!(
            body.get("output_config").is_some(),
            "output_config should be present"
        );
        assert_eq!(body["output_config"]["effort"], "medium");
        Ok(())
    }

    #[test]
    fn test_build_request_body_effort_xhigh_opus_47() -> Result<(), Box<dyn std::error::Error>> {
        let client = AnthropicClient::new("test-key".to_string())?;

        let request = LlmRequest::new(
            "claude-opus-4-7",
            vec![Message::User(UserMessage::text("test".to_string()))],
        )
        .with_anthropic_tag_merge(|t| t.effort = Some(AnthropicEffort::XHigh));

        let body = client.build_request_body(&request)?;

        assert_eq!(body["output_config"]["effort"], "xhigh");
        Ok(())
    }

    #[test]
    fn test_build_request_body_effort_with_structured_output()
    -> Result<(), Box<dyn std::error::Error>> {
        let client = AnthropicClient::new("test-key".to_string())?;

        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "name": {"type": "string"}
            },
            "required": ["name"]
        });

        let output_schema: OutputSchema = serde_json::from_value(serde_json::json!({
            "schema": schema,
            "name": "output",
            "strict": true
        }))?;
        let request = LlmRequest::new(
            "claude-opus-4-6",
            vec![Message::User(UserMessage::text("test".to_string()))],
        )
        .with_anthropic_tag_merge(|t| {
            t.effort = Some(AnthropicEffort::High);
            t.structured_output = Some(output_schema);
        });

        let body = client.build_request_body(&request)?;

        // output_config should have BOTH effort and format
        let output_config = &body["output_config"];
        assert_eq!(output_config["effort"], "high");
        assert_eq!(output_config["format"]["type"], "json_schema");
        assert!(output_config["format"]["schema"].is_object());
        Ok(())
    }

    #[test]
    fn test_build_request_body_adaptive_thinking_does_not_set_interleaved_beta()
    -> Result<(), Box<dyn std::error::Error>> {
        let client = AnthropicClient::new("test-key".to_string())?;

        let request = LlmRequest::new(
            "claude-opus-4-6",
            vec![Message::User(UserMessage::text("test".to_string()))],
        )
        .with_anthropic_tag_merge(|t| {
            t.thinking =
                Some(meerkat_core::lifecycle::run_primitive::AnthropicThinkingConfig::Adaptive);
        });

        let body = client.build_request_body(&request)?;

        // Adaptive thinking should be in the body
        assert_eq!(body["thinking"]["type"], "adaptive");
        assert!(
            body["thinking"].get("budget_tokens").is_none(),
            "adaptive thinking should not have budget_tokens"
        );
        Ok(())
    }

    #[test]
    fn test_build_request_body_legacy_thinking_still_works()
    -> Result<(), Box<dyn std::error::Error>> {
        let client = AnthropicClient::new("test-key".to_string())?;

        // Legacy flat format should still produce type: enabled
        let request = LlmRequest::new(
            "claude-sonnet-4-5",
            vec![Message::User(UserMessage::text("test".to_string()))],
        )
        .with_anthropic_tag_merge(|t| t.thinking_budget_tokens = Some(10000));

        let body = client.build_request_body(&request)?;

        assert_eq!(body["thinking"]["type"], "enabled");
        assert_eq!(body["thinking"]["budget_tokens"], 10000);
        Ok(())
    }

    #[test]
    fn test_build_request_body_inference_geo() -> Result<(), Box<dyn std::error::Error>> {
        let client = AnthropicClient::new("test-key".to_string())?;

        let request = LlmRequest::new(
            "claude-opus-4-6",
            vec![Message::User(UserMessage::text("test".to_string()))],
        )
        .with_anthropic_tag_merge(|t| {
            t.inference_geo =
                Some(meerkat_core::lifecycle::run_primitive::AnthropicInferenceGeo::Us);
        });

        let body = client.build_request_body(&request)?;

        assert_eq!(body["inference_geo"], "us");
        Ok(())
    }

    #[test]
    fn test_build_request_body_compaction_auto() -> Result<(), Box<dyn std::error::Error>> {
        let client = AnthropicClient::new("test-key".to_string())?;

        let request = LlmRequest::new(
            "claude-opus-4-6",
            vec![Message::User(UserMessage::text("test".to_string()))],
        )
        .with_anthropic_tag_merge(|t| {
            t.compaction =
                Some(meerkat_core::lifecycle::run_primitive::AnthropicCompactionConfig::Auto);
        });

        let body = client.build_request_body(&request)?;

        let edits = body["context_management"]["edits"].as_array().unwrap();
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0]["type"], "compact_20260112");
        Ok(())
    }

    #[test]
    fn test_build_request_body_compaction_with_trigger() -> Result<(), Box<dyn std::error::Error>> {
        let client = AnthropicClient::new("test-key".to_string())?;

        let custom_edit_body = serde_json::json!({
            "trigger": {"type": "input_tokens", "value": 100_000}
        });
        let request = LlmRequest::new(
            "claude-opus-4-6",
            vec![Message::User(UserMessage::text("test".to_string()))],
        )
        .with_anthropic_tag_merge(|t| {
            t.compaction = Some(AnthropicCompactionConfig::Custom {
                edit: meerkat_core::lifecycle::run_primitive::OpaqueProviderBody::from_value(
                    &custom_edit_body,
                ),
            });
        });

        let body = client.build_request_body(&request)?;

        let edits = body["context_management"]["edits"].as_array().unwrap();
        assert_eq!(edits[0]["type"], "compact_20260112");
        assert_eq!(edits[0]["trigger"]["value"], 100_000);
        Ok(())
    }

    #[test]
    fn test_build_request_body_redacted_thinking_roundtrip()
    -> Result<(), Box<dyn std::error::Error>> {
        let client = AnthropicClient::new("test-key".to_string())?;

        // Redacted thinking is stored as Reasoning with AnthropicRedacted meta
        let assistant_msg = Message::BlockAssistant(BlockAssistantMessage {
            blocks: vec![
                AssistantBlock::Reasoning {
                    text: String::new(),
                    meta: Some(Box::new(ProviderMeta::AnthropicRedacted {
                        data: "encrypted_blob_abc123".to_string(),
                    })),
                },
                AssistantBlock::Text {
                    text: "The answer.".to_string(),
                    meta: None,
                },
            ],
            stop_reason: StopReason::EndTurn,
            created_at: meerkat_core::types::message_timestamp_now(),
        });

        let request = LlmRequest::new(
            "claude-opus-4-6",
            vec![
                Message::User(UserMessage::text("Question".to_string())),
                assistant_msg,
            ],
        );

        let body = client.build_request_body(&request)?;
        let messages = body["messages"].as_array().unwrap();
        let assistant_content = messages[1]["content"].as_array().unwrap();

        // First block should be redacted_thinking with encrypted data
        assert_eq!(assistant_content[0]["type"], "redacted_thinking");
        assert_eq!(assistant_content[0]["data"], "encrypted_blob_abc123");

        // Second block should be text
        assert_eq!(assistant_content[1]["type"], "text");
        Ok(())
    }

    #[test]
    fn test_build_request_body_compaction_block_roundtrip() -> Result<(), Box<dyn std::error::Error>>
    {
        let client = AnthropicClient::new("test-key".to_string())?;

        // Compaction is stored as Reasoning with AnthropicCompaction meta
        let assistant_msg = Message::BlockAssistant(BlockAssistantMessage {
            blocks: vec![
                AssistantBlock::Reasoning {
                    text: String::new(),
                    meta: Some(Box::new(ProviderMeta::AnthropicCompaction {
                        content: "Summary of prior conversation...".to_string(),
                    })),
                },
                AssistantBlock::Text {
                    text: "Continuing from summary.".to_string(),
                    meta: None,
                },
            ],
            stop_reason: StopReason::EndTurn,
            created_at: meerkat_core::types::message_timestamp_now(),
        });

        let request = LlmRequest::new(
            "claude-opus-4-6",
            vec![
                Message::User(UserMessage::text("Continue".to_string())),
                assistant_msg,
            ],
        );

        let body = client.build_request_body(&request)?;
        let messages = body["messages"].as_array().unwrap();
        let assistant_content = messages[1]["content"].as_array().unwrap();

        // First block should be compaction with summary
        assert_eq!(assistant_content[0]["type"], "compaction");
        assert_eq!(
            assistant_content[0]["content"],
            "Summary of prior conversation..."
        );

        // Second block should be text
        assert_eq!(assistant_content[1]["type"], "text");
        Ok(())
    }

    // =========================================================================
    // SSE stream regression tests
    // =========================================================================

    use axum::{Router, extract::State, response::IntoResponse, routing::post};
    use tokio::net::TcpListener;

    async fn messages_sse(State(payload): State<String>) -> impl IntoResponse {
        ([("content-type", "text/event-stream")], payload)
    }

    async fn spawn_anthropic_stub_server(payload: String) -> (String, tokio::task::JoinHandle<()>) {
        let app = Router::new()
            .route("/v1/messages", post(messages_sse))
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

    /// Regression: message_delta with stop_reason (no "type" in delta) must yield Done.
    ///
    /// Previously, AnthropicDelta required delta_type (mapped from "type") as a non-optional
    /// String. Anthropic's message_delta sends {"delta": {"stop_reason": "end_turn"}} with
    /// no "type" field, causing the entire event to fail serde parsing silently. When the
    /// stream ended before message_stop arrived, no Done was ever emitted.
    #[tokio::test]
    async fn test_regression_message_delta_stop_reason_without_type_yields_done() {
        // Simulate a stream where message_stop is NOT sent (e.g., connection dropped
        // after message_delta). Done must come from message_delta's stop_reason.
        let payload = [
            r#"data: {"type":"message_start","message":{"usage":{"input_tokens":10,"output_tokens":0}}}"#,
            r#"data: {"type":"content_block_start","content_block":{"type":"text","text":""}}"#,
            r#"data: {"type":"content_block_delta","delta":{"type":"text_delta","text":"Hello"}}"#,
            r#"data: {"type":"content_block_stop"}"#,
            // message_delta has "delta": {"stop_reason": "end_turn"} with NO "type" field
            r#"data: {"type":"message_delta","usage":{"output_tokens":5},"delta":{"stop_reason":"end_turn"}}"#,
            // Simulate stream ending WITHOUT message_stop (connection dropped)
            "",
        ]
        .join("\n");
        let (base_url, server) = spawn_anthropic_stub_server(payload).await;
        let client = AnthropicClient::builder("test-key".to_string())
            .base_url(base_url)
            .build()
            .unwrap();
        let request = LlmRequest::new(
            "claude-sonnet-4-5",
            vec![Message::User(UserMessage::text("hello".to_string()))],
        );

        let mut stream = client.stream(&request);
        let mut saw_text = false;
        let mut saw_done = false;
        let mut done_is_success = false;
        while let Some(event) = stream.next().await {
            match event.expect("stream event") {
                LlmEvent::TextDelta { delta, .. } if delta == "Hello" => saw_text = true,
                LlmEvent::Done { outcome } => {
                    saw_done = true;
                    done_is_success = matches!(outcome, LlmDoneOutcome::Success { .. });
                    break;
                }
                _ => {}
            }
        }
        server.abort();

        assert!(saw_text, "Expected text delta");
        assert!(
            saw_done,
            "Expected Done event from message_delta stop_reason"
        );
        assert!(done_is_success, "Expected successful Done outcome");
    }

    #[tokio::test]
    async fn test_web_search_server_tool_blocks_are_emitted() {
        let payload = [
            r#"data: {"type":"message_start","message":{"usage":{"input_tokens":10,"output_tokens":0}}}"#,
            r#"data: {"type":"content_block_start","content_block":{"type":"server_tool_use","id":"srvtoolu_1","name":"web_search"}}"#,
            r#"data: {"type":"content_block_delta","delta":{"type":"input_json_delta","partial_json":"{\"query\":\"latest meerkat runtime\"}"}}"#,
            r#"data: {"type":"content_block_stop"}"#,
            r#"data: {"type":"content_block_start","content_block":{"type":"web_search_tool_result","tool_use_id":"srvtoolu_1","content":[{"type":"web_search_result","title":"Result","url":"https://example.com"}]}}"#,
            r#"data: {"type":"content_block_stop"}"#,
            r#"data: {"type":"message_delta","usage":{"output_tokens":5},"delta":{"stop_reason":"end_turn"}}"#,
            "",
        ]
        .join("\n");
        let (base_url, server) = spawn_anthropic_stub_server(payload).await;
        let client = AnthropicClient::builder("test-key".to_string())
            .base_url(base_url)
            .build()
            .unwrap();
        let request = LlmRequest::new(
            "claude-sonnet-4-5",
            vec![Message::User(UserMessage::text("search".to_string()))],
        );

        let mut stream = client.stream(&request);
        let mut server_blocks = Vec::new();
        while let Some(event) = stream.next().await {
            match event.expect("stream event") {
                LlmEvent::ServerToolContent {
                    id, name, content, ..
                } => {
                    server_blocks.push((id, name, content));
                }
                LlmEvent::Done { .. } => break,
                _ => {}
            }
        }
        server.abort();

        assert_eq!(server_blocks.len(), 2);
        assert_eq!(server_blocks[0].0.as_deref(), Some("srvtoolu_1"));
        assert_eq!(server_blocks[0].1, "web_search");
        assert_eq!(
            server_blocks[0].2["input"]["query"],
            "latest meerkat runtime"
        );
        assert_eq!(server_blocks[1].0.as_deref(), Some("srvtoolu_1"));
        assert_eq!(
            server_blocks[1].2["content"][0]["url"],
            "https://example.com"
        );
    }

    /// Regression: Anthropic streaming error event must yield Done with error.
    #[tokio::test]
    async fn test_regression_anthropic_error_event_yields_done_with_error() {
        let payload = [
            r#"data: {"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#,
            "",
        ]
        .join("\n");
        let (base_url, server) = spawn_anthropic_stub_server(payload).await;
        let client = AnthropicClient::builder("test-key".to_string())
            .base_url(base_url)
            .build()
            .unwrap();
        let request = LlmRequest::new(
            "claude-sonnet-4-5",
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
                    matches!(error, LlmError::ServerOverloaded),
                    "expected ServerOverloaded, got: {error:?}"
                );
                saw_error_done = true;
                break;
            }
        }
        server.abort();

        assert!(saw_error_done, "Expected Done with error outcome");
    }

    #[tokio::test]
    async fn test_stream_valid_tool_use_args_project_successfully() {
        let payload = [
            r#"data: {"type":"message_start","message":{"usage":{"input_tokens":10,"output_tokens":0}}}"#,
            r#"data: {"type":"content_block_start","content_block":{"type":"tool_use","id":"tool_1","name":"read_file"}}"#,
            r#"data: {"type":"content_block_delta","delta":{"type":"input_json_delta","partial_json":"{\"path\":\"/tmp/test.txt\"}"}}"#,
            r#"data: {"type":"content_block_stop"}"#,
            r#"data: {"type":"message_stop"}"#,
            "",
        ]
        .join("\n");
        let (base_url, server) = spawn_anthropic_stub_server(payload).await;
        let client = AnthropicClient::builder("test-key".to_string())
            .base_url(base_url)
            .build()
            .unwrap();
        let request = LlmRequest::new(
            "claude-sonnet-4-5",
            vec![Message::User(UserMessage::text("read".to_string()))],
        );

        let mut stream = client.stream(&request);
        let mut tool_args = None;
        let mut saw_success_done = false;
        while let Some(event) = stream.next().await {
            match event.expect("stream event") {
                LlmEvent::ToolCallComplete { args, .. } => tool_args = Some(args),
                LlmEvent::Done {
                    outcome: LlmDoneOutcome::Success { .. },
                } => {
                    saw_success_done = true;
                    break;
                }
                LlmEvent::Done {
                    outcome: LlmDoneOutcome::Error { error },
                } => panic!("valid tool args should not fail: {error:?}"),
                _ => {}
            }
        }
        server.abort();

        assert_eq!(
            tool_args.expect("expected tool call complete"),
            serde_json::json!({"path": "/tmp/test.txt"})
        );
        assert!(saw_success_done, "Expected successful Done outcome");
    }

    #[tokio::test]
    async fn test_stream_malformed_tool_use_args_fail_closed() {
        let payload = [
            r#"data: {"type":"message_start","message":{"usage":{"input_tokens":10,"output_tokens":0}}}"#,
            r#"data: {"type":"content_block_start","content_block":{"type":"tool_use","id":"tool_bad","name":"read_file"}}"#,
            r#"data: {"type":"content_block_delta","delta":{"type":"input_json_delta","partial_json":"{\"path\":"}}"#,
            r#"data: {"type":"content_block_stop"}"#,
            r#"data: {"type":"message_stop"}"#,
            "",
        ]
        .join("\n");
        let (base_url, server) = spawn_anthropic_stub_server(payload).await;
        let client = AnthropicClient::builder("test-key".to_string())
            .base_url(base_url)
            .build()
            .unwrap();
        let request = LlmRequest::new(
            "claude-sonnet-4-5",
            vec![Message::User(UserMessage::text("read".to_string()))],
        );

        let mut stream = client.stream(&request);
        let mut tool_completes = 0;
        let mut error_done = None;
        while let Some(event) = stream.next().await {
            match event.expect("stream wrapper should convert errors to Done") {
                LlmEvent::ToolCallComplete { .. } => tool_completes += 1,
                LlmEvent::Done {
                    outcome: LlmDoneOutcome::Error { error },
                } => {
                    error_done = Some(error);
                    break;
                }
                LlmEvent::Done {
                    outcome: LlmDoneOutcome::Success { .. },
                } => panic!("malformed tool args must not complete successfully"),
                _ => {}
            }
        }
        server.abort();

        assert_eq!(tool_completes, 0);
        let error = error_done.expect("expected terminal error for malformed args");
        assert!(
            matches!(error, LlmError::StreamParseError { .. }),
            "expected StreamParseError, got: {error:?}"
        );
        assert!(
            error
                .to_string()
                .contains("invalid Anthropic tool call arguments JSON"),
            "error should name invalid Anthropic tool args: {error}"
        );
    }

    /// Normal stream with message_stop should still work (baseline).
    #[tokio::test]
    async fn test_normal_stream_with_message_stop_yields_done() {
        let payload = [
            r#"data: {"type":"message_start","message":{"usage":{"input_tokens":10,"output_tokens":0}}}"#,
            r#"data: {"type":"content_block_start","content_block":{"type":"text","text":""}}"#,
            r#"data: {"type":"content_block_delta","delta":{"type":"text_delta","text":"Hi"}}"#,
            r#"data: {"type":"content_block_stop"}"#,
            r#"data: {"type":"message_delta","usage":{"output_tokens":3},"delta":{"stop_reason":"end_turn"}}"#,
            r#"data: {"type":"message_stop"}"#,
            "",
        ]
        .join("\n");
        let (base_url, server) = spawn_anthropic_stub_server(payload).await;
        let client = AnthropicClient::builder("test-key".to_string())
            .base_url(base_url)
            .build()
            .unwrap();
        let request = LlmRequest::new(
            "claude-sonnet-4-5",
            vec![Message::User(UserMessage::text("hello".to_string()))],
        );

        let mut stream = client.stream(&request);
        let mut done_count = 0;
        while let Some(event) = stream.next().await {
            if let LlmEvent::Done { .. } = event.expect("stream event") {
                done_count += 1;
                break;
            }
        }
        server.abort();

        assert_eq!(done_count, 1, "Expected exactly one Done event");
    }

    // =========================================================================
    // Multimodal content (ContentBlock::Image) serialization tests
    // =========================================================================

    #[test]
    fn anthropic_text_only_tool_result_as_string() -> Result<(), Box<dyn std::error::Error>> {
        let client = AnthropicClient::new("test-key".to_string())?;
        let request = LlmRequest::new(
            "claude-sonnet-4-5",
            vec![
                Message::User(UserMessage::text("Read the file")),
                Message::ToolResults {
                    results: vec![meerkat_core::ToolResult::new(
                        "tu_1".to_string(),
                        "file contents here".to_string(),
                        false,
                    )],
                    created_at: meerkat_core::types::message_timestamp_now(),
                },
            ],
        );

        let body = client.build_request_body(&request)?;
        let messages = body["messages"].as_array().unwrap();
        let tool_msg = &messages[1]["content"].as_array().unwrap()[0];

        // Text-only tool result should serialize content as a string
        assert_eq!(tool_msg["type"], "tool_result");
        assert_eq!(tool_msg["tool_use_id"], "tu_1");
        assert_eq!(tool_msg["content"], "file contents here");
        assert!(
            tool_msg["content"].is_string(),
            "text-only tool result content should be a string"
        );
        Ok(())
    }

    #[test]
    fn anthropic_image_tool_result_as_array() -> Result<(), Box<dyn std::error::Error>> {
        let client = AnthropicClient::new("test-key".to_string())?;
        let request = LlmRequest::new(
            "claude-sonnet-4-5",
            vec![
                Message::User(UserMessage::text("Take a screenshot")),
                Message::ToolResults {
                    results: vec![meerkat_core::ToolResult::with_blocks(
                        "tu_2".to_string(),
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
                    created_at: meerkat_core::types::message_timestamp_now(),
                },
            ],
        );

        let body = client.build_request_body(&request)?;
        let messages = body["messages"].as_array().unwrap();
        let tool_msg = &messages[1]["content"].as_array().unwrap()[0];

        assert_eq!(tool_msg["type"], "tool_result");
        assert_eq!(tool_msg["tool_use_id"], "tu_2");

        // Content should be an array with text + image blocks
        let content = tool_msg["content"].as_array().unwrap();
        assert_eq!(content.len(), 2);

        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[0]["text"], "screenshot taken");

        assert_eq!(content[1]["type"], "image");
        assert_eq!(content[1]["source"]["type"], "base64");
        assert_eq!(content[1]["source"]["media_type"], "image/png");
        assert_eq!(content[1]["source"]["data"], "iVBOR...");

        // source_path must NOT leak into serialized output
        assert!(
            !serde_json::to_string(&body)
                .unwrap()
                .contains("source_path"),
            "source_path must never appear in provider payload"
        );
        assert!(
            !serde_json::to_string(&body)
                .unwrap()
                .contains("/tmp/screenshot.png"),
            "source_path value must never appear in provider payload"
        );
        Ok(())
    }

    #[test]
    fn anthropic_user_message_with_image() -> Result<(), Box<dyn std::error::Error>> {
        let client = AnthropicClient::new("test-key".to_string())?;
        let request = LlmRequest::new(
            "claude-sonnet-4-5",
            vec![Message::User(UserMessage::with_blocks(vec![
                ContentBlock::Text {
                    text: "describe this".to_string(),
                },
                ContentBlock::Image {
                    media_type: "image/jpeg".to_string(),
                    data: "/9j/4AAQ...".into(),
                },
            ]))],
        );

        let body = client.build_request_body(&request)?;
        let messages = body["messages"].as_array().unwrap();
        let user_msg = &messages[0];

        assert_eq!(user_msg["role"], "user");

        // Content should be an array (not a string) when images are present
        let content = user_msg["content"].as_array().unwrap();
        assert_eq!(content.len(), 2);

        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[0]["text"], "describe this");

        assert_eq!(content[1]["type"], "image");
        assert_eq!(content[1]["source"]["type"], "base64");
        assert_eq!(content[1]["source"]["media_type"], "image/jpeg");
        assert_eq!(content[1]["source"]["data"], "/9j/4AAQ...");

        // source_path must NOT leak
        assert!(
            !serde_json::to_string(&body)
                .unwrap()
                .contains("source_path"),
            "source_path must never appear in provider payload"
        );
        Ok(())
    }

    #[test]
    fn anthropic_text_only_user_message_stays_string() -> Result<(), Box<dyn std::error::Error>> {
        let client = AnthropicClient::new("test-key".to_string())?;
        let request = LlmRequest::new(
            "claude-sonnet-4-5",
            vec![Message::User(UserMessage::text("just text"))],
        );

        let body = client.build_request_body(&request)?;
        let messages = body["messages"].as_array().unwrap();

        // Text-only user message should remain a plain string
        assert!(
            messages[0]["content"].is_string(),
            "text-only user message content should be a string"
        );
        assert_eq!(messages[0]["content"], "just text");
        Ok(())
    }

    // =========================================================================
    // Web search tool injection tests
    // =========================================================================

    #[test]
    fn test_web_search_tool_appended() -> Result<(), Box<dyn std::error::Error>> {
        let client = AnthropicClient::new("test-key".to_string())?;
        let tool = std::sync::Arc::new(meerkat_core::ToolDef::new(
            "my_tool",
            "A test tool",
            serde_json::json!({"type": "object"}),
        ));
        let mut request = LlmRequest::new(
            "claude-sonnet-4-6",
            vec![Message::User(UserMessage::text("hi".to_string()))],
        );
        request.tools = vec![tool];
        request = request.with_anthropic_tag_merge(|t| {
            t.web_search = Some(
                meerkat_core::lifecycle::run_primitive::OpaqueProviderBody::from_value(
                    &serde_json::json!({"type": "web_search_20250305", "name": "web_search"}),
                ),
            );
        });
        let body = client.build_request_body(&request)?;
        let tools = body["tools"].as_array().expect("tools should be array");
        assert_eq!(tools.len(), 2, "should have regular tool + web_search");
        assert_eq!(tools[1]["type"], "web_search_20250305");
        Ok(())
    }

    #[test]
    fn test_web_search_tool_alone() -> Result<(), Box<dyn std::error::Error>> {
        let client = AnthropicClient::new("test-key".to_string())?;
        let mut request = LlmRequest::new(
            "claude-sonnet-4-6",
            vec![Message::User(UserMessage::text("hi".to_string()))],
        );
        request = request.with_anthropic_tag_merge(|t| {
            t.web_search = Some(
                meerkat_core::lifecycle::run_primitive::OpaqueProviderBody::from_value(
                    &serde_json::json!({"type": "web_search_20250305", "name": "web_search"}),
                ),
            );
        });
        let body = client.build_request_body(&request)?;
        let tools = body["tools"].as_array().expect("tools should be array");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["type"], "web_search_20250305");
        Ok(())
    }

    #[test]
    fn test_no_web_search_when_absent() -> Result<(), Box<dyn std::error::Error>> {
        let client = AnthropicClient::new("test-key".to_string())?;
        let tool = std::sync::Arc::new(meerkat_core::ToolDef::new(
            "my_tool",
            "A test tool",
            serde_json::json!({"type": "object"}),
        ));
        let mut request = LlmRequest::new(
            "claude-sonnet-4-6",
            vec![Message::User(UserMessage::text("hi".to_string()))],
        );
        request.tools = vec![tool];
        let body = client.build_request_body(&request)?;
        let tools = body["tools"].as_array().expect("tools should be array");
        assert_eq!(tools.len(), 1, "should have only the regular tool");
        assert_eq!(tools[0]["name"], "my_tool");
        Ok(())
    }

    #[test]
    fn test_web_search_not_leaked_to_body() -> Result<(), Box<dyn std::error::Error>> {
        let client = AnthropicClient::new("test-key".to_string())?;
        let mut request = LlmRequest::new(
            "claude-sonnet-4-6",
            vec![Message::User(UserMessage::text("hi".to_string()))],
        );
        request = request.with_anthropic_tag_merge(|t| {
            t.web_search = Some(
                meerkat_core::lifecycle::run_primitive::OpaqueProviderBody::from_value(
                    &serde_json::json!({"type": "web_search_20250305", "name": "web_search"}),
                ),
            );
        });
        let body = client.build_request_body(&request)?;
        assert!(
            body.get("web_search").is_none(),
            "web_search should not be a top-level body key"
        );
        Ok(())
    }
}
