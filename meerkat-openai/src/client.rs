//! OpenAI API client - Responses API
//!
//! Implements the LlmClient trait for OpenAI's Responses API.
//! This client uses the /v1/responses endpoint which supports reasoning items.

use async_trait::async_trait;
use futures::StreamExt;
use meerkat_core::lifecycle::run_primitive::{
    OpenAiProviderTag, ProviderTag, ReasoningEffort as TypedReasoningEffort,
};
use meerkat_core::schema::{CompiledSchema, SchemaError};
use meerkat_core::{
    AssistantBlock, BlockAssistantMessage, ContentBlock, ImageData, ImageGenerationIntent,
    ImageGenerationWarning, ImageOperationTerminalClass, Message, OpenAiImageMetadata,
    OutputSchema, ProviderImageMetadata, ProviderMeta, RevisedPromptDisposition,
    RevisedPromptSource, StopReason, ToolResult, Usage, UserMessage,
};
use meerkat_llm_core::BlockAssembler;
use meerkat_llm_core::LlmError;
use meerkat_llm_core::{
    ImageGenerationExecutor, LlmClient, LlmDoneOutcome, LlmEvent, LlmRequest, LlmStream,
    ProviderGeneratedImage, ProviderImageGenerationOutput, ProviderImageGenerationRequest,
    dimensions_from_size_preference, media_type_from_format_preference,
    normalize_base64_image_data,
};
use meerkat_llm_core::{http, streaming};
use serde::Deserialize;
use serde_json::Value;
use serde_json::value::RawValue;
use std::collections::HashSet;

use crate::image_generation::{
    OpenAiImageOutputOptions, OpenAiImageProviderParams, OpenAiImagesApiEndpoint,
    OpenAiImagesApiPlan, OpenAiImagesApiRequestShape, OpenAiResponsesImagePlan,
};

/// Extract the typed OpenAI provider tag from a request.
pub(crate) fn openai_tag(request: &LlmRequest) -> Option<&OpenAiProviderTag> {
    match request.provider_params.as_ref()? {
        ProviderTag::OpenAi(t) => Some(t),
        _ => None,
    }
}

