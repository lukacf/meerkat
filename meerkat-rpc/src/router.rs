//! Method router - dispatches JSON-RPC requests to the correct handler.

use std::collections::{HashMap, HashSet};
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
use meerkat_core::service::SessionHistoryQuery;
use meerkat_core::session::Session;
use meerkat_core::types::SessionId;
use meerkat_runtime::SessionServiceRuntimeExt as _;
use serde_json::json;

use crate::error;
use crate::handlers;
use crate::handlers::RpcResponseExt;
use crate::protocol::{RpcNotification, RpcRequest, RpcResponse};
use crate::session_runtime::SessionRuntime;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SessionOwner {
    Runtime,
    #[cfg(feature = "mob")]
    Mob,
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
    /// allows SDKs to distinguish sub-agent and mob-member scoped events from
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
    runtime_adapter: Arc<meerkat_runtime::RuntimeSessionAdapter>,
}

impl MethodRouter {
    fn session_metadata_marks_archived(session: &Session) -> bool {
        session
            .metadata()
            .get("session_archived")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
    }

    /// Create a new method router.
    pub fn new(
        runtime: Arc<SessionRuntime>,
        config_store: Arc<dyn ConfigStore>,
        notification_sink: NotificationSink,
    ) -> Self {
        let runtime_adapter = runtime.runtime_adapter();
        #[cfg(feature = "mob")]
        let mob_state = Arc::new(
            meerkat_mob_mcp::MobMcpState::new_with_runtime_adapter(
                runtime.session_service(),
                Some(runtime_adapter.clone()),
            )
            .with_default_llm_client_provider(Some(Arc::new({
                let runtime = runtime.clone();
                move || runtime.default_llm_client()
            }))),
        );
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
        }
    }

    /// Get a reference to the runtime adapter for session registration.
    pub fn runtime_adapter(&self) -> &Arc<meerkat_runtime::RuntimeSessionAdapter> {
        &self.runtime_adapter
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
        let persisted = self
            .runtime
            .load_persisted_session(session_id)
            .await
            .ok()
            .flatten();
        if persisted
            .as_ref()
            .is_some_and(Self::session_metadata_marks_archived)
        {
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
                    self.notification_sink.clone(),
                ))
            }
            #[cfg(feature = "mob")]
            Some(SessionOwner::Mob) => {
                Box::new(crate::session_executor::MobRpcRuntimeExecutor::new(
                    self.mob_state.session_service(),
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

    #[cfg(feature = "mob")]
    pub fn new_with_mob_state(
        runtime: Arc<SessionRuntime>,
        config_store: Arc<dyn ConfigStore>,
        notification_sink: NotificationSink,
        mob_state: Arc<meerkat_mob_mcp::MobMcpState>,
    ) -> Self {
        let runtime_adapter = runtime.runtime_adapter();
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
        }
    }

    /// Replace the default ephemeral runtime adapter with a custom one
    /// (e.g., persistent-backed for durable runtime semantics).
    pub fn with_runtime_adapter(
        mut self,
        adapter: Arc<meerkat_runtime::RuntimeSessionAdapter>,
    ) -> Self {
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
        if self.mob_state.owns_live_session(session_id).await
            || self.mob_state.owns_persisted_session(session_id).await
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
                    .map(|realm| meerkat_contracts::format_session_ref(realm, session_id));
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
            "session/create" => {
                handlers::session::handle_create(
                    id,
                    params,
                    self.runtime.clone(),
                    &self.notification_sink,
                    &self.runtime_adapter,
                )
                .await
            }
            "session/list" => handlers::session::handle_list(id, params, &self.runtime).await,
            "session/read" => self.handle_session_read(id, params).await,
            "session/history" => self.handle_session_history(id, params).await,
            "session/archive" => self.handle_session_archive(id, params).await,
            "session/inject_context" => self.handle_session_inject_context(id, params).await,
            "session/stream_open" => self.handle_session_stream_open(id, params).await,
            "session/stream_close" => self.handle_session_stream_close(id, params).await,
            "turn/start" => {
                handlers::turn::handle_start(
                    id,
                    params,
                    self.runtime.clone(),
                    &self.notification_sink,
                    &self.runtime_adapter,
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
            "mob/prefabs" => handlers::mob::handle_prefabs(id).await,
            #[cfg(feature = "mob")]
            "mob/tools" => handlers::mob::handle_tools(id).await,
            #[cfg(feature = "mob")]
            "mob/call" => handlers::mob::handle_call(id, params, &self.mob_state).await,
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
            "mob/send" => handlers::mob::handle_send(id, params, &self.mob_state).await,
            #[cfg(feature = "mob")]
            "mob/events" => handlers::mob::handle_events(id, params, &self.mob_state).await,
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
            "mob/stream_open" => self.handle_mob_stream_open(id, params).await,
            #[cfg(feature = "mob")]
            "mob/stream_close" => self.handle_mob_stream_close(id, params).await,
            #[cfg(feature = "comms")]
            "comms/send" => self.handle_comms_send(id, params).await,
            #[cfg(feature = "comms")]
            "comms/peers" => self.handle_comms_peers(id, params).await,
            // M12: event/push removed. Use comms/send instead.
            "skills/list" => handlers::skills::handle_list(id, &self.skill_runtime).await,
            "skills/inspect" => {
                handlers::skills::handle_inspect(id, params, &self.skill_runtime).await
            }
            "capabilities/get" => {
                let config = self.config_store.get().await.unwrap_or_default();
                handlers::capabilities::handle_get(id, &config)
            }
            "models/catalog" => handlers::models::handle_catalog(id),
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
            "mcp/add" => handlers::mcp::handle_add(id, params, &self.runtime).await,
            "mcp/remove" => handlers::mcp::handle_remove(id, params, &self.runtime).await,
            "mcp/reload" => handlers::mcp::handle_reload(id, params, &self.runtime).await,
            "runtime/state" => {
                if self.runtime_adapter.runtime_mode() != meerkat_runtime::RuntimeMode::V9Compliant
                {
                    RpcResponse::error(
                        id,
                        error::METHOD_NOT_FOUND,
                        "Method not found: runtime/state",
                    )
                } else {
                    let session_id = match self.session_id_from_runtime_params(id.clone(), params) {
                        Ok(session_id) => session_id,
                        Err(response) => return Some(response),
                    };
                    if let Err(response) = self.ensure_runtime_session_registered(&session_id).await
                    {
                        return Some(response.with_id(id));
                    }
                    handlers::runtime::handle_runtime_state(
                        id,
                        params,
                        self.runtime_adapter.as_ref(),
                    )
                    .await
                }
            }
            "runtime/accept" => {
                if self.runtime_adapter.runtime_mode() != meerkat_runtime::RuntimeMode::V9Compliant
                {
                    RpcResponse::error(
                        id,
                        error::METHOD_NOT_FOUND,
                        "Method not found: runtime/accept",
                    )
                } else {
                    let session_id = match self.session_id_from_runtime_params(id.clone(), params) {
                        Ok(session_id) => session_id,
                        Err(response) => return Some(response),
                    };
                    if let Err(response) = self.ensure_runtime_session_registered(&session_id).await
                    {
                        return Some(response.with_id(id));
                    }
                    handlers::runtime::handle_runtime_accept(
                        id,
                        params,
                        self.runtime_adapter.as_ref(),
                    )
                    .await
                }
            }
            "runtime/retire" => {
                if self.runtime_adapter.runtime_mode() != meerkat_runtime::RuntimeMode::V9Compliant
                {
                    RpcResponse::error(
                        id,
                        error::METHOD_NOT_FOUND,
                        "Method not found: runtime/retire",
                    )
                } else {
                    let session_id = match self.session_id_from_runtime_params(id.clone(), params) {
                        Ok(session_id) => session_id,
                        Err(response) => return Some(response),
                    };
                    if let Err(response) = self.ensure_runtime_session_registered(&session_id).await
                    {
                        return Some(response.with_id(id));
                    }
                    handlers::runtime::handle_runtime_retire(
                        id,
                        params,
                        self.runtime_adapter.as_ref(),
                    )
                    .await
                }
            }
            "runtime/reset" => {
                if self.runtime_adapter.runtime_mode() != meerkat_runtime::RuntimeMode::V9Compliant
                {
                    RpcResponse::error(
                        id,
                        error::METHOD_NOT_FOUND,
                        "Method not found: runtime/reset",
                    )
                } else {
                    let session_id = match self.session_id_from_runtime_params(id.clone(), params) {
                        Ok(session_id) => session_id,
                        Err(response) => return Some(response),
                    };
                    if let Err(response) = self.ensure_runtime_session_registered(&session_id).await
                    {
                        return Some(response.with_id(id));
                    }
                    handlers::runtime::handle_runtime_reset(
                        id,
                        params,
                        self.runtime_adapter.as_ref(),
                    )
                    .await
                }
            }
            "input/state" => {
                if self.runtime_adapter.runtime_mode() != meerkat_runtime::RuntimeMode::V9Compliant
                {
                    RpcResponse::error(id, error::METHOD_NOT_FOUND, "Method not found: input/state")
                } else {
                    let session_id = match self.session_id_from_runtime_params(id.clone(), params) {
                        Ok(session_id) => session_id,
                        Err(response) => return Some(response),
                    };
                    if let Err(response) = self.ensure_runtime_session_registered(&session_id).await
                    {
                        return Some(response.with_id(id));
                    }
                    handlers::runtime::handle_input_state(id, params, self.runtime_adapter.as_ref())
                        .await
                }
            }
            "input/list" => {
                if self.runtime_adapter.runtime_mode() != meerkat_runtime::RuntimeMode::V9Compliant
                {
                    RpcResponse::error(id, error::METHOD_NOT_FOUND, "Method not found: input/list")
                } else {
                    let session_id = match self.session_id_from_runtime_params(id.clone(), params) {
                        Ok(session_id) => session_id,
                        Err(response) => return Some(response),
                    };
                    if let Err(response) = self.ensure_runtime_session_registered(&session_id).await
                    {
                        return Some(response.with_id(id));
                    }
                    handlers::runtime::handle_input_list(id, params, self.runtime_adapter.as_ref())
                        .await
                }
            }
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
                        .map(|realm| meerkat_contracts::format_session_ref(realm, &session_id));
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
                        info.session_ref = self
                            .runtime
                            .realm_id()
                            .map(|realm| meerkat_contracts::format_session_ref(realm, &session_id));
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
                        .map(|realm| meerkat_contracts::format_session_ref(realm, &session_id));
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
                    self.runtime_adapter.unregister_session(&session_id).await;
                    RpcResponse::success(id, json!({"archived": true}))
                }
                Err(rpc_err) => RpcResponse::error(id, rpc_err.code, rpc_err.message),
            },
            #[cfg(feature = "mob")]
            Some(SessionOwner::Mob) => match self
                .mob_state
                .retire_member_by_session_id(&session_id)
                .await
            {
                Ok(()) => RpcResponse::success(id, json!({"archived": true})),
                Err(err) => RpcResponse::error(id, error::SESSION_NOT_FOUND, err.to_string()),
            },
            None => RpcResponse::error(
                id,
                error::SESSION_NOT_FOUND,
                format!("Session not found: {session_id}"),
            ),
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
        let cmd = match handlers::comms::build_comms_command(&params, &session_id) {
            Ok(cmd) => cmd,
            Err(details) => {
                return RpcResponse::error(id, error::INVALID_PARAMS, serde_json::json!({
                    "code": "invalid_command",
                    "message": "Command validation failed",
                    "details": meerkat_core::comms::CommsCommandRequest::validation_errors_to_json(&details),
                }).to_string())
            }
        };
        match comms.send(cmd).await {
            Ok(receipt) => {
                let result = match receipt {
                    meerkat_core::comms::SendReceipt::InputAccepted {
                        interaction_id,
                        stream_reserved,
                    } => {
                        json!({"kind":"input_accepted","interaction_id": interaction_id.0.to_string(),"stream_reserved": stream_reserved})
                    }
                    meerkat_core::comms::SendReceipt::PeerMessageSent { envelope_id, acked } => {
                        json!({"kind":"peer_message_sent","envelope_id": envelope_id.to_string(),"acked": acked})
                    }
                    meerkat_core::comms::SendReceipt::PeerRequestSent {
                        envelope_id,
                        interaction_id,
                        stream_reserved,
                    } => {
                        json!({"kind":"peer_request_sent","envelope_id": envelope_id.to_string(),"interaction_id": interaction_id.0.to_string(),"stream_reserved": stream_reserved})
                    }
                    meerkat_core::comms::SendReceipt::PeerResponseSent {
                        envelope_id,
                        in_reply_to,
                    } => {
                        json!({"kind":"peer_response_sent","envelope_id": envelope_id.to_string(),"in_reply_to": in_reply_to.0.to_string()})
                    }
                };
                RpcResponse::success(id, result)
            }
            Err(e) => RpcResponse::error(id, error::INTERNAL_ERROR, e.to_string()),
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
        let entries: Vec<serde_json::Value> = peers
            .iter()
            .map(|p| {
                json!({
                    "name": p.name.to_string(),
                    "peer_id": p.peer_id,
                    "address": p.address,
                    "source": format!("{:?}", p.source),
                    "sendable_kinds": p.sendable_kinds,
                    "capabilities": p.capabilities,
                })
            })
            .collect();
        RpcResponse::success(id, json!({"peers": entries}))
    }

    async fn handle_session_stream_open(
        &self,
        id: Option<crate::protocol::RpcId>,
        params: Option<&serde_json::value::RawValue>,
    ) -> RpcResponse {
        use serde::Deserialize;
        use serde_json::json;

        #[derive(Deserialize)]
        struct SessionStreamOpenParams {
            session_id: String,
        }

        let params = match handlers::parse_params::<SessionStreamOpenParams>(params) {
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

        RpcResponse::success(
            id,
            json!({
                "stream_id": stream_id.to_string(),
                "session_id": session_id.to_string(),
                "opened": true,
            }),
        )
    }

    async fn handle_session_stream_close(
        &self,
        id: Option<crate::protocol::RpcId>,
        params: Option<&serde_json::value::RawValue>,
    ) -> RpcResponse {
        use serde::Deserialize;
        use serde_json::json;

        #[derive(Deserialize)]
        struct SessionStreamCloseParams {
            stream_id: String,
        }

        let params = match handlers::parse_params::<SessionStreamCloseParams>(params) {
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

        RpcResponse::success(
            id,
            json!({
                "stream_id": stream_id.to_string(),
                "closed": true,
                "already_closed": already_closed,
            }),
        )
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
            member_id: Option<String>,
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

        if let Some(member_id_str) = params.member_id {
            // Per-member stream: subscribe to a specific member's agent events.
            let meerkat_id = meerkat_mob::MeerkatId::from(member_id_str.as_str());
            let stream: meerkat_core::comms::EventStream =
                match handle.subscribe_agent_events(&meerkat_id).await {
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
            let mut router_handle = handle.subscribe_mob_events();

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

    use async_trait::async_trait;
    use futures::stream;
    use meerkat::AgentFactory;
    use meerkat_client::{LlmClient, LlmError};
    use meerkat_core::skills::{
        SkillKey, SkillKeyRemap, SkillName, SourceIdentityLineage, SourceIdentityLineageEvent,
        SourceIdentityRecord, SourceIdentityRegistry, SourceIdentityStatus, SourceTransportKind,
        SourceUuid,
    };
    use meerkat_core::{Config, ConfigRuntime, MemoryConfigStore, Message, StopReason};
    use serde::Serialize;

    use crate::protocol::RpcId;

    // -----------------------------------------------------------------------
    // Mock LLM client (same as session_runtime tests)
    // -----------------------------------------------------------------------

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

    async fn test_router() -> (MethodRouter, mpsc::Receiver<RpcNotification>) {
        let temp = tempfile::tempdir().unwrap();
        let factory = AgentFactory::new(temp.path().join("sessions"));
        let config = Config::default();
        let store: Arc<dyn meerkat::SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let mut runtime = SessionRuntime::new(
            factory,
            config,
            10,
            meerkat::PersistenceBundle::new(store, None),
            NotificationSink::noop(),
        );
        let config_store: Arc<dyn ConfigStore> =
            Arc::new(MemoryConfigStore::new(Config::default()));
        runtime.default_llm_client = Some(Arc::new(MockLlmClient));
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
        let runtime_adapter = Arc::new(meerkat_runtime::RuntimeSessionAdapter::persistent(
            Arc::new(meerkat_runtime::InMemoryRuntimeStore::new()),
        ));
        (router.with_runtime_adapter(runtime_adapter), notif_rx)
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
            meerkat::PersistenceBundle::new(store, None),
            NotificationSink::noop(),
        );
        let config_store: Arc<dyn ConfigStore> =
            Arc::new(MemoryConfigStore::new(Config::default()));
        runtime.default_llm_client = Some(llm_client);
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
            meerkat::PersistenceBundle::new(store, None),
            NotificationSink::noop(),
        );
        let config_store: Arc<dyn ConfigStore> =
            Arc::new(MemoryConfigStore::new(Config::default()));
        runtime.default_llm_client = Some(llm_client);
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
            meerkat::PersistenceBundle::new(store, None),
            NotificationSink::noop(),
        );
        let config_store: Arc<dyn ConfigStore> =
            Arc::new(MemoryConfigStore::new(Config::default()));
        runtime.default_llm_client = Some(Arc::new(MockLlmClient));
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
            meerkat::PersistenceBundle::new(store, None),
            NotificationSink::noop(),
        );
        let config_store: Arc<dyn ConfigStore> =
            Arc::new(MemoryConfigStore::new(Config::default()));
        runtime.default_llm_client = Some(Arc::new(MockLlmClient));
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
        assert!(method_names.contains(&"session/create"));
        assert!(method_names.contains(&"session/history"));
        assert!(method_names.contains(&"session/inject_context"));
        assert!(method_names.contains(&"turn/start"));
        assert!(
            !method_names.contains(&"session/destroy"),
            "generic session/destroy must not appear until it has member-aware mob semantics"
        );
        #[cfg(feature = "mob")]
        {
            assert!(method_names.contains(&"mob/prefabs"));
            assert!(method_names.contains(&"mob/tools"));
            assert!(method_names.contains(&"mob/call"));
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

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn mob_prefabs_returns_prefab_templates() {
        let (router, _notif_rx) = test_router().await;
        let req = make_request_no_params("mob/prefabs");

        let resp = router.dispatch(req).await.unwrap();
        let result = result_value(&resp);
        let prefabs = result["prefabs"]
            .as_array()
            .expect("prefabs should be an array");
        assert!(
            prefabs.len() >= 4,
            "expected built-in prefabs, got {}",
            prefabs.len()
        );

        let keys: Vec<&str> = prefabs
            .iter()
            .filter_map(|entry| entry["key"].as_str())
            .collect();
        assert!(keys.contains(&"coding_swarm"));
        assert!(keys.contains(&"code_review"));
        assert!(keys.contains(&"research_team"));
        assert!(keys.contains(&"pipeline"));

        for entry in prefabs {
            assert!(entry["key"].is_string());
            let template = entry["toml_template"]
                .as_str()
                .expect("toml_template should be a string");
            assert!(
                template.contains("id ="),
                "template should look like TOML definition"
            );
        }
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn mob_tools_and_call_flow_work() {
        let (router, _notif_rx) = test_router().await;

        let tools_resp = router
            .dispatch(make_request_no_params("mob/tools"))
            .await
            .unwrap();
        let tools_binding = result_value(&tools_resp);
        let tools = tools_binding["tools"]
            .as_array()
            .expect("tools should be array");
        assert!(tools.iter().any(|tool| tool["name"] == "mob_create"));

        let create_resp = router
            .dispatch(make_request(
                "mob/call",
                serde_json::json!({
                    "name": "mob_create",
                    "arguments": { "prefab": "coding_swarm" }
                }),
            ))
            .await
            .unwrap();
        let created = result_value(&create_resp);
        assert!(created["mob_id"].as_str().is_some());
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
                    "meerkat_id": "worker-1",
                    "runtime_mode": "turn_driven"
                }),
            ))
            .await
            .unwrap();
        let spawned = result_value(&spawn_resp);
        assert_eq!(spawned["meerkat_id"], "worker-1");

        let append_resp = router
            .dispatch(make_request(
                "mob/append_system_context",
                serde_json::json!({
                    "mob_id": mob_id,
                    "meerkat_id": "worker-1",
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
                    "meerkat_id": "worker-1",
                    "runtime_mode": "turn_driven"
                }),
            ))
            .await
            .unwrap();
        let spawned = result_value(&spawn_resp);
        let session_id = spawned["session_id"].as_str().unwrap().to_string();
        assert_eq!(spawned["meerkat_id"], "worker-1");

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

        let spawn_resp = router
            .dispatch(make_request(
                "mob/spawn",
                serde_json::json!({
                    "mob_id": mob_id,
                    "profile": "worker",
                    "meerkat_id": "worker-1",
                    "runtime_mode": "turn_driven"
                }),
            ))
            .await
            .unwrap();
        let session_id = result_value(&spawn_resp)["session_id"]
            .as_str()
            .unwrap()
            .to_string();

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

        let spawn_resp = router
            .dispatch(make_request(
                "mob/spawn",
                serde_json::json!({
                    "mob_id": mob_id,
                    "profile": "worker",
                    "meerkat_id": "worker-1",
                    "runtime_mode": "turn_driven"
                }),
            ))
            .await
            .unwrap();
        let session_id = result_value(&spawn_resp)["session_id"]
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

        let spawn_resp = router
            .dispatch(make_request(
                "mob/spawn",
                serde_json::json!({
                    "mob_id": mob_id,
                    "profile": "worker",
                    "meerkat_id": "worker-1",
                    "runtime_mode": "turn_driven"
                }),
            ))
            .await
            .unwrap();
        let session_id = result_value(&spawn_resp)["session_id"]
            .as_str()
            .unwrap()
            .to_string();

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

        let spawn_resp = router
            .dispatch(make_request(
                "mob/spawn",
                serde_json::json!({
                    "mob_id": mob_id,
                    "profile": "worker",
                    "meerkat_id": "worker-1",
                    "runtime_mode": "turn_driven"
                }),
            ))
            .await
            .unwrap();
        let session_id = result_value(&spawn_resp)["session_id"]
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
        assert!(
            interrupt_resp.error.is_some(),
            "retired mob-backed session must reject generic turn/interrupt"
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

        let spawn_resp = router
            .dispatch(make_request(
                "mob/spawn",
                serde_json::json!({
                    "mob_id": mob_id,
                    "profile": "worker",
                    "meerkat_id": "worker-1",
                    "runtime_mode": "turn_driven"
                }),
            ))
            .await
            .unwrap();
        let session_id = result_value(&spawn_resp)["session_id"]
            .as_str()
            .unwrap()
            .to_string();

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

        let spawn_resp = router
            .dispatch(make_request(
                "mob/spawn",
                serde_json::json!({
                    "mob_id": mob_id,
                    "profile": "worker",
                    "meerkat_id": "worker-1",
                    "runtime_mode": "turn_driven"
                }),
            ))
            .await
            .unwrap();
        let session_id = result_value(&spawn_resp)["session_id"]
            .as_str()
            .unwrap()
            .to_string();

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

        let spawn_resp = router
            .dispatch(make_request(
                "mob/spawn",
                serde_json::json!({
                    "mob_id": mob_id,
                    "profile": "worker",
                    "meerkat_id": "worker-1",
                    "runtime_mode": "turn_driven"
                }),
            ))
            .await
            .unwrap();
        let session_id = result_value(&spawn_resp)["session_id"]
            .as_str()
            .unwrap()
            .to_string();

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
        let envelope = EventEnvelope::new(
            session_id.to_string(),
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
        let envelope = EventEnvelope::new(
            session_id.to_string(),
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

    #[tokio::test]
    async fn archived_session_read_remains_available_and_mutations_reject() {
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
            "archived session should remain readable: {read_resp:?}"
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

        let spawn_resp = router
            .dispatch(make_request(
                "mob/spawn",
                serde_json::json!({
                    "mob_id": mob_id,
                    "profile": "worker",
                    "meerkat_id": "worker-1",
                    "runtime_mode": "turn_driven"
                }),
            ))
            .await
            .unwrap();
        let session_id = result_value(&spawn_resp)["session_id"]
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
    async fn mob_send_rejects_while_archive_retirement_is_in_flight() {
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

        let spawn_resp = router
            .dispatch(make_request(
                "mob/spawn",
                serde_json::json!({
                    "mob_id": &mob_id,
                    "profile": "lead",
                    "meerkat_id": "lead-1",
                    "runtime_mode": "turn_driven"
                }),
            ))
            .await
            .unwrap();
        let session_id = result_value(&spawn_resp)["session_id"]
            .as_str()
            .unwrap()
            .to_string();

        let archive = {
            let router = router.clone();
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
        assert_eq!(members[0]["meerkat_id"], "lead-1");
        assert_eq!(members[0]["state"], "Retiring");

        let send_resp = router
            .dispatch(make_request(
                "mob/send",
                serde_json::json!({
                    "mob_id": &mob_id,
                    "meerkat_id": "lead-1",
                    "message": "do work while retiring"
                }),
            ))
            .await
            .unwrap();
        assert!(
            send_resp.error.is_some(),
            "retiring member must reject new mob/send work before archive completes"
        );

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

        let spawn_resp = router
            .dispatch(make_request(
                "mob/spawn",
                serde_json::json!({
                    "mob_id": mob_id,
                    "profile": "worker",
                    "meerkat_id": "worker-1",
                    "runtime_mode": "turn_driven"
                }),
            ))
            .await
            .unwrap();
        let session_id = result_value(&spawn_resp)["session_id"]
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
                "mob/call",
                serde_json::json!({
                    "name": "mob_create",
                    "arguments": { "prefab": "coding_swarm" }
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
                "mob/call",
                serde_json::json!({
                    "name": "mob_create",
                    "arguments": { "prefab": "coding_swarm" }
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
            error_message(&resp).contains("mob-managed sessions"),
            "reserved mob label rejection should explain the trust boundary"
        );
    }

    #[tokio::test]
    async fn deferred_session_runtime_endpoints_register_pending_session() {
        let (router, _notif_rx) = test_router_with_v9_runtime().await;
        let create_resp = router
            .dispatch(make_request(
                "session/create",
                serde_json::json!({
                    "prompt": "deferred",
                    "initial_turn": "deferred"
                }),
            ))
            .await
            .unwrap();
        let created = result_value(&create_resp);
        let session_id = created["session_id"]
            .as_str()
            .expect("session_id")
            .to_string();

        let state_resp = router
            .dispatch(make_request(
                "runtime/state",
                serde_json::json!({ "session_id": session_id }),
            ))
            .await
            .unwrap();
        let state = result_value(&state_resp);
        assert_eq!(
            state["state"].as_str(),
            Some("idle"),
            "deferred sessions should be routable through runtime/state before their first turn"
        );

        let accept_resp = router
            .dispatch(make_request(
                "runtime/accept",
                serde_json::json!({
                    "session_id": session_id,
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
                        "text": "drive via runtime"
                    }
                }),
            ))
            .await
            .unwrap();
        let accepted = result_value(&accept_resp);
        assert_eq!(
            accepted["outcome_type"].as_str(),
            Some("accepted"),
            "deferred sessions should also be routable through runtime/accept before their first turn"
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
                    "archived session must not emit a later run_started that replays dropped staged context: {notif:?}"
                );
            }
        }
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

    /// 8b2. Alias refs resolve through configured non-default identity registry.
    #[tokio::test]
    async fn session_create_accepts_alias_ref_with_registry() {
        let (router, _notif_rx) = test_router_with_registry(alias_registry()).await;
        let req = make_request(
            "session/create",
            serde_json::json!({
                "prompt": "Say hello",
                "skill_references": ["legacy/email"]
            }),
        );

        let resp = router.dispatch(req).await.unwrap();
        let result = result_value(&resp);
        assert!(result["session_id"].as_str().is_some());
        assert!(result["text"].as_str().unwrap().contains("Hello from mock"));
    }

    /// 8c. `turn/start` accepts legacy+structured refs together.
    #[tokio::test]
    async fn turn_start_accepts_mixed_skill_ref_formats() {
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
                    "source_uuid": "dc256086-0d2f-4f61-a307-320d4148107f",
                    "skill_name": "email-extractor"
                }],
                "skill_references": ["dc256086-0d2f-4f61-a307-320d4148107f/email-extractor"]
            }),
        );

        let turn_resp = router.dispatch(turn_req).await.unwrap();
        let turn_result = result_value(&turn_resp);
        assert!(
            turn_result["text"]
                .as_str()
                .unwrap()
                .contains("Hello from mock")
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
                        "source_uuid": "not-a-uuid",
                        "skill_name": "email-extractor"
                    }
                ]
            }),
        );

        let resp = router.dispatch(req).await.unwrap();
        assert_eq!(error_code(&resp), error::INVALID_PARAMS);
    }

    /// 8e. Unknown aliases fail deterministically with configured registry.
    #[tokio::test]
    async fn session_create_rejects_unknown_alias_with_registry() {
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

    /// 12b. `config/patch` refreshes runtime identity registry used by session handlers.
    #[tokio::test]
    async fn config_patch_refreshes_identity_registry_for_alias_resolution() {
        let (router, _notif_rx) = test_router().await;

        let fail_before = make_request(
            "session/create",
            serde_json::json!({
                "prompt": "hello",
                "skill_references": ["legacy/email"]
            }),
        );
        let fail_before_resp = router.dispatch(fail_before).await.unwrap();
        assert_eq!(error_code(&fail_before_resp), error::INVALID_PARAMS);

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

        let success_after = make_request(
            "session/create",
            serde_json::json!({
                "prompt": "hello",
                "skill_references": ["legacy/email"]
            }),
        );
        let success_after_resp = router.dispatch(success_after).await.unwrap();
        let result = result_value(&success_after_resp);
        assert!(result["session_id"].as_str().is_some());
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
