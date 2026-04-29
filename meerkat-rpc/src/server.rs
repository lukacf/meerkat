//! RPC server main loop.
//!
//! Wires together the JSON-RPC transport, method router, and notification
//! channel. Uses `tokio::select!` to multiplex incoming requests, outgoing
//! responses from spawned dispatch tasks, and event notifications.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use meerkat::surface::{
    RequestTerminal, RequestTerminalResolution, SurfaceRequestExecutor, SurfaceRequestSemantics,
    noop_request_action,
};
use tokio::io::{AsyncBufRead, BufReader};
use tokio::sync::{mpsc, oneshot};

use meerkat_core::ConfigStore;

use crate::NOTIFICATION_CHANNEL_CAPACITY;
use crate::handlers::RpcResponseExt;
use crate::protocol::{RpcId, RpcMessage, RpcNotification, RpcRequest, RpcResponse};
use crate::router::{MethodRouter, NotificationSink};
use crate::session_runtime::SessionRuntime;
use crate::transport::{BlockingWriter, JsonlTransport, TransportError, TransportWriter};

struct LongRunningResponse {
    request_key: String,
    terminal: RequestTerminal<RpcResponse>,
}

/// Errors from the RPC server.
#[derive(Debug, thiserror::Error)]
pub enum ServerError {
    /// Transport-level error.
    #[error("Transport error: {0}")]
    Transport(#[from] TransportError),
    /// I/O error.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

/// JSON-RPC server over async reader/writer streams.
pub struct RpcServer<R, W> {
    transport: JsonlTransport<R, W>,
    router: MethodRouter,
    notification_rx: mpsc::Receiver<RpcNotification>,
    /// Channel for responses from concurrently dispatched requests.
    response_rx: mpsc::Receiver<RpcResponse>,
    response_tx: mpsc::Sender<RpcResponse>,
    /// Channel for callback requests sent from tool dispatchers → server loop → client.
    callback_request_rx: mpsc::Receiver<(RpcRequest, oneshot::Sender<RpcResponse>)>,
    /// Sender half cloned into `CallbackToolDispatcher` instances.
    callback_request_tx: mpsc::Sender<(RpcRequest, oneshot::Sender<RpcResponse>)>,
    /// Outstanding server→client requests awaiting a response.
    pending_callbacks: HashMap<RpcId, oneshot::Sender<RpcResponse>>,
    /// Counter for generating server-originated request IDs.
    callback_id_counter: Arc<AtomicU64>,
    /// Tool definitions registered via `tools/register`.
    registered_tools: Arc<std::sync::RwLock<Vec<meerkat_core::ToolDef>>>,
    long_running_tx: mpsc::Sender<LongRunningResponse>,
    long_running_rx: mpsc::Receiver<LongRunningResponse>,
    request_executor: SurfaceRequestExecutor,
    /// When true, skip runtime shutdown on EOF. Used by `serve_tcp` where the
    /// runtime is shared across sequential connections and must not be destroyed
    /// when a single client disconnects.
    pub skip_shutdown_on_eof: bool,
}

impl<R: AsyncBufRead + Unpin, W: TransportWriter> RpcServer<R, W> {
    /// Create a new RPC server.
    ///
    /// The server reads JSON-RPC requests from `reader`, dispatches them
    /// via the `MethodRouter`, and writes responses/notifications to `writer`.
    pub fn new(
        reader: R,
        writer: W,
        runtime: Arc<SessionRuntime>,
        config_store: Arc<dyn ConfigStore>,
    ) -> Self {
        Self::new_with_skill_runtime(reader, writer, runtime, config_store, None)
    }

    /// Create a new RPC server with an optional skill runtime for introspection.
    pub fn new_with_skill_runtime(
        reader: R,
        writer: W,
        runtime: Arc<SessionRuntime>,
        config_store: Arc<dyn ConfigStore>,
        skill_runtime: Option<Arc<meerkat_core::skills::SkillRuntime>>,
    ) -> Self {
        let (notification_tx, notification_rx) = mpsc::channel(NOTIFICATION_CHANNEL_CAPACITY);
        let notification_sink = NotificationSink::new(notification_tx);
        let transport = JsonlTransport::new(reader, writer);
        let (response_tx, response_rx) = mpsc::channel(NOTIFICATION_CHANNEL_CAPACITY);
        let (callback_request_tx, callback_request_rx) =
            mpsc::channel(NOTIFICATION_CHANNEL_CAPACITY);
        let callback_id_counter = Arc::new(AtomicU64::new(0));
        let registered_tools = Arc::new(std::sync::RwLock::new(Vec::new()));
        let (long_running_tx, long_running_rx) = mpsc::channel(NOTIFICATION_CHANNEL_CAPACITY);

        // Wire callback tool state into the runtime so session/create handlers
        // can build CallbackToolDispatchers that route through this server.
        runtime.set_callback_channel(
            callback_request_tx.clone(),
            callback_id_counter.clone(),
            registered_tools.clone(),
        );

        let router = MethodRouter::new(runtime, config_store, notification_sink)
            .with_skill_runtime(skill_runtime);
        Self {
            transport,
            router,
            notification_rx,
            response_rx,
            response_tx,
            callback_request_rx,
            callback_request_tx,
            pending_callbacks: HashMap::new(),
            callback_id_counter,
            registered_tools,
            long_running_tx,
            long_running_rx,
            request_executor: SurfaceRequestExecutor::new(tokio::time::Duration::from_secs(5)),
            skip_shutdown_on_eof: false,
        }
    }

    /// Attach the shared realtime websocket bootstrap host.
    pub fn with_realtime_ws_host(mut self, host: Arc<crate::realtime_ws::RealtimeWsHost>) -> Self {
        self.router = self.router.with_realtime_ws_host(host);
        self
    }

    #[cfg(feature = "mob")]
    /// Create a new RPC server with an optional skill runtime and explicit mob state.
    ///
    /// `callback_rx` is the receiver half of the callback channel, pre-created via
    /// `SessionRuntime::init_callback_channel()`. This ensures the callback channel
    /// exists before mob resume (so `ExternalToolsProvider` closures can read it),
    /// while the server owns the receiver for the `tool/execute` dispatch loop.
    pub fn new_with_skill_runtime_and_mob_state(
        reader: R,
        writer: W,
        runtime: Arc<SessionRuntime>,
        config_store: Arc<dyn ConfigStore>,
        skill_runtime: Option<Arc<meerkat_core::skills::SkillRuntime>>,
        mob_state: Arc<meerkat_mob_mcp::MobMcpState>,
        callback_request_rx: mpsc::Receiver<(
            crate::protocol::RpcRequest,
            tokio::sync::oneshot::Sender<crate::protocol::RpcResponse>,
        )>,
    ) -> Self {
        let (notification_tx, notification_rx) = mpsc::channel(NOTIFICATION_CHANNEL_CAPACITY);
        let notification_sink = NotificationSink::new(notification_tx);
        let transport = JsonlTransport::new(reader, writer);
        let (response_tx, response_rx) = mpsc::channel(NOTIFICATION_CHANNEL_CAPACITY);
        let (long_running_tx, long_running_rx) = mpsc::channel(NOTIFICATION_CHANNEL_CAPACITY);

        // Callback channel was pre-created by the caller via init_callback_channel().
        // Read the shared state (tx, counter, tools) from the runtime.
        // If somehow not initialized, the callback_request_rx the caller passed
        // will never receive, but the server won't crash.
        let callback_request_tx = runtime.callback_request_tx().unwrap_or_else(|| {
            tracing::warn!(
                "callback channel not pre-initialized on runtime; callback tools will not work"
            );
            let (tx, _) = mpsc::channel(1);
            tx
        });
        let callback_id_counter = runtime.callback_id_counter();
        let registered_tools = runtime.registered_tools();

        let router =
            MethodRouter::new_with_mob_state(runtime, config_store, notification_sink, mob_state)
                .with_skill_runtime(skill_runtime);
        Self {
            transport,
            router,
            notification_rx,
            response_rx,
            response_tx,
            callback_request_rx,
            callback_request_tx,
            pending_callbacks: HashMap::new(),
            callback_id_counter,
            registered_tools,
            long_running_tx,
            long_running_rx,
            request_executor: SurfaceRequestExecutor::new(tokio::time::Duration::from_secs(5)),
            skip_shutdown_on_eof: false,
        }
    }

    /// Get the callback request sender for creating `CallbackToolDispatcher` instances.
    pub fn callback_request_tx(&self) -> mpsc::Sender<(RpcRequest, oneshot::Sender<RpcResponse>)> {
        self.callback_request_tx.clone()
    }

    /// Get the callback ID counter for generating unique server-originated IDs.
    pub fn callback_id_counter(&self) -> Arc<AtomicU64> {
        self.callback_id_counter.clone()
    }

    /// Get the shared registered tools list.
    pub fn registered_tools(&self) -> Arc<std::sync::RwLock<Vec<meerkat_core::ToolDef>>> {
        self.registered_tools.clone()
    }

    /// Run the server until EOF or a fatal I/O error.
    ///
    /// Requests are dispatched concurrently in spawned tasks so that long-running
    /// operations (e.g. `session/create`, `turn/start`) do not block the server
    /// from processing other requests (e.g. `turn/interrupt`).
    ///
    /// Parse errors are reported to the client and do not terminate the loop.
    /// EOF (reader returns `None`) triggers graceful shutdown.
    pub async fn run(&mut self) -> Result<(), ServerError> {
        // Periodic sweep for timed-out callback entries. Fires even when
        // the connection is idle, preventing stale leaks.
        let mut callback_sweep = tokio::time::interval(tokio::time::Duration::from_secs(30));
        callback_sweep.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                biased;

                // Read the next message from the transport.
                msg = self.transport.read_message() => {
                    match msg {
                        Ok(Some(RpcMessage::Request(request))) => {
                            // Handle `tools/register` synchronously (needs server state).
                            if request.method == "tools/register" {
                                let response = self.handle_tools_register(
                                    request.id.clone(),
                                    request.params.as_deref(),
                                );
                                self.transport.write_response(&response).await?;
                                continue;
                            }

                            if request.is_notification() && request.method == "cancel" {
                                if let Some(target) = request_cancel_target(request.params.as_deref()) {
                                    let _ = self.request_executor.cancel_request(&target).await;
                                }
                                continue;
                            }

                            if let Some((request_key, task)) = self.spawn_long_running_request(&request) {
                                self.request_executor
                                    .attach_task(&request_key, task);
                                continue;
                            }

                            let router = self.router.clone();
                            let resp_tx = self.response_tx.clone();
                            tokio::spawn(async move {
                                if let Some(response) = Box::pin(router.dispatch(request)).await {
                                    let _ = resp_tx.send(response).await;
                                }
                            });
                        }
                        Ok(Some(RpcMessage::Response(response))) => {
                            // Callback response from the client.
                            if let Some(ref resp_id) = response.id {
                                if let Some(tx) = self.pending_callbacks.remove(resp_id) {
                                    let _ = tx.send(response);
                                } else {
                                    tracing::warn!(
                                        id = ?resp_id,
                                        "Received response for unknown callback ID"
                                    );
                                }
                            }
                        }
                        Ok(None) => break, // EOF - clean shutdown
                        Err(TransportError::Parse(err)) => {
                            let response = RpcResponse::error(
                                None,
                                crate::error::PARSE_ERROR,
                                format!("Parse error: {err}"),
                            );
                            self.transport.write_response(&response).await?;
                        }
                        Err(TransportError::Io(err)) => {
                            return Err(ServerError::Io(err));
                        }
                    }
                }

                // Write responses from completed dispatch tasks.
                Some(response) = self.response_rx.recv() => {
                    self.transport.write_response(&response).await?;
                }

                Some(response) = self.long_running_rx.recv() => {
                    if !self.write_long_running_response(response).await {
                        break;
                    }
                }

                // Forward queued notifications to the transport.
                Some(notification) = self.notification_rx.recv() => {
                    self.transport.write_notification(&notification).await?;
                }

                // Send callback requests to the client.
                Some((request, response_tx)) = self.callback_request_rx.recv() => {
                    if let Some(ref req_id) = request.id {
                        self.pending_callbacks.insert(req_id.clone(), response_tx);
                    }
                    self.transport.write_request(&request).await?;
                }

                // Periodic sweep of timed-out callback entries.
                _ = callback_sweep.tick() => {
                    self.pending_callbacks.retain(|_, tx| !tx.is_closed());
                }
            }
        }

        // Graceful shutdown: close all sessions (unless this is a shared TCP
        // runtime where client disconnect should not destroy state).
        self.request_executor.shutdown_and_abort_stragglers().await;
        if !self.skip_shutdown_on_eof {
            self.router.runtime().shutdown().await;
        }
        Ok(())
    }

