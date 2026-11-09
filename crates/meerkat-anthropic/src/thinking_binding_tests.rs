//! Thinking-block binding (Anthropic "preserved thinking"), driven against a
//! mock Messages endpoint that enforces the documented rules for an account
//! created on or after 2026-08-31, which Claude Fable 5.2 and Mythos 5.2
//! apply to every account, whatever its age.
//!
//! Claude Fable 5.2, Mythos 5.2, Fable 5.1, Mythos 5.1, Opus 5.5, Sonnet 5.5
//! and Haiku 5.5 accept a replayed thinking block only while the top-level
//! `system`, the `tools` and every earlier message are unchanged; on such an
//! account a changed prefix is a 400 unless the request sets
//! `thinking.block_binding.prefix_mismatch_behavior: "drop_block"`. That field
//! needs the `thinking-binding-controls-2026-08-01` beta header, is accepted
//! beside `adaptive` and `enabled` only (`between_tools` takes no other field;
//! Haiku 5.5 refuses it beside `disabled`), and makes the API drop the
//! failing block and every later one, reporting each in
//! `input_transformations`.
//! <https://platform.claude.com/docs/en/build-with-claude/preserved-thinking>
//! <https://platform.claude.com/docs/en/build-with-claude/thinking>
//! <https://platform.claude.com/docs/en/models/fable-5-2/whats-new-fable-5-2>
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use futures::StreamExt;
use meerkat_core::lifecycle::run_primitive::{
    AnthropicCacheControlPolicy, AnthropicThinkingConfig,
};
use meerkat_core::{
    AssistantBlock, BlockAssistantMessage, Message, ProviderMeta, StopReason, SystemNoticeKind,
    SystemNoticeMessage, ToolChoice, ToolDef, ToolResult, UserMessage,
};
use meerkat_llm_core::{LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest};
use serde_json::{Value, json};

use crate::AnthropicClient;

const BINDING_BETA: &str = "thinking-binding-controls-2026-08-01";

/// The models whose replayed thinking blocks the prefix check covers.
const PREFIX_CHECKED: &[&str] = &[
    "claude-fable-5-2",
    "claude-mythos-5-2",
    "claude-fable-5-1",
    "claude-mythos-5-1",
    "claude-opus-5-5",
    "claude-sonnet-5-5",
    "claude-haiku-5-5",
];

/// The prefix-checked models Meerkat's catalog carries.
const CATALOGUED_PREFIX_CHECKED: &[&str] = &[
    "claude-fable-5-2",
    "claude-mythos-5-2",
    "claude-fable-5-1",
    "claude-opus-5-5",
    "claude-sonnet-5-5",
    "claude-haiku-5-5",
];

/// A user turn ending in this text gets a `tool_use` answer from the mock.
const TOOL_CUE: &str = "Use the lookup tool.";

/// A tool the mock account refuses with a 400 of its own, whatever else the
/// request carries.
const REFUSED_TOOL: &str = "refused_tool";

/// A tool the mock account answers with a 529 overload when the request
/// replays no thinking block.
const OVERLOADED_TOOL: &str = "overloaded_tool";

/// Whether `body` lists a tool named `name`.
fn carries_tool(body: &Value, name: &str) -> bool {
    body.get("tools")
        .and_then(Value::as_array)
        .is_some_and(|tools| {
            tools
                .iter()
                .any(|tool| tool.get("name").and_then(Value::as_str) == Some(name))
        })
}

/// One request the endpoint received.
#[derive(Debug, Clone)]
struct Seen {
    betas: Vec<String>,
    body: Value,
}

impl Seen {
    fn carries_binding_beta(&self) -> bool {
        self.betas.iter().any(|beta| beta == BINDING_BETA)
    }
}

/// The mock account: every request it received.
#[derive(Clone, Default)]
struct EnforcingAccount {
    seen: Arc<Mutex<Vec<Seen>>>,
}

impl EnforcingAccount {
    fn last(&self) -> Seen {
        self.seen
            .lock()
            .unwrap()
            .last()
            .cloned()
            .expect("a request reached the endpoint")
    }

    /// Every request the endpoint received, in order.
    fn all(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }
}

async fn messages(
    State(account): State<EnforcingAccount>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let body: Value = serde_json::from_str(&body).expect("the request body is JSON");
    let betas: Vec<String> = headers
        .get_all("anthropic-beta")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .map(|beta| beta.trim().to_string())
        .filter(|beta| !beta.is_empty())
        .collect();
    account.seen.lock().unwrap().push(Seen {
        betas: betas.clone(),
        body: body.clone(),
    });
    if carries_tool(&body, OVERLOADED_TOOL) && replayed_thinking_blocks(&body) == 0 {
        return (
            StatusCode::from_u16(529).unwrap(),
            [("content-type", "application/json")],
            json!({
                "type": "error",
                "error": {"type": "overloaded_error", "message": "Overloaded"}
            })
            .to_string(),
        )
            .into_response();
    }
    match judge(&body, &betas) {
        Ok(dropped) => stream_reply(&body, &betas, &dropped),
        Err(message) => (
            StatusCode::BAD_REQUEST,
            [("content-type", "application/json")],
            json!({
                "type": "error",
                "error": {"type": "invalid_request_error", "message": message}
            })
            .to_string(),
        )
            .into_response(),
    }
}

/// The documented answer to `body`: the paths of the replayed thinking blocks
/// dropped under `drop_block`, or the 400 message.
fn judge(body: &Value, betas: &[String]) -> Result<Vec<String>, String> {
    let thinking = body.get("thinking");
    let kind = thinking
        .and_then(|thinking| thinking.get("type"))
        .and_then(Value::as_str);
    let binding = thinking.and_then(|thinking| thinking.get("block_binding"));
    if binding.is_some() && !betas.iter().any(|beta| beta == BINDING_BETA) {
        return Err("thinking.block_binding: Extra inputs are not permitted".to_string());
    }
    if kind == Some("between_tools")
        && thinking
            .and_then(Value::as_object)
            .is_some_and(|thinking| thinking.len() > 1)
    {
        return Err(
            "thinking: `between_tools` takes no other field: `display`, `budget_tokens`, or \
             `block_binding` sent with it returns a 400 error"
                .to_string(),
        );
    }
    if kind == Some("disabled") && binding.is_some() {
        return Err("thinking.block_binding: not accepted with thinking type disabled".to_string());
    }
    if body
        .get("tools")
        .and_then(Value::as_array)
        .is_some_and(|tools| {
            tools
                .iter()
                .any(|tool| tool.get("name").and_then(Value::as_str) == Some(REFUSED_TOOL))
        })
    {
        return Err(format!(
            "tools: `{REFUSED_TOOL}` is refused (the request replayed {} thinking blocks)",
            replayed_thinking_blocks(body)
        ));
    }
    let messages = body
        .get("messages")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    // Manual (`enabled`) thinking requires the assistant message a request
    // continues with tool results to start with a thinking block; adaptive
    // thinking, and a request that omits thinking, do not.
    if kind == Some("enabled")
        && let Some(index) = continued_assistant_index(&messages)
        && !matches!(
            messages[index]["content"][0]["type"].as_str(),
            Some("thinking" | "redacted_thinking")
        )
    {
        return Err(format!(
            "messages.{index}.content.0.type: Expected `thinking` or `redacted_thinking`. When \
             `thinking` is enabled, a final `assistant` message must start with a thinking block."
        ));
    }
    let model = body
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !PREFIX_CHECKED.contains(&model) {
        return Ok(Vec::new());
    }
    let drop_block = binding
        .and_then(|binding| binding.get("prefix_mismatch_behavior"))
        .and_then(Value::as_str)
        == Some("drop_block");
    let mut failing = false;
    let mut dropped = Vec::new();
    for (index, message) in messages.iter().enumerate() {
        let Some(blocks) = message.get("content").and_then(Value::as_array) else {
            continue;
        };
        for (position, block) in blocks.iter().enumerate() {
            if block.get("type").and_then(Value::as_str) != Some("thinking") {
                continue;
            }
            let bound = bound_signature(body, &messages[..index]);
            failing =
                failing || block.get("signature").and_then(Value::as_str) != Some(bound.as_str());
            if !failing {
                continue;
            }
            let path = format!("messages.{index}.content.{position}");
            if !drop_block {
                return Err(format!(
                    "{path}: Invalid `signature` in `thinking` block. The block is bound to a \
                     different conversation. Remove the block, or set \
                     `thinking.block_binding.prefix_mismatch_behavior` to \"drop_block\"."
                ));
            }
            dropped.push(path);
        }
    }
    Ok(dropped)
}

