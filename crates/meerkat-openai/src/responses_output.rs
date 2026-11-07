//! Mechanical lowering of complete Responses output into the canonical block
//! vocabulary. Terminal items may contain continuity data absent from deltas.

use meerkat_core::{AssistantBlock, ProviderMeta, ServerToolKind};
use meerkat_llm_core::LlmError;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet};

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

/// The kind an output item's `type` names on the Copilot route: a web search
/// result belongs to its web search call.
fn copilot_item_kind(item_type: &str) -> &str {
    match item_type {
        "web_search_result" => "web_search_call",
        other => other,
    }
}

/// The kind of item a Responses stream event of `event_type` that names its
/// item by `item_id` belongs to; `None` for events that name no item.
fn copilot_event_kind(event_type: &str) -> Option<&'static str> {
    let event = event_type.strip_prefix("response.")?;
    if event.starts_with("output_text.")
        || event.starts_with("content_part.")
        || event.starts_with("refusal.")
    {
        Some("message")
    } else if event.starts_with("reasoning") {
        Some("reasoning")
    } else if event.starts_with("function_call_arguments.") {
        Some("function_call")
    } else if event.starts_with("web_search_call.") {
        Some("web_search_call")
    } else {
        None
    }
}

/// Whether two items of `kind` that one Copilot slot carried on different
/// events are the same item apart from Copilot's re-identification: for a
/// message, the same role, phase and full content; for a function call, the
/// same call_id, name and arguments; for anything else every field but the
/// id and the slot's `output_index`, and for reasoning also the opaque
/// `encrypted_content`, which Copilot re-encodes.
fn copilot_same_item(kind: &str, left: &Value, right: &Value) -> bool {
    let fields = |item: &Value, names: &[&str]| -> Vec<Option<Value>> {
        names.iter().map(|name| item.get(*name).cloned()).collect()
    };
    match kind {
        "message" => {
            fields(left, &["role", "phase", "content"])
                == fields(right, &["role", "phase", "content"])
        }
        "function_call" => {
            fields(left, &["call_id", "name", "arguments"])
                == fields(right, &["call_id", "name", "arguments"])
        }
        _ => {
            let comparable = |item: &Value| {
                let mut item = item.clone();
                if let Some(object) = item.as_object_mut() {
                    object.remove("id");
                    object.remove("output_index");
                    if kind == "reasoning" {
                        object.remove("encrypted_content");
                    }
                }
                item
            };
            comparable(left) == comparable(right)
        }
    }
}

/// The outcome of a GitHub Copilot Responses stream whose terminal output
/// does not line up with the items the stream announced, or that
/// contradicts itself ([`CopilotSlots`]): the retryable
/// [`LlmError::IncompleteResponse`], so the turn is tried again and nothing
/// of this response is committed.
pub(crate) fn copilot_mismatch(detail: impl std::fmt::Display) -> LlmError {
    LlmError::IncompleteResponse {
        message: format!("GitHub Copilot Responses output does not match its stream: {detail}"),
    }
}

/// A coverage refusal on the Copilot route ([`validate_completed_coverage`],
/// [`validate_coverage`]): the terminal output lacks an item the stream
/// carried, which is the same mismatch as [`copilot_mismatch`].
pub(crate) fn copilot_coverage_mismatch(error: LlmError) -> LlmError {
    match error {
        LlmError::StreamParseError { message } => copilot_mismatch(message),
        other => other,
    }
}

/// The output items of one GitHub Copilot Responses stream, by slot.
///
/// Copilot gives an output item a new id on most events of its lifecycle
/// (added, deltas, done, the terminal response) while the item's wire
/// `output_index` stays, and it re-encodes a reasoning item's ciphertext in
/// the terminal response. On that route an item is its `output_index` slot:
/// the first non-empty id any event naming the slot carries is the item's
/// id, and every later event of the slot carries that id instead of its own.
/// An event that names no slot keeps its ids unless an earlier event carried
/// them at a slot. Function calls keep their `call_id`, and no ciphertext is
/// moved or merged.
///
/// The terminal output is paired with the slots by each item's own
/// `output_index`, else by its array position, and is taken only when it
/// lines up with them: exactly one terminal item for every slot the stream
/// announced and none for another, each of its slot's kind and, where the
/// stream completed the slot, the same item apart from its id
/// ([`copilot_same_item`]). Each terminal item then carries its slot's id and
/// keeps everything else, its ciphertext included. A terminal output that
/// does not line up, and a stream that contradicts itself (another kind at a
/// slot, one id at two slots, a slot added or completed again as a different
/// item), is never repaired: no item is invented, moved or merged, and the
/// response ends with [`copilot_mismatch`].
#[derive(Debug, Default)]
pub(crate) struct CopilotSlots {
    slots: BTreeMap<u64, CopilotSlot>,
    /// Every id an event carried at a slot, to that slot.
    seen: HashMap<String, u64>,
}