    fn spawn_long_running_request(
        &self,
        request: &RpcRequest,
    ) -> Option<(String, tokio::task::JoinHandle<()>)> {
        let semantics = request_semantics(request);
        if !semantics.requires_long_running_executor() {
            return None;
        }
        let id = request.id.clone()?;
        let request_key = request_key(&id);
        let context = self
            .request_executor
            .begin_request(request_key.clone(), noop_request_action());
        let router = self.router.clone();
        let long_running_tx = self.long_running_tx.clone();
        let request = request.clone();
        let request_key_for_task = request_key.clone();
        let handle = tokio::spawn(async move {
            if let Some(response) =
                Box::pin(router.dispatch_with_request_context(request, Some(context))).await
            {
                let terminal = classify_long_running_response(&response, semantics);
                let _ = long_running_tx
                    .send(LongRunningResponse {
                        request_key: request_key_for_task,
                        terminal,
                    })
                    .await;
            }
        });
        Some((request_key, handle))
    }

    async fn write_long_running_response(&mut self, response: LongRunningResponse) -> bool {
        let cancel_id = response.terminal.payload().id.clone();
        let to_write = match self
            .request_executor
            .resolve_terminal(Some(&response.request_key), response.terminal)
            .await
        {
            RequestTerminalResolution::Emit(rpc_response) => rpc_response,
            RequestTerminalResolution::Cancelled => request_cancelled_response(cancel_id),
            RequestTerminalResolution::LifecycleError(err) => {
                tracing::warn!(
                    request_key = %response.request_key,
                    error = %err,
                    "request lifecycle rejected publish response"
                );
                request_lifecycle_error_response(cancel_id, err)
            }
        };

        self.transport.write_response(&to_write).await.is_ok()
    }

    /// Handle `tools/register` — register callback tool definitions.
    fn handle_tools_register(
        &self,
        id: Option<RpcId>,
        params: Option<&serde_json::value::RawValue>,
    ) -> RpcResponse {
        #[derive(serde::Deserialize)]
        struct RegisterParams {
            tools: Vec<meerkat_core::ToolDef>,
        }

        let params: RegisterParams = match crate::handlers::parse_params(params) {
            Ok(p) => p,
            Err(resp) => {
                return resp.with_id(id);
            }
        };

        match self.registered_tools.write() {
            Ok(mut tools) => {
                let count = params.tools.len();
                for new_tool in params.tools {
                    let stamped = meerkat_core::ToolDef {
                        provenance: Some(meerkat_core::types::ToolProvenance {
                            kind: meerkat_core::types::ToolSourceKind::Callback,
                            source_id: "callback".into(),
                        }),
                        ..new_tool
                    };
                    if let Some(existing) = tools.iter_mut().find(|t| t.name == stamped.name) {
                        *existing = stamped;
                    } else {
                        tools.push(stamped);
                    }
                }
                RpcResponse::success(id, serde_json::json!({ "registered": count }))
            }
            Err(_) => RpcResponse::error(
                id,
                crate::error::INTERNAL_ERROR,
                "Failed to acquire tool registry lock",
            ),
        }
    }
}

fn classify_long_running_response(
    response: &RpcResponse,
    semantics: SurfaceRequestSemantics,
) -> RequestTerminal<RpcResponse> {
    semantics.classify_terminal(response.error.is_none(), response.clone())
}

fn request_key(id: &RpcId) -> String {
    serde_json::to_string(id).unwrap_or_else(|_| format!("{id:?}"))
}

fn request_cancel_target(params: Option<&serde_json::value::RawValue>) -> Option<String> {
    let params = params?;
    let value: serde_json::Value = serde_json::from_str(params.get()).ok()?;
    let request_id = value.get("request_id")?;
    serde_json::to_string(request_id).ok()
}

fn request_cancelled_response(id: Option<RpcId>) -> RpcResponse {
    RpcResponse::error(
        id,
        crate::error::REQUEST_CANCELLED,
        "request cancelled before response publish",
    )
}