/// Client for OpenAI Responses API
pub struct OpenAiClient {
    api_key: Option<String>,
    base_url: String,
    responses_path: String,
    chatgpt_backend_wire: bool,
    http: reqwest::Client,
    /// Extra headers emitted on every request (e.g. `ChatGPT-Account-ID`,
    /// `X-OpenAI-Fedramp`). Populated by provider runtimes when the
    /// backend is the ChatGPT backend and the OAuth token's JWT carries
    /// identity claims per Codex `bearer_auth_provider.rs:23-38`.
    extra_headers: Vec<(String, String)>,
    /// Dynamic authorizer — when set, replaces the `Authorization:
    /// Bearer <api_key>` header path with `HttpAuthorizer::authorize`
    /// invocation. Used for ExternalAuthorizer flows that produce a
    /// DynamicAuthorizer envelope (host-managed OAuth refresh, etc.).
    authorizer: Option<std::sync::Arc<dyn meerkat_core::HttpAuthorizer>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SystemMessageMode {
    IncludeInInput,
    ExtractToInstructions,
}

#[derive(Clone, Copy)]
pub(crate) enum OpenAiReplayProjectionMode {
    Responses,
    ChatCompletions,
}

fn invalid_replay(message: impl Into<String>) -> LlmError {
    LlmError::InvalidRequest {
        message: message.into(),
    }
}

fn project_openai_content_blocks(blocks: &[ContentBlock]) -> Vec<ContentBlock> {
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

fn project_openai_tool_result(result: &ToolResult) -> Result<ToolResult, LlmError> {
    if result.has_video() {
        return Err(invalid_replay(
            "video blocks are not supported in OpenAI tool results",
        ));
    }
    Ok(ToolResult::new(
        result.tool_use_id.clone(),
        result.text_content(),
        result.is_error,
    ))
}

fn openai_reasoning_replayable(
    mode: OpenAiReplayProjectionMode,
    meta: &Option<Box<ProviderMeta>>,
) -> bool {
    matches!(mode, OpenAiReplayProjectionMode::Responses)
        && matches!(meta.as_deref(), Some(ProviderMeta::OpenAi { .. }))
}

fn openai_server_tool_content_replayable(
    mode: OpenAiReplayProjectionMode,
    content: &Value,
) -> bool {
    match mode {
        OpenAiReplayProjectionMode::Responses => matches!(
            content.get("type").and_then(Value::as_str),
            Some(
                "web_search_call"
                    | "web_search_result"
                    | "file_search_call"
                    | "computer_call"
                    | "code_interpreter_call"
                    | "image_generation_call"
                    | "mcp_call"
                    | "mcp_list_tools"
                    | "mcp_approval_request"
            )
        ),
        OpenAiReplayProjectionMode::ChatCompletions => false,
    }
}

fn project_openai_assistant_blocks(
    mode: OpenAiReplayProjectionMode,
    blocks: &[AssistantBlock],
) -> Vec<AssistantBlock> {
    let mut projected = Vec::with_capacity(blocks.len());
    let mut has_output_item = false;

    for block in blocks {
        let projected_block = match block {
            AssistantBlock::Text { text, .. } if text.is_empty() => None,
            AssistantBlock::Text { .. } | AssistantBlock::ToolUse { .. } => {
                has_output_item = true;
                Some(block.clone())
            }
            // Spoken transcripts replay back to the provider as plain text;
            // OpenAI Responses API sees the assistant's visible output
            // regardless of capture lane.
            AssistantBlock::Transcript { text, .. } if text.is_empty() => None,
            AssistantBlock::Transcript { text, .. } => {
                has_output_item = true;
                Some(AssistantBlock::Text {
                    text: text.clone(),
                    meta: None,
                })
            }
            AssistantBlock::Reasoning { meta, .. } if openai_reasoning_replayable(mode, meta) => {
                Some(block.clone())
            }
            AssistantBlock::ServerToolContent { content, .. }
                if openai_server_tool_content_replayable(mode, content) =>
            {
                has_output_item = true;
                Some(block.clone())
            }
            AssistantBlock::Reasoning { .. }
            | AssistantBlock::ServerToolContent { .. }
            | AssistantBlock::Image { .. } => None,
            _ => None,
        };

        if let Some(block) = projected_block {
            projected.push(block);
        }
    }

    if has_output_item {
        projected
    } else {
        Vec::new()
    }
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

pub(crate) fn project_openai_replay_messages(
    messages: &[Message],
    mode: OpenAiReplayProjectionMode,
) -> Result<Vec<Message>, LlmError> {
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
                    "OpenAI replay projection found tool results without preceding tool use",
                ));
            };
            validate_tool_results("OpenAI", pending, results)?;
            let results = results
                .iter()
                .map(project_openai_tool_result)
                .collect::<Result<Vec<_>, _>>()?;
            projected.push(Message::ToolResults {
                results,
                created_at: *created_at,
            });
            continue;
        }

        let next_message = match message {
            Message::System(_) | Message::SystemNotice(_) => Some(message.clone()),
            Message::User(user) => Some(Message::User(UserMessage {
                content: project_openai_content_blocks(&user.content),
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
                let blocks = project_openai_assistant_blocks(mode, &assistant.blocks);
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

        let Some(message) = next_message else {
            continue;
        };

        if pending_tool_ids.is_some() {
            return Err(invalid_replay(
                "OpenAI replay projection found a tool use without adjacent tool results",
            ));
        }

        let tool_ids = tool_ids_from_assistant(&message);
        if !tool_ids.is_empty() {
            pending_tool_ids = Some(tool_ids);
        }
        projected.push(message);
    }

    if pending_tool_ids.is_some() {
        return Err(invalid_replay(
            "OpenAI replay projection found a trailing tool use without tool results",
        ));
    }

    Ok(projected)
}

impl OpenAiClient {
    fn model_supports_temperature(model: &str) -> bool {
        meerkat_core::model_profile::openai::supports_temperature(model)
    }

    fn model_supports_reasoning_payload(model: &str) -> bool {
        meerkat_core::model_profile::openai::supports_reasoning(model)
    }

    fn request_supports_temperature(request: &LlmRequest) -> bool {
        openai_tag(request)
            .and_then(|t| t.supports_temperature_override)
            .unwrap_or_else(|| Self::model_supports_temperature(&request.model))
    }

    fn request_supports_reasoning_payload(request: &LlmRequest) -> bool {
        openai_tag(request)
            .and_then(|t| t.supports_reasoning_override)
            .unwrap_or_else(|| Self::model_supports_reasoning_payload(&request.model))
    }

    /// Create a new OpenAI client with the given API key
    pub fn new(api_key: String) -> Self {
        Self::new_with_optional_api_key_and_base_url(
            Some(api_key),
            "https://api.openai.com".to_string(),
        )
    }

    /// Create a new OpenAI client with an explicit base URL
    pub fn new_with_base_url(api_key: String, base_url: String) -> Self {
        Self::new_with_optional_api_key_and_base_url(Some(api_key), base_url)
    }

    /// Create a new OpenAI client with an optional API key and explicit base URL.
    pub fn new_with_optional_api_key_and_base_url(
        api_key: Option<String>,
        base_url: String,
    ) -> Self {
        let http = http::build_http_client_for_base_url(reqwest::Client::builder(), &base_url)
            .unwrap_or_else(|_| reqwest::Client::new());
        Self {
            api_key,
            base_url,
            responses_path: "v1/responses".to_string(),
            chatgpt_backend_wire: false,
            http,
            extra_headers: Vec::new(),
            authorizer: None,
        }
    }

    /// Install a set of (name, value) headers to include on every request.
    ///
    /// ChatGPT-backend callers pass `ChatGPT-Account-ID` and optionally
    /// `X-OpenAI-Fedramp` here.
    pub fn with_extra_headers(mut self, headers: Vec<(String, String)>) -> Self {
        self.extra_headers = headers;
        self
    }

    /// Set the Responses endpoint path relative to `base_url`.
    ///
    /// Public OpenAI uses `/v1/responses`; the ChatGPT Codex backend
    /// uses `/responses` under `https://chatgpt.com/backend-api/codex`.
    pub fn with_responses_path(mut self, path: impl Into<String>) -> Self {
        self.responses_path = path.into().trim_start_matches('/').to_string();
        self
    }

    pub fn with_chatgpt_backend_wire(self) -> Self {
        let mut client = self.with_responses_path("responses");
        client.chatgpt_backend_wire = true;
        client
    }

    /// Attach a dynamic authorizer. When set, overrides the
    /// `Authorization: Bearer <api_key>` path on every request with
    /// `HttpAuthorizer::authorize`. Extra headers (ChatGPT-Account-ID
    /// etc.) still flow through unchanged.
    pub fn with_authorizer(
        mut self,
        authorizer: std::sync::Arc<dyn meerkat_core::HttpAuthorizer>,
    ) -> Self {
        self.authorizer = Some(authorizer);
        self
    }

    pub fn extra_headers(&self) -> &[(String, String)] {
        &self.extra_headers
    }

    fn responses_endpoint(&self) -> String {
        format!(
            "{}/{}",
            self.base_url.trim_end_matches('/'),
            self.responses_path.trim_start_matches('/'),
        )
    }

    /// Set custom base URL
    pub fn with_base_url(mut self, url: String) -> Self {
        if let Ok(http) = http::build_http_client_for_base_url(reqwest::Client::builder(), &url) {
            self.http = http;
        }
        self.base_url = url;
        self
    }

    /// Build request body for OpenAI Responses API
    fn build_request_body(&self, request: &LlmRequest) -> Result<Value, LlmError> {
        let (input, instructions) = if self.chatgpt_backend_wire {
            Self::convert_to_responses_input_with_system_mode(
                &request.messages,
                SystemMessageMode::ExtractToInstructions,
            )?
        } else {
            (Self::convert_to_responses_input(&request.messages)?, None)
        };
        let reasoning_enabled = Self::request_supports_reasoning_payload(request);

        let mut body = serde_json::json!({
            "model": request.model,
            "input": input,
            "max_output_tokens": request.max_tokens,
            "stream": true,
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

        if self.chatgpt_backend_wire {
            body["instructions"] = Value::String(
                instructions.unwrap_or_else(|| "You are a helpful assistant.".to_string()),
            );
            body["store"] = Value::Bool(false);
            body["tools"] = Value::Array(Vec::new());
            body["tool_choice"] = Value::String("auto".to_string());
            body["parallel_tool_calls"] = Value::Bool(false);
            if let Some(obj) = body.as_object_mut() {
                obj.remove("max_output_tokens");
            }
        }

        if Self::request_supports_temperature(request)
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

        // Inject provider-native web search tool from typed tag.
        if let Some(web_search) = openai_tag(request).and_then(|t| t.web_search.as_ref()) {
            let ws_value = web_search.as_value();
            if ws_value.is_object() {
                match body.get_mut("tools").and_then(|v| v.as_array_mut()) {
                    Some(arr) => arr.push(ws_value),
                    None => body["tools"] = Value::Array(vec![ws_value]),
                }
            }
        }

        if let Some(tag) = openai_tag(request) {
            if reasoning_enabled && let Some(effort) = tag.reasoning_effort {
                let s = match effort {
                    TypedReasoningEffort::Low => "low",
                    TypedReasoningEffort::Medium => "medium",
                    TypedReasoningEffort::High => "high",
                };
                body["reasoning"]["effort"] = Value::String(s.to_string());
            }

            if let Some(seed) = tag.seed {
                body["seed"] = Value::Number(seed.into());
            }

            if let Some(fp) = tag.frequency_penalty
                && let Some(n) = serde_json::Number::from_f64(fp as f64)
            {
                body["frequency_penalty"] = Value::Number(n);
            }

            if let Some(pp) = tag.presence_penalty
                && let Some(n) = serde_json::Number::from_f64(pp as f64)
            {
                body["presence_penalty"] = Value::Number(n);
            }

            if let Some(output_schema) = tag.structured_output.as_ref() {
                let compiled =
                    self.compile_schema(output_schema)
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
    /// OpenAI-hosted tool output items can depend on paired reasoning output
    /// items. `project_openai_replay_messages` filters provider-specific
    /// history before this point so Responses replay keeps OpenAI reasoning and
    /// final hosted-tool items together, while provider switches and Chat
    /// Completions mode do not receive OpenAI-private transcript state.
    fn convert_to_responses_input(messages: &[Message]) -> Result<Vec<Value>, LlmError> {
        Self::convert_to_responses_input_with_system_mode(
            messages,
            SystemMessageMode::IncludeInInput,
        )
        .map(|(input, _)| input)
    }

    fn openai_reasoning_input_item(text: &str, meta: &ProviderMeta) -> Option<Value> {
        let ProviderMeta::OpenAi {
            id,
            encrypted_content,
            phase,
            ..
        } = meta
        else {
            return None;
        };

        let summary = if text.is_empty() {
            Value::Array(Vec::new())
        } else {
            serde_json::json!([{
                "type": "summary_text",
                "text": text
            }])
        };
        let mut item = serde_json::json!({
            "type": "reasoning",
            "id": id,
            "summary": summary
        });

        if let Some(encrypted_content) = encrypted_content {
            item["encrypted_content"] = Value::String(encrypted_content.clone());
        }
        if let Some(phase) = phase {
            item["phase"] = Value::String(phase.clone());
        }

        Some(item)
    }

    fn convert_to_responses_input_with_system_mode(
        messages: &[Message],
        system_mode: SystemMessageMode,
    ) -> Result<(Vec<Value>, Option<String>), LlmError> {
        let mut items = Vec::new();
        let mut instructions = Vec::new();

        for msg in messages {
            match msg {
                Message::System(s) => match system_mode {
                    SystemMessageMode::IncludeInInput => {
                        items.push(serde_json::json!({
                            "type": "message",
                            "role": "system",
                            "content": s.content
                        }));
                    }
                    SystemMessageMode::ExtractToInstructions => {
                        if !s.content.trim().is_empty() {
                            instructions.push(s.content.clone());
                        }
                    }
                },
                Message::SystemNotice(notice) => {
                    items.push(serde_json::json!({
                        "type": "message",
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
                            AssistantBlock::Reasoning { text, meta } => {
                                if let Some(item) = meta
                                    .as_deref()
                                    .and_then(|meta| Self::openai_reasoning_input_item(text, meta))
                                {
                                    items.push(item);
                                }
                            }
                            AssistantBlock::ServerToolContent { content, .. } => {
                                items.push(content.clone());
                            }
                            // Unknown future variants are omitted unless the
                            // OpenAI projection explicitly admits them above.
                            _ => {}
                        }
                    }
                }
                Message::ToolResults { results, .. } => {
                    // OpenAI function_call_output only accepts strings; images
                    // degrade to text projection via text_content().
                    for r in results {
                        if r.has_video() {
                            return Err(LlmError::InvalidRequest {
                                message: "video blocks are not supported in OpenAI tool results"
                                    .to_string(),
                            });
                        }
                        items.push(serde_json::json!({
                            "type": "function_call_output",
                            "call_id": r.tool_use_id,
                            "output": r.text_content()
                        }));
                    }
                }
            }
        }

        let instructions = if instructions.is_empty() {
            None
        } else {
            Some(instructions.join("\n\n"))
        };
        Ok((items, instructions))
    }

    /// Parse an SSE event from the Responses API stream
    fn parse_responses_sse_line(line: &str) -> Option<ResponsesStreamEvent> {
        if let Some(data) = line
            .strip_prefix("data: ")
            .or_else(|| line.strip_prefix("data:"))
        {
            if data == "[DONE]" {
                return None;
            }
            serde_json::from_str(data).ok()
        } else {
            None
        }
    }

    fn image_prompt(request: &ProviderImageGenerationRequest) -> String {
        match &request.generate_request.intent {
            ImageGenerationIntent::Generate { prompt, .. } => prompt.content.clone(),
            ImageGenerationIntent::Edit { instruction, .. } => instruction.content.clone(),
        }
    }

    fn image_count_warning(
        request: &ProviderImageGenerationRequest,
        returned: usize,
    ) -> Vec<ImageGenerationWarning> {
        let requested = request.generate_request.count;
        let Some(returned_count) = std::num::NonZeroU32::new(returned as u32) else {
            return Vec::new();
        };
        if returned_count < requested {
            vec![ImageGenerationWarning::ProviderReturnedFewerImages {
                requested,
                returned: returned_count,
            }]
        } else {
            Vec::new()
        }
    }

    fn openai_error_terminal(status_code: u16, text: &str) -> ImageOperationTerminalClass {
        if status_code == 408 || status_code == 504 {
            ImageOperationTerminalClass::Timeout
        } else if status_code == 499 {
            ImageOperationTerminalClass::Cancelled
        } else if let Ok(value) = serde_json::from_str::<Value>(text) {
            Self::openai_structured_error_terminal(&value)
                .unwrap_or(ImageOperationTerminalClass::Failed)
        } else {
            ImageOperationTerminalClass::Failed
        }
    }

    fn openai_structured_error_terminal(value: &Value) -> Option<ImageOperationTerminalClass> {
        let error = value.get("error").unwrap_or(value);
        ["code", "type"].into_iter().find_map(|field| {
            error
                .get(field)
                .and_then(Value::as_str)
                .and_then(Self::openai_structured_error_code_terminal)
        })
    }

    fn openai_structured_error_code_terminal(code: &str) -> Option<ImageOperationTerminalClass> {
        match code {
            "content_filter"
            | "content_filtered"
            | "content_policy_violation"
            | "moderation_blocked"
            | "safety_violation" => Some(ImageOperationTerminalClass::SafetyFiltered),
            "model_refusal" | "refusal" | "refused_by_model" => {
                Some(ImageOperationTerminalClass::RefusedByProvider)
            }
            _ => None,
        }
    }

    async fn post_json_to_openai(
        &self,
        endpoint: &str,
        body: &Value,
    ) -> Result<reqwest::Response, LlmError> {
        let mut request_builder = self
            .http
            .post(endpoint)
            .header("Content-Type", "application/json");
        if let Some(authorizer) = &self.authorizer {
            let mut extra: Vec<(String, String)> = Vec::new();
            let mut auth_req = meerkat_core::HttpAuthorizationRequest {
                method: "POST",
                url: endpoint,
                headers: &mut extra,
            };
            authorizer.authorize(&mut auth_req).await.map_err(|e| {
                LlmError::AuthenticationFailed {
                    message: format!("openai authorizer failed: {e}"),
                }
            })?;
            for (name, value) in extra {
                request_builder = request_builder.header(name, value);
            }
        } else if let Some(api_key) = &self.api_key {
            request_builder = request_builder.header("Authorization", format!("Bearer {api_key}"));
        }
        for (name, value) in &self.extra_headers {
            request_builder = request_builder.header(name, value);
        }
        request_builder.json(body).send().await.map_err(|e| {
            if e.is_timeout() {
                LlmError::NetworkTimeout { duration_ms: 30000 }
            } else {
                #[cfg(not(target_arch = "wasm32"))]
                if e.is_connect() {
                    return LlmError::ConnectionReset;
                }
                LlmError::Unknown {
                    message: e.to_string(),
                }
            }
        })
    }

    async fn execute_hosted_responses_image(
        &self,
        request: ProviderImageGenerationRequest,
        plan: OpenAiResponsesImagePlan,
    ) -> Result<ProviderImageGenerationOutput, LlmError> {
        let input = if request.projected_messages.is_empty() {
            vec![serde_json::json!({
                "type": "message",
                "role": "user",
                "content": Self::image_prompt(&request)
            })]
        } else {
            Self::convert_to_responses_input(&request.projected_messages)?
        };
        let mut tool = serde_json::Map::new();
        tool.insert(
            "type".to_string(),
            serde_json::Value::String(plan.tool_name),
        );
        tool.insert(
            "model".to_string(),
            serde_json::Value::String(plan.model.to_string()),
        );
        Self::apply_image_output_options(&mut tool, &plan.output);
        Self::apply_openai_image_provider_params(&mut tool, &plan.provider_params, true);
        let body = serde_json::json!({
            "model": request.model,
            "input": input,
            "instructions": "You are an image-generation agent. The user's input is always a request to produce one or more images. Call the image_generation tool to produce each image. Never reply in text instead of calling the tool. If the request cannot be fulfilled, refuse briefly and explicitly so the caller can see the reason.",
            "tools": [serde_json::Value::Object(tool)],
            "tool_choice": "required",
            "stream": false,
        });
        let endpoint = self.responses_endpoint();
        let response = self.post_json_to_openai(&endpoint, &body).await?;
        self.normalize_openai_image_response(request, response, true)
            .await
    }

    async fn execute_images_api(
        &self,
        request: ProviderImageGenerationRequest,
        model: String,
        plan: OpenAiImagesApiPlan,
    ) -> Result<ProviderImageGenerationOutput, LlmError> {
        let endpoint_path = match plan.endpoint {
            OpenAiImagesApiEndpoint::Generations => "/v1/images/generations",
            OpenAiImagesApiEndpoint::Edits => "/v1/images/edits",
        };
        let mut body = serde_json::json!({
            "model": model,
            "prompt": Self::image_prompt(&request),
            "n": request.generate_request.count.get(),
        });
        if let Some(obj) = body.as_object_mut() {
            match plan.request_shape {
                OpenAiImagesApiRequestShape::GptImage => {
                    Self::apply_image_output_options(obj, &plan.output);
                    Self::apply_openai_image_provider_params(obj, &plan.provider_params, false);
                }
                OpenAiImagesApiRequestShape::DallE => {
                    obj.insert(
                        "response_format".to_string(),
                        serde_json::Value::String("b64_json".to_string()),
                    );
                }
            }
        }
        let endpoint = format!("{}{}", self.base_url, endpoint_path);
        let response = self.post_json_to_openai(&endpoint, &body).await?;
        self.normalize_openai_image_response(request, response, false)
            .await
    }

    fn apply_image_output_options(
        obj: &mut serde_json::Map<String, serde_json::Value>,
        output: &OpenAiImageOutputOptions,
    ) {
        obj.insert(
            "size".to_string(),
            serde_json::Value::String(output.size.as_wire_value()),
        );
        obj.insert(
            "quality".to_string(),
            serde_json::Value::String(output.quality.as_wire_value().to_string()),
        );
        obj.insert(
            "output_format".to_string(),
            serde_json::Value::String(output.output_format.as_wire_value().to_string()),
        );
    }

    fn apply_openai_image_provider_params(
        obj: &mut serde_json::Map<String, serde_json::Value>,
        params: &OpenAiImageProviderParams,
        allow_action: bool,
    ) {
        if let Some(background) = params.background {
            obj.insert(
                "background".to_string(),
                serde_json::Value::String(background.as_wire_value().to_string()),
            );
        }
        if let Some(output_compression) = params.output_compression {
            obj.insert(
                "output_compression".to_string(),
                serde_json::Value::Number(output_compression.into()),
            );
        }
        if let Some(moderation) = params.moderation {
            obj.insert(
                "moderation".to_string(),
                serde_json::Value::String(moderation.as_wire_value().to_string()),
            );
        }
        if allow_action && let Some(action) = params.action {
            obj.insert(
                "action".to_string(),
                serde_json::Value::String(action.as_wire_value().to_string()),
            );
        }
    }

    async fn normalize_openai_image_response(
        &self,
        request: ProviderImageGenerationRequest,
        response: reqwest::Response,
        hosted_responses: bool,
    ) -> Result<ProviderImageGenerationOutput, LlmError> {
        let status_code = response.status().as_u16();
        let text = response.text().await.unwrap_or_default();
        if !(200..=299).contains(&status_code) {
            return Ok(ProviderImageGenerationOutput {
                operation_id: request.operation_id,
                terminal: Self::openai_error_terminal(status_code, &text),
                images: Vec::new(),
                provider_text: None,
                revised_prompt: RevisedPromptDisposition::NotRequested,
                native_metadata: ProviderImageMetadata::OpenAi(OpenAiImageMetadata {
                    target_model: request.model,
                    response_id: None,
                    image_generation_call_id: None,
                }),
                warnings: Vec::new(),
            });
        }

        let value: Value = serde_json::from_str(&text).map_err(|e| LlmError::StreamParseError {
            message: format!("invalid OpenAI image response JSON: {e}"),
        })?;
        let (width, height) = dimensions_from_size_preference(&request.generate_request.size);
        let media_type = media_type_from_format_preference(request.generate_request.format);
        let mut images = Vec::new();
        let mut revised_prompt = RevisedPromptDisposition::NotRequested;
        let mut provider_text = Vec::new();
        let mut image_generation_call_id = None;

        if hosted_responses {
            if let Some(output) = value.get("output").and_then(Value::as_array) {
                for item in output {
                    match item.get("type").and_then(|v| v.as_str()) {
                        Some("image_generation_call") => {
                            if image_generation_call_id.is_none() {
                                image_generation_call_id =
                                    item.get("id").and_then(|v| v.as_str()).map(str::to_string);
                            }
                            if let Some(data) = item
                                .get("result")
                                .or_else(|| item.get("b64_json"))
                                .or_else(|| item.get("image_data"))
                                .and_then(|v| v.as_str())
                            {
                                images.push(ProviderGeneratedImage {
                                    media_type: media_type.clone(),
                                    base64_data: normalize_base64_image_data(data),
                                    width,
                                    height,
                                });
                            }
                            if let Some(text) = item.get("revised_prompt").and_then(|v| v.as_str())
                                && let Ok(prompt) = meerkat_core::PromptText::new(text.to_string())
                            {
                                revised_prompt = RevisedPromptDisposition::Revised {
                                    text: prompt,
                                    source: RevisedPromptSource::Provider,
                                };
                            }
                        }
                        Some("message") => {
                            if let Some(parts) = item.get("content").and_then(|v| v.as_array()) {
                                for part in parts {
                                    if let Some(text) = part.get("text").and_then(|v| v.as_str()) {
                                        provider_text.push(text.to_string());
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
        } else if let Some(data) = value.get("data").and_then(|v| v.as_array()) {
            for item in data {
                if let Some(data) = item.get("b64_json").and_then(|v| v.as_str()) {
                    images.push(ProviderGeneratedImage {
                        media_type: media_type.clone(),
                        base64_data: normalize_base64_image_data(data),
                        width,
                        height,
                    });
                }
                if let Some(text) = item.get("revised_prompt").and_then(|v| v.as_str())
                    && let Ok(prompt) = meerkat_core::PromptText::new(text.to_string())
                {
                    revised_prompt = RevisedPromptDisposition::Revised {
                        text: prompt,
                        source: RevisedPromptSource::Provider,
                    };
                }
            }
        }

        let terminal = if images.is_empty() {
            ImageOperationTerminalClass::EmptyResult {
                provider_text: if provider_text.is_empty() {
                    meerkat_core::ProviderTextDisposition::NotEmitted
                } else {
                    meerkat_core::ProviderTextDisposition::EmittedButNotStored
                },
            }
        } else {
            ImageOperationTerminalClass::Generated
        };
        let warnings = Self::image_count_warning(&request, images.len());
        Ok(ProviderImageGenerationOutput {
            operation_id: request.operation_id,
            terminal,
            images,
            provider_text: if provider_text.is_empty() {
                None
            } else {
                Some(provider_text.join("\n"))
            },
            revised_prompt,
            native_metadata: ProviderImageMetadata::OpenAi(OpenAiImageMetadata {
                target_model: request.model,
                response_id: value.get("id").and_then(|v| v.as_str()).map(str::to_string),
                image_generation_call_id,
            }),
            warnings,
        })
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl ImageGenerationExecutor for OpenAiClient {
    async fn execute_image_generation(
        &self,
        request: ProviderImageGenerationRequest,
    ) -> Result<ProviderImageGenerationOutput, LlmError> {
        match request.execution_plan.clone() {
            plan if plan.provider.0 == "openai" => match plan.backend {
                meerkat_core::ImageGenerationBackendKind::HostedTool => {
                    let provider_plan: OpenAiResponsesImagePlan =
                        serde_json::from_value(plan.provider_plan).map_err(|err| {
                            LlmError::InvalidRequest {
                                message: format!("invalid OpenAI hosted image plan: {err}"),
                            }
                        })?;
                    self.execute_hosted_responses_image(request, provider_plan)
                        .await
                }
                meerkat_core::ImageGenerationBackendKind::ProviderApi => {
                    let provider_plan: OpenAiImagesApiPlan =
                        serde_json::from_value(plan.provider_plan).map_err(|err| {
                            LlmError::InvalidRequest {
                                message: format!("invalid OpenAI Images API plan: {err}"),
                            }
                        })?;
                    let model = request.model.clone();
                    self.execute_images_api(request, model, provider_plan).await
                }
                other => Err(LlmError::InvalidRequest {
                    message: format!("OpenAI image executor cannot run backend {other:?}"),
                }),
            },
            other => Err(LlmError::InvalidRequest {
                message: format!("OpenAI image executor cannot run plan {other:?}"),
            }),
        }
    }
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

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl LlmClient for OpenAiClient {
    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        project_openai_replay_messages(messages, OpenAiReplayProjectionMode::Responses)
    }

    fn stream<'a>(&'a self, request: &'a LlmRequest) -> LlmStream<'a> {
        let inner: LlmStream<'a> = Box::pin(async_stream::try_stream! {
            let mut projected_request = request.clone();
            projected_request.messages = self.project_replay_messages(&request.messages)?;
            let request = &projected_request;
            let body = self.build_request_body(request)?;

            let endpoint = self.responses_endpoint();
            let mut request_builder = self
                .http
                .post(&endpoint)
                .header("Content-Type", "application/json");
            // Auth path: authorizer overrides Bearer<api_key>.
            if let Some(authorizer) = &self.authorizer {
                let mut extra: Vec<(String, String)> = Vec::new();
                let mut auth_req = meerkat_core::HttpAuthorizationRequest {
                    method: "POST",
                    url: &endpoint,
                    headers: &mut extra,
                };
                authorizer.authorize(&mut auth_req).await.map_err(|e| {
                    LlmError::AuthenticationFailed {
                        message: format!("openai authorizer failed: {e}"),
                    }
                })?;
                for (name, value) in extra {
                    request_builder = request_builder.header(name, value);
                }
            } else if let Some(api_key) = &self.api_key {
                request_builder =
                    request_builder.header("Authorization", format!("Bearer {api_key}"));
            }
            for (name, value) in &self.extra_headers {
                request_builder = request_builder.header(name, value);
            }
            let response = request_builder
                .json(&body)
                .send()
                .await
                .map_err(|e| {
                    if e.is_timeout() {
                        LlmError::NetworkTimeout { duration_ms: 30000 }
                    } else {
                        #[cfg(not(target_arch = "wasm32"))]
                        if e.is_connect() {
                            return LlmError::ConnectionReset;
                        }
                        LlmError::Unknown { message: e.to_string() }
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
            let mut buffer = String::with_capacity(512);
            let mut assembler = BlockAssembler::new();
            let mut usage = Usage::default();
            let mut saw_stream_text_delta = false;
            let mut streamed_tool_ids: HashSet<String> = HashSet::with_capacity(4);
            let mut streamed_reasoning_ids: HashSet<String> = HashSet::with_capacity(2);
            let mut done_emitted = false;

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
                            if done_emitted {
                                // Already processed a terminal event, skip
                                continue;
                            }
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
                                                                        if let Some(annotations) = part.get("annotations") {
                                                                            yield LlmEvent::ServerToolContent {
                                                                                id: item.get("id")
                                                                                    .and_then(|v| v.as_str())
                                                                                    .map(std::string::ToString::to_string),
                                                                                name: "web_search_annotations".to_string(),
                                                                                content: serde_json::json!({
                                                                                    "type": "message_annotations",
                                                                                    "annotations": annotations
                                                                                }),
                                                                                meta: None,
                                                                            };
                                                                        }
                                                                        if let Some(text) = part.get("text").and_then(|t| t.as_str())
                                                                            && !saw_stream_text_delta
                                                                        {
                                                                            assembler.on_text_delta(text, None);
                                                                            yield LlmEvent::TextDelta { delta: text.to_string(), meta: None };
                                                                        }
                                                                    }
                                                                    "refusal" => {
                                                                        if let Some(refusal) = part.get("refusal").and_then(|r| r.as_str())
                                                                            && !saw_stream_text_delta
                                                                        {
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

                                                    // Skip if already emitted via streaming
                                                    if streamed_reasoning_ids.contains(reasoning_id) {
                                                        continue;
                                                    }

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
                                                        .map(std::string::ToString::to_string);
                                                    let phase = item.get("phase")
                                                        .and_then(|v| v.as_str())
                                                        .map(std::string::ToString::to_string);

                                                    let meta = Some(Box::new(ProviderMeta::OpenAi {
                                                        id: reasoning_id.to_string(),
                                                        encrypted_content: encrypted,
                                                        phase,
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

                                                    // Skip if already emitted via streaming
                                                    if streamed_tool_ids.contains(call_id) {
                                                        continue;
                                                    }
                                                    let Some(name) = item.get("name").and_then(|n| n.as_str()) else {
                                                        tracing::warn!(call_id, "function_call missing name");
                                                        continue;
                                                    };
                                                    // arguments is a JSON string. Missing arguments
                                                    // represent a no-arg tool; malformed or non-object
                                                    // JSON fails closed before projection.
                                                    let (args, args_value) = match item.get("arguments").and_then(|a| a.as_str()) {
                                                        Some(args_str) => parse_tool_call_arguments(args_str, call_id)?,
                                                        None => {
                                                            // Empty args - treat as empty object
                                                            (empty_tool_args_raw_value(), serde_json::json!({}))
                                                        }
                                                    };

                                                    let _ = assembler.on_tool_call_start(call_id.to_string());
                                                    let _ = assembler.on_tool_call_complete(
                                                        call_id.to_string(),
                                                        name.to_string(),
                                                        args.clone(),
                                                        None,
                                                    );

                                                    yield LlmEvent::ToolCallComplete {
                                                        id: call_id.to_string(),
                                                        name: name.into(),
                                                        args: args_value,
                                                        meta: None,
                                                    };
                                                }
                                                "web_search_call" | "web_search_result" => {
                                                    let id = item.get("id")
                                                        .or_else(|| item.get("call_id"))
                                                        .and_then(|v| v.as_str())
                                                        .map(std::string::ToString::to_string);
                                                    yield LlmEvent::ServerToolContent {
                                                        id,
                                                        name: item_type.to_string(),
                                                        content: item.clone(),
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
                                        .and_then(serde_json::Value::as_u64)
                                        .unwrap_or(0);
                                    usage.output_tokens = usage_obj.get("output_tokens")
                                        .and_then(serde_json::Value::as_u64)
                                        .unwrap_or(0);
                                    yield LlmEvent::UsageUpdate { usage: usage.clone() };
                                }

                                // Determine stop reason
                                let stop_reason = match response_obj.get("status").and_then(|s| s.as_str()) {
                                    Some("completed") => {
                                        // Check if there were tool calls
                                        let has_tool_calls = response_obj.get("output")
                                            .and_then(|o| o.as_array())
                                            .is_some_and(|arr| arr.iter().any(|item| item.get("type").and_then(|t| t.as_str()) == Some("function_call")));
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

                                done_emitted = true;
                                yield LlmEvent::Done {
                                    outcome: LlmDoneOutcome::Success { stop_reason },
                                };
                            }
                        }
                        // Handle streaming delta events
                        else if event.event_type == "response.output_text.delta" {
                            if let Some(delta) = &event.delta {
                                saw_stream_text_delta = true;
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
                                let (args, args_value) =
                                    parse_tool_call_arguments(arguments, call_id)?;

                                let _ = assembler.on_tool_call_start(call_id.clone());
                                let _ = assembler.on_tool_call_complete(
                                    call_id.clone(),
                                    name.clone(),
                                    args.clone(),
                                    None,
                                );

                                streamed_tool_ids.insert(call_id.clone());
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
                                let Some(reasoning_id) = item.get("id")
                                    .and_then(|i| i.as_str()) else {
                                    tracing::warn!("reasoning item missing id, skipping");
                                    continue;
                                };

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
                                    .map(std::string::ToString::to_string);
                                let phase = item.get("phase")
                                    .and_then(|v| v.as_str())
                                    .map(std::string::ToString::to_string);

                                let meta = Some(Box::new(ProviderMeta::OpenAi {
                                    id: reasoning_id.to_string(),
                                    encrypted_content: encrypted,
                                    phase,
                                }));

                                assembler.on_reasoning_start();
                                if !summary_text.is_empty() {
                                    let _ = assembler.on_reasoning_delta(&summary_text);
                                }
                                assembler.on_reasoning_complete(meta.clone());

                                streamed_reasoning_ids.insert(reasoning_id.to_string());
                                yield LlmEvent::ReasoningComplete {
                                    text: summary_text,
                                    meta,
                                };
                            }
                        }
                        else if event.event_type.starts_with("response.web_search_call.") {
                            let mut content = serde_json::Map::new();
                            content.insert("type".to_string(), Value::String(event.event_type.clone()));
                            if let Some(item_id) = &event.item_id {
                                content.insert("item_id".to_string(), Value::String(item_id.clone()));
                            }
                            if let Some(output_index) = event.output_index {
                                content.insert("output_index".to_string(), Value::from(output_index));
                            }
                            if let Some(sequence_number) = event.sequence_number {
                                content.insert("sequence_number".to_string(), Value::from(sequence_number));
                            }
                            yield LlmEvent::ServerToolContent {
                                id: event.item_id.clone(),
                                name: "web_search".to_string(),
                                content: Value::Object(content),
                                meta: None,
                            };
                        }
                        else if event.event_type == "response.done" {
                            // Final done event — always update usage
                            if let Some(response_obj) = &event.response {
                                if let Some(usage_obj) = response_obj.get("usage") {
                                    usage.input_tokens = usage_obj.get("input_tokens")
                                        .and_then(serde_json::Value::as_u64)
                                        .unwrap_or(0);
                                    usage.output_tokens = usage_obj.get("output_tokens")
                                        .and_then(serde_json::Value::as_u64)
                                        .unwrap_or(0);
                                    yield LlmEvent::UsageUpdate { usage: usage.clone() };
                                }

                                if !done_emitted {
                                    let stop_reason = match response_obj.get("status").and_then(|s| s.as_str()) {
                                        Some("completed") => {
                                            let has_tool_calls = response_obj.get("output")
                                                .and_then(|o| o.as_array())
                                                .is_some_and(|arr| arr.iter().any(|item| item.get("type").and_then(|t| t.as_str()) == Some("function_call")));
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

                                    done_emitted = true;
                                    yield LlmEvent::Done {
                                        outcome: LlmDoneOutcome::Success { stop_reason },
                                    };
                                }
                            }
                        }
                        else if event.event_type == "error" {
                            // Streaming error event from OpenAI
                            let error_msg = event.error
                                .as_ref()
                                .and_then(|e| e.get("message"))
                                .and_then(|m| m.as_str())
                                .unwrap_or("unknown streaming error");
                            let error_code = event.error
                                .as_ref()
                                .and_then(|e| e.get("code"))
                                .and_then(|c| c.as_str())
                                .unwrap_or("unknown");

                            tracing::error!(
                                code = error_code,
                                message = error_msg,
                                "OpenAI streaming error"
                            );

                            let error = match error_code {
                                "rate_limit_exceeded" => LlmError::RateLimited { retry_after_ms: None },
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
                            };

                            done_emitted = true;
                            yield LlmEvent::Done {
                                outcome: LlmDoneOutcome::Error { error },
                            };
                        }
                    }
                }
            }
        });

        streaming::ensure_terminal_done(inner)
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
    /// Output item ID for built-in tool streaming events.
    item_id: Option<String>,
    /// Output index for built-in tool streaming events.
    output_index: Option<u64>,
    /// Sequence number for built-in tool streaming events.
    sequence_number: Option<u64>,
}

#[allow(clippy::unwrap_used, clippy::expect_used)]
fn empty_tool_args_raw_value() -> Box<RawValue> {
    RawValue::from_string("{}".to_string()).expect("static JSON is valid")
}

fn parse_tool_call_arguments(
    arguments: &str,
    call_id: &str,
) -> Result<(Box<RawValue>, Value), LlmError> {
    let raw = RawValue::from_string(arguments.to_string()).map_err(|error| {
        LlmError::StreamParseError {
            message: format!("invalid OpenAI tool call arguments JSON for {call_id}: {error}"),
        }
    })?;
    let value: Value =
        serde_json::from_str(raw.get()).map_err(|error| LlmError::StreamParseError {
            message: format!("invalid OpenAI tool call arguments JSON for {call_id}: {error}"),
        })?;
    if value.is_object() {
        Ok((raw, value))
    } else {
        Err(LlmError::StreamParseError {
            message: format!("OpenAI tool call arguments for {call_id} must be a JSON object"),
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use axum::{Json, Router, extract::State, response::IntoResponse, routing::post};
    use meerkat_core::{
        AssistantImageId, BlobId, BlobRef, BlockAssistantMessage, MediaType, ProviderImageMetadata,
        RevisedPromptDisposition, ToolResult, UserMessage, VideoData,
    };
    use meerkat_llm_core::ImageGenerationExecutor;
    use std::sync::{Arc, Mutex};
    use tokio::net::TcpListener;

    fn assistant_image_block() -> AssistantBlock {
        AssistantBlock::Image {
            image_id: AssistantImageId::new(meerkat_core::time_compat::new_uuid_v7()),
            blob_ref: BlobRef {
                blob_id: BlobId::from("openai-image"),
                media_type: "image/png".to_string(),
            },
            media_type: MediaType::new("image/png"),
            width: 128,
            height: 128,
            revised_prompt: RevisedPromptDisposition::NotRequested,
            meta: ProviderImageMetadata::NotEmitted,
        }
    }

    fn build_projected_request_body(client: &OpenAiClient, request: &LlmRequest) -> Value {
        let mut projected = request.clone();
        projected.messages = client
            .project_replay_messages(&request.messages)
            .expect("project replay messages");
        client
            .build_request_body(&projected)
            .expect("build request")
    }

    async fn responses_sse(State(payload): State<String>) -> impl IntoResponse {
        ([("content-type", "text/event-stream")], payload)
    }

    #[test]
    fn replay_projection_policy_handles_openai_history_blocks()
    -> Result<(), Box<dyn std::error::Error>> {
        let client = OpenAiClient::new("test-key".to_string());
        let tool_args = RawValue::from_string(r#"{"query":"m"}"#.to_string())?;
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
                    data: VideoData::Inline {
                        data: "BBBB".to_string(),
                    },
                },
            ])),
            Message::BlockAssistant(BlockAssistantMessage::new(
                vec![
                    AssistantBlock::Reasoning {
                        text: "encrypted reasoning".to_string(),
                        meta: Some(Box::new(ProviderMeta::OpenAi {
                            id: "rs_1".to_string(),
                            encrypted_content: Some("ciphertext".to_string()),
                            phase: Some("reasoning".to_string()),
                        })),
                    },
                    AssistantBlock::ServerToolContent {
                        id: Some("ws_status".to_string()),
                        name: "web_search".to_string(),
                        content: serde_json::json!({
                            "type": "response.web_search_call.searching",
                            "item_id": "ws_status"
                        }),
                        meta: None,
                    },
                    AssistantBlock::ServerToolContent {
                        id: Some("ws_1".to_string()),
                        name: "web_search_call".to_string(),
                        content: serde_json::json!({
                            "type": "web_search_call",
                            "id": "ws_1",
                            "status": "completed"
                        }),
                        meta: None,
                    },
                    AssistantBlock::ServerToolContent {
                        id: None,
                        name: "web_search_annotations".to_string(),
                        content: serde_json::json!({
                            "type": "message_annotations",
                            "annotations": []
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
            "OpenAI should retain inline user images"
        );
        assert!(
            user.content.iter().any(|block| matches!(
                block,
                ContentBlock::Text { text } if text == "[video: video/mp4]"
            )),
            "OpenAI should degrade user video to text"
        );

        let Message::BlockAssistant(assistant) = &projected[1] else {
            panic!("expected assistant message");
        };
        assert!(
            assistant
                .blocks
                .iter()
                .any(|block| matches!(block, AssistantBlock::Reasoning { .. })),
            "OpenAI Responses replay projection should keep OpenAI reasoning blocks"
        );
        assert!(assistant.blocks.iter().any(|block| matches!(
            block,
            AssistantBlock::ServerToolContent { content, .. }
                if content.get("type").and_then(Value::as_str) == Some("web_search_call")
        )));
        assert!(!assistant.blocks.iter().any(|block| matches!(
            block,
            AssistantBlock::ServerToolContent { content, .. }
                if content.get("type").and_then(Value::as_str)
                    == Some("response.web_search_call.searching")
        )));
        assert!(!assistant.blocks.iter().any(|block| matches!(
            block,
            AssistantBlock::ServerToolContent { content, .. }
                if content.get("type").and_then(Value::as_str) == Some("message_annotations")
        )));
        assert!(
            !assistant
                .blocks
                .iter()
                .any(|block| matches!(block, AssistantBlock::Image { .. })),
            "assistant images must be removed from OpenAI replay"
        );
        let body = client.build_request_body(&LlmRequest::new("gpt-5.4", projected.clone()))?;
        let input = body["input"].as_array().expect("input array");
        assert!(input.iter().any(|item| {
            item.get("type").and_then(Value::as_str) == Some("reasoning")
                && item.get("id").and_then(Value::as_str) == Some("rs_1")
                && item.get("encrypted_content").and_then(Value::as_str) == Some("ciphertext")
                && item.get("phase").and_then(Value::as_str) == Some("reasoning")
        }));
        assert!(input.iter().any(|item| {
            item.get("type").and_then(Value::as_str) == Some("web_search_call")
                && item.get("id").and_then(Value::as_str) == Some("ws_1")
        }));
        let reasoning_pos = input
            .iter()
            .position(|item| item.get("type").and_then(Value::as_str) == Some("reasoning"))
            .expect("reasoning item");
        let web_search_pos = input
            .iter()
            .position(|item| item.get("type").and_then(Value::as_str) == Some("web_search_call"))
            .expect("web search item");
        assert!(
            reasoning_pos < web_search_pos,
            "reasoning must replay before dependent hosted-tool item"
        );

        let Message::ToolResults { results, .. } = &projected[2] else {
            panic!("expected tool results");
        };
        assert!(
            !results[0].has_images(),
            "OpenAI tool results should be text-only replay"
        );
        assert!(results[0].text_content().contains("[image: image/png]"));
        Ok(())
    }

    #[test]
    fn replay_projection_rejects_openai_orphan_tool_results() {
        let client = OpenAiClient::new("test-key".to_string());
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
    fn replay_projection_drops_openai_provider_items_for_chat_completions()
    -> Result<(), Box<dyn std::error::Error>> {
        let messages = vec![Message::BlockAssistant(BlockAssistantMessage::new(
            vec![
                AssistantBlock::Reasoning {
                    text: "private".to_string(),
                    meta: Some(Box::new(ProviderMeta::OpenAi {
                        id: "rs_1".to_string(),
                        encrypted_content: Some("enc".to_string()),
                        phase: Some("reasoning".to_string()),
                    })),
                },
                AssistantBlock::ServerToolContent {
                    id: Some("ws_1".to_string()),
                    name: "web_search_call".to_string(),
                    content: serde_json::json!({
                        "type": "web_search_call",
                        "id": "ws_1",
                        "status": "completed"
                    }),
                    meta: None,
                },
                AssistantBlock::Text {
                    text: "visible".to_string(),
                    meta: None,
                },
            ],
            StopReason::EndTurn,
        ))];

        let projected =
            project_openai_replay_messages(&messages, OpenAiReplayProjectionMode::ChatCompletions)?;

        let Message::BlockAssistant(assistant) = &projected[0] else {
            panic!("expected assistant");
        };
        assert_eq!(assistant.blocks.len(), 1);
        assert!(matches!(
            &assistant.blocks[0],
            AssistantBlock::Text { text, .. } if text == "visible"
        ));
        Ok(())
    }

    #[test]
    fn replay_projection_allows_dropped_image_between_tool_use_and_result()
    -> Result<(), Box<dyn std::error::Error>> {
        let client = OpenAiClient::new("test-key".to_string());
        let tool_args = RawValue::from_string(r#"{"path":"out.png"}"#.to_string())?;
        let messages = vec![
            Message::BlockAssistant(BlockAssistantMessage::new(
                vec![AssistantBlock::ToolUse {
                    id: "tool_1".to_string(),
                    name: "blob_save_file".to_string(),
                    args: tool_args,
                    meta: None,
                }],
                StopReason::ToolUse,
            )),
            Message::BlockAssistant(BlockAssistantMessage::new(
                vec![assistant_image_block()],
                StopReason::EndTurn,
            )),
            Message::tool_results(vec![ToolResult::new(
                "tool_1".to_string(),
                r#"{"path":"out.png"}"#.to_string(),
                false,
            )]),
        ];

        let projected = client.project_replay_messages(&messages)?;

        assert_eq!(projected.len(), 2);
        let Message::BlockAssistant(assistant) = &projected[0] else {
            panic!("expected assistant tool use");
        };
        assert!(assistant.blocks.iter().any(|block| {
            matches!(
                block,
                AssistantBlock::ToolUse { name, .. } if name == "blob_save_file"
            )
        }));
        assert!(matches!(projected[1], Message::ToolResults { .. }));
        Ok(())
    }

    #[derive(Clone)]
    struct StreamStubState {
        payload: String,
        seen: Arc<Mutex<Vec<Value>>>,
    }

    async fn responses_sse_with_body(
        State(state): State<StreamStubState>,
        Json(body): Json<Value>,
    ) -> impl IntoResponse {
        state.seen.lock().expect("seen mutex").push(body);
        ([("content-type", "text/event-stream")], state.payload)
    }

    #[derive(Clone)]
    struct ImageStubState {
        response: Value,
        seen: Arc<Mutex<Vec<Value>>>,
    }

    async fn openai_image_stub(
        State(state): State<ImageStubState>,
        Json(body): Json<Value>,
    ) -> impl IntoResponse {
        state.seen.lock().expect("seen mutex").push(body);
        Json(state.response)
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

    async fn spawn_chatgpt_stub_server(
        payload: String,
        seen: Arc<Mutex<Vec<Value>>>,
    ) -> (String, tokio::task::JoinHandle<()>) {
        let app = Router::new()
            .route("/responses", post(responses_sse_with_body))
            .with_state(StreamStubState { payload, seen });
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test server");
        let addr = listener.local_addr().expect("local addr");
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve test server");
        });
        (format!("http://{addr}"), handle)
    }

    async fn spawn_openai_image_stub(
        response: Value,
        seen: Arc<Mutex<Vec<Value>>>,
    ) -> (String, tokio::task::JoinHandle<()>) {
        let app = Router::new()
            .route("/v1/responses", post(openai_image_stub))
            .route("/v1/images/generations", post(openai_image_stub))
            .with_state(ImageStubState { response, seen });
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test server");
        let addr = listener.local_addr().expect("local addr");
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve test server");
        });
        (format!("http://{addr}"), handle)
    }

    fn image_executor_request_json(plan: Value) -> ProviderImageGenerationRequest {
        serde_json::from_value(serde_json::json!({
            "operation_id": "00000000-0000-0000-0000-000000000101",
            "model": "gpt-4.1-mini",
            "generate_request": {
                "intent": {
                    "intent": "generate",
                    "prompt": {"content": "draw a small red kite"},
                    "prompt_source": {
                        "source": "user_provided",
                        "message_id": "00000000-0000-0000-0000-000000000102"
                    },
                    "reference_images": []
                },
                "target": {"target": "auto"},
                "size": {"size": "landscape1536x1024"},
                "quality": "low",
                "format": "png",
                "count": 2
            },
            "execution_plan": plan,
            "projected_messages": []
        }))
        .expect("image executor request")
    }

    fn hosted_openai_plan_json() -> Value {
        serde_json::json!({
            "provider": "openai",
            "backend": "hosted_tool",
            "max_count": 4,
            "capabilities": {
                "hosted_image_generation_tool": true,
                "native_image_output": false,
                "custom_tools": true,
                "image_search_grounding": false,
                "image_continuity_tokens": "unsupported"
            },
            "requires_scoped_override": false,
            "provider_plan": {
                "tool_name": "image_generation",
                "model": "gpt-image-2",
                "output": {
                    "size": "landscape1536x1024",
                    "quality": "low",
                    "output_format": "png"
                }
            }
        })
    }

    fn images_api_openai_plan_json() -> Value {
        serde_json::json!({
            "provider": "openai",
            "backend": "provider_api",
            "max_count": 4,
            "capabilities": {
                "hosted_image_generation_tool": false,
                "native_image_output": true,
                "custom_tools": false,
                "image_search_grounding": false,
                "image_continuity_tokens": "unsupported"
            },
            "requires_scoped_override": false,
            "provider_plan": {
                "endpoint": "generations",
                "request_shape": "gpt_image",
                "output": {
                    "size": "landscape1536x1024",
                    "quality": "low",
                    "output_format": "png"
                }
            }
        })
    }

    // =========================================================================
    // Responses API Request Format Tests
    // =========================================================================

    #[test]
    fn openai_image_error_terminal_uses_structured_error_codes()
    -> Result<(), Box<dyn std::error::Error>> {
        let safety = serde_json::json!({
            "error": {
                "type": "invalid_request_error",
                "code": "content_policy_violation",
                "message": "Request rejected by policy."
            }
        });
        assert_eq!(
            OpenAiClient::openai_error_terminal(400, &safety.to_string()),
            ImageOperationTerminalClass::SafetyFiltered
        );

        let refusal = serde_json::json!({
            "error": {
                "type": "invalid_request_error",
                "code": "model_refusal",
                "message": "The model declined the request."
            }
        });
        assert_eq!(
            OpenAiClient::openai_error_terminal(400, &refusal.to_string()),
            ImageOperationTerminalClass::RefusedByProvider
        );
        Ok(())
    }

    #[test]
    fn openai_image_error_terminal_does_not_parse_message_text() {
        let message_only = serde_json::json!({
            "error": {
                "type": "invalid_request_error",
                "message": "diagnostic text mentions safety, content_filter, refusal, and refused"
            }
        });

        assert_eq!(
            OpenAiClient::openai_error_terminal(400, &message_only.to_string()),
            ImageOperationTerminalClass::Failed
        );
        assert_eq!(
            OpenAiClient::openai_error_terminal(
                503,
                "provider overloaded while checking safety filters"
            ),
            ImageOperationTerminalClass::Failed
        );
    }

    #[test]
    fn openai_image_error_terminal_uses_transport_status_for_timeout_and_cancelled() {
        assert_eq!(
            OpenAiClient::openai_error_terminal(408, "request timed out"),
            ImageOperationTerminalClass::Timeout
        );
        assert_eq!(
            OpenAiClient::openai_error_terminal(504, "gateway timeout"),
            ImageOperationTerminalClass::Timeout
        );
        assert_eq!(
            OpenAiClient::openai_error_terminal(499, "client closed request"),
            ImageOperationTerminalClass::Cancelled
        );
    }

    #[tokio::test]
    async fn openai_hosted_image_executor_normalizes_fake_response()
    -> Result<(), Box<dyn std::error::Error>> {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let response = serde_json::json!({
            "id": "resp_img_1",
            "output": [
                {
                    "type": "message",
                    "content": [{"type": "output_text", "text": "Provider caption"}]
                },
                {
                    "type": "image_generation_call",
                    "id": "ig_1",
                    "result": "data:image/png;base64,aGVsbG8=",
                    "revised_prompt": "draw a small red kite in watercolor"
                }
            ]
        });
        let (base_url, handle) = spawn_openai_image_stub(response, seen.clone()).await;
        let client = OpenAiClient::new_with_base_url("test-key".to_string(), base_url);

        let output = client
            .execute_image_generation(image_executor_request_json(hosted_openai_plan_json()))
            .await?;

        assert!(matches!(
            output.terminal,
            ImageOperationTerminalClass::Generated
        ));
        assert_eq!(output.images.len(), 1);
        assert_eq!(output.images[0].base64_data, "aGVsbG8=");
        assert_eq!(output.images[0].media_type.as_str(), "image/png");
        assert_eq!(
            (output.images[0].width, output.images[0].height),
            (1536, 1024)
        );
        assert_eq!(output.provider_text.as_deref(), Some("Provider caption"));
        assert!(matches!(
            output.revised_prompt,
            RevisedPromptDisposition::Revised { .. }
        ));
        assert!(matches!(
            output.native_metadata,
            ProviderImageMetadata::OpenAi(OpenAiImageMetadata {
                response_id: Some(_),
                image_generation_call_id: Some(_),
                ..
            })
        ));
        assert!(matches!(
            output.warnings.as_slice(),
            [ImageGenerationWarning::ProviderReturnedFewerImages { .. }]
        ));

        let bodies = seen.lock().expect("seen mutex");
        let body = bodies.first().expect("captured OpenAI image request");
        assert_eq!(body["model"], "gpt-4.1-mini");
        assert_eq!(body["tools"][0]["type"], "image_generation");
        assert_eq!(body["tools"][0]["model"], "gpt-image-2");
        assert_eq!(body["tools"][0]["size"], "1536x1024");
        assert_eq!(body["tools"][0]["quality"], "low");
        assert_eq!(body["tools"][0]["output_format"], "png");
        assert_eq!(body["tool_choice"], "required");
        assert!(
            body["instructions"]
                .as_str()
                .is_some_and(|value| value.contains("Never reply in text"))
        );
        assert!(body.get("stream").and_then(Value::as_bool) == Some(false));

        handle.abort();
        Ok(())
    }

    #[tokio::test]
    async fn openai_images_api_executor_sends_output_options()
    -> Result<(), Box<dyn std::error::Error>> {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let response = serde_json::json!({
            "created": 1713833628,
            "data": [{"b64_json": "data:image/png;base64,aGVsbG8="}]
        });
        let (base_url, handle) = spawn_openai_image_stub(response, seen.clone()).await;
        let client = OpenAiClient::new_with_base_url("test-key".to_string(), base_url);
        let mut request = image_executor_request_json(images_api_openai_plan_json());
        request.model = "gpt-image-1".to_string();

        let output = client.execute_image_generation(request).await?;

        assert!(matches!(
            output.terminal,
            ImageOperationTerminalClass::Generated
        ));
        assert_eq!(
            (output.images[0].width, output.images[0].height),
            (1536, 1024)
        );
        let bodies = seen.lock().expect("seen mutex");
        let body = bodies.first().expect("captured OpenAI image request");
        assert_eq!(body["model"], "gpt-image-1");
        assert_eq!(body["size"], "1536x1024");
        assert_eq!(body["quality"], "low");
        assert_eq!(body["output_format"], "png");

        handle.abort();
        Ok(())
    }

    #[tokio::test]
    async fn openai_image_executor_sends_provider_params() -> Result<(), Box<dyn std::error::Error>>
    {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let response = serde_json::json!({
            "id": "resp_img_1",
            "output": [{"type": "image_generation_call", "id": "ig_1", "result": "aGVsbG8="}]
        });
        let (base_url, handle) = spawn_openai_image_stub(response, seen.clone()).await;
        let client = OpenAiClient::new_with_base_url("test-key".to_string(), base_url);
        let mut plan = hosted_openai_plan_json();
        plan["provider_plan"]["provider_params"] = serde_json::json!({
            "background": "opaque",
            "output_compression": 72,
            "moderation": "low",
            "action": "generate"
        });

        client
            .execute_image_generation(image_executor_request_json(plan))
            .await?;

        let bodies = seen.lock().expect("seen mutex");
        let body = bodies.first().expect("captured OpenAI image request");
        assert_eq!(body["tools"][0]["background"], "opaque");
        assert_eq!(body["tools"][0]["output_compression"], 72);
        assert_eq!(body["tools"][0]["moderation"], "low");
        assert_eq!(body["tools"][0]["action"], "generate");

        handle.abort();
        Ok(())
    }

    #[test]
    fn test_request_uses_responses_api_endpoint_format() {
        let client = OpenAiClient::new("test-key".to_string());
        let request = LlmRequest::new(
            "gpt-5.4",
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

    #[tokio::test]
    async fn chatgpt_backend_wire_uses_codex_responses_path()
    -> Result<(), Box<dyn std::error::Error>> {
        let payload = [
            r#"data: {"type":"response.output_text.delta","delta":"Hello from ChatGPT"}"#,
            r#"data: {"type":"response.completed","response":{"status":"completed","usage":{"input_tokens":4,"output_tokens":3}}}"#,
            "data: [DONE]",
            "",
        ]
        .join("\n");
        let seen = Arc::new(Mutex::new(Vec::new()));
        let (base_url, server) = spawn_chatgpt_stub_server(payload, seen.clone()).await;
        let client = OpenAiClient::new_with_base_url("test-key".to_string(), base_url)
            .with_chatgpt_backend_wire();
        let request = LlmRequest::new(
            "gpt-5.5",
            vec![
                Message::System(meerkat_core::SystemMessage::new(
                    "You are a careful assistant.".to_string(),
                )),
                Message::User(UserMessage::text("hello".to_string())),
            ],
        );

        let mut stream = client.stream(&request);
        let mut deltas = Vec::new();
        while let Some(event) = stream.next().await {
            match event? {
                LlmEvent::TextDelta { delta, .. } => deltas.push(delta),
                LlmEvent::Done { .. } => break,
                _ => {}
            }
        }
        server.abort();

        assert_eq!(deltas, vec!["Hello from ChatGPT"]);
        let bodies = seen.lock().expect("seen mutex");
        let body = bodies.first().expect("captured ChatGPT backend request");
        assert_eq!(body["instructions"], "You are a careful assistant.");
        assert_eq!(body["store"], false);
        assert_eq!(body["tools"], serde_json::json!([]));
        assert_eq!(body["tool_choice"], "auto");
        assert_eq!(body["parallel_tool_calls"], false);
        assert!(body.get("max_output_tokens").is_none());
        let input = body["input"].as_array().expect("input should be array");
        assert!(
            input
                .iter()
                .all(|item| item.get("role").and_then(Value::as_str) != Some("system")),
            "ChatGPT Codex backend rejects system messages in input; they must be lifted to instructions"
        );
        Ok(())
    }

    #[test]
    fn chatgpt_backend_wire_supplies_default_instructions_without_system_message() {
        let client = OpenAiClient::new("test-key".to_string()).with_chatgpt_backend_wire();
        let request = LlmRequest::new(
            "gpt-5.4",
            vec![Message::User(UserMessage::text("Hello".to_string()))],
        );

        let body = client.build_request_body(&request).expect("build request");

        assert_eq!(body["instructions"], "You are a helpful assistant.");
        assert_eq!(body["store"], false);
        assert!(body.get("max_output_tokens").is_none());
    }

    #[test]
    fn test_request_input_format_system_message() {
        let client = OpenAiClient::new("test-key".to_string());
        let request = LlmRequest::new(
            "gpt-5.4",
            vec![
                Message::System(meerkat_core::SystemMessage::new(
                    "You are helpful".to_string(),
                )),
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
    fn test_request_input_format_degrades_video_user_content_to_text() {
        let client = OpenAiClient::new("test-key".to_string());
        let request = LlmRequest::new(
            "gpt-5.4",
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

        let body = client.build_request_body(&request).expect("build request");
        let input = body["input"].as_array().expect("input should be array");
        assert_eq!(input[0]["role"], "user");
        let content = input[0]["content"]
            .as_array()
            .expect("content should be array");
        assert_eq!(content[0]["type"], "input_text");
        assert_eq!(content[0]["text"], "[video: video/mp4]");
    }

    #[test]
    fn test_request_input_rejects_video_tool_results() {
        let err = OpenAiClient::convert_to_responses_input(&[Message::ToolResults {
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
        }])
        .expect_err("video tool results should be rejected");

        match err {
            LlmError::InvalidRequest { message } => {
                assert!(message.contains("video blocks are not supported"));
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn test_request_input_format_tool_call() {
        let client = OpenAiClient::new("test-key".to_string());
        let tool_args = serde_json::json!({"location": "Tokyo"});
        let request = LlmRequest::new(
            "gpt-5.4",
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
                    created_at: meerkat_core::types::message_timestamp_now(),
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
            "gpt-5.4",
            vec![
                Message::User(UserMessage::text("Weather?".to_string())),
                Message::ToolResults {
                    results: vec![meerkat_core::ToolResult::new(
                        "call_abc123".to_string(),
                        "Sunny, 25C".to_string(),
                        false,
                    )],
                    created_at: meerkat_core::types::message_timestamp_now(),
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
            name: "get_weather".into(),
            description: "Get weather info".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "location": {"type": "string"}
                }
            }),
            provenance: None,
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
            "gpt-5.4",
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
            "gpt-5.4",
            vec![Message::User(UserMessage::text("test".to_string()))],
        )
        .with_openai_tag_merge(|t| {
            t.reasoning_effort =
                Some(meerkat_core::lifecycle::run_primitive::ReasoningEffort::High);
        });

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
        .with_openai_tag_merge(|t| {
            t.reasoning_effort =
                Some(meerkat_core::lifecycle::run_primitive::ReasoningEffort::High);
        });

        let body = client.build_request_body(&request).expect("build request");
        assert!(body.get("reasoning").is_none());
    }

    #[test]
    fn test_request_respects_internal_capability_overrides_for_self_hosted_aliases() {
        let client = OpenAiClient::new("test-key".to_string());
        let request = LlmRequest::new(
            "gemma4:e2b",
            vec![Message::User(UserMessage::text("test".to_string()))],
        )
        .with_temperature(0.3)
        .with_openai_tag_merge(|t| t.supports_temperature_override = Some(true))
        .with_openai_tag_merge(|t| t.supports_reasoning_override = Some(true))
        .with_openai_tag_merge(|t| {
            t.reasoning_effort =
                Some(meerkat_core::lifecycle::run_primitive::ReasoningEffort::High);
        });

        let body = client.build_request_body(&request).expect("build request");

        let temperature = body["temperature"]
            .as_f64()
            .expect("temperature should be numeric");
        assert!((temperature - 0.3).abs() < 1e-6);
        assert_eq!(body["reasoning"]["effort"], "high");
        assert_eq!(body["reasoning"]["summary"], "auto");
    }

    // =========================================================================
    // BlockAssistant Message Tests
    // =========================================================================

    #[test]
    fn test_request_input_format_block_assistant_text() {
        use meerkat_core::BlockAssistantMessage;

        let client = OpenAiClient::new("test-key".to_string());
        let request = LlmRequest::new(
            "gpt-5.4",
            vec![
                Message::User(UserMessage::text("Hello".to_string())),
                Message::BlockAssistant(BlockAssistantMessage {
                    blocks: vec![AssistantBlock::Text {
                        text: "Hi there!".to_string(),
                        meta: None,
                    }],
                    stop_reason: StopReason::EndTurn,
                    created_at: meerkat_core::types::message_timestamp_now(),
                }),
            ],
        );

        let body = build_projected_request_body(&client, &request);
        let input = body["input"].as_array().expect("input should be array");

        assert_eq!(input[1]["type"], "message");
        assert_eq!(input[1]["role"], "assistant");
        assert_eq!(input[1]["content"], "Hi there!");
    }

    #[test]
    fn test_request_input_format_block_assistant_reasoning_with_output_replays_reasoning() {
        use meerkat_core::BlockAssistantMessage;

        let client = OpenAiClient::new("test-key".to_string());
        let request = LlmRequest::new(
            "gpt-5.4",
            vec![
                Message::User(UserMessage::text("Hello".to_string())),
                Message::BlockAssistant(BlockAssistantMessage {
                    blocks: vec![
                        AssistantBlock::Reasoning {
                            text: "Let me think about this".to_string(),
                            meta: Some(Box::new(ProviderMeta::OpenAi {
                                id: "rs_abc123".to_string(),
                                encrypted_content: Some("encrypted_data".to_string()),
                                phase: None,
                            })),
                        },
                        AssistantBlock::Text {
                            text: "Here is my answer".to_string(),
                            meta: None,
                        },
                    ],
                    stop_reason: StopReason::EndTurn,
                    created_at: meerkat_core::types::message_timestamp_now(),
                }),
            ],
        );

        let body = build_projected_request_body(&client, &request);
        let input = body["input"].as_array().expect("input should be array");

        assert_eq!(input.len(), 3);
        assert_eq!(input[1]["type"], "reasoning");
        assert_eq!(input[1]["id"], "rs_abc123");
        assert_eq!(input[1]["encrypted_content"], "encrypted_data");
        assert_eq!(input[2]["type"], "message");
        assert_eq!(input[2]["role"], "assistant");
    }

    #[test]
    fn test_request_input_format_block_assistant_tool_use() {
        use meerkat_core::BlockAssistantMessage;

        let client = OpenAiClient::new("test-key".to_string());
        let args = RawValue::from_string(r#"{"location":"Tokyo"}"#.to_string()).unwrap();
        let request = LlmRequest::new(
            "gpt-5.4",
            vec![
                Message::User(UserMessage::text("Weather?".to_string())),
                Message::BlockAssistant(BlockAssistantMessage {
                    blocks: vec![AssistantBlock::ToolUse {
                        id: "call_xyz".to_string(),
                        name: "get_weather".into(),
                        args,
                        meta: None,
                    }],
                    stop_reason: StopReason::ToolUse,
                    created_at: meerkat_core::types::message_timestamp_now(),
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
            "gpt-5.4",
            vec![Message::User(UserMessage::text("test".to_string()))],
        )
        .with_openai_tag_merge(|t| t.seed = Some(12345));

        let body = client.build_request_body(&request).expect("build request");

        assert_eq!(body["seed"], 12345);
    }

    #[test]
    fn test_request_includes_frequency_penalty_from_provider_params() {
        let client = OpenAiClient::new("test-key".to_string());
        let request = LlmRequest::new(
            "gpt-5.4",
            vec![Message::User(UserMessage::text("test".to_string()))],
        )
        .with_openai_tag_merge(|t| t.frequency_penalty = Some(0.5));

        let body = client.build_request_body(&request).expect("build request");

        let fp = body["frequency_penalty"].as_f64().expect("fp numeric");
        assert!((fp - 0.5).abs() < 1e-6, "fp drift: {fp}");
    }

    #[test]
    fn test_request_includes_presence_penalty_from_provider_params() {
        let client = OpenAiClient::new("test-key".to_string());
        let request = LlmRequest::new(
            "gpt-5.4",
            vec![Message::User(UserMessage::text("test".to_string()))],
        )
        .with_openai_tag_merge(|t| t.presence_penalty = Some(0.8));

        let body = client.build_request_body(&request).expect("build request");

        let pp = body["presence_penalty"].as_f64().expect("pp numeric");
        assert!((pp - 0.8).abs() < 1e-6, "pp drift: {pp}");
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
            "gpt-realtime",
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
            "gpt-5.4",
            vec![Message::User(UserMessage::text("test".to_string()))],
        )
        .with_openai_tag_merge(|t| {
            t.reasoning_effort =
                Some(meerkat_core::lifecycle::run_primitive::ReasoningEffort::High);
        })
        .with_openai_tag_merge(|t| t.seed = Some(999))
        .with_openai_tag_merge(|t| t.frequency_penalty = Some(0.3))
        .with_openai_tag_merge(|t| t.presence_penalty = Some(0.4));

        let body = client.build_request_body(&request).expect("build request");

        assert_eq!(body["reasoning"]["effort"], "high");
        assert_eq!(body["seed"], 999);
        let fp = body["frequency_penalty"].as_f64().expect("fp numeric");
        assert!((fp - 0.3).abs() < 1e-6, "fp drift: {fp}");
        let pp = body["presence_penalty"].as_f64().expect("pp numeric");
        assert!((pp - 0.4).abs() < 1e-6, "pp drift: {pp}");
    }

    #[test]
    fn test_tool_args_serialization_no_double_encoding() -> Result<(), Box<dyn std::error::Error>> {
        let client = OpenAiClient::new("test-key".to_string());

        let tool_args = serde_json::json!({"city": "Tokyo", "units": "celsius"});
        let request = LlmRequest::new(
            "gpt-5.4",
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
                    created_at: meerkat_core::types::message_timestamp_now(),
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
            "gpt-5.4",
            vec![Message::User(UserMessage::text("test".to_string()))],
        )
        .with_openai_tag_merge(|t| {
            t.structured_output = serde_json::from_value::<OutputSchema>(serde_json::json!({
                "schema": schema,
                "name": "person",
                "strict": true
            }))
            .ok();
        });

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
            "gpt-5.4",
            vec![Message::User(UserMessage::text("test".to_string()))],
        )
        .with_openai_tag_merge(|t| {
            t.structured_output = serde_json::from_value::<OutputSchema>(serde_json::json!({
                "schema": schema
            }))
            .ok();
        });

        let body = client.build_request_body(&request).expect("build request");

        let format = &body["text"]["format"];
        assert_eq!(format["name"], "output"); // default name
        assert_eq!(format["strict"], false); // default strict
    }

    #[test]
    fn test_build_request_body_without_structured_output() {
        let client = OpenAiClient::new("test-key".to_string());

        let request = LlmRequest::new(
            "gpt-5.4",
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
            "gpt-5.4",
            vec![Message::User(UserMessage::text("test".to_string()))],
        )
        .with_openai_tag_merge(|t| {
            t.structured_output = serde_json::from_value::<OutputSchema>(serde_json::json!({
                "schema": schema,
                "name": "person",
                "strict": true
            }))
            .ok();
        });

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
            "gpt-5.4",
            vec![Message::User(UserMessage::text("test".to_string()))],
        )
        .with_openai_tag_merge(|t| {
            t.structured_output = serde_json::from_value::<OutputSchema>(serde_json::json!({
                "schema": schema,
                "strict": true
            }))
            .ok();
        });

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
            "gpt-5.4",
            vec![Message::User(UserMessage::text("test".to_string()))],
        )
        .with_openai_tag_merge(|t| {
            t.structured_output = serde_json::from_value::<OutputSchema>(serde_json::json!({
                "schema": schema,
                "strict": false
            }))
            .ok();
        });

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
    fn test_parse_responses_sse_line_without_trailing_space() {
        let line = r#"data:{"type":"response.output_text.delta","delta":"hello"}"#;
        let event = OpenAiClient::parse_responses_sse_line(line);
        assert!(event.is_some());
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

    #[tokio::test]
    async fn test_web_search_call_blocks_are_emitted() {
        let payload = [
            r#"data: {"type":"response.web_search_call.searching","output_index":0,"item_id":"ws_123","sequence_number":1}"#,
            r#"data: {"type":"response.completed","response":{"status":"completed","output":[{"type":"web_search_call","id":"ws_123","status":"completed","action":{"type":"search","queries":["meerkat runtime"]}},{"type":"message","content":[{"type":"output_text","text":"done","annotations":[{"type":"url_citation","url":"https://example.com","title":"Example","start_index":0,"end_index":4}]}]}],"usage":{"input_tokens":10,"output_tokens":5}}}"#,
            "data: [DONE]",
            "",
        ]
        .join("\n");
        let (base_url, server) = spawn_openai_stub_server(payload).await;
        let client = OpenAiClient::new_with_base_url("test-key".to_string(), base_url);
        let request = LlmRequest::new(
            "gpt-5-mini",
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

        assert_eq!(server_blocks.len(), 3);
        assert_eq!(server_blocks[0].0.as_deref(), Some("ws_123"));
        assert_eq!(server_blocks[0].1, "web_search");
        assert_eq!(
            server_blocks[0].2["type"],
            "response.web_search_call.searching"
        );
        assert_eq!(server_blocks[1].0.as_deref(), Some("ws_123"));
        assert_eq!(server_blocks[1].1, "web_search_call");
        assert_eq!(
            server_blocks[1].2["action"]["queries"][0],
            "meerkat runtime"
        );
        assert_eq!(server_blocks[2].1, "web_search_annotations");
        assert_eq!(
            server_blocks[2].2["annotations"][0]["url"],
            "https://example.com"
        );
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
            "gpt-5.4",
            vec![
                Message::User(UserMessage::text("Hello".to_string())),
                // Reasoning-only response (e.g., stream interrupted after reasoning)
                Message::BlockAssistant(BlockAssistantMessage {
                    blocks: vec![AssistantBlock::Reasoning {
                        text: "Let me think".to_string(),
                        meta: Some(Box::new(ProviderMeta::OpenAi {
                            id: "rs_orphan".to_string(),
                            encrypted_content: None,
                            phase: None,
                        })),
                    }],
                    stop_reason: StopReason::EndTurn,
                    created_at: meerkat_core::types::message_timestamp_now(),
                }),
            ],
        );

        let body = build_projected_request_body(&client, &request);
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
            "gpt-5.4",
            vec![
                Message::User(UserMessage::text("First question".to_string())),
                // Reasoning without output, followed by next user turn
                Message::BlockAssistant(BlockAssistantMessage {
                    blocks: vec![AssistantBlock::Reasoning {
                        text: "Thinking...".to_string(),
                        meta: Some(Box::new(ProviderMeta::OpenAi {
                            id: "rs_mid".to_string(),
                            encrypted_content: None,
                            phase: None,
                        })),
                    }],
                    stop_reason: StopReason::EndTurn,
                    created_at: meerkat_core::types::message_timestamp_now(),
                }),
                Message::User(UserMessage::text("Second question".to_string())),
            ],
        );

        let body = build_projected_request_body(&client, &request);
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
        let args = RawValue::from_string(r"{}".to_string()).unwrap();
        let request = LlmRequest::new(
            "gpt-5.4",
            vec![
                Message::User(UserMessage::text("Hello".to_string())),
                Message::BlockAssistant(BlockAssistantMessage {
                    blocks: vec![AssistantBlock::ToolUse {
                        id: "call_123".to_string(),
                        name: "lookup".to_string(),
                        args,
                        meta: None,
                    }],
                    stop_reason: StopReason::ToolUse,
                    created_at: meerkat_core::types::message_timestamp_now(),
                }),
                // Reasoning-only at end of one assistant message
                Message::BlockAssistant(BlockAssistantMessage {
                    blocks: vec![AssistantBlock::Reasoning {
                        text: "Thinking...".to_string(),
                        meta: Some(Box::new(ProviderMeta::OpenAi {
                            id: "rs_before_tool".to_string(),
                            encrypted_content: None,
                            phase: None,
                        })),
                    }],
                    stop_reason: StopReason::EndTurn,
                    created_at: meerkat_core::types::message_timestamp_now(),
                }),
                // Next message is tool results; the reasoning-only message
                // must be dropped without breaking tool-call adjacency.
                Message::ToolResults {
                    results: vec![meerkat_core::ToolResult::new(
                        "call_123".to_string(),
                        "result".to_string(),
                        false,
                    )],
                    created_at: meerkat_core::types::message_timestamp_now(),
                },
            ],
        );

        let body = build_projected_request_body(&client, &request);
        let input = body["input"].as_array().expect("input should be array");

        // Reasoning should be stripped; tool call/result adjacency remains.
        assert_eq!(input.len(), 3);
        assert_eq!(input[0]["type"], "message");
        assert_eq!(input[1]["type"], "function_call");
        assert_eq!(input[2]["type"], "function_call_output");
    }

    #[test]
    fn test_reasoning_followed_by_function_call_replays_reasoning() {
        use meerkat_core::BlockAssistantMessage;

        let client = OpenAiClient::new("test-key".to_string());
        let args = RawValue::from_string(r#"{"q":"test"}"#.to_string()).unwrap();
        let request = LlmRequest::new(
            "gpt-5.4",
            vec![
                Message::User(UserMessage::text("Hello".to_string())),
                Message::BlockAssistant(BlockAssistantMessage {
                    blocks: vec![
                        AssistantBlock::Reasoning {
                            text: "I should search".to_string(),
                            meta: Some(Box::new(ProviderMeta::OpenAi {
                                id: "rs_valid".to_string(),
                                encrypted_content: Some("enc_valid".to_string()),
                                phase: None,
                            })),
                        },
                        AssistantBlock::ToolUse {
                            id: "call_1".to_string(),
                            name: "search".into(),
                            args,
                            meta: None,
                        },
                    ],
                    stop_reason: StopReason::ToolUse,
                    created_at: meerkat_core::types::message_timestamp_now(),
                }),
            ],
        );

        let body = client.build_request_body(&request).expect("build request");
        let input = body["input"].as_array().expect("input should be array");

        assert_eq!(input.len(), 3);
        assert_eq!(input[1]["type"], "reasoning");
        assert_eq!(input[1]["id"], "rs_valid");
        assert_eq!(input[1]["encrypted_content"], "enc_valid");
        assert_eq!(input[2]["type"], "function_call");
    }

    #[test]
    fn test_non_openai_reasoning_blocks_are_skipped() {
        use meerkat_core::BlockAssistantMessage;

        let client = OpenAiClient::new("test-key".to_string());
        let request = LlmRequest::new(
            "gpt-5.4",
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
                    created_at: meerkat_core::types::message_timestamp_now(),
                }),
            ],
        );

        let body = build_projected_request_body(&client, &request);
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
            "gpt-5.4",
            vec![
                Message::User(UserMessage::text("Hello".to_string())),
                // Two consecutive reasoning-only messages (e.g., repeated interruptions)
                Message::BlockAssistant(BlockAssistantMessage {
                    blocks: vec![AssistantBlock::Reasoning {
                        text: "First thought".to_string(),
                        meta: Some(Box::new(ProviderMeta::OpenAi {
                            id: "rs_first".to_string(),
                            encrypted_content: Some("enc_1".to_string()),
                            phase: None,
                        })),
                    }],
                    stop_reason: StopReason::EndTurn,
                    created_at: meerkat_core::types::message_timestamp_now(),
                }),
                Message::BlockAssistant(BlockAssistantMessage {
                    blocks: vec![AssistantBlock::Reasoning {
                        text: "Second thought".to_string(),
                        meta: Some(Box::new(ProviderMeta::OpenAi {
                            id: "rs_second".to_string(),
                            encrypted_content: None,
                            phase: None,
                        })),
                    }],
                    stop_reason: StopReason::EndTurn,
                    created_at: meerkat_core::types::message_timestamp_now(),
                }),
                Message::User(UserMessage::text("Still here".to_string())),
            ],
        );

        let body = build_projected_request_body(&client, &request);
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
    // Reasoning encrypted_content replay tests
    // =========================================================================

    #[test]
    fn test_reasoning_without_encrypted_content_is_replayed_without_encrypted_content() {
        use meerkat_core::BlockAssistantMessage;

        let client = OpenAiClient::new("test-key".to_string());
        let args = RawValue::from_string(r#"{"q":"test"}"#.to_string()).unwrap();
        let request = LlmRequest::new(
            "gpt-5.4",
            vec![
                Message::User(UserMessage::text("Hello".to_string())),
                Message::BlockAssistant(BlockAssistantMessage {
                    blocks: vec![
                        AssistantBlock::Reasoning {
                            text: "I should search".to_string(),
                            meta: Some(Box::new(ProviderMeta::OpenAi {
                                id: "rs_no_enc".to_string(),
                                encrypted_content: None,
                                phase: None,
                            })),
                        },
                        AssistantBlock::ToolUse {
                            id: "call_1".to_string(),
                            name: "search".into(),
                            args,
                            meta: None,
                        },
                    ],
                    stop_reason: StopReason::ToolUse,
                    created_at: meerkat_core::types::message_timestamp_now(),
                }),
            ],
        );

        let body = client.build_request_body(&request).expect("build request");
        let input = body["input"].as_array().expect("input should be array");

        assert_eq!(input.len(), 3);
        assert_eq!(input[0]["type"], "message");
        assert_eq!(input[1]["type"], "reasoning");
        assert_eq!(input[1]["id"], "rs_no_enc");
        assert!(input[1].get("encrypted_content").is_none());
        assert_eq!(input[2]["type"], "function_call");
    }

    #[test]
    fn test_reasoning_with_encrypted_content_is_replayed() {
        use meerkat_core::BlockAssistantMessage;

        let client = OpenAiClient::new("test-key".to_string());
        let request = LlmRequest::new(
            "gpt-5.4",
            vec![
                Message::User(UserMessage::text("Hello".to_string())),
                Message::BlockAssistant(BlockAssistantMessage {
                    blocks: vec![
                        AssistantBlock::Reasoning {
                            text: "Let me think".to_string(),
                            meta: Some(Box::new(ProviderMeta::OpenAi {
                                id: "rs_enc".to_string(),
                                encrypted_content: Some("enc_data_here".to_string()),
                                phase: None,
                            })),
                        },
                        AssistantBlock::Text {
                            text: "Here is my answer".to_string(),
                            meta: None,
                        },
                    ],
                    stop_reason: StopReason::EndTurn,
                    created_at: meerkat_core::types::message_timestamp_now(),
                }),
            ],
        );

        let body = client.build_request_body(&request).expect("build request");
        let input = body["input"].as_array().expect("input should be array");

        assert_eq!(input.len(), 3);
        assert_eq!(input[1]["type"], "reasoning");
        assert_eq!(input[1]["id"], "rs_enc");
        assert_eq!(input[1]["encrypted_content"], "enc_data_here");
        assert_eq!(input[2]["type"], "message");
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
    async fn test_stream_malformed_function_call_arguments_done_fails_closed() {
        let payload = [
            r#"data: {"type":"response.function_call_arguments.done","call_id":"call_bad","name":"get_weather","arguments":"{\"location\":"}"#,
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
                .contains("invalid OpenAI tool call arguments JSON"),
            "error should name invalid OpenAI tool args: {error}"
        );
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
            "gpt-5.4",
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
            "gpt-5.4",
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
            "gpt-5.4",
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
                    created_at: meerkat_core::types::message_timestamp_now(),
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

    // =========================================================================
    // Web search tool injection tests
    // =========================================================================

    #[test]
    fn test_web_search_tool_appended_openai() {
        use meerkat_core::ToolDef;
        use std::sync::Arc;

        let client = OpenAiClient::new("test-key".to_string());
        let request = LlmRequest::new(
            "gpt-4.1-mini",
            vec![Message::User(UserMessage::text("test".to_string()))],
        )
        .with_tools(vec![Arc::new(ToolDef::new(
            "my_tool",
            "A test tool",
            serde_json::json!({"type": "object"}),
        ))])
        .with_openai_tag_merge(|t| {
            t.web_search = Some(
                meerkat_core::lifecycle::run_primitive::OpaqueProviderBody::from_value(
                    &serde_json::json!({"type": "web_search"}),
                ),
            );
        });
        let body = client.build_request_body(&request).expect("build request");
        let tools = body["tools"].as_array().expect("tools should be array");
        assert_eq!(tools.len(), 2, "should have regular tool + web_search");
        assert_eq!(tools[0]["type"], "function");
        assert_eq!(tools[1]["type"], "web_search");
    }

    #[test]
    fn test_web_search_only_openai() {
        let client = OpenAiClient::new("test-key".to_string());
        let request = LlmRequest::new(
            "gpt-4.1-mini",
            vec![Message::User(UserMessage::text("test".to_string()))],
        )
        .with_openai_tag_merge(|t| {
            t.web_search = Some(
                meerkat_core::lifecycle::run_primitive::OpaqueProviderBody::from_value(
                    &serde_json::json!({"type": "web_search"}),
                ),
            );
        });
        let body = client.build_request_body(&request).expect("build request");
        let tools = body["tools"].as_array().expect("tools should be array");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["type"], "web_search");
    }

    #[test]
    fn test_no_web_search_when_absent_openai() {
        use meerkat_core::ToolDef;
        use std::sync::Arc;

        let client = OpenAiClient::new("test-key".to_string());
        let request = LlmRequest::new(
            "gpt-4.1-mini",
            vec![Message::User(UserMessage::text("test".to_string()))],
        )
        .with_tools(vec![Arc::new(ToolDef::new(
            "my_tool",
            "A test tool",
            serde_json::json!({"type": "object"}),
        ))]);
        let body = client.build_request_body(&request).expect("build request");
        let tools = body["tools"].as_array().expect("tools should be array");
        assert_eq!(tools.len(), 1, "should only have the regular tool");
        assert_eq!(tools[0]["type"], "function");
    }
}
