//! Anthropic structured output with a schema that carries numeric bounds.
//!
//! Anthropic's native structured-output slot (`output_config.format`) rejects
//! `minimum` / `maximum` with HTTP 400, so the adapter sends a lowered slot
//! schema without them while the agent keeps validating the reply against the
//! full schema. These tests run the real `AnthropicClient` against a local
//! Messages stub that records every request body, and pin both halves:
//!
//! - the extraction requests carry the lowered slot (no bounds), and
//! - a reply that honors the slot but breaks a bound fails meerkat's
//!   validation and goes through the existing retry path.
//!
//! They also pin what the lowering leaves in the slot: a string `format`
//! outside Anthropic's list stays (the extraction-phase reply validator does
//! not assert `format`, so removing it would leave the extraction reply
//! unchecked; validate-first asserts it on the final reply only), while a pydantic
//! discriminated union loses its `discriminator` annotation and has `oneOf`
//! widened to `anyOf`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axum::{Json, Router, extract::State, response::IntoResponse, routing::post};
use meerkat::{
    AgentBuilder, AgentFactory, AgentToolDispatcher, AnthropicClient, OutputSchema, ToolDef,
    ToolError,
};
use meerkat_core::{ToolCallView, ToolDispatchOutcome};
use serde_json::{Value, json};
use tokio::net::TcpListener;

#[path = "support/test_session_store.rs"]
mod test_session_store;
use test_session_store::TestSessionStore;

const MODEL: &str = "claude-sonnet-4-6";

#[derive(Clone)]
struct StubState {
    replies: Arc<Vec<String>>,
    requests: Arc<Mutex<Vec<Value>>>,
}

/// Serve the scripted replies in order (the last one repeats) and record
/// every request body.
async fn messages(State(state): State<StubState>, Json(body): Json<Value>) -> impl IntoResponse {
    let index = {
        let mut requests = state.requests.lock().unwrap();
        requests.push(body);
        requests.len() - 1
    };
    let reply = state
        .replies
        .get(index)
        .or_else(|| state.replies.last())
        .cloned()
        .unwrap_or_default();
    (
        [("content-type", "text/event-stream")],
        sse_text_reply(&reply),
    )
}

fn sse_text_reply(text: &str) -> String {
    let delta =
        json!({"type": "content_block_delta", "delta": {"type": "text_delta", "text": text}});
    [
        r#"data: {"type":"message_start","message":{"usage":{"input_tokens":1,"output_tokens":0}}}"#.to_string(),
        r#"data: {"type":"content_block_start","content_block":{"type":"text","text":""}}"#.to_string(),
        format!("data: {delta}"),
        r#"data: {"type":"content_block_stop"}"#.to_string(),
        r#"data: {"type":"message_delta","usage":{"output_tokens":1},"delta":{"stop_reason":"end_turn"}}"#.to_string(),
        r#"data: {"type":"message_stop"}"#.to_string(),
        String::new(),
    ]
    .join("\n")
}

async fn spawn_messages_stub(
    replies: Vec<String>,
) -> (String, Arc<Mutex<Vec<Value>>>, tokio::task::JoinHandle<()>) {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let app = Router::new()
        .route("/v1/messages", post(messages))
        .with_state(StubState {
            replies: Arc::new(replies),
            requests: Arc::clone(&requests),
        });
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind stub");
    let addr = listener.local_addr().expect("stub addr");
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve stub");
    });
    (format!("http://{addr}"), requests, handle)
}

struct EmptyDispatcher;

#[async_trait]
impl AgentToolDispatcher for EmptyDispatcher {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::new([])
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        Err(ToolError::NotFound {
            name: call.name.to_string(),
        })
    }
}

fn review_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "verdict": {"type": "string", "enum": ["approve", "request_changes", "comment"]},
            "inline_comments": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "path": {"type": "string"},
                        "line": {"type": "integer", "minimum": 1},
                        "confidence": {"type": "number", "minimum": 0, "maximum": 1}
                    },
                    "required": ["path", "line", "confidence"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["verdict", "inline_comments"],
        "additionalProperties": false
    })
}

