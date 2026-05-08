//! RPC server main loop.
//!
//! Wires together the JSON-RPC transport, method router, and notification
//! channel. Uses `tokio::select!` to multiplex incoming requests, outgoing
//! responses from spawned dispatch tasks, and event notifications.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use meerkat::surface::{
    RequestTerminalResolution, SurfaceRequestExecutor, SurfaceRequestSemantics, noop_request_action,
};
use meerkat_contracts::rpc_request_lifecycle;
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
    success: bool,
    response: RpcResponse,
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
        let context = self.request_executor.begin_request_with_semantics(
            request_key.clone(),
            noop_request_action(),
            semantics,
        );
        let router = self.router.clone();
        let long_running_tx = self.long_running_tx.clone();
        let request = request.clone();
        let request_key_for_task = request_key.clone();
        let handle = tokio::spawn(async move {
            if let Some(response) =
                Box::pin(router.dispatch_with_request_context(request, Some(context))).await
            {
                let success = response.error.is_none();
                let _ = long_running_tx
                    .send(LongRunningResponse {
                        request_key: request_key_for_task,
                        success,
                        response,
                    })
                    .await;
            }
        });
        Some((request_key, handle))
    }

    async fn write_long_running_response(&mut self, response: LongRunningResponse) -> bool {
        let cancel_id = response.response.id.clone();
        let to_write = match self
            .request_executor
            .resolve_admitted_terminal(
                Some(&response.request_key),
                response.success,
                response.response,
            )
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
    SurfaceRequestSemantics::from(rpc_request_lifecycle(
        request.method.as_str(),
        request.params.as_deref().map(|params| params.get()),
    ))
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
    let stdin = tokio::io::stdin();
    let stdout = BlockingWriter::stdout();
    let reader = BufReader::new(stdin);
    let mut server =
        RpcServer::new_with_skill_runtime(reader, stdout, runtime, config_store, skill_runtime);
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
    let (read_half, write_half) = stream.into_split();
    let reader = BufReader::new(read_half);
    let mut server =
        RpcServer::new_with_skill_runtime(reader, write_half, runtime, config_store, skill_runtime);
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
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!("RPC TCP listener bound to {addr}");
    loop {
        let (stream, peer_addr) = listener.accept().await?;
        tracing::info!("RPC client connected from {peer_addr}");
        let runtime = Arc::clone(&runtime);
        let config_store = Arc::clone(&config_store);
        let skill_runtime = skill_runtime.clone();
        tokio::spawn(async move {
            if let Err(e) = serve_tcp_connection(stream, runtime, config_store, skill_runtime).await
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

    #[tokio::test]
    async fn turn_start_is_publish_on_success() {
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
        let executor = SurfaceRequestExecutor::new(tokio::time::Duration::from_millis(1));
        let request_key = request_key(request.id.as_ref().expect("request id"));
        let context = executor.begin_request_with_semantics(
            request_key.clone(),
            noop_request_action(),
            semantics,
        );
        let response = RpcResponse::success(request.id, serde_json::json!({"ok": true}));

        assert!(matches!(
            executor
                .resolve_admitted_terminal(Some(context.key()), response.error.is_none(), response)
                .await,
            RequestTerminalResolution::Emit(_)
        ));
        assert_eq!(executor.phase(&request_key), None);
        assert_eq!(
            executor.cancel_request(&request_key).await,
            meerkat::surface::CancelOutcome::NotFound
        );
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
        let _ctx = server.request_executor.begin_request_with_semantics(
            request_key.clone(),
            noop_request_action(),
            SurfaceRequestSemantics::long_running_publish_on_success(),
        );

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
                    success: publish_response.error.is_none(),
                    response: publish_response,
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
    use meerkat::{AgentBuildConfig, AgentFactory};
    use meerkat_client::{LlmClient, LlmError};
    use meerkat_core::lifecycle::RunId;
    use meerkat_core::lifecycle::core_executor::{
        CoreApplyOutput, CoreExecutor, CoreExecutorError,
    };
    use meerkat_core::lifecycle::run_primitive::RunPrimitive;
    use meerkat_core::session::ToolCategoryOverride;
    use meerkat_core::{Config, ConfigRuntime, MemoryConfigStore, StopReason};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader as TokioBufReader};
    use tokio::net::TcpStream;

    /// Mock LLM that immediately returns "Hello from mock" and ends the turn.
    struct MockLlmClient;

    #[async_trait]
    impl LlmClient for MockLlmClient {
        fn project_replay_messages(
            &self,
            messages: &[meerkat_core::Message],
        ) -> Result<Vec<meerkat_core::Message>, meerkat_client::LlmError> {
            Ok(messages.to_vec())
        }

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
            unreachable!("tests do not drive executor apply")
        }

        async fn cancel_after_boundary(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            Ok(())
        }

        async fn stop_runtime_executor(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            Ok(())
        }
    }
}
