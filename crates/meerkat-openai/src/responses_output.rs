//! Mechanical lowering of complete Responses output into the canonical block
//! vocabulary. Terminal items may contain continuity data absent from deltas.

use meerkat_core::{AssistantBlock, ProviderMeta, ServerToolKind};
use meerkat_llm_core::LlmError;
use serde_json::Value;
use std::collections::{HashMap, HashSet};

use crate::client::{
    message_citations_content, openai_response_meta, parse_tool_call_arguments,
    web_search_message_annotations,
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
                let mut text_parts = 0;
                for part in parts {
                    match required(part, "type")? {
                        "output_text" => {
                            text_parts += 1;
                            text.push_str(required(part, "text")?);
                            if let Some(citations) = part
                                .get("annotations")
                                .and_then(web_search_message_annotations)
                            {
                                annotations.extend(citations);
                            }
                        }
                        // A refusal is never answer text: it ends the response
                        // as a refusal and travels as the stop's provider detail.
                        "refusal" => {
                            required(part, "refusal")?;
                        }
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
                        content: message_citations_content(annotations, text_parts),
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

/// The item kinds whose completed identity GitHub Copilot's Responses stream
/// re-issues in the terminal response.
pub(crate) const COPILOT_ALIASED_KINDS: [&str; 2] = ["message", "reasoning"];

/// Whether two items of `kind` are the same item for Copilot's
/// re-identification: for a message, the same role, phase and full content;
/// for reasoning, every field except the top-level id and the opaque
/// `encrypted_content` (which must be a string on both when present), so the
/// terminal id and ciphertext pair is taken as provider-owned. On the items
/// it accepts this is an equivalence; a reasoning item whose
/// `encrypted_content` is not a string matches nothing, not even itself.
fn copilot_same_item(kind: &str, left: &Value, right: &Value) -> bool {
    match kind {
        "message" => {
            let semantics = |item: &Value| {
                (
                    item.get("role").cloned(),
                    item.get("phase").cloned(),
                    item.get("content").cloned(),
                )
            };
            semantics(left) == semantics(right)
        }
        "reasoning" => {
            let opaque_ok = |item: &Value| {
                item.get("encrypted_content")
                    .is_none_or(|ciphertext| ciphertext.is_string())
            };
            let visible = |item: &Value| {
                let mut item = item.clone();
                if let Some(object) = item.as_object_mut() {
                    object.remove("id");
                    object.remove("encrypted_content");
                }
                item
            };
            opaque_ok(left) && opaque_ok(right) && visible(left) == visible(right)
        }
        _ => false,
    }
}

/// Copilot's completed-to-terminal identity aliases, one map per aliased kind.
#[derive(Debug, Default)]
pub(crate) struct CopilotAliases {
    by_kind: HashMap<&'static str, HashMap<String, String>>,
}

impl CopilotAliases {
    /// The terminal identity of a completed item of `kind` with `id`: its
    /// proven alias, or its own id when the terminal output kept it.
    pub(crate) fn terminal_id<'a>(&'a self, kind: &str, id: &'a str) -> &'a str {
        self.by_kind
            .get(kind)
            .and_then(|aliases| aliases.get(id))
            .map_or(id, String::as_str)
    }

    /// `item` with its completed id replaced by its terminal alias.
    pub(crate) fn apply(&self, item: &Value) -> Value {
        let mut item = item.clone();
        let Some(kind) = item.get("type").and_then(Value::as_str).map(str::to_owned) else {
            return item;
        };
        if let Some(alias) = item
            .get("id")
            .and_then(Value::as_str)
            .and_then(|id| self.by_kind.get(kind.as_str())?.get(id))
            .cloned()
            && let Some(object) = item.as_object_mut()
        {
            object.insert("id".to_owned(), Value::String(alias));
        }
        item
    }
}

/// GitHub Copilot's Responses stream re-identifies completed messages and
/// reasoning: an item completed by `response.output_item.done` as `F` can be
/// carried by the terminal response as `G`. For that route only, return the
/// aliases `F -> G` for the re-identified completed items (those whose id
/// the terminal output lacks), each to a terminal item of the same type that
/// no completed item claims and that [`copilot_same_item`] proves the same
/// item. Identity conflicts are refused before anything is aliased: a
/// completed id reused by two completed items, and a terminal id carried by
/// two terminal items.
///
/// One response can hold several items of a type that differ only in their
/// ids and ciphertext (two reasoning items without a summary, say), so items
/// are matched by group: the group of a re-identified completed item is the
/// k re-identified completed items and the m unclaimed terminal items that
/// are the same item as it ([`copilot_same_item`] is an equivalence on the
/// items it accepts). A group is aliased only when it is complete on both
/// sides, k = m: walking the completed items in their given order, each
/// takes the first terminal item of its group, in output order, that no
/// earlier one took. Pairing inside a complete group is immaterial to the
/// verdicts and the committed blocks: the blocks are lowered from the
/// terminal items, each keeping its own id and ciphertext, and every pairing
/// covers the same terminal ids, which carry the same visible content, so
/// the coverage checks in the client give the same verdicts. The live events
/// match too, unless a streamed id of one item is the terminal id of
/// another, where the client's one-pass streamed-id reconciliation depends
/// on the slot order. Array positions are never used to decide which items
/// are the same; inside a complete group they only pick one of these
/// pairings.
///
/// Otherwise a re-identified completed item is refused, by its group:
/// - m = 0: as a re-identification with different content when an unclaimed
///   terminal item of its type exists, otherwise by the ordinary coverage
///   check as omitted;
/// - k < m: as having no one-to-one match, since nothing tells which
///   terminal item is the extra one;
/// - k > m: as having no one-to-one match, at the first item of the group
///   left without a terminal item.
///
/// Function calls (keyed by `call_id`) and other references are never
/// aliased, and the terminal identity is kept.
pub(crate) fn copilot_item_aliases<'a>(
    output: &[Value],
    completed: impl IntoIterator<Item = &'a Value>,
) -> Result<CopilotAliases, LlmError> {
    let completed: Vec<&Value> = completed.into_iter().collect();
    let mut aliases = CopilotAliases::default();
    for kind in COPILOT_ALIASED_KINDS {
        let of_kind = |item: &&Value| item.get("type").and_then(Value::as_str) == Some(kind);
        let completed: Vec<&Value> = completed.iter().copied().filter(of_kind).collect();
        let terminal: Vec<&Value> = output.iter().filter(of_kind).collect();
        let mut completed_ids = HashSet::new();
        for item in &completed {
            if let Some(id) = item.get("id").and_then(Value::as_str)
                && !completed_ids.insert(id)
            {
                return Err(malformed(format!(
                    "completed Responses {kind} id '{id}' is reused by two completed items"
                )));
            }
        }
        let mut terminal_ids = HashSet::new();
        for item in &terminal {
            if let Some(id) = item.get("id").and_then(Value::as_str)
                && !terminal_ids.insert(id)
            {
                return Err(malformed(format!(
                    "terminal Responses output repeats {kind} id '{id}'"
                )));
            }
        }
        let unclaimed: Vec<&Value> = terminal
            .iter()
            .copied()
            .filter(|item| {
                item.get("id")
                    .and_then(Value::as_str)
                    .is_some_and(|id| !completed_ids.contains(id))
            })
            .collect();
        let reidentified: Vec<(&str, &Value)> = completed
            .iter()
            .copied()
            .filter_map(|item| item.get("id").and_then(Value::as_str).map(|id| (id, item)))
            .filter(|(id, _)| !terminal_ids.contains(id))
            .collect();
        let mut kind_aliases = HashMap::new();
        let mut aliased_targets: HashSet<&str> = HashSet::new();
        for &(done_id, done) in &reidentified {
            // Its group: the unclaimed terminal items that are the same item,
            // in output order, and the re-identified completed items that are.
            let group_terminal: Vec<&str> = unclaimed
                .iter()
                .filter(|candidate| copilot_same_item(kind, done, candidate))
                .filter_map(|candidate| candidate.get("id").and_then(Value::as_str))
                .collect();
            if group_terminal.is_empty() {
                if unclaimed.is_empty() {
                    continue;
                }
                return Err(malformed(format!(
                    "terminal Responses output re-identified completed {kind} item '{done_id}' with different content"
                )));
            }
            let group_completed = reidentified
                .iter()
                .filter(|(_, other)| copilot_same_item(kind, done, other))
                .count();
            // Only a complete group pairs: with more terminal items than
            // completed ones nothing tells which one is extra, and with fewer
            // the last completed items find none left.
            let target = if group_completed < group_terminal.len() {
                None
            } else {
                group_terminal
                    .iter()
                    .copied()
                    .find(|id| !aliased_targets.contains(id))
            };
            let Some(target) = target else {
                return Err(malformed(format!(
                    "terminal Responses output has no one-to-one match for re-identified completed {kind} item '{done_id}' ({group_completed} re-identified completed and {} unclaimed terminal items with the same content)",
                    group_terminal.len()
                )));
            };
            aliased_targets.insert(target);
            kind_aliases.insert(done_id.to_owned(), target.to_owned());
        }
        aliases.by_kind.insert(kind, kind_aliases);
    }
    Ok(aliases)
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

    fn wire_message_id(uuid: &str) -> meerkat_core::AssistantMessageId {
        serde_json::from_value(json!(uuid)).expect("assistant message id")
    }

    /// Responses stub that records every request body it receives.
    async fn capturing_stub(
        events: Vec<Value>,
    ) -> (
        OpenAiClient,
        Arc<std::sync::Mutex<Vec<Value>>>,
        tokio::task::JoinHandle<()>,
    ) {
        let mut payload = String::new();
        for event in events {
            write!(payload, "data: {event}\n\n").expect("formatting into a String cannot fail");
        }
        type Captured = (String, Arc<std::sync::Mutex<Vec<Value>>>);
        async fn respond(
            State((payload, bodies)): State<Captured>,
            axum::Json(body): axum::Json<Value>,
        ) -> impl axum::response::IntoResponse {
            bodies.lock().unwrap().push(body);
            ([("content-type", "text/event-stream")], payload)
        }
        let bodies = Arc::new(std::sync::Mutex::new(Vec::new()));
        let app = Router::new()
            .route("/v1/responses", post(respond))
            .with_state((payload, Arc::clone(&bodies)));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (
            OpenAiClient::new_with_base_url("unused".into(), url),
            bodies,
            server,
        )
    }

    #[tokio::test]
    async fn adapter_attempt_stamps_responses_live_events_and_keeps_request_bytes() {
        let reasoning = json!({"type":"reasoning","id":"rs_1","summary":[{"type":"summary_text","text":"Plan"}]});
        let search = json!({"type":"web_search_call","id":"ws_1","status":"completed","action":{"type":"search","query":"meerkat"}});
        let answer = message("msg_1", "final_answer", "Found it.");
        let events = vec![
            json!({"type":"response.created","response":{"id":"resp_1"}}),
            json!({"type":"response.reasoning_summary_text.delta","item_id":"rs_1","delta":"Plan"}),
            json!({"type":"response.reasoning_summary.done","item":reasoning}),
            json!({"type":"response.web_search_call.searching","item_id":"ws_1","output_index":1}),
            json!({"type":"response.output_item.added","output_index":2,"item":answer}),
            json!({"type":"response.output_text.delta","item_id":"msg_1","delta":"Found it."}),
            json!({"type":"response.completed","response":{"id":"resp_1","status":"completed",
                "output":[reasoning, search, answer],
                "usage":{"input_tokens":5,"output_tokens":3,"total_tokens":8}}}),
        ];
        let (client, bodies, server) = capturing_stub(events).await;
        let (tx, mut rx) = tokio::sync::mpsc::channel(64);
        let adapter = Arc::new(LlmClientAdapter::with_event_channel(
            Arc::new(client),
            "gpt-5.5".into(),
            tx,
        ));
        let history_id = wire_message_id("0190f5c2-4a1e-7c3d-8e2f-00000000bb01");
        let turn_id = wire_message_id("0190f5c2-4a1e-7c3d-8e2f-00000000bb02");
        let mut prior = meerkat_core::BlockAssistantMessage::new(
            vec![meerkat_core::AssistantBlock::Text {
                text: "prior answer".into(),
                meta: None,
            }],
            meerkat_core::StopReason::EndTurn,
        );
        prior.assistant_message_id = Some(history_id);
        let with_ids = vec![
            Message::User(UserMessage::text("first")),
            Message::BlockAssistant(prior.clone()),
            Message::User(UserMessage::text("again")),
        ];
        prior.assistant_message_id = None;
        let without_ids = vec![
            with_ids[0].clone(),
            Message::BlockAssistant(prior),
            with_ids[2].clone(),
        ];

        for messages in [with_ids, without_ids] {
            let attempt = Arc::clone(&adapter)
                .prepare_request_attempt(Arc::new(messages), Arc::from([]), 1000, None, None)
                .unwrap();
            attempt.stream_response(turn_id).await.unwrap();
        }
        server.abort();

        let published = std::iter::from_fn(|| rx.try_recv().ok()).collect::<Vec<_>>();
        let kinds = published
            .iter()
            .map(meerkat_core::agent_event_type)
            .collect::<std::collections::BTreeSet<_>>();
        for kind in [
            "reasoning_delta",
            "reasoning_complete",
            "server_tool_content",
            "text_delta",
        ] {
            assert!(kinds.contains(kind), "{kind} published: {kinds:?}");
        }
        assert!(
            published
                .iter()
                .all(|event| event.assistant_message_id() == Some(turn_id)),
            "every live event names the provider turn's message"
        );
        let bodies = bodies.lock().unwrap();
        assert_eq!(bodies.len(), 2);
        assert_eq!(
            bodies[0], bodies[1],
            "assistant message ids never change the provider request"
        );
        assert!(!bodies[0].to_string().contains(&history_id.to_string()));
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
            if let meerkat_core::AgentEvent::TextDelta { delta, .. } = event {
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

    /// A Copilot-shaped Responses stream for one message at wire output
    /// index 0 that Copilot re-identifies per event: added `ids[0]`,
    /// content_part.added `ids[1]`, delta `ids[2]`, output_text.done
    /// `ids[3]`, content_part.done `ids[4]`, item.done `ids[5]`, terminal
    /// `ids[6]`, all with the same role, phase and content.
    fn copilot_message_lifecycle(ids: [&str; 7]) -> Vec<Value> {
        let at = |id: &str| message(id, "final_answer", "Done.");
        let part = json!({"type":"output_text","text":"Done.","annotations":[]});
        vec![
            json!({"type":"response.created","response":{"id":"resp_1"}}),
            json!({"type":"response.output_item.added","output_index":0,"item":at(ids[0])}),
            json!({"type":"response.content_part.added","item_id":ids[1],"output_index":0,"content_index":0,"part":part}),
            json!({"type":"response.output_text.delta","item_id":ids[2],"output_index":0,"content_index":0,"delta":"Done."}),
            json!({"type":"response.output_text.done","item_id":ids[3],"output_index":0,"content_index":0,"text":"Done."}),
            json!({"type":"response.content_part.done","item_id":ids[4],"output_index":0,"content_index":0,"part":part}),
            json!({"type":"response.output_item.done","output_index":0,"item":at(ids[5])}),
            json!({"type":"response.completed","response":{"id":"resp_1","status":"completed",
                "output":[at(ids[6])],
                "usage":{"input_tokens":3,"output_tokens":2,"total_tokens":5}}}),
        ]
    }

    /// The first reported shape: one id for the whole lifecycle, renamed only
    /// in the terminal response.
    fn copilot_reidentified_events() -> Vec<Value> {
        copilot_message_lifecycle([
            "message-f",
            "message-f",
            "message-f",
            "message-f",
            "message-f",
            "message-f",
            "message-g",
        ])
    }

    async fn copilot_stub(events: Vec<Value>) -> (OpenAiClient, tokio::task::JoinHandle<()>) {
        let (client, server) = stub(events).await;
        (client.with_copilot_responses_item_identity(), server)
    }

    /// The Copilot route accepts a message Copilot re-identified on every
    /// event of its lifecycle, or only in the terminal response, or with the
    /// terminal id equal to the completed one but a distinct delta id: one
    /// visible text sequence, and the terminal identity is kept for replay.
    #[tokio::test]
    async fn copilot_reidentified_message_keeps_its_terminal_identity() {
        let shapes: [(&str, [&str; 7]); 3] = [
            ("terminal rename", ["F", "F", "F", "F", "F", "F", "G"]),
            ("every event", ["A", "B", "C", "D", "E", "F", "G"]),
            (
                "terminal kept, delta distinct",
                ["A", "B", "D", "D", "E", "F", "F"],
            ),
        ];
        for (shape, ids, retransmit) in shapes
            .into_iter()
            .map(|(shape, ids)| (shape, ids, false))
            .chain([(
                "identical added retransmission",
                ["A", "B", "C", "D", "E", "F", "G"],
                true,
            )])
        {
            let mut events = copilot_message_lifecycle(ids);
            if retransmit {
                let added = events[1].clone();
                events.insert(2, added);
            }
            let (client, server) = copilot_stub(events).await;
            let (tx, mut rx) = tokio::sync::mpsc::channel(32);
            let result =
                LlmClientAdapter::with_event_channel(Arc::new(client), "gpt-5.5".into(), tx)
                    .stream_response(&[], &[], 1000, None, None)
                    .await;
            server.abort();
            let Ok(result) = result else {
                panic!("{shape}: the re-identified terminal message is the completed message");
            };
            let terminal = message(ids[6], "final_answer", "Done.");
            assert_eq!(
                result.blocks(),
                lower_items([&terminal], Some("resp_1")).unwrap(),
                "{shape}: the terminal identity is kept for replay"
            );
            let mut visible = String::new();
            while let Ok(event) = rx.try_recv() {
                if let meerkat_core::AgentEvent::TextDelta { delta, .. } = event {
                    visible.push_str(&delta);
                }
            }
            assert_eq!(visible, "Done.", "{shape}: one visible text sequence");
        }
    }

    /// Plain OpenAI keeps the strict identity check: the same re-identified
    /// stream is refused as an omission.
    #[tokio::test]
    async fn plain_openai_refuses_a_reidentified_terminal_item() {
        let (client, server) = stub(copilot_reidentified_events()).await;
        let Err(error) = LlmClientAdapter::new(Arc::new(client), "gpt-5.5".into())
            .stream_response(&[], &[], 1000, None, None)
            .await
        else {
            panic!("plain OpenAI refuses re-identified items");
        };
        server.abort();
        assert!(
            error
                .to_string()
                .contains("omitted completed message item 'message-f'"),
            "{error}"
        );
    }

    /// A Copilot reasoning item re-identified (added `A`, done `B`, terminal
    /// `I`) with a re-encoded ciphertext and identical visible fields is
    /// accepted; the terminal id and ciphertext stay together for replay.
    #[tokio::test]
    async fn copilot_reencoded_reasoning_keeps_the_terminal_pair() {
        let reasoning = |id: &str, ciphertext: &str| json!({"type":"reasoning","id":id,"summary":[{"type":"summary_text","text":"Plan"}],"encrypted_content":ciphertext});
        let answer = message("msg_1", "final_answer", "Done.");
        let terminal = reasoning("I", "terminal-cipher");
        let events = vec![
            json!({"type":"response.created","response":{"id":"resp_1"}}),
            json!({"type":"response.output_item.added","output_index":0,"item":reasoning("A", "added-cipher")}),
            json!({"type":"response.reasoning_summary.done","output_index":0,"item":reasoning("X", "summary-cipher")}),
            json!({"type":"response.output_item.done","output_index":0,"item":reasoning("B", "done-cipher")}),
            json!({"type":"response.output_item.added","output_index":1,"item":answer}),
            json!({"type":"response.output_text.delta","item_id":"msg_1","output_index":1,"delta":"Done."}),
            json!({"type":"response.output_item.done","output_index":1,"item":answer}),
            json!({"type":"response.completed","response":{"id":"resp_1","status":"completed",
                "output":[terminal, answer],
                "usage":{"input_tokens":3,"output_tokens":2,"total_tokens":5}}}),
        ];
        let (client, server) = copilot_stub(events).await;
        let result = LlmClientAdapter::new(Arc::new(client), "gpt-5.5".into())
            .stream_response(&[], &[], 1000, None, None)
            .await;
        server.abort();
        let Ok(result) = result else {
            panic!("the re-encoded terminal reasoning is the completed reasoning");
        };
        assert_eq!(
            result.blocks(),
            lower_items([&terminal, &answer], Some("resp_1")).unwrap(),
            "the terminal reasoning id and ciphertext are lowered together"
        );
        let replay_client = OpenAiClient::new("unused".into());
        let projected = replay_client
            .project_replay_messages(&[Message::BlockAssistant(result.into_message())])
            .unwrap();
        let body = replay_client
            .build_request_body(&LlmRequest::new("gpt-5.5", projected))
            .unwrap();
        let input = body["input"].as_array().unwrap();
        assert_eq!(input[0]["id"], "I");
        assert_eq!(input[0]["encrypted_content"], "terminal-cipher");
    }

    /// A Copilot response of reasoning items and then an answer: each
    /// `(id, summary)` of `completed` completes under that id at its own
    /// output index, and the terminal response carries `terminal` in that
    /// order, then the answer. Every reasoning item has a ciphertext of its
    /// own, `cipher-<id>`. With `streamed`, each completed lifecycle is first
    /// added under its own canonical id and completes its summary in the
    /// stream. Returns the events and the terminal output.
    fn copilot_reasoning_response(
        completed: &[(&str, Option<&str>)],
        terminal: &[(&str, Option<&str>)],
        streamed: bool,
    ) -> (Vec<Value>, Vec<Value>) {
        let reasoning = |id: &str, summary: Option<&str>| {
            let summary = match summary {
                Some(text) => json!([{"type":"summary_text","text":text}]),
                None => json!([]),
            };
            json!({"type":"reasoning","id":id,"summary":summary,"encrypted_content":format!("cipher-{id}")})
        };
        let mut events = vec![json!({"type":"response.created","response":{"id":"resp_1"}})];
        for (index, &(id, summary)) in completed.iter().enumerate() {
            let event =
                |kind: &str, item: Value| json!({"type":kind,"output_index":index,"item":item});
            if streamed {
                events.push(event(
                    "response.output_item.added",
                    reasoning(&format!("A{index}"), summary),
                ));
                if let Some(text) = summary {
                    events.push(json!({"type":"response.reasoning_summary_text.delta","output_index":index,"delta":text}));
                }
                events.push(event(
                    "response.reasoning_summary.done",
                    reasoning(&format!("S{index}"), summary),
                ));
            }
            events.push(event("response.output_item.done", reasoning(id, summary)));
        }
        let answer = message("msg_1", "final_answer", "Done.");
        let output: Vec<Value> = terminal
            .iter()
            .map(|&(id, summary)| reasoning(id, summary))
            .chain([answer.clone()])
            .collect();
        let index = completed.len();
        events.extend([
            json!({"type":"response.output_item.added","output_index":index,"item":answer}),
            json!({"type":"response.output_text.delta","item_id":"msg_1","output_index":index,"delta":"Done."}),
            json!({"type":"response.output_item.done","output_index":index,"item":answer}),
            json!({"type":"response.completed","response":{"id":"resp_1","status":"completed",
                "output":output,
                "usage":{"input_tokens":3,"output_tokens":2,"total_tokens":5}}}),
        ]);
        (events, output)
    }

    /// Asserts that `assistant` replays exactly the reasoning items
    /// `expected`, as `(id, encrypted_content)` pairs in replay order.
    fn assert_replayed_reasoning(
        assistant: meerkat_core::BlockAssistantMessage,
        expected: &[(&str, &str)],
        case: &str,
    ) {
        let client = OpenAiClient::new("unused".into());
        let projected = client
            .project_replay_messages(&[Message::BlockAssistant(assistant)])
            .unwrap();
        let body = client
            .build_request_body(&LlmRequest::new("gpt-5.5", projected))
            .unwrap();
        let replayed: Vec<(&str, &str)> = body["input"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|item| item["type"] == "reasoning")
            .map(|item| {
                (
                    item["id"].as_str().unwrap(),
                    item["encrypted_content"].as_str().unwrap(),
                )
            })
            .collect();
        assert_eq!(
            replayed, expected,
            "{case}: each terminal reasoning id replays with its own ciphertext"
        );
    }

    /// Two reasoning items with the same visible fields (no summary, or the
    /// same summary), each re-identified with fresh ciphertexts, are one
    /// group with as many completed as terminal items: accepted, the blocks
    /// are the terminal items, and each terminal id replays with its own
    /// ciphertext, in terminal order.
    #[tokio::test]
    async fn copilot_identical_reasoning_items_are_matched_as_a_group() {
        for (shape, summary) in [
            ("empty summaries", None),
            ("identical summaries", Some("Plan")),
        ] {
            let (events, output) = copilot_reasoning_response(
                &[("B", summary), ("C", summary)],
                &[("I", summary), ("J", summary)],
                false,
            );
            let (client, server) = copilot_stub(events).await;
            let result = LlmClientAdapter::new(Arc::new(client), "gpt-5.5".into())
                .stream_response(&[], &[], 1000, None, None)
                .await;
            server.abort();
            let result = result.unwrap_or_else(|error| {
                panic!("{shape}: the terminal pair is the completed pair: {error}")
            });
            assert_eq!(
                result.blocks(),
                lower_items(&output, Some("resp_1")).unwrap(),
                "{shape}: the blocks are the terminal items"
            );
            assert_replayed_reasoning(
                result.into_message(),
                &[("I", "cipher-I"), ("J", "cipher-J")],
                shape,
            );
        }
    }

    /// The same groups with streamed reasoning: each lifecycle is added under
    /// its own canonical id and completes its summary in the stream, so the
    /// streamed ids reconcile with the group's terminal ids. Accepted, and
    /// each reasoning item completes once in the live events.
    #[tokio::test]
    async fn copilot_identical_reasoning_group_reconciles_its_streamed_ids() {
        for (shape, summary) in [
            ("empty summaries", None),
            ("identical summaries", Some("Plan")),
        ] {
            let (events, output) = copilot_reasoning_response(
                &[("B", summary), ("C", summary)],
                &[("I", summary), ("J", summary)],
                true,
            );
            let (client, server) = copilot_stub(events).await;
            let (tx, mut rx) = tokio::sync::mpsc::channel(32);
            let result =
                LlmClientAdapter::with_event_channel(Arc::new(client), "gpt-5.5".into(), tx)
                    .stream_response(&[], &[], 1000, None, None)
                    .await;
            server.abort();
            let result = result.unwrap_or_else(|error| {
                panic!("{shape}: the streamed group is the terminal group: {error}")
            });
            assert_eq!(
                result.blocks(),
                lower_items(&output, Some("resp_1")).unwrap(),
                "{shape}: the blocks are the terminal items"
            );
            let completions = std::iter::from_fn(|| rx.try_recv().ok())
                .filter(|event| matches!(event, meerkat_core::AgentEvent::ReasoningComplete { .. }))
                .count();
            assert_eq!(
                completions, 2,
                "{shape}: each reasoning item completes once"
            );
        }
    }

    /// Two identical messages, each re-identified on every event of its
    /// lifecycle, are one group: accepted, the terminal identities are kept,
    /// and each visible text streams once.
    #[tokio::test]
    async fn copilot_identical_messages_are_matched_as_a_group() {
        let at = |id: &str| message(id, "final_answer", "Done.");
        let output = [at("G0"), at("G1")];
        let events = vec![
            json!({"type":"response.created","response":{"id":"resp_1"}}),
            json!({"type":"response.output_item.added","output_index":0,"item":at("A0")}),
            json!({"type":"response.output_text.delta","item_id":"D0","output_index":0,"delta":"Done."}),
            json!({"type":"response.output_item.done","output_index":0,"item":at("F0")}),
            json!({"type":"response.output_item.added","output_index":1,"item":at("A1")}),
            json!({"type":"response.output_text.delta","item_id":"D1","output_index":1,"delta":"Done."}),
            json!({"type":"response.output_item.done","output_index":1,"item":at("F1")}),
            json!({"type":"response.completed","response":{"id":"resp_1","status":"completed",
                "output":output,
                "usage":{"input_tokens":3,"output_tokens":2,"total_tokens":5}}}),
        ];
        let (client, server) = copilot_stub(events).await;
        let (tx, mut rx) = tokio::sync::mpsc::channel(32);
        let result = LlmClientAdapter::with_event_channel(Arc::new(client), "gpt-5.5".into(), tx)
            .stream_response(&[], &[], 1000, None, None)
            .await;
        server.abort();
        let result = result.unwrap_or_else(|error| {
            panic!("the terminal messages are the completed messages: {error}")
        });
        assert_eq!(
            result.blocks(),
            lower_items(&output, Some("resp_1")).unwrap(),
            "the terminal identities are kept for replay"
        );
        let mut visible = String::new();
        while let Ok(event) = rx.try_recv() {
            if let meerkat_core::AgentEvent::TextDelta { delta, .. } = event {
                visible.push_str(&delta);
            }
        }
        assert_eq!(visible, "Done.Done.", "each visible text streams once");
    }

    /// A group beside a distinct reasoning item in one response: the pair
    /// without a summary is matched as a group and the summarized item on its
    /// own, streamed or not.
    #[tokio::test]
    async fn copilot_reasoning_group_beside_a_distinct_item_is_accepted() {
        for streamed in [false, true] {
            let (events, output) = copilot_reasoning_response(
                &[("B", None), ("E", Some("Plan")), ("C", None)],
                &[("I", None), ("K", Some("Plan")), ("J", None)],
                streamed,
            );
            let (client, server) = copilot_stub(events).await;
            let result = LlmClientAdapter::new(Arc::new(client), "gpt-5.5".into())
                .stream_response(&[], &[], 1000, None, None)
                .await;
            server.abort();
            let result = result.unwrap_or_else(|error| {
                panic!("streamed {streamed}: the group and the distinct item both match: {error}")
            });
            assert_eq!(
                result.blocks(),
                lower_items(&output, Some("resp_1")).unwrap(),
                "streamed {streamed}: the blocks are the terminal items"
            );
        }
    }

    /// Distinguishable reasoning items match by content, never by position:
    /// with the two re-identified items listed in the other order by the
    /// terminal output, each completed item is aliased to the terminal item
    /// with its content, and the blocks follow the terminal order with each
    /// terminal id and ciphertext kept together.
    #[tokio::test]
    async fn copilot_reordered_terminal_items_match_by_content() {
        let (events, output) = copilot_reasoning_response(
            &[("B", Some("A plan")), ("C", Some("B plan"))],
            &[("J", Some("B plan")), ("I", Some("A plan"))],
            false,
        );
        let completed = events
            .iter()
            .filter(|event| event["type"] == "response.output_item.done")
            .map(|event| &event["item"]);
        let aliases = copilot_item_aliases(&output, completed).unwrap();
        assert_eq!(aliases.terminal_id("reasoning", "B"), "I", "'A plan' is I");
        assert_eq!(aliases.terminal_id("reasoning", "C"), "J", "'B plan' is J");
        let (client, server) = copilot_stub(events).await;
        let result = LlmClientAdapter::new(Arc::new(client), "gpt-5.5".into())
            .stream_response(&[], &[], 1000, None, None)
            .await;
        server.abort();
        let result = result
            .unwrap_or_else(|error| panic!("each completed item has its terminal item: {error}"));
        assert_eq!(
            result.blocks(),
            lower_items(&output, Some("resp_1")).unwrap(),
            "the blocks follow the terminal order"
        );
        assert_replayed_reasoning(
            result.into_message(),
            &[("J", "cipher-J"), ("I", "cipher-I")],
            "reordered",
        );
    }

    /// A group with a kept member: the completed item whose id the terminal
    /// output keeps is neither aliased nor counted, so its re-identified twin
    /// is aliased to the one unclaimed terminal item, and the response is
    /// accepted.
    #[tokio::test]
    async fn copilot_group_with_a_kept_member_aliases_only_the_reidentified_one() {
        let (events, output) = copilot_reasoning_response(
            &[("B", None), ("C", None)],
            &[("B", None), ("J", None)],
            false,
        );
        let completed = events
            .iter()
            .filter(|event| event["type"] == "response.output_item.done")
            .map(|event| &event["item"]);
        let aliases = copilot_item_aliases(&output, completed).unwrap();
        assert_eq!(aliases.terminal_id("reasoning", "B"), "B", "B is kept");
        assert_eq!(aliases.terminal_id("reasoning", "C"), "J", "C is now J");
        let (client, server) = copilot_stub(events).await;
        let result = LlmClientAdapter::new(Arc::new(client), "gpt-5.5".into())
            .stream_response(&[], &[], 1000, None, None)
            .await;
        server.abort();
        let result =
            result.unwrap_or_else(|error| panic!("the kept item and its twin both match: {error}"));
        assert_eq!(
            result.blocks(),
            lower_items(&output, Some("resp_1")).unwrap(),
            "the blocks are the terminal items"
        );
        assert_replayed_reasoning(
            result.into_message(),
            &[("B", "cipher-B"), ("J", "cipher-J")],
            "kept member",
        );
    }

    /// Copilot rotates a function call's item id on every event while its
    /// call_id stays: the call is identified by call_id, with no alias.
    #[tokio::test]
    async fn copilot_rotating_function_item_ids_keep_the_call_id() {
        let call = |id: &str| json!({"type":"function_call","id":id,"call_id":"call_1","name":"lookup","arguments":"{\"q\":5}","status":"completed"});
        let mut added = call("id1");
        added["arguments"] = json!("");
        let events = vec![
            json!({"type":"response.created","response":{"id":"resp_1"}}),
            json!({"type":"response.output_item.added","output_index":0,"item":added}),
            json!({"type":"response.function_call_arguments.delta","item_id":"id3","output_index":0,"delta":"{\"q\":"}),
            json!({"type":"response.function_call_arguments.delta","item_id":"id4","output_index":0,"delta":"5}"}),
            json!({"type":"response.function_call_arguments.done","item_id":"id11","output_index":0,"arguments":"{\"q\":5}"}),
            json!({"type":"response.output_item.done","output_index":0,"item":call("id12")}),
            json!({"type":"response.completed","response":{"id":"resp_1","status":"completed",
                "output":[call("id14")],
                "usage":{"input_tokens":3,"output_tokens":2,"total_tokens":5}}}),
        ];
        let (client, server) = copilot_stub(events).await;
        let result = LlmClientAdapter::new(Arc::new(client), "gpt-5.5".into())
            .stream_response(&[], &[], 1000, None, None)
            .await;
        server.abort();
        let Ok(result) = result else {
            panic!("rotating function item ids keep the stable call_id");
        };
        let calls: Vec<_> = result
            .blocks()
            .iter()
            .filter_map(|block| match block {
                AssistantBlock::ToolUse { id, name, args, .. } => {
                    Some((id.clone(), name.clone(), args.get().to_owned()))
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            calls,
            vec![(
                "call_1".to_owned(),
                "lookup".to_owned(),
                "{\"q\":5}".to_owned()
            )]
        );
    }

    /// On the Copilot route nothing but a proven one-to-one same item is
    /// aliased: each unproven or conflicting identity still fails with its
    /// own diagnosis.
    #[tokio::test]
    async fn copilot_unproven_reidentifications_still_fail() {
        let done = message("message-f", "final_answer", "Done.");
        let completed = |output: Value| {
            json!({"type":"response.completed","response":{"id":"resp_1","status":"completed",
                "output":output,"usage":{"input_tokens":3,"output_tokens":2,"total_tokens":5}}})
        };
        let done_at = |index: u64, item: &Value| json!({"type":"response.output_item.done","output_index":index,"item":item});
        let reasoning = |id: &str, text: &str, ciphertext: &str| json!({"type":"reasoning","id":id,"summary":[{"type":"summary_text","text":text}],"encrypted_content":ciphertext});
        let call_f = json!({"type":"function_call","id":"fc_f","call_id":"call_f","name":"lookup","arguments":"{}","status":"completed"});
        let mut call_g = call_f.clone();
        call_g["call_id"] = json!("call_g");
        let cases = [
            (
                "different content",
                vec![
                    done_at(0, &done),
                    completed(json!([message("message-g", "final_answer", "Other.")])),
                ],
                "re-identified completed message item 'message-f' with different content",
            ),
            (
                "omitted",
                vec![
                    done_at(0, &done),
                    completed(json!([{"type":"reasoning","id":"rs_g","summary":[]}])),
                ],
                "omitted completed message item 'message-f'",
            ),
            (
                "ambiguous",
                vec![
                    done_at(0, &done),
                    completed(json!([
                        message("message-g", "final_answer", "Done."),
                        message("message-k", "final_answer", "Done.")
                    ])),
                ],
                "no one-to-one match for re-identified completed message item 'message-f'",
            ),
            (
                "conflicting index",
                vec![
                    done_at(0, &done),
                    done_at(0, &message("message-h", "final_answer", "More.")),
                    completed(json!([done])),
                ],
                "conflicting Responses output items completed at output_index 0",
            ),
            (
                "repeated terminal id",
                vec![
                    done_at(0, &done),
                    completed(json!([
                        message("message-g", "final_answer", "Done."),
                        message("message-g", "final_answer", "Other.")
                    ])),
                ],
                "terminal Responses output repeats message id 'message-g'",
            ),
            (
                "reused completed id",
                vec![
                    done_at(0, &done),
                    done_at(1, &message("message-f", "final_answer", "Other.")),
                    completed(json!([
                        message("message-g", "final_answer", "Done."),
                        message("message-h", "final_answer", "Other.")
                    ])),
                ],
                "completed Responses message id 'message-f' is reused by two completed items",
            ),
            (
                "one canonical id for two slots",
                vec![
                    json!({"type":"response.output_item.added","output_index":0,"item":message("A", "final_answer", "Left.")}),
                    json!({"type":"response.output_text.delta","item_id":"D0","output_index":0,"delta":"Left."}),
                    json!({"type":"response.output_item.added","output_index":1,"item":message("A", "final_answer", "Right.")}),
                    json!({"type":"response.output_text.delta","item_id":"D1","output_index":1,"delta":"Right."}),
                    done_at(1, &message("F", "final_answer", "Right.")),
                    completed(json!([message("G", "final_answer", "Right.")])),
                ],
                "Responses output item id 'A' added at output_index 0 and 1",
            ),
            (
                "fresh lifecycle at a completed slot",
                vec![
                    json!({"type":"response.output_item.added","output_index":0,"item":message("A", "final_answer", "Done.")}),
                    done_at(0, &message("F", "final_answer", "Done.")),
                    json!({"type":"response.output_item.added","output_index":0,"item":message("B", "final_answer", "Later.")}),
                    json!({"type":"response.output_text.delta","item_id":"D1","output_index":0,"delta":"Later."}),
                    completed(json!([message("G", "final_answer", "Done.")])),
                ],
                "conflicting Responses output items added at output_index 0",
            ),
            (
                "orphan delta id",
                vec![
                    json!({"type":"response.output_text.delta","item_id":"orphan","output_index":0,"delta":"Done."}),
                    done_at(0, &done),
                    completed(json!([done])),
                ],
                "omitted streamed assistant message 'orphan'",
            ),
            (
                "function call",
                vec![done_at(0, &call_f), completed(json!([call_g]))],
                "omitted completed function_call item 'call_f'",
            ),
            (
                "reasoning visible change",
                vec![
                    done_at(0, &reasoning("B", "Plan", "c1")),
                    completed(json!([reasoning("I", "Other plan", "c2")])),
                ],
                "re-identified completed reasoning item 'B' with different content",
            ),
            (
                "reasoning omitted",
                vec![
                    done_at(0, &reasoning("B", "Plan", "c1")),
                    completed(json!([done])),
                ],
                "omitted completed reasoning item 'B'",
            ),
            (
                "two reasoning slots, one candidate",
                vec![
                    done_at(0, &reasoning("B", "Plan", "c1")),
                    done_at(1, &reasoning("C", "Plan", "c2")),
                    completed(json!([reasoning("I", "Plan", "c3")])),
                ],
                "no one-to-one match for re-identified completed reasoning item 'C'",
            ),
            (
                "indistinguishable reasoning candidates",
                vec![
                    done_at(0, &reasoning("B", "Plan", "c1")),
                    completed(json!([
                        reasoning("I", "Plan", "c2"),
                        reasoning("J", "Plan", "c3")
                    ])),
                ],
                "no one-to-one match for re-identified completed reasoning item 'B'",
            ),
            (
                "group too many terminal",
                vec![
                    done_at(0, &reasoning("B", "Plan", "c1")),
                    done_at(1, &reasoning("C", "Plan", "c2")),
                    completed(json!([
                        reasoning("I", "Plan", "c3"),
                        reasoning("J", "Plan", "c4"),
                        reasoning("K", "Plan", "c5")
                    ])),
                ],
                "no one-to-one match for re-identified completed reasoning item 'B'",
            ),
            (
                "group member changed",
                vec![
                    done_at(0, &reasoning("B", "Plan", "c1")),
                    done_at(1, &reasoning("C", "Plan", "c2")),
                    completed(json!([
                        reasoning("I", "Plan", "c3"),
                        reasoning("J", "Other", "c4")
                    ])),
                ],
                "no one-to-one match for re-identified completed reasoning item 'C'",
            ),
            (
                "group too few terminal",
                vec![
                    done_at(0, &reasoning("B", "Plan", "c1")),
                    done_at(1, &reasoning("C", "Plan", "c2")),
                    done_at(2, &reasoning("D", "Plan", "c3")),
                    completed(json!([
                        reasoning("I", "Plan", "c4"),
                        reasoning("J", "Plan", "c5")
                    ])),
                ],
                "no one-to-one match for re-identified completed reasoning item 'D'",
            ),
            (
                "unrelated reasoning completion",
                vec![
                    json!({"type":"response.reasoning_summary.done","output_index":5,"item":reasoning("X", "Plan", "c0")}),
                    done_at(0, &reasoning("B", "Plan", "c1")),
                    completed(json!([reasoning("B", "Plan", "c1")])),
                ],
                "omitted streamed reasoning 'X'",
            ),
        ];
        for (case, events, expected) in cases {
            let (client, server) = copilot_stub(events).await;
            let Err(error) = LlmClientAdapter::new(Arc::new(client), "gpt-5.5".into())
                .stream_response(&[], &[], 1000, None, None)
                .await
            else {
                panic!("{case}: refused");
            };
            server.abort();
            assert!(error.to_string().contains(expected), "{case}: {error}");
        }
    }

    /// The pruned-terminal negative holds with the Copilot compatibility on:
    /// a different terminal message never stands in for a completed item.
    #[tokio::test]
    async fn copilot_pruned_terminal_output_cannot_discard_atomic_completed_items() {
        for item in [
            message("msg_atomic", "commentary", "Working."),
            json!({"type":"reasoning","id":"rs_atomic","summary":[],"encrypted_content":"atomic-cipher"}),
            json!({"type":"web_search_call","id":"ws_atomic","status":"completed"}),
        ] {
            let (client, server) = copilot_stub(vec![
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
