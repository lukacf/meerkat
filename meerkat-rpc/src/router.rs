//! Method router - dispatches JSON-RPC requests to the correct handler.

use std::collections::{HashMap, HashSet};
#[cfg(feature = "mob")]
use std::path::PathBuf;
use std::sync::Arc;

use futures::StreamExt;
use tokio::sync::Mutex;
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use uuid::Uuid;

use meerkat_core::ConfigStore;
use meerkat_core::EventEnvelope;
use meerkat_core::event::AgentEvent;
use meerkat_core::service::{
    SessionError, SessionForkAtRequest, SessionForkReplaceRequest, SessionHistoryQuery,
};
use meerkat_core::session::Session;
use meerkat_core::types::SessionId;
#[cfg(feature = "mob")]
use meerkat_core::{AgentToolDispatcher, DynamicToolComposite};
use meerkat_runtime::SessionServiceRuntimeExt as _;
use serde::Deserialize;
use serde_json::json;

use crate::error;
use crate::handlers;
use crate::handlers::RpcResponseExt;
use crate::protocol::{RpcNotification, RpcRequest, RpcResponse};
use crate::session_runtime::SessionRuntime;
use meerkat::surface::RequestContext;

fn is_transport_internal(message: &str) -> bool {
    message.starts_with("Transport error:") || message.starts_with("IO error:")
}

#[cfg(feature = "mob")]
fn mob_destroy_cleanup_error_response(
    id: Option<crate::protocol::RpcId>,
    destroy_error: meerkat_mob_mcp::MobMcpDestroyError,
) -> RpcResponse {
    match destroy_error {
        meerkat_mob_mcp::MobMcpDestroyError::Incomplete { report } => RpcResponse::error_with_data(
            id,
            error::INTERNAL_ERROR,
            meerkat_mob_mcp::MobMcpDestroyError::incomplete_message(&report),
            meerkat_mob_mcp::MobMcpDestroyError::incomplete_error_data(&report),
        ),
        meerkat_mob_mcp::MobMcpDestroyError::Mob(error) => {
            RpcResponse::error(id, error::INTERNAL_ERROR, error.to_string())
        }
    }
}

#[cfg(feature = "mob")]
fn mob_archive_session_error_response(
    id: Option<crate::protocol::RpcId>,
    session_id: &SessionId,
    archive_error: SessionError,
) -> RpcResponse {
    match archive_error {
        SessionError::NotFound { .. } => RpcResponse::error(
            id,
            error::SESSION_NOT_FOUND,
            format!("Session not found: {session_id}"),
        ),
        SessionError::FailedWithData { message, data } => {
            RpcResponse::error_with_data(id, error::INTERNAL_ERROR, message, data)
        }
        other => RpcResponse::error(id, error::INTERNAL_ERROR, other.to_string()),
    }
}

#[cfg(feature = "mob")]
fn compose_rpc_mob_external_tools(
    callback_tools: Option<Arc<dyn AgentToolDispatcher>>,
    configured_tools: Option<Arc<dyn AgentToolDispatcher>>,
) -> Option<Arc<dyn AgentToolDispatcher>> {
    match (callback_tools, configured_tools) {
        (Some(callback_tools), Some(configured_tools)) => {
            Some(Arc::new(DynamicToolComposite::new(vec![
                callback_tools,
                configured_tools,
            ])))
        }
        (Some(callback_tools), None) => Some(callback_tools),
        (None, Some(configured_tools)) => Some(configured_tools),
        (None, None) => None,
    }
}

#[cfg(feature = "mob")]
fn rpc_mob_external_tools_provider_from_parts(
    callback_tools_provider: Arc<dyn Fn() -> Option<Arc<dyn AgentToolDispatcher>> + Send + Sync>,
    configured_mcp_tools: Option<Arc<dyn AgentToolDispatcher>>,
    #[cfg(feature = "mcp")] configured_mcp_keepalive: Option<Arc<ConfiguredRpcMobMcpKeepalive>>,
) -> meerkat_mob::ExternalToolsProvider {
    Arc::new(move || {
        #[cfg(feature = "mcp")]
        let _configured_mcp_keepalive = configured_mcp_keepalive.as_ref();
        let callback_tools = callback_tools_provider();
        compose_rpc_mob_external_tools(callback_tools, configured_mcp_tools.clone())
    })
}

/// Bundle returned by [`load_configured_mcp_tools_for_rpc_mob`]: the dispatcher
/// callers wire into mob members, plus a keepalive guard that owns the loader
/// runtime thread's lifetime.
#[cfg(all(feature = "mob", feature = "mcp"))]
pub(crate) struct LoadedRpcMobMcp {
    pub(crate) dispatcher: Arc<dyn AgentToolDispatcher>,
    pub(crate) keepalive: ConfiguredRpcMobMcpKeepalive,
}

/// Owns the loader runtime thread that keeps configured MCP transports alive.
///
/// Dropping the guard signals the thread to call
/// `McpRouterAdapter::shutdown()` and exit, releasing every MCP transport and
/// child process the loader opened. The guard is held by the same closure that
/// captures the dispatcher `Arc`, so the thread exits the moment the dispatcher
/// itself becomes unreachable.
#[cfg(all(feature = "mob", feature = "mcp"))]
pub(crate) struct ConfiguredRpcMobMcpKeepalive {
    shutdown_tx: std::sync::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    join: Option<std::thread::JoinHandle<()>>,
}

#[cfg(all(feature = "mob", feature = "mcp"))]
impl ConfiguredRpcMobMcpKeepalive {
    fn signal_shutdown(&self) {
        if let Ok(mut guard) = self.shutdown_tx.lock()
            && let Some(tx) = guard.take()
        {
            let _ = tx.send(());
        }
    }

    /// Block until the keepalive runtime thread has exited and the adapter has
    /// finished its graceful shutdown. Used by tests; production callers rely
    /// on `Drop` to fire the shutdown signal without blocking.
    #[cfg(test)]
    fn shutdown_and_join(mut self) {
        self.signal_shutdown();
        if let Some(handle) = self.join.take() {
            let _ = handle.join();
        }
    }
}

#[cfg(all(feature = "mob", feature = "mcp"))]
impl Drop for ConfiguredRpcMobMcpKeepalive {
    fn drop(&mut self) {
        self.signal_shutdown();
        // Detach the join handle: callers on hot drop paths must not block
        // waiting for the loader runtime to finish `adapter.shutdown()`. The
        // signal above guarantees the thread will exit on its own.
    }
}

/// Load `.rkat/mcp.toml` servers, connect them, and hand back a dispatcher
/// plus a keepalive guard.
///
/// `MethodRouter::new` is synchronous (it cannot await), so the loader runs on
/// dedicated OS threads:
///
/// 1. **Config-load thread:** builds a one-shot `tokio` current-thread runtime
///    purely to drive `McpConfig::load_with_scopes_from_roots`, then exits.
///    Its runtime is torn down before we touch any MCP transports, so it
///    cannot host long-lived I/O.
/// 2. **Connect / keepalive thread (`rpc-mob-mcp-runtime`):** owns a second
///    current-thread runtime that runs `apply_staged` + `poll_external_updates`
///    until every server has connected (or the 60s deadline trips). On success
///    it parks on a `tokio::sync::oneshot` shutdown channel so the MCP child
///    processes stay alive for the lifetime of the [`LoadedRpcMobMcp`] holder.
///    Dropping the holder fires the channel; the thread then calls
///    `McpRouterAdapter::shutdown()` and exits.
///
/// The two runtimes are intentionally separate: collapsing them into one would
/// either tie the lifetime of the transports to a startup-only runtime (and
/// kill them as soon as we exit), or force the config-load step to share a
/// long-lived runtime even when no servers are configured.
///
/// Returns `None` when no servers are configured or the config could not be
/// loaded; pre-existing log lines warn on every recoverable failure.
#[cfg(all(feature = "mob", feature = "mcp"))]
fn load_configured_mcp_tools_for_rpc_mob(
    context_root: Option<PathBuf>,
    user_root: Option<PathBuf>,
) -> Option<LoadedRpcMobMcp> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let result = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| format!("build MCP loader runtime: {error}"))
            .and_then(|runtime| {
                runtime.block_on(async move {
                    meerkat_core::mcp_config::McpConfig::load_with_scopes_from_roots(
                        context_root.as_deref(),
                        user_root.as_deref(),
                    )
                    .await
                    .map_err(|error| format!("load MCP config: {error}"))
                })
            });
        let _ = tx.send(result);
    });
    let servers = match rx.recv() {
        Ok(Ok(servers)) => servers,
        Ok(Err(error)) => {
            tracing::warn!(error = %error, "failed to load configured MCP tools for RPC mob members");
            return None;
        }
        Err(error) => {
            tracing::warn!(error = %error, "MCP tool loader thread exited without a result");
            return None;
        }
    };
    if servers.is_empty() {
        return None;
    }

    let mut router = meerkat::McpRouter::new();
    for scoped in &servers {
        if let Err(error) = router.stage_add(scoped.server.clone()) {
            tracing::warn!(error = %error, "failed to stage configured MCP server for RPC mob members");
        }
    }
    let adapter = Arc::new(meerkat::McpRouterAdapter::new(router));
    let adapter_for_poll = adapter.clone();
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let thread_ready_tx = ready_tx.clone();
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let join = match std::thread::Builder::new()
        .name("rpc-mob-mcp-runtime".to_string())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    let _ =
                        thread_ready_tx.send(Err(format!("build MCP connect runtime: {error}")));
                    return;
                }
            };
            runtime.block_on(async move {
                let result = async {
                    let apply = adapter_for_poll
                        .apply_staged()
                        .await
                        .map_err(|error| format!("apply staged MCP servers: {error}"))?;
                    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
                    loop {
                        let update = adapter_for_poll.poll_external_updates().await;
                        if apply.pending_count == 0 || update.pending.is_empty() {
                            adapter_for_poll
                                .refresh_tools()
                                .await
                                .map_err(|error| format!("refresh MCP tools: {error}"))?;
                            return Ok(());
                        }
                        if std::time::Instant::now() >= deadline {
                            return Err(format!(
                                "timed out waiting for {} MCP server(s)",
                                update.pending.len()
                            ));
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                    }
                }
                .await;
                let connected = result.is_ok();
                let _ = thread_ready_tx.send(result);
                if connected {
                    // Park on the oneshot shutdown channel so configured MCP
                    // transports stay open for the lifetime of the holder.
                    // `await` resolves either when the holder explicitly sends
                    // (Drop / shutdown_and_join) or when the sender is dropped
                    // without sending; both paths cue us to wind down.
                    let _ = shutdown_rx.await;
                }
                adapter_for_poll.shutdown().await;
            });
        }) {
        Ok(handle) => Some(handle),
        Err(error) => {
            let _ = ready_tx.send(Err(format!("spawn MCP connect runtime thread: {error}")));
            None
        }
    };
    match ready_rx.recv() {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            tracing::warn!(error = %error, "configured MCP tools for RPC mob members did not fully connect before serving");
        }
        Err(error) => {
            tracing::warn!(error = %error, "MCP connect thread exited without a result");
        }
    }
    let keepalive = ConfiguredRpcMobMcpKeepalive {
        shutdown_tx: std::sync::Mutex::new(Some(shutdown_tx)),
        join,
    };
    Some(LoadedRpcMobMcp {
        dispatcher: adapter as Arc<dyn AgentToolDispatcher>,
        keepalive,
    })
}

