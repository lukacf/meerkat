//! OpenAI Realtime **text-turn** adapter.
//!
//! Implements [`LlmClient`] for realtime-capable OpenAI models (currently
//! `gpt-realtime-2`). The Responses API endpoint `/v1/responses` rejects
//! realtime model IDs with `model_not_found`, so any session whose resolved
//! model advertises `ModelCapabilities.realtime == true` must reach the
//! model through the Realtime WebSocket instead.
//!
//! Strategy (per-turn, stateless replay):
//! 1. Open a WebSocket to `wss://api.openai.com/v1/realtime?model=<model>`
//!    via `oai-rt-rs` (GA protocol; no `OpenAI-Beta` header).
//! 2. `session.update` → `type: "realtime"`, `output_modalities: Text`,
//!    and tool definitions.
//! 3. Replay the full message history as `conversation.item.create`
//!    events (including System messages in their exact authored positions).
//! 4. `response.create` with `output_modalities: Text` triggers inference.
//! 5. Translate `response.output_text.delta`,
//!    `response.function_call_arguments.delta`/`…done`, and `response.done`
//!    into [`LlmEvent::TextDelta`], [`LlmEvent::ToolCallDelta`] /
//!    [`LlmEvent::ToolCallComplete`], and [`LlmEvent::Done`].
//!
//! The per-turn WS is closed on `response.done`. Sharing one WebSocket
//! across turns of the same session is a future optimization; correctness
//! and the capability-gated routing land first.

use async_stream::try_stream;
use async_trait::async_trait;
use meerkat_core::schema::{CompiledSchema, SchemaError};
use meerkat_core::{
    AssistantBlock, ContentBlock, ImageData, Message, OutputSchema, StopReason, Usage,
};
use meerkat_llm_core::{LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest, LlmStream};

use oai_rt_rs::protocol::models::{
    ContentPart, ConversationMode, Item, MaxTokens, OutputModalities, ResponseConfig, Role,
    SessionUpdate, SessionUpdateConfig, Temperature, Tool, Usage as OaiUsage,
};
use oai_rt_rs::{ClientEvent, RealtimeClient, ServerEvent};

/// OpenAI's Realtime API caps `response.max_output_tokens` at 4096 for
/// integer values; larger values fail with `integer above maximum value`.
/// Callers may request a higher per-turn cap because the same session-level
/// `max_tokens_per_turn` flows through the Responses-API path (which allows
/// 128K). Clamp at the realtime boundary so the API ceiling is honored
/// regardless of caller intent.
const REALTIME_MAX_OUTPUT_TOKENS: u32 = 4096;

/// Translate an agent-level `max_tokens` budget into the
/// `response.max_output_tokens` the Realtime API accepts, clamping to the
/// protocol ceiling.
///
/// #230: an explicit `max_tokens == 0` is a typed [`LlmError::InvalidRequest`]
/// reject rather than a silent downgrade to the provider default. A zero cap
/// is already invalid caller policy upstream — `agent.max_tokens_per_turn == 0`
/// fails config validation — so the realtime boundary fails closed on an
/// explicit zero instead of fail-open reverting to the provider default. A
/// finite positive budget clamps to the protocol ceiling.
///
/// (The remaining caller-Unset distinction — a request that genuinely did not
/// set a finite budget — would need `LlmRequest.max_tokens: Option<u32>` in
/// `meerkat-llm-core`; until that lands every request carries a positive
/// budget from the validated config path.)
fn realtime_max_output_tokens(max_tokens: u32) -> Result<MaxTokens, LlmError> {
    if max_tokens == 0 {
        return Err(LlmError::InvalidRequest {
            message: "realtime max_output_tokens must be greater than 0".to_string(),
        });
    }
    Ok(MaxTokens::Count(max_tokens.min(REALTIME_MAX_OUTPUT_TOKENS)))
}

/// Resolve the optional caller temperature into a realtime `Temperature`,
/// distinguishing caller-Unset from an explicit-but-invalid value.
///
/// `None` is the caller-Unset distinction: the provider default applies. An
/// explicit `Some(t)` outside the realtime protocol's accepted `[0.0, 2.0]`
/// range is a typed [`LlmError::InvalidRequest`] reject rather than a silent
/// downgrade to the provider default, so an out-of-range caller policy never
/// fails open.
fn resolve_realtime_temperature(temperature: Option<f32>) -> Result<Option<Temperature>, LlmError> {
    match temperature {
        None => Ok(None),
        Some(value) => {
            Temperature::new(value)
                .map(Some)
                .map_err(|error| LlmError::InvalidRequest {
                    message: format!("invalid realtime temperature: {error}"),
                })
        }
    }
}

