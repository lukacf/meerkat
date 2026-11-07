//! A request this host abandons ends its own HTTP exchange, which a server in
//! stateless mode, deaf to `notifications/cancelled`, sees as a disconnect.
//!
//! Through real HTTP: rmcp's streamable-HTTP server in stateless mode, behind
//! a probe that records an exchange dropped before its response finished.
//! Each case runs with SSE responses and with JSON responses.
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
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

const LIMIT: Duration = Duration::from_secs(10);
/// How soon the server must see an abandoned exchange end.
const DISCONNECT_WITHIN: Duration = Duration::from_secs(1);

/// `quick` answers at once; `slow` never answers.
#[derive(Clone)]
struct Server {
    entered: Arc<Notify>,
}

impl ServerHandler for Server {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
    }

    async fn list_tools(
        &self,
        _: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let mut result = ListToolsResult::default();
        for name in ["quick", "slow"] {
            result.tools.push(Tool::new(
                name,
                "abandon fixture",
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
        self.entered.notify_one();
        std::future::pending::<Result<CallToolResult, ErrorData>>().await
    }
}

/// What the probe saw: every tools/call exchange, and those that ended
/// before their response finished.
#[derive(Clone, Default)]
struct Probe {
    calls: Arc<Mutex<usize>>,
    disconnects: Arc<Mutex<usize>>,
    disconnected: Arc<Notify>,
    /// Requests that tried to resume a stream (`Last-Event-ID`).
    resumes: Arc<Mutex<usize>>,
}

impl Probe {
    fn disconnects(&self) -> usize {
        *self.disconnects.lock().unwrap()
    }
}

/// One exchange: dropped before `finish`, it counts as a disconnect.
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

/// A response body that finishes its exchange when it is read to the end.
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
    let is_call = serde_json::from_slice::<serde_json::Value>(&bytes)
        .ok()
        .is_some_and(|message| message["method"] == "tools/call");
    let request = axum::extract::Request::from_parts(parts, axum::body::Body::from(bytes));
    if !is_call {
        return next.run(request).await;
    }
    *probe.calls.lock().unwrap() += 1;
    // Held by this future until the response head exists, then by its body:
    // the exchange ends early if the client goes away at either stage.
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
    Sse,
    Json,
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
            entered: Arc::new(Notify::new()),
        };
        let probe = Probe::default();
        let stop = CancellationToken::new();
        let mut config = StreamableHttpServerConfig::default();
        config.stateful_mode = false;
        config.json_response = matches!(mode, Mode::Json);
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
                "abandon-fixture",
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

    /// Waits up to `DISCONNECT_WITHIN` for the server to see a disconnect.
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
        let _ = tokio::time::timeout(LIMIT, connection.close()).await;
        self.stop.cancel();
        let _ = tokio::time::timeout(LIMIT, self.task).await;
    }
}

/// A tool call whose future is dropped while the server works on it ends
/// its HTTP exchange: the server sees the disconnect within a second.
async fn a_dropped_tool_call_ends_its_http_exchange(mode: Mode) {
    let fixture = Fixture::start(mode).await;
    let connection = fixture.connect().await;
    let args = json!({});
    let mut call = Box::pin(connection.call_tool("slow", &args));
    tokio::select! {
        result = &mut call => panic!("{mode:?}: the slow call returned: {result:?}"),
        () = fixture.server.entered.notified() => {}
    }
    drop(call);
    assert!(fixture.disconnected().await, "{mode:?}: no disconnect seen");
    assert_eq!(fixture.probe.disconnects(), 1, "{mode:?}");
    // The aborted stream is never resumed, and the session stays usable.
    tokio::time::sleep(DISCONNECT_WITHIN).await;
    assert_eq!(
        *fixture.probe.resumes.lock().unwrap(),
        0,
        "{mode:?}: resumed"
    );
    let after = tokio::time::timeout(LIMIT, connection.call_tool("quick", &args))
        .await
        .expect("inside the deadline")
        .expect("a call after the abandon completes");
    assert_eq!(
        meerkat_core::types::text_content(&after),
        "done",
        "{mode:?}"
    );
    assert_eq!(fixture.probe.disconnects(), 1, "{mode:?}");
    fixture.stop(connection).await;
}

#[tokio::test]
async fn a_dropped_tool_call_ends_its_http_exchange_sse() {
    a_dropped_tool_call_ends_its_http_exchange(Mode::Sse).await;
}

#[tokio::test]
async fn a_dropped_tool_call_ends_its_http_exchange_json() {
    a_dropped_tool_call_ends_its_http_exchange(Mode::Json).await;
}