/// The slot schema every extraction request must carry for
/// [`review_schema`]: bounds removed from the grammar, restated in the
/// descriptions, everything else unchanged.
fn expected_slot_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "verdict": {"type": "string", "enum": ["approve", "request_changes", "comment"]},
            "inline_comments": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "path": {"type": "string"},
                        "line": {
                            "type": "integer",
                            "description": "Constraints (JSON Schema): {\"minimum\":1}"
                        },
                        "confidence": {
                            "type": "number",
                            "description": "Constraints (JSON Schema): {\"minimum\":0,\"maximum\":1}"
                        }
                    },
                    "required": ["path", "line", "confidence"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["verdict", "inline_comments"],
        "additionalProperties": false
    })
}

const OUT_OF_BOUNDS: &str = r#"{"verdict":"request_changes","inline_comments":[{"path":"calc.py","line":3,"confidence":95}]}"#;
const IN_BOUNDS: &str = r#"{"verdict":"request_changes","inline_comments":[{"path":"calc.py","line":3,"confidence":0.95}]}"#;

async fn run_review(
    replies: Vec<&str>,
    retries: u32,
) -> (meerkat::RunResult, Vec<Value>, tokio::task::JoinHandle<()>) {
    run_with_schema(review_schema(), replies, retries).await
}

async fn run_with_schema(
    schema: Value,
    replies: Vec<&str>,
    retries: u32,
) -> (meerkat::RunResult, Vec<Value>, tokio::task::JoinHandle<()>) {
    let (base_url, requests, server) =
        spawn_messages_stub(replies.into_iter().map(str::to_string).collect()).await;
    let client = AnthropicClient::builder("test-key".to_string())
        .base_url(base_url)
        .build()
        .expect("anthropic client");

    let factory = AgentFactory::new(".rkat/sessions");
    let llm_adapter = Arc::new(factory.build_llm_adapter(Arc::new(client), MODEL).await);
    let store_adapter = Arc::new(
        factory
            .build_store_adapter(Arc::new(TestSessionStore::new()))
            .await,
    );
    let tools: Arc<dyn AgentToolDispatcher> = Arc::new(EmptyDispatcher);
    let mut agent = AgentBuilder::new()
        .model(MODEL)
        .max_tokens_per_turn(256)
        .output_schema(OutputSchema::new(schema).expect("schema"))
        .structured_output_retries(retries)
        .build(llm_adapter, tools, store_adapter)
        .await
        .expect("agent");

    let result = agent
        .run("Review calc.py.".to_string().into())
        .await
        .expect("run completes");
    let requests = requests.lock().unwrap().clone();
    (result, requests, server)
}

fn slot_schema(request: &Value) -> Option<&Value> {
    request
        .get("output_config")
        .and_then(|config| config.get("format"))
        .map(|format| &format["schema"])
}

fn last_user_text(request: &Value) -> String {
    let last = request["messages"].as_array().unwrap().last().unwrap();
    assert_eq!(last["role"], "user");
    last["content"].to_string()
}

#[tokio::test]
async fn out_of_bounds_reply_fails_validation_and_the_retry_recovers() {
    let (result, requests, server) = run_review(
        vec![
            "I found one bug in calc.py line 3.",
            OUT_OF_BOUNDS,
            IN_BOUNDS,
        ],
        2,
    )
    .await;
    server.abort();

    assert_eq!(
        result.structured_output,
        Some(serde_json::from_str::<Value>(IN_BOUNDS).unwrap()),
        "the in-bounds retry is the structured output"
    );
    assert!(result.extraction_error.is_none());
    assert_eq!(
        requests.len(),
        3,
        "main turn, rejected extraction, accepted retry"
    );

    assert!(
        slot_schema(&requests[0]).is_none(),
        "the main turn carries no native schema slot"
    );
    for (index, request) in requests.iter().enumerate().skip(1) {
        assert_eq!(
            slot_schema(request),
            Some(&expected_slot_schema()),
            "extraction request {index} carries the lowered slot, never the bounds"
        );
    }

    // The retry prompt carries meerkat's own validation failure for the
    // bound the native slot could not enforce.
    let retry_prompt = last_user_text(&requests[2]);
    assert!(
        retry_prompt.contains("The previous output was invalid")
            && retry_prompt.contains("maximum"),
        "retry prompt names the violated bound: {retry_prompt}"
    );
}