#[cfg(feature = "comms")]
fn send_receipt_json(receipt: meerkat_core::comms::SendReceipt) -> serde_json::Value {
    serde_json::to_value(meerkat_contracts::CommsSendResult::from(receipt)).unwrap_or_else(
        |error| {
            tracing::error!(?error, "failed to serialize CommsSendResult");
            serde_json::Value::Object(serde_json::Map::new())
        },
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SessionOwner {
    Runtime,
    #[cfg(feature = "mob")]
    Mob,
}

#[derive(Debug, Deserialize)]
struct BlobGetParams {
    blob_id: String,
}

// ---------------------------------------------------------------------------
// NotificationSink
// ---------------------------------------------------------------------------

/// Channel-based sink for sending notifications (agent events) back to the
/// client transport layer.
#[derive(Clone)]
pub struct NotificationSink {
    tx: mpsc::Sender<RpcNotification>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StreamEmitStatus {
    Delivered,
    Overflow,
    ReceiverGone,
}

impl NotificationSink {
    /// Create a new notification sink backed by the given channel sender.
    pub fn new(tx: mpsc::Sender<RpcNotification>) -> Self {
        Self { tx }
    }

    /// Create a no-op sink that discards all notifications.
    /// Used in test/CLI contexts where no RPC transport exists.
    pub fn noop() -> Self {
        let (tx, _rx) = mpsc::channel(1);
        Self { tx }
    }

    /// Emit an agent event as a JSON-RPC notification.
    pub async fn emit_event(&self, session_id: &SessionId, event: &EventEnvelope<AgentEvent>) {
        let params = serde_json::json!({
            "session_id": session_id.to_string(),
            "event": event,
        });
        let notification = RpcNotification::new("session/event", params);
        // Best-effort: drop if the channel is full or the receiver is gone.
        // Must not block — the runtime executor's event forwarder calls this,
        // and blocking here backpressures through the session task into the
        // agent run, causing deadlocks with bounded notification channels.
        let _ = self.tx.try_send(notification);
    }

    /// Emit a standalone session stream event notification.
    ///
    /// When `scope_id` and `scope_path` are provided, they are included as
    /// additional fields on the notification params alongside the event. This
    /// allows SDKs to distinguish delegated-branch and mob-member scoped events from
    /// direct session events.
    async fn emit_session_stream_event(
        &self,
        stream_id: &Uuid,
        sequence: u64,
        session_id: &SessionId,
        event: &EventEnvelope<AgentEvent>,
    ) -> StreamEmitStatus {
        let params = serde_json::json!({
            "stream_id": stream_id.to_string(),
            "sequence": sequence,
            "session_id": session_id.to_string(),
            "event": event,
        });
        let notification = RpcNotification::new("session/stream_event", params);
        match self.tx.try_send(notification) {
            Ok(()) => StreamEmitStatus::Delivered,
            Err(TrySendError::Full(_)) => StreamEmitStatus::Overflow,
            Err(TrySendError::Closed(_)) => StreamEmitStatus::ReceiverGone,
        }
    }

    /// Emit a scoped session stream event notification with scope metadata.
    pub async fn emit_scoped_session_stream_event(
        &self,
        stream_id: &Uuid,
        sequence: u64,
        session_id: &SessionId,
        event: &EventEnvelope<AgentEvent>,
        scope_id: &str,
        scope_path: &[meerkat_core::event::StreamScopeFrame],
    ) {
        let params = serde_json::json!({
            "stream_id": stream_id.to_string(),
            "sequence": sequence,
            "session_id": session_id.to_string(),
            "event": event,
            "scope_id": scope_id,
            "scope_path": scope_path,
        });
        let notification = RpcNotification::new("session/stream_event", params);
        let _ = self.tx.send(notification).await;
    }

    /// Emit an explicit terminal notification for a standalone session stream.
    pub async fn emit_session_stream_end(
        &self,
        stream_id: &Uuid,
        session_id: &SessionId,
        outcome: &str,
        detail: Option<&str>,
    ) {
        let mut params = serde_json::json!({
            "stream_id": stream_id.to_string(),
            "session_id": session_id.to_string(),
            "ended": true,
            "outcome": outcome,
        });
        if let Some(detail) = detail {
            params["error"] = serde_json::json!({
                "code": "stream_queue_overflow",
                "message": detail,
            });
        }
        let notification = RpcNotification::new("session/stream_end", params);
        let _ = self.tx.send(notification).await;
    }

    #[cfg(feature = "mob")]
    /// Emit a mob stream event as a JSON-RPC notification.
    ///
    /// For mob-wide streams the event is an [`AttributedEvent`] (source + profile + envelope).
    /// For per-member streams the event is the raw [`EventEnvelope<AgentEvent>`].
    async fn emit_mob_stream_event(
        &self,
        stream_id: &Uuid,
        sequence: u64,
        event: &serde_json::Value,
    ) -> StreamEmitStatus {
        let params = serde_json::json!({
            "stream_id": stream_id.to_string(),
            "sequence": sequence,
            "event": event,
        });
        let notification = RpcNotification::new("mob/stream_event", params);
        match self.tx.try_send(notification) {
            Ok(()) => StreamEmitStatus::Delivered,
            Err(TrySendError::Full(_)) => StreamEmitStatus::Overflow,
            Err(TrySendError::Closed(_)) => StreamEmitStatus::ReceiverGone,
        }
    }

    #[cfg(feature = "mob")]
    async fn emit_mob_stream_end(&self, stream_id: &Uuid, outcome: &str, detail: Option<&str>) {
        let mut params = serde_json::json!({
            "stream_id": stream_id.to_string(),
            "ended": true,
            "outcome": outcome,
        });
        if let Some(detail) = detail {
            params["error"] = serde_json::json!({
                "code": "stream_queue_overflow",
                "message": detail,
            });
        }
        let notification = RpcNotification::new("mob/stream_end", params);
        let _ = self.tx.send(notification).await;
    }
}

struct StreamForwarder {
    terminal: StreamTerminal,
    state: StreamForwarderState,
}

#[derive(Clone)]
enum StreamTerminal {
    Session(SessionId),
    #[cfg(feature = "mob")]
    Mob,
}

enum StreamForwarderState {
    Active {
        stop_tx: Option<oneshot::Sender<()>>,
        task: JoinHandle<()>,
    },
}

/// Bounded set of recently-closed stream IDs for idempotent close detection.
///
/// Prevents unbounded memory growth on long-lived RPC servers: when the set
/// exceeds `MAX_ENTRIES`, the oldest entries are evicted. This means very old
/// stream IDs may lose `already_closed` tracking, which is acceptable — the
/// alternative is unbounded growth.
struct ClosedStreamSet {
    entries: std::collections::VecDeque<Uuid>,
    set: HashSet<Uuid>,
}

const CLOSED_STREAM_SET_MAX: usize = 1024;

impl ClosedStreamSet {
    fn new() -> Self {
        Self {
            entries: std::collections::VecDeque::new(),
            set: HashSet::new(),
        }
    }

    /// Insert a stream ID. Returns false if already present.
    fn insert(&mut self, id: Uuid) -> bool {
        if !self.set.insert(id) {
            return false;
        }
        self.entries.push_back(id);
        while self.entries.len() > CLOSED_STREAM_SET_MAX {
            if let Some(evicted) = self.entries.pop_front() {
                self.set.remove(&evicted);
            }
        }
        true
    }

    /// Check and remove a stream ID. Returns true if it was present.
    fn remove(&mut self, id: &Uuid) -> bool {
        if self.set.remove(id) {
            self.entries.retain(|e| e != id);
            true
        } else {
            false
        }
    }
}

// ---------------------------------------------------------------------------
// MethodRouter
// ---------------------------------------------------------------------------

/// Dispatches incoming JSON-RPC requests to the appropriate handler.
#[derive(Clone)]
pub struct MethodRouter {
    runtime: Arc<SessionRuntime>,
    config_store: Arc<dyn ConfigStore>,
    notification_sink: NotificationSink,
    skill_runtime: Option<Arc<meerkat_core::skills::SkillRuntime>>,
    active_session_streams: Arc<Mutex<HashMap<Uuid, StreamForwarder>>>,
    /// Recently-closed stream IDs for idempotent close detection.
    /// Bounded to prevent unbounded growth on long-lived servers.
    closed_session_streams: Arc<Mutex<ClosedStreamSet>>,
    #[cfg(feature = "mob")]
    mob_state: Arc<meerkat_mob_mcp::MobMcpState>,
    #[cfg(feature = "mob")]
    active_mob_streams: Arc<Mutex<HashMap<Uuid, StreamForwarder>>>,
    #[cfg(feature = "mob")]
    closed_mob_streams: Arc<Mutex<ClosedStreamSet>>,
    runtime_adapter: Arc<meerkat_runtime::MeerkatMachine>,
    live_adapter_host: Arc<meerkat_live::LiveAdapterHost>,
    live_ws_state: Option<Arc<meerkat_live::LiveWsState>>,
    live_ws_base_url: Option<String>,
    live_session_factory: Option<Arc<dyn meerkat_client::realtime_session::RealtimeSessionFactory>>,
}

impl MethodRouter {
    /// Create a new method router.
    ///
    /// Reuses existing mob state from the runtime if available, otherwise
    /// creates a default. Also spawns schedule host startup.
    pub fn new(
        runtime: Arc<SessionRuntime>,
        config_store: Arc<dyn ConfigStore>,
        notification_sink: NotificationSink,
    ) -> Self {
        let runtime_adapter = runtime.runtime_adapter();
        // Reuse the runtime's existing mob state if one was pre-configured
        // (e.g., by a kennel that created a hive mob before serving TCP
        // connections). Only create a fresh MobMcpState when no existing
        // state is present.
        #[cfg(feature = "mob")]
        let mob_state = if let Some(existing) = runtime.mob_state() {
            existing
        } else {
            #[cfg(feature = "mcp")]
            let (configured_mcp_tools, configured_mcp_keepalive): (
                Option<Arc<dyn AgentToolDispatcher>>,
                Option<Arc<ConfiguredRpcMobMcpKeepalive>>,
            ) = {
                let (context_root, user_root) = runtime.skill_identity_roots();
                match load_configured_mcp_tools_for_rpc_mob(context_root, user_root) {
                    Some(LoadedRpcMobMcp {
                        dispatcher,
                        keepalive,
                    }) => (Some(dispatcher), Some(Arc::new(keepalive))),
                    None => (None, None),
                }
            };
            #[cfg(not(feature = "mcp"))]
            let configured_mcp_tools: Option<Arc<dyn AgentToolDispatcher>> = None;
            let persistent_mob_root = config_store
                .metadata()
                .and_then(|metadata| metadata.resolved_paths)
                .map(|paths| PathBuf::from(paths.root));
            let mob_state = Arc::new({
                let llm_provider: Arc<
                    dyn Fn() -> Option<Arc<dyn meerkat_client::LlmClient>> + Send + Sync,
                > = Arc::new({
                    let runtime = runtime.clone();
                    move || runtime.default_llm_client()
                });
                let callback_tools_provider: Arc<
                    dyn Fn() -> Option<Arc<dyn AgentToolDispatcher>> + Send + Sync,
                > = Arc::new({
                    let runtime = runtime.clone();
                    move || {
                        runtime.callback_request_tx().map(|tx| {
                            Arc::new(crate::callback_dispatcher::CallbackToolDispatcher::new(
                                runtime.registered_tools(),
                                tx,
                                runtime.callback_id_counter(),
                                vec![],
                            )) as Arc<dyn AgentToolDispatcher>
                        })
                    }
                });
                let tools_provider = rpc_mob_external_tools_provider_from_parts(
                    callback_tools_provider,
                    configured_mcp_tools.clone(),
                    #[cfg(feature = "mcp")]
                    configured_mcp_keepalive,
                );
                meerkat_mob_mcp::MobMcpState::new_with_runtime_adapter(
                    runtime.session_service(),
                    Some(runtime_adapter.clone()),
                )
                .with_persistent_storage_root(persistent_mob_root)
                .with_default_llm_client_provider(Some(llm_provider))
                .with_external_tools_provider(Some(tools_provider))
            });
            runtime.set_mob_state(mob_state.clone());
            mob_state
        };
        let schedule_runtime = runtime.clone();
        tokio::spawn(async move {
            if let Err(error) = schedule_runtime.ensure_schedule_host_started().await {
                tracing::warn!("failed to start RPC schedule host: {error}");
            }
        });
        // Ensure the runtime's notification sink is up-to-date so that
        // executors created lazily read the current sink at apply time.
        runtime.set_notification_sink(notification_sink.clone());
        Self {
            runtime,
            config_store,
            notification_sink,
            skill_runtime: None,
            active_session_streams: Arc::new(Mutex::new(HashMap::new())),
            closed_session_streams: Arc::new(Mutex::new(ClosedStreamSet::new())),
            #[cfg(feature = "mob")]
            mob_state,
            #[cfg(feature = "mob")]
            active_mob_streams: Arc::new(Mutex::new(HashMap::new())),
            #[cfg(feature = "mob")]
            closed_mob_streams: Arc::new(Mutex::new(ClosedStreamSet::new())),
            runtime_adapter,
            // R5-1 (P2 dogma): the live host now requires a projection sink at
            // construction. This default RpcRouter wiring path predates any
            // attached `LiveWsState` (which carries the canonical
            // `SessionServiceProjectionSink`); a `with_live_ws` call replaces
            // this placeholder host with the real one. Until then, no
            // observations can be applied (the host has no adapter / channel),
            // so installing `NoOpProjectionSink` is safe — it cannot mask a
            // semantic owner because there is no traffic to project.
            live_adapter_host: Arc::new(meerkat_live::LiveAdapterHost::new(Arc::new(
                meerkat_live::NoOpProjectionSink,
            ))),
            live_ws_state: None,
            live_ws_base_url: None,
            live_session_factory: None,
        }
    }

    /// Attach a live WebSocket transport state for `live/open` token minting.
    ///
    /// Also closes A4/A5: the runtime's callback-backed tool dispatcher is
    /// built and installed on the live host via `set_tool_dispatcher`. By the
    /// time this runs (after `RpcServer::new_*` has called
    /// `runtime.set_callback_channel`), the dispatcher is ready; if it isn't,
    /// the host stays at `ToolCallSkipped { NoDispatcher }` (audited skip).
    pub fn with_live_ws(mut self, state: Arc<meerkat_live::LiveWsState>, base_url: String) -> Self {
        self.live_adapter_host = Arc::clone(state.host());
        if let Some(dispatcher) = self.runtime.live_tool_dispatcher() {
            self.live_adapter_host.set_tool_dispatcher(dispatcher);
        }
        // P1#5: hand the host to the runtime so `propagate_config_to_live_channels`
        // can fan out `Refresh`/`Close` commands when an upstream config patch
        // changes a session's resolved model/provider.
        self.runtime
            .set_live_adapter_host(Arc::clone(&self.live_adapter_host));
        self.live_ws_state = Some(state);
        self.live_ws_base_url = Some(base_url);
        self
    }

    /// Attach a live session factory for creating provider adapters on `live/open`.
    pub fn with_live_session_factory(
        mut self,
        factory: Arc<dyn meerkat_client::realtime_session::RealtimeSessionFactory>,
    ) -> Self {
        self.live_session_factory = Some(factory);
        self
    }

    /// Get a reference to the runtime adapter for session registration.
    pub fn runtime_adapter(&self) -> &Arc<meerkat_runtime::MeerkatMachine> {
        &self.runtime_adapter
    }

    /// Get a reference to the mob state for authoritative inspection (testing).
    #[cfg(feature = "mob")]
    pub fn mob_state(&self) -> &Arc<meerkat_mob_mcp::MobMcpState> {
        &self.mob_state
    }

    #[allow(clippy::result_large_err)]
    fn session_id_from_runtime_params(
        &self,
        id: Option<crate::protocol::RpcId>,
        params: Option<&serde_json::value::RawValue>,
    ) -> Result<SessionId, RpcResponse> {
        let Some(params) = params else {
            return Err(RpcResponse::error(
                id,
                error::INVALID_PARAMS,
                "missing params",
            ));
        };
        let value: serde_json::Value = match serde_json::from_str(params.get()) {
            Ok(value) => value,
            Err(err) => {
                return Err(RpcResponse::error(
                    id,
                    error::INVALID_PARAMS,
                    format!("invalid params: {err}"),
                ));
            }
        };
        let Some(session_id) = value.get("session_id").and_then(|value| value.as_str()) else {
            return Err(RpcResponse::error(
                id,
                error::INVALID_PARAMS,
                "missing session_id",
            ));
        };
        SessionId::parse(session_id)
            .map_err(|err| RpcResponse::error(id, error::INVALID_PARAMS, err.to_string()))
    }

    async fn ensure_runtime_session_registered(
        &self,
        session_id: &SessionId,
    ) -> Result<(), RpcResponse> {
        let archived = self
            .runtime
            .authoritative_session_archived(session_id)
            .await
            .map_err(|error| RpcResponse {
                jsonrpc: "2.0".to_string(),
                id: None,
                result: None,
                error: Some(error),
            })?;
        if archived && !self.runtime.pending_session_exists(session_id).await {
            self.runtime_adapter.unregister_session(session_id).await;
            return Err(RpcResponse::error(
                None,
                error::SESSION_NOT_FOUND,
                format!("Session not found: {session_id}"),
            ));
        }

        let owner = self.resolve_session_owner(session_id).await;

        if owner.is_none() {
            return Err(RpcResponse::error(
                None,
                error::SESSION_NOT_FOUND,
                format!("Session not found: {session_id}"),
            ));
        }

        let executor: Box<dyn meerkat_core::lifecycle::CoreExecutor> = match owner {
            Some(SessionOwner::Runtime) => {
                Box::new(crate::session_executor::SessionRuntimeExecutor::new(
                    self.runtime.clone(),
                    session_id.clone(),
                ))
            }
            #[cfg(feature = "mob")]
            Some(SessionOwner::Mob) => {
                Box::new(crate::session_executor::MobRpcRuntimeExecutor::new(
                    self.mob_state.session_service(),
                    Some(self.runtime.clone()),
                    session_id.clone(),
                    self.notification_sink.clone(),
                ))
            }
            None => return Ok(()),
        };
        self.runtime_adapter
            .ensure_session_with_executor(session_id.clone(), executor)
            .await;
        Ok(())
    }

    async fn handle_blob_get(
        &self,
        id: Option<crate::protocol::RpcId>,
        params: Option<&serde_json::value::RawValue>,
    ) -> RpcResponse {
        let params: BlobGetParams = match handlers::parse_params(params) {
            Ok(params) => params,
            Err(response) => return response.with_id(id),
        };
        let blob_id = meerkat_core::BlobId::new(params.blob_id);
        match self.runtime.blob_store().get(&blob_id).await {
            Ok(payload) => RpcResponse::success(id, payload),
            Err(meerkat_core::BlobStoreError::NotFound(missing)) => RpcResponse::error(
                id,
                error::INVALID_PARAMS,
                format!("blob not found: {missing}"),
            ),
            Err(err) => RpcResponse::error(id, error::INTERNAL_ERROR, err.to_string()),
        }
    }

    async fn handle_artifact_list(
        &self,
        id: Option<crate::protocol::RpcId>,
        params: Option<&serde_json::value::RawValue>,
    ) -> RpcResponse {
        let params: meerkat_contracts::ArtifactListParams = match params {
            Some(raw) => match serde_json::from_str(raw.get()) {
                Ok(params) => params,
                Err(err) => {
                    return RpcResponse::error(
                        id,
                        error::INVALID_PARAMS,
                        format!("Invalid params: {err}"),
                    );
                }
            },
            None => meerkat_contracts::ArtifactListParams::default(),
        };
        match self
            .runtime
            .artifact_store()
            .list(params.into_filter())
            .await
        {
            Ok(artifacts) => {
                RpcResponse::success(id, meerkat_contracts::ArtifactListResult { artifacts })
            }
            Err(err) => RpcResponse::error(id, error::INTERNAL_ERROR, err.to_string()),
        }
    }

    async fn handle_artifact_get(
        &self,
        id: Option<crate::protocol::RpcId>,
        params: Option<&serde_json::value::RawValue>,
    ) -> RpcResponse {
        let params: meerkat_contracts::ArtifactIdParams = match handlers::parse_params(params) {
            Ok(params) => params,
            Err(response) => return response.with_id(id),
        };
        match self.runtime.artifact_store().get(&params.artifact_id).await {
            Ok(record) => RpcResponse::success(id, record),
            Err(meerkat_core::ArtifactError::NotFound(missing)) => RpcResponse::error(
                id,
                error::INVALID_PARAMS,
                format!("artifact not found: {missing}"),
            ),
            Err(err) => RpcResponse::error(id, error::INTERNAL_ERROR, err.to_string()),
        }
    }

    async fn handle_artifact_download(
        &self,
        id: Option<crate::protocol::RpcId>,
        params: Option<&serde_json::value::RawValue>,
    ) -> RpcResponse {
        let params: meerkat_contracts::ArtifactDownloadParams = match handlers::parse_params(params)
        {
            Ok(params) => params,
            Err(response) => return response.with_id(id),
        };
        let record = match self.runtime.artifact_store().get(&params.artifact_id).await {
            Ok(record) => record,
            Err(meerkat_core::ArtifactError::NotFound(missing)) => {
                return RpcResponse::error(
                    id,
                    error::INVALID_PARAMS,
                    format!("artifact not found: {missing}"),
                );
            }
            Err(err) => return RpcResponse::error(id, error::INTERNAL_ERROR, err.to_string()),
        };
        if let Some(expected) = params.expected_media_type.as_ref()
            && expected != &record.media_type
        {
            return RpcResponse::error(
                id,
                error::INVALID_PARAMS,
                format!(
                    "artifact media type mismatch: expected {expected}, found {}",
                    record.media_type
                ),
            );
        }
        let blob_ref = match &record.content_handle {
            meerkat_core::ArtifactContentHandle::Blob(blob_ref) => blob_ref,
            other => {
                return RpcResponse::error(
                    id,
                    error::INVALID_PARAMS,
                    meerkat_core::ArtifactError::UnsupportedContentHandle(other.opaque_id())
                        .to_string(),
                );
            }
        };
        let blob_payload = match self.runtime.blob_store().get(&blob_ref.blob_id).await {
            Ok(payload) => payload,
            Err(meerkat_core::BlobStoreError::NotFound(_)) => {
                return RpcResponse::error(
                    id,
                    error::INVALID_PARAMS,
                    format!("artifact payload not found: {}", record.artifact_id),
                );
            }
            Err(err) => return RpcResponse::error(id, error::INTERNAL_ERROR, err.to_string()),
        };
        match meerkat_core::ArtifactPayload::from_record_and_blob(&record, blob_payload) {
            Ok(payload) => RpcResponse::success(
                id,
                meerkat_contracts::ArtifactDownloadResult { record, payload },
            ),
            Err(err) => RpcResponse::error(id, error::INVALID_PARAMS, err.to_string()),
        }
    }

    /// Create a new method router with an explicit mob state.
    ///
    /// The mob state is registered on the runtime. Also spawns schedule host
    /// startup and updates the notification sink.
    #[cfg(feature = "mob")]
    pub fn new_with_mob_state(
        runtime: Arc<SessionRuntime>,
        config_store: Arc<dyn ConfigStore>,
        notification_sink: NotificationSink,
        mob_state: Arc<meerkat_mob_mcp::MobMcpState>,
    ) -> Self {
        let runtime_adapter = runtime.runtime_adapter();
        runtime.set_mob_state(mob_state.clone());
        let schedule_runtime = runtime.clone();
        tokio::spawn(async move {
            if let Err(error) = schedule_runtime.ensure_schedule_host_started().await {
                tracing::warn!("failed to start RPC schedule host: {error}");
            }
        });
        // Ensure the runtime's notification sink is up-to-date so that
        // executors created lazily read the current sink at apply time.
        runtime.set_notification_sink(notification_sink.clone());
        Self {
            runtime,
            config_store,
            notification_sink,
            skill_runtime: None,
            active_session_streams: Arc::new(Mutex::new(HashMap::new())),
            closed_session_streams: Arc::new(Mutex::new(ClosedStreamSet::new())),
            mob_state,
            active_mob_streams: Arc::new(Mutex::new(HashMap::new())),
            closed_mob_streams: Arc::new(Mutex::new(ClosedStreamSet::new())),
            runtime_adapter,
            // R5-1 (P2 dogma): the live host now requires a projection sink at
            // construction. This default RpcRouter wiring path predates any
            // attached `LiveWsState` (which carries the canonical
            // `SessionServiceProjectionSink`); a `with_live_ws` call replaces
            // this placeholder host with the real one. Until then, no
            // observations can be applied (the host has no adapter / channel),
            // so installing `NoOpProjectionSink` is safe — it cannot mask a
            // semantic owner because there is no traffic to project.
            live_adapter_host: Arc::new(meerkat_live::LiveAdapterHost::new(Arc::new(
                meerkat_live::NoOpProjectionSink,
            ))),
            live_ws_state: None,
            live_ws_base_url: None,
            live_session_factory: None,
        }
    }

    /// Replace the default ephemeral runtime adapter with a custom one
    /// (e.g., persistent-backed for durable runtime semantics).
    pub fn with_runtime_adapter(mut self, adapter: Arc<meerkat_runtime::MeerkatMachine>) -> Self {
        self.runtime_adapter = adapter;
        self
    }

    /// Set the skill runtime for introspection methods.
    pub fn with_skill_runtime(
        mut self,
        runtime: Option<Arc<meerkat_core::skills::SkillRuntime>>,
    ) -> Self {
        self.skill_runtime = runtime;
        self
    }

    // This intentionally does only the minimum owner probe. Handlers perform
    // the authoritative operation-specific read again so they observe the
    // freshest lifecycle state instead of routing off a cached snapshot.
    async fn resolve_session_owner(&self, session_id: &SessionId) -> Option<SessionOwner> {
        #[cfg(feature = "mob")]
        if self.mob_state.owns_live_bridge_session(session_id).await
            || self
                .mob_state
                .owns_service_reported_bridge_session(session_id)
                .await
            || self
                .mob_state
                .owns_persisted_bridge_session(session_id)
                .await
        {
            return Some(SessionOwner::Mob);
        }

        if self.runtime.pending_session_exists(session_id).await
            || self.runtime.session_state(session_id).await.is_some()
            || self.runtime.read_session(session_id).await.is_ok()
            || self
                .runtime
                .load_persisted_session(session_id)
                .await
                .ok()
                .flatten()
                .is_some()
        {
            return Some(SessionOwner::Runtime);
        }

        None
    }

    #[cfg(feature = "mob")]
    async fn try_read_mob_session_history(
        &self,
        id: Option<crate::protocol::RpcId>,
        session_id: &SessionId,
        query: SessionHistoryQuery,
    ) -> Option<RpcResponse> {
        match self
            .mob_state
            .session_service()
            .read_history(session_id, query)
            .await
        {
            Ok(page) => {
                let mut history: meerkat_contracts::WireSessionHistory = page.into();
                history.session_ref = self
                    .runtime
                    .realm_id()
                    .map(|realm| meerkat_contracts::format_session_ref(&realm, session_id));
                Some(RpcResponse::success(id, history))
            }
            Err(meerkat_core::service::SessionError::NotFound { .. }) => None,
            Err(err) => Some(RpcResponse::error(
                id,
                error::SESSION_NOT_FOUND,
                err.to_string(),
            )),
        }
    }

    /// Dispatch a request to the appropriate handler.
    ///
    /// Returns `None` for notifications (requests without an id) that do not
    /// require a response.
    #[allow(clippy::if_not_else)]
    pub async fn dispatch(&self, request: RpcRequest) -> Option<RpcResponse> {
        Box::pin(self.dispatch_with_request_context(request, None)).await
    }

    /// Dispatch a request with optional host-level request context for
    /// long-running cancel/publish coordination.
    #[allow(clippy::if_not_else)]
    pub async fn dispatch_with_request_context(
        &self,
        request: RpcRequest,
        request_context: Option<RequestContext>,
    ) -> Option<RpcResponse> {
        // Notifications (no id) are fire-and-forget
        if request.is_notification() {
            // Handle known notification methods silently
            match request.method.as_str() {
                "initialized" => { /* no-op ack */ }
                _ => {
                    tracing::debug!("Unknown notification method: {}", request.method);
                }
            }
            return None;
        }

        let id = request.id.clone();
        let params = request.params.as_deref();

        let response = match request.method.as_str() {
            "initialize" => handlers::initialize::handle_initialize(
                id,
                self.runtime_adapter.runtime_mode() == meerkat_runtime::RuntimeMode::V9Compliant,
            ),
            "help/ask" => {
                Box::pin(handlers::help::handle_ask(
                    id,
                    params,
                    self.runtime.clone(),
                    &self.notification_sink,
                    &self.runtime_adapter,
                    request_context.clone(),
                ))
                .await
            }
            "session/create" => {
                Box::pin(handlers::session::handle_create(
                    id,
                    params,
                    self.runtime.clone(),
                    &self.notification_sink,
                    &self.runtime_adapter,
                    request_context.clone(),
                ))
                .await
            }
            "session/list" => handlers::session::handle_list(id, params, &self.runtime).await,
            "session/read" => self.handle_session_read(id, params).await,
            "session/history" => self.handle_session_history(id, params).await,
            "session/fork_at" => self.handle_session_fork_at(id, params).await,
            "session/fork_replace" => self.handle_session_fork_replace(id, params).await,
            "blob/get" => self.handle_blob_get(id, params).await,
            "artifact/list" => self.handle_artifact_list(id, params).await,
            "artifact/get" => self.handle_artifact_get(id, params).await,
            "artifact/download" => self.handle_artifact_download(id, params).await,
            "session/archive" => self.handle_session_archive(id, params).await,
            "session/external_event" => {
                handlers::event::handle_external_event(id, params, self.runtime.clone()).await
            }
            "session/peer_response_terminal" => {
                handlers::event::handle_peer_response_terminal(id, params, self.runtime.clone())
                    .await
            }
            "events/latest_cursor" => {
                handlers::event::handle_events_latest_cursor(id, params, self.runtime.clone()).await
            }
            "events/list_since" => {
                handlers::event::handle_events_list_since(id, params, self.runtime.clone()).await
            }
            "events/snapshot" => {
                handlers::event::handle_events_snapshot(id, params, self.runtime.clone()).await
            }
            "session/inject_context" => self.handle_session_inject_context(id, params).await,
            "session/stream_open" => self.handle_session_stream_open(id, params).await,
            "session/stream_close" => self.handle_session_stream_close(id, params).await,
            "schedule/create" => {
                handlers::schedule::handle_create(id, params, self.runtime.clone()).await
            }
            "schedule/get" => {
                handlers::schedule::handle_get(id, params, self.runtime.clone()).await
            }
            "schedule/list" => {
                handlers::schedule::handle_list(id, params, self.runtime.clone()).await
            }
            "schedule/update" => {
                handlers::schedule::handle_update(id, params, self.runtime.clone()).await
            }
            "schedule/pause" => {
                handlers::schedule::handle_pause(id, params, self.runtime.clone()).await
            }
            "schedule/resume" => {
                handlers::schedule::handle_resume(id, params, self.runtime.clone()).await
            }
            "schedule/delete" => {
                handlers::schedule::handle_delete(id, params, self.runtime.clone()).await
            }
            "schedule/occurrences" => {
                handlers::schedule::handle_occurrences(id, params, self.runtime.clone()).await
            }
            "schedule/tools" => handlers::schedule::handle_tools(id).await,
            "schedule/call" => {
                handlers::schedule::handle_call(id, params, self.runtime.clone()).await
            }
            "turn/start" => {
                handlers::turn::handle_start(
                    id,
                    params,
                    self.runtime.clone(),
                    &self.notification_sink,
                    &self.runtime_adapter,
                    request_context.clone(),
                )
                .await
            }
            "turn/interrupt" => {
                #[cfg(feature = "mob")]
                {
                    handlers::turn::handle_interrupt(id, params, &self.runtime, &self.mob_state)
                        .await
                }
                #[cfg(not(feature = "mob"))]
                {
                    handlers::turn::handle_interrupt(id, params, &self.runtime).await
                }
            }
            #[cfg(feature = "mob")]
            "mob/create" => handlers::mob::handle_create(id, params, &self.mob_state).await,
            #[cfg(feature = "mob")]
            "mob/list" => handlers::mob::handle_list(id, &self.mob_state).await,
            #[cfg(feature = "mob")]
            "mob/status" => handlers::mob::handle_status(id, params, &self.mob_state).await,
            #[cfg(feature = "mob")]
            "mob/lifecycle" => handlers::mob::handle_lifecycle(id, params, &self.mob_state).await,
            #[cfg(feature = "mob")]
            "mob/spawn" => handlers::mob::handle_spawn(id, params, &self.mob_state).await,
            #[cfg(feature = "mob")]
            "mob/spawn_many" => handlers::mob::handle_spawn_many(id, params, &self.mob_state).await,
            #[cfg(feature = "mob")]
            "mob/ensure_member" => {
                handlers::mob::handle_ensure_member(id, params, &self.mob_state).await
            }
            #[cfg(feature = "mob")]
            "mob/reconcile" => handlers::mob::handle_reconcile(id, params, &self.mob_state).await,
            #[cfg(feature = "mob")]
            "mob/list_members_matching" => {
                handlers::mob::handle_list_members_matching(id, params, &self.mob_state).await
            }
            #[cfg(feature = "mob")]
            "mob/members" => handlers::mob::handle_members(id, params, &self.mob_state).await,
            #[cfg(feature = "mob")]
            "mob/retire" => handlers::mob::handle_retire(id, params, &self.mob_state).await,
            #[cfg(feature = "mob")]
            "mob/respawn" => handlers::mob::handle_respawn(id, params, &self.mob_state).await,
            #[cfg(feature = "mob")]
            "mob/wire" => handlers::mob::handle_wire(id, params, &self.mob_state).await,
            #[cfg(feature = "mob")]
            "mob/unwire" => handlers::mob::handle_unwire(id, params, &self.mob_state).await,
            #[cfg(feature = "mob")]
            "mob/events" => handlers::mob::handle_events(id, params, &self.mob_state).await,
            #[cfg(feature = "mob")]
            "mob/turn_start" => {
                handlers::mob::handle_mob_turn_start(
                    id,
                    params,
                    &self.mob_state,
                    self.runtime.clone(),
                    &self.notification_sink,
                    &self.runtime_adapter,
                    request_context.clone(),
                )
                .await
            }
            #[cfg(feature = "mob")]
            "mob/member_send" => {
                handlers::mob::handle_member_send(id, params, &self.mob_state).await
            }
            #[cfg(feature = "mob")]
            "mob/ingress_interaction" => {
                handlers::mob::handle_ingress_interaction(id, params, &self.mob_state).await
            }
            #[cfg(feature = "mob")]
            "mob/append_system_context" => {
                handlers::mob::handle_append_system_context(
                    id,
                    params,
                    &self.mob_state,
                    &self.runtime,
                )
                .await
            }
            #[cfg(feature = "mob")]
            "mob/flows" => handlers::mob::handle_flows(id, params, &self.mob_state).await,
            #[cfg(feature = "mob")]
            "mob/flow_run" => handlers::mob::handle_flow_run(id, params, &self.mob_state).await,
            #[cfg(feature = "mob")]
            "mob/flow_status" => {
                handlers::mob::handle_flow_status(id, params, &self.mob_state).await
            }
            #[cfg(feature = "mob")]
            "mob/flow_cancel" => {
                handlers::mob::handle_flow_cancel(id, params, &self.mob_state).await
            }
            #[cfg(feature = "mob")]
            "mob/spawn_helper" => {
                handlers::mob::handle_spawn_helper(id, params, &self.mob_state).await
            }
            #[cfg(feature = "mob")]
            "mob/fork_helper" => {
                handlers::mob::handle_fork_helper(id, params, &self.mob_state).await
            }
            #[cfg(feature = "mob")]
            "mob/force_cancel" => {
                handlers::mob::handle_force_cancel(id, params, &self.mob_state).await
            }
            #[cfg(feature = "mob")]
            "mob/member_status" => {
                handlers::mob::handle_member_status(id, params, &self.mob_state).await
            }
            #[cfg(feature = "mob")]
            "mob/snapshot" => handlers::mob::handle_snapshot(id, params, &self.mob_state).await,
            #[cfg(feature = "mob")]
            "mob/destroy" => handlers::mob::handle_destroy(id, params, &self.mob_state).await,
            #[cfg(feature = "mob")]
            "mob/rotate_supervisor" => {
                handlers::mob::handle_rotate_supervisor(id, params, &self.mob_state).await
            }
            #[cfg(feature = "mob")]
            "mob/submit_work" => {
                handlers::mob::handle_submit_work(id, params, &self.mob_state).await
            }
            #[cfg(feature = "mob")]
            "mob/cancel_work" => {
                handlers::mob::handle_cancel_work(id, params, &self.mob_state).await
            }
            #[cfg(feature = "mob")]
            "mob/cancel_all_work" => {
                handlers::mob::handle_cancel_all_work(id, params, &self.mob_state).await
            }
            #[cfg(feature = "mob")]
            "mob/wait_kickoff" => {
                handlers::mob::handle_wait_kickoff(id, params, &self.mob_state).await
            }
            #[cfg(feature = "mob")]
            "mob/wait_ready" => handlers::mob::handle_wait_ready(id, params, &self.mob_state).await,
            #[cfg(feature = "mob")]
            "mob/profile/create" => {
                handlers::mob::handle_profile_create(id, params, &self.mob_state).await
            }
            #[cfg(feature = "mob")]
            "mob/profile/get" => {
                handlers::mob::handle_profile_get(id, params, &self.mob_state).await
            }
            #[cfg(feature = "mob")]
            "mob/profile/list" => handlers::mob::handle_profile_list(id, &self.mob_state).await,
            #[cfg(feature = "mob")]
            "mob/profile/update" => {
                handlers::mob::handle_profile_update(id, params, &self.mob_state).await
            }
            #[cfg(feature = "mob")]
            "mob/profile/delete" => {
                handlers::mob::handle_profile_delete(id, params, &self.mob_state).await
            }
            #[cfg(feature = "mob")]
            "mob/stream_open" => self.handle_mob_stream_open(id, params).await,
            #[cfg(feature = "mob")]
            "mob/stream_close" => self.handle_mob_stream_close(id, params).await,
            #[cfg(feature = "comms")]
            "comms/send" => self.handle_comms_send(id, params).await,
            #[cfg(feature = "comms")]
            "comms/peers" => self.handle_comms_peers(id, params).await,
            "skills/list" => handlers::skills::handle_list(id, &self.skill_runtime).await,
            "skills/inspect" => {
                // Post-wave-a dogma: the shell-side skill inspection path was
                // retired; callers consult canonical skill registry surfaces.
                let _ = params;
                RpcResponse::error(
                    id,
                    error::METHOD_NOT_FOUND,
                    "skills/inspect is no longer served; resolve skills through the typed registry surface".to_string(),
                )
            }
            "capabilities/get" => {
                let config = self.config_store.get().await.unwrap_or_default();
                handlers::capabilities::handle_get(id, &config)
            }
            "runtime/host_info" => handlers::runtime_host::handle_info(
                id,
                &self.runtime,
                &self.config_store,
                self.runtime_adapter.runtime_mode() == meerkat_runtime::RuntimeMode::V9Compliant,
            ),
            "runtime/capabilities" => handlers::runtime_host::handle_capabilities(
                id,
                &self.runtime,
                self.runtime_adapter.runtime_mode() == meerkat_runtime::RuntimeMode::V9Compliant,
            ),
            "runtime/health" => handlers::runtime_host::handle_health(id),
            "approval/request" => {
                handlers::approval::handle_request(id, params, self.runtime.clone()).await
            }
            "approval/list" => {
                handlers::approval::handle_list(id, params, self.runtime.clone()).await
            }
            "approval/get" => {
                handlers::approval::handle_get(id, params, self.runtime.clone()).await
            }
            "approval/decide" => {
                handlers::approval::handle_decide(id, params, self.runtime.clone()).await
            }
            "models/catalog" => {
                let config = self.config_store.get().await.unwrap_or_default();
                handlers::models::handle_catalog(id, &config)
            }
            // Auth + realm methods (Phase 4d).
            "auth/profile/list" => {
                handlers::auth::handle_auth_profile_list(id, params, &self.runtime).await
            }
            "auth/profile/get" => {
                handlers::auth::handle_auth_profile_get(id, params, &self.runtime).await
            }
            "auth/profile/create" => {
                handlers::auth::handle_auth_profile_create(id, params, &self.runtime).await
            }
            "auth/profile/delete" => {
                handlers::auth::handle_auth_profile_delete(id, params, &self.runtime).await
            }
            "auth/login/start" => {
                handlers::auth::handle_auth_login_start(id, params, &self.runtime).await
            }
            "auth/login/complete" => {
                handlers::auth::handle_auth_login_complete(id, params, &self.runtime).await
            }
            "auth/login/device_start" => {
                handlers::auth::handle_auth_login_device_start(id, params, &self.runtime).await
            }
            "auth/login/device_complete" => {
                handlers::auth::handle_auth_login_device_complete(id, params, &self.runtime).await
            }
            "auth/login/provision_api_key" => {
                handlers::auth::handle_auth_login_provision_api_key(id, params, &self.runtime).await
            }
            "auth/status/get" => {
                handlers::auth::handle_auth_status_get(id, params, &self.runtime).await
            }
            "auth/logout" => handlers::auth::handle_auth_logout(id, params, &self.runtime).await,
            "realm/list" => handlers::auth::handle_realm_list(id, &self.runtime).await,
            "realm/get" => handlers::auth::handle_realm_get(id, params, &self.runtime).await,
            "config/get" => {
                handlers::config::handle_get(id, &self.config_store, self.runtime.config_runtime())
                    .await
            }
            "config/set" => {
                handlers::config::handle_set(
                    id,
                    params,
                    &self.runtime,
                    &self.config_store,
                    self.runtime.config_runtime(),
                )
                .await
            }
            "config/patch" => {
                handlers::config::handle_patch(
                    id,
                    params,
                    &self.runtime,
                    &self.config_store,
                    self.runtime.config_runtime(),
                )
                .await
            }
            // B16: live/* is only registered when `with_live_ws` was called.
            // Without --live-ws, the router has no transport state and would
            // hand out empty WS URLs/tokens — refuse the method instead of
            // silently routing it through a broken handler.
            "live/open" if self.live_ws_state.is_some() => {
                handlers::live::handle_live_open(
                    id,
                    params,
                    &self.live_adapter_host,
                    self.live_ws_state.as_deref(),
                    self.live_ws_base_url.as_deref(),
                    &self.runtime,
                    self.live_session_factory.as_ref().map(Arc::as_ref),
                )
                .await
            }
            "live/status" if self.live_ws_state.is_some() => {
                handlers::live::handle_live_status(id, params, &self.live_adapter_host).await
            }
            "live/close" if self.live_ws_state.is_some() => {
                handlers::live::handle_live_close(id, params, &self.live_adapter_host).await
            }
            // P1#5: push a fresh projection snapshot into an already-open
            // live adapter (model switch via `config/patch`, snapshot drift
            // after a session edit, etc.). Same gating as the other live/*
            // arms — without `--live-ws` the router has no transport state.
            "live/refresh" if self.live_ws_state.is_some() => {
                handlers::live::handle_live_refresh(
                    id,
                    params,
                    &self.live_adapter_host,
                    &self.runtime,
                )
                .await
            }
            "live/send_input" if self.live_ws_state.is_some() => {
                handlers::live::handle_live_send_input(id, params, &self.live_adapter_host).await
            }
            // I50: surface the buffered-input commit verb. Same gating as the
            // other live/* arms — without --live-ws the router has no
            // transport state and the method falls through to METHOD_NOT_FOUND.
            "live/commit_input" if self.live_ws_state.is_some() => {
                handlers::live::handle_live_commit_input(id, params, &self.live_adapter_host).await
            }
            // A7: explicit barge-in surface. Without these arms callers can
            // only rely on provider-native VAD; with them, a client can
            // signal interrupt directly and truncate an assistant item at a
            // specific playback cursor.
            "live/interrupt" if self.live_ws_state.is_some() => {
                handlers::live::handle_live_interrupt(id, params, &self.live_adapter_host).await
            }
            "live/truncate" if self.live_ws_state.is_some() => {
                handlers::live::handle_live_truncate(id, params, &self.live_adapter_host).await
            }
            // A7: no `live/playback_cursor` arm — playback is a client-side
            // fact (jitter buffers, end-of-stream silence trim). Clients
            // track the cursor locally and pass `audio_played_ms` into
            // `live/truncate`. See the doc-comment in `handlers/live.rs`.
            "mcp/add" => handlers::mcp::handle_add(id, params, &self.runtime).await,
            "mcp/remove" => handlers::mcp::handle_remove(id, params, &self.runtime).await,
            "mcp/reload" => handlers::mcp::handle_reload(id, params, &self.runtime).await,
            _ => RpcResponse::error(
                id,
                error::METHOD_NOT_FOUND,
                format!("Method not found: {}", request.method),
            ),
        };

        Some(response)
    }

    /// Access the underlying session runtime.
    pub fn runtime(&self) -> &SessionRuntime {
        &self.runtime
    }

    async fn handle_session_read(
        &self,
        id: Option<crate::protocol::RpcId>,
        params: Option<&serde_json::value::RawValue>,
    ) -> RpcResponse {
        let params: handlers::session::ReadSessionParams = match handlers::parse_params(params) {
            Ok(p) => p,
            Err(resp) => return resp.with_id(id),
        };

        let session_id = match handlers::parse_session_id_for_runtime(
            id.clone(),
            &params.session_id,
            &self.runtime,
        ) {
            Ok(sid) => sid,
            Err(resp) => return resp,
        };

        // Use resolve_session_owner (from main) with enriched WireSessionInfo (from this PR).
        match self.resolve_session_owner(&session_id).await {
            Some(SessionOwner::Runtime) => {
                if let Some(mut info) = self.runtime.read_session_rich(&session_id).await {
                    info.session_ref = self
                        .runtime
                        .realm_id()
                        .map(|realm| meerkat_contracts::format_session_ref(&realm, &session_id));
                    RpcResponse::success(id, info)
                } else {
                    RpcResponse::error(
                        id,
                        error::SESSION_NOT_FOUND,
                        format!("Session not found: {session_id}"),
                    )
                }
            }
            #[cfg(feature = "mob")]
            Some(SessionOwner::Mob) => {
                match self.mob_state.session_service().read(&session_id).await {
                    Ok(view) => {
                        let mut info: meerkat_contracts::WireSessionInfo = view.state.into();
                        info.session_ref = self.runtime.realm_id().map(|realm| {
                            meerkat_contracts::format_session_ref(&realm, &session_id)
                        });
                        RpcResponse::success(id, info)
                    }
                    Err(err) => RpcResponse::error(id, error::SESSION_NOT_FOUND, err.to_string()),
                }
            }
            None => RpcResponse::error(
                id,
                error::SESSION_NOT_FOUND,
                format!("Session not found: {session_id}"),
            ),
        }
    }

    async fn handle_session_history(
        &self,
        id: Option<crate::protocol::RpcId>,
        params: Option<&serde_json::value::RawValue>,
    ) -> RpcResponse {
        let params: handlers::session::ReadSessionHistoryParams =
            match handlers::parse_params(params) {
                Ok(p) => p,
                Err(resp) => return resp.with_id(id),
            };

        let session_id = match handlers::parse_session_id_for_runtime(
            id.clone(),
            &params.session_id,
            &self.runtime,
        ) {
            Ok(sid) => sid,
            Err(resp) => return resp,
        };

        let query = SessionHistoryQuery {
            offset: params.offset.unwrap_or(0),
            limit: params.limit,
        };

        match self.resolve_session_owner(&session_id).await {
            Some(SessionOwner::Runtime) => {
                if let Some(mut history) = self
                    .runtime
                    .read_session_history_rich(&session_id, query)
                    .await
                {
                    history.session_ref = self
                        .runtime
                        .realm_id()
                        .map(|realm| meerkat_contracts::format_session_ref(&realm, &session_id));
                    RpcResponse::success(id, history)
                } else {
                    RpcResponse::error(
                        id,
                        error::SESSION_NOT_FOUND,
                        format!("Session not found: {session_id}"),
                    )
                }
            }
            #[cfg(feature = "mob")]
            Some(SessionOwner::Mob) => match self
                .try_read_mob_session_history(id.clone(), &session_id, query)
                .await
            {
                Some(resp) => resp,
                None => RpcResponse::error(
                    id,
                    error::SESSION_NOT_FOUND,
                    format!("Session not found: {session_id}"),
                ),
            },
            None => {
                #[cfg(feature = "mob")]
                if let Some(resp) = self
                    .try_read_mob_session_history(id.clone(), &session_id, query)
                    .await
                {
                    return resp;
                }
                RpcResponse::error(
                    id,
                    error::SESSION_NOT_FOUND,
                    format!("Session not found: {session_id}"),
                )
            }
        }
    }

    async fn handle_session_fork_at(
        &self,
        id: Option<crate::protocol::RpcId>,
        params: Option<&serde_json::value::RawValue>,
    ) -> RpcResponse {
        let params: meerkat_contracts::ForkSessionAtParams = match handlers::parse_params(params) {
            Ok(p) => p,
            Err(resp) => return resp.with_id(id),
        };

        let session_id = match handlers::parse_session_id_for_runtime(
            id.clone(),
            &params.session_id,
            &self.runtime,
        ) {
            Ok(sid) => sid,
            Err(resp) => return resp,
        };

        match self.resolve_session_owner(&session_id).await {
            Some(SessionOwner::Runtime) => match self
                .runtime
                .fork_session_at(
                    &session_id,
                    SessionForkAtRequest {
                        message_index: params.message_index,
                        running_behavior: params.running_behavior,
                    },
                )
                .await
            {
                Ok(mut result) => {
                    result.session_ref = self.runtime.realm_id().map(|realm| {
                        meerkat_contracts::format_session_ref(&realm, &result.session_id)
                    });
                    RpcResponse::success(id, result)
                }
                Err(rpc_err) => RpcResponse::error(id, rpc_err.code, rpc_err.message),
            },
            #[cfg(feature = "mob")]
            Some(SessionOwner::Mob) => RpcResponse::error(
                id,
                error::INVALID_PARAMS,
                "mob-owned session transcripts cannot be edited through the generic session surface",
            ),
            None => RpcResponse::error(
                id,
                error::SESSION_NOT_FOUND,
                format!("Session not found: {session_id}"),
            ),
        }
    }

    async fn handle_session_fork_replace(
        &self,
        id: Option<crate::protocol::RpcId>,
        params: Option<&serde_json::value::RawValue>,
    ) -> RpcResponse {
        let params: meerkat_contracts::ForkSessionReplaceParams =
            match handlers::parse_params(params) {
                Ok(p) => p,
                Err(resp) => return resp.with_id(id),
            };

        let session_id = match handlers::parse_session_id_for_runtime(
            id.clone(),
            &params.session_id,
            &self.runtime,
        ) {
            Ok(sid) => sid,
            Err(resp) => return resp,
        };

        match self.resolve_session_owner(&session_id).await {
            Some(SessionOwner::Runtime) => match self
                .runtime
                .fork_session_replace(
                    &session_id,
                    SessionForkReplaceRequest {
                        message_index: params.message_index,
                        replacement: params.replacement,
                        running_behavior: params.running_behavior,
                    },
                )
                .await
            {
                Ok(mut result) => {
                    result.session_ref = self.runtime.realm_id().map(|realm| {
                        meerkat_contracts::format_session_ref(&realm, &result.session_id)
                    });
                    RpcResponse::success(id, result)
                }
                Err(rpc_err) => RpcResponse::error(id, rpc_err.code, rpc_err.message),
            },
            #[cfg(feature = "mob")]
            Some(SessionOwner::Mob) => RpcResponse::error(
                id,
                error::INVALID_PARAMS,
                "mob-owned session transcripts cannot be edited through the generic session surface",
            ),
            None => RpcResponse::error(
                id,
                error::SESSION_NOT_FOUND,
                format!("Session not found: {session_id}"),
            ),
        }
    }

    async fn handle_session_archive(
        &self,
        id: Option<crate::protocol::RpcId>,
        params: Option<&serde_json::value::RawValue>,
    ) -> RpcResponse {
        let params: handlers::session::ArchiveSessionParams = match handlers::parse_params(params) {
            Ok(p) => p,
            Err(resp) => return resp.with_id(id),
        };
        let session_id = match handlers::parse_session_id_for_runtime(
            id.clone(),
            &params.session_id,
            &self.runtime,
        ) {
            Ok(sid) => sid,
            Err(resp) => return resp,
        };
        match self.resolve_session_owner(&session_id).await {
            Some(SessionOwner::Runtime) => match self.runtime.archive_session(&session_id).await {
                Ok(()) => {
                    // Clean up session-owned mobs (implicit + explicit).
                    #[cfg(feature = "mob")]
                    if let Err(error) = self
                        .mob_state
                        .destroy_bridge_session_mobs(&session_id.to_string())
                        .await
                    {
                        return mob_destroy_cleanup_error_response(id, error);
                    }
                    self.runtime_adapter.unregister_session(&session_id).await;
                    RpcResponse::success(id, json!({"archived": true}))
                }
                Err(rpc_err) => {
                    #[cfg(feature = "mob")]
                    {
                        let retained_cleanup = self
                            .mob_state
                            .has_bridge_session_scoped_mobs(&session_id.to_string())
                            .await;
                        if rpc_err.code == error::SESSION_NOT_FOUND && retained_cleanup {
                            return match self
                                .mob_state
                                .destroy_bridge_session_mobs(&session_id.to_string())
                                .await
                            {
                                Ok(()) => RpcResponse::success(id, json!({"archived": true})),
                                Err(error) => mob_destroy_cleanup_error_response(id, error),
                            };
                        }
                    }

                    match rpc_err.data {
                        Some(data) => {
                            RpcResponse::error_with_data(id, rpc_err.code, rpc_err.message, data)
                        }
                        None => RpcResponse::error(id, rpc_err.code, rpc_err.message),
                    }
                }
            },
            #[cfg(feature = "mob")]
            Some(SessionOwner::Mob) => match self
                .mob_state
                .archive_mob_owned_bridge_session_with_cleanup(
                    &session_id,
                    "mob cleanup during archive incomplete",
                )
                .await
            {
                Ok(true) => RpcResponse::success(id, json!({"archived": true})),
                Ok(false) => {
                    if self
                        .mob_state
                        .has_bridge_session_scoped_mobs(&session_id.to_string())
                        .await
                    {
                        match self
                            .mob_state
                            .destroy_bridge_session_mobs(&session_id.to_string())
                            .await
                        {
                            Ok(()) => RpcResponse::success(id, json!({"archived": true})),
                            Err(error) => mob_destroy_cleanup_error_response(id, error),
                        }
                    } else {
                        RpcResponse::error(
                            id,
                            error::SESSION_NOT_FOUND,
                            format!("Session not found: {session_id}"),
                        )
                    }
                }
                Err(error) => mob_archive_session_error_response(id, &session_id, error),
            },
            None => {
                #[cfg(feature = "mob")]
                if self
                    .mob_state
                    .has_bridge_session_scoped_mobs(&session_id.to_string())
                    .await
                {
                    return match self
                        .mob_state
                        .destroy_bridge_session_mobs(&session_id.to_string())
                        .await
                    {
                        Ok(()) => RpcResponse::success(id, json!({"archived": true})),
                        Err(error) => mob_destroy_cleanup_error_response(id, error),
                    };
                }
                RpcResponse::error(
                    id,
                    error::SESSION_NOT_FOUND,
                    format!("Session not found: {session_id}"),
                )
            }
        }
    }

    async fn handle_session_inject_context(
        &self,
        id: Option<crate::protocol::RpcId>,
        params: Option<&serde_json::value::RawValue>,
    ) -> RpcResponse {
        let params: handlers::session::InjectSystemContextParams =
            match handlers::parse_params(params) {
                Ok(p) => p,
                Err(resp) => return resp.with_id(id),
            };
        let session_id = match handlers::parse_session_id_for_runtime(
            id.clone(),
            &params.session_id,
            &self.runtime,
        ) {
            Ok(sid) => sid,
            Err(resp) => return resp,
        };
        let req = meerkat_core::AppendSystemContextRequest {
            text: params.text,
            source: params.source,
            idempotency_key: params.idempotency_key,
        };
        match self.resolve_session_owner(&session_id).await {
            Some(SessionOwner::Runtime) => {
                match self.runtime.append_system_context(&session_id, req).await {
                    Ok(result) => RpcResponse::success(id, json!({"status": result.status})),
                    Err(rpc_err) => RpcResponse::error(id, rpc_err.code, rpc_err.message),
                }
            }
            #[cfg(feature = "mob")]
            Some(SessionOwner::Mob) => match self
                .mob_state
                .session_service()
                .append_system_context(&session_id, req)
                .await
            {
                Ok(result) => RpcResponse::success(id, json!({"status": result.status})),
                Err(err) => RpcResponse::error(id, error::SESSION_NOT_FOUND, err.to_string()),
            },
            None => RpcResponse::error(
                id,
                error::SESSION_NOT_FOUND,
                format!("Session not found: {session_id}"),
            ),
        }
    }

    #[cfg(feature = "comms")]
    async fn handle_comms_send(
        &self,
        id: Option<crate::protocol::RpcId>,
        params: Option<&serde_json::value::RawValue>,
    ) -> RpcResponse {
        let params: handlers::comms::CommsSendParams = match handlers::parse_params(params) {
            Ok(p) => p,
            Err(resp) => return resp,
        };
        let session_id = match handlers::parse_session_id_for_runtime(
            id.clone(),
            params.session_id(),
            &self.runtime,
        ) {
            Ok(sid) => sid,
            Err(resp) => return resp,
        };
        let comms = match self.resolve_session_owner(&session_id).await {
            Some(SessionOwner::Runtime) => {
                if self.runtime.session_state(&session_id).await.is_none() {
                    return RpcResponse::error(
                        id,
                        error::INVALID_PARAMS,
                        format!("Session is archived: {session_id}"),
                    );
                }
                self.runtime.comms_runtime(&session_id).await
            }
            #[cfg(feature = "mob")]
            Some(SessionOwner::Mob) => {
                self.mob_state
                    .session_service()
                    .comms_runtime(&session_id)
                    .await
            }
            None => None,
        };
        let Some(comms) = comms else {
            return RpcResponse::error(
                id,
                error::INVALID_PARAMS,
                format!("Session not found or comms not enabled: {session_id}"),
            );
        };
        let peer_name = params.peer_label();
        let cmd = match params.into_command().into_command(&session_id) {
            Ok(cmd) => cmd,
            Err(err) => {
                return RpcResponse::error_with_data(
                    id,
                    error::INVALID_PARAMS,
                    "Command validation failed",
                    json!({
                        "code": "invalid_command",
                        "message": err.to_string(),
                    }),
                );
            }
        };
        match comms.send(cmd).await {
            Ok(receipt) => RpcResponse::success(id, send_receipt_json(receipt)),
            Err(e) => {
                let normalized = match &e {
                    meerkat_core::comms::SendError::PeerNotFound(peer) => json!({
                        "code": "peer_not_found_or_not_trusted",
                        "peer": peer,
                        "message": format!("peer '{peer}' is not found or not trusted"),
                    }),
                    meerkat_core::comms::SendError::PeerOffline => {
                        let peer = peer_name.as_deref().unwrap_or("<unknown>");
                        json!({
                            "code": "peer_unreachable",
                            "peer": peer,
                            "reason": "offline_or_no_ack",
                            "message": format!("peer '{peer}' is unreachable: offline_or_no_ack"),
                        })
                    }
                    meerkat_core::comms::SendError::Internal(details)
                        if peer_name.is_some() && is_transport_internal(details) =>
                    {
                        let peer = peer_name.as_deref().unwrap_or("<unknown>");
                        json!({
                            "code": "peer_unreachable",
                            "peer": peer,
                            "reason": "transport_error",
                            "message": format!("peer '{peer}' is unreachable: transport_error"),
                            "details": details,
                        })
                    }
                    _ => json!({
                        "code": "send_failed",
                        "message": e.to_string(),
                    }),
                };
                let message = normalized
                    .get("message")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("Comms send failed")
                    .to_string();
                RpcResponse::error_with_data(id, error::INTERNAL_ERROR, message, normalized)
            }
        }
    }

    #[cfg(feature = "comms")]
    async fn handle_comms_peers(
        &self,
        id: Option<crate::protocol::RpcId>,
        params: Option<&serde_json::value::RawValue>,
    ) -> RpcResponse {
        let params: handlers::comms::CommsPeersParams = match handlers::parse_params(params) {
            Ok(p) => p,
            Err(resp) => return resp,
        };
        let session_id = match handlers::parse_session_id_for_runtime(
            id.clone(),
            &params.session_id,
            &self.runtime,
        ) {
            Ok(sid) => sid,
            Err(resp) => return resp,
        };
        let comms = match self.resolve_session_owner(&session_id).await {
            Some(SessionOwner::Runtime) => {
                if self.runtime.session_state(&session_id).await.is_none() {
                    return RpcResponse::error(
                        id,
                        error::INVALID_PARAMS,
                        format!("Session is archived: {session_id}"),
                    );
                }
                self.runtime.comms_runtime(&session_id).await
            }
            #[cfg(feature = "mob")]
            Some(SessionOwner::Mob) => {
                self.mob_state
                    .session_service()
                    .comms_runtime(&session_id)
                    .await
            }
            None => None,
        };
        let Some(comms) = comms else {
            return RpcResponse::error(
                id,
                error::INVALID_PARAMS,
                format!("Session not found or comms not enabled: {session_id}"),
            );
        };
        let peers = comms.peers().await;
        let result = meerkat_contracts::CommsPeersResult::from_entries(&peers);
        match serde_json::to_value(result) {
            Ok(value) => RpcResponse::success(id, value),
            Err(serialize_error) => RpcResponse::error(
                id,
                error::INTERNAL_ERROR,
                format!("Serialize error: {serialize_error}"),
            ),
        }
    }

    async fn handle_session_stream_open(
        &self,
        id: Option<crate::protocol::RpcId>,
        params: Option<&serde_json::value::RawValue>,
    ) -> RpcResponse {
        let params =
            match handlers::parse_params::<meerkat_contracts::SessionStreamOpenParams>(params) {
                Ok(p) => p,
                Err(resp) => return resp.with_id(id),
            };

        let session_id = match handlers::parse_session_id_for_runtime(
            id.clone(),
            &params.session_id,
            &self.runtime,
        ) {
            Ok(session_id) => session_id,
            Err(resp) => return resp,
        };

        let stream = match self.resolve_session_owner(&session_id).await {
            Some(SessionOwner::Runtime) => {
                match self.runtime.subscribe_session_events(&session_id).await {
                    Ok(stream) => stream,
                    Err(err) => {
                        return RpcResponse::error(
                            id,
                            error::INVALID_PARAMS,
                            format!("Failed to subscribe to session events: {err}"),
                        );
                    }
                }
            }
            #[cfg(feature = "mob")]
            Some(SessionOwner::Mob) => {
                let session_service = self.mob_state.session_service();
                match meerkat_mob::MobSessionService::subscribe_session_events(
                    &*session_service,
                    &session_id,
                )
                .await
                {
                    Ok(stream) => stream,
                    Err(err) => {
                        return RpcResponse::error(
                            id,
                            error::INVALID_PARAMS,
                            format!("Failed to subscribe to session events: {err}"),
                        );
                    }
                }
            }
            None => {
                return RpcResponse::error(
                    id,
                    error::SESSION_NOT_FOUND,
                    format!("Session not found: {session_id}"),
                );
            }
        };

        let stream_id = Uuid::new_v4();
        self.closed_session_streams.lock().await.remove(&stream_id);
        let (stop_tx, stop_rx) = oneshot::channel::<()>();
        let notification_sink = self.notification_sink.clone();
        let active_session_streams = self.active_session_streams.clone();
        let closed_session_streams = self.closed_session_streams.clone();
        let stream_id_for_task = stream_id;
        let session_id_for_task = session_id.clone();

        let task = tokio::spawn(async move {
            let mut stream = stream;
            let mut stop_rx = stop_rx;
            let mut sequence = 0u64;

            loop {
                tokio::select! {
                    _ = &mut stop_rx => {
                        break;
                    }
                    event = stream.next() => {
                        match event {
                            Some(envelope) => {
                                sequence += 1;
                                let emit_status = notification_sink
                                    .emit_session_stream_event(&stream_id_for_task, sequence, &session_id_for_task, &envelope)
                                    .await;
                                if emit_status == StreamEmitStatus::Overflow {
                                    let should_emit = {
                                        let mut streams = active_session_streams.lock().await;
                                        if streams
                                            .get(&stream_id_for_task)
                                            .is_some_and(|s| matches!(s.state, StreamForwarderState::Active { .. }))
                                        {
                                            streams.remove(&stream_id_for_task);
                                            closed_session_streams
                                                .lock()
                                                .await
                                                .insert(stream_id_for_task);
                                            true
                                        } else {
                                            false
                                        }
                                    };
                                    if should_emit {
                                        notification_sink
                                            .emit_session_stream_end(
                                                &stream_id_for_task,
                                                &session_id_for_task,
                                                "terminal_error",
                                                Some("transport stream notification queue overflow"),
                                            )
                                            .await;
                                    }
                                    break;
                                }
                            }
                            None => {
                                let should_emit = {
                                    let mut streams = active_session_streams.lock().await;
                                    if streams
                                        .get(&stream_id_for_task)
                                        .is_some_and(|s| matches!(s.state, StreamForwarderState::Active { .. }))
                                    {
                                        streams.remove(&stream_id_for_task);
                                        closed_session_streams
                                            .lock()
                                            .await
                                            .insert(stream_id_for_task);
                                        true
                                    } else {
                                        false
                                    }
                                };
                                if should_emit {
                                    notification_sink
                                        .emit_session_stream_end(
                                            &stream_id_for_task,
                                            &session_id_for_task,
                                            "remote_end",
                                            None,
                                        )
                                        .await;
                                }
                                break;
                            }
                        }
                    }
                }
            }
        });

        self.active_session_streams.lock().await.insert(
            stream_id,
            StreamForwarder {
                terminal: StreamTerminal::Session(session_id.clone()),
                state: StreamForwarderState::Active {
                    stop_tx: Some(stop_tx),
                    task,
                },
            },
        );

        let result = meerkat_contracts::SessionStreamOpenResult {
            stream_id: stream_id.to_string(),
            session_id: session_id.to_string(),
            opened: true,
        };
        match serde_json::to_value(result) {
            Ok(value) => RpcResponse::success(id, value),
            Err(serialize_error) => RpcResponse::error(
                id,
                error::INTERNAL_ERROR,
                format!("Serialize error: {serialize_error}"),
            ),
        }
    }

    async fn handle_session_stream_close(
        &self,
        id: Option<crate::protocol::RpcId>,
        params: Option<&serde_json::value::RawValue>,
    ) -> RpcResponse {
        let params =
            match handlers::parse_params::<meerkat_contracts::SessionStreamCloseParams>(params) {
                Ok(p) => p,
                Err(resp) => return resp.with_id(id),
            };

        let stream_id = match Uuid::parse_str(&params.stream_id) {
            Ok(stream_id) => stream_id,
            Err(_) => {
                return RpcResponse::error(
                    id,
                    error::INVALID_PARAMS,
                    format!("Invalid stream_id: {}", params.stream_id),
                );
            }
        };

        let closed_terminal = {
            let mut active_session_streams = self.active_session_streams.lock().await;
            match active_session_streams.remove(&stream_id) {
                Some(mut stream) => match &mut stream.state {
                    StreamForwarderState::Active { stop_tx, task } => {
                        if let Some(stop_tx) = stop_tx.take() {
                            let _ = stop_tx.send(());
                        }
                        task.abort();
                        self.closed_session_streams.lock().await.insert(stream_id);
                        Some(stream.terminal)
                    }
                },
                None => None,
            }
        };
        let already_closed = match closed_terminal {
            Some(StreamTerminal::Session(session_id)) => {
                self.notification_sink
                    .emit_session_stream_end(&stream_id, &session_id, "explicit_close", None)
                    .await;
                false
            }
            #[cfg(feature = "mob")]
            Some(StreamTerminal::Mob) => {
                unreachable!("session stream stored mob terminal metadata")
            }
            None => self.closed_session_streams.lock().await.remove(&stream_id),
        };

        let result = meerkat_contracts::SessionStreamCloseResult {
            stream_id: stream_id.to_string(),
            closed: true,
            already_closed,
        };
        match serde_json::to_value(result) {
            Ok(value) => RpcResponse::success(id, value),
            Err(serialize_error) => RpcResponse::error(
                id,
                error::INTERNAL_ERROR,
                format!("Serialize error: {serialize_error}"),
            ),
        }
    }

    #[cfg(feature = "mob")]
    async fn handle_mob_stream_open(
        &self,
        id: Option<crate::protocol::RpcId>,
        params: Option<&serde_json::value::RawValue>,
    ) -> RpcResponse {
        use serde::Deserialize;
        use serde_json::json;

        #[derive(Deserialize)]
        struct MobStreamOpenParams {
            mob_id: String,
            #[serde(default)]
            agent_identity: Option<String>,
        }

        let params = match handlers::parse_params::<MobStreamOpenParams>(params) {
            Ok(p) => p,
            Err(resp) => return resp,
        };

        let mob_id = meerkat_mob::MobId::from(params.mob_id.as_str());
        let handle: meerkat_mob::MobHandle = match self.mob_state.handle_for(&mob_id).await {
            Ok(h) => h,
            Err(e) => {
                return RpcResponse::error(
                    id,
                    error::INVALID_PARAMS,
                    format!("Mob not found: {e}"),
                );
            }
        };

        let stream_id = Uuid::new_v4();
        self.closed_mob_streams.lock().await.remove(&stream_id);
        let (stop_tx, stop_rx) = oneshot::channel::<()>();
        let notification_sink = self.notification_sink.clone();
        let active_mob_streams = self.active_mob_streams.clone();
        let closed_mob_streams = self.closed_mob_streams.clone();
        let stream_id_for_task = stream_id;

        if let Some(agent_identity) = params.agent_identity {
            // Per-member stream: subscribe to a specific member's agent events.
            let identity = meerkat_mob::AgentIdentity::from(agent_identity.as_str());
            let stream: meerkat_core::comms::EventStream =
                match handle.subscribe_agent_events(&identity).await {
                    Ok(s) => s,
                    Err(e) => {
                        return RpcResponse::error(
                            id,
                            error::INVALID_PARAMS,
                            format!("Failed to subscribe to member events: {e}"),
                        );
                    }
                };

            let task = tokio::spawn(async move {
                let mut stream = stream;
                let mut stop_rx = stop_rx;
                let mut sequence = 0u64;

                loop {
                    tokio::select! {
                        _ = &mut stop_rx => {
                            break;
                        }
                        event = stream.next() => {
                            match event {
                                Some(envelope) => {
                                    sequence += 1;
                                    let event_json = serde_json::to_value(&envelope)
                                        .unwrap_or(serde_json::Value::Null);
                                    let emit_status = notification_sink
                                        .emit_mob_stream_event(
                                            &stream_id_for_task,
                                            sequence,
                                            &event_json,
                                        )
                                        .await;
                                    if emit_status == StreamEmitStatus::Overflow {
                                        let should_emit = {
                                            let mut streams = active_mob_streams.lock().await;
                                            if streams
                                                .get(&stream_id_for_task)
                                                .is_some_and(|s| matches!(s.state, StreamForwarderState::Active { .. }))
                                            {
                                                streams.remove(&stream_id_for_task);
                                                closed_mob_streams.lock().await.insert(stream_id_for_task);
                                                true
                                            } else {
                                                false
                                            }
                                        };
                                        if should_emit {
                                            notification_sink
                                                .emit_mob_stream_end(
                                                    &stream_id_for_task,
                                                    "terminal_error",
                                                    Some("transport stream notification queue overflow"),
                                                )
                                                .await;
                                        }
                                        break;
                                    }
                                }
                                None => {
                                    let should_emit = {
                                        let mut streams = active_mob_streams.lock().await;
                                        if streams
                                            .get(&stream_id_for_task)
                                            .is_some_and(|s| matches!(s.state, StreamForwarderState::Active { .. }))
                                        {
                                            streams.remove(&stream_id_for_task);
                                            closed_mob_streams.lock().await.insert(stream_id_for_task);
                                            true
                                        } else {
                                            false
                                        }
                                    };
                                    if should_emit {
                                        notification_sink
                                            .emit_mob_stream_end(&stream_id_for_task, "remote_end", None)
                                            .await;
                                    }
                                    break;
                                }
                            }
                        }
                    }
                }
            });

            self.active_mob_streams.lock().await.insert(
                stream_id,
                StreamForwarder {
                    terminal: StreamTerminal::Mob,
                    state: StreamForwarderState::Active {
                        stop_tx: Some(stop_tx),
                        task,
                    },
                },
            );
        } else {
            // Mob-wide stream: subscribe to all members' events (attributed).
            let mut router_handle = handle.subscribe_mob_events().await;

            let task = tokio::spawn(async move {
                let mut stop_rx = stop_rx;
                let mut sequence = 0u64;

                loop {
                    tokio::select! {
                        _ = &mut stop_rx => {
                            break;
                        }
                        event = router_handle.event_rx.recv() => {
                            match event {
                                Some(attributed) => {
                                    sequence += 1;
                                    let event_json = serde_json::to_value(&attributed)
                                        .unwrap_or(serde_json::Value::Null);
                                    let emit_status = notification_sink
                                        .emit_mob_stream_event(
                                            &stream_id_for_task,
                                            sequence,
                                            &event_json,
                                        )
                                        .await;
                                    if emit_status == StreamEmitStatus::Overflow {
                                        let should_emit = {
                                            let mut streams = active_mob_streams.lock().await;
                                            if streams
                                                .get(&stream_id_for_task)
                                                .is_some_and(|s| matches!(s.state, StreamForwarderState::Active { .. }))
                                            {
                                                streams.remove(&stream_id_for_task);
                                                closed_mob_streams.lock().await.insert(stream_id_for_task);
                                                true
                                            } else {
                                                false
                                            }
                                        };
                                        if should_emit {
                                            notification_sink
                                                .emit_mob_stream_end(
                                                    &stream_id_for_task,
                                                    "terminal_error",
                                                    Some("transport stream notification queue overflow"),
                                                )
                                                .await;
                                        }
                                        break;
                                    }
                                }
                                None => {
                                    let should_emit = {
                                        let mut streams = active_mob_streams.lock().await;
                                        if streams
                                            .get(&stream_id_for_task)
                                            .is_some_and(|s| matches!(s.state, StreamForwarderState::Active { .. }))
                                        {
                                            streams.remove(&stream_id_for_task);
                                            closed_mob_streams.lock().await.insert(stream_id_for_task);
                                            true
                                        } else {
                                            false
                                        }
                                    };
                                    if should_emit {
                                        notification_sink
                                            .emit_mob_stream_end(&stream_id_for_task, "remote_end", None)
                                            .await;
                                    }
                                    break;
                                }
                            }
                        }
                    }
                }
            });

            self.active_mob_streams.lock().await.insert(
                stream_id,
                StreamForwarder {
                    terminal: StreamTerminal::Mob,
                    state: StreamForwarderState::Active {
                        stop_tx: Some(stop_tx),
                        task,
                    },
                },
            );
        }

        RpcResponse::success(
            id,
            json!({
                "stream_id": stream_id.to_string(),
                "opened": true,
            }),
        )
    }

    #[cfg(feature = "mob")]
    async fn handle_mob_stream_close(
        &self,
        id: Option<crate::protocol::RpcId>,
        params: Option<&serde_json::value::RawValue>,
    ) -> RpcResponse {
        use serde::Deserialize;
        use serde_json::json;

        #[derive(Deserialize)]
        struct MobStreamCloseParams {
            stream_id: String,
        }

        let params = match handlers::parse_params::<MobStreamCloseParams>(params) {
            Ok(p) => p,
            Err(resp) => return resp,
        };

        let stream_id = match Uuid::parse_str(&params.stream_id) {
            Ok(stream_id) => stream_id,
            Err(_) => {
                return RpcResponse::error(
                    id,
                    error::INVALID_PARAMS,
                    format!("Invalid stream_id: {}", params.stream_id),
                );
            }
        };

        let closed_terminal = {
            let mut active_mob_streams = self.active_mob_streams.lock().await;
            match active_mob_streams.remove(&stream_id) {
                Some(mut stream) => match &mut stream.state {
                    StreamForwarderState::Active { stop_tx, task } => {
                        if let Some(stop_tx) = stop_tx.take() {
                            let _ = stop_tx.send(());
                        }
                        task.abort();
                        self.closed_mob_streams.lock().await.insert(stream_id);
                        Some(stream.terminal)
                    }
                },
                None => None,
            }
        };
        let already_closed = match closed_terminal {
            Some(StreamTerminal::Mob) => {
                self.notification_sink
                    .emit_mob_stream_end(&stream_id, "explicit_close", None)
                    .await;
                false
            }
            Some(StreamTerminal::Session(_)) => {
                unreachable!("mob stream stored session terminal metadata")
            }
            None => self.closed_mob_streams.lock().await.remove(&stream_id),
        };

        RpcResponse::success(
            id,
            json!({
                "stream_id": stream_id.to_string(),
                "closed": true,
                "already_closed": already_closed,
            }),
        )
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    use std::pin::Pin;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use async_trait::async_trait;
    use futures::stream;
    use meerkat::AgentFactory;
    use meerkat::surface::{RequestContext, SurfaceRequestExecutor, noop_request_action};
    use meerkat_client::{LlmClient, LlmError};
    use meerkat_core::skills::{
        SkillKey, SkillKeyRemap, SkillName, SourceIdentityLineage, SourceIdentityLineageEvent,
        SourceIdentityRecord, SourceIdentityRegistry, SourceIdentityStatus, SourceTransportKind,
        SourceUuid,
    };
    use meerkat_core::types::ContentInput;
    use meerkat_core::{
        Config, ConfigRuntime, MemoryConfigStore, Message, StopReason, ToolCallView, ToolDef,
        ToolDispatchOutcome, ToolError,
    };
    use serde::Serialize;
    use serde_json::value::RawValue;

    use crate::protocol::RpcId;

    #[cfg(feature = "mob")]
    struct StaticDispatcher {
        tools: Arc<[Arc<ToolDef>]>,
    }

    #[cfg(feature = "mob")]
    impl StaticDispatcher {
        fn new(name: &str) -> Self {
            let tool = Arc::new(ToolDef {
                name: name.into(),
                description: format!("tool {name}"),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
                }),
                provenance: None,
            });
            Self {
                tools: Arc::from([tool]),
            }
        }
    }

    #[cfg(feature = "mob")]
    #[async_trait]
    impl AgentToolDispatcher for StaticDispatcher {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            self.tools.clone()
        }

        async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
            Err(ToolError::not_found(call.name))
        }
    }

    #[cfg(feature = "mob")]
    struct RouterFailClearEventStore {
        inner: meerkat_mob::store::InMemoryMobEventStore,
        fail_clear: AtomicBool,
        fail_member_retired: AtomicBool,
    }

    #[cfg(feature = "mob")]
    impl RouterFailClearEventStore {
        fn new() -> Self {
            Self {
                inner: meerkat_mob::store::InMemoryMobEventStore::new(),
                fail_clear: AtomicBool::new(true),
                fail_member_retired: AtomicBool::new(false),
            }
        }

        fn allow_clear(&self) {
            self.fail_clear.store(false, Ordering::Relaxed);
        }

        fn fail_member_retired_appends(&self) {
            self.fail_member_retired.store(true, Ordering::Relaxed);
        }
    }

    #[cfg(feature = "mob")]
    #[async_trait]
    impl meerkat_mob::store::MobEventStore for RouterFailClearEventStore {
        async fn append(
            &self,
            event: meerkat_mob::NewMobEvent,
        ) -> Result<meerkat_mob::MobEvent, meerkat_mob::store::MobStoreError> {
            if self.fail_member_retired.load(Ordering::Relaxed)
                && matches!(event.kind, meerkat_mob::MobEventKind::MemberRetired { .. })
            {
                return Err(meerkat_mob::store::MobStoreError::Internal(
                    "forced router mob retire event failure".to_string(),
                ));
            }
            self.inner.append(event).await
        }

        async fn append_terminal_event_if_absent(
            &self,
            event: meerkat_mob::NewMobEvent,
        ) -> Result<Option<meerkat_mob::MobEvent>, meerkat_mob::store::MobStoreError> {
            self.inner.append_terminal_event_if_absent(event).await
        }

        async fn append_batch(
            &self,
            events: Vec<meerkat_mob::NewMobEvent>,
        ) -> Result<Vec<meerkat_mob::MobEvent>, meerkat_mob::store::MobStoreError> {
            self.inner.append_batch(events).await
        }

        async fn poll(
            &self,
            after_cursor: u64,
            limit: usize,
        ) -> Result<Vec<meerkat_mob::MobEvent>, meerkat_mob::store::MobStoreError> {
            self.inner.poll(after_cursor, limit).await
        }

        async fn replay_all(
            &self,
        ) -> Result<Vec<meerkat_mob::MobEvent>, meerkat_mob::store::MobStoreError> {
            self.inner.replay_all().await
        }

        async fn latest_cursor(&self) -> Result<u64, meerkat_mob::store::MobStoreError> {
            self.inner.latest_cursor().await
        }

        fn subscribe(
            &self,
        ) -> Result<meerkat_mob::store::MobEventReceiver, meerkat_mob::store::MobStoreError>
        {
            self.inner.subscribe()
        }

        async fn clear(&self) -> Result<(), meerkat_mob::store::MobStoreError> {
            if self.fail_clear.load(Ordering::Relaxed) {
                return Err(meerkat_mob::store::MobStoreError::Internal(
                    "forced router archive mob destroy clear failure".to_string(),
                ));
            }
            self.inner.clear().await
        }
    }

    #[cfg(feature = "mob")]
    #[test]
    fn rpc_mob_external_tools_keep_callbacks_and_configured_mcp_tools() {
        let callback_tools: Arc<dyn AgentToolDispatcher> =
            Arc::new(StaticDispatcher::new("callback_tool"));
        let configured_tools: Arc<dyn AgentToolDispatcher> =
            Arc::new(StaticDispatcher::new("linear_add_comment"));

        let merged = compose_rpc_mob_external_tools(Some(callback_tools), Some(configured_tools))
            .expect("merged dispatcher");
        let names: std::collections::BTreeSet<String> = merged
            .tools()
            .iter()
            .map(|tool| tool.name.to_string())
            .collect();

        assert!(names.contains("callback_tool"));
        assert!(names.contains("linear_add_comment"));
    }

    #[cfg(feature = "mob")]
    #[test]
    fn rpc_mob_external_tools_keep_configured_mcp_without_callback_transport() {
        let configured_tools: Arc<dyn AgentToolDispatcher> =
            Arc::new(StaticDispatcher::new("linear_upsert_workpad"));

        let merged = compose_rpc_mob_external_tools(None, Some(configured_tools))
            .expect("configured MCP dispatcher must remain visible");
        let names: std::collections::BTreeSet<String> = merged
            .tools()
            .iter()
            .map(|tool| tool.name.to_string())
            .collect();

        assert!(
            names.contains("linear_upsert_workpad"),
            "RPC mob members launched over TCP may not have a callback transport, \
             but configured MCP tools from .rkat/mcp.toml must still be exposed"
        );
    }

    #[cfg(all(feature = "mob", feature = "mcp"))]
    #[test]
    fn rpc_mob_external_tools_provider_holds_configured_mcp_keepalive_until_provider_drop() {
        let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        let keepalive = Arc::new(ConfiguredRpcMobMcpKeepalive {
            shutdown_tx: std::sync::Mutex::new(Some(shutdown_tx)),
            join: None,
        });

        let provider = rpc_mob_external_tools_provider_from_parts(
            Arc::new(|| None),
            None,
            Some(keepalive.clone()),
        );

        drop(keepalive);
        assert!(
            matches!(
                shutdown_rx.try_recv(),
                Err(tokio::sync::oneshot::error::TryRecvError::Empty)
            ),
            "dropping the caller's keepalive Arc must not shut down configured MCP transports while the provider is alive"
        );

        drop(provider);
        assert!(
            shutdown_rx.try_recv().is_ok(),
            "dropping the provider should release the captured MCP keepalive guard"
        );
    }

    #[cfg(all(feature = "mob", feature = "mcp"))]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn rpc_mob_configured_mcp_tools_remain_callable_after_loader_returns() {
        let temp = tempfile::tempdir().unwrap();
        let rkat_dir = temp.path().join(".rkat");
        std::fs::create_dir_all(&rkat_dir).unwrap();
        let server_path = temp.path().join("echo_mcp.py");
        std::fs::write(
            &server_path,
            r#"#!/usr/bin/env python3
import json
import sys

for line in sys.stdin:
    if not line.strip():
        continue
    request = json.loads(line)
    if "id" not in request:
        continue
    method = request.get("method")
    if method == "initialize":
        result = {
            "protocolVersion": "2024-11-05",
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "echo-mcp", "version": "test"},
        }
    elif method == "tools/list":
        result = {
            "tools": [{
                "name": "echo",
                "description": "Echoes the input message",
                "inputSchema": {
                    "type": "object",
                    "properties": {"message": {"type": "string"}},
                    "required": ["message"],
                },
            }]
        }
    elif method == "tools/call":
        arguments = request.get("params", {}).get("arguments", {})
        result = {
            "content": [{
                "type": "text",
                "text": arguments.get("message", ""),
            }],
            "isError": False,
        }
    else:
        print(json.dumps({
            "jsonrpc": "2.0",
            "id": request["id"],
            "error": {"code": -32601, "message": "not found"},
        }), flush=True)
        continue
    print(json.dumps({"jsonrpc": "2.0", "id": request["id"], "result": result}), flush=True)
"#,
        )
        .unwrap();
        std::fs::write(
            rkat_dir.join("mcp.toml"),
            format!(
                r#"[[servers]]
name = "echo-server"
command = "python3"
args = [{}]
"#,
                serde_json::to_string(server_path.to_str().unwrap()).unwrap()
            ),
        )
        .unwrap();

        let bundle = load_configured_mcp_tools_for_rpc_mob(Some(temp.path().to_path_buf()), None)
            .expect("configured MCP dispatcher");
        let LoadedRpcMobMcp {
            dispatcher,
            keepalive,
        } = bundle;
        assert!(
            dispatcher.tools().iter().any(|tool| tool.name == "echo"),
            "configured MCP tool should be visible before the mob member turn"
        );

        let args = RawValue::from_string(r#"{"message":"runtime is alive"}"#.to_string()).unwrap();
        let outcome = dispatcher
            .dispatch(ToolCallView {
                id: "call-1",
                name: "echo",
                args: &args,
            })
            .await
            .expect("configured MCP tool call should remain callable after loader returns");

        assert_eq!(outcome.result.text_content(), "runtime is alive");

        drop(dispatcher);
        keepalive.shutdown_and_join();
    }

    #[cfg(all(feature = "mob", feature = "mcp"))]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn rpc_mob_configured_mcp_keepalive_thread_terminates_on_drop() {
        let temp = tempfile::tempdir().unwrap();
        let rkat_dir = temp.path().join(".rkat");
        std::fs::create_dir_all(&rkat_dir).unwrap();
        let server_path = temp.path().join("server.py");
        std::fs::write(
            &server_path,
            r#"#!/usr/bin/env python3
import json, sys
for line in sys.stdin:
    request = json.loads(line)
    method = request.get("method")
    if method == "initialize":
        print(json.dumps({"jsonrpc": "2.0", "id": request["id"], "result": {
            "protocolVersion": "2024-11-05",
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "echo-server", "version": "1"}
        }}), flush=True)
    elif method == "notifications/initialized":
        pass
    elif method == "tools/list":
        print(json.dumps({"jsonrpc": "2.0", "id": request["id"], "result": {
            "tools": [{"name": "echo", "description": "echoes", "inputSchema": {"type": "object"}}]
        }}), flush=True)
    else:
        print(json.dumps({"jsonrpc": "2.0", "id": request["id"], "error": {"code": -32601, "message": "not found"}}), flush=True)
