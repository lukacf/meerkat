//! A Streamable HTTP server that executes `tools/call` and then answers
//! `404` (its session dropped) must see exactly one POST for that call: the
//! client neither re-initializes nor re-sends it, types its outcome as
//! uncertain, and refuses later calls unsent until it reconnects.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use axum::Router;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use meerkat_core::McpServerConfig;
use meerkat_mcp::{McpConnection, McpError};
use serde_json::{Value, json};
use tokio::net::TcpListener;

const SESSION: &str = "fixture-session-1";

#[derive(Default)]
struct Fixture {
    initializes: AtomicUsize,
    tool_posts: AtomicUsize,
    executions: AtomicUsize,
    /// Whether `tools/call` runs the effect before answering `404`.
    execute_then_expire: AtomicBool,
    /// Whether `tools/call` answers `404` (the session was dropped).
    expire_calls: AtomicBool,
    /// Whether `tools/call` is routed with a same-origin 307 to `/mcp/`,
    /// whose handler fails.
    redirect_calls: AtomicBool,
    redirected_posts: AtomicUsize,
}

async fn mcp(State(fixture): State<Arc<Fixture>>, headers: HeaderMap, body: String) -> Response {
    let message: Value = serde_json::from_str(&body).unwrap();
    let id = message.get("id").cloned();
    let method = message["method"].as_str().unwrap_or_default();
    let reply = |result: Value| {
        (
            StatusCode::OK,
            [
                ("content-type", "application/json"),
                ("mcp-session-id", SESSION),
            ],
            json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string(),
        )
            .into_response()
    };
    match method {
        "initialize" => {
            fixture.initializes.fetch_add(1, Ordering::SeqCst);
            reply(json!({
                "protocolVersion": message["params"]["protocolVersion"],
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "session-expiry-fixture", "version": "1"},
            }))
        }
        _ if id.is_none() => StatusCode::ACCEPTED.into_response(),
        "tools/call" => {
            assert_eq!(
                headers.get("mcp-session-id").and_then(|v| v.to_str().ok()),
                Some(SESSION),
                "a call always carries the session it was admitted in"
            );
            fixture.tool_posts.fetch_add(1, Ordering::SeqCst);
            if fixture.redirect_calls.load(Ordering::SeqCst) {
                return (StatusCode::TEMPORARY_REDIRECT, [("location", "/mcp/")]).into_response();
            }
            if fixture.expire_calls.load(Ordering::SeqCst) {
                if fixture.execute_then_expire.load(Ordering::SeqCst) {
                    fixture.executions.fetch_add(1, Ordering::SeqCst);
                }
                return StatusCode::NOT_FOUND.into_response();
            }
            fixture.executions.fetch_add(1, Ordering::SeqCst);
            reply(json!({"content": [{"type": "text", "text": "done"}], "isError": false}))
        }
        "tools/list" => reply(json!({"tools": []})),
        _ => reply(json!({})),
    }
}

async fn start(fixture: Arc<Fixture>) -> String {
    let app = Router::new()
        .route(
            "/mcp",
            post(mcp)
                .get(|| async { StatusCode::METHOD_NOT_ALLOWED })
                .delete(|| async { StatusCode::OK }),
        )
        .route(
            "/mcp/",
            post(|State(fixture): State<Arc<Fixture>>| async move {
                fixture.redirected_posts.fetch_add(1, Ordering::SeqCst);
                StatusCode::INTERNAL_SERVER_ERROR
            }),
        )
        .with_state(fixture);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}/mcp")
}