/// The signature the endpoint binds a thinking block to: the request's
/// `system` and `tools` and the messages before the block.
fn bound_signature(body: &Value, earlier: &[Value]) -> String {
    let system = body.get("system").map(|system| match system {
        Value::String(text) => json!([{"type": "text", "text": text}]),
        other => canonical(other),
    });
    let prefix = json!({
        "system": system,
        "tools": body.get("tools").map(canonical),
        "messages": earlier.iter().map(canonical).collect::<Vec<_>>(),
    });
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    prefix.to_string().hash(&mut hasher);
    format!("bound-{:016x}", hasher.finish())
}

/// `value` as the prefix check reads it: `cache_control` markers are not part
/// of the prefix, a bare-string content is its one text block, and key order
/// does not matter.
fn canonical(value: &Value) -> Value {
    match value {
        Value::Object(fields) => {
            let sorted: BTreeMap<String, Value> = fields
                .iter()
                .filter(|(key, _)| key.as_str() != "cache_control")
                .map(|(key, field)| {
                    let field = match (key.as_str(), field) {
                        ("content", Value::String(text)) => {
                            json!([{"type": "text", "text": text}])
                        }
                        _ => canonical(field),
                    };
                    (key.clone(), field)
                })
                .collect();
            Value::Object(sorted.into_iter().collect())
        }
        Value::Array(items) => Value::Array(items.iter().map(canonical).collect()),
        other => other.clone(),
    }
}

/// A streamed answer with one thinking block, bound to this request, and one
/// text block, or, when the last user turn asks for the tool
/// ([`TOOL_CUE`]), one `lookup` call. Under the beta header `message_start`
/// reports the dropped blocks in `input_transformations`.
fn stream_reply(body: &Value, betas: &[String], dropped: &[String]) -> Response {
    let mut message = json!({"usage": {"input_tokens": 12, "output_tokens": 0}});
    if betas.iter().any(|beta| beta == BINDING_BETA) {
        message["input_transformations"] = Value::Array(
            dropped
                .iter()
                .map(|path| {
                    json!({
                        "type": "thinking_dropped",
                        "path": path,
                        "reason": "prefix_binding_mismatch"
                    })
                })
                .collect(),
        );
    }
    let messages = body
        .get("messages")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let signature = bound_signature(body, &messages);
    let asks_for_the_tool = messages.last().is_some_and(|last| {
        last["role"] == "user" && last["content"].to_string().contains(TOOL_CUE)
    });
    let answer = if asks_for_the_tool {
        [
            json!({"type": "content_block_start", "index": 1,
                   "content_block": {"type": "tool_use", "id": "toolu_lookup",
                                     "name": "lookup", "input": {}}}),
            json!({"type": "content_block_delta", "index": 1,
                   "delta": {"type": "input_json_delta", "partial_json": "{\"key\":\"alpha\"}"}}),
            json!({"type": "content_block_stop", "index": 1}),
            json!({"type": "message_delta", "delta": {"stop_reason": "tool_use"},
                   "usage": {"output_tokens": 7}}),
        ]
    } else {
        [
            json!({"type": "content_block_start", "index": 1,
                   "content_block": {"type": "text", "text": ""}}),
            json!({"type": "content_block_delta", "index": 1,
                   "delta": {"type": "text_delta", "text": "42."}}),
            json!({"type": "content_block_stop", "index": 1}),
            json!({"type": "message_delta", "delta": {"stop_reason": "end_turn"},
                   "usage": {"output_tokens": 7}}),
        ]
    };
    let mut events = vec![
        json!({"type": "message_start", "message": message}),
        json!({"type": "content_block_start", "index": 0,
               "content_block": {"type": "thinking", "thinking": ""}}),
        json!({"type": "content_block_delta", "index": 0,
               "delta": {"type": "thinking_delta", "thinking": "Multiply."}}),
        json!({"type": "content_block_delta", "index": 0,
               "delta": {"type": "signature_delta", "signature": signature}}),
        json!({"type": "content_block_stop", "index": 0}),
    ];
    events.extend(answer);
    events.push(json!({"type": "message_stop"}));
    event_stream(&events)
}

