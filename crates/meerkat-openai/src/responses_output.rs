//! Mechanical lowering of complete Responses output into the canonical block
//! vocabulary. Terminal items may contain continuity data absent from deltas.

use meerkat_core::{AssistantBlock, ProviderMeta, ServerToolKind};
use meerkat_llm_core::LlmError;
use serde_json::Value;
use std::collections::HashSet;

use crate::client::{
    openai_response_meta, parse_tool_call_arguments, web_search_message_annotations,
};

fn malformed(message: impl Into<String>) -> LlmError {
    LlmError::StreamParseError {
        message: message.into(),
    }
}

fn required<'a>(item: &'a Value, field: &str) -> Result<&'a str, LlmError> {
    item.get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| malformed(format!("Responses output item missing string '{field}'")))
}

/// Unknown fields are additive metadata. Unknown item/content discriminators
/// and execution modes are not: accepting those would discard effects.
pub(crate) enum OutputKind {
    Message,
    Reasoning,
    FunctionCall,
    WebSearch,
}

pub(crate) fn validate_item(item: &Value) -> Result<OutputKind, LlmError> {
    let kind = required(item, "type")?;
    match kind {
        "message" => Ok(OutputKind::Message),
        "reasoning" => Ok(OutputKind::Reasoning),
        "web_search_call" | "web_search_result" => Ok(OutputKind::WebSearch),
        "function_call" => {
            if item
                .get("async")
                .is_some_and(|value| !value.is_null() && value != false)
                || item.get("namespace").is_some_and(|value| !value.is_null())
                || item.get("caller").is_some_and(|caller| {
                    !caller.is_null()
                        && caller.get("type").and_then(Value::as_str) != Some("direct")
                })
            {
                return Err(LlmError::InvalidRequest {
                    message: "unsupported Responses function-call execution semantics (async, namespace, or programmatic caller)".into(),
                });
            }
            Ok(OutputKind::FunctionCall)
        }
        _ => Err(LlmError::InvalidRequest {
            message: format!(
                "unsupported Responses output item '{kind}'; execution semantics cannot be discarded"
            ),
        }),
    }
}

pub(crate) fn assistant_meta(
    item: &Value,
    response_id: Option<&str>,
) -> Result<Option<Box<ProviderMeta>>, LlmError> {
    let phase = item
        .get("phase")
        .filter(|phase| !phase.is_null())
        .map(|phase| {
            serde_json::from_value(phase.clone()).map_err(|error| {
                malformed(format!("unsupported Responses assistant phase: {error}"))
            })
        })
        .transpose()?;
    match item.get("id").and_then(Value::as_str) {
        Some(id) => Ok(Some(Box::new(ProviderMeta::OpenAiAssistantMessage {
            id: id.to_owned(),
            phase,
            response_id: response_id.map(str::to_owned),
        }))),
        None if phase.is_some() => Err(malformed(
            "Responses assistant phase requires item identity",
        )),
        // Some compatible endpoints omit item ids on unphased messages.
        None => Ok(openai_response_meta(response_id)),
    }
}