/// LlmClient implementation that serves text turns via OpenAI Realtime WS.
#[derive(Debug, Clone)]
pub struct OpenAiRealtimeTextAdapter {
    api_key: String,
}

fn project_realtime_replay_messages(messages: &[Message]) -> Result<Vec<Message>, LlmError> {
    crate::client::project_openai_replay_messages_for_target(
        messages,
        crate::client::OpenAiReplayProjectionMode::ChatCompletions,
        false,
        false,
    )
}

impl OpenAiRealtimeTextAdapter {
    /// Create a new adapter bound to the given API key. Callers should
    /// acquire the key through
    /// `meerkat::resolve_provider_api_key(&config, Provider::OpenAI)` so
    /// env reads flow through the canonical `ProviderRuntimeRegistry`.
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
        }
    }
}

#[async_trait]
impl LlmClient for OpenAiRealtimeTextAdapter {
    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        project_realtime_replay_messages(messages)
    }

    fn stream<'a>(&'a self, request: &'a LlmRequest) -> LlmStream<'a> {
        let api_key = self.api_key.clone();
        Box::pin(try_stream! {
            let mut projected_request = request.clone();
            projected_request.messages = self.project_replay_messages(&request.messages)?;
            let request = &projected_request;
            let history_items = convert_messages(&request.messages)?;
            let tools = build_tools(request);

            // Connect WS — GA protocol (no OpenAI-Beta header).
            let mut client = RealtimeClient::connect(
                &api_key,
                Some(&request.model),
                None,
            )
            .await
            .map_err(map_oai_error)?;

            // session.update → text-only output and declared tools. Canonical
            // System messages are conversation items, never session config.
            let session_update = SessionUpdate {
                config: SessionUpdateConfig {
                    output_modalities: Some(OutputModalities::Text),
                    instructions: None,
                    tools: Some(tools.clone()),
                    ..SessionUpdateConfig::default()
                },
            };
            client
                .send(ClientEvent::SessionUpdate {
                    event_id: None,
                    session: Box::new(session_update),
                })
                .await
                .map_err(map_oai_error)?;

            // Replay prior turns as conversation.item.create events.
            for item in history_items {
                client
                    .send(ClientEvent::ConversationItemCreate {
                        event_id: None,
                        previous_item_id: None,
                        item: Box::new(item),
                    })
                    .await
                    .map_err(map_oai_error)?;
            }

            // Kick off inference. Stateless conversation mode to avoid
            // surprise coupling to any prior server-side conversation
            // state — we replayed the full history above.
            //
            // Propagate the session-level LlmRequest tunables so
            // realtime turns honor the same max_output_tokens / temperature
            // as the Responses-API path. `temperature == None` is the
            // caller-Unset distinction (provider default applies); an
            // explicit-but-invalid temperature is a typed reject rather than
            // a silent downgrade to the provider default. #230: likewise an
            // explicit zero output-token cap is a typed reject, not a silent
            // fall-through to the provider default.
            let max_output_tokens = realtime_max_output_tokens(request.max_tokens)?;
            let temperature = resolve_realtime_temperature(request.temperature)?;
            let response_config = ResponseConfig {
                conversation: Some(ConversationMode::None),
                output_modalities: Some(OutputModalities::Text),
                instructions: None,
                tools: Some(tools.clone()),
                max_output_tokens: Some(max_output_tokens),
                temperature,
                ..ResponseConfig::default()
            };
            client
                .send(ClientEvent::ResponseCreate {
                    event_id: None,
                    response: Some(Box::new(response_config)),
                })
                .await
                .map_err(map_oai_error)?;

            // Pump server events until response.done or error.
            let mut stop_reason = StopReason::EndTurn;
            let mut last_usage: Option<Usage> = None;

            loop {
                let event = match client.next_event().await {
                    Ok(Some(event)) => event,
                    Ok(None) => {
                        Err(LlmError::ConnectionReset)?;
                        unreachable!()
                    }
                    Err(err) => {
                        Err(map_oai_error(err))?;
                        unreachable!()
                    }
                };

                match event {
                    ServerEvent::ResponseOutputTextDelta { delta, .. } => {
                        yield LlmEvent::TextDelta {
                            delta,
                            meta: None,
                        };
                    }
                    ServerEvent::ResponseFunctionCallArgumentsDelta {
                        call_id, delta, ..
                    } => {
                        yield LlmEvent::ToolCallDelta {
                            id: call_id,
                            name: None,
                            args_delta: delta,
                        };
                    }
                    ServerEvent::ResponseFunctionCallArgumentsDone {
                        call_id,
                        name,
                        arguments,
                        ..
                    } => {
                        // Fail the tool-call boundary on malformed provider
                        // JSON instead of laundering it into a Value::String
                        // blob and yielding a synthetic successful ToolUse turn.
                        let args = crate::live::parse_tool_call_args(&arguments, &call_id)?;
                        stop_reason = StopReason::ToolUse;
                        yield LlmEvent::ToolCallComplete {
                            id: call_id,
                            name,
                            args,
                            meta: None,
                        };
                    }
                    ServerEvent::ResponseDone { response, .. } => {
                        if let Some(usage) = response.usage.as_ref() {
                            last_usage = Some(map_usage(usage, &request.model));
                        }
                        if let Some(usage) = last_usage.clone() {
                            yield LlmEvent::UsageUpdate {
                                usage: meerkat_core::TurnUsage::try_from_usage(usage)
                                    .map_err(|error| LlmError::Unknown {
                                        message: error.to_string(),
                                    })?,
                            };
                        }
                        yield LlmEvent::Done {
                            outcome: LlmDoneOutcome::Success { stop_reason },
                        };
                        break;
                    }
                    ServerEvent::Error { error, .. } => {
                        Err(map_server_error(error))?;
                    }
                    _ => {
                        // Other event types (session.*, conversation.item.*,
                        // rate_limits.updated, etc.) are informational for the
                        // text-turn path. Ignore.
                    }
                }
            }
        })
    }

    fn provider(&self) -> meerkat_core::Provider {
        // Share the OpenAI provider identity so factory-level provider
        // inference / logging treats realtime sessions uniformly.
        meerkat_core::Provider::OpenAI
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        // The real healthcheck is per-WebSocket; opening a probe socket
        // on every call would be expensive. Surface a cheap "configured"
        // signal instead.
        if self.api_key.trim().is_empty() {
            return Err(LlmError::AuthenticationFailed {
                message: "OpenAiRealtimeTextAdapter has no API key".to_string(),
            });
        }
        Ok(())
    }

    fn compile_schema(&self, output_schema: &OutputSchema) -> Result<CompiledSchema, SchemaError> {
        Ok(CompiledSchema {
            schema: output_schema.schema.as_value().clone(),
            warnings: Vec::new(),
        })
    }
}