/// The thinking and redacted thinking blocks `body` replays.
fn replayed_thinking_blocks(body: &Value) -> usize {
    body.get("messages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|message| message.get("content").and_then(Value::as_array))
        .flatten()
        .filter(|block| {
            matches!(
                block.get("type").and_then(Value::as_str),
                Some("thinking" | "redacted_thinking")
            )
        })
        .count()
}

/// The index of the assistant message a request continues: the last one,
/// when the user messages after it carry tool results.
fn continued_assistant_index(messages: &[Value]) -> Option<usize> {
    let index = messages
        .iter()
        .rposition(|message| message["role"] == "assistant")?;
    messages[index + 1..]
        .iter()
        .any(|message| {
            message["content"]
                .as_array()
                .is_some_and(|blocks| blocks.iter().any(|block| block["type"] == "tool_result"))
        })
        .then_some(index)
}

/// `events` as a `text/event-stream` response, one `event:`/`data:` pair
/// each.
fn event_stream(events: &[Value]) -> Response {
    use std::fmt::Write;
    let mut stream = String::new();
    for event in events {
        let _ = write!(
            stream,
            "event: {}\ndata: {event}\n\n",
            event["type"].as_str().unwrap_or_default()
        );
    }
    ([("content-type", "text/event-stream")], stream).into_response()
}

async fn spawn_account() -> (
    EnforcingAccount,
    AnthropicClient,
    tokio::task::JoinHandle<()>,
) {
    spawn_account_on_a_route(true).await
}

/// The mock account behind a route that does not take the binding control
/// (Amazon Bedrock, the GitHub Copilot route).
async fn spawn_account_without_the_control() -> (
    EnforcingAccount,
    AnthropicClient,
    tokio::task::JoinHandle<()>,
) {
    spawn_account_on_a_route(false).await
}

async fn spawn_account_on_a_route(
    controls_supported: bool,
) -> (
    EnforcingAccount,
    AnthropicClient,
    tokio::task::JoinHandle<()>,
) {
    let account = EnforcingAccount::default();
    let app = Router::new()
        .route("/v1/messages", post(messages))
        .with_state(account.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let client = AnthropicClient::builder("test-key".to_string())
        .base_url(format!("http://{addr}"))
        .thinking_binding_controls_supported(controls_supported)
        .build()
        .unwrap();
    (account, client, server)
}

fn tool(name: &str) -> Arc<ToolDef> {
    Arc::new(ToolDef {
        audience: Default::default(),
        name: name.into(),
        description: format!("{name} tool"),
        input_schema: json!({"type": "object", "properties": {}}),
        provenance: None,
    })
}

/// The tools before an MCP server connects, and after.
fn tools_before() -> Vec<Arc<ToolDef>> {
    vec![tool("lookup")]
}

fn tools_after() -> Vec<Arc<ToolDef>> {
    vec![tool("lookup"), tool("search_issues")]
}

/// What one streamed turn produced.
#[derive(Debug, Default)]
struct Turn {
    reasoning: String,
    signature: Option<String>,
    text: String,
    /// Each tool call: its id, name and arguments.
    tool_calls: Vec<(String, String, Value)>,
    done: Option<LlmDoneOutcome>,
    error: Option<LlmError>,
}

impl Turn {
    fn succeeded(&self) -> bool {
        self.error.is_none() && matches!(self.done, Some(LlmDoneOutcome::Success { .. }))
    }

    /// The assistant message the turn committed, as Meerkat replays it.
    fn assistant(&self) -> Message {
        let mut blocks = vec![
            AssistantBlock::Reasoning {
                text: self.reasoning.clone(),
                meta: Some(Box::new(ProviderMeta::Anthropic {
                    signature: self
                        .signature
                        .clone()
                        .expect("the endpoint signs every thinking block"),
                })),
            },
            AssistantBlock::Text {
                text: self.text.clone(),
                meta: None,
            },
        ];
        for (id, name, args) in &self.tool_calls {
            blocks.push(AssistantBlock::ToolUse {
                id: id.clone(),
                name: name.clone(),
                args: serde_json::value::to_raw_value(args).unwrap(),
                meta: None,
            });
        }
        let stop_reason = if self.tool_calls.is_empty() {
            StopReason::EndTurn
        } else {
            StopReason::ToolUse
        };
        Message::BlockAssistant(BlockAssistantMessage::new(blocks, stop_reason))
    }

    /// The tool results that answer the turn's tool calls.
    fn tool_results(&self) -> Message {
        Message::tool_results(
            self.tool_calls
                .iter()
                .map(|(id, _, _)| ToolResult::new(id.clone(), "alpha is 7".to_string(), false))
                .collect(),
        )
    }
}

async fn run(client: &AnthropicClient, request: &LlmRequest) -> Turn {
    let mut stream = client.stream(request);
    let mut turn = Turn::default();
    while let Some(event) = stream.next().await {
        match event {
            Ok(LlmEvent::ReasoningDelta { delta }) => turn.reasoning.push_str(&delta),
            Ok(LlmEvent::ReasoningComplete {
                meta: Some(meta), ..
            }) => {
                if let ProviderMeta::Anthropic { signature } = *meta {
                    turn.signature = Some(signature);
                }
            }
            Ok(LlmEvent::TextDelta { delta, .. }) => turn.text.push_str(&delta),
            Ok(LlmEvent::ToolCallComplete { id, name, args, .. }) => {
                turn.tool_calls.push((id, name, args));
            }
            Ok(LlmEvent::Done { outcome }) => {
                if let LlmDoneOutcome::Error { error } = outcome {
                    turn.error = Some(error);
                } else {
                    turn.done = Some(outcome);
                }
                break;
            }
            Ok(_) => {}
            Err(error) => {
                turn.error = Some(error);
                break;
            }
        }
    }
    turn
}

fn first_question() -> Message {
    Message::User(UserMessage::text("What is 6 * 7?"))
}

fn second_question() -> Message {
    Message::User(UserMessage::text("And 7 * 8?"))
}

/// A two-turn session on `model` whose second turn runs after an MCP server
/// connected (`tools_after`), each request shaped by `shape`. Returns both
/// turns.
async fn session_across_a_tools_change(
    client: &AnthropicClient,
    model: &str,
    shape: impl Fn(LlmRequest) -> LlmRequest,
) -> (Turn, Turn) {
    let first = run(
        client,
        &shape(LlmRequest::new(model, vec![first_question()]).with_tools(tools_before())),
    )
    .await;
    assert!(
        first.succeeded(),
        "{model}: the first turn completes: {first:?}"
    );
    let second = run(
        client,
        &shape(
            LlmRequest::new(
                model,
                vec![first_question(), first.assistant(), second_question()],
            )
            .with_tools(tools_after()),
        ),
    )
    .await;
    (first, second)
}

fn adaptive(request: LlmRequest) -> LlmRequest {
    request.with_anthropic_tag_merge(|tag| tag.thinking = Some(AnthropicThinkingConfig::Adaptive))
}

fn expected_binding() -> Value {
    json!({"type": "adaptive", "block_binding": {"prefix_mismatch_behavior": "drop_block"}})
}

/// Row (a): the default configuration (the default Anthropic model, Claude
/// Opus 5.5, with no thinking configured; the model thinks by default) keeps
/// working after an MCP server connects mid-session on an account that
/// enforces thinking-block binding. The request names the model's own
/// default, adaptive, beside the control.
#[tokio::test]
async fn default_config_survives_a_tools_change_on_an_enforcing_account() {
    let (account, client, server) = spawn_account().await;
    let (_, second) =
        session_across_a_tools_change(&client, "claude-opus-5-5", |request| request).await;
    let sent = account.last();
    server.abort();
    assert!(
        second.succeeded(),
        "the turn after the tools change completes: {second:?}"
    );
    assert_eq!(sent.body["thinking"], expected_binding(), "{}", sent.body);
    assert!(sent.carries_binding_beta(), "{:?}", sent.betas);
}

/// Claude Fable 5.2 and Mythos 5.2 run the prefix check on every account,
/// whatever its age, so the enforcing account is every account for them.
/// With thinking omitted, every request of a session names the always-on
/// adaptive default beside the control and carries the beta, the history is
/// replayed whole, and the turn after an MCP server connects completes: the
/// API drops the earlier turn's thinking instead of refusing the request.
/// <https://platform.claude.com/docs/en/models/fable-5-2/whats-new-fable-5-2>
#[tokio::test]
async fn fable_5_2_and_mythos_5_2_with_thinking_omitted_survive_a_tools_change() {
    let (account, client, server) = spawn_account().await;
    for model in ["claude-fable-5-2", "claude-mythos-5-2"] {
        let before = account.all().len();
        let (_, second) = session_across_a_tools_change(&client, model, unchanged).await;
        let sent = account.all().split_off(before);
        assert!(
            second.succeeded(),
            "{model}: the turn after the tools change completes: {second:?}"
        );
        assert_eq!(sent.len(), 2, "{model}: one request a turn: {sent:#?}");
        for request in &sent {
            assert_eq!(
                request.body["thinking"],
                expected_binding(),
                "{model}: {}",
                request.body
            );
            assert!(
                request.carries_binding_beta(),
                "{model}: {:?}",
                request.betas
            );
        }
        assert_eq!(
            replayed_thinking_blocks(&sent[1].body),
            1,
            "{model}: the completed turn's thinking is replayed: {}",
            sent[1].body
        );
    }
    server.abort();
}

/// A session that moves from Claude Fable 5.1 onto Claude Fable 5.2 keeps
/// sending the earlier turn's thinking: Fable 5.2 reads Fable 5.1's blocks,
/// and its requests carry the control, so nothing is held back. The API
/// drops, without an error, any block the serving model cannot read.
/// <https://platform.claude.com/docs/en/build-with-claude/preserved-thinking>
#[tokio::test]
async fn a_session_moving_onto_fable_5_2_keeps_the_earlier_thinking() {
    let (account, client, server) = spawn_account().await;
    let first = run(
        &client,
        &LlmRequest::new("claude-fable-5-1", vec![first_question()]).with_tools(tools_before()),
    )
    .await;
    assert!(first.succeeded(), "{first:?}");
    let second = run(
        &client,
        &LlmRequest::new(
            "claude-fable-5-2",
            vec![first_question(), first.assistant(), second_question()],
        )
        .with_tools(tools_before()),
    )
    .await;
    let sent = account.last();
    server.abort();
    assert!(second.succeeded(), "{second:?}");
    assert_eq!(sent.body["thinking"], expected_binding(), "{}", sent.body);
    assert!(sent.carries_binding_beta(), "{:?}", sent.betas);
    assert_eq!(
        replayed_thinking_blocks(&sent.body),
        1,
        "the Fable 5.1 turn's thinking is sent: {}",
        sent.body
    );
}

/// A prefix-checked model served under an id the catalog does not carry
/// (Claude Mythos 5.1 here; a proxy alias or a custom `[models.<id>]` entry
/// alike) with thinking configured gets the control and the beta on every
/// request, the session's first included, so the request shape never changes
/// and the session survives a tools change.
#[tokio::test]
async fn an_uncatalogued_prefix_checked_id_survives_a_tools_change() {
    let model = "claude-mythos-5-1";
    assert!(!CATALOGUED_PREFIX_CHECKED.contains(&model));
    let (account, client, server) = spawn_account().await;
    let (_, second) = session_across_a_tools_change(&client, model, adaptive).await;
    let sent = account.all();
    server.abort();
    assert!(
        second.succeeded(),
        "the turn after the tools change completes: {second:?}"
    );
    assert_eq!(sent.len(), 2, "{sent:?}");
    for request in &sent {
        assert_eq!(
            request.body["thinking"],
            expected_binding(),
            "{}",
            request.body
        );
        assert!(request.carries_binding_beta(), "{:?}", request.betas);
    }
}

/// The same uncatalogued id with thinking omitted gets no control, because no
/// catalog row names its default (the alias may serve an `enabled`-only
/// model). Its request carries no thinking and no beta, and survives the tools
/// change because it replays no thinking block from the completed turn.
#[tokio::test]
async fn an_uncatalogued_id_with_thinking_omitted_survives_a_tools_change() {
    let model = "claude-mythos-5-1";
    assert!(!CATALOGUED_PREFIX_CHECKED.contains(&model));
    let (account, client, server) = spawn_account().await;
    let (_, second) = session_across_a_tools_change(&client, model, |request| request).await;
    let sent = account.last();
    let requests = account.all().len();
    server.abort();
    assert!(
        second.succeeded(),
        "the turn after the tools change completes: {second:?}"
    );
    assert_eq!(requests, 2, "one request a turn, none refused");
    assert!(sent.body.get("thinking").is_none(), "{}", sent.body);
    assert!(!sent.carries_binding_beta(), "{:?}", sent.betas);
    assert_eq!(replayed_thinking_blocks(&sent.body), 0, "{}", sent.body);
}

/// Row (c): explicit adaptive thinking carries the control on every
/// catalogued prefix-checked model, and the session survives a tools change.
#[tokio::test]
async fn explicit_adaptive_thinking_carries_the_binding_on_every_binding_model() {
    let (account, client, server) = spawn_account().await;
    for model in CATALOGUED_PREFIX_CHECKED {
        let (_, second) = session_across_a_tools_change(&client, model, adaptive).await;
        let sent = account.last();
        assert!(
            second.succeeded(),
            "{model}: the turn after the tools change completes: {second:?}"
        );
        assert_eq!(sent.body["thinking"], expected_binding(), "{model}");
        assert!(sent.carries_binding_beta(), "{model}: {:?}", sent.betas);
    }
    server.abort();
}

/// Row (b): a model whose default is no thinking, with thinking omitted, sends
/// no thinking and no beta, also when its history replays a thinking block
/// from an earlier configured turn.
#[tokio::test]
async fn default_off_model_with_thinking_omitted_sends_no_thinking_and_no_beta() {
    let (account, client, server) = spawn_account().await;
    for model in ["claude-opus-4-8", "claude-opus-4-7", "claude-sonnet-4-6"] {
        let first = run(
            &client,
            &adaptive(LlmRequest::new(model, vec![first_question()])).with_tools(tools_before()),
        )
        .await;
        assert!(first.succeeded(), "{model}: {first:?}");
        let second = run(
            &client,
            &LlmRequest::new(
                model,
                vec![first_question(), first.assistant(), second_question()],
            )
            .with_tools(tools_after()),
        )
        .await;
        let sent = account.last();
        assert!(second.succeeded(), "{model}: {second:?}");
        assert!(
            sent.body.get("thinking").is_none(),
            "{model}: {}",
            sent.body
        );
        assert!(!sent.carries_binding_beta(), "{model}: {:?}", sent.betas);
        assert_eq!(
            sent.body["messages"][1]["content"][0]["type"], "thinking",
            "{model}: the earlier thinking block is replayed unchanged"
        );
    }
    server.abort();
}

/// Row (d): `between_tools` (Claude Sonnet 5.5) takes no other field, so it
/// never carries the control; neither does `disabled` (Claude Haiku 5.5, which
/// refuses the control beside it). The endpoint answers 400 to either
/// combination. The replayed block comes from the session's first turn: a
/// `between_tools` progress update, and an adaptive turn before thinking was
/// turned off.
#[tokio::test]
async fn between_tools_never_carries_the_binding() {
    let (account, client, server) = spawn_account().await;
    for (model, first_thinking, thinking) in [
        (
            "claude-sonnet-5-5",
            AnthropicThinkingConfig::BetweenTools,
            AnthropicThinkingConfig::BetweenTools,
        ),
        (
            "claude-haiku-5-5",
            AnthropicThinkingConfig::Adaptive,
            AnthropicThinkingConfig::Disabled,
        ),
    ] {
        let first = run(
            &client,
            &LlmRequest::new(model, vec![first_question()])
                .with_tools(tools_before())
                .with_anthropic_tag_merge(|tag| tag.thinking = Some(first_thinking.clone())),
        )
        .await;
        assert!(first.succeeded(), "{model}: {first:?}");
        let second = run(
            &client,
            &LlmRequest::new(
                model,
                vec![first_question(), first.assistant(), second_question()],
            )
            .with_tools(tools_before())
            .with_anthropic_tag_merge(|tag| tag.thinking = Some(thinking.clone())),
        )
        .await;
        let sent = account.last();
        assert!(second.succeeded(), "{model}: {second:?}");
        assert_eq!(
            sent.body["thinking"],
            json!({"type": thinking.wire_type()}),
            "{model}"
        );
        assert!(!sent.carries_binding_beta(), "{model}: {:?}", sent.betas);
    }
    server.abort();
}

/// A request that forces a tool call with thinking omitted keeps its shape:
/// no thinking and no binding beta. This client refuses a forced call beside
/// explicit thinking, so it does not name the model's default thinking there
/// either; a forced call skips thinking on every model that accepts one
/// (Claude Haiku 5.5 among the binding models).
/// <https://platform.claude.com/docs/en/agents-and-tools/tool-use/define-tools>
#[tokio::test]
async fn forced_tool_call_with_thinking_omitted_sends_no_thinking_and_no_beta() {
    let (account, client, server) = spawn_account().await;
    let turn = run(
        &client,
        &LlmRequest::new("claude-haiku-5-5", vec![first_question()])
            .with_tools(tools_before())
            .with_tool_choice(ToolChoice::Required),
    )
    .await;
    let sent = account.last();
    server.abort();
    assert!(turn.succeeded(), "{turn:?}");
    assert!(sent.body.get("thinking").is_none(), "{}", sent.body);
    assert!(!sent.carries_binding_beta(), "{:?}", sent.betas);
    assert_eq!(sent.body["tool_choice"], json!({"type": "any"}));
}

/// Collects the events logged on this thread.
#[derive(Clone, Default)]
struct Logged(Arc<Mutex<Vec<String>>>);

impl tracing::Subscriber for Logged {
    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        struct Fields<'a>(&'a mut String);
        impl tracing::field::Visit for Fields<'_> {
            fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
                use std::fmt::Write;
                let _ = write!(self.0, " {}={value:?}", field.name());
            }
        }
        let mut line = event.metadata().level().to_string();
        event.record(&mut Fields(&mut line));
        self.0.lock().unwrap().push(line);
    }
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
}

