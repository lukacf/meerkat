//! Method router - dispatches JSON-RPC requests to the correct handler.

use std::collections::HashMap;
#[cfg(feature = "mob")]
use std::path::PathBuf;
use std::sync::Arc;
#[cfg(feature = "openai-live")]
use std::sync::RwLock as StdRwLock;

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
    SessionTranscriptRestoreRevisionRequest, SessionTranscriptRevisionListQuery,
    SessionTranscriptRevisionQuery, SessionTranscriptRewriteRequest,
};
use meerkat_core::session::Session;
use meerkat_core::types::SessionId;
#[cfg(feature = "mob")]
use meerkat_core::{AgentToolDispatcher, DynamicToolComposite};
use meerkat_runtime::SessionServiceRuntimeExt as _;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error;
use crate::handlers;
use crate::handlers::RpcResponseExt;
use crate::protocol::{
    RpcNotification, RpcOutboundAdmission, RpcRequest, RpcResponse, RpcStreamTerminalAdmission,
    RpcStreamTerminalReservation, bounded_collection_limit, bounded_collection_offset,
    bounded_stream_terminal_detail,
};
use crate::session_runtime::SessionRuntime;
use meerkat::surface::RequestContext;
use meerkat_contracts::wire::{ToolsRegisterParams, ToolsRegisterResult};

struct RuntimeLiveToolDispatcher {
    runtime: Arc<SessionRuntime>,
}

#[async_trait::async_trait]
impl meerkat_live::LiveToolDispatcher for RuntimeLiveToolDispatcher {
    async fn dispatch_live_tool_call(
        &self,
        session_id: &SessionId,
        call: meerkat_core::ToolCall,
    ) -> Result<meerkat_core::ToolDispatchOutcome, meerkat_live::LiveToolDispatchError> {
        self.runtime
            .dispatch_external_tool_call(session_id, call)
            .await
            .map_err(|err| meerkat_live::LiveToolDispatchError::from_session_error(session_id, err))
    }
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

/// Shared typed `SessionError` → JSON-RPC error mapper for mob-owned session
/// reads/archives. Only `NotFound` maps to `SESSION_NOT_FOUND`; store-layer
/// and other service failures surface as typed internal errors instead of
/// being laundered into "session not found" (K17).
#[cfg(feature = "mob")]
fn mob_session_service_error_response(
    id: Option<crate::protocol::RpcId>,
    session_id: &SessionId,
    service_error: SessionError,
) -> RpcResponse {
    match service_error {
        SessionError::NotFound { .. } => RpcResponse::error(
            id,
            error::SESSION_NOT_FOUND,
            format!("Session not found: {session_id}"),
        ),
        SessionError::Busy { .. } => RpcResponse::error(
            id,
            error::SESSION_BUSY,
            format!("Session is busy: {session_id}"),
        ),
        ref in_progress @ SessionError::FailedWithData {
            ref message,
            ref data,
        } if in_progress.is_runtime_teardown_in_progress() => {
            RpcResponse::error_with_data(id, error::SESSION_BUSY, message.clone(), data.clone())
        }
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
) -> meerkat_mob::ExternalToolsProvider {
    Arc::new(move || {
        let callback_tools = callback_tools_provider();
        compose_rpc_mob_external_tools(callback_tools, configured_mcp_tools.clone())
    })
}

/// The member external tools of one RPC mob: its callback route slot's
/// tools, composed with the configured MCP tools.
#[cfg(feature = "mob")]
fn rpc_mob_slot_external_tools_provider(
    runtime: &Arc<SessionRuntime>,
    slot: Arc<crate::callback_dispatcher::MobCallbackRouteSlot>,
    configured_mcp_tools: Option<Arc<dyn AgentToolDispatcher>>,
) -> meerkat_mob::ExternalToolsProvider {
    let runtime = Arc::downgrade(runtime);
    let callback_tools_provider: Arc<
        dyn Fn() -> Option<Arc<dyn AgentToolDispatcher>> + Send + Sync,
    > = Arc::new(move || {
        Some(Arc::new(
            runtime
                .upgrade()?
                .callback_tool_dispatcher_for_mob_slot(&slot, vec![]),
        ) as Arc<dyn AgentToolDispatcher>)
    });
    rpc_mob_external_tools_provider_from_parts(callback_tools_provider, configured_mcp_tools)
}

/// Compose the canonical RPC mob state before serving a transport.
///
/// Shipping binaries use this seam to install process-scoped capabilities
/// (such as the one D1 reverse-lane acceptor) on the shared runtime. The
/// synchronous router fallback calls the same helper with no acceptor and
/// therefore continues to fail closed for embedders that did not compose one.
#[cfg(feature = "mob")]
pub fn compose_rpc_mob_state(
    runtime: &Arc<SessionRuntime>,
    config_store: &Arc<dyn ConfigStore>,
    controlling_acceptor: Option<meerkat_mob::ControllingAcceptorConfig>,
) -> Result<Arc<meerkat_mob_mcp::MobMcpState>, meerkat_runtime::RuntimeDriverError> {
    // RPC mob member MCP config needs a member-session surface handle. Until
    // an authority-owned handoff exists, configured MCP fails closed instead
    // of staging facts on a router-local owner.
    let configured_mcp_tools: Option<Arc<dyn AgentToolDispatcher>> = None;
    let persistent_mob_root = config_store
        .metadata()
        .and_then(|metadata| metadata.resolved_paths)
        .map(|paths| PathBuf::from(paths.root));
    // Both providers hold the runtime weakly, as the detached owner host
    // below does: the runtime owns this mob state, and the state can outlive
    // the runtime's slots wherever something else keeps it.
    let llm_provider: Arc<dyn Fn() -> Option<Arc<dyn meerkat_client::LlmClient>> + Send + Sync> =
        Arc::new({
            let runtime = Arc::downgrade(runtime);
            move || runtime.upgrade()?.default_llm_client()
        });
    let callback_tools_provider: Arc<
        dyn Fn() -> Option<Arc<dyn AgentToolDispatcher>> + Send + Sync,
    > = Arc::new({
        let runtime = Arc::downgrade(runtime);
        move || {
            runtime
                .upgrade()?
                .callback_tool_dispatcher(vec![])
                .map(|dispatcher| Arc::new(dispatcher) as Arc<dyn AgentToolDispatcher>)
        }
    });
    let tools_provider = rpc_mob_external_tools_provider_from_parts(
        callback_tools_provider,
        configured_mcp_tools.clone(),
    );
    // Every mob built without its creator's route (a restored mob, or one
    // created other than by `mob/create`) gets a route slot of its own, which
    // `mob/claim_callback_route` binds to a connection. On a connection-owned
    // server it starts unclaimed. A single-client host that installed the
    // process-default route holds it with that route, so its restored mobs
    // keep reaching its callbacks as before. The slot is indexed for claims
    // only once its mob is managed, so a build whose mob never gets there (a
    // create that lost to a duplicate) never shadows the live mob's slot.
    let member_external_tools_factory: meerkat_mob_mcp::MemberExternalToolsFactory = Arc::new({
        let runtime = Arc::downgrade(runtime);
        move |_mob_id: &meerkat_mob::MobId| {
            let live = runtime.upgrade()?;
            let slot = Arc::new(match live.default_callback_route() {
                Some(route) => crate::callback_dispatcher::MobCallbackRouteSlot::claimed_by(route),
                None => crate::callback_dispatcher::MobCallbackRouteSlot::unclaimed(),
            });
            let provider = rpc_mob_slot_external_tools_provider(
                &live,
                Arc::clone(&slot),
                configured_mcp_tools.clone(),
            );
            let index_on = Arc::downgrade(&live);
            let on_managed: meerkat_mob_mcp::MemberExternalToolsOnManaged =
                Box::new(move |mob_id: &meerkat_mob::MobId| {
                    if let Some(runtime) = index_on.upgrade() {
                        runtime.index_mob_callback_route_slot(mob_id.clone(), &slot);
                    }
                });
            Some(meerkat_mob_mcp::MemberExternalTools {
                provider,
                on_managed: Some(on_managed),
            })
        }
    });
    let mut state = meerkat_mob_mcp::MobMcpState::new_with_runtime_adapter(
        runtime.session_service(),
        Some(runtime.runtime_adapter()),
        // A16: the local stdio/loopback RPC console is the owning operator
        // (explicit mint, DEC-P5E-8; bearer principals are the v2 seam).
        meerkat_mob::MobControlPrincipal::Owner,
    )?
    .with_persistent_storage_root(persistent_mob_root)
    // Mobs rescope this service's store to their own realm, so the host
    // service is supplied even when no runtime realm identity is active. A
    // disabled WorkGraph backend supplies nothing: members then build without
    // WorkGraph tools and live delegation takes its serial path, instead of
    // every mob operation failing against a store that supports no operation.
    .with_workgraph_service(
        (runtime.workgraph_store().kind() != meerkat::WorkGraphStoreKind::Disabled).then(|| {
            runtime
                .workgraph_service()
                .unwrap_or_else(|_| meerkat::WorkGraphService::new(runtime.workgraph_store()))
        }),
    )
    .with_default_llm_client_provider(Some(llm_provider))
    .with_external_tools_provider(Some(tools_provider))
    .with_member_external_tools_factory(member_external_tools_factory)
    // A top-level session that convenes a detached council is revived
    // through this runtime when its idle executor was retired meanwhile.
    .with_detached_owner_host(Arc::new(crate::detached_owner::RpcDetachedOwnerHost::new(
        runtime,
    )));
    if let Some(acceptor) = controlling_acceptor {
        state = state.with_controlling_acceptor(acceptor);
    }
    // Shared with its self-reference installed, as every other host shares
    // it: each mob create and resume then runs to its managed-set insertion
    // as the state's own task, so a caller cancelled or disconnected
    // mid-create cannot leave a started mob (and its durable store) outside
    // the managed set; restoration can also schedule its own council and
    // fork_off recovery.
    let state = state.into_shared();
    // fork_off and council outcomes are submitted durably to this runtime's
    // delivery owner, which resolves members and confirms jobs through it.
    if let Err(error) = state.bind_continuations(
        runtime.runtime_delivery_inbox(),
        runtime.continuation_bindings(),
    ) {
        tracing::warn!(%error, "fork_off and council run in the turn: continuation owner not bound");
    }
    state.start_workgraph_flow_reconciler();
    Ok(state)
}

#[cfg(feature = "comms")]
fn send_receipt_json(
    receipt: meerkat_core::comms::SendReceipt,
) -> Result<serde_json::Value, serde_json::Error> {
    serde_json::to_value(meerkat_contracts::CommsSendResult::from(receipt))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SessionOwner {
    Runtime,
    #[cfg(feature = "mob")]
    Mob,
}

// `blob/get` params are a canonical contracts wire type (schema-emitted).
use meerkat_contracts::wire::BlobGetParams;

// ---------------------------------------------------------------------------
// NotificationSink
// ---------------------------------------------------------------------------

/// Channel-based sink for sending notifications (agent events) back to the
/// client transport layer.
#[derive(Clone)]
pub struct NotificationSink {
    tx: mpsc::Sender<RpcNotification>,
    outbound_admission: RpcOutboundAdmission,
    stream_terminal_admission: RpcStreamTerminalAdmission,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StreamEmitStatus {
    Delivered,
    Overflow,
    ReceiverGone,
}

#[derive(Serialize)]
struct SessionEventNotificationParams<'a> {
    session_id: String,
    event: &'a EventEnvelope<AgentEvent>,
}

#[derive(Serialize)]
struct SessionStreamEventNotificationParams<'a> {
    stream_id: String,
    sequence: u64,
    session_id: String,
    event: &'a EventEnvelope<AgentEvent>,
}

#[derive(Serialize)]
struct ScopedSessionStreamEventNotificationParams<'a> {
    stream_id: String,
    sequence: u64,
    session_id: String,
    event: &'a EventEnvelope<AgentEvent>,
    scope_id: &'a str,
    scope_path: &'a [meerkat_core::event::StreamScopeFrame],
}

#[cfg(feature = "mob")]
#[derive(Serialize)]
struct MobStreamEventNotificationParams<'a> {
    stream_id: String,
    sequence: u64,
    event: &'a serde_json::Value,
}

impl NotificationSink {
    /// Create a new notification sink backed by the given channel sender.
    pub fn new(tx: mpsc::Sender<RpcNotification>) -> Self {
        Self {
            tx,
            outbound_admission: RpcOutboundAdmission::production(),
            stream_terminal_admission: RpcStreamTerminalAdmission::production(),
        }
    }

    /// Create a no-op sink that discards all notifications.
    /// Used in test/CLI contexts where no RPC transport exists.
    pub fn noop() -> Self {
        let (tx, _rx) = mpsc::channel(1);
        Self::new(tx)
    }

    /// Emit one notification of `method` with `params`, best effort and
    /// without waiting: a full channel or a closed transport drops it.
    /// Returns whether the notification was queued on the connection.
    pub(crate) fn emit_notification<T: Serialize>(&self, method: &str, params: &T) -> bool {
        match RpcNotification::try_new_with_admission(
            method,
            params,
            self.outbound_admission.clone(),
        ) {
            Ok(notification) => self.tx.try_send(notification).is_ok(),
            Err(error) => {
                tracing::warn!(error = %error, method, "dropping notification at outbound RPC admission");
                false
            }
        }
    }

    /// Emit an agent event as a JSON-RPC notification.
    pub async fn emit_event(&self, session_id: &SessionId, event: &EventEnvelope<AgentEvent>) {
        let params = SessionEventNotificationParams {
            session_id: session_id.to_string(),
            event,
        };
        let notification = match RpcNotification::try_new_with_admission(
            "session/event",
            &params,
            self.outbound_admission.clone(),
        ) {
            Ok(notification) => notification,
            Err(error) => {
                tracing::warn!(error = %error, "dropping session event at outbound RPC admission");
                return;
            }
        };
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
        stream_id: &StreamRef,
        sequence: u64,
        session_id: &SessionId,
        event: &EventEnvelope<AgentEvent>,
    ) -> StreamEmitStatus {
        let params = SessionStreamEventNotificationParams {
            stream_id: stream_id.as_string(),
            sequence,
            session_id: session_id.to_string(),
            event,
        };
        let notification = match RpcNotification::try_new_with_admission(
            "session/stream_event",
            &params,
            self.outbound_admission.clone(),
        ) {
            Ok(notification) => notification,
            Err(error) => {
                tracing::warn!(error = %error, "dropping session stream event at outbound RPC admission");
                return StreamEmitStatus::Overflow;
            }
        };
        match self.tx.try_send(notification) {
            Ok(()) => StreamEmitStatus::Delivered,
            Err(TrySendError::Full(_)) => StreamEmitStatus::Overflow,
            Err(TrySendError::Closed(_)) => StreamEmitStatus::ReceiverGone,
        }
    }

    /// Emit a scoped session stream event notification with scope metadata.
    pub(crate) async fn emit_scoped_session_stream_event(
        &self,
        stream_id: &StreamRef,
        sequence: u64,
        session_id: &SessionId,
        event: &EventEnvelope<AgentEvent>,
        scope_id: &str,
        scope_path: &[meerkat_core::event::StreamScopeFrame],
    ) {
        let params = ScopedSessionStreamEventNotificationParams {
            stream_id: stream_id.as_string(),
            sequence,
            session_id: session_id.to_string(),
            event,
            scope_id,
            scope_path,
        };
        let notification = match RpcNotification::try_new_with_admission(
            "session/stream_event",
            &params,
            self.outbound_admission.clone(),
        ) {
            Ok(notification) => notification,
            Err(error) => {
                tracing::warn!(error = %error, "dropping scoped session stream event at outbound RPC admission");
                return;
            }
        };
        let _ = self.tx.send(notification).await;
    }

    /// Emit a machine-authorized terminal notification for a session stream.
    async fn emit_session_stream_end(
        &self,
        terminal: &SessionStreamTerminalAuthority,
        reservation: RpcStreamTerminalReservation,
    ) {
        let mut params = serde_json::json!({
            "stream_id": terminal.stream_id.as_str(),
            "session_id": terminal.session_id.as_str(),
            "ended": true,
            "outcome": rpc_event_stream_terminal_reason_wire(terminal.reason),
        });
        match (terminal.error_code, terminal.detail.as_deref()) {
            (Some(error_code), Some(detail)) => {
                params["error"] = serde_json::json!({
                    "code": rpc_event_stream_terminal_error_code_wire(error_code),
                    "message": detail,
                });
            }
            (Some(_), None) | (None, Some(_)) => {
                tracing::warn!(
                    stream_id = %terminal.stream_id,
                    "generated session event stream terminal effect had inconsistent error fields"
                );
                return;
            }
            (None, None) => {}
        }
        let notification = match reservation.session_notification(&params) {
            Ok(notification) => notification,
            Err(error) => {
                tracing::warn!(error = %error, "session stream terminal exceeded its reserved wire budget");
                return;
            }
        };
        let _ = self.tx.send(notification).await;
    }

    #[cfg(feature = "mob")]
    /// Emit a mob stream event as a JSON-RPC notification.
    ///
    /// For mob-wide streams the event is an [`AttributedEvent`] (source + profile + envelope).
    /// For per-member streams the event is the raw [`EventEnvelope<AgentEvent>`].
    async fn emit_mob_stream_event(
        &self,
        stream_id: &StreamRef,
        sequence: u64,
        event: &serde_json::Value,
    ) -> StreamEmitStatus {
        let params = MobStreamEventNotificationParams {
            stream_id: stream_id.as_string(),
            sequence,
            event,
        };
        let notification = match RpcNotification::try_new_with_admission(
            "mob/stream_event",
            &params,
            self.outbound_admission.clone(),
        ) {
            Ok(notification) => notification,
            Err(error) => {
                tracing::warn!(error = %error, "dropping mob stream event at outbound RPC admission");
                return StreamEmitStatus::Overflow;
            }
        };
        match self.tx.try_send(notification) {
            Ok(()) => StreamEmitStatus::Delivered,
            Err(TrySendError::Full(_)) => StreamEmitStatus::Overflow,
            Err(TrySendError::Closed(_)) => StreamEmitStatus::ReceiverGone,
        }
    }

    #[cfg(feature = "mob")]
    async fn emit_mob_stream_end(
        &self,
        terminal: &MobStreamTerminalAuthority,
        reservation: RpcStreamTerminalReservation,
    ) {
        let mut params = serde_json::json!({
            "stream_id": terminal.stream_id.as_str(),
            "ended": true,
            "outcome": rpc_event_stream_terminal_reason_wire(terminal.reason),
        });
        match (terminal.error_code, terminal.detail.as_deref()) {
            (Some(error_code), Some(detail)) => {
                params["error"] = serde_json::json!({
                    "code": rpc_event_stream_terminal_error_code_wire(error_code),
                    "message": detail,
                });
            }
            (Some(_), None) | (None, Some(_)) => {
                tracing::warn!(
                    stream_id = %terminal.stream_id,
                    "generated mob event stream terminal effect had inconsistent error fields"
                );
                return;
            }
            (None, None) => {}
        }
        let notification = match reservation.mob_notification(&params) {
            Ok(notification) => notification,
            Err(error) => {
                tracing::warn!(error = %error, "mob stream terminal exceeded its reserved wire budget");
                return;
            }
        };
        let _ = self.tx.send(notification).await;
    }
}

/// Typed owner of an RPC event-stream identity.
///
/// A stream identity is minted once on stream open (`StreamRef::mint`) and is
/// the single key the router uses for its in-flight stream registries. The
/// `MeerkatMachine` keys the same stream by its canonical string form, so the
/// only conversions are mint (open), parse-at-ingress from the untrusted wire
/// `stream_id` (`StreamRef::parse`, fail-closed on a malformed value), the
/// canonical authority string (`Display`), and the wire-out string in the
/// open/close results. There is no bare-`Uuid` stream key anywhere.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct StreamRef(Uuid);

impl StreamRef {
    /// Mint a fresh stream identity on stream open.
    fn mint() -> Self {
        StreamRef(Uuid::new_v4())
    }

    /// Parse an untrusted wire `stream_id` into the typed identity.
    ///
    /// Returns `None` for a malformed value so the caller can fail the request
    /// closed at the ingress boundary instead of fabricating an identity.
    fn parse(raw: &str) -> Option<Self> {
        Uuid::parse_str(raw).ok().map(StreamRef)
    }

    /// Canonical string form, as keyed by the `MeerkatMachine` authority and
    /// echoed back on the wire.
    fn as_string(&self) -> String {
        self.0.to_string()
    }
}

impl std::fmt::Display for StreamRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.0, f)
    }
}

struct StreamForwarder {
    terminal: StreamTerminal,
    terminal_reservation: RpcStreamTerminalReservation,
    state: StreamForwarderState,
    session_delivery: Option<Arc<Mutex<SessionStreamDelivery>>>,
}

/// Connection-local delivery accounting, not session lifecycle authority.
/// The same guard covers nonblocking admission and close so a pending gap
/// cannot be hidden by a racing explicit close.
#[derive(Default)]
struct SessionStreamDelivery {
    sequence: u64,
    dropped: u64,
    closing: bool,
}

impl SessionStreamDelivery {
    async fn emit(
        &mut self,
        sink: &NotificationSink,
        stream_id: &StreamRef,
        session_id: &SessionId,
        envelope: &EventEnvelope<AgentEvent>,
    ) -> Option<StreamEmitStatus> {
        if self.closing {
            return None;
        }
        if self.dropped != 0 {
            // Sequence zero identifies an advisory marker, never a fabricated
            // canonical session event. RPC notification order remains monotonic.
            let marker = EventEnvelope::new_session(
                session_id.clone(),
                0,
                None,
                AgentEvent::StreamTruncated {
                    reason: meerkat_core::event::StreamTruncationReason::StreamLagged {
                        dropped: self.dropped,
                    },
                },
            );
            self.sequence += 1;
            match sink
                .emit_session_stream_event(stream_id, self.sequence, session_id, &marker)
                .await
            {
                StreamEmitStatus::Delivered => self.dropped = 0,
                StreamEmitStatus::Overflow => {
                    // This source event must not overtake the unreported gap.
                    // A refused synthetic marker is not itself a lost event.
                    self.dropped = self.dropped.saturating_add(1);
                    return Some(StreamEmitStatus::Overflow);
                }
                StreamEmitStatus::ReceiverGone => return Some(StreamEmitStatus::ReceiverGone),
            }
        }
        self.sequence += 1;
        let status = sink
            .emit_session_stream_event(stream_id, self.sequence, session_id, envelope)
            .await;
        if status == StreamEmitStatus::Overflow {
            self.dropped = self.dropped.saturating_add(1);
        }
        Some(status)
    }

    fn pending_gap_detail(&self) -> Option<String> {
        (self.dropped != 0).then(|| {
            format!("transport stream notification queue overflow: {} event(s) dropped without a delivered gap marker", self.dropped)
        })
    }
}

struct SessionStreamForwarding {
    notification_sink: NotificationSink,
    active_session_streams: Arc<Mutex<HashMap<StreamRef, StreamForwarder>>>,
    stream_authority: RpcStreamAuthority,
    stream_id: StreamRef,
    session_id: SessionId,
    delivery: Arc<Mutex<SessionStreamDelivery>>,
}

impl SessionStreamForwarding {
    async fn run(self, stream: meerkat_core::comms::EventStream, stop_rx: oneshot::Receiver<()>) {
        let Self {
            notification_sink,
            active_session_streams,
            stream_authority,
            stream_id: stream_id_for_task,
            session_id: session_id_for_task,
            delivery: session_delivery,
        } = self;
        let mut stream = stream;
        let mut stop_rx = stop_rx;

        loop {
            tokio::select! {
                _ = &mut stop_rx => {
                    break;
                }
                _ = notification_sink.tx.closed() => {
                    emit_authorized_session_stream_terminal(
                        &notification_sink, &active_session_streams, &stream_authority,
                        stream_id_for_task,
                        meerkat_runtime::meerkat_machine::dsl::RpcEventStreamTerminalObservationKind::NotificationReceiverGone,
                        Some("transport stream notification receiver closed".to_string()),
                    ).await;
                    break;
                }
                event = stream.next() => {
                    match event {
                        Some(envelope) => {
                            let status = session_delivery.lock().await
                                .emit(&notification_sink, &stream_id_for_task, &session_id_for_task, &envelope)
                                .await;
                            let Some(emit_status) = status else { break; };
                            // A temporary backlog drops data with a typed gap;
                            // it does not retire the subscription or await permits.
                            if emit_status == StreamEmitStatus::Overflow {
                                continue;
                            }
                            if let Some((observation, detail)) =
                                terminal_observation_from_emit_status(emit_status)
                            {
                                emit_authorized_session_stream_terminal(
                                    &notification_sink,
                                    &active_session_streams,
                                    &stream_authority,
                                    stream_id_for_task,
                                    observation,
                                    Some(detail.to_string()),
                                )
                                .await;
                                break;
                            }
                        }
                        None => {
                            let mut delivery = session_delivery.lock().await;
                            if delivery.closing { break; }
                            delivery.closing = true;
                            let gap = delivery.pending_gap_detail();
                            drop(delivery);
                            let observation = if gap.is_some() {
                                meerkat_runtime::meerkat_machine::dsl::RpcEventStreamTerminalObservationKind::NotificationQueueOverflow
                            } else {
                                meerkat_runtime::meerkat_machine::dsl::RpcEventStreamTerminalObservationKind::TransportEnded
                            };
                            emit_authorized_session_stream_terminal(
                                &notification_sink,
                                &active_session_streams,
                                &stream_authority,
                                stream_id_for_task,
                                observation,
                                gap,
                            )
                            .await;
                            break;
                        }
                    }
                }
            }
        }
    }
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
        task: Option<JoinHandle<()>>,
    },
}

type RpcStreamAuthority =
    Arc<Mutex<meerkat_runtime::meerkat_machine::dsl::MeerkatMachineAuthority>>;

#[derive(Debug, Clone)]
struct SessionStreamOpenAuthority {
    session_id: String,
    opened: bool,
}

#[derive(Debug, Clone)]
struct SessionStreamTerminalAuthority {
    stream_id: String,
    session_id: String,
    reason: meerkat_runtime::meerkat_machine::dsl::RpcEventStreamTerminalReason,
    error_code: Option<meerkat_runtime::meerkat_machine::dsl::RpcEventStreamTerminalErrorCode>,
    detail: Option<String>,
}

#[derive(Debug, Clone)]
struct SessionStreamCloseAuthority {
    closed: bool,
    already_closed: bool,
    terminal: Option<SessionStreamTerminalAuthority>,
}

#[derive(Debug, Clone)]
struct MobStreamOpenAuthority {
    opened: bool,
}

#[derive(Debug, Clone)]
struct MobStreamTerminalAuthority {
    stream_id: String,
    reason: meerkat_runtime::meerkat_machine::dsl::RpcEventStreamTerminalReason,
    error_code: Option<meerkat_runtime::meerkat_machine::dsl::RpcEventStreamTerminalErrorCode>,
    detail: Option<String>,
}

#[derive(Debug, Clone)]
struct MobStreamCloseAuthority {
    closed: bool,
    already_closed: bool,
    terminal: Option<MobStreamTerminalAuthority>,
}

fn new_rpc_stream_authority() -> meerkat_runtime::meerkat_machine::dsl::MeerkatMachineAuthority {
    let mut authority = meerkat_runtime::meerkat_machine::dsl::MeerkatMachineAuthority::new();
    if let Err(err) = authority
        .apply_signal(meerkat_runtime::meerkat_machine::dsl::MeerkatMachineSignal::Initialize)
    {
        tracing::error!(error = %err, "generated RPC stream authority initialization failed");
    }
    authority
}

fn rpc_event_stream_terminal_reason_wire(
    reason: meerkat_runtime::meerkat_machine::dsl::RpcEventStreamTerminalReason,
) -> &'static str {
    match reason {
        meerkat_runtime::meerkat_machine::dsl::RpcEventStreamTerminalReason::RemoteEnd => {
            "remote_end"
        }
        meerkat_runtime::meerkat_machine::dsl::RpcEventStreamTerminalReason::TerminalError => {
            "terminal_error"
        }
        meerkat_runtime::meerkat_machine::dsl::RpcEventStreamTerminalReason::ExplicitClose => {
            "explicit_close"
        }
    }
}

fn rpc_event_stream_terminal_error_code_wire(
    code: meerkat_runtime::meerkat_machine::dsl::RpcEventStreamTerminalErrorCode,
) -> &'static str {
    match code {
        meerkat_runtime::meerkat_machine::dsl::RpcEventStreamTerminalErrorCode::StreamQueueOverflow => {
            "stream_queue_overflow"
        }
        meerkat_runtime::meerkat_machine::dsl::RpcEventStreamTerminalErrorCode::StreamReceiverGone => {
            "stream_receiver_gone"
        }
    }
}

fn terminal_observation_from_emit_status(
    status: StreamEmitStatus,
) -> Option<(
    meerkat_runtime::meerkat_machine::dsl::RpcEventStreamTerminalObservationKind,
    &'static str,
)> {
    match status {
        StreamEmitStatus::Delivered => None,
        StreamEmitStatus::Overflow => Some((
            meerkat_runtime::meerkat_machine::dsl::RpcEventStreamTerminalObservationKind::NotificationQueueOverflow,
            "transport stream notification queue overflow",
        )),
        StreamEmitStatus::ReceiverGone => Some((
            meerkat_runtime::meerkat_machine::dsl::RpcEventStreamTerminalObservationKind::NotificationReceiverGone,
            "transport stream notification receiver closed",
        )),
    }
}

fn stop_stream_forwarder(stream: &mut StreamForwarder) {
    match &mut stream.state {
        StreamForwarderState::Active { stop_tx, task } => {
            if let Some(stop_tx) = stop_tx.take() {
                let _ = stop_tx.send(());
            }
            if let Some(task) = task.take() {
                task.abort();
            }
        }
    }
}

async fn attach_stream_forwarder_task(
    streams: &Arc<Mutex<HashMap<StreamRef, StreamForwarder>>>,
    stream_id: StreamRef,
    task: JoinHandle<()>,
) {
    let mut task = Some(task);
    {
        let mut streams = streams.lock().await;
        if let Some(stream) = streams.get_mut(&stream_id) {
            match &mut stream.state {
                StreamForwarderState::Active {
                    task: task_slot, ..
                } => {
                    *task_slot = task.take();
                }
            }
        }
    }
    if let Some(task) = task {
        // The forwarder can remove itself before its task handle is attached,
        // while its reserved terminal is waiting for a slow reader. Aborting
        // here would discard that terminal. Explicit close already signals
        // stop_tx; terminal sends remain bounded by their reservations.
        drop(task);
    }
}

async fn resolve_session_stream_open_authority(
    authority: &RpcStreamAuthority,
    stream_id: &StreamRef,
    session_id: &SessionId,
) -> Result<SessionStreamOpenAuthority, String> {
    let stream_id = stream_id.as_string();
    let session_id = session_id.to_string();
    let mut authority = authority.lock().await;
    let transition = meerkat_runtime::meerkat_machine::dsl::MeerkatMachineMutator::apply(
        &mut *authority,
        meerkat_runtime::meerkat_machine::dsl::MeerkatMachineInput::RecordSessionEventStreamOpened {
            stream_id: stream_id.clone(),
            session_id: session_id.clone(),
        },
    )
    .map_err(|error| error.to_string())?;

    transition
        .effects()
        .iter()
        .find_map(|effect| match effect {
            meerkat_runtime::meerkat_machine::dsl::MeerkatMachineEffect::SessionEventStreamOpenResolved {
                stream_id: effect_stream_id,
                session_id: effect_session_id,
                opened,
                ..
            } if *effect_stream_id == stream_id && *effect_session_id == session_id => {
                Some(SessionStreamOpenAuthority {
                    session_id: effect_session_id.clone(),
                    opened: *opened,
                })
            }
            _ => None,
        })
        .ok_or_else(|| {
            format!(
                "RecordSessionEventStreamOpened for stream '{stream_id}' emitted no SessionEventStreamOpenResolved effect"
            )
        })
}

async fn record_session_stream_terminal_authority(
    authority: &RpcStreamAuthority,
    stream_id: &StreamRef,
    observation: meerkat_runtime::meerkat_machine::dsl::RpcEventStreamTerminalObservationKind,
    detail: Option<String>,
) -> Result<SessionStreamTerminalAuthority, String> {
    let mut authority = authority.lock().await;
    record_session_stream_terminal_authority_locked(&mut authority, stream_id, observation, detail)
}

fn record_session_stream_terminal_authority_locked(
    authority: &mut meerkat_runtime::meerkat_machine::dsl::MeerkatMachineAuthority,
    stream_id: &StreamRef,
    observation: meerkat_runtime::meerkat_machine::dsl::RpcEventStreamTerminalObservationKind,
    detail: Option<String>,
) -> Result<SessionStreamTerminalAuthority, String> {
    let detail = bounded_stream_terminal_detail(detail);
    let stream_id = stream_id.as_string();
    let transition = meerkat_runtime::meerkat_machine::dsl::MeerkatMachineMutator::apply(
        authority,
        meerkat_runtime::meerkat_machine::dsl::MeerkatMachineInput::RecordSessionEventStreamTerminated {
            stream_id: stream_id.clone(),
            observation,
            detail: detail.clone(),
        },
    )
    .map_err(|error| error.to_string())?;

    transition
        .effects()
        .iter()
        .find_map(|effect| match effect {
            meerkat_runtime::meerkat_machine::dsl::MeerkatMachineEffect::SessionEventStreamTerminalResolved {
                stream_id: effect_stream_id,
                session_id,
                reason: effect_reason,
                error_code: effect_error_code,
                detail: effect_detail,
                ..
            } if *effect_stream_id == stream_id => {
                Some(SessionStreamTerminalAuthority {
                    stream_id: effect_stream_id.clone(),
                    session_id: session_id.clone(),
                    reason: *effect_reason,
                    error_code: *effect_error_code,
                    detail: effect_detail.clone(),
                })
            }
            _ => None,
        })
        .ok_or_else(|| {
            format!(
                "RecordSessionEventStreamTerminated for stream '{stream_id}' emitted no SessionEventStreamTerminalResolved effect"
            )
        })
}

async fn emit_authorized_session_stream_terminal(
    notification_sink: &NotificationSink,
    active_session_streams: &Arc<Mutex<HashMap<StreamRef, StreamForwarder>>>,
    stream_authority: &RpcStreamAuthority,
    stream_id: StreamRef,
    observation: meerkat_runtime::meerkat_machine::dsl::RpcEventStreamTerminalObservationKind,
    detail: Option<String>,
) {
    let terminal =
        record_session_stream_terminal_authority(stream_authority, &stream_id, observation, detail)
            .await;
    let terminal = match terminal {
        Ok(terminal) => terminal,
        Err(error) => {
            tracing::warn!(
                stream_id = %stream_id,
                "failed to authorize session event stream terminal notification: {error}"
            );
            return;
        }
    };
    let forwarder = active_session_streams.lock().await.remove(&stream_id);
    if let Some(forwarder) = forwarder {
        notification_sink
            .emit_session_stream_end(&terminal, forwarder.terminal_reservation)
            .await;
    }
}

fn resolve_session_stream_close_authority_locked(
    authority: &mut meerkat_runtime::meerkat_machine::dsl::MeerkatMachineAuthority,
    stream_id: &StreamRef,
) -> Result<SessionStreamCloseAuthority, String> {
    let stream_id = stream_id.as_string();
    let transition = meerkat_runtime::meerkat_machine::dsl::MeerkatMachineMutator::apply(
        authority,
        meerkat_runtime::meerkat_machine::dsl::MeerkatMachineInput::ResolveSessionEventStreamClose {
            stream_id: stream_id.clone(),
        },
    )
    .map_err(|error| error.to_string())?;

    let close = transition.effects().iter().find_map(|effect| match effect {
        meerkat_runtime::meerkat_machine::dsl::MeerkatMachineEffect::SessionEventStreamCloseResolved {
            stream_id: effect_stream_id,
            closed,
            already_closed,
            ..
        } if *effect_stream_id == stream_id => Some((*closed, *already_closed)),
        _ => None,
    });
    let terminal = transition.effects().iter().find_map(|effect| match effect {
            meerkat_runtime::meerkat_machine::dsl::MeerkatMachineEffect::SessionEventStreamTerminalResolved {
                stream_id: effect_stream_id,
                session_id,
                reason,
                error_code,
                detail,
                ..
            } if *effect_stream_id == stream_id => Some(SessionStreamTerminalAuthority {
                stream_id: effect_stream_id.clone(),
                session_id: session_id.clone(),
                reason: *reason,
                error_code: *error_code,
                detail: detail.clone(),
            }),
        _ => None,
    });

    close
        .map(|(closed, already_closed)| SessionStreamCloseAuthority {
            closed,
            already_closed,
            terminal,
        })
        .ok_or_else(|| {
            format!(
                "ResolveSessionEventStreamClose for stream '{stream_id}' emitted no SessionEventStreamCloseResolved effect"
            )
        })
}

async fn close_session_stream(
    notification_sink: &NotificationSink,
    active_streams: &Arc<Mutex<HashMap<StreamRef, StreamForwarder>>>,
    stream_authority: &RpcStreamAuthority,
    stream_id: &StreamRef,
) -> Result<SessionStreamCloseAuthority, String> {
    {
        let mut authority = stream_authority.lock().await;
        if authority
            .state()
            .closed_session_event_streams
            .contains(&stream_id.as_string())
        {
            // Another terminal issuer may be waiting for the physical map.
            // A repeated close must not take or wait for its reservation.
            return resolve_session_stream_close_authority_locked(&mut authority, stream_id);
        }
    }
    let delivery = active_streams
        .lock()
        .await
        .get(stream_id)
        .and_then(|stream| stream.session_delivery.clone());
    let mut delivery = match delivery.as_ref() {
        Some(delivery) => Some(delivery.lock().await),
        None => None,
    };
    let mut authority = stream_authority.lock().await;
    if authority
        .state()
        .closed_session_event_streams
        .contains(&stream_id.as_string())
    {
        return resolve_session_stream_close_authority_locked(&mut authority, stream_id);
    }
    let mut streams = active_streams.lock().await;
    // All awaited locks precede sealing or machine mutation. From the first
    // effect through reservation transfer there is no cancellation point.
    let gap = delivery
        .as_ref()
        .and_then(|delivery| delivery.pending_gap_detail());
    let pending_terminal = if let Some(detail) = gap {
        Some(record_session_stream_terminal_authority_locked(
            &mut authority,
            stream_id,
            meerkat_runtime::meerkat_machine::dsl::RpcEventStreamTerminalObservationKind::NotificationQueueOverflow,
            Some(detail),
        )?)
    } else {
        None
    };
    let close = resolve_session_stream_close_authority_locked(&mut authority, stream_id);
    if let Some(delivery) = delivery.as_mut() {
        delivery.closing = true;
    }
    let terminal =
        pending_terminal.or_else(|| close.as_ref().ok().and_then(|close| close.terminal.clone()));
    let send = terminal.and_then(|terminal| {
        streams.remove(stream_id).map(|mut stream| {
            stop_stream_forwarder(&mut stream);
            let sink = notification_sink.clone();
            // Custody survives caller cancellation. The existing reservation bounds
            // this task, and receiver closure releases it without a second queue.
            tokio::spawn(async move {
                sink.emit_session_stream_end(&terminal, stream.terminal_reservation)
                    .await;
            })
        })
    });
    if close.is_err() {
        if let Some(mut stream) = streams.remove(stream_id) {
            stop_stream_forwarder(&mut stream);
        }
    }
    drop(streams);
    drop(authority);
    drop(delivery);
    if let Some(send) = send {
        send.await
            .map_err(|error| format!("session stream terminal sender failed: {error}"))?;
    }
    close
}

async fn resolve_mob_stream_open_authority(
    authority: &RpcStreamAuthority,
    stream_id: &StreamRef,
) -> Result<MobStreamOpenAuthority, String> {
    let stream_id = stream_id.as_string();
    let mut authority = authority.lock().await;
    let transition = meerkat_runtime::meerkat_machine::dsl::MeerkatMachineMutator::apply(
        &mut *authority,
        meerkat_runtime::meerkat_machine::dsl::MeerkatMachineInput::RecordMobEventStreamOpened {
            stream_id: stream_id.clone(),
        },
    )
    .map_err(|error| error.to_string())?;

    transition
        .effects()
        .iter()
        .find_map(|effect| match effect {
            meerkat_runtime::meerkat_machine::dsl::MeerkatMachineEffect::MobEventStreamOpenResolved {
                stream_id: effect_stream_id,
                opened,
                ..
            } if *effect_stream_id == stream_id => Some(MobStreamOpenAuthority { opened: *opened }),
            _ => None,
        })
        .ok_or_else(|| {
            format!(
                "RecordMobEventStreamOpened for stream '{stream_id}' emitted no MobEventStreamOpenResolved effect"
            )
        })
}

async fn record_mob_stream_terminal_authority(
    authority: &RpcStreamAuthority,
    stream_id: &StreamRef,
    observation: meerkat_runtime::meerkat_machine::dsl::RpcEventStreamTerminalObservationKind,
    detail: Option<String>,
) -> Result<MobStreamTerminalAuthority, String> {
    let detail = bounded_stream_terminal_detail(detail);
    let stream_id = stream_id.as_string();
    let mut authority = authority.lock().await;
    let transition = meerkat_runtime::meerkat_machine::dsl::MeerkatMachineMutator::apply(
        &mut *authority,
        meerkat_runtime::meerkat_machine::dsl::MeerkatMachineInput::RecordMobEventStreamTerminated {
            stream_id: stream_id.clone(),
            observation,
            detail: detail.clone(),
        },
    )
    .map_err(|error| error.to_string())?;

    transition
        .effects()
        .iter()
        .find_map(|effect| match effect {
            meerkat_runtime::meerkat_machine::dsl::MeerkatMachineEffect::MobEventStreamTerminalResolved {
                stream_id: effect_stream_id,
                reason: effect_reason,
                error_code: effect_error_code,
                detail: effect_detail,
                ..
            } if *effect_stream_id == stream_id => {
                Some(MobStreamTerminalAuthority {
                    stream_id: effect_stream_id.clone(),
                    reason: *effect_reason,
                    error_code: *effect_error_code,
                    detail: effect_detail.clone(),
                })
            }
            _ => None,
        })
        .ok_or_else(|| {
            format!(
                "RecordMobEventStreamTerminated for stream '{stream_id}' emitted no MobEventStreamTerminalResolved effect"
            )
        })
}

/// Serialize an authoritative mob stream event, refusing to fabricate a
/// `Value::Null` (or any other) event when serialization fails.
///
/// A serialization failure on the authoritative stream is a terminal fault: we
/// cannot lawfully continue the stream after silently dropping or fabricating an
/// event, because either choice would launder the fault into a fabricated
/// success on the wire. The caller terminates the stream with the recorded fault
/// detail instead.
#[cfg(feature = "mob")]
fn serialize_mob_stream_event<T: serde::Serialize>(
    event: &T,
) -> Result<serde_json::Value, serde_json::Error> {
    serde_json::to_value(event)
}

/// Terminate a mob stream after an authoritative event failed to serialize.
///
/// This records the typed serialization fault as the terminal detail and emits a
/// machine-authorized terminal-error notification, so the client observes a
/// truthful terminal instead of a fabricated `Value::Null` event.
#[cfg(feature = "mob")]
async fn emit_authorized_mob_stream_serialization_terminal(
    notification_sink: &NotificationSink,
    active_mob_streams: &Arc<Mutex<HashMap<StreamRef, StreamForwarder>>>,
    stream_authority: &RpcStreamAuthority,
    stream_id: StreamRef,
    serialize_error: &serde_json::Error,
) {
    tracing::error!(
        stream_id = %stream_id,
        error = %serialize_error,
        "failed to serialize authoritative mob stream event; terminating stream as terminal error"
    );
    emit_authorized_mob_stream_terminal(
        notification_sink,
        active_mob_streams,
        stream_authority,
        stream_id,
        // No observation variant models a serialization fault yet; the
        // queue-overflow observation is the closest terminal-*error* class
        // (vs. TransportEnded, which would mislabel this as a clean remote
        // end). The cause is recorded in the bounded diagnostic detail.
        meerkat_runtime::meerkat_machine::dsl::RpcEventStreamTerminalObservationKind::NotificationQueueOverflow,
        Some(format!(
            "authoritative mob stream event failed to serialize: {serialize_error}"
        )),
    )
    .await;
}

#[cfg(feature = "mob")]
async fn emit_authorized_mob_stream_terminal(
    notification_sink: &NotificationSink,
    active_mob_streams: &Arc<Mutex<HashMap<StreamRef, StreamForwarder>>>,
    stream_authority: &RpcStreamAuthority,
    stream_id: StreamRef,
    observation: meerkat_runtime::meerkat_machine::dsl::RpcEventStreamTerminalObservationKind,
    detail: Option<String>,
) {
    let terminal =
        record_mob_stream_terminal_authority(stream_authority, &stream_id, observation, detail)
            .await;
    let terminal = match terminal {
        Ok(terminal) => terminal,
        Err(error) => {
            tracing::warn!(
                stream_id = %stream_id,
                "failed to authorize mob event stream terminal notification: {error}"
            );
            return;
        }
    };
    let forwarder = active_mob_streams.lock().await.remove(&stream_id);
    if let Some(forwarder) = forwarder {
        notification_sink
            .emit_mob_stream_end(&terminal, forwarder.terminal_reservation)
            .await;
    }
}

async fn resolve_mob_stream_close_authority(
    authority: &RpcStreamAuthority,
    stream_id: &StreamRef,
) -> Result<MobStreamCloseAuthority, String> {
    let stream_id = stream_id.as_string();
    let mut authority = authority.lock().await;
    let transition = meerkat_runtime::meerkat_machine::dsl::MeerkatMachineMutator::apply(
        &mut *authority,
        meerkat_runtime::meerkat_machine::dsl::MeerkatMachineInput::ResolveMobEventStreamClose {
            stream_id: stream_id.clone(),
        },
    )
    .map_err(|error| error.to_string())?;

    let close = transition.effects().iter().find_map(|effect| {
        match effect {
        meerkat_runtime::meerkat_machine::dsl::MeerkatMachineEffect::MobEventStreamCloseResolved {
            stream_id: effect_stream_id,
            closed,
            already_closed,
            ..
        } if *effect_stream_id == stream_id => Some((*closed, *already_closed)),
        _ => None,
    }
    });
    let terminal = transition.effects().iter().find_map(|effect| match effect {
            meerkat_runtime::meerkat_machine::dsl::MeerkatMachineEffect::MobEventStreamTerminalResolved {
                stream_id: effect_stream_id,
                reason,
                error_code,
                detail,
                ..
            } if *effect_stream_id == stream_id => Some(MobStreamTerminalAuthority {
                stream_id: effect_stream_id.clone(),
                reason: *reason,
                error_code: *error_code,
                detail: detail.clone(),
            }),
        _ => None,
    });

    close
        .map(|(closed, already_closed)| MobStreamCloseAuthority {
            closed,
            already_closed,
            terminal,
        })
        .ok_or_else(|| {
            format!(
                "ResolveMobEventStreamClose for stream '{stream_id}' emitted no MobEventStreamCloseResolved effect"
            )
        })
}

// ---------------------------------------------------------------------------
// MethodRouter
// ---------------------------------------------------------------------------

fn lower_session_fork_at_request(
    params: &meerkat_contracts::ForkSessionAtParams,
) -> SessionForkAtRequest {
    SessionForkAtRequest {
        message_index: params.message_index,
        running_behavior: params.running_behavior,
        tool_access_policy: params
            .tool_access_policy
            .clone()
            .map(meerkat_contracts::wire::WireToolAccessPolicy::into_core),
        inherited_instructions: params.inherited_instructions,
    }
}

fn lower_session_fork_replace_request(
    params: &meerkat_contracts::ForkSessionReplaceParams,
    replacement: meerkat_core::session::TranscriptReplacement,
) -> SessionForkReplaceRequest {
    SessionForkReplaceRequest {
        message_index: params.message_index,
        replacement,
        running_behavior: params.running_behavior,
        tool_access_policy: params
            .tool_access_policy
            .clone()
            .map(meerkat_contracts::wire::WireToolAccessPolicy::into_core),
        inherited_instructions: params.inherited_instructions,
    }
}

/// One routed response plus any custody that ends at transport delivery.
#[cfg(feature = "openai-live")]
#[derive(Clone, Copy)]
enum ExperimentalLiveOpenDeliveryDisposition {
    Delivered,
    Rejected,
}

/// Transport-delivery custody for one strict experimental live/open result.
///
/// The exact channel and token remain provisional until the JSON-RPC writer
/// confirms delivery. Dropping this owner closes the decision channel, which
/// the coordinator treats as rejection and routes through the same exact
/// publication-failure cleanup used for serialization failure.
#[cfg(feature = "openai-live")]
pub(crate) struct ExperimentalLiveOpenDeliveryCustody {
    decision_tx: tokio::sync::oneshot::Sender<ExperimentalLiveOpenDeliveryDisposition>,
    cleanup_task: tokio::task::JoinHandle<Result<(), String>>,
}

#[cfg(feature = "openai-live")]
impl ExperimentalLiveOpenDeliveryCustody {
    fn new(
        runtime: Arc<SessionRuntime>,
        host: Arc<meerkat_live::LiveAdapterHost>,
        publication: handlers::live::ExperimentalLiveOpenPublication,
        #[cfg(feature = "live-webrtc")]
        playback_custodies: handlers::live::ExperimentalLivePlaybackCustodies,
    ) -> Self {
        let (decision_tx, decision_rx) = tokio::sync::oneshot::channel();
        let cleanup_task = tokio::spawn(async move {
            if matches!(
                decision_rx.await,
                Ok(ExperimentalLiveOpenDeliveryDisposition::Delivered)
            ) {
                return Ok(());
            }
            #[cfg(feature = "live-webrtc")]
            playback_custodies
                .lock()
                .await
                .remove(&publication.channel_id);
            runtime
                .cleanup_experimental_live_channel_after_publication_failure(
                    &host,
                    publication.authority.as_ref(),
                    &publication.session_id,
                    &publication.channel_id,
                )
                .await
                .map_err(|error| error.to_string())
        });
        Self {
            decision_tx,
            cleanup_task,
        }
    }

    async fn settle(
        self,
        disposition: ExperimentalLiveOpenDeliveryDisposition,
    ) -> Result<(), String> {
        self.decision_tx.send(disposition).map_err(|_| {
            "experimental live/open cleanup coordinator stopped before delivery decision"
                .to_string()
        })?;
        self.cleanup_task
            .await
            .map_err(|error| format!("experimental live/open cleanup task failed: {error}"))?
    }

    async fn delivered(self) -> Result<(), String> {
        self.settle(ExperimentalLiveOpenDeliveryDisposition::Delivered)
            .await
    }

    async fn rejected(self) -> Result<(), String> {
        self.settle(ExperimentalLiveOpenDeliveryDisposition::Rejected)
            .await
    }
}

#[cfg(all(test, feature = "openai-live"))]
pub(crate) fn test_experimental_live_open_delivery_custody() -> (
    ExperimentalLiveOpenDeliveryCustody,
    tokio::sync::oneshot::Receiver<bool>,
) {
    let (decision_tx, decision_rx) = tokio::sync::oneshot::channel();
    let (settled_tx, settled_rx) = tokio::sync::oneshot::channel();
    let cleanup_task = tokio::spawn(async move {
        let delivered = matches!(
            decision_rx.await,
            Ok(ExperimentalLiveOpenDeliveryDisposition::Delivered)
        );
        let _ = settled_tx.send(delivered);
        Ok(())
    });
    (
        ExperimentalLiveOpenDeliveryCustody {
            decision_tx,
            cleanup_task,
        },
        settled_rx,
    )
}

pub(crate) struct RoutedRpcResponse {
    pub(crate) response: RpcResponse,
    #[cfg(feature = "live-webrtc")]
    live_webrtc_answer_delivery: Option<handlers::live::LiveWebrtcAnswerDeliveryCustody>,
    #[cfg(feature = "openai-live")]
    experimental_live_open_delivery: Option<ExperimentalLiveOpenDeliveryCustody>,
}

impl RoutedRpcResponse {
    pub(crate) fn new(response: RpcResponse) -> Self {
        Self {
            response,
            #[cfg(feature = "live-webrtc")]
            live_webrtc_answer_delivery: None,
            #[cfg(feature = "openai-live")]
            experimental_live_open_delivery: None,
        }
    }

    #[cfg(feature = "live-webrtc")]
    fn with_live_webrtc_answer(result: handlers::live::LiveWebrtcAnswerHandlerResult) -> Self {
        Self::with_live_webrtc_delivery(result.response, result.delivery_custody)
    }

    #[cfg(feature = "live-webrtc")]
    pub(crate) fn with_live_webrtc_delivery(
        response: RpcResponse,
        delivery: Option<handlers::live::LiveWebrtcAnswerDeliveryCustody>,
    ) -> Self {
        Self {
            response,
            live_webrtc_answer_delivery: delivery,
            #[cfg(feature = "openai-live")]
            experimental_live_open_delivery: None,
        }
    }

    #[cfg(feature = "openai-live")]
    pub(crate) fn with_experimental_live_open(
        response: RpcResponse,
        custody: Option<ExperimentalLiveOpenDeliveryCustody>,
    ) -> Self {
        Self {
            response,
            #[cfg(feature = "live-webrtc")]
            live_webrtc_answer_delivery: None,
            experimental_live_open_delivery: custody,
        }
    }

    pub(crate) async fn settle_delivery(&mut self, delivered: bool) -> Result<(), String> {
        #[cfg(feature = "live-webrtc")]
        if let Some(custody) = self.live_webrtc_answer_delivery.take() {
            if delivered {
                custody.delivered().await?;
            } else {
                custody.rejected().await?;
            }
        }
        #[cfg(feature = "openai-live")]
        if let Some(custody) = self.experimental_live_open_delivery.take() {
            if delivered {
                custody.delivered().await?;
            } else {
                custody.rejected().await?;
            }
        }
        #[cfg(not(feature = "live-webrtc"))]
        let _ = delivered;
        Ok(())
    }

    fn into_response(self) -> RpcResponse {
        self.response
    }
}

/// Dispatches incoming JSON-RPC requests to the appropriate handler.
/// The host-only native consent decision method, reachable only through
/// [`MethodRouter::dispatch_with_consent_event`].
pub const CONSENT_DECIDE_METHOD: &str = "approval/consent_decide";

#[derive(Clone)]
pub struct MethodRouter {
    #[cfg(feature = "local-authorization")]
    governed_connection: Option<Arc<crate::governed_jsonl::GovernedConnection>>,
    runtime: Arc<SessionRuntime>,
    /// The callback route owned by this router's connection. `None` falls
    /// back to the runtime's process-default route.
    callback_route: Option<crate::callback_dispatcher::CallbackRoute>,
    config_store: Arc<dyn ConfigStore>,
    notification_sink: NotificationSink,
    skill_runtime: Option<Arc<meerkat_core::skills::SkillRuntime>>,
    active_session_streams: Arc<Mutex<HashMap<StreamRef, StreamForwarder>>>,
    stream_authority: RpcStreamAuthority,
    #[cfg(feature = "mob")]
    mob_state: Arc<meerkat_mob_mcp::MobMcpState>,
    #[cfg(feature = "mob")]
    active_mob_streams: Arc<Mutex<HashMap<StreamRef, StreamForwarder>>>,
    runtime_adapter: Arc<meerkat_runtime::MeerkatMachine>,
    live_adapter_host: Arc<meerkat_live::LiveAdapterHost>,
    live_ws_state: Option<Arc<meerkat_live::LiveWsState>>,
    live_ws_base_url: Option<String>,
    #[cfg(feature = "live-webrtc")]
    live_webrtc_state: Option<Arc<meerkat_live::LiveWebrtcState>>,
    #[cfg(feature = "live-webrtc")]
    live_webrtc_answer_transport: Option<Arc<dyn meerkat_live::LiveWebrtcAnswerTransport>>,
    #[cfg(feature = "openai-live")]
    experimental_live_open_authority:
        Option<Arc<dyn meerkat::experimental_gpt_live::ExperimentalLiveOpenAuthorityProvider>>,
    #[cfg(feature = "openai-live")]
    experimental_live_public_observation_publisher:
        Option<Arc<dyn meerkat::experimental_gpt_live::ExperimentalLivePublicObservationPublisher>>,
    #[cfg(all(feature = "openai-live", feature = "live-webrtc"))]
    experimental_live_playback_custodies: handlers::live::ExperimentalLivePlaybackCustodies,
    #[cfg(all(feature = "openai-live", feature = "mob"))]
    experimental_live_delegation_coordinator:
        Arc<meerkat_mob_mcp::live_delegation::ExperimentalLiveDelegationCoordinator>,
    #[cfg(all(feature = "openai-live", feature = "mob", feature = "live-webrtc"))]
    experimental_live_context_mirror_host: Arc<
        StdRwLock<
            Option<
                Arc<
                    meerkat::surface::ExperimentalGptLiveContextMirrorHost<
                        meerkat::FactoryAgentBuilder,
                    >,
                >,
            >,
        >,
    >,
    live_session_factory: Option<Arc<dyn meerkat_client::realtime_session::RealtimeSessionFactory>>,
}

use meerkat_runtime::stack_relief::box_in_own_frame as routed_arm;

impl MethodRouter {
    /// Create a new method router.
    ///
    /// Reuses existing mob state from the runtime if available, otherwise
    /// creates a default. Also spawns schedule host startup.
    pub fn new(
        runtime: Arc<SessionRuntime>,
        config_store: Arc<dyn ConfigStore>,
        notification_sink: NotificationSink,
    ) -> Result<Self, meerkat_runtime::RuntimeDriverError> {
        let runtime_adapter = runtime.runtime_adapter();
        // Reuse the runtime's existing mob state if one was pre-configured
        // (e.g., by a kennel that created a hive mob before serving TCP
        // connections). Only create a fresh MobMcpState when no existing
        // state is present.
        #[cfg(feature = "mob")]
        let mob_state = if let Some(existing) = runtime.mob_state() {
            existing
        } else {
            let mob_state = compose_rpc_mob_state(&runtime, &config_store, None)?;
            runtime.set_mob_state(mob_state.clone());
            mob_state
        };
        #[cfg(feature = "mob")]
        runtime.set_mob_tools(Arc::new(meerkat_mob_mcp::AgentMobToolSurfaceFactory::new(
            mob_state.clone(),
        )));
        #[cfg(all(feature = "openai-live", feature = "mob"))]
        let experimental_live_delegation_coordinator =
            meerkat_mob_mcp::live_delegation::compose_experimental_live_delegation_coordinator(
                Arc::clone(&runtime_adapter),
                Arc::clone(&mob_state),
            );
        let schedule_runtime = runtime.clone();
        tokio::spawn(async move {
            if let Err(error) = schedule_runtime.ensure_schedule_host_started().await {
                tracing::warn!("failed to start RPC schedule host: {error}");
            }
        });
        // Ensure the runtime's notification sink is up-to-date so that
        // executors created lazily read the current sink at apply time.
        runtime.set_notification_sink(notification_sink.clone());
        runtime.arm_runtime_delivery_owner();
        Ok(Self {
            #[cfg(feature = "local-authorization")]
            governed_connection: None,
            runtime,
            config_store,
            notification_sink,
            skill_runtime: None,
            callback_route: None,
            active_session_streams: Arc::new(Mutex::new(HashMap::new())),
            stream_authority: Arc::new(Mutex::new(new_rpc_stream_authority())),
            #[cfg(feature = "mob")]
            mob_state,
            #[cfg(feature = "mob")]
            active_mob_streams: Arc::new(Mutex::new(HashMap::new())),
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
            #[cfg(feature = "live-webrtc")]
            live_webrtc_state: None,
            #[cfg(feature = "live-webrtc")]
            live_webrtc_answer_transport: None,
            #[cfg(feature = "openai-live")]
            experimental_live_open_authority: None,
            #[cfg(feature = "openai-live")]
            experimental_live_public_observation_publisher: None,
            #[cfg(all(feature = "openai-live", feature = "live-webrtc"))]
            experimental_live_playback_custodies: Arc::new(Mutex::new(HashMap::new())),
            #[cfg(all(feature = "openai-live", feature = "mob"))]
            experimental_live_delegation_coordinator,
            #[cfg(all(feature = "openai-live", feature = "mob", feature = "live-webrtc"))]
            experimental_live_context_mirror_host: Arc::new(StdRwLock::new(None)),
            live_session_factory: None,
        })
    }

    #[cfg(feature = "local-authorization")]
    pub(crate) fn with_governed_connection(
        mut self,
        connection: Arc<crate::governed_jsonl::GovernedConnection>,
        tools: Vec<meerkat_core::ToolDef>,
    ) -> Result<Self, meerkat_runtime::RuntimeDriverError> {
        let route = self
            .callback_route
            .as_ref()
            .filter(|route| !route.is_closed())
            .ok_or_else(crate::governed_jsonl::unsupported)?;
        // Commission only this connection's catalog, once before serving.
        route
            .registry()
            .replace_or_add(tools)
            .map_err(|()| crate::governed_jsonl::unsupported())?;
        self.governed_connection = Some(connection);
        Ok(self)
    }

    /// The connection this router serves closed: its governed connection's
    /// pipe-lifetime facts (the pipe's consent scope) end here.
    #[cfg(feature = "local-authorization")]
    pub(crate) fn close_governed_connection(&self) {
        if let Some(connection) = &self.governed_connection {
            connection.close();
        }
    }

    fn attach_live_host(&mut self, host: Arc<meerkat_live::LiveAdapterHost>) {
        self.live_adapter_host = host;
        self.live_adapter_host
            .set_live_tool_dispatcher(Arc::new(RuntimeLiveToolDispatcher {
                runtime: Arc::clone(&self.runtime),
            }));
        // P1#5: hand the host to the runtime so config propagation fans out
        // through the shared live runtime, independent of the transport skin.
        self.runtime
            .set_live_adapter_host(Arc::clone(&self.live_adapter_host));
    }

    /// Attach a live WebSocket transport state for `live/open` token minting.
    ///
    /// Also closes A4/A5: the live host gets a session-scoped tool dispatcher
    /// that calls `SessionRuntime::dispatch_external_tool_call`, so provider
    /// tool requests use the same composed dispatcher as ordinary Meerkat
    /// turns. SDK callback tools remain one possible external tool source
    /// inside that composed dispatcher.
    pub fn with_live_ws(mut self, state: Arc<meerkat_live::LiveWsState>, base_url: String) -> Self {
        self.attach_live_host(Arc::clone(state.host()));
        self.live_ws_state = Some(state);
        self.live_ws_base_url = Some(base_url);
        self.sync_member_live_host();
        self
    }

    /// Phase 6b (ADJ-P6B-16): once the live-capable composition is complete
    /// (any live transport + per-open factory), install the facade
    /// `ServiceMemberLiveHost` on (a) the machine slot — member sessions
    /// bound to a remote mob supervisor serve the bridge live arms through
    /// it — and (b) the mob console state, which passes it through the
    /// `MobBuilder::with_member_live_host` knob for the LOCAL placement
    /// branch. Partial composition installs nothing: the arms and the
    /// local branch degrade typed `LiveTransportUnavailable`.
    fn sync_member_live_host(&self) {
        let Some(session_factory) = self.live_session_factory.as_ref() else {
            return;
        };
        if !self.live_enabled() {
            return;
        }
        let live_host = meerkat::surface::ServiceMemberLiveHost::new(
            meerkat::surface::ServiceMemberLiveHostConfig {
                service: self.runtime.persistent_service(),
                runtime_adapter: Arc::clone(&self.runtime_adapter),
                host: Arc::clone(&self.live_adapter_host),
                ws_state: self.live_ws_state.clone(),
                base_url: self.live_ws_base_url.clone(),
                session_factory: Arc::clone(session_factory),
                realm_id: self.runtime.realm_id(),
                instance_id: self.runtime.instance_id(),
                backend: self.runtime.backend(),
            },
        );
        #[cfg(feature = "live-webrtc")]
        let live_host = match self.live_webrtc_state.as_ref() {
            Some(state) => live_host.with_webrtc_cleanup_state(Arc::clone(state)),
            None => live_host,
        };
        let live_host = Arc::new(live_host);
        #[cfg(all(feature = "openai-live", feature = "mob", feature = "live-webrtc"))]
        if let Some(authority) = self.experimental_live_open_authority.as_ref() {
            authority.bind_post_close_work_source(Arc::clone(
                &self.experimental_live_delegation_coordinator,
            )
                as Arc<dyn meerkat::experimental_gpt_live::LivePostCloseWorkSource>);
            let context_host = meerkat::surface::ExperimentalGptLiveContextMirrorHost::new(
                Arc::clone(&self.runtime_adapter),
                Arc::clone(&live_host),
                Arc::clone(authority),
                Arc::clone(&self.experimental_live_delegation_coordinator)
                    as Arc<
                        dyn meerkat::experimental_gpt_live::ExperimentalLiveBoundChannelActivator,
                    >,
            );
            *self
                .experimental_live_context_mirror_host
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(context_host);
        }
        let live_host: Arc<dyn meerkat_runtime::member_live::MemberLiveHost> = live_host;
        self.runtime_adapter
            .set_member_live_host(Arc::clone(&live_host));
        #[cfg(feature = "mob")]
        self.mob_state.set_member_live_host(live_host);
    }

    /// Attach live WebRTC signaling state for `live/open` token minting and
    /// `live/webrtc/answer` SDP answers.
    #[cfg(feature = "live-webrtc")]
    pub fn with_live_webrtc(mut self, state: Arc<meerkat_live::LiveWebrtcState>) -> Self {
        self.attach_live_host(Arc::clone(state.host()));
        self.live_webrtc_answer_transport = Some(state.clone());
        self.live_webrtc_state = Some(state);
        self.sync_member_live_host();
        self
    }

    /// Replace only SDP answer construction/sideband custody while retaining
    /// the existing machine-owned token and public-result path.
    #[cfg(feature = "live-webrtc")]
    pub fn with_live_webrtc_answer_transport(
        mut self,
        transport: Arc<dyn meerkat_live::LiveWebrtcAnswerTransport>,
    ) -> Self {
        self.live_webrtc_answer_transport = Some(transport);
        self
    }

    /// Install the host-owned authority that may admit strict channel-scoped
    /// execution identity requests. Capability advertisement must be wired
    /// from the same configured authority by the embedding host.
    #[cfg(feature = "openai-live")]
    pub fn with_experimental_live_open_authority(
        mut self,
        authority: Arc<dyn meerkat::experimental_gpt_live::ExperimentalLiveOpenAuthorityProvider>,
    ) -> Self {
        self.experimental_live_open_authority = Some(authority);
        self.sync_member_live_host();
        self
    }

    /// Install the connection-scoped publisher that acknowledges an
    /// experimental assistant output only after the outer JSON-RPC writer has
    /// flushed its sanitized notification.
    #[cfg(feature = "openai-live")]
    pub(crate) fn with_experimental_live_public_observation_publisher(
        mut self,
        publisher: Arc<
            dyn meerkat::experimental_gpt_live::ExperimentalLivePublicObservationPublisher,
        >,
    ) -> Self {
        self.experimental_live_public_observation_publisher = Some(publisher);
        self
    }

    #[cfg(feature = "openai-live")]
    fn experimental_live_bound_channel_activator(
        &self,
    ) -> Option<Arc<dyn meerkat::experimental_gpt_live::ExperimentalLiveBoundChannelActivator>>
    {
        #[cfg(feature = "mob")]
        {
            #[cfg(feature = "live-webrtc")]
            if let Some(host) = self
                .experimental_live_context_mirror_host
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_ref()
            {
                return Some(Arc::clone(host)
                    as Arc<
                        dyn meerkat::experimental_gpt_live::ExperimentalLiveBoundChannelActivator,
                    >);
            }
            Some(Arc::clone(&self.experimental_live_delegation_coordinator)
                as Arc<
                    dyn meerkat::experimental_gpt_live::ExperimentalLiveBoundChannelActivator,
                >)
        }
        #[cfg(not(feature = "mob"))]
        {
            None
        }
    }

    #[cfg(all(feature = "openai-live", feature = "mob", feature = "live-webrtc"))]
    pub async fn pending_experimental_live_replacement_required(
        &self,
        session_id: &SessionId,
    ) -> Option<meerkat::surface::ExperimentalLiveReplacementRequired> {
        let host = self
            .experimental_live_context_mirror_host
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()?;
        host.pending_replacement_required(session_id).await
    }

    fn live_enabled(&self) -> bool {
        self.live_ws_state.is_some() || {
            #[cfg(feature = "live-webrtc")]
            {
                self.live_webrtc_state.is_some()
            }
            #[cfg(not(feature = "live-webrtc"))]
            {
                false
            }
        }
    }

    fn live_webrtc_enabled(&self) -> bool {
        #[cfg(feature = "live-webrtc")]
        {
            self.live_webrtc_state.is_some()
        }
        #[cfg(not(feature = "live-webrtc"))]
        {
            false
        }
    }

    /// Attach a live session factory for creating provider adapters on `live/open`.
    pub fn with_live_session_factory(
        mut self,
        factory: Arc<dyn meerkat_client::realtime_session::RealtimeSessionFactory>,
    ) -> Self {
        self.live_session_factory = Some(factory);
        self.sync_member_live_host();
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
            .map_err(|error| RpcResponse::from_error(None, error))?;
        if archived && !self.runtime.pending_session_exists(session_id).await {
            // Join the archived registration's teardown until terminal: the
            // plain unregister's caller grace answered a saga still completing
            // with `UnregisterInProgress`, reported here as an internal error
            // instead of the session's absence.
            self.runtime_adapter
                .unregister_current_session_registration_until_terminal(session_id)
                .await
                .map_err(|unregister_error| {
                    RpcResponse::error(
                        None,
                        error::INTERNAL_ERROR,
                        format!(
                            "failed to unregister archived runtime session {session_id}: {unregister_error}"
                        ),
                    )
                })?;
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

        self.runtime
            .ensure_runtime_executor(session_id)
            .await
            .map_err(|error| RpcResponse::from_error(None, error))
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
        runtime.set_mob_tools(Arc::new(meerkat_mob_mcp::AgentMobToolSurfaceFactory::new(
            mob_state.clone(),
        )));
        #[cfg(feature = "openai-live")]
        let experimental_live_delegation_coordinator =
            meerkat_mob_mcp::live_delegation::compose_experimental_live_delegation_coordinator(
                Arc::clone(&runtime_adapter),
                Arc::clone(&mob_state),
            );
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
            #[cfg(feature = "local-authorization")]
            governed_connection: None,
            runtime,
            config_store,
            notification_sink,
            skill_runtime: None,
            callback_route: None,
            active_session_streams: Arc::new(Mutex::new(HashMap::new())),
            stream_authority: Arc::new(Mutex::new(new_rpc_stream_authority())),
            mob_state,
            active_mob_streams: Arc::new(Mutex::new(HashMap::new())),
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
            #[cfg(feature = "live-webrtc")]
            live_webrtc_state: None,
            #[cfg(feature = "live-webrtc")]
            live_webrtc_answer_transport: None,
            #[cfg(feature = "openai-live")]
            experimental_live_open_authority: None,
            #[cfg(feature = "openai-live")]
            experimental_live_public_observation_publisher: None,
            #[cfg(all(feature = "openai-live", feature = "live-webrtc"))]
            experimental_live_playback_custodies: Arc::new(Mutex::new(HashMap::new())),
            #[cfg(feature = "openai-live")]
            experimental_live_delegation_coordinator,
            #[cfg(all(feature = "openai-live", feature = "live-webrtc"))]
            experimental_live_context_mirror_host: Arc::new(StdRwLock::new(None)),
            live_session_factory: None,
        }
    }

    /// Replace the default ephemeral runtime adapter with a custom one
    /// (e.g., persistent-backed for durable runtime semantics).
    pub fn with_runtime_adapter(mut self, adapter: Arc<meerkat_runtime::MeerkatMachine>) -> Self {
        self.runtime_adapter = Arc::clone(&adapter);
        #[cfg(all(feature = "openai-live", feature = "mob"))]
        {
            self.experimental_live_delegation_coordinator =
                meerkat_mob_mcp::live_delegation::compose_experimental_live_delegation_coordinator(
                    adapter,
                    Arc::clone(&self.mob_state),
                );
            #[cfg(feature = "live-webrtc")]
            {
                *self
                    .experimental_live_context_mirror_host
                    .write()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
            }
        }
        self.sync_member_live_host();
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

    /// Bind this router to its connection's callback route: callback tools
    /// registered and sessions created through it route to that connection.
    pub fn with_callback_route(mut self, route: crate::callback_dispatcher::CallbackRoute) -> Self {
        self.callback_route = Some(route);
        self
    }

    /// This connection's callback route, or the runtime's process-default
    /// route when the router owns none.
    pub fn callback_route(&self) -> Option<crate::callback_dispatcher::CallbackRoute> {
        self.callback_route
            .clone()
            .or_else(|| self.runtime.default_callback_route())
    }

    /// `mob/create`: the new mob's members call the callback tools of this
    /// connection's route, through a route slot this connection holds for
    /// that mob alone. A mob later created under the same id, or restored,
    /// gets a slot of its own and never reaches this connection's callbacks
    /// unless the connection claims it (`mob/claim_callback_route`).
    #[cfg(feature = "mob")]
    async fn handle_mob_create(
        &self,
        id: Option<crate::protocol::RpcId>,
        params: Option<&serde_json::value::RawValue>,
    ) -> RpcResponse {
        let slot = self.callback_route().map(|route| {
            Arc::new(crate::callback_dispatcher::MobCallbackRouteSlot::claimed_by(route))
        });
        let member_external_tools = slot.as_ref().map(|slot| {
            rpc_mob_slot_external_tools_provider(&self.runtime, Arc::clone(slot), None)
        });
        handlers::mob::handle_create_with_member_external_tools(
            id,
            params,
            &self.mob_state,
            member_external_tools,
            |mob_id| {
                if let Some(slot) = &slot {
                    self.runtime
                        .index_mob_callback_route_slot(mob_id.clone(), slot);
                }
            },
        )
        .await
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
            // Best-effort owner probe: a store error here is treated as "not
            // owned via this check" so the disjunction can fall through to the
            // other existence checks. The authoritative read is performed again
            // by the handler.
            || self
                .runtime
                .session_state(session_id)
                .await
                .map(|opt| opt.is_some())
                .unwrap_or(false)
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
            // A store-layer failure must surface as a typed error, not be
            // laundered into a "session not found" (K17).
            Err(err) => Some(mob_session_service_error_response(id, session_id, err)),
        }
    }

    /// Dispatch one native consent decision whose human event the embedding
    /// host's trusted transport adapter authenticated.
    ///
    /// This is the only entry to [`CONSENT_DECIDE_METHOD`]: plain dispatch
    /// has no route for it, so caller JSON, an audit actor string or a
    /// lifecycle request context can never decide native consent. The
    /// request names the approval and decision and must equal what the event
    /// binds; the event alone supplies the approver, its authentication
    /// scope and the reason. The decision is recorded by this runtime's
    /// approval service, which must be the service the host gave its
    /// operation review composition: only the issuing owner holds the
    /// candidate.
    pub async fn dispatch_with_consent_event(
        &self,
        request: RpcRequest,
        event: meerkat_core::approval::consent::AuthenticatedConsentEvent,
    ) -> RpcResponse {
        let id = request.id.clone();
        // The same envelope check as plain dispatch, before any handler: an
        // unsupported JSON-RPC version never reaches consent mutation.
        if !request.has_supported_version() {
            return RpcResponse::error(
                id,
                error::INVALID_REQUEST,
                format!(
                    "Unsupported JSON-RPC version: expected \"{}\", got \"{}\"",
                    crate::protocol::JSONRPC_VERSION,
                    request.jsonrpc
                ),
            );
        }
        if request.method != CONSENT_DECIDE_METHOD {
            return RpcResponse::error(
                id,
                error::METHOD_NOT_FOUND,
                format!("an authenticated consent event only decides {CONSENT_DECIDE_METHOD}"),
            );
        }
        handlers::approval::handle_consent_decide(
            id,
            request.params.as_deref(),
            self.runtime.clone(),
            event,
        )
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
        let mut routed =
            Box::pin(self.dispatch_routed_with_request_context(request, request_context)).await?;
        let response_id = routed.response.id.clone();
        if let Err(settlement_error) = routed.settle_delivery(true).await {
            return Some(RpcResponse::error(
                response_id,
                error::INTERNAL_ERROR,
                format!("RPC response delivery settlement failed: {settlement_error}"),
            ));
        }
        Some(routed.into_response())
    }

    /// Dispatch for a concrete transport connection. Unlike direct in-process
    /// dispatch, the returned delivery owner is not settled until the server's
    /// writer confirms that the response crossed the transport boundary.
    pub(crate) async fn dispatch_for_transport(
        &self,
        request: RpcRequest,
    ) -> Option<RoutedRpcResponse> {
        self.dispatch_routed_with_request_context(request, None)
            .await
    }

    #[allow(clippy::if_not_else)]
    async fn dispatch_routed_with_request_context(
        &self,
        request: RpcRequest,
        request_context: Option<RequestContext>,
    ) -> Option<RoutedRpcResponse> {
        // Validate the JSON-RPC envelope version at the boundary. The transport
        // carries `jsonrpc` as a free String; an envelope that does not declare
        // the supported "2.0" version is rejected here (typed via
        // `has_supported_version`) rather than dispatched. A notification (no
        // id) with a bad version gets no response per the JSON-RPC spec.
        if !request.has_supported_version() {
            if request.is_notification() {
                tracing::debug!(
                    jsonrpc = %request.jsonrpc,
                    method = %request.method,
                    "Dropping notification with unsupported JSON-RPC version"
                );
                return None;
            }
            return Some(RoutedRpcResponse::new(RpcResponse::error(
                request.id.clone(),
                error::INVALID_REQUEST,
                format!(
                    "Unsupported JSON-RPC version: expected \"{}\", got \"{}\"",
                    crate::protocol::JSONRPC_VERSION,
                    request.jsonrpc
                ),
            )));
        }

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

        #[cfg(feature = "local-authorization")]
        if let Some(connection) = &self.governed_connection {
            return Some(RoutedRpcResponse::new(
                crate::governed_jsonl::dispatch(
                    connection,
                    &request.method,
                    id,
                    params,
                    &self.runtime,
                    &self.notification_sink,
                    self.callback_route.clone(),
                    request_context,
                )
                .await,
            ));
        }
        // A legacy constructor is not a process-authenticated governed entry.
        // Refuse before any setup/cold/tool/config handler sees the request.
        if request.method != "initialize"
            && self.runtime_adapter.has_native_work_authorization_host()
        {
            return Some(RoutedRpcResponse::new(RpcResponse::from_error(
                id,
                crate::session_runtime::runtime_driver_error_to_rpc(
                    meerkat_runtime::RuntimeDriverError::ControllerReadinessUnavailable {
                        reason:
                            meerkat_runtime::traits::ControllerReadinessFailure::UnsupportedScope,
                    },
                ),
            )));
        }

        let response = match request.method.as_str() {
            "initialize" => handlers::initialize::handle_initialize(
                id,
                // Runtime-backed only: every session runs the v9 runtime.
                true,
                self.live_enabled(),
                self.live_webrtc_enabled(),
                self.skill_runtime.is_some(),
            ),
            "help/ask" => {
                routed_arm(|| {
                    handlers::help::handle_ask(
                        id,
                        params,
                        self.runtime.clone(),
                        &self.notification_sink,
                        &self.runtime_adapter,
                        request_context.clone(),
                    )
                })
                .await
            }
            "session/create" => {
                routed_arm(|| {
                    handlers::session::handle_create_on_route(
                        id,
                        params,
                        self.runtime.clone(),
                        &self.notification_sink,
                        &self.runtime_adapter,
                        request_context.clone(),
                        self.callback_route(),
                    )
                })
                .await
            }
            "session/list" => {
                routed_arm(|| handlers::session::handle_list(id, params, &self.runtime)).await
            }
            "session/read" => routed_arm(|| self.handle_session_read(id, params)).await,
            "session/history" => routed_arm(|| self.handle_session_history(id, params)).await,
            "session/export_atif" => {
                routed_arm(|| handlers::session::handle_export_atif(id, params, &self.runtime))
                    .await
            }
            "session/transcript_revision" => {
                routed_arm(|| self.handle_session_transcript_revision(id, params)).await
            }
            "session/transcript_revisions" => {
                routed_arm(|| self.handle_session_transcript_revisions(id, params)).await
            }
            "session/rewrite_transcript" => {
                routed_arm(|| self.handle_session_rewrite_transcript(id, params)).await
            }
            "session/update_system_prompt" => {
                routed_arm(|| self.handle_session_update_system_prompt(id, params)).await
            }
            "session/activate_instruction" => {
                routed_arm(|| self.handle_session_activate_instruction(id, params)).await
            }
            "session/instruction_activations" => {
                routed_arm(|| self.handle_session_instruction_activations(id, params)).await
            }
            "session/restore_transcript_revision" => {
                routed_arm(|| self.handle_session_restore_transcript_revision(id, params)).await
            }
            "session/fork_at" => routed_arm(|| self.handle_session_fork_at(id, params)).await,
            "session/fork_replace" => {
                routed_arm(|| self.handle_session_fork_replace(id, params)).await
            }
            "blob/get" => routed_arm(|| self.handle_blob_get(id, params)).await,
            "artifact/list" => routed_arm(|| self.handle_artifact_list(id, params)).await,
            "artifact/get" => routed_arm(|| self.handle_artifact_get(id, params)).await,
            "artifact/download" => routed_arm(|| self.handle_artifact_download(id, params)).await,
            "session/archive" => routed_arm(|| self.handle_session_archive(id, params)).await,
            "session/external_event" => {
                routed_arm(|| {
                    handlers::event::handle_external_event(id, params, self.runtime.clone())
                })
                .await
            }
            "session/peer_response_terminal" => {
                routed_arm(|| {
                    handlers::event::handle_peer_response_terminal(id, params, self.runtime.clone())
                })
                .await
            }
            "events/latest_cursor" => {
                routed_arm(|| {
                    handlers::event::handle_events_latest_cursor(id, params, self.runtime.clone())
                })
                .await
            }
            "events/list_since" => {
                routed_arm(|| {
                    handlers::event::handle_events_list_since(id, params, self.runtime.clone())
                })
                .await
            }
            "events/snapshot" => {
                routed_arm(|| {
                    handlers::event::handle_events_snapshot(id, params, self.runtime.clone())
                })
                .await
            }
            "session/inject_context" => {
                routed_arm(|| self.handle_session_inject_context(id, params)).await
            }
            "session/input_status" => {
                routed_arm(|| self.handle_session_input_state(id, params)).await
            }
            "session/stream_open" => {
                routed_arm(|| self.handle_session_stream_open(id, params)).await
            }
            "session/stream_close" => {
                routed_arm(|| self.handle_session_stream_close(id, params)).await
            }
            "jobs/get" => {
                routed_arm(|| handlers::jobs::handle_get(id, params, &self.runtime)).await
            }
            "jobs/list" => {
                routed_arm(|| handlers::jobs::handle_list(id, params, &self.runtime)).await
            }
            "jobs/cancel" => {
                let callback_route = self.callback_route();
                routed_arm(|| {
                    handlers::jobs::handle_cancel_on_route(
                        id,
                        params,
                        &self.runtime,
                        callback_route.as_ref(),
                    )
                })
                .await
            }
            "jobs/progress" => {
                routed_arm(|| handlers::jobs::handle_get_progress(id, params, &self.runtime)).await
            }
            "jobs/result" => {
                routed_arm(|| handlers::jobs::handle_result(id, params, &self.runtime)).await
            }
            "jobs/artifacts" => {
                routed_arm(|| handlers::jobs::handle_artifacts(id, params, &self.runtime)).await
            }
            "jobs/retry" => {
                let callback_route = self.callback_route();
                routed_arm(|| {
                    handlers::jobs::handle_retry_on_route(
                        id,
                        params,
                        &self.runtime,
                        callback_route.as_ref(),
                    )
                })
                .await
            }
            "jobs/health" => routed_arm(|| handlers::jobs::handle_health(id, &self.runtime)).await,
            "monitors/start" => {
                routed_arm(|| handlers::jobs::handle_monitor_start(id, params, &self.runtime)).await
            }
            "jobs/subscribe" => {
                routed_arm(|| handlers::jobs::handle_subscribe(id, params, &self.runtime)).await
            }
            "jobs/unsubscribe" => {
                routed_arm(|| handlers::jobs::handle_unsubscribe(id, params, &self.runtime)).await
            }
            "meta/jobs/heartbeat" => {
                routed_arm(|| handlers::jobs::handle_heartbeat(id, params, &self.runtime)).await
            }
            "meta/jobs/progress" => {
                routed_arm(|| handlers::jobs::handle_progress(id, params, &self.runtime)).await
            }
            "meta/jobs/checkpoint" => {
                routed_arm(|| handlers::jobs::handle_checkpoint(id, params, &self.runtime)).await
            }
            "meta/jobs/complete" => {
                routed_arm(|| handlers::jobs::handle_complete(id, params, &self.runtime)).await
            }
            "meta/jobs/fail" => {
                routed_arm(|| handlers::jobs::handle_fail(id, params, &self.runtime)).await
            }
            "meta/jobs/cancel_ack" => {
                routed_arm(|| handlers::jobs::handle_cancel_ack(id, params, &self.runtime)).await
            }
            "schedule/create" => {
                routed_arm(|| handlers::schedule::handle_create(id, params, self.runtime.clone()))
                    .await
            }
            "schedule/get" => {
                routed_arm(|| handlers::schedule::handle_get(id, params, self.runtime.clone()))
                    .await
            }
            "schedule/list" => {
                routed_arm(|| handlers::schedule::handle_list(id, params, self.runtime.clone()))
                    .await
            }
            "schedule/update" => {
                routed_arm(|| handlers::schedule::handle_update(id, params, self.runtime.clone()))
                    .await
            }
            "schedule/pause" => {
                routed_arm(|| handlers::schedule::handle_pause(id, params, self.runtime.clone()))
                    .await
            }
            "schedule/resume" => {
                routed_arm(|| handlers::schedule::handle_resume(id, params, self.runtime.clone()))
                    .await
            }
            "schedule/delete" => {
                routed_arm(|| handlers::schedule::handle_delete(id, params, self.runtime.clone()))
                    .await
            }
            "schedule/occurrences" => {
                routed_arm(|| {
                    handlers::schedule::handle_occurrences(id, params, self.runtime.clone())
                })
                .await
            }
            "schedule/tools" => routed_arm(|| handlers::schedule::handle_tools(id)).await,
            "schedule/call" => {
                routed_arm(|| handlers::schedule::handle_call(id, params, self.runtime.clone()))
                    .await
            }
            "workgraph/get" => {
                routed_arm(|| handlers::workgraph::handle_get(id, params, self.runtime.clone()))
                    .await
            }
            "workgraph/goal/status" => {
                routed_arm(|| {
                    handlers::workgraph::handle_goal_status(id, params, self.runtime.clone())
                })
                .await
            }
            "workgraph/attention/list" => {
                routed_arm(|| {
                    handlers::workgraph::handle_attention_list(id, params, self.runtime.clone())
                })
                .await
            }
            "workgraph/list" => {
                routed_arm(|| handlers::workgraph::handle_list(id, params, self.runtime.clone()))
                    .await
            }
            "workgraph/ready" => {
                routed_arm(|| handlers::workgraph::handle_ready(id, params, self.runtime.clone()))
                    .await
            }
            "workgraph/snapshot" => {
                routed_arm(|| {
                    handlers::workgraph::handle_snapshot(id, params, self.runtime.clone())
                })
                .await
            }
            "workgraph/events" => {
                routed_arm(|| handlers::workgraph::handle_events(id, params, self.runtime.clone()))
                    .await
            }
            "turn/start" => {
                routed_arm(|| {
                    handlers::turn::handle_start(
                        id,
                        params,
                        self.runtime.clone(),
                        &self.notification_sink,
                        &self.runtime_adapter,
                        request_context.clone(),
                    )
                })
                .await
            }
            "turn/stop_run" => {
                routed_arm(|| handlers::turn::handle_stop_run(id, params, &self.runtime)).await
            }
            "turn/interrupt" => {
                #[cfg(feature = "mob")]
                {
                    routed_arm(|| {
                        handlers::turn::handle_interrupt(id, params, &self.runtime, &self.mob_state)
                    })
                    .await
                }
                #[cfg(not(feature = "mob"))]
                {
                    routed_arm(|| handlers::turn::handle_interrupt(id, params, &self.runtime)).await
                }
            }
            #[cfg(feature = "mob")]
            "mob/create" => routed_arm(|| self.handle_mob_create(id, params)).await,
            #[cfg(feature = "mob")]
            "mob/list" => routed_arm(|| handlers::mob::handle_list(id, &self.mob_state)).await,
            #[cfg(feature = "mob")]
            "mob/status" => {
                routed_arm(|| handlers::mob::handle_status(id, params, &self.mob_state)).await
            }
            #[cfg(feature = "mob")]
            "mob/lifecycle" => {
                routed_arm(|| handlers::mob::handle_lifecycle(id, params, &self.mob_state)).await
            }
            #[cfg(feature = "mob")]
            "mob/spawn" => {
                routed_arm(|| handlers::mob::handle_spawn(id, params, &self.mob_state)).await
            }
            #[cfg(feature = "mob")]
            "mob/spawn_many" => {
                routed_arm(|| handlers::mob::handle_spawn_many(id, params, &self.mob_state)).await
            }
            #[cfg(feature = "mob")]
            "mob/ensure_member" => {
                routed_arm(|| handlers::mob::handle_ensure_member(id, params, &self.mob_state))
                    .await
            }
            #[cfg(feature = "mob")]
            "mob/reconcile" => {
                routed_arm(|| handlers::mob::handle_reconcile(id, params, &self.mob_state)).await
            }
            #[cfg(feature = "mob")]
            "mob/list_members_matching" => {
                routed_arm(|| {
                    handlers::mob::handle_list_members_matching(id, params, &self.mob_state)
                })
                .await
            }
            #[cfg(feature = "mob")]
            "mob/members" => {
                routed_arm(|| handlers::mob::handle_members(id, params, &self.mob_state)).await
            }
            #[cfg(feature = "mob")]
            "mob/member_tool_declaration" => {
                routed_arm(|| {
                    handlers::mob::handle_member_tool_declaration(id, params, &self.mob_state)
                })
                .await
            }
            #[cfg(feature = "mob")]
            "mob/apply_member_tool_declaration" => {
                routed_arm(|| {
                    handlers::mob::handle_apply_member_tool_declaration(id, params, &self.mob_state)
                })
                .await
            }
            #[cfg(feature = "mob")]
            "mob/adopt_member_identity_declaration" => {
                routed_arm(|| {
                    handlers::mob::handle_adopt_member_identity_declaration(
                        id,
                        params,
                        &self.mob_state,
                    )
                })
                .await
            }
            #[cfg(feature = "mob")]
            "mob/resolve_identity_convergence_block" => {
                routed_arm(|| {
                    handlers::mob::handle_resolve_identity_convergence_block(
                        id,
                        params,
                        &self.mob_state,
                    )
                })
                .await
            }
            #[cfg(feature = "mob")]
            "mob/retire" => {
                routed_arm(|| handlers::mob::handle_retire(id, params, &self.mob_state)).await
            }
            #[cfg(feature = "mob")]
            "mob/respawn" => {
                routed_arm(|| handlers::mob::handle_respawn(id, params, &self.mob_state)).await
            }
            #[cfg(feature = "mob")]
            "mob/wire" => {
                routed_arm(|| handlers::mob::handle_wire(id, params, &self.mob_state)).await
            }
            #[cfg(feature = "mob")]
            "mob/wire_members_batch" => {
                routed_arm(|| handlers::mob::handle_wire_members_batch(id, params, &self.mob_state))
                    .await
            }
            #[cfg(feature = "mob")]
            "mob/unwire" => {
                routed_arm(|| handlers::mob::handle_unwire(id, params, &self.mob_state)).await
            }
            #[cfg(feature = "mob")]
            "mob/events" => {
                routed_arm(|| handlers::mob::handle_events(id, params, &self.mob_state)).await
            }
            #[cfg(feature = "mob")]
            "mob/turn_start" => {
                routed_arm(|| {
                    handlers::mob::handle_mob_turn_start(
                        id,
                        params,
                        &self.mob_state,
                        self.runtime.clone(),
                        &self.notification_sink,
                        &self.runtime_adapter,
                        request_context.clone(),
                    )
                })
                .await
            }
            #[cfg(feature = "mob")]
            "mob/member_send" => {
                routed_arm(|| handlers::mob::handle_member_send(id, params, &self.mob_state)).await
            }
            #[cfg(feature = "mob")]
            "mob/ingress_interaction" => {
                routed_arm(|| {
                    handlers::mob::handle_ingress_interaction(id, params, &self.mob_state)
                })
                .await
            }
            #[cfg(feature = "mob")]
            "mob/append_system_context" => {
                routed_arm(|| {
                    handlers::mob::handle_append_system_context(
                        id,
                        params,
                        &self.mob_state,
                        &self.runtime,
                    )
                })
                .await
            }
            #[cfg(feature = "mob")]
            "mob/flows" => {
                routed_arm(|| handlers::mob::handle_flows(id, params, &self.mob_state)).await
            }
            #[cfg(feature = "mob")]
            "mob/flow_run" => {
                routed_arm(|| handlers::mob::handle_flow_run(id, params, &self.mob_state)).await
            }
            #[cfg(feature = "mob")]
            "mob/run" => {
                routed_arm(|| handlers::mob::handle_run(id, params, &self.mob_state)).await
            }
            #[cfg(feature = "mob")]
            "mob/flow_status" => {
                routed_arm(|| handlers::mob::handle_flow_status(id, params, &self.mob_state)).await
            }
            #[cfg(feature = "mob")]
            "mob/run_result" => {
                routed_arm(|| handlers::mob::handle_run_result(id, params, &self.mob_state)).await
            }
            #[cfg(feature = "mob")]
            "mob/flow_cancel" => {
                routed_arm(|| handlers::mob::handle_flow_cancel(id, params, &self.mob_state)).await
            }
            #[cfg(feature = "mob")]
            "mob/spawn_helper" => {
                routed_arm(|| handlers::mob::handle_spawn_helper(id, params, &self.mob_state)).await
            }
            #[cfg(feature = "mob")]
            "mob/fork_helper" => {
                routed_arm(|| handlers::mob::handle_fork_helper(id, params, &self.mob_state)).await
            }
            #[cfg(feature = "mob")]
            "mob/force_cancel" => {
                routed_arm(|| handlers::mob::handle_force_cancel(id, params, &self.mob_state)).await
            }
            #[cfg(feature = "mob")]
            "mob/member_status" => {
                routed_arm(|| {
                    handlers::mob::handle_member_status(id, params, &self.mob_state, &self.runtime)
                })
                .await
            }
            #[cfg(feature = "mob")]
            "mob/snapshot" => {
                routed_arm(|| handlers::mob::handle_snapshot(id, params, &self.mob_state)).await
            }
            #[cfg(feature = "mob")]
            "mob/destroy" => {
                routed_arm(|| handlers::mob::handle_destroy(id, params, &self.mob_state)).await
            }
            #[cfg(feature = "mob")]
            "mob/rotate_supervisor" => {
                routed_arm(|| handlers::mob::handle_rotate_supervisor(id, params, &self.mob_state))
                    .await
            }
            #[cfg(feature = "mob")]
            "mob/claim_callback_route" => {
                routed_arm(|| {
                    handlers::mob::handle_claim_callback_route(
                        id,
                        params,
                        &self.mob_state,
                        &self.runtime,
                        self.callback_route(),
                    )
                })
                .await
            }
            #[cfg(feature = "mob")]
            "mob/submit_work" => {
                routed_arm(|| handlers::mob::handle_submit_work(id, params, &self.mob_state)).await
            }
            #[cfg(feature = "mob")]
            "mob/conclude_objective" => {
                routed_arm(|| handlers::mob::handle_conclude_objective(id, params, &self.mob_state))
                    .await
            }
            #[cfg(feature = "mob")]
            "mob/cancel_work" => {
                routed_arm(|| handlers::mob::handle_cancel_work(id, params, &self.mob_state)).await
            }
            #[cfg(feature = "mob")]
            "mob/cancel_all_work" => {
                routed_arm(|| handlers::mob::handle_cancel_all_work(id, params, &self.mob_state))
                    .await
            }
            #[cfg(feature = "mob")]
            "mob/wait_kickoff" => {
                routed_arm(|| handlers::mob::handle_wait_kickoff(id, params, &self.mob_state)).await
            }
            #[cfg(feature = "mob")]
            "mob/wait_ready" => {
                routed_arm(|| handlers::mob::handle_wait_ready(id, params, &self.mob_state)).await
            }
            #[cfg(feature = "mob")]
            "mob/profile/create" => {
                routed_arm(|| handlers::mob::handle_profile_create(id, params, &self.mob_state))
                    .await
            }
            #[cfg(feature = "mob")]
            "mob/profile/get" => {
                routed_arm(|| handlers::mob::handle_profile_get(id, params, &self.mob_state)).await
            }
            #[cfg(feature = "mob")]
            "mob/profile/list" => {
                routed_arm(|| handlers::mob::handle_profile_list(id, &self.mob_state)).await
            }
            #[cfg(feature = "mob")]
            "mob/profile/update" => {
                routed_arm(|| handlers::mob::handle_profile_update(id, params, &self.mob_state))
                    .await
            }
            #[cfg(feature = "mob")]
            "mob/profile/delete" => {
                routed_arm(|| handlers::mob::handle_profile_delete(id, params, &self.mob_state))
                    .await
            }
            #[cfg(feature = "mob")]
            "mob/stream_open" => routed_arm(|| self.handle_mob_stream_open(id, params)).await,
            #[cfg(feature = "mob")]
            "mob/stream_close" => routed_arm(|| self.handle_mob_stream_close(id, params)).await,
            #[cfg(feature = "mob")]
            "mob/grant_scopes" => {
                routed_arm(|| handlers::mob::handle_grant_scopes(id, params, &self.mob_state)).await
            }
            #[cfg(feature = "mob")]
            "mob/revoke_scopes" => {
                routed_arm(|| handlers::mob::handle_revoke_scopes(id, params, &self.mob_state))
                    .await
            }
            #[cfg(feature = "mob")]
            "mob/grants" => {
                routed_arm(|| handlers::mob::handle_grants(id, params, &self.mob_state)).await
            }
            #[cfg(feature = "mob")]
            "mob/member_history" => {
                routed_arm(|| handlers::mob::handle_member_history(id, params, &self.mob_state))
                    .await
            }
            #[cfg(feature = "mob")]
            "mob/hosts" => {
                routed_arm(|| handlers::mob::handle_hosts(id, params, &self.mob_state)).await
            }
            #[cfg(feature = "mob")]
            "mob/route_installs" => {
                routed_arm(|| handlers::mob::handle_route_installs(id, params, &self.mob_state))
                    .await
            }
            #[cfg(feature = "mob")]
            "mob/bind_host" => {
                routed_arm(|| handlers::mob::handle_bind_host(id, params, &self.mob_state)).await
            }
            #[cfg(feature = "mob")]
            "mob/revoke_host" => {
                routed_arm(|| handlers::mob::handle_revoke_host(id, params, &self.mob_state)).await
            }
            #[cfg(feature = "mob")]
            "mob/hard_cancel_member" => {
                routed_arm(|| handlers::mob::handle_hard_cancel_member(id, params, &self.mob_state))
                    .await
            }
            #[cfg(feature = "mob")]
            "mob/stop_member_run" => {
                routed_arm(|| handlers::mob::handle_stop_member_run(id, params, &self.mob_state))
                    .await
            }
            #[cfg(feature = "mob")]
            "mob/member_live_open" => {
                routed_arm(|| handlers::mob::handle_member_live_open(id, params, &self.mob_state))
                    .await
            }
            #[cfg(feature = "mob")]
            "mob/member_live_close" => {
                routed_arm(|| handlers::mob::handle_member_live_close(id, params, &self.mob_state))
                    .await
            }
            #[cfg(feature = "mob")]
            "mob/member_live_status" => {
                routed_arm(|| handlers::mob::handle_member_live_status(id, params, &self.mob_state))
                    .await
            }
            #[cfg(feature = "mob")]
            "mob/member_live_control" => {
                routed_arm(|| {
                    handlers::mob::handle_member_live_control(id, params, &self.mob_state)
                })
                .await
            }
            #[cfg(feature = "comms")]
            "comms/send" => routed_arm(|| self.handle_comms_send(id, params)).await,
            #[cfg(feature = "comms")]
            "comms/peers" => routed_arm(|| self.handle_comms_peers(id, params)).await,
            "skills/list" => {
                routed_arm(|| handlers::skills::handle_list(id, &self.skill_runtime)).await
            }
            "tools/register" => routed_arm(|| self.handle_tools_register(id, params)).await,
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
            // Compose the realm chain so a capability gated on an inherited config
            // field reports the same status as the other surfaces.
            "capabilities/get" => match routed_arm(|| self.runtime.effective_config()).await {
                Ok(config) => handlers::capabilities::handle_get(id, &config),
                Err(e) => RpcResponse::from_error(id, e),
            },
            "runtime/host_info" => {
                routed_arm(|| {
                    handlers::runtime_host::handle_info(
                        id,
                        &self.runtime,
                        &self.config_store,
                        // Runtime-backed only: every session runs the v9 runtime.
                        true,
                        self.live_enabled(),
                        self.live_webrtc_enabled(),
                        self.skill_runtime.is_some(),
                    )
                })
                .await
            }
            "runtime/capabilities" => handlers::runtime_host::handle_capabilities(
                id,
                &self.runtime,
                // Runtime-backed only: every session runs the v9 runtime.
                true,
                self.live_enabled(),
                self.live_webrtc_enabled(),
                self.skill_runtime.is_some(),
            ),
            "runtime/health" => {
                routed_arm(|| handlers::runtime_host::handle_health(id, &self.runtime)).await
            }
            "approval/request" => {
                routed_arm(|| handlers::approval::handle_request(id, params, self.runtime.clone()))
                    .await
            }
            "approval/list" => {
                routed_arm(|| handlers::approval::handle_list(id, params, self.runtime.clone()))
                    .await
            }
            "approval/get" => {
                routed_arm(|| handlers::approval::handle_get(id, params, self.runtime.clone()))
                    .await
            }
            "approval/decide" => {
                routed_arm(|| handlers::approval::handle_decide(id, params, self.runtime.clone()))
                    .await
            }
            // Compose the realm chain so an inherited (ancestor-realm) self-hosted
            // alias / per-provider default is listed identically to the other
            // surfaces and matches what a session build resolves.
            "models/catalog" => match routed_arm(|| self.runtime.effective_config()).await {
                Ok(config) => handlers::models::handle_catalog(id, &config),
                Err(e) => RpcResponse::from_error(id, e),
            },
            // Auth + realm methods (Phase 4d).
            "auth/profile/list" => {
                routed_arm(|| handlers::auth::handle_auth_profile_list(id, params, &self.runtime))
                    .await
            }
            "auth/profile/get" => {
                routed_arm(|| handlers::auth::handle_auth_profile_get(id, params, &self.runtime))
                    .await
            }
            "auth/profile/create" => {
                routed_arm(|| handlers::auth::handle_auth_profile_create(id, params, &self.runtime))
                    .await
            }
            "auth/profile/delete" => {
                routed_arm(|| handlers::auth::handle_auth_profile_delete(id, params, &self.runtime))
                    .await
            }
            "auth/login/start" => {
                routed_arm(|| handlers::auth::handle_auth_login_start(id, params, &self.runtime))
                    .await
            }
            "auth/login/complete" => {
                routed_arm(|| handlers::auth::handle_auth_login_complete(id, params, &self.runtime))
                    .await
            }
            "auth/login/cancel" => {
                routed_arm(|| handlers::auth::handle_auth_login_cancel(id, params, &self.runtime))
                    .await
            }
            "auth/login/device_start" => {
                routed_arm(|| {
                    handlers::auth::handle_auth_login_device_start(id, params, &self.runtime)
                })
                .await
            }
            "auth/login/device_complete" => {
                routed_arm(|| {
                    handlers::auth::handle_auth_login_device_complete(id, params, &self.runtime)
                })
                .await
            }
            "auth/login/provision_api_key" => {
                routed_arm(|| {
                    handlers::auth::handle_auth_login_provision_api_key(id, params, &self.runtime)
                })
                .await
            }
            "auth/status/get" => {
                routed_arm(|| handlers::auth::handle_auth_status_get(id, params, &self.runtime))
                    .await
            }
            "auth/logout" => {
                routed_arm(|| handlers::auth::handle_auth_logout(id, params, &self.runtime)).await
            }
            "realm/list" => {
                routed_arm(|| handlers::auth::handle_realm_list(id, &self.runtime)).await
            }
            "realm/get" => {
                routed_arm(|| handlers::auth::handle_realm_get(id, params, &self.runtime)).await
            }
            "config/get" => {
                routed_arm(|| {
                    handlers::config::handle_get(
                        id,
                        &self.config_store,
                        self.runtime.config_runtime(),
                    )
                })
                .await
            }
            "config/set" => {
                routed_arm(|| {
                    handlers::config::handle_set(
                        id,
                        params,
                        &self.runtime,
                        &self.config_store,
                        self.runtime.config_runtime(),
                    )
                })
                .await
            }
            "config/patch" => {
                routed_arm(|| {
                    handlers::config::handle_patch(
                        id,
                        params,
                        &self.runtime,
                        &self.config_store,
                        self.runtime.config_runtime(),
                    )
                })
                .await
            }
            // live/* is registered when at least one live transport is
            // configured. The handler owns transport selection; provider
            // setup remains shared.
            "live/open" if self.live_enabled() => {
                let result = routed_arm(|| {
                    handlers::live::handle_live_open_routed(
                        id,
                        params,
                        handlers::live::LiveOpenHandlerContext {
                            host: &self.live_adapter_host,
                            live_ws: self.live_ws_state.as_deref(),
                            live_ws_base_url: self.live_ws_base_url.as_deref(),
                            #[cfg(feature = "live-webrtc")]
                            live_webrtc: self.live_webrtc_state.as_deref(),
                            runtime: &self.runtime,
                            session_factory: self.live_session_factory.as_ref().map(Arc::as_ref),
                            #[cfg(feature = "openai-live")]
                            experimental_live_open_authority: self
                                .experimental_live_open_authority
                                .as_ref(),
                            #[cfg(all(feature = "openai-live", feature = "live-webrtc"))]
                            experimental_live_playback_custodies: &self
                                .experimental_live_playback_custodies,
                        },
                    )
                })
                .await;
                #[cfg(feature = "openai-live")]
                {
                    let handlers::live::LiveOpenHandlerResult {
                        response,
                        experimental_publication,
                    } = result;
                    let custody = experimental_publication.map(|publication| {
                        ExperimentalLiveOpenDeliveryCustody::new(
                            Arc::clone(&self.runtime),
                            Arc::clone(&self.live_adapter_host),
                            publication,
                            #[cfg(feature = "live-webrtc")]
                            Arc::clone(&self.experimental_live_playback_custodies),
                        )
                    });
                    return Some(RoutedRpcResponse::with_experimental_live_open(
                        response, custody,
                    ));
                }
                #[cfg(not(feature = "openai-live"))]
                result.response
            }
            #[cfg(feature = "live-webrtc")]
            "live/webrtc/answer" if self.live_webrtc_answer_transport.is_some() => {
                if let Some(answer_transport) = self.live_webrtc_answer_transport.as_ref() {
                    return Some(RoutedRpcResponse::with_live_webrtc_answer(
                        routed_arm(|| {
                            handlers::live::handle_live_webrtc_answer(
                                id,
                                params,
                                answer_transport,
                                &self.runtime,
                                #[cfg(feature = "openai-live")]
                                self.experimental_live_open_authority.as_deref(),
                                #[cfg(feature = "openai-live")]
                                self.experimental_live_bound_channel_activator(),
                                #[cfg(feature = "openai-live")]
                                self.experimental_live_public_observation_publisher.clone(),
                                #[cfg(feature = "openai-live")]
                                &self.live_adapter_host,
                                #[cfg(feature = "openai-live")]
                                &self.experimental_live_playback_custodies,
                            )
                        })
                        .await,
                    ));
                }
                RpcResponse::error(
                    id,
                    error::INTERNAL_ERROR,
                    "live/webrtc/answer reached handler without WebRTC state".to_string(),
                )
            }
            "live/status" if self.live_enabled() => {
                routed_arm(|| {
                    handlers::live::handle_live_status(
                        id,
                        params,
                        &self.live_adapter_host,
                        &self.runtime,
                    )
                })
                .await
            }
            #[cfg(all(feature = "live-webrtc", feature = "openai-live"))]
            "live/media_health" if self.live_webrtc_answer_transport.is_some() => {
                routed_arm(|| {
                    handlers::live::handle_live_media_health(
                        id,
                        params,
                        &self.live_adapter_host,
                        &self.runtime,
                        self.experimental_live_open_authority.as_deref(),
                    )
                })
                .await
            }
            "live/close" if self.live_enabled() => {
                routed_arm(|| {
                    handlers::live::handle_live_close(
                        id,
                        params,
                        &self.live_adapter_host,
                        &self.runtime,
                        #[cfg(feature = "live-webrtc")]
                        self.live_webrtc_answer_transport.as_deref(),
                        #[cfg(feature = "openai-live")]
                        self.experimental_live_open_authority.as_deref(),
                    )
                })
                .await
            }
            // P1#5: push a fresh projection snapshot into an already-open
            // live adapter (model switch via `config/patch`, snapshot drift
            // after a session edit, etc.). Same gating as the other live/*
            // arms — without a live transport the router has no live state.
            "live/refresh" if self.live_enabled() => {
                routed_arm(|| {
                    handlers::live::handle_live_refresh(
                        id,
                        params,
                        &self.live_adapter_host,
                        &self.runtime,
                    )
                })
                .await
            }
            "live/send_input" if self.live_enabled() => {
                routed_arm(|| {
                    handlers::live::handle_live_send_input(
                        id,
                        params,
                        &self.live_adapter_host,
                        &self.runtime,
                    )
                })
                .await
            }
            // I50: surface the buffered-input commit verb. Same gating as the
            // other live/* arms — without --live-ws the router has no
            // transport state and the method falls through to METHOD_NOT_FOUND.
            "live/commit_input" if self.live_enabled() => {
                routed_arm(|| {
                    handlers::live::handle_live_commit_input(
                        id,
                        params,
                        &self.live_adapter_host,
                        &self.runtime,
                    )
                })
                .await
            }
            // A7: explicit barge-in surface. Without these arms callers can
            // only rely on provider-native VAD; with them, a client can
            // signal interrupt directly and truncate an assistant item at a
            // specific playback cursor.
            "live/interrupt" if self.live_enabled() => {
                #[cfg(feature = "live-webrtc")]
                {
                    routed_arm(|| {
                        handlers::live::handle_live_interrupt(
                            id,
                            params,
                            &self.live_adapter_host,
                            &self.runtime,
                            self.live_webrtc_state.as_deref(),
                        )
                    })
                    .await
                }
                #[cfg(not(feature = "live-webrtc"))]
                {
                    routed_arm(|| {
                        handlers::live::handle_live_interrupt(
                            id,
                            params,
                            &self.live_adapter_host,
                            &self.runtime,
                        )
                    })
                    .await
                }
            }
            "live/truncate" if self.live_enabled() => {
                #[cfg(feature = "live-webrtc")]
                {
                    routed_arm(|| {
                        handlers::live::handle_live_truncate(
                            id,
                            params,
                            &self.live_adapter_host,
                            &self.runtime,
                            self.live_webrtc_state.as_deref(),
                        )
                    })
                    .await
                }
                #[cfg(not(feature = "live-webrtc"))]
                {
                    routed_arm(|| {
                        handlers::live::handle_live_truncate(
                            id,
                            params,
                            &self.live_adapter_host,
                            &self.runtime,
                        )
                    })
                    .await
                }
            }
            "live/playback_complete" if self.live_enabled() => {
                routed_arm(|| {
                    handlers::live::handle_live_playback_complete(
                        id,
                        params,
                        &self.live_adapter_host,
                        &self.runtime,
                    )
                })
                .await
            }
            // A7: no `live/playback_cursor` arm — playback is a client-side
            // fact (jitter buffers, end-of-stream silence trim). Clients
            // track the cursor locally and pass `audio_played_ms` into
            // `live/truncate`. See the doc-comment in `handlers/live.rs`.
            "mcp/add" => routed_arm(|| handlers::mcp::handle_add(id, params, &self.runtime)).await,
            "mcp/remove" => {
                routed_arm(|| handlers::mcp::handle_remove(id, params, &self.runtime)).await
            }
            "mcp/reload" => {
                routed_arm(|| handlers::mcp::handle_reload(id, params, &self.runtime)).await
            }
            _ => RpcResponse::error(
                id,
                error::METHOD_NOT_FOUND,
                format!("Method not found: {}", request.method),
            ),
        };

        Some(RoutedRpcResponse::new(response))
    }

    /// Access the underlying session runtime.
    pub fn runtime(&self) -> &SessionRuntime {
        &self.runtime
    }

    async fn handle_tools_register(
        &self,
        id: Option<crate::protocol::RpcId>,
        params: Option<&serde_json::value::RawValue>,
    ) -> RpcResponse {
        let params: ToolsRegisterParams = match handlers::parse_params(params) {
            Ok(p) => p,
            Err(resp) => {
                return resp.with_id(id);
            }
        };

        let replacements: Result<Vec<_>, String> = params
            .tools
            .into_iter()
            .map(|mut new_tool| {
                let execution = crate::callback_dispatcher::execution_contract_from_wire(
                    new_tool.execution.take(),
                )?;
                let tool = meerkat_core::ToolDef {
                    provenance: Some(meerkat_core::types::ToolProvenance {
                        kind: meerkat_core::types::ToolSourceKind::Callback,
                        source_id: "callback".into(),
                        artifact: None,
                    }),
                    ..new_tool.into()
                };
                Ok((tool, execution))
            })
            .collect();
        let replacements = match replacements {
            Ok(replacements) => replacements,
            Err(error_message) => {
                return RpcResponse::error(
                    id,
                    error::INVALID_PARAMS,
                    format!("Invalid callback execution contract: {error_message}"),
                );
            }
        };
        // Registration mutates this connection's own registry only.
        let callback_route = self.callback_route();
        let registry = callback_route
            .as_ref()
            .map(crate::callback_dispatcher::CallbackRoute::registry)
            .unwrap_or_else(|| self.runtime.callback_tool_registry());
        match registry.replace_or_add_with_contracts(replacements) {
            Ok(count) => {
                if let Some(dispatcher) = callback_route.as_ref().map(|route| {
                    self.runtime
                        .callback_tool_dispatcher_for_route(route, vec![])
                }) {
                    tokio::spawn(async move {
                        if let Err(error) = dispatcher.reconcile_detached_jobs().await {
                            tracing::warn!(
                                %error,
                                "detached callback reconciliation did not complete"
                            );
                        }
                    });
                }
                RpcResponse::success(id, ToolsRegisterResult { registered: count })
            }
            Err(()) => RpcResponse::error(
                id,
                error::INTERNAL_ERROR,
                "Failed to acquire tool registry lock",
            ),
        }
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
                match self.runtime.read_session_rich(&session_id).await {
                    Ok(Some(mut info)) => {
                        info.session_ref = self.runtime.realm_id().map(|realm| {
                            meerkat_contracts::format_session_ref(&realm, &session_id)
                        });
                        RpcResponse::success(id, info)
                    }
                    Ok(None) => RpcResponse::error(
                        id,
                        error::SESSION_NOT_FOUND,
                        format!("Session not found: {session_id}"),
                    ),
                    // A store-layer failure must surface as a typed error, not
                    // be laundered into a "session not found".
                    Err(err) => RpcResponse::from_error(id, err),
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
                    // A store-layer failure must surface as a typed error, not
                    // be laundered into a "session not found" (K17).
                    Err(err) => mob_session_service_error_response(id, &session_id, err),
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

        let offset = match bounded_collection_offset(params.offset) {
            Ok(offset) => offset,
            Err(message) => return RpcResponse::error(id, error::INVALID_PARAMS, message),
        };
        let limit = match bounded_collection_limit(params.limit) {
            Ok(limit) => limit,
            Err(message) => return RpcResponse::error(id, error::INVALID_PARAMS, message),
        };
        let query = SessionHistoryQuery {
            offset,
            limit: Some(limit),
        };

        match self.resolve_session_owner(&session_id).await {
            Some(SessionOwner::Runtime) => {
                match self
                    .runtime
                    .read_session_history_rich(&session_id, query)
                    .await
                {
                    Ok(Some(mut history)) => {
                        history.session_ref = self.runtime.realm_id().map(|realm| {
                            meerkat_contracts::format_session_ref(&realm, &session_id)
                        });
                        RpcResponse::success(id, history)
                    }
                    Ok(None) => RpcResponse::error(
                        id,
                        error::SESSION_NOT_FOUND,
                        format!("Session not found: {session_id}"),
                    ),
                    // A store/control-plane fault is surfaced as itself —
                    // never collapsed into a not-found.
                    Err(err) => RpcResponse::from_error(id, err),
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

    async fn handle_session_transcript_revision(
        &self,
        id: Option<crate::protocol::RpcId>,
        params: Option<&serde_json::value::RawValue>,
    ) -> RpcResponse {
        let params: meerkat_contracts::ReadSessionTranscriptRevisionParams =
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
                .read_session_transcript_revision_rich(
                    &session_id,
                    SessionTranscriptRevisionQuery {
                        revision: params.revision,
                        offset: params.offset.unwrap_or(0),
                        limit: params.limit,
                    },
                )
                .await
            {
                Ok(revision) => RpcResponse::success(id, revision),
                Err(rpc_err) => RpcResponse::from_error(id, rpc_err),
            },
            #[cfg(feature = "mob")]
            Some(SessionOwner::Mob) => RpcResponse::error(
                id,
                error::INVALID_PARAMS,
                "mob-owned session transcript revisions cannot be read through the generic session surface",
            ),
            None => RpcResponse::error(
                id,
                error::SESSION_NOT_FOUND,
                format!("Session not found: {session_id}"),
            ),
        }
    }

    async fn handle_session_transcript_revisions(
        &self,
        id: Option<crate::protocol::RpcId>,
        params: Option<&serde_json::value::RawValue>,
    ) -> RpcResponse {
        let params: meerkat_contracts::ListSessionTranscriptRevisionsParams =
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
                .list_session_transcript_revisions_rich(
                    &session_id,
                    SessionTranscriptRevisionListQuery {
                        limit: params.limit,
                        offset: params.offset,
                    },
                )
                .await
            {
                Ok(list) => RpcResponse::success(id, list),
                Err(rpc_err) => RpcResponse::from_error(id, rpc_err),
            },
            #[cfg(feature = "mob")]
            Some(SessionOwner::Mob) => RpcResponse::error(
                id,
                error::INVALID_PARAMS,
                "mob-owned session transcript revisions cannot be read through the generic session surface",
            ),
            None => RpcResponse::error(
                id,
                error::SESSION_NOT_FOUND,
                format!("Session not found: {session_id}"),
            ),
        }
    }

    async fn handle_session_rewrite_transcript(
        &self,
        id: Option<crate::protocol::RpcId>,
        params: Option<&serde_json::value::RawValue>,
    ) -> RpcResponse {
        let params: meerkat_contracts::RewriteSessionTranscriptParams =
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
        let replacement = match params
            .replacement
            .into_iter()
            .map(meerkat_contracts::TranscriptRewriteMessage::into_core)
            .collect::<Result<Vec<_>, _>>()
        {
            Ok(replacement) => replacement,
            Err(err) => {
                return RpcResponse::error(
                    id,
                    error::INVALID_PARAMS,
                    format!("Invalid transcript replacement: {err}"),
                );
            }
        };

        match self.resolve_session_owner(&session_id).await {
            Some(SessionOwner::Runtime) => match self
                .runtime
                .rewrite_session_transcript(
                    &session_id,
                    SessionTranscriptRewriteRequest {
                        selection: params.selection,
                        replacement,
                        reason: params.reason,
                        actor: params.actor,
                        expected_parent_revision: params.expected_parent_revision,
                        running_behavior: params.running_behavior,
                    },
                )
                .await
            {
                Ok(result) => RpcResponse::success(id, result),
                Err(rpc_err) => RpcResponse::from_error(id, rpc_err),
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

    async fn handle_session_restore_transcript_revision(
        &self,
        id: Option<crate::protocol::RpcId>,
        params: Option<&serde_json::value::RawValue>,
    ) -> RpcResponse {
        let params: meerkat_contracts::RestoreSessionTranscriptRevisionParams =
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
                .restore_session_transcript_revision(
                    &session_id,
                    SessionTranscriptRestoreRevisionRequest {
                        revision: params.revision,
                        reason: params.reason,
                        actor: params.actor,
                        expected_parent_revision: params.expected_parent_revision,
                        running_behavior: params.running_behavior,
                    },
                )
                .await
            {
                Ok(result) => RpcResponse::success(id, result),
                Err(rpc_err) => RpcResponse::from_error(id, rpc_err),
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

    async fn handle_session_update_system_prompt(
        &self,
        id: Option<crate::protocol::RpcId>,
        params: Option<&serde_json::value::RawValue>,
    ) -> RpcResponse {
        let params: meerkat_contracts::UpdateSystemPromptParams =
            match handlers::parse_params(params) {
                Ok(params) => params,
                Err(response) => return response.with_id(id),
            };
        let session_id = match handlers::parse_session_id_for_runtime(
            id.clone(),
            &params.session_id,
            &self.runtime,
        ) {
            Ok(session_id) => session_id,
            Err(response) => return response,
        };

        match self.resolve_session_owner(&session_id).await {
            Some(SessionOwner::Runtime) => match self
                .runtime
                .update_system_prompt(&session_id, params.into_core())
                .await
            {
                Ok(result) => RpcResponse::success(id, result),
                Err(rpc_error) => RpcResponse::from_error(id, rpc_error),
            },
            #[cfg(feature = "mob")]
            Some(SessionOwner::Mob) => RpcResponse::error(
                id,
                error::INVALID_PARAMS,
                "mob-owned system prompts must be updated through the mob owner",
            ),
            None => RpcResponse::error(
                id,
                error::SESSION_NOT_FOUND,
                format!("Session not found: {session_id}"),
            ),
        }
    }

    async fn handle_session_activate_instruction(
        &self,
        id: Option<crate::protocol::RpcId>,
        params: Option<&serde_json::value::RawValue>,
    ) -> RpcResponse {
        let params: meerkat_contracts::ActivateInstructionParams =
            match handlers::parse_params(params) {
                Ok(params) => params,
                Err(response) => return response.with_id(id),
            };
        let session_id = match handlers::parse_session_id_for_runtime(
            id.clone(),
            &params.session_id,
            &self.runtime,
        ) {
            Ok(session_id) => session_id,
            Err(response) => return response,
        };

        match self.resolve_session_owner(&session_id).await {
            Some(SessionOwner::Runtime) => {
                let runtime = Arc::clone(&self.runtime);
                let activation_session_id = session_id.clone();
                match tokio::spawn(async move {
                    runtime
                        .activate_instruction(&activation_session_id, params.activation)
                        .await
                })
                .await
                {
                    Ok(Ok(receipt)) => RpcResponse::success(id, receipt),
                    Ok(Err(error)) => RpcResponse::from_error(id, error),
                    Err(error) => RpcResponse::error(
                        id,
                        error::INTERNAL_ERROR,
                        format!("instruction activation owner task failed: {error}"),
                    ),
                }
            }
            #[cfg(feature = "mob")]
            Some(SessionOwner::Mob) => RpcResponse::error(
                id,
                error::INVALID_PARAMS,
                "mob-owned instruction activation must be reconciled through the mob owner",
            ),
            None => RpcResponse::error(
                id,
                error::SESSION_NOT_FOUND,
                format!("Session not found: {session_id}"),
            ),
        }
    }

    async fn handle_session_instruction_activations(
        &self,
        id: Option<crate::protocol::RpcId>,
        params: Option<&serde_json::value::RawValue>,
    ) -> RpcResponse {
        let params: meerkat_contracts::ReadInstructionActivationsParams =
            match handlers::parse_params(params) {
                Ok(params) => params,
                Err(response) => return response.with_id(id),
            };
        let session_id = match handlers::parse_session_id_for_runtime(
            id.clone(),
            &params.session_id,
            &self.runtime,
        ) {
            Ok(session_id) => session_id,
            Err(response) => return response,
        };
        let query = params.into_core();

        match self.resolve_session_owner(&session_id).await {
            Some(SessionOwner::Runtime) => match self
                .runtime
                .read_instruction_activation_records(&session_id, query)
                .await
            {
                Ok(page) => RpcResponse::success(id, page),
                Err(error) => RpcResponse::from_error(id, error),
            },
            #[cfg(feature = "mob")]
            Some(SessionOwner::Mob) => RpcResponse::error(
                id,
                error::INVALID_PARAMS,
                "mob-owned instruction status must be read through the mob owner",
            ),
            None => RpcResponse::error(
                id,
                error::SESSION_NOT_FOUND,
                format!("Session not found: {session_id}"),
            ),
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
                .fork_session_at(&session_id, lower_session_fork_at_request(&params))
                .await
            {
                Ok(mut result) => {
                    result.session_ref = self.runtime.realm_id().map(|realm| {
                        meerkat_contracts::format_session_ref(&realm, &result.session_id)
                    });
                    RpcResponse::success(id, result)
                }
                Err(rpc_err) => RpcResponse::from_error(id, rpc_err),
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

        // Lower the typed wire replacement into the core `TranscriptReplacement`
        // at the boundary; a malformed replacement fails closed with a typed
        // parse error rather than being forwarded.
        let replacement = match params.replacement.clone().into_core() {
            Ok(replacement) => replacement,
            Err(err) => {
                return RpcResponse::error(id, crate::error::INVALID_PARAMS, err.to_string());
            }
        };

        match self.resolve_session_owner(&session_id).await {
            Some(SessionOwner::Runtime) => match self
                .runtime
                .fork_session_replace(
                    &session_id,
                    lower_session_fork_replace_request(&params, replacement),
                )
                .await
            {
                Ok(mut result) => {
                    result.session_ref = self.runtime.realm_id().map(|realm| {
                        meerkat_contracts::format_session_ref(&realm, &result.session_id)
                    });
                    RpcResponse::success(id, result)
                }
                Err(rpc_err) => RpcResponse::from_error(id, rpc_err),
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
        // Owned once: the boxed handler futures borrow it across their awaits.
        #[cfg(feature = "mob")]
        let bridge_session_id = session_id.to_string();
        match routed_arm(|| self.resolve_session_owner(&session_id)).await {
            Some(SessionOwner::Runtime) => {
                match routed_arm(|| self.runtime.archive_session(&session_id)).await {
                    Ok(()) => {
                        // Clean up session-owned mobs (implicit + explicit).
                        #[cfg(feature = "mob")]
                        if let Err(error) = routed_arm(|| {
                            self.mob_state
                                .destroy_bridge_session_mobs(&bridge_session_id)
                        })
                        .await
                        {
                            return mob_destroy_cleanup_error_response(id, error);
                        }
                        // The archive committed; join the runtime teardown until
                        // terminal rather than reporting a saga still completing
                        // (`UnregisterInProgress` after the caller grace) as a
                        // failed unregister.
                        if let Err(error) = routed_arm(|| {
                            self.runtime_adapter
                                .unregister_current_session_registration_until_terminal(&session_id)
                        })
                        .await
                        {
                            return RpcResponse::error(
                                id,
                                error::INTERNAL_ERROR,
                                format!(
                                    "session archived but runtime unregister failed for {session_id}: {error}"
                                ),
                            );
                        }
                        RpcResponse::success(id, json!({"archived": true}))
                    }
                    Err(rpc_err) => {
                        #[cfg(feature = "mob")]
                        {
                            let retained_cleanup = routed_arm(|| {
                                self.mob_state
                                    .has_bridge_session_scoped_mobs(&bridge_session_id)
                            })
                            .await;
                            if rpc_err.code == error::SESSION_NOT_FOUND
                                && retained_cleanup
                                && let Err(error) = routed_arm(|| {
                                    self.mob_state
                                        .destroy_bridge_session_mobs(&bridge_session_id)
                                })
                                .await
                            {
                                return mob_destroy_cleanup_error_response(id, error);
                            }
                        }

                        RpcResponse::from_error(id, rpc_err)
                    }
                }
            }
            #[cfg(feature = "mob")]
            Some(SessionOwner::Mob) => match routed_arm(|| {
                self.mob_state
                    .archive_mob_owned_bridge_session_with_cleanup(
                        &session_id,
                        "mob cleanup during archive incomplete",
                    )
            })
            .await
            {
                Ok(true) => RpcResponse::success(id, json!({"archived": true})),
                Ok(false) => {
                    if routed_arm(|| {
                        self.mob_state
                            .has_bridge_session_scoped_mobs(&bridge_session_id)
                    })
                    .await
                        && let Err(error) = routed_arm(|| {
                            self.mob_state
                                .destroy_bridge_session_mobs(&bridge_session_id)
                        })
                        .await
                    {
                        return mob_destroy_cleanup_error_response(id, error);
                    }
                    RpcResponse::error(
                        id,
                        error::SESSION_NOT_FOUND,
                        format!("Session not found: {session_id}"),
                    )
                }
                Err(error) => mob_session_service_error_response(id, &session_id, error),
            },
            None => {
                #[cfg(feature = "mob")]
                if routed_arm(|| {
                    self.mob_state
                        .has_bridge_session_scoped_mobs(&bridge_session_id)
                })
                .await
                    && let Err(error) = routed_arm(|| {
                        self.mob_state
                            .destroy_bridge_session_mobs(&bridge_session_id)
                    })
                    .await
                {
                    return mob_destroy_cleanup_error_response(id, error);
                }
                RpcResponse::error(
                    id,
                    error::SESSION_NOT_FOUND,
                    format!("Session not found: {session_id}"),
                )
            }
        }
    }

    async fn handle_session_input_state(
        &self,
        id: Option<crate::protocol::RpcId>,
        params: Option<&serde_json::value::RawValue>,
    ) -> RpcResponse {
        use meerkat_runtime::service_ext::SessionServiceRuntimeExt as _;

        let params: meerkat_contracts::SessionInputStateParams =
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
        let adapter = self.runtime.runtime_adapter();
        let stored = match &params.selector {
            meerkat_contracts::SessionInputStateSelector::InputId(raw) => {
                let input_id = match raw
                    .parse::<uuid::Uuid>()
                    .map(meerkat_core::lifecycle::InputId::from_uuid)
                {
                    Ok(input_id) => input_id,
                    Err(error) => {
                        return RpcResponse::error(
                            id,
                            crate::error::INVALID_PARAMS,
                            format!("invalid input_id '{raw}': {error}"),
                        );
                    }
                };
                adapter.input_state(&session_id, &input_id).await
            }
            meerkat_contracts::SessionInputStateSelector::IdempotencyKey(key) => {
                adapter
                    .input_state_by_idempotency_key(&session_id, key)
                    .await
            }
        };
        match stored {
            Ok(state) => {
                let state = match state
                    .map(handlers::runtime::to_wire_input_state)
                    .transpose()
                {
                    Ok(state) => state,
                    Err(error) => {
                        return RpcResponse::error(
                            id,
                            crate::error::INTERNAL_ERROR,
                            format!("input state projection failed: {error}"),
                        );
                    }
                };
                match serde_json::to_value(meerkat_contracts::SessionInputStateResult { state }) {
                    Ok(value) => RpcResponse::success(id, value),
                    Err(error) => RpcResponse::error(
                        id,
                        crate::error::INTERNAL_ERROR,
                        format!("input state serialization failed: {error}"),
                    ),
                }
            }
            Err(error) => RpcResponse::error(
                id,
                crate::error::INTERNAL_ERROR,
                format!("input state read failed: {error}"),
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
            content: params.content,
            source: params.source,
            idempotency_key: params.idempotency_key,
        };
        match self.resolve_session_owner(&session_id).await {
            Some(SessionOwner::Runtime) => {
                match self.runtime.append_system_context(&session_id, req).await {
                    Ok(result) => RpcResponse::success(id, json!({"status": result.status})),
                    Err(rpc_err) => RpcResponse::from_error(id, rpc_err),
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
                // Typed control-error mapping: session faults go through the
                // shared store-error mapper; request-shape faults keep their
                // typed control codes instead of laundering to SESSION_NOT_FOUND.
                Err(meerkat_core::SessionControlError::Session(err)) => {
                    mob_session_service_error_response(id, &session_id, err)
                }
                Err(control_err @ meerkat_core::SessionControlError::Authorization(_)) => {
                    RpcResponse::error_with_data(
                        id,
                        if matches!(
                            &control_err,
                            meerkat_core::SessionControlError::Authorization(
                                meerkat_core::OperationAuthorizationError::Refused(_)
                            )
                        ) {
                            meerkat_contracts::ErrorCode::ScopeDenied.jsonrpc_code()
                        } else {
                            error::INTERNAL_ERROR
                        },
                        control_err.to_string(),
                        json!({ "code": control_err.code() }),
                    )
                }
                Err(meerkat_core::SessionControlError::Review(refusal)) => {
                    let rpc = crate::session_runtime::review_refusal_to_rpc(refusal);
                    RpcResponse::error_with_data(
                        id,
                        rpc.code,
                        rpc.message,
                        rpc.data.unwrap_or(serde_json::Value::Null),
                    )
                }
                Err(control_err) => RpcResponse::error_with_data(
                    id,
                    error::INVALID_REQUEST,
                    control_err.to_string(),
                    json!({ "code": control_err.code() }),
                ),
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
                match self.runtime.session_state(&session_id).await {
                    // Session exists and is live: proceed.
                    Ok(Some(_)) => {}
                    // Session genuinely absent: it has been archived.
                    Ok(None) => {
                        return RpcResponse::error(
                            id,
                            error::INVALID_PARAMS,
                            format!("Session is archived: {session_id}"),
                        );
                    }
                    // A store-layer failure must surface as a typed error, not
                    // be conflated with an archived session.
                    Err(err) => {
                        return RpcResponse::from_error(id, err);
                    }
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
                let data = meerkat_contracts::CommsSendErrorData::InvalidCommand {
                    message: err.to_string(),
                };
                return match serde_json::to_value(&data) {
                    Ok(data) => RpcResponse::error_with_data(
                        id,
                        error::INVALID_PARAMS,
                        "Command validation failed",
                        data,
                    ),
                    Err(serialize_error) => RpcResponse::error(
                        id,
                        error::INTERNAL_ERROR,
                        format!("failed to serialize comms error data: {serialize_error}"),
                    ),
                };
            }
        };
        match comms.send(cmd).await {
            Ok(receipt) => match send_receipt_json(receipt) {
                Ok(value) => RpcResponse::success(id, value),
                Err(serialize_error) => RpcResponse::error(
                    id,
                    error::INTERNAL_ERROR,
                    format!("failed to serialize comms send receipt: {serialize_error}"),
                ),
            },
            Err(e) => {
                let normalized = handlers::comms::normalize_send_error(peer_name.as_deref(), &e);
                let message = normalized.message().to_string();
                match serde_json::to_value(&normalized) {
                    Ok(data) => {
                        RpcResponse::error_with_data(id, error::INTERNAL_ERROR, message, data)
                    }
                    Err(serialize_error) => RpcResponse::error(
                        id,
                        error::INTERNAL_ERROR,
                        format!("failed to serialize comms error data: {serialize_error}"),
                    ),
                }
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
                match self.runtime.session_state(&session_id).await {
                    // Session exists and is live: proceed.
                    Ok(Some(_)) => {}
                    // Session genuinely absent: it has been archived.
                    Ok(None) => {
                        return RpcResponse::error(
                            id,
                            error::INVALID_PARAMS,
                            format!("Session is archived: {session_id}"),
                        );
                    }
                    // A store-layer failure must surface as a typed error, not
                    // be conflated with an archived session.
                    Err(err) => {
                        return RpcResponse::from_error(id, err);
                    }
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

        let terminal_reservation = match self
            .notification_sink
            .stream_terminal_admission
            .try_reserve()
        {
            Ok(reservation) => reservation,
            Err(error) => {
                return RpcResponse::error(
                    id,
                    error::BUDGET_EXHAUSTED,
                    format!("Cannot reserve event stream terminal delivery: {error}"),
                );
            }
        };
        let stream_id = StreamRef::mint();
        let open_authority = match resolve_session_stream_open_authority(
            &self.stream_authority,
            &stream_id,
            &session_id,
        )
        .await
        {
            Ok(authority) => authority,
            Err(error) => {
                return RpcResponse::error(
                    id,
                    error::INTERNAL_ERROR,
                    format!("Failed to authorize session event stream open: {error}"),
                );
            }
        };
        let (stop_tx, stop_rx) = oneshot::channel::<()>();
        let notification_sink = self.notification_sink.clone();
        let active_session_streams = self.active_session_streams.clone();
        let stream_authority = self.stream_authority.clone();
        let stream_id_for_task = stream_id;
        let session_id_for_task = session_id.clone();
        let session_delivery = Arc::new(Mutex::new(SessionStreamDelivery::default()));

        self.active_session_streams.lock().await.insert(
            stream_id,
            StreamForwarder {
                terminal: StreamTerminal::Session(session_id.clone()),
                terminal_reservation,
                session_delivery: Some(session_delivery.clone()),
                state: StreamForwarderState::Active {
                    stop_tx: Some(stop_tx),
                    task: None,
                },
            },
        );

        let task = tokio::spawn(
            SessionStreamForwarding {
                notification_sink,
                active_session_streams,
                stream_authority,
                stream_id: stream_id_for_task,
                session_id: session_id_for_task,
                delivery: session_delivery,
            }
            .run(stream, stop_rx),
        );

        attach_stream_forwarder_task(&self.active_session_streams, stream_id, task).await;

        let result = meerkat_contracts::SessionStreamOpenResult {
            stream_id: stream_id.as_string(),
            session_id: open_authority.session_id,
            opened: open_authority.opened,
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

        let stream_id = match StreamRef::parse(&params.stream_id) {
            Some(stream_id) => stream_id,
            None => {
                return RpcResponse::error(
                    id,
                    error::INVALID_PARAMS,
                    format!("Invalid stream_id: {}", params.stream_id),
                );
            }
        };

        let close_authority = match close_session_stream(
            &self.notification_sink,
            &self.active_session_streams,
            &self.stream_authority,
            &stream_id,
        )
        .await
        {
            Ok(authority) => authority,
            Err(error) => {
                return RpcResponse::error(
                    id,
                    error::INVALID_PARAMS,
                    format!("Session event stream not tracked: {stream_id} ({error})"),
                );
            }
        };

        let result = meerkat_contracts::SessionStreamCloseResult {
            stream_id: stream_id.as_string(),
            closed: close_authority.closed,
            already_closed: close_authority.already_closed,
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
        let params = match handlers::parse_params::<meerkat_contracts::MobStreamOpenParams>(params)
        {
            Ok(p) => p,
            Err(resp) => return resp,
        };

        let mob_id = meerkat_mob::MobId::from(params.mob_id.as_str());

        let stream_id = StreamRef::mint();
        let notification_sink = self.notification_sink.clone();
        let active_mob_streams = self.active_mob_streams.clone();
        let stream_authority = self.stream_authority.clone();
        let stream_id_for_task = stream_id;

        let terminal_reservation = match notification_sink.stream_terminal_admission.try_reserve() {
            Ok(reservation) => reservation,
            Err(error) => {
                return RpcResponse::error(
                    id,
                    error::BUDGET_EXHAUSTED,
                    format!("Cannot reserve event stream terminal delivery: {error}"),
                );
            }
        };
        let opened = if let Some(agent_identity) = params.agent_identity {
            // Per-member stream: subscribe to a specific member's agent events.
            let identity = meerkat_mob::AgentIdentity::from(agent_identity.as_str());
            let stream: meerkat_core::comms::EventStream = match self
                .mob_state
                .subscribe_agent_events(&mob_id, &identity)
                .await
            {
                Ok(s) => s,
                Err(e) => {
                    return handlers::mob::mob_error_response(id, &e);
                }
            };

            let open_authority =
                match resolve_mob_stream_open_authority(&self.stream_authority, &stream_id).await {
                    Ok(authority) => authority,
                    Err(error) => {
                        return RpcResponse::error(
                            id,
                            error::INTERNAL_ERROR,
                            format!("Failed to authorize mob event stream open: {error}"),
                        );
                    }
                };
            let (stop_tx, stop_rx) = oneshot::channel::<()>();
            let notification_sink = notification_sink.clone();
            let active_mob_streams = active_mob_streams.clone();
            let stream_authority = stream_authority.clone();
            self.active_mob_streams.lock().await.insert(
                stream_id,
                StreamForwarder {
                    terminal: StreamTerminal::Mob,
                    terminal_reservation,
                    session_delivery: None,
                    state: StreamForwarderState::Active {
                        stop_tx: Some(stop_tx),
                        task: None,
                    },
                },
            );
            let task = tokio::spawn(async move {
                let mut stream = stream;
                let mut stop_rx = stop_rx;
                let mut sequence = 0u64;

                loop {
                    tokio::select! {
                        _ = &mut stop_rx => {
                            break;
                        }
                        _ = notification_sink.tx.closed() => {
                            emit_authorized_mob_stream_terminal(
                                &notification_sink, &active_mob_streams, &stream_authority,
                                stream_id_for_task,
                                meerkat_runtime::meerkat_machine::dsl::RpcEventStreamTerminalObservationKind::NotificationReceiverGone,
                                Some("transport stream notification receiver closed".to_string()),
                            ).await;
                            break;
                        }
                        event = stream.next() => {
                            match event {
                                Some(envelope) => {
                                    sequence += 1;
                                    let event_json = match serialize_mob_stream_event(&envelope) {
                                        Ok(value) => value,
                                        Err(serialize_error) => {
                                            emit_authorized_mob_stream_serialization_terminal(
                                                &notification_sink,
                                                &active_mob_streams,
                                                &stream_authority,
                                                stream_id_for_task,
                                                &serialize_error,
                                            )
                                            .await;
                                            break;
                                        }
                                    };
                                    let emit_status = notification_sink
                                        .emit_mob_stream_event(
                                            &stream_id_for_task,
                                            sequence,
                                            &event_json,
                                        )
                                        .await;
                                    if let Some((observation, detail)) =
                                        terminal_observation_from_emit_status(emit_status)
                                    {
                                        emit_authorized_mob_stream_terminal(
                                            &notification_sink,
                                            &active_mob_streams,
                                            &stream_authority,
                                            stream_id_for_task,
                                            observation,
                                            Some(detail.to_string()),
                                        )
                                        .await;
                                        break;
                                    }
                                }
                                None => {
                                    emit_authorized_mob_stream_terminal(
                                        &notification_sink,
                                        &active_mob_streams,
                                        &stream_authority,
                                        stream_id_for_task,
                                        meerkat_runtime::meerkat_machine::dsl::RpcEventStreamTerminalObservationKind::TransportEnded,
                                        None,
                                    )
                                    .await;
                                    break;
                                }
                            }
                        }
                    }
                }
            });

            attach_stream_forwarder_task(&self.active_mob_streams, stream_id, task).await;
            open_authority.opened
        } else {
            // Mob-wide stream: subscribe to all members' events (attributed).
            let mut router_handle = match self.mob_state.subscribe_mob_events(&mob_id).await {
                Ok(handle) => handle,
                Err(error) => {
                    return handlers::mob::mob_error_response(id, &error);
                }
            };

            let open_authority =
                match resolve_mob_stream_open_authority(&self.stream_authority, &stream_id).await {
                    Ok(authority) => authority,
                    Err(error) => {
                        return RpcResponse::error(
                            id,
                            error::INTERNAL_ERROR,
                            format!("Failed to authorize mob event stream open: {error}"),
                        );
                    }
                };
            let (stop_tx, stop_rx) = oneshot::channel::<()>();
            let notification_sink = notification_sink.clone();
            let active_mob_streams = active_mob_streams.clone();
            let stream_authority = stream_authority.clone();
            self.active_mob_streams.lock().await.insert(
                stream_id,
                StreamForwarder {
                    terminal: StreamTerminal::Mob,
                    terminal_reservation,
                    session_delivery: None,
                    state: StreamForwarderState::Active {
                        stop_tx: Some(stop_tx),
                        task: None,
                    },
                },
            );
            let task = tokio::spawn(async move {
                let mut stop_rx = stop_rx;
                let mut sequence = 0u64;

                loop {
                    tokio::select! {
                        _ = &mut stop_rx => {
                            break;
                        }
                        _ = notification_sink.tx.closed() => {
                            emit_authorized_mob_stream_terminal(
                                &notification_sink, &active_mob_streams, &stream_authority,
                                stream_id_for_task,
                                meerkat_runtime::meerkat_machine::dsl::RpcEventStreamTerminalObservationKind::NotificationReceiverGone,
                                Some("transport stream notification receiver closed".to_string()),
                            ).await;
                            break;
                        }
                        event = router_handle.event_rx.recv() => {
                            match event {
                                Some(attributed) => {
                                    sequence += 1;
                                    let event_json = match serialize_mob_stream_event(&attributed) {
                                        Ok(value) => value,
                                        Err(serialize_error) => {
                                            emit_authorized_mob_stream_serialization_terminal(
                                                &notification_sink,
                                                &active_mob_streams,
                                                &stream_authority,
                                                stream_id_for_task,
                                                &serialize_error,
                                            )
                                            .await;
                                            break;
                                        }
                                    };
                                    let emit_status = notification_sink
                                        .emit_mob_stream_event(
                                            &stream_id_for_task,
                                            sequence,
                                            &event_json,
                                        )
                                        .await;
                                    if let Some((observation, detail)) =
                                        terminal_observation_from_emit_status(emit_status)
                                    {
                                        emit_authorized_mob_stream_terminal(
                                            &notification_sink,
                                            &active_mob_streams,
                                            &stream_authority,
                                            stream_id_for_task,
                                            observation,
                                            Some(detail.to_string()),
                                        )
                                        .await;
                                        break;
                                    }
                                }
                                None => {
                                    emit_authorized_mob_stream_terminal(
                                        &notification_sink,
                                        &active_mob_streams,
                                        &stream_authority,
                                        stream_id_for_task,
                                        meerkat_runtime::meerkat_machine::dsl::RpcEventStreamTerminalObservationKind::TransportEnded,
                                        None,
                                    )
                                    .await;
                                    break;
                                }
                            }
                        }
                    }
                }
            });

            attach_stream_forwarder_task(&self.active_mob_streams, stream_id, task).await;
            open_authority.opened
        };

        let result = meerkat_contracts::MobStreamOpenResult {
            stream_id: stream_id.as_string(),
            opened,
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
    async fn handle_mob_stream_close(
        &self,
        id: Option<crate::protocol::RpcId>,
        params: Option<&serde_json::value::RawValue>,
    ) -> RpcResponse {
        let params = match handlers::parse_params::<meerkat_contracts::MobStreamCloseParams>(params)
        {
            Ok(p) => p,
            Err(resp) => return resp,
        };

        let stream_id = match StreamRef::parse(&params.stream_id) {
            Some(stream_id) => stream_id,
            None => {
                return RpcResponse::error(
                    id,
                    error::INVALID_PARAMS,
                    format!("Invalid stream_id: {}", params.stream_id),
                );
            }
        };

        let close_authority =
            match resolve_mob_stream_close_authority(&self.stream_authority, &stream_id).await {
                Ok(authority) => authority,
                Err(error) => {
                    if let Some(mut stream) =
                        self.active_mob_streams.lock().await.remove(&stream_id)
                    {
                        stop_stream_forwarder(&mut stream);
                    }
                    return RpcResponse::error(
                        id,
                        error::INVALID_PARAMS,
                        format!("Mob event stream not tracked: {stream_id} ({error})"),
                    );
                }
            };
        // Already-closed callers must not abort the issuer that still owns
        // the machine terminal effect and the pending reservation handoff.
        if let Some(terminal) = close_authority.terminal.as_ref() {
            let stream = self.active_mob_streams.lock().await.remove(&stream_id);
            if let Some(mut stream) = stream {
                stop_stream_forwarder(&mut stream);
                self.notification_sink
                    .emit_mob_stream_end(terminal, stream.terminal_reservation)
                    .await;
            }
        }

        let result = meerkat_contracts::MobStreamCloseResult {
            stream_id: stream_id.as_string(),
            closed: close_authority.closed,
            already_closed: close_authority.already_closed,
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
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    use std::pin::Pin;
    use std::sync::{Arc, Condvar, Mutex as StdMutex};
    use std::time::Duration;

    use async_trait::async_trait;
    use futures::stream;
    use meerkat::AgentFactory;
    use meerkat::surface::{RequestContext, SurfaceRequestExecutor, noop_request_action};
    use meerkat_client::{LlmClient, LlmError};
    #[cfg(feature = "comms")]
    use meerkat_core::agent::CommsRuntime as CoreCommsRuntime;
    #[cfg(feature = "comms")]
    use meerkat_core::comms::TrustedPeerDescriptor;
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

    struct BlockingInstructionActivationFence {
        started: Arc<tokio::sync::Notify>,
        release: Arc<(StdMutex<bool>, Condvar)>,
    }

    impl meerkat_runtime::RuntimeStoreWriteFence for BlockingInstructionActivationFence {
        fn execute_if_current(
            &self,
            operation: Box<dyn FnOnce() -> Result<(), meerkat_runtime::RuntimeStoreError> + '_>,
        ) -> Result<
            meerkat_runtime::RuntimeStoreWriteFenceOutcome,
            meerkat_runtime::RuntimeStoreError,
        > {
            self.started.notify_one();
            let (lock, wake) = &*self.release;
            let mut released = lock.lock().expect("release lock");
            while !*released {
                released = wake.wait(released).expect("release wait");
            }
            operation()?;
            Ok(meerkat_runtime::RuntimeStoreWriteFenceOutcome::Applied)
        }
    }

    fn release_instruction_activation_fence(release: &Arc<(StdMutex<bool>, Condvar)>) {
        let (lock, wake) = &**release;
        *lock.lock().expect("release lock") = true;
        wake.notify_one();
    }

    #[derive(Clone, Copy)]
    enum RefusingInstructionActivationFence {
        Conflict,
        Backoff,
    }

    impl meerkat_runtime::RuntimeStoreWriteFence for RefusingInstructionActivationFence {
        fn execute_if_current(
            &self,
            _operation: Box<dyn FnOnce() -> Result<(), meerkat_runtime::RuntimeStoreError> + '_>,
        ) -> Result<
            meerkat_runtime::RuntimeStoreWriteFenceOutcome,
            meerkat_runtime::RuntimeStoreError,
        > {
            Ok(match self {
                Self::Conflict => meerkat_runtime::RuntimeStoreWriteFenceOutcome::Conflict {
                    reason: "superseded identity generation".to_string(),
                },
                Self::Backoff => meerkat_runtime::RuntimeStoreWriteFenceOutcome::Backoff {
                    reason: "identity authority busy".to_string(),
                },
            })
        }
    }

    #[test]
    fn session_fork_request_lowering_preserves_each_tool_policy() {
        let policies = [
            None,
            Some(meerkat_contracts::wire::WireToolAccessPolicy::Inherit),
            Some(meerkat_contracts::wire::WireToolAccessPolicy::AllowList(
                vec!["shell".to_string()],
            )),
            Some(meerkat_contracts::wire::WireToolAccessPolicy::DenyList(
                vec!["browser".to_string()],
            )),
        ];

        for policy in policies {
            let expected = policy
                .clone()
                .map(meerkat_contracts::wire::WireToolAccessPolicy::into_core);
            let at_params = meerkat_contracts::ForkSessionAtParams {
                session_id: "source-session".to_string(),
                message_index: 4,
                running_behavior: Default::default(),
                tool_access_policy: policy.clone(),
                inherited_instructions: None,
            };
            let at_request = lower_session_fork_at_request(&at_params);
            assert_eq!(at_request.tool_access_policy, expected);

            let replace_params = meerkat_contracts::ForkSessionReplaceParams {
                session_id: "source-session".to_string(),
                message_index: 4,
                replacement: serde_json::from_value(serde_json::json!({
                    "type": "message",
                    "message": {
                        "role": "system",
                        "content": "replacement"
                    }
                }))
                .expect("typed replacement should parse"),
                running_behavior: Default::default(),
                tool_access_policy: policy,
                inherited_instructions: None,
            };
            let replace_request = lower_session_fork_replace_request(
                &replace_params,
                meerkat_core::session::TranscriptReplacement::UserContentBlock {
                    block_index: 0,
                    block: meerkat_core::types::ContentBlock::Text {
                        text: "replacement".to_string(),
                    },
                },
            );
            assert_eq!(replace_request.tool_access_policy, expected);
        }
    }

    macro_rules! notification_params {
        ($notification:expr) => {
            $notification
                .params_value()
                .expect("notification params must be valid JSON")
        };
    }

    /// K17 regression: mob session-service failures must keep their typed
    /// identity on the wire. Only `NotFound` may map to `SESSION_NOT_FOUND`;
    /// a store-layer fault is a typed internal error, not a fabricated
    /// "session not found".
    #[cfg(feature = "mob")]
    #[test]
    fn mob_session_store_error_is_not_laundered_to_not_found() {
        let session_id = meerkat_core::SessionId::new();

        let store_error = SessionError::Store("sqlite I/O failure".into());
        let response =
            mob_session_service_error_response(Some(RpcId::Num(1)), &session_id, store_error);
        let err = response.error.expect("store failure must be an error");
        assert_eq!(err.code, error::INTERNAL_ERROR);
        assert_ne!(err.code, error::SESSION_NOT_FOUND);
        assert!(err.message.contains("sqlite I/O failure"));

        let not_found = SessionError::NotFound {
            id: session_id.clone(),
        };
        let response =
            mob_session_service_error_response(Some(RpcId::Num(2)), &session_id, not_found);
        let err = response.error.expect("not-found must be an error");
        assert_eq!(err.code, error::SESSION_NOT_FOUND);

        let busy = SessionError::Busy {
            id: session_id.clone(),
        };
        let response = mob_session_service_error_response(Some(RpcId::Num(3)), &session_id, busy);
        let err = response.error.expect("busy must be an error");
        assert_eq!(err.code, error::SESSION_BUSY);
    }

    #[cfg(feature = "mob")]
    struct StaticDispatcher {
        tools: Arc<[Arc<ToolDef>]>,
    }

    #[cfg(feature = "mob")]
    impl StaticDispatcher {
        fn new(name: &str) -> Self {
            let tool = Arc::new(ToolDef {
                audience: Default::default(),
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

    #[test]
    fn live_tool_dispatch_routes_session_error_through_typed_classifier() {
        // Row 113: the RPC live-tool dispatch seam must route SessionError
        // through the typed `LiveToolDispatchError::from_session_error` owner
        // instead of collapsing every non-NotFound variant into a prose-only
        // `Internal(to_string())`. A `Busy` error must land in the distinct
        // typed `SessionBusy` variant (the class the inline match previously
        // erased), and a `NotFound` must land in `SessionNotFound` — both
        // preserving the terminal class for callers downstream of the live
        // adapter.
        let session_id = SessionId::new();
        let busy = meerkat_live::LiveToolDispatchError::from_session_error(
            &session_id,
            SessionError::Busy {
                id: session_id.clone(),
            },
        );
        assert!(
            matches!(busy, meerkat_live::LiveToolDispatchError::SessionBusy(_)),
            "Busy must map to the typed SessionBusy variant, not Internal"
        );
        let not_found = meerkat_live::LiveToolDispatchError::from_session_error(
            &session_id,
            SessionError::NotFound {
                id: session_id.clone(),
            },
        );
        assert!(matches!(
            not_found,
            meerkat_live::LiveToolDispatchError::SessionNotFound(_)
        ));
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
             but authority-owned configured tool dispatchers must still be exposed"
        );
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
            delivery: meerkat_core::comms::PeerDeliveryOutcome::Queued,
        })
        .expect("well-formed receipt serializes");

        assert_eq!(
            payload["request_id"],
            serde_json::json!(envelope_id.to_string())
        );
        assert_eq!(
            payload["interaction_id"],
            serde_json::json!(interaction_id.0.to_string())
        );
        assert_eq!(payload["delivery"], serde_json::json!("queued"));
    }

    // Gate (#62): a serialization failure on the authoritative reply/stream must
    // surface as a typed JSON-RPC error, never a fabricated success carrying an
    // empty `{}` object or `null`. `send_receipt_json` now returns a `Result`
    // (no `{}` fallback), and `RpcResponse::success` converts a serialize fault
    // into an `INTERNAL_ERROR` response instead of a success-with-null.
    #[test]
    fn gate_serialize_failure_surfaces_typed_error_not_null_success() {
        // A type whose Serialize impl always fails, standing in for a payload
        // that cannot be serialized onto the authoritative reply.
        struct AlwaysFailsToSerialize;
        impl serde::Serialize for AlwaysFailsToSerialize {
            fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
                Err(serde::ser::Error::custom("forced serialization failure"))
            }
        }

        let response =
            RpcResponse::success(Some(crate::protocol::RpcId::Num(7)), AlwaysFailsToSerialize);

        // OLD behavior fabricated a success-with-{}/null; the fixed contract
        // yields a typed error and no result payload at all.
        assert!(
            response.result.is_none(),
            "serialization failure must not fabricate a success result"
        );
        let err = response
            .error
            .expect("serialization failure must surface as a typed JSON-RPC error");
        assert_eq!(err.code, error::INTERNAL_ERROR);
        assert!(
            err.message.contains("serialize"),
            "error message should name the serialization fault, got: {}",
            err.message
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
                runtime_epoch_id: None,
                initial_run_start_holds: std::collections::BTreeSet::new(),
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

    #[cfg(feature = "comms")]
    #[allow(clippy::expect_used)]
    fn test_peer_projection_reconcile_obligation(
        session_id: impl Into<String>,
        local_peer_id: meerkat_core::comms::PeerId,
        endpoints: std::collections::BTreeSet<meerkat_runtime::meerkat_machine::dsl::PeerEndpoint>,
    ) -> (
        Arc<meerkat_runtime::HandleDslAuthority>,
        meerkat_runtime::protocol_comms_trust_reconcile::CommsTrustReconcileObligation,
    ) {
        let session_id = session_id.into();
        let dsl = Arc::new(meerkat_runtime::HandleDslAuthority::ephemeral());
        dsl.apply_signal(
            meerkat_runtime::meerkat_machine::dsl::MeerkatMachineSignal::Initialize,
            "test::initialize",
        )
        .expect("Initialize signal");
        dsl.apply_input(
            meerkat_runtime::meerkat_machine::dsl::MeerkatMachineInput::RegisterSession {
                session_id: meerkat_runtime::meerkat_machine::dsl::SessionId::from(
                    session_id.clone(),
                ),
                runtime_epoch_id: None,
                initial_run_start_holds: std::collections::BTreeSet::new(),
            },
            "test::register_session",
        )
        .expect("RegisterSession input");
        dsl.apply_input(
            meerkat_runtime::meerkat_machine::dsl::MeerkatMachineInput::PrepareBindings {
                agent_runtime_id: meerkat_runtime::meerkat_machine::dsl::AgentRuntimeId::from(
                    format!("test-runtime-{session_id}"),
                ),
                fence_token: meerkat_runtime::meerkat_machine::dsl::FenceToken::from(0),
                generation: Some(meerkat_runtime::meerkat_machine::dsl::Generation::from(0)),
                runtime_epoch_id: None,
                session_id: meerkat_runtime::meerkat_machine::dsl::SessionId::from(session_id),
            },
            "test::prepare_bindings",
        )
        .expect("PrepareBindings input");
        dsl.apply_input(
            meerkat_runtime::meerkat_machine::dsl::MeerkatMachineInput::PublishLocalEndpoint {
                endpoint: meerkat_runtime::meerkat_machine::dsl::PeerEndpoint::new(
                    "local",
                    local_peer_id.to_string(),
                    "inproc://local",
                    [0x7f; 32],
                ),
            },
            "test::publish_local_endpoint",
        )
        .expect("PublishLocalEndpoint input");
        let transition = dsl
            .apply_input_with_transition(
                meerkat_runtime::meerkat_machine::dsl::MeerkatMachineInput::ApplyMobPeerOverlay {
                    epoch: 1,
                    endpoints,
                },
                "test::apply_mob_peer_overlay",
            )
            .expect("ApplyMobPeerOverlay input");
        let mut obligations =
            meerkat_runtime::protocol_comms_trust_reconcile::extract_obligations_with_freshness(
                &transition,
                dsl.peer_projection_freshness_authority(),
            );
        assert_eq!(
            obligations.len(),
            1,
            "test reconcile effect must produce one generated obligation"
        );
        (dsl, obligations.pop().expect("obligation count checked"))
    }

    #[cfg(feature = "comms")]
    async fn add_generated_peer_projection_trust(
        runtime: &meerkat::CommsRuntime,
        peer: TrustedPeerDescriptor,
        context: &'static str,
    ) {
        let endpoint = meerkat_runtime::meerkat_machine::dsl::PeerEndpoint::from(&peer);
        let (dsl, obligation) = test_peer_projection_reconcile_obligation(
            "rpc-router-test-comms-reconcile",
            runtime
                .peer_id()
                .unwrap_or_else(|| panic!("{context}: runtime peer_id unavailable")),
            std::collections::BTreeSet::from([endpoint.clone()]),
        );
        meerkat_runtime::RuntimePeerCommsHandle::install_generated_on(dsl, runtime)
            .expect("install generated peer-comms handle");
        CoreCommsRuntime::apply_trust_mutation(
            runtime,
            meerkat_core::comms::CommsTrustMutation::AddTrustedPeer {
                authority: meerkat_runtime::protocol_comms_trust_reconcile::authority_for_endpoint(
                    &obligation,
                    &endpoint,
                )
                .expect("generated peer projection add authority"),
                peer,
            },
        )
        .await
        .unwrap_or_else(|error| panic!("{context}: {error}"));
    }

    #[cfg(feature = "comms")]
    async fn stage_generated_peer_projection_trust(
        adapter: Arc<meerkat_runtime::MeerkatMachine>,
        session_id: &meerkat_core::SessionId,
        runtime: Arc<dyn CoreCommsRuntime>,
        peer: TrustedPeerDescriptor,
        context: &'static str,
    ) {
        adapter
            .stage_add_direct_peer_endpoint(
                session_id,
                meerkat_runtime::meerkat_machine::dsl::PeerEndpoint::from(&peer),
                runtime,
            )
            .await
            .unwrap_or_else(|error| panic!("{context}: {error}"));
    }

    // -----------------------------------------------------------------------
    // Mock LLM client (same as session_runtime tests)
    // -----------------------------------------------------------------------

    struct MockLlmClient {
        provider: meerkat_core::Provider,
    }

    impl MockLlmClient {
        fn anthropic() -> Self {
            Self {
                provider: meerkat_core::Provider::Anthropic,
            }
        }

        fn openai() -> Self {
            Self {
                provider: meerkat_core::Provider::OpenAI,
            }
        }
    }

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
            request: &'a meerkat_client::LlmRequest,
        ) -> Pin<
            Box<dyn futures::Stream<Item = Result<meerkat_client::LlmEvent, LlmError>> + Send + 'a>,
        > {
            Box::pin(stream::iter(vec![
                Ok(meerkat_client::LlmEvent::TextDelta {
                    delta: "Hello from mock".to_string(),
                    meta: None,
                }),
                Ok(meerkat_client::LlmEvent::UsageUpdate {
                    usage: meerkat_core::TurnUsage::host_declared(
                        self.provider,
                        &request.model,
                        meerkat_core::Usage::default(),
                    ),
                }),
                Ok(meerkat_client::LlmEvent::Done {
                    outcome: meerkat_client::LlmDoneOutcome::success(StopReason::EndTurn),
                }),
            ]))
        }

        fn provider(&self) -> meerkat_core::Provider {
            self.provider
        }

        async fn health_check(&self) -> Result<(), LlmError> {
            Ok(())
        }
    }

    struct RecordingMockLlmClient {
        requests: Arc<std::sync::Mutex<Vec<Vec<Message>>>>,
        tool_names: Option<Arc<std::sync::Mutex<Vec<Vec<String>>>>>,
        delay_ms: Option<u64>,
    }

    impl RecordingMockLlmClient {
        fn new(requests: Arc<std::sync::Mutex<Vec<Vec<Message>>>>) -> Self {
            Self {
                requests,
                tool_names: None,
                delay_ms: None,
            }
        }

        fn with_delay(requests: Arc<std::sync::Mutex<Vec<Vec<Message>>>>, delay_ms: u64) -> Self {
            Self {
                requests,
                tool_names: None,
                delay_ms: Some(delay_ms),
            }
        }

        /// Also record the names of the tools each request offers.
        fn with_tool_names(
            requests: Arc<std::sync::Mutex<Vec<Vec<Message>>>>,
            tool_names: Arc<std::sync::Mutex<Vec<Vec<String>>>>,
        ) -> Self {
            Self {
                requests,
                tool_names: Some(tool_names),
                delay_ms: None,
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
            if let Some(tool_names) = &self.tool_names {
                tool_names
                    .lock()
                    .expect("recorded tool names lock poisoned")
                    .push(
                        request
                            .tools
                            .iter()
                            .map(|tool| tool.name.to_string())
                            .collect(),
                    );
            }
            let delay = self.delay_ms;
            let usage = meerkat_core::TurnUsage::host_declared(
                meerkat_core::Provider::Anthropic,
                &request.model,
                meerkat_core::Usage::default(),
            );
            Box::pin(stream::unfold(0u8, move |state| {
                let usage = usage.clone();
                async move {
                    match state {
                        0 => {
                            if let Some(delay_ms) = delay {
                                tokio::time::sleep(std::time::Duration::from_millis(delay_ms))
                                    .await;
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
                            Ok(meerkat_client::LlmEvent::UsageUpdate {
                                usage: usage.clone(),
                            }),
                            2,
                        )),
                        2 => Some((
                            Ok(meerkat_client::LlmEvent::Done {
                                outcome: meerkat_client::LlmDoneOutcome::success(
                                    StopReason::EndTurn,
                                ),
                            }),
                            3,
                        )),
                        _ => None,
                    }
                }
            }))
        }

        fn provider(&self) -> meerkat_core::Provider {
            meerkat_core::Provider::Anthropic
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

    fn anthropic_test_config() -> Config {
        let mut config = Config::default();
        config.agent.model = "claude-sonnet-4-5".to_string();
        config
    }

    fn openai_live_test_config() -> Config {
        let mut config = Config::default();
        config.agent.model = "gpt-realtime-2".to_string();
        config
    }

    fn runtime_backed_persistence(
        store: Arc<dyn meerkat::SessionStore>,
    ) -> meerkat::PersistenceBundle {
        let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> =
            Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());
        meerkat::PersistenceBundle::new(store, runtime_store, memory_blob_store())
            .expect("construct runtime authority")
    }

    /// Config store whose `get()` always returns a typed fault, used to prove
    /// that config-read handlers propagate `ConfigError` instead of laundering
    /// it into `Config::default()`.
    struct FailingConfigStore;

    #[async_trait]
    impl ConfigStore for FailingConfigStore {
        async fn get(&self) -> Result<Config, meerkat_core::ConfigError> {
            Err(meerkat_core::ConfigError::Validation("boom".to_string()))
        }

        async fn set(&self, _config: Config) -> Result<(), meerkat_core::ConfigError> {
            Err(meerkat_core::ConfigError::Validation("boom".to_string()))
        }

        async fn patch(
            &self,
            _delta: meerkat_core::ConfigDelta,
        ) -> Result<Config, meerkat_core::ConfigError> {
            Err(meerkat_core::ConfigError::Validation("boom".to_string()))
        }
    }

    async fn test_router() -> (MethodRouter, mpsc::Receiver<RpcNotification>) {
        let config = anthropic_test_config();
        test_router_with_config_store(Arc::new(MemoryConfigStore::new(
            config,
            meerkat_models::canonical(),
        )))
        .await
    }

    async fn test_router_with_config_store(
        config_store: Arc<dyn ConfigStore>,
    ) -> (MethodRouter, mpsc::Receiver<RpcNotification>) {
        let temp = tempfile::tempdir().unwrap();
        let factory = AgentFactory::new(temp.path().join("sessions"));
        let config = anthropic_test_config();
        let store: Arc<dyn meerkat::SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let runtime = SessionRuntime::new(
            factory,
            config,
            10,
            runtime_backed_persistence(store),
            NotificationSink::noop(),
        )
        .expect("compose the RPC session runtime");
        runtime.set_default_llm_client(Some(Arc::new(MockLlmClient::anthropic())));
        runtime.set_config_runtime(Arc::new(ConfigRuntime::new(
            Arc::clone(&config_store),
            temp.path().join("config_state.json"),
        )));
        let runtime = Arc::new(runtime);
        let (notif_tx, notif_rx) = mpsc::channel(100);
        let sink = NotificationSink::new(notif_tx);
        let router =
            MethodRouter::new(runtime, config_store, sink).expect("construct runtime authority");
        (router, notif_rx)
    }

    async fn test_router_with_v9_runtime() -> (MethodRouter, mpsc::Receiver<RpcNotification>) {
        test_router().await
    }

    async fn test_router_with_v9_runtime_and_max_sessions(
        max_sessions: usize,
    ) -> (MethodRouter, mpsc::Receiver<RpcNotification>) {
        let temp = tempfile::tempdir().unwrap();
        let factory = AgentFactory::new(temp.path().join("sessions"));
        let mut config = anthropic_test_config();
        config.limits.max_sessions = Some(max_sessions);
        let store: Arc<dyn meerkat::SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let runtime = SessionRuntime::new(
            factory,
            config.clone(),
            max_sessions,
            runtime_backed_persistence(store),
            NotificationSink::noop(),
        )
        .expect("compose the RPC session runtime");
        let config_store: Arc<dyn ConfigStore> =
            Arc::new(MemoryConfigStore::new(config, meerkat_models::canonical()));
        runtime.set_default_llm_client(Some(Arc::new(MockLlmClient::anthropic())));
        runtime.set_config_runtime(Arc::new(ConfigRuntime::new(
            Arc::clone(&config_store),
            temp.path().join("config_state.json"),
        )));
        let runtime = Arc::new(runtime);
        let (notif_tx, notif_rx) = mpsc::channel(100);
        let sink = NotificationSink::new(notif_tx);
        let router =
            MethodRouter::new(runtime, config_store, sink).expect("construct runtime authority");
        (router, notif_rx)
    }

    #[tokio::test]
    async fn capabilities_get_propagates_config_fault() {
        let (router, _rx) = test_router_with_config_store(Arc::new(FailingConfigStore)).await;
        let response = router
            .dispatch(RpcRequest {
                jsonrpc: "2.0".to_string(),
                method: "capabilities/get".to_string(),
                params: None,
                id: Some(RpcId::Num(1)),
            })
            .await
            .expect("response");
        let value = serde_json::to_value(response).expect("response serializes");
        assert!(
            value["result"].is_null(),
            "capabilities/get must not return a success payload on a config fault: {value}"
        );
        assert_eq!(
            value["error"]["code"],
            error::INTERNAL_ERROR,
            "capabilities/get must surface INTERNAL_ERROR on a config fault: {value}"
        );
    }

    #[tokio::test]
    async fn models_catalog_propagates_config_fault() {
        let (router, _rx) = test_router_with_config_store(Arc::new(FailingConfigStore)).await;
        let response = router
            .dispatch(RpcRequest {
                jsonrpc: "2.0".to_string(),
                method: "models/catalog".to_string(),
                params: None,
                id: Some(RpcId::Num(1)),
            })
            .await
            .expect("response");
        let value = serde_json::to_value(response).expect("response serializes");
        assert!(
            value["result"].is_null(),
            "models/catalog must not return a success payload on a config fault: {value}"
        );
        assert_eq!(
            value["error"]["code"],
            error::INTERNAL_ERROR,
            "models/catalog must surface INTERNAL_ERROR on a config fault: {value}"
        );
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

        // `mob/claim_callback_route` is a method name, not a topology claim:
        // it is the one catalog name allowed past the token scan.
        let mut projection = result.clone();
        let methods = projection["endpoints"]["rpc_methods"]
            .as_array_mut()
            .expect("the projection lists the callable methods");
        let listed = methods.len();
        methods.retain(|method| method != "mob/claim_callback_route");
        assert_eq!(
            methods.len() + 1,
            listed,
            "the claim method is listed once: {result}"
        );
        let text = serde_json::to_string(&projection).expect("result serializes");
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
            capabilities["result"]["features"]["multi_host_mobs"],
            cfg!(feature = "mob"),
            "runtime/capabilities must advertise the multi-host mob surface when compiled"
        );
        assert_eq!(capabilities["result"]["features"]["durable_jobs"], true);
        assert_eq!(
            capabilities["result"]["features"]["secure_remote_rpc"],
            false
        );
        assert_eq!(capabilities["result"]["features"]["approvals"], false);
        assert_eq!(health["result"]["checks"]["jobs"], "ok");
        // Nothing is wrong with this host and every measurement says so, so
        // the rollup is `ok`. Every declared dimension is measured on this
        // surface; the remaining coverage boundary (mid-turn progress, which
        // has no declared dimension yet) is documented on `runtime_health`.
        assert_eq!(
            health["result"]["status"], "ok",
            "a host with no measured fault must not stand permanently amber: {health}"
        );
    }

    /// `runtime/host_info` must publish the health THIS surface measured, not
    /// the projection default.
    ///
    /// `meerkat::surface::build_runtime_host_info` is a pure projection over
    /// surface options and config metadata: it holds no runtime and can observe
    /// no health dimension, so the rollup it embeds is the probed-nothing one -
    /// `degraded` with every declared dimension named `unmeasured:*`. That is
    /// the correct default for a required wire field a pure projection cannot
    /// compute, because a caller that forgets to replace it ships a visibly
    /// incomplete payload rather than a silent `ok`. But `host_info` is not
    /// allowed to SHIP that default from a surface that can probe, and this
    /// endpoint can.
    ///
    /// Byte equality against `runtime/health`, not a spot check on `status`, so
    /// a partially-wired `host_info` fails here too. This is the assertion that
    /// would have caught the defect it exists for: `handle_info` inheriting
    /// whatever `build_runtime_host_health` happened to return.
    #[tokio::test]
    async fn runtime_host_info_publishes_the_health_this_surface_measured() {
        let (router, _rx) = test_router_with_v9_runtime().await;
        let info = serde_json::to_value(
            router
                .dispatch(RpcRequest {
                    jsonrpc: "2.0".to_string(),
                    method: "runtime/host_info".to_string(),
                    params: None,
                    id: Some(RpcId::Num(17)),
                })
                .await
                .expect("host_info response"),
        )
        .expect("serialize host_info");
        let health = serde_json::to_value(
            router
                .dispatch(RpcRequest {
                    jsonrpc: "2.0".to_string(),
                    method: "runtime/health".to_string(),
                    params: None,
                    id: Some(RpcId::Num(19)),
                })
                .await
                .expect("health response"),
        )
        .expect("serialize health");

        assert_eq!(
            info["result"]["health"], health["result"],
            "runtime/host_info must embed the measured health, not the \
             probed-nothing projection: {info}"
        );
        // Named separately so the equality above cannot be satisfied by both
        // sides being the default. `session_durability` is only ever published
        // by a probe that ran.
        assert_eq!(
            info["result"]["health"]["checks"]["session_durability"], "ok",
            "the embedded health must carry a MEASURED dimension: {info}"
        );
        assert!(
            info["result"]["health"]["checks"]
                .get("unmeasured:session_durability")
                .is_none(),
            "a measured dimension must not also be published as unmeasured: {info}"
        );
        assert_eq!(
            info["result"]["health"]["status"], "ok",
            "and a host with no measured fault must not stand permanently \
             amber on this route either: {info}"
        );
    }

    /// `runtime/health` may only assert the scope it measured, and must assert
    /// the scope it did.
    ///
    /// The payload is produced by dispatching the real method against the real
    /// router; the expectations below are literals, not values re-derived from
    /// the same production constant, so dropping either the measured or the
    /// unmeasured entries fails this test instead of quietly agreeing with it.
    #[tokio::test]
    async fn runtime_health_names_unmeasured_dimensions_and_rolls_up_measured_ones() {
        let (router, _rx) = test_router_with_v9_runtime().await;
        let response = router
            .dispatch(RpcRequest {
                jsonrpc: "2.0".to_string(),
                method: "runtime/health".to_string(),
                params: None,
                id: Some(RpcId::Num(7)),
            })
            .await
            .expect("health response");
        let health = serde_json::to_value(response).expect("serialize health");
        let result = &health["result"];
        let checks = result["checks"]
            .as_object()
            .expect("health payload carries a checks map");

        // Measured. The four session dimensions are read off live runtime
        // state, so their presence here is the wiring pin: a handler that
        // forgot to insert them would publish `unmeasured:*` instead and fail
        // on the next assertion block. `session_liveness` earning its place in
        // this list is the point of the lane-truth probe: 0.8.23 shipped it
        // honestly unmeasured, and the field promptly produced a five-day
        // outage in exactly that class.
        for measured in [
            "jobs",
            "session_durability",
            "session_runtime_loop",
            "session_run_start",
            "session_liveness",
        ] {
            assert_eq!(
                checks.get(measured).and_then(serde_json::Value::as_str),
                Some("ok"),
                "runtime/health must publish `{measured}` as measured: {result}"
            );
            assert!(
                !checks.contains_key(&format!("unmeasured:{measured}")),
                "a measured dimension must not also be published as unmeasured: {result}"
            );
        }

        // Every declared dimension is measured on this surface, so no
        // `unmeasured:*` key may remain: a leftover one would mean a probe was
        // wired without clearing its coverage claim, or a dimension was
        // declared without a probe.
        assert!(
            !checks.keys().any(|key| key.starts_with("unmeasured:")),
            "every declared dimension is probed on this surface: {result}"
        );

        // The third coverage vocabulary, absent here on purpose. On a readable
        // host both session probes return a reading, so neither may publish a
        // failed read. This is the assertion that would catch a probe wired to
        // report `unreadable:*` unconditionally.
        assert!(
            !checks.keys().any(|key| key.starts_with("unreadable:")),
            "no probe failed on this host, so none may claim it did: {result}"
        );

        // The rollup speaks for the dimensions this handler ATTEMPTED. A host
        // with no fault and no failed read reports `ok`; a permanently amber
        // endpoint is a muted endpoint, and a muted endpoint is why this whole
        // projection exists.
        assert_eq!(
            result["status"], "ok",
            "unmeasured coverage is published, not folded into the rollup: {result}"
        );
        let worst_measured = [
            "jobs",
            "session_durability",
            "session_runtime_loop",
            "session_run_start",
            "session_liveness",
        ]
        .into_iter()
        .filter_map(|key| checks.get(key).and_then(serde_json::Value::as_str))
        .map(|status| match status {
            "ok" => 0u8,
            "degraded" => 1,
            _ => 2,
        })
        .max()
        .expect("at least one measured check");
        assert_eq!(
            result["status"].as_str(),
            Some(match worst_measured {
                0 => "ok",
                1 => "degraded",
                _ => "unhealthy",
            }),
            "status must equal the worst rung present among the MEASURED checks: {result}"
        );
    }

    /// The alert-fires pin, and the only test in the tree that fails if the
    /// handler stops inserting `session_durability`.
    ///
    /// A real session is created through `session/create`, then its shared
    /// durability gate is degraded through the machine's own deterministic
    /// seam - no fabricated store outcome, no hand-built handle. The endpoint
    /// must move off `ok` on the strength of that, because an operator alert
    /// built on `status != "ok"` is exactly what this release is asking them to
    /// write.
    #[tokio::test]
    async fn runtime_health_turns_amber_when_a_real_session_needs_a_durability_reload() {
        let (router, _rx) = test_router_with_v9_runtime().await;
        let create = router
            .dispatch(make_request(
                "session/create",
                serde_json::json!({ "prompt": "hello" }),
            ))
            .await
            .expect("session/create response");
        let created = result_value(&create);
        let session_id = meerkat_core::SessionId::parse(
            created["session_id"]
                .as_str()
                .expect("session/create returns a session id"),
        )
        .expect("session id parses");

        let health_request = || RpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "runtime/health".to_string(),
            params: None,
            id: Some(RpcId::Num(11)),
        };
        let healthy = serde_json::to_value(
            router
                .dispatch(health_request())
                .await
                .expect("health response"),
        )
        .expect("serialize health");
        assert_eq!(
            healthy["result"]["checks"]["session_durability"], "ok",
            "a freshly created session is durability-ready: {healthy}"
        );
        assert_eq!(
            healthy["result"]["status"], "ok",
            "the pin is worthless unless the endpoint was ok before the fault: {healthy}"
        );

        assert!(
            router
                .runtime_adapter
                .force_session_durability_reload_required_for_test(&session_id)
                .await,
            "the created session must own a real persistent durability gate"
        );

        let degraded = serde_json::to_value(
            router
                .dispatch(health_request())
                .await
                .expect("health response"),
        )
        .expect("serialize health");
        assert_eq!(
            degraded["result"]["checks"]["session_durability"], "degraded",
            "a session that needs a cold reload must be visible in `checks`: {degraded}"
        );
        assert_eq!(
            degraded["result"]["status"], "degraded",
            "a measured fault must reach the rollup an operator alerts on: {degraded}"
        );
        assert_eq!(
            degraded["result"]["checks"]["jobs"], "ok",
            "the fault must not smear across dimensions that are still fine: {degraded}"
        );
    }

    /// The `session_runtime_loop` alert-fires pin: a real session with a real
    /// spawned runtime loop is driven to a dead loop, and the endpoint's own
    /// payload must move that key to `degraded`.
    ///
    /// Two fixture choices carry the test and both are deliberate.
    ///
    /// `ensure_runtime_executor` is the production spawn path - the router
    /// itself calls it before any turn - so the loop being killed is the loop a
    /// real session would have, with a real executor and real channels, not a
    /// hand-built attachment.
    ///
    /// The durability fault is injected FIRST, and not as decoration: killing a
    /// runtime loop normally wakes the teardown watcher, which retires the shell
    /// and leaves nothing to detect - that is the system working. The state
    /// worth alerting on is the one the watcher cannot clear, because
    /// `observe_runtime_loop_teardown` refuses a session whose durability gate
    /// demands a cold reload. That leaves a registered session pointing at a
    /// dead loop indefinitely, which is a real production wedge.
    ///
    /// The consequence, stated rather than papered over: `status` is already
    /// `degraded` from the durability fault by the time the loop dies, so this
    /// test cannot pin a `status` transition attributable to the loop dimension
    /// alone. What it does pin is that the loop dimension is measured, is `ok`
    /// while the loop is alive - including while the durability gate is already
    /// degraded, so the key demonstrably does not borrow its sibling's verdict -
    /// and flips to `degraded` when and only when the loop actually dies. The
    /// "an ok->degraded transition on this dimension alone" property is pinned
    /// one level down, by
    /// `dead_runtime_loop_session_count_sees_a_real_corpse_and_not_a_clean_shutdown`
    /// in meerkat-runtime, which observes `Some(0)` with a degraded durability
    /// gate and `Some(1)` after the abort.
    #[tokio::test]
    async fn runtime_health_turns_amber_when_a_real_session_runtime_loop_dies() {
        let (router, _rx) = test_router_with_v9_runtime().await;
        let create = router
            .dispatch(make_request(
                "session/create",
                serde_json::json!({ "prompt": "hello" }),
            ))
            .await
            .expect("session/create response");
        let created = result_value(&create);
        let session_id = meerkat_core::SessionId::parse(
            created["session_id"]
                .as_str()
                .expect("session/create returns a session id"),
        )
        .expect("session id parses");

        let health_request = || RpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "runtime/health".to_string(),
            params: None,
            id: Some(RpcId::Num(13)),
        };
        let health_now = || async {
            serde_json::to_value(
                router
                    .dispatch(health_request())
                    .await
                    .expect("health response"),
            )
            .expect("serialize health")
        };

        // Spawn the real runtime loop the way a turn would.
        router
            .runtime
            .ensure_runtime_executor(&session_id)
            .await
            .expect("the production spawn path must attach a runtime loop");
        // Wait until that loop is parked with no run in flight: its startup
        // recovery and any queued work are done, so nothing will take the
        // session gate after the durability fault below. A loop that takes
        // the gate of a degraded session detaches its own attachment, which
        // would leave the abort below with no task to kill.
        let mut parked = router.runtime_adapter.runtime_loop_parked();
        loop {
            tokio::time::timeout(
                std::time::Duration::from_secs(30),
                parked.wait_for(|sessions| sessions.contains(&session_id)),
            )
            .await
            .expect("the spawned runtime loop parks")
            .expect("runtime-loop park watch");
            if router
                .runtime_adapter
                .current_run(&session_id)
                .await
                .is_none()
            {
                break;
            }
            router
                .runtime_adapter
                .wait_current_run_settled(&session_id)
                .await;
        }

        let live = health_now().await;
        assert_eq!(
            live["result"]["checks"]["session_runtime_loop"], "ok",
            "a session with a live spawned runtime loop is not a corpse: {live}"
        );
        assert_eq!(
            live["result"]["status"], "ok",
            "the pin is worthless unless the endpoint was ok before the fault: {live}"
        );

        // Block the teardown watcher through a real refusal, then prove the
        // refusal alone does not move the loop dimension. Without this
        // assertion the final one would not mean the loop died; it would only
        // mean something went wrong somewhere.
        assert!(
            router
                .runtime_adapter
                .force_session_durability_reload_required_for_test(&session_id)
                .await,
            "the created session must own a real persistent durability gate"
        );
        let durability_only = health_now().await;
        assert_eq!(
            durability_only["result"]["checks"]["session_runtime_loop"], "ok",
            "a degraded durability gate is a different dimension; the loop key \
             must not borrow its verdict: {durability_only}"
        );
        assert_eq!(
            durability_only["result"]["checks"]["session_durability"], "degraded",
            "the durability fault is real, or the teardown watcher is not \
             actually blocked: {durability_only}"
        );

        // Kill the loop task without cooperative cleanup: a task panic or an
        // early runtime-loop return, modelled by the machine's own fault
        // injector, which returns only after the task has genuinely exited.
        assert!(
            router
                .runtime_adapter
                .test_abort_runtime_loop_without_cleanup(&session_id)
                .await
                .expect("abort the runtime loop without cooperative cleanup"),
            "the fixture must have had a real runtime-loop task to abort"
        );

        let dead = health_now().await;
        assert_eq!(
            dead["result"]["checks"]["session_runtime_loop"], "degraded",
            "a registered session still claiming a dead runtime loop must be \
             visible in `checks`: {dead}"
        );
        assert_eq!(
            dead["result"]["status"], "degraded",
            "and the endpoint an operator alerts on must not read ok: {dead}"
        );
        assert_eq!(
            dead["result"]["checks"]["jobs"], "ok",
            "the fault must not smear across dimensions that are still fine: {dead}"
        );
    }

    #[tokio::test]
    async fn jobs_surface_is_realm_scoped_and_reports_healthy_queued_work() {
        let (router, _rx) = test_router().await;
        router.runtime.set_realm_context(
            Some(meerkat_core::connection::RealmId::parse("realm-a").unwrap()),
            None,
            None,
        );
        let session_id = meerkat_core::SessionId::new();
        let service = router.runtime.detached_job_service();
        let receipt = service
            .submit(meerkat::JobSpec::new(
                "realm-a",
                session_id,
                meerkat::ExecutionIntentId::from_string("intent-rpc").unwrap(),
                meerkat::InteractionLineageId::from_string("lineage-rpc").unwrap(),
                meerkat::ToolIdentity::new("scan", "v1").unwrap(),
                meerkat::RunnerIdentity::new("meta.callback", "v1").unwrap(),
                meerkat::RestartClass::NonResumable,
                meerkat::CanonicalArgumentsHash::new("sha256:args").unwrap(),
                meerkat::JobSubmissionKey::new("rpc-job").unwrap(),
            ))
            .await
            .unwrap();
        let other = service
            .submit(meerkat::JobSpec::new(
                "realm-b",
                meerkat_core::SessionId::new(),
                meerkat::ExecutionIntentId::from_string("intent-other").unwrap(),
                meerkat::InteractionLineageId::from_string("lineage-other").unwrap(),
                meerkat::ToolIdentity::new("scan", "v1").unwrap(),
                meerkat::RunnerIdentity::new("meta.callback", "v1").unwrap(),
                meerkat::RestartClass::NonResumable,
                meerkat::CanonicalArgumentsHash::new("sha256:other").unwrap(),
                meerkat::JobSubmissionKey::new("rpc-job-other").unwrap(),
            ))
            .await
            .unwrap();

        let get = router
            .dispatch(make_request(
                "jobs/get",
                serde_json::json!({"job_id": receipt.job_id.to_string()}),
            ))
            .await
            .expect("response");
        let get = result_value(&get);
        assert_eq!(get["job"]["phase"], "queued");
        assert!(get["job"].get("fence").is_none());

        let health = router
            .dispatch(make_request_no_params("jobs/health"))
            .await
            .expect("response");
        let health = result_value(&health);
        assert_eq!(health["detached_jobs"]["status"], "ok");
        assert_eq!(health["detached_jobs"]["queued"], 1);

        let cross_realm = router
            .dispatch(make_request(
                "jobs/get",
                serde_json::json!({"job_id": other.job_id.to_string()}),
            ))
            .await
            .expect("response");
        assert!(cross_realm.result.is_none());
        assert_eq!(
            cross_realm.error.as_ref().map(|error| error.code),
            Some(error::INVALID_PARAMS)
        );
    }

    #[tokio::test]
    async fn approval_request_get_list_and_decide_round_trip() {
        let (router, _rx) = test_router().await;
        let request = make_request(
            "approval/request",
            serde_json::json!({
                "requester": "human:alice",
                "owner": {"owner_type": "session", "session_id": "00000000-0000-0000-0000-000000000001"},
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

    async fn pending_runtime_approval(router: &MethodRouter) -> String {
        let created = router
            .dispatch(make_request(
                "approval/request",
                serde_json::json!({
                    "requester": "human:alice",
                    "owner": {"owner_type": "runtime"},
                    "resource": {"kind": "runtime", "id": "local"},
                    "proposed_action": {"kind": "other", "summary": "manual gate"},
                    "risk": "medium",
                    "allowed_decisions": ["approve", "deny"]
                }),
            ))
            .await
            .expect("request response");
        result_value(&created)["approval_id"]
            .as_str()
            .expect("approval id")
            .to_string()
    }

    async fn approval_status(router: &MethodRouter, approval_id: &str) -> serde_json::Value {
        let fetched = router
            .dispatch(make_request(
                "approval/get",
                serde_json::json!({"approval_id": approval_id}),
            ))
            .await
            .expect("get response");
        result_value(&fetched)["status"].clone()
    }

    /// An event the host adapter authenticated: the human, its login and
    /// the exact approval and decision it made.
    fn consent_event(
        approval_id: &str,
        decision: meerkat_core::ApprovalDecision,
    ) -> meerkat_core::approval::consent::AuthenticatedConsentEvent {
        use meerkat_core::approval::consent::{
            AuthenticatedApprover, AuthenticatedConsentDecision, AuthenticatedConsentEvent,
            HostAuthenticationScope,
        };
        let approver = AuthenticatedApprover::new(
            meerkat_core::PrincipalRef::in_domain(
                meerkat_core::PrincipalKind::Human,
                "lead-1",
                meerkat_core::TrustDomainId::new("team").expect("domain"),
            )
            .expect("principal"),
            meerkat_core::ApprovalAssurance::PersonAuthenticated,
        )
        .expect("qualified approver")
        .with_authentication_scope(HostAuthenticationScope::new("login-1"));
        AuthenticatedConsentEvent::new(
            approval_id.parse().expect("approval id"),
            AuthenticatedConsentDecision {
                approver,
                decision,
                reason: None,
            },
        )
    }

    /// Plain dispatch has no route to the consent decision method: caller
    /// JSON cannot decide native consent, and nothing changes.
    #[tokio::test]
    async fn plain_dispatch_cannot_decide_native_consent() {
        let (router, _rx) = test_router().await;
        let approval_id = pending_runtime_approval(&router).await;
        let refused = router
            .dispatch(make_request(
                CONSENT_DECIDE_METHOD,
                serde_json::json!({"approval_id": approval_id, "decision": "approve"}),
            ))
            .await
            .expect("consent decide response");
        assert_eq!(
            refused.error.as_ref().map(|error| error.code),
            Some(error::METHOD_NOT_FOUND)
        );
        assert!(refused.result.is_none());
        assert_eq!(approval_status(&router, &approval_id).await, "pending");
    }

    /// An authenticated event decides only the request it binds: another
    /// decision, another approval or another method is refused before the
    /// approval owner is asked, and nothing changes.
    #[tokio::test]
    async fn a_consent_event_decides_only_the_request_it_binds() {
        let (router, _rx) = test_router().await;
        let approval_id = pending_runtime_approval(&router).await;
        let other_id = pending_runtime_approval(&router).await;
        let approve = || consent_event(&approval_id, meerkat_core::ApprovalDecision::Approve);
        for (method, params, code) in [
            (
                CONSENT_DECIDE_METHOD,
                serde_json::json!({"approval_id": approval_id, "decision": "deny"}),
                error::INVALID_PARAMS,
            ),
            (
                CONSENT_DECIDE_METHOD,
                serde_json::json!({"approval_id": other_id, "decision": "approve"}),
                error::INVALID_PARAMS,
            ),
            (
                "approval/decide",
                serde_json::json!({"approval_id": approval_id, "decision": "approve"}),
                error::METHOD_NOT_FOUND,
            ),
        ] {
            let refused = router
                .dispatch_with_consent_event(make_request(method, params.clone()), approve())
                .await;
            assert_eq!(
                refused.error.as_ref().map(|error| error.code),
                Some(code),
                "{method} {params}"
            );
        }
        let mut wrong_version = make_request(
            CONSENT_DECIDE_METHOD,
            serde_json::json!({"approval_id": approval_id, "decision": "approve"}),
        );
        wrong_version.jsonrpc = "1.0".to_string();
        let refused = router
            .dispatch_with_consent_event(wrong_version, approve())
            .await;
        assert_eq!(
            refused.error.as_ref().map(|error| error.code),
            Some(error::INVALID_REQUEST),
            "an unsupported envelope version is refused before the handler"
        );
        assert_eq!(approval_status(&router, &approval_id).await, "pending");
        assert_eq!(approval_status(&router, &other_id).await, "pending");
    }

    /// Request parameters never carry authority, even beside an
    /// authenticated event: an approver, actor, assurance, login or reason
    /// field is an invalid parameter, not an identity.
    #[tokio::test]
    async fn consent_decide_params_cannot_carry_authority() {
        let (router, _rx) = test_router().await;
        let approval_id = pending_runtime_approval(&router).await;
        for smuggled in [
            serde_json::json!({"actor": "human:bob"}),
            serde_json::json!({"approver": {"principal": "human:bob", "assurance": "person_authenticated"}}),
            serde_json::json!({"assurance": "person_authenticated"}),
            serde_json::json!({"authentication_scope": "login-2"}),
            serde_json::json!({"reason": "approved by bob"}),
        ] {
            let mut params = serde_json::json!({"approval_id": approval_id, "decision": "approve"});
            for (key, value) in smuggled.as_object().expect("object") {
                params[key] = value.clone();
            }
            let refused = router
                .dispatch_with_consent_event(
                    make_request(CONSENT_DECIDE_METHOD, params),
                    consent_event(&approval_id, meerkat_core::ApprovalDecision::Approve),
                )
                .await;
            assert_eq!(
                refused.error.as_ref().map(|error| error.code),
                Some(error::INVALID_PARAMS),
                "{smuggled}"
            );
        }
        assert_eq!(approval_status(&router, &approval_id).await, "pending");
    }

    /// A matching event reaches the runtime's approval owner, which decides
    /// only consent it issued and holds: an ordinary approval has no consent
    /// candidate, so the owner refuses and the approval stays pending. A
    /// native R3 candidate retained by that same approval service is
    /// approved by the event, which executes nothing; one explicit retained
    /// attempt then enters exactly once, and a reuse or a second decision is
    /// refused without entering.
    ///
    /// Its scope is the router's composition with the approval owner. The
    /// candidate is dispatched under the default `ToolDispatchContext`,
    /// which carries no session or run identity, and its work and approver
    /// eligibility are trusted fixtures; the event is built here, as an
    /// authenticated host would supply it. It is not a runtime-admitted run,
    /// a stock authorization-policy test or a host-authentication test.
    #[tokio::test]
    async fn a_matching_consent_event_reaches_the_approval_owner() {
        let (router, _rx) = test_router().await;
        let approval_id = pending_runtime_approval(&router).await;
        let refused = router
            .dispatch_with_consent_event(
                make_request(
                    CONSENT_DECIDE_METHOD,
                    serde_json::json!({"approval_id": approval_id, "decision": "approve"}),
                ),
                consent_event(&approval_id, meerkat_core::ApprovalDecision::Approve),
            )
            .await;
        let error = refused.error.as_ref().expect("refused by the owner");
        assert_eq!(error.code, error::INVALID_PARAMS);
        assert!(error.message.contains("consent"), "{}", error.message);
        assert_eq!(approval_status(&router, &approval_id).await, "pending");

        // The positive native route, on the router's own approval service,
        // with trusted fixture work and eligibility and no session or run.
        let route = Arc::new(TeamConsentRoute::default());
        let review = Arc::new(
            meerkat_core::approval::review::BoundOperationReview::new(
                Arc::new(NoModelReview),
                router.runtime.approval_service(),
                Duration::from_secs(30),
            )
            .with_human_consent(route.clone(), Duration::from_secs(4 * 60 * 60)),
        );
        let context = meerkat_core::ToolDispatchContext::default()
            .with_work_authorization(Some(meerkat_core::WorkAuthorizationContext::new(
                Arc::new(ConsentRequiredWork),
                meerkat_core::OperationExecutionScope::Domain,
            )))
            .with_operation_review(Some(review));
        let effect = Arc::new(ConsentedRecordWrite::default());
        let args = RawValue::from_string(r#"{"namespace":"private"}"#.to_string()).expect("args");
        let call = ToolCallView {
            id: "call-r3",
            name: "write_record",
            args: &args,
        };
        let plan = meerkat_core::resolve_tool_execution_plan_fenced(
            &effect,
            call,
            &context,
            &meerkat_core::ToolExecutionResolutionContext::new(
                meerkat_core::ToolDeadlineChain::new(vec![
                    meerkat_core::ToolDeadlineContributor::finite(
                        meerkat_core::ToolDeadlineOwner::DirectCaller,
                        Duration::from_secs(600),
                    ),
                ])
                .expect("finite caller deadline"),
            ),
        )
        .expect("plan");
        let settled =
            meerkat_core::dispatch_tool_execution_plan_fenced(&effect, call, &context, &plan).await;
        let consent_id = route.requested();
        assert_eq!(
            settled.err(),
            Some(ToolError::ReviewUnsatisfied {
                kind: meerkat_core::ReviewUnsatisfiedKind::HumanConsentRequired,
                approval_id: Some(consent_id.clone()),
            }),
            "the R3 call settles locally with its one consent approval"
        );
        assert_eq!(
            approval_status(&router, &consent_id.to_string()).await,
            "pending"
        );
        assert_eq!(effect.entered(), 0);

        let decide = || {
            router.dispatch_with_consent_event(
                make_request(
                    CONSENT_DECIDE_METHOD,
                    serde_json::json!({"approval_id": consent_id.to_string(), "decision": "approve"}),
                ),
                consent_event(
                    &consent_id.to_string(),
                    meerkat_core::ApprovalDecision::Approve,
                ),
            )
        };
        let decided = decide().await;
        assert!(decided.error.is_none(), "{:?}", decided.error);
        assert_eq!(result_value(&decided)["status"], "approved");
        assert_eq!(effect.entered(), 0, "the decision itself executes nothing");

        meerkat_core::dispatch_retained_consent_attempt(&effect, &context, &consent_id)
            .await
            .expect("the explicit retained attempt enters");
        assert_eq!(effect.entered(), 1, "exactly once");
        assert!(
            meerkat_core::dispatch_retained_consent_attempt(&effect, &context, &consent_id)
                .await
                .is_err(),
            "a reuse of the spent consent is refused"
        );
        assert!(
            decide().await.error.is_some(),
            "the decided approval cannot be decided again"
        );
        assert_eq!(effect.entered(), 1, "and neither enters");
    }

    /// R3 work for one tool: each prepared decision requires human consent,
    /// stays current, and names no review material.
    struct ConsentRequiredWork;

    struct ConsentRequiredDecision;

    impl meerkat_core::authorization::PreparedOperationAuthorization for ConsentRequiredDecision {
        fn review_tier(&self) -> meerkat_core::authorization::OperationReviewTier {
            meerkat_core::authorization::OperationReviewTier::R3
        }

        fn check_current(
            &self,
            _binding: &meerkat_core::authorization::PreparedAuthorizationBinding,
        ) -> Result<(), meerkat_core::OperationAuthorizationError> {
            Ok(())
        }
    }

    impl meerkat_core::authorization::WorkAuthorization for ConsentRequiredWork {
        fn prepare(
            &self,
            _binding: &meerkat_core::authorization::PreparedAuthorizationBinding,
        ) -> Result<
            Arc<dyn meerkat_core::authorization::PreparedOperationAuthorization>,
            meerkat_core::OperationAuthorizationError,
        > {
            Ok(Arc::new(ConsentRequiredDecision))
        }

        fn review_operation_context(
            &self,
            _binding: &meerkat_core::authorization::PreparedAuthorizationBinding,
        ) -> Result<
            meerkat_core::authorization::ReviewOperationContextResolution,
            meerkat_core::OperationAuthorizationError,
        > {
            Ok(meerkat_core::authorization::ReviewOperationContextResolution::NotApplicable)
        }
    }

    /// R3 is never model-reviewed.
    struct NoModelReview;

    #[async_trait]
    impl meerkat_core::approval::review::OperationReviewer for NoModelReview {
        async fn review(
            &self,
            _candidate: &meerkat_core::approval::review::ReviewCandidate<'_>,
        ) -> Result<
            meerkat_core::approval::review::ReviewVerdict,
            meerkat_core::approval::review::ReviewerFailure,
        > {
            Err(meerkat_core::approval::review::ReviewerFailure::Unavailable)
        }
    }

    /// The host's human route: its eligibility owner publishes every change
    /// through one local authorization publication and lets `lead-1`
    /// decide; its notices tell the host which approval was requested.
    struct TeamConsentRoute {
        publication: meerkat_authorization::publication::LocalAuthorizationPublication,
        eligible: Vec<String>,
        notices: StdMutex<
            Vec<(
                meerkat_core::ApprovalId,
                meerkat_core::approval::consent::ConsentNoticeKind,
            )>,
        >,
    }

    impl Default for TeamConsentRoute {
        fn default() -> Self {
            Self {
                publication: meerkat_authorization::publication::LocalAuthorizationPublication::new(
                ),
                eligible: vec!["lead-1".to_string()],
                notices: StdMutex::default(),
            }
        }
    }

    impl TeamConsentRoute {
        /// The approval the last notice requested.
        fn requested(&self) -> meerkat_core::ApprovalId {
            let notices = self.notices.lock().expect("notices");
            let (approval_id, kind) = notices.last().cloned().expect("a requested notice");
            assert_eq!(
                kind,
                meerkat_core::approval::consent::ConsentNoticeKind::Requested
            );
            approval_id
        }
    }

    impl meerkat_core::approval::consent::HumanConsentRoute for TeamConsentRoute {
        fn present(
            &self,
            _candidate: &meerkat_core::approval::consent::ConsentPresentationCandidate<'_>,
        ) -> Result<
            meerkat_core::approval::consent::ConsentPresentation,
            meerkat_core::approval::consent::ConsentRouteUnavailable,
        > {
            Ok(meerkat_core::approval::consent::ConsentPresentation {
                requester: meerkat_core::ApprovalPrincipalId::new("human:child")
                    .expect("principal"),
                risk: meerkat_core::ApprovalRisk::High,
                metadata: meerkat_core::SurfaceMetadata::default(),
            })
        }

        fn eligibility(
            &self,
            approver: &meerkat_core::approval::consent::AuthenticatedApprover,
            _approval: &meerkat_core::ApprovalRecord,
        ) -> Result<
            Arc<dyn meerkat_core::approval::consent::ConsentEligibility>,
            meerkat_core::approval::consent::ConsentIneligible,
        > {
            let approver = approver.principal().id.as_str();
            meerkat_authorization::publication::PublishedConsentEligibility::observe(
                &self.publication,
                || self.eligible.iter().any(|eligible| eligible == approver),
            )
        }

        fn notice(&self, notice: meerkat_core::approval::consent::ConsentNotice) {
            self.notices
                .lock()
                .expect("notices")
                .push((notice.approval_id, notice.kind));
        }
    }

    /// The consented effect: spends the retained review once at its entry,
    /// immediately before its counted body.
    #[derive(Default)]
    struct ConsentedRecordWrite {
        entered: std::sync::atomic::AtomicUsize,
    }

    impl ConsentedRecordWrite {
        fn entered(&self) -> usize {
            self.entered.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl meerkat_core::AgentToolDispatcher for ConsentedRecordWrite {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            Arc::from([Arc::new(ToolDef::new(
                "write_record",
                "fixture record write",
                serde_json::json!({"type": "object"}),
            ))])
        }

        fn review_entry_support(
            &self,
            _tool_name: &str,
        ) -> meerkat_core::approval::review::ReviewEntrySupport {
            meerkat_core::approval::review::ReviewEntrySupport::ConsumesAtEntry
        }

        async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
            self.dispatch_with_context(call, &meerkat_core::ToolDispatchContext::default())
                .await
        }

        async fn dispatch_with_context(
            &self,
            call: ToolCallView<'_>,
            context: &meerkat_core::ToolDispatchContext,
        ) -> Result<ToolDispatchOutcome, ToolError> {
            let _entered = context.enter_reviewed_effect(call, None)?;
            self.entered
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(meerkat_core::ToolResult::new(call.id.into(), "written".into(), false).into())
        }
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
        test_router_with_llm_for_config(anthropic_test_config(), llm_client).await
    }

    async fn test_router_with_llm_for_config(
        config: Config,
        llm_client: Arc<dyn LlmClient>,
    ) -> (MethodRouter, mpsc::Receiver<RpcNotification>) {
        let temp = tempfile::tempdir().unwrap();
        let factory = AgentFactory::new(temp.path().join("sessions"));
        let store: Arc<dyn meerkat::SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let runtime = SessionRuntime::new(
            factory,
            config.clone(),
            10,
            runtime_backed_persistence(store),
            NotificationSink::noop(),
        )
        .expect("compose the RPC session runtime");
        let config_store: Arc<dyn ConfigStore> =
            Arc::new(MemoryConfigStore::new(config, meerkat_models::canonical()));
        runtime.set_default_llm_client(Some(llm_client));
        runtime.set_config_runtime(Arc::new(ConfigRuntime::new(
            Arc::clone(&config_store),
            temp.path().join("config_state.json"),
        )));
        let runtime = Arc::new(runtime);
        let (notif_tx, notif_rx) = mpsc::channel(100);
        let sink = NotificationSink::new(notif_tx);
        let router =
            MethodRouter::new(runtime, config_store, sink).expect("construct runtime authority");
        (router, notif_rx)
    }

    async fn test_openai_live_router() -> (MethodRouter, mpsc::Receiver<RpcNotification>) {
        test_router_with_llm_for_config(
            openai_live_test_config(),
            Arc::new(MockLlmClient::openai()),
        )
        .await
    }

    async fn test_router_with_llm_and_notification_capacity(
        llm_client: Arc<dyn LlmClient>,
        notification_capacity: usize,
    ) -> (MethodRouter, mpsc::Receiver<RpcNotification>) {
        let temp = tempfile::tempdir().unwrap();
        let factory = AgentFactory::new(temp.path().join("sessions"));
        let config = anthropic_test_config();
        let store: Arc<dyn meerkat::SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let runtime = SessionRuntime::new(
            factory,
            config.clone(),
            10,
            runtime_backed_persistence(store),
            NotificationSink::noop(),
        )
        .expect("compose the RPC session runtime");
        let config_store: Arc<dyn ConfigStore> =
            Arc::new(MemoryConfigStore::new(config, meerkat_models::canonical()));
        runtime.set_default_llm_client(Some(llm_client));
        runtime.set_config_runtime(Arc::new(ConfigRuntime::new(
            Arc::clone(&config_store),
            temp.path().join("config_state.json"),
        )));
        let runtime = Arc::new(runtime);
        let (notif_tx, notif_rx) = mpsc::channel(notification_capacity);
        let sink = NotificationSink::new(notif_tx);
        let router =
            MethodRouter::new(runtime, config_store, sink).expect("construct runtime authority");
        (router, notif_rx)
    }

    /// The RPC-built mob state serves live delegation's WorkGraph even when
    /// the runtime has no realm identity (the e2e harness and `rkat serve`
    /// without a manifest): the host service is supplied from the store and
    /// each mob rescopes it to `mob.<id>`, where its members build. Without
    /// this the coordinator silently ran its serial fallback.
    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn rpc_mob_state_serves_a_mob_scoped_workgraph_without_a_runtime_realm() {
        let temp = tempfile::tempdir().unwrap();
        let factory = AgentFactory::new(temp.path().join("sessions"));
        let config = anthropic_test_config();
        let store: Arc<dyn meerkat::SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let persistence = meerkat::PersistenceBundle::new_with_subsystem_stores(
            store,
            Arc::new(meerkat_runtime::InMemoryRuntimeStore::new()),
            memory_blob_store(),
            Arc::new(meerkat::MemoryScheduleStore::default()),
            Arc::new(meerkat::MemoryWorkGraphStore::new()),
        )
        .expect("construct runtime authority");
        let runtime = SessionRuntime::new(
            factory,
            config.clone(),
            10,
            persistence,
            NotificationSink::noop(),
        )
        .expect("compose the RPC session runtime");
        let config_store: Arc<dyn ConfigStore> =
            Arc::new(MemoryConfigStore::new(config, meerkat_models::canonical()));
        runtime.set_default_llm_client(Some(Arc::new(MockLlmClient::anthropic())));
        runtime.set_config_runtime(Arc::new(ConfigRuntime::new(
            Arc::clone(&config_store),
            temp.path().join("config_state.json"),
        )));
        let runtime = Arc::new(runtime);
        assert!(
            runtime.workgraph_service().is_err(),
            "this runtime has no realm identity"
        );
        let state = compose_rpc_mob_state(&runtime, &config_store, None)
            .expect("construct runtime authority");
        let host = state.workgraph_service().expect("host WorkGraph service");
        let mob_id = state
            .mob_create_definition(meerkat_mob::MobDefinition::implicit(
                "rpc-workgraph-scope",
                "claude-sonnet-4-5",
            ))
            .await
            .expect("mob");
        let scoped = state
            .workgraph_service_for_mob(&mob_id)
            .expect("mob scope")
            .expect("mob WorkGraph service");
        assert_eq!(
            scoped.default_realm_id(),
            meerkat_core::mob_realm_id(mob_id.as_str())
                .expect("mob realm")
                .as_str()
        );
        assert_ne!(scoped.default_realm_id(), host.default_realm_id());
        assert!(Arc::ptr_eq(scoped.store(), host.store()));
    }

    /// The default runtime carries a disabled WorkGraph backend. The RPC mob
    /// state then supplies no WorkGraph service at all: mob create and
    /// destroy work, members build without WorkGraph tools, and live
    /// delegation takes its serial path, instead of every mob operation
    /// failing against a store that supports nothing.
    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn rpc_mob_state_without_a_workgraph_backend_creates_and_destroys_mobs() {
        let temp = tempfile::tempdir().unwrap();
        let factory = AgentFactory::new(temp.path().join("sessions"));
        let config = anthropic_test_config();
        let store: Arc<dyn meerkat::SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let runtime = SessionRuntime::new(
            factory,
            config.clone(),
            10,
            runtime_backed_persistence(store),
            NotificationSink::noop(),
        )
        .expect("compose the RPC session runtime");
        assert_eq!(
            runtime.workgraph_store().kind(),
            meerkat::WorkGraphStoreKind::Disabled
        );
        let config_store: Arc<dyn ConfigStore> =
            Arc::new(MemoryConfigStore::new(config, meerkat_models::canonical()));
        runtime.set_default_llm_client(Some(Arc::new(MockLlmClient::anthropic())));
        runtime.set_config_runtime(Arc::new(ConfigRuntime::new(
            Arc::clone(&config_store),
            temp.path().join("config_state.json"),
        )));
        let runtime = Arc::new(runtime);
        let state = compose_rpc_mob_state(&runtime, &config_store, None)
            .expect("construct runtime authority");
        assert!(
            state.workgraph_service().is_none(),
            "a disabled backend supplies no host WorkGraph service"
        );
        let mob_id = state
            .mob_create_definition(meerkat_mob::MobDefinition::implicit(
                "rpc-workgraph-disabled",
                "claude-sonnet-4-5",
            ))
            .await
            .expect("mob creates without a WorkGraph backend");
        assert!(
            state
                .workgraph_service_for_mob(&mob_id)
                .expect("mob scope")
                .is_none(),
            "no mob-scoped service is derived from a disabled backend"
        );
        state
            .mob_destroy(&mob_id)
            .await
            .expect("mob destroys without a WorkGraph backend");
    }

    #[cfg(feature = "mob")]
    async fn test_router_with_mob_state(
        mob_state: Arc<meerkat_mob_mcp::MobMcpState>,
    ) -> (MethodRouter, mpsc::Receiver<RpcNotification>) {
        let temp = tempfile::tempdir().unwrap();
        let factory = AgentFactory::new(temp.path().join("sessions"));
        let config = anthropic_test_config();
        let store: Arc<dyn meerkat::SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let runtime = SessionRuntime::new(
            factory,
            config.clone(),
            10,
            runtime_backed_persistence(store),
            NotificationSink::noop(),
        )
        .expect("compose the RPC session runtime");
        let config_store: Arc<dyn ConfigStore> =
            Arc::new(MemoryConfigStore::new(config, meerkat_models::canonical()));
        runtime.set_default_llm_client(Some(Arc::new(MockLlmClient::anthropic())));
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
    ) -> (
        meerkat_mob::MobId,
        Arc<meerkat_mob::store::InMemoryMobEventStore>,
    ) {
        let mob_id = meerkat_mob::MobId::from(format!(
            "router-session-archive-partial-destroy-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let mut profiles = std::collections::BTreeMap::new();
        profiles.insert(
            meerkat_mob::ProfileName::from("worker"),
            meerkat_mob::ProfileBinding::Inline(Box::new(meerkat_mob::Profile {
                model_fallback: None,
                model: "claude-sonnet-4-5".to_string(),
                provider: None,
                self_hosted_server_id: None,
                image_generation_provider: None,
                auto_compact_threshold: None,
                resume_overrides: Vec::new(),
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
                durable: false,
            })),
        );
        let mut definition = meerkat_mob::MobDefinition::explicit(mob_id.clone());
        definition.profiles = profiles;
        let owner_session_id =
            SessionId::parse(owner_session_id).expect("valid owner bridge session id");
        let events = Arc::new(meerkat_mob::store::InMemoryMobEventStore::new());
        events.fail_clear_until_allowed();
        let storage = meerkat_mob::MobStorage::with_events(events.clone());
        let handle = meerkat_mob::MobBuilder::new(definition, storage)
            .with_owner_bridge_session_create_authority(owner_session_id, true, false)
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
        Arc<meerkat_mob::store::InMemoryMobEventStore>,
    ) {
        // Unique per test: the mob id names the process-global in-process
        // supervisor participant (`{mob_id}/__mob_supervisor__`), which admits
        // one holder, so tests sharing a fixed id collide under a threaded
        // runner.
        let mob_id = meerkat_mob::MobId::from(format!(
            "router-session-archive-live-retire-failure-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let mut profiles = std::collections::BTreeMap::new();
        profiles.insert(
            meerkat_mob::ProfileName::from("worker"),
            meerkat_mob::ProfileBinding::Inline(Box::new(meerkat_mob::Profile {
                model_fallback: None,
                model: "claude-sonnet-4-5".to_string(),
                provider: None,
                self_hosted_server_id: None,
                image_generation_provider: None,
                auto_compact_threshold: None,
                resume_overrides: Vec::new(),
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
                durable: false,
            })),
        );
        let mut definition = meerkat_mob::MobDefinition::explicit(mob_id.clone());
        definition.profiles = profiles;
        let events = Arc::new(meerkat_mob::store::InMemoryMobEventStore::new());
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
        let config = anthropic_test_config();
        let store: Arc<dyn meerkat::SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let runtime = SessionRuntime::new(
            factory,
            config.clone(),
            10,
            runtime_backed_persistence(store),
            NotificationSink::noop(),
        )
        .expect("compose the RPC session runtime");
        let config_store: Arc<dyn ConfigStore> =
            Arc::new(MemoryConfigStore::new(config, meerkat_models::canonical()));
        runtime.set_default_llm_client(Some(Arc::new(MockLlmClient::anthropic())));
        runtime.set_config_runtime(Arc::new(ConfigRuntime::new(
            Arc::clone(&config_store),
            temp.path().join("config_state.json"),
        )));
        runtime.set_skill_identity_registry(registry);
        let runtime = Arc::new(runtime);
        let (notif_tx, notif_rx) = mpsc::channel(100);
        let sink = NotificationSink::new(notif_tx);
        let router =
            MethodRouter::new(runtime, config_store, sink).expect("construct runtime authority");
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
                if notification_params!(notif)["event"]["payload"]["type"] != "run_started" {
                    continue;
                }
                // K3: `run_started` carries the typed `RunInput`; a
                // content-bearing run exposes its content under
                // `input.content`.
                break notification_params!(notif)["event"]["payload"]["input"]["content"]
                    .as_str()
                    .expect("run_started content input")
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

    fn exact_system_message_count(messages: &[Message], expected: &str) -> usize {
        messages
            .iter()
            .filter(|message| {
                matches!(
                    message,
                    Message::System(system) if system.content == expected
                )
            })
            .count()
    }

    // -----------------------------------------------------------------------
    // Tests
    // -----------------------------------------------------------------------

    /// #326: a request envelope declaring an unsupported JSON-RPC version is
    /// rejected with the standard INVALID_REQUEST error at the dispatch
    /// boundary rather than dispatched to the named method handler.
    #[tokio::test]
    async fn unsupported_jsonrpc_version_request_rejected_before_dispatch() {
        let (router, _notif_rx) = test_router().await;
        let req = RpcRequest {
            jsonrpc: "1.0".to_string(),
            id: Some(RpcId::Num(7)),
            method: "initialize".to_string(),
            params: None,
        };

        let resp = router
            .dispatch(req)
            .await
            .expect("request frame must yield a response");
        assert_eq!(error_code(&resp), crate::error::INVALID_REQUEST);
        assert_eq!(resp.id, Some(RpcId::Num(7)));
        // The handler never ran: the success-shaped capability payload is absent.
        assert!(resp.result.is_none());
    }

    /// #326: a missing JSON-RPC version (deserialized to empty string) is also
    /// rejected, not silently treated as 2.0.
    #[tokio::test]
    async fn missing_jsonrpc_version_request_rejected_before_dispatch() {
        let (router, _notif_rx) = test_router().await;
        let req = RpcRequest {
            jsonrpc: String::new(),
            id: Some(RpcId::Num(8)),
            method: "initialize".to_string(),
            params: None,
        };

        let resp = router
            .dispatch(req)
            .await
            .expect("request frame must yield a response");
        assert_eq!(error_code(&resp), crate::error::INVALID_REQUEST);
    }

    /// #326: a notification (no id) with an unsupported version gets no
    /// response and is not dispatched.
    #[tokio::test]
    async fn unsupported_jsonrpc_version_notification_dropped() {
        let (router, _notif_rx) = test_router().await;
        let notif = RpcRequest {
            jsonrpc: "1.0".to_string(),
            id: None,
            method: "initialized".to_string(),
            params: None,
        };

        assert!(
            router.dispatch(notif).await.is_none(),
            "bad-version notification must be dropped with no response"
        );
    }

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
        assert!(method_names.contains(&"session/activate_instruction"));
        assert!(method_names.contains(&"session/instruction_activations"));
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
            assert!(method_names.contains(&"mob/wire_members_batch"));
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
        let recorded_tools = Arc::new(std::sync::Mutex::new(Vec::<Vec<String>>::new()));
        let (router, _notif_rx) =
            test_router_with_llm(Arc::new(RecordingMockLlmClient::with_tool_names(
                Arc::clone(&recorded_requests),
                Arc::clone(&recorded_tools),
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
        {
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
            // Both help skills reach the model whole, past the 32 KiB that a
            // preload is cut at.
            for body in [
                meerkat::help::MEERKAT_PLATFORM_SKILL_BODY,
                meerkat::help::MEERKAT_CLI_REFERENCE_SKILL_BODY,
            ] {
                assert!(system_prompt.contains(body));
            }
            assert!(!system_prompt.contains("[truncated]"));
        }
        // The session's only tools read the platform skill's references.
        let mut tools = recorded_tools
            .lock()
            .expect("recorded tool names lock poisoned")
            .first()
            .cloned()
            .expect("help session should reach the LLM request");
        tools.sort();
        assert_eq!(tools, ["skill_list_resources", "skill_read_resource"]);

        // The help session starts no live MCP router; an ordinary session on
        // the same runtime is unchanged and still gets one.
        #[cfg(feature = "mcp")]
        {
            let help_session = meerkat_core::SessionId::parse(
                result_value(&response)["session_id"]
                    .as_str()
                    .expect("help/ask returns a session id"),
            )
            .expect("session id parses");
            assert!(
                !router
                    .runtime()
                    .has_live_mcp_session_for_testing(&help_session)
                    .await
            );
            let created = router
                .dispatch(make_request(
                    "session/create",
                    serde_json::json!({ "prompt": "hello" }),
                ))
                .await
                .expect("session/create response");
            assert!(
                created.error.is_none(),
                "session/create failed: {created:?}"
            );
            let session = meerkat_core::SessionId::parse(
                result_value(&created)["session_id"]
                    .as_str()
                    .expect("session/create returns a session id"),
            )
            .expect("session id parses");
            assert!(
                router
                    .runtime()
                    .has_live_mcp_session_for_testing(&session)
                    .await
            );
        }
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
                            "id": format!("test_mob_{}", uuid::Uuid::new_v4().simple()),
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
        assert_eq!(notification_params!(notification)["stream_id"], stream_id);
        assert_eq!(notification_params!(notification)["session_id"], session_id);
        assert_eq!(
            notification_params!(notification)["outcome"],
            "explicit_close"
        );
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
        assert_eq!(
            notification_params!(notification)["outcome"],
            "explicit_close"
        );

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
    async fn session_stream_close_unknown_is_rejected_without_authority_witness() {
        let (router, _notif_rx) = test_router().await;

        let close_resp = router
            .dispatch(make_request(
                "session/stream_close",
                serde_json::json!({ "stream_id": "00000000-0000-0000-0000-000000000000" }),
            ))
            .await
            .unwrap();
        assert_eq!(error_code(&close_resp), error::INVALID_PARAMS);
    }

    #[tokio::test]
    async fn session_stream_close_trusts_machine_terminal_over_active_map() {
        for overflow in [false, true] {
            let (mut router, mut notif_rx) = test_router().await;
            router.notification_sink.stream_terminal_admission = RpcStreamTerminalAdmission::new(1);
            let stream_ref = StreamRef::mint();
            let session_id = SessionId::new();
            resolve_session_stream_open_authority(
                &router.stream_authority,
                &stream_ref,
                &session_id,
            )
            .await
            .unwrap();
            router.active_session_streams.lock().await.insert(
                stream_ref,
                reserved_test_stream_forwarder(
                    &router.notification_sink,
                    StreamTerminal::Session(session_id.clone()),
                ),
            );
            let (recorded_tx, recorded_rx) = oneshot::channel();
            let (resume_tx, resume_rx) = oneshot::channel();
            let (delivered_tx, delivered_rx) = oneshot::channel();
            let task = tokio::spawn({
                let authority = router.stream_authority.clone();
                let streams = router.active_session_streams.clone();
                let sink = router.notification_sink.clone();
                async move {
                    let (observation, detail) = if overflow {
                        (
                            meerkat_runtime::meerkat_machine::dsl::RpcEventStreamTerminalObservationKind::NotificationQueueOverflow,
                            Some("synthetic queue overflow".to_string()),
                        )
                    } else {
                        (
                            meerkat_runtime::meerkat_machine::dsl::RpcEventStreamTerminalObservationKind::TransportEnded,
                            None,
                        )
                    };
                    let terminal = record_session_stream_terminal_authority(
                        &authority,
                        &stream_ref,
                        observation,
                        detail,
                    )
                    .await
                    .unwrap();
                    // Pause at the real forwarder's boundary between recording
                    // the machine effect and claiming its physical reservation.
                    recorded_tx.send(()).unwrap();
                    resume_rx.await.unwrap();
                    let forwarder = streams
                        .lock()
                        .await
                        .remove(&stream_ref)
                        .expect("the terminal sender still owns its reservation");
                    sink.emit_session_stream_end(&terminal, forwarder.terminal_reservation)
                        .await;
                    delivered_tx.send(()).unwrap();
                }
            });
            attach_stream_forwarder_task(&router.active_session_streams, stream_ref, task).await;
            tokio::time::timeout(Duration::from_secs(1), recorded_rx)
                .await
                .unwrap()
                .unwrap();

            for _ in 0..2 {
                let close_resp = router
                    .dispatch(make_request(
                        "session/stream_close",
                        serde_json::json!({ "stream_id": stream_ref.as_string() }),
                    ))
                    .await
                    .unwrap();
                let closed = result_value(&close_resp);
                assert_eq!(closed["closed"], true);
                assert_eq!(closed["already_closed"], true);
                assert!(
                    router
                        .active_session_streams
                        .lock()
                        .await
                        .contains_key(&stream_ref),
                    "already-closed callers must not abort the authoritative terminal sender"
                );
                assert!(
                    router
                        .notification_sink
                        .stream_terminal_admission
                        .try_reserve()
                        .is_err(),
                    "pending terminal custody must retain the bounded reservation"
                );
                assert!(
                    notif_rx.try_recv().is_err(),
                    "close must not fabricate a replacement terminal"
                );
            }
            resume_tx
                .send(())
                .expect("close must leave the terminal sender alive");
            tokio::time::timeout(Duration::from_secs(1), delivered_rx)
                .await
                .unwrap()
                .unwrap();
            let terminal = notif_rx.try_recv().expect("original terminal delivered");
            assert_eq!(terminal.method, "session/stream_end");
            assert_eq!(
                notification_params!(terminal)["stream_id"],
                stream_ref.as_string()
            );
            assert_eq!(
                notification_params!(terminal)["session_id"],
                session_id.to_string()
            );
            assert_eq!(
                notification_params!(terminal)["outcome"],
                if overflow {
                    "terminal_error"
                } else {
                    "remote_end"
                }
            );
            if overflow {
                assert_eq!(
                    notification_params!(terminal)["error"]["code"],
                    "stream_queue_overflow"
                );
                assert_eq!(
                    notification_params!(terminal)["error"]["message"],
                    "synthetic queue overflow"
                );
            }
            assert!(router.active_session_streams.lock().await.is_empty());
            assert!(
                notif_rx.try_recv().is_err(),
                "terminal must be delivered only once"
            );
            assert!(
                router
                    .notification_sink
                    .stream_terminal_admission
                    .try_reserve()
                    .is_err(),
                "the queued terminal retains its reservation until written or dropped"
            );
            drop(terminal);
            assert!(
                router
                    .notification_sink
                    .stream_terminal_admission
                    .try_reserve()
                    .is_ok()
            );
        }
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
                    && notification_params!(notif)["stream_id"].as_str() == Some(stream_id.as_str())
                    && notification_params!(notif)["event"]["payload"]["type"].as_str()
                        == Some("run_started")
                {
                    break notif;
                }
            }
        })
        .await
        .expect("pending session stream should receive first-turn events");

        assert_eq!(
            notification_params!(notification)["session_id"].as_str(),
            Some(session_id.as_str())
        );
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn router_created_deferred_sessions_project_mob_tools_to_live_open_config() {
        let _projection_guard = crate::session_runtime::realtime_open_projection_test_guard().await;
        let (router, _notif_rx) = test_router().await;

        let create_resp = router
            .dispatch(make_request(
                "session/create",
                serde_json::json!({
                    "prompt": "deferred live mob tool fixture",
                    "initial_turn": "deferred",
                    "enable_mob": true
                }),
            ))
            .await
            .expect("session/create response");
        assert!(
            create_resp.error.is_none(),
            "session/create should succeed: {create_resp:?}"
        );
        let create_result = result_value(&create_resp);
        let session_id = create_result["session_id"].as_str().expect("session_id");
        let session_id = SessionId::parse(session_id).expect("created session id should parse");

        let open_config = router
            .runtime
            .live_open_config_for_session(
                &session_id,
                meerkat_contracts::RealtimeTurningMode::ProviderManaged,
            )
            .await
            .expect("live open config");
        let tool_names: std::collections::BTreeSet<&str> = open_config
            .visible_tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect();

        for expected in ["delegate", "mob_create", "mob_spawn_member", "mob_list"] {
            assert!(
                tool_names.contains(expected),
                "live/open config must expose agent-facing mob tool `{expected}`; got {tool_names:?}"
            );
        }
    }

    /// JSON-RPC surface: `session/create` with
    /// `initial_turn: "deferred"` keeps its create-time prompt in the staged
    /// slot; the `live/open` projection promotes the slot into the live
    /// service with `DeferredPromptPolicy::Stage`; the first later
    /// `turn/start` is runtime-routed (typed appends author the turn). The
    /// create-time prompt must reach the provider request and the durable
    /// transcript.
    #[tokio::test]
    async fn router_deferred_create_prompt_survives_live_open_promotion_and_turn_start() {
        let _projection_guard = crate::session_runtime::realtime_open_projection_test_guard().await;
        let requests = Arc::new(StdMutex::new(Vec::new()));
        let (router, _notif_rx) =
            test_router_with_llm(Arc::new(RecordingMockLlmClient::new(Arc::clone(&requests))))
                .await;

        let create_resp = router
            .dispatch(make_request(
                "session/create",
                serde_json::json!({
                    "prompt": "Remember the codeword ORBIT-7 for later.",
                    "initial_turn": "deferred"
                }),
            ))
            .await
            .expect("session/create response");
        assert!(
            create_resp.error.is_none(),
            "session/create should succeed: {create_resp:?}"
        );
        let create_result = result_value(&create_resp);
        let session_id = create_result["session_id"].as_str().expect("session_id");
        let session_id = SessionId::parse(session_id).expect("created session id should parse");

        router
            .runtime
            .live_open_config_for_session(
                &session_id,
                meerkat_contracts::RealtimeTurningMode::ProviderManaged,
            )
            .await
            .expect("live/open projection should materialize the deferred session");

        let turn_resp = router
            .dispatch(make_request(
                "turn/start",
                serde_json::json!({
                    "session_id": session_id.to_string(),
                    "prompt": "What is the codeword?"
                }),
            ))
            .await
            .expect("turn/start response");
        assert!(
            turn_resp.error.is_none(),
            "turn/start should succeed: {turn_resp:?}"
        );

        let recorded = requests
            .lock()
            .expect("recorded requests lock poisoned")
            .clone();
        let request_dump = format!("{recorded:?}");
        let session = router
            .runtime
            .load_persisted_session(&session_id)
            .await
            .expect("load persisted session")
            .expect("session persisted");
        let user_texts = session
            .messages()
            .iter()
            .filter_map(|message| match message {
                Message::User(user) => Some(user.text_content()),
                _ => None,
            })
            .collect::<Vec<_>>();
        let llm_request_has_staged = request_dump.contains("ORBIT-7");
        let transcript_has_staged = user_texts.iter().any(|text| text.contains("ORBIT-7"));
        assert!(
            llm_request_has_staged && transcript_has_staged,
            "deferred create prompt lost after live/open promotion: \
             llm_request_contains={llm_request_has_staged} \
             transcript_contains={transcript_has_staged}; \
             transcript_user_messages={user_texts:?}; llm_requests={}",
            recorded.len()
        );
    }

    /// Full JSON-RPC surface, no internal seam: the SDK flow
    /// `create_deferred_session(prompt)` then `live.open()` then a text
    /// turn. `live/open` promotes the deferred session with
    /// `DeferredPromptPolicy::Stage`; `live/close` ends the voice channel;
    /// the next `turn/start` is runtime-routed. The create-time prompt must
    /// reach the provider request and the durable transcript.
    #[cfg(feature = "comms")]
    #[tokio::test]
    async fn router_live_open_close_then_turn_start_delivers_deferred_create_prompt() {
        struct OpenAiRecordingLlmClient {
            requests: Arc<StdMutex<Vec<Vec<Message>>>>,
        }

        #[async_trait]
        impl LlmClient for OpenAiRecordingLlmClient {
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
                Box<
                    dyn futures::Stream<Item = Result<meerkat_client::LlmEvent, LlmError>>
                        + Send
                        + 'a,
                >,
            > {
                self.requests
                    .lock()
                    .expect("recorded requests lock poisoned")
                    .push(request.messages.clone());
                Box::pin(stream::iter(vec![
                    Ok(meerkat_client::LlmEvent::TextDelta {
                        delta: "recorded".to_string(),
                        meta: None,
                    }),
                    Ok(meerkat_client::LlmEvent::UsageUpdate {
                        usage: meerkat_core::TurnUsage::host_declared(
                            meerkat_core::Provider::OpenAI,
                            &request.model,
                            meerkat_core::Usage::default(),
                        ),
                    }),
                    Ok(meerkat_client::LlmEvent::Done {
                        outcome: meerkat_client::LlmDoneOutcome::success(StopReason::EndTurn),
                    }),
                ]))
            }

            fn provider(&self) -> meerkat_core::Provider {
                meerkat_core::Provider::OpenAI
            }

            async fn health_check(&self) -> Result<(), LlmError> {
                Ok(())
            }
        }

        let _projection_guard = crate::session_runtime::realtime_open_projection_test_guard().await;
        let requests = Arc::new(StdMutex::new(Vec::new()));
        let (router, _notif_rx) = test_router_with_llm_for_config(
            openai_live_test_config(),
            Arc::new(OpenAiRecordingLlmClient {
                requests: Arc::clone(&requests),
            }),
        )
        .await;
        let host = Arc::new(meerkat_live::LiveAdapterHost::new(Arc::new(
            meerkat_live::NoOpProjectionSink,
        )));
        let close_feedback: Arc<dyn meerkat_live::LiveChannelCloseFeedback> = Arc::new(
            crate::live_projection_sink::SessionServiceProjectionSink::new(Arc::clone(
                &router.runtime,
            )),
        );
        let status_feedback: Arc<dyn meerkat_live::LiveChannelStatusFeedback> = Arc::new(
            crate::live_projection_sink::SessionServiceProjectionSink::new(Arc::clone(
                &router.runtime,
            )),
        );
        let token_authority: Arc<dyn meerkat_live::LiveWsTokenAuthority> = Arc::new(
            crate::live_projection_sink::SessionServiceProjectionSink::new(Arc::clone(
                &router.runtime,
            )),
        );
        let live_ws = Arc::new(meerkat_live::LiveWsState::new(
            host,
            close_feedback,
            status_feedback,
            token_authority,
        ));
        let router = router
            .with_live_ws(live_ws, "ws://127.0.0.1:0".to_string())
            .with_live_session_factory(Arc::new(RecordingLiveFactory {
                log: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            }));

        let create_resp = router
            .dispatch(make_request(
                "session/create",
                serde_json::json!({
                    "prompt": "Remember the codeword ORBIT-7 for later.",
                    "initial_turn": "deferred",
                    "model": "gpt-realtime-2",
                    "provider": "openai"
                }),
            ))
            .await
            .expect("session/create response");
        assert!(
            create_resp.error.is_none(),
            "session/create should succeed: {create_resp:?}"
        );
        let session_id = result_value(&create_resp)["session_id"]
            .as_str()
            .expect("session_id")
            .to_string();

        let open_resp = router
            .dispatch(make_request(
                "live/open",
                serde_json::json!({
                    "session_id": session_id,
                    "transport": "websocket"
                }),
            ))
            .await
            .expect("live/open response");
        assert!(
            open_resp.error.is_none(),
            "live/open should succeed: {open_resp:?}"
        );
        let channel_id = result_value(&open_resp)["channel_id"]
            .as_str()
            .expect("channel_id")
            .to_string();
        let close_resp = router
            .dispatch(make_request(
                "live/close",
                serde_json::json!({ "channel_id": channel_id }),
            ))
            .await
            .expect("live/close response");
        assert!(
            close_resp.error.is_none(),
            "live/close should succeed: {close_resp:?}"
        );

        let turn_resp = router
            .dispatch(make_request(
                "turn/start",
                serde_json::json!({
                    "session_id": session_id,
                    "prompt": "What is the codeword?"
                }),
            ))
            .await
            .expect("turn/start response");
        assert!(
            turn_resp.error.is_none(),
            "turn/start should succeed: {turn_resp:?}"
        );

        let recorded = requests
            .lock()
            .expect("recorded requests lock poisoned")
            .clone();
        let request_dump = format!("{recorded:?}");
        let session_id = SessionId::parse(&session_id).expect("created session id should parse");
        let session = router
            .runtime
            .load_persisted_session(&session_id)
            .await
            .expect("load persisted session")
            .expect("session persisted");
        let user_texts = session
            .messages()
            .iter()
            .filter_map(|message| match message {
                Message::User(user) => Some(user.text_content()),
                _ => None,
            })
            .collect::<Vec<_>>();
        let llm_request_has_staged = request_dump.contains("ORBIT-7");
        let transcript_has_staged = user_texts.iter().any(|text| text.contains("ORBIT-7"));
        assert!(
            llm_request_has_staged && transcript_has_staged,
            "deferred create prompt lost after live/open + live/close: \
             llm_request_contains={llm_request_has_staged} \
             transcript_contains={transcript_has_staged}; \
             transcript_user_messages={user_texts:?}; llm_requests={}",
            recorded.len()
        );
    }

    #[cfg(feature = "comms")]
    #[tokio::test]
    async fn router_created_deferred_live_session_exposes_comms_tools_before_peers_exist() {
        let _projection_guard = crate::session_runtime::realtime_open_projection_test_guard().await;
        let (router, _notif_rx) = test_router().await;

        let create_resp = router
            .dispatch(make_request(
                "session/create",
                serde_json::json!({
                    "prompt": "deferred live comms tool fixture",
                    "initial_turn": "deferred",
                    "keep_alive": true,
                    "comms_name": "live-smoke-controller-test"
                }),
            ))
            .await
            .expect("session/create response");
        assert!(
            create_resp.error.is_none(),
            "session/create should succeed: {create_resp:?}"
        );
        let create_result = result_value(&create_resp);
        let session_id = create_result["session_id"].as_str().expect("session_id");
        let session_id = SessionId::parse(session_id).expect("created session id should parse");

        let open_config = router
            .runtime
            .live_open_config_for_session(
                &session_id,
                meerkat_contracts::RealtimeTurningMode::ProviderManaged,
            )
            .await
            .expect("live open config");
        let tool_names: std::collections::BTreeSet<&str> = open_config
            .visible_tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect();

        for expected in ["peers", "send_message"] {
            assert!(
                tool_names.contains(expected),
                "live/open config must advertise comms tool `{expected}` before delegate wiring so the hosted realtime provider can call it later; got {tool_names:?}"
            );
        }
    }

    #[cfg(feature = "comms")]
    struct RecordingLiveAdapter {
        log: Arc<tokio::sync::Mutex<Vec<meerkat_core::live_adapter::LiveAdapterCommand>>>,
    }

    #[cfg(feature = "comms")]
    #[async_trait::async_trait]
    impl meerkat_core::live_adapter::LiveAdapter for RecordingLiveAdapter {
        async fn send_command(
            &self,
            command: meerkat_core::live_adapter::LiveAdapterCommand,
        ) -> Result<(), meerkat_core::live_adapter::LiveAdapterError> {
            self.log.lock().await.push(command);
            Ok(())
        }

        async fn next_observation(
            &self,
        ) -> Result<
            Option<meerkat_core::live_adapter::LiveAdapterObservation>,
            meerkat_core::live_adapter::LiveAdapterError,
        > {
            Ok(None)
        }

        fn status(&self) -> meerkat_core::live_adapter::LiveAdapterStatus {
            meerkat_core::live_adapter::LiveAdapterStatus::Ready
        }

        async fn close(&self) -> Result<(), meerkat_core::live_adapter::LiveAdapterError> {
            Ok(())
        }
    }

    #[cfg(feature = "comms")]
    struct RecordingLiveFactory {
        log: Arc<tokio::sync::Mutex<Vec<meerkat_core::live_adapter::LiveAdapterCommand>>>,
    }

    #[cfg(feature = "comms")]
    #[async_trait::async_trait]
    impl meerkat_client::realtime_session::RealtimeSessionFactory for RecordingLiveFactory {
        fn capabilities(&self) -> meerkat_contracts::RealtimeCapabilities {
            // #176: the live WS transport resolves its audio policy from the
            // factory's typed `RealtimeCapabilities`. A realistic realtime
            // factory advertises PCM 24 kHz mono in both directions (the only
            // binary format the WS transport negotiates), so `live/open`
            // resolves a typed audio policy instead of failing closed.
            meerkat_contracts::RealtimeCapabilities {
                audio_input_format: Some(meerkat_contracts::RealtimeAudioFormat::pcm(24_000, 1)),
                audio_output_format: Some(meerkat_contracts::RealtimeAudioFormat::pcm(24_000, 1)),
                ..meerkat_contracts::RealtimeCapabilities::default()
            }
        }

        fn supports_provider(&self, provider: meerkat_core::Provider) -> bool {
            provider == meerkat_core::Provider::OpenAI
        }

        async fn open_session(
            &self,
            _open_config: &meerkat_client::realtime_session::RealtimeSessionOpenConfig,
        ) -> Result<
            Box<dyn meerkat_client::realtime_session::RealtimeSession>,
            meerkat_client::LlmError,
        > {
            Err(meerkat_client::LlmError::InvalidRequest {
                message: "test factory only supports direct live adapters".to_string(),
            })
        }

        async fn attach_external_session(
            &self,
            _target: &meerkat_client::realtime_session::RealtimeExternalSessionTarget,
            _open_config: &meerkat_client::realtime_session::RealtimeSessionOpenConfig,
        ) -> Result<
            Box<dyn meerkat_client::realtime_session::RealtimeSession>,
            meerkat_client::LlmError,
        > {
            Err(meerkat_client::LlmError::InvalidRequest {
                message: "test factory only supports direct live adapters".to_string(),
            })
        }

        async fn open_live_adapter(
            &self,
            _open_config: &meerkat_client::realtime_session::RealtimeSessionOpenConfig,
        ) -> Result<Arc<dyn meerkat_core::live_adapter::LiveAdapter>, meerkat_client::LlmError>
        {
            Ok(Arc::new(RecordingLiveAdapter {
                log: Arc::clone(&self.log),
            }))
        }
    }

    #[cfg(feature = "comms")]
    async fn open_deferred_keep_alive_live_controller(
        comms_name: &str,
    ) -> (
        MethodRouter,
        SessionId,
        Arc<tokio::sync::Mutex<Vec<meerkat_core::live_adapter::LiveAdapterCommand>>>,
    ) {
        let (router, _notif_rx) = test_openai_live_router().await;
        let host = Arc::new(meerkat_live::LiveAdapterHost::new(Arc::new(
            meerkat_live::NoOpProjectionSink,
        )));
        let close_feedback: Arc<dyn meerkat_live::LiveChannelCloseFeedback> = Arc::new(
            crate::live_projection_sink::SessionServiceProjectionSink::new(Arc::clone(
                &router.runtime,
            )),
        );
        let status_feedback: Arc<dyn meerkat_live::LiveChannelStatusFeedback> = Arc::new(
            crate::live_projection_sink::SessionServiceProjectionSink::new(Arc::clone(
                &router.runtime,
            )),
        );
        let token_authority: Arc<dyn meerkat_live::LiveWsTokenAuthority> = Arc::new(
            crate::live_projection_sink::SessionServiceProjectionSink::new(Arc::clone(
                &router.runtime,
            )),
        );
        let live_ws = Arc::new(meerkat_live::LiveWsState::new(
            host,
            close_feedback,
            status_feedback,
            token_authority,
        ));
        let command_log = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let router = router
            .with_live_ws(live_ws, "ws://127.0.0.1:0".to_string())
            .with_live_session_factory(Arc::new(RecordingLiveFactory {
                log: Arc::clone(&command_log),
            }));

        let create_resp = router
            .dispatch(make_request(
                "session/create",
                serde_json::json!({
                    "prompt": "deferred live comms drain fixture",
                    "initial_turn": "deferred",
                    "model": "gpt-realtime-2",
                    "provider": "openai",
                    "keep_alive": true,
                    "comms_name": comms_name
                }),
            ))
            .await
            .expect("session/create response");
        assert!(
            create_resp.error.is_none(),
            "session/create should succeed: {create_resp:?}"
        );
        let create_result = result_value(&create_resp);
        let session_id = create_result["session_id"].as_str().expect("session_id");
        let session_id = SessionId::parse(session_id).expect("created session id should parse");

        let open_resp = router
            .dispatch(make_request(
                "live/open",
                serde_json::json!({
                    "session_id": session_id.to_string(),
                    "transport": "websocket"
                }),
            ))
            .await
            .expect("live/open response");
        assert!(
            open_resp.error.is_none(),
            "live/open should succeed: {open_resp:?}"
        );

        (router, session_id, command_log)
    }

    #[cfg(feature = "comms")]
    #[tokio::test]
    async fn live_open_starts_peer_ingress_for_deferred_keep_alive_controller() {
        let _projection_guard = crate::session_runtime::realtime_open_projection_test_guard().await;
        let (router, session_id, _command_log) =
            open_deferred_keep_alive_live_controller("live-smoke-controller-drain-test").await;

        let snapshot = router
            .runtime_adapter()
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .expect("runtime spine snapshot");
        assert_eq!(
            snapshot.drain.phase,
            Some(meerkat_runtime::CommsDrainPhase::Running),
            "live/open must start the session-owned comms drain so wired delegate helpers can report back to the live controller; got {:?}",
            snapshot.drain
        );
        assert!(
            snapshot.drain.handle_present,
            "running live peer ingress must have a drain task handle: {:?}",
            snapshot.drain
        );
    }

    #[cfg(feature = "comms")]
    #[tokio::test]
    async fn live_peer_ingress_forwards_peer_message_to_active_live_adapter() {
        let _projection_guard = crate::session_runtime::realtime_open_projection_test_guard().await;
        let (router, session_id, command_log) =
            open_deferred_keep_alive_live_controller("live-smoke-controller-forward-test").await;

        let input = meerkat_runtime::Input::Peer(meerkat_runtime::PeerInput {
            directed_interaction_id: None,
            objective_id: None,
            system_prompts: Vec::new(),
            injected_context: Vec::new(),
            sender_taint: None,
            header: meerkat_runtime::InputHeader {
                ingress_context: None,
                retained_resume: None,
                authority_association: None,
                id: meerkat_core::lifecycle::InputId::new(),
                timestamp: chrono::Utc::now(),
                source: meerkat_runtime::InputOrigin::Peer {
                    peer_id: "550e8400-e29b-41d4-a716-446655440000".to_string(),
                    display_identity: Some("helper-forward-test".to_string()),
                    runtime_id: None,
                },
                durability: meerkat_runtime::InputDurability::Durable,
                visibility: meerkat_runtime::InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            convention: Some(meerkat_runtime::PeerConvention::Message),
            content: "helper finished via comms".into(),
            payload: None,
            handling_mode: None,
            in_reply_to: None,
        });

        let (_outcome, handle) = router
            .runtime_adapter()
            .accept_input_with_completion(&session_id, input)
            .await
            .expect("accept peer input");
        if let Some(handle) = handle {
            let _ = handle.wait().await;
        }

        let recorded = command_log.lock().await;
        assert!(
            recorded.iter().any(|command| matches!(
                command,
                meerkat_core::live_adapter::LiveAdapterCommand::SendInput {
                    chunk: meerkat_core::live_adapter::LiveInputChunk::Text { text }
                } if text.contains("helper finished via comms")
            )),
            "live peer ingress must project the peer message to the active live adapter; got {recorded:?}"
        );
    }

    #[cfg(feature = "comms")]
    #[tokio::test]
    async fn live_peer_ingress_steer_interrupts_before_forwarding_to_active_live_adapter() {
        let _projection_guard = crate::session_runtime::realtime_open_projection_test_guard().await;
        let (router, session_id, command_log) =
            open_deferred_keep_alive_live_controller("live-smoke-controller-steer-test").await;

        let input = meerkat_runtime::Input::Peer(meerkat_runtime::PeerInput {
            directed_interaction_id: None,
            objective_id: None,
            system_prompts: Vec::new(),
            injected_context: Vec::new(),
            sender_taint: None,
            header: meerkat_runtime::InputHeader {
                ingress_context: None,
                retained_resume: None,
                authority_association: None,
                id: meerkat_core::lifecycle::InputId::new(),
                timestamp: chrono::Utc::now(),
                source: meerkat_runtime::InputOrigin::Peer {
                    peer_id: "550e8400-e29b-41d4-a716-446655440001".to_string(),
                    display_identity: Some("helper-steer-test".to_string()),
                    runtime_id: None,
                },
                durability: meerkat_runtime::InputDurability::Durable,
                visibility: meerkat_runtime::InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            convention: Some(meerkat_runtime::PeerConvention::Message),
            content: "urgent helper update via comms".into(),
            payload: None,
            handling_mode: Some(meerkat_core::types::HandlingMode::Steer),
            in_reply_to: None,
        });

        let (_outcome, handle) = router
            .runtime_adapter()
            .accept_input_with_completion(&session_id, input)
            .await
            .expect("accept steered peer input");
        if let Some(handle) = handle {
            let _ = handle.wait().await;
        }

        let recorded = command_log.lock().await;
        assert!(
            matches!(
                recorded.as_slice(),
                [
                    meerkat_core::live_adapter::LiveAdapterCommand::Interrupt,
                    meerkat_core::live_adapter::LiveAdapterCommand::SendInput {
                        chunk: meerkat_core::live_adapter::LiveInputChunk::Text { text }
                    },
                    ..
                ] if text.contains("urgent helper update via comms")
            ),
            "steered live peer ingress must interrupt provider output before sending the new text input; got {recorded:?}"
        );
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn router_created_deferred_sessions_apply_tool_filter_to_live_open_config() {
        let _projection_guard = crate::session_runtime::realtime_open_projection_test_guard().await;
        let (router, _notif_rx) = test_router().await;

        let create_resp = router
            .dispatch(make_request(
                "session/create",
                serde_json::json!({
                    "prompt": "deferred live filtered mob tool fixture",
                    "initial_turn": "deferred",
                    "enable_mob": true,
                    "tool_filter": { "Deny": ["mob_check_member"] }
                }),
            ))
            .await
            .expect("session/create response");
        assert!(
            create_resp.error.is_none(),
            "session/create should succeed: {create_resp:?}"
        );
        let create_result = result_value(&create_resp);
        let session_id = create_result["session_id"].as_str().expect("session_id");
        let session_id = SessionId::parse(session_id).expect("created session id should parse");

        let open_config = router
            .runtime
            .live_open_config_for_session(
                &session_id,
                meerkat_contracts::RealtimeTurningMode::ProviderManaged,
            )
            .await
            .expect("live open config");
        let tool_names: std::collections::BTreeSet<&str> = open_config
            .visible_tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect();

        assert!(
            !tool_names.contains("mob_check_member"),
            "session-local tool filter must hide status side-channel from live/open config; got {tool_names:?}"
        );
        assert!(
            tool_names.contains("delegate"),
            "session-local deny filter must keep ordinary mob tools visible; got {tool_names:?}"
        );
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn live_tool_dispatcher_routes_through_session_composed_tools() {
        let _projection_guard = crate::session_runtime::realtime_open_projection_test_guard().await;
        let (router, _notif_rx) = test_router().await;

        let create_resp = router
            .dispatch(make_request(
                "session/create",
                serde_json::json!({
                    "prompt": "deferred live mob dispatch fixture",
                    "initial_turn": "deferred",
                    "enable_mob": true
                }),
            ))
            .await
            .expect("session/create response");
        assert!(
            create_resp.error.is_none(),
            "session/create should succeed: {create_resp:?}"
        );
        let create_result = result_value(&create_resp);
        let session_id = create_result["session_id"].as_str().expect("session_id");
        let session_id = SessionId::parse(session_id).expect("created session id should parse");

        router
            .runtime
            .live_open_config_for_session(
                &session_id,
                meerkat_contracts::RealtimeTurningMode::ProviderManaged,
            )
            .await
            .expect("materialize deferred session for live");

        let dispatcher = RuntimeLiveToolDispatcher {
            runtime: Arc::clone(&router.runtime),
        };
        let outcome = meerkat_live::LiveToolDispatcher::dispatch_live_tool_call(
            &dispatcher,
            &session_id,
            meerkat_core::ToolCall::new(
                "live-call-1".to_string(),
                "mob_list".to_string(),
                serde_json::json!({}),
            ),
        )
        .await
        .expect("live tool dispatch");

        assert_eq!(outcome.result.tool_use_id, "live-call-1");
        assert!(
            !outcome.result.is_error,
            "mob_list must execute through the session-composed dispatcher, not the callback-only path: {:?}",
            outcome.result
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
        assert_eq!(appended["status"], "applied");
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

        let spawn_resp = router
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
        assert!(
            spawn_resp.error.is_none(),
            "worker spawn should establish its bridge session before the late-response scenario: {spawn_resp:?}"
        );
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
        add_generated_peer_projection_trust(
            &sender,
            TrustedPeerDescriptor::unsigned_with_pubkey(
                format!("{mob_id}/worker/worker-1"),
                operator_pubkey.to_peer_id().to_string(),
                *operator_pubkey.as_bytes(),
                operator_addr,
            )
            .expect("worker trusted peer spec"),
            "sender trusts worker",
        )
        .await;
        stage_generated_peer_projection_trust(
            router.runtime.runtime_adapter(),
            &session_id,
            operator_comms.clone(),
            TrustedPeerDescriptor::unsigned_with_pubkey(
                "router-peer-response-sender",
                sender_peer_id,
                *sender.public_key().as_bytes(),
                sender_addr,
            )
            .expect("sender trusted peer spec"),
            "worker trusts sender",
        )
        .await;

        let in_reply_to = InteractionId(uuid::Uuid::new_v4());
        let corr_id = meerkat_core::PeerCorrelationId::from_uuid(in_reply_to.0);
        operator_comms
            .peer_interaction_handle()
            .expect("worker peer request authority")
            .request_sent(corr_id, sender.public_key().to_peer_id())
            .expect("seed outbound request before terminal peer response");
        sender
            .peer_interaction_handle()
            .expect("sender peer response authority")
            .request_received(
                corr_id,
                meerkat_core::types::HandlingMode::Queue,
                operator_pubkey.to_peer_id(),
            )
            .expect("seed inbound request before terminal peer response");

        CoreCommsRuntime::send(
            &*sender,
            CommsCommand::PeerResponse {
                objective_id: None,
                content_taint: None,
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
                let mut mob_run_completed = None;
                let mut session_run_completed = None;
                loop {
                    let notif = notif_rx.recv().await.expect("notification");
                    if notif.method == "mob/stream_event"
                        && notification_params!(notif)["stream_id"] == stream_id
                        && notification_params!(notif)["event"]["payload"]["type"].as_str()
                            == Some("run_completed")
                    {
                        mob_run_completed = Some(notif);
                    } else if notif.method == "session/stream_event"
                        && notification_params!(notif)["stream_id"] == session_stream_id
                        && notification_params!(notif)["event"]["payload"]["type"].as_str()
                            == Some("run_completed")
                    {
                        session_run_completed = Some(notif);
                    }
                    if mob_run_completed.is_some() && session_run_completed.is_some() {
                        break (mob_run_completed, session_run_completed);
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
                    "run_completed notification should arrive on both subscribed streams; runtime_state={runtime_state:?}; ingress_snapshot={ingress_snapshot:?}"
                );
            }
        };

        assert!(session_run_completed.is_some());
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
        assert_eq!(
            members.len(),
            0,
            "unexpected member list: {members_value:?}"
        );

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
                    "content": { "type": "text", "text": "Coordinate with the lead before acting." },
                    "source": "mob",
                    "idempotency_key": "ctx-worker-1"
                }),
            ))
            .await
            .unwrap();

        assert_eq!(result_value(&inject_resp)["status"], "applied");
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
                assistant_message_id: None,
            },
        );

        assert!(
            sink.emit_session_stream_event(&StreamRef::mint(), 1, &session_id, &envelope)
                .await
                == StreamEmitStatus::Delivered
        );
        assert!(
            sink.emit_session_stream_event(&StreamRef::mint(), 2, &session_id, &envelope)
                .await
                == StreamEmitStatus::Overflow,
            "second send should surface overflow to the caller"
        );

        let first = rx.recv().await.expect("first notification");
        assert_eq!(first.method, "session/stream_event");
    }

    fn reserved_test_stream_forwarder(
        sink: &NotificationSink,
        terminal: StreamTerminal,
    ) -> StreamForwarder {
        let session_delivery = match &terminal {
            StreamTerminal::Session(_) => {
                Some(Arc::new(Mutex::new(SessionStreamDelivery::default())))
            }
            #[cfg(feature = "mob")]
            StreamTerminal::Mob => None,
        };
        StreamForwarder {
            terminal,
            session_delivery,
            terminal_reservation: sink
                .stream_terminal_admission
                .try_reserve()
                .expect("terminal reservation"),
            state: StreamForwarderState::Active {
                stop_tx: None,
                task: None,
            },
        }
    }

    async fn assert_idempotent_close_preserves_pending_terminal(terminal: StreamTerminal) {
        let (mut router, mut notif_rx) = test_router().await;
        router.notification_sink.stream_terminal_admission = RpcStreamTerminalAdmission::new(1);
        if matches!(&terminal, StreamTerminal::Session(_)) {
            let (tx, rx) = mpsc::channel(1);
            router.notification_sink.tx = tx;
            notif_rx = rx;
        }
        let router = Arc::new(router);
        let stream_ref = StreamRef::mint();
        let (streams, close_method, terminal_method) = match &terminal {
            StreamTerminal::Session(session_id) => {
                resolve_session_stream_open_authority(
                    &router.stream_authority,
                    &stream_ref,
                    session_id,
                )
                .await
                .unwrap();
                (
                    router.active_session_streams.clone(),
                    "session/stream_close",
                    "session/stream_end",
                )
            }
            #[cfg(feature = "mob")]
            StreamTerminal::Mob => {
                resolve_mob_stream_open_authority(&router.stream_authority, &stream_ref)
                    .await
                    .unwrap();
                (
                    router.active_mob_streams.clone(),
                    "mob/stream_close",
                    "mob/stream_end",
                )
            }
        };
        streams.lock().await.insert(
            stream_ref,
            reserved_test_stream_forwarder(&router.notification_sink, terminal.clone()),
        );
        // Session close atomically transfers its terminal reservation to an
        // owned sender. Hold it at the channel; mob's existing boundary is the
        // map claim after machine authorization. Repeated closes must not take
        // the reservation from either issuer.
        let streams_guard: Option<
            tokio::sync::MutexGuard<'_, HashMap<StreamRef, StreamForwarder>>,
        > = match &terminal {
            StreamTerminal::Session(session_id) => {
                assert_eq!(
                    router
                        .notification_sink
                        .emit_session_stream_event(
                            &stream_ref,
                            1,
                            session_id,
                            &recovery_envelope(session_id, 1),
                        )
                        .await,
                    StreamEmitStatus::Delivered
                );
                None
            }
            #[cfg(feature = "mob")]
            StreamTerminal::Mob => Some(streams.lock().await),
        };
        let first_close = tokio::spawn({
            let router = router.clone();
            async move {
                router
                    .dispatch(make_request(
                        close_method,
                        serde_json::json!({ "stream_id": stream_ref.as_string() }),
                    ))
                    .await
                    .unwrap()
            }
        });
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let closed = {
                    let authority = router.stream_authority.lock().await;
                    match &terminal {
                        StreamTerminal::Session(_) => authority
                            .state()
                            .closed_session_event_streams
                            .contains(&stream_ref.as_string()),
                        #[cfg(feature = "mob")]
                        StreamTerminal::Mob => authority
                            .state()
                            .closed_mob_event_streams
                            .contains(&stream_ref.as_string()),
                    }
                };
                if closed {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("first close has the machine terminal effect");
        let repeated_close = tokio::time::timeout(
            Duration::from_secs(1),
            router.dispatch(make_request(
                close_method,
                serde_json::json!({ "stream_id": stream_ref.as_string() }),
            )),
        )
        .await
        .expect("already-closed caller must not compete for terminal custody")
        .unwrap();
        assert_eq!(result_value(&repeated_close)["closed"], true);
        assert_eq!(result_value(&repeated_close)["already_closed"], true);
        if let Some(streams_guard) = &streams_guard {
            assert!(streams_guard.contains_key(&stream_ref));
        } else {
            assert!(
                streams.lock().await.is_empty(),
                "owned sender has the reservation"
            );
        }
        assert!(!first_close.is_finished());
        assert!(
            router
                .notification_sink
                .stream_terminal_admission
                .try_reserve()
                .is_err()
        );
        drop(streams_guard);
        if matches!(&terminal, StreamTerminal::Session(_)) {
            assert_eq!(
                notif_rx.recv().await.unwrap().method,
                "session/stream_event"
            );
        }
        let first_close = tokio::time::timeout(Duration::from_secs(1), first_close)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(result_value(&first_close)["closed"], true);
        assert_eq!(result_value(&first_close)["already_closed"], false);
        let notification = notif_rx
            .try_recv()
            .expect("canonical explicit-close terminal");
        assert_eq!(notification.method, terminal_method);
        assert_eq!(
            notification_params!(notification)["stream_id"],
            stream_ref.as_string()
        );
        assert_eq!(
            notification_params!(notification)["outcome"],
            "explicit_close"
        );
        assert!(streams.lock().await.is_empty());
        assert!(
            notif_rx.try_recv().is_err(),
            "only the effect owner emits a terminal"
        );
        assert!(
            router
                .notification_sink
                .stream_terminal_admission
                .try_reserve()
                .is_err()
        );
        drop(notification);
        assert!(
            router
                .notification_sink
                .stream_terminal_admission
                .try_reserve()
                .is_ok()
        );
    }

    #[tokio::test]
    async fn session_stream_idempotent_close_preserves_pending_explicit_terminal() {
        assert_idempotent_close_preserves_pending_terminal(StreamTerminal::Session(
            SessionId::new(),
        ))
        .await;
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn mob_stream_idempotent_close_preserves_pending_explicit_terminal() {
        assert_idempotent_close_preserves_pending_terminal(StreamTerminal::Mob).await;
    }

    #[tokio::test]
    async fn stream_task_attachment_preserves_pending_terminal_delivery() {
        let (tx, mut rx) = mpsc::channel(1);
        let mut sink = NotificationSink::new(tx);
        sink.stream_terminal_admission = RpcStreamTerminalAdmission::new(1);
        let authority = Arc::new(Mutex::new(new_rpc_stream_authority()));
        let active_streams = Arc::new(Mutex::new(HashMap::new()));
        let session_id = SessionId::new();
        let stream_id = StreamRef::mint();
        active_streams.lock().await.insert(
            stream_id,
            reserved_test_stream_forwarder(&sink, StreamTerminal::Session(session_id.clone())),
        );
        resolve_session_stream_open_authority(&authority, &stream_id, &session_id)
            .await
            .unwrap();
        assert!(sink.emit_notification("synthetic/occupied", &serde_json::json!({})));
        let task = tokio::spawn({
            let sink = sink.clone();
            let authority = authority.clone();
            let active_streams = active_streams.clone();
            async move {
                emit_authorized_session_stream_terminal(&sink, &active_streams, &authority,
                    stream_id, meerkat_runtime::meerkat_machine::dsl::RpcEventStreamTerminalObservationKind::TransportEnded, None).await;
            }
        });
        tokio::time::timeout(Duration::from_secs(1), async {
            while !active_streams.lock().await.is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("forwarder reaches terminal send before handle attachment");
        attach_stream_forwarder_task(&active_streams, stream_id, task).await;
        drop(rx.recv().await.expect("occupied notification"));
        let terminal = tokio::time::timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("late task attachment must not abort terminal delivery")
            .unwrap();
        assert_eq!(terminal.method, "session/stream_end");
        assert_eq!(notification_params!(terminal)["outcome"], "remote_end");
        drop(terminal);
        assert!(sink.stream_terminal_admission.try_reserve().is_ok());
    }

    #[tokio::test]
    async fn closed_transport_releases_terminal_reservation() {
        let (tx, rx) = mpsc::channel(1);
        drop(rx);
        let mut sink = NotificationSink::new(tx);
        sink.stream_terminal_admission = RpcStreamTerminalAdmission::new(1);
        let authority = Arc::new(Mutex::new(new_rpc_stream_authority()));
        let active_streams = Arc::new(Mutex::new(HashMap::new()));
        let session_id = SessionId::new();
        let stream_id = StreamRef::mint();
        active_streams.lock().await.insert(
            stream_id,
            reserved_test_stream_forwarder(&sink, StreamTerminal::Session(session_id.clone())),
        );
        resolve_session_stream_open_authority(&authority, &stream_id, &session_id)
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(1), emit_authorized_session_stream_terminal(
            &sink, &active_streams, &authority, stream_id,
            meerkat_runtime::meerkat_machine::dsl::RpcEventStreamTerminalObservationKind::NotificationReceiverGone,
            Some("transport receiver closed".to_string()))).await.expect("closed transport cannot retain the slot");
        assert!(active_streams.lock().await.is_empty());
        assert!(sink.stream_terminal_admission.try_reserve().is_ok());
    }

    #[tokio::test]
    async fn session_stream_terminal_survives_production_count_and_byte_admission_saturation() {
        for (message_count, wire_bytes) in [(256, 1), (8, 8 * 1024 * 1024)] {
            // Exercise the real production byte/count admission, isolated so
            // parallel tests do not starve each other's process-global pool.
            // The notification queue itself has normal production capacity.
            let admission = RpcOutboundAdmission::isolated_production();
            let (tx, mut rx) = mpsc::channel(crate::NOTIFICATION_CHANNEL_CAPACITY);
            let mut sink = NotificationSink::new(tx);
            sink.outbound_admission = admission.clone();
            sink.stream_terminal_admission = RpcStreamTerminalAdmission::new(1);
            let authority = Arc::new(Mutex::new(new_rpc_stream_authority()));
            let active_streams = Arc::new(Mutex::new(HashMap::new()));
            let session_id = SessionId::new();
            let stream_id = StreamRef::mint();
            active_streams.lock().await.insert(
                stream_id,
                reserved_test_stream_forwarder(&sink, StreamTerminal::Session(session_id.clone())),
            );
            resolve_session_stream_open_authority(&authority, &stream_id, &session_id)
                .await
                .expect("stream admitted before data saturation");
            let held: Vec<_> = (0..message_count)
                .map(|_| {
                    admission
                        .try_admit(wire_bytes)
                        .expect("production data permit")
                })
                .collect();
            assert!(matches!(
                admission.try_admit(1),
                Err(crate::protocol::RpcOutboundAdmissionError::Backpressured { .. })
            ));
            let event = EventEnvelope::new_session(
                session_id.clone(),
                1,
                None,
                AgentEvent::TextDelta {
                    delta: "synthetic burst".to_string(),
                    assistant_message_id: None,
                },
            );
            let emit = tokio::time::timeout(
                Duration::from_secs(1),
                sink.emit_session_stream_event(&stream_id, 1, &session_id, &event),
            )
            .await
            .expect("data producer must not wait for admission");
            assert_eq!(emit, StreamEmitStatus::Overflow);
            let (observation, detail) = terminal_observation_from_emit_status(emit).unwrap();
            tokio::time::timeout(
                Duration::from_secs(1),
                emit_authorized_session_stream_terminal(
                    &sink,
                    &active_streams,
                    &authority,
                    stream_id,
                    observation,
                    Some(detail.to_string()),
                ),
            )
            .await
            .expect("reserved terminal can queue while all data permits remain held");
            let notification = rx.try_recv().expect("overflow has an observable terminal");
            assert_eq!(notification.method, "session/stream_end");
            assert_eq!(
                notification_params!(notification)["outcome"],
                "terminal_error"
            );
            assert_eq!(
                notification_params!(notification)["error"]["code"],
                "stream_queue_overflow"
            );
            assert!(active_streams.lock().await.is_empty());
            assert!(
                admission.try_admit(1).is_err(),
                "test still holds the full data budget"
            );
            assert!(
                sink.stream_terminal_admission.try_reserve().is_err(),
                "queued terminal retains its slot"
            );
            drop(notification);
            assert!(sink.stream_terminal_admission.try_reserve().is_ok());
            drop(held);
        }
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn mob_stream_terminal_survives_production_admission_saturation() {
        let admission = RpcOutboundAdmission::isolated_production();
        let (tx, mut rx) = mpsc::channel(crate::NOTIFICATION_CHANNEL_CAPACITY);
        let mut sink = NotificationSink::new(tx);
        sink.outbound_admission = admission.clone();
        sink.stream_terminal_admission = RpcStreamTerminalAdmission::new(1);
        let authority = Arc::new(Mutex::new(new_rpc_stream_authority()));
        let active_streams = Arc::new(Mutex::new(HashMap::new()));
        let stream_id = StreamRef::mint();
        active_streams.lock().await.insert(
            stream_id,
            reserved_test_stream_forwarder(&sink, StreamTerminal::Mob),
        );
        resolve_mob_stream_open_authority(&authority, &stream_id)
            .await
            .expect("admitted mob stream");
        let held: Vec<_> = (0..256)
            .map(|_| admission.try_admit(1).expect("data permit"))
            .collect();
        let emit = sink
            .emit_mob_stream_event(&stream_id, 1, &serde_json::json!({"type": "synthetic"}))
            .await;
        assert_eq!(emit, StreamEmitStatus::Overflow);
        let (observation, detail) = terminal_observation_from_emit_status(emit).unwrap();
        tokio::time::timeout(
            Duration::from_secs(1),
            emit_authorized_mob_stream_terminal(
                &sink,
                &active_streams,
                &authority,
                stream_id,
                observation,
                Some(detail.to_string()),
            ),
        )
        .await
        .expect("mob terminal bypasses saturated data admission");
        let terminal = rx.try_recv().expect("mob terminal queued");
        assert_eq!(terminal.method, "mob/stream_end");
        assert_eq!(
            notification_params!(terminal)["error"]["code"],
            "stream_queue_overflow"
        );
        assert!(admission.try_admit(1).is_err());
        assert!(active_streams.lock().await.is_empty());
        drop(terminal);
        assert!(sink.stream_terminal_admission.try_reserve().is_ok());
        drop(held);
    }

    #[tokio::test]
    async fn session_stream_open_refuses_unfunded_terminals_and_reuses_written_capacity() {
        let (mut router, mut rx) = test_router().await;
        router.notification_sink.stream_terminal_admission = RpcStreamTerminalAdmission::new(1);
        let create = router
            .dispatch(make_request(
                "session/create",
                serde_json::json!({
                    "prompt": "synthetic deferred stream", "initial_turn": "deferred"
                }),
            ))
            .await
            .unwrap();
        let session_id = result_value(&create)["session_id"]
            .as_str()
            .unwrap()
            .to_string();
        let open = router
            .dispatch(make_request(
                "session/stream_open",
                serde_json::json!({"session_id": session_id}),
            ))
            .await
            .unwrap();
        let stream_id = result_value(&open)["stream_id"]
            .as_str()
            .unwrap()
            .to_string();
        let rejected = router
            .dispatch(make_request(
                "session/stream_open",
                serde_json::json!({"session_id": session_id}),
            ))
            .await
            .unwrap();
        assert_eq!(error_code(&rejected), error::BUDGET_EXHAUSTED);
        assert_eq!(router.active_session_streams.lock().await.len(), 1);
        let close = router
            .dispatch(make_request(
                "session/stream_close",
                serde_json::json!({"stream_id": stream_id}),
            ))
            .await
            .unwrap();
        assert_eq!(result_value(&close)["closed"], true);
        let terminal = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let notification = rx.recv().await.expect("notification receiver");
                if notification.method == "session/stream_end" {
                    break notification;
                }
            }
        })
        .await
        .expect("explicit close terminal");
        let still_full = router
            .dispatch(make_request(
                "session/stream_open",
                serde_json::json!({"session_id": session_id}),
            ))
            .await
            .unwrap();
        assert_eq!(error_code(&still_full), error::BUDGET_EXHAUSTED);
        drop(terminal);
        let reopened = router
            .dispatch(make_request(
                "session/stream_open",
                serde_json::json!({"session_id": session_id}),
            ))
            .await
            .unwrap();
        assert!(
            reopened.error.is_none(),
            "written terminal releases admission"
        );
        let new_id = result_value(&reopened)["stream_id"]
            .as_str()
            .unwrap()
            .to_string();
        router
            .dispatch(make_request(
                "session/stream_close",
                serde_json::json!({"stream_id": new_id}),
            ))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn disconnected_idle_session_stream_releases_terminal_capacity() {
        let (mut router, rx) = test_router().await;
        router.notification_sink.stream_terminal_admission = RpcStreamTerminalAdmission::new(1);
        let create = router
            .dispatch(make_request(
                "session/create",
                serde_json::json!({
                    "prompt": "synthetic idle stream", "initial_turn": "deferred"
                }),
            ))
            .await
            .unwrap();
        let session_id = result_value(&create)["session_id"]
            .as_str()
            .unwrap()
            .to_string();
        let open = router
            .dispatch(make_request(
                "session/stream_open",
                serde_json::json!({"session_id": session_id}),
            ))
            .await
            .unwrap();
        assert!(open.error.is_none());
        assert!(
            router
                .notification_sink
                .stream_terminal_admission
                .try_reserve()
                .is_err()
        );
        drop(rx);
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if router
                    .notification_sink
                    .stream_terminal_admission
                    .try_reserve()
                    .is_ok()
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("disconnect releases capacity even without another source event");
        assert!(router.active_session_streams.lock().await.is_empty());
    }

    #[tokio::test]
    async fn session_stream_overflow_emits_terminal_error_outcome() {
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        let sink = NotificationSink::new(tx);
        let authority = Arc::new(Mutex::new(new_rpc_stream_authority()));
        let active_streams = Arc::new(Mutex::new(HashMap::new()));
        let session_id = SessionId::new();
        let stream_id = StreamRef::mint();
        active_streams.lock().await.insert(
            stream_id,
            reserved_test_stream_forwarder(&sink, StreamTerminal::Session(session_id.clone())),
        );
        resolve_session_stream_open_authority(&authority, &stream_id, &session_id)
            .await
            .expect("machine should accept session stream open");
        let envelope = EventEnvelope::new_session(
            session_id.clone(),
            1,
            None,
            AgentEvent::TextDelta {
                delta: "hello".to_string(),
                assistant_message_id: None,
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
            let authority = authority.clone();
            let active_streams = active_streams.clone();
            async move {
                emit_authorized_session_stream_terminal(
                    &sink,
                    &active_streams,
                    &authority,
                    stream_id,
                    meerkat_runtime::meerkat_machine::dsl::RpcEventStreamTerminalObservationKind::NotificationQueueOverflow,
                    Some("transport stream notification queue overflow".to_string()),
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
        assert_eq!(
            notification_params!(terminal_notification)["outcome"],
            "terminal_error"
        );
        assert_eq!(
            notification_params!(terminal_notification)["error"]["code"],
            "stream_queue_overflow"
        );
    }

    #[tokio::test]
    async fn session_stream_receiver_gone_records_machine_terminal() {
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        drop(rx);
        let sink = NotificationSink::new(tx);
        let authority = Arc::new(Mutex::new(new_rpc_stream_authority()));
        let session_id = SessionId::new();
        let stream_id = StreamRef::mint();
        resolve_session_stream_open_authority(&authority, &stream_id, &session_id)
            .await
            .expect("machine should accept session stream open");
        let envelope = EventEnvelope::new_session(
            session_id.clone(),
            1,
            None,
            AgentEvent::TextDelta {
                delta: "hello".to_string(),
                assistant_message_id: None,
            },
        );

        let emit_status = sink
            .emit_session_stream_event(&stream_id, 1, &session_id, &envelope)
            .await;
        assert_eq!(emit_status, StreamEmitStatus::ReceiverGone);
        let (observation, detail) = terminal_observation_from_emit_status(emit_status)
            .expect("closed receiver should lower into a terminal observation");
        assert_eq!(
            observation,
            meerkat_runtime::meerkat_machine::dsl::RpcEventStreamTerminalObservationKind::NotificationReceiverGone
        );
        let terminal = record_session_stream_terminal_authority(
            &authority,
            &stream_id,
            observation,
            Some(detail.to_string()),
        )
        .await
        .expect("machine should derive receiver-gone terminal class");
        assert_eq!(
            terminal.reason,
            meerkat_runtime::meerkat_machine::dsl::RpcEventStreamTerminalReason::TerminalError
        );
        assert_eq!(
            terminal.error_code,
            Some(
                meerkat_runtime::meerkat_machine::dsl::RpcEventStreamTerminalErrorCode::StreamReceiverGone
            )
        );
    }

    #[tokio::test]
    async fn pending_session_stream_recovers_after_overflow_and_follows_journal_bridge() {
        let requests = Arc::new(std::sync::Mutex::new(Vec::<Vec<Message>>::new()));
        let llm_client = Arc::new(RecordingMockLlmClient::new(requests));
        let (mut router, mut rx) = test_router_with_llm_and_notification_capacity(
            llm_client,
            crate::NOTIFICATION_CHANNEL_CAPACITY,
        )
        .await;
        let admission = RpcOutboundAdmission::isolated_production();
        router.notification_sink.outbound_admission = admission.clone();
        // The runtime holds its own handle on the connection's sink, installed
        // at router construction; in production both share one admission, so
        // the turns' session/event copies meet the same full data budget.
        router
            .runtime
            .set_notification_sink(router.notification_sink.clone());
        let created = router
            .dispatch(make_request(
                "session/create",
                serde_json::json!({
                    "prompt": "pending stream recovery", "initial_turn": "deferred"
                }),
            ))
            .await
            .unwrap();
        let session_id = result_value(&created)["session_id"]
            .as_str()
            .unwrap()
            .to_string();
        let opened = router
            .dispatch(make_request(
                "session/stream_open",
                serde_json::json!({
                    "session_id": session_id
                }),
            ))
            .await
            .unwrap();
        let stream_id =
            StreamRef::parse(result_value(&opened)["stream_id"].as_str().unwrap()).unwrap();
        let delivery = router.active_session_streams.lock().await[&stream_id]
            .session_delivery
            .clone()
            .unwrap();
        let held: Vec<_> = (0..256).map(|_| admission.try_admit(1).unwrap()).collect();
        let first = router
            .dispatch(make_request(
                "turn/start",
                serde_json::json!({
                    "session_id": session_id, "prompt": "first pending turn"
                }),
            ))
            .await
            .unwrap();
        assert!(first.error.is_none());
        tokio::time::timeout(Duration::from_secs(2), async {
            while delivery.lock().await.dropped == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the real pending forwarder observes admission loss");
        assert!(
            router
                .active_session_streams
                .lock()
                .await
                .contains_key(&stream_id)
        );
        assert!(
            rx.try_recv().is_err(),
            "no premature terminal while data budget is full"
        );
        drop(held);
        let second = router
            .dispatch(make_request(
                "turn/start",
                serde_json::json!({
                    "session_id": session_id, "prompt": "journal bridge recovery"
                }),
            ))
            .await
            .unwrap();
        assert!(second.error.is_none());
        let mut saw_gap = false;
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let notification = rx.recv().await.unwrap();
                assert_ne!(notification.method, "session/stream_end");
                if notification.method != "session/stream_event" {
                    continue;
                }
                let params = notification_params!(notification);
                let event = &params["event"];
                if event["payload"]["type"] == "stream_truncated" {
                    assert_eq!(event["seq"], 0);
                    assert!(event["payload"]["reason"]["dropped"].as_u64().unwrap() > 0);
                    saw_gap = true;
                }
                if event["payload"]["type"] == "run_started"
                    && event["payload"]["input"]["content"] == "journal bridge recovery"
                {
                    assert!(saw_gap, "the gap precedes the bridged turn's event");
                    break;
                }
            }
        })
        .await
        .expect("same pending subscription follows the later journal turn");
        let close = router
            .dispatch(make_request(
                "session/stream_close",
                serde_json::json!({
                    "stream_id": stream_id.as_string()
                }),
            ))
            .await
            .unwrap();
        assert!(close.error.is_none());
    }

    fn recovery_envelope(session_id: &SessionId, seq: u64) -> EventEnvelope<AgentEvent> {
        EventEnvelope::new_session(
            session_id.clone(),
            seq,
            None,
            AgentEvent::TextDelta {
                delta: format!("event-{seq}"),
                assistant_message_id: None,
            },
        )
    }

    async fn recovery_forwarder(
        sink: NotificationSink,
    ) -> (
        SessionStreamForwarding,
        mpsc::Sender<EventEnvelope<AgentEvent>>,
        oneshot::Receiver<()>,
        meerkat_core::comms::EventStream,
    ) {
        let authority = Arc::new(Mutex::new(new_rpc_stream_authority()));
        let streams = Arc::new(Mutex::new(HashMap::new()));
        let session_id = SessionId::new();
        let stream_id = StreamRef::mint();
        let mut entry =
            reserved_test_stream_forwarder(&sink, StreamTerminal::Session(session_id.clone()));
        let delivery = entry.session_delivery.clone().unwrap();
        let (stop_tx, stop_rx) = oneshot::channel();
        entry.state = StreamForwarderState::Active {
            stop_tx: Some(stop_tx),
            task: None,
        };
        streams.lock().await.insert(stream_id, entry);
        resolve_session_stream_open_authority(&authority, &stream_id, &session_id)
            .await
            .unwrap();
        let (events_tx, events_rx) = mpsc::channel(8);
        let stream = Box::pin(futures::stream::unfold(events_rx, |mut rx| async {
            rx.recv().await.map(|event| (event, rx))
        }));
        (
            SessionStreamForwarding {
                notification_sink: sink,
                active_session_streams: streams,
                stream_authority: authority,
                stream_id,
                session_id,
                delivery,
            },
            events_tx,
            stop_rx,
            stream,
        )
    }

    async fn await_recovery_drop(delivery: &Mutex<SessionStreamDelivery>, count: u64) {
        tokio::time::timeout(Duration::from_secs(2), async {
            while delivery.lock().await.dropped != count {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("forwarder consumes the refused source event without waiting for permits");
    }

    #[tokio::test]
    async fn session_stream_recovery_survives_count_and_byte_admission_saturation() {
        for (message_count, wire_bytes) in [(256, 1), (8, 8 * 1024 * 1024)] {
            let admission = RpcOutboundAdmission::isolated_production();
            let (tx, mut rx) = mpsc::channel(crate::NOTIFICATION_CHANNEL_CAPACITY);
            let mut sink = NotificationSink::new(tx);
            sink.outbound_admission = admission.clone();
            sink.stream_terminal_admission = RpcStreamTerminalAdmission::new(1);
            let (forwarder, events, stop_rx, stream) = recovery_forwarder(sink.clone()).await;
            let session_id = forwarder.session_id.clone();
            let delivery = forwarder.delivery.clone();
            let streams = forwarder.active_session_streams.clone();
            let task = tokio::spawn(forwarder.run(stream, stop_rx));
            let mut held: Vec<_> = (0..message_count)
                .map(|_| admission.try_admit(wire_bytes).unwrap())
                .collect();
            events
                .send(recovery_envelope(&session_id, 1))
                .await
                .unwrap();
            await_recovery_drop(&delivery, 1).await;
            assert_eq!(streams.lock().await.len(), 1);
            assert!(rx.try_recv().is_err());
            // The idle forwarder does not reserve the first released permit or
            // await admission ahead of ordinary responses/callback requests.
            drop(held.pop());
            let response = RpcResponse::success_with_admission(
                Some(crate::protocol::RpcId::Num(9)),
                serde_json::json!({"ok":true}),
                admission.clone(),
            );
            assert!(response.error.is_none());
            drop(response);
            drop(held);
            events
                .send(recovery_envelope(&session_id, 2))
                .await
                .unwrap();
            let marker = tokio::time::timeout(Duration::from_secs(2), rx.recv())
                .await
                .unwrap()
                .unwrap();
            let event = tokio::time::timeout(Duration::from_secs(2), rx.recv())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(marker.method, "session/stream_event");
            assert_eq!(event.method, "session/stream_event");
            assert_eq!(notification_params!(marker)["sequence"], 2);
            assert_eq!(notification_params!(marker)["event"]["seq"], 0);
            assert_eq!(
                notification_params!(marker)["event"]["payload"],
                serde_json::json!({
                    "type":"stream_truncated", "reason":{"kind":"stream_lagged","dropped":1}
                })
            );
            assert_eq!(notification_params!(event)["sequence"], 3);
            assert_eq!(notification_params!(event)["event"]["seq"], 2);
            assert_eq!(delivery.lock().await.dropped, 0);
            assert_eq!(streams.lock().await.len(), 1);
            drop(events);
            let terminal = tokio::time::timeout(Duration::from_secs(2), rx.recv())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(notification_params!(terminal)["outcome"], "remote_end");
            task.await.unwrap();
        }
    }

    #[tokio::test]
    async fn session_stream_unreported_gap_uses_reserved_error_terminal_when_source_ends() {
        let admission = RpcOutboundAdmission::isolated_production();
        let (tx, mut rx) = mpsc::channel(crate::NOTIFICATION_CHANNEL_CAPACITY);
        let mut sink = NotificationSink::new(tx);
        sink.outbound_admission = admission.clone();
        sink.stream_terminal_admission = RpcStreamTerminalAdmission::new(1);
        let (forwarder, events, stop_rx, stream) = recovery_forwarder(sink.clone()).await;
        let session_id = forwarder.session_id.clone();
        let delivery = forwarder.delivery.clone();
        let streams = forwarder.active_session_streams.clone();
        let task = tokio::spawn(forwarder.run(stream, stop_rx));
        let held: Vec<_> = (0..256).map(|_| admission.try_admit(1).unwrap()).collect();
        events
            .send(recovery_envelope(&session_id, 1))
            .await
            .unwrap();
        await_recovery_drop(&delivery, 1).await;
        events
            .send(recovery_envelope(&session_id, 2))
            .await
            .unwrap();
        await_recovery_drop(&delivery, 2).await;
        drop(events);
        let terminal = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(terminal.method, "session/stream_end");
        assert_eq!(notification_params!(terminal)["outcome"], "terminal_error");
        assert_eq!(
            notification_params!(terminal)["error"]["code"],
            "stream_queue_overflow"
        );
        assert!(
            notification_params!(terminal)["error"]["message"]
                .as_str()
                .unwrap()
                .contains("2 event(s)")
        );
        assert!(admission.try_admit(1).is_err());
        assert!(streams.lock().await.is_empty());
        task.await.unwrap();
        drop(terminal);
        assert!(sink.stream_terminal_admission.try_reserve().is_ok());
        drop(held);
    }

    #[tokio::test]
    async fn session_stream_repeated_marker_refusal_counts_only_source_events() {
        let (tx, mut rx) = mpsc::channel(1);
        let sink = NotificationSink::new(tx);
        let session_id = SessionId::new();
        let stream_id = StreamRef::mint();
        let mut delivery = SessionStreamDelivery::default();
        assert_eq!(
            delivery
                .emit(
                    &sink,
                    &stream_id,
                    &session_id,
                    &recovery_envelope(&session_id, 1)
                )
                .await,
            Some(StreamEmitStatus::Delivered)
        );
        for seq in 2..=4 {
            assert_eq!(
                delivery
                    .emit(
                        &sink,
                        &stream_id,
                        &session_id,
                        &recovery_envelope(&session_id, seq)
                    )
                    .await,
                Some(StreamEmitStatus::Overflow)
            );
        }
        assert_eq!(delivery.dropped, 3);
        drop(rx.recv().await.unwrap());
        // The marker occupies the only channel slot. Its following real event
        // is refused and starts a new, separate one-event gap.
        assert_eq!(
            delivery
                .emit(
                    &sink,
                    &stream_id,
                    &session_id,
                    &recovery_envelope(&session_id, 5)
                )
                .await,
            Some(StreamEmitStatus::Overflow)
        );
        let marker = rx.recv().await.unwrap();
        assert_eq!(
            notification_params!(marker)["event"]["payload"]["reason"]["dropped"],
            3
        );
        assert_eq!(delivery.dropped, 1);
        delivery.closing = true;
        assert_eq!(
            delivery
                .emit(
                    &sink,
                    &stream_id,
                    &session_id,
                    &recovery_envelope(&session_id, 6)
                )
                .await,
            None
        );
        assert_eq!(delivery.dropped, 1);
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn session_stream_close_waits_for_emission_and_reports_pending_gap_once() {
        let (router, mut rx) = test_router().await;
        let router = Arc::new(router);
        let session_id = SessionId::new();
        let stream_id = StreamRef::mint();
        let entry = reserved_test_stream_forwarder(
            &router.notification_sink,
            StreamTerminal::Session(session_id.clone()),
        );
        let delivery = entry.session_delivery.clone().unwrap();
        router
            .active_session_streams
            .lock()
            .await
            .insert(stream_id, entry);
        resolve_session_stream_open_authority(&router.stream_authority, &stream_id, &session_id)
            .await
            .unwrap();
        let mut emission = delivery.lock().await;
        let (started_tx, started_rx) = oneshot::channel();
        let close = tokio::spawn({
            let router = router.clone();
            async move {
                started_tx.send(()).unwrap();
                router
                    .dispatch(make_request(
                        "session/stream_close",
                        serde_json::json!({
                            "stream_id":stream_id.as_string()
                        }),
                    ))
                    .await
                    .unwrap()
            }
        });
        started_rx.await.unwrap();
        tokio::task::yield_now().await;
        assert!(
            !close.is_finished(),
            "close cannot race past admission accounting"
        );
        emission.dropped = 2;
        drop(emission);
        let response = close.await.unwrap();
        assert!(response.error.is_none());
        let terminal = rx.recv().await.unwrap();
        assert_eq!(terminal.method, "session/stream_end");
        assert_eq!(notification_params!(terminal)["outcome"], "terminal_error");
        assert_eq!(
            notification_params!(terminal)["error"]["code"],
            "stream_queue_overflow"
        );
        assert!(
            notification_params!(terminal)["error"]["message"]
                .as_str()
                .unwrap()
                .contains("2 event(s)")
        );
        assert!(delivery.lock().await.closing);
        let repeated = router
            .dispatch(make_request(
                "session/stream_close",
                serde_json::json!({
                    "stream_id":stream_id.as_string()
                }),
            ))
            .await
            .unwrap();
        assert_eq!(result_value(&repeated)["already_closed"], true);
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn canceled_session_close_before_authority_does_not_seal_delivery() {
        let (router, mut rx) = test_router().await;
        let router = Arc::new(router);
        let session_id = SessionId::new();
        let stream_id = StreamRef::mint();
        let entry = reserved_test_stream_forwarder(
            &router.notification_sink,
            StreamTerminal::Session(session_id.clone()),
        );
        let delivery = entry.session_delivery.clone().unwrap();
        router
            .active_session_streams
            .lock()
            .await
            .insert(stream_id, entry);
        resolve_session_stream_open_authority(&router.stream_authority, &stream_id, &session_id)
            .await
            .unwrap();
        let mut emission = delivery.lock().await;
        emission.dropped = 2;
        let mut close = Box::pin(close_session_stream(
            &router.notification_sink,
            &router.active_session_streams,
            &router.stream_authority,
            &stream_id,
        ));
        // Poll the production future to the two exact cancellation boundaries.
        // A mutex wakeup alone cannot prove its waiter has resumed execution.
        assert!(futures::poll!(close.as_mut()).is_pending());
        let authority = router.stream_authority.lock().await;
        drop(emission);
        assert!(futures::poll!(close.as_mut()).is_pending());
        drop(close);
        let emission = delivery.lock().await;
        assert!(!emission.closing);
        assert_eq!(emission.dropped, 2);
        assert!(
            authority
                .state()
                .active_session_event_streams
                .contains(&stream_id.as_string())
        );
        assert!(
            router
                .active_session_streams
                .lock()
                .await
                .contains_key(&stream_id)
        );
        assert!(rx.try_recv().is_err());
        drop(emission);
        drop(authority);
        // The still-open stream can deliver its pending marker and source event.
        assert_eq!(
            delivery
                .lock()
                .await
                .emit(
                    &router.notification_sink,
                    &stream_id,
                    &session_id,
                    &recovery_envelope(&session_id, 3),
                )
                .await,
            Some(StreamEmitStatus::Delivered)
        );
        assert_eq!(
            notification_params!(rx.recv().await.unwrap())["event"]["seq"],
            0
        );
        assert_eq!(
            notification_params!(rx.recv().await.unwrap())["event"]["seq"],
            3
        );
        close_session_stream(
            &router.notification_sink,
            &router.active_session_streams,
            &router.stream_authority,
            &stream_id,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn canceled_session_close_keeps_owned_reserved_gap_terminal() {
        let (mut router, _) = test_router().await;
        let (tx, mut rx) = mpsc::channel(1);
        router.notification_sink.tx = tx;
        router.notification_sink.stream_terminal_admission = RpcStreamTerminalAdmission::new(1);
        let router = Arc::new(router);
        let session_id = SessionId::new();
        let stream_id = StreamRef::mint();
        let entry = reserved_test_stream_forwarder(
            &router.notification_sink,
            StreamTerminal::Session(session_id.clone()),
        );
        let delivery = entry.session_delivery.clone().unwrap();
        delivery.lock().await.dropped = 3;
        router
            .active_session_streams
            .lock()
            .await
            .insert(stream_id, entry);
        resolve_session_stream_open_authority(&router.stream_authority, &stream_id, &session_id)
            .await
            .unwrap();
        assert_eq!(
            router
                .notification_sink
                .emit_session_stream_event(
                    &stream_id,
                    1,
                    &session_id,
                    &recovery_envelope(&session_id, 1),
                )
                .await,
            StreamEmitStatus::Delivered
        );
        let close = tokio::spawn({
            let router = router.clone();
            async move {
                router
                    .dispatch(make_request(
                        "session/stream_close",
                        serde_json::json!({
                            "stream_id": stream_id.as_string(),
                        }),
                    ))
                    .await
            }
        });
        tokio::time::timeout(Duration::from_secs(1), async {
            while router
                .active_session_streams
                .lock()
                .await
                .contains_key(&stream_id)
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("terminal custody transferred to reserved sender");
        assert!(
            router
                .stream_authority
                .lock()
                .await
                .state()
                .closed_session_event_streams
                .contains(&stream_id.as_string())
        );
        assert!(
            !close.is_finished(),
            "reserved sender awaits the slow reader"
        );
        close.abort();
        assert!(close.await.unwrap_err().is_cancelled());
        let repeated = close_session_stream(
            &router.notification_sink,
            &router.active_session_streams,
            &router.stream_authority,
            &stream_id,
        )
        .await
        .unwrap();
        assert!(repeated.already_closed);
        assert!(
            router
                .notification_sink
                .stream_terminal_admission
                .try_reserve()
                .is_err()
        );
        assert_eq!(rx.recv().await.unwrap().method, "session/stream_event");
        let terminal = tokio::time::timeout(Duration::from_secs(1), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(notification_params!(terminal)["outcome"], "terminal_error");
        assert_eq!(
            notification_params!(terminal)["error"]["code"],
            "stream_queue_overflow"
        );
        assert!(
            notification_params!(terminal)["error"]["message"]
                .as_str()
                .unwrap()
                .contains("3 event(s)")
        );
        assert!(rx.try_recv().is_err());
        drop(terminal);
        assert!(
            router
                .notification_sink
                .stream_terminal_admission
                .try_reserve()
                .is_ok()
        );
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn mob_stream_overflow_emits_terminal_error_outcome() {
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        let sink = NotificationSink::new(tx);
        let authority = Arc::new(Mutex::new(new_rpc_stream_authority()));
        let active_streams = Arc::new(Mutex::new(HashMap::new()));
        let stream_id = StreamRef::mint();
        active_streams.lock().await.insert(
            stream_id,
            reserved_test_stream_forwarder(&sink, StreamTerminal::Mob),
        );
        resolve_mob_stream_open_authority(&authority, &stream_id)
            .await
            .expect("machine should accept mob stream open");
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
            let authority = authority.clone();
            let active_streams = active_streams.clone();
            async move {
                emit_authorized_mob_stream_terminal(
                    &sink,
                    &active_streams,
                    &authority,
                    stream_id,
                    meerkat_runtime::meerkat_machine::dsl::RpcEventStreamTerminalObservationKind::NotificationQueueOverflow,
                    Some("transport stream notification queue overflow".to_string()),
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
        assert_eq!(
            notification_params!(terminal_notification)["outcome"],
            "terminal_error"
        );
        assert_eq!(
            notification_params!(terminal_notification)["error"]["code"],
            "stream_queue_overflow"
        );
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn mob_stream_receiver_gone_records_machine_terminal() {
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        drop(rx);
        let sink = NotificationSink::new(tx);
        let authority = Arc::new(Mutex::new(new_rpc_stream_authority()));
        let stream_id = StreamRef::mint();
        resolve_mob_stream_open_authority(&authority, &stream_id)
            .await
            .expect("machine should accept mob stream open");
        let event = serde_json::json!({
            "event_id": "e1",
            "payload": {
                "type": "text_delta",
                "delta": "hello",
            }
        });

        let emit_status = sink.emit_mob_stream_event(&stream_id, 1, &event).await;
        assert_eq!(emit_status, StreamEmitStatus::ReceiverGone);
        let (observation, detail) = terminal_observation_from_emit_status(emit_status)
            .expect("closed receiver should lower into a terminal observation");
        assert_eq!(
            observation,
            meerkat_runtime::meerkat_machine::dsl::RpcEventStreamTerminalObservationKind::NotificationReceiverGone
        );
        let terminal = record_mob_stream_terminal_authority(
            &authority,
            &stream_id,
            observation,
            Some(detail.to_string()),
        )
        .await
        .expect("machine should derive receiver-gone terminal class");
        assert_eq!(
            terminal.reason,
            meerkat_runtime::meerkat_machine::dsl::RpcEventStreamTerminalReason::TerminalError
        );
        assert_eq!(
            terminal.error_code,
            Some(
                meerkat_runtime::meerkat_machine::dsl::RpcEventStreamTerminalErrorCode::StreamReceiverGone
            )
        );
    }

    async fn archived_session_is_not_found_and_mutations_reject_inner() {
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
        assert_eq!(
            error_code(&read_resp),
            error::SESSION_NOT_FOUND,
            "archive is an absorbing lifecycle terminal projected as NotFound"
        );

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
                    "content": { "type": "text", "text": "should be rejected" }
                }),
            ))
            .await
            .unwrap();
        assert!(
            inject_resp.error.is_some(),
            "archived session must reject System-message append"
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
        // The injected archive delay runs after the member turns retiring; it
        // keeps that window observable under load.
        let (router, _notif_rx) = test_router_with_mob_state(
            meerkat_mob_mcp::MobMcpState::new_in_memory_with_archive_delay(2_000),
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

        // The archive's read-only owner checks and roster scan precede the
        // retire transition, so wait for the published retiring state rather
        // than a fixed delay; the archive must still be in flight then.
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
        let members = loop {
            assert!(
                !archive.is_finished(),
                "archive completed before retirement was observed: {:?}",
                archive.await
            );
            let members_resp = router
                .dispatch(make_request(
                    "mob/members",
                    serde_json::json!({ "mob_id": &mob_id }),
                ))
                .await
                .unwrap();
            let members = result_value(&members_resp)["members"]
                .as_array()
                .expect("members array")
                .clone();
            if members
                .first()
                .is_some_and(|member| member["status"] == "retiring")
            {
                break members;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "retirement was not observed within 10 s: {members:?}"
            );
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        };
        assert!(
            !archive.is_finished(),
            "archive must still be in flight when retirement is observed"
        );
        assert_eq!(members.len(), 1, "retiring member should remain observable");
        assert_eq!(members[0]["agent_identity"], "lead-1");
        assert_eq!(members[0]["status"], "retiring");

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
        assert_eq!(notification_params!(notification)["stream_id"], stream_id);
        assert_eq!(notification_params!(notification)["ended"], true);
        assert_eq!(notification_params!(notification)["outcome"], "remote_end");
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn mob_stream_open_requires_subscribe_events_scope_before_registration() {
        let owner = meerkat_mob_mcp::MobMcpState::new_in_memory();
        let mob_id = meerkat_mob::MobId::from(format!(
            "scope-denied-mob-stream-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let mut profiles = std::collections::BTreeMap::new();
        profiles.insert(
            meerkat_mob::ProfileName::from("worker"),
            meerkat_mob::ProfileBinding::Inline(Box::new(meerkat_mob::Profile {
                model_fallback: None,
                model: "claude-sonnet-4-5".to_string(),
                provider: None,
                self_hosted_server_id: None,
                image_generation_provider: None,
                auto_compact_threshold: None,
                resume_overrides: Vec::new(),
                skills: Vec::new(),
                tools: meerkat_mob::ToolConfig::default(),
                peer_description: "worker".to_string(),
                external_addressable: false,
                backend: None,
                runtime_mode: meerkat_mob::MobRuntimeMode::TurnDriven,
                max_inline_peer_notifications: None,
                output_schema: None,
                provider_params: None,
                durable: false,
            })),
        );
        let mut definition = meerkat_mob::MobDefinition::explicit(mob_id.clone());
        definition.profiles = profiles;
        owner
            .mob_create_definition(definition)
            .await
            .expect("owner creates stream fixture mob");

        let viewer = meerkat_mob_mcp::MobMcpState::new_in_memory_as(
            meerkat_mob::MobControlPrincipal::External(
                meerkat_core::auth::PrincipalId::new("viewer").expect("principal id"),
            ),
        );
        let handle = owner.handle_for(&mob_id).await.expect("fixture mob handle");
        viewer.mob_insert_handle(mob_id.clone(), handle).await;
        let (router, _notif_rx) = test_router_with_mob_state(viewer).await;

        let response = router
            .dispatch(make_request(
                "mob/stream_open",
                serde_json::json!({ "mob_id": mob_id.as_str() }),
            ))
            .await
            .expect("request response");

        let error = response.error.expect("scope denial");
        assert_eq!(
            error.code,
            meerkat_contracts::ErrorCode::ScopeDenied.jsonrpc_code()
        );
        assert_eq!(
            error.data,
            Some(serde_json::json!({
                "required": "subscribe_events",
                "presented": []
            }))
        );
        assert!(
            router.active_mob_streams.lock().await.is_empty(),
            "a denied open must not register a forwarder"
        );
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
                        "id": format!("test_mob_{}", uuid::Uuid::new_v4().simple()),
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
        assert_eq!(notification_params!(notification)["stream_id"], stream_id);
        assert_eq!(
            notification_params!(notification)["outcome"],
            "explicit_close"
        );
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
                        "id": format!("test_mob_{}", uuid::Uuid::new_v4().simple()),
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
    async fn mob_stream_close_unknown_is_rejected_without_authority_witness() {
        let (router, _notif_rx) = test_router().await;

        let close_resp = router
            .dispatch(make_request(
                "mob/stream_close",
                serde_json::json!({ "stream_id": "00000000-0000-0000-0000-000000000000" }),
            ))
            .await
            .unwrap();
        assert_eq!(error_code(&close_resp), error::INVALID_PARAMS);
    }

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn mob_stream_close_trusts_machine_terminal_over_active_map() {
        for overflow in [false, true] {
            let (mut router, mut notif_rx) = test_router().await;
            router.notification_sink.stream_terminal_admission = RpcStreamTerminalAdmission::new(1);
            let stream_ref = StreamRef::mint();
            resolve_mob_stream_open_authority(&router.stream_authority, &stream_ref)
                .await
                .unwrap();
            router.active_mob_streams.lock().await.insert(
                stream_ref,
                reserved_test_stream_forwarder(&router.notification_sink, StreamTerminal::Mob),
            );
            let (recorded_tx, recorded_rx) = oneshot::channel();
            let (resume_tx, resume_rx) = oneshot::channel();
            let (delivered_tx, delivered_rx) = oneshot::channel();
            let task = tokio::spawn({
                let authority = router.stream_authority.clone();
                let streams = router.active_mob_streams.clone();
                let sink = router.notification_sink.clone();
                async move {
                    let (observation, detail) = if overflow {
                        (
                            meerkat_runtime::meerkat_machine::dsl::RpcEventStreamTerminalObservationKind::NotificationQueueOverflow,
                            Some("synthetic queue overflow".to_string()),
                        )
                    } else {
                        (
                            meerkat_runtime::meerkat_machine::dsl::RpcEventStreamTerminalObservationKind::TransportEnded,
                            None,
                        )
                    };
                    let terminal = record_mob_stream_terminal_authority(
                        &authority,
                        &stream_ref,
                        observation,
                        detail,
                    )
                    .await
                    .unwrap();
                    recorded_tx.send(()).unwrap();
                    resume_rx.await.unwrap();
                    let forwarder = streams
                        .lock()
                        .await
                        .remove(&stream_ref)
                        .expect("the terminal sender still owns its reservation");
                    sink.emit_mob_stream_end(&terminal, forwarder.terminal_reservation)
                        .await;
                    delivered_tx.send(()).unwrap();
                }
            });
            attach_stream_forwarder_task(&router.active_mob_streams, stream_ref, task).await;
            tokio::time::timeout(Duration::from_secs(1), recorded_rx)
                .await
                .unwrap()
                .unwrap();

            for _ in 0..2 {
                let close_resp = router
                    .dispatch(make_request(
                        "mob/stream_close",
                        serde_json::json!({ "stream_id": stream_ref.as_string() }),
                    ))
                    .await
                    .unwrap();
                let closed = result_value(&close_resp);
                assert_eq!(closed["closed"], true);
                assert_eq!(closed["already_closed"], true);
                assert!(
                    router
                        .active_mob_streams
                        .lock()
                        .await
                        .contains_key(&stream_ref),
                    "already-closed callers must not abort the authoritative terminal sender"
                );
                assert!(
                    router
                        .notification_sink
                        .stream_terminal_admission
                        .try_reserve()
                        .is_err()
                );
                assert!(
                    notif_rx.try_recv().is_err(),
                    "close must not fabricate a replacement terminal"
                );
            }
            resume_tx
                .send(())
                .expect("close must leave the terminal sender alive");
            tokio::time::timeout(Duration::from_secs(1), delivered_rx)
                .await
                .unwrap()
                .unwrap();
            let terminal = notif_rx.try_recv().expect("original terminal delivered");
            assert_eq!(terminal.method, "mob/stream_end");
            assert_eq!(
                notification_params!(terminal)["stream_id"],
                stream_ref.as_string()
            );
            assert_eq!(
                notification_params!(terminal)["outcome"],
                if overflow {
                    "terminal_error"
                } else {
                    "remote_end"
                }
            );
            if overflow {
                assert_eq!(
                    notification_params!(terminal)["error"]["code"],
                    "stream_queue_overflow"
                );
                assert_eq!(
                    notification_params!(terminal)["error"]["message"],
                    "synthetic queue overflow"
                );
            }
            assert!(router.active_mob_streams.lock().await.is_empty());
            assert!(
                notif_rx.try_recv().is_err(),
                "terminal must be delivered only once"
            );
            assert!(
                router
                    .notification_sink
                    .stream_terminal_admission
                    .try_reserve()
                    .is_err()
            );
            drop(terminal);
            assert!(
                router
                    .notification_sink
                    .stream_terminal_admission
                    .try_reserve()
                    .is_ok()
            );
        }
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
    async fn turn_start_accepts_turn_tool_overlay() {
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
                "turn_tool_overlay": {
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

    #[tokio::test]
    async fn turn_start_rejects_dispatch_context_in_turn_tool_overlay() {
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
                "turn_tool_overlay": {
                    "allowed_tools": [],
                    "dispatch_context": {
                        "workgraph.attention_projection": {
                            "binding_id": "attention-1"
                        }
                    }
                }
            }),
        );
        let turn_resp = router.dispatch(turn_req).await.unwrap();
        assert_eq!(error_code(&turn_resp), error::INVALID_PARAMS);
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

    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn mob_wire_members_batch_routes_to_mob_handle_report() {
        let (router, _notif_rx) = test_router().await;
        let create = router
            .dispatch(make_request(
                "mob/create",
                serde_json::json!({
                    "definition": {
                        "id": "mob-batch-wire",
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
            .expect("create response");
        let mob_id = result_value(&create)["mob_id"]
            .as_str()
            .expect("mob id")
            .to_string();

        for identity in ["worker-a", "worker-b"] {
            let response = router
                .dispatch(make_request(
                    "mob/spawn",
                    serde_json::json!({
                        "mob_id": mob_id,
                        "profile": "worker",
                        "agent_identity": identity,
                        "runtime_mode": "turn_driven"
                    }),
                ))
                .await
                .expect("spawn response");
            assert!(
                response.error.is_none(),
                "spawn {identity} failed: {:?}",
                response.error
            );
        }

        let first = router
            .dispatch(make_request(
                "mob/wire_members_batch",
                serde_json::json!({
                    "mob_id": mob_id,
                    "edges": [{ "a": "worker-b", "b": "worker-a" }]
                }),
            ))
            .await
            .expect("batch wire response");
        let first = result_value(&first);
        assert_eq!(first["requested"], 1);
        assert_eq!(first["wired"][0]["a"], "worker-a");
        assert_eq!(first["wired"][0]["b"], "worker-b");
        assert!(first["already_wired"].as_array().is_some_and(Vec::is_empty));

        let second = router
            .dispatch(make_request(
                "mob/wire_members_batch",
                serde_json::json!({
                    "mob_id": mob_id,
                    "edges": [{ "a": "worker-a", "b": "worker-b" }]
                }),
            ))
            .await
            .expect("batch wire response");
        let second = result_value(&second);
        assert_eq!(second["requested"], 1);
        assert!(second["wired"].as_array().is_some_and(Vec::is_empty));
        assert_eq!(second["already_wired"][0]["a"], "worker-a");
        assert_eq!(second["already_wired"][0]["b"], "worker-b");
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
            .await
            .expect("target session should unregister cleanly");
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

    /// 5b. `session/inject_context` appends an ordinary durable System message.
    #[tokio::test]
    async fn session_inject_context_returns_applied_status() {
        let (router, _notif_rx) = test_router().await;

        let create_req = make_request("session/create", serde_json::json!({"prompt": "Hello"}));
        let create_resp = router.dispatch(create_req).await.unwrap();
        let created = result_value(&create_resp);
        let session_id = created["session_id"].as_str().unwrap().to_string();

        let inject_req = make_request(
            "session/inject_context",
            serde_json::json!({
                "session_id": session_id,
                "content": { "type": "text", "text": "Coordinate with the orchestrator." },
                "source": "mob",
                "idempotency_key": "ctx-router-test"
            }),
        );
        let inject_resp = router.dispatch(inject_req).await.unwrap();
        let injected = result_value(&inject_resp);
        assert_eq!(injected["status"], "applied");
    }

    #[tokio::test]
    async fn session_inject_context_is_durable_across_later_turns() {
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
                    "content": { "type": "text", "text": injected_text },
                    "source": "mob",
                    "idempotency_key": "ctx-next-turn-once"
                }),
            ))
            .await
            .unwrap();
        assert_eq!(result_value(&inject_resp)["status"], "applied");

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
        assert_eq!(
            exact_system_message_count(&first_request, injected_text),
            1,
            "next eligible turn must include the durable ordered System message: {first_request:?}"
        );
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
        assert_eq!(
            exact_system_message_count(&second_request, injected_text),
            1,
            "durable System messages must remain in transcript order on later turns: {second_request:?}"
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

        let injected_text = "Late durable System message";
        let inject_resp = router
            .dispatch(make_request(
                "session/inject_context",
                serde_json::json!({
                    "session_id": &session_id,
                    "content": { "type": "text", "text": injected_text },
                    "source": "mob",
                    "idempotency_key": "ctx-rpc-during-active-turn"
                }),
            ))
            .await
            .unwrap();
        assert_eq!(result_value(&inject_resp)["status"], "applied");

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
        assert_eq!(
            exact_system_message_count(&first_request, injected_text),
            0,
            "context appended during an active RPC turn must not affect the in-flight request: {first_request:?}"
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
        assert_eq!(
            exact_system_message_count(&second_request, injected_text),
            1,
            "context appended during an active RPC turn must apply on the next eligible turn: {second_request:?}"
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
                    "content": { "type": "text", "text": injected_text },
                    "source": "mob",
                    "idempotency_key": "ctx-dedup"
                }),
            ))
            .await
            .unwrap();
        assert_eq!(result_value(&first_inject)["status"], "applied");

        let second_inject = router
            .dispatch(make_request(
                "session/inject_context",
                serde_json::json!({
                    "session_id": &session_id,
                    "content": { "type": "text", "text": injected_text },
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
        assert_eq!(
            exact_system_message_count(&request, injected_text),
            1,
            "duplicate idempotency keys must not enqueue multiple ordered System rows: {request:?}"
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
                .contains("forced mob event store member retired append failure"),
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
            matches!(complete_retry_resp.error.as_ref(), Some(err) if err.code == error::SESSION_NOT_FOUND),
            "cleanup-only retry must not become archive success: {complete_retry_resp:?}"
        );
        assert!(
            mob_state.handle_for(&child_mob_id).await.is_err(),
            "cleanup retry must remove the child mob retry anchor"
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
    async fn instruction_activation_is_typed_durable_and_idempotent() {
        let (router, _notif_rx) = test_router().await;
        let create_resp = router
            .dispatch(make_request(
                "session/create",
                serde_json::json!({
                    "prompt": "Hello",
                    "model": "claude-opus-5"
                }),
            ))
            .await
            .unwrap();
        let session_id = result_value(&create_resp)["session_id"]
            .as_str()
            .unwrap()
            .to_string();
        let body = "Always report uncertainty explicitly.";
        let activation = meerkat_core::InstructionActivationRequest {
            revision: meerkat_core::InstructionRevisionRef {
                namespace: meerkat_core::InstructionNamespace::new("app.example").unwrap(),
                key: meerkat_core::InstructionKey::new("primary").unwrap(),
                revision_id: meerkat_core::InstructionRevisionId::new("revision-1").unwrap(),
                content_sha256: meerkat_core::InstructionContentDigest::for_body(body),
            },
            activation_id: meerkat_core::InstructionActivationId::new("activation-1").unwrap(),
            expectation: meerkat_core::InstructionActivationExpectation::Absent,
            supersedes: None,
            body: body.to_string(),
        };

        let apply = router
            .dispatch(make_request(
                "session/activate_instruction",
                serde_json::json!({
                    "session_id": &session_id,
                    "activation": activation.clone(),
                }),
            ))
            .await
            .unwrap();
        assert!(apply.error.is_none(), "activation failed: {apply:?}");
        assert_eq!(result_value(&apply)["disposition"], "applied");

        let retry = router
            .dispatch(make_request(
                "session/activate_instruction",
                serde_json::json!({
                    "session_id": &session_id,
                    "activation": activation,
                }),
            ))
            .await
            .unwrap();
        assert!(retry.error.is_none(), "exact retry failed: {retry:?}");
        assert_eq!(result_value(&retry)["disposition"], "duplicate");

        let records = router
            .dispatch(make_request(
                "session/instruction_activations",
                serde_json::json!({
                    "session_id": &session_id,
                    "namespace": "app.example",
                    "key": "primary"
                }),
            ))
            .await
            .unwrap();
        let records = result_value(&records);
        assert_eq!(records["records"].as_array().unwrap().len(), 1);
        assert_eq!(
            records["records"][0]["identity"]["activation_id"],
            "activation-1"
        );

        let history = router
            .dispatch(make_request(
                "session/history",
                serde_json::json!({ "session_id": &session_id }),
            ))
            .await
            .unwrap();
        let history_result = result_value(&history);
        let activation_rows = history_result["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|message| message.get("instruction_activation").is_some())
            .collect::<Vec<_>>();
        assert_eq!(activation_rows.len(), 1);
        assert_eq!(
            activation_rows[0]["instruction_activation"]["activation_id"],
            "activation-1"
        );
        assert!(
            activation_rows[0]["content"]
                .as_str()
                .unwrap()
                .starts_with("[meerkat-instruction-activation-v1]")
        );
    }

    /// A `session/create` refusal keeps its typed error data: a deferred
    /// first prompt carrying inline video for a model without inline-video
    /// support is refused with `INVALID_PARAMS` and the unsupported-capability
    /// evidence in `error.data`, as `turn/start` reports the same refusal.
    #[tokio::test]
    async fn session_create_refusal_keeps_its_typed_error_data() {
        let (router, _notif_rx) = test_router().await;
        let prompt = serde_json::to_value(vec![meerkat_core::ContentBlock::Video {
            media_type: "video/mp4".to_string(),
            duration_ms: 1000,
            data: meerkat_core::VideoData::Inline {
                data: "AAAA".to_string(),
            },
        }])
        .expect("serialize the video prompt");
        let response = router
            .dispatch(make_request(
                "session/create",
                serde_json::json!({
                    "prompt": prompt,
                    "model": "claude-sonnet-4-6",
                    "initial_turn": "deferred"
                }),
            ))
            .await
            .unwrap();
        let error = response
            .error
            .expect("inline video for a model without it is refused");
        assert_eq!(error.code, crate::error::INVALID_PARAMS, "{error:?}");
        let data = error
            .data
            .expect("the refusal keeps its typed unsupported-capability data");
        assert_eq!(
            data["unsupported_capability"]["capability"],
            serde_json::json!("inline_video"),
            "{data}"
        );
    }

    #[tokio::test]
    async fn instruction_activation_rejects_unsupported_current_lowering() {
        let (router, _notif_rx) = test_router().await;
        let create_resp = router
            .dispatch(make_request(
                "session/create",
                serde_json::json!({
                    "prompt": "Hello",
                    "model": "claude-sonnet-4-6"
                }),
            ))
            .await
            .unwrap();
        let session_id = result_value(&create_resp)["session_id"]
            .as_str()
            .unwrap()
            .to_string();
        let body = "Policy";
        let response = router
            .dispatch(make_request(
                "session/activate_instruction",
                serde_json::json!({
                    "session_id": &session_id,
                    "activation": {
                        "revision": {
                            "namespace": "app.example",
                            "key": "primary",
                            "revision_id": "revision-1",
                            "content_sha256": meerkat_core::InstructionContentDigest::for_body(body)
                        },
                        "activation_id": "activation-1",
                        "expectation": { "kind": "absent" },
                        "body": body
                    }
                }),
            ))
            .await
            .unwrap();
        let error = response.error.expect("unsupported lowering must fail");
        assert_eq!(
            error.data.unwrap()["instruction_activation_code"],
            "unsupported_current_lowering"
        );
    }

    #[tokio::test]
    async fn instruction_activation_waits_for_the_turn_finalization_boundary() {
        let (router, _notif_rx) = test_router().await;
        let create_resp = router
            .dispatch(make_request(
                "session/create",
                serde_json::json!({
                    "prompt": "Hello",
                    "model": "claude-opus-5"
                }),
            ))
            .await
            .unwrap();
        let session_id =
            SessionId::parse(result_value(&create_resp)["session_id"].as_str().unwrap()).unwrap();
        let boundary = router
            .runtime
            .acquire_instruction_activation_test_boundary(&session_id)
            .await;
        let body = "Policy";
        let activation = meerkat_core::InstructionActivationRequest {
            revision: meerkat_core::InstructionRevisionRef {
                namespace: meerkat_core::InstructionNamespace::new("app.example").unwrap(),
                key: meerkat_core::InstructionKey::new("primary").unwrap(),
                revision_id: meerkat_core::InstructionRevisionId::new("revision-1").unwrap(),
                content_sha256: meerkat_core::InstructionContentDigest::for_body(body),
            },
            activation_id: meerkat_core::InstructionActivationId::new("activation-1").unwrap(),
            expectation: meerkat_core::InstructionActivationExpectation::Absent,
            supersedes: None,
            body: body.to_string(),
        };
        let runtime = Arc::clone(&router.runtime);
        let activation_session_id = session_id.clone();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let _ = started_tx.send(());
            runtime
                .activate_instruction(&activation_session_id, activation)
                .await
        });

        started_rx.await.unwrap();
        tokio::task::yield_now().await;
        assert!(
            !task.is_finished(),
            "activation must not mutate while the turn-finalization boundary is held"
        );
        drop(boundary);
        let receipt = task.await.unwrap().unwrap();
        assert_eq!(
            receipt.disposition,
            meerkat_core::InstructionActivationDisposition::Applied
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn instruction_activation_commit_survives_caller_cancellation() {
        let (router, _notif_rx) = test_router().await;
        let create_resp = router
            .dispatch(make_request(
                "session/create",
                serde_json::json!({
                    "prompt": "Hello",
                    "model": "claude-opus-5"
                }),
            ))
            .await
            .unwrap();
        let session_id =
            SessionId::parse(result_value(&create_resp)["session_id"].as_str().unwrap()).unwrap();
        let body = "Policy";
        let activation = meerkat_core::InstructionActivationRequest {
            revision: meerkat_core::InstructionRevisionRef {
                namespace: meerkat_core::InstructionNamespace::new("app.example").unwrap(),
                key: meerkat_core::InstructionKey::new("primary").unwrap(),
                revision_id: meerkat_core::InstructionRevisionId::new("revision-1").unwrap(),
                content_sha256: meerkat_core::InstructionContentDigest::for_body(body),
            },
            activation_id: meerkat_core::InstructionActivationId::new("activation-1").unwrap(),
            expectation: meerkat_core::InstructionActivationExpectation::Absent,
            supersedes: None,
            body: body.to_string(),
        };
        let started = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new((StdMutex::new(false), Condvar::new()));
        let fence = Arc::new(BlockingInstructionActivationFence {
            started: Arc::clone(&started),
            release: Arc::clone(&release),
        });
        let runtime = Arc::clone(router.runtime.inner());
        let activation_session_id = session_id.clone();
        let caller = tokio::spawn(async move {
            runtime
                .activate_instruction_with_write_fence(&activation_session_id, activation, fence)
                .await
        });

        started.notified().await;
        caller.abort();
        release_instruction_activation_fence(&release);

        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let page = router
                    .runtime
                    .read_instruction_activation_records(
                        &session_id,
                        meerkat_core::InstructionActivationReadQuery::default(),
                    )
                    .await
                    .unwrap();
                if page.records.len() == 1 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("detached actor-owned activation must commit after caller cancellation");
    }

    #[tokio::test]
    async fn refused_instruction_activation_fence_never_becomes_durable_effect() {
        for decision in [
            RefusingInstructionActivationFence::Conflict,
            RefusingInstructionActivationFence::Backoff,
        ] {
            let (router, _notif_rx) = test_router().await;
            let create_resp = router
                .dispatch(make_request(
                    "session/create",
                    serde_json::json!({
                        "prompt": "Hello",
                        "model": "claude-opus-5"
                    }),
                ))
                .await
                .unwrap();
            let session_id =
                SessionId::parse(result_value(&create_resp)["session_id"].as_str().unwrap())
                    .unwrap();
            let body = "Policy";
            let activation = meerkat_core::InstructionActivationRequest {
                revision: meerkat_core::InstructionRevisionRef {
                    namespace: meerkat_core::InstructionNamespace::new("app.example").unwrap(),
                    key: meerkat_core::InstructionKey::new("primary").unwrap(),
                    revision_id: meerkat_core::InstructionRevisionId::new("revision-1").unwrap(),
                    content_sha256: meerkat_core::InstructionContentDigest::for_body(body),
                },
                activation_id: meerkat_core::InstructionActivationId::new("activation-1").unwrap(),
                expectation: meerkat_core::InstructionActivationExpectation::Absent,
                supersedes: None,
                body: body.to_string(),
            };

            let error = router
                .runtime
                .inner()
                .activate_instruction_with_write_fence(&session_id, activation, Arc::new(decision))
                .await
                .expect_err("refused fence must reject activation");
            assert_eq!(
                error.admission_code(),
                Some(match decision {
                    RefusingInstructionActivationFence::Conflict => {
                        meerkat_core::InstructionActivationAdmissionErrorCode::ExternalWriteFenceConflict
                    }
                    RefusingInstructionActivationFence::Backoff => {
                        meerkat_core::InstructionActivationAdmissionErrorCode::ExternalWriteFenceBackoff
                    }
                })
            );
            let page = router
                .runtime
                .read_instruction_activation_records(
                    &session_id,
                    meerkat_core::InstructionActivationReadQuery::default(),
                )
                .await
                .unwrap();
            assert!(
                page.records.is_empty(),
                "a refused external fence must not publish an activation record"
            );
        }
    }

    #[tokio::test]
    async fn refused_fence_on_exact_duplicate_never_returns_success_receipt() {
        for decision in [
            RefusingInstructionActivationFence::Conflict,
            RefusingInstructionActivationFence::Backoff,
        ] {
            let (router, _notif_rx) = test_router().await;
            let create_resp = router
                .dispatch(make_request(
                    "session/create",
                    serde_json::json!({
                        "prompt": "Hello",
                        "model": "claude-opus-5"
                    }),
                ))
                .await
                .unwrap();
            let session_id =
                SessionId::parse(result_value(&create_resp)["session_id"].as_str().unwrap())
                    .unwrap();
            let body = "Policy";
            let activation = meerkat_core::InstructionActivationRequest {
                revision: meerkat_core::InstructionRevisionRef {
                    namespace: meerkat_core::InstructionNamespace::new("app.example").unwrap(),
                    key: meerkat_core::InstructionKey::new("primary").unwrap(),
                    revision_id: meerkat_core::InstructionRevisionId::new("revision-1").unwrap(),
                    content_sha256: meerkat_core::InstructionContentDigest::for_body(body),
                },
                activation_id: meerkat_core::InstructionActivationId::new("activation-1").unwrap(),
                expectation: meerkat_core::InstructionActivationExpectation::Absent,
                supersedes: None,
                body: body.to_string(),
            };
            let applied = router
                .runtime
                .inner()
                .activate_instruction(&session_id, activation.clone())
                .await
                .unwrap();
            assert_eq!(
                applied.disposition,
                meerkat_core::InstructionActivationDisposition::Applied
            );

            let error = router
                .runtime
                .inner()
                .activate_instruction_with_write_fence(&session_id, activation, Arc::new(decision))
                .await
                .expect_err("duplicate must still revalidate external write authority");
            assert_eq!(
                error.admission_code(),
                Some(match decision {
                    RefusingInstructionActivationFence::Conflict => {
                        meerkat_core::InstructionActivationAdmissionErrorCode::ExternalWriteFenceConflict
                    }
                    RefusingInstructionActivationFence::Backoff => {
                        meerkat_core::InstructionActivationAdmissionErrorCode::ExternalWriteFenceBackoff
                    }
                })
            );
            let page = router
                .runtime
                .read_instruction_activation_records(
                    &session_id,
                    meerkat_core::InstructionActivationReadQuery::default(),
                )
                .await
                .unwrap();
            assert_eq!(page.records.len(), 1, "duplicate must append no second row");
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn instruction_activation_and_live_open_share_the_machine_lifecycle_lease() {
        let (router, _notif_rx) = test_router().await;
        let create_resp = router
            .dispatch(make_request(
                "session/create",
                serde_json::json!({
                    "prompt": "Hello",
                    "model": "claude-opus-5"
                }),
            ))
            .await
            .unwrap();
        let session_id =
            SessionId::parse(result_value(&create_resp)["session_id"].as_str().unwrap()).unwrap();
        let machine = router.runtime.runtime_adapter();
        let open_lease = machine
            .acquire_live_open_lifecycle_lease(&session_id)
            .await
            .expect("manual live/open lease");
        let body = "Policy";
        let activation = meerkat_core::InstructionActivationRequest {
            revision: meerkat_core::InstructionRevisionRef {
                namespace: meerkat_core::InstructionNamespace::new("app.example").unwrap(),
                key: meerkat_core::InstructionKey::new("primary").unwrap(),
                revision_id: meerkat_core::InstructionRevisionId::new("revision-1").unwrap(),
                content_sha256: meerkat_core::InstructionContentDigest::for_body(body),
            },
            activation_id: meerkat_core::InstructionActivationId::new("activation-1").unwrap(),
            expectation: meerkat_core::InstructionActivationExpectation::Absent,
            supersedes: None,
            body: body.to_string(),
        };
        let started = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new((StdMutex::new(false), Condvar::new()));
        let fence = Arc::new(BlockingInstructionActivationFence {
            started: Arc::clone(&started),
            release: Arc::clone(&release),
        });
        let runtime = Arc::clone(router.runtime.inner());
        let activation_session_id = session_id.clone();
        let activation_task = tokio::spawn(async move {
            runtime
                .activate_instruction_with_write_fence(&activation_session_id, activation, fence)
                .await
        });
        let mut fence_started = Box::pin(started.notified());
        assert!(
            tokio::time::timeout(Duration::from_millis(25), &mut fence_started)
                .await
                .is_err(),
            "activation must wait behind an in-progress live/open lease"
        );
        drop(open_lease);
        tokio::time::timeout(Duration::from_secs(5), &mut fence_started)
            .await
            .expect("activation reaches external fence after live/open lease release");

        let waiting_machine = Arc::clone(&machine);
        let waiting_session = session_id.clone();
        let mut waiting_open = tokio::spawn(async move {
            waiting_machine
                .acquire_live_open_lifecycle_lease(&waiting_session)
                .await
        });
        assert!(
            tokio::time::timeout(Duration::from_millis(25), &mut waiting_open)
                .await
                .is_err(),
            "live/open must wait while activation retains the shared lease through commit"
        );
        release_instruction_activation_fence(&release);
        activation_task
            .await
            .expect("activation task joins")
            .expect("activation commits");
        let waiting_lease = waiting_open
            .await
            .expect("live/open waiter joins")
            .expect("live/open acquires after activation commit");
        drop(waiting_lease);
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

    /// A `session/fork_at` child runs its first turn: the fork commits the
    /// child with a build state derived from its source, which the child's
    /// cold resume requires.
    #[tokio::test]
    async fn session_fork_at_child_runs_its_first_turn() {
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
        assert!(fork_resp.error.is_none(), "the fork commits: {fork_resp:?}");
        let forked_id = result_value(&fork_resp)["session_id"]
            .as_str()
            .unwrap()
            .to_string();

        let turn_resp = router
            .dispatch(make_request(
                "turn/start",
                serde_json::json!({
                    "session_id": &forked_id,
                    "prompt": "Follow up on the branch",
                }),
            ))
            .await
            .unwrap();
        assert!(turn_resp.error.is_none(), "the child runs: {turn_resp:?}");

        let history_resp = router
            .dispatch(make_request(
                "session/history",
                serde_json::json!({ "session_id": &forked_id }),
            ))
            .await
            .unwrap();
        let history = result_value(&history_resp);
        let messages = history["messages"].as_array().unwrap();
        assert!(
            messages.len() > 2,
            "the child's turn extends the forked prefix: {history}"
        );
        assert!(
            messages.iter().any(|message| message["role"] == "user"
                && message["content"] == "Follow up on the branch"),
            "the child's transcript holds its own turn: {history}"
        );
    }

    /// A `session/fork_replace` child runs its first turn, as a `fork_at`
    /// child does.
    #[tokio::test]
    async fn session_fork_replace_child_runs_its_first_turn() {
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
        let history_resp = router
            .dispatch(make_request(
                "session/history",
                serde_json::json!({ "session_id": &session_id }),
            ))
            .await
            .unwrap();
        let message_index = result_value(&history_resp)["messages"]
            .as_array()
            .unwrap()
            .iter()
            .position(|message| message["role"] == "user" && message["content"] == "Hello")
            .expect("the source transcript holds the first user message");

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
                            "content": "Edited hello"
                        }
                    },
                    "running_behavior": "reject"
                }),
            ))
            .await
            .unwrap();
        assert!(fork_resp.error.is_none(), "the fork commits: {fork_resp:?}");
        let forked_id = result_value(&fork_resp)["session_id"]
            .as_str()
            .unwrap()
            .to_string();

        let turn_resp = router
            .dispatch(make_request(
                "turn/start",
                serde_json::json!({
                    "session_id": &forked_id,
                    "prompt": "Follow up on the edited branch",
                }),
            ))
            .await
            .unwrap();
        assert!(turn_resp.error.is_none(), "the child runs: {turn_resp:?}");
    }

    /// The durable stores of one RPC server, which a later server opens again
    /// as a restarted process would.
    struct ReopenableServerStores {
        sessions: Arc<dyn meerkat::SessionStore>,
        runtime: Arc<dyn meerkat_runtime::RuntimeStore>,
        blobs: Arc<dyn meerkat_core::BlobStore>,
    }

    impl ReopenableServerStores {
        fn new() -> Self {
            Self {
                sessions: Arc::new(meerkat::MemoryStore::new()),
                runtime: Arc::new(meerkat_runtime::InMemoryRuntimeStore::new()),
                blobs: memory_blob_store(),
            }
        }

        /// A new server over these stores: its runtime, and a router over it.
        fn open_server(
            &self,
            root: &std::path::Path,
        ) -> (
            MethodRouter,
            Arc<SessionRuntime>,
            mpsc::Receiver<RpcNotification>,
        ) {
            let config = anthropic_test_config();
            let config_store: Arc<dyn ConfigStore> = Arc::new(MemoryConfigStore::new(
                config.clone(),
                meerkat_models::canonical(),
            ));
            let persistence = meerkat::PersistenceBundle::new(
                Arc::clone(&self.sessions),
                Arc::clone(&self.runtime),
                Arc::clone(&self.blobs),
            )
            .expect("construct runtime authority");
            let runtime = SessionRuntime::new(
                AgentFactory::new(root.join("sessions")),
                config,
                10,
                persistence,
                NotificationSink::noop(),
            )
            .expect("compose the RPC session runtime");
            runtime.set_default_llm_client(Some(Arc::new(MockLlmClient::anthropic())));
            runtime.set_config_runtime(Arc::new(ConfigRuntime::new(
                Arc::clone(&config_store),
                root.join("config_state.json"),
            )));
            let runtime = Arc::new(runtime);
            let (notif_tx, notif_rx) = mpsc::channel(100);
            let router = MethodRouter::new(
                Arc::clone(&runtime),
                config_store,
                NotificationSink::new(notif_tx),
            )
            .expect("construct runtime authority");
            (router, runtime, notif_rx)
        }
    }

    /// After the RPC server restarts, an idle persisted session has no
    /// runtime registration in the new server: sessions are registered when
    /// they are created, run a turn or open a live channel, and persisted
    /// ones recover lazily. Every transcript edit on that idle session is
    /// still admitted instead of answering SESSION_BUSY as if the session
    /// were active: two concurrent `session/fork_at` calls, which both find
    /// the session unregistered, each commit a child, and
    /// `session/fork_replace`, `session/rewrite_transcript`,
    /// `session/restore_transcript_revision` and
    /// `session/update_system_prompt` apply. The session then runs its next
    /// turn. A session archived before the restart is still not found.
    ///
    /// Not pinned here: an edit refused as busy over a durable input still
    /// pending after the restart. A clean shutdown abandons a queued input
    /// (`Abandoned { reason: Stopped }`), so only a crash leaves one pending,
    /// and a test cannot restore the row because its persistence record is
    /// crate-private to the runtime. That needs a child-process harness like
    /// `tests/lazy_resume_uncommitted_terminal.rs`. The edit's registration
    /// recovers the input ledger before the busy verdict reads it.
    #[tokio::test]
    async fn transcript_edits_on_an_idle_session_are_admitted_after_a_server_restart() {
        let temp = tempfile::tempdir().unwrap();
        let stores = ReopenableServerStores::new();
        let (router, runtime, _notif_rx) = stores.open_server(temp.path());
        let create_resp = router
            .dispatch(make_request(
                "session/create",
                serde_json::json!({
                    "prompt": "Hello",
                    "system_prompt": "You are a test assistant.",
                }),
            ))
            .await
            .unwrap();
        let session_id = result_value(&create_resp)["session_id"]
            .as_str()
            .unwrap()
            .to_string();
        let archived_create_resp = router
            .dispatch(make_request(
                "session/create",
                serde_json::json!({ "prompt": "Archive me" }),
            ))
            .await
            .unwrap();
        let archived_id = result_value(&archived_create_resp)["session_id"]
            .as_str()
            .unwrap()
            .to_string();
        let archive_resp = router
            .dispatch(make_request(
                "session/archive",
                serde_json::json!({ "session_id": &archived_id }),
            ))
            .await
            .unwrap();
        assert!(
            archive_resp.error.is_none(),
            "the second session is archived: {archive_resp:?}"
        );

        // The first server stops; a new one opens the same stores.
        tokio::time::timeout(Duration::from_secs(60), runtime.try_shutdown())
            .await
            .expect("the first server shuts down in bounded time")
            .expect("the first server shuts down");
        drop(router);
        drop(runtime);
        let (router, runtime, _notif_rx) = stores.open_server(temp.path());
        let parsed_session_id = SessionId::parse(&session_id).expect("session id");
        assert!(
            !runtime
                .runtime_adapter()
                .contains_session(&parsed_session_id)
                .await,
            "the restarted server has not registered the idle session"
        );

        let history_resp = router
            .dispatch(make_request(
                "session/history",
                serde_json::json!({ "session_id": &session_id }),
            ))
            .await
            .unwrap();
        let history = result_value(&history_resp);
        let messages = history["messages"].as_array().unwrap();
        let system_index = messages
            .iter()
            .position(|message| message["role"] == "system")
            .unwrap_or_else(|| panic!("the session holds its System message: {history}"));
        let hello_index = messages
            .iter()
            .position(|message| message["role"] == "user" && message["content"] == "Hello")
            .unwrap_or_else(|| panic!("the session holds its first user message: {history}"));
        let revision_resp = router
            .dispatch(make_request(
                "session/transcript_revision",
                serde_json::json!({ "session_id": &session_id, "revision": "current" }),
            ))
            .await
            .unwrap();
        let original_revision = result_value(&revision_resp)["revision"]
            .as_str()
            .unwrap()
            .to_string();

        let fork_at_request = || {
            make_request(
                "session/fork_at",
                serde_json::json!({
                    "session_id": &session_id,
                    "message_index": hello_index + 1,
                    "running_behavior": "reject"
                }),
            )
        };
        let (first_fork, second_fork) = tokio::join!(
            router.dispatch(fork_at_request()),
            router.dispatch(fork_at_request()),
        );
        let first_fork = first_fork.unwrap();
        let second_fork = second_fork.unwrap();
        let fork_replace = router
            .dispatch(make_request(
                "session/fork_replace",
                serde_json::json!({
                    "session_id": &session_id,
                    "message_index": hello_index,
                    "replacement": {
                        "type": "message",
                        "message": {
                            "role": "user",
                            "content": "Edited hello"
                        }
                    },
                    "running_behavior": "reject"
                }),
            ))
            .await
            .unwrap();
        let rewrite = router
            .dispatch(make_request(
                "session/rewrite_transcript",
                serde_json::json!({
                    "session_id": &session_id,
                    "selection": {
                        "type": "message_range",
                        "start": hello_index,
                        "end": hello_index + 1
                    },
                    "replacement": [{ "role": "user", "content": "Hello again" }],
                    "reason": { "kind": "test", "note": "rewrite after a server restart" },
                    "expected_parent_revision": &original_revision
                }),
            ))
            .await
            .unwrap();
        let restore = router
            .dispatch(make_request(
                "session/restore_transcript_revision",
                serde_json::json!({
                    "session_id": &session_id,
                    "revision": &original_revision,
                    "reason": { "kind": "test", "note": "restore after a server restart" }
                }),
            ))
            .await
            .unwrap();
        let update = router
            .dispatch(make_request(
                "session/update_system_prompt",
                serde_json::json!({
                    "session_id": &session_id,
                    "key": "persona",
                    "target_message_index": system_index,
                    "content": "You are an edited test assistant."
                }),
            ))
            .await
            .unwrap();
        let refused: Vec<String> = [
            ("session/fork_at (first)", &first_fork),
            ("session/fork_at (second)", &second_fork),
            ("session/fork_replace", &fork_replace),
            ("session/rewrite_transcript", &rewrite),
            ("session/restore_transcript_revision", &restore),
            ("session/update_system_prompt", &update),
        ]
        .into_iter()
        .filter_map(|(method, response)| {
            response
                .error
                .as_ref()
                .map(|error| format!("{method}: {} {}", error.code, error.message))
        })
        .collect();
        assert!(
            refused.is_empty(),
            "every transcript edit on the idle session is admitted after the restart: {refused:#?}"
        );
        let first_child = result_value(&first_fork)["session_id"]
            .as_str()
            .unwrap()
            .to_string();
        let second_child = result_value(&second_fork)["session_id"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(
            first_child != session_id && second_child != session_id && first_child != second_child,
            "each concurrent fork commits its own child: {first_child} {second_child}"
        );
        assert_eq!(result_value(&update)["status"], "applied", "{update:?}");

        // The registration the edits left serves the session's next turn.
        let turn_resp = router
            .dispatch(make_request(
                "turn/start",
                serde_json::json!({
                    "session_id": &session_id,
                    "prompt": "Follow up after the restart",
                }),
            ))
            .await
            .unwrap();
        assert!(
            turn_resp.error.is_none(),
            "the edited session runs its next turn: {turn_resp:?}"
        );

        let archived_fork = router
            .dispatch(make_request(
                "session/fork_at",
                serde_json::json!({
                    "session_id": &archived_id,
                    "message_index": 1,
                    "running_behavior": "reject"
                }),
            ))
            .await
            .unwrap();
        let archived_error = archived_fork
            .error
            .as_ref()
            .unwrap_or_else(|| panic!("an archived session is not forked: {archived_fork:?}"));
        assert_eq!(
            archived_error.code,
            error::SESSION_NOT_FOUND,
            "an archived session is not found after the restart: {archived_error:?}"
        );
    }

    /// The control for the restart case above: a transcript edit on a
    /// session whose turn is in flight is still refused with SESSION_BUSY,
    /// and the session's turn finishes unaffected.
    #[tokio::test]
    async fn transcript_edits_on_a_session_with_a_turn_in_flight_answer_session_busy() {
        // Bounds a failure only; the passing path never waits for it.
        const FAILURE_BOUND: Duration = Duration::from_secs(30);
        let calls = Arc::new(tokio::sync::watch::channel(0usize).0);
        let release = Arc::new(tokio::sync::Semaphore::new(0));
        let (router, _notif_rx) = test_router_with_llm(Arc::new(GatedLlmClient {
            calls: Arc::clone(&calls),
            release: Arc::clone(&release),
            requests: Arc::new(StdMutex::new(Vec::new())),
        }))
        .await;
        // The created session's first turn runs to completion.
        release.add_permits(1);
        let create_resp = router
            .dispatch(make_request(
                "session/create",
                serde_json::json!({
                    "prompt": "Hello",
                    "system_prompt": "You are a test assistant.",
                }),
            ))
            .await
            .unwrap();
        let session_id = result_value(&create_resp)["session_id"]
            .as_str()
            .unwrap()
            .to_string();

        let mut seen = calls.subscribe();
        let turn = {
            let router = router.clone();
            let session_id = session_id.clone();
            tokio::spawn(async move {
                router
                    .dispatch(make_request(
                        "turn/start",
                        serde_json::json!({
                            "session_id": session_id,
                            "prompt": "a turn in flight",
                        }),
                    ))
                    .await
                    .expect("turn/start response")
            })
        };
        tokio::time::timeout(FAILURE_BOUND, seen.wait_for(|calls| *calls >= 2))
            .await
            .expect("the second turn reaches the provider")
            .expect("call counter stays open");

        let fork_resp = router
            .dispatch(make_request(
                "session/fork_at",
                serde_json::json!({
                    "session_id": &session_id,
                    "message_index": 1,
                    "running_behavior": "reject"
                }),
            ))
            .await
            .unwrap();
        let update_resp = router
            .dispatch(make_request(
                "session/update_system_prompt",
                serde_json::json!({
                    "session_id": &session_id,
                    "key": "persona",
                    "target_message_index": 0,
                    "content": "You are an edited test assistant."
                }),
            ))
            .await
            .unwrap();
        for (method, response) in [
            ("session/fork_at", &fork_resp),
            ("session/update_system_prompt", &update_resp),
        ] {
            let error = response
                .error
                .as_ref()
                .unwrap_or_else(|| panic!("{method} is refused while a turn runs: {response:?}"));
            assert_eq!(error.code, error::SESSION_BUSY, "{method}: {error:?}");
            assert!(
                error.message.contains("is active"),
                "{method} is refused as an active session: {error:?}"
            );
        }

        release.add_permits(1);
        let turn = tokio::time::timeout(FAILURE_BOUND, turn)
            .await
            .expect("the turn finishes once released")
            .expect("turn task");
        assert!(turn.error.is_none(), "the turn in flight: {:?}", turn.error);
    }

    /// A transcript edit on a session whose durability gate requires a
    /// reload is refused with the runtime's typed reload-required refusal,
    /// not with SESSION_BUSY: the session is not busy, and no retry admits
    /// the edit until the session is reloaded. The session is registered
    /// after a server restart without a runtime loop, so the refusal is the
    /// admission's own.
    #[tokio::test]
    async fn transcript_edits_on_a_session_that_needs_a_durability_reload_answer_its_typed_refusal()
    {
        // Bounds a failure only; the passing path never waits for it.
        const FAILURE_BOUND: Duration = Duration::from_secs(30);
        let temp = tempfile::tempdir().unwrap();
        let stores = ReopenableServerStores::new();
        let (router, runtime, _notif_rx) = stores.open_server(temp.path());
        let create_resp = router
            .dispatch(make_request(
                "session/create",
                serde_json::json!({
                    "prompt": "Hello",
                    "system_prompt": "You are a test assistant.",
                }),
            ))
            .await
            .unwrap();
        let session_id = result_value(&create_resp)["session_id"]
            .as_str()
            .unwrap()
            .to_string();
        tokio::time::timeout(Duration::from_secs(60), runtime.try_shutdown())
            .await
            .expect("the first server shuts down in bounded time")
            .expect("the first server shuts down");
        drop(router);
        drop(runtime);
        let (router, _runtime, _notif_rx) = stores.open_server(temp.path());
        let parsed_session_id = SessionId::parse(&session_id).expect("session id");
        router
            .runtime_adapter
            .prepare_bindings(parsed_session_id.clone())
            .await
            .expect("register the idle session without a runtime loop");
        assert!(
            router
                .runtime_adapter
                .force_session_durability_reload_required_for_test(&parsed_session_id)
                .await,
            "the registered session owns a real persistent durability gate"
        );

        let fork_resp = tokio::time::timeout(
            FAILURE_BOUND,
            router.dispatch(make_request(
                "session/fork_at",
                serde_json::json!({
                    "session_id": &session_id,
                    "message_index": 1,
                    "running_behavior": "reject"
                }),
            )),
        )
        .await
        .expect("session/fork_at answers in bounded time")
        .unwrap();
        let update_resp = tokio::time::timeout(
            FAILURE_BOUND,
            router.dispatch(make_request(
                "session/update_system_prompt",
                serde_json::json!({
                    "session_id": &session_id,
                    "key": "persona",
                    "target_message_index": 0,
                    "content": "You are an edited test assistant."
                }),
            )),
        )
        .await
        .expect("session/update_system_prompt answers in bounded time")
        .unwrap();
        for (method, response) in [
            ("session/fork_at", &fork_resp),
            ("session/update_system_prompt", &update_resp),
        ] {
            let error = response.error.as_ref().unwrap_or_else(|| {
                panic!("{method} is refused on a session that needs a reload: {response:?}")
            });
            assert_ne!(
                error.code,
                error::SESSION_BUSY,
                "{method} is not refused as busy: {error:?}"
            );
            assert!(
                error
                    .message
                    .contains("durability reload required after `test_reload_required_discard`"),
                "{method} carries the typed reload-required refusal: {error:?}"
            );
        }
    }

    /// A transcript edit on a session whose durability gate requires a
    /// reload while its runtime loop is still attached (the session ran its
    /// first turn in this server) answers the typed reload-required refusal
    /// in bounded time: neither edit waits on the degraded session.
    #[tokio::test]
    async fn transcript_edit_on_a_reload_required_session_with_an_attached_runtime_loop_answers() {
        // Bounds a failure only; the passing path never waits for it.
        const STEP_BOUND: Duration = Duration::from_secs(45);
        let (router, _notif_rx) = test_router().await;
        let create_resp = tokio::time::timeout(
            STEP_BOUND,
            router.dispatch(make_request(
                "session/create",
                serde_json::json!({
                    "prompt": "Hello",
                    "system_prompt": "You are a test assistant.",
                }),
            )),
        )
        .await
        .expect("session/create answers in bounded time")
        .unwrap();
        let session_id = result_value(&create_resp)["session_id"]
            .as_str()
            .unwrap()
            .to_string();
        let parsed_session_id = SessionId::parse(&session_id).expect("session id");
        assert!(
            router
                .runtime_adapter
                .force_session_durability_reload_required_for_test(&parsed_session_id)
                .await,
            "the created session owns a real persistent durability gate"
        );

        for (method, params) in [
            (
                "session/fork_at",
                serde_json::json!({
                    "session_id": &session_id,
                    "message_index": 1,
                    "running_behavior": "reject"
                }),
            ),
            (
                "session/update_system_prompt",
                serde_json::json!({
                    "session_id": &session_id,
                    "key": "persona",
                    "target_message_index": 0,
                    "content": "You are an edited test assistant."
                }),
            ),
        ] {
            let response = tokio::time::timeout(
                STEP_BOUND,
                router.dispatch(make_request(method, params)),
            )
            .await
            .unwrap_or_else(|_| {
                panic!("{method} on a reload-required session did not answer within {STEP_BOUND:?}")
            })
            .unwrap();
            let error = response.error.as_ref().unwrap_or_else(|| {
                panic!("{method} is refused on a session that needs a reload: {response:?}")
            });
            assert_ne!(
                error.code,
                error::SESSION_BUSY,
                "{method} is not refused as busy: {error:?}"
            );
            assert!(
                error
                    .message
                    .contains("durability reload required after `test_reload_required_discard`"),
                "{method} carries the typed reload-required refusal: {error:?}"
            );
        }
    }

    /// A router over a runtime store the test can also reach.
    async fn test_router_with_runtime_store() -> (
        MethodRouter,
        mpsc::Receiver<RpcNotification>,
        Arc<dyn meerkat_runtime::RuntimeStore>,
    ) {
        let temp = tempfile::tempdir().unwrap();
        let factory = AgentFactory::new(temp.path().join("sessions"));
        let config = anthropic_test_config();
        let config_store: Arc<dyn ConfigStore> = Arc::new(MemoryConfigStore::new(
            config.clone(),
            meerkat_models::canonical(),
        ));
        let store: Arc<dyn meerkat::SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> =
            Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());
        let persistence =
            meerkat::PersistenceBundle::new(store, Arc::clone(&runtime_store), memory_blob_store())
                .expect("construct runtime authority");
        let runtime =
            SessionRuntime::new(factory, config, 10, persistence, NotificationSink::noop())
                .expect("compose the RPC session runtime");
        runtime.set_default_llm_client(Some(Arc::new(MockLlmClient::anthropic())));
        runtime.set_config_runtime(Arc::new(ConfigRuntime::new(
            Arc::clone(&config_store),
            temp.path().join("config_state.json"),
        )));
        let runtime = Arc::new(runtime);
        let (notif_tx, notif_rx) = mpsc::channel(100);
        let router = MethodRouter::new(runtime, config_store, NotificationSink::new(notif_tx))
            .expect("construct runtime authority");
        (router, notif_rx, runtime_store)
    }

    /// A session with metadata but no build state (a child forked by an
    /// earlier release has that shape) cannot be forked: both fork methods
    /// answer invalid params with the refusal's kind and source.
    #[tokio::test]
    async fn session_forks_of_a_source_without_build_state_answer_invalid_params() {
        let (router, _notif_rx, runtime_store) = test_router_with_runtime_store().await;
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
        let history_resp = router
            .dispatch(make_request(
                "session/history",
                serde_json::json!({ "session_id": &session_id }),
            ))
            .await
            .unwrap();
        let hello_index = result_value(&history_resp)["messages"]
            .as_array()
            .unwrap()
            .iter()
            .position(|message| message["role"] == "user" && message["content"] == "Hello")
            .expect("the source transcript holds the first user message");

        // Commit the source's body again without its build state.
        let runtime_id = meerkat_runtime::identifiers::LogicalRuntimeId::for_session(
            &SessionId::parse(&session_id).expect("session id"),
        );
        let committed = runtime_store
            .load_committed_whole_blob_snapshot(&runtime_id)
            .await
            .expect("load the committed source")
            .expect("the source is committed");
        assert!(
            committed
                .session()
                .try_session_metadata()
                .expect("decode source metadata")
                .is_some(),
            "the source has canonical metadata"
        );
        let mut document = serde_json::to_value(committed.session()).expect("source document");
        assert!(
            document["metadata"]
                .as_object_mut()
                .expect("metadata map")
                .remove(meerkat_core::SESSION_BUILD_STATE_KEY)
                .is_some(),
            "the source had a build state"
        );
        runtime_store
            .commit_session_snapshot(
                &runtime_id,
                meerkat_runtime::store::SerializedSessionSnapshot {
                    session_snapshot: serde_json::to_vec(&document)
                        .expect("encode source document")
                        .into(),
                },
            )
            .await
            .expect("commit the source without its build state");

        let fork_at = router
            .dispatch(make_request(
                "session/fork_at",
                serde_json::json!({
                    "session_id": &session_id,
                    "message_index": hello_index + 1,
                    "running_behavior": "reject"
                }),
            ))
            .await
            .unwrap();
        let fork_replace = router
            .dispatch(make_request(
                "session/fork_replace",
                serde_json::json!({
                    "session_id": &session_id,
                    "message_index": hello_index,
                    "replacement": {
                        "type": "message",
                        "message": {
                            "role": "user",
                            "content": "Edited hello"
                        }
                    },
                    "running_behavior": "reject"
                }),
            ))
            .await
            .unwrap();
        for (method, response) in [
            ("session/fork_at", &fork_at),
            ("session/fork_replace", &fork_replace),
        ] {
            let error = response
                .error
                .as_ref()
                .unwrap_or_else(|| panic!("{method} is refused: {response:?}"));
            assert_eq!(error.code, error::INVALID_PARAMS, "{method}: {error:?}");
            let data = error.data.as_ref().expect("the refusal carries its data");
            assert_eq!(
                data["kind"], "fork_source_build_state_missing",
                "{method}: {error:?}"
            );
            assert_eq!(
                data["source_session_id"],
                session_id.as_str(),
                "{method}: {error:?}"
            );
        }
    }

    /// A fork child of a source created with additional instructions runs
    /// its first turn on the transcript's own System message: the request
    /// carries that System message and no second one authored from the
    /// instructions its build state carries.
    #[tokio::test]
    async fn session_fork_child_first_turn_keeps_the_prefix_system_message() {
        const INSTRUCTION: &str = "Answer in one short sentence.";
        let requests = Arc::new(std::sync::Mutex::new(Vec::<Vec<Message>>::new()));
        let (router, _notif_rx) =
            test_router_with_llm(Arc::new(RecordingMockLlmClient::new(Arc::clone(&requests))))
                .await;
        let create_resp = router
            .dispatch(make_request(
                "session/create",
                serde_json::json!({
                    "prompt": "Hello",
                    "additional_instructions": [INSTRUCTION],
                }),
            ))
            .await
            .unwrap();
        assert!(create_resp.error.is_none(), "{create_resp:?}");
        let session_id = result_value(&create_resp)["session_id"]
            .as_str()
            .unwrap()
            .to_string();
        let source_system = {
            let recorded = requests.lock().expect("recorded requests lock poisoned");
            let source_request = recorded.last().expect("the source's turn was sent");
            system_prompt_from_request(source_request)
                .expect("the source's turn carries its System message")
                .to_string()
        };
        assert!(
            source_system.contains(INSTRUCTION),
            "the source's System message carries the instruction: {source_system}"
        );
        let history_resp = router
            .dispatch(make_request(
                "session/history",
                serde_json::json!({ "session_id": &session_id }),
            ))
            .await
            .unwrap();
        let source_len = result_value(&history_resp)["message_count"]
            .as_u64()
            .expect("message count");

        let fork_resp = router
            .dispatch(make_request(
                "session/fork_at",
                serde_json::json!({
                    "session_id": &session_id,
                    "message_index": source_len,
                    "running_behavior": "reject"
                }),
            ))
            .await
            .unwrap();
        assert!(fork_resp.error.is_none(), "the fork commits: {fork_resp:?}");
        let forked_id = result_value(&fork_resp)["session_id"]
            .as_str()
            .unwrap()
            .to_string();
        let turn_resp = router
            .dispatch(make_request(
                "turn/start",
                serde_json::json!({
                    "session_id": &forked_id,
                    "prompt": "Follow up on the branch",
                }),
            ))
            .await
            .unwrap();
        assert!(turn_resp.error.is_none(), "the child runs: {turn_resp:?}");

        let recorded = requests.lock().expect("recorded requests lock poisoned");
        let child_request = recorded.last().expect("the child's turn was sent");
        let system_messages: Vec<&str> = child_request
            .iter()
            .filter_map(|message| match message {
                Message::System(system) => Some(system.content.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            system_messages,
            vec![source_system.as_str()],
            "the child's turn sends the prefix's System message and no second one"
        );
    }

    #[tokio::test]
    async fn archived_session_rejects_later_turn_after_system_append() {
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

        let injected_text = "This System message precedes archive.";
        let inject_resp = router
            .dispatch(make_request(
                "session/inject_context",
                serde_json::json!({
                    "session_id": session_id,
                    "content": { "type": "text", "text": injected_text },
                    "source": "mob",
                    "idempotency_key": "ctx-archive-drop"
                }),
            ))
            .await
            .unwrap();
        assert_eq!(result_value(&inject_resp)["status"], "applied");

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
            "archived session must reject later turns after the System append"
        );
        assert!(
            recorded_requests
                .lock()
                .expect("recorded requests lock poisoned")
                .is_empty(),
            "archived session must not reach the LLM again"
        );

        let notification = tokio::time::timeout(std::time::Duration::from_millis(200), async {
            notif_rx.recv().await
        })
        .await;
        match notification {
            Err(_) => {}
            Ok(None) => {}
            Ok(Some(notif)) => {
                let params = notification_params!(notif);
                let event_type = params["event"]["payload"]["type"].as_str();
                assert!(
                    !(notif.method == "session/event"
                        && event_type == Some("run_started")
                        && params["session_id"].as_str() == Some(session_id.as_str())
                        && params["event"]["payload"]["prompt"]
                            .as_str()
                            .is_some_and(|prompt| prompt.contains(injected_text))),
                    "archived session must not emit a later run_started"
                );
            }
        }
    }

    #[tokio::test]
    async fn archived_session_is_not_found_and_mutations_reject() {
        Box::pin(archived_session_is_not_found_and_mutations_reject_inner()).await;
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

    // Debug worker-stack canary for the RPC dispatch path.
    //
    // At opt-level 0 LLVM does not colour stack slots, so every local in
    // every branch of a function gets its own slot and a frame is the SUM of
    // its branches. That is the shape that reached 24 MiB on this harness
    // (13.4 MiB of it in `dispatch_routed_with_request_context` alone) and
    // forced 32 MiB worker stacks plus a workspace-wide `RUST_MIN_STACK`.
    // After the per-arm frames (`routed_arm`, `box_in_own_frame`) the same
    // path fits well under this budget; release needs under 1 MiB.
    //
    // The canary runs the real router through session/create, turn/start,
    // and session/archive on ONE thread with a literal stack budget, using a
    // current-thread runtime so no other stack is involved. It is measured at
    // opt-level 0 on purpose: it fails on exactly the debug frame bloat that
    // release hides, before a release measurement ever sees it. Never
    // re-ignore it, raise the opt-level, or relax the budget; move
    // construction into its own frame instead (see
    // `meerkat_runtime::stack_relief`).
    const DEBUG_DISPATCH_STACK_BUDGET: usize = 4 * 1024 * 1024;

    #[test]
    fn rpc_dispatch_path_fits_debug_worker_stack_budget() {
        std::thread::Builder::new()
            .name("rpc-dispatch-stack-canary".to_string())
            .stack_size(DEBUG_DISPATCH_STACK_BUDGET)
            .spawn(|| {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("canary runtime should build");
                runtime.block_on(async {
                    let (router, _notif_rx) = test_router().await;
                    let create_req =
                        make_request("session/create", serde_json::json!({"prompt": "Hello"}));
                    let create_resp = router.dispatch(create_req).await.unwrap();
                    let session_id = result_value(&create_resp)["session_id"]
                        .as_str()
                        .unwrap()
                        .to_string();
                    let turn_req = make_request(
                        "turn/start",
                        serde_json::json!({"session_id": session_id, "prompt": "Follow up"}),
                    );
                    let turn_resp = router.dispatch(turn_req).await.unwrap();
                    assert_eq!(
                        result_value(&turn_resp)["session_id"].as_str().unwrap(),
                        session_id
                    );
                    let archive_req = make_request(
                        "session/archive",
                        serde_json::json!({"session_id": session_id}),
                    );
                    let archive_resp = router.dispatch(archive_req).await.unwrap();
                    assert_eq!(result_value(&archive_resp)["archived"], true);
                });
            })
            .expect("canary thread should spawn")
            .join()
            .expect("the RPC dispatch path must fit the debug worker-stack budget");
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

    /// The build settings a turn cannot change, with a valid value each.
    fn turn_build_settings() -> [(&'static str, serde_json::Value); 3] {
        [
            ("max_tokens", serde_json::json!(1024)),
            ("output_schema", serde_json::json!({"type": "object"})),
            ("structured_output_retries", serde_json::json!(5)),
        ]
    }

    /// `turn/start` refuses a build setting while it parses its params,
    /// before any session work: even an unknown session gets the typed
    /// invalid-params refusal that names the setting, not a session lookup
    /// result or a refusal that only arrives after the session was touched.
    #[tokio::test]
    async fn turn_start_refuses_build_settings_before_session_work() {
        let (router, _notif_rx) = test_router().await;
        let unknown_session = SessionId::new().to_string();
        for (field, value) in turn_build_settings() {
            let mut params = serde_json::json!({
                "session_id": &unknown_session,
                "prompt": "Follow up"
            });
            params[field] = value;
            let response = router
                .dispatch(make_request("turn/start", params))
                .await
                .unwrap();
            let error = response
                .error
                .as_ref()
                .unwrap_or_else(|| panic!("turn/start must refuse {field}"));
            assert_eq!(
                error.code,
                error::INVALID_PARAMS,
                "{field} is refused as invalid params while parsing: {error:?}"
            );
            assert!(
                error.message.contains(field) && error.message.contains("session/create"),
                "the refusal names {field} and where to set it: {error:?}"
            );
        }
    }

    /// `mob/turn_start` refuses a build setting as soon as its params are
    /// parsed, before it resolves the mob or the member.
    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn mob_turn_start_refuses_build_settings_before_member_resolution() {
        let (router, _notif_rx) = test_router().await;
        for (field, value) in turn_build_settings() {
            let mut params = serde_json::json!({
                "mob_id": "no-such-mob",
                "agent_identity": "worker-1",
                "prompt": "Follow up"
            });
            params[field] = value;
            let response = router
                .dispatch(make_request("mob/turn_start", params))
                .await
                .unwrap();
            let error = response
                .error
                .as_ref()
                .unwrap_or_else(|| panic!("mob/turn_start must refuse {field}"));
            assert_eq!(
                error.code,
                error::INVALID_PARAMS,
                "{field} is refused as invalid params before member resolution: {error:?}"
            );
            assert!(
                error.message.contains(field),
                "the refusal names {field}: {error:?}"
            );
        }
    }

    /// Answers the first provider call and fails every later one with a
    /// non-retryable provider error.
    struct FailAfterFirstLlmClient {
        calls: std::sync::atomic::AtomicUsize,
    }

    #[async_trait]
    impl LlmClient for FailAfterFirstLlmClient {
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
            if self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) > 0 {
                return Box::pin(stream::iter(vec![Err(LlmError::InvalidRequest {
                    message: "synthetic provider refusal".to_string(),
                })]));
            }
            Box::pin(stream::iter(vec![
                Ok(meerkat_client::LlmEvent::TextDelta {
                    delta: "first turn succeeds".to_string(),
                    meta: None,
                }),
                Ok(meerkat_client::LlmEvent::UsageUpdate {
                    usage: meerkat_core::TurnUsage::host_declared(
                        meerkat_core::Provider::Anthropic,
                        &request.model,
                        meerkat_core::Usage::default(),
                    ),
                }),
                Ok(meerkat_client::LlmEvent::Done {
                    outcome: meerkat_client::LlmDoneOutcome::success(StopReason::EndTurn),
                }),
            ]))
        }

        fn provider(&self) -> meerkat_core::Provider {
            meerkat_core::Provider::Anthropic
        }

        async fn health_check(&self) -> Result<(), LlmError> {
            Ok(())
        }
    }

    /// A turn whose provider call fails reports the canonical provider-error
    /// code, as the documented session-error mapping does for the same
    /// cause, with the typed turn error kept in `data.error`; never the
    /// generic internal error.
    #[tokio::test]
    async fn turn_start_provider_failure_reports_provider_error_code() {
        let (router, _notif_rx) = test_router_with_llm(Arc::new(FailAfterFirstLlmClient {
            calls: std::sync::atomic::AtomicUsize::new(0),
        }))
        .await;
        let created = router
            .dispatch(make_request(
                "session/create",
                serde_json::json!({ "prompt": "the first turn succeeds" }),
            ))
            .await
            .unwrap();
        let session_id = result_value(&created)["session_id"]
            .as_str()
            .unwrap()
            .to_string();

        let failed = router
            .dispatch(make_request(
                "turn/start",
                serde_json::json!({
                    "session_id": session_id,
                    "prompt": "this turn's provider call fails"
                }),
            ))
            .await
            .unwrap();
        let error = failed
            .error
            .as_ref()
            .expect("a failed provider call fails the turn");
        assert_eq!(
            error.code,
            error::PROVIDER_ERROR,
            "a provider failure reports PROVIDER_ERROR: {error:?}"
        );
        let data = error.data.as_ref().expect("typed turn error data");
        assert_eq!(data["error"]["kind"], "llm_failure", "{data}");
    }

    /// Holds every provider call until the test releases it, and records the
    /// transcript each call saw.
    struct GatedLlmClient {
        calls: Arc<tokio::sync::watch::Sender<usize>>,
        release: Arc<tokio::sync::Semaphore>,
        requests: Arc<StdMutex<Vec<String>>>,
    }

    #[async_trait]
    impl LlmClient for GatedLlmClient {
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
                .push(serde_json::to_string(&request.messages).expect("serialize messages"));
            self.calls.send_modify(|calls| *calls += 1);
            let release = Arc::clone(&self.release);
            let usage = meerkat_core::TurnUsage::host_declared(
                meerkat_core::Provider::Anthropic,
                &request.model,
                meerkat_core::Usage::default(),
            );
            Box::pin(stream::unfold(0u8, move |state| {
                let release = Arc::clone(&release);
                let usage = usage.clone();
                async move {
                    match state {
                        0 => {
                            release
                                .acquire()
                                .await
                                .expect("release semaphore stays open")
                                .forget();
                            Some((
                                Ok(meerkat_client::LlmEvent::TextDelta {
                                    delta: "gated reply".to_string(),
                                    meta: None,
                                }),
                                1,
                            ))
                        }
                        1 => Some((Ok(meerkat_client::LlmEvent::UsageUpdate { usage }), 2)),
                        2 => Some((
                            Ok(meerkat_client::LlmEvent::Done {
                                outcome: meerkat_client::LlmDoneOutcome::success(
                                    StopReason::EndTurn,
                                ),
                            }),
                            3,
                        )),
                        _ => None,
                    }
                }
            }))
        }

        fn provider(&self) -> meerkat_core::Provider {
            meerkat_core::Provider::Anthropic
        }

        async fn health_check(&self) -> Result<(), LlmError> {
            Ok(())
        }
    }

    /// A `turn/start` that arrives while another turn on the same session is
    /// running is queued behind it, not refused with SESSION_BUSY: it runs
    /// once the running turn finishes and returns its own result.
    #[tokio::test]
    async fn turn_start_while_a_turn_runs_queues_instead_of_session_busy() {
        // Bounds a failure only; the passing path never waits for it.
        const FAILURE_BOUND: Duration = Duration::from_secs(30);
        let calls = Arc::new(tokio::sync::watch::channel(0usize).0);
        let release = Arc::new(tokio::sync::Semaphore::new(0));
        let requests = Arc::new(StdMutex::new(Vec::new()));
        let (router, _notif_rx) = test_router_with_llm(Arc::new(GatedLlmClient {
            calls: Arc::clone(&calls),
            release: Arc::clone(&release),
            requests: Arc::clone(&requests),
        }))
        .await;
        let created = router
            .dispatch(make_request(
                "session/create",
                serde_json::json!({ "prompt": "seed", "initial_turn": "deferred" }),
            ))
            .await
            .unwrap();
        let session_id = result_value(&created)["session_id"]
            .as_str()
            .unwrap()
            .to_string();
        let turn = |id: i64, prompt: &'static str| {
            let router = router.clone();
            let session_id = session_id.clone();
            tokio::spawn(async move {
                let mut request = make_request(
                    "turn/start",
                    serde_json::json!({ "session_id": session_id, "prompt": prompt }),
                );
                request.id = Some(RpcId::Num(id));
                router.dispatch(request).await.expect("turn/start response")
            })
        };
        let mut seen = calls.subscribe();

        let first = turn(1, "first running turn");
        tokio::time::timeout(FAILURE_BOUND, seen.wait_for(|calls| *calls >= 1))
            .await
            .expect("the first turn reaches the provider")
            .expect("call counter stays open");

        let mut second = turn(2, "second queued turn");
        assert!(
            tokio::time::timeout(Duration::from_millis(300), &mut second)
                .await
                .is_err(),
            "a turn/start on a running session waits behind the running turn \
             instead of answering at once (no SESSION_BUSY)"
        );

        release.add_permits(1);
        let first = tokio::time::timeout(FAILURE_BOUND, first)
            .await
            .expect("the first turn finishes once released")
            .expect("first turn task");
        assert!(first.error.is_none(), "first turn: {:?}", first.error);
        tokio::time::timeout(FAILURE_BOUND, seen.wait_for(|calls| *calls >= 2))
            .await
            .expect("the queued turn runs after the first one")
            .expect("call counter stays open");
        release.add_permits(1);
        let second = tokio::time::timeout(FAILURE_BOUND, second)
            .await
            .expect("the queued turn finishes once released")
            .expect("second turn task");
        assert!(
            second.error.is_none(),
            "the queued turn returns its own result, not SESSION_BUSY: {:?}",
            second.error
        );
        assert_eq!(result_value(&second)["session_id"], session_id.as_str());

        let requests = requests.lock().expect("recorded requests lock poisoned");
        assert_eq!(
            requests.len(),
            2,
            "one provider call per turn: {requests:?}"
        );
        assert!(
            requests[0].contains("first running turn")
                && !requests[0].contains("second queued turn"),
            "the first call sees only the first turn: {}",
            requests[0]
        );
        assert!(
            requests[1].contains("second queued turn"),
            "the second call is the queued turn: {}",
            requests[1]
        );
    }

    /// A `mob/create` whose RPC handler is dropped mid-creation (a cancelled
    /// or disconnected caller) still registers its mob. The composed RPC mob
    /// state runs each create as its own task, so a create that already
    /// wrote its durable store is never left outside the managed set.
    #[cfg(feature = "mob")]
    #[tokio::test]
    async fn mob_create_dropped_mid_creation_still_registers_its_mob() {
        // Bounds a failure only; the passing path never waits for it.
        const FAILURE_BOUND: Duration = Duration::from_secs(10);
        let root = tempfile::tempdir().unwrap();
        let root_path = root.path().to_path_buf();
        let config_store = meerkat_core::TaggedConfigStore::new(
            Arc::new(MemoryConfigStore::new(
                anthropic_test_config(),
                meerkat_models::canonical(),
            )),
            meerkat_core::ConfigStoreMetadata {
                realm_id: None,
                instance_id: None,
                backend: None,
                resolved_paths: Some(meerkat_core::ConfigResolvedPaths {
                    root: root_path.display().to_string(),
                    manifest_path: root_path.join("realm.toml").display().to_string(),
                    config_path: root_path.join("config.toml").display().to_string(),
                    sessions_sqlite_path: None,
                    sessions_jsonl_dir: root_path.join("sessions").display().to_string(),
                }),
            },
        );
        let (router, _notif_rx) = test_router_with_config_store(Arc::new(config_store)).await;
        let mob_id = "dropped-mid-create";
        let store = root_path.join("mobs").join(format!("mob--{mob_id}.db"));

        let mut create = Box::pin(router.dispatch(make_request(
            "mob/create",
            serde_json::json!({
                "definition": {
                    "id": mob_id,
                    "profiles": { "worker": { "model": "claude-sonnet-4-5" } }
                }
            }),
        )));
        // Drive the create one step at a time until it has written its
        // durable store, then drop it there, mid-creation.
        let deadline = tokio::time::Instant::now() + FAILURE_BOUND;
        loop {
            if let std::task::Poll::Ready(response) = futures::poll!(create.as_mut()) {
                panic!("the create finished before it could be dropped mid-creation: {response:?}");
            }
            if store.exists() {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "the create never reached its durable store at {}",
                store.display()
            );
            tokio::task::yield_now().await;
        }
        drop(create);

        let registered = tokio::time::timeout(FAILURE_BOUND, async {
            loop {
                let listed = router
                    .dispatch(make_request("mob/list", serde_json::json!({})))
                    .await
                    .unwrap();
                if result_value(&listed)["mobs"]
                    .as_array()
                    .is_some_and(|mobs| mobs.iter().any(|mob| mob["mob_id"] == mob_id))
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
        assert!(
            registered.is_ok(),
            "a create dropped mid-creation left its store at {} without a managed mob",
            store.display()
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
            let params = notification_params!(notif);
            assert!(params["session_id"].is_string());
            assert!(params["event"].is_object());
        }
    }

    // -----------------------------------------------------------------------
    // Structured output over JSON-RPC: validate-first and extraction fallback
    // -----------------------------------------------------------------------

    /// The loop's default extraction prompt, verbatim.
    const RPC_DEFAULT_EXTRACTION_PROMPT: &str = "Provide the final output as valid JSON \
matching the required schema. Output ONLY the JSON, no additional text or markdown formatting.";

    /// Scripted client for structured-output runs: answers the n-th request
    /// with the n-th scripted reply and records every request it receives.
    struct ScriptedStructuredOutputClient {
        replies: Vec<&'static str>,
        requests: Arc<std::sync::Mutex<Vec<meerkat_client::LlmRequest>>>,
    }

    #[async_trait]
    impl LlmClient for ScriptedStructuredOutputClient {
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
            let reply = {
                let mut requests = self.requests.lock().expect("recorded requests lock");
                requests.push(request.clone());
                self.replies
                    .get(requests.len() - 1)
                    .copied()
                    .expect("the run sent more requests than scripted")
            };
            Box::pin(stream::iter(vec![
                Ok(meerkat_client::LlmEvent::TextDelta {
                    delta: reply.to_string(),
                    meta: None,
                }),
                Ok(meerkat_client::LlmEvent::UsageUpdate {
                    usage: meerkat_core::TurnUsage::host_declared(
                        meerkat_core::Provider::Anthropic,
                        &request.model,
                        meerkat_core::Usage::default(),
                    ),
                }),
                Ok(meerkat_client::LlmEvent::Done {
                    outcome: meerkat_client::LlmDoneOutcome::success(StopReason::EndTurn),
                }),
            ]))
        }

        fn provider(&self) -> meerkat_core::Provider {
            meerkat_core::Provider::Anthropic
        }

        async fn health_check(&self) -> Result<(), LlmError> {
            Ok(())
        }
    }

    fn rpc_verdict_schema() -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "verdict": {"type": "string", "enum": ["approve", "reject"]}
            },
            "required": ["verdict"],
            "additionalProperties": false
        })
    }

    /// Run `session/create` with an output schema against the scripted
    /// replies. Returns the result, the recorded requests, and the
    /// `session/event` payloads in emission order, collected until the run's
    /// structured-output terminal event arrived.
    async fn run_structured_output_session_create(
        replies: Vec<&'static str>,
    ) -> (
        serde_json::Value,
        Vec<meerkat_client::LlmRequest>,
        Vec<serde_json::Value>,
    ) {
        let requests = Arc::new(std::sync::Mutex::new(Vec::new()));
        let (router, mut notif_rx) =
            test_router_with_llm(Arc::new(ScriptedStructuredOutputClient {
                replies,
                requests: Arc::clone(&requests),
            }))
            .await;

        let response = router
            .dispatch(make_request(
                "session/create",
                serde_json::json!({
                    "prompt": "Review the change and give a verdict.",
                    "output_schema": rpc_verdict_schema()
                }),
            ))
            .await
            .expect("session/create response");
        let result = result_value(&response);

        let payloads = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            let mut payloads = Vec::new();
            loop {
                let notif = notif_rx.recv().await.expect("notification");
                if notif.method != "session/event" {
                    continue;
                }
                let payload = notification_params!(notif)["event"]["payload"].clone();
                let terminal = matches!(
                    payload["type"].as_str(),
                    Some("extraction_succeeded" | "extraction_failed")
                );
                payloads.push(payload);
                if terminal {
                    break payloads;
                }
            }
        })
        .await
        .expect("the structured-output terminal event must reach the notification sink");
        let recorded = requests.lock().expect("recorded requests lock").clone();
        (result, recorded, payloads)
    }

    fn payload_positions(payloads: &[serde_json::Value], kind: &str) -> Vec<usize> {
        payloads
            .iter()
            .enumerate()
            .filter(|(_, payload)| payload["type"] == kind)
            .map(|(index, _)| index)
            .collect()
    }

    fn leads_with_schema_section(request: &meerkat_client::LlmRequest) -> bool {
        matches!(
            request.messages.first(),
            Some(Message::System(system))
                if system.content.matches(
                    meerkat_core::structured_output::OUTPUT_SCHEMA_INSTRUCTIONS_OPEN
                ).count() == 1
        )
    }

    /// Validate-first over JSON-RPC: the final reply already matches the
    /// schema, so no extraction request is sent. `run_completed` (announcing
    /// that extraction is required and carrying no structured output) is
    /// notified before `extraction_succeeded`, which carries
    /// `origin: "final_reply"` on the wire, and the `session/create` result
    /// carries the structured output.
    #[tokio::test]
    async fn session_create_validate_first_notifies_run_completed_then_final_reply_extraction() {
        let (result, requests, payloads) =
            run_structured_output_session_create(vec![r#"{"verdict":"approve"}"#]).await;

        assert_eq!(
            requests.len(),
            1,
            "validate-first sends no extraction request"
        );
        assert!(
            leads_with_schema_section(&requests[0]),
            "the request leads with exactly one schema section: {:?}",
            requests[0].messages.first()
        );

        assert_eq!(
            result["structured_output"],
            serde_json::json!({"verdict": "approve"})
        );
        assert_eq!(result["turns"], 1);
        assert_eq!(result["text"], r#"{"verdict":"approve"}"#);
        assert!(result.get("extraction_error").is_none(), "{result}");

        let run_completed = payload_positions(&payloads, "run_completed");
        let succeeded = payload_positions(&payloads, "extraction_succeeded");
        assert_eq!(run_completed.len(), 1, "one run_completed: {payloads:#?}");
        assert_eq!(
            succeeded.len(),
            1,
            "one extraction_succeeded: {payloads:#?}"
        );
        assert!(
            run_completed[0] < succeeded[0],
            "run_completed must be notified before extraction_succeeded: {payloads:#?}"
        );
        assert!(payload_positions(&payloads, "extraction_failed").is_empty());

        let run_completed = &payloads[run_completed[0]];
        assert_eq!(run_completed["extraction_required"], true);
        assert!(
            run_completed.get("structured_output").is_none(),
            "run_completed carries no structured output when extraction is required: {run_completed}"
        );

        let succeeded = &payloads[succeeded[0]];
        assert_eq!(succeeded["origin"], "final_reply");
        assert_eq!(
            succeeded["structured_output"],
            serde_json::json!({"verdict": "approve"})
        );
        assert!(
            succeeded.get("request_usage").is_none(),
            "no extraction request ran, so there are no extraction usage rows: {succeeded}"
        );
        assert_eq!(succeeded["session_id"], result["session_id"]);
    }

    /// Extraction fallback over JSON-RPC: a prose final reply fails
    /// validate-first, the unchanged extraction request runs (default prompt,
    /// temperature 0, no tools, same schema section), and
    /// `extraction_succeeded` follows `run_completed` with `origin` omitted
    /// (an extraction request produced the value).
    #[tokio::test]
    async fn session_create_extraction_fallback_notifies_run_completed_then_extraction_request() {
        let (result, requests, payloads) = run_structured_output_session_create(vec![
            "I approve this change.",
            r#"{"verdict":"approve"}"#,
        ])
        .await;

        assert_eq!(requests.len(), 2, "the extraction request ran");
        assert!(leads_with_schema_section(&requests[0]));
        assert!(leads_with_schema_section(&requests[1]));
        assert_eq!(
            requests[0].messages.first(),
            requests[1].messages.first(),
            "the extraction request keeps the main turn's system prompt byte for byte"
        );
        let extraction = &requests[1];
        assert!(
            matches!(
                extraction.messages.last(),
                Some(Message::User(user)) if user.text_content() == RPC_DEFAULT_EXTRACTION_PROMPT
            ),
            "the unchanged extraction prompt closes the request: {:?}",
            extraction.messages.last()
        );
        assert_eq!(extraction.temperature, Some(0.0));
        assert!(extraction.tools.is_empty(), "no tools on extraction");

        assert_eq!(
            result["structured_output"],
            serde_json::json!({"verdict": "approve"})
        );
        assert_eq!(result["turns"], 2);
        assert_eq!(
            result["text"], "I approve this change.",
            "text stays the primary final reply"
        );
        assert!(result.get("extraction_error").is_none(), "{result}");

        let run_completed = payload_positions(&payloads, "run_completed");
        let succeeded = payload_positions(&payloads, "extraction_succeeded");
        assert_eq!(run_completed.len(), 1, "{payloads:#?}");
        assert_eq!(succeeded.len(), 1, "{payloads:#?}");
        assert!(run_completed[0] < succeeded[0], "{payloads:#?}");
        assert!(payload_positions(&payloads, "extraction_failed").is_empty());
        assert_eq!(payloads[run_completed[0]]["extraction_required"], true);
        let succeeded = &payloads[succeeded[0]];
        assert!(
            succeeded.get("origin").is_none(),
            "an extraction-request success omits origin on the wire: {succeeded}"
        );
        assert_eq!(
            succeeded["structured_output"],
            serde_json::json!({"verdict": "approve"})
        );
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
                        "source_uuid": "00000000-0000-4b11-8111-000000000001",
                        "skill_name": "mcp-server-setup"
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

    /// `turn/stop_run` wire contract: a stale run id on a live idle session
    /// is the typed `not_current` receipt (never an error, never an
    /// interrupt), and a malformed run id is invalid params.
    #[tokio::test]
    async fn turn_stop_run_reports_not_current_for_a_stale_run_and_rejects_malformed_ids() {
        let (router, _notif_rx) = test_router().await;
        let create_req = make_request("session/create", serde_json::json!({"prompt": "Hello"}));
        let create_resp = router.dispatch(create_req).await.unwrap();
        let session_id = result_value(&create_resp)["session_id"]
            .as_str()
            .unwrap()
            .to_string();
        let stale = meerkat_core::lifecycle::RunId::new().to_string();

        let resp = router
            .dispatch(make_request(
                "turn/stop_run",
                serde_json::json!({"session_id": session_id, "run_id": stale, "reason": "stale"}),
            ))
            .await
            .unwrap();
        let result = result_value(&resp);
        assert_eq!(result["session_id"], serde_json::json!(session_id));
        assert_eq!(result["receipt"]["outcome"], "not_current");
        assert_eq!(result["receipt"]["run_id"], serde_json::json!(stale));
        let decoded: meerkat_contracts::StopRunResult =
            serde_json::from_value(result.clone()).expect("typed StopRunResult");
        assert!(matches!(
            decoded.receipt,
            meerkat_contracts::WireRunStopReceipt::NotCurrent { .. }
        ));

        let resp = router
            .dispatch(make_request(
                "turn/stop_run",
                serde_json::json!({"session_id": session_id, "run_id": "not-a-uuid", "reason": "x"}),
            ))
            .await
            .unwrap();
        assert_eq!(error_code(&resp), error::INVALID_PARAMS);

        let resp = router
            .dispatch(make_request(
                "turn/stop_run",
                serde_json::json!({
                    "session_id": meerkat_core::SessionId::new().to_string(),
                    "run_id": stale,
                    "reason": "x"
                }),
            ))
            .await
            .unwrap();
        assert_eq!(error_code(&resp), error::SESSION_NOT_FOUND);
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
        let llm_override: Arc<dyn LlmClient> = Arc::new(MockLlmClient::anthropic());
        let created = router
            .runtime
            .core_session_service()
            .create_session(meerkat_core::service::CreateSessionRequest {
                injected_context: Vec::new(),
                model: "claude-sonnet-4-5".to_string(),
                prompt: "Hello".to_string().into(),
                system_prompt: meerkat::SystemPromptOverride::Inherit,
                max_tokens: None,
                event_tx: None,
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
    async fn session_create_returns_request_cancelled_before_stateful_work_when_pre_cancelled() {
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
            "pre-cancelled create must stop before creating a session: {sessions:?}"
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

        // `max_tokens` is optional (None => inherit / template default) and is
        // omitted from the serialized config when unset, so seed an explicit
        // value first, then verify a patch merges over it.
        let seed_max_tokens = 4096u64;
        let seed_req = make_request(
            "config/patch",
            serde_json::json!({"max_tokens": seed_max_tokens}),
        );
        let seed_resp = router.dispatch(seed_req).await.unwrap();
        let seeded = result_value(&seed_resp);
        assert_eq!(seeded["config"]["max_tokens"], seed_max_tokens);

        // Patch max_tokens to a different value
        let new_max_tokens = seed_max_tokens + 1000;
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

    /// 12e. `config/patch` on a child realm's document writes only the named
    /// keys: the child keeps inheriting its parent's tool policy, including
    /// capabilities the parent disabled, and a stale `expected_generation` is
    /// refused without touching the document.
    #[tokio::test]
    async fn config_patch_keeps_child_realm_presence_and_inherited_policy() {
        use meerkat_core::RealmConfigSource;
        use meerkat_core::connection::RealmId;

        let temp = tempfile::tempdir().unwrap();
        let source = Arc::new(meerkat_store::FilesystemRealmConfigSource::new(
            temp.path().join("realms"),
            temp.path().join("home").join(".rkat").join("config.toml"),
            meerkat_models::canonical(),
        ));
        let parent = RealmId::parse("parent").unwrap();
        let child = RealmId::parse("child").unwrap();
        let parent_path = source.config_doc_path(&parent);
        let child_path = source.config_doc_path(&child);
        for (path, content) in [
            (
                &parent_path,
                "[realm.parent]\n\n[tools]\nshell_enabled = true\nmax_concurrent = 3\n\
                 schedule_enabled = false\n\n[provider_tools.openai]\nweb_search = false\n",
            ),
            (
                &child_path,
                "[realm.child]\nparent = \"parent\"\n\n[skills]\nenabled = true\n",
            ),
        ] {
            tokio::fs::create_dir_all(path.parent().unwrap())
                .await
                .unwrap();
            tokio::fs::write(path, content).await.unwrap();
        }
        let (router, _notif_rx) = test_router_with_config_store(Arc::new(
            meerkat_core::FileConfigStore::new(child_path.clone(), meerkat_models::canonical()),
        ))
        .await;

        let get_resp = router
            .dispatch(make_request_no_params("config/get"))
            .await
            .unwrap();
        let generation = result_value(&get_resp)["generation"].as_u64().unwrap();
        let patch = serde_json::json!({
            "tools": { "mcp_servers": [{
                "name": "sentinel",
                "command": "sentinel-mcp",
                "args": ["--read-only"],
            }]}
        });
        let patch_resp = router
            .dispatch(make_request(
                "config/patch",
                serde_json::json!({ "patch": patch, "expected_generation": generation }),
            ))
            .await
            .unwrap();
        assert!(
            patch_resp.error.is_none(),
            "config/patch failed: {:?}",
            patch_resp.error
        );
        assert_eq!(
            result_value(&patch_resp)["generation"].as_u64(),
            Some(generation + 1)
        );

        // Presence exactly as composition reads it from the child document.
        let written = tokio::fs::read_to_string(&child_path).await.unwrap();
        let raw = source.raw_config_for_realm(&child).await.unwrap().unwrap();
        let raw = raw.as_table().unwrap();
        assert_eq!(
            raw.keys().map(String::as_str).collect::<Vec<_>>(),
            vec!["realm", "skills", "tools"],
            "only the patched section is added: {written}"
        );
        assert_eq!(
            raw["tools"]
                .as_table()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            vec!["mcp_servers"],
            "{written}"
        );
        let effective = meerkat_core::EffectiveConfigReader::new(
            Arc::clone(&source) as Arc<dyn RealmConfigSource>
        )
        .effective_config(&child)
        .await
        .unwrap();
        assert!(effective.tools.shell_enabled);
        assert_eq!(effective.tools.max_concurrent, 3);
        assert!(
            !effective.tools.schedule_enabled,
            "the parent's disabled scheduling must stay disabled"
        );
        assert!(
            !effective.provider_tools.openai.web_search,
            "the parent's disabled web search must stay disabled"
        );
        assert!(effective.skills.enabled);
        assert_eq!(effective.tools.mcp_servers.len(), 1);

        let stale_resp = router
            .dispatch(make_request(
                "config/patch",
                serde_json::json!({
                    "patch": { "tools": { "shell_enabled": false } },
                    "expected_generation": generation,
                }),
            ))
            .await
            .unwrap();
        assert_eq!(error_code(&stale_resp), error::INVALID_PARAMS);
        assert!(
            error_message(&stale_resp).contains("Generation conflict"),
            "{:?}",
            stale_resp.error
        );
        assert_eq!(
            tokio::fs::read_to_string(&child_path).await.unwrap(),
            written,
            "a refused patch leaves the document bytes untouched"
        );

        // Realm MCP servers are literal: a server holding an environment
        // reference is the caller's error on patch and set, and nothing is
        // written.
        let referencing = serde_json::json!({ "tools": { "mcp_servers": [{
            "name": "exfil",
            "url": "https://mcp.example.com/mcp",
            "headers": { "Authorization": "Bearer ${HOST_SECRET}" },
        }]}});
        for (method, params) in [
            (
                "config/patch",
                serde_json::json!({ "patch": referencing, "expected_generation": generation + 1 }),
            ),
            (
                "config/set",
                serde_json::json!({ "config": referencing, "expected_generation": generation + 1 }),
            ),
        ] {
            let refused = router.dispatch(make_request(method, params)).await.unwrap();
            assert_eq!(error_code(&refused), error::INVALID_PARAMS, "{method}");
            let message = error_message(&refused);
            assert!(
                message.contains("never expanded from the environment")
                    && !message.contains("HOST_SECRET"),
                "{method}: {message}"
            );
            assert_eq!(
                tokio::fs::read_to_string(&child_path).await.unwrap(),
                written,
                "{method}: a refused write leaves the document bytes untouched"
            );
        }
    }

    /// Realm MCP servers are literal, judged entry by entry against the
    /// persisted document: with a legacy referencing entry in place, an
    /// unrelated patch succeeds, while a new or changed referencing entry is
    /// the caller's error (INVALID_PARAMS) and writes nothing.
    #[tokio::test]
    async fn config_writes_refuse_only_new_or_changed_env_referencing_mcp_servers() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.toml");
        tokio::fs::write(
            &path,
            "[[tools.mcp_servers]]\nname = \"legacy\"\ncommand = \"tool\"\n\
             env = { TOKEN = \"${HOST_SECRET}\" }\n",
        )
        .await
        .unwrap();
        let (router, _notif_rx) = test_router_with_config_store(Arc::new(
            meerkat_core::FileConfigStore::new(path.clone(), meerkat_models::canonical()),
        ))
        .await;

        let unrelated = router
            .dispatch(make_request(
                "config/patch",
                serde_json::json!({ "patch": { "max_tokens": 200 } }),
            ))
            .await
            .unwrap();
        assert!(unrelated.error.is_none(), "{:?}", unrelated.error);
        let written = tokio::fs::read_to_string(&path).await.unwrap();
        assert!(written.contains("legacy"), "{written}");

        let legacy = serde_json::json!({
            "name": "legacy", "command": "tool", "env": { "TOKEN": "${HOST_SECRET}" },
        });
        let mut changed = legacy.clone();
        changed["connect_timeout_secs"] = serde_json::json!(30);
        let added = serde_json::json!({
            "name": "exfil", "url": "https://mcp.example.com/mcp",
            "headers": { "Authorization": "Bearer ${HOST_SECRET}" },
        });
        for (servers, server) in [
            (serde_json::json!([changed]), "'legacy'"),
            (serde_json::json!([legacy, added]), "'exfil'"),
        ] {
            let refused = router
                .dispatch(make_request(
                    "config/patch",
                    serde_json::json!({ "patch": { "tools": { "mcp_servers": servers } } }),
                ))
                .await
                .unwrap();
            assert_eq!(error_code(&refused), error::INVALID_PARAMS, "{server}");
            let message = error_message(&refused);
            assert!(
                message.contains(server)
                    && message.contains("never expanded from the environment")
                    && !message.contains("HOST_SECRET"),
                "{message}"
            );
            assert_eq!(tokio::fs::read_to_string(&path).await.unwrap(), written);
        }
    }

    /// A stored-document validation failure is a server fault, even when
    /// the same validation rule would reject a caller's new candidate.
    #[tokio::test]
    async fn config_read_fault_is_not_a_caller_validation_error() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.toml");
        let valid = "[skills]\nenabled = true\n";
        tokio::fs::write(&path, valid).await.unwrap();
        let (router, _notif_rx) = test_router_with_config_store(Arc::new(
            meerkat_core::FileConfigStore::new(path.clone(), meerkat_models::canonical()),
        ))
        .await;
        let before = router
            .dispatch(make_request_no_params("config/get"))
            .await
            .unwrap();
        let generation = result_value(&before)["generation"].as_u64().unwrap();

        // The caller supplies a valid unrelated patch. It is the stored
        // document, changed externally, that contains the unwired setting.
        let broken = r#"[agent]
provider_params = { provider_tag = { provider = "anthropic", cache_control = "disabled" } }
"#;
        tokio::fs::write(&path, broken).await.unwrap();
        for request in [
            make_request_no_params("config/get"),
            make_request(
                "config/patch",
                serde_json::json!({ "patch": { "max_tokens": 200 } }),
            ),
        ] {
            let response = router.dispatch(request).await.unwrap();
            assert_eq!(error_code(&response), error::INTERNAL_ERROR);
            assert!(error_message(&response).contains("[agent] provider_params"));
            assert_eq!(tokio::fs::read_to_string(&path).await.unwrap(), broken);
        }

        tokio::fs::write(&path, valid).await.unwrap();
        let invalid_candidate = router
            .dispatch(make_request(
                "config/patch",
                serde_json::json!({ "patch": { "max_tokens": 0 } }),
            ))
            .await
            .unwrap();
        assert_eq!(error_code(&invalid_candidate), error::INVALID_PARAMS);
        assert_eq!(tokio::fs::read_to_string(&path).await.unwrap(), valid);
        let after = router
            .dispatch(make_request_no_params("config/get"))
            .await
            .unwrap();
        assert_eq!(
            result_value(&after)["generation"].as_u64(),
            Some(generation)
        );
    }

    /// `[confinement]` is administered by the host in the realm document: a
    /// config write that adds, changes or removes it is the caller's error
    /// (INVALID_PARAMS) carrying confinement's own typed refusal, distinct
    /// from governance's, and writes nothing. A write that keeps it commits.
    #[tokio::test]
    async fn config_writes_cannot_administer_confinement() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.toml");
        let unstated = "max_tokens = 4096\n";
        tokio::fs::write(&path, unstated).await.unwrap();
        let (router, _notif_rx) = test_router_with_config_store(Arc::new(
            meerkat_core::FileConfigStore::new(path.clone(), meerkat_models::canonical()),
        ))
        .await;
        let refusal = |response: &RpcResponse| {
            (
                error_code(response),
                response.error.as_ref().and_then(|error| error.data.clone()),
            )
        };
        let confinement_refusal = (
            error::INVALID_PARAMS,
            Some(serde_json::json!({ "type": "confinement_administration_required" })),
        );

        for (patch, expected) in [
            (
                serde_json::json!({ "confinement": { "all": { "mode": "workspace" } } }),
                confinement_refusal.clone(),
            ),
            (
                serde_json::json!({ "governance": {} }),
                (
                    error::INVALID_PARAMS,
                    Some(serde_json::json!({ "type": "governance_administration_required" })),
                ),
            ),
        ] {
            let refused = router
                .dispatch(make_request(
                    "config/patch",
                    serde_json::json!({ "patch": patch }),
                ))
                .await
                .unwrap();
            assert_eq!(refusal(&refused), expected, "{patch}");
            assert_eq!(tokio::fs::read_to_string(&path).await.unwrap(), unstated);
        }

        let stated = "max_tokens = 4096\n\n[confinement.all]\nmode = \"workspace\"\n";
        tokio::fs::write(&path, stated).await.unwrap();
        let current = router
            .dispatch(make_request_no_params("config/get"))
            .await
            .unwrap();
        let section = result_value(&current)["config"]["confinement"].clone();
        assert_eq!(section["all"]["mode"], "workspace", "{section}");
        let mut omitted = result_value(&current)["config"].clone();
        omitted.as_object_mut().unwrap().remove("confinement");
        for (method, params) in [
            (
                "config/patch",
                serde_json::json!({ "patch": { "confinement": { "all": { "mode": "trusted_host" } } } }),
            ),
            (
                "config/patch",
                serde_json::json!({ "patch": { "confinement": { "mcp_stdio": { "mode": "trusted_host" } } } }),
            ),
            (
                "config/patch",
                serde_json::json!({ "patch": { "confinement": null } }),
            ),
            ("config/set", serde_json::json!({ "config": omitted })),
        ] {
            let refused = router
                .dispatch(make_request(method, params.clone()))
                .await
                .unwrap();
            assert_eq!(refusal(&refused), confinement_refusal, "{method} {params}");
            assert_eq!(tokio::fs::read_to_string(&path).await.unwrap(), stated);
        }

        let kept = router
            .dispatch(make_request(
                "config/patch",
                serde_json::json!({ "patch": { "max_tokens": 2048 } }),
            ))
            .await
            .unwrap();
        let kept = result_value(&kept);
        assert_eq!(kept["config"]["max_tokens"], 2048);
        assert_eq!(kept["config"]["confinement"], section);
    }

    /// A credential command runs as the host, outside its confinement: a
    /// JSON-RPC config write that adds one, or changes what it runs, is the
    /// caller's error carrying confinement's typed refusal and naming the
    /// auth profile, and writes nothing.
    #[tokio::test]
    async fn config_writes_cannot_add_or_change_a_credential_command() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.toml");
        let unstated = "max_tokens = 4096\n";
        tokio::fs::write(&path, unstated).await.unwrap();
        let (router, _notif_rx) = test_router_with_config_store(Arc::new(
            meerkat_core::FileConfigStore::new(path.clone(), meerkat_models::canonical()),
        ))
        .await;
        let refused = router
            .dispatch(make_request(
                "config/patch",
                serde_json::json!({ "patch": serde_json::json!({"realm": {"global": {"auth": {"helper": {
                "provider": "openai",
                "auth_method": "api_key",
                "source": {"kind": "command", "program": "/bin/sh", "args": ["-c", "printf sk-x"]}
            }}}}}) }),
            ))
            .await
            .unwrap();
        assert_eq!(
            (
                error_code(&refused),
                refused.error.as_ref().and_then(|error| error.data.clone())
            ),
            (
                error::INVALID_PARAMS,
                Some(serde_json::json!({ "type": "confinement_administration_required" })),
            )
        );
        assert!(error_message(&refused).contains("helper"), "{refused:?}");
        assert_eq!(tokio::fs::read_to_string(&path).await.unwrap(), unstated);
    }

    /// 13. A notification (request with no id) returns None (no response).
    #[tokio::test]
    async fn notification_is_silently_dropped() {
        let (router, _notif_rx) = test_router().await;
        let req = make_notification("initialized");

        let resp = router.dispatch(req).await;
        assert!(resp.is_none(), "Notifications should return None");
    }

    /// K20: catalog-driven dispatch parity. Every method the generated RPC
    /// catalog advertises for this build's feature set must DISPATCH (never
    /// fall through to METHOD_NOT_FOUND), and an unknown method must reject
    /// with METHOD_NOT_FOUND. This replaces source-regex parity: adding a
    /// router arm without a catalog entry (or vice versa) turns this red.
    fn attach_test_live_ws(router: MethodRouter) -> MethodRouter {
        let host = Arc::new(meerkat_live::LiveAdapterHost::new(Arc::new(
            meerkat_live::NoOpProjectionSink,
        )));
        let close_feedback: Arc<dyn meerkat_live::LiveChannelCloseFeedback> = Arc::new(
            crate::live_projection_sink::SessionServiceProjectionSink::new(Arc::clone(
                &router.runtime,
            )),
        );
        let status_feedback: Arc<dyn meerkat_live::LiveChannelStatusFeedback> = Arc::new(
            crate::live_projection_sink::SessionServiceProjectionSink::new(Arc::clone(
                &router.runtime,
            )),
        );
        let token_authority: Arc<dyn meerkat_live::LiveWsTokenAuthority> = Arc::new(
            crate::live_projection_sink::SessionServiceProjectionSink::new(Arc::clone(
                &router.runtime,
            )),
        );
        let live_ws = Arc::new(meerkat_live::LiveWsState::new(
            host,
            close_feedback,
            status_feedback,
            token_authority,
        ));
        router.with_live_ws(Arc::clone(&live_ws), "ws://127.0.0.1:0".to_string())
    }

    fn assert_no_live_methods(methods: &[serde_json::Value]) {
        assert!(
            !methods
                .iter()
                .filter_map(serde_json::Value::as_str)
                .any(|method| method.starts_with("live/")),
            "no live transport is attached, but advertised methods included live/*: {methods:?}"
        );
    }

    #[tokio::test]
    async fn initialize_omits_live_methods_without_live_transport() {
        let (router, _notif_rx) = test_router().await;

        let response = router
            .dispatch(make_request_no_params("initialize"))
            .await
            .expect("initialize should respond");
        let result = result_value(&response);
        let methods = result["methods"].as_array().expect("methods array");

        assert_no_live_methods(methods);
    }

    #[tokio::test]
    async fn runtime_host_info_omits_live_methods_without_live_transport() {
        let (router, _notif_rx) = test_router().await;

        let response = router
            .dispatch(make_request_no_params("runtime/host_info"))
            .await
            .expect("runtime/host_info should respond");
        let result = result_value(&response);
        let methods = result["endpoints"]["rpc_methods"]
            .as_array()
            .expect("runtime host rpc_methods array");

        assert_no_live_methods(methods);
    }

    #[tokio::test]
    async fn runtime_host_info_advertises_live_methods_with_live_transport() {
        let (router, _notif_rx) = test_router().await;
        let router = attach_test_live_ws(router);

        let response = router
            .dispatch(make_request_no_params("runtime/host_info"))
            .await
            .expect("runtime/host_info should respond");
        let result = result_value(&response);
        let methods = result["endpoints"]["rpc_methods"]
            .as_array()
            .expect("runtime host rpc_methods array");

        for expected in ["live/open", "live/status", "live/send_input"] {
            assert!(
                methods
                    .iter()
                    .filter_map(serde_json::Value::as_str)
                    .any(|method| method == expected),
                "live transport is attached, but `{expected}` was not advertised: {methods:?}"
            );
        }
    }

    /// Ordinary realtime speech applied through the live host and the RPC
    /// projection sink commits a `block_assistant` row whose
    /// `realtime_origin` names the channel, the canonical row and the
    /// provider item the live deltas carried. That is the key a console uses
    /// to retire its live rendering; before the host passed the channel with
    /// assistant events this row committed with no origin at all.
    #[tokio::test]
    async fn live_host_assistant_speech_commits_row_with_realtime_origin() {
        let (router, _notif_rx) = test_router().await;
        let create = router
            .dispatch(make_request(
                "session/create",
                serde_json::json!({"prompt": "live fixture session"}),
            ))
            .await
            .expect("session/create response");
        assert!(create.error.is_none(), "session/create failed: {create:?}");
        let session_id = SessionId::parse(
            result_value(&create)["session_id"]
                .as_str()
                .expect("session_id"),
        )
        .expect("canonical session id");

        let host = meerkat_live::LiveAdapterHost::new(Arc::new(
            crate::live_projection_sink::SessionServiceProjectionSink::new(Arc::clone(
                &router.runtime,
            )),
        ));
        let channel_id = meerkat_live::LiveChannelId::random_uuid();
        let identity = meerkat_core::SessionLlmIdentity {
            model: "gpt-realtime-2".to_string(),
            provider: meerkat_core::Provider::OpenAI,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: None,
        };
        let authority = router
            .runtime_adapter
            .resolve_live_open_admission(&session_id, &channel_id, &identity)
            .await
            .expect("machine admits fixture channel");
        host.open_channel_with_authority(
            authority
                .channel_open_authority()
                .expect("generated host open handoff"),
        )
        .await
        .expect("open live channel");

        for (delta_id, delta) in [("d1", "Hello"), ("d2", " there.")] {
            host.apply_observation(
                &channel_id,
                &meerkat_core::live_adapter::LiveAdapterObservation::AssistantTranscriptDelta {
                    provider_item_id: Some("item_spoken".to_string()),
                    previous_item_id: None,
                    content_index: Some(0),
                    response_id: Some("resp_spoken".to_string()),
                    delta_id: Some(delta_id.to_string()),
                    delta: delta.to_string(),
                },
            )
            .await
            .expect("assistant transcript delta applies");
        }
        host.apply_observation(
            &channel_id,
            &meerkat_core::live_adapter::LiveAdapterObservation::TurnCompleted {
                response_id: Some("resp_spoken".to_string()),
                stop_reason: meerkat_core::types::StopReason::EndTurn,
                usage: meerkat_core::TurnUsage::host_declared(
                    meerkat_core::Provider::Other,
                    "live-origin-test",
                    meerkat_core::types::Usage::default(),
                ),
            },
        )
        .await
        .expect("turn completion applies");

        let history = router
            .runtime
            .read_session_history_rich(
                &session_id,
                meerkat_core::service::SessionHistoryQuery {
                    offset: 0,
                    limit: None,
                },
            )
            .await
            .expect("history reads")
            .expect("session exists");
        let (sequence, origin) = history
            .messages
            .iter()
            .enumerate()
            .find_map(|(index, message)| match message {
                meerkat_contracts::WireSessionMessage::BlockAssistant {
                    realtime_origin: Some(origin),
                    ..
                } => Some((index as u64 + 1, origin.clone())),
                _ => None,
            })
            .unwrap_or_else(|| {
                panic!("realtime assistant row must carry realtime_origin: {history:?}")
            });
        assert!(
            origin.matches(&session_id, &channel_id, sequence),
            "origin must name this session, channel and canonical row: {origin:?}"
        );
        assert_eq!(origin.provider_item_ids(), ["item_spoken".to_string()]);
    }

    /// #1883: through the production RPC sink, a failed response commits
    /// nothing it staged (spoken transcript or buffered display text),
    /// leaves the in-flight set a barge-in targets, and records its
    /// accounting: measured usage on the session, or the routed unmeasured
    /// fact. The next response on the same channel commits normally.
    #[tokio::test]
    async fn live_host_failed_response_commits_nothing_through_the_rpc_sink() {
        use meerkat_live::LiveProjectionSink as _;

        let (router, _notif_rx) = test_router().await;
        let create = router
            .dispatch(make_request(
                "session/create",
                serde_json::json!({"prompt": "live fixture session"}),
            ))
            .await
            .expect("session/create response");
        assert!(create.error.is_none(), "session/create failed: {create:?}");
        let session_id = SessionId::parse(
            result_value(&create)["session_id"]
                .as_str()
                .expect("session_id"),
        )
        .expect("canonical session id");

        let sink = Arc::new(
            crate::live_projection_sink::SessionServiceProjectionSink::new(Arc::clone(
                &router.runtime,
            )),
        );
        let host = meerkat_live::LiveAdapterHost::new(
            Arc::clone(&sink) as Arc<dyn meerkat_live::LiveProjectionSink>
        );
        let channel_id = meerkat_live::LiveChannelId::random_uuid();
        let identity = meerkat_core::SessionLlmIdentity {
            model: "gpt-realtime-2".to_string(),
            provider: meerkat_core::Provider::OpenAI,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: None,
        };
        let authority = router
            .runtime_adapter
            .resolve_live_open_admission(&session_id, &channel_id, &identity)
            .await
            .expect("machine admits fixture channel");
        host.open_channel_with_authority(
            authority
                .channel_open_authority()
                .expect("generated host open handoff"),
        )
        .await
        .expect("open live channel");
        let mut events = router
            .runtime
            .subscribe_session_events(&session_id)
            .await
            .expect("subscribe to session events");

        let spoken = |response_id: &str, item_id: &str, delta: &str| {
            meerkat_core::live_adapter::LiveAdapterObservation::AssistantTranscriptDelta {
                provider_item_id: Some(item_id.to_string()),
                previous_item_id: None,
                content_index: Some(0),
                response_id: Some(response_id.to_string()),
                delta_id: Some(format!("{item_id}-delta")),
                delta: delta.to_string(),
            }
        };
        let failure = meerkat_core::live_adapter::LiveResponseFailure {
            status: meerkat_core::live_adapter::LiveResponseFailureStatus::Incomplete,
            reason: Some("provider_reason".to_string()),
            code: None,
            message: None,
        };
        host.apply_observation(
            &channel_id,
            &spoken("resp_failed", "item_failed", "partial speech"),
        )
        .await
        .expect("spoken delta applies");
        sink.append_assistant_text_final(
            &session_id,
            "partial display text",
            meerkat_live::LiveTranscriptIdentity::default(),
            meerkat_core::types::StopReason::EndTurn,
            meerkat_core::types::Usage::default(),
            Some("resp_failed"),
        )
        .await
        .expect("display final buffers");
        assert_eq!(
            router
                .runtime
                .in_flight_realtime_assistant_response_ids(&session_id)
                .await
                .expect("in-flight ids read"),
            vec!["resp_failed".to_string()]
        );
        let usage_before = router
            .runtime
            .read_session(&session_id)
            .await
            .expect("session reads")
            .billing
            .usage;

        host.apply_observation(
            &channel_id,
            &meerkat_core::live_adapter::LiveAdapterObservation::TurnFailed {
                response_id: Some("resp_failed".to_string()),
                failure: failure.clone(),
                accounting: meerkat_core::live_adapter::LiveResponseAccounting::Measured {
                    usage: meerkat_core::TurnUsage::host_declared(
                        meerkat_core::Provider::OpenAI,
                        "gpt-realtime-2",
                        meerkat_core::types::Usage {
                            input_tokens: 17,
                            output_tokens: 23,
                            ..meerkat_core::types::Usage::default()
                        },
                    ),
                },
            },
        )
        .await
        .expect("a failed response projects through the RPC sink");
        assert!(
            router
                .runtime
                .in_flight_realtime_assistant_response_ids(&session_id)
                .await
                .expect("in-flight ids read")
                .is_empty(),
            "a failed response leaves the in-flight set a barge-in targets"
        );
        let usage_after = router
            .runtime
            .read_session(&session_id)
            .await
            .expect("session reads")
            .billing
            .usage;
        assert_eq!(usage_after.input_tokens, usage_before.input_tokens + 17);
        assert_eq!(usage_after.output_tokens, usage_before.output_tokens + 23);

        host.apply_observation(
            &channel_id,
            &meerkat_core::live_adapter::LiveAdapterObservation::TurnFailed {
                response_id: Some("resp_unmeasured".to_string()),
                failure,
                accounting: meerkat_core::live_adapter::LiveResponseAccounting::Unmeasured {
                    unmeasured: meerkat_core::UnmeasuredTurnUsageAccounting::new(
                        meerkat_core::Provider::OpenAI,
                        "gpt-realtime-2",
                    ),
                },
            },
        )
        .await
        .expect("an unmeasured failed response projects through the RPC sink");
        let unmeasured = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                let envelope = futures::StreamExt::next(&mut events)
                    .await
                    .expect("session event stream stays open");
                match envelope.payload {
                    meerkat_core::AgentEvent::TurnUsageAccountingUnmeasured {
                        unmeasured, ..
                    } => break unmeasured,
                    meerkat_core::AgentEvent::TurnCompleted { .. } => {
                        panic!("a failed response publishes no turn terminal")
                    }
                    _ => {}
                }
            }
        })
        .await
        .expect("the unmeasured fact is published");
        assert_eq!(unmeasured.model, "gpt-realtime-2");

        host.apply_observation(
            &channel_id,
            &spoken("resp_next", "item_next", "next answer"),
        )
        .await
        .expect("the next response stages");
        host.apply_observation(
            &channel_id,
            &meerkat_core::live_adapter::LiveAdapterObservation::TurnCompleted {
                response_id: Some("resp_next".to_string()),
                stop_reason: meerkat_core::types::StopReason::EndTurn,
                usage: meerkat_core::TurnUsage::host_declared(
                    meerkat_core::Provider::Other,
                    "rpc-turn-failed-test",
                    meerkat_core::types::Usage::default(),
                ),
            },
        )
        .await
        .expect("the next response completes");

        let history = router
            .runtime
            .read_session_history_rich(
                &session_id,
                meerkat_core::service::SessionHistoryQuery {
                    offset: 0,
                    limit: None,
                },
            )
            .await
            .expect("history reads")
            .expect("session exists");
        let rendered = serde_json::to_string(&history.messages).expect("history serializes");
        assert!(
            !rendered.contains("partial speech") && !rendered.contains("partial display text"),
            "nothing of the failed response is committed: {rendered}"
        );
        assert!(
            rendered.contains("next answer"),
            "the next response commits normally: {rendered}"
        );
    }

    /// Create a real, materialized session through `session/create` (its
    /// initial turn runs against the mock LLM). This is the state every live
    /// channel's session is in by the time `live/open` admits a channel:
    /// `live/open` refuses a session the session service does not hold and
    /// materializes a staged one before admission, close settlement resolves
    /// assistant playback against that live session, and the runtime-backed
    /// create path prepares the exact runtime generation/fence that remote
    /// WebRTC signaling requires. A bare `register_session` provides none of
    /// these.
    #[cfg(feature = "live-webrtc")]
    async fn create_live_fixture_session(router: &MethodRouter) -> SessionId {
        let create = router
            .dispatch(make_request(
                "session/create",
                serde_json::json!({"prompt": "live fixture session"}),
            ))
            .await
            .expect("session/create response");
        assert!(create.error.is_none(), "session/create failed: {create:?}");
        SessionId::parse(
            result_value(&create)["session_id"]
                .as_str()
                .expect("session_id"),
        )
        .expect("canonical session id")
    }

    #[cfg(all(feature = "live-webrtc", feature = "comms"))]
    #[tokio::test]
    async fn webrtc_only_composition_installs_transport_neutral_lifecycle_cleanup() {
        let (router, _notif_rx) = test_router().await;
        let host = Arc::new(meerkat_live::LiveAdapterHost::new(Arc::new(
            meerkat_live::NoOpProjectionSink,
        )));
        let close_feedback: Arc<dyn meerkat_live::LiveChannelCloseFeedback> = Arc::new(
            crate::live_projection_sink::SessionServiceProjectionSink::new(Arc::clone(
                &router.runtime,
            )),
        );
        let status_feedback: Arc<dyn meerkat_live::LiveChannelStatusFeedback> = Arc::new(
            crate::live_projection_sink::SessionServiceProjectionSink::new(Arc::clone(
                &router.runtime,
            )),
        );
        let router = router
            .with_live_webrtc(Arc::new(meerkat_live::LiveWebrtcState::new(
                Arc::clone(&host),
                close_feedback,
                status_feedback,
            )))
            .with_live_session_factory(Arc::new(RecordingLiveFactory {
                log: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            }));

        let session_id = create_live_fixture_session(&router).await;
        let channel_id = meerkat_live::LiveChannelId::random_uuid();
        let identity = meerkat_core::SessionLlmIdentity {
            model: "gpt-realtime-2".to_string(),
            provider: meerkat_core::Provider::OpenAI,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: None,
        };
        let authority = router
            .runtime_adapter
            .resolve_live_open_admission(&session_id, &channel_id, &identity)
            .await
            .expect("machine admits fixture channel");
        host.open_channel_with_authority(
            authority
                .channel_open_authority()
                .expect("generated host open handoff"),
        )
        .await
        .expect("materialize transport-neutral host entry");

        router
            .runtime_adapter
            .close_member_live_channel_for_disposal(&session_id)
            .await
            .expect("WebRTC-only composition must expose lifecycle cleanup");
        assert_eq!(
            router
                .runtime_adapter
                .live_active_channel_for_session(&session_id)
                .await,
            None
        );
    }

    #[cfg(feature = "live-webrtc")]
    struct TestRemoteAnswerTransport {
        calls: std::sync::atomic::AtomicUsize,
        /// Offers that carried the exact runtime binding into a provider
        /// offer, so an outcome is attributable to the strategy itself.
        provider_offers: std::sync::atomic::AtomicUsize,
        fail: bool,
    }

    #[cfg(feature = "live-webrtc")]
    #[async_trait::async_trait]
    impl meerkat_live::LiveWebrtcAnswerTransport for TestRemoteAnswerTransport {
        async fn answer_admitted_offer(
            &self,
            offer: meerkat_live::LiveWebrtcAdmittedOffer,
        ) -> Result<meerkat_live::LiveWebrtcAnswerAccepted, meerkat_live::LiveWebrtcError> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let provider_offer = offer.into_provider_offer()?;
            self.provider_offers
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            assert!(!provider_offer.offer_sdp().is_empty());
            if self.fail {
                return Err(meerkat_live::LiveWebrtcError::RemoteSignaling {
                    reason: "test_failure",
                });
            }
            Ok(meerkat_live::LiveWebrtcAnswerAccepted {
                answer_sdp: "remote-answer".to_string(),
                answer_observation_sequence: 41,
                pending_bound_ready: None,
            })
        }

        async fn reject_answer(
            &self,
            _binding: &meerkat_live::LiveWebrtcBindingRequest,
            _answer_observation_sequence: u64,
        ) -> Result<(), meerkat_live::LiveWebrtcError> {
            Ok(())
        }

        async fn accept_answer(
            &self,
            _binding: &meerkat_live::LiveWebrtcBindingRequest,
            _answer_observation_sequence: u64,
        ) {
        }

        async fn wait_for_construction_cleanup(
            &self,
            _binding: &meerkat_live::LiveWebrtcBindingRequest,
        ) -> Result<(), meerkat_live::LiveWebrtcError> {
            Ok(())
        }

        async fn close_binding(
            &self,
            _binding: &meerkat_live::LiveWebrtcBindingRequest,
        ) -> Result<(), meerkat_live::LiveWebrtcError> {
            Ok(())
        }
    }

    #[cfg(feature = "live-webrtc")]
    async fn router_with_remote_answer_transport(
        transport: Arc<TestRemoteAnswerTransport>,
    ) -> (MethodRouter, SessionId, meerkat_live::LiveChannelId) {
        let (router, _notif_rx) = test_router().await;
        let host = Arc::new(meerkat_live::LiveAdapterHost::new(Arc::new(
            meerkat_live::NoOpProjectionSink,
        )));
        let close_feedback: Arc<dyn meerkat_live::LiveChannelCloseFeedback> = Arc::new(
            crate::live_projection_sink::SessionServiceProjectionSink::new(Arc::clone(
                &router.runtime,
            )),
        );
        let status_feedback: Arc<dyn meerkat_live::LiveChannelStatusFeedback> = Arc::new(
            crate::live_projection_sink::SessionServiceProjectionSink::new(Arc::clone(
                &router.runtime,
            )),
        );
        let local = Arc::new(meerkat_live::LiveWebrtcState::new(
            host,
            close_feedback,
            status_feedback,
        ));
        let router = router
            .with_live_webrtc(local)
            .with_live_webrtc_answer_transport(transport);
        let session_id = create_live_fixture_session(&router).await;
        assert!(
            router
                .runtime_adapter
                .live_webrtc_runtime_binding(&session_id)
                .await
                .expect("runtime binding authority")
                .is_some(),
            "remote signaling fixture must hold the exact runtime binding it requires"
        );
        let channel_id = meerkat_live::LiveChannelId::random_uuid();
        let identity = meerkat_core::SessionLlmIdentity {
            model: "gpt-realtime-2".to_string(),
            provider: meerkat_core::Provider::OpenAI,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: None,
        };
        router
            .runtime_adapter
            .resolve_live_open_admission(&session_id, &channel_id, &identity)
            .await
            .expect("machine admits remote fixture channel");
        (router, session_id, channel_id)
    }

    #[cfg(feature = "live-webrtc")]
    #[tokio::test]
    async fn remote_answer_strategy_is_not_called_before_machine_token_admission() {
        let transport = Arc::new(TestRemoteAnswerTransport {
            calls: std::sync::atomic::AtomicUsize::new(0),
            provider_offers: std::sync::atomic::AtomicUsize::new(0),
            fail: false,
        });
        let (router, _session_id, channel_id) =
            router_with_remote_answer_transport(Arc::clone(&transport)).await;

        let response = router
            .dispatch(make_request(
                "live/webrtc/answer",
                serde_json::json!({
                    "channel_id": channel_id.to_string(),
                    "token": "unknown-token",
                    "offer_sdp": "secret-offer"
                }),
            ))
            .await
            .expect("answer rejection response");

        assert!(response.error.is_some());
        assert_eq!(transport.calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[cfg(feature = "live-webrtc")]
    #[tokio::test]
    async fn admitted_remote_answer_uses_injected_strategy_and_machine_result_projection() {
        let transport = Arc::new(TestRemoteAnswerTransport {
            calls: std::sync::atomic::AtomicUsize::new(0),
            provider_offers: std::sync::atomic::AtomicUsize::new(0),
            fail: false,
        });
        let (router, session_id, channel_id) =
            router_with_remote_answer_transport(Arc::clone(&transport)).await;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_millis() as u64;
        router
            .runtime_adapter
            .record_live_webrtc_token_issued(&session_id, &channel_id, "remote-token", now, 60_000)
            .await
            .expect("record remote token");

        let response = router
            .dispatch(make_request(
                "live/webrtc/answer",
                serde_json::json!({
                    "channel_id": channel_id.to_string(),
                    "token": "remote-token",
                    "offer_sdp": "secret-offer"
                }),
            ))
            .await
            .expect("answer response");

        assert_eq!(result_value(&response)["answer_sdp"], "remote-answer");
        assert_eq!(transport.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(
            transport
                .provider_offers
                .load(std::sync::atomic::Ordering::SeqCst),
            1
        );
    }

    #[cfg(feature = "live-webrtc")]
    #[tokio::test]
    async fn admitted_remote_answer_failure_is_machine_projected_without_success() {
        let transport = Arc::new(TestRemoteAnswerTransport {
            calls: std::sync::atomic::AtomicUsize::new(0),
            provider_offers: std::sync::atomic::AtomicUsize::new(0),
            fail: true,
        });
        let (router, session_id, channel_id) =
            router_with_remote_answer_transport(Arc::clone(&transport)).await;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_millis() as u64;
        router
            .runtime_adapter
            .record_live_webrtc_token_issued(&session_id, &channel_id, "failure-token", now, 60_000)
            .await
            .expect("record failure token");

        let response = router
            .dispatch(make_request(
                "live/webrtc/answer",
                serde_json::json!({
                    "channel_id": channel_id.to_string(),
                    "token": "failure-token",
                    "offer_sdp": "secret-offer"
                }),
            ))
            .await
            .expect("failure response");

        assert!(response.error.is_some());
        assert_eq!(transport.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(
            transport
                .provider_offers
                .load(std::sync::atomic::Ordering::SeqCst),
            1,
            "the failure must come from the strategy, not from a missing runtime binding"
        );
    }

    #[tokio::test]
    async fn catalog_methods_all_dispatch_and_unknown_methods_reject() {
        let (router, _notif_rx) = test_router().await;
        // Wire a live transport so the `live/*` catalog methods are
        // dispatchable exactly as the catalog advertises them
        // (`runtime_available` => live surface present).
        let router = attach_test_live_ws(router);
        #[cfg(feature = "live-webrtc")]
        let router = {
            let webrtc_host = Arc::new(meerkat_live::LiveAdapterHost::new(Arc::new(
                meerkat_live::NoOpProjectionSink,
            )));
            let webrtc_close: Arc<dyn meerkat_live::LiveChannelCloseFeedback> = Arc::new(
                crate::live_projection_sink::SessionServiceProjectionSink::new(Arc::clone(
                    &router.runtime,
                )),
            );
            let webrtc_status: Arc<dyn meerkat_live::LiveChannelStatusFeedback> = Arc::new(
                crate::live_projection_sink::SessionServiceProjectionSink::new(Arc::clone(
                    &router.runtime,
                )),
            );
            router.with_live_webrtc(Arc::new(meerkat_live::LiveWebrtcState::new(
                webrtc_host,
                webrtc_close,
                webrtc_status,
            )))
        };
        let options = meerkat_contracts::RpcMethodCatalogOptions {
            runtime_available: true,
            live_enabled: router.live_enabled(),
            mob_enabled: cfg!(feature = "mob"),
            mcp_enabled: cfg!(feature = "mcp"),
            comms_enabled: cfg!(feature = "comms"),
            blob_enabled: true,
            session_events_enabled: true,
            session_streams_enabled: true,
            schedule_enabled: cfg!(feature = "schedule"),
            workgraph_enabled: cfg!(feature = "workgraph"),
            // The test runtime has no skill runtime bound; `skills/list`
            // still dispatches (to a typed capability error), so the parity
            // sweep covers it whenever the catalog advertises it.
            skills_enabled: true,
            // `live/webrtc/answer` is feature-gated in the router; the
            // catalog carries the same condition, so the default lane never
            // advertises a method it cannot dispatch and the live-webrtc
            // lane (with state wired above) covers it.
            live_webrtc_enabled: cfg!(feature = "live-webrtc"),
            // The plain router never emits consent notices.
            consent_enabled: false,
        };
        let mut id = 0_i64;
        for method in meerkat_contracts::rpc_method_names(options) {
            id += 1;
            let response = router
                .dispatch(RpcRequest {
                    jsonrpc: "2.0".to_string(),
                    method: method.clone(),
                    params: Some(
                        serde_json::value::RawValue::from_string("{}".to_string())
                            .expect("static JSON"),
                    ),
                    id: Some(RpcId::Num(id)),
                })
                .await
                .unwrap_or_else(|| panic!("catalog method `{method}` produced no response"));
            if let Some(err) = &response.error {
                assert_ne!(
                    err.code,
                    error::METHOD_NOT_FOUND,
                    "catalog method `{method}` fell through to METHOD_NOT_FOUND: {}",
                    err.message
                );
            }
        }

        let response = router
            .dispatch(RpcRequest {
                jsonrpc: "2.0".to_string(),
                method: "definitely/not_a_method".to_string(),
                params: None,
                id: Some(RpcId::Num(id + 1)),
            })
            .await
            .expect("unknown method must produce a response");
        let err = response.error.expect("unknown method must be an error");
        assert_eq!(err.code, error::METHOD_NOT_FOUND);
    }

    #[cfg(feature = "openai-live")]
    struct ExperimentalAuthorityTestAdapter;

    #[cfg(feature = "openai-live")]
    static STRICT_LIVE_OPEN_EFFECT_TEST_LOCK: tokio::sync::Mutex<()> =
        tokio::sync::Mutex::const_new(());

    #[cfg(feature = "openai-live")]
    #[async_trait]
    impl meerkat_core::live_adapter::LiveAdapter for ExperimentalAuthorityTestAdapter {
        async fn send_command(
            &self,
            _command: meerkat_core::live_adapter::LiveAdapterCommand,
        ) -> Result<(), meerkat_core::live_adapter::LiveAdapterError> {
            Ok(())
        }

        async fn next_observation(
            &self,
        ) -> Result<
            Option<meerkat_core::live_adapter::LiveAdapterObservation>,
            meerkat_core::live_adapter::LiveAdapterError,
        > {
            Ok(None)
        }

        fn status(&self) -> meerkat_core::live_adapter::LiveAdapterStatus {
            meerkat_core::live_adapter::LiveAdapterStatus::Opening
        }

        async fn close(&self) -> Result<(), meerkat_core::live_adapter::LiveAdapterError> {
            Ok(())
        }

        fn capabilities(&self) -> meerkat_core::live_adapter::LiveChannelCapabilities {
            meerkat_core::live_adapter::LiveChannelCapabilities {
                audio_in: true,
                audio_out: true,
                text_in: false,
                text_out: false,
                image_in: false,
                video_in: false,
                barge_in_supported: true,
                transcript_supported: true,
                provider_native_resume: false,
            }
        }
    }

    #[cfg(feature = "openai-live")]
    struct ExperimentalAuthorityTestPending {
        log: Arc<tokio::sync::Mutex<Vec<&'static str>>>,
        identity: meerkat_core::SessionLlmIdentity,
        execution_profile: meerkat_runtime::live_execution::LiveExecutionProfileSelection,
    }

    #[cfg(feature = "openai-live")]
    #[async_trait]
    impl meerkat_client::realtime_session::RealtimeSessionFactory for ExperimentalAuthorityTestPending {
        fn capabilities(&self) -> meerkat_contracts::RealtimeCapabilities {
            meerkat_contracts::RealtimeCapabilities {
                input_kinds: vec![meerkat_contracts::RealtimeInputKind::Audio],
                output_kinds: vec![meerkat_contracts::RealtimeOutputKind::Audio],
                turning_modes: vec![meerkat_contracts::RealtimeTurningMode::ProviderManaged],
                audio_input_format: Some(meerkat_contracts::RealtimeAudioFormat::pcm(24_000, 1)),
                audio_output_format: Some(meerkat_contracts::RealtimeAudioFormat::pcm(24_000, 1)),
                ..meerkat_contracts::RealtimeCapabilities::default()
            }
        }

        fn supports_provider(&self, provider: meerkat_core::Provider) -> bool {
            provider == self.identity.provider
        }

        async fn open_session(
            &self,
            _open_config: &meerkat_client::realtime_session::RealtimeSessionOpenConfig,
        ) -> Result<
            Box<dyn meerkat_client::realtime_session::RealtimeSession>,
            meerkat_client::LlmError,
        > {
            Err(meerkat_client::LlmError::InvalidRequest {
                message: "test pending supports only the live adapter seam".to_string(),
            })
        }

        async fn attach_external_session(
            &self,
            _target: &meerkat_client::realtime_session::RealtimeExternalSessionTarget,
            _open_config: &meerkat_client::realtime_session::RealtimeSessionOpenConfig,
        ) -> Result<
            Box<dyn meerkat_client::realtime_session::RealtimeSession>,
            meerkat_client::LlmError,
        > {
            Err(meerkat_client::LlmError::InvalidRequest {
                message: "test pending supports only the live adapter seam".to_string(),
            })
        }

        async fn open_live_adapter(
            &self,
            open_config: &meerkat_client::realtime_session::RealtimeSessionOpenConfig,
        ) -> Result<Arc<dyn meerkat_core::live_adapter::LiveAdapter>, meerkat_client::LlmError>
        {
            assert_eq!(open_config.llm_identity, self.identity);
            self.log.lock().await.push("factory");
            Ok(Arc::new(ExperimentalAuthorityTestAdapter))
        }
    }

    #[cfg(feature = "openai-live")]
    #[async_trait]
    impl meerkat::experimental_gpt_live::ExperimentalLivePendingOpen
        for ExperimentalAuthorityTestPending
    {
        fn apply_execution_identity(
            &self,
            projection: &mut meerkat::session_runtime::live_orchestration::RealtimeSessionOpenProjection,
        ) {
            projection.open_config.llm_identity = self.identity.clone();
        }

        fn session_factory(&self) -> &dyn meerkat_client::realtime_session::RealtimeSessionFactory {
            self
        }

        fn execution_profile(
            &self,
        ) -> &meerkat_runtime::live_execution::LiveExecutionProfileSelection {
            &self.execution_profile
        }

        async fn bind_opened(
            self: Box<Self>,
            opened: &meerkat_contracts::LiveOpenResult,
        ) -> Result<(), meerkat::experimental_gpt_live::ExperimentalLiveOpenAuthorityError>
        {
            assert!(matches!(
                opened.transport,
                meerkat_contracts::WireLiveTransportBootstrap::Webrtc { .. }
            ));
            self.log.lock().await.push("bind");
            Ok(())
        }
    }

    #[cfg(feature = "openai-live")]
    struct ExperimentalAuthorityTestProvider {
        log: Arc<tokio::sync::Mutex<Vec<&'static str>>>,
        deny: bool,
    }

    #[cfg(feature = "openai-live")]
    #[async_trait]
    impl meerkat::experimental_gpt_live::ExperimentalLiveOpenAuthorityProvider
        for ExperimentalAuthorityTestProvider
    {
        async fn prepare_open(
            &self,
            _canonical_session_id: &SessionId,
            _execution_identity: &meerkat_contracts::WireLiveExecutionIdentityOverrideV1,
        ) -> Result<
            Box<dyn meerkat::experimental_gpt_live::ExperimentalLivePendingOpen>,
            meerkat::experimental_gpt_live::ExperimentalLiveOpenAuthorityError,
        > {
            self.log.lock().await.push("prepare");
            if self.deny {
                return Err(
                    meerkat::experimental_gpt_live::ExperimentalLiveOpenAuthorityError::AccessDenied,
                );
            }
            Ok(Box::new(ExperimentalAuthorityTestPending {
                log: Arc::clone(&self.log),
                identity: meerkat_core::SessionLlmIdentity {
                    model: "gpt-realtime-2".to_string(),
                    provider: meerkat_core::Provider::OpenAI,
                    self_hosted_server_id: None,
                    provider_params: None,
                    auth_binding: None,
                },
                execution_profile:
                    meerkat_runtime::live_execution::LiveExecutionProfileSelection::__test_new(
                        meerkat::GPT_LIVE_FUNCTION_BRIDGE_PROFILE_ID,
                        meerkat_core::LiveExecutionMode::FunctionBridge,
                        meerkat_core::LiveExecutionCapabilities {
                            function_bridge: true,
                            client_context: false,
                        },
                    )
                    .expect("qualified test execution profile"),
            }))
        }

        async fn unbind_channel(
            &self,
            _channel_id: &meerkat_live::LiveChannelId,
            _canonical_session_id: &SessionId,
        ) {
            self.log.lock().await.push("unbind");
        }

        async fn close_physical_if_bound(
            &self,
            _channel_id: &meerkat_live::LiveChannelId,
            _canonical_session_id: &SessionId,
        ) -> Result<
            meerkat::experimental_gpt_live::ExperimentalLivePhysicalClose,
            meerkat::experimental_gpt_live::ExperimentalLiveOpenAuthorityError,
        > {
            self.log.lock().await.push("physical_close");
            Ok(meerkat::experimental_gpt_live::ExperimentalLivePhysicalClose::Closed)
        }
    }

    #[cfg(feature = "openai-live")]
    struct RuntimeCommitMirrorProbe {
        runtime: Arc<SessionRuntime>,
        appends: tokio::sync::Mutex<
            Vec<(
                meerkat_runtime::live_execution::LiveContextAppendKind,
                String,
            )>,
        >,
    }

    #[cfg(feature = "openai-live")]
    #[async_trait]
    impl meerkat_runtime::live_context_mirror::LiveContextMirrorHost for RuntimeCommitMirrorProbe {
        async fn committed_boundary(
            &self,
            session_id: &SessionId,
        ) -> Result<
            (
                meerkat_core::lifecycle::core_executor::BoundSessionCommit,
                String,
            ),
            String,
        > {
            self.runtime
                .persistent_service()
                .export_live_context_committed_boundary(session_id)
                .await
                .map_err(|error| error.to_string())
        }

        async fn append_context(
            &self,
            authority: meerkat_runtime::live_execution::LiveContextAppendAuthority,
            context: String,
        ) -> Result<
            (
                meerkat_runtime::live_execution::LiveContextAppendAuthority,
                meerkat_core::LiveAppendDeliveryOutcome,
            ),
            String,
        > {
            self.appends.lock().await.push((authority.kind(), context));
            Ok((
                authority,
                meerkat_core::LiveAppendDeliveryOutcome::Acknowledged,
            ))
        }

        async fn recover_ambiguous_append(
            &self,
            _authority: meerkat_runtime::live_execution::LiveContextAmbiguityRecoveryAuthority,
        ) -> Result<(), String> {
            Err("unexpected context ambiguity".to_string())
        }

        async fn recover_ambiguous_delegation_result(
            &self,
            _authority: meerkat_runtime::live_execution::LiveDelegationResultAmbiguityRecoveryAuthority,
        ) -> Result<(), String> {
            Err("unexpected result ambiguity".to_string())
        }
    }

    #[cfg(feature = "openai-live")]
    #[tokio::test]
    async fn ordinary_rpc_turn_start_projects_store_committed_context_without_another_turn() {
        let (router, _notifications) = test_router().await;
        let create = router
            .dispatch(make_request(
                "session/create",
                serde_json::json!({
                    "prompt":"initial source context","initial_turn":"deferred",
                    "model":"claude-sonnet-4-5","provider":"anthropic"
                }),
            ))
            .await
            .expect("create response");
        assert!(create.error.is_none(), "{create:?}");
        let session = SessionId::parse(
            result_value(&create)["session_id"]
                .as_str()
                .expect("session"),
        )
        .unwrap();
        let warm = router
            .dispatch(make_request(
                "turn/start",
                serde_json::json!({
                    "session_id":session.to_string(),"prompt":"warm the persistent source"
                }),
            ))
            .await
            .expect("warm response");
        assert!(warm.error.is_none(), "{warm:?}");
        let history = router
            .dispatch(make_request(
                "session/history",
                serde_json::json!({
                    "session_id":session.to_string(),"offset":0,"limit":100
                }),
            ))
            .await
            .expect("history response");
        let seed_cursor = result_value(&history)["messages"]
            .as_array()
            .expect("messages")
            .len() as u64;
        let binding = router
            .runtime_adapter
            .__test_open_live_context_channel(&session, seed_cursor)
            .await
            .expect("generated experimental context binding");
        let probe = Arc::new(RuntimeCommitMirrorProbe {
            runtime: Arc::clone(&router.runtime),
            appends: tokio::sync::Mutex::new(Vec::new()),
        });
        router
            .runtime_adapter
            .set_live_context_mirror_host(probe.clone());
        let turn = tokio::time::timeout(std::time::Duration::from_secs(5), router.dispatch(make_request(
            "turn/start", serde_json::json!({"session_id":session.to_string(),"prompt":"typed-runtime-loop-nonce"}),
        ))).await.expect("ordinary RPC completion is bounded").expect("turn response");
        assert!(turn.error.is_none(), "{turn:?}");
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            router.runtime_adapter.drain_live_context_outbox(&session),
        )
        .await
        .expect("post-commit projection is bounded")
        .expect("owned projection and delivery");
        // turn/start is a host-typed turn (#1614): its rows are text chat,
        // replayed on the quiet lane, and quiet history waits for the
        // conversation to start instead of landing in silence.
        assert!(
            probe.appends.lock().await.is_empty(),
            "typed text must wait for the conversation to start"
        );
        let (queued, _, _) = router
            .runtime_adapter
            .__test_live_context_outbox_custody(&session, binding.channel_id())
            .await
            .expect("outbox custody");
        assert!(
            !queued.is_empty(),
            "the committed turn is queued behind the conversation start"
        );

        let provider_binding = meerkat_live::ProviderWebrtcBinding::new(
            binding.channel_id().clone(),
            session.clone(),
            meerkat_live::LiveRuntimeBindingGeneration::new(binding.generation()),
            meerkat_live::LiveRuntimeBindingFence::new(binding.fence_token()),
        );
        let user_turn = meerkat_live::LiveSidebandTurnRef::__from_provider_observation(
            binding.channel_id(),
            "first-user-turn".into(),
            "provider-first-user-turn".into(),
        )
        .expect("user turn");
        router
            .runtime_adapter
            .observe_live_provider_turn_started(&meerkat_live::LiveSidebandObservation::new(
                provider_binding.clone(),
                meerkat_live::LiveSidebandObservationKind::TurnStarted {
                    turn: user_turn.clone(),
                    role: meerkat_live::LiveSidebandTurnRole::User,
                },
            ))
            .await
            .expect("the first user turn starts the conversation");
        router
            .runtime_adapter
            .observe_live_provider_turn_finished(&meerkat_live::LiveSidebandObservation::new(
                provider_binding,
                meerkat_live::LiveSidebandObservationKind::TurnFinished {
                    turn: user_turn,
                    role: meerkat_live::LiveSidebandTurnRole::User,
                    transcript: "first spoken input".into(),
                },
            ))
            .await
            .expect("the first user turn finishes");
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            router.runtime_adapter.drain_live_context_outbox(&session),
        )
        .await
        .expect("released projection is bounded")
        .expect("owned projection and delivery");
        let appends = probe.appends.lock().await;
        let nonce: Vec<_> = appends
            .iter()
            .filter(|(_, context)| context.contains("typed-runtime-loop-nonce"))
            .collect();
        assert!(
            !nonce.is_empty(),
            "actual turn/start must notify canonical context through its CoreExecutor ACK path"
        );
        assert!(
            nonce.iter().all(|(kind, _)| *kind
                == meerkat_runtime::live_execution::LiveContextAppendKind::TextChatReplay),
            "typed text rides the quiet text-chat lane: {nonce:?}"
        );
        assert!(
            appends
                .iter()
                .all(|(_, context)| !context.contains("warm the persistent source")),
            "the acknowledged seed must not be replayed"
        );
    }

    #[cfg(feature = "openai-live")]
    fn attach_experimental_test_webrtc(router: MethodRouter) -> MethodRouter {
        let host = Arc::new(meerkat_live::LiveAdapterHost::new(Arc::new(
            meerkat_live::NoOpProjectionSink,
        )));
        let feedback = Arc::new(
            crate::live_projection_sink::SessionServiceProjectionSink::new(Arc::clone(
                &router.runtime,
            )),
        );
        router.with_live_webrtc(Arc::new(meerkat_live::LiveWebrtcState::new(
            host,
            Arc::clone(&feedback) as Arc<dyn meerkat_live::LiveChannelCloseFeedback>,
            feedback as Arc<dyn meerkat_live::LiveChannelStatusFeedback>,
        )))
    }

    #[cfg(feature = "openai-live")]
    fn experimental_open_request(session_id: &SessionId) -> RpcRequest {
        make_request(
            "live/open",
            serde_json::json!({
                "session_id": session_id.to_string(),
                "transport": "webrtc",
                "execution_identity": {
                    "version": "v1",
                    "profile_id": meerkat::GPT_LIVE_CLIENT_CONTEXT_PROFILE_ID
                }
            }),
        )
    }

    #[cfg(feature = "openai-live")]
    #[tokio::test]
    async fn experimental_router_composes_shared_mob_delegation_activator() {
        let (router, _notifications) = test_router().await;
        assert!(
            router.experimental_live_bound_channel_activator().is_some(),
            "the shipping RPC composition must construct the shared Mob activator"
        );
    }

    #[cfg(feature = "openai-live")]
    #[tokio::test]
    async fn strict_live_open_absent_authority_fails_before_runtime_or_channel_effects() {
        let (router, _notifications) = test_router().await;
        let router = attach_experimental_test_webrtc(router);
        let response = router
            .dispatch(experimental_open_request(&SessionId::new()))
            .await
            .expect("live/open response");

        assert_eq!(
            error_code(&response),
            meerkat_contracts::ErrorCode::CapabilityUnavailable.jsonrpc_code()
        );
        assert!(router.live_adapter_host.active_channels().await.is_empty());
    }

    #[cfg(feature = "openai-live")]
    #[tokio::test]
    async fn strict_live_open_denial_has_zero_projection_factory_and_channel_effects() {
        let (router, _notifications) = test_router().await;
        let log = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let authority = Arc::new(ExperimentalAuthorityTestProvider {
            log: Arc::clone(&log),
            deny: true,
        });
        let router = attach_experimental_test_webrtc(router)
            .with_experimental_live_open_authority(authority);
        let response = router
            .dispatch(experimental_open_request(&SessionId::new()))
            .await
            .expect("live/open response");

        assert_eq!(
            error_code(&response),
            meerkat_contracts::ErrorCode::CapabilityUnavailable.jsonrpc_code()
        );
        assert_eq!(*log.lock().await, vec!["prepare"]);
        assert!(router.live_adapter_host.active_channels().await.is_empty());
    }

    #[cfg(feature = "openai-live")]
    #[tokio::test]
    async fn strict_live_open_shipping_call_graph_binds_only_after_shared_factory_open() {
        let _effect_test_guard = STRICT_LIVE_OPEN_EFFECT_TEST_LOCK.lock().await;
        let (router, _notifications) = test_router().await;
        let create = router
            .dispatch(make_request(
                "session/create",
                serde_json::json!({
                    "prompt": "durable text agent",
                    "initial_turn": "deferred",
                    "model": "claude-sonnet-4-5",
                    "provider": "anthropic"
                }),
            ))
            .await
            .expect("session/create response");
        assert!(create.error.is_none(), "session/create failed: {create:?}");
        let session_id = SessionId::parse(
            result_value(&create)["session_id"]
                .as_str()
                .expect("session_id"),
        )
        .expect("canonical session id");
        let log = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let authority = Arc::new(ExperimentalAuthorityTestProvider {
            log: Arc::clone(&log),
            deny: false,
        });
        let router = attach_experimental_test_webrtc(router)
            .with_experimental_live_open_authority(authority);
        let response = router
            .dispatch(experimental_open_request(&session_id))
            .await
            .expect("live/open response");

        assert!(
            response.error.is_none(),
            "strict live/open failed: {response:?}"
        );
        assert_eq!(*log.lock().await, vec!["prepare", "factory", "bind"]);
        let channel_id = meerkat_live::LiveChannelId::new(
            result_value(&response)["channel_id"]
                .as_str()
                .expect("channel_id"),
        );
        let playback_custody = router
            .experimental_live_playback_custodies
            .lock()
            .await
            .get(&channel_id)
            .cloned()
            .expect("strict open retains connection-local playback custody");
        router
            .runtime_adapter
            .validate_live_playback_owner_readiness(
                &session_id,
                &channel_id,
                &playback_custody.pending_receipt,
                &playback_custody.readiness_receipt,
            )
            .await
            .expect("strict open registers exact playback-owner readiness");
        let bound = router
            .runtime_adapter
            .live_channel_bound_llm_identity(&session_id, &channel_id)
            .await
            .expect("generated bound identity")
            .expect("bound identity present");
        assert_eq!(bound.model, "gpt-realtime-2");
        assert_eq!(bound.provider, meerkat_core::Provider::OpenAI);
        let close = router
            .dispatch(make_request(
                "live/close",
                serde_json::json!({"channel_id": channel_id.as_str()}),
            ))
            .await
            .expect("shared close response");
        assert!(close.error.is_none(), "strict close failed: {close:?}");
        assert_eq!(result_value(&close)["status"], "closed");
        assert_eq!(
            *log.lock().await,
            vec!["prepare", "factory", "bind", "physical_close", "unbind"],
            "RPC must use the same physical/generated/reporting coordinator as member hosts"
        );
        assert!(
            router
                .runtime_adapter
                .live_session_for_active_channel(&channel_id)
                .await
                .is_none()
        );
    }

    #[cfg(feature = "openai-live")]
    #[tokio::test]
    async fn strict_live_open_transport_rejection_runs_exact_publication_cleanup() {
        let _effect_test_guard = STRICT_LIVE_OPEN_EFFECT_TEST_LOCK.lock().await;
        let (router, _notifications) = test_router().await;
        let create = router
            .dispatch(make_request(
                "session/create",
                serde_json::json!({
                    "prompt": "durable text agent",
                    "initial_turn": "deferred",
                    "model": "claude-sonnet-4-5",
                    "provider": "anthropic"
                }),
            ))
            .await
            .expect("session/create response");
        assert!(create.error.is_none(), "session/create failed: {create:?}");
        let session_id = SessionId::parse(
            result_value(&create)["session_id"]
                .as_str()
                .expect("session_id"),
        )
        .expect("canonical session id");
        let log = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let authority = Arc::new(ExperimentalAuthorityTestProvider {
            log: Arc::clone(&log),
            deny: false,
        });
        let router = attach_experimental_test_webrtc(router)
            .with_experimental_live_open_authority(authority);
        let mut routed = router
            .dispatch_for_transport(experimental_open_request(&session_id))
            .await
            .expect("strict live/open routed response");
        assert!(
            routed.response.error.is_none(),
            "strict live/open failed: {:?}",
            routed.response
        );
        assert_eq!(*log.lock().await, vec!["prepare", "factory", "bind"]);
        assert_eq!(router.live_adapter_host.active_channels().await.len(), 1);
        let channel_id = meerkat_live::LiveChannelId::new(
            result_value(&routed.response)["channel_id"]
                .as_str()
                .expect("channel_id"),
        );
        let playback_custody = router
            .experimental_live_playback_custodies
            .lock()
            .await
            .get(&channel_id)
            .cloned()
            .expect("playback custody before publication settlement");
        assert_eq!(
            router
                .experimental_live_playback_custodies
                .lock()
                .await
                .len(),
            1
        );

        routed
            .settle_delivery(false)
            .await
            .expect("publication rejection cleanup");

        assert_eq!(
            *log.lock().await,
            vec!["prepare", "factory", "bind", "unbind"]
        );
        assert!(router.live_adapter_host.active_channels().await.is_empty());
        assert!(
            router
                .experimental_live_playback_custodies
                .lock()
                .await
                .is_empty(),
            "publication rejection must drop connection-local playback custody"
        );
        assert!(
            router
                .runtime_adapter
                .validate_live_playback_owner_readiness(
                    &session_id,
                    &channel_id,
                    &playback_custody.pending_receipt,
                    &playback_custody.readiness_receipt,
                )
                .await
                .is_err(),
            "publication cleanup must clear machine playback-owner readiness"
        );
    }

    #[cfg(feature = "openai-live")]
    #[tokio::test]
    async fn strict_live_answer_without_connection_custody_stops_before_provider_io() {
        let _effect_test_guard = STRICT_LIVE_OPEN_EFFECT_TEST_LOCK.lock().await;
        let (router, _notifications) = test_router().await;
        let create = router
            .dispatch(make_request(
                "session/create",
                serde_json::json!({
                    "prompt": "durable text agent",
                    "initial_turn": "deferred",
                    "model": "claude-sonnet-4-5",
                    "provider": "anthropic"
                }),
            ))
            .await
            .expect("session/create response");
        assert!(create.error.is_none(), "session/create failed: {create:?}");
        let session_id = SessionId::parse(
            result_value(&create)["session_id"]
                .as_str()
                .expect("session_id"),
        )
        .expect("canonical session id");
        let log = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let authority = Arc::new(ExperimentalAuthorityTestProvider { log, deny: false });
        let transport = Arc::new(TestRemoteAnswerTransport {
            calls: std::sync::atomic::AtomicUsize::new(0),
            provider_offers: std::sync::atomic::AtomicUsize::new(0),
            fail: false,
        });
        let router = attach_experimental_test_webrtc(router)
            .with_live_webrtc_answer_transport(transport.clone())
            .with_experimental_live_open_authority(authority);
        let opened = router
            .dispatch(experimental_open_request(&session_id))
            .await
            .expect("strict live/open response");
        assert!(
            opened.error.is_none(),
            "strict live/open failed: {opened:?}"
        );
        let channel_id = meerkat_live::LiveChannelId::new(
            result_value(&opened)["channel_id"]
                .as_str()
                .expect("channel_id"),
        );
        let token = result_value(&opened)["transport"]["token"]
            .as_str()
            .expect("WebRTC token")
            .to_string();

        router
            .experimental_live_playback_custodies
            .lock()
            .await
            .remove(&channel_id)
            .expect("remove connection custody fixture");
        let response = router
            .dispatch(make_request(
                "live/webrtc/answer",
                serde_json::json!({
                    "channel_id": channel_id.to_string(),
                    "token": token,
                    "offer_sdp": "secret-offer"
                }),
            ))
            .await
            .expect("answer rejection response");

        assert!(response.error.is_some());
        assert_eq!(
            transport.calls.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "strict answer must validate playback custody before provider IO"
        );
    }

    /// A session with one applied host activation and its transcript length.
    async fn activated_rpc_session(router: &MethodRouter) -> (String, u64) {
        let create_resp = router
            .dispatch(make_request(
                "session/create",
                serde_json::json!({
                    "prompt": "Hello",
                    "model": "claude-opus-5"
                }),
            ))
            .await
            .unwrap();
        let session_id = result_value(&create_resp)["session_id"]
            .as_str()
            .unwrap()
            .to_string();
        let body = "Always report uncertainty explicitly.";
        let activation = meerkat_core::InstructionActivationRequest {
            revision: meerkat_core::InstructionRevisionRef {
                namespace: meerkat_core::InstructionNamespace::new("app.example").unwrap(),
                key: meerkat_core::InstructionKey::new("primary").unwrap(),
                revision_id: meerkat_core::InstructionRevisionId::new("revision-1").unwrap(),
                content_sha256: meerkat_core::InstructionContentDigest::for_body(body),
            },
            activation_id: meerkat_core::InstructionActivationId::new("activation-1").unwrap(),
            expectation: meerkat_core::InstructionActivationExpectation::Absent,
            supersedes: None,
            body: body.to_string(),
        };
        let apply = router
            .dispatch(make_request(
                "session/activate_instruction",
                serde_json::json!({
                    "session_id": &session_id,
                    "activation": activation,
                }),
            ))
            .await
            .unwrap();
        assert!(apply.error.is_none(), "activation failed: {apply:?}");
        let history = router
            .dispatch(make_request(
                "session/history",
                serde_json::json!({ "session_id": &session_id }),
            ))
            .await
            .unwrap();
        let message_count = result_value(&history)["message_count"].as_u64().unwrap();
        (session_id, message_count)
    }

    async fn rpc_session_count(router: &MethodRouter) -> usize {
        let list_resp = router
            .dispatch(make_request_no_params("session/list"))
            .await
            .unwrap();
        result_value(&list_resp)["sessions"]
            .as_array()
            .unwrap()
            .len()
    }

    /// Assert an RPC response is the typed fork instruction refusal: its own
    /// error code and the typed cause and keys in `data`.
    fn assert_fork_instruction_refused(response: &RpcResponse) {
        let error = response
            .error
            .as_ref()
            .unwrap_or_else(|| panic!("expected a fork instruction refusal, got {response:?}"));
        assert_eq!(error.code, -32037, "the dedicated error code: {error:?}");
        let data = error.data.as_ref().expect("typed refusal data");
        assert_eq!(data["kind"], "fork_instruction_refused");
        assert_eq!(
            data["fork_instruction_refusal"]["cause"]["kind"],
            "policy_refuses"
        );
        assert_eq!(
            data["fork_instruction_refusal"]["keys"],
            serde_json::json!([["app.example", "primary"]])
        );
    }

    /// `session/fork_at` without `inherited_instructions` on a session with an
    /// applied activation is refused before commit: the child could never be
    /// activated (it is not materialized), so committing it would leave a
    /// session whose first turn must fail.
    #[tokio::test]
    async fn session_fork_at_refuses_an_activated_source_by_default() {
        let (router, _notif_rx) = test_router().await;
        let (session_id, message_count) = activated_rpc_session(&router).await;
        let sessions_before = rpc_session_count(&router).await;

        let fork_resp = router
            .dispatch(make_request(
                "session/fork_at",
                serde_json::json!({
                    "session_id": &session_id,
                    "message_index": message_count,
                    "running_behavior": "reject"
                }),
            ))
            .await
            .unwrap();
        assert_fork_instruction_refused(&fork_resp);
        assert_eq!(rpc_session_count(&router).await, sessions_before);
    }

    /// `session/fork_replace` refuses the same way (its own code path).
    #[tokio::test]
    async fn session_fork_replace_refuses_an_activated_source_by_default() {
        let (router, _notif_rx) = test_router().await;
        let (session_id, _message_count) = activated_rpc_session(&router).await;
        // A turn after the activation, so the replaced message follows the
        // activation row and the fork's prefix carries it.
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
        assert!(
            turn_resp.error.is_none(),
            "the source runs a turn after the activation: {turn_resp:?}"
        );
        let history = router
            .dispatch(make_request(
                "session/history",
                serde_json::json!({ "session_id": &session_id }),
            ))
            .await
            .unwrap();
        let user_index = result_value(&history)["messages"]
            .as_array()
            .unwrap()
            .iter()
            .rposition(|message| message["role"] == "user")
            .expect("a user message to replace");
        let sessions_before = rpc_session_count(&router).await;

        let fork_resp = router
            .dispatch(make_request(
                "session/fork_replace",
                serde_json::json!({
                    "session_id": &session_id,
                    "message_index": user_index,
                    "replacement": {
                        "type": "message",
                        "message": {"role": "user", "content": "A replaced question"}
                    },
                    "running_behavior": "reject"
                }),
            ))
            .await
            .unwrap();
        assert_fork_instruction_refused(&fork_resp);
        assert_eq!(rpc_session_count(&router).await, sessions_before);
    }

    /// With `inherited_instructions: reactivate_inherited` the fork commits a
    /// child that carries its own activation of the inherited key, and the
    /// child runs its first turn.
    #[tokio::test]
    async fn session_fork_at_reactivates_when_asked() {
        let (router, _notif_rx) = test_router().await;
        let (session_id, message_count) = activated_rpc_session(&router).await;

        let fork_resp = router
            .dispatch(make_request(
                "session/fork_at",
                serde_json::json!({
                    "session_id": &session_id,
                    "message_index": message_count,
                    "running_behavior": "reject",
                    "inherited_instructions": "reactivate_inherited"
                }),
            ))
            .await
            .unwrap();
        assert!(fork_resp.error.is_none(), "the fork commits: {fork_resp:?}");
        let forked = result_value(&fork_resp);
        assert_eq!(forked["reactivated_instruction_count"], 1);
        assert_eq!(forked["message_count"], message_count + 1);
        let forked_id = forked["session_id"].as_str().unwrap().to_string();

        let history = router
            .dispatch(make_request(
                "session/history",
                serde_json::json!({ "session_id": &forked_id }),
            ))
            .await
            .unwrap();
        let rows = result_value(&history)["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|message| message.get("instruction_activation").is_some())
            .cloned()
            .collect::<Vec<_>>();
        let own = rows
            .iter()
            .filter(|row| row["instruction_activation"]["origin_session_id"] == forked_id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(own.len(), 1, "the child's own activation: {rows:?}");
        assert_eq!(
            own[0]["instruction_activation"]["supersedes"],
            "activation-1"
        );

        // The child activates the inherited key itself, so the turn-time
        // activation gate admits its first turn.
        let turn_resp = router
            .dispatch(make_request(
                "turn/start",
                serde_json::json!({
                    "session_id": &forked_id,
                    "prompt": "Follow up on the branch",
                }),
            ))
            .await
            .unwrap();
        assert!(
            turn_resp.error.is_none(),
            "the re-activated child runs its first turn: {turn_resp:?}"
        );
    }

    /// Control: a session without activation rows forks with the field absent,
    /// as before, and its child carries no activation rows.
    #[tokio::test]
    async fn session_fork_at_without_activation_rows_forks_as_before() {
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
        assert!(fork_resp.error.is_none(), "the fork commits: {fork_resp:?}");
        let forked = result_value(&fork_resp);
        assert!(
            forked.get("reactivated_instruction_count").is_none(),
            "no re-activation rows: {forked}"
        );
        assert_eq!(forked["message_count"], 2);
    }

    /// the optional caller idempotency key on `session/external_event`
    /// and `mob/member_send`. Every row drives the wire (JSON params), so it
    /// compiles against a contract that has no such field yet.
    mod caller_idempotency_key {
        use super::*;

        #[cfg(feature = "mob")]
        const STATUS_WAIT: Duration = Duration::from_secs(60);

        /// A router over in-memory stores. With `realm`, construction arms
        /// the runtime's delivery owner, so committed continuations drain;
        /// without it nothing drains them until the test arms the owner.
        async fn keyed_router(
            realm: bool,
        ) -> (MethodRouter, Arc<SessionRuntime>, tempfile::TempDir) {
            let temp = tempfile::tempdir().unwrap();
            let factory = AgentFactory::new(temp.path().join("sessions"));
            let config = anthropic_test_config();
            let store: Arc<dyn meerkat::SessionStore> = Arc::new(meerkat::MemoryStore::new());
            let runtime = SessionRuntime::new(
                factory,
                config.clone(),
                10,
                runtime_backed_persistence(store),
                NotificationSink::noop(),
            )
            .expect("compose the RPC session runtime");
            runtime.set_default_llm_client(Some(Arc::new(MockLlmClient::anthropic())));
            let config_store: Arc<dyn ConfigStore> =
                Arc::new(MemoryConfigStore::new(config, meerkat_models::canonical()));
            runtime.set_config_runtime(Arc::new(ConfigRuntime::new(
                Arc::clone(&config_store),
                temp.path().join("config_state.json"),
            )));
            if realm {
                runtime.set_realm_context(
                    Some(meerkat_core::connection::RealmId::global()),
                    None,
                    None,
                );
            }
            let runtime = Arc::new(runtime);
            let router =
                MethodRouter::new(Arc::clone(&runtime), config_store, NotificationSink::noop())
                    .expect("construct runtime authority");
            (router, runtime, temp)
        }

        async fn create_session(router: &MethodRouter) -> String {
            let resp = router
                .dispatch(make_request(
                    "session/create",
                    serde_json::json!({ "prompt": "Hello" }),
                ))
                .await
                .unwrap();
            result_value(&resp)["session_id"]
                .as_str()
                .expect("session id")
                .to_string()
        }

        fn external_event_params(
            session_id: &str,
            key: Option<&str>,
            payload: serde_json::Value,
        ) -> serde_json::Value {
            let mut params = serde_json::json!({
                "session_id": session_id,
                "kind": "generic_json",
                "event_type": "inbox.item",
                "payload": payload,
            });
            if let Some(key) = key {
                params["idempotency_key"] = serde_json::json!(key);
            }
            params
        }

        async fn external_event(
            router: &MethodRouter,
            session_id: &str,
            key: Option<&str>,
            payload: serde_json::Value,
        ) -> RpcResponse {
            router
                .dispatch(make_request(
                    "session/external_event",
                    external_event_params(session_id, key, payload),
                ))
                .await
                .unwrap()
        }

        fn error_data(resp: &RpcResponse) -> serde_json::Value {
            resp.error
                .as_ref()
                .unwrap_or_else(|| panic!("expected an error response, got {resp:?}"))
                .data
                .clone()
                .unwrap_or(serde_json::Value::Null)
        }

        /// the same key with the same payload names the first admission
        /// and admits nothing new.
        #[tokio::test]
        async fn external_event_same_key_same_payload_returns_the_first_admission() {
            use meerkat_runtime::SessionServiceRuntimeExt as _;
            let (router, runtime, _temp) = keyed_router(false).await;
            let session_id = create_session(&router).await;
            let payload = serde_json::json!({ "item": "msg-1", "body": "hello" });
            let first = result_value(
                &external_event(&router, &session_id, Some("inbox:msg-1"), payload.clone()).await,
            );
            assert_eq!(first["outcome_type"], "accepted", "{first}");
            let first_id = first["input_id"]
                .as_str()
                .expect("first input id")
                .to_string();
            let replay = result_value(
                &external_event(&router, &session_id, Some("inbox:msg-1"), payload).await,
            );
            assert_eq!(
                replay["outcome_type"], "deduplicated",
                "a retry under the same key admits nothing new: {replay}"
            );
            assert_eq!(
                replay["existing_id"].as_str(),
                Some(first_id.as_str()),
                "the retry names the first admission: {replay}"
            );
            // Indexed in the caller namespace the REST surface shares.
            let parsed = meerkat_core::SessionId::parse(&session_id).expect("session id");
            let row = runtime
                .runtime_adapter()
                .input_state_by_idempotency_key(&parsed, "caller:inbox:msg-1")
                .await
                .expect("lookup")
                .expect("the admission holds caller:<key>");
            assert_eq!(row.state.input_id.to_string(), first_id);
        }

        /// the same key with another payload is the typed
        /// INPUT_IDEMPOTENCY_CONFLICT naming the first admission, and the
        /// first admission stands.
        #[tokio::test]
        async fn external_event_same_key_other_payload_is_input_idempotency_conflict() {
            let (router, _runtime, _temp) = keyed_router(false).await;
            let session_id = create_session(&router).await;
            let first = result_value(
                &external_event(
                    &router,
                    &session_id,
                    Some("inbox:msg-2"),
                    serde_json::json!({ "body": "original" }),
                )
                .await,
            );
            assert_eq!(first["outcome_type"], "accepted", "{first}");
            let first_id = first["input_id"]
                .as_str()
                .expect("first input id")
                .to_string();

            let conflict = external_event(
                &router,
                &session_id,
                Some("inbox:msg-2"),
                serde_json::json!({ "body": "changed" }),
            )
            .await;
            assert_eq!(
                conflict.error.as_ref().map(|error| error.code),
                Some(crate::error::INVALID_PARAMS),
                "a changed payload under a used key is refused: {conflict:?}"
            );
            let data = error_data(&conflict);
            assert_eq!(data["code"], "INPUT_IDEMPOTENCY_CONFLICT", "{data}");
            assert_eq!(
                data["existing_id"].as_str(),
                Some(first_id.as_str()),
                "{data}"
            );

            let replay = result_value(
                &external_event(
                    &router,
                    &session_id,
                    Some("inbox:msg-2"),
                    serde_json::json!({ "body": "original" }),
                )
                .await,
            );
            assert_eq!(replay["outcome_type"], "deduplicated", "{replay}");
            assert_eq!(replay["existing_id"].as_str(), Some(first_id.as_str()));
        }

        /// the key outlives the input: after the event was consumed by a
        /// turn, a retry still names the first admission.
        #[tokio::test]
        async fn external_event_key_replays_after_the_input_completed() {
            let (router, _runtime, _temp) = keyed_router(false).await;
            let session_id = create_session(&router).await;
            let payload = serde_json::json!({ "body": "consumed by the next turn" });
            let first = result_value(
                &external_event(&router, &session_id, Some("inbox:msg-3"), payload.clone()).await,
            );
            assert_eq!(first["outcome_type"], "accepted", "{first}");
            let first_id = first["input_id"]
                .as_str()
                .expect("first input id")
                .to_string();

            let turn = router
                .dispatch(make_request(
                    "turn/start",
                    serde_json::json!({ "session_id": session_id, "prompt": "continue" }),
                ))
                .await
                .unwrap();
            assert!(turn.error.is_none(), "the next turn runs: {turn:?}");

            let replay = result_value(
                &external_event(&router, &session_id, Some("inbox:msg-3"), payload).await,
            );
            assert_eq!(
                replay["outcome_type"], "deduplicated",
                "the key is not released when its input completes: {replay}"
            );
            assert_eq!(replay["existing_id"].as_str(), Some(first_id.as_str()));
        }

        /// Guard (a single key space for all sessions would fail it): the key is
        /// scoped to its session.
        #[tokio::test]
        async fn external_event_key_is_scoped_to_one_session() {
            let (router, _runtime, _temp) = keyed_router(false).await;
            let first_session = create_session(&router).await;
            let second_session = create_session(&router).await;
            let payload = serde_json::json!({ "body": "same key, two sessions" });
            for session_id in [&first_session, &second_session] {
                let admitted = result_value(
                    &external_event(&router, session_id, Some("inbox:shared"), payload.clone())
                        .await,
                );
                assert_eq!(
                    admitted["outcome_type"], "accepted",
                    "each session admits its own keyed event: {admitted}"
                );
            }
        }

        /// an invalid key is refused before
        /// anything is admitted, by the one canonical-key rule.
        #[tokio::test]
        async fn external_event_invalid_key_is_refused_before_admission() {
            let (router, _runtime, _temp) = keyed_router(false).await;
            let session_id = create_session(&router).await;
            let too_long = "k".repeat(257);
            for key in ["", " padded", "bell\u{7}", too_long.as_str()] {
                let refused = external_event(
                    &router,
                    &session_id,
                    Some(key),
                    serde_json::json!({ "body": "never admitted" }),
                )
                .await;
                let error = refused
                    .error
                    .as_ref()
                    .unwrap_or_else(|| panic!("key {key:?} must be refused: {refused:?}"));
                assert_eq!(error.code, crate::error::INVALID_PARAMS, "{error:?}");
                assert!(
                    error.message.contains("idempotency_key must be canonical"),
                    "the one canonical-key rule refuses it: {error:?}"
                );
            }
            let admitted = result_value(
                &external_event(
                    &router,
                    &session_id,
                    Some("inbox:valid"),
                    serde_json::json!({ "body": "admitted" }),
                )
                .await,
            );
            assert_eq!(admitted["outcome_type"], "accepted", "{admitted}");
        }

        /// payload equality is the event identity's: object key order
        /// is not content (serde_json without preserve_order sorts maps), so
        /// a retry whose payload keys arrive reordered replays.
        #[tokio::test]
        async fn external_event_reordered_payload_keys_replay() {
            let (router, _runtime, _temp) = keyed_router(false).await;
            let session_id = create_session(&router).await;
            let raw_request = |payload_json: &str| {
                RpcRequest {
                jsonrpc: "2.0".to_string(),
                id: Some(RpcId::Num(1)),
                method: "session/external_event".to_string(),
                params: Some(
                    RawValue::from_string(format!(
                        r#"{{"session_id":"{session_id}","kind":"generic_json","event_type":"inbox.item","idempotency_key":"inbox:order","payload":{payload_json}}}"#
                    ))
                    .unwrap(),
                ),
            }
            };
            let first = result_value(
                &router
                    .dispatch(raw_request(r#"{"a":1,"b":{"c":2,"d":[3,4]}}"#))
                    .await
                    .unwrap(),
            );
            assert_eq!(first["outcome_type"], "accepted", "{first}");
            let replay = result_value(
                &router
                    .dispatch(raw_request(r#"{"b":{"d":[3,4],"c":2},"a":1}"#))
                    .await
                    .unwrap(),
            );
            assert_eq!(
                replay["outcome_type"], "deduplicated",
                "reordered object keys are the same payload: {replay}"
            );
            assert_eq!(replay["existing_id"], first["input_id"]);
        }

        #[cfg(feature = "mob")]
        async fn create_mob_with_worker(router: &MethodRouter, mob: &str) -> String {
            let create_resp = router
                .dispatch(make_request(
                    "mob/create",
                    serde_json::json!({
                        "definition": {
                            "id": mob,
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
            assert_eq!(result_value(&spawn_resp)["agent_identity"], "worker-1");
            mob_id
        }

        #[cfg(feature = "mob")]
        async fn member_send(
            router: &MethodRouter,
            mob_id: &str,
            content: &str,
            key: Option<&str>,
        ) -> RpcResponse {
            let mut params = serde_json::json!({
                "mob_id": mob_id,
                "agent_identity": "worker-1",
                "content": content,
            });
            if let Some(key) = key {
                params["idempotency_key"] = serde_json::json!(key);
            }
            router
                .dispatch(make_request("mob/member_send", params))
                .await
                .unwrap()
        }

        #[cfg(feature = "mob")]
        fn delivery(sent: &serde_json::Value) -> (String, u64, String) {
            let continuation = &sent["continuation"];
            (
                continuation["delivery_id"]
                    .as_str()
                    .unwrap_or_else(|| panic!("a keyed send answers its receipt: {sent}"))
                    .to_string(),
                continuation["committed_at_ms"]
                    .as_u64()
                    .unwrap_or_else(|| panic!("a keyed send answers its commit time: {sent}")),
                continuation["status"]["state"]
                    .as_str()
                    .unwrap_or_else(|| panic!("a keyed send answers its status: {sent}"))
                    .to_string(),
            )
        }

        /// Replay `key` until its status is `admitted`, returning that answer.
        #[cfg(feature = "mob")]
        async fn replay_until_admitted(
            router: &MethodRouter,
            mob_id: &str,
            content: &str,
            key: &str,
        ) -> serde_json::Value {
            tokio::time::timeout(STATUS_WAIT, async {
                loop {
                    let replay =
                        result_value(&member_send(router, mob_id, content, Some(key)).await);
                    if replay["continuation"]["status"]["state"] == "admitted" {
                        return replay;
                    }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            })
            .await
            .expect("the keyed send is admitted")
        }

        /// the first answer is a receipt with a pending or admitted
        /// status; on replay delivery_id and committed_at_ms are identical,
        /// the status may advance, and nothing new is admitted.
        #[cfg(feature = "mob")]
        #[tokio::test]
        async fn member_send_same_key_same_content_returns_the_first_receipt() {
            let (router, _runtime, _temp) = keyed_router(true).await;
            let mob_id = create_mob_with_worker(&router, "mob-keyed-send").await;
            let content = "Ingest item 7.";
            let first = result_value(&member_send(&router, &mob_id, content, Some("item-7")).await);
            let (delivery_id, committed_at_ms, state) = delivery(&first);
            assert!(
                state == "pending" || state == "admitted",
                "the first answer reports pending or admitted: {first}"
            );
            let admitted = replay_until_admitted(&router, &mob_id, content, "item-7").await;
            let (replayed_id, replayed_at, _) = delivery(&admitted);
            assert_eq!(
                replayed_id, delivery_id,
                "a replay names the first delivery"
            );
            assert_eq!(
                replayed_at, committed_at_ms,
                "a replay keeps the first commit time"
            );
            let input_id = admitted["continuation"]["status"]["input_id"].clone();
            assert!(input_id.is_string(), "admitted names its input: {admitted}");
            let session_id = admitted["continuation"]["status"]["session_id"].clone();
            assert!(
                session_id.is_string(),
                "admitted names its session: {admitted}"
            );
            let again = result_value(&member_send(&router, &mob_id, content, Some("item-7")).await);
            assert_eq!(
                again["continuation"]["status"]["input_id"], input_id,
                "one admission, however often the key is replayed: {again}"
            );
            // Admitted means queued in that session's ledger; the host
            // follows it with session/input_status.
            let followed = result_value(
                &router
                    .dispatch(make_request(
                        "session/input_status",
                        serde_json::json!({
                            "session_id": session_id,
                            "selector": { "by": "input_id", "value": input_id },
                        }),
                    ))
                    .await
                    .unwrap(),
            );
            assert_eq!(
                followed["state"]["input_id"], input_id,
                "the admitted pair names a row session/input_status reads: {followed}"
            );
        }

        /// the first answer reports pending or admitted, and an admitted
        /// answer names its session and input.
        #[cfg(feature = "mob")]
        #[tokio::test]
        async fn member_send_first_answer_reports_pending_or_admitted() {
            let (router, _runtime, _temp) = keyed_router(true).await;
            let mob_id = create_mob_with_worker(&router, "mob-keyed-first").await;
            let first = result_value(
                &member_send(&router, &mob_id, "Ingest item 6.", Some("item-6")).await,
            );
            let (_, _, state) = delivery(&first);
            match state.as_str() {
                "pending" => {}
                "admitted" => {
                    let status = &first["continuation"]["status"];
                    assert!(status["session_id"].is_string(), "{first}");
                    assert!(status["input_id"].is_string(), "{first}");
                }
                other => panic!("the first answer is pending or admitted, got {other}: {first}"),
            }
        }

        /// other content under a used key is the typed conflict, naming
        /// the first delivery.
        #[cfg(feature = "mob")]
        #[tokio::test]
        async fn member_send_same_key_other_content_is_input_idempotency_conflict() {
            let (router, _runtime, _temp) = keyed_router(true).await;
            let mob_id = create_mob_with_worker(&router, "mob-keyed-conflict").await;
            let first =
                result_value(&member_send(&router, &mob_id, "original", Some("item-8")).await);
            let (delivery_id, _, _) = delivery(&first);
            let conflict = member_send(&router, &mob_id, "changed", Some("item-8")).await;
            assert_eq!(
                conflict.error.as_ref().map(|error| error.code),
                Some(crate::error::INVALID_PARAMS),
                "{conflict:?}"
            );
            let data = error_data(&conflict);
            assert_eq!(data["kind"], "input_idempotency_conflict", "{data}");
            assert_eq!(
                data["delivery_id"].as_str(),
                Some(delivery_id.as_str()),
                "{data}"
            );
        }

        /// a respawn does not reopen the key: the replay names the first
        /// delivery and its admission, and nothing new is committed.
        #[cfg(feature = "mob")]
        #[tokio::test]
        async fn member_send_key_survives_a_member_respawn() {
            let (router, _runtime, _temp) = keyed_router(true).await;
            let mob_id = create_mob_with_worker(&router, "mob-keyed-respawn").await;
            let content = "Ingest item 9.";
            let first = result_value(&member_send(&router, &mob_id, content, Some("item-9")).await);
            let (delivery_id, committed_at_ms, _) = delivery(&first);
            let admitted = replay_until_admitted(&router, &mob_id, content, "item-9").await;
            let input_id = admitted["continuation"]["status"]["input_id"].clone();

            let respawn = router
                .dispatch(make_request(
                    "mob/respawn",
                    serde_json::json!({ "mob_id": mob_id, "agent_identity": "worker-1" }),
                ))
                .await
                .unwrap();
            assert!(respawn.error.is_none(), "respawn: {respawn:?}");

            let replay =
                result_value(&member_send(&router, &mob_id, content, Some("item-9")).await);
            let (replayed_id, replayed_at, state) = delivery(&replay);
            assert_eq!(replayed_id, delivery_id);
            assert_eq!(replayed_at, committed_at_ms);
            assert_eq!(state, "admitted", "the first admission stands: {replay}");
            assert_eq!(replay["continuation"]["status"]["input_id"], input_id);
        }

        /// a keyed send committed but not drained when its incarnation
        /// is replaced answers `stranded`, typed, on every replay; the same
        /// content under a new key is then delivered once to the new
        /// incarnation.
        #[cfg(feature = "mob")]
        #[tokio::test]
        async fn member_send_key_stranded_by_a_respawn_answers_stranded_then_a_new_key_delivers_once()
         {
            let (router, runtime, _temp) = keyed_router(false).await;
            let mob_id = create_mob_with_worker(&router, "mob-keyed-strand").await;
            let content = "Ingest item 10.";
            let first =
                result_value(&member_send(&router, &mob_id, content, Some("item-10")).await);
            let (delivery_id, _, state) = delivery(&first);
            assert_eq!(state, "pending", "no delivery owner drains it yet: {first}");

            let respawn = router
                .dispatch(make_request(
                    "mob/respawn",
                    serde_json::json!({ "mob_id": mob_id, "agent_identity": "worker-1" }),
                ))
                .await
                .unwrap();
            assert!(respawn.error.is_none(), "respawn: {respawn:?}");

            let stranded =
                result_value(&member_send(&router, &mob_id, content, Some("item-10")).await);
            let (stranded_id, _, state) = delivery(&stranded);
            assert_eq!(stranded_id, delivery_id);
            assert_eq!(
                state, "stranded",
                "never admitted, incarnation replaced: {stranded}"
            );

            runtime.set_realm_context(
                Some(meerkat_core::connection::RealmId::global()),
                None,
                None,
            );
            runtime.arm_runtime_delivery_owner();
            let resent =
                result_value(&member_send(&router, &mob_id, content, Some("item-10b")).await);
            let (resent_id, _, _) = delivery(&resent);
            assert_ne!(resent_id, delivery_id, "a new key is a new delivery");
            replay_until_admitted(&router, &mob_id, content, "item-10b").await;
            let still =
                result_value(&member_send(&router, &mob_id, content, Some("item-10")).await);
            assert_eq!(
                still["continuation"]["status"]["state"], "stranded",
                "a stranded delivery stays stranded: {still}"
            );
        }

        /// The drain's restart path: a keyed send committed while no
        /// delivery owner runs is admitted exactly once by the owner that
        /// arms later; replays name that one admission.
        #[cfg(feature = "mob")]
        #[tokio::test]
        async fn member_send_key_committed_before_the_drain_is_admitted_once_when_an_owner_arms() {
            let (router, runtime, _temp) = keyed_router(false).await;
            let mob_id = create_mob_with_worker(&router, "mob-keyed-arm").await;
            let content = "Ingest item 11.";
            let first =
                result_value(&member_send(&router, &mob_id, content, Some("item-11")).await);
            let (delivery_id, committed_at_ms, state) = delivery(&first);
            assert_eq!(state, "pending", "{first}");
            runtime.set_realm_context(
                Some(meerkat_core::connection::RealmId::global()),
                None,
                None,
            );
            runtime.arm_runtime_delivery_owner();
            let admitted = replay_until_admitted(&router, &mob_id, content, "item-11").await;
            let (replayed_id, replayed_at, _) = delivery(&admitted);
            assert_eq!(replayed_id, delivery_id);
            assert_eq!(replayed_at, committed_at_ms);
            let input_id = admitted["continuation"]["status"]["input_id"].clone();
            let again =
                result_value(&member_send(&router, &mob_id, content, Some("item-11")).await);
            assert_eq!(again["continuation"]["status"]["input_id"], input_id);
        }

        /// render_metadata has no slot on the keyed door and is refused
        /// with a key, before anything commits.
        #[cfg(feature = "mob")]
        #[tokio::test]
        async fn member_send_key_with_render_metadata_is_refused() {
            let (router, _runtime, _temp) = keyed_router(true).await;
            let mob_id = create_mob_with_worker(&router, "mob-keyed-render").await;
            let refused = router
                .dispatch(make_request(
                    "mob/member_send",
                    serde_json::json!({
                        "mob_id": mob_id,
                        "agent_identity": "worker-1",
                        "content": "styled",
                        "render_metadata": { "class": "system_notice" },
                        "idempotency_key": "item-12",
                    }),
                ))
                .await
                .unwrap();
            let error = refused.error.as_ref().expect("refused");
            assert_eq!(error.code, crate::error::INVALID_PARAMS, "{error:?}");
            assert!(
                error
                    .message
                    .contains("render_metadata does not combine with idempotency_key"),
                "{error:?}"
            );
            // The refused call committed nothing: the key is still free.
            let accepted =
                result_value(&member_send(&router, &mob_id, "styled", Some("item-12")).await);
            assert!(
                accepted["continuation"]["delivery_id"].is_string(),
                "the key commits its first delivery now: {accepted}"
            );
        }

        /// Without the caller namespace this row would fail: a caller key never
        /// meets another continuation producer's key for the same member.
        #[cfg(feature = "mob")]
        #[tokio::test]
        async fn member_send_caller_key_never_meets_a_host_continuation_key() {
            let (router, _runtime, _temp) = keyed_router(true).await;
            let mob_id = create_mob_with_worker(&router, "mob-keyed-namespace").await;
            let service = router
                .mob_state
                .continuation_service()
                .expect("the RPC mob state binds its continuation service");
            let owner = meerkat::ContinuationOwner::Member {
                mob_id: mob_id.clone(),
                identity: "worker-1".to_string(),
            };
            service
                .submit(
                    &owner,
                    meerkat::ContinuationDelivery {
                        key: meerkat::ContinuationKey::new("shared-key").expect("key"),
                        result: meerkat::ContinuationResultRef {
                            producer: meerkat::ContinuationProducer::Host {
                                namespace: "tasks".into(),
                            },
                            producer_id: "op-1".into(),
                            result_digest: "sha256:result".into(),
                            summary: None,
                        },
                        body: "a host result".into(),
                        handling: meerkat::ContinuationHandling::Queue,
                    },
                    1,
                )
                .await
                .expect("a host continuation commits");
            let sent = result_value(
                &member_send(&router, &mob_id, "caller content", Some("shared-key")).await,
            );
            assert!(
                sent["continuation"]["delivery_id"].is_string(),
                "the caller key commits its own delivery: {sent}"
            );
        }

        /// an invalid key is refused by the one rule.
        #[cfg(feature = "mob")]
        #[tokio::test]
        async fn member_send_invalid_key_is_refused_before_commit() {
            let (router, _runtime, _temp) = keyed_router(true).await;
            let mob_id = create_mob_with_worker(&router, "mob-keyed-invalid").await;
            let too_long = "k".repeat(257);
            for key in ["", " padded", "bell\u{7}", too_long.as_str()] {
                let refused = member_send(&router, &mob_id, "never committed", Some(key)).await;
                let error = refused
                    .error
                    .as_ref()
                    .unwrap_or_else(|| panic!("key {key:?} must be refused: {refused:?}"));
                assert_eq!(error.code, crate::error::INVALID_PARAMS, "{error:?}");
                assert!(
                    error.message.contains("idempotency_key must be canonical"),
                    "the one canonical-key rule refuses it: {error:?}"
                );
            }
        }

        /// A first keyed send to a member the mob does not hold is refused,
        /// typed, before anything commits: the key binds nothing for that
        /// identity.
        #[cfg(feature = "mob")]
        #[tokio::test]
        async fn member_send_key_to_an_unknown_member_is_refused_before_commit() {
            let (router, _runtime, _temp) = keyed_router(true).await;
            let mob_id = create_mob_with_worker(&router, "mob-keyed-unknown").await;
            let refused = router
                .dispatch(make_request(
                    "mob/member_send",
                    serde_json::json!({
                        "mob_id": mob_id,
                        "agent_identity": "ghost-1",
                        "content": "never committed",
                        "idempotency_key": "item-17",
                    }),
                ))
                .await
                .unwrap();
            let error = refused
                .error
                .as_ref()
                .expect("an unknown member is refused");
            assert_ne!(error.code, crate::error::INTERNAL_ERROR, "typed: {error:?}");
            let service = router
                .mob_state
                .continuation_service()
                .expect("the RPC mob state binds its continuation service");
            let status = service
                .continuation_status(
                    &meerkat::ContinuationOwner::Member {
                        mob_id: mob_id.clone(),
                        identity: "ghost-1".to_string(),
                    },
                    &meerkat::ContinuationKey::new(format!(
                        "{}item-17",
                        meerkat_mob_mcp::MEMBER_SEND_CONTINUATION_KEY_PREFIX
                    ))
                    .expect("key"),
                )
                .await
                .expect("status");
            assert_eq!(
                status,
                meerkat::ContinuationStatus::NotCommitted,
                "nothing was committed"
            );
        }
    }
}
