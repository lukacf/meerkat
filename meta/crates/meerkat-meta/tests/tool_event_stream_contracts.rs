#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::uninlined_format_args,
    clippy::collapsible_if,
    clippy::redundant_clone,
    clippy::needless_raw_string_hashes,
    clippy::single_match,
    clippy::redundant_closure_for_method_calls,
    clippy::redundant_pattern_matching,
    clippy::ignored_unit_patterns,
    clippy::clone_on_copy,
    clippy::manual_assert,
    clippy::unwrap_in_result,
    clippy::useless_vec
)]
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use futures::StreamExt;
use meerkat_core::AgentEvent;
use meerkat_core::EventEnvelope;
use meerkat_core::comms::EventStream;
use meerkat_meta::{MobRuntimeError, agent_events_sse_router};
use serde_json::{Value, json};
use tower::ServiceExt;

#[tokio::test]
async fn phase0_contract_007_tool_events_stream_with_tool_call_id() {
    // The tool's output travels once, on `AgentEvent::ToolResultReceived`
    // (typed `content` blocks). `ToolExecutionCompleted` is the execution
    // fact for the same id and carries no body. Meta's SSE edge derives the
    // wire `result` field from the result event's text blocks (see
    // console_agent_event_payload), keeping the SDK contract.
    let completed = AgentEvent::ToolExecutionCompleted {
        id: "tool-1".to_string(),
        name: "search".to_string(),
        is_error: false,
        duration_ms: 12,
    };
    let received = AgentEvent::ToolResultReceived {
        id: "tool-1".to_string(),
        name: "search".to_string(),
        content: vec![meerkat_core::types::ContentBlock::Text {
            text: "done".to_string(),
        }],
        is_error: false,
    };
    let app = agent_events_sse_router(
        Arc::new(move |_agent_id| {
            let events = vec![
                EventEnvelope::new("worker-1", 0, None, completed.clone()),
                EventEnvelope::new("worker-1", 1, None, received.clone()),
            ];
            Box::pin(async move {
                let stream: EventStream = Box::pin(futures::stream::iter(events));
                Ok::<EventStream, MobRuntimeError>(stream)
            })
        }),
        // No decisions: this test exercises the streaming shape, not
        // auth gating. Auth coverage lives in `sse_auth_gate.rs`.
        None,
    );

    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/agents/worker-1/events")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);

    let mut stream = response.into_body().into_data_stream();
    let mut payloads = Vec::new();
    while payloads.len() < 2 {
        let chunk = stream
            .next()
            .await
            .expect("both tool frames are streamed")
            .expect("sse bytes");
        let text = String::from_utf8(chunk.to_vec()).expect("utf8 sse chunk");
        for data in text.lines().filter_map(|line| line.strip_prefix("data:")) {
            payloads.push(serde_json::from_str::<Value>(data.trim()).expect("json payload"));
        }
    }

    let completed = &payloads[0];
    assert_eq!(completed["type"], json!("tool_execution_completed"));
    assert_eq!(completed["id"], json!("tool-1"));
    assert_eq!(completed["tool_call_id"], json!("tool-1"));
    assert_eq!(completed["name"], json!("search"));
    assert_eq!(completed["duration_ms"], json!(12));
    assert!(completed.get("result").is_none(), "{completed}");
    assert!(completed.get("content").is_none(), "{completed}");

    let received = &payloads[1];
    assert_eq!(received["type"], json!("tool_result_received"));
    assert_eq!(received["id"], json!("tool-1"));
    assert_eq!(received["tool_call_id"], json!("tool-1"));
    assert_eq!(received["name"], json!("search"));
    assert_eq!(received["result"], json!("done"));
    assert_eq!(received["content"][0]["text"], json!("done"));
}