/// Dropped thinking blocks are visible: the response's
/// `input_transformations` (the lost reasoning, and the prompt cache that
/// restarts at the first dropped block) is recorded as a warning that names
/// each block and why it was dropped.
#[tokio::test]
async fn reported_thinking_drops_are_logged() {
    let logged = Logged::default();
    let _subscriber = tracing::subscriber::set_default(logged.clone());
    let (_, client, server) = spawn_account().await;
    let (_, second) = session_across_a_tools_change(&client, "claude-sonnet-5-5", adaptive).await;
    server.abort();
    assert!(second.succeeded(), "{second:?}");
    let lines = logged.0.lock().unwrap().clone();
    assert!(
        lines.iter().any(|line| line.starts_with("WARN")
            && line.contains("messages.1.content.0")
            && line.contains("thinking_dropped")
            && line.contains("prefix_binding_mismatch")),
        "a warning names the dropped block and its reason: {lines:#?}"
    );
}

async fn fixed_stream(State(events): State<Arc<Vec<Value>>>) -> Response {
    event_stream(&events)
}

/// After a mid-stream server-side fallback the final `message_delta`
/// carries the report again, with the serving model's entries; a report
/// there (beside its delta or inside it) is logged too, and an empty report
/// on `message_start` logs nothing.
#[tokio::test]
async fn a_report_on_the_final_message_delta_is_logged() {
    let entries = json!([{
        "type": "thinking_dropped",
        "path": "messages.1.content.0",
        "reason": "model_binding_mismatch"
    }]);
    for message_delta in [
        json!({"type": "message_delta", "delta": {"stop_reason": "end_turn"},
               "usage": {"output_tokens": 3}, "input_transformations": entries}),
        json!({"type": "message_delta",
               "delta": {"stop_reason": "end_turn", "input_transformations": entries},
               "usage": {"output_tokens": 3}}),
    ] {
        let logged = Logged::default();
        let _subscriber = tracing::subscriber::set_default(logged.clone());
        let events = Arc::new(vec![
            json!({"type": "message_start", "message": {
                "usage": {"input_tokens": 5, "output_tokens": 0},
                "input_transformations": []
            }}),
            json!({"type": "content_block_start", "index": 0,
                   "content_block": {"type": "text", "text": ""}}),
            json!({"type": "content_block_delta", "index": 0,
                   "delta": {"type": "text_delta", "text": "42."}}),
            json!({"type": "content_block_stop", "index": 0}),
            message_delta.clone(),
            json!({"type": "message_stop"}),
        ]);
        let app = Router::new()
            .route("/v1/messages", post(fixed_stream))
            .with_state(events);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let client = AnthropicClient::builder("test-key".to_string())
            .base_url(format!("http://{addr}"))
            .build()
            .unwrap();
        let turn = run(
            &client,
            &LlmRequest::new("claude-opus-5-5", vec![first_question()]),
        )
        .await;
        server.abort();
        assert!(turn.succeeded(), "{message_delta}: {turn:?}");
        let warnings: Vec<String> = logged
            .0
            .lock()
            .unwrap()
            .iter()
            .filter(|line| line.starts_with("WARN"))
            .cloned()
            .collect();
        assert_eq!(warnings.len(), 1, "{message_delta}: {warnings:#?}");
        assert!(
            warnings[0].contains("message_delta")
                && warnings[0].contains("messages.1.content.0")
                && warnings[0].contains("model_binding_mismatch"),
            "{message_delta}: {warnings:#?}"
        );
    }
}