// ---- message / tool / usage conversion helpers ---------------------------

/// Convert canonical history into realtime conversation items without
/// changing role or order.
fn convert_messages(messages: &[Message]) -> Result<Vec<Item>, LlmError> {
    let mut items: Vec<Item> = Vec::new();

    for msg in messages {
        match msg {
            Message::System(s) => {
                items.push(Item::Message {
                    id: None,
                    status: None,
                    phase: None,
                    role: Role::System,
                    content: vec![ContentPart::InputText {
                        text: s.content.clone(),
                    }],
                });
            }
            Message::SystemNotice(notice) => {
                items.push(Item::Message {
                    id: None,
                    status: None,
                    phase: None,
                    role: Role::User,
                    content: vec![ContentPart::InputText {
                        text: notice.model_projection_text(),
                    }],
                });
            }
            Message::User(u) => {
                let text = u.text_content();
                if meerkat_core::has_non_text_content(&u.content) {
                    let parts = u
                        .content
                        .iter()
                        .map(|block| match block {
                            ContentBlock::Text { text } => {
                                ContentPart::InputText { text: text.clone() }
                            }
                            ContentBlock::Image {
                                media_type, data, ..
                            } => match data {
                                ImageData::Inline { data } => ContentPart::InputText {
                                    text: format!("[image {media_type} data:{} bytes]", data.len()),
                                },
                                ImageData::Blob { .. } => ContentPart::InputText {
                                    text: block.text_projection().into_owned(),
                                },
                            },
                            _ => ContentPart::InputText {
                                text: block.text_projection().into_owned(),
                            },
                        })
                        .collect::<Vec<_>>();
                    if !parts.is_empty() {
                        items.push(Item::Message {
                            id: None,
                            status: None,
                            phase: None,
                            role: Role::User,
                            content: parts,
                        });
                    }
                } else if !text.is_empty() {
                    items.push(Item::Message {
                        id: None,
                        status: None,
                        phase: None,
                        role: Role::User,
                        content: vec![ContentPart::InputText { text }],
                    });
                }
            }
            Message::BlockAssistant(a) => {
                for block in &a.blocks {
                    match block {
                        AssistantBlock::Text { text, .. } => {
                            if !text.is_empty() {
                                items.push(Item::Message {
                                    id: None,
                                    status: None,
                                    phase: None,
                                    role: Role::Assistant,
                                    content: vec![ContentPart::OutputText { text: text.clone() }],
                                });
                            }
                        }
                        AssistantBlock::ToolUse { id, name, args, .. } => {
                            items.push(Item::FunctionCall {
                                id: None,
                                status: None,
                                phase: None,
                                name: name.clone(),
                                call_id: id.clone(),
                                arguments: args.get().to_string(),
                            });
                        }
                        _ => {
                            // Reasoning / other blocks aren't replayed to
                            // Realtime; it has no typed slot for them and
                            // OpenAI enforces adjacency constraints anyway.
                        }
                    }
                }
            }
            Message::ToolResults { results, .. } => {
                for r in results {
                    if r.has_video() {
                        return Err(LlmError::InvalidRequest {
                            message:
                                "video blocks are not supported in OpenAI realtime tool results"
                                    .to_string(),
                        });
                    }
                    items.push(Item::FunctionCallOutput {
                        id: None,
                        phase: None,
                        call_id: r.tool_use_id.clone(),
                        output: r.text_content(),
                    });
                }
            }
        }
    }

    Ok(items)
}

