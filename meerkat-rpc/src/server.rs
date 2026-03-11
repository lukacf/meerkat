//! RPC server main loop.
//!
//! Wires together the JSON-RPC transport, method router, and notification
//! channel. Uses `tokio::select!` to multiplex incoming requests, outgoing
//! responses from spawned dispatch tasks, and event notifications.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use tokio::io::{AsyncBufRead, AsyncWrite, BufReader};
use tokio::sync::{mpsc, oneshot};

use meerkat_core::ConfigStore;

use crate::NOTIFICATION_CHANNEL_CAPACITY;
use crate::handlers::RpcResponseExt;
use crate::protocol::{RpcId, RpcMessage, RpcNotification, RpcRequest, RpcResponse};
use crate::router::{MethodRouter, NotificationSink};
use crate::session_runtime::SessionRuntime;
use crate::transport::{JsonlTransport, TransportError};

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
}

impl<R: AsyncBufRead + Unpin, W: AsyncWrite + Unpin> RpcServer<R, W> {
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

                            let router = self.router.clone();
                            let resp_tx = self.response_tx.clone();
                            tokio::spawn(async move {
                                if let Some(response) = router.dispatch(request).await {
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

                // Forward queued notifications to the transport.
                Some(notification) = self.notification_rx.recv() => {
                    self.transport.write_notification(&notification).await?;
                }

                // Send callback requests to the client.
                Some((request, response_tx)) = self.callback_request_rx.recv() => {
                    if let Some(ref req_id) = request.id {
                        // Sweep stale entries whose waiters timed out / dropped.
                        self.pending_callbacks.retain(|_, tx| !tx.is_closed());
                        self.pending_callbacks.insert(req_id.clone(), response_tx);
                    }
                    self.transport.write_request(&request).await?;
                }
            }
        }

        // Graceful shutdown: close all sessions.
        self.router.runtime().shutdown().await;
        Ok(())
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
                tools.extend(params.tools);
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
    let stdout = tokio::io::stdout();
    let reader = BufReader::new(stdin);
    let mut server =
        RpcServer::new_with_skill_runtime(reader, stdout, runtime, config_store, skill_runtime);
    server.run().await
}