fn unchanged(request: LlmRequest) -> LlmRequest {
    request
}

fn enabled(request: LlmRequest) -> LlmRequest {
    request.with_anthropic_tag_merge(|tag| {
        tag.thinking = Some(AnthropicThinkingConfig::Enabled {
            budget_tokens: 2048,
        });
    })
}

/// One tool round on `model`: the first request asks for the `lookup` tool
/// under `tools_before`, and the continuation returns its result under
/// `tools`, each request shaped by `shape`. Returns the first turn, the
/// continuation, and every request the continuation sent.
async fn tool_round(
    account: &EnforcingAccount,
    client: &AnthropicClient,
    model: &str,
    shape: impl Fn(LlmRequest) -> LlmRequest,
    tools: Vec<Arc<ToolDef>>,
) -> (Turn, Turn, Vec<Seen>) {
    let question = Message::User(UserMessage::text(format!("What is alpha? {TOOL_CUE}")));
    let first = run(
        client,
        &shape(LlmRequest::new(model, vec![question.clone()]).with_tools(tools_before())),
    )
    .await;
    assert!(
        first.succeeded(),
        "{model}: the tool call completes: {first:?}"
    );
    assert_eq!(first.tool_calls.len(), 1, "{model}: {first:?}");
    let before = account.all().len();
    let continuation = run(
        client,
        &shape(
            LlmRequest::new(
                model,
                vec![question, first.assistant(), first.tool_results()],
            )
            .with_tools(tools),
        ),
    )
    .await;
    let sent = account.all().split_off(before);
    (first, continuation, sent)
}

