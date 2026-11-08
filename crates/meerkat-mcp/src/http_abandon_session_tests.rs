//! Abandoned streamable-HTTP requests beside other traffic: two calls on the
//! wire at once (SSE, stateless and stateful), an abandon that lands before
//! the transport posts its request (JSON), a stateful session that keeps
//! serving after an aborted exchange and a reconnect, and a read without an
//! entry step.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::McpConnection;
use futures::Stream;
use meerkat_core::McpServerConfig;
use rmcp::model::{
    CallToolRequestParams, CallToolResult, Content, ListToolsResult, PaginatedRequestParams,
    ServerCapabilities, ServerInfo, Tool,
};
use rmcp::service::RequestContext;
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use rmcp::{ErrorData, RoleServer, ServerHandler};
use serde_json::json;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::sync::{Notify, Semaphore};
use tokio_util::sync::CancellationToken;

const LIMIT: Duration = Duration::from_secs(10);
const DISCONNECT_WITHIN: Duration = Duration::from_secs(1);

/// `quick` answers at once; `gated` answers once released; `slow` never.
#[derive(Clone)]
struct Server {
    entered: Arc<Semaphore>,
    release: Arc<Semaphore>,
}

impl ServerHandler for Server {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .build(),
        )
    }

    async fn read_resource(
        &self,
        _request: rmcp::model::ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<rmcp::model::ReadResourceResult, ErrorData> {
        self.entered.add_permits(1);
        std::future::pending::<Result<rmcp::model::ReadResourceResult, ErrorData>>().await
    }

    async fn list_tools(
        &self,
        _: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let mut result = ListToolsResult::default();
        for name in ["quick", "gated", "slow"] {
            result.tools.push(Tool::new(
                name,
                "session fixture",
                json!({"type": "object"}).as_object().unwrap().clone(),
            ));
        }
        Ok(result)
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        if request.name == "quick" {
            return Ok(CallToolResult::success(vec![Content::text("done")]));
        }
        self.entered.add_permits(1);
        if request.name == "gated" {
            self.release.acquire().await.unwrap().forget();
            return Ok(CallToolResult::success(vec![Content::text("released")]));
        }
        std::future::pending::<Result<CallToolResult, ErrorData>>().await
    }
}

#[derive(Clone, Default)]
struct Probe {
    calls: Arc<Mutex<Vec<String>>>,
    cancels: Arc<Mutex<Vec<serde_json::Value>>>,
    disconnects: Arc<Mutex<usize>>,
    disconnected: Arc<Notify>,
    resumes: Arc<Mutex<usize>>,
}

impl Probe {
    fn disconnects(&self) -> usize {
        *self.disconnects.lock().unwrap()
    }
    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
    fn cancels(&self) -> Vec<serde_json::Value> {
        self.cancels.lock().unwrap().clone()
    }
}

struct Exchange {
    probe: Probe,
    finished: bool,
}

impl Drop for Exchange {
    fn drop(&mut self) {
        if !self.finished {
            *self.probe.disconnects.lock().unwrap() += 1;
            self.probe.disconnected.notify_one();
        }
    }
}

struct ProbedBody<S> {
    inner: Pin<Box<S>>,
    exchange: Exchange,
}

impl<S: Stream> Stream for ProbedBody<S> {
    type Item = S::Item;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let polled = self.inner.as_mut().poll_next(cx);
        if let Poll::Ready(None) = polled {
            self.exchange.finished = true;
        }
        polled
    }
}

async fn probe(
    axum::extract::State(probe): axum::extract::State<Probe>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let (parts, body) = request.into_parts();
    if parts.headers.contains_key("last-event-id") {
        *probe.resumes.lock().unwrap() += 1;
    }
    let bytes = axum::body::to_bytes(body, 1 << 20).await.unwrap();
    let message = serde_json::from_slice::<serde_json::Value>(&bytes).ok();
    let request = axum::extract::Request::from_parts(parts, axum::body::Body::from(bytes));
    let Some(message) = message else {
        return next.run(request).await;
    };
    if message["method"] == "notifications/cancelled" {
        probe
            .cancels
            .lock()
            .unwrap()
            .push(message["params"]["requestId"].clone());
    }
    if message["method"] != "tools/call" && message["method"] != "resources/read" {
        return next.run(request).await;
    }
    let name = if message["method"] == "resources/read" {
        format!(
            "read:{}",
            message["params"]["uri"].as_str().unwrap_or_default()
        )
    } else {
        message["params"]["name"]
            .as_str()
            .unwrap_or_default()
            .to_owned()
    };
    probe.calls.lock().unwrap().push(name);
    let exchange = Exchange {
        probe,
        finished: false,
    };
    let response = next.run(request).await;
    let (parts, body) = response.into_parts();
    let body = ProbedBody {
        inner: Box::pin(body.into_data_stream()),
        exchange,
    };
    axum::response::Response::from_parts(parts, axum::body::Body::from_stream(body))
}