fn build_tools(request: &LlmRequest) -> Vec<Tool> {
    request
        .tools
        .iter()
        .map(|tool| Tool::Function {
            name: tool.name.clone().into(),
            description: (!tool.description.trim().is_empty()).then(|| tool.description.clone()),
            parameters: tool.input_schema.clone(),
        })
        .collect()
}

fn map_usage(u: &OaiUsage, model: &str) -> Usage {
    let cache_read_tokens = u
        .input_token_details
        .as_ref()
        .and_then(|d| d.cached_tokens)
        .or(u.cached_tokens)
        .map(u64::from);
    Usage {
        input_tokens: u64::from(u.input_tokens),
        output_tokens: u64::from(u.output_tokens),
        cache_creation_tokens: None,
        cache_read_tokens,
        provider_accounting: Some(meerkat_core::ProviderTokenAccounting::openai(
            model,
            u64::from(u.input_tokens),
        )),
    }
}

fn map_oai_error(err: oai_rt_rs::Error) -> LlmError {
    use oai_rt_rs::Error;
    match err {
        Error::Http(e) => map_http_error(e),
        Error::Serialization(e) => LlmError::Unknown {
            message: format!("openai realtime json: {e}"),
        },
        Error::InvalidClientEvent(msg) => LlmError::InvalidRequest { message: msg },
        Error::Url(e) => LlmError::Unknown {
            message: format!("openai realtime url: {e}"),
        },
        Error::WebSocket(_) | Error::ConnectionClosed => LlmError::ConnectionReset,
        Error::Header(e) => LlmError::Unknown {
            message: format!("openai realtime header: {e}"),
        },
        Error::Api(server_error) => map_server_error(server_error),
        other => LlmError::Unknown {
            message: format!("openai realtime error: {other}"),
        },
    }
}

fn map_http_error(error: reqwest::Error) -> LlmError {
    let message = error.to_string();
    if let Some(status) = error.status() {
        return map_http_status(status, message);
    }
    if error.is_timeout() {
        return LlmError::NetworkTimeout { duration_ms: 30000 };
    }
    LlmError::Unknown { message }
}

fn map_http_status(status: reqwest::StatusCode, message: String) -> LlmError {
    match status.as_u16() {
        401 | 403 => LlmError::AuthenticationFailed { message },
        408 | 504 => LlmError::NetworkTimeout { duration_ms: 30000 },
        404 => LlmError::ModelNotFound { model: message },
        429 => LlmError::RateLimited {
            retry_after_ms: None,
        },
        503 => LlmError::ServerOverloaded,
        s if s >= 500 => LlmError::ServerError { status: s, message },
        s if s >= 400 => LlmError::InvalidRequest { message },
        _ => LlmError::Unknown { message },
    }
}