pub(crate) fn lower_items<'a>(
    items: impl IntoIterator<Item = &'a Value>,
    response_id: Option<&str>,
) -> Result<Vec<AssistantBlock>, LlmError> {
    let mut blocks = Vec::new();
    let mut call_ids = HashSet::new();
    for item in items {
        match validate_item(item)? {
            OutputKind::Message => {
                if item
                    .get("role")
                    .and_then(Value::as_str)
                    .is_some_and(|role| role != "assistant")
                {
                    return Err(malformed(
                        "Responses output message is not an assistant message",
                    ));
                }
                let parts = item
                    .get("content")
                    .and_then(Value::as_array)
                    .ok_or_else(|| {
                        malformed("Responses assistant message missing content array")
                    })?;
                let mut text = String::new();
                let mut annotations = Vec::new();
                for part in parts {
                    match required(part, "type")? {
                        "output_text" => {
                            text.push_str(required(part, "text")?);
                            if let Some(citations) = part
                                .get("annotations")
                                .and_then(web_search_message_annotations)
                            {
                                annotations.extend(citations);
                            }
                        }
                        "refusal" => text.push_str(required(part, "refusal")?),
                        kind => {
                            return Err(LlmError::InvalidRequest {
                                message: format!(
                                    "unsupported Responses assistant content '{kind}'"
                                ),
                            });
                        }
                    }
                }
                blocks.push(AssistantBlock::Text {
                    text,
                    meta: assistant_meta(item, response_id)?,
                });
                if !annotations.is_empty() {
                    blocks.push(AssistantBlock::ServerToolContent {
                        id: item.get("id").and_then(Value::as_str).map(str::to_owned),
                        kind: ServerToolKind::WebSearch,
                        content: serde_json::json!({"type":"message_annotations","annotations":annotations}),
                        meta: openai_response_meta(response_id),
                    });
                }
            }
            OutputKind::Reasoning => {
                let id = required(item, "id")?;
                let mut text = String::new();
                if let Some(summaries) = item.get("summary").and_then(Value::as_array) {
                    for summary in summaries {
                        if !text.is_empty() {
                            text.push('\n');
                        }
                        text.push_str(required(summary, "text")?);
                    }
                }
                blocks.push(AssistantBlock::Reasoning {
                    text,
                    meta: Some(Box::new(ProviderMeta::OpenAi {
                        id: id.to_owned(),
                        encrypted_content: item
                            .get("encrypted_content")
                            .and_then(Value::as_str)
                            .map(str::to_owned),
                        phase: item.get("phase").and_then(Value::as_str).map(str::to_owned),
                        response_id: response_id.map(str::to_owned),
                    })),
                });
            }
            OutputKind::FunctionCall => {
                if item
                    .get("status")
                    .and_then(Value::as_str)
                    .is_some_and(|status| status != "completed")
                {
                    return Err(LlmError::InvalidRequest {
                        message: "unsupported non-completed Responses function call".into(),
                    });
                }
                let id = required(item, "call_id")?;
                if !call_ids.insert(id) {
                    return Err(malformed(format!(
                        "duplicate Responses function call '{id}'"
                    )));
                }
                let name = required(item, "name")?;
                let arguments = match item.get("arguments") {
                    None => "{}",
                    Some(Value::String(arguments)) => arguments,
                    Some(_) => {
                        return Err(malformed("Responses function arguments must be a string"));
                    }
                };
                let (args, _) = parse_tool_call_arguments(arguments, id)?;
                blocks.push(AssistantBlock::ToolUse {
                    id: id.to_owned(),
                    name: name.to_owned(),
                    args,
                    meta: openai_response_meta(response_id),
                });
            }
            OutputKind::WebSearch => blocks.push(AssistantBlock::ServerToolContent {
                id: item
                    .get("id")
                    .or_else(|| item.get("call_id"))
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                kind: ServerToolKind::WebSearch,
                content: item.clone(),
                meta: openai_response_meta(response_id),
            }),
        }
    }
    Ok(blocks)
}

fn item_id<'a>(item: &'a Value, kind: &str) -> Option<&'a str> {
    match kind {
        "function_call" => item.get("call_id"),
        "web_search_call" | "web_search_result" => item.get("id").or_else(|| item.get("call_id")),
        _ => item.get("id"),
    }
    .and_then(Value::as_str)
}