#[derive(Clone, Copy, Debug)]
enum Mode {
    StatelessSse,
    StatelessJson,
    StatefulSse,
}

struct Fixture {
    server: Server,
    probe: Probe,
    config: McpServerConfig,
    stop: CancellationToken,
    task: tokio::task::JoinHandle<std::io::Result<()>>,
}

impl Fixture {
    async fn start(mode: Mode) -> Self {
        let server = Server {
            entered: Arc::new(Semaphore::new(0)),
            release: Arc::new(Semaphore::new(0)),
        };
        let probe = Probe::default();
        let stop = CancellationToken::new();
        let mut config = StreamableHttpServerConfig::default();
        config.stateful_mode = matches!(mode, Mode::StatefulSse);
        config.json_response = matches!(mode, Mode::StatelessJson);
        config.cancellation_token = stop.child_token();
        let handler = server.clone();
        let service = StreamableHttpService::new(
            move || Ok(handler.clone()),
            Arc::new(LocalSessionManager::default()),
            config,
        );
        let app = axum::Router::new().nest_service("/mcp", service).layer(
            axum::middleware::from_fn_with_state(probe.clone(), self::probe),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let stopping = stop.clone();
        let task = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(stopping.cancelled_owned())
                .await
        });
        Self {
            server,
            probe,
            config: McpServerConfig::streamable_http(
                "session-fixture",
                format!("http://{address}/mcp"),
                Default::default(),
            ),
            stop,
            task,
        }
    }

    async fn connect(&self) -> McpConnection {
        McpConnection::connect_within(&self.config, LIMIT)
            .await
            .unwrap()
    }

    async fn disconnected(&self) -> bool {
        if self.probe.disconnects() > 0 {
            return true;
        }
        tokio::time::timeout(DISCONNECT_WITHIN, self.probe.disconnected.notified())
            .await
            .is_ok()
            || self.probe.disconnects() > 0
    }

    async fn stop(self, connection: McpConnection) {
        self.server.release.add_permits(64);
        let _ = tokio::time::timeout(LIMIT, connection.close()).await;
        self.stop.cancel();
        let _ = tokio::time::timeout(LIMIT, self.task).await;
    }
}

/// Two calls in flight on the wire at once (SSE): abandoning one ends only
/// that exchange; the other, released afterwards, returns its result.
async fn two_in_flight_sse_calls_abandon_one_the_other_completes(mode: Mode) {
    let fixture = Fixture::start(mode).await;
    let connection = fixture.connect().await;
    let args = json!({});
    let mut first = Box::pin(connection.call_tool("gated", &args));
    let mut second = Box::pin(connection.call_tool("gated", &args));
    tokio::select! {
        result = &mut first => panic!("{mode:?}: first returned early: {result:?}"),
        result = &mut second => panic!("{mode:?}: second returned early: {result:?}"),
        permit = fixture.server.entered.acquire_many(2) => permit.unwrap().forget(),
        () = tokio::time::sleep(LIMIT) => panic!("{mode:?}: both calls never entered"),
    }
    drop(first);
    assert!(fixture.disconnected().await, "{mode:?}: no disconnect seen");
    assert_eq!(fixture.probe.disconnects(), 1, "{mode:?}");
    // Both handlers wait on the same semaphore; release both.
    fixture.server.release.add_permits(2);
    let result = tokio::time::timeout(LIMIT, &mut second)
        .await
        .expect("the in-flight second call completes")
        .expect("the second call answers");
    drop(second);
    assert_eq!(
        meerkat_core::types::text_content(&result),
        "released",
        "{mode:?}"
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        fixture.probe.disconnects(),
        1,
        "{mode:?}: only the abandoned call"
    );
    assert_eq!(fixture.probe.calls().len(), 2, "{mode:?}");
    assert_eq!(
        fixture.probe.cancels().len(),
        1,
        "{mode:?}: one cancellation"
    );
    assert_eq!(
        *fixture.probe.resumes.lock().unwrap(),
        0,
        "{mode:?}: resumed"
    );
    fixture.stop(connection).await;
}

#[tokio::test]
async fn two_in_flight_sse_calls_abandon_one_the_other_completes_stateless_sse() {
    two_in_flight_sse_calls_abandon_one_the_other_completes(Mode::StatelessSse).await;
}

#[tokio::test]
async fn two_in_flight_sse_calls_abandon_one_the_other_completes_stateful_sse() {
    two_in_flight_sse_calls_abandon_one_the_other_completes(Mode::StatefulSse).await;
}