#[tokio::test]
async fn out_of_bounds_replies_exhaust_retries_with_an_extraction_error() {
    let (result, requests, server) = run_review(vec!["I found one bug.", OUT_OF_BOUNDS], 1).await;
    server.abort();

    assert!(
        result.structured_output.is_none(),
        "an out-of-bounds value is never accepted as structured output"
    );
    let error = result.extraction_error.expect("extraction error");
    assert!(
        error.reason.contains("maximum"),
        "the failure is meerkat's bound check: {}",
        error.reason
    );
    assert_eq!(requests.len(), 3, "main turn, extraction, one retry");
    for request in &requests[1..] {
        assert_eq!(slot_schema(request), Some(&expected_slot_schema()));
    }
}

/// A pydantic discriminated union (`oneOf` over `$ref`s with an OpenAPI
/// `discriminator` beside it) plus a string `format` outside Anthropic's list.
fn union_and_pointer_schema() -> Value {
    json!({
        "$defs": {
            "Cat": {"properties": {"kind": {"const": "cat", "title": "Kind", "type": "string"},
                                   "meows": {"title": "Meows", "type": "integer"}},
                    "required": ["kind", "meows"], "title": "Cat", "type": "object"},
            "Dog": {"properties": {"kind": {"const": "dog", "title": "Kind", "type": "string"},
                                   "barks": {"title": "Barks", "type": "integer"}},
                    "required": ["kind", "barks"], "title": "Dog", "type": "object"}
        },
        "properties": {
            "pet": {
                "discriminator": {"mapping": {"cat": "#/$defs/Cat", "dog": "#/$defs/Dog"},
                                  "propertyName": "kind"},
                "oneOf": [{"$ref": "#/$defs/Cat"}, {"$ref": "#/$defs/Dog"}],
                "title": "Pet"
            },
            "ptr": {"type": "string", "format": "json-pointer"}
        },
        "required": ["pet", "ptr"],
        "title": "M",
        "type": "object"
    })
}

#[tokio::test]
async fn unsupported_format_stays_in_the_slot_and_discriminated_unions_are_lowered() {
    let reply = r#"{"pet":{"kind":"cat","meows":3},"ptr":"/a/b"}"#;
    let (result, requests, server) =
        run_with_schema(union_and_pointer_schema(), vec!["Done.", reply], 0).await;
    server.abort();

    assert_eq!(
        result.structured_output,
        Some(serde_json::from_str::<Value>(reply).unwrap())
    );
    assert_eq!(requests.len(), 2, "main turn and one extraction");
    let closed = |mut def: Value| {
        def["additionalProperties"] = json!(false);
        def
    };
    let defs = &union_and_pointer_schema()["$defs"];
    assert_eq!(
        slot_schema(&requests[1]),
        Some(&json!({
            "$defs": {"Cat": closed(defs["Cat"].clone()), "Dog": closed(defs["Dog"].clone())},
            "properties": {
                "pet": {
                    "anyOf": [{"$ref": "#/$defs/Cat"}, {"$ref": "#/$defs/Dog"}],
                    "title": "Pet",
                    "description": "Constraints (JSON Schema): {\"discriminator\":{\"mapping\":{\"cat\":\"#/$defs/Cat\",\"dog\":\"#/$defs/Dog\"},\"propertyName\":\"kind\"}}"
                },
                // The extraction-phase validator does not assert `format`, so the slot
                // keeps it and Anthropic rejects it loudly rather than
                // leaving it enforced nowhere.
                "ptr": {"type": "string", "format": "json-pointer"}
            },
            "required": ["pet", "ptr"],
            "title": "M",
            "type": "object",
            "additionalProperties": false
        })),
        "discriminator removed and restated, oneOf widened to anyOf, format kept"
    );
}

// ---------------------------------------------------------------------------
// Interplay with schema visibility and validate-first
// ---------------------------------------------------------------------------
//
// Every request of a schema-bearing run shows the model a `<structured_output>`
// section built from `compile_schema()`, the validation schema, and the final
// reply is validated against that same schema before any extraction request
// (with known string formats asserted). The native slot lowering must stay out
// of both: the section keeps every bound, validate-first enforces every bound,
// and only the extraction request's `output_config.format` carries the lowered
// slot schema.

/// The review schema plus a string `format` Anthropic's slot supports
/// (`date-time`), so the lowering keeps it and validate-first asserts it.
fn bounded_schema_with_format() -> Value {
    let mut schema = review_schema();
    schema["properties"]["due"] = json!({"type": "string", "format": "date-time"});
    schema["required"] = json!(["verdict", "inline_comments", "due"]);
    schema
}