"#,
        )
        .unwrap();
        std::fs::write(
            rkat_dir.join("mcp.toml"),
            format!(
                r#"[[servers]]
name = "echo-server"
command = "python3"
args = [{}]
"#,
                serde_json::to_string(server_path.to_str().unwrap()).unwrap()
            ),
        )
        .unwrap();

        let bundle = load_configured_mcp_tools_for_rpc_mob(Some(temp.path().to_path_buf()), None)
            .expect("configured MCP dispatcher");
        let LoadedRpcMobMcp {
            dispatcher,
            keepalive,
        } = bundle;

        // Drop the dispatcher first; then signal + join the keepalive thread.
        // If shutdown signalling were missing, the thread would park on
        // `pending()` forever and `join` would hang the test (well past the
        // tokio runtime worker timeouts).
        drop(dispatcher);
        let join_started = std::time::Instant::now();
        let keepalive_join = std::thread::spawn(move || keepalive.shutdown_and_join());
        while !keepalive_join.is_finished() {
            assert!(
                join_started.elapsed() < std::time::Duration::from_secs(10),
                "MCP keepalive thread did not terminate within 10s of shutdown signal"
            );
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        keepalive_join
            .join()
            .expect("keepalive shutdown thread must not panic");
    }

    #[cfg(feature = "comms")]
    #[test]
    fn test_router_send_receipt_json_peer_request_uses_envelope_id_as_request_id() {
        let envelope_id = uuid::Uuid::new_v4();
        let interaction_id = meerkat_core::interaction::InteractionId(uuid::Uuid::new_v4());

        let payload = send_receipt_json(meerkat_core::comms::SendReceipt::PeerRequestSent {
            envelope_id,
            interaction_id,
            stream_reserved: true,
        });

        assert_eq!(
            payload["request_id"],
            serde_json::json!(envelope_id.to_string())
        );
        assert_eq!(
            payload["interaction_id"],
            serde_json::json!(interaction_id.0.to_string())
        );
    }

    #[cfg(feature = "comms")]
    fn install_ephemeral_peer_request_response_authority(
        runtime: &Arc<meerkat::CommsRuntime>,
        session: &str,
    ) {
        let dsl = Arc::new(meerkat_runtime::HandleDslAuthority::ephemeral());
        dsl.apply_signal(
            meerkat_runtime::meerkat_machine::dsl::MeerkatMachineSignal::Initialize,
            "test::initialize",
        )
        .expect("Initialize");
        dsl.apply_input(
            meerkat_runtime::meerkat_machine::dsl::MeerkatMachineInput::RegisterSession {
                session_id: meerkat_runtime::meerkat_machine::dsl::SessionId::from(
                    session.to_string(),
                ),
            },
            "test::register_session",
        )
        .expect("RegisterSession");

        runtime.install_peer_request_response_authority(
            meerkat_comms::PeerRequestResponseAuthority::new(
                Arc::new(meerkat_runtime::RuntimePeerInteractionHandle::new(
                    Arc::clone(&dsl),
                )),
                Arc::new(meerkat_runtime::RuntimeInteractionStreamHandle::new(dsl)),
            ),
        );
    }

    // -----------------------------------------------------------------------
    // Mock LLM client (same as session_runtime tests)
    // -----------------------------------------------------------------------

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

    struct RecordingMockLlmClient {
        requests: Arc<std::sync::Mutex<Vec<Vec<Message>>>>,
        delay_ms: Option<u64>,
    }

    impl RecordingMockLlmClient {
        fn new(requests: Arc<std::sync::Mutex<Vec<Vec<Message>>>>) -> Self {
            Self {
                requests,
                delay_ms: None,
            }
        }

        fn with_delay(requests: Arc<std::sync::Mutex<Vec<Vec<Message>>>>, delay_ms: u64) -> Self {
            Self {
                requests,
                delay_ms: Some(delay_ms),
            }
        }
    }

    #[async_trait]
    impl LlmClient for RecordingMockLlmClient {
        fn project_replay_messages(
            &self,
            messages: &[meerkat_core::Message],
        ) -> Result<Vec<meerkat_core::Message>, meerkat_client::LlmError> {
            Ok(messages.to_vec())
        }

        fn stream<'a>(
            &'a self,
            request: &'a meerkat_client::LlmRequest,
        ) -> Pin<
            Box<dyn futures::Stream<Item = Result<meerkat_client::LlmEvent, LlmError>> + Send + 'a>,
        > {
            self.requests
                .lock()
                .expect("recorded requests lock poisoned")
                .push(request.messages.clone());
            let delay = self.delay_ms;
            Box::pin(stream::unfold(0u8, move |state| async move {
                match state {
                    0 => {
                        if let Some(delay_ms) = delay {
                            tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                        }
                        Some((
                            Ok(meerkat_client::LlmEvent::TextDelta {
                                delta: "Hello from mock".to_string(),
                                meta: None,
                            }),
                            1,
                        ))
                    }
                    1 => Some((
                        Ok(meerkat_client::LlmEvent::Done {
                            outcome: meerkat_client::LlmDoneOutcome::Success {
                                stop_reason: StopReason::EndTurn,
                            },
                        }),
                        2,
                    )),
                    _ => None,
                }
            }))
        }

        fn provider(&self) -> &'static str {
            "mock"
        }

        async fn health_check(&self) -> Result<(), LlmError> {
            Ok(())
        }
    }

    // -----------------------------------------------------------------------
    // Test helpers
    // -----------------------------------------------------------------------

    fn memory_blob_store() -> Arc<dyn meerkat_core::BlobStore> {
        Arc::new(meerkat_store::MemoryBlobStore::new())
    }

    async fn test_router() -> (MethodRouter, mpsc::Receiver<RpcNotification>) {
        let temp = tempfile::tempdir().unwrap();
        let factory = AgentFactory::new(temp.path().join("sessions"));
        let config = Config::default();
        let store: Arc<dyn meerkat::SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let mut runtime = SessionRuntime::new(
            factory,
            config,
            10,
            meerkat::PersistenceBundle::new(store, None, memory_blob_store()),
            NotificationSink::noop(),
        );
        let config_store: Arc<dyn ConfigStore> =
            Arc::new(MemoryConfigStore::new(Config::default()));
        runtime.set_default_llm_client(Some(Arc::new(MockLlmClient)));
        runtime.set_config_runtime(Arc::new(ConfigRuntime::new(
            Arc::clone(&config_store),
            temp.path().join("config_state.json"),
        )));
        let runtime = Arc::new(runtime);
        let (notif_tx, notif_rx) = mpsc::channel(100);
        let sink = NotificationSink::new(notif_tx);
        let router = MethodRouter::new(runtime, config_store, sink);
        (router, notif_rx)
    }

    async fn test_router_with_v9_runtime() -> (MethodRouter, mpsc::Receiver<RpcNotification>) {
        let (router, notif_rx) = test_router().await;
        let runtime_adapter = Arc::new(meerkat_runtime::MeerkatMachine::persistent(
            Arc::new(meerkat_runtime::InMemoryRuntimeStore::new()),
            memory_blob_store(),
        ));
        (router.with_runtime_adapter(runtime_adapter), notif_rx)
    }

    async fn test_router_with_v9_runtime_and_max_sessions(
        max_sessions: usize,
    ) -> (MethodRouter, mpsc::Receiver<RpcNotification>) {
        let temp = tempfile::tempdir().unwrap();
        let factory = AgentFactory::new(temp.path().join("sessions"));
        let mut config = Config::default();
        config.limits.max_sessions = Some(max_sessions);
        let store: Arc<dyn meerkat::SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let mut runtime = SessionRuntime::new(
            factory,
            config.clone(),
            max_sessions,
            meerkat::PersistenceBundle::new(store, None, memory_blob_store()),
            NotificationSink::noop(),
        );
        let config_store: Arc<dyn ConfigStore> = Arc::new(MemoryConfigStore::new(config));
        runtime.set_default_llm_client(Some(Arc::new(MockLlmClient)));
        runtime.set_config_runtime(Arc::new(ConfigRuntime::new(
            Arc::clone(&config_store),
            temp.path().join("config_state.json"),
        )));
        let runtime = Arc::new(runtime);
        let (notif_tx, notif_rx) = mpsc::channel(100);
        let sink = NotificationSink::new(notif_tx);
        let router = MethodRouter::new(runtime, config_store, sink);
        let runtime_adapter = Arc::new(meerkat_runtime::MeerkatMachine::persistent(
            Arc::new(meerkat_runtime::InMemoryRuntimeStore::new()),
            memory_blob_store(),
        ));
        (router.with_runtime_adapter(runtime_adapter), notif_rx)
    }

    #[tokio::test]
    async fn runtime_host_info_reports_projection_without_topology_authority() {
        let (router, _rx) = test_router_with_v9_runtime().await;
        let response = router
            .dispatch(RpcRequest {
                jsonrpc: "2.0".to_string(),
                method: "runtime/host_info".to_string(),
                params: None,
                id: Some(RpcId::Num(1)),
            })
            .await
            .expect("response");
        let value = serde_json::to_value(response).expect("response serializes");
        let result = &value["result"];

        assert!(
            result["host_id"]
                .as_str()
                .is_some_and(|id| id.starts_with("process:")),
            "default host id should be process-scoped: {result}"
        );
        assert_eq!(result["host_id_scope"], "process");
        assert_eq!(
            result["capabilities"]["features"]["runtime_backed_sessions"],
            true
        );
        assert_eq!(result["capabilities"]["features"]["event_replay"], false);

        let text = serde_json::to_string(result).expect("result serializes");
        for forbidden in ["topology", "registry", "lease", "claim", "project"] {
            assert!(
                !text.contains(forbidden),
                "runtime host projection must not claim topology authority token `{forbidden}`: {text}"
            );
        }
    }

    #[tokio::test]
    async fn runtime_host_capabilities_and_health_are_read_only_projections() {
        let (router, _rx) = test_router_with_v9_runtime().await;
        let capabilities = router
            .dispatch(RpcRequest {
                jsonrpc: "2.0".to_string(),
                method: "runtime/capabilities".to_string(),
                params: None,
                id: Some(RpcId::Num(1)),
            })
            .await
            .expect("capabilities response");
        let health = router
            .dispatch(RpcRequest {
                jsonrpc: "2.0".to_string(),
                method: "runtime/health".to_string(),
                params: None,
                id: Some(RpcId::Num(2)),
            })
            .await
            .expect("health response");

        let capabilities = serde_json::to_value(capabilities).expect("serialize capabilities");
        let health = serde_json::to_value(health).expect("serialize health");
        assert_eq!(
            capabilities["result"]["features"]["runtime_backed_sessions"],
            true
        );
        assert_eq!(
            capabilities["result"]["features"]["secure_remote_rpc"],
            false
        );
        assert_eq!(capabilities["result"]["features"]["approvals"], false);
        assert_eq!(health["result"]["status"], "ok");
    }

    #[tokio::test]
    async fn approval_request_get_list_and_decide_round_trip() {
        let (router, _rx) = test_router().await;
        let request = make_request(
            "approval/request",
            serde_json::json!({
                "requester": "human:alice",
                "owner": {"owner_type": "session", "session_id": "session-1"},
                "resource": {"kind": "shell_command", "id": "shell:rm"},
                "proposed_action": {
                    "kind": "shell_command",
                    "summary": "run destructive shell command",
                    "body": {"cmd": "rm -rf target/tmp"}
                },
                "risk": "high",
                "request_body": {"why": "cleanup"},
                "allowed_decisions": ["approve", "deny"],
                "metadata": {
                    "labels": {"client.thread_id": "thread-1"},
                    "app_context": {"client_ref": "opaque"}
                },
                "request_provenance": {"tool_call_id": "call-1"}
            }),
        );

        let created = router.dispatch(request).await.expect("response");
        let created = result_value(&created);
        let approval_id = created["approval_id"]
            .as_str()
            .expect("approval id")
            .to_string();
        assert_eq!(created["status"], "pending");
        assert_eq!(created["request_provenance"]["tool_call_id"], "call-1");

        let listed = router
            .dispatch(make_request(
                "approval/list",
                serde_json::json!({"filter": {"status": "pending"}}),
            ))
            .await
            .expect("list response");
        let listed = result_value(&listed);
        assert_eq!(listed["approvals"].as_array().expect("approvals").len(), 1);

        let decided = router
            .dispatch(make_request(
                "approval/decide",
                serde_json::json!({
                    "approval_id": approval_id,
                    "decision": "approve",
                    "actor": "human:bob",
                    "reason": "reviewed",
                    "provenance": {"client": "mobile"}
                }),
            ))
            .await
            .expect("decide response");
        let decided = result_value(&decided);
        assert_eq!(decided["status"], "approved");
        assert_eq!(decided["decision"]["actor"], "human:bob");
        assert_eq!(decided["decision"]["provenance"]["client"], "mobile");
        assert_eq!(decided["request_provenance"]["tool_call_id"], "call-1");

        let fetched = router
            .dispatch(make_request(
                "approval/get",
                serde_json::json!({"approval_id": decided["approval_id"]}),
            ))
            .await
            .expect("get response");
        let fetched = result_value(&fetched);
        assert_eq!(fetched["status"], "approved");
    }

    #[tokio::test]
    async fn approval_decide_rejects_duplicate_decisions() {
        let (router, _rx) = test_router().await;
        let created = router
            .dispatch(make_request(
                "approval/request",
                serde_json::json!({
                    "requester": "human:alice",
                    "owner": {"owner_type": "runtime"},
                    "resource": {"kind": "runtime", "id": "local"},
                    "proposed_action": {"kind": "other", "summary": "manual gate"},
                    "risk": "medium",
                    "allowed_decisions": ["deny"]
                }),
            ))
            .await
            .expect("request response");
        let approval_id = result_value(&created)["approval_id"]
            .as_str()
            .expect("approval id")
            .to_string();

        let first = router
            .dispatch(make_request(
                "approval/decide",
                serde_json::json!({
                    "approval_id": approval_id,
                    "decision": "deny",
                    "actor": "human:bob"
                }),
            ))
            .await
            .expect("first decision");
        assert!(first.error.is_none());

        let duplicate = router
            .dispatch(make_request(
                "approval/decide",
                serde_json::json!({
                    "approval_id": approval_id,
                    "decision": "deny",
                    "actor": "human:bob"
                }),
            ))
            .await
            .expect("duplicate decision");
        assert_eq!(error_code(&duplicate), error::INVALID_PARAMS);
        assert!(error_message(&duplicate).contains("already been decided"));
    }

    #[tokio::test]
    async fn approval_request_rejects_reserved_metadata_spoofing() {
        let (router, _rx) = test_router().await;
        let response = router
            .dispatch(make_request(
                "approval/request",
                serde_json::json!({
                    "requester": "human:alice",
                    "owner": {"owner_type": "runtime"},
                    "resource": {"kind": "runtime", "id": "local"},
                    "proposed_action": {"kind": "other", "summary": "manual gate"},
                    "risk": "medium",
                    "allowed_decisions": ["approve", "deny"],
                    "metadata": {"labels": {"meerkat.approval_id": "spoof"}}
                }),
            ))
            .await
            .expect("response");
        assert_eq!(error_code(&response), error::INVALID_PARAMS);
        assert!(error_message(&response).contains("reserved"));
    }

    async fn test_router_with_llm(
        llm_client: Arc<dyn LlmClient>,
    ) -> (MethodRouter, mpsc::Receiver<RpcNotification>) {
        let temp = tempfile::tempdir().unwrap();
        let factory = AgentFactory::new(temp.path().join("sessions"));
        let config = Config::default();
        let store: Arc<dyn meerkat::SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let mut runtime = SessionRuntime::new(
            factory,
            config,
            10,
            meerkat::PersistenceBundle::new(store, None, memory_blob_store()),
            NotificationSink::noop(),
        );
        let config_store: Arc<dyn ConfigStore> =
            Arc::new(MemoryConfigStore::new(Config::default()));
        runtime.set_default_llm_client(Some(llm_client));
        runtime.set_config_runtime(Arc::new(ConfigRuntime::new(
            Arc::clone(&config_store),
            temp.path().join("config_state.json"),
        )));
        let runtime = Arc::new(runtime);
        let (notif_tx, notif_rx) = mpsc::channel(100);
        let sink = NotificationSink::new(notif_tx);
        let router = MethodRouter::new(runtime, config_store, sink);
        (router, notif_rx)
    }

    async fn test_router_with_llm_and_notification_capacity(
        llm_client: Arc<dyn LlmClient>,
        notification_capacity: usize,
    ) -> (MethodRouter, mpsc::Receiver<RpcNotification>) {
        let temp = tempfile::tempdir().unwrap();
        let factory = AgentFactory::new(temp.path().join("sessions"));
        let config = Config::default();
        let store: Arc<dyn meerkat::SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let mut runtime = SessionRuntime::new(
            factory,
            config,
            10,
            meerkat::PersistenceBundle::new(store, None, memory_blob_store()),
            NotificationSink::noop(),
        );
        let config_store: Arc<dyn ConfigStore> =
            Arc::new(MemoryConfigStore::new(Config::default()));
        runtime.set_default_llm_client(Some(llm_client));
        runtime.set_config_runtime(Arc::new(ConfigRuntime::new(
            Arc::clone(&config_store),
            temp.path().join("config_state.json"),
        )));
        let runtime = Arc::new(runtime);
        let (notif_tx, notif_rx) = mpsc::channel(notification_capacity);
        let sink = NotificationSink::new(notif_tx);
        let router = MethodRouter::new(runtime, config_store, sink);
        (router, notif_rx)
    }

    #[cfg(feature = "mob")]
    async fn test_router_with_mob_state(
        mob_state: Arc<meerkat_mob_mcp::MobMcpState>,
    ) -> (MethodRouter, mpsc::Receiver<RpcNotification>) {
        let temp = tempfile::tempdir().unwrap();
        let factory = AgentFactory::new(temp.path().join("sessions"));
        let config = Config::default();
        let store: Arc<dyn meerkat::SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let mut runtime = SessionRuntime::new(
            factory,
            config,
            10,
            meerkat::PersistenceBundle::new(store, None, memory_blob_store()),
            NotificationSink::noop(),
        );
        let config_store: Arc<dyn ConfigStore> =
            Arc::new(MemoryConfigStore::new(Config::default()));
        runtime.set_default_llm_client(Some(Arc::new(MockLlmClient)));
        runtime.set_config_runtime(Arc::new(ConfigRuntime::new(
            Arc::clone(&config_store),
            temp.path().join("config_state.json"),
        )));
        let runtime = Arc::new(runtime);
        let (notif_tx, notif_rx) = mpsc::channel(100);
        let sink = NotificationSink::new(notif_tx);
        let router = MethodRouter::new_with_mob_state(runtime, config_store, sink, mob_state);
        (router, notif_rx)
    }

    #[cfg(feature = "mob")]
    async fn insert_router_archive_partial_destroy_mob(
        mob_state: &Arc<meerkat_mob_mcp::MobMcpState>,
        owner_session_id: &str,
    ) -> (meerkat_mob::MobId, Arc<RouterFailClearEventStore>) {
        let mob_id = meerkat_mob::MobId::from("router-session-archive-partial-destroy");
        let mut profiles = std::collections::BTreeMap::new();
        profiles.insert(
            meerkat_mob::ProfileName::from("worker"),
            meerkat_mob::ProfileBinding::Inline(meerkat_mob::Profile {
                model: "claude-sonnet-4-5".to_string(),
                skills: Vec::new(),
                tools: meerkat_mob::ToolConfig {
                    comms: true,
                    ..meerkat_mob::ToolConfig::default()
                },
                peer_description: "worker".to_string(),
                external_addressable: false,
                backend: None,
                runtime_mode: meerkat_mob::MobRuntimeMode::TurnDriven,
                max_inline_peer_notifications: None,
                output_schema: None,
                provider_params: None,
            }),
        );
        let mut definition = meerkat_mob::MobDefinition::explicit(mob_id.clone());
        definition.profiles = profiles;
        definition.mark_owner_bridge_session_indexed(owner_session_id);
        let events = Arc::new(RouterFailClearEventStore::new());
        let storage = meerkat_mob::MobStorage::with_events(events.clone());
        let handle = meerkat_mob::MobBuilder::new(definition, storage)
            .with_session_service(mob_state.session_service())
            .allow_ephemeral_sessions(true)
            .create()
            .await
            .expect("create archive-owned mob with failing event clear");
        mob_state.mob_insert_handle(mob_id.clone(), handle).await;
        (mob_id, events)
    }

    #[cfg(feature = "mob")]
    async fn insert_router_archive_live_member_with_optional_retire_event_failure(
        mob_state: &Arc<meerkat_mob_mcp::MobMcpState>,
        fail_member_retired_append: bool,
    ) -> (
        meerkat_mob::MobId,
        SessionId,
        Arc<RouterFailClearEventStore>,
    ) {
        let mob_id = meerkat_mob::MobId::from("router-session-archive-live-retire-failure");
        let mut profiles = std::collections::BTreeMap::new();
        profiles.insert(
            meerkat_mob::ProfileName::from("worker"),
            meerkat_mob::ProfileBinding::Inline(meerkat_mob::Profile {
                model: "claude-sonnet-4-5".to_string(),
                skills: Vec::new(),
                tools: meerkat_mob::ToolConfig {
                    comms: true,
                    ..meerkat_mob::ToolConfig::default()
                },
                peer_description: "worker".to_string(),
                external_addressable: false,
                backend: None,
                runtime_mode: meerkat_mob::MobRuntimeMode::TurnDriven,
                max_inline_peer_notifications: None,
                output_schema: None,
                provider_params: None,
            }),
        );
        let mut definition = meerkat_mob::MobDefinition::explicit(mob_id.clone());
        definition.profiles = profiles;
        let events = Arc::new(RouterFailClearEventStore::new());
        events.allow_clear();
        let storage = meerkat_mob::MobStorage::with_events(events.clone());
        let handle = meerkat_mob::MobBuilder::new(definition, storage)
            .with_session_service(mob_state.session_service())
            .allow_ephemeral_sessions(true)
            .create()
            .await
            .expect("create live mob for archive failure");
        let identity = meerkat_mob::AgentIdentity::from("worker-1");
        handle
            .spawn_spec(meerkat_mob::SpawnMemberSpec::new(
                meerkat_mob::ProfileName::from("worker"),
                identity.clone(),
            ))
            .await
            .expect("spawn live member for archive failure");
        let bridge_session_id = handle
            .resolve_bridge_session_id(&identity)
            .await
            .expect("turn-driven worker should have a bridge session");
        if fail_member_retired_append {
            events.fail_member_retired_appends();
        }
        mob_state.mob_insert_handle(mob_id.clone(), handle).await;
        (mob_id, bridge_session_id, events)
    }

    async fn test_router_with_registry(
        registry: SourceIdentityRegistry,
    ) -> (MethodRouter, mpsc::Receiver<RpcNotification>) {
        let temp = tempfile::tempdir().unwrap();
        let factory = AgentFactory::new(temp.path().join("sessions"));
        let config = Config::default();
        let store: Arc<dyn meerkat::SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let mut runtime = SessionRuntime::new(
            factory,
            config,
            10,
            meerkat::PersistenceBundle::new(store, None, memory_blob_store()),
            NotificationSink::noop(),
        );
        let config_store: Arc<dyn ConfigStore> =
            Arc::new(MemoryConfigStore::new(Config::default()));
        runtime.set_default_llm_client(Some(Arc::new(MockLlmClient)));
        runtime.set_config_runtime(Arc::new(ConfigRuntime::new(
            Arc::clone(&config_store),
            temp.path().join("config_state.json"),
        )));
        runtime.set_skill_identity_registry(registry);
        let runtime = Arc::new(runtime);
        let (notif_tx, notif_rx) = mpsc::channel(100);
        let sink = NotificationSink::new(notif_tx);
        let router = MethodRouter::new(runtime, config_store, sink);
        (router, notif_rx)
    }

    /// Resolve the bridge session ID for a mob member via the mob state.
    ///
    /// Tests that need a session_id for routing should use this helper instead
    /// of reading session_id from spawn results (which no longer carry it).
    #[cfg(feature = "mob")]
    async fn resolve_mob_bridge_session_id(
        router: &MethodRouter,
        mob_id: &str,
        agent_identity: &str,
    ) -> String {
        let mob_id = meerkat_mob::MobId::from(mob_id);
        let identity = meerkat_mob::AgentIdentity::from(agent_identity);
        let handle = router.mob_state().handle_for(&mob_id).await.unwrap();
        handle
            .resolve_bridge_session_id(&identity)
            .await
            .expect("member should have a bridge session binding")
            .to_string()
    }

    #[tokio::test]
    async fn blob_get_returns_payload() {
        let (router, _rx) = test_router().await;
        let blob_ref = router
            .runtime
            .blob_store()
            .put_image("image/png", "aGVsbG8=")
            .await
            .expect("blob stored");

        let response = router
            .dispatch(crate::protocol::RpcRequest {
                jsonrpc: "2.0".to_string(),
                id: Some(crate::protocol::RpcId::Num(1)),
                method: "blob/get".to_string(),
                params: Some(
                    serde_json::value::to_raw_value(
                        &serde_json::json!({ "blob_id": blob_ref.blob_id.as_str() }),
                    )
                    .expect("raw value"),
                ),
            })
            .await;

        let response = response.expect("response");
        assert!(response.error.is_none(), "blob/get should succeed");
        let result: serde_json::Value =
            serde_json::from_str(response.result.expect("blob payload").get())
                .expect("valid blob payload");
        assert_eq!(result["blob_id"], blob_ref.blob_id.as_str());
        assert_eq!(result["media_type"], "image/png");
        assert_eq!(result["data"], "aGVsbG8=");
    }

    fn test_artifact_record(
        artifact_id: &str,
        blob_ref: meerkat_core::BlobRef,
    ) -> meerkat_core::ArtifactRecord {
        let mut record = meerkat_core::ArtifactRecord::new(
            meerkat_core::ArtifactId::new(artifact_id).unwrap(),
            meerkat_core::ArtifactType::Json,
            "Report".to_string(),
            "application/json".to_string(),
            2,
            Some("sha256:test-report".to_string()),
            meerkat_core::ArtifactContentHandle::Blob(blob_ref),
        )
        .unwrap();
        record.owner.session_id = Some("session-a".to_string());
        record
            .metadata
            .labels
            .insert("client.thread_id".to_string(), "thread-a".to_string());
        record
    }

    async fn seed_test_artifact(router: &MethodRouter) -> meerkat_core::ArtifactRecord {
        let blob_ref = router
            .runtime
            .blob_store()
            .put_image("application/json", "e30=")
            .await
            .expect("blob stored");
        let record = test_artifact_record("artifact-1", blob_ref);
        router
            .runtime
            .artifact_store()
            .put(record.clone())
            .await
            .expect("artifact stored");
        record
    }

    #[tokio::test]
    async fn artifact_list_and_get_return_stable_records() {
        let (router, _rx) = test_router().await;
        seed_test_artifact(&router).await;

        let list_response = router
            .dispatch(crate::protocol::RpcRequest {
                jsonrpc: "2.0".to_string(),
                id: Some(crate::protocol::RpcId::Num(1)),
                method: "artifact/list".to_string(),
                params: Some(
                    serde_json::value::to_raw_value(&serde_json::json!({
                        "session_id": "session-a",
                        "label_equals": {"client.thread_id": "thread-a"}
                    }))
                    .expect("raw value"),
                ),
            })
            .await
            .expect("response");
        assert!(
            list_response.error.is_none(),
            "artifact/list should succeed: {:?}",
            list_response.error
        );
        let list_result: serde_json::Value =
            serde_json::from_str(list_response.result.unwrap().get()).unwrap();
        assert_eq!(list_result["artifacts"][0]["artifact_id"], "artifact-1");
        assert!(!list_result.to_string().contains("/tmp"));
        assert!(!list_result.to_string().contains("path"));

        let get_response = router
            .dispatch(crate::protocol::RpcRequest {
                jsonrpc: "2.0".to_string(),
                id: Some(crate::protocol::RpcId::Num(2)),
                method: "artifact/get".to_string(),
                params: Some(
                    serde_json::value::to_raw_value(
                        &serde_json::json!({ "artifact_id": "artifact-1" }),
                    )
                    .expect("raw value"),
                ),
            })
            .await
            .expect("response");
        assert!(get_response.error.is_none(), "artifact/get should succeed");
        let get_result: serde_json::Value =
            serde_json::from_str(get_response.result.unwrap().get()).unwrap();
        assert_eq!(get_result["artifact_id"], "artifact-1");
    }

    #[tokio::test]
    async fn artifact_download_uses_artifact_id_and_blob_payload() {
        let (router, _rx) = test_router().await;
        seed_test_artifact(&router).await;

        let response = router
            .dispatch(crate::protocol::RpcRequest {
                jsonrpc: "2.0".to_string(),
                id: Some(crate::protocol::RpcId::Num(1)),
                method: "artifact/download".to_string(),
                params: Some(
                    serde_json::value::to_raw_value(&serde_json::json!({
                        "artifact_id": "artifact-1",
                        "expected_media_type": "application/json"
                    }))
                    .expect("raw value"),
                ),
            })
            .await
            .expect("response");

        assert!(
            response.error.is_none(),
            "artifact/download should succeed: {:?}",
            response.error
        );
        let result: serde_json::Value =
            serde_json::from_str(response.result.unwrap().get()).unwrap();
        assert_eq!(result["record"]["artifact_id"], "artifact-1");
        assert_eq!(result["payload"]["artifact_id"], "artifact-1");
        assert_eq!(result["payload"]["media_type"], "application/json");
        assert_eq!(result["payload"]["data"], "e30=");
    }

    #[tokio::test]
    async fn artifact_get_missing_is_typed_error() {
        let (router, _rx) = test_router().await;

        let response = router
            .dispatch(crate::protocol::RpcRequest {
                jsonrpc: "2.0".to_string(),
                id: Some(crate::protocol::RpcId::Num(1)),
                method: "artifact/get".to_string(),
                params: Some(
                    serde_json::value::to_raw_value(
                        &serde_json::json!({ "artifact_id": "missing" }),
                    )
                    .expect("raw value"),
                ),
            })
            .await
            .expect("response");

        let err = response.error.expect("missing artifact should error");
        assert_eq!(err.code, error::INVALID_PARAMS);
        assert!(err.message.contains("artifact not found"));
    }

    #[tokio::test]
    async fn artifact_download_rejects_media_type_mismatch() {
        let (router, _rx) = test_router().await;
        seed_test_artifact(&router).await;

        let response = router
            .dispatch(crate::protocol::RpcRequest {
                jsonrpc: "2.0".to_string(),
                id: Some(crate::protocol::RpcId::Num(1)),
                method: "artifact/download".to_string(),
                params: Some(
                    serde_json::value::to_raw_value(&serde_json::json!({
                        "artifact_id": "artifact-1",
                        "expected_media_type": "text/plain"
                    }))
                    .expect("raw value"),
                ),
            })
            .await
            .expect("response");

        let err = response.error.expect("mismatched media type should error");
        assert_eq!(err.code, error::INVALID_PARAMS);
        assert!(err.message.contains("media type mismatch"));
    }

    fn source_uuid(raw: &str) -> SourceUuid {
        SourceUuid::parse(raw).expect("valid source uuid")
    }

    fn skill_name(raw: &str) -> SkillName {
        SkillName::parse(raw).expect("valid skill name")
    }

    fn key(source: &str, name: &str) -> SkillKey {
        SkillKey {
            source_uuid: source_uuid(source),
            skill_name: skill_name(name),
        }
    }

    fn record(source: &str, fingerprint: &str) -> SourceIdentityRecord {
        SourceIdentityRecord {
            source_uuid: source_uuid(source),
            display_name: source.to_string(),
            transport_kind: SourceTransportKind::Filesystem,
            fingerprint: fingerprint.to_string(),
            status: SourceIdentityStatus::Active,
        }
    }

    fn alias_registry() -> SourceIdentityRegistry {
        SourceIdentityRegistry::build(
            vec![
                record("dc256086-0d2f-4f61-a307-320d4148107f", "fp-1"),
                record("a93d587d-8f44-438f-8189-6e8cf549f6e7", "fp-1"),
            ],
            vec![SourceIdentityLineage {
                event_id: "rotate-1".to_string(),
                recorded_at_unix_secs: 1,
                required_from_skills: vec![skill_name("email-extractor")],
                event: SourceIdentityLineageEvent::Rotate {
                    from: source_uuid("dc256086-0d2f-4f61-a307-320d4148107f"),
                    to: source_uuid("a93d587d-8f44-438f-8189-6e8cf549f6e7"),
                },
            }],
            vec![SkillKeyRemap {
                from: key("dc256086-0d2f-4f61-a307-320d4148107f", "email-extractor"),
                to: key("a93d587d-8f44-438f-8189-6e8cf549f6e7", "mail-extractor"),
                reason: Some("rotate".to_string()),
            }],
            vec![meerkat_core::skills::SkillAlias {
                alias: "legacy/email".to_string(),
                to: key("dc256086-0d2f-4f61-a307-320d4148107f", "email-extractor"),
            }],
        )
        .expect("registry")
    }

    fn make_request(method: &str, params: impl Serialize) -> RpcRequest {
        let params_raw =
            serde_json::value::RawValue::from_string(serde_json::to_string(&params).unwrap())
                .unwrap();
        RpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(RpcId::Num(1)),
            method: method.to_string(),
            params: Some(params_raw),
        }
    }

    fn make_request_no_params(method: &str) -> RpcRequest {
        RpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(RpcId::Num(1)),
            method: method.to_string(),
            params: None,
        }
    }

    fn make_notification(method: &str) -> RpcRequest {
        RpcRequest {
            jsonrpc: "2.0".to_string(),
            id: None,
            method: method.to_string(),
            params: None,
        }
    }

    /// Extract the result JSON value from a successful response.
    fn result_value(resp: &RpcResponse) -> serde_json::Value {
        assert!(
            resp.error.is_none(),
            "Expected success response, got error: {:?}",
            resp.error
        );
        let raw = resp
            .result
            .as_ref()
            .expect("Missing result in success response");
        serde_json::from_str(raw.get()).unwrap()
    }

    /// Extract the error code from an error response.
    fn error_code(resp: &RpcResponse) -> i32 {
        resp.error.as_ref().expect("Expected error response").code
    }

    async fn cancelled_request_context(id: &RpcId) -> RequestContext {
        use meerkat::surface::CancelOutcome;
        let executor = SurfaceRequestExecutor::new(Duration::from_millis(1));
        let key = serde_json::to_string(id).expect("request id should serialize");
        let context = executor.begin_request(key.clone(), noop_request_action());
        let outcome = executor.cancel_request(&key).await;
        assert_eq!(
            outcome,
            CancelOutcome::Cancelled,
            "pre-cancel should transition Pending → Cancelled"
        );
        context
    }

    fn error_message(resp: &RpcResponse) -> String {
        resp.error
            .as_ref()
            .expect("Expected error response")
            .message
            .clone()
    }

    async fn drain_notifications(notif_rx: &mut mpsc::Receiver<RpcNotification>) {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        while notif_rx.try_recv().is_ok() {}
    }

    async fn next_run_started_prompt(notif_rx: &mut mpsc::Receiver<RpcNotification>) -> String {
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                let notif = notif_rx.recv().await.expect("notification");
                if notif.method != "session/event" {
                    continue;
                }
                if notif.params["event"]["payload"]["type"] != "run_started" {
                    continue;
                }
                break notif.params["event"]["payload"]["prompt"]
                    .as_str()
                    .expect("run_started prompt")
                    .to_string();
            }
        })
        .await
        .expect("run_started notification")
    }

    fn system_prompt_from_request(messages: &[Message]) -> Option<&str> {
        messages.iter().find_map(|message| match message {
            Message::System(system) => Some(system.content.as_str()),
            _ => None,
        })
    }

    // -----------------------------------------------------------------------
    // Tests
    // -----------------------------------------------------------------------

    /// 1. `initialize` returns server capabilities with server info and methods.
    #[tokio::test]
    async fn initialize_returns_capabilities() {
        let (router, _notif_rx) = test_router().await;
        let req = make_request_no_params("initialize");

        let resp = router.dispatch(req).await.unwrap();
        let result = result_value(&resp);

        // Verify server info
        assert_eq!(result["server_info"]["name"], "meerkat-rpc");
        assert!(result["server_info"]["version"].is_string());

        // Verify methods list includes expected methods
        let methods = result["methods"].as_array().unwrap();
        let method_names: Vec<&str> = methods.iter().map(|m| m.as_str().unwrap()).collect();
        assert!(method_names.contains(&"initialize"));
        assert!(method_names.contains(&"help/ask"));
        assert!(method_names.contains(&"session/create"));
        assert!(method_names.contains(&"session/history"));
        assert!(method_names.contains(&"session/fork_at"));
        assert!(method_names.contains(&"session/fork_replace"));
        assert!(method_names.contains(&"session/external_event"));
        assert!(method_names.contains(&"session/peer_response_terminal"));
        assert!(method_names.contains(&"session/inject_context"));
        // session/realtime_attachment_status removed in live-adapter MVP
        assert!(method_names.contains(&"turn/start"));
        assert!(method_names.contains(&"approval/request"));
        assert!(method_names.contains(&"approval/list"));
        assert!(method_names.contains(&"approval/get"));
        assert!(method_names.contains(&"approval/decide"));
        assert!(
            !method_names.contains(&"session/destroy"),
            "generic session/destroy must not appear until it has member-aware mob semantics"
        );
        for retired in [
            "runtime/session_status",
            "runtime/session_submit",
            "runtime/session_submission",
            "runtime/session_submissions",
            "runtime/session_retire",
            "runtime/session_reset",
        ] {
            assert!(
                !method_names.contains(&retired),
                "retired runtime/session control noun must not be advertised: {retired}"
            );
        }
        #[cfg(feature = "mob")]
        {
            assert!(!method_names.contains(&"mob/prefabs"));
            assert!(method_names.contains(&"mob/spawn_helper"));
            assert!(method_names.contains(&"mob/fork_helper"));
            assert!(method_names.contains(&"mob/force_cancel"));
            assert!(method_names.contains(&"mob/member_status"));
            assert!(method_names.contains(&"mob/member_send"));
            assert!(method_names.contains(&"mob/ingress_interaction"));
            assert!(!method_names.contains(&"mob/tools"));
            assert!(!method_names.contains(&"mob/call"));
            assert!(method_names.contains(&"mob/stream_open"));
            assert!(method_names.contains(&"mob/stream_close"));
        }
        #[cfg(not(feature = "mob"))]
        {
            assert!(!method_names.contains(&"mob/prefabs"));
            assert!(!method_names.contains(&"mob/tools"));
            assert!(!method_names.contains(&"mob/call"));
            assert!(!method_names.contains(&"mob/stream_open"));
            assert!(!method_names.contains(&"mob/stream_close"));
        }
        assert!(method_names.contains(&"config/get"));
    }

    #[tokio::test]
    async fn help_ask_runs_help_session_with_platform_skill_prompt() {
        let recorded_requests = Arc::new(std::sync::Mutex::new(Vec::<Vec<Message>>::new()));
        let (router, _notif_rx) = test_router_with_llm(Arc::new(RecordingMockLlmClient::new(
            Arc::clone(&recorded_requests),
        )))
        .await;

        let response = router
            .dispatch(make_request(
                "help/ask",
                serde_json::json!({
                    "question": "How do I add an MCP server?",
                    "prompt": "Write a match-3 game",
                    "execution_mode": "plan_execution"
                }),
            ))
            .await
            .expect("help response");

        assert!(response.error.is_none(), "help/ask failed: {response:?}");
        assert_eq!(result_value(&response)["text"], "Hello from mock");
        let requests = recorded_requests
            .lock()
            .expect("recorded requests lock poisoned");
        let system_prompt = system_prompt_from_request(
            requests
                .first()
                .expect("help session should reach the LLM request"),
        )
        .expect("help session should include a system prompt");
        assert!(system_prompt.contains("dedicated help surface"));
        assert!(system_prompt.contains("meerkat-platform"));
        assert!(system_prompt.contains("Meerkat Platform Guide"));
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn mob_compatibility_routes_are_not_found() {
        let (router, _notif_rx) = test_router().await;

        let tools_resp = router
            .dispatch(make_request_no_params("mob/tools"))
            .await
            .unwrap();
        assert_eq!(error_code(&tools_resp), error::METHOD_NOT_FOUND);

        let create_resp = router
            .dispatch(make_request(
                "mob/call",
                serde_json::json!({
                    "name": "mob_create",
                    "arguments": {
                        "definition": {
                            "id": "test_mob",
                            "profiles": {
                                "worker": {
                                    "model": "claude-sonnet-4-6",
                                    "tools": { "comms": true }
                                }
                            }
                        }
                    }
                }),
            ))
            .await
            .unwrap();
        assert_eq!(error_code(&create_resp), error::METHOD_NOT_FOUND);
    }

    #[tokio::test]
    async fn session_stream_close_removes_forwarder_from_active_map() {
        let (router, mut notif_rx) = test_router().await;

        let create_resp = router
            .dispatch(make_request(
                "session/create",
                serde_json::json!({ "prompt": "hello" }),
            ))
            .await
            .unwrap();
        let created = result_value(&create_resp);
        let session_id = created["session_id"].as_str().unwrap().to_string();

        let open_resp = router
            .dispatch(make_request(
                "session/stream_open",
                serde_json::json!({ "session_id": session_id }),
            ))
            .await
            .unwrap();
        let opened = result_value(&open_resp);
        let stream_id = opened["stream_id"].as_str().unwrap().to_string();
        assert_eq!(router.active_session_streams.lock().await.len(), 1);

        let close_resp = router
            .dispatch(make_request(
                "session/stream_close",
                serde_json::json!({ "stream_id": stream_id }),
            ))
            .await
            .unwrap();
        let closed = result_value(&close_resp);
        assert_eq!(closed["closed"], true);
        assert_eq!(closed["already_closed"], false);
        assert_eq!(router.active_session_streams.lock().await.len(), 0);

        let notification = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                let notif = notif_rx.recv().await.expect("notification");
                if notif.method == "session/stream_end" {
                    break notif;
                }
            }
        })
        .await
        .expect("session explicit-close notification");
        assert_eq!(notification.params["stream_id"], stream_id);
        assert_eq!(notification.params["session_id"], session_id);
        assert_eq!(notification.params["outcome"], "explicit_close");
    }

    #[tokio::test]
    async fn session_stream_close_is_idempotent_after_explicit_close() {
        let (router, mut notif_rx) = test_router().await;

        let create_resp = router
            .dispatch(make_request(
                "session/create",
                serde_json::json!({ "prompt": "hello" }),
            ))
            .await
            .unwrap();
        let session_id = result_value(&create_resp)["session_id"]
            .as_str()
            .unwrap()
            .to_string();

        let open_resp = router
            .dispatch(make_request(
                "session/stream_open",
                serde_json::json!({ "session_id": session_id }),
            ))
            .await
            .unwrap();
        let stream_id = result_value(&open_resp)["stream_id"]
            .as_str()
            .unwrap()
            .to_string();

        let close_resp = router
            .dispatch(make_request(
                "session/stream_close",
                serde_json::json!({ "stream_id": stream_id }),
            ))
            .await
            .unwrap();
        assert_eq!(result_value(&close_resp)["already_closed"], false);

        let notification = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                let notif = notif_rx.recv().await.expect("notification");
                if notif.method == "session/stream_end" {
                    break notif;
                }
            }
        })
        .await
        .expect("explicit-close notification");
        assert_eq!(notification.params["outcome"], "explicit_close");

        let close_again_resp = router
            .dispatch(make_request(
                "session/stream_close",
                serde_json::json!({ "stream_id": stream_id }),
            ))
            .await
            .unwrap();
        assert_eq!(result_value(&close_again_resp)["already_closed"], true);

        let extra_notification =
            tokio::time::timeout(std::time::Duration::from_millis(200), notif_rx.recv()).await;
        assert!(
            extra_notification.is_err(),
            "idempotent close must not emit a second terminal notification"
        );
    }

    #[tokio::test]
    async fn session_stream_open_accepts_deferred_session_before_first_turn() {
        let (router, mut notif_rx) = test_router().await;

        let create_resp = router
            .dispatch(make_request(
                "session/create",
                serde_json::json!({
                    "prompt": "deferred bootstrap",
                    "initial_turn": "deferred"
                }),
            ))
            .await
            .unwrap();
        let session_id = result_value(&create_resp)["session_id"]
            .as_str()
            .unwrap()
            .to_string();

        let open_resp = router
            .dispatch(make_request(
                "session/stream_open",
                serde_json::json!({ "session_id": session_id }),
            ))
            .await
            .unwrap();
        assert!(
            open_resp.error.is_none(),
            "session/stream_open should accept pending deferred sessions: {open_resp:?}"
        );
        let stream_id = result_value(&open_resp)["stream_id"]
            .as_str()
            .unwrap()
            .to_string();

        let turn_resp = router
            .dispatch(make_request(
                "turn/start",
                serde_json::json!({
                    "session_id": session_id,
                    "prompt": "start first turn"
                }),
            ))
            .await
            .unwrap();
        assert!(
            turn_resp.error.is_none(),
            "turn/start should materialize deferred session: {turn_resp:?}"
        );

        let notification = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                let notif = notif_rx.recv().await.expect("notification");
                if notif.method == "session/stream_event"
                    && notif.params["stream_id"].as_str() == Some(stream_id.as_str())
                    && notif.params["event"]["payload"]["type"].as_str() == Some("run_started")
                {
                    break notif;
                }
            }
        })
        .await
        .expect("pending session stream should receive first-turn events");

        assert_eq!(
            notification.params["session_id"].as_str(),
            Some(session_id.as_str())
        );
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn mob_append_system_context_targets_mob_backing_service() {
        let (router, _notif_rx) = test_router().await;

        let create_resp = router
            .dispatch(make_request(
                "mob/create",
                serde_json::json!({
                    "definition": {
                        "id": "mob-system-context",
                        "profiles": {
                            "worker": {
                                "model": "claude-sonnet-4-5",
                                "external_addressable": true,
                                "tools": { "comms": true }
                            }
                        }
                    }
                }),
            ))
            .await
            .unwrap();
        let created = result_value(&create_resp);
        let mob_id = created["mob_id"].as_str().unwrap().to_string();

        let spawn_resp = router
            .dispatch(make_request(
                "mob/spawn",
                serde_json::json!({
                    "mob_id": mob_id,
                    "profile": "worker",
                    "agent_identity": "worker-1",
                    "runtime_mode": "turn_driven"
                }),
            ))
            .await
            .unwrap();
        let spawned = result_value(&spawn_resp);
        assert_eq!(spawned["agent_identity"], "worker-1");

        let append_resp = router
            .dispatch(make_request(
                "mob/append_system_context",
                serde_json::json!({
                    "mob_id": mob_id,
                    "agent_identity": "worker-1",
                    "text": "Prioritize the lead.",
                    "source": "mob",
                    "idempotency_key": "ctx-worker-1"
                }),
            ))
            .await
            .unwrap();
        let appended = result_value(&append_resp);
        assert_eq!(appended["status"], "staged");
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn mob_member_send_host_route_targets_canonical_member_path() {
        let (router, _notif_rx) = test_router().await;

        let create_resp = router
            .dispatch(make_request(
                "mob/create",
                serde_json::json!({
                    "definition": {
                        "id": "mob-member-send",
                        "profiles": {
                            "worker": {
                                "model": "claude-sonnet-4-5",
                                "external_addressable": true,
                                "tools": { "comms": true }
                            }
                        }
                    }
                }),
            ))
            .await
            .unwrap();
        let mob_id = result_value(&create_resp)["mob_id"]
            .as_str()
            .unwrap()
            .to_string();

        let spawn_resp = router
            .dispatch(make_request(
                "mob/spawn",
                serde_json::json!({
                    "mob_id": mob_id,
                    "profile": "worker",
                    "agent_identity": "worker-1",
                    "runtime_mode": "turn_driven"
                }),
            ))
            .await
            .unwrap();
        let spawned = result_value(&spawn_resp);
        assert_eq!(spawned["agent_identity"], "worker-1");

        let send_resp = router
            .dispatch(make_request(
                "mob/member_send",
                serde_json::json!({
                    "mob_id": mob_id,
                    "agent_identity": "worker-1",
                    "content": "Please acknowledge with HOST_ROUTE_OK."
                }),
            ))
            .await
            .unwrap();
        let sent = result_value(&send_resp);
        assert_eq!(sent["agent_identity"], "worker-1");
        assert!(
            sent["member_ref"].as_str().is_some_and(|s| !s.is_empty()),
            "mob/member_send must return the opaque member_ref"
        );
        assert!(
            sent.get("agent_runtime_id").is_none(),
            "binding-era agent_runtime_id must not leak to app-facing mob/member_send"
        );
        assert_eq!(sent["handling_mode"], "queue");
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn mob_ingress_interaction_ensures_member_and_returns_replay_anchor() {
        let (router, _notif_rx) = test_router().await;

        let create_resp = router
            .dispatch(make_request(
                "mob/create",
                serde_json::json!({
                    "definition": {
                        "id": "mob-ingress-interaction",
                        "profiles": {
                            "worker": {
                                "model": "claude-sonnet-4-5",
                                "external_addressable": true,
                                "tools": { "comms": true }
                            }
                        }
                    }
                }),
            ))
            .await
            .unwrap();
        let mob_id = result_value(&create_resp)["mob_id"]
            .as_str()
            .unwrap()
            .to_string();

        let ingress_resp = router
            .dispatch(make_request(
                "mob/ingress_interaction",
                serde_json::json!({
                    "mob_id": mob_id,
                    "spec": {
                        "profile": "worker",
                        "agent_identity": "ingress-1",
                        "runtime_mode": "turn_driven"
                    },
                    "content": "Please acknowledge with INGRESS_OK."
                }),
            ))
            .await
            .unwrap();
        let receipt = result_value(&ingress_resp);
        assert_eq!(receipt["mob_id"], mob_id);
        assert_eq!(receipt["agent_identity"], "ingress-1");
        assert_eq!(receipt["delivery"]["agent_identity"], "ingress-1");
        assert_eq!(receipt["delivery"]["handling_mode"], "queue");
        assert!(
            receipt["member_ref"]
                .as_str()
                .is_some_and(|s| !s.is_empty()),
            "ingress helper must return the opaque member_ref"
        );
        assert!(
            receipt["ensure_outcome"].get("spawned").is_some(),
            "first interaction should spawn the ingress member: {receipt}"
        );
        assert!(
            receipt["events_after_cursor"].as_u64().is_some(),
            "receipt must carry a replay anchor cursor"
        );
        assert!(
            receipt["latest_event_cursor"].as_u64().is_some(),
            "receipt must carry the post-delivery event cursor"
        );
        assert!(
            receipt.get("agent_runtime_id").is_none(),
            "binding-era agent_runtime_id must not leak from ingress helper"
        );

        let second_resp = router
            .dispatch(make_request(
                "mob/ingress_interaction",
                serde_json::json!({
                    "mob_id": mob_id,
                    "spec": {
                        "profile": "worker",
                        "agent_identity": "ingress-1",
                        "runtime_mode": "turn_driven"
                    },
                    "content": "Second interaction."
                }),
            ))
            .await
            .unwrap();
        let second = result_value(&second_resp);
        assert!(
            second["ensure_outcome"].get("existed").is_some(),
            "second interaction should reuse the ingress member: {second}"
        );
        assert_eq!(second["delivery"]["agent_identity"], "ingress-1");
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn mob_ingress_interaction_rejects_missing_mob() {
        let (router, _notif_rx) = test_router().await;

        let resp = router
            .dispatch(make_request(
                "mob/ingress_interaction",
                serde_json::json!({
                    "mob_id": "missing-mob",
                    "spec": {
                        "profile": "worker",
                        "agent_identity": "ingress-missing",
                        "runtime_mode": "turn_driven"
                    },
                    "content": "hello"
                }),
            ))
            .await
            .unwrap();
        let err = resp.error.expect("missing mob should fail");
        assert!(
            err.message.contains("mob not found"),
            "unexpected error: {}",
            err.message
        );
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn mob_events_strict_rejects_stale_cursor() {
        let (router, _notif_rx) = test_router().await;

        let create_resp = router
            .dispatch(make_request(
                "mob/create",
                serde_json::json!({
                    "definition": {
                        "id": "mob-events-strict",
                        "profiles": {
                            "worker": {
                                "model": "claude-sonnet-4-5",
                                "external_addressable": true,
                                "tools": { "comms": true }
                            }
                        }
                    }
                }),
            ))
            .await
            .unwrap();
        let mob_id = result_value(&create_resp)["mob_id"]
            .as_str()
            .unwrap()
            .to_string();

        let resp = router
            .dispatch(make_request(
                "mob/events",
                serde_json::json!({
                    "mob_id": mob_id,
                    "after_cursor": 999_999_u64,
                    "limit": 10,
                    "strict": true
                }),
            ))
            .await
            .unwrap();
        let err = resp.error.expect("strict stale cursor should fail");
        assert!(
            err.message.contains("stale mob event cursor"),
            "unexpected error: {}",
            err.message
        );
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn mob_member_status_omits_runtime_identity_fields() {
        let (router, _notif_rx) = test_router().await;

        let create_resp = router
            .dispatch(make_request(
                "mob/create",
                serde_json::json!({
                    "definition": {
                        "id": "mob-member-status-runtime-identity",
                        "profiles": {
                            "worker": {
                                "model": "claude-sonnet-4-5",
                                "external_addressable": true,
                                "tools": { "comms": true }
                            }
                        }
                    }
                }),
            ))
            .await
            .unwrap();
        let mob_id = result_value(&create_resp)["mob_id"]
            .as_str()
            .unwrap()
            .to_string();

        router
            .dispatch(make_request(
                "mob/spawn",
                serde_json::json!({
                    "mob_id": mob_id,
                    "profile": "worker",
                    "agent_identity": "worker-1",
                    "runtime_mode": "turn_driven"
                }),
            ))
            .await
            .unwrap();

        let status_resp = router
            .dispatch(make_request(
                "mob/member_status",
                serde_json::json!({
                    "mob_id": mob_id,
                    "agent_identity": "worker-1"
                }),
            ))
            .await
            .unwrap();
        let status = result_value(&status_resp);
        assert!(
            status.get("agent_runtime_id").is_none(),
            "binding-era agent_runtime_id must not leak to app-facing mob/member_status"
        );
        assert!(
            status.get("fence_token").is_none(),
            "binding-era fence_token must not leak to app-facing mob/member_status"
        );
        assert_eq!(status["status"], "active");
        assert!(status["tokens_used"].is_number());
        assert!(status["is_final"].is_boolean());
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn mob_spawn_rejects_unknown_fields() {
        let (router, _notif_rx) = test_router().await;

        let create_resp = router
            .dispatch(make_request(
                "mob/create",
                serde_json::json!({
                    "definition": {
                        "id": "mob-spawn-unknown-field",
                        "profiles": {
                            "worker": {
                                "model": "claude-sonnet-4-5",
                                "tools": { "comms": true }
                            }
                        }
                    }
                }),
            ))
            .await
            .unwrap();
        let mob_id = result_value(&create_resp)["mob_id"]
            .as_str()
            .unwrap()
            .to_string();

        let spawn_resp = router
            .dispatch(make_request(
                "mob/spawn",
                serde_json::json!({
                    "mob_id": mob_id,
                    "profile": "worker",
                    "agent_identity": "worker-1",
                    "initial_turn": "deferred"
                }),
            ))
            .await
            .unwrap();
        let error = spawn_resp.error.expect("unknown field should be rejected");
        assert!(
            error.message.contains("unknown field") && error.message.contains("initial_turn"),
            "unexpected error for unknown mob/spawn field: {error:?}"
        );
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn mob_member_send_host_route_accepts_steer_mode() {
        let (router, _notif_rx) = test_router().await;

        let create_resp = router
            .dispatch(make_request(
                "mob/create",
                serde_json::json!({
                    "definition": {
                        "id": "mob-member-send-steer",
                        "profiles": {
                            "worker": {
                                "model": "claude-sonnet-4-5",
                                "external_addressable": true,
                                "tools": { "comms": true }
                            }
                        }
                    }
                }),
            ))
            .await
            .unwrap();
        let mob_id = result_value(&create_resp)["mob_id"]
            .as_str()
            .unwrap()
            .to_string();

        let spawn_resp = router
            .dispatch(make_request(
                "mob/spawn",
                serde_json::json!({
                    "mob_id": mob_id,
                    "profile": "worker",
                    "agent_identity": "worker-1",
                    "runtime_mode": "turn_driven"
                }),
            ))
            .await
            .unwrap();
        let spawned = result_value(&spawn_resp);
        assert_eq!(spawned["agent_identity"], "worker-1");

        let send_resp = router
            .dispatch(make_request(
                "mob/member_send",
                serde_json::json!({
                    "mob_id": mob_id,
                    "agent_identity": "worker-1",
                    "content": "Please acknowledge with HOST_ROUTE_STEER.",
                    "handling_mode": "steer"
                }),
            ))
            .await
            .unwrap();
        assert!(
            send_resp.error.is_none(),
            "steer send should be accepted by mob member_send; the provisioner flattens to Queue: {:?}",
            send_resp.error
        );
        let result = send_resp.result.expect("expected success result");
        let result: serde_json::Value = serde_json::from_str(result.get()).unwrap();
        assert_eq!(result["agent_identity"], "worker-1");
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn mob_spawned_session_id_routes_through_session_and_comms_handlers() {
        let (router, _notif_rx) = test_router().await;

        let create_resp = router
            .dispatch(make_request(
                "mob/create",
                serde_json::json!({
                    "definition": {
                        "id": "mob-routed-session",
                        "profiles": {
                            "worker": {
                                "model": "claude-sonnet-4-5",
                                "tools": { "comms": true }
                            }
                        }
                    }
                }),
            ))
            .await
            .unwrap();
        let created = result_value(&create_resp);
        let mob_id = created["mob_id"].as_str().unwrap().to_string();

        let spawn_resp = router
            .dispatch(make_request(
                "mob/spawn",
                serde_json::json!({
                    "mob_id": mob_id,
                    "profile": "worker",
                    "agent_identity": "worker-1",
                    "runtime_mode": "turn_driven"
                }),
            ))
            .await
            .unwrap();
        let spawned = result_value(&spawn_resp);
        assert_eq!(spawned["agent_identity"], "worker-1");
        let session_id =
            resolve_mob_bridge_session_id(&router, "mob-routed-session", "worker-1").await;

        let read_resp = router
            .dispatch(make_request(
                "session/read",
                serde_json::json!({ "session_id": session_id }),
            ))
            .await
            .unwrap();
        let read = result_value(&read_resp);
        assert_eq!(read["session_id"], session_id);

        let peers_resp = router
            .dispatch(make_request(
                "comms/peers",
                serde_json::json!({ "session_id": session_id }),
            ))
            .await
            .unwrap();
        let peers = result_value(&peers_resp);
        assert!(peers["peers"].is_array());
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn mob_archived_session_history_remains_routable() {
        let (router, _notif_rx) = test_router().await;

        let create_resp = router
            .dispatch(make_request(
                "mob/create",
                serde_json::json!({
                    "definition": {
                        "id": "mob-archived-history",
                        "profiles": {
                            "worker": {
                                "model": "claude-sonnet-4-5",
                                "tools": { "comms": true }
                            }
                        }
                    }
                }),
            ))
            .await
            .unwrap();
        let mob_id = result_value(&create_resp)["mob_id"]
            .as_str()
            .unwrap()
            .to_string();

        let _spawn_resp = router
            .dispatch(make_request(
                "mob/spawn",
                serde_json::json!({
                    "mob_id": mob_id,
                    "profile": "worker",
                    "agent_identity": "worker-1",
                    "runtime_mode": "turn_driven"
                }),
            ))
            .await
            .unwrap();
        let session_id =
            resolve_mob_bridge_session_id(&router, "mob-archived-history", "worker-1").await;

        let archive_resp = router
            .dispatch(make_request(
                "session/archive",
                serde_json::json!({ "session_id": &session_id }),
            ))
            .await
            .unwrap();
        assert_eq!(result_value(&archive_resp)["archived"], true);

        let history_resp = router
            .dispatch(make_request(
                "session/history",
                serde_json::json!({ "session_id": &session_id }),
            ))
            .await
            .unwrap();
        assert!(
            history_resp.error.is_none(),
            "archived mob-owned sessions should still route to session/history"
        );
        let history = result_value(&history_resp);
        assert_eq!(history["session_id"], session_id);
        assert!(history["messages"].is_array());
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn mob_spawned_session_id_supports_session_stream_open() {
        let (router, _notif_rx) = test_router().await;

        let create_resp = router
            .dispatch(make_request(
                "mob/create",
                serde_json::json!({
                    "definition": {
                        "id": "mob-session-stream",
                        "profiles": {
                            "worker": {
                                "model": "claude-sonnet-4-5",
                                "tools": { "comms": true }
                            }
                        }
                    }
                }),
            ))
            .await
            .unwrap();
        let mob_id = result_value(&create_resp)["mob_id"]
            .as_str()
            .unwrap()
            .to_string();

        let _spawn_resp = router
            .dispatch(make_request(
                "mob/spawn",
                serde_json::json!({
                    "mob_id": mob_id,
                    "profile": "worker",
                    "agent_identity": "worker-1",
                    "runtime_mode": "turn_driven"
                }),
            ))
            .await
            .unwrap();
        let session_id =
            resolve_mob_bridge_session_id(&router, "mob-session-stream", "worker-1").await;

        let open_resp = router
            .dispatch(make_request(
                "session/stream_open",
                serde_json::json!({ "session_id": session_id }),
            ))
            .await
            .unwrap();
        let opened = result_value(&open_resp);
        assert_eq!(opened["opened"], true);
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn mob_spawned_session_id_supports_turn_interrupt() {
        let (router, _notif_rx) = test_router().await;

        let create_resp = router
            .dispatch(make_request(
                "mob/create",
                serde_json::json!({
                    "definition": {
                        "id": "mob-session-interrupt",
                        "profiles": {
                            "worker": {
                                "model": "claude-sonnet-4-5",
                                "tools": { "comms": true }
                            }
                        }
                    }
                }),
            ))
            .await
            .unwrap();
        let mob_id = result_value(&create_resp)["mob_id"]
            .as_str()
            .unwrap()
            .to_string();

        let _spawn_resp = router
            .dispatch(make_request(
                "mob/spawn",
                serde_json::json!({
                    "mob_id": mob_id,
                    "profile": "worker",
                    "agent_identity": "worker-1",
                    "runtime_mode": "turn_driven"
                }),
            ))
            .await
            .unwrap();
        let session_id =
            resolve_mob_bridge_session_id(&router, "mob-session-interrupt", "worker-1").await;

        let interrupt_resp = router
            .dispatch(make_request(
                "turn/interrupt",
                serde_json::json!({ "session_id": session_id }),
            ))
            .await
            .unwrap();
        assert!(
            interrupt_resp.error.is_none(),
            "mob-backed interrupt should route through the authoritative owner: {interrupt_resp:?}"
        );
        assert_eq!(result_value(&interrupt_resp)["interrupted"], true);
    }

    #[cfg(all(feature = "mob", feature = "comms"))]
    #[tokio::test]
    async fn mob_member_stream_surfaces_run_completed_for_late_terminal_peer_response() {
        use meerkat_core::agent::CommsRuntime as CoreCommsRuntime;
        use meerkat_core::comms::{CommsCommand, PeerName, PeerRoute, TrustedPeerDescriptor};
        use meerkat_core::interaction::InteractionId;

        let (router, mut notif_rx) = test_router().await;

        let create_resp = router
            .dispatch(make_request(
                "mob/create",
                serde_json::json!({
                    "definition": {
                        "id": "mob-peer-response-run-completed",
                        "profiles": {
                            "worker": {
                                "model": "claude-sonnet-4-5",
                                "tools": { "comms": true }
                            }
                        }
                    }
                }),
            ))
            .await
            .unwrap();
        let mob_id = result_value(&create_resp)["mob_id"]
            .as_str()
            .unwrap()
            .to_string();

        let _spawn_resp = router
            .dispatch(make_request(
                "mob/spawn",
                serde_json::json!({
                    "mob_id": &mob_id,
                    "profile": "worker",
                    "agent_identity": "worker-1",
                    "runtime_mode": "turn_driven"
                }),
            ))
            .await
            .unwrap();
        let session_id =
            resolve_mob_bridge_session_id(&router, "mob-peer-response-run-completed", "worker-1")
                .await;
        let session_id = SessionId::parse(&session_id).expect("valid bridge session id");

        let _turn_resp = router
            .dispatch(make_request(
                "mob/turn_start",
                serde_json::json!({
                    "mob_id": &mob_id,
                    "agent_identity": "worker-1",
                    "prompt": "Reply exactly READY."
                }),
            ))
            .await
            .unwrap();

        let open_resp = router
            .dispatch(make_request(
                "mob/stream_open",
                serde_json::json!({
                    "mob_id": &mob_id,
                    "agent_identity": "worker-1"
                }),
            ))
            .await
            .unwrap();
        let stream_id = result_value(&open_resp)["stream_id"]
            .as_str()
            .unwrap()
            .to_string();
        let session_stream_resp = router
            .dispatch(make_request(
                "session/stream_open",
                serde_json::json!({ "session_id": session_id.clone() }),
            ))
            .await
            .unwrap();
        let session_stream_id = result_value(&session_stream_resp)["stream_id"]
            .as_str()
            .unwrap()
            .to_string();

        let operator_comms = router
            .runtime
            .comms_runtime(&session_id)
            .await
            .expect("worker comms runtime");
        let sender = std::sync::Arc::new(
            meerkat::CommsRuntime::inproc_only("router-peer-response-sender")
                .expect("sender comms runtime"),
        );
        install_ephemeral_peer_request_response_authority(&sender, "router-peer-response-sender");
        let sender_peer_id = sender.public_key().to_peer_id().to_string();
        let sender_addr = sender.advertised_address();
        let operator_peer_id = operator_comms.public_key().expect("worker peer id");
        let operator_addr = operator_comms
            .advertised_address()
            .expect("worker advertised address");

        let operator_pubkey =
            meerkat_comms::PubKey::from_pubkey_string(&operator_peer_id).expect("operator pubkey");
        CoreCommsRuntime::add_trusted_peer(
            &*sender,
            TrustedPeerDescriptor::unsigned_with_pubkey(
                format!("{mob_id}/worker/worker-1"),
                operator_pubkey.to_peer_id().to_string(),
                *operator_pubkey.as_bytes(),
                operator_addr,
            )
            .expect("worker trusted peer spec"),
        )
        .await
        .expect("sender trusts worker");
        CoreCommsRuntime::add_trusted_peer(
            operator_comms.as_ref(),
            TrustedPeerDescriptor::unsigned_with_pubkey(
                "router-peer-response-sender",
                sender_peer_id,
                *sender.public_key().as_bytes(),
                sender_addr,
            )
            .expect("sender trusted peer spec"),
        )
        .await
        .expect("worker trusts sender");

        let in_reply_to = InteractionId(uuid::Uuid::new_v4());
        sender
            .peer_interaction_handle()
            .expect("sender peer response authority")
            .request_received(meerkat_core::PeerCorrelationId::from_uuid(in_reply_to.0))
            .expect("seed inbound request before terminal peer response");

        CoreCommsRuntime::send(
            &*sender,
            CommsCommand::PeerResponse {
                to: PeerRoute::with_display_name(
                    operator_pubkey.to_peer_id(),
                    PeerName::new(format!("{mob_id}/worker/worker-1")).expect("valid peer name"),
                ),
                in_reply_to,
                status: meerkat_core::ResponseStatus::Completed,
                result: serde_json::json!({
                    "request_intent": "checksum_token",
                    "request_subject": "alpha beta gamma",
                    "token": "birch seventeen",
                }),
                blocks: None,
                handling_mode: None,
            },
        )
        .await
        .expect("send terminal peer response");

        let (mob_run_completed, session_run_completed) = match tokio::time::timeout(
            std::time::Duration::from_secs(5),
            async {
                loop {
                    let notif = notif_rx.recv().await.expect("notification");
                    if notif.method == "mob/stream_event"
                        && notif.params["stream_id"] == stream_id
                        && notif.params["event"]["payload"]["type"].as_str()
                            == Some("run_completed")
                    {
                        break (Some(notif), None);
                    }
                    if notif.method == "session/stream_event"
                        && notif.params["stream_id"] == session_stream_id
                        && notif.params["event"]["payload"]["type"].as_str()
                            == Some("run_completed")
                    {
                        break (None, Some(notif));
                    }
                }
            },
        )
        .await
        {
            Ok(result) => result,
            Err(_) => {
                let runtime_state = router
                    .runtime_adapter()
                    .runtime_state(&session_id)
                    .await
                    .expect("runtime state");
                let ingress_snapshot = operator_comms
                    .peer_ingress_runtime_snapshot()
                    .await
                    .expect("peer ingress runtime snapshot");
                panic!(
                    "run_completed notification should arrive on one of the subscribed streams; runtime_state={runtime_state:?}; ingress_snapshot={ingress_snapshot:?}"
                );
            }
        };

        assert!(
            mob_run_completed.is_some() || session_run_completed.is_some(),
            "expected a run_completed notification on either mob or session stream"
        );
        assert!(
            mob_run_completed.is_some(),
            "session stream received run_completed but mob stream did not; per-member mob stream forwarding is dropping the event"
        );
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn session_archive_for_mob_session_retires_member() {
        let (router, _notif_rx) = test_router().await;

        let create_resp = router
            .dispatch(make_request(
                "mob/create",
                serde_json::json!({
                    "definition": {
                        "id": "mob-archive-session",
                        "profiles": {
                            "worker": {
                                "model": "claude-sonnet-4-5",
                                "tools": { "comms": true }
                            }
                        }
                    }
                }),
            ))
            .await
            .unwrap();
        let mob_id = result_value(&create_resp)["mob_id"]
            .as_str()
            .unwrap()
            .to_string();

        let _spawn_resp = router
            .dispatch(make_request(
                "mob/spawn",
                serde_json::json!({
                    "mob_id": mob_id,
                    "profile": "worker",
                    "agent_identity": "worker-1",
                    "runtime_mode": "turn_driven"
                }),
            ))
            .await
            .unwrap();
        let session_id =
            resolve_mob_bridge_session_id(&router, "mob-archive-session", "worker-1").await;

        let archive_resp = router
            .dispatch(make_request(
                "session/archive",
                serde_json::json!({ "session_id": session_id.clone() }),
            ))
            .await
            .unwrap();
        assert_eq!(result_value(&archive_resp)["archived"], true);

        let members_resp = router
            .dispatch(make_request(
                "mob/members",
                serde_json::json!({ "mob_id": mob_id }),
            ))
            .await
            .unwrap();
        let members_value = result_value(&members_resp);
        let members = members_value["members"].as_array().unwrap();
        assert!(members.is_empty());

        let interrupt_resp = router
            .dispatch(make_request(
                "turn/interrupt",
                serde_json::json!({ "session_id": session_id }),
            ))
            .await
            .unwrap();
        assert_eq!(
            error_code(&interrupt_resp),
            error::SESSION_NOT_FOUND,
            "archived mob-backed session must reject generic turn/interrupt"
        );
        assert!(
            error_message(&interrupt_resp).contains("Session not found"),
            "unexpected error: {:?}",
            interrupt_resp.error
        );
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn mob_spawned_session_id_supports_session_read() {
        let (router, _notif_rx) = test_router().await;

        let create_resp = router
            .dispatch(make_request(
                "mob/create",
                serde_json::json!({
                    "definition": {
                        "id": "mob-session-read",
                        "profiles": {
                            "worker": {
                                "model": "claude-sonnet-4-5",
                                "tools": { "comms": true }
                            }
                        }
                    }
                }),
            ))
            .await
            .unwrap();
        let mob_id = result_value(&create_resp)["mob_id"]
            .as_str()
            .unwrap()
            .to_string();

        let _spawn_resp = router
            .dispatch(make_request(
                "mob/spawn",
                serde_json::json!({
                    "mob_id": mob_id,
                    "profile": "worker",
                    "agent_identity": "worker-1",
                    "runtime_mode": "turn_driven"
                }),
            ))
            .await
            .unwrap();
        let session_id =
            resolve_mob_bridge_session_id(&router, "mob-session-read", "worker-1").await;

        let read_resp = router
            .dispatch(make_request(
                "session/read",
                serde_json::json!({ "session_id": session_id }),
            ))
            .await
            .unwrap();
        let read_value = result_value(&read_resp);

        assert_eq!(
            read_value["session_id"].as_str().unwrap(),
            session_id,
            "read response: {read_value}"
        );
        // WireSessionInfo uses is_active (bool), not state (string)
        assert!(
            read_value["is_active"].is_boolean(),
            "is_active should be boolean, got: {read_value}"
        );
        // labels may be omitted when empty (skip_serializing_if = "BTreeMap::is_empty")
        assert!(
            read_value.get("labels").is_none() || read_value["labels"].is_object(),
            "labels should be object or absent, got: {read_value}"
        );
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn mob_spawned_session_id_supports_session_history() {
        let (router, _notif_rx) = test_router().await;

        let create_resp = router
            .dispatch(make_request(
                "mob/create",
                serde_json::json!({
                    "definition": {
                        "id": "mob-session-history",
                        "profiles": {
                            "worker": {
                                "model": "claude-sonnet-4-5",
                                "tools": { "comms": true }
                            }
                        }
                    }
                }),
            ))
            .await
            .unwrap();
        let mob_id = result_value(&create_resp)["mob_id"]
            .as_str()
            .unwrap()
            .to_string();

        let _spawn_resp = router
            .dispatch(make_request(
                "mob/spawn",
                serde_json::json!({
                    "mob_id": mob_id,
                    "profile": "worker",
                    "agent_identity": "worker-1",
                    "runtime_mode": "turn_driven"
                }),
            ))
            .await
            .unwrap();
        let session_id =
            resolve_mob_bridge_session_id(&router, "mob-session-history", "worker-1").await;

        let history_resp = router
            .dispatch(make_request(
                "session/history",
                serde_json::json!({ "session_id": session_id }),
            ))
            .await
            .unwrap();
        let history_value = result_value(&history_resp);

        assert_eq!(
            history_value["session_id"].as_str().unwrap(),
            session_id,
            "history response: {history_value}"
        );
        assert!(
            history_value["messages"].is_array(),
            "history should expose a message array, got: {history_value}"
        );
        assert!(
            history_value["message_count"].is_u64(),
            "history should expose a message_count, got: {history_value}"
        );
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn session_inject_context_for_mob_session_targets_backing_service() {
        let (router, _notif_rx) = test_router().await;

        let create_resp = router
            .dispatch(make_request(
                "mob/create",
                serde_json::json!({
                    "definition": {
                        "id": "mob-session-inject-context",
                        "profiles": {
                            "worker": {
                                "model": "claude-sonnet-4-5",
                                "tools": { "comms": true }
                            }
                        }
                    }
                }),
            ))
            .await
            .unwrap();
        let mob_id = result_value(&create_resp)["mob_id"]
            .as_str()
            .unwrap()
            .to_string();

        let _spawn_resp = router
            .dispatch(make_request(
                "mob/spawn",
                serde_json::json!({
                    "mob_id": mob_id,
                    "profile": "worker",
                    "agent_identity": "worker-1",
                    "runtime_mode": "turn_driven"
                }),
            ))
            .await
            .unwrap();
        let session_id =
            resolve_mob_bridge_session_id(&router, "mob-session-inject-context", "worker-1").await;

        let inject_resp = router
            .dispatch(make_request(
                "session/inject_context",
                serde_json::json!({
                    "session_id": session_id,
                    "text": "Coordinate with the lead before acting.",
                    "source": "mob",
                    "idempotency_key": "ctx-worker-1"
                }),
            ))
            .await
            .unwrap();

        assert_eq!(result_value(&inject_resp)["status"], "staged");
    }

    #[tokio::test]
    async fn session_archive_unknown_returns_not_found() {
        let (router, _notif_rx) = test_router().await;
        let response = router
            .dispatch(make_request(
                "session/archive",
                serde_json::json!({ "session_id": SessionId::new() }),
            ))
            .await
            .unwrap();
        assert_eq!(
            response.error.as_ref().map(|e| e.code),
            Some(crate::error::SESSION_NOT_FOUND)
        );
    }

    #[tokio::test]
    async fn notification_sink_reports_overflow_for_stream_events() {
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        let sink = NotificationSink::new(tx);
        let session_id = SessionId::new();
        let envelope = EventEnvelope::new_session(
            session_id.clone(),
            1,
            None,
            AgentEvent::TextDelta {
                delta: "hello".to_string(),
            },
        );

        assert!(
            sink.emit_session_stream_event(&Uuid::new_v4(), 1, &session_id, &envelope)
                .await
                == StreamEmitStatus::Delivered
        );
        assert!(
            sink.emit_session_stream_event(&Uuid::new_v4(), 2, &session_id, &envelope)
                .await
                == StreamEmitStatus::Overflow,
            "second send should surface overflow to the caller"
        );

        let first = rx.recv().await.expect("first notification");
        assert_eq!(first.method, "session/stream_event");
    }

    #[tokio::test]
    async fn session_stream_overflow_emits_terminal_error_outcome() {
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        let sink = NotificationSink::new(tx);
        let session_id = SessionId::new();
        let stream_id = Uuid::new_v4();
        let envelope = EventEnvelope::new_session(
            session_id.clone(),
            1,
            None,
            AgentEvent::TextDelta {
                delta: "hello".to_string(),
            },
        );

        assert_eq!(
            sink.emit_session_stream_event(&stream_id, 1, &session_id, &envelope)
                .await,
            StreamEmitStatus::Delivered
        );
        assert_eq!(
            sink.emit_session_stream_event(&stream_id, 2, &session_id, &envelope)
                .await,
            StreamEmitStatus::Overflow
        );

        let terminal = tokio::spawn({
            let sink = sink.clone();
            let session_id = session_id.clone();
            async move {
                sink.emit_session_stream_end(
                    &stream_id,
                    &session_id,
                    "terminal_error",
                    Some("transport stream notification queue overflow"),
                )
                .await;
            }
        });

        let first = rx.recv().await.expect("first notification");
        assert_eq!(first.method, "session/stream_event");

        let terminal_notification =
            tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
                .await
                .expect("terminal notification should arrive after queue drains")
                .expect("terminal notification");
        terminal.await.expect("terminal send join");
        assert_eq!(terminal_notification.method, "session/stream_end");
        assert_eq!(terminal_notification.params["outcome"], "terminal_error");
        assert_eq!(
            terminal_notification.params["error"]["code"],
            "stream_queue_overflow"
        );
    }

    #[tokio::test]
    async fn session_stream_router_overflow_emits_terminal_error_outcome() {
        let requests = Arc::new(std::sync::Mutex::new(Vec::<Vec<Message>>::new()));
        let llm_client = Arc::new(RecordingMockLlmClient::new(requests));
        let (router, mut notif_rx) =
            test_router_with_llm_and_notification_capacity(llm_client, 1).await;

        let create_resp = router
            .dispatch(make_request(
                "session/create",
                serde_json::json!({ "prompt": "create overflow session" }),
            ))
            .await
            .unwrap();
        let session_id = result_value(&create_resp)["session_id"]
            .as_str()
            .unwrap()
            .to_string();

        drain_notifications(&mut notif_rx).await;

        let open_resp = router
            .dispatch(make_request(
                "session/stream_open",
                serde_json::json!({ "session_id": session_id }),
            ))
            .await
            .unwrap();
        let stream_id = result_value(&open_resp)["stream_id"]
            .as_str()
            .unwrap()
            .to_string();

        let turn_resp = router
            .dispatch(make_request(
                "turn/start",
                serde_json::json!({
                    "session_id": session_id,
                    "prompt": "overflow please"
                }),
            ))
            .await
            .unwrap();
        assert!(turn_resp.error.is_none(), "turn should succeed");

        let notification = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                let notif = notif_rx.recv().await.expect("notification");
                if notif.method == "session/stream_end" {
                    break notif;
                }
            }
        })
        .await
        .expect("session stream end notification");
        assert_eq!(notification.params["stream_id"], stream_id);
        assert_eq!(notification.params["outcome"], "terminal_error");
        assert_eq!(
            notification.params["error"]["code"],
            "stream_queue_overflow"
        );
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn mob_stream_overflow_emits_terminal_error_outcome() {
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        let sink = NotificationSink::new(tx);
        let stream_id = Uuid::new_v4();
        let event = serde_json::json!({
            "event_id": "e1",
            "payload": {
                "type": "text_delta",
                "delta": "hello",
            }
        });

        assert_eq!(
            sink.emit_mob_stream_event(&stream_id, 1, &event).await,
            StreamEmitStatus::Delivered
        );
        assert_eq!(
            sink.emit_mob_stream_event(&stream_id, 2, &event).await,
            StreamEmitStatus::Overflow
        );

        let terminal = tokio::spawn({
            let sink = sink.clone();
            async move {
                sink.emit_mob_stream_end(
                    &stream_id,
                    "terminal_error",
                    Some("transport stream notification queue overflow"),
                )
                .await;
            }
        });

        let first = rx.recv().await.expect("first notification");
        assert_eq!(first.method, "mob/stream_event");

        let terminal_notification =
            tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
                .await
                .expect("terminal notification should arrive after queue drains")
                .expect("terminal notification");
        terminal.await.expect("terminal send join");
        assert_eq!(terminal_notification.method, "mob/stream_end");
        assert_eq!(terminal_notification.params["outcome"], "terminal_error");
        assert_eq!(
            terminal_notification.params["error"]["code"],
            "stream_queue_overflow"
        );
    }

    async fn archived_session_read_remains_available_and_mutations_reject_inner() {
        let (router, _notif_rx) = test_router().await;

        let create_resp = router
            .dispatch(make_request(
                "session/create",
                serde_json::json!({
                    "prompt": "Create a session that will be archived"
                }),
            ))
            .await
            .unwrap();
        let session_id = result_value(&create_resp)["session_id"]
            .as_str()
            .unwrap()
            .to_string();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let archive_resp = router
            .dispatch(make_request(
                "session/archive",
                serde_json::json!({ "session_id": session_id }),
            ))
            .await
            .unwrap();
        assert_eq!(result_value(&archive_resp)["archived"], true);

        let read_resp = router
            .dispatch(make_request(
                "session/read",
                serde_json::json!({ "session_id": session_id }),
            ))
            .await
            .unwrap();
        assert!(
            read_resp.error.is_none(),
            "archived session should remain readable"
        );
        let read = result_value(&read_resp);
        assert_eq!(read["session_id"], session_id);

        let stream_resp = router
            .dispatch(make_request(
                "session/stream_open",
                serde_json::json!({ "session_id": session_id }),
            ))
            .await
            .unwrap();
        assert!(
            stream_resp.error.is_some(),
            "archived session must reject stream_open"
        );

        let inject_resp = router
            .dispatch(make_request(
                "session/inject_context",
                serde_json::json!({
                    "session_id": session_id,
                    "text": "should be rejected"
                }),
            ))
            .await
            .unwrap();
        assert!(
            inject_resp.error.is_some(),
            "archived session must reject staged context append"
        );

        let peers_resp = router
            .dispatch(make_request(
                "comms/peers",
                serde_json::json!({ "session_id": session_id }),
            ))
            .await
            .unwrap();
        assert!(
            peers_resp.error.is_some(),
            "archived session must reject comms/peers"
        );

        let send_resp = router
            .dispatch(make_request(
                "comms/send",
                serde_json::json!({
                    "session_id": session_id,
                    "kind": "peer_message",
                    "target": "nobody",
                    "content": "hello after archive"
                }),
            ))
            .await
            .unwrap();
        assert!(
            send_resp.error.is_some(),
            "archived session must reject comms/send"
        );
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn archived_mob_backed_session_rejects_comms_after_retirement() {
        let (router, _notif_rx) = test_router().await;

        let create_resp = router
            .dispatch(make_request(
                "mob/create",
                serde_json::json!({
                    "definition": {
                        "id": "mob-archive-comms-session",
                        "profiles": {
                            "worker": {
                                "model": "claude-sonnet-4-5",
                                "tools": { "comms": true }
                            }
                        }
                    }
                }),
            ))
            .await
            .unwrap();
        let mob_id = result_value(&create_resp)["mob_id"]
            .as_str()
            .unwrap()
            .to_string();

        let _spawn_resp = router
            .dispatch(make_request(
                "mob/spawn",
                serde_json::json!({
                    "mob_id": mob_id,
                    "profile": "worker",
                    "agent_identity": "worker-1",
                    "runtime_mode": "turn_driven"
                }),
            ))
            .await
            .unwrap();
        let session_id =
            resolve_mob_bridge_session_id(&router, "mob-archive-comms-session", "worker-1").await;

        let archive_resp = router
            .dispatch(make_request(
                "session/archive",
                serde_json::json!({ "session_id": session_id }),
            ))
            .await
            .unwrap();
        assert_eq!(result_value(&archive_resp)["archived"], true);

        let peers_resp = router
            .dispatch(make_request(
                "comms/peers",
                serde_json::json!({ "session_id": session_id }),
            ))
            .await
            .unwrap();
        assert!(
            peers_resp.error.is_some(),
            "retired mob member must reject comms/peers"
        );

        let send_resp = router
            .dispatch(make_request(
                "comms/send",
                serde_json::json!({
                    "session_id": session_id,
                    "kind": "peer_message",
                    "target": "worker-2",
                    "content": "hello after archive"
                }),
            ))
            .await
            .unwrap();
        assert!(
            send_resp.error.is_some(),
            "retired mob member must reject comms/send"
        );
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn mob_send_route_is_unavailable_while_archive_retirement_is_in_flight() {
        let (router, _notif_rx) = test_router_with_mob_state(
            meerkat_mob_mcp::MobMcpState::new_in_memory_with_archive_delay(250),
        )
        .await;

        let create_resp = router
            .dispatch(make_request(
                "mob/create",
                serde_json::json!({
                    "definition": {
                        "id": "mob-retiring-send",
                        "profiles": {
                            "lead": {
                                "model": "claude-sonnet-4-5",
                                "external_addressable": true,
                                "tools": { "comms": true }
                            }
                        }
                    }
                }),
            ))
            .await
            .unwrap();
        let mob_id = result_value(&create_resp)["mob_id"]
            .as_str()
            .unwrap()
            .to_string();

        let _spawn_resp = router
            .dispatch(make_request(
                "mob/spawn",
                serde_json::json!({
                    "mob_id": &mob_id,
                    "profile": "lead",
                    "agent_identity": "lead-1",
                    "runtime_mode": "turn_driven"
                }),
            ))
            .await
            .unwrap();
        let session_id =
            resolve_mob_bridge_session_id(&router, "mob-retiring-send", "lead-1").await;

        let archive = {
            let router = router.clone();
            let session_id = session_id.clone();
            tokio::spawn(async move {
                router
                    .dispatch(make_request(
                        "session/archive",
                        serde_json::json!({ "session_id": &session_id }),
                    ))
                    .await
                    .expect("archive response")
            })
        };

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let members_resp = router
            .dispatch(make_request(
                "mob/members",
                serde_json::json!({ "mob_id": &mob_id }),
            ))
            .await
            .unwrap();
        let members_value = result_value(&members_resp);
        let members = members_value["members"].as_array().expect("members array");
        assert_eq!(members.len(), 1, "retiring member should remain observable");
        assert_eq!(members[0]["agent_identity"], "lead-1");
        assert_eq!(members[0]["state"], "retiring");

        let send_resp = router
            .dispatch(make_request(
                "mob/send",
                serde_json::json!({
                    "mob_id": &mob_id,
                    "agent_identity": "lead-1",
                    "content": "do work while retiring"
                }),
            ))
            .await
            .unwrap();
        assert_eq!(error_code(&send_resp), error::METHOD_NOT_FOUND);

        let archive_resp = archive.await.expect("archive join");
        assert_eq!(result_value(&archive_resp)["archived"], true);
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn session_stream_open_emits_terminal_notification_when_session_ends() {
        let (router, mut notif_rx) = test_router().await;

        let create_resp = router
            .dispatch(make_request(
                "mob/create",
                serde_json::json!({
                    "definition": {
                        "id": "mob-session-terminal",
                        "profiles": {
                            "worker": {
                                "model": "claude-sonnet-4-5",
                                "tools": { "comms": true }
                            }
                        }
                    }
                }),
            ))
            .await
            .unwrap();
        let mob_id = result_value(&create_resp)["mob_id"]
            .as_str()
            .unwrap()
            .to_string();

        let _spawn_resp = router
            .dispatch(make_request(
                "mob/spawn",
                serde_json::json!({
                    "mob_id": mob_id,
                    "profile": "worker",
                    "agent_identity": "worker-1",
                    "runtime_mode": "turn_driven"
                }),
            ))
            .await
            .unwrap();
        let session_id =
            resolve_mob_bridge_session_id(&router, "mob-session-terminal", "worker-1").await;

        let open_resp = router
            .dispatch(make_request(
                "session/stream_open",
                serde_json::json!({ "session_id": session_id }),
            ))
            .await
            .unwrap();
        let stream_id = result_value(&open_resp)["stream_id"]
            .as_str()
            .unwrap()
            .to_string();

        let archive_resp = router
            .dispatch(make_request(
                "session/archive",
                serde_json::json!({ "session_id": session_id }),
            ))
            .await
            .unwrap();
        assert_eq!(result_value(&archive_resp)["archived"], true);

        let notification = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                let notif = notif_rx.recv().await.expect("notification");
                if notif.method == "session/stream_end" {
                    break notif;
                }
            }
        })
        .await
        .expect("session stream end notification");
        assert_eq!(notification.method, "session/stream_end");
        assert_eq!(notification.params["stream_id"], stream_id);
        assert_eq!(notification.params["ended"], true);
        assert_eq!(notification.params["outcome"], "remote_end");
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn mob_stream_close_removes_forwarder_from_active_map() {
        let (router, mut notif_rx) = test_router().await;

        let create_resp = router
            .dispatch(make_request(
                "mob/create",
                serde_json::json!({
                    "definition": {
                        "id": "test_mob",
                        "profiles": {
                            "worker": {
                                "model": "claude-sonnet-4-6",
                                "tools": { "comms": true }
                            }
                        }
                    }
                }),
            ))
            .await
            .unwrap();
        let created = result_value(&create_resp);
        let mob_id = created["mob_id"].as_str().unwrap().to_string();

        let open_resp = router
            .dispatch(make_request(
                "mob/stream_open",
                serde_json::json!({ "mob_id": mob_id }),
            ))
            .await
            .unwrap();
        let opened = result_value(&open_resp);
        let stream_id = opened["stream_id"].as_str().unwrap().to_string();
        assert_eq!(router.active_mob_streams.lock().await.len(), 1);

        let close_resp = router
            .dispatch(make_request(
                "mob/stream_close",
                serde_json::json!({ "stream_id": stream_id }),
            ))
            .await
            .unwrap();
        let closed = result_value(&close_resp);
        assert_eq!(closed["closed"], true);
        assert_eq!(closed["already_closed"], false);
        assert_eq!(router.active_mob_streams.lock().await.len(), 0);

        let notification = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                let notif = notif_rx.recv().await.expect("notification");
                if notif.method == "mob/stream_end" {
                    break notif;
                }
            }
        })
        .await
        .expect("mob explicit-close notification");
        assert_eq!(notification.params["stream_id"], stream_id);
        assert_eq!(notification.params["outcome"], "explicit_close");
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn mob_stream_open_close_roundtrip() {
        let (router, _notif_rx) = test_router().await;

        // Create a mob first.
        let create_resp = router
            .dispatch(make_request(
                "mob/create",
                serde_json::json!({
                    "definition": {
                        "id": "test_mob",
                        "profiles": {
                            "worker": {
                                "model": "claude-sonnet-4-6",
                                "tools": { "comms": true }
                            }
                        }
                    }
                }),
            ))
            .await
            .unwrap();
        let created = result_value(&create_resp);
        let mob_id = created["mob_id"].as_str().unwrap().to_string();

        // Open a mob-wide stream.
        let open_resp = router
            .dispatch(make_request(
                "mob/stream_open",
                serde_json::json!({ "mob_id": mob_id }),
            ))
            .await
            .unwrap();
        let opened = result_value(&open_resp);
        assert_eq!(opened["opened"], true);
        let stream_id = opened["stream_id"].as_str().unwrap().to_string();

        // Close the stream.
        let close_resp = router
            .dispatch(make_request(
                "mob/stream_close",
                serde_json::json!({ "stream_id": stream_id }),
            ))
            .await
            .unwrap();
        let closed = result_value(&close_resp);
        assert_eq!(closed["closed"], true);
        assert_eq!(closed["already_closed"], false);

        // Idempotent: second close succeeds with already_closed=true.
        let close_again_resp = router
            .dispatch(make_request(
                "mob/stream_close",
                serde_json::json!({ "stream_id": stream_id }),
            ))
            .await
            .unwrap();
        let closed_again = result_value(&close_again_resp);
        assert_eq!(closed_again["closed"], true);
        assert_eq!(closed_again["already_closed"], true);
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn mob_stream_close_unknown_returns_not_already_closed() {
        let (router, _notif_rx) = test_router().await;

        let close_resp = router
            .dispatch(make_request(
                "mob/stream_close",
                serde_json::json!({ "stream_id": "00000000-0000-0000-0000-000000000000" }),
            ))
            .await
            .unwrap();
        let closed = result_value(&close_resp);
        assert_eq!(closed["closed"], true);
        assert_eq!(closed["already_closed"], false);
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn mob_stream_open_unknown_mob_returns_error() {
        let (router, _notif_rx) = test_router().await;

        let open_resp = router
            .dispatch(make_request(
                "mob/stream_open",
                serde_json::json!({ "mob_id": "nonexistent-mob" }),
            ))
            .await
            .unwrap();
        assert_eq!(error_code(&open_resp), error::INVALID_PARAMS);
    }

    #[tokio::test]
    async fn turn_start_accepts_flow_tool_overlay() {
        let (router, _notif_rx) = test_router().await;
        let create_req = make_request("session/create", serde_json::json!({"prompt":"Hello"}));
        let create_resp = router.dispatch(create_req).await.unwrap();
        let created = result_value(&create_resp);
        let session_id = created["session_id"]
            .as_str()
            .expect("session_id")
            .to_string();

        let turn_req = make_request(
            "turn/start",
            serde_json::json!({
                "session_id": session_id,
                "prompt": "continue with overlay",
                "flow_tool_overlay": {
                    "allowed_tools": [],
                    "blocked_tools": []
                }
            }),
        );
        let turn_resp = router.dispatch(turn_req).await.unwrap();
        let turned = result_value(&turn_resp);
        assert_eq!(
            turned["session_id"].as_str().expect("session id"),
            created["session_id"].as_str().expect("session id")
        );
    }

    /// 2. Unknown method returns METHOD_NOT_FOUND error.
    #[tokio::test]
    async fn unknown_method_returns_method_not_found() {
        let (router, _notif_rx) = test_router().await;
        let req = make_request_no_params("foo/bar");

        let resp = router.dispatch(req).await.unwrap();
        assert_eq!(error_code(&resp), error::METHOD_NOT_FOUND);
    }

    #[tokio::test]
    async fn retired_runtime_session_control_methods_fail_closed_and_leave_state_unchanged() {
        let (router, _notif_rx) = test_router_with_v9_runtime().await;
        let create_resp = router
            .dispatch(make_request(
                "session/create",
                serde_json::json!({
                    "prompt": "deferred retired-route fixture",
                    "initial_turn": "deferred"
                }),
            ))
            .await
            .expect("create response");
        let session_id = result_value(&create_resp)["session_id"]
            .as_str()
            .expect("session_id")
            .to_string();
        let parsed_session_id =
            SessionId::parse(&session_id).expect("created session id should parse");
        let before_registered = router
            .runtime_adapter()
            .contains_session(&parsed_session_id)
            .await;
        let before_read = router
            .dispatch(make_request(
                "session/read",
                serde_json::json!({ "session_id": &session_id }),
            ))
            .await
            .expect("read response");
        let before_session = result_value(&before_read).clone();

        let input_id = meerkat_core::InputId::new().to_string();
        let cases = vec![
            (
                "runtime/session_status",
                serde_json::json!({ "session_id": &session_id }),
            ),
            (
                "runtime/session_submit",
                serde_json::json!({
                    "session_id": &session_id,
                    "input": {
                        "input_type": "prompt",
                        "header": {
                            "id": meerkat_core::InputId::new(),
                            "timestamp": "2026-03-12T00:00:00Z",
                            "source": { "type": "operator" },
                            "durability": "durable",
                            "visibility": {
                                "transcript_eligible": true,
                                "operator_eligible": true
                            }
                        },
                        "text": "must not enter runtime queue"
                    }
                }),
            ),
            (
                "runtime/session_submission",
                serde_json::json!({
                    "session_id": &session_id,
                    "input_id": input_id,
                }),
            ),
            (
                "runtime/session_submissions",
                serde_json::json!({ "session_id": &session_id }),
            ),
            (
                "runtime/session_retire",
                serde_json::json!({ "session_id": &session_id }),
            ),
            (
                "runtime/session_reset",
                serde_json::json!({ "session_id": &session_id }),
            ),
        ];

        for (method, params) in cases {
            let resp = router
                .dispatch(make_request(method, params))
                .await
                .expect("response");
            assert_eq!(
                error_code(&resp),
                error::METHOD_NOT_FOUND,
                "{method} must fail closed after retirement"
            );
        }

        let after_registered = router
            .runtime_adapter()
            .contains_session(&parsed_session_id)
            .await;
        assert_eq!(
            after_registered, before_registered,
            "retired runtime/session_* methods must not register or unregister runtime state"
        );
        let after_read = router
            .dispatch(make_request(
                "session/read",
                serde_json::json!({ "session_id": &session_id }),
            ))
            .await
            .expect("read response");
        let after_session = result_value(&after_read);
        assert_eq!(after_session["session_id"], before_session["session_id"]);
        assert_eq!(
            after_session["message_count"],
            before_session["message_count"]
        );
        assert_eq!(
            after_session["last_assistant_text"],
            before_session["last_assistant_text"]
        );
    }

    /// 3. `session/create` happy path - creates session, runs first turn, returns result.
    #[tokio::test]
    async fn session_create_returns_session_id_and_result() {
        let (router, _notif_rx) = test_router().await;
        let req = make_request(
            "session/create",
            serde_json::json!({
                "prompt": "Say hello"
            }),
        );

        let resp = router.dispatch(req).await.unwrap();
        let result = result_value(&resp);

        // session_id should be a non-empty string
        let sid = result["session_id"].as_str().unwrap();
        assert!(!sid.is_empty());

        // text should contain the mock response
        let text = result["text"].as_str().unwrap();
        assert!(
            text.contains("Hello from mock"),
            "Expected mock text, got: {text}"
        );

        // turns and tool_calls should be present
        assert!(result["turns"].is_u64());
        assert!(result["tool_calls"].is_u64());

        // usage should be present
        assert!(result["usage"]["input_tokens"].is_u64());
        assert!(result["usage"]["output_tokens"].is_u64());
    }

    /// 4. `session/create` with missing prompt returns INVALID_PARAMS.
    #[tokio::test]
    async fn session_create_missing_prompt_returns_invalid_params() {
        let (router, _notif_rx) = test_router().await;
        let req = make_request(
            "session/create",
            serde_json::json!({
                "model": "claude-sonnet-4-5"
            }),
        );

        let resp = router.dispatch(req).await.unwrap();
        assert_eq!(error_code(&resp), error::INVALID_PARAMS);
    }

    #[tokio::test]
    async fn session_create_rejects_reserved_mob_peer_meta_labels() {
        let (router, _notif_rx) = test_router().await;
        let req = make_request(
            "session/create",
            serde_json::json!({
                "prompt": "hello",
                "peer_meta": {
                    "labels": {
                        "mob_id": "team"
                    }
                }
            }),
        );

        let resp = router.dispatch(req).await.unwrap();
        assert_eq!(error_code(&resp), error::INVALID_PARAMS);
        assert!(
            error_message(&resp).contains("reserved for Meerkat-owned runtime facts"),
            "reserved mob label rejection should explain the trust boundary"
        );
    }

    #[tokio::test]
    async fn events_latest_cursor_reports_unsupported_without_event_projection() {
        let (router, _notif_rx) = test_router().await;
        let req = make_request(
            "events/latest_cursor",
            serde_json::json!({
                "scope": {
                    "type": "session",
                    "session_id": meerkat_core::SessionId::new().to_string()
                }
            }),
        );

        let resp = router.dispatch(req).await.unwrap();
        assert_eq!(error_code(&resp), error::INVALID_REQUEST);
        assert!(
            error_message(&resp).contains("event replay is not enabled"),
            "unsupported replay error should be explicit: {}",
            error_message(&resp)
        );
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn mob_send_route_is_not_found() {
        let (router, _notif_rx) = test_router().await;
        let req = make_request(
            "mob/send",
            serde_json::json!({
                "mob_id": "mob-1",
                "agent_identity": "worker-1",
                "message": "legacy payload"
            }),
        );

        let resp = router.dispatch(req).await.unwrap();
        assert_eq!(error_code(&resp), error::METHOD_NOT_FOUND);
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn mob_wait_kickoff_rejects_empty_mob_id() {
        let (router, _notif_rx) = test_router().await;
        let resp = router
            .dispatch(make_request(
                "mob/wait_kickoff",
                serde_json::json!({
                    "mob_id": "",
                    "timeout_ms": 1000
                }),
            ))
            .await
            .expect("response");
        assert_eq!(error_code(&resp), error::INVALID_PARAMS);
        assert!(
            error_message(&resp).contains("mob_id must not be empty"),
            "empty mob_id should be rejected before dispatching to mob state"
        );
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn mob_wire_rejects_legacy_endpoint_shapes() {
        let (router, _notif_rx) = test_router().await;
        let req = make_request(
            "mob/wire",
            serde_json::json!({
                "mob_id": "mob-1",
                "local": "worker-a",
                "target": { "local": "worker-b" }
            }),
        );

        let resp = router.dispatch(req).await.unwrap();
        assert_eq!(error_code(&resp), error::INVALID_PARAMS);
        assert!(
            error_message(&resp).contains("unknown field `local`"),
            "legacy mob/wire payloads should be rejected after the 0.5 clean cut"
        );
    }

    #[tokio::test]
    async fn turn_start_capacity_full_rejects_before_executor_registration() {
        let (router, _notif_rx) = test_router_with_v9_runtime_and_max_sessions(1).await;
        let target_resp = router
            .dispatch(make_request(
                "session/create",
                serde_json::json!({ "prompt": "completed session" }),
            ))
            .await
            .unwrap();
        let target_session_id = SessionId::parse(
            result_value(&target_resp)["session_id"]
                .as_str()
                .expect("target session id"),
        )
        .expect("valid target session id");
        router
            .runtime_adapter()
            .unregister_session(&target_session_id)
            .await;
        assert!(
            !router
                .runtime_adapter()
                .contains_session(&target_session_id)
                .await,
            "test setup should leave the persisted target without runtime registration"
        );

        let filler_resp = router
            .dispatch(make_request(
                "session/create",
                serde_json::json!({
                    "prompt": "capacity filler",
                    "initial_turn": "deferred"
                }),
            ))
            .await
            .unwrap();
        assert!(
            result_value(&filler_resp)["session_id"].is_string(),
            "deferred filler should reserve active capacity"
        );

        let rejected = router
            .dispatch(make_request(
                "turn/start",
                serde_json::json!({
                    "session_id": target_session_id.to_string(),
                    "prompt": "must not register executor before admission"
                }),
            ))
            .await
            .unwrap();
        assert!(
            error_message(&rejected).contains("Max sessions"),
            "capacity-full turn/start should reject before runtime registration: {}",
            error_message(&rejected)
        );
        assert!(
            !router
                .runtime_adapter()
                .contains_session(&target_session_id)
                .await,
            "capacity-full turn/start must not register a runtime executor"
        );
    }

    /// 5. `session/list` returns the list of sessions after creating one.
    #[tokio::test]
    async fn session_list_returns_sessions() {
        let (router, _notif_rx) = test_router().await;

        // Create a session first
        let create_req = make_request("session/create", serde_json::json!({"prompt": "Hello"}));
        let create_resp = router.dispatch(create_req).await.unwrap();
        let created = result_value(&create_resp);
        let created_id = created["session_id"].as_str().unwrap();

        // Now list sessions
        let list_req = make_request_no_params("session/list");
        let list_resp = router.dispatch(list_req).await.unwrap();
        let list_result = result_value(&list_resp);

        let sessions = list_result["sessions"].as_array().unwrap();
        assert!(!sessions.is_empty(), "Should have at least one session");

        // Find our session in the list
        let found = sessions
            .iter()
            .any(|s| s["session_id"].as_str() == Some(created_id));
        assert!(found, "Created session should appear in list");
    }

    /// 5b. `session/inject_context` stages runtime system context for the session.
    #[tokio::test]
    async fn session_inject_context_returns_staged_status() {
        let (router, _notif_rx) = test_router().await;

        let create_req = make_request("session/create", serde_json::json!({"prompt": "Hello"}));
        let create_resp = router.dispatch(create_req).await.unwrap();
        let created = result_value(&create_resp);
        let session_id = created["session_id"].as_str().unwrap().to_string();

        let inject_req = make_request(
            "session/inject_context",
            serde_json::json!({
                "session_id": session_id,
                "text": "Coordinate with the orchestrator.",
                "source": "mob",
                "idempotency_key": "ctx-router-test"
            }),
        );
        let inject_resp = router.dispatch(inject_req).await.unwrap();
        let injected = result_value(&inject_resp);
        assert_eq!(injected["status"], "staged");
    }

    #[tokio::test]
    async fn session_inject_context_is_consumed_by_next_turn_exactly_once() {
        let recorded_requests = Arc::new(std::sync::Mutex::new(Vec::<Vec<Message>>::new()));
        let (router, mut notif_rx) = test_router_with_llm(Arc::new(RecordingMockLlmClient::new(
            Arc::clone(&recorded_requests),
        )))
        .await;

        let create_resp = router
            .dispatch(make_request(
                "session/create",
                serde_json::json!({ "prompt": "Hello" }),
            ))
            .await
            .unwrap();
        let session_id = result_value(&create_resp)["session_id"]
            .as_str()
            .unwrap()
            .to_string();
        drain_notifications(&mut notif_rx).await;
        recorded_requests
            .lock()
            .expect("recorded requests lock poisoned")
            .clear();

        let injected_text = "Coordinate with the orchestrator exactly once.";
        let inject_resp = router
            .dispatch(make_request(
                "session/inject_context",
                serde_json::json!({
                    "session_id": &session_id,
                    "text": injected_text,
                    "source": "mob",
                    "idempotency_key": "ctx-next-turn-once"
                }),
            ))
            .await
            .unwrap();
        assert_eq!(result_value(&inject_resp)["status"], "staged");

        let first_turn_resp = router
            .dispatch(make_request(
                "turn/start",
                serde_json::json!({
                    "session_id": &session_id,
                    "prompt": "first follow up"
                }),
            ))
            .await
            .unwrap();
        assert!(first_turn_resp.error.is_none(), "first turn should succeed");
        let first_prompt = next_run_started_prompt(&mut notif_rx).await;
        assert!(
            first_prompt.contains("first follow up"),
            "turn notification must still reflect the user prompt: {first_prompt}"
        );
        let first_request = recorded_requests
            .lock()
            .expect("recorded requests lock poisoned")
            .last()
            .cloned()
            .expect("first follow-up request");
        let first_system_prompt =
            system_prompt_from_request(&first_request).expect("system prompt on first turn");
        assert!(
            first_system_prompt.contains(injected_text),
            "next eligible turn must include staged context in the LLM system prompt: {first_system_prompt}"
        );
        assert_eq!(first_system_prompt.matches(injected_text).count(), 1);
        recorded_requests
            .lock()
            .expect("recorded requests lock poisoned")
            .clear();

        let second_turn_resp = router
            .dispatch(make_request(
                "turn/start",
                serde_json::json!({
                    "session_id": &session_id,
                    "prompt": "second follow up"
                }),
            ))
            .await
            .unwrap();
        assert!(
            second_turn_resp.error.is_none(),
            "second turn should succeed"
        );
        let second_prompt = next_run_started_prompt(&mut notif_rx).await;
        assert!(
            second_prompt.contains("second follow up"),
            "turn notification must still reflect the user prompt: {second_prompt}"
        );
        let second_request = recorded_requests
            .lock()
            .expect("recorded requests lock poisoned")
            .last()
            .cloned()
            .expect("second follow-up request");
        let second_system_prompt =
            system_prompt_from_request(&second_request).expect("system prompt on second turn");
        assert!(
            !second_system_prompt.contains(injected_text),
            "staged context must be consumed exactly once, not replayed on later turns: {second_system_prompt}"
        );
    }

    #[tokio::test]
    async fn session_inject_context_during_active_turn_waits_for_next_rpc_turn() {
        let recorded_requests = Arc::new(std::sync::Mutex::new(Vec::<Vec<Message>>::new()));
        let (router, _notif_rx) = test_router_with_llm(Arc::new(
            RecordingMockLlmClient::with_delay(Arc::clone(&recorded_requests), 200),
        ))
        .await;

        let create_resp = router
            .dispatch(make_request(
                "session/create",
                serde_json::json!({ "prompt": "Hello" }),
            ))
            .await
            .unwrap();
        let session_id = result_value(&create_resp)["session_id"]
            .as_str()
            .unwrap()
            .to_string();
        recorded_requests
            .lock()
            .expect("recorded requests lock poisoned")
            .clear();

        let first_turn = {
            let router = router.clone();
            let session_id = session_id.clone();
            tokio::spawn(async move {
                router
                    .dispatch(make_request(
                        "turn/start",
                        serde_json::json!({
                            "session_id": &session_id,
                            "prompt": "first turn"
                        }),
                    ))
                    .await
                    .expect("first turn response")
            })
        };

        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if !recorded_requests
                    .lock()
                    .expect("recorded requests lock poisoned")
                    .is_empty()
                {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("first request should reach the LLM");

        let injected_text = "Late staged context";
        let inject_resp = router
            .dispatch(make_request(
                "session/inject_context",
                serde_json::json!({
                    "session_id": &session_id,
                    "text": injected_text,
                    "source": "mob",
                    "idempotency_key": "ctx-rpc-during-active-turn"
                }),
            ))
            .await
            .unwrap();
        assert_eq!(result_value(&inject_resp)["status"], "staged");

        let first_turn_resp = first_turn.await.expect("first turn join");
        assert!(
            first_turn_resp.error.is_none(),
            "first turn should still complete"
        );
        let first_request = recorded_requests
            .lock()
            .expect("recorded requests lock poisoned")
            .first()
            .cloned()
            .expect("first request");
        let first_system_prompt =
            system_prompt_from_request(&first_request).expect("system prompt on first turn");
        assert!(
            !first_system_prompt.contains(injected_text),
            "context appended during an active RPC turn must not affect the in-flight request"
        );

        let second_turn_resp = router
            .dispatch(make_request(
                "turn/start",
                serde_json::json!({
                    "session_id": &session_id,
                    "prompt": "second turn"
                }),
            ))
            .await
            .unwrap();
        assert!(
            second_turn_resp.error.is_none(),
            "second turn should succeed"
        );
        let second_request = recorded_requests
            .lock()
            .expect("recorded requests lock poisoned")
            .last()
            .cloned()
            .expect("second request");
        let second_system_prompt =
            system_prompt_from_request(&second_request).expect("system prompt on second turn");
        assert!(
            second_system_prompt.contains(injected_text),
            "context appended during an active RPC turn must apply on the next eligible turn"
        );
    }

    #[tokio::test]
    async fn session_inject_context_duplicate_idempotency_key_does_not_double_stage() {
        let recorded_requests = Arc::new(std::sync::Mutex::new(Vec::<Vec<Message>>::new()));
        let (router, mut notif_rx) = test_router_with_llm(Arc::new(RecordingMockLlmClient::new(
            Arc::clone(&recorded_requests),
        )))
        .await;

        let create_resp = router
            .dispatch(make_request(
                "session/create",
                serde_json::json!({ "prompt": "Hello" }),
            ))
            .await
            .unwrap();
        let session_id = result_value(&create_resp)["session_id"]
            .as_str()
            .unwrap()
            .to_string();
        drain_notifications(&mut notif_rx).await;
        recorded_requests
            .lock()
            .expect("recorded requests lock poisoned")
            .clear();

        let injected_text = "Only stage this once.";
        let first_inject = router
            .dispatch(make_request(
                "session/inject_context",
                serde_json::json!({
                    "session_id": &session_id,
                    "text": injected_text,
                    "source": "mob",
                    "idempotency_key": "ctx-dedup"
                }),
            ))
            .await
            .unwrap();
        assert_eq!(result_value(&first_inject)["status"], "staged");

        let second_inject = router
            .dispatch(make_request(
                "session/inject_context",
                serde_json::json!({
                    "session_id": &session_id,
                    "text": injected_text,
                    "source": "mob",
                    "idempotency_key": "ctx-dedup"
                }),
            ))
            .await
            .unwrap();
        assert_eq!(result_value(&second_inject)["status"], "duplicate");

        let turn_resp = router
            .dispatch(make_request(
                "turn/start",
                serde_json::json!({
                    "session_id": &session_id,
                    "prompt": "follow up"
                }),
            ))
            .await
            .unwrap();
        assert!(turn_resp.error.is_none(), "turn should succeed");
        let _prompt = next_run_started_prompt(&mut notif_rx).await;

        let request = recorded_requests
            .lock()
            .expect("recorded requests lock poisoned")
            .last()
            .cloned()
            .expect("follow-up request");
        let system_prompt =
            system_prompt_from_request(&request).expect("system prompt on follow-up turn");
        assert_eq!(
            system_prompt.matches(injected_text).count(),
            1,
            "duplicate idempotency keys must not enqueue multiple staged copies: {system_prompt}"
        );
    }

    /// 6. `session/archive` removes a session.
    #[tokio::test]
    async fn session_archive_removes_session() {
        let (router, _notif_rx) = test_router().await;

        // Create a session
        let create_req = make_request("session/create", serde_json::json!({"prompt": "Hello"}));
        let create_resp = router.dispatch(create_req).await.unwrap();
        let created = result_value(&create_resp);
        let session_id = created["session_id"].as_str().unwrap().to_string();

        // Archive it
        let archive_req = make_request(
            "session/archive",
            serde_json::json!({"session_id": session_id}),
        );
        let archive_resp = router.dispatch(archive_req).await.unwrap();
        let archive_result = result_value(&archive_resp);
        assert_eq!(archive_result["archived"], true);

        // Verify it's gone from list
        let list_req = make_request_no_params("session/list");
        let list_resp = router.dispatch(list_req).await.unwrap();
        let list_result = result_value(&list_resp);
        let sessions = list_result["sessions"].as_array().unwrap();
        let found = sessions
            .iter()
            .any(|s| s["session_id"].as_str() == Some(&session_id));
        assert!(!found, "Archived session should not appear in list");
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn session_archive_does_not_mask_live_mob_retire_failure_with_child_cleanup_anchor() {
        let mob_state = meerkat_mob_mcp::MobMcpState::new_in_memory();
        let (router, _notif_rx) = test_router_with_mob_state(mob_state.clone()).await;
        let (_parent_mob_id, member_session_id, _events) =
            insert_router_archive_live_member_with_optional_retire_event_failure(&mob_state, true)
                .await;
        let member_session_key = member_session_id.to_string();
        let (child_mob_id, child_events) =
            insert_router_archive_partial_destroy_mob(&mob_state, &member_session_key).await;
        child_events.allow_clear();

        let archive_resp = router
            .dispatch(make_request(
                "session/archive",
                serde_json::json!({"session_id": member_session_key}),
            ))
            .await
            .unwrap();

        assert_eq!(
            error_code(&archive_resp),
            error::INTERNAL_ERROR,
            "live mob retire failure must not be success-classified through child cleanup"
        );
        let error = archive_resp.error.expect("archive should fail");
        assert!(
            error
                .message
                .contains("forced router mob retire event failure"),
            "archive should surface the live mob retire failure: {error:?}"
        );
        assert!(
            mob_state.owns_live_bridge_session(&member_session_id).await,
            "failed live retire must leave the parent mob ownership anchor intact"
        );
        assert!(
            mob_state
                .has_bridge_session_scoped_mobs(&member_session_key)
                .await,
            "child cleanup anchor must still exist after parent retire failure"
        );
        assert!(
            mob_state.handle_for(&child_mob_id).await.is_ok(),
            "child cleanup must not be run as a success fallback while the parent retire failed"
        );
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn session_archive_mob_member_surfaces_incomplete_child_cleanup_data() {
        let mob_state = meerkat_mob_mcp::MobMcpState::new_in_memory();
        let (router, _notif_rx) = test_router_with_mob_state(mob_state.clone()).await;
        let (_parent_mob_id, member_session_id, _events) =
            insert_router_archive_live_member_with_optional_retire_event_failure(&mob_state, false)
                .await;
        let member_session_key = member_session_id.to_string();
        let (child_mob_id, child_events) =
            insert_router_archive_partial_destroy_mob(&mob_state, &member_session_key).await;

        let archive_resp = router
            .dispatch(make_request(
                "session/archive",
                serde_json::json!({"session_id": member_session_key}),
            ))
            .await
            .unwrap();

        assert_eq!(error_code(&archive_resp), error::INTERNAL_ERROR);
        let data = archive_resp
            .error
            .expect("partial child cleanup should fail session/archive")
            .data
            .expect("typed incomplete cleanup data");
        assert_eq!(
            data.get("code").and_then(serde_json::Value::as_str),
            Some("mob_destroy_incomplete")
        );
        assert_eq!(
            data.get("retryable").and_then(serde_json::Value::as_bool),
            Some(true)
        );
        assert!(
            mob_state.handle_for(&child_mob_id).await.is_ok(),
            "incomplete child cleanup must retain the child mob retry anchor"
        );

        let retry_resp = router
            .dispatch(make_request(
                "session/archive",
                serde_json::json!({"session_id": member_session_key}),
            ))
            .await
            .unwrap();
        assert_eq!(error_code(&retry_resp), error::INTERNAL_ERROR);
        let retry_data = retry_resp
            .error
            .expect("retry should still report retained child cleanup")
            .data
            .expect("typed retry incomplete cleanup data");
        assert_eq!(
            retry_data.get("code").and_then(serde_json::Value::as_str),
            Some("mob_destroy_incomplete")
        );

        child_events.allow_clear();
        let complete_retry_resp = router
            .dispatch(make_request(
                "session/archive",
                serde_json::json!({"session_id": member_session_key}),
            ))
            .await
            .unwrap();
        assert!(
            complete_retry_resp.error.is_none(),
            "retry after child event store recovery should complete cleanup: {complete_retry_resp:?}"
        );
        assert!(
            mob_state.handle_for(&child_mob_id).await.is_err(),
            "successful retry must remove the child mob retry anchor"
        );
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn session_archive_does_not_mask_post_retire_mob_archive_failure_with_child_cleanup_anchor()
     {
        let (mob_state, archive_failures) =
            meerkat_mob_mcp::MobMcpState::new_in_memory_with_archive_failure_control();
        let (router, _notif_rx) = test_router_with_mob_state(mob_state.clone()).await;
        let (_parent_mob_id, member_session_id, _events) =
            insert_router_archive_live_member_with_optional_retire_event_failure(&mob_state, false)
                .await;
        archive_failures
            .fail_archive(
                member_session_id.clone(),
                "forced router mob archive failure after retire event",
            )
            .await;
        let member_session_key = member_session_id.to_string();
        let (child_mob_id, child_events) =
            insert_router_archive_partial_destroy_mob(&mob_state, &member_session_key).await;
        child_events.allow_clear();

        let archive_resp = router
            .dispatch(make_request(
                "session/archive",
                serde_json::json!({"session_id": member_session_key}),
            ))
            .await
            .unwrap();

        assert_eq!(
            error_code(&archive_resp),
            error::INTERNAL_ERROR,
            "post-retire archive failure must not be success-classified through child cleanup"
        );
        let error = archive_resp.error.expect("archive should fail");
        assert!(
            error
                .message
                .contains("forced router mob archive failure after retire event"),
            "archive should surface the failed parent bridge-session archive: {error:?}"
        );
        assert!(
            mob_state
                .session_service()
                .has_live_session(&member_session_id)
                .await
                .expect("check failed parent bridge session"),
            "failed ArchiveSession must leave the parent bridge session retry anchor intact"
        );
        assert!(
            mob_state
                .has_bridge_session_scoped_mobs(&member_session_key)
                .await,
            "child cleanup anchor must still exist after parent archive failure"
        );
        assert!(
            mob_state.handle_for(&child_mob_id).await.is_ok(),
            "child cleanup must not be run as a success fallback while parent archive failed"
        );

        archive_failures
            .clear_archive_failure(&member_session_id)
            .await;
        let retry_resp = router
            .dispatch(make_request(
                "session/archive",
                serde_json::json!({"session_id": member_session_key}),
            ))
            .await
            .unwrap();
        assert!(
            retry_resp.error.is_none(),
            "retry after parent archive failure clears should complete cleanup: {retry_resp:?}"
        );
        assert!(
            !mob_state
                .session_service()
                .has_live_session(&member_session_id)
                .await
                .expect("check retried parent bridge session"),
            "successful retry must archive the parent bridge session"
        );
        assert!(
            mob_state.handle_for(&child_mob_id).await.is_err(),
            "successful retry must remove the child cleanup retry anchor"
        );
    }

    #[tokio::test]
    async fn session_history_returns_messages_for_live_and_archived_sessions() {
        let (router, _notif_rx) = test_router().await;

        let create_resp = router
            .dispatch(make_request(
                "session/create",
                serde_json::json!({ "prompt": "Hello" }),
            ))
            .await
            .unwrap();
        let session_id = result_value(&create_resp)["session_id"]
            .as_str()
            .unwrap()
            .to_string();

        let turn_resp = router
            .dispatch(make_request(
                "turn/start",
                serde_json::json!({
                    "session_id": &session_id,
                    "prompt": "Follow up",
                }),
            ))
            .await
            .unwrap();
        assert!(turn_resp.error.is_none(), "second turn should succeed");

        let history_resp = router
            .dispatch(make_request(
                "session/history",
                serde_json::json!({
                    "session_id": &session_id,
                    "offset": 1,
                    "limit": 2,
                }),
            ))
            .await
            .unwrap();
        let history = result_value(&history_resp);
        assert_eq!(history["session_id"], session_id);
        assert!(
            history["message_count"].as_u64().unwrap_or(0) >= 4,
            "history should expose the multi-turn transcript: {history}"
        );
        assert_eq!(history["offset"], 1);
        assert_eq!(history["limit"], 2);
        assert_eq!(history["has_more"], true);
        assert_eq!(history["messages"].as_array().unwrap().len(), 2);

        let archive_resp = router
            .dispatch(make_request(
                "session/archive",
                serde_json::json!({ "session_id": &session_id }),
            ))
            .await
            .unwrap();
        assert_eq!(result_value(&archive_resp)["archived"], true);

        let archived_history_resp = router
            .dispatch(make_request(
                "session/history",
                serde_json::json!({ "session_id": &session_id }),
            ))
            .await
            .unwrap();
        let archived_history = result_value(&archived_history_resp);
        assert_eq!(archived_history["session_id"], session_id);
        assert!(
            archived_history["message_count"].as_u64().unwrap_or(0) >= 4,
            "archived history should preserve the transcript: {archived_history}"
        );
        assert!(
            archived_history["messages"].as_array().unwrap().len() >= 4,
            "archived history should return the full transcript"
        );
    }

    #[tokio::test]
    async fn session_fork_at_creates_readable_branch_history() {
        let (router, _notif_rx) = test_router().await;

        let create_resp = router
            .dispatch(make_request(
                "session/create",
                serde_json::json!({ "prompt": "Hello" }),
            ))
            .await
            .unwrap();
        let session_id = result_value(&create_resp)["session_id"]
            .as_str()
            .unwrap()
            .to_string();

        let turn_resp = router
            .dispatch(make_request(
                "turn/start",
                serde_json::json!({
                    "session_id": &session_id,
                    "prompt": "Follow up",
                }),
            ))
            .await
            .unwrap();
        assert!(turn_resp.error.is_none(), "second turn should succeed");

        let fork_resp = router
            .dispatch(make_request(
                "session/fork_at",
                serde_json::json!({
                    "session_id": &session_id,
                    "message_index": 2,
                    "running_behavior": "reject"
                }),
            ))
            .await
            .unwrap();
        let forked = result_value(&fork_resp);
        let forked_id = forked["session_id"].as_str().unwrap().to_string();
        assert_ne!(forked_id, session_id);
        assert_eq!(forked["source_session_id"], session_id);
        assert_eq!(forked["message_count"], 2);

        let history_resp = router
            .dispatch(make_request(
                "session/history",
                serde_json::json!({ "session_id": &forked_id }),
            ))
            .await
            .unwrap();
        let history = result_value(&history_resp);
        assert_eq!(history["message_count"], 2);
        assert_eq!(history["messages"].as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn session_fork_replace_creates_changed_branch_without_mutating_parent() {
        let (router, _notif_rx) = test_router().await;

        let create_resp = router
            .dispatch(make_request(
                "session/create",
                serde_json::json!({ "prompt": "Hello" }),
            ))
            .await
            .unwrap();
        let session_id = result_value(&create_resp)["session_id"]
            .as_str()
            .unwrap()
            .to_string();

        let turn_resp = router
            .dispatch(make_request(
                "turn/start",
                serde_json::json!({
                    "session_id": &session_id,
                    "prompt": "Follow up",
                }),
            ))
            .await
            .unwrap();
        assert!(turn_resp.error.is_none(), "second turn should succeed");

        let parent_before_resp = router
            .dispatch(make_request(
                "session/history",
                serde_json::json!({ "session_id": &session_id }),
            ))
            .await
            .unwrap();
        let parent_before = result_value(&parent_before_resp);
        let parent_messages = parent_before["messages"].as_array().unwrap();
        let message_index = parent_messages
            .iter()
            .position(|message| message["role"] == "user" && message["content"] == "Follow up")
            .expect("parent transcript should contain the follow-up user message");
        let parent_message_count = parent_before["message_count"]
            .as_u64()
            .expect("parent message_count should be numeric");

        let fork_resp = router
            .dispatch(make_request(
                "session/fork_replace",
                serde_json::json!({
                    "session_id": &session_id,
                    "message_index": message_index,
                    "replacement": {
                        "type": "message",
                        "message": {
                            "role": "user",
                            "content": "Edited follow up"
                        }
                    },
                    "running_behavior": "reject"
                }),
            ))
            .await
            .unwrap();
        let forked = result_value(&fork_resp);
        let forked_id = forked["session_id"].as_str().unwrap().to_string();
        assert_ne!(forked_id, session_id);
        assert_eq!(forked["message_count"], (message_index + 1) as u64);

        let fork_history_resp = router
            .dispatch(make_request(
                "session/history",
                serde_json::json!({ "session_id": &forked_id }),
            ))
            .await
            .unwrap();
        let fork_history = result_value(&fork_history_resp);
        assert_eq!(fork_history["message_count"], (message_index + 1) as u64);
        assert_eq!(fork_history["messages"][message_index]["role"], "user");
        assert_eq!(
            fork_history["messages"][message_index]["content"],
            "Edited follow up"
        );

        let parent_history_resp = router
            .dispatch(make_request(
                "session/history",
                serde_json::json!({ "session_id": &session_id }),
            ))
            .await
            .unwrap();
        let parent_history = result_value(&parent_history_resp);
        assert_eq!(parent_history["message_count"], parent_message_count);
        assert_eq!(
            parent_history["messages"][message_index]["content"],
            "Follow up"
        );
    }

    #[tokio::test]
    async fn archived_session_drops_unapplied_staged_context_before_any_later_turn() {
        let recorded_requests = Arc::new(std::sync::Mutex::new(Vec::<Vec<Message>>::new()));
        let (router, mut notif_rx) = test_router_with_llm(Arc::new(RecordingMockLlmClient::new(
            Arc::clone(&recorded_requests),
        )))
        .await;

        let create_resp = router
            .dispatch(make_request(
                "session/create",
                serde_json::json!({ "prompt": "Hello" }),
            ))
            .await
            .unwrap();
        let session_id = result_value(&create_resp)["session_id"]
            .as_str()
            .unwrap()
            .to_string();
        drain_notifications(&mut notif_rx).await;
        recorded_requests
            .lock()
            .expect("recorded requests lock poisoned")
            .clear();

        let injected_text = "This context must be dropped on archive.";
        let inject_resp = router
            .dispatch(make_request(
                "session/inject_context",
                serde_json::json!({
                    "session_id": session_id,
                    "text": injected_text,
                    "source": "mob",
                    "idempotency_key": "ctx-archive-drop"
                }),
            ))
            .await
            .unwrap();
        assert_eq!(result_value(&inject_resp)["status"], "staged");

        let archive_resp = router
            .dispatch(make_request(
                "session/archive",
                serde_json::json!({ "session_id": session_id }),
            ))
            .await
            .unwrap();
        assert_eq!(result_value(&archive_resp)["archived"], true);

        let rejected_turn = router
            .dispatch(make_request(
                "turn/start",
                serde_json::json!({
                    "session_id": session_id,
                    "prompt": "should never run"
                }),
            ))
            .await
            .unwrap();
        assert!(
            rejected_turn.error.is_some(),
            "archived session must reject later turns after staged context was accepted"
        );
        assert!(
            recorded_requests
                .lock()
                .expect("recorded requests lock poisoned")
                .is_empty(),
            "archived session must not reach the LLM again after staged context is dropped"
        );

        let notification = tokio::time::timeout(std::time::Duration::from_millis(200), async {
            notif_rx.recv().await
        })
        .await;
        match notification {
            Err(_) => {}
            Ok(None) => {}
            Ok(Some(notif)) => {
                let event_type = notif.params["event"]["payload"]["type"].as_str();
                assert!(
                    !(notif.method == "session/event"
                        && event_type == Some("run_started")
                        && notif.params["session_id"].as_str() == Some(session_id.as_str())
                        && notif.params["event"]["payload"]["prompt"]
                            .as_str()
                            .is_some_and(|prompt| prompt.contains(injected_text))),
                    "archived session must not emit a later run_started that replays dropped staged context"
                );
            }
        }
    }

    #[tokio::test]
    async fn archived_session_read_remains_available_and_mutations_reject() {
        Box::pin(archived_session_read_remains_available_and_mutations_reject_inner()).await;
    }

    /// 7. `turn/start` returns a result for an existing session.
    #[tokio::test]
    async fn turn_start_returns_result() {
        let (router, _notif_rx) = test_router().await;

        // Create a session first
        let create_req = make_request("session/create", serde_json::json!({"prompt": "Hello"}));
        let create_resp = router.dispatch(create_req).await.unwrap();
        let created = result_value(&create_resp);
        let session_id = created["session_id"].as_str().unwrap().to_string();

        // Start another turn
        let turn_req = make_request(
            "turn/start",
            serde_json::json!({
                "session_id": session_id,
                "prompt": "Follow up"
            }),
        );
        let turn_resp = router.dispatch(turn_req).await.unwrap();
        let turn_result = result_value(&turn_resp);

        assert_eq!(turn_result["session_id"].as_str().unwrap(), session_id);
        let text = turn_result["text"].as_str().unwrap();
        assert!(
            text.contains("Hello from mock"),
            "Expected mock text in turn, got: {text}"
        );
    }

    #[tokio::test]
    async fn turn_start_returns_request_cancelled_when_pre_cancelled() {
        let (router, _notif_rx) = test_router().await;

        let create_req = make_request(
            "session/create",
            serde_json::json!({
                "prompt": "Hello",
                "initial_turn": "deferred"
            }),
        );
        let create_resp = router.dispatch(create_req).await.unwrap();
        let created = result_value(&create_resp);
        let session_id = created["session_id"].as_str().unwrap().to_string();

        let mut turn_req = make_request(
            "turn/start",
            serde_json::json!({
                "session_id": session_id,
                "prompt": "Follow up"
            }),
        );
        turn_req.id = Some(RpcId::Num(42));
        let context = cancelled_request_context(turn_req.id.as_ref().unwrap()).await;

        let turn_resp = router
            .dispatch_with_request_context(turn_req, Some(context))
            .await
            .unwrap();

        assert_eq!(error_code(&turn_resp), error::REQUEST_CANCELLED);
    }

    /// 8. `turn/start` emits notifications via the notification sink.
    #[tokio::test]
    async fn turn_start_emits_notifications() {
        let (router, mut notif_rx) = test_router().await;

        // Create a session
        let create_req = make_request("session/create", serde_json::json!({"prompt": "Hello"}));
        let _create_resp = router.dispatch(create_req).await.unwrap();

        // Give the event forwarder task a moment to send notifications
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Check that we received at least one notification
        let mut notifications = Vec::new();
        while let Ok(notif) = notif_rx.try_recv() {
            notifications.push(notif);
        }

        assert!(
            !notifications.is_empty(),
            "Should have received at least one notification"
        );

        // All notifications should be session/event
        for notif in &notifications {
            assert_eq!(notif.method, "session/event");
            assert!(notif.params["session_id"].is_string());
            assert!(notif.params["event"].is_object());
        }
    }

    /// 8b. `session/create` accepts structured skill refs at wire boundary.
    #[tokio::test]
    async fn session_create_accepts_structured_skill_refs() {
        let (router, _notif_rx) = test_router().await;
        let req = make_request(
            "session/create",
            serde_json::json!({
                "prompt": "Say hello",
                "skill_refs": [
                    {
                        "kind": "structured",
                        "source_uuid": "dc256086-0d2f-4f61-a307-320d4148107f",
                        "skill_name": "email-extractor"
                    }
                ]
            }),
        );

        let resp = router.dispatch(req).await.unwrap();
        let result = result_value(&resp);
        assert!(result["session_id"].as_str().is_some());
        assert!(result["text"].as_str().unwrap().contains("Hello from mock"));
    }

    /// 8b2. Retired legacy string refs are rejected even when a registry has aliases.
    #[tokio::test]
    async fn session_create_rejects_legacy_skill_references_with_registry() {
        let (router, _notif_rx) = test_router_with_registry(alias_registry()).await;
        let req = make_request(
            "session/create",
            serde_json::json!({
                "prompt": "Say hello",
                "skill_references": ["legacy/email"]
            }),
        );

        let resp = router.dispatch(req).await.unwrap();
        assert_eq!(error_code(&resp), error::INVALID_PARAMS);
        assert!(
            error_message(&resp).contains("skill_references is retired"),
            "unexpected error: {:?}",
            resp.error
        );
    }

    /// 8c. `turn/start` rejects retired legacy refs even with structured refs.
    #[tokio::test]
    async fn turn_start_rejects_legacy_skill_references() {
        let (router, _notif_rx) = test_router().await;

        let create_req = make_request("session/create", serde_json::json!({"prompt": "Hello"}));
        let create_resp = router.dispatch(create_req).await.unwrap();
        let created = result_value(&create_resp);
        let session_id = created["session_id"].as_str().unwrap().to_string();

        let turn_req = make_request(
            "turn/start",
            serde_json::json!({
                "session_id": session_id,
                "prompt": "Follow up",
                "skill_refs": [{
                    "kind": "structured",
                    "source_uuid": "dc256086-0d2f-4f61-a307-320d4148107f",
                    "skill_name": "email-extractor"
                }],
                "skill_references": ["dc256086-0d2f-4f61-a307-320d4148107f/email-extractor"]
            }),
        );

        let turn_resp = router.dispatch(turn_req).await.unwrap();
        assert_eq!(error_code(&turn_resp), error::INVALID_PARAMS);
        assert!(
            error_message(&turn_resp).contains("skill_references is retired"),
            "unexpected error: {:?}",
            turn_resp.error
        );
    }

    /// 8d. Invalid structured refs fail deterministically at the wire boundary.
    #[tokio::test]
    async fn session_create_rejects_invalid_structured_skill_ref() {
        let (router, _notif_rx) = test_router().await;
        let req = make_request(
            "session/create",
            serde_json::json!({
                "prompt": "Say hello",
                "skill_refs": [
                    {
                        "kind": "structured",
                        "source_uuid": "not-a-uuid",
                        "skill_name": "email-extractor"
                    }
                ]
            }),
        );

        let resp = router.dispatch(req).await.unwrap();
        assert_eq!(error_code(&resp), error::INVALID_PARAMS);
    }

    /// 8e. Retired legacy alias ingress is rejected even when a registry exists.
    #[tokio::test]
    async fn session_create_rejects_unknown_legacy_skill_references_with_registry() {
        let (router, _notif_rx) = test_router_with_registry(alias_registry()).await;
        let req = make_request(
            "session/create",
            serde_json::json!({
                "prompt": "Say hello",
                "skill_references": ["legacy/unknown"]
            }),
        );

        let resp = router.dispatch(req).await.unwrap();
        assert_eq!(error_code(&resp), error::INVALID_PARAMS);
        assert!(
            error_message(&resp).contains("skill_references is retired"),
            "unexpected error: {:?}",
            resp.error
        );
    }

    /// 9. `turn/interrupt` on an idle session returns ok.
    #[tokio::test]
    async fn turn_interrupt_returns_ok() {
        let (router, _notif_rx) = test_router().await;

        // Create a session
        let create_req = make_request("session/create", serde_json::json!({"prompt": "Hello"}));
        let create_resp = router.dispatch(create_req).await.unwrap();
        let created = result_value(&create_resp);
        let session_id = created["session_id"].as_str().unwrap().to_string();

        // Interrupt (session is idle, should be a no-op success)
        let interrupt_req = make_request(
            "turn/interrupt",
            serde_json::json!({"session_id": session_id}),
        );
        let interrupt_resp = router.dispatch(interrupt_req).await.unwrap();
        let interrupt_result = result_value(&interrupt_resp);
        assert_eq!(interrupt_result["interrupted"], true);
    }

    #[tokio::test]
    async fn turn_interrupt_unknown_session_returns_not_found() {
        let (router, _notif_rx) = test_router().await;
        let unknown_session_id = meerkat_core::SessionId::new().to_string();

        let interrupt_req = make_request(
            "turn/interrupt",
            serde_json::json!({"session_id": unknown_session_id}),
        );
        let interrupt_resp = router.dispatch(interrupt_req).await.unwrap();

        assert_eq!(error_code(&interrupt_resp), error::SESSION_NOT_FOUND);
    }

    #[tokio::test]
    async fn turn_interrupt_service_owned_idle_session_returns_ok() {
        let (router, _notif_rx) = test_router().await;
        let llm_override: Arc<dyn LlmClient> = Arc::new(MockLlmClient);
        let created = router
            .runtime
            .core_session_service()
            .create_session(meerkat_core::service::CreateSessionRequest {
                model: "claude-sonnet-4-5".to_string(),
                prompt: "Hello".to_string().into(),
                render_metadata: None,
                system_prompt: None,
                max_tokens: None,
                event_tx: None,
                skill_references: None,
                initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
                deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
                build: Some(meerkat_core::service::SessionBuildOptions {
                    llm_client_override: Some(meerkat::encode_llm_client_override_for_service(
                        llm_override,
                    )),
                    ..Default::default()
                }),
                labels: None,
            })
            .await
            .expect("service-owned idle session should be created");

        let interrupt_resp = router
            .dispatch(make_request(
                "turn/interrupt",
                serde_json::json!({"session_id": created.session_id}),
            ))
            .await
            .unwrap();

        assert!(
            interrupt_resp.error.is_none(),
            "service-owned idle interrupt should no-op: {interrupt_resp:?}"
        );
        assert_eq!(result_value(&interrupt_resp)["interrupted"], true);
    }

    #[tokio::test]
    async fn session_create_returns_request_cancelled_and_rolls_back_when_pre_cancelled() {
        let (router, _notif_rx) = test_router_with_v9_runtime().await;

        let mut create_req = make_request(
            "session/create",
            serde_json::json!({
                "prompt": "Hello"
            }),
        );
        create_req.id = Some(RpcId::Num(77));
        let context = cancelled_request_context(create_req.id.as_ref().unwrap()).await;

        let create_resp = router
            .dispatch_with_request_context(create_req, Some(context))
            .await
            .unwrap();

        assert_eq!(error_code(&create_resp), error::REQUEST_CANCELLED);

        let list_resp = router
            .dispatch(make_request("session/list", serde_json::json!({})))
            .await
            .unwrap();
        let listed = result_value(&list_resp);
        let sessions = listed["sessions"]
            .as_array()
            .expect("session/list should return an array");
        assert!(
            sessions.is_empty(),
            "pre-start cancelled create should not leave a live session behind: {sessions:?}"
        );
    }

    /// 10. `config/get` returns the default config.
    #[tokio::test]
    async fn config_get_returns_config() {
        let (router, _notif_rx) = test_router().await;
        let req = make_request_no_params("config/get");

        let resp = router.dispatch(req).await.unwrap();
        let result = result_value(&resp);

        // Config response should be an envelope with config + metadata
        assert!(result.is_object(), "Config should be a JSON object");
        assert!(
            result.get("config").is_some(),
            "Config envelope should include 'config'"
        );
        assert!(
            result
                .get("config")
                .and_then(|cfg| cfg.get("agent"))
                .is_some(),
            "Config envelope should have 'config.agent' field"
        );
    }

    /// 11. `config/set` then `config/get` roundtrip.
    #[tokio::test]
    async fn config_set_and_get_roundtrip() {
        let (router, _notif_rx) = test_router().await;

        // Get the current config
        let get_req = make_request_no_params("config/get");
        let get_resp = router.dispatch(get_req).await.unwrap();
        let mut config = result_value(&get_resp)["config"].clone();

        // Modify max_tokens
        config["max_tokens"] = serde_json::json!(2048);

        // Set the modified config
        let set_req = make_request("config/set", serde_json::json!({ "config": config }));
        let set_resp = router.dispatch(set_req).await.unwrap();
        let set_result = result_value(&set_resp);
        assert_eq!(set_result["config"]["max_tokens"], 2048);
        assert!(set_result["generation"].as_u64().is_some());

        // Get again and verify
        let get_req2 = make_request_no_params("config/get");
        let get_resp2 = router.dispatch(get_req2).await.unwrap();
        let config2 = result_value(&get_resp2);
        assert_eq!(config2["config"]["max_tokens"], 2048);
    }

    /// 11b. `config/set` rejects invalid config with INVALID_PARAMS for REST parity.
    #[tokio::test]
    async fn config_set_rejects_invalid_config() {
        let (router, _notif_rx) = test_router().await;

        let get_req = make_request_no_params("config/get");
        let get_resp = router.dispatch(get_req).await.unwrap();
        let mut config = result_value(&get_resp)["config"].clone();
        config["max_tokens"] = serde_json::json!(0);

        let set_req = make_request("config/set", serde_json::json!({ "config": config }));
        let set_resp = router.dispatch(set_req).await.unwrap();
        assert_eq!(error_code(&set_resp), error::INVALID_PARAMS);
    }

    /// 12. `config/patch` merges a delta.
    #[tokio::test]
    async fn config_patch_merges_delta() {
        let (router, _notif_rx) = test_router().await;

        // Get initial max_tokens
        let get_req = make_request_no_params("config/get");
        let get_resp = router.dispatch(get_req).await.unwrap();
        let initial = result_value(&get_resp);
        let initial_max_tokens = initial["config"]["max_tokens"].as_u64().unwrap();

        // Patch max_tokens to a different value
        let new_max_tokens = initial_max_tokens + 1000;
        let patch_req = make_request(
            "config/patch",
            serde_json::json!({"max_tokens": new_max_tokens}),
        );
        let patch_resp = router.dispatch(patch_req).await.unwrap();
        let patched = result_value(&patch_resp);
        assert_eq!(patched["config"]["max_tokens"], new_max_tokens);

        // Verify via get
        let get_req2 = make_request_no_params("config/get");
        let get_resp2 = router.dispatch(get_req2).await.unwrap();
        let final_config = result_value(&get_resp2);
        assert_eq!(final_config["config"]["max_tokens"], new_max_tokens);
    }

    /// 12b. `config/patch` does not re-enable retired legacy skill alias ingress.
    #[tokio::test]
    async fn config_patch_keeps_legacy_skill_references_rejected() {
        let (router, _notif_rx) = test_router().await;

        let rejected_before = make_request(
            "session/create",
            serde_json::json!({
                "prompt": "hello",
                "skill_references": ["legacy/email"]
            }),
        );
        let rejected_before_resp = router.dispatch(rejected_before).await.unwrap();
        assert_eq!(error_code(&rejected_before_resp), error::INVALID_PARAMS);

        let set_req = make_request(
            "config/patch",
            serde_json::json!({
                "patch": {
                    "skills": {
                        "repositories": [
                            {
                                "name": "legacy-source",
                                "source_uuid": "dc256086-0d2f-4f61-a307-320d4148107f",
                                "type": "filesystem",
                                "path": ".rkat/skills/legacy"
                            },
                            {
                                "name": "new-source",
                                "source_uuid": "a93d587d-8f44-438f-8189-6e8cf549f6e7",
                                "type": "filesystem",
                                "path": ".rkat/skills/new"
                            }
                        ],
                        "identity": {
                            "aliases": [{
                                "alias": "legacy/email",
                                "to": {
                                    "source_uuid": "dc256086-0d2f-4f61-a307-320d4148107f",
                                    "skill_name": "email-extractor"
                                }
                            }]
                        }
                    }
                }
            }),
        );
        let set_resp = router.dispatch(set_req).await.unwrap();
        assert!(
            set_resp.error.is_none(),
            "config/patch failed unexpectedly: {:?}",
            set_resp.error
        );

        let rejected_after = make_request(
            "session/create",
            serde_json::json!({
                "prompt": "hello",
                "skill_references": ["legacy/email"]
            }),
        );
        let rejected_after_resp = router.dispatch(rejected_after).await.unwrap();
        assert_eq!(error_code(&rejected_after_resp), error::INVALID_PARAMS);
        assert!(
            error_message(&rejected_after_resp).contains("skill_references is retired"),
            "unexpected error: {:?}",
            rejected_after_resp.error
        );
    }

    /// 12c. Invalid identity configs are rejected on patch and do not advance generation.
    #[tokio::test]
    async fn config_patch_rejects_invalid_identity_registry_update() {
        let (router, _notif_rx) = test_router().await;

        let before = make_request_no_params("config/get");
        let before_resp = router.dispatch(before).await.unwrap();
        let before_value = result_value(&before_resp);
        let generation_before = before_value["generation"].as_u64().unwrap_or(0);

        let patch_req = make_request(
            "config/patch",
            serde_json::json!({
                "patch": {
                    "skills": {
                        "identity": {
                            "lineage": [{
                                "event_id": "split-1",
                                "recorded_at_unix_secs": 1,
                                "event": {
                                    "type": "split",
                                    "from": "dc256086-0d2f-4f61-a307-320d4148107f",
                                    "into": [
                                        "a93d587d-8f44-438f-8189-6e8cf549f6e7",
                                        "e8df561d-d38f-4242-af55-3a6efb34c950"
                                    ]
                                }
                            }],
                            "remaps": []
                        }
                    }
                }
            }),
        );
        let patch_resp = router.dispatch(patch_req).await.unwrap();
        assert_eq!(error_code(&patch_resp), error::INVALID_PARAMS);

        let after = make_request_no_params("config/get");
        let after_resp = router.dispatch(after).await.unwrap();
        let after_value = result_value(&after_resp);
        let generation_after = after_value["generation"].as_u64().unwrap_or(0);
        assert_eq!(generation_after, generation_before);
    }

    /// 12d. Invalid config patches fail as INVALID_PARAMS for REST parity.
    #[tokio::test]
    async fn config_patch_rejects_invalid_config() {
        let (router, _notif_rx) = test_router().await;
        let patch_req = make_request("config/patch", serde_json::json!({"max_tokens": 0}));

        let patch_resp = router.dispatch(patch_req).await.unwrap();
        assert_eq!(error_code(&patch_resp), error::INVALID_PARAMS);
    }

    /// 13. A notification (request with no id) returns None (no response).
    #[tokio::test]
    async fn notification_is_silently_dropped() {
        let (router, _notif_rx) = test_router().await;
        let req = make_notification("initialized");

        let resp = router.dispatch(req).await;
        assert!(resp.is_none(), "Notifications should return None");
    }
}