/// JSON mode: a call abandoned while it still waits behind another in the
/// transport is never posted once the first is abandoned too.
#[tokio::test]
async fn early_abandon_in_json_mode_is_never_posted() {
    let fixture = Fixture::start(Mode::StatelessJson).await;
    let connection = fixture.connect().await;
    let args = json!({});
    let mut first = Box::pin(connection.call_tool("slow", &args));
    tokio::select! {
        result = &mut first => panic!("first returned: {result:?}"),
        permit = fixture.server.entered.acquire() => permit.unwrap().forget(),
        () = tokio::time::sleep(LIMIT) => panic!("first never entered"),
    }
    let mut queued = Box::pin(connection.call_tool("quick", &args));
    let early = tokio::time::timeout(Duration::from_millis(300), &mut queued).await;
    assert!(early.is_err(), "the queued call waited behind the first");
    drop(queued);
    drop(first);
    assert!(fixture.disconnected().await, "no disconnect seen");
    let later = tokio::time::timeout(LIMIT, connection.call_tool("quick", &args))
        .await
        .expect("inside the deadline")
        .expect("a later call answers");
    assert_eq!(meerkat_core::types::text_content(&later), "done");
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        fixture.probe.calls(),
        vec!["slow".to_owned(), "quick".to_owned()],
        "the early-abandoned call never reached the server"
    );
    assert_eq!(fixture.probe.disconnects(), 1);
    assert_eq!(
        fixture.probe.cancels().len(),
        2,
        "one cancellation per abandoned call"
    );
    fixture.stop(connection).await;
}

/// Stateful SSE server: an aborted exchange leaves the session usable, the
/// server gets exactly one cancellation, nothing resumes, and a reconnect
/// works.
#[tokio::test]
async fn stateful_session_survives_an_abort_and_reconnects() {
    let fixture = Fixture::start(Mode::StatefulSse).await;
    let connection = fixture.connect().await;
    let args = json!({});
    let mut call = Box::pin(connection.call_tool("slow", &args));
    tokio::select! {
        result = &mut call => panic!("slow returned: {result:?}"),
        permit = fixture.server.entered.acquire() => permit.unwrap().forget(),
        () = tokio::time::sleep(LIMIT) => panic!("slow never entered"),
    }
    drop(call);
    assert!(fixture.disconnected().await, "no disconnect seen");
    tokio::time::sleep(DISCONNECT_WITHIN).await;
    assert_eq!(*fixture.probe.resumes.lock().unwrap(), 0, "resumed");
    assert_eq!(fixture.probe.cancels().len(), 1, "one cancellation");
    let after = tokio::time::timeout(LIMIT, connection.call_tool("quick", &args))
        .await
        .expect("inside the deadline")
        .expect("the session still serves calls");
    assert_eq!(meerkat_core::types::text_content(&after), "done");
    let _ = tokio::time::timeout(LIMIT, connection.close()).await;
    let again = fixture.connect().await;
    let reconnected = tokio::time::timeout(LIMIT, again.call_tool("quick", &args))
        .await
        .expect("inside the deadline")
        .expect("a reconnected session serves calls");
    assert_eq!(meerkat_core::types::text_content(&reconnected), "done");
    assert_eq!(fixture.probe.disconnects(), 1);
    fixture.stop(again).await;
}

/// A read past its deadline with no entry step returns at the deadline,
/// ends its exchange, and is cancelled exactly once.
async fn read_without_entry_step_returns_and_is_cancelled_once(mode: Mode) {
    let fixture = Fixture::start(mode).await;
    let connection = fixture.connect().await;
    let started = tokio::time::Instant::now();
    let deadline = started + Duration::from_millis(300);
    let read = connection
        .read_resource_entering("ui://fixture/slow", None, deadline, || Ok(()))
        .await;
    let waited = started.elapsed();
    assert!(read.is_err(), "{mode:?}: the read timed out: {read:?}");
    assert!(
        waited < Duration::from_secs(1),
        "{mode:?}: returned at the deadline: {waited:?}"
    );
    assert!(fixture.disconnected().await, "{mode:?}: no disconnect seen");
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        fixture.probe.calls(),
        vec!["read:ui://fixture/slow".to_owned()],
        "{mode:?}"
    );
    assert_eq!(
        fixture.probe.cancels().len(),
        1,
        "{mode:?}: one cancellation"
    );
    assert_eq!(fixture.probe.disconnects(), 1, "{mode:?}");
    fixture.stop(connection).await;
}

#[tokio::test]
async fn read_without_entry_step_returns_and_is_cancelled_once_stateless_json() {
    read_without_entry_step_returns_and_is_cancelled_once(Mode::StatelessJson).await;
}

#[tokio::test]
async fn read_without_entry_step_returns_and_is_cancelled_once_stateless_sse() {
    read_without_entry_step_returns_and_is_cancelled_once(Mode::StatelessSse).await;
}