const DUE_IN_BOUNDS: &str = r#"{"verdict":"request_changes","inline_comments":[{"path":"calc.py","line":3,"confidence":0.95}],"due":"2026-10-01T12:00:00Z"}"#;
const DUE_OUT_OF_BOUNDS: &str = r#"{"verdict":"request_changes","inline_comments":[{"path":"calc.py","line":3,"confidence":1.5}],"due":"2026-10-01T12:00:00Z"}"#;
const DUE_NOT_A_DATE_TIME: &str = r#"{"verdict":"request_changes","inline_comments":[{"path":"calc.py","line":3,"confidence":0.95}],"due":"next Friday"}"#;

struct InterplayRun {
    result: meerkat::RunResult,
    requests: Vec<Value>,
    origins: Vec<meerkat_core::StructuredOutputOrigin>,
    /// The section the loop must show: rendered from the validation schema.
    section: String,
    validation_schema: Value,
}

async fn run_interplay(replies: Vec<&str>) -> InterplayRun {
    use meerkat::LlmClient;

    let (base_url, requests, server) =
        spawn_messages_stub(replies.into_iter().map(str::to_string).collect()).await;
    let client = AnthropicClient::builder("test-key".to_string())
        .base_url(base_url)
        .build()
        .expect("anthropic client");
    let schema = OutputSchema::new(bounded_schema_with_format()).expect("schema");
    let validation_schema = client.compile_schema(&schema).expect("compile").schema;
    let section =
        meerkat_core::structured_output::render_output_schema_instructions(&validation_schema);

    let factory = AgentFactory::new(".rkat/sessions");
    let llm_adapter = Arc::new(factory.build_llm_adapter(Arc::new(client), MODEL).await);
    let store_adapter = Arc::new(
        factory
            .build_store_adapter(Arc::new(TestSessionStore::new()))
            .await,
    );
    let tools: Arc<dyn AgentToolDispatcher> = Arc::new(EmptyDispatcher);
    let mut agent = AgentBuilder::new()
        .model(MODEL)
        .max_tokens_per_turn(256)
        .output_schema(schema)
        .structured_output_retries(1)
        .build(llm_adapter, tools, store_adapter)
        .await
        .expect("agent");

    let (tx, mut rx) = tokio::sync::mpsc::channel(1024);
    let result = agent
        .run_with_events("Review calc.py.".to_string().into(), tx)
        .await
        .expect("run completes");
    server.abort();
    let mut origins = Vec::new();
    while let Ok(event) = rx.try_recv() {
        if let meerkat_core::AgentEvent::ExtractionSucceeded { origin, .. } = event {
            origins.push(origin);
        }
    }
    let requests = requests.lock().unwrap().clone();
    InterplayRun {
        result,
        requests,
        origins,
        section,
        validation_schema,
    }
}

/// The leading system text of a recorded Messages request.
fn system_text(request: &Value) -> String {
    match &request["system"] {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter_map(|block| block["text"].as_str())
            .collect(),
        other => panic!("request without a system prompt: {other}"),
    }
}

/// Every object key anywhere in `value`.
fn keys_anywhere(value: &Value, keys: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (key, item) in map {
                keys.push(key.clone());
                keys_anywhere(item, keys);
            }
        }
        Value::Array(items) => items.iter().for_each(|item| keys_anywhere(item, keys)),
        _ => {}
    }
}

/// The section on every request is the validation schema, bounds and format
/// intact, and never the lowered slot schema.
fn assert_section_shows_the_validation_schema(run: &InterplayRun) {
    for (index, request) in run.requests.iter().enumerate() {
        let system = system_text(request);
        assert!(
            system.ends_with(&run.section),
            "request {index}: the section closes the system prompt: {system}"
        );
        assert_eq!(
            system
                .matches(meerkat_core::structured_output::OUTPUT_SCHEMA_INSTRUCTIONS_OPEN)
                .count(),
            1,
            "request {index}: one section"
        );
        for shown in [
            r#""confidence":{"maximum":1,"minimum":0,"type":"number"}"#,
            r#""line":{"minimum":1,"type":"integer"}"#,
            r#""due":{"format":"date-time","type":"string"}"#,
        ] {
            assert!(
                system.contains(shown),
                "request {index}: the section shows {shown}: {system}"
            );
        }
        assert!(
            !system.contains("Constraints (JSON Schema)"),
            "request {index}: the section never shows the lowered slot schema"
        );
    }
    let first = system_text(&run.requests[0]);
    assert!(
        run.requests
            .iter()
            .all(|request| system_text(request) == first),
        "the section is byte-identical on every request, extraction included"
    );
}