async fn expired_call_is_sent_once(execute_then_expire: bool) {
    let fixture = Arc::new(Fixture::default());
    fixture.expire_calls.store(true, Ordering::SeqCst);
    fixture
        .execute_then_expire
        .store(execute_then_expire, Ordering::SeqCst);
    let url = start(Arc::clone(&fixture)).await;
    let config = McpServerConfig::streamable_http("session-expiry", url, HashMap::new());
    let connection = McpConnection::connect(&config).await.unwrap();
    assert_eq!(fixture.initializes.load(Ordering::SeqCst), 1);

    let error = connection
        .call_tool("effect", &json!({"n": 1}))
        .await
        .expect_err("an expired session is not a success");
    assert!(
        matches!(&error, McpError::SessionExpired { tool, .. } if tool == "effect"),
        "{error:?}"
    );
    assert_eq!(
        fixture.tool_posts.load(Ordering::SeqCst),
        1,
        "exactly one POST for the call"
    );
    assert_eq!(
        fixture.initializes.load(Ordering::SeqCst),
        1,
        "no transparent re-initialization"
    );
    assert_eq!(
        fixture.executions.load(Ordering::SeqCst),
        usize::from(execute_then_expire)
    );

    // The dead session is never resumed: a later call is refused unsent.
    let refused = connection
        .call_tool("effect", &json!({"n": 2}))
        .await
        .expect_err("a dead session refuses later calls");
    assert!(
        matches!(refused, McpError::ServerUnavailable { .. }),
        "{refused:?}"
    );
    assert_eq!(fixture.tool_posts.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.initializes.load(Ordering::SeqCst), 1);
    let _ = connection.close().await;

    // Only an explicit reconnect starts a new session; its call runs once.
    fixture.expire_calls.store(false, Ordering::SeqCst);
    let reconnected = McpConnection::connect(&config).await.unwrap();
    assert_eq!(fixture.initializes.load(Ordering::SeqCst), 2);
    reconnected
        .call_tool("effect", &json!({"n": 3}))
        .await
        .unwrap();
    assert_eq!(fixture.tool_posts.load(Ordering::SeqCst), 2);
    assert_eq!(
        fixture.executions.load(Ordering::SeqCst),
        usize::from(execute_then_expire) + 1
    );
    let _ = reconnected.close().await;
}

#[tokio::test]
async fn a_call_executed_before_a_session_404_is_never_resent() {
    expired_call_is_sent_once(true).await;
}

/// Paired control: a server that answers 404 without executing gives the
/// same single POST and the same uncertain outcome. The client cannot tell
/// the two apart, which is why the outcome is uncertain, not "did not run".
#[tokio::test]
async fn a_call_refused_with_a_session_404_is_equally_uncertain() {
    expired_call_is_sent_once(false).await;
}

/// A connection converted into the public protocol wrapper keeps the same
/// expiry owner and call path: one POST, a typed uncertain outcome, then
/// later calls refused unsent.
#[tokio::test]
async fn the_converted_protocol_wrapper_keeps_the_typed_outcome_and_refusal() {
    let fixture = Arc::new(Fixture::default());
    fixture.expire_calls.store(true, Ordering::SeqCst);
    fixture.execute_then_expire.store(true, Ordering::SeqCst);
    let url = start(Arc::clone(&fixture)).await;
    let config = McpServerConfig::streamable_http("session-expiry", url, HashMap::new());
    let protocol = McpConnection::connect(&config)
        .await
        .unwrap()
        .into_protocol();

    let error = protocol
        .call_tool("effect", &json!({"n": 1}))
        .await
        .expect_err("an expired session is not a success");
    assert!(
        matches!(&error, McpError::SessionExpired { server, tool } if server == "session-expiry" && tool == "effect"),
        "{error:?}"
    );
    let refused = protocol
        .call_tool("effect", &json!({"n": 2}))
        .await
        .expect_err("a dead session refuses later calls");
    assert!(
        matches!(refused, McpError::ServerUnavailable { .. }),
        "{refused:?}"
    );
    assert_eq!(fixture.tool_posts.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.initializes.load(Ordering::SeqCst), 1);
    let _ = protocol.close().await;
}

/// A tool call routed by a same-origin redirect whose handler then fails:
/// two physical POSTs, a typed uncertain outcome, and no re-send. A later
/// independent call on the same connection is unaffected.
#[tokio::test]
async fn a_redirected_tool_call_that_fails_is_uncertain_and_not_resent() {
    let fixture = Arc::new(Fixture::default());
    fixture.redirect_calls.store(true, Ordering::SeqCst);
    let url = start(Arc::clone(&fixture)).await;
    let config = McpServerConfig::streamable_http("redirected", url, HashMap::new());
    let connection = McpConnection::connect(&config).await.unwrap();
    let error = connection
        .call_tool("effect", &json!({"n": 1}))
        .await
        .expect_err("the routed handler failed");
    assert!(
        matches!(&error, McpError::RedirectedOutcomeUncertain { tool, .. } if tool == "effect"),
        "{error:?}"
    );
    assert_eq!(fixture.tool_posts.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.redirected_posts.load(Ordering::SeqCst), 1);
    assert_eq!(
        fixture.initializes.load(Ordering::SeqCst),
        1,
        "no reinitialization"
    );
    // Independent control: an unrouted call on the same connection runs.
    fixture.redirect_calls.store(false, Ordering::SeqCst);
    connection
        .call_tool("effect", &json!({"n": 2}))
        .await
        .unwrap();
    assert_eq!(fixture.tool_posts.load(Ordering::SeqCst), 2);
    let _ = connection.close().await;
}
