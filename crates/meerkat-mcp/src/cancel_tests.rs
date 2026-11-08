//! A request this host stops waiting for is cancelled at the server.
//!
//! Through real rmcp I/O: a server that records the requests it receives and
//! the `notifications/cancelled` it is sent. A tool call whose future is
//! dropped, or whose caller's deadline expires, and a resource read that
//! times out each send exactly one cancellation naming that request. A call
//! that completes sends none.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::McpConnection;
use meerkat_core::McpServerConfig;
use rmcp::{
    ErrorData, RoleServer, ServerHandler, ServiceExt,
    model::{
        CallToolRequestParams, CallToolResult, CancelledNotificationParam, Content,
        ReadResourceRequestParams, ReadResourceResult, RequestId, ServerCapabilities, ServerInfo,
    },
    service::{NotificationContext, RequestContext, RoleClient, RunningService},
};
use serde_json::json;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Notify;

/// What the server saw: each request's id, and every cancellation.
#[derive(Clone, Default)]
struct Seen {
    requests: Arc<Mutex<Vec<RequestId>>>,
    cancels: Arc<Mutex<Vec<RequestId>>>,
    request_arrived: Arc<Notify>,
    cancel_arrived: Arc<Notify>,
}

impl Seen {
    fn requests(&self) -> Vec<RequestId> {
        self.requests.lock().unwrap().clone()
    }
    fn cancels(&self) -> Vec<RequestId> {
        self.cancels.lock().unwrap().clone()
    }
}

/// `quick` answers at once; any other tool, and every resource read, waits
/// until the request is cancelled.
struct Server {
    seen: Seen,
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

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.seen.requests.lock().unwrap().push(context.id.clone());
        self.seen.request_arrived.notify_one();
        if request.name == "quick" {
            return Ok(CallToolResult::success(vec![Content::text("done")]));
        }
        context.ct.cancelled().await;
        Err(ErrorData::internal_error("cancelled", None))
    }

    async fn read_resource(
        &self,
        _request: ReadResourceRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, ErrorData> {
        self.seen.requests.lock().unwrap().push(context.id.clone());
        self.seen.request_arrived.notify_one();
        context.ct.cancelled().await;
        Err(ErrorData::internal_error("cancelled", None))
    }

    async fn on_cancelled(
        &self,
        notification: CancelledNotificationParam,
        _context: NotificationContext<RoleServer>,
    ) {
        self.seen
            .cancels
            .lock()
            .unwrap()
            .push(notification.request_id);
        self.seen.cancel_arrived.notify_one();
    }
}

/// A connection to a fresh recording server over an in-memory transport.
async fn connect() -> (McpConnection, Seen, tokio::task::JoinHandle<()>) {
    let seen = Seen::default();
    let server = Server { seen: seen.clone() };
    let (client_io, server_io) = tokio::io::duplex(8192);
    let server_task = tokio::spawn(async move {
        let running = server.serve(server_io).await.expect("server handshake");
        let _ = running.waiting().await;
    });
    let service: RunningService<RoleClient, ()> =
        ().serve(client_io).await.expect("client handshake");
    let connection = McpConnection {
        config: McpServerConfig::stdio(
            "cancel-fixture",
            "unused-duplex",
            vec![],
            Default::default(),
        ),
        connection_id: crate::McpConnectionId::allocate().unwrap(),
        standard_tools: Default::default(),
        protected_metadata: Default::default(),
        service: service.into(),
        stdio_child: None,
        session_expiry: Default::default(),
        oauth_target: None,
        attempt: None,
        account: Arc::new(meerkat_core::OperationAccount::Unavailable),
        service_secret_launch: None,
        credential_source: None,
    };
    (connection, seen, server_task)
}

/// Waits for the server's first cancellation, then for any stray second one.
async fn settled_cancels(seen: &Seen) -> Vec<RequestId> {
    let _ = tokio::time::timeout(Duration::from_secs(2), seen.cancel_arrived.notified()).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    seen.cancels()
}

async fn finish(connection: McpConnection, server_task: tokio::task::JoinHandle<()>) {
    connection.close().await.expect("client cleanup");
    server_task.await.expect("server task joined");
}

/// A tool call whose future is dropped while the server works on it sends
/// one cancellation, for that request.
#[tokio::test]
async fn a_dropped_tool_call_is_cancelled_once() {
    let (connection, seen, server_task) = connect().await;
    let args = json!({});
    let mut call = Box::pin(connection.call_tool("slow", &args));
    tokio::select! {
        result = &mut call => panic!("the slow call returned: {result:?}"),
        () = seen.request_arrived.notified() => {}
    }
    drop(call);

    let cancels = settled_cancels(&seen).await;
    assert_eq!(seen.requests().len(), 1);
    assert_eq!(
        cancels,
        seen.requests(),
        "one cancellation, naming the call"
    );
    finish(connection, server_task).await;
}

/// A tool call whose caller's deadline expires sends one cancellation, for
/// that request.
#[tokio::test]
async fn a_tool_call_past_its_deadline_is_cancelled_once() {
    let (connection, seen, server_task) = connect().await;
    let args = json!({});
    let timed_out = tokio::time::timeout(
        Duration::from_millis(300),
        connection.call_tool("slow", &args),
    )
    .await;
    assert!(timed_out.is_err(), "the caller's deadline expired");

    let cancels = settled_cancels(&seen).await;
    assert_eq!(seen.requests().len(), 1);
    assert_eq!(
        cancels,
        seen.requests(),
        "one cancellation, naming the call"
    );
    finish(connection, server_task).await;
}

/// A call that completes, inside a deadline or not, sends no cancellation.
#[tokio::test]
async fn a_completed_tool_call_sends_no_cancellation() {
    let (connection, seen, server_task) = connect().await;
    let args = json!({});
    connection
        .call_tool("quick", &args)
        .await
        .expect("quick answers");
    tokio::time::timeout(Duration::from_secs(5), connection.call_tool("quick", &args))
        .await
        .expect("inside the deadline")
        .expect("quick answers");

    let cancels = settled_cancels(&seen).await;
    assert_eq!(seen.requests().len(), 2);
    assert!(cancels.is_empty(), "no cancellation: {cancels:?}");
    finish(connection, server_task).await;
}

/// A resource read whose deadline passes sends one cancellation, for that
/// request, even when its entry step takes part of the time.
#[tokio::test]
async fn a_resource_read_past_its_deadline_is_cancelled_once() {
    let (connection, seen, server_task) = connect().await;
    let deadline = tokio::time::Instant::now() + Duration::from_millis(300);
    let read = connection
        .read_resource_entering("ui://fixture/slow", None, deadline, || {
            // A slow entry step: the read is handed over 50 ms into its time.
            std::thread::sleep(Duration::from_millis(50));
            Ok(())
        })
        .await;
    assert!(read.is_err(), "the read timed out: {read:?}");

    let cancels = settled_cancels(&seen).await;
    assert_eq!(seen.requests().len(), 1);
    assert_eq!(
        cancels,
        seen.requests(),
        "one cancellation, naming the read"
    );
    finish(connection, server_task).await;
}
