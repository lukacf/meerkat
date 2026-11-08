//! OpenAI refusals through a real agent loop and the `web_search` executor.
//!
//! These tests run the real `OpenAiClient` against a local Responses stub
//! that records every request body, and pin that an OpenAI refusal (a
//! `refusal` output part, streamed as `response.refusal.delta`) is never
//! answer text:
//!
//! - a refused turn fails the run as content filtered with the typed stop
//!   `refusal`, nothing of it committed, its text the provider detail;
//! - a refused native search comes back from the `web_search` executor
//!   failed, without the refusal as its answer (#1887).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use axum::{Json, Router, extract::State, response::IntoResponse, routing::post};
use meerkat::{AgentBuilder, AgentFactory, AgentToolDispatcher, OpenAiClient, ToolDef, ToolError};
use meerkat_core::event::{AgentErrorClass, AgentErrorReason};
use meerkat_core::{
    AgentError, AgentEvent, ContentFilteredStop, Message, ProviderStopDetail, RetryPolicy,
    ToolCallView, ToolDispatchOutcome,
};
use serde_json::Value;
use tokio::net::TcpListener;

#[path = "support/test_session_store.rs"]
mod test_session_store;
use test_session_store::TestSessionStore;

const MODEL: &str = "gpt-5.5";
const REFUSAL: &str = "I can't help with that.";

#[derive(Clone)]
struct StubState {
    reply: Arc<String>,
    requests: Arc<Mutex<Vec<Value>>>,
}

/// Serve the scripted reply to every request and record each request body.
async fn responses(State(state): State<StubState>, Json(body): Json<Value>) -> impl IntoResponse {
    state.requests.lock().unwrap().push(body);
    (
        [("content-type", "text/event-stream")],
        state.reply.as_str().to_owned(),
    )
}

async fn spawn_responses_stub(
    reply: String,
) -> (String, Arc<Mutex<Vec<Value>>>, tokio::task::JoinHandle<()>) {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let app = Router::new()
        .route("/v1/responses", post(responses))
        .with_state(StubState {
            reply: Arc::new(reply),
            requests: Arc::clone(&requests),
        });
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind stub");
    let addr = listener.local_addr().expect("stub addr");
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve stub");
    });
    (format!("http://{addr}"), requests, handle)
}

fn sse(lines: &[&str]) -> String {
    let mut body = lines.join("\n\n");
    body.push_str("\n\n");
    body
}

/// A streamed refusal, after a native web search when `searched`.
fn refusal_reply(searched: bool) -> String {
    let search_item = r#"{"type":"web_search_call","id":"ws_1","status":"completed","action":{"type":"search","queries":["meerkat"]}},"#;
    let completed = format!(
        r#"data: {{"type":"response.completed","response":{{"id":"resp_1","status":"completed","output":[{}{{"type":"message","id":"msg_1","role":"assistant","content":[{{"type":"refusal","refusal":"{REFUSAL}"}}]}}],"usage":{{"input_tokens":10,"output_tokens":5}}}}}}"#,
        if searched { search_item } else { "" }
    );
    sse(&[
        r#"data: {"type":"response.refusal.delta","item_id":"msg_1","output_index":0,"content_index":0,"delta":"I can't "}"#,
        r#"data: {"type":"response.refusal.delta","item_id":"msg_1","output_index":0,"content_index":0,"delta":"help with that."}"#,
        &completed,
    ])
}

struct NoTools;

#[async_trait]
impl AgentToolDispatcher for NoTools {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::from([])
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        Err(ToolError::not_found(call.name))
    }
}

/// An OpenAI refusal fails the run as content filtered with
/// the typed stop `refusal` after one request. Nothing of the refused turn
/// is committed, and the refusal text reaches the caller only as the typed
/// reason's provider detail.
#[tokio::test]
async fn openai_refusal_fails_the_run_content_filtered_with_its_text() {
    let (base_url, requests, server) = spawn_responses_stub(refusal_reply(false)).await;
    let client = OpenAiClient::new_with_base_url("test-key".to_string(), base_url);
    let factory = AgentFactory::new(".rkat/sessions");
    let llm_adapter = Arc::new(factory.build_llm_adapter(Arc::new(client), MODEL).await);
    let store_adapter = Arc::new(
        factory
            .build_store_adapter(Arc::new(TestSessionStore::new()))
            .await,
    );
    let tools: Arc<dyn AgentToolDispatcher> = Arc::new(NoTools);
    let mut agent = AgentBuilder::new()
        .model(MODEL)
        .max_tokens_per_turn(256)
        .retry_policy(RetryPolicy {
            max_retries: 2,
            initial_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(5),
            ..RetryPolicy::default()
        })
        .build(llm_adapter, tools, store_adapter)
        .await
        .expect("agent");

    let (tx, mut rx) = tokio::sync::mpsc::channel(4096);
    let result = agent
        .run_with_events("Do the forbidden thing.".to_string().into(), tx)
        .await;
    server.abort();
    let mut events = Vec::new();
    while let Ok(event) = rx.try_recv() {
        events.push(event);
    }

    let expected = ProviderStopDetail::openai("refusal", Some(REFUSAL.to_string()));
    match &result {
        Err(AgentError::ContentFiltered {
            stop: ContentFilteredStop::Refusal,
            detail,
            ..
        }) => assert_eq!(detail.as_deref(), Some(&expected)),
        other => panic!("expected a content-filtered refusal, got {other:?}"),
    }
    assert_eq!(
        requests.lock().unwrap().len(),
        1,
        "a refusal is not retried"
    );
    assert!(
        !agent
            .session()
            .messages()
            .iter()
            .any(|message| matches!(message, Message::BlockAssistant(_))),
        "nothing of a refused turn is committed"
    );
    let report = events
        .iter()
        .find_map(|event| match event {
            AgentEvent::RunFailed { error_report, .. } => Some(error_report.clone()),
            _ => None,
        })
        .expect("run failed event");
    assert_eq!(report.class, AgentErrorClass::ContentFiltered);
    assert_eq!(
        report.reason,
        Some(AgentErrorReason::ContentFiltered {
            stop: ContentFilteredStop::Refusal,
            detail: Some(expected),
        })
    );
    assert!(
        !report.message.contains(REFUSAL),
        "the refusal text travels typed, not in the message: {}",
        report.message
    );
}

/// #1887: a native search OpenAI refused comes back from the
/// `web_search` executor failed, with no answer, through the real client.
#[tokio::test]
async fn refused_native_search_fails_without_its_answer() {
    use meerkat_core::web_search::{WebSearchRequest, WebSearchStatus};
    use meerkat_llm_core::WebSearchExecutor;

    let (base_url, requests, server) = spawn_responses_stub(refusal_reply(true)).await;
    let client = OpenAiClient::new_with_base_url("test-key".to_string(), base_url);
    let adapter = AgentFactory::new(".rkat/sessions")
        .build_llm_adapter(Arc::new(client), MODEL)
        .await;
    let result = meerkat_openai::OpenAiWebSearchExecutor::new(MODEL.to_string(), Arc::new(adapter))
        .execute_web_search(WebSearchRequest {
            query: "meerkat".to_string(),
            provider: None,
            provider_params: None,
            context: None,
        })
        .await
        .expect("a refused search resolves to a typed result");
    server.abort();

    assert_eq!(requests.lock().unwrap().len(), 1);
    assert_eq!(result.status, WebSearchStatus::Failed, "{result:?}");
    assert_eq!(result.answer, None, "{result:?}");
    assert!(
        result
            .error
            .as_deref()
            .is_some_and(|error| error.contains("`refusal`")),
        "{result:?}"
    );
}
