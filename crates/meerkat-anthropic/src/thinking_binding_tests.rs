//! Thinking-block binding (Anthropic "preserved thinking"), driven against a
//! mock Messages endpoint that enforces the documented rules for an account
//! created on or after 2026-08-31.
//!
//! Claude Fable 5.1, Mythos 5.1, Opus 5.5, Sonnet 5.5 and Haiku 5.5 accept a
//! replayed thinking block only while the top-level `system`, the `tools` and
//! every earlier message are unchanged; on such an account a changed prefix is
//! a 400 unless the request sets
//! `thinking.block_binding.prefix_mismatch_behavior: "drop_block"`. That field
//! needs the `thinking-binding-controls-2026-08-01` beta header, is accepted
//! beside `adaptive` and `enabled` only (`between_tools` takes no other field;
//! Haiku 5.5 refuses it beside `disabled`), and makes the API drop the
//! failing block and every later one, reporting each in
//! `input_transformations`.
//! <https://platform.claude.com/docs/en/build-with-claude/preserved-thinking>
//! <https://platform.claude.com/docs/en/build-with-claude/thinking>
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
use meerkat_core::lifecycle::run_primitive::AnthropicThinkingConfig;
use meerkat_core::{
    AssistantBlock, BlockAssistantMessage, Message, ProviderMeta, StopReason, ToolChoice, ToolDef,
    UserMessage,
};
use meerkat_llm_core::{LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest};
use serde_json::{Value, json};

use crate::AnthropicClient;

const BINDING_BETA: &str = "thinking-binding-controls-2026-08-01";

/// The models whose replayed thinking blocks the prefix check covers.
const PREFIX_CHECKED: &[&str] = &[
    "claude-fable-5-1",
    "claude-mythos-5-1",
    "claude-opus-5-5",
    "claude-sonnet-5-5",
    "claude-haiku-5-5",
];

/// The prefix-checked models Meerkat's catalog carries.
const CATALOGUED_PREFIX_CHECKED: &[&str] = &[
    "claude-fable-5-1",
    "claude-opus-5-5",
    "claude-sonnet-5-5",
    "claude-haiku-5-5",
];

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
    let messages = body
        .get("messages")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
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
/// text block. Under the beta header `message_start` reports the dropped
/// blocks in `input_transformations`.
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
    let events = [
        json!({"type": "message_start", "message": message}),
        json!({"type": "content_block_start", "index": 0,
               "content_block": {"type": "thinking", "thinking": ""}}),
        json!({"type": "content_block_delta", "index": 0,
               "delta": {"type": "thinking_delta", "thinking": "Multiply."}}),
        json!({"type": "content_block_delta", "index": 0,
               "delta": {"type": "signature_delta", "signature": signature}}),
        json!({"type": "content_block_stop", "index": 0}),
        json!({"type": "content_block_start", "index": 1,
               "content_block": {"type": "text", "text": ""}}),
        json!({"type": "content_block_delta", "index": 1,
               "delta": {"type": "text_delta", "text": "42."}}),
        json!({"type": "content_block_stop", "index": 1}),
        json!({"type": "message_delta", "delta": {"stop_reason": "end_turn"},
               "usage": {"output_tokens": 7}}),
        json!({"type": "message_stop"}),
    ];
    event_stream(&events)
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
    done: Option<LlmDoneOutcome>,
    error: Option<LlmError>,
}

impl Turn {
    fn succeeded(&self) -> bool {
        self.error.is_none() && matches!(self.done, Some(LlmDoneOutcome::Success { .. }))
    }

    /// The assistant message the turn committed, as Meerkat replays it.
    fn assistant(&self) -> Message {
        Message::BlockAssistant(BlockAssistantMessage::new(
            vec![
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
            ],
            StopReason::EndTurn,
        ))
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

/// A prefix-checked model served under an id the catalog does not carry
/// (Claude Mythos 5.1 here; a proxy alias or a custom `[models.<id>]` entry
/// alike) with thinking configured gets the control and the beta once its
/// history replays a signed thinking block, so the session survives a tools
/// change.
#[tokio::test]
async fn an_uncatalogued_prefix_checked_id_survives_a_tools_change() {
    let model = "claude-mythos-5-1";
    assert!(!CATALOGUED_PREFIX_CHECKED.contains(&model));
    let (account, client, server) = spawn_account().await;
    let (_, second) = session_across_a_tools_change(&client, model, adaptive).await;
    let sent = account.last();
    server.abort();
    assert!(
        second.succeeded(),
        "the turn after the tools change completes: {second:?}"
    );
    assert_eq!(sent.body["thinking"], expected_binding(), "{}", sent.body);
    assert!(sent.carries_binding_beta(), "{:?}", sent.betas);
}

/// The documented limit: the same uncatalogued id with thinking omitted gets
/// no control, because no catalog row names its default (the alias may serve
/// an `enabled`-only model), so the request keeps today's shape and the
/// enforcing account refuses the replayed block after the tools change. An
/// uncatalogued id needs thinking configured to get the control.
#[tokio::test]
async fn an_uncatalogued_id_with_thinking_omitted_gets_no_control_after_a_tools_change() {
    let model = "claude-mythos-5-1";
    assert!(!CATALOGUED_PREFIX_CHECKED.contains(&model));
    let (account, client, server) = spawn_account().await;
    let (_, second) = session_across_a_tools_change(&client, model, |request| request).await;
    let sent = account.last();
    server.abort();
    assert!(sent.body.get("thinking").is_none(), "{}", sent.body);
    assert!(!sent.carries_binding_beta(), "{:?}", sent.betas);
    assert!(
        !second.succeeded(),
        "the enforcing account refuses the replayed block: {second:?}"
    );
    assert!(
        format!("{:?}", second.error).contains("bound to a different conversation"),
        "{second:?}"
    );
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