#[derive(Debug)]
struct CopilotSlot {
    /// The first id an event naming the slot carried.
    id: String,
    /// The kind of the slot's item, once an event names it.
    kind: Option<String>,
    /// The item the slot's first `response.output_item.added` carried.
    added: Option<Value>,
    /// The item the slot's first `response.output_item.done` carried.
    completed: Option<Value>,
}

impl CopilotSlots {
    /// Give a stream event's ids their slot's identity. `output_index` is
    /// the event's slot, `item` the item it carries (an added or done item,
    /// or a reasoning completion) and `item_id` the item it names.
    pub(crate) fn observe(
        &mut self,
        event_type: &str,
        output_index: Option<u64>,
        mut item: Option<&mut Value>,
        item_id: Option<&mut String>,
    ) -> Result<(), LlmError> {
        let kind = match item
            .as_deref()
            .and_then(|item| item.get("type"))
            .and_then(Value::as_str)
        {
            Some(item_type) => copilot_item_kind(item_type).to_owned(),
            None => match copilot_event_kind(event_type) {
                Some(kind) => kind.to_owned(),
                None => return Ok(()),
            },
        };
        let carried_item_id = item
            .as_deref()
            .and_then(|item| item.get("id"))
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .map(str::to_owned);
        let named_item_id = item_id
            .as_deref()
            .filter(|id| !id.is_empty())
            .map(String::to_owned);
        let Some(index) = output_index else {
            // An event that names no slot: an id an earlier event carried
            // at a slot is that slot's, any other id stays as it is.
            let slot_id = |id: Option<&String>| {
                let index = self.seen.get(id?)?;
                self.slots.get(index).map(|slot| slot.id.clone())
            };
            if let Some(item) = item
                && let Some(slot_id) = slot_id(carried_item_id.as_ref())
            {
                item["id"] = Value::String(slot_id);
            }
            if let Some(item_id) = item_id
                && let Some(slot_id) = slot_id(named_item_id.as_ref())
            {
                *item_id = slot_id;
            }
            return Ok(());
        };
        for id in carried_item_id.iter().chain(named_item_id.iter()) {
            self.place(index, id, &kind)?;
        }
        let Some(slot) = self.slots.get_mut(&index) else {
            // An event that carries no id cannot open a slot.
            return Ok(());
        };
        if let Some(slot_kind) = slot.kind.as_deref()
            && slot_kind != kind
        {
            return Err(copilot_mismatch(format!(
                "a {kind} at output_index {index}, which holds a {slot_kind}"
            )));
        }
        if let Some(item) = item.as_deref_mut()
            && item.get("id").is_some_and(Value::is_string)
        {
            item["id"] = Value::String(slot.id.clone());
        }
        if let Some(item_id) = item_id {
            item_id.clone_from(&slot.id);
        }
        let (first, again) = match event_type {
            "response.output_item.added" => (&mut slot.added, "added"),
            "response.output_item.done" => (&mut slot.completed, "completed"),
            _ => return Ok(()),
        };
        if let Some(item) = item.as_deref() {
            if let Some(previous) = first.as_ref() {
                if !copilot_same_item(&kind, previous, item) {
                    return Err(copilot_mismatch(format!(
                        "output_index {index} was {again} again as a different item"
                    )));
                }
            } else {
                *first = Some(item.clone());
            }
        }
        Ok(())
    }

    /// Record `id`, carried by an event of a `kind` item at `index`: the
    /// first id a slot sees is its identity.
    fn place(&mut self, index: u64, id: &str, kind: &str) -> Result<(), LlmError> {
        if let Some(&other) = self.seen.get(id)
            && other != index
        {
            return Err(copilot_mismatch(format!(
                "id '{id}' appears at output_index {other} and {index}"
            )));
        }
        let slot = self.slots.entry(index).or_insert_with(|| CopilotSlot {
            id: id.to_owned(),
            kind: None,
            added: None,
            completed: None,
        });
        if slot.kind.is_none() {
            slot.kind = Some(kind.to_owned());
        }
        self.seen.entry(id.to_owned()).or_insert(index);
        Ok(())
    }

    /// The first slot the stream announced whose item never completed (no
    /// `response.output_item.done`), if any. A stream that ends without a
    /// terminal response commits only what it completed, so such a slot (a
    /// started tool call included) is a mismatch, never dropped.
    pub(crate) fn first_uncompleted(&self) -> Option<u64> {
        self.slots
            .iter()
            .find(|(_, slot)| slot.completed.is_none())
            .map(|(index, _)| *index)
    }