/// A final reply inside every bound and format validates first: no extraction
/// request and no native slot are sent, and the value is the structured output.
#[tokio::test]
async fn bounded_schema_final_reply_in_bounds_is_accepted_by_validate_first() {
    let run = run_interplay(vec![DUE_IN_BOUNDS]).await;

    assert_eq!(
        run.requests.len(),
        1,
        "validate-first sends no extraction request"
    );
    assert_eq!(run.result.turns, 1);
    assert_eq!(
        run.result.structured_output,
        Some(serde_json::from_str::<Value>(DUE_IN_BOUNDS).unwrap())
    );
    assert!(run.result.extraction_error.is_none());
    assert_eq!(
        run.origins,
        vec![meerkat_core::StructuredOutputOrigin::FinalReply]
    );
    assert!(
        slot_schema(&run.requests[0]).is_none(),
        "the main turn carries no native schema slot"
    );
    assert_section_shows_the_validation_schema(&run);
}

/// A final reply that breaks `maximum` fails validate-first against the
/// validation schema. The extraction request then carries the lowered slot
/// (no bounds, `format` kept) while its section still shows the bounds, and
/// the in-bounds extraction reply becomes the structured output.
#[tokio::test]
async fn bounded_schema_out_of_bounds_final_reply_falls_back_to_the_lowered_slot() {
    let run = run_interplay(vec![DUE_OUT_OF_BOUNDS, DUE_IN_BOUNDS]).await;

    assert_eq!(
        run.requests.len(),
        2,
        "the out-of-bounds final reply was rejected and one extraction request ran"
    );
    assert_eq!(run.result.turns, 2);
    assert_eq!(
        run.result.structured_output,
        Some(serde_json::from_str::<Value>(DUE_IN_BOUNDS).unwrap())
    );
    assert_eq!(
        run.origins,
        vec![meerkat_core::StructuredOutputOrigin::ExtractionRequest]
    );
    assert_eq!(
        run.result.text, DUE_OUT_OF_BOUNDS,
        "text stays the primary final reply"
    );
    assert_section_shows_the_validation_schema(&run);

    assert!(slot_schema(&run.requests[0]).is_none());
    let slot = slot_schema(&run.requests[1]).expect("the extraction request carries the slot");
    let mut expected = expected_slot_schema();
    expected["properties"]["due"] = json!({"type": "string", "format": "date-time"});
    expected["required"] = json!(["verdict", "inline_comments", "due"]);
    assert_eq!(slot, &expected, "the slot carries the lowered schema");
    assert_ne!(
        slot, &run.validation_schema,
        "the slot is not the validation schema the section shows"
    );
    let mut keys = Vec::new();
    keys_anywhere(slot, &mut keys);
    assert!(
        !keys.iter().any(|key| key == "minimum" || key == "maximum"),
        "no bound reaches the slot: {slot}"
    );
    let last = run.requests[1]["messages"]
        .as_array()
        .unwrap()
        .last()
        .unwrap();
    assert!(
        last.to_string()
            .contains("Provide the final output as valid JSON"),
        "the unchanged extraction prompt closes the extraction request: {last}"
    );
}

/// Validate-first still asserts known string formats with a bounded schema: a
/// reply inside every bound whose `due` is not a date-time falls back to the
/// extraction request, whose slot keeps `format` (Anthropic supports
/// `date-time`, and the lowering never removes `format`).
#[tokio::test]
async fn bounded_schema_validate_first_still_asserts_formats() {
    let run = run_interplay(vec![DUE_NOT_A_DATE_TIME, DUE_IN_BOUNDS]).await;

    assert_eq!(run.requests.len(), 2, "the format violation ran extraction");
    assert_eq!(
        run.result.structured_output,
        Some(serde_json::from_str::<Value>(DUE_IN_BOUNDS).unwrap())
    );
    assert_eq!(
        run.origins,
        vec![meerkat_core::StructuredOutputOrigin::ExtractionRequest]
    );
    assert_section_shows_the_validation_schema(&run);
    let slot = slot_schema(&run.requests[1]).expect("slot");
    assert_eq!(
        slot["properties"]["due"],
        json!({"type": "string", "format": "date-time"})
    );
}