fn request_lifecycle_error_response(
    id: Option<RpcId>,
    err: meerkat::surface::RequestTransitionError,
) -> RpcResponse {
    RpcResponse::error(
        id,
        crate::error::INTERNAL_ERROR,
        format!("request lifecycle rejected publish response: {err}"),
    )
}

fn request_semantics(request: &RpcRequest) -> SurfaceRequestSemantics {
    SurfaceRequestSemantics::for_rpc_request(
        request.method.as_str(),
        request.params.as_deref().map(|params| params.get()),
    )
}

/// Start the RPC server on stdin/stdout.
///
/// This is the main entry point called from the CLI `rpc` subcommand.
pub async fn serve_stdio(
    runtime: Arc<SessionRuntime>,
    config_store: Arc<dyn ConfigStore>,
) -> Result<(), ServerError> {
    serve_stdio_with_skill_runtime(runtime, config_store, None).await
}

/// Start the RPC server on stdin/stdout with an optional skill runtime.
pub async fn serve_stdio_with_skill_runtime(
    runtime: Arc<SessionRuntime>,
    config_store: Arc<dyn ConfigStore>,
    skill_runtime: Option<Arc<meerkat_core::skills::SkillRuntime>>,
) -> Result<(), ServerError> {
    serve_stdio_with_skill_runtime_and_realtime_ws_host(runtime, config_store, skill_runtime, None)
        .await
}

/// Start the RPC server on stdin/stdout with an optional sibling realtime host.
pub async fn serve_stdio_with_skill_runtime_and_realtime_ws_host(
    runtime: Arc<SessionRuntime>,
    config_store: Arc<dyn ConfigStore>,
    skill_runtime: Option<Arc<meerkat_core::skills::SkillRuntime>>,
    realtime_ws_host: Option<Arc<crate::realtime_ws::RealtimeWsHost>>,
) -> Result<(), ServerError> {
    let stdin = tokio::io::stdin();
    let stdout = BlockingWriter::stdout();
    let reader = BufReader::new(stdin);
    let mut server =
        RpcServer::new_with_skill_runtime(reader, stdout, runtime, config_store, skill_runtime);
    if let Some(realtime_ws_host) = realtime_ws_host {
        server = server.with_realtime_ws_host(realtime_ws_host);
    }
    server.run().await
}

/// Accept a single RPC client over an already-connected TCP stream.
///
/// The stream is split into read/write halves and wired directly into the
/// generic `RpcServer`. The caller is responsible for the accept loop —
/// this function handles exactly one connection lifetime.
pub async fn serve_tcp_connection(
    stream: tokio::net::TcpStream,
    runtime: Arc<SessionRuntime>,
    config_store: Arc<dyn ConfigStore>,
    skill_runtime: Option<Arc<meerkat_core::skills::SkillRuntime>>,
) -> Result<(), ServerError> {
    serve_tcp_connection_with_realtime_ws_host(stream, runtime, config_store, skill_runtime, None)
        .await
}

/// Accept a single RPC TCP client with an optional sibling realtime host.
pub async fn serve_tcp_connection_with_realtime_ws_host(
    stream: tokio::net::TcpStream,
    runtime: Arc<SessionRuntime>,
    config_store: Arc<dyn ConfigStore>,
    skill_runtime: Option<Arc<meerkat_core::skills::SkillRuntime>>,
    realtime_ws_host: Option<Arc<crate::realtime_ws::RealtimeWsHost>>,
) -> Result<(), ServerError> {
    let (read_half, write_half) = stream.into_split();
    let reader = BufReader::new(read_half);
    let mut server =
        RpcServer::new_with_skill_runtime(reader, write_half, runtime, config_store, skill_runtime);
    if let Some(realtime_ws_host) = realtime_ws_host {
        server = server.with_realtime_ws_host(realtime_ws_host);
    }
    server.skip_shutdown_on_eof = true;
    server.run().await
}

/// Listen on a TCP address and serve RPC clients concurrently.
///
/// Each accepted connection is spawned into its own task with a dedicated
/// `RpcServer` sharing the underlying `SessionRuntime`. This ensures a
/// dead client (missing TCP FIN after kill) does not block new connections.
pub async fn serve_tcp(
    addr: &str,
    runtime: Arc<SessionRuntime>,
    config_store: Arc<dyn ConfigStore>,
    skill_runtime: Option<Arc<meerkat_core::skills::SkillRuntime>>,
) -> Result<(), ServerError> {
    serve_tcp_with_realtime_ws_host(addr, runtime, config_store, skill_runtime, None).await
}