/// Atomic item-done events need coverage even when they emitted no deltas.
pub(crate) fn validate_completed_coverage<'a>(
    output: &[Value],
    completed: impl IntoIterator<Item = &'a Value>,
) -> Result<(), LlmError> {
    for observed in completed {
        let kind = required(observed, "type")?;
        let id = item_id(observed, kind);
        let covered = output.iter().any(|item| {
            item.get("type").and_then(Value::as_str) == Some(kind)
                && match id {
                    Some(id) => item_id(item, kind) == Some(id),
                    // Id-less compatible items cannot be correlated safely
                    // unless the complete item is retained verbatim.
                    None => item == observed,
                }
        });
        if !covered {
            return Err(malformed(format!(
                "terminal Responses output omitted completed {kind} item '{}'",
                id.unwrap_or("<id-less; exact item required>")
            )));
        }
    }
    Ok(())
}

/// Replacing provisional assembly is only safe when complete output covers
/// previously observed items. A pruned terminal summary is a protocol error,
/// not permission to drop a call or silently change replay history.
pub(crate) fn validate_coverage(
    blocks: &[AssistantBlock],
    tools: &HashSet<String>,
    reasoning: &HashSet<String>,
    messages: &HashSet<String>,
    searches: &HashSet<String>,
) -> Result<(), LlmError> {
    for id in tools {
        if !blocks.iter().any(
            |block| matches!(block, AssistantBlock::ToolUse { id: actual, .. } if actual == id),
        ) {
            return Err(malformed(format!(
                "terminal Responses output omitted streamed function call '{id}'"
            )));
        }
    }
    for id in reasoning {
        if !blocks.iter().any(|block| {
            matches!(block, AssistantBlock::Reasoning { meta: Some(meta), .. }
            if matches!(meta.as_ref(), ProviderMeta::OpenAi { id: actual, .. } if actual == id))
        }) {
            return Err(malformed(format!(
                "terminal Responses output omitted streamed reasoning '{id}'"
            )));
        }
    }
    for id in messages {
        if !blocks.iter().any(|block| matches!(block, AssistantBlock::Text { meta: Some(meta), .. }
            if matches!(meta.as_ref(), ProviderMeta::OpenAiAssistantMessage { id: actual, .. } if actual == id))) {
            return Err(malformed(format!("terminal Responses output omitted streamed assistant message '{id}'")));
        }
    }
    for id in searches {
        if !blocks.iter().any(|block| matches!(block, AssistantBlock::ServerToolContent { id: Some(actual), kind: ServerToolKind::WebSearch, .. } if actual == id)) {
            return Err(malformed(format!("terminal Responses output omitted streamed web search '{id}'")));
        }
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::OpenAiClient;
    use axum::{Router, extract::State, routing::post};
    use futures::StreamExt;
    use meerkat_core::{AgentLlmClient, Message, Session, ToolResult, UserMessage};
    use meerkat_llm_core::{LlmClient, LlmClientAdapter, LlmDoneOutcome, LlmEvent, LlmRequest};
    use serde_json::json;
    use std::fmt::Write as _;
    use std::sync::Arc;

    async fn stub(events: Vec<Value>) -> (OpenAiClient, tokio::task::JoinHandle<()>) {
        let mut payload = String::new();
        for event in events {
            write!(payload, "data: {event}\n\n").expect("formatting into a String cannot fail");
        }
        async fn respond(State(payload): State<String>) -> impl axum::response::IntoResponse {
            ([("content-type", "text/event-stream")], payload)
        }
        let app = Router::new()
            .route("/v1/responses", post(respond))
            .with_state(payload);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (
            OpenAiClient::new_with_base_url("unused".into(), url),
            server,
        )
    }

    fn message(id: &str, phase: &str, text: &str) -> Value {
        json!({"type":"message","id":id,"role":"assistant","phase":phase,
            "content":[{"type":"output_text","text":text,"annotations":[]}]})
    }

    #[tokio::test]
    async fn phases_order_and_terminal_reasoning_survive_session_resume() {
        let first = message("msg_first", "commentary", "Working.");
        let final_message = message("msg_final", "final_answer", "Done.");
        let output = json!([
            first,
            {"type":"reasoning","id":"rs_1","summary":[{"type":"summary_text","text":"Plan"}],"encrypted_content":"terminal-cipher"},
            {"type":"function_call","id":"fc_1","call_id":"call_1","name":"lookup","arguments":"{}","status":"completed"},
            final_message,
            {"type":"reasoning","id":"rs_tail","summary":[],"encrypted_content":"tail-cipher"}
        ]);
        let events = vec![
            json!({"type":"response.created","response":{"id":"resp_1"}}),
            json!({"type":"response.output_item.added","output_index":0,"item":first}),
            json!({"type":"response.output_text.delta","item_id":"msg_first","delta":"Working."}),
            json!({"type":"response.reasoning_summary.done","item":{"type":"reasoning","id":"rs_1","summary":[{"text":"Plan"}]}}),
            json!({"type":"response.function_call_arguments.done","call_id":"call_1","name":"lookup","arguments":"{}"}),
            json!({"type":"response.completed","response":{"id":"resp_1","status":"completed","output":output,
                "usage":{"input_tokens":13,"output_tokens":7,"total_tokens":20}}}),
        ];
        let (client, server) = stub(events).await;
        let (tx, mut rx) = tokio::sync::mpsc::channel(32);
        let adapter =
            LlmClientAdapter::with_event_channel(Arc::new(client), "gpt-6-astra".into(), tx);
        let result = adapter
            .stream_response(
                &[Message::User(UserMessage::text("go"))],
                &[],
                1000,
                None,
                None,
            )
            .await
            .unwrap();
        assert_eq!(result.blocks().len(), 5);
        assert_eq!(result.usage().input_tokens, 13);
        assert_eq!(result.usage().output_tokens, 7);
        let mut visible = String::new();
        while let Ok(event) = rx.try_recv() {
            if let meerkat_core::AgentEvent::TextDelta { delta } = event {
                visible.push_str(&delta);
            }
        }
        assert_eq!(
            visible, "Working.Done.",
            "terminal-only second message must stream once"
        );
        let mut session = Session::new();
        session.push(Message::BlockAssistant(result.into_message()));
        session.push(Message::tool_results(vec![ToolResult::new(
            "call_1".into(),
            "ok".into(),
            false,
        )]));
        let restored: Session =
            serde_json::from_slice(&serde_json::to_vec(&session).unwrap()).unwrap();
        assert_eq!(restored.messages(), session.messages());
        let replay_client = OpenAiClient::new("unused".into());
        let projected = replay_client
            .project_replay_messages(restored.messages())
            .unwrap();
        let body = replay_client
            .build_request_body(&LlmRequest::new("gpt-6-astra", projected))
            .unwrap();
        assert_eq!(body["store"], false);
        assert!(body.get("previous_response_id").is_none());
        let input = body["input"].as_array().unwrap();
        assert_eq!(input.len(), 6);
        assert_eq!(input[0]["id"], "msg_first");
        assert_eq!(input[0]["phase"], "commentary");
        assert_eq!(input[1]["encrypted_content"], "terminal-cipher");
        assert_eq!(input[2]["call_id"], "call_1");
        assert_eq!(input[3]["id"], "msg_final");
        assert_eq!(input[3]["phase"], "final_answer");
        assert_eq!(input[4]["encrypted_content"], "tail-cipher");
        assert_eq!(input[5]["type"], "function_call_output");
        server.abort();
    }

    #[tokio::test]
    async fn item_done_fallback_keeps_phase_and_output_index_order() {
        let first = message("msg_1", "commentary", "First");
        let last = message("msg_2", "final_answer", "Last");
        let (client, server) = stub(vec![
            json!({"type":"response.output_item.done","output_index":1,"item":last}),
            json!({"type":"response.output_item.done","output_index":0,"item":first}),
            json!({"type":"response.done","response":{"id":"resp_1","status":"completed"}}),
        ])
        .await;
        let adapter = LlmClientAdapter::new(Arc::new(client), "gpt-6-astra".into());
        let result = adapter
            .stream_response(&[], &[], 1000, None, None)
            .await
            .unwrap();
        assert_eq!(
            result.blocks(),
            lower_items([&first, &last], Some("resp_1")).unwrap()
        );
        server.abort();
    }

    #[test]
    fn empty_phased_message_and_associated_reasoning_are_retained() {
        let items = [
            message("empty", "commentary", ""),
            json!({"type":"reasoning","id":"rs","summary":[],"encrypted_content":"cipher"}),
        ];
        let blocks = lower_items(&items, None).unwrap();
        let client = OpenAiClient::new("unused".into());
        let messages = vec![Message::BlockAssistant(
            meerkat_core::BlockAssistantMessage::new(
                blocks.clone(),
                meerkat_core::StopReason::MaxTokens,
            ),
        )];
        let projected = client.project_replay_messages(&messages).unwrap();
        let body = client
            .build_request_body(&LlmRequest::new("gpt-6-astra", projected))
            .unwrap();
        assert_eq!(body["input"].as_array().unwrap().len(), blocks.len());
    }

    #[tokio::test]
    async fn structured_policy_errors_terminalize_without_a_success_snapshot() {
        let error = json!({"code":"misalignment_policy_violation","message":"blocked"});
        for terminal in [
            json!({"type":"error","code":"misalignment_policy_violation","message":"blocked"}),
            json!({"type":"error","error":error}),
            json!({"type":"response.failed","response":{"error":error}}),
            json!({"type":"response.failed","response":{"status_details":{"error":error}}}),
            json!({"type":"response.completed","response":{"status":"completed","error":error,"output":[message("msg","final_answer","must not commit")]}}),
        ] {
            let (client, server) = stub(vec![
                json!({"type":"response.output_text.delta","delta":"partial"}),
                terminal,
            ])
            .await;
            let request = LlmRequest::new("gpt-6-astra", vec![]);
            let events = client.stream(&request).collect::<Vec<_>>().await;
            assert!(
                !events
                    .iter()
                    .any(|event| matches!(event, Ok(LlmEvent::AssistantOutput { .. })))
            );
            assert!(matches!(
                events.last().unwrap(),
                Ok(LlmEvent::Done {
                    outcome: LlmDoneOutcome::Error {
                        error: LlmError::PolicyStop { .. }
                    }
                })
            ));
            server.abort();
        }
    }

    #[tokio::test]
    async fn pruned_terminal_output_cannot_discard_streamed_items() {
        for observed in [
            json!({"type":"response.output_text.delta","item_id":"msg_lost","delta":"partial"}),
            json!({"type":"response.function_call_arguments.done","call_id":"call_lost","name":"lookup","arguments":"{}"}),
            json!({"type":"response.reasoning_summary.done","item":{"type":"reasoning","id":"rs_lost","summary":[{"text":"Plan"}]}}),
            json!({"type":"response.web_search_call.searching","item_id":"ws_lost","output_index":0}),
        ] {
            let (client, server) = stub(vec![observed,
                json!({"type":"response.completed","response":{"status":"completed","output":[message("other","final_answer","Done")]}})
            ]).await;
            let request = LlmRequest::new("gpt-6-astra", vec![]);
            let events = client.stream(&request).collect::<Vec<_>>().await;
            assert!(matches!(
                events.last().unwrap(),
                Ok(LlmEvent::Done {
                    outcome: LlmDoneOutcome::Error {
                        error: LlmError::StreamParseError { .. }
                    }
                })
            ));
            assert!(
                !events
                    .iter()
                    .any(|event| matches!(event, Ok(LlmEvent::AssistantOutput { .. })))
            );
            server.abort();
        }
    }

    #[tokio::test]
    async fn pruned_terminal_output_cannot_discard_atomic_completed_items() {
        for item in [
            message("msg_atomic", "commentary", "Working."),
            json!({"type":"reasoning","id":"rs_atomic","summary":[],"encrypted_content":"atomic-cipher"}),
            json!({"type":"web_search_call","id":"ws_atomic","status":"completed"}),
        ] {
            let (client, server) = stub(vec![
                json!({"type":"response.output_item.done","output_index":0,"item":item}),
                json!({"type":"response.completed","response":{"status":"completed","output":[message("other","final_answer","Done")]}}),
            ]).await;
            let request = LlmRequest::new("gpt-6-astra", vec![]);
            let events = client.stream(&request).collect::<Vec<_>>().await;
            assert!(matches!(
                events.last().unwrap(),
                Ok(LlmEvent::Done {
                    outcome: LlmDoneOutcome::Error {
                        error: LlmError::StreamParseError { .. }
                    }
                })
            ));
            assert!(
                !events
                    .iter()
                    .any(|event| matches!(event, Ok(LlmEvent::AssistantOutput { .. })))
            );
            server.abort();
        }
    }

    #[test]
    fn completed_coverage_uses_typed_identity_and_handles_idless_items() {
        let observed = message("same_id", "final_answer", "Done");
        let different_kind = json!({"type":"reasoning","id":"same_id","summary":[]});
        assert!(validate_completed_coverage(&[different_kind], [&observed]).is_err());

        let mut enriched = observed.clone();
        enriched["future_metadata"] = json!({"inert":true});
        validate_completed_coverage(&[enriched], [&observed]).unwrap();

        let call = json!({"type":"function_call","id":"fc_old","call_id":"call_stable","name":"lookup","arguments":"{}"});
        let mut terminal_call = call.clone();
        terminal_call["id"] = json!("fc_new");
        validate_completed_coverage(&[terminal_call], [&call]).unwrap();

        let idless = json!({"type":"message","role":"assistant","content":[{"type":"output_text","text":"Done"}]});
        validate_completed_coverage(std::slice::from_ref(&idless), [&idless]).unwrap();
        assert!(validate_completed_coverage(&[observed], [&idless]).is_err());
    }

    #[tokio::test]
    async fn unsupported_effects_fail_but_web_search_and_inert_metadata_survive() {
        for kind in [
            "program",
            "tool_search_call",
            "mcp_call",
            "computer_call",
            "compaction",
            "configuration_update",
        ] {
            let item = json!({"type":kind,"id":"effect_1"});
            assert!(matches!(
                lower_items([&item], None),
                Err(LlmError::InvalidRequest { .. })
            ));
            let (client, server) = stub(vec![
                json!({"type":"response.output_item.added","item":item}),
            ])
            .await;
            let request = LlmRequest::new("gpt-6-astra", vec![]);
            let events = client.stream(&request).collect::<Vec<_>>().await;
            assert!(matches!(
                events.last().unwrap(),
                Ok(LlmEvent::Done {
                    outcome: LlmDoneOutcome::Error {
                        error: LlmError::InvalidRequest { .. }
                    }
                })
            ));
            server.abort();
        }
        let items = [
            json!({"type":"web_search_call","id":"ws_1","status":"completed","future_metadata":{"value":true}}),
            message("msg_1", "final_answer", "Answer"),
        ];
        let blocks = lower_items(&items, None).unwrap();
        assert!(
            matches!(&blocks[0], AssistantBlock::ServerToolContent { kind: ServerToolKind::WebSearch, content, .. } if content == &items[0])
        );
        for extra in [
            json!({"async":true}),
            json!({"namespace":"tools"}),
            json!({"caller":{"type":"program","id":"p"}}),
        ] {
            let mut call =
                json!({"type":"function_call","call_id":"c","name":"tool","arguments":"{}"});
            call.as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            assert!(matches!(
                lower_items([&call], None),
                Err(LlmError::InvalidRequest { .. })
            ));
        }
    }
}