    /// Pair the terminal `response`'s output with the slots, give each item
    /// its slot's id and order the output by slot. Without slots the output
    /// stays as the provider sent it; an `output` that is not an array is
    /// left for the caller to refuse.
    pub(crate) fn reconcile_terminal(&self, response: &mut Value) -> Result<(), LlmError> {
        if self.slots.is_empty() {
            return Ok(());
        }
        let output = match response.get_mut("output") {
            Some(Value::Array(output)) => std::mem::take(output),
            None | Some(Value::Null) => Vec::new(),
            Some(_) => return Ok(()),
        };
        let mut paired: BTreeMap<u64, Value> = BTreeMap::new();
        for (position, mut item) in output.into_iter().enumerate() {
            let index = item
                .get("output_index")
                .and_then(Value::as_u64)
                .unwrap_or(position as u64);
            let Some(slot) = self.slots.get(&index) else {
                return Err(copilot_mismatch(format!(
                    "the terminal output has an item at output_index {index} the stream never announced"
                )));
            };
            let kind = item
                .get("type")
                .and_then(Value::as_str)
                .map_or("untyped item", copilot_item_kind)
                .to_owned();
            if let Some(slot_kind) = slot.kind.as_deref()
                && slot_kind != kind
            {
                return Err(copilot_mismatch(format!(
                    "the terminal output has a {kind} at output_index {index}, where the stream had a {slot_kind}"
                )));
            }
            if let Some(completed) = slot.completed.as_ref()
                && !copilot_same_item(&kind, completed, &item)
            {
                return Err(copilot_mismatch(format!(
                    "the terminal output's item at output_index {index} differs from the completed one"
                )));
            }
            if item.get("id").is_some_and(Value::is_string) {
                item["id"] = Value::String(slot.id.clone());
            }
            if paired.insert(index, item).is_some() {
                return Err(copilot_mismatch(format!(
                    "the terminal output has two items at output_index {index}"
                )));
            }
        }
        if let Some(index) = self.slots.keys().find(|index| !paired.contains_key(index)) {
            return Err(copilot_mismatch(format!(
                "output_index {index} is missing from the terminal output"
            )));
        }
        response["output"] = Value::Array(paired.into_values().collect());
        Ok(())
    }
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
        raw_stub(events.iter().map(Value::to_string).collect()).await
    }

    /// A Responses stub that sends each of `data` as one SSE `data:` line,
    /// exactly as given (so `[DONE]` goes out as the protocol's sentinel).
    async fn raw_stub(data: Vec<String>) -> (OpenAiClient, tokio::task::JoinHandle<()>) {
        let mut payload = String::new();
        for line in data {
            write!(payload, "data: {line}\n\n").expect("formatting into a String cannot fail");
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

    /// Every event of one Copilot route request over `events`, which the
    /// stub sends as `data:` lines in order, then `[DONE]` when `done`.
    async fn copilot_events(events: Vec<Value>, done: bool) -> Vec<Result<LlmEvent, LlmError>> {
        let mut data: Vec<String> = events.iter().map(Value::to_string).collect();
        if done {
            data.push("[DONE]".to_owned());
        }
        let (client, server) = raw_stub(data).await;
        let client = client.with_copilot_responses_item_identity();
        let request = LlmRequest::new("gpt-5.5", vec![]);
        let collected = client.stream(&request).collect::<Vec<_>>().await;
        server.abort();
        collected
    }

    /// `events` end the request with the retryable outcome of a Copilot
    /// stream whose terminal output does not match it, naming `detail`, and
    /// commit nothing.
    fn assert_copilot_mismatch(events: &[Result<LlmEvent, LlmError>], detail: &str, case: &str) {
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, Ok(LlmEvent::AssistantOutput { .. }))),
            "{case}: nothing is committed"
        );
        let Some(Ok(LlmEvent::Done {
            outcome: LlmDoneOutcome::Error { error },
        })) = events.last()
        else {
            panic!(
                "{case}: the request ends with an error, not {:?}",
                events.last()
            );
        };
        assert!(
            matches!(
                error,
                LlmError::IncompleteResponse { message }
                    if message.contains("does not match its stream") && message.contains(detail)
            ),
            "{case}: a retryable incomplete response naming '{detail}', not {error:?}"
        );
        assert!(error.is_retryable(), "{case}: the mismatch is retryable");
    }

    /// The Copilot route gives a message the first id any event of its
    /// output_index slot carries, whichever event that is: Copilot
    /// re-identifies it on every event, only in the terminal response, or
    /// on a delta that comes before the item is added. The terminal item is
    /// committed under that id, and its text streams once.
    #[tokio::test]
    async fn copilot_reidentified_message_takes_its_slot_identity() {
        let shapes: [(&str, [&str; 7]); 3] = [
            ("terminal rename", ["F", "F", "F", "F", "F", "F", "G"]),
            ("every event", ["A", "B", "C", "D", "E", "F", "G"]),
            (
                "terminal kept, delta distinct",
                ["A", "B", "D", "D", "E", "F", "F"],
            ),
        ];
        let mut cases: Vec<(&str, Vec<Value>, &str)> = shapes
            .into_iter()
            .map(|(shape, ids)| (shape, copilot_message_lifecycle(ids), ids[0]))
            .collect();
        let mut retransmitted = copilot_message_lifecycle(["A", "B", "C", "D", "E", "F", "G"]);
        let added = retransmitted[1].clone();
        retransmitted.insert(2, added);
        cases.push(("identical added retransmission", retransmitted, "A"));
        let mut delta_first = copilot_message_lifecycle(["A", "B", "C", "D", "E", "F", "G"]);
        let delta = delta_first.remove(3);
        delta_first.insert(1, delta);
        cases.push(("a delta before the added item", delta_first, "C"));
        for (shape, events, slot_id) in cases {
            let (client, server) = copilot_stub(events).await;
            let (tx, mut rx) = tokio::sync::mpsc::channel(32);
            let result =
                LlmClientAdapter::with_event_channel(Arc::new(client), "gpt-5.5".into(), tx)
                    .stream_response(&[], &[], 1000, None, None)
                    .await;
            server.abort();
            let result = result.unwrap_or_else(|error| {
                panic!("{shape}: the terminal message is the slot's message: {error}")
            });
            let committed = message(slot_id, "final_answer", "Done.");
            assert_eq!(
                result.blocks(),
                lower_items([&committed], Some("resp_1")).unwrap(),
                "{shape}: the slot's first id is the committed id"
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

    /// A Copilot reasoning item re-identified (added `A`, summary done `X`,
    /// done `B`, terminal `I`) with a re-encoded ciphertext and identical
    /// visible fields is committed under its slot's first id with the
    /// terminal ciphertext, exactly as the terminal response carried it.
    #[tokio::test]
    async fn copilot_reencoded_reasoning_replays_the_slot_id_with_the_terminal_ciphertext() {
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
            panic!("the re-encoded terminal reasoning is the slot's reasoning");
        };
        let committed = reasoning("A", "terminal-cipher");
        assert_eq!(
            result.blocks(),
            lower_items([&committed, &answer], Some("resp_1")).unwrap(),
            "the slot id and the terminal ciphertext are lowered together"
        );
        assert_replayed_reasoning(
            result.into_message(),
            &[("A", "terminal-cipher")],
            "re-encoded",
        );
    }

    /// A Copilot response of reasoning items and then an answer: each
    /// `(id, summary)` of `completed` completes under that id at its own
    /// output index, and the terminal response carries `terminal` in that
    /// order, then the answer. Every reasoning item has a ciphertext of its
    /// own, `cipher-<id>`. With `streamed`, each completed lifecycle is first
    /// added as `A<index>` and completes its summary in the stream. Returns
    /// the events and the terminal output.
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

    /// `output` with each item's id replaced by the slot id at its index in
    /// `slot_ids` (the reasoning items lead the terminal output).
    fn with_slot_ids(output: &[Value], slot_ids: &[&str]) -> Vec<Value> {
        output
            .iter()
            .enumerate()
            .map(|(index, item)| {
                let mut item = item.clone();
                if let Some(id) = slot_ids.get(index) {
                    item["id"] = json!(id);
                }
                item
            })
            .collect()
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
            "{case}: each slot id replays with its terminal item's ciphertext"
        );
    }

    /// Reasoning items with the same visible fields (no summary, or the same
    /// summary), each re-identified with a fresh ciphertext, pair by their
    /// slots, streamed or not: the blocks are the terminal items under their
    /// slot ids, each with its own terminal ciphertext, and a streamed item
    /// completes once in the live events.
    #[tokio::test]
    async fn copilot_identical_reasoning_items_pair_by_slot() {
        for streamed in [false, true] {
            for (shape, summary) in [
                ("empty summaries", None),
                ("identical summaries", Some("Plan")),
            ] {
                let case = format!("{shape}, streamed {streamed}");
                let (events, output) = copilot_reasoning_response(
                    &[("B", summary), ("C", summary)],
                    &[("I", summary), ("J", summary)],
                    streamed,
                );
                let slot_ids: [&str; 2] = if streamed { ["A0", "A1"] } else { ["B", "C"] };
                let (client, server) = copilot_stub(events).await;
                let (tx, mut rx) = tokio::sync::mpsc::channel(32);
                let result =
                    LlmClientAdapter::with_event_channel(Arc::new(client), "gpt-5.5".into(), tx)
                        .stream_response(&[], &[], 1000, None, None)
                        .await;
                server.abort();
                let result = result.unwrap_or_else(|error| {
                    panic!("{case}: each slot has its terminal item: {error}")
                });
                assert_eq!(
                    result.blocks(),
                    lower_items(&with_slot_ids(&output, &slot_ids), Some("resp_1")).unwrap(),
                    "{case}: the blocks are the terminal items under their slot ids"
                );
                let completions = std::iter::from_fn(|| rx.try_recv().ok())
                    .filter(|event| {
                        matches!(event, meerkat_core::AgentEvent::ReasoningComplete { .. })
                    })
                    .count();
                assert_eq!(completions, 2, "{case}: each reasoning item completes once");
                assert_replayed_reasoning(
                    result.into_message(),
                    &[(slot_ids[0], "cipher-I"), (slot_ids[1], "cipher-J")],
                    &case,
                );
            }
        }
    }

    /// Two identical messages, each re-identified on every event of its
    /// lifecycle, pair by their slots: the terminal messages are committed
    /// under their slot ids, and each visible text streams once.
    #[tokio::test]
    async fn copilot_identical_messages_pair_by_slot() {
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
        let result =
            result.unwrap_or_else(|error| panic!("each slot has its terminal message: {error}"));
        assert_eq!(
            result.blocks(),
            lower_items(&[at("A0"), at("A1")], Some("resp_1")).unwrap(),
            "the terminal messages are committed under their slot ids"
        );
        let mut visible = String::new();
        while let Ok(event) = rx.try_recv() {
            if let meerkat_core::AgentEvent::TextDelta { delta, .. } = event {
                visible.push_str(&delta);
            }
        }
        assert_eq!(visible, "Done.Done.", "each visible text streams once");
    }

    /// Reasoning items without a summary beside a summarized one pair by
    /// their slots, streamed or not.
    #[tokio::test]
    async fn copilot_reasoning_slots_beside_a_distinct_item_pair_by_slot() {
        for streamed in [false, true] {
            let (events, output) = copilot_reasoning_response(
                &[("B", None), ("E", Some("Plan")), ("C", None)],
                &[("I", None), ("K", Some("Plan")), ("J", None)],
                streamed,
            );
            let slot_ids: [&str; 3] = if streamed {
                ["A0", "A1", "A2"]
            } else {
                ["B", "E", "C"]
            };
            let (client, server) = copilot_stub(events).await;
            let result = LlmClientAdapter::new(Arc::new(client), "gpt-5.5".into())
                .stream_response(&[], &[], 1000, None, None)
                .await;
            server.abort();
            let result = result.unwrap_or_else(|error| {
                panic!("streamed {streamed}: each slot has its terminal item: {error}")
            });
            assert_eq!(
                result.blocks(),
                lower_items(&with_slot_ids(&output, &slot_ids), Some("resp_1")).unwrap(),
                "streamed {streamed}: the blocks are the terminal items under their slot ids"
            );
            assert_replayed_reasoning(
                result.into_message(),
                &[
                    (slot_ids[0], "cipher-I"),
                    (slot_ids[1], "cipher-K"),
                    (slot_ids[2], "cipher-J"),
                ],
                &format!("streamed {streamed}"),
            );
        }
    }

    /// A terminal output that keeps one slot's id and re-identifies the
    /// other pairs both by slot.
    #[tokio::test]
    async fn copilot_terminal_that_keeps_a_slot_id_pairs_by_slot() {
        let (events, output) = copilot_reasoning_response(
            &[("B", None), ("C", None)],
            &[("B", None), ("J", None)],
            false,
        );
        let (client, server) = copilot_stub(events).await;
        let result = LlmClientAdapter::new(Arc::new(client), "gpt-5.5".into())
            .stream_response(&[], &[], 1000, None, None)
            .await;
        server.abort();
        let result =
            result.unwrap_or_else(|error| panic!("each slot has its terminal item: {error}"));
        assert_eq!(
            result.blocks(),
            lower_items(&with_slot_ids(&output, &["B", "C"]), Some("resp_1")).unwrap(),
            "the blocks are the terminal items under their slot ids"
        );
        assert_replayed_reasoning(
            result.into_message(),
            &[("B", "cipher-B"), ("C", "cipher-J")],
            "kept slot id",
        );
    }

    /// Synthetic: a terminal output whose items carry their own
    /// output_index pairs each item with that slot, in slot order, whatever
    /// the array order.
    #[tokio::test]
    async fn copilot_synthetic_terminal_items_pair_by_their_own_output_index() {
        let (events, output) = copilot_reasoning_response(
            &[("B", Some("A plan")), ("C", Some("B plan"))],
            &[("I", Some("A plan")), ("J", Some("B plan"))],
            false,
        );
        let mut indexed: Vec<Value> = output
            .iter()
            .enumerate()
            .map(|(index, item)| {
                let mut item = item.clone();
                item["output_index"] = json!(index);
                item
            })
            .collect();
        indexed.swap(0, 1);
        let mut events = events;
        let terminal = events.last_mut().unwrap();
        terminal["response"]["output"] = json!(indexed);
        let (client, server) = copilot_stub(events).await;
        let result = LlmClientAdapter::new(Arc::new(client), "gpt-5.5".into())
            .stream_response(&[], &[], 1000, None, None)
            .await;
        server.abort();
        let result =
            result.unwrap_or_else(|error| panic!("each item names its own output_index: {error}"));
        assert_replayed_reasoning(
            result.into_message(),
            &[("B", "cipher-I"), ("C", "cipher-J")],
            "own output_index",
        );
    }

    /// A delta without an output_index whose item id an earlier event of a
    /// slot carried belongs to that slot.
    #[tokio::test]
    async fn copilot_delta_without_output_index_follows_an_id_seen_at_its_slot() {
        let mut events = copilot_message_lifecycle(["A", "B", "C", "D", "E", "F", "G"]);
        events[3] = json!({"type":"response.output_text.delta","item_id":"B","content_index":0,"delta":"Done."});
        let (client, server) = copilot_stub(events).await;
        let (tx, mut rx) = tokio::sync::mpsc::channel(32);
        let result = LlmClientAdapter::with_event_channel(Arc::new(client), "gpt-5.5".into(), tx)
            .stream_response(&[], &[], 1000, None, None)
            .await;
        server.abort();
        let result = result.unwrap_or_else(|error| panic!("the delta is the slot's: {error}"));
        assert_eq!(
            result.blocks(),
            lower_items([&message("A", "final_answer", "Done.")], Some("resp_1")).unwrap()
        );
        let mut visible = String::new();
        while let Ok(event) = rx.try_recv() {
            if let meerkat_core::AgentEvent::TextDelta { delta, .. } = event {
                visible.push_str(&delta);
            }
        }
        assert_eq!(visible, "Done.", "one visible text sequence");
    }

    /// A Copilot stream that ends with `[DONE]` and no terminal response
    /// commits its completed items, each under its slot's first id.
    #[tokio::test]
    async fn copilot_stream_without_a_terminal_response_commits_its_slots() {
        let mut events = copilot_message_lifecycle(["A", "B", "C", "D", "E", "F", "G"]);
        events.pop();
        let events = copilot_events(events, true).await;
        let blocks = events
            .iter()
            .find_map(|event| match event {
                Ok(LlmEvent::AssistantOutput { blocks }) => Some(blocks.clone()),
                _ => None,
            })
            .unwrap_or_else(|| panic!("the completed items are committed: {:?}", events.last()));
        assert_eq!(
            blocks,
            lower_items([&message("A", "final_answer", "Done.")], Some("resp_1")).unwrap()
        );
        assert!(matches!(
            events.last(),
            Some(Ok(LlmEvent::Done {
                outcome: LlmDoneOutcome::Success { .. }
            }))
        ));
    }

    /// A stream whose message completed and whose function call was added
    /// and started its arguments, then ended without completing the call.
    fn a_call_started_but_never_completed() -> Vec<Value> {
        let answer = message("msg_1", "final_answer", "Done.");
        let call = json!({"type":"function_call","id":"fc_1","call_id":"call_1","name":"lookup","arguments":"","status":"in_progress"});
        vec![
            json!({"type":"response.created","response":{"id":"resp_1"}}),
            json!({"type":"response.output_item.added","output_index":0,"item":answer}),
            json!({"type":"response.output_text.delta","item_id":"msg_1","output_index":0,"delta":"Done."}),
            json!({"type":"response.output_item.done","output_index":0,"item":answer}),
            json!({"type":"response.output_item.added","output_index":1,"item":call}),
            json!({"type":"response.function_call_arguments.delta","item_id":"fc_1","output_index":1,"delta":"{\"q\":"}),
        ]
    }

    /// Without a terminal response a stream commits only what it completed,
    /// so on the Copilot route a slot the stream announced and never
    /// completed (here a started tool call) ends the response with the
    /// retryable mismatch, and nothing is committed. (Fails-old: the call
    /// was dropped and the turn ended as complete.)
    #[tokio::test]
    async fn copilot_done_without_a_terminal_response_refuses_an_uncompleted_slot() {
        let events = copilot_events(a_call_started_but_never_completed(), true).await;
        assert_copilot_mismatch(
            &events,
            "output_index 1 was announced but never completed",
            "a started call at [DONE]",
        );
    }

    /// The plain OpenAI route keeps the same rule: an item the stream added
    /// that never completed before `[DONE]` ends the response as the
    /// retryable incomplete response, and nothing is committed. (Fails-old:
    /// the call was dropped and the turn ended as complete.)
    #[tokio::test]
    async fn plain_openai_done_without_a_terminal_response_refuses_an_uncompleted_item() {
        let data: Vec<String> = a_call_started_but_never_completed()
            .iter()
            .map(Value::to_string)
            .chain(["[DONE]".to_owned()])
            .collect();
        let (client, server) = raw_stub(data).await;
        let request = LlmRequest::new("gpt-5.5", vec![]);
        let events = client.stream(&request).collect::<Vec<_>>().await;
        server.abort();
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, Ok(LlmEvent::AssistantOutput { .. }))),
            "nothing is committed"
        );
        let Some(Ok(LlmEvent::Done {
            outcome: LlmDoneOutcome::Error { error },
        })) = events.last()
        else {
            panic!("the request ends with an error, not {:?}", events.last());
        };
        assert!(
            matches!(
                error,
                LlmError::IncompleteResponse { message }
                    if message.contains("output_index 1 was announced but never completed")
            ),
            "{error:?}"
        );
        assert!(error.is_retryable());
    }

    /// Copilot rotates a function call's item id on every event while its
    /// call_id stays: the call keeps its call_id, and its argument deltas,
    /// whose item ids are those of the call's slot, stream live.
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
        let events = copilot_events(events, false).await;
        let calls: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                Ok(LlmEvent::AssistantOutput { blocks }) => Some(blocks.clone()),
                _ => None,
            })
            .flatten()
            .filter_map(|block| match block {
                AssistantBlock::ToolUse { id, name, args, .. } => {
                    Some((id, name, args.get().to_owned()))
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
            )],
            "rotating function item ids keep the stable call_id"
        );
        let streamed: String = events
            .iter()
            .filter_map(|event| match event {
                Ok(LlmEvent::ToolCallDelta { id, args_delta, .. }) if id == "call_1" => {
                    Some(args_delta.as_str())
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            streamed, "{\"q\":5}",
            "the call's argument deltas stream under its call_id"
        );
    }

    /// Synthetic (no captured stream shows these): on the Copilot route a
    /// terminal output that does not line up with the items its stream
    /// announced, or a stream that contradicts itself, is never repaired:
    /// no item is invented, moved or merged, and no ciphertext changes. The
    /// request ends with the retryable incomplete-response outcome that
    /// names the slot, and nothing is committed.
    #[tokio::test]
    async fn copilot_synthetic_stream_mismatches_are_retryable_incomplete_responses() {
        let done = message("message-f", "final_answer", "Done.");
        let completed = |output: Value| {
            json!({"type":"response.completed","response":{"id":"resp_1","status":"completed",
                "output":output,"usage":{"input_tokens":3,"output_tokens":2,"total_tokens":5}}})
        };
        let added_at = |index: u64, item: &Value| json!({"type":"response.output_item.added","output_index":index,"item":item});
        let done_at = |index: u64, item: &Value| json!({"type":"response.output_item.done","output_index":index,"item":item});
        let reasoning = |id: &str, text: &str, ciphertext: &str| json!({"type":"reasoning","id":id,"summary":[{"type":"summary_text","text":text}],"encrypted_content":ciphertext});
        let at_index = |mut item: Value, index: u64| {
            item["output_index"] = json!(index);
            item
        };
        let call_f = json!({"type":"function_call","id":"fc_f","call_id":"call_f","name":"lookup","arguments":"{}","status":"completed"});
        let mut call_g = call_f.clone();
        call_g["call_id"] = json!("call_g");
        let mut with_status = reasoning("I", "Plan", "c2");
        with_status["status"] = json!("completed");
        let other = message("other", "final_answer", "Done");
        let cases = [
            (
                "drifted message content",
                vec![
                    done_at(0, &done),
                    completed(json!([message("message-g", "final_answer", "Other.")])),
                ],
                "item at output_index 0 differs",
            ),
            (
                "drifted reasoning summary",
                vec![
                    done_at(0, &reasoning("B", "Plan", "c1")),
                    completed(json!([reasoning("I", "Other plan", "c2")])),
                ],
                "item at output_index 0 differs",
            ),
            (
                "a reasoning field on one side only",
                vec![
                    done_at(0, &reasoning("B", "Plan", "c1")),
                    completed(json!([with_status])),
                ],
                "item at output_index 0 differs",
            ),
            (
                "another kind in the slot",
                vec![
                    done_at(0, &done),
                    completed(json!([{"type":"reasoning","id":"rs_g","summary":[]}])),
                ],
                "a reasoning at output_index 0",
            ),
            (
                "more terminal items than slots",
                vec![
                    done_at(0, &done),
                    completed(json!([
                        message("message-g", "final_answer", "Done."),
                        message("message-k", "final_answer", "Done.")
                    ])),
                ],
                "output_index 1 the stream never announced",
            ),
            (
                "fewer terminal items than slots",
                vec![
                    done_at(0, &reasoning("B", "Plan", "c1")),
                    done_at(1, &reasoning("C", "Plan", "c2")),
                    completed(json!([reasoning("I", "Plan", "c3")])),
                ],
                "output_index 1 is missing",
            ),
            (
                "reordered distinct reasoning",
                vec![
                    done_at(0, &reasoning("B", "A plan", "c1")),
                    done_at(1, &reasoning("C", "B plan", "c2")),
                    completed(json!([
                        reasoning("J", "B plan", "c4"),
                        reasoning("I", "A plan", "c3")
                    ])),
                ],
                "item at output_index 0 differs",
            ),
            (
                "one id at two slots",
                vec![
                    added_at(0, &message("A", "final_answer", "Left.")),
                    json!({"type":"response.output_text.delta","item_id":"D0","output_index":0,"delta":"Left."}),
                    added_at(1, &message("A", "final_answer", "Right.")),
                    done_at(1, &message("F", "final_answer", "Right.")),
                    completed(json!([message("G", "final_answer", "Right.")])),
                ],
                "id 'A' appears at output_index 0 and 1",
            ),
            (
                "added again as a different item",
                vec![
                    added_at(0, &message("A", "final_answer", "Done.")),
                    done_at(0, &message("F", "final_answer", "Done.")),
                    added_at(0, &message("B", "final_answer", "Later.")),
                    json!({"type":"response.output_text.delta","item_id":"D1","output_index":0,"delta":"Later."}),
                    completed(json!([message("G", "final_answer", "Done.")])),
                ],
                "output_index 0 was added again as a different item",
            ),
            (
                "completed again as a different item",
                vec![
                    done_at(0, &done),
                    done_at(0, &message("message-h", "final_answer", "More.")),
                    completed(json!([done])),
                ],
                "output_index 0 was completed again as a different item",
            ),
            (
                "a function call that changed its call_id",
                vec![done_at(0, &call_f), completed(json!([call_g]))],
                "item at output_index 0 differs",
            ),
            (
                "a reasoning completion at a slot the terminal lacks",
                vec![
                    json!({"type":"response.reasoning_summary.done","output_index":5,"item":reasoning("X", "Plan", "c0")}),
                    done_at(0, &reasoning("B", "Plan", "c1")),
                    completed(json!([reasoning("B", "Plan", "c1")])),
                ],
                "output_index 5 is missing",
            ),
            (
                "an empty terminal output",
                vec![
                    added_at(0, &message("A", "final_answer", "Done.")),
                    json!({"type":"response.output_text.delta","item_id":"C","output_index":0,"delta":"Done."}),
                    done_at(0, &message("F", "final_answer", "Done.")),
                    completed(json!([])),
                ],
                "output_index 0 is missing",
            ),
            (
                "a terminal response without output",
                vec![
                    added_at(0, &message("A", "final_answer", "Done.")),
                    done_at(0, &message("F", "final_answer", "Done.")),
                    json!({"type":"response.done","response":{"id":"resp_1","status":"completed"}}),
                ],
                "output_index 0 is missing",
            ),
            (
                "a delta of an id no slot carries",
                vec![
                    json!({"type":"response.output_text.delta","item_id":"stray","delta":"Done."}),
                    done_at(0, &done),
                    completed(json!([message("message-g", "final_answer", "Done.")])),
                ],
                "omitted streamed assistant message 'stray'",
            ),
            (
                "two terminal items at one output_index",
                vec![
                    done_at(0, &reasoning("B", "Plan", "c1")),
                    completed(json!([
                        at_index(reasoning("I", "Plan", "c2"), 0),
                        at_index(reasoning("J", "Plan", "c3"), 0)
                    ])),
                ],
                "two items at output_index 0",
            ),
            (
                "an atomic message replaced",
                vec![
                    done_at(0, &message("msg_atomic", "commentary", "Working.")),
                    completed(json!([other])),
                ],
                "item at output_index 0 differs",
            ),
            (
                "an atomic reasoning replaced by a message",
                vec![
                    done_at(
                        0,
                        &json!({"type":"reasoning","id":"rs_atomic","summary":[],"encrypted_content":"atomic-cipher"}),
                    ),
                    completed(json!([other])),
                ],
                "a message at output_index 0",
            ),
            (
                "an atomic web search replaced by a message",
                vec![
                    done_at(
                        0,
                        &json!({"type":"web_search_call","id":"ws_atomic","status":"completed"}),
                    ),
                    completed(json!([other])),
                ],
                "a message at output_index 0",
            ),
        ];
        for (case, events, detail) in cases {
            let events = copilot_events(events, false).await;
            assert_copilot_mismatch(&events, detail, case);
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