/// On a route that does not take the binding control (Amazon Bedrock, the
/// GitHub Copilot route), a thinking session survives a tools change between
/// turns on an account that enforces thinking-block binding. The request
/// carries no control and no beta (nothing such a route might refuse) and
/// replays no thinking block of the completed turn, which Anthropic allows.
#[tokio::test]
async fn a_route_without_the_control_survives_a_tools_change_between_turns() {
    let (account, client, server) = spawn_account_without_the_control().await;
    let shapes: [(&str, fn(LlmRequest) -> LlmRequest); 2] =
        [("thinking omitted", unchanged), ("adaptive", adaptive)];
    for model in CATALOGUED_PREFIX_CHECKED
        .iter()
        .copied()
        .chain(["claude-mythos-5-1"])
    {
        for (label, shape) in shapes {
            let before = account.all().len();
            let (_, second) = session_across_a_tools_change(&client, model, shape).await;
            let sent = account.last();
            assert!(
                second.succeeded(),
                "{model}, {label}: the turn after the tools change completes: {second:?}"
            );
            assert_eq!(
                account.all().len() - before,
                2,
                "{model}, {label}: one request a turn, none refused"
            );
            assert!(
                sent.body["thinking"].get("block_binding").is_none(),
                "{model}, {label}: {}",
                sent.body
            );
            assert!(
                !sent.carries_binding_beta(),
                "{model}, {label}: {:?}",
                sent.betas
            );
            assert_eq!(
                replayed_thinking_blocks(&sent.body),
                0,
                "{model}, {label}: {}",
                sent.body
            );
        }
    }
    server.abort();
}

/// A tools change inside a turn (a server finishing its connect, a deferred
/// tool loaded) on a route without the control: the continuation keeps the
/// thinking of the turn in progress, the enforcing account refuses it, and the
/// client sends it once more without its thinking blocks. The warning names
/// how many blocks it removed and no model text.
#[tokio::test]
async fn a_route_without_the_control_survives_a_tools_change_within_a_turn() {
    let logged = Logged::default();
    let _subscriber = tracing::subscriber::set_default(logged.clone());
    let (account, client, server) = spawn_account_without_the_control().await;
    let rounds: [(&str, fn(LlmRequest) -> LlmRequest); 2] = [
        ("claude-opus-5-5", unchanged),
        ("claude-mythos-5-1", adaptive),
    ];
    for (model, shape) in rounds {
        let (_, continuation, sent) =
            tool_round(&account, &client, model, shape, tools_after()).await;
        assert!(
            continuation.succeeded(),
            "{model}: the continuation completes: {continuation:?}"
        );
        assert_eq!(
            sent.len(),
            2,
            "{model}: one refused attempt, then one resend: {sent:#?}"
        );
        assert_eq!(
            replayed_thinking_blocks(&sent[0].body),
            1,
            "{model}: the turn in progress keeps its thinking: {}",
            sent[0].body
        );
        assert_eq!(
            replayed_thinking_blocks(&sent[1].body),
            0,
            "{model}: the resend replays no thinking: {}",
            sent[1].body
        );
        for attempt in &sent {
            assert!(
                attempt.body["thinking"].get("block_binding").is_none(),
                "{model}: {}",
                attempt.body
            );
            assert!(
                !attempt.carries_binding_beta(),
                "{model}: {:?}",
                attempt.betas
            );
        }
    }
    server.abort();
    let lines = logged.0.lock().unwrap().clone();
    let resends: Vec<&String> = lines
        .iter()
        .filter(|line| line.starts_with("WARN") && line.contains("thinking_blocks_removed=1"))
        .collect();
    assert_eq!(resends.len(), 2, "one warning per resend: {lines:#?}");
    assert!(
        lines.iter().all(|line| !line.contains("Multiply")),
        "no model text is logged: {lines:#?}"
    );
}

/// A continuation with nothing changed is sent once, with the thinking block
/// of the turn in progress exactly as the model returned it.
#[tokio::test]
async fn a_route_without_the_control_keeps_the_thinking_of_the_turn_in_progress() {
    let (account, client, server) = spawn_account_without_the_control().await;
    let (first, continuation, sent) = tool_round(
        &account,
        &client,
        "claude-opus-5-5",
        unchanged,
        tools_before(),
    )
    .await;
    server.abort();
    assert!(continuation.succeeded(), "{continuation:?}");
    assert_eq!(sent.len(), 1, "{sent:#?}");
    let replayed = &sent[0].body["messages"][1]["content"][0];
    assert_eq!(replayed["type"], "thinking", "{}", sent[0].body);
    assert_eq!(
        replayed["signature"].as_str(),
        first.signature.as_deref(),
        "{}",
        sent[0].body
    );
}

/// A resend that is refused too surfaces the error of the request the caller
/// made, not the resend's: nothing is masked.
#[tokio::test]
async fn a_refused_resend_surfaces_the_original_error() {
    let (account, client, server) = spawn_account_without_the_control().await;
    let mut tools = tools_after();
    tools.push(tool(REFUSED_TOOL));
    let (_, continuation, sent) =
        tool_round(&account, &client, "claude-opus-5-5", unchanged, tools).await;
    server.abort();
    assert_eq!(sent.len(), 2, "one attempt, then one resend: {sent:#?}");
    assert_eq!(
        replayed_thinking_blocks(&sent[1].body),
        0,
        "{}",
        sent[1].body
    );
    let error = format!("{:?}", continuation.error);
    assert!(
        error.contains("replayed 1 thinking blocks"),
        "the original error surfaces: {error}"
    );
}

/// A refused request that replayed no thinking is not sent again.
#[tokio::test]
async fn a_refused_request_that_replayed_no_thinking_is_not_sent_again() {
    let (account, client, server) = spawn_account_without_the_control().await;
    let turn = run(
        &client,
        &LlmRequest::new("claude-opus-5-5", vec![first_question()])
            .with_tools(vec![tool("lookup"), tool(REFUSED_TOOL)]),
    )
    .await;
    let sent = account.all();
    server.abort();
    assert!(!turn.succeeded(), "{turn:?}");
    assert_eq!(sent.len(), 1, "{sent:#?}");
    assert!(
        format!("{:?}", turn.error).contains("replayed 0 thinking blocks"),
        "{turn:?}"
    );
}