/// Listen on a TCP address and serve RPC clients with an optional realtime host.
pub async fn serve_tcp_with_realtime_ws_host(
    addr: &str,
    runtime: Arc<SessionRuntime>,
    config_store: Arc<dyn ConfigStore>,
    skill_runtime: Option<Arc<meerkat_core::skills::SkillRuntime>>,
    realtime_ws_host: Option<Arc<crate::realtime_ws::RealtimeWsHost>>,
) -> Result<(), ServerError> {
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!("RPC TCP listener bound to {addr}");
    loop {
        let (stream, peer_addr) = listener.accept().await?;
        tracing::info!("RPC client connected from {peer_addr}");
        let runtime = Arc::clone(&runtime);
        let config_store = Arc::clone(&config_store);
        let skill_runtime = skill_runtime.clone();
        let realtime_ws_host = realtime_ws_host.clone();
        tokio::spawn(async move {
            if let Err(e) = serve_tcp_connection_with_realtime_ws_host(
                stream,
                runtime,
                config_store,
                skill_runtime,
                realtime_ws_host,
            )
            .await
            {
                tracing::warn!("RPC client {peer_addr} disconnected: {e}");
            } else {
                tracing::info!("RPC client {peer_addr} disconnected cleanly");
            }
        });
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn raw_params(value: serde_json::Value) -> Box<serde_json::value::RawValue> {
        let raw = serde_json::value::to_raw_value(&value);
        assert!(raw.is_ok(), "raw params should serialize: {raw:?}");
        match raw {
            Ok(raw) => raw,
            Err(_) => unreachable!("assert above"),
        }
    }

    #[test]
    fn deferred_session_create_uses_simple_path() {
        let request = RpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(RpcId::Num(1)),
            method: "session/create".to_string(),
            params: Some(raw_params(serde_json::json!({
                "prompt": "hello",
                "initial_turn": "deferred"
            }))),
        };

        assert!(!request_semantics(&request).requires_long_running_executor());
    }

    #[test]
    fn immediate_session_create_uses_long_running_executor() {
        let request = RpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(RpcId::Num(1)),
            method: "session/create".to_string(),
            params: Some(raw_params(serde_json::json!({
                "prompt": "hello"
            }))),
        };

        assert!(request_semantics(&request).requires_long_running_executor());
    }

    #[test]
    fn turn_start_is_publish_on_success() {
        let request = RpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(RpcId::Num(1)),
            method: "turn/start".to_string(),
            params: Some(raw_params(serde_json::json!({
                "session_id": "01234567-89ab-cdef-0123-456789abcdef",
                "prompt": "hello"
            }))),
        };

        let semantics = request_semantics(&request);
        assert!(semantics.requires_long_running_executor());
        let response = RpcResponse::success(request.id, serde_json::json!({"ok": true}));
        assert!(matches!(
            classify_long_running_response(&response, semantics),
            RequestTerminal::Publish(_)
        ));
    }

    #[derive(Clone)]
    struct SharedBufferWriter(Arc<std::sync::Mutex<Vec<u8>>>);

    #[async_trait]
    impl TransportWriter for SharedBufferWriter {
        async fn write_message(&mut self, bytes: Vec<u8>) -> Result<(), TransportError> {
            let mut output = self
                .0
                .lock()
                .map_err(|_| std::io::Error::other("test writer mutex poisoned"))?;
            output.extend(bytes);
            Ok(())
        }
    }

    #[tokio::test]
    async fn publish_completion_after_cancel_writes_cancel_response() {
        let temp = tempfile::tempdir().unwrap();
        let (runtime, config_store) = build_test_runtime(&temp);
        let output = Arc::new(std::sync::Mutex::new(Vec::new()));
        let reader = BufReader::new(std::io::Cursor::new(Vec::new()));
        let writer = SharedBufferWriter(Arc::clone(&output));
        let mut server = RpcServer::new(reader, writer, runtime, config_store);
        let request_id = RpcId::Num(42);
        let request_key = request_key(&request_id);
        let _ctx = server
            .request_executor
            .begin_request(request_key.clone(), noop_request_action());

        assert_eq!(
            server.request_executor.cancel_request(&request_key).await,
            meerkat::surface::CancelOutcome::Cancelled
        );

        let publish_response =
            RpcResponse::success(Some(request_id.clone()), serde_json::json!({"ok": true}));
        assert!(
            server
                .write_long_running_response(LongRunningResponse {
                    request_key: request_key.clone(),
                    terminal: RequestTerminal::Publish(publish_response),
                })
                .await
        );

        assert_eq!(server.request_executor.phase(&request_key), None);
        let bytes = output.lock().expect("output lock").clone();
        let line = String::from_utf8(bytes).expect("output should be utf8");
        let response: RpcResponse =
            serde_json::from_str(line.trim()).expect("response should parse");
        assert_eq!(response.id, Some(request_id));
        assert!(response.result.is_none());
        assert_eq!(
            response.error.expect("expected cancel error").code,
            crate::error::REQUEST_CANCELLED
        );
    }

    // -----------------------------------------------------------------------
    // TCP integration tests
    // -----------------------------------------------------------------------

    use std::pin::Pin;
    use std::sync::Arc;

    use async_trait::async_trait;
    use futures::{SinkExt, StreamExt, stream};
    use meerkat::AgentFactory;
    use meerkat_client::{LlmClient, LlmError};
    use meerkat_contracts::{
        RealtimeCapabilities, RealtimeChannelOpenFrame, RealtimeChannelRole, RealtimeChannelTarget,
        RealtimeClientFrame, RealtimeInputKind, RealtimeOpenRequest, RealtimeOutputKind,
        RealtimeServerFrame, RealtimeTurningMode,
    };
    use meerkat_core::lifecycle::RunId;
    use meerkat_core::lifecycle::core_executor::{
        CoreApplyOutput, CoreExecutor, CoreExecutorError,
    };
    use meerkat_core::lifecycle::run_control::RunControlCommand;
    use meerkat_core::lifecycle::run_primitive::RunPrimitive;
    use meerkat_core::{Config, ConfigRuntime, MemoryConfigStore, StopReason};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader as TokioBufReader};
    use tokio::net::TcpStream;
    use tokio_tungstenite::{connect_async, tungstenite::Message as WsMessage};

    /// Mock LLM that immediately returns "Hello from mock" and ends the turn.
    struct MockLlmClient;

    #[async_trait]
    impl LlmClient for MockLlmClient {
        fn stream<'a>(
            &'a self,
            _request: &'a meerkat_client::LlmRequest,
        ) -> Pin<
            Box<dyn futures::Stream<Item = Result<meerkat_client::LlmEvent, LlmError>> + Send + 'a>,
        > {
            Box::pin(stream::iter(vec![
                Ok(meerkat_client::LlmEvent::TextDelta {
                    delta: "Hello from mock".to_string(),
                    meta: None,
                }),
                Ok(meerkat_client::LlmEvent::Done {
                    outcome: meerkat_client::LlmDoneOutcome::Success {
                        stop_reason: StopReason::EndTurn,
                    },
                }),
            ]))
        }

        fn provider(&self) -> &'static str {
            "mock"
        }

        async fn health_check(&self) -> Result<(), LlmError> {
            Ok(())
        }
    }

    fn memory_blob_store() -> Arc<dyn meerkat_core::BlobStore> {
        Arc::new(meerkat_store::MemoryBlobStore::new())
    }

    /// Build a `SessionRuntime` + `ConfigStore` pair for TCP tests.
    fn build_test_runtime(
        temp: &tempfile::TempDir,
    ) -> (Arc<SessionRuntime>, Arc<dyn meerkat_core::ConfigStore>) {
        let factory = AgentFactory::new(temp.path().join("sessions"));
        let config = Config::default();
        let store: Arc<dyn meerkat::SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let mut runtime = SessionRuntime::new(
            factory,
            config,
            10,
            meerkat::PersistenceBundle::new(store, None, memory_blob_store()),
            crate::router::NotificationSink::noop(),
        );
        let config_store: Arc<dyn meerkat_core::ConfigStore> =
            Arc::new(MemoryConfigStore::new(Config::default()));
        runtime.set_default_llm_client(Some(Arc::new(MockLlmClient)));
        runtime.set_config_runtime(Arc::new(ConfigRuntime::new(
            Arc::clone(&config_store),
            temp.path().join("config_state.json"),
        )));
        (Arc::new(runtime), config_store)
    }

    struct NeverAppliedExecutor;

    #[async_trait]
    impl CoreExecutor for NeverAppliedExecutor {
        async fn apply(
            &mut self,
            _run_id: RunId,
            _primitive: RunPrimitive,
        ) -> Result<CoreApplyOutput, CoreExecutorError> {
            unreachable!("server realtime websocket tests do not drive executor apply")
        }

        async fn control(&mut self, _command: RunControlCommand) -> Result<(), CoreExecutorError> {
            Ok(())
        }
    }

    async fn register_live_session(runtime: &Arc<SessionRuntime>, session_id: &str) {
        let session_id =
            meerkat_core::SessionId::parse(session_id).expect("session_id should parse");
        runtime
            .runtime_adapter()
            .register_session_with_executor(session_id, Box::new(NeverAppliedExecutor))
            .await;
    }

    /// Send a single JSONL line over a TCP stream.
    async fn send_jsonl(stream: &mut TcpStream, value: &serde_json::Value) {
        let mut bytes = serde_json::to_vec(value).unwrap();
        bytes.push(b'\n');
        stream.write_all(&bytes).await.unwrap();
        stream.flush().await.unwrap();
    }

    /// Read a single JSONL line from a buffered reader, with a timeout.
    async fn read_jsonl_line(
        reader: &mut TokioBufReader<tokio::net::tcp::OwnedReadHalf>,
    ) -> Option<serde_json::Value> {
        let mut line = String::new();
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            reader.read_line(&mut line),
        )
        .await;
        match result {
            Ok(Ok(0)) => None, // EOF
            Ok(Ok(_)) => {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    return None;
                }
                Some(serde_json::from_str(trimmed).unwrap())
            }
            Ok(Err(e)) => panic!("read error: {e}"),
            Err(elapsed) => panic!("read timed out after 5s: {elapsed}"),
        }
    }

    /// Read all JSONL lines until no more arrive within `timeout`.
    async fn drain_jsonl(
        reader: &mut TokioBufReader<tokio::net::tcp::OwnedReadHalf>,
        timeout: std::time::Duration,
    ) -> Vec<serde_json::Value> {
        let mut lines = Vec::new();
        loop {
            let mut line = String::new();
            match tokio::time::timeout(timeout, reader.read_line(&mut line)).await {
                Ok(Ok(0)) => break, // EOF
                Ok(Ok(_)) => {
                    let trimmed = line.trim();
                    if !trimmed.is_empty() {
                        lines.push(serde_json::from_str(trimmed).unwrap());
                    }
                }
                Ok(Err(_)) => break,
                Err(_) => break, // timeout — no more data
            }
        }
        lines
    }

    // -- Test 1: TCP initialize handshake --

    #[tokio::test]
    async fn tcp_initialize_handshake() {
        let temp = tempfile::tempdir().unwrap();
        let (runtime, config_store) = build_test_runtime(&temp);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        // Spawn the server for one connection.
        let rt = Arc::clone(&runtime);
        let cs = Arc::clone(&config_store);
        let server_handle = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            serve_tcp_connection(stream, rt, cs, None).await
        });

        // Connect client.
        let mut client = TcpStream::connect(addr).await.unwrap();
        let init_request = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "initialize",
            "params": {},
            "id": 1
        });
        send_jsonl(&mut client, &init_request).await;

        let (read_half, _write_half) = client.into_split();
        let mut reader = TokioBufReader::new(read_half);
        let response = read_jsonl_line(&mut reader)
            .await
            .expect("expected response");

        // Verify it's a valid JSON-RPC success response.
        assert_eq!(response["jsonrpc"], "2.0");
        assert_eq!(response["id"], 1);
        assert!(response["error"].is_null(), "expected no error: {response}");

        let result = &response["result"];
        assert!(
            result["server_info"]["name"].is_string(),
            "expected server_info.name in result: {result}"
        );
        assert!(
            result["methods"].is_array(),
            "expected methods array in result: {result}"
        );

        // Drop the write half to close the connection -> server exits.
        drop(_write_half);
        drop(reader);

        let server_result = tokio::time::timeout(std::time::Duration::from_secs(5), server_handle)
            .await
            .expect("server should finish within timeout")
            .expect("server task should not panic");

        assert!(
            server_result.is_ok(),
            "serve_tcp_connection should return Ok on clean disconnect"
        );
    }

    // -- Test 2: Session create + turn over TCP --

    #[tokio::test]
    async fn tcp_session_create_deferred_then_turn() {
        let temp = tempfile::tempdir().unwrap();
        let (runtime, config_store) = build_test_runtime(&temp);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let rt = Arc::clone(&runtime);
        let cs = Arc::clone(&config_store);
        let server_handle = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            serve_tcp_connection(stream, rt, cs, None).await
        });

        let client = TcpStream::connect(addr).await.unwrap();
        let (read_half, mut write_half) = client.into_split();
        let mut reader = TokioBufReader::new(read_half);

        // 1. Initialize
        let init = serde_json::json!({
            "jsonrpc": "2.0", "method": "initialize", "params": {}, "id": 1
        });
        let mut bytes = serde_json::to_vec(&init).unwrap();
        bytes.push(b'\n');
        write_half.write_all(&bytes).await.unwrap();
        write_half.flush().await.unwrap();

        let resp = read_jsonl_line(&mut reader).await.expect("init response");
        assert_eq!(resp["id"], 1);
        assert!(resp["error"].is_null());

        // 2. Create session with deferred initial turn.
        let create = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "session/create",
            "params": {
                "prompt": "Say hello",
                "initial_turn": "deferred"
            },
            "id": 2
        });
        let mut bytes = serde_json::to_vec(&create).unwrap();
        bytes.push(b'\n');
        write_half.write_all(&bytes).await.unwrap();
        write_half.flush().await.unwrap();

        let resp = read_jsonl_line(&mut reader).await.expect("create response");
        assert_eq!(resp["id"], 2);
        assert!(resp["error"].is_null(), "session/create error: {resp}");

        let create_result = &resp["result"];
        let session_id = create_result["session_id"]
            .as_str()
            .expect("expected session_id in create response");
        assert!(!session_id.is_empty());

        // 3. Start a turn on the session.
        let turn = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "turn/start",
            "params": {
                "session_id": session_id,
                "prompt": "Say hello"
            },
            "id": 3
        });
        let mut bytes = serde_json::to_vec(&turn).unwrap();
        bytes.push(b'\n');
        write_half.write_all(&bytes).await.unwrap();
        write_half.flush().await.unwrap();

        // Drain all messages (notifications + final response) with a generous timeout.
        let all_messages = drain_jsonl(&mut reader, std::time::Duration::from_secs(5)).await;

        // There should be at least one message (the turn response).
        assert!(
            !all_messages.is_empty(),
            "expected at least the turn/start response"
        );

        // The turn response should be present (has id: 3).
        let turn_response = all_messages
            .iter()
            .find(|m| m.get("id").and_then(serde_json::Value::as_i64) == Some(3));
        assert!(
            turn_response.is_some(),
            "expected turn/start response with id=3 among: {all_messages:?}"
        );

        // Any session/event notifications should have the correct session_id.
        for msg in &all_messages {
            if msg.get("method").and_then(|v| v.as_str()) == Some("session/event") {
                let params = &msg["params"];
                assert_eq!(
                    params["session_id"].as_str(),
                    Some(session_id),
                    "notification session_id mismatch"
                );
            }
        }

        // Clean up: drop write half to close connection.
        drop(write_half);
        drop(reader);

        let server_result = tokio::time::timeout(std::time::Duration::from_secs(5), server_handle)
            .await
            .expect("server should finish")
            .expect("server task should not panic");

        assert!(server_result.is_ok());
    }

    // -- Test 3: Client disconnect does not panic --

    #[tokio::test]
    async fn tcp_client_disconnect_returns_ok() {
        let temp = tempfile::tempdir().unwrap();
        let (runtime, config_store) = build_test_runtime(&temp);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let rt = Arc::clone(&runtime);
        let cs = Arc::clone(&config_store);
        let server_handle = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            serve_tcp_connection(stream, rt, cs, None).await
        });

        // Connect, send initialize, read response, then immediately drop.
        {
            let mut client = TcpStream::connect(addr).await.unwrap();
            let init = serde_json::json!({
                "jsonrpc": "2.0", "method": "initialize", "params": {}, "id": 1
            });
            send_jsonl(&mut client, &init).await;

            let (read_half, _write_half) = client.into_split();
            let mut reader = TokioBufReader::new(read_half);
            let _resp = read_jsonl_line(&mut reader).await;
            // Drop everything — simulates client disconnect.
        }

        let server_result = tokio::time::timeout(std::time::Duration::from_secs(5), server_handle)
            .await
            .expect("server should finish within timeout")
            .expect("server task should not panic");

        // Server should return Ok, not an error, on a clean TCP close.
        assert!(
            server_result.is_ok(),
            "expected Ok on client disconnect, got: {server_result:?}"
        );
    }

    // -- Test 4: Multiple sequential connections via serve_tcp --

    #[tokio::test]
    async fn tcp_multiple_sequential_connections() {
        let temp = tempfile::tempdir().unwrap();
        let (runtime, config_store) = build_test_runtime(&temp);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        // Spawn serve_tcp in a background task (it loops forever).
        let rt = Arc::clone(&runtime);
        let cs = Arc::clone(&config_store);
        let addr_str = addr.to_string();
        let server_handle = tokio::spawn(async move {
            // We can't easily pass the pre-bound listener to serve_tcp since it
            // binds internally, so we use serve_tcp_connection in a manual loop
            // to simulate the same behavior.
            loop {
                let (stream, _) = match listener.accept().await {
                    Ok(s) => s,
                    Err(_) => break,
                };
                let _ = serve_tcp_connection(stream, Arc::clone(&rt), Arc::clone(&cs), None).await;
            }
        });

        // --- Client 1 ---
        {
            let mut client = TcpStream::connect(&addr_str).await.unwrap();
            let init = serde_json::json!({
                "jsonrpc": "2.0", "method": "initialize", "params": {}, "id": 1
            });
            send_jsonl(&mut client, &init).await;

            let (read_half, _write) = client.into_split();
            let mut reader = TokioBufReader::new(read_half);
            let resp = read_jsonl_line(&mut reader)
                .await
                .expect("client 1 init response");
            assert_eq!(resp["id"], 1);
            assert!(resp["error"].is_null());
            // Client 1 disconnects.
        }

        // Brief pause to let the server complete the first connection teardown.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // --- Client 2 ---
        {
            let mut client = TcpStream::connect(&addr_str).await.unwrap();
            let init = serde_json::json!({
                "jsonrpc": "2.0", "method": "initialize", "params": {}, "id": 42
            });
            send_jsonl(&mut client, &init).await;

            let (read_half, _write) = client.into_split();
            let mut reader = TokioBufReader::new(read_half);
            let resp = read_jsonl_line(&mut reader)
                .await
                .expect("client 2 init response");
            assert_eq!(resp["id"], 42);
            assert!(resp["error"].is_null());

            assert!(
                resp["result"]["server_info"]["name"].is_string(),
                "client 2 should receive valid capabilities"
            );
        }

        // Abort the infinite accept loop.
        server_handle.abort();
    }

    // -- Test 5: config/get round-trip over TCP --

    #[tokio::test]
    async fn tcp_config_get_round_trip() {
        let temp = tempfile::tempdir().unwrap();
        let (runtime, config_store) = build_test_runtime(&temp);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let rt = Arc::clone(&runtime);
        let cs = Arc::clone(&config_store);
        let server_handle = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            serve_tcp_connection(stream, rt, cs, None).await
        });

        let client = TcpStream::connect(addr).await.unwrap();
        let (read_half, mut write_half) = client.into_split();
        let mut reader = TokioBufReader::new(read_half);

        // Initialize first (required handshake).
        let init = serde_json::json!({
            "jsonrpc": "2.0", "method": "initialize", "params": {}, "id": 1
        });
        let mut bytes = serde_json::to_vec(&init).unwrap();
        bytes.push(b'\n');
        write_half.write_all(&bytes).await.unwrap();
        write_half.flush().await.unwrap();

        let init_resp = read_jsonl_line(&mut reader).await.expect("init response");
        assert!(init_resp["error"].is_null());

        // Send config/get.
        let config_get = serde_json::json!({
            "jsonrpc": "2.0", "method": "config/get", "params": {}, "id": 2
        });
        let mut bytes = serde_json::to_vec(&config_get).unwrap();
        bytes.push(b'\n');
        write_half.write_all(&bytes).await.unwrap();
        write_half.flush().await.unwrap();

        let resp = read_jsonl_line(&mut reader)
            .await
            .expect("config/get response");
        assert_eq!(resp["jsonrpc"], "2.0");
        assert_eq!(resp["id"], 2);
        assert!(resp["error"].is_null(), "config/get should succeed: {resp}");

        // The result should contain a "config" key.
        assert!(
            resp["result"].get("config").is_some(),
            "config/get result should contain 'config' key: {}",
            resp["result"]
        );

        // Clean up.
        drop(write_half);
        drop(reader);

        let server_result = tokio::time::timeout(std::time::Duration::from_secs(5), server_handle)
            .await
            .expect("server should finish")
            .expect("server task should not panic");

        assert!(server_result.is_ok());
    }

    // -- Test 6: sibling realtime websocket host can coexist with TCP host --

    #[tokio::test]
    async fn realtime_ws_listener_accepts_channel_open_and_coexists_with_tcp_initialize() {
        let temp = tempfile::tempdir().unwrap();
        let (runtime, config_store) = build_test_runtime(&temp);
        register_live_session(&runtime, "01234567-89ab-cdef-0123-456789abcdef").await;

        let tcp_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let tcp_addr = tcp_listener.local_addr().unwrap();
        let ws_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let ws_addr = ws_listener.local_addr().unwrap();
        let host = Arc::new(crate::realtime_ws::RealtimeWsHost::new(format!(
            "ws://{ws_addr}{}",
            crate::REALTIME_WS_PATH
        )));
        let open_info = host
            .issue_open_info(
                RealtimeOpenRequest {
                    target: RealtimeChannelTarget::SessionTarget {
                        session_id: "01234567-89ab-cdef-0123-456789abcdef".to_string(),
                    },
                    role: RealtimeChannelRole::Primary,
                    turning_mode: RealtimeTurningMode::ProviderManaged,
                    reconnect_policy: None,
                    channel_config: None,
                },
                RealtimeCapabilities {
                    input_kinds: vec![RealtimeInputKind::Text, RealtimeInputKind::Audio],
                    output_kinds: vec![RealtimeOutputKind::Text, RealtimeOutputKind::Audio],
                    turning_modes: vec![RealtimeTurningMode::ProviderManaged],
                    interrupt_supported: true,
                    transcript_supported: true,
                    tool_lifecycle_events_supported: false,
                    video_supported: false,
                    audio_input_format: None,
                    audio_output_format: None,
                },
                None,
            )
            .await;

        let tcp_rt = Arc::clone(&runtime);
        let tcp_cs = Arc::clone(&config_store);
        let tcp_server = tokio::spawn(async move {
            let (stream, _) = tcp_listener.accept().await.unwrap();
            serve_tcp_connection(stream, tcp_rt, tcp_cs, None).await
        });

        let ws_rt = Arc::clone(&runtime);
        let ws_cs = Arc::clone(&config_store);
        let ws_host = Arc::clone(&host);
        let ws_server = tokio::spawn(async move {
            crate::serve_realtime_ws_listener(ws_listener, ws_host, ws_rt, ws_cs).await
        });

        let ws_url = format!("ws://{ws_addr}/realtime/ws");
        let (mut ws_stream, _response) = connect_async(&ws_url)
            .await
            .expect("websocket handshake should succeed");
        ws_stream
            .send(WsMessage::Text(
                serde_json::to_string(&RealtimeClientFrame::ChannelOpen(
                    RealtimeChannelOpenFrame {
                        protocol_version: open_info.default_protocol_version.clone(),
                        open_token: open_info.open_token.clone(),
                        role: RealtimeChannelRole::Primary,
                        turning_mode: RealtimeTurningMode::ProviderManaged,
                    },
                ))
                .expect("channel.open should serialize")
                .into(),
            ))
            .await
            .expect("channel.open should send");
        let opened_frame = ws_stream
            .next()
            .await
            .expect("expected channel.opened from websocket host")
            .expect("websocket frame should arrive");
        assert!(
            matches!(opened_frame, WsMessage::Text(_)),
            "expected websocket text frame, got {opened_frame:?}"
        );
        let opened_payload = match opened_frame {
            WsMessage::Text(text) => serde_json::from_str::<RealtimeServerFrame>(&text)
                .expect("channel.opened should deserialize"),
            other => panic!("expected websocket text frame, got {other:?}"),
        };
        assert!(
            matches!(opened_payload, RealtimeServerFrame::ChannelOpened(_)),
            "expected channel.opened, got {opened_payload:?}"
        );

        let mut client = TcpStream::connect(tcp_addr).await.unwrap();
        let init_request = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "initialize",
            "params": {},
            "id": 1
        });
        send_jsonl(&mut client, &init_request).await;

        let (read_half, _write_half) = client.into_split();
        let mut reader = TokioBufReader::new(read_half);
        let response = read_jsonl_line(&mut reader)
            .await
            .expect("expected initialize response");
        assert_eq!(response["id"], 1);
        assert!(response["error"].is_null(), "expected no error: {response}");

        drop(_write_half);
        drop(reader);
        let _ = ws_stream.close(None).await;

        let tcp_result = tokio::time::timeout(std::time::Duration::from_secs(5), tcp_server)
            .await
            .expect("tcp server should finish within timeout")
            .expect("tcp server task should not panic");
        assert!(tcp_result.is_ok());

        ws_server.abort();
    }

    #[tokio::test]
    async fn realtime_ws_listener_rejects_unsupported_protocol_version() {
        let temp = tempfile::tempdir().unwrap();
        let (runtime, config_store) = build_test_runtime(&temp);

        let ws_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let ws_addr = ws_listener.local_addr().unwrap();
        let host = Arc::new(crate::realtime_ws::RealtimeWsHost::new(format!(
            "ws://{ws_addr}{}",
            crate::REALTIME_WS_PATH
        )));
        let open_info = host
            .issue_open_info(
                RealtimeOpenRequest {
                    target: RealtimeChannelTarget::SessionTarget {
                        session_id: "01234567-89ab-cdef-0123-456789abcdef".to_string(),
                    },
                    role: RealtimeChannelRole::Primary,
                    turning_mode: RealtimeTurningMode::ProviderManaged,
                    reconnect_policy: None,
                    channel_config: None,
                },
                RealtimeCapabilities {
                    input_kinds: vec![RealtimeInputKind::Text, RealtimeInputKind::Audio],
                    output_kinds: vec![RealtimeOutputKind::Text, RealtimeOutputKind::Audio],
                    turning_modes: vec![RealtimeTurningMode::ProviderManaged],
                    interrupt_supported: true,
                    transcript_supported: true,
                    tool_lifecycle_events_supported: false,
                    video_supported: false,
                    audio_input_format: None,
                    audio_output_format: None,
                },
                None,
            )
            .await;

        let ws_rt = Arc::clone(&runtime);
        let ws_cs = Arc::clone(&config_store);
        let ws_host = Arc::clone(&host);
        let ws_server = tokio::spawn(async move {
            crate::serve_realtime_ws_listener(ws_listener, ws_host, ws_rt, ws_cs).await
        });

        let ws_url = format!("ws://{ws_addr}/realtime/ws");
        let (mut ws_stream, _response) = connect_async(&ws_url)
            .await
            .expect("websocket handshake should succeed");
        ws_stream
            .send(WsMessage::Text(
                serde_json::to_string(&RealtimeClientFrame::ChannelOpen(
                    RealtimeChannelOpenFrame {
                        protocol_version: "999".to_string(),
                        open_token: open_info.open_token.clone(),
                        role: RealtimeChannelRole::Primary,
                        turning_mode: RealtimeTurningMode::ProviderManaged,
                    },
                ))
                .expect("channel.open should serialize")
                .into(),
            ))
            .await
            .expect("channel.open should send");
        let error_frame = ws_stream
            .next()
            .await
            .expect("expected channel.error from websocket host")
            .expect("websocket frame should arrive");
        let error_payload = match error_frame {
            WsMessage::Text(text) => serde_json::from_str::<RealtimeServerFrame>(&text)
                .expect("channel.error should deserialize"),
            other => panic!("expected websocket text frame, got {other:?}"),
        };
        match error_payload {
            RealtimeServerFrame::ChannelError(frame) => {
                assert_eq!(
                    frame.code,
                    meerkat_contracts::RealtimeErrorCode::UnsupportedProtocolVersion
                );
            }
            other => panic!("expected channel.error, got {other:?}"),
        }

        let _ = ws_stream.close(None).await;
        ws_server.abort();
    }

    #[tokio::test]
    async fn realtime_ws_listener_rejects_second_primary_for_same_target() {
        let temp = tempfile::tempdir().unwrap();
        let (runtime, config_store) = build_test_runtime(&temp);
        register_live_session(&runtime, "01234567-89ab-cdef-0123-456789abcdef").await;

        let ws_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let ws_addr = ws_listener.local_addr().unwrap();
        let host = Arc::new(crate::realtime_ws::RealtimeWsHost::new(format!(
            "ws://{ws_addr}{}",
            crate::REALTIME_WS_PATH
        )));
        let first_open_info = host
            .issue_open_info(
                RealtimeOpenRequest {
                    target: RealtimeChannelTarget::SessionTarget {
                        session_id: "01234567-89ab-cdef-0123-456789abcdef".to_string(),
                    },
                    role: RealtimeChannelRole::Primary,
                    turning_mode: RealtimeTurningMode::ProviderManaged,
                    reconnect_policy: None,
                    channel_config: None,
                },
                RealtimeCapabilities {
                    input_kinds: vec![RealtimeInputKind::Text, RealtimeInputKind::Audio],
                    output_kinds: vec![RealtimeOutputKind::Text, RealtimeOutputKind::Audio],
                    turning_modes: vec![RealtimeTurningMode::ProviderManaged],
                    interrupt_supported: true,
                    transcript_supported: true,
                    tool_lifecycle_events_supported: false,
                    video_supported: false,
                    audio_input_format: None,
                    audio_output_format: None,
                },
                None,
            )
            .await;
        let second_open_info = host
            .issue_open_info(
                RealtimeOpenRequest {
                    target: RealtimeChannelTarget::SessionTarget {
                        session_id: "01234567-89ab-cdef-0123-456789abcdef".to_string(),
                    },
                    role: RealtimeChannelRole::Primary,
                    turning_mode: RealtimeTurningMode::ProviderManaged,
                    reconnect_policy: None,
                    channel_config: None,
                },
                RealtimeCapabilities {
                    input_kinds: vec![RealtimeInputKind::Text, RealtimeInputKind::Audio],
                    output_kinds: vec![RealtimeOutputKind::Text, RealtimeOutputKind::Audio],
                    turning_modes: vec![RealtimeTurningMode::ProviderManaged],
                    interrupt_supported: true,
                    transcript_supported: true,
                    tool_lifecycle_events_supported: false,
                    video_supported: false,
                    audio_input_format: None,
                    audio_output_format: None,
                },
                None,
            )
            .await;

        let ws_rt = Arc::clone(&runtime);
        let ws_cs = Arc::clone(&config_store);
        let ws_host = Arc::clone(&host);
        let ws_server = tokio::spawn(async move {
            crate::serve_realtime_ws_listener(ws_listener, ws_host, ws_rt, ws_cs).await
        });

        let ws_url = format!("ws://{ws_addr}/realtime/ws");
        let (mut first_stream, _response) = connect_async(&ws_url)
            .await
            .expect("first websocket handshake should succeed");
        first_stream
            .send(WsMessage::Text(
                serde_json::to_string(&RealtimeClientFrame::ChannelOpen(
                    RealtimeChannelOpenFrame {
                        protocol_version: first_open_info.default_protocol_version.clone(),
                        open_token: first_open_info.open_token.clone(),
                        role: RealtimeChannelRole::Primary,
                        turning_mode: RealtimeTurningMode::ProviderManaged,
                    },
                ))
                .expect("channel.open should serialize")
                .into(),
            ))
            .await
            .expect("first channel.open should send");
        let first_opened = first_stream
            .next()
            .await
            .expect("expected first channel.opened")
            .expect("first channel.opened should arrive");
        assert!(
            matches!(first_opened, WsMessage::Text(_)),
            "expected first websocket text frame, got {first_opened:?}"
        );

        let (mut second_stream, _response) = connect_async(&ws_url)
            .await
            .expect("second websocket handshake should succeed");
        second_stream
            .send(WsMessage::Text(
                serde_json::to_string(&RealtimeClientFrame::ChannelOpen(
                    RealtimeChannelOpenFrame {
                        protocol_version: second_open_info.default_protocol_version.clone(),
                        open_token: second_open_info.open_token.clone(),
                        role: RealtimeChannelRole::Primary,
                        turning_mode: RealtimeTurningMode::ProviderManaged,
                    },
                ))
                .expect("channel.open should serialize")
                .into(),
            ))
            .await
            .expect("second channel.open should send");
        let second_error = second_stream
            .next()
            .await
            .expect("expected second channel.error")
            .expect("second channel.error should arrive");
        let second_payload = match second_error {
            WsMessage::Text(text) => serde_json::from_str::<RealtimeServerFrame>(&text)
                .expect("channel.error should deserialize"),
            other => panic!("expected websocket text frame, got {other:?}"),
        };
        match second_payload {
            RealtimeServerFrame::ChannelError(frame) => {
                assert_eq!(frame.code, meerkat_contracts::RealtimeErrorCode::TargetBusy);
            }
            other => panic!("expected channel.error, got {other:?}"),
        }

        let _ = first_stream.close(None).await;
        let _ = second_stream.close(None).await;
        ws_server.abort();
    }

    #[tokio::test]
    async fn realtime_ws_listener_enforces_observer_read_only_frames() {
        let temp = tempfile::tempdir().unwrap();
        let (runtime, config_store) = build_test_runtime(&temp);
        register_live_session(&runtime, "fedcba98-7654-3210-fedc-ba9876543210").await;

        let ws_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let ws_addr = ws_listener.local_addr().unwrap();
        let host = Arc::new(crate::realtime_ws::RealtimeWsHost::new(format!(
            "ws://{ws_addr}{}",
            crate::REALTIME_WS_PATH
        )));
        let open_info = host
            .issue_open_info(
                RealtimeOpenRequest {
                    target: RealtimeChannelTarget::SessionTarget {
                        session_id: "fedcba98-7654-3210-fedc-ba9876543210".to_string(),
                    },
                    role: RealtimeChannelRole::Observer,
                    turning_mode: RealtimeTurningMode::ProviderManaged,
                    reconnect_policy: None,
                    channel_config: None,
                },
                RealtimeCapabilities {
                    input_kinds: vec![RealtimeInputKind::Text, RealtimeInputKind::Audio],
                    output_kinds: vec![RealtimeOutputKind::Text, RealtimeOutputKind::Audio],
                    turning_modes: vec![RealtimeTurningMode::ProviderManaged],
                    interrupt_supported: true,
                    transcript_supported: true,
                    tool_lifecycle_events_supported: false,
                    video_supported: false,
                    audio_input_format: None,
                    audio_output_format: None,
                },
                None,
            )
            .await;

        let ws_rt = Arc::clone(&runtime);
        let ws_cs = Arc::clone(&config_store);
        let ws_host = Arc::clone(&host);
        let ws_server = tokio::spawn(async move {
            crate::serve_realtime_ws_listener(ws_listener, ws_host, ws_rt, ws_cs).await
        });

        let ws_url = format!("ws://{ws_addr}/realtime/ws");
        let (mut ws_stream, _response) = connect_async(&ws_url)
            .await
            .expect("websocket handshake should succeed");
        ws_stream
            .send(WsMessage::Text(
                serde_json::to_string(&RealtimeClientFrame::ChannelOpen(
                    RealtimeChannelOpenFrame {
                        protocol_version: open_info.default_protocol_version.clone(),
                        open_token: open_info.open_token.clone(),
                        role: RealtimeChannelRole::Observer,
                        turning_mode: RealtimeTurningMode::ProviderManaged,
                    },
                ))
                .expect("channel.open should serialize")
                .into(),
            ))
            .await
            .expect("channel.open should send");
        let _opened = ws_stream
            .next()
            .await
            .expect("expected channel.opened")
            .expect("channel.opened should arrive");

        ws_stream
            .send(WsMessage::Text(
                serde_json::to_string(&RealtimeClientFrame::ChannelInput(
                    meerkat_contracts::RealtimeChannelInputFrame {
                        chunk: meerkat_contracts::RealtimeInputChunk::TextChunk(
                            meerkat_contracts::RealtimeTextChunk {
                                text: "observer should not write".to_string(),
                            },
                        ),
                    },
                ))
                .expect("channel.input should serialize")
                .into(),
            ))
            .await
            .expect("channel.input should send");
        let error_frame = ws_stream
            .next()
            .await
            .expect("expected observer channel.error")
            .expect("observer channel.error should arrive");
        let error_payload = match error_frame {
            WsMessage::Text(text) => serde_json::from_str::<RealtimeServerFrame>(&text)
                .expect("channel.error should deserialize"),
            other => panic!("expected websocket text frame, got {other:?}"),
        };
        match error_payload {
            RealtimeServerFrame::ChannelError(frame) => {
                assert_eq!(
                    frame.code,
                    meerkat_contracts::RealtimeErrorCode::ObserverReadOnly
                );
            }
            other => panic!("expected channel.error, got {other:?}"),
        }

        let _ = ws_stream.close(None).await;
        ws_server.abort();
    }

    #[tokio::test]
    async fn realtime_ws_listener_releases_primary_slot_after_close() {
        let temp = tempfile::tempdir().unwrap();
        let (runtime, config_store) = build_test_runtime(&temp);
        register_live_session(&runtime, "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee").await;

        let ws_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let ws_addr = ws_listener.local_addr().unwrap();
        let host = Arc::new(crate::realtime_ws::RealtimeWsHost::new(format!(
            "ws://{ws_addr}{}",
            crate::REALTIME_WS_PATH
        )));
        let first_open_info = host
            .issue_open_info(
                RealtimeOpenRequest {
                    target: RealtimeChannelTarget::SessionTarget {
                        session_id: "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".to_string(),
                    },
                    role: RealtimeChannelRole::Primary,
                    turning_mode: RealtimeTurningMode::ProviderManaged,
                    reconnect_policy: None,
                    channel_config: None,
                },
                RealtimeCapabilities {
                    input_kinds: vec![RealtimeInputKind::Text, RealtimeInputKind::Audio],
                    output_kinds: vec![RealtimeOutputKind::Text, RealtimeOutputKind::Audio],
                    turning_modes: vec![RealtimeTurningMode::ProviderManaged],
                    interrupt_supported: true,
                    transcript_supported: true,
                    tool_lifecycle_events_supported: false,
                    video_supported: false,
                    audio_input_format: None,
                    audio_output_format: None,
                },
                None,
            )
            .await;
        let second_open_info = host
            .issue_open_info(
                RealtimeOpenRequest {
                    target: RealtimeChannelTarget::SessionTarget {
                        session_id: "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".to_string(),
                    },
                    role: RealtimeChannelRole::Primary,
                    turning_mode: RealtimeTurningMode::ProviderManaged,
                    reconnect_policy: None,
                    channel_config: None,
                },
                RealtimeCapabilities {
                    input_kinds: vec![RealtimeInputKind::Text, RealtimeInputKind::Audio],
                    output_kinds: vec![RealtimeOutputKind::Text, RealtimeOutputKind::Audio],
                    turning_modes: vec![RealtimeTurningMode::ProviderManaged],
                    interrupt_supported: true,
                    transcript_supported: true,
                    tool_lifecycle_events_supported: false,
                    video_supported: false,
                    audio_input_format: None,
                    audio_output_format: None,
                },
                None,
            )
            .await;

        let ws_rt = Arc::clone(&runtime);
        let ws_cs = Arc::clone(&config_store);
        let ws_host = Arc::clone(&host);
        let ws_server = tokio::spawn(async move {
            crate::serve_realtime_ws_listener(ws_listener, ws_host, ws_rt, ws_cs).await
        });

        let ws_url = format!("ws://{ws_addr}/realtime/ws");
        let (mut first_stream, _response) = connect_async(&ws_url)
            .await
            .expect("first websocket handshake should succeed");
        first_stream
            .send(WsMessage::Text(
                serde_json::to_string(&RealtimeClientFrame::ChannelOpen(
                    RealtimeChannelOpenFrame {
                        protocol_version: first_open_info.default_protocol_version.clone(),
                        open_token: first_open_info.open_token.clone(),
                        role: RealtimeChannelRole::Primary,
                        turning_mode: RealtimeTurningMode::ProviderManaged,
                    },
                ))
                .expect("channel.open should serialize")
                .into(),
            ))
            .await
            .expect("first channel.open should send");
        let _opened = first_stream
            .next()
            .await
            .expect("expected first channel.opened")
            .expect("first channel.opened should arrive");
        let _ = first_stream.close(None).await;

        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        let (mut second_stream, _response) = connect_async(&ws_url)
            .await
            .expect("second websocket handshake should succeed");
        second_stream
            .send(WsMessage::Text(
                serde_json::to_string(&RealtimeClientFrame::ChannelOpen(
                    RealtimeChannelOpenFrame {
                        protocol_version: second_open_info.default_protocol_version.clone(),
                        open_token: second_open_info.open_token.clone(),
                        role: RealtimeChannelRole::Primary,
                        turning_mode: RealtimeTurningMode::ProviderManaged,
                    },
                ))
                .expect("channel.open should serialize")
                .into(),
            ))
            .await
            .expect("second channel.open should send");
        let opened_frame = second_stream
            .next()
            .await
            .expect("expected second channel.opened")
            .expect("second channel.opened should arrive");
        let opened_payload = match opened_frame {
            WsMessage::Text(text) => serde_json::from_str::<RealtimeServerFrame>(&text)
                .expect("channel.opened should deserialize"),
            other => panic!("expected websocket text frame, got {other:?}"),
        };
        assert!(
            matches!(opened_payload, RealtimeServerFrame::ChannelOpened(_)),
            "expected channel.opened after releasing the prior primary, got {opened_payload:?}"
        );

        let _ = second_stream.close(None).await;
        ws_server.abort();
    }

    // -- Test 7: Bare disconnect (connect then immediately drop) --

    #[tokio::test]
    async fn tcp_bare_disconnect_no_data() {
        let temp = tempfile::tempdir().unwrap();
        let (runtime, config_store) = build_test_runtime(&temp);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let rt = Arc::clone(&runtime);
        let cs = Arc::clone(&config_store);
        let server_handle = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            serve_tcp_connection(stream, rt, cs, None).await
        });

        // Connect and immediately drop without sending any data.
        {
            let _client = TcpStream::connect(addr).await.unwrap();
        }

        let server_result = tokio::time::timeout(std::time::Duration::from_secs(5), server_handle)
            .await
            .expect("server should finish within timeout")
            .expect("server task should not panic");

        assert!(
            server_result.is_ok(),
            "bare disconnect (no data) should not error: {server_result:?}"
        );
    }

    // -- Test 8: Malformed JSON over TCP produces parse error, server stays alive --

    #[tokio::test]
    async fn tcp_malformed_json_returns_parse_error() {
        let temp = tempfile::tempdir().unwrap();
        let (runtime, config_store) = build_test_runtime(&temp);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let rt = Arc::clone(&runtime);
        let cs = Arc::clone(&config_store);
        let server_handle = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            serve_tcp_connection(stream, rt, cs, None).await
        });

        let client = TcpStream::connect(addr).await.unwrap();
        let (read_half, mut write_half) = client.into_split();
        let mut reader = TokioBufReader::new(read_half);

        // Send garbage.
        write_half.write_all(b"this is not json\n").await.unwrap();
        write_half.flush().await.unwrap();

        let resp = read_jsonl_line(&mut reader).await.expect("error response");
        assert!(
            resp["error"].is_object(),
            "should receive a parse error response: {resp}"
        );
        assert_eq!(resp["error"]["code"], crate::error::PARSE_ERROR);

        // Server should still be alive — send a valid request.
        let init = serde_json::json!({
            "jsonrpc": "2.0", "method": "initialize", "params": {}, "id": 99
        });
        let mut bytes = serde_json::to_vec(&init).unwrap();
        bytes.push(b'\n');
        write_half.write_all(&bytes).await.unwrap();
        write_half.flush().await.unwrap();

        let resp2 = read_jsonl_line(&mut reader)
            .await
            .expect("init response after error");
        assert_eq!(resp2["id"], 99);
        assert!(
            resp2["error"].is_null(),
            "init should succeed after parse error: {resp2}"
        );

        drop(write_half);
        drop(reader);

        let server_result = tokio::time::timeout(std::time::Duration::from_secs(5), server_handle)
            .await
            .expect("server should finish")
            .expect("server task should not panic");

        assert!(server_result.is_ok());
    }
}
