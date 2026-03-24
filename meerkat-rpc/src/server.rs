//! RPC server main loop.
//!
//! Wires together the JSON-RPC transport, method router, and notification
//! channel. Uses `tokio::select!` to multiplex incoming requests, outgoing
//! responses from spawned dispatch tasks, and event notifications.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use meerkat::surface::{RequestTerminal, SurfaceRequestExecutor, noop_request_action};
use tokio::io::{AsyncBufRead, AsyncWrite, BufReader};
use tokio::sync::{mpsc, oneshot};

use meerkat_core::ConfigStore;

use crate::NOTIFICATION_CHANNEL_CAPACITY;
use crate::handlers::RpcResponseExt;
use crate::protocol::{RpcId, RpcMessage, RpcNotification, RpcRequest, RpcResponse};
use crate::router::{MethodRouter, NotificationSink};
use crate::session_runtime::SessionRuntime;
use crate::transport::{JsonlTransport, TransportError};

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
        }
    }

    #[cfg(feature = "mob")]
    /// Create a new RPC server with an optional skill runtime and explicit mob state.
    pub fn new_with_skill_runtime_and_mob_state(
        reader: R,
        writer: W,
        runtime: Arc<SessionRuntime>,
        config_store: Arc<dyn ConfigStore>,
        skill_runtime: Option<Arc<meerkat_core::skills::SkillRuntime>>,
        mob_state: Arc<meerkat_mob_mcp::MobMcpState>,
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

        runtime.set_callback_channel(
            callback_request_tx.clone(),
            callback_id_counter.clone(),
            registered_tools.clone(),
        );

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

                Some(response) = self.long_running_rx.recv() => {
                    // Late cancel must not override committed work: if the task
                    // produced a Publish terminal, the work already ran and the
                    // client must learn the result. Only suppress uncommitted
                    // (RespondWithoutPublish) terminals when cancel was requested.
                    let terminal = match response.terminal {
                        RequestTerminal::Publish(_) => response.terminal,
                        RequestTerminal::RespondWithoutPublish(ref _resp)
                            if self.request_executor.cancel_requested(&response.request_key) =>
                        {
                            RequestTerminal::RespondWithoutPublish(
                                request_cancelled_response(request_id_from_response(&response.terminal)),
                            )
                        }
                        other => other,
                    };

                    match terminal {
                        RequestTerminal::Publish(rpc_response) => {
                            // Defer ownership publication until the response is
                            // actually written to the transport (not just enqueued).
                            match self.transport.write_response(&rpc_response).await {
                                Ok(()) => {
                                    self.request_executor.mark_published(&response.request_key);
                                    self.request_executor.remove_published(&response.request_key);
                                }
                                Err(_) => {
                                    self.request_executor.finish_unpublished(&response.request_key).await;
                                    break;
                                }
                            }
                        }
                        RequestTerminal::RespondWithoutPublish(rpc_response) => {
                            let write_result = self.transport.write_response(&rpc_response).await;
                            self.request_executor.finish_unpublished(&response.request_key).await;
                            if write_result.is_err() {
                                break;
                            }
                        }
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

        // Graceful shutdown: close all sessions.
        self.request_executor.shutdown_and_abort_stragglers().await;
        self.router.runtime().shutdown().await;
        Ok(())
    }

    fn spawn_long_running_request(
        &self,
        request: &RpcRequest,
    ) -> Option<(String, tokio::task::JoinHandle<()>)> {
        if !request_requires_long_running_executor(request) {
            return None;
        }
        let id = request.id.clone()?;
        let request_key = request_key(&id);
        let publish_on_success = request.method == "session/create";
        let context = self
            .request_executor
            .begin_request(request_key.clone(), noop_request_action());
        let router = self.router.clone();
        let long_running_tx = self.long_running_tx.clone();
        let request = request.clone();
        let request_key_for_task = request_key.clone();
        let handle = tokio::spawn(async move {
            if let Some(response) = router
                .dispatch_with_request_context(request, Some(context))
                .await
            {
                let terminal = classify_long_running_response(&response, publish_on_success);
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
                    if let Some(existing) = tools.iter_mut().find(|t| t.name == new_tool.name) {
                        *existing = new_tool;
                    } else {
                        tools.push(new_tool);
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
    publish_on_success: bool,
) -> RequestTerminal<RpcResponse> {
    if publish_on_success && response.error.is_none() {
        RequestTerminal::Publish(response.clone())
    } else {
        RequestTerminal::RespondWithoutPublish(response.clone())
    }
}

fn request_key(id: &RpcId) -> String {
    serde_json::to_string(id).unwrap_or_else(|_| format!("{id:?}"))
}

fn request_cancel_target(params: Option<&serde_json::value::RawValue>) -> Option<String> {
    let params = params?;
    let value: serde_json::Value = serde_json::from_str(params.get()).ok()?;
    let request_id = value.get("request_id")?;
    Some(serde_json::to_string(request_id).ok()?)
}

fn request_cancelled_response(id: Option<RpcId>) -> RpcResponse {
    RpcResponse::error(
        id,
        crate::error::REQUEST_CANCELLED,
        "request cancelled before response publish",
    )
}

fn request_requires_long_running_executor(request: &RpcRequest) -> bool {
    match request.method.as_str() {
        "turn/start" => true,
        "session/create" => session_create_runs_immediately(request.params.as_deref()),
        _ => false,
    }
}

fn session_create_runs_immediately(params: Option<&serde_json::value::RawValue>) -> bool {
    let Some(params) = params else {
        return true;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(params.get()) else {
        return true;
    };
    !matches!(
        value
            .get("initial_turn")
            .and_then(serde_json::Value::as_str),
        Some("deferred")
    )
}

fn request_id_from_response(terminal: &RequestTerminal<RpcResponse>) -> Option<RpcId> {
    match terminal {
        RequestTerminal::Publish(response) | RequestTerminal::RespondWithoutPublish(response) => {
            response.id.clone()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deferred_session_create_uses_simple_path() {
        let request = RpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(RpcId::Num(1)),
            method: "session/create".to_string(),
            params: Some(
                serde_json::value::to_raw_value(&serde_json::json!({
                    "prompt": "hello",
                    "initial_turn": "deferred"
                }))
                .expect("raw params"),
            ),
        };

        assert!(!request_requires_long_running_executor(&request));
    }

    #[test]
    fn immediate_session_create_uses_long_running_executor() {
        let request = RpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(RpcId::Num(1)),
            method: "session/create".to_string(),
            params: Some(
                serde_json::value::to_raw_value(&serde_json::json!({
                    "prompt": "hello"
                }))
                .expect("raw params"),
            ),
        };

        assert!(request_requires_long_running_executor(&request));
    }
}