/// A model without the prefix check keeps every replayed thinking block on a
/// route without the control.
#[tokio::test]
async fn a_model_without_the_prefix_check_replays_earlier_turns_on_a_route_without_the_control() {
    let (account, client, server) = spawn_account_without_the_control().await;
    for model in ["claude-opus-4-8", "claude-sonnet-4-6"] {
        let (_, second) = session_across_a_tools_change(&client, model, adaptive).await;
        let sent = account.last();
        assert!(second.succeeded(), "{model}: {second:?}");
        assert_eq!(
            sent.body["messages"][1]["content"][0]["type"], "thinking",
            "{model}: the earlier thinking block is replayed unchanged"
        );
    }
    server.abort();
}

/// Requests on the Claude API that cannot carry the control (`between_tools`
/// on Claude Sonnet 5.5, `disabled` on Claude Haiku 5.5) survive a tools change
/// the same way: no thinking block of the completed turn is replayed.
#[tokio::test]
async fn direct_requests_that_cannot_carry_the_control_survive_a_tools_change() {
    let (account, client, server) = spawn_account().await;
    for (model, thinking) in [
        ("claude-sonnet-5-5", AnthropicThinkingConfig::BetweenTools),
        ("claude-haiku-5-5", AnthropicThinkingConfig::Disabled),
    ] {
        let first = run(
            &client,
            &adaptive(LlmRequest::new(model, vec![first_question()]).with_tools(tools_before())),
        )
        .await;
        assert!(first.succeeded(), "{model}: {first:?}");
        let before = account.all().len();
        let second = run(
            &client,
            &LlmRequest::new(
                model,
                vec![first_question(), first.assistant(), second_question()],
            )
            .with_tools(tools_after())
            .with_anthropic_tag_merge(|tag| tag.thinking = Some(thinking.clone())),
        )
        .await;
        let sent = account.last();
        assert!(
            second.succeeded(),
            "{model}: the turn after the tools change completes: {second:?}"
        );
        assert_eq!(
            account.all().len() - before,
            1,
            "{model}: one request, none refused"
        );
        assert_eq!(
            sent.body["thinking"],
            json!({"type": thinking.wire_type()}),
            "{model}"
        );
        assert!(!sent.carries_binding_beta(), "{model}: {:?}", sent.betas);
        assert_eq!(
            replayed_thinking_blocks(&sent.body),
            0,
            "{model}: {}",
            sent.body
        );
    }
    server.abort();
}

/// Manual (`enabled`) thinking requires the continued assistant message to
/// start with a thinking block, and adaptive thinking does not, so the resend
/// that removes the thinking blocks names the model's own default (thinking
/// omitted) for that one request.
/// <https://platform.claude.com/docs/en/build-with-claude/thinking>
#[tokio::test]
async fn a_resend_after_manual_thinking_names_the_model_default() {
    let (account, client, server) = spawn_account_without_the_control().await;
    let (_, continuation, sent) = tool_round(
        &account,
        &client,
        "claude-mythos-5-1",
        enabled,
        tools_after(),
    )
    .await;
    server.abort();
    assert!(
        continuation.succeeded(),
        "the continuation completes: {continuation:?}"
    );
    assert_eq!(sent.len(), 2, "{sent:#?}");
    assert_eq!(
        sent[0].body["thinking"]["type"], "enabled",
        "{}",
        sent[0].body
    );
    assert!(
        sent[1].body.get("thinking").is_none(),
        "the resend names the model's default: {}",
        sent[1].body
    );
    assert_eq!(
        replayed_thinking_blocks(&sent[1].body),
        0,
        "{}",
        sent[1].body
    );
}

/// The turn boundary on a route without the control: a request replays
/// thinking only for the turn in progress, the assistant messages after the
/// last user turn that carries no tool result (a steer written after tool
/// results stays in the turn). A completed turn keeps its server compaction
/// block and loses its thinking and redacted thinking; an assistant message
/// left with nothing to send is not sent.
#[test]
fn completed_turns_replay_no_thinking_on_a_route_without_the_control() {
    let client = AnthropicClient::builder("test-key".to_string())
        .thinking_binding_controls_supported(false)
        .build()
        .unwrap();
    let reasoning = |meta: ProviderMeta| AssistantBlock::Reasoning {
        text: String::new(),
        meta: Some(Box::new(meta)),
    };
    let thinking = |signature: &str| {
        reasoning(ProviderMeta::Anthropic {
            signature: signature.to_string(),
        })
    };
    let call = |id: &str| AssistantBlock::ToolUse {
        id: id.to_string(),
        name: "lookup".to_string(),
        args: serde_json::value::RawValue::from_string("{}".to_string()).unwrap(),
        meta: None,
    };
    let assistant = |blocks: Vec<AssistantBlock>, stop_reason: StopReason| {
        Message::BlockAssistant(BlockAssistantMessage::new(blocks, stop_reason))
    };
    let results = |id: &str| {
        Message::tool_results(vec![ToolResult::new(
            id.to_string(),
            "found".to_string(),
            false,
        )])
    };
    let request = LlmRequest::new(
        "claude-opus-5-5",
        vec![
            Message::User(UserMessage::text("first")),
            assistant(
                vec![
                    reasoning(ProviderMeta::AnthropicCompaction {
                        content: "summary".to_string(),
                    }),
                    thinking("sig-1"),
                    reasoning(ProviderMeta::AnthropicRedacted {
                        data: "redacted-1".to_string(),
                    }),
                    AssistantBlock::Text {
                        text: "one".to_string(),
                        meta: None,
                    },
                ],
                StopReason::EndTurn,
            ),
            Message::User(UserMessage::text("second")),
            assistant(vec![thinking("sig-2")], StopReason::EndTurn),
            Message::User(UserMessage::text("third")),
            assistant(vec![thinking("sig-3"), call("call-3")], StopReason::ToolUse),
            results("call-3"),
            Message::User(UserMessage::text("also check the tests")),
            assistant(vec![thinking("sig-4"), call("call-4")], StopReason::ToolUse),
            results("call-4"),
        ],
    );
    let body = client.build_request_body(&request).unwrap();
    let messages = body["messages"].as_array().unwrap();
    let types = |message: &Value| -> Vec<String> {
        message["content"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|block| block["type"].as_str().unwrap_or_default().to_string())
            .collect()
    };
    assert_eq!(
        messages.len(),
        9,
        "the completed assistant message holding only thinking is not sent: {body}"
    );
    assert_eq!(types(&messages[1]), ["compaction", "text"], "{body}");
    assert_eq!(types(&messages[4]), ["thinking", "tool_use"], "{body}");
    assert_eq!(messages[4]["content"][0]["signature"], "sig-3", "{body}");
    assert_eq!(types(&messages[7]), ["thinking", "tool_use"], "{body}");
    assert_eq!(messages[7]["content"][0]["signature"], "sig-4", "{body}");
}

