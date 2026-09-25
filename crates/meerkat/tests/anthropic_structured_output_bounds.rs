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
        .output_schema(OutputSchema::new(review_schema()).expect("schema"))
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