/// A tool call whose caller's deadline expires ends its HTTP exchange.
async fn a_tool_call_past_its_deadline_ends_its_http_exchange(mode: Mode) {
    let fixture = Fixture::start(mode).await;
    let connection = fixture.connect().await;
    let args = json!({});
    let deadline = tokio::time::Instant::now() + Duration::from_millis(300);
    let mut call = Box::pin(connection.call_tool("slow", &args));
    // The request reaches the server before the deadline is let fire, so
    // this row never takes the path of an abandon that lands first.
    tokio::select! {
        result = &mut call => panic!("{mode:?}: the slow call returned: {result:?}"),
        () = fixture.server.entered.notified() => {}
    }
    let timed_out = tokio::time::timeout_at(deadline, &mut call).await;
    drop(call);
    assert!(timed_out.is_err(), "{mode:?}: the deadline expired");
    assert!(fixture.disconnected().await, "{mode:?}: no disconnect seen");
    assert_eq!(fixture.probe.disconnects(), 1, "{mode:?}");
    fixture.stop(connection).await;
}

#[tokio::test]
async fn a_tool_call_past_its_deadline_ends_its_http_exchange_sse() {
    a_tool_call_past_its_deadline_ends_its_http_exchange(Mode::Sse).await;
}

#[tokio::test]
async fn a_tool_call_past_its_deadline_ends_its_http_exchange_json() {
    a_tool_call_past_its_deadline_ends_its_http_exchange(Mode::Json).await;
}

/// A call that completes ends its exchange normally: no disconnect.
async fn a_completed_tool_call_ends_its_http_exchange_normally(mode: Mode) {
    let fixture = Fixture::start(mode).await;
    let connection = fixture.connect().await;
    let args = json!({});
    for _ in 0..2 {
        tokio::time::timeout(LIMIT, connection.call_tool("quick", &args))
            .await
            .expect("inside the deadline")
            .expect("quick answers");
    }
    assert!(
        !fixture.disconnected().await,
        "{mode:?}: a completed call disconnected"
    );
    assert_eq!(*fixture.probe.calls.lock().unwrap(), 2, "{mode:?}");
    fixture.stop(connection).await;
}

#[tokio::test]
async fn a_completed_tool_call_ends_its_http_exchange_normally_sse() {
    a_completed_tool_call_ends_its_http_exchange_normally(Mode::Sse).await;
}

#[tokio::test]
async fn a_completed_tool_call_ends_its_http_exchange_normally_json() {
    a_completed_tool_call_ends_its_http_exchange_normally(Mode::Json).await;
}

/// Two calls on one session: the one abandoned ends its own exchange and
/// only that; the other returns its whole result.
async fn abandoning_one_of_two_concurrent_calls_leaves_the_other(mode: Mode) {
    let fixture = Fixture::start(mode).await;
    let connection = fixture.connect().await;
    let args = json!({});
    let mut slow = Box::pin(connection.call_tool("slow", &args));
    tokio::select! {
        result = &mut slow => panic!("{mode:?}: the slow call returned: {result:?}"),
        () = fixture.server.entered.notified() => {}
    }
    // The second call is sent while the first is still open; with JSON
    // responses it waits behind the first until that one is abandoned.
    let mut quick_call = Box::pin(connection.call_tool("quick", &args));
    let early = tokio::time::timeout(Duration::from_millis(300), &mut quick_call).await;
    drop(slow);
    let quick = match early {
        Ok(result) => result,
        Err(_) => tokio::time::timeout(LIMIT, &mut quick_call)
            .await
            .expect("the second call completes after the abandon"),
    }
    .expect("the second call answers");
    drop(quick_call);
    assert_eq!(
        meerkat_core::types::text_content(&quick),
        "done",
        "{mode:?}"
    );
    assert!(fixture.disconnected().await, "{mode:?}: no disconnect seen");
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        fixture.probe.disconnects(),
        1,
        "{mode:?}: only the abandoned call"
    );
    assert_eq!(*fixture.probe.calls.lock().unwrap(), 2, "{mode:?}");
    fixture.stop(connection).await;
}

#[tokio::test]
async fn abandoning_one_of_two_concurrent_calls_leaves_the_other_sse() {
    abandoning_one_of_two_concurrent_calls_leaves_the_other(Mode::Sse).await;
}

#[tokio::test]
async fn abandoning_one_of_two_concurrent_calls_leaves_the_other_json() {
    abandoning_one_of_two_concurrent_calls_leaves_the_other(Mode::Json).await;
}

/// A connection without an HTTP client keeps a registry nothing registers
/// in: an abandon there only remembers the id, bounded, and ends nothing.
#[tokio::test]
async fn a_duplex_connection_keeps_an_inert_registry() {
    let (client_io, _server_io) = tokio::io::duplex(64);
    let running: rmcp::service::RunningService<rmcp::RoleClient, ()> =
        rmcp::service::serve_directly((), client_io, None);
    let held: super::HeldClient = running.into();
    held.abandoned
        .abandon(&rmcp::model::NumberOrString::Number(1));
    assert_eq!(held.abandoned.in_flight(), 0);
}