/// The history of a turn whose tool round ended in an answer holding only
/// thinking (an empty answer the agent loop accepts once the turn called a
/// tool), followed by a new user turn, on a route without the control.
async fn history_after_a_thinking_only_answer(
    account: &EnforcingAccount,
    client: &AnthropicClient,
    model: &str,
    shape: fn(LlmRequest) -> LlmRequest,
) -> Vec<Message> {
    let (first, continuation, _) = tool_round(account, client, model, shape, tools_before()).await;
    assert!(continuation.succeeded(), "{model}: {continuation:?}");
    let mut thinking_only_answer = continuation;
    thinking_only_answer.text.clear();
    vec![
        Message::User(UserMessage::text(format!("What is alpha? {TOOL_CUE}"))),
        first.assistant(),
        first.tool_results(),
        thinking_only_answer.assistant(),
        second_question(),
    ]
}

/// A completed answer holding only thinking has nothing else to send and is
/// left out, so the API joins the tool results before it with the new user
/// turn: the tool round before it is the assistant message the request
/// continues, and it keeps its thinking, which manual `enabled` thinking
/// requires there.
#[tokio::test]
async fn a_tool_round_continued_after_a_left_out_answer_keeps_its_thinking() {
    let (account, client, server) = spawn_account_without_the_control().await;
    let model = "claude-mythos-5-1";
    let history = history_after_a_thinking_only_answer(&account, &client, model, enabled).await;
    let before = account.all().len();
    let second = run(
        &client,
        &enabled(LlmRequest::new(model, history).with_tools(tools_before())),
    )
    .await;
    let sent = account.all().split_off(before);
    server.abort();
    assert!(
        second.succeeded(),
        "the next turn completes: {second:?}; sent: {:#?}",
        sent.iter()
            .map(|seen| seen.body["messages"].clone())
            .collect::<Vec<_>>()
    );
    assert_eq!(sent.len(), 1, "{sent:#?}");
    let messages = sent[0].body["messages"].as_array().unwrap();
    assert_eq!(
        messages.len(),
        4,
        "the answer holding only thinking is left out: {}",
        sent[0].body
    );
    assert_eq!(
        messages[1]["content"][0]["type"], "thinking",
        "the continued tool round keeps its thinking: {}",
        sent[0].body
    );
}

/// The same history under adaptive thinking, across a tools change: the next
/// turn completes.
#[tokio::test]
async fn a_left_out_answer_then_an_adaptive_turn_across_a_tools_change() {
    let (account, client, server) = spawn_account_without_the_control().await;
    let model = "claude-mythos-5-1";
    let history = history_after_a_thinking_only_answer(&account, &client, model, adaptive).await;
    let before = account.all().len();
    let second = run(
        &client,
        &adaptive(LlmRequest::new(model, history).with_tools(tools_after())),
    )
    .await;
    let sent = account.all().split_off(before);
    server.abort();
    assert!(second.succeeded(), "the next turn completes: {second:?}");
    for attempt in &sent {
        assert!(
            attempt.body["thinking"].get("block_binding").is_none(),
            "{}",
            attempt.body
        );
        assert!(!attempt.carries_binding_beta(), "{:?}", attempt.betas);
    }
}

/// A turn opened by a system notice (a background job that finished, a
/// message from a peer) is a turn of its own: across a tools change it
/// replays no thinking of the turn before it and takes one request.
#[tokio::test]
async fn a_turn_opened_by_a_notice_replays_no_earlier_thinking() {
    let (account, client, server) = spawn_account_without_the_control().await;
    let model = "claude-opus-5-5";
    let first = run(
        &client,
        &LlmRequest::new(model, vec![first_question()]).with_tools(tools_before()),
    )
    .await;
    assert!(first.succeeded(), "{first:?}");
    let notice = Message::SystemNotice(SystemNoticeMessage::new(
        SystemNoticeKind::Generic,
        "a background job finished",
    ));
    let before = account.all().len();
    let second = run(
        &client,
        &LlmRequest::new(model, vec![first_question(), first.assistant(), notice])
            .with_tools(tools_after()),
    )
    .await;
    let sent = account.all().split_off(before);
    server.abort();
    assert!(
        second.succeeded(),
        "the turn after the tools change completes: {second:?}"
    );
    assert_eq!(sent.len(), 1, "one request, none refused: {sent:#?}");
    assert_eq!(
        replayed_thinking_blocks(&sent[0].body),
        0,
        "{}",
        sent[0].body
    );
}

/// Cache breakpoint claims follow the messages as sent. Under
/// `system_and_conversation` on a route without the control, a transcript
/// whose completed answer holds only thinking (left out of the request) gets
/// no claim, since no lowered boundary maps back to the transcript; one whose
/// completed turn only loses its thinking keeps its claims.
#[test]
fn cache_claims_follow_the_messages_as_sent() {
    let client = AnthropicClient::builder("test-key".to_string())
        .thinking_binding_controls_supported(false)
        .build()
        .unwrap();
    let thinking = |signature: &str| AssistantBlock::Reasoning {
        text: String::new(),
        meta: Some(Box::new(ProviderMeta::Anthropic {
            signature: signature.to_string(),
        })),
    };
    let text = |text: &str| AssistantBlock::Text {
        text: text.to_string(),
        meta: None,
    };
    let assistant = |blocks: Vec<AssistantBlock>| {
        Message::BlockAssistant(BlockAssistantMessage::new(blocks, StopReason::EndTurn))
    };
    let user = |text: &str| Message::User(UserMessage::text(text));
    let claims = |messages: Vec<Message>| {
        let request =
            LlmRequest::new("claude-opus-5-5", messages).with_anthropic_tag_merge(|tag| {
                tag.cache_control = Some(AnthropicCacheControlPolicy::SystemAndConversation);
            });
        client
            .authored_cache_breakpoints(&request, &request.messages)
            .unwrap()
    };
    let left_out = claims(vec![
        user("first"),
        assistant(vec![thinking("sig-1"), text("one")]),
        user("second"),
        assistant(vec![thinking("sig-2")]),
        user("third"),
    ]);
    assert!(
        left_out.is_empty(),
        "no claim maps a lowered boundary back to the transcript: {left_out:?}"
    );
    let stripped = claims(vec![
        user("first"),
        assistant(vec![thinking("sig-1"), text("one")]),
        user("second"),
    ]);
    assert!(
        !stripped.is_empty(),
        "a transcript whose messages are all sent keeps its claims"
    );
}

/// A resend that fails with an error the client retries (an overload here;
/// a rate limit or a network error alike) returns that error, so the call is
/// retried. A resend refused for another reason returns the first error
/// (`a_refused_resend_surfaces_the_original_error`).
#[tokio::test]
async fn a_resend_that_fails_with_a_retryable_error_returns_that_error() {
    let (account, client, server) = spawn_account_without_the_control().await;
    let mut tools = tools_after();
    tools.push(tool(OVERLOADED_TOOL));
    let (_, continuation, sent) =
        tool_round(&account, &client, "claude-opus-5-5", unchanged, tools).await;
    server.abort();
    assert_eq!(sent.len(), 2, "one attempt, then one resend: {sent:#?}");
    let error = continuation.error.expect("the call fails");
    assert!(
        matches!(error, LlmError::ServerError { status: 529, .. }),
        "the resend's overload is returned: {error:?}"
    );
    assert!(error.is_retryable(), "{error:?}");
}
