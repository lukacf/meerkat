//! A resource read past the host's deadline, over streamable HTTP with JSON
//! responses: the caller returns at its deadline, the server is sent exactly
//! one `notifications/cancelled` for that read, and a response that arrives
//! after the abandonment is dropped without a second one.
//!
//! The abandoned read's own HTTP exchange ends at once, so the transport is
//! free and the cancellation goes out right away; the server, whose request
//! went with the exchange, never answers it. (Without that abort the worker,
//! which reads a JSON answer inline, would hold the cancellation until the
//! server answered.) The caller is never held for it.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::McpConnection;
use meerkat_core::McpServerConfig;
use rmcp::model::{
    ReadResourceRequestParams, ReadResourceResult, ResourceContents, ServerCapabilities, ServerInfo,
};
use rmcp::service::RequestContext;
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use rmcp::{ErrorData, RoleServer, ServerHandler};
use serde_json::Value;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{Notify, Semaphore};
use tokio_util::sync::CancellationToken;

const LIMIT: Duration = Duration::from_secs(10);

/// Answers reads at once, or, while `block` is set, only once released.
#[derive(Clone)]
struct Server {
    block: Arc<AtomicBool>,
    entered: Arc<Notify>,
    release: Arc<Semaphore>,
}

impl ServerHandler for Server {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_resources().build())
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, ErrorData> {
        if self.block.load(Ordering::SeqCst) {
            self.entered.notify_one();
            self.release.acquire().await.unwrap().forget();
        }
        Ok(ReadResourceResult::new(vec![ResourceContents::text(
            "late or not",
            request.uri,
        )]))
    }
}

/// The JSON-RPC traffic the server received: each read's id, and each
/// cancellation's request id.
#[derive(Clone, Default)]
struct Traffic {
    reads: Arc<Mutex<Vec<Value>>>,
    cancels: Arc<Mutex<Vec<Value>>>,
    cancelled: Arc<Notify>,
}

async fn record(
    axum::extract::State(traffic): axum::extract::State<Traffic>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let (parts, body) = request.into_parts();
    let bytes = axum::body::to_bytes(body, 1 << 20).await.unwrap();
    if let Ok(message) = serde_json::from_slice::<Value>(&bytes) {
        match message["method"].as_str() {
            Some("resources/read") => traffic.reads.lock().unwrap().push(message["id"].clone()),
            Some("notifications/cancelled") => {
                traffic
                    .cancels
                    .lock()
                    .unwrap()
                    .push(message["params"]["requestId"].clone());
                traffic.cancelled.notify_one();
            }
            _ => {}
        }
    }
    next.run(axum::extract::Request::from_parts(
        parts,
        axum::body::Body::from(bytes),
    ))
    .await
}

struct Fixture {
    server: Server,
    traffic: Traffic,
    config: McpServerConfig,
    stop: CancellationToken,
    task: tokio::task::JoinHandle<std::io::Result<()>>,
}

impl Fixture {
    async fn start() -> Self {
        let server = Server {
            block: Default::default(),
            entered: Arc::new(Notify::new()),
            release: Arc::new(Semaphore::new(0)),
        };
        let traffic = Traffic::default();
        let stop = CancellationToken::new();
        let mut config = StreamableHttpServerConfig::default();
        config.stateful_mode = false;
        config.json_response = true;
        config.cancellation_token = stop.child_token();
        let handler = server.clone();
        let service = StreamableHttpService::new(
            move || Ok(handler.clone()),
            Arc::new(LocalSessionManager::default()),
            config,
        );
        let app = axum::Router::new().nest_service("/mcp", service).layer(
            axum::middleware::from_fn_with_state(traffic.clone(), record),
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
            traffic,
            config: McpServerConfig::streamable_http(
                "read-cancel-fixture",
                format!("http://{address}/mcp"),
                Default::default(),
            ),
            stop,
            task,
        }
    }
}

/// A read past the host's deadline returns at the deadline, the server is
/// sent exactly one cancellation naming that read, and its late response is
/// dropped: no second cancellation, and the connection keeps serving.
#[tokio::test]
async fn a_json_read_past_its_deadline_returns_promptly_and_is_cancelled_once() {
    let fixture = Fixture::start().await;
    let connection = McpConnection::connect_within(&fixture.config, LIMIT)
        .await
        .unwrap();
    fixture.server.block.store(true, Ordering::SeqCst);

    let started = tokio::time::Instant::now();
    let deadline = started + Duration::from_millis(300);
    let read = connection
        .read_resource_entering("ui://fixture/slow", None, deadline, || {
            // A slow entry step: the read is handed over 50 ms into its time.
            std::thread::sleep(Duration::from_millis(50));
            Ok(())
        })
        .await;
    let waited = started.elapsed();
    assert!(read.is_err(), "the read timed out: {read:?}");
    assert!(
        waited < Duration::from_secs(1),
        "the caller returned at its deadline, not later: {waited:?}"
    );

    // The cancellation goes out at once, and only one. Releasing the server's
    // handler changes nothing: its exchange already ended with the abandon.
    fixture.server.release.add_permits(1);
    let _ =
        tokio::time::timeout(Duration::from_secs(3), fixture.traffic.cancelled.notified()).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let reads = fixture.traffic.reads.lock().unwrap().clone();
    let cancels = fixture.traffic.cancels.lock().unwrap().clone();
    assert_eq!(reads.len(), 1);
    assert_eq!(cancels, reads, "exactly one cancellation, naming the read");

    // The late response was dropped; the connection still serves reads.
    fixture.server.block.store(false, Ordering::SeqCst);
    let later = tokio::time::Instant::now() + LIMIT;
    let next = connection
        .read_resource_entering("ui://fixture/next", None, later, || Ok(()))
        .await
        .expect("a later read is answered");
    assert_eq!(next.contents.len(), 1);
    assert_eq!(
        fixture.traffic.cancels.lock().unwrap().len(),
        1,
        "no second cancellation"
    );

    let _ = tokio::time::timeout(LIMIT, connection.close()).await;
    fixture.stop.cancel();
    let _ = tokio::time::timeout(LIMIT, fixture.task).await;
}