fn map_server_error(err: oai_rt_rs::error::ServerError) -> LlmError {
    use oai_rt_rs::error::ApiErrorType;
    let message = err.message.clone();
    let code = err.code.as_deref().unwrap_or_default();
    match err.error_type {
        ApiErrorType::InvalidRequestError => {
            if code == "model_not_found" {
                LlmError::ModelNotFound { model: message }
            } else if matches!(code, "context_length_exceeded" | "context_window_exceeded") {
                LlmError::ContextLengthExceeded {
                    max: 0,
                    requested: 1,
                }
            } else {
                LlmError::InvalidRequest { message }
            }
        }
        ApiErrorType::AuthenticationError => LlmError::AuthenticationFailed { message },
        ApiErrorType::RateLimitError => LlmError::RateLimited {
            retry_after_ms: None,
        },
        _ => LlmError::Unknown { message },
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use meerkat_core::{
        AssistantImageId, BlobId, BlobRef, BlockAssistantMessage, ImageData, MediaType,
        ProviderImageMetadata, RevisedPromptDisposition, ServerToolKind, SystemMessage,
        SystemNoticeKind, SystemNoticeMessage, ToolResult, UserMessage,
    };

    fn sys(text: &str) -> Message {
        Message::System(SystemMessage::new(text.to_string()))
    }

    fn user(text: &str) -> Message {
        Message::User(UserMessage::text(text))
    }

    fn asst(text: &str) -> Message {
        Message::BlockAssistant(BlockAssistantMessage {
            blocks: vec![AssistantBlock::Text {
                text: text.to_string(),
                meta: None,
            }],
            stop_reason: StopReason::EndTurn,
            identity: meerkat_core::types::TranscriptMessageIdentity::default(),
            created_at: meerkat_core::types::message_timestamp_now(),
        })
    }

    fn assistant_image_block() -> AssistantBlock {
        AssistantBlock::Image {
            image_id: AssistantImageId::new(meerkat_core::time_compat::new_uuid_v7()),
            blob_ref: BlobRef {
                blob_id: BlobId::from("realtime-image"),
                media_type: "image/png".to_string(),
            },
            media_type: MediaType::new("image/png"),
            width: 64,
            height: 64,
            revised_prompt: RevisedPromptDisposition::NotRequested,
            meta: ProviderImageMetadata::NotEmitted,
        }
    }

    #[test]
    fn replay_projection_policy_handles_realtime_history_blocks()
    -> Result<(), Box<dyn std::error::Error>> {
        let tool_args = serde_json::value::RawValue::from_string(r#"{"query":"m"}"#.to_string())?;
        let messages = vec![
            Message::User(UserMessage::with_blocks(vec![
                ContentBlock::Text {
                    text: "look".to_string(),
                },
                ContentBlock::Image {
                    media_type: "image/png".to_string(),
                    data: ImageData::Inline {
                        data: "AAAA".to_string(),
                    },
                },
            ])),
            Message::BlockAssistant(BlockAssistantMessage::new(
                vec![
                    AssistantBlock::Reasoning {
                        text: "plan".to_string(),
                        meta: None,
                    },
                    AssistantBlock::ServerToolContent {
                        id: None,
                        kind: ServerToolKind::WebSearch,
                        content: serde_json::json!({"type": "web_search_call"}),
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
                vec![ContentBlock::Image {
                    media_type: "image/png".to_string(),
                    data: ImageData::Inline {
                        data: "BBBB".to_string(),
                    },
                }],
                false,
            )]),
        ];

        let projected = project_realtime_replay_messages(&messages)?;
        let Message::User(user) = &projected[0] else {
            panic!("expected user");
        };
        assert!(
            user.content
                .iter()
                .all(|block| matches!(block, ContentBlock::Text { .. })),
            "Realtime replay should text-project multimodal user content"
        );

        let Message::BlockAssistant(assistant) = &projected[1] else {
            panic!("expected assistant");
        };
        assert!(
            assistant.blocks.iter().all(|block| matches!(
                block,
                AssistantBlock::Text { .. } | AssistantBlock::ToolUse { .. }
            )),
            "Realtime replay should only keep assistant text and tool-use blocks"
        );

        let Message::ToolResults { results, .. } = &projected[2] else {
            panic!("expected tool results");
        };
        assert!(!results[0].has_images());
        assert!(results[0].text_content().contains("[image: image/png]"));
        Ok(())
    }

    #[test]
    fn replay_projection_rejects_realtime_orphan_tool_results() {
        let err =
            project_realtime_replay_messages(&[Message::tool_results(vec![ToolResult::new(
                "tool_1".to_string(),
                "orphaned".to_string(),
                false,
            )])])
            .expect_err("orphan tool results must be rejected");
        assert!(matches!(err, LlmError::InvalidRequest { .. }));
    }

    #[test]
    fn convert_system_and_user_preserves_both_conversation_items() {
        let items = convert_messages(&[sys("You are a helper."), user("Hi!")]).expect("convert");
        assert_eq!(items.len(), 2);
        match &items[0] {
            Item::Message { role, content, .. } => {
                assert_eq!(*role, Role::System);
                assert_eq!(content.len(), 1);
                match &content[0] {
                    ContentPart::InputText { text } => assert_eq!(text, "You are a helper."),
                    other => panic!("unexpected content part: {other:?}"),
                }
            }
            other => panic!("expected Item::Message, got {other:?}"),
        }
    }

    #[test]
    fn convert_messages_preserves_interleaved_systems_exactly() {
        let messages = vec![
            sys(""),
            user("work"),
            sys(" \t "),
            Message::SystemNotice(SystemNoticeMessage::new(
                SystemNoticeKind::Generic,
                "notice",
            )),
            sys("duplicate"),
            user("continue"),
            sys("duplicate"),
        ];
        let original = messages.clone();
        let items = convert_messages(&messages).expect("convert ordered Systems");
        assert_eq!(messages, original);
        assert_eq!(items.len(), 7);
        let roles = items
            .iter()
            .map(|item| match item {
                Item::Message { role, .. } => *role,
                other => panic!("unexpected item: {other:?}"),
            })
            .collect::<Vec<_>>();
        assert_eq!(
            roles,
            vec![
                Role::System,
                Role::User,
                Role::System,
                Role::User,
                Role::System,
                Role::User,
                Role::System,
            ]
        );
        assert!(matches!(
            &items[2],
            Item::Message {
                role: Role::System,
                content,
                ..
            } if matches!(
                content.as_slice(),
                [ContentPart::InputText { text }] if text == " \t "
            )
        ));
        assert!(matches!(
            &items[3],
            Item::Message {
                role: Role::User,
                content,
                ..
            } if matches!(
                content.as_slice(),
                [ContentPart::InputText { text }] if text.contains("notice")
            )
        ));
    }

    #[test]
    fn convert_assistant_history_emits_output_text() {
        let items = convert_messages(&[user("ping"), asst("pong")]).expect("convert");
        assert_eq!(items.len(), 2);
        match &items[1] {
            Item::Message { role, content, .. } => {
                assert_eq!(*role, Role::Assistant);
                match &content[0] {
                    ContentPart::OutputText { text } => assert_eq!(text, "pong"),
                    other => panic!("expected OutputText, got {other:?}"),
                }
            }
            other => panic!("expected assistant Message, got {other:?}"),
        }
    }

    #[test]
    fn convert_tool_call_and_result_round_trips_to_function_items() {
        let asst_with_tool = Message::BlockAssistant(BlockAssistantMessage {
            blocks: vec![AssistantBlock::ToolUse {
                id: "call_42".to_string(),
                name: "read_file".to_string(),
                args: serde_json::value::RawValue::from_string(
                    serde_json::json!({"path": "/tmp/x"}).to_string(),
                )
                .expect("valid args"),
                meta: None,
            }],
            stop_reason: StopReason::ToolUse,
            identity: meerkat_core::types::TranscriptMessageIdentity::default(),
            created_at: meerkat_core::types::message_timestamp_now(),
        });
        let tool_results = Message::ToolResults {
            results: vec![ToolResult::new(
                "call_42".to_string(),
                "file contents".to_string(),
                false,
            )],
            created_at: meerkat_core::types::message_timestamp_now(),
        };

        let items =
            convert_messages(&[user("work"), asst_with_tool, tool_results]).expect("convert");
        assert_eq!(items.len(), 3);
        match &items[1] {
            Item::FunctionCall {
                name,
                call_id,
                arguments,
                ..
            } => {
                assert_eq!(name, "read_file");
                assert_eq!(call_id, "call_42");
                let parsed: serde_json::Value = serde_json::from_str(arguments).expect("args json");
                assert_eq!(parsed["path"], "/tmp/x");
            }
            other => panic!("expected FunctionCall, got {other:?}"),
        }
        match &items[2] {
            Item::FunctionCallOutput {
                call_id, output, ..
            } => {
                assert_eq!(call_id, "call_42");
                assert_eq!(output, "file contents");
            }
            other => panic!("expected FunctionCallOutput, got {other:?}"),
        }
    }

    #[test]
    fn build_tools_collects_function_names_and_descriptions() {
        use meerkat_core::ToolDef;
        use std::sync::Arc;
        let request =
            LlmRequest::new("gpt-realtime-1.5", vec![user("run the tool")]).with_tools(vec![
                Arc::new(ToolDef {
                    name: "read_file".into(),
                    description: "read a file".to_string(),
                    input_schema: serde_json::json!({"type":"object"}),
                    provenance: None,
                }),
            ]);
        let tools = build_tools(&request);
        assert_eq!(tools.len(), 1);
        match &tools[0] {
            Tool::Function {
                name, description, ..
            } => {
                assert_eq!(name, "read_file");
                assert_eq!(description.as_deref(), Some("read a file"));
            }
            other => panic!("expected Tool::Function, got {other:?}"),
        }
    }

    #[test]
    fn realtime_max_output_tokens_rejects_explicit_zero_budget() {
        // #230: an explicit zero cap is a typed InvalidRequest reject, never a
        // silent downgrade to the provider default.
        match realtime_max_output_tokens(0) {
            Err(LlmError::InvalidRequest { message }) => {
                assert!(
                    message.contains("greater than 0"),
                    "unexpected reject message: {message}"
                );
            }
            other => panic!("expected InvalidRequest for zero cap, got {other:?}"),
        }
    }

    #[test]
    fn realtime_max_output_tokens_passes_values_at_or_below_ceiling() {
        match realtime_max_output_tokens(1) {
            Ok(MaxTokens::Count(n)) => assert_eq!(n, 1),
            other => panic!("expected Count(1), got {other:?}"),
        }
        match realtime_max_output_tokens(REALTIME_MAX_OUTPUT_TOKENS) {
            Ok(MaxTokens::Count(n)) => assert_eq!(n, REALTIME_MAX_OUTPUT_TOKENS),
            other => panic!("expected Count(4096), got {other:?}"),
        }
    }

    #[test]
    fn realtime_max_output_tokens_clamps_above_ceiling() {
        // Default agent.max_tokens_per_turn is 16384 — above the realtime API's
        // 4096 integer ceiling. Exercise the two specific values that triggered
        // the s71/s72 smoke failures.
        for requested in [4097_u32, 8_192, 16_384, u32::MAX] {
            match realtime_max_output_tokens(requested) {
                Ok(MaxTokens::Count(n)) => assert_eq!(
                    n, REALTIME_MAX_OUTPUT_TOKENS,
                    "requested={requested} should clamp to {REALTIME_MAX_OUTPUT_TOKENS}"
                ),
                other => panic!("expected clamped Count, got {other:?}"),
            }
        }
    }

    #[test]
    fn http_status_unauthorized_maps_to_authentication_failed() {
        let mapped = map_http_status(
            reqwest::StatusCode::UNAUTHORIZED,
            "request failed".to_string(),
        );

        assert!(matches!(mapped, LlmError::AuthenticationFailed { .. }));
    }

    #[test]
    fn http_status_rate_limit_maps_to_rate_limited() {
        let mapped = map_http_status(
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            "request failed".to_string(),
        );

        assert!(matches!(
            mapped,
            LlmError::RateLimited {
                retry_after_ms: None
            }
        ));
    }

    #[test]
    fn http_status_request_timeout_maps_to_network_timeout() {
        let mapped = map_http_status(
            reqwest::StatusCode::REQUEST_TIMEOUT,
            "request failed".to_string(),
        );

        assert!(matches!(mapped, LlmError::NetworkTimeout { .. }));
    }

    #[test]
    fn server_error_model_not_found_uses_structured_code() {
        let mapped = map_server_error(oai_rt_rs::error::ServerError {
            error_type: oai_rt_rs::error::ApiErrorType::InvalidRequestError,
            code: Some("model_not_found".to_string()),
            message: "realtime model is unavailable".to_string(),
            param: Some("model".to_string()),
            event_id: None,
        });

        assert!(matches!(mapped, LlmError::ModelNotFound { .. }));
    }

    #[test]
    fn server_error_context_limit_uses_structured_code() {
        for code in ["context_length_exceeded", "context_window_exceeded"] {
            let mapped = map_server_error(oai_rt_rs::error::ServerError {
                error_type: oai_rt_rs::error::ApiErrorType::InvalidRequestError,
                code: Some(code.to_string()),
                message: "the request exceeds the model context".to_string(),
                param: None,
                event_id: None,
            });

            assert!(matches!(mapped, LlmError::ContextLengthExceeded { .. }));
        }
    }

    #[test]
    fn server_error_context_words_without_structured_code_stay_invalid_request() {
        let message = "free-form context_length_exceeded text is not provider evidence".to_string();
        let mapped = map_server_error(oai_rt_rs::error::ServerError {
            error_type: oai_rt_rs::error::ApiErrorType::InvalidRequestError,
            code: Some("invalid_request_error".to_string()),
            message: message.clone(),
            param: None,
            event_id: None,
        });

        assert!(matches!(
            mapped,
            LlmError::InvalidRequest { message: mapped_message }
                if mapped_message == message
        ));
    }

    #[test]
    fn server_error_free_form_message_substrings_do_not_classify() {
        let message = "free-form provider text mentioning unauthorized timeout 401 model_not_found"
            .to_string();
        let mapped = map_server_error(oai_rt_rs::error::ServerError {
            error_type: oai_rt_rs::error::ApiErrorType::InvalidRequestError,
            code: None,
            message: message.clone(),
            param: None,
            event_id: None,
        });

        assert!(matches!(
            mapped,
            LlmError::InvalidRequest { message: mapped_message }
                if mapped_message == message
        ));
    }

    #[test]
    fn health_check_empty_key_fails() {
        let adapter = OpenAiRealtimeTextAdapter::new("");
        let result = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(adapter.health_check());
        assert!(matches!(result, Err(LlmError::AuthenticationFailed { .. })));
    }

    #[test]
    fn provider_is_openai() {
        let adapter = OpenAiRealtimeTextAdapter::new("sk-test");
        assert_eq!(adapter.provider(), meerkat_core::Provider::OpenAI);
    }

    // Row #230: an unset caller temperature is the Unset distinction and
    // defers to the provider default; an explicit-but-invalid temperature
    // surfaces a typed reject rather than a silent downgrade.
    #[test]
    fn resolve_realtime_temperature_none_is_caller_unset() {
        assert!(
            resolve_realtime_temperature(None)
                .expect("unset temperature is valid")
                .is_none(),
            "None must defer to the provider default, not be a reject"
        );
    }

    #[test]
    fn resolve_realtime_temperature_accepts_in_range() {
        let resolved = resolve_realtime_temperature(Some(0.5)).expect("in-range temperature");
        assert!(resolved.is_some());
    }

    #[test]
    fn resolve_realtime_temperature_rejects_out_of_range() {
        // Without the fix this silently became `None` (provider default),
        // dropping explicit caller policy. It must now be a typed reject.
        let err = resolve_realtime_temperature(Some(5.0))
            .expect_err("an out-of-range temperature must be a typed reject, not a silent drop");
        assert!(
            matches!(err, LlmError::InvalidRequest { .. }),
            "expected InvalidRequest, got {err:?}"
        );

        let negative = resolve_realtime_temperature(Some(-1.0))
            .expect_err("a negative temperature must be a typed reject");
        assert!(matches!(negative, LlmError::InvalidRequest { .. }));
    }

    #[test]
    fn replay_projection_rejects_duplicate_tool_use_ids() {
        let client = OpenAiRealtimeTextAdapter::new("test-key");
        let args = serde_json::value::RawValue::from_string("{}".to_string())
            .unwrap_or_else(|error| panic!("test args: {error}"));
        let messages = [
            Message::BlockAssistant(BlockAssistantMessage::new(
                vec![
                    AssistantBlock::ToolUse {
                        id: "duplicate".to_string(),
                        name: "a".to_string(),
                        args: args.clone(),
                        meta: None,
                    },
                    AssistantBlock::ToolUse {
                        id: "duplicate".to_string(),
                        name: "b".to_string(),
                        args,
                        meta: None,
                    },
                ],
                StopReason::ToolUse,
            )),
            Message::tool_results(vec![ToolResult::new(
                "duplicate".to_string(),
                "result".to_string(),
                false,
            )]),
        ];
        assert!(matches!(
            client.project_replay_messages(&messages),
            Err(LlmError::InvalidRequest { .. })
        ));
    }

    #[test]
    fn replay_projection_rejects_duplicate_tool_result_ids() {
        let client = OpenAiRealtimeTextAdapter::new("test-key");
        let args = serde_json::value::RawValue::from_string("{}".to_string())
            .unwrap_or_else(|error| panic!("test args: {error}"));
        let messages = [
            Message::BlockAssistant(BlockAssistantMessage::new(
                vec![AssistantBlock::ToolUse {
                    id: "tool".to_string(),
                    name: "a".to_string(),
                    args,
                    meta: None,
                }],
                StopReason::ToolUse,
            )),
            Message::tool_results(vec![
                ToolResult::new("tool".to_string(), "first".to_string(), false),
                ToolResult::new("tool".to_string(), "second".to_string(), false),
            ]),
        ];
        assert!(matches!(
            client.project_replay_messages(&messages),
            Err(LlmError::InvalidRequest { .. })
        ));
    }

    #[test]
    fn replay_projection_rejects_mismatched_tool_result_id() {
        let client = OpenAiRealtimeTextAdapter::new("test-key");
        let args = serde_json::value::RawValue::from_string("{}".to_string())
            .unwrap_or_else(|error| panic!("test args: {error}"));
        let messages = [
            Message::BlockAssistant(BlockAssistantMessage::new(
                vec![AssistantBlock::ToolUse {
                    id: "tool".to_string(),
                    name: "a".to_string(),
                    args,
                    meta: None,
                }],
                StopReason::ToolUse,
            )),
            Message::tool_results(vec![ToolResult::new(
                "other".to_string(),
                "result".to_string(),
                false,
            )]),
        ];
        assert!(matches!(
            client.project_replay_messages(&messages),
            Err(LlmError::InvalidRequest { .. })
        ));
    }
}
