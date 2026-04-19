//! OpenAI Realtime **text-turn** adapter.
//!
//! Implements [`LlmClient`] for realtime-capable OpenAI models (e.g.
//! `gpt-realtime-1.5`). The Responses API endpoint `/v1/responses` rejects
//! realtime model IDs with `model_not_found`, so any session whose resolved
//! model advertises `ModelCapabilities.realtime == true` must reach the
//! model through the Realtime WebSocket instead.
//!
//! Strategy (per-turn, stateless replay):
//! 1. Open a WebSocket to `wss://api.openai.com/v1/realtime?model=<model>`
//!    via `oai-rt-rs` (GA protocol; no `OpenAI-Beta` header).
//! 2. `session.update` → `type: "realtime"`, `output_modalities: Text`,
//!    tool definitions, and any system prompt as `instructions`.
//! 3. Replay the full message history as `conversation.item.create`
//!    events (user/assistant messages, function_call, function_call_output).
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

/// LlmClient implementation that serves text turns via OpenAI Realtime WS.
#[derive(Debug, Clone)]
pub struct OpenAiRealtimeTextAdapter {
    api_key: String,
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
    fn stream<'a>(&'a self, request: &'a LlmRequest) -> LlmStream<'a> {
        let api_key = self.api_key.clone();
        Box::pin(try_stream! {
            let (instructions, history_items) = convert_messages(&request.messages)?;
            let tools = build_tools(request);

            // Connect WS — GA protocol (no OpenAI-Beta header).
            let mut client = RealtimeClient::connect(
                &api_key,
                Some(&request.model),
                None,
            )
            .await
            .map_err(map_oai_error)?;

            // session.update → text-only output, declare tools, set instructions.
            let session_update = SessionUpdate {
                config: SessionUpdateConfig {
                    output_modalities: Some(OutputModalities::Text),
                    instructions: instructions.clone(),
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
            // as the Responses-API path. Temperatures outside the realtime
            // protocol's accepted [0.0, 2.0] range are dropped rather than
            // rejected — the upstream constructor returns a typed error we
            // do not want to elevate mid-stream.
            let max_output_tokens =
                (request.max_tokens > 0).then_some(MaxTokens::Count(request.max_tokens));
            let temperature = request
                .temperature
                .and_then(|t| Temperature::new(t).ok());
            let response_config = ResponseConfig {
                conversation: Some(ConversationMode::None),
                output_modalities: Some(OutputModalities::Text),
                instructions: instructions.clone(),
                tools: Some(tools.clone()),
                max_output_tokens,
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
                        let args = if arguments.trim().is_empty() {
                            serde_json::json!({})
                        } else {
                            match serde_json::from_str::<serde_json::Value>(&arguments) {
                                Ok(value) => value,
                                Err(err) => {
                                    tracing::warn!(
                                        call_id = %call_id,
                                        tool = %name,
                                        error = %err,
                                        "openai realtime: tool_call arguments failed JSON parse; falling back to raw string"
                                    );
                                    serde_json::Value::String(arguments.clone())
                                }
                            }
                        };
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
                            last_usage = Some(map_usage(usage));
                        }
                        if let Some(usage) = last_usage.clone() {
                            yield LlmEvent::UsageUpdate { usage };
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

    fn provider(&self) -> &'static str {
        // Share the "openai" provider label so factory-level provider
        // inference / logging treats realtime sessions uniformly.
        "openai"
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

/// Convert a meerkat message history into an `instructions` string
/// (collected from system messages) plus a list of realtime `Item`s for
/// replay through `conversation.item.create`.
fn convert_messages(messages: &[Message]) -> Result<(Option<String>, Vec<Item>), LlmError> {
    let mut instructions_parts: Vec<String> = Vec::new();
    let mut items: Vec<Item> = Vec::new();

    for msg in messages {
        match msg {
            Message::System(s) => {
                if !s.content.trim().is_empty() {
                    instructions_parts.push(s.content.clone());
                }
            }
            Message::SystemNotice(notice) => {
                let rendered = notice.rendered_text();
                if !rendered.trim().is_empty() {
                    items.push(Item::Message {
                        id: None,
                        status: None,
                        role: Role::User,
                        content: vec![ContentPart::InputText { text: rendered }],
                    });
                }
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
                            role: Role::User,
                            content: parts,
                        });
                    }
                } else if !text.is_empty() {
                    items.push(Item::Message {
                        id: None,
                        status: None,
                        role: Role::User,
                        content: vec![ContentPart::InputText { text }],
                    });
                }
            }
            Message::Assistant(a) => {
                if !a.content.trim().is_empty() {
                    items.push(Item::Message {
                        id: None,
                        status: None,
                        role: Role::Assistant,
                        content: vec![ContentPart::OutputText {
                            text: a.content.clone(),
                        }],
                    });
                }
                for tc in &a.tool_calls {
                    items.push(Item::FunctionCall {
                        id: None,
                        status: None,
                        name: tc.name.clone(),
                        call_id: tc.id.clone(),
                        arguments: tc.args.to_string(),
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
                                    role: Role::Assistant,
                                    content: vec![ContentPart::OutputText { text: text.clone() }],
                                });
                            }
                        }
                        AssistantBlock::ToolUse { id, name, args, .. } => {
                            items.push(Item::FunctionCall {
                                id: None,
                                status: None,
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
            Message::ToolResults { results } => {
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
                        call_id: r.tool_use_id.clone(),
                        output: r.text_content(),
                    });
                }
            }
        }
    }

    let instructions = if instructions_parts.is_empty() {
        None
    } else {
        Some(instructions_parts.join("\n\n"))
    };
    Ok((instructions, items))
}

fn build_tools(request: &LlmRequest) -> Vec<Tool> {
    request
        .tools
        .iter()
        .map(|tool| Tool::Function {
            name: tool.name.clone(),
            description: (!tool.description.trim().is_empty()).then(|| tool.description.clone()),
            parameters: tool.input_schema.clone(),
        })
        .collect()
}

fn map_usage(u: &OaiUsage) -> Usage {
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
    }
}

fn map_oai_error(err: oai_rt_rs::Error) -> LlmError {
    use oai_rt_rs::Error;
    match err {
        Error::Http(e) => {
            let msg = e.to_string();
            if msg.to_lowercase().contains("401") || msg.to_lowercase().contains("unauthorized") {
                LlmError::AuthenticationFailed { message: msg }
            } else if msg.to_lowercase().contains("timeout") {
                LlmError::NetworkTimeout { duration_ms: 30000 }
            } else {
                LlmError::Unknown { message: msg }
            }
        }
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

fn map_server_error(err: oai_rt_rs::error::ServerError) -> LlmError {
    use oai_rt_rs::error::ApiErrorType;
    let message = err.message.clone();
    let code = err.code.as_deref().unwrap_or_default();
    match err.error_type {
        ApiErrorType::InvalidRequestError => {
            if code == "model_not_found" || message.contains("model_not_found") {
                LlmError::ModelNotFound { model: message }
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
    use meerkat_core::{AssistantMessage, SystemMessage, ToolCall, ToolResult, UserMessage};

    fn sys(text: &str) -> Message {
        Message::System(SystemMessage {
            content: text.to_string(),
        })
    }

    fn user(text: &str) -> Message {
        Message::User(UserMessage::text(text))
    }

    fn asst(text: &str) -> Message {
        Message::Assistant(AssistantMessage {
            content: text.to_string(),
            tool_calls: Vec::new(),
            stop_reason: StopReason::EndTurn,
            usage: Usage::default(),
        })
    }

    #[test]
    fn convert_system_and_user_produces_instructions_and_one_user_item() {
        let (instructions, items) =
            convert_messages(&[sys("You are a helper."), user("Hi!")]).expect("convert");
        assert_eq!(instructions.as_deref(), Some("You are a helper."));
        assert_eq!(items.len(), 1);
        match &items[0] {
            Item::Message { role, content, .. } => {
                assert_eq!(*role, Role::User);
                assert_eq!(content.len(), 1);
                match &content[0] {
                    ContentPart::InputText { text } => assert_eq!(text, "Hi!"),
                    other => panic!("unexpected content part: {other:?}"),
                }
            }
            other => panic!("expected Item::Message, got {other:?}"),
        }
    }

    #[test]
    fn convert_assistant_history_emits_output_text() {
        let (_, items) = convert_messages(&[user("ping"), asst("pong")]).expect("convert");
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
        let asst_with_tool = Message::Assistant(AssistantMessage {
            content: String::new(),
            tool_calls: vec![ToolCall::new(
                "call_42".to_string(),
                "read_file".to_string(),
                serde_json::json!({"path": "/tmp/x"}),
            )],
            stop_reason: StopReason::ToolUse,
            usage: Usage::default(),
        });
        let tool_results = Message::ToolResults {
            results: vec![ToolResult::new(
                "call_42".to_string(),
                "file contents".to_string(),
                false,
            )],
        };

        let (_, items) =
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
                    name: "read_file".to_string(),
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
        assert_eq!(adapter.provider(), "openai");
    }
}
